#!/usr/bin/env python3
"""Send synthetic data only to a disposable Enterprise local v1 receiver."""

import argparse
import base64
import hashlib
import ipaddress
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid


CHUNK_SIZE = 1 << 20
MAX_FIXTURE_BYTES = 512 << 20
PREFIX = "/v1/enterprise"
KEY = Path(__file__).resolve().parent.parent / "fixtures/enterprise-local/fixture-ed25519.pem"


class SmokeError(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def json_bytes(value):
    return json.dumps(value, separators=(",", ":")).encode("utf-8")


def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=")


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def new_id():
    return str(uuid.uuid4())


def make_proof(openssl, user, session, event, scopes, manifest):
    now = int(time.time())
    header = {"alg": "EdDSA", "typ": "aurora-upload-local+jwt", "kid": "fixture-ed25519"}
    claims = {
        "v": 1,
        "iss": "https://issuer.example.test/aurora",
        "aud": "aurora-company-sync-local",
        "org_id": "fixture-org",
        "receiver_id": "fixture-receiver",
        "user_id": user,
        "session_id": session,
        "event_id": event,
        "manifest_sha256": sha256(manifest),
        "scopes": scopes,
        "jti": new_id(),
        "iat": now,
        "nbf": now,
        "exp": now + 300,
    }
    encoded = b64url(json_bytes(header)) + b"." + b64url(json_bytes(claims))
    # Ed25519 signing in OpenSSL 3.0 needs a seekable input of known length.
    with tempfile.TemporaryDirectory() as temp:
        message = Path(temp) / "signing-input"
        message.write_bytes(encoded)
        signature = subprocess.run(
            [openssl, "pkeyutl", "-sign", "-rawin", "-inkey", str(KEY), "-in", str(message)],
            capture_output=True,
            timeout=15,
            check=False,
        )
    if signature.returncode or len(signature.stdout) != 64:
        raise SmokeError("fixture signing failed; OpenSSL 3+ with Ed25519 is required")
    return (encoded + b"." + b64url(signature.stdout)).decode("ascii")


def wav_chunks(size):
    """Generate deterministic mono 8-bit PCM silence, at most 1 MiB per chunk."""
    data_size = size - 44
    rate = 8000
    header = struct.pack(
        "<4sI4s4sIHHIIHH4sI",
        b"RIFF", size - 8, b"WAVE", b"fmt ", 16, 1, 1, rate, rate, 1, 8, b"data", data_size,
    )
    first = min(size, CHUNK_SIZE)
    silence = b"\x80" * CHUNK_SIZE
    yield header + silence[:first - len(header)]
    for offset in range(first, size, CHUNK_SIZE):
        yield silence[:min(size - offset, CHUNK_SIZE)]


def make_manifest(session, event, revision, **fields):
    return json_bytes(dict(version=1, session_id=session, event_id=event, revision=revision, **fields))


class Receiver:
    def __init__(self, base):
        self.base = base + PREFIX
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def send(self, step, method, path, proof, *statuses, body=None, headers=None):
        request = urllib.request.Request(
            self.base + path,
            data=body,
            headers={"Authorization": "Bearer " + proof, **(headers or {})},
            method=method,
        )
        try:
            with self.opener.open(request, timeout=15) as response:
                if response.status not in statuses:
                    raise SmokeError(f"{step}: unexpected HTTP {response.status}")
                if response.status == 204:
                    return None
                raw = response.read(4097)
        except urllib.error.HTTPError as error:
            error.close()
            if error.code in statuses:
                return None
            raise SmokeError(f"{step}: unexpected HTTP {error.code}") from None
        except (urllib.error.URLError, OSError):
            raise SmokeError(f"{step}: connection failed") from None
        if len(raw) > 4096:
            raise SmokeError(f"{step}: oversized receipt")
        try:
            return json.loads(raw)
        except (UnicodeError, ValueError):
            raise SmokeError(f"{step}: invalid receipt JSON") from None

    def manifest(self, step, path, proof, status, raw):
        return self.send(step, "POST", path, proof, status, body=raw,
                         headers={"Content-Type": "application/json"})

    def chunk(self, path, proof, index, raw):
        return self.send(f"audio chunk {index}", "PUT", f"{path}/chunks/{index}", proof, 200,
                         body=raw, headers={"Content-Type": "application/octet-stream",
                                            "X-Aurora-Chunk-Sha256": sha256(raw)})


def receipt(raw, session, event, revision, state, *, artifact=None, offset=0):
    expected = dict(session_id=session, event_id=event, revision=revision, state=state,
                    artifact_id=artifact, received_bytes=offset)
    if not isinstance(raw, dict) or any(k not in raw or raw[k] != v for k, v in expected.items()):
        raise SmokeError("receipt did not match the submitted fixture")
    expires = raw.get("expires_at")
    if type(expires) is not int or expires <= 0:
        raise SmokeError("missing receipt expiry")
    return expires


def run(receiver, openssl, size):
    employee, other_employee = new_id(), new_id()
    session, event, artifact = new_id(), new_id(), new_id()
    path = f"/events/{event}"
    audio_path = f"{path}/audio/{artifact}"

    check = make_proof(openssl, employee, new_id(), new_id(), ["check"], b"")
    receiver.send("connectivity check", "POST", "/check", check, 204)

    digest = hashlib.sha256()
    for chunk in wav_chunks(size):
        digest.update(chunk)
    raw = make_manifest(session, event, 1,
                        text={"title": "Synthetic Enterprise smoke"},
                        audio=dict(artifact_id=artifact, length=size, sha256=digest.hexdigest(),
                                   media_type="audio/wav"))
    token = make_proof(openssl, employee, session, event, ["manifest", "audio"], raw)
    admitted = receiver.manifest("audio manifest", path, token, 201, raw)
    deadline = receipt(admitted, session, event, 1, "uploading", artifact=artifact)
    if receiver.manifest("exact manifest replay", path, token, 200, raw) != admitted:
        raise SmokeError("duplicate manifest changed the receipt")

    offset = 0
    refresh_at = time.monotonic() + 240
    for index, chunk in enumerate(wav_chunks(size)):
        if index == 1 or time.monotonic() >= refresh_at:
            token = make_proof(openssl, employee, session, event, ["manifest", "audio"], raw)
            refresh_at = time.monotonic() + 240
        committed = receiver.chunk(audio_path, token, index, chunk)
        offset += len(chunk)
        if receipt(committed, session, event, 1, "uploading", artifact=artifact, offset=offset) != deadline:
            raise SmokeError("upload receipt deadline changed")
        if index == 0 and receiver.send("resume status", "GET", path, token, 200) != committed:
            raise SmokeError("resume receipt did not reflect the committed chunk")

    complete = receiver.send("finalize", "POST", path + "/finalize", token, 200)
    receipt(complete, session, event, 1, "complete", artifact=artifact, offset=size)
    if receiver.send("finalize replay", "POST", path + "/finalize", token, 200) != complete:
        raise SmokeError("repeated finalize changed the receipt")
    if receiver.chunk(audio_path, token, 0, next(wav_chunks(size))) != complete:
        raise SmokeError("committed chunk replay changed the completed receipt")

    foreign = make_proof(openssl, other_employee, session, event, ["manifest"], raw)
    receiver.send("cross-employee status", "GET", path, foreign, 404, 409)

    outcome = new_id()
    raw = make_manifest(session, outcome, 2, outcome="won")
    token = make_proof(openssl, employee, session, outcome, ["manifest", "outcome"], raw)
    updated = receiver.manifest("same employee/session outcome revision", f"/events/{outcome}", token, 201, raw)
    receipt(updated, session, outcome, 2, "complete")

    other_session, other_event = new_id(), new_id()
    raw = make_manifest(other_session, other_event, 1, text={"title": "Second synthetic employee"})
    token = make_proof(openssl, other_employee, other_session, other_event, ["manifest"], raw)
    other = receiver.manifest("second employee text manifest", f"/events/{other_event}", token, 201, raw)
    receipt(other, other_session, other_event, 1, "complete")
    return event


def receiver_origin(value):
    try:
        parsed = urllib.parse.urlsplit(value)
        port = parsed.port
        if (parsed.scheme not in ("http", "https") or not parsed.hostname or port == 0
                or parsed.username is not None or parsed.password is not None
                or parsed.path not in ("", "/") or "?" in value or "#" in value):
            raise ValueError
        if parsed.scheme == "http" and not ipaddress.ip_address(parsed.hostname).is_loopback:
            raise ValueError
    except ValueError:
        raise argparse.ArgumentTypeError("use a literal loopback HTTP or private HTTPS origin") from None
    return f"{parsed.scheme}://{parsed.netloc}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", type=receiver_origin, default="http://127.0.0.1:8787",
                        help="receiver origin (no path, query, or credentials)")
    parser.add_argument("--size", type=int, default=CHUNK_SIZE + 18,
                        help="exact synthetic WAV bytes (even, from 46 through 536870912)")
    args = parser.parse_args()
    if not 46 <= args.size <= MAX_FIXTURE_BYTES or args.size % 2:
        parser.error("--size must be even and between 46 and 536870912 bytes")
    openssl = shutil.which("openssl")
    if not openssl:
        parser.error("OpenSSL 3+ with Ed25519 is required")
    try:
        recording = run(Receiver(args.url), openssl, args.size)
    except SmokeError as error:
        parser.exit(1, f"Enterprise local smoke failed: {error}\n")
    except (OSError, subprocess.TimeoutExpired):
        parser.exit(1, "Enterprise local smoke failed while signing the fixture\n")
    print(f"Enterprise local smoke passed: two synthetic employees, {args.size} audio bytes")
    print(f"Synthetic recording event: {recording}")


if __name__ == "__main__":
    main()
