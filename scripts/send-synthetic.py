#!/usr/bin/env python3
"""Send only the bundled synthetic fixture; never reads real meeting data."""
import argparse
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import time
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8787/webhooks/aurora")
    parser.add_argument("--test", action="store_true", help="Connectivity-only; never dispatch")
    parser.add_argument("--event-id", default="evt_00000000-0000-4000-8000-000000000001")
    args = parser.parse_args()
    try:
        target = urllib.parse.urlsplit(args.url)
        _ = target.port
    except ValueError:
        parser.error("Use a valid loopback HTTP or private HTTPS URL")
    if (not target.hostname or target.username is not None or target.password is not None
            or target.fragment or target.scheme not in ("http", "https")
            or (target.scheme == "http" and target.hostname not in ("127.0.0.1", "::1"))):
        parser.error("Use loopback HTTP or private HTTPS without credentials or fragments")
    secret = os.environ.get("AURORA_WEBHOOK_SECRET", "")
    if secret.startswith("whsec_"):
        secret = secret[6:]
    try:
        key = base64.b64decode(secret, validate=True)
    except ValueError:
        parser.error("AURORA_WEBHOOK_SECRET must be base64")
    if len(key) != 32:
        parser.error("AURORA_WEBHOOK_SECRET must decode to 32 bytes")
    event = json.loads((Path(__file__).resolve().parent.parent / "fixtures/meeting.completed.json").read_text())
    event["id"] = args.event_id
    if args.test:
        event["test"] = True
    body = json.dumps(event, separators=(",", ":")).encode()
    stamp = str(int(time.time()))
    sig = base64.b64encode(hmac.new(key, (args.event_id + "." + stamp + ".").encode() + body, hashlib.sha256).digest()).decode()
    headers = {"Content-Type": "application/json", "webhook-id": args.event_id,
               "webhook-timestamp": stamp, "webhook-signature": "v1," + sig}
    token = os.environ.get("AURORA_BEARER_TOKEN")
    if token:
        headers["Authorization"] = "Bearer " + token
    req = urllib.request.Request(args.url, body, headers, method="POST")
    try:
        with urllib.request.build_opener(NoRedirect).open(req, timeout=15) as response:
            print("Synthetic event HTTP", response.status)
    except urllib.error.HTTPError as error:
        print("Synthetic event HTTP", error.code)
        raise SystemExit(1)
    except urllib.error.URLError:
        print("Synthetic delivery failed; check endpoint/connectivity")
        raise SystemExit(1)


if __name__ == "__main__":
    main()
