# Enterprise Company Sync: local v1 contract

**Local integration only.** No Operations-minted credential or native Company
Sync contract was available in the adjacent checkouts. This adapter uses a
non-production issuer, audience and token type. Do not enroll customers or
forward real recordings. Freeze a shared Operations/native contract and test
its actual signed vectors and clients before production. Consumer
`POST /webhooks/aurora` is unchanged.

## Upload proof

Each request sends one `Authorization: Bearer <JWT>` header. The credential is
compact JWS (RFC 7515/7519), using unpadded base64url and Ed25519 (`alg: EdDSA`,
RFC 8037). Sign the ASCII `encoded_header.encoded_payload`; the final
component encodes the 64-byte signature. The protected header contains
exactly `alg`, `typ: aurora-upload-local+jwt`, and `kid`. Select the raw 32-byte
Ed25519 public key by case-sensitive `kid` from the operator's configured map.
No inline keys, URLs, or negotiated algorithms. This is not a WorkOS token.

Claims are exactly `v` (integer `1`), `iss` (`https://issuer.example.test/aurora`),
`aud` (`aurora-company-sync-local`), `org_id`, `receiver_id`, `user_id`,
`session_id`, `event_id`, `manifest_sha256`, `scopes`, `jti`, `iat`, `nbf`,
`exp`. Times are Unix seconds: `iat <= nbf < exp`, `exp - iat <= 300`,
`iat` and `nbf` at most 30 seconds ahead of verifier UTC, and `now < exp`
without grace. Org, receiver and key IDs are 1–128 case-sensitive ASCII
letters, digits, `_` or `-`; user, session, event, artifact and replay IDs
are canonical lowercase, non-nil hyphenated UUIDs. Hashes are exactly 64
lowercase hex digits. The configured org/receiver must exactly match;
`user_id` comes only from the proof.

Scope entries are distinct:

- `manifest`: admit a manifest, read its receipt or finalize.
- `audio`: additionally required to admit or finalize a recording or write a chunk.
- `outcome`: additionally required to admit a sales answer.
- `check`: exclusive, stateless connectivity probe. Its manifest hash is
  SHA-256 of zero bytes. It writes no replay, session or destination data.

The durable `jti` binding covers org, receiver, employee, session, event,
manifest hash, scopes and validity timestamps, independent of key ID. The
same proof may replay authorized operations until expiry; divergent reuse
conflicts. To resume after expiry, obtain proof with a **new** `jti` and the
same upload identifiers/digest after the issuer checks license and receiver
authorization. Previously issued proof remains usable until expiry unless
its pinned key is removed; the receiver does not check employee revocation
online. Do not derive tenant, employee or authority from a device ID or text.

## Requests

All paths start with `/v1/enterprise`. No content encoding, redirects, query
credentials or external downloads. JSON is UTF-8. Manifest hash is SHA-256 of
the **exact POST body**, including whitespace; send identical bytes on retry.
Bodyless operations require an empty body.

| Method/path | Body | Success |
| --- | --- | --- |
| `POST /check` | empty | `204`, no writes |
| `POST /events/{event_id}` | `application/json`, max 1 MiB | `201` admitted, `200` exact duplicate; receipt JSON |
| `GET /events/{event_id}` | empty | `200`, receipt JSON, resume offset |
| `PUT /events/{event_id}/audio/{artifact_id}/chunks/{index}` | `application/octet-stream`, exact chunk bytes; `x-aurora-chunk-sha256` required | `200`, receipt JSON after fsync and commit or matching retry |
| `POST /events/{event_id}/finalize` | empty | `200` complete (including repeat); receipt JSON after durable publication |

The manifest requires `version: 1`, `session_id`, `event_id` and `revision`
(positive signed 32-bit). Include at least one of:

- `text`: an object with optional string `title`, `transcript`, `summary`,
  or `action_items` array of `{text, completed}`.
- `audio`: `artifact_id`, positive byte `length`, `sha256`, and `media_type`
  (`audio/mp4`, `audio/m4a`, `audio/aac`, `audio/mpeg`, `audio/wav`,
  `audio/x-wav`, `audio/ogg`, `audio/flac`, `audio/webm`).
- `outcome`: `won`, `not_won`, `unknown` or `skipped`. Only `won` is a sale.

Unrecognized fields are rejected. A `null` optional field counts as absent
but still contributes raw bytes to the digest. A new revision has a new event
UUID and strictly greater revision for the same employee/session. Finalize or
let the preceding incomplete event expire before admitting a newer revision.
Omission means no update; an outcome-only revision does not erase prior text
or audio. Verified attribution identifies the uploader, not the truth of the
meeting, sale, or participant consent.

Receipts expose `event_id`, `session_id`, `revision`, `state` (`uploading` or
`complete`), nullable `artifact_id`, `received_bytes` and `expires_at` (Unix
seconds). The first incomplete deadline is fixed. Text-only or outcome-only
events complete on admission. Audio reserves its full declared length.

Chunks start at index 0, exactly 1 MiB except the final remainder, in order.
Resume at `received_bytes`. A retry compares exact committed bytes, including
after finalize. Finalize hashes the whole recording with bounded memory and
publishes only a match. Restart may discard bytes beyond the last committed
offset; retransmit from the receipt. If the durable rename succeeded but its
receipt did not commit, repeat finalize with current proof to reconcile it.
Completed content gets a separate fixed deadline; retries never extend it.

Errors are non-PII text: `400` malformed manifest, path, chunk or unexpected
body; `401 unauthorized` missing or invalid proof (including expired, wrong
org/receiver or scope); `404` absent event/artifact or disabled routes;
`408` body timeout; `409` divergent replay/event/chunk, ownership or revision
conflict, missing or reordered chunks; `410` retained expired receipt;
`413` body/artifact limit; `415` wrong content type or any content encoding;
`422` hash mismatch; `429` concurrent Enterprise intake busy; `503` unexpected
storage failure; `507` capacity/headroom, WAL pressure or filesystem full.
Retry transient capacity and timeouts with still-valid/new proof and unchanged
bytes. An Enterprise failure is not a reason to bypass the consumer route.

A total audio hash mismatch does not let the sender overwrite acknowledged
chunks. Diagnose the local recording/manifest mismatch, let the failed upload
expire, then obtain proof for a corrected manifest with a new event ID and
higher revision.

## Trust and local fixtures

Use a separate disposable private data directory and loopback or private
HTTPS. Set `enterprise_local_v1` explicitly. Public keys are configured
offline, never fetched from the request. Verify the SHA-256 fingerprints of
the decoded raw keys out of band. To rotate, apply both pins, allow at least
330 seconds after the last old-key issuance to drain, then remove that key.
Remove a compromised pin immediately. Never put an issuer signing key on an
actual receiver. The fixture key is public test material, not real data trust.

Start with the [credential-free receiver example](../fixtures/enterprise-local/config.json)
and [local smoke instructions](getting-started.md#try-enterprise-company-sync-with-synthetic-data).
The [signed examples](../fixtures/enterprise-local/proof-vectors.json) are for
their declared fixture clock (`at`), not a live receiver clock. The smoke
sender mints fresh synthetic proofs. The example's SHA-256 key fingerprints
are `5d27510ba6c96ad52942a3dfe6a581ba9fa71424b4405b35549a2ad775765b09`
(`fixture-ed25519`) and `6a1d98f5cba6d66a3abed46687b6408a388823e5648dc420e2b15f19a59ee5c0`
(`fixture-rotation`).

`max_artifact_bytes` limits one recording. `max_storage_bytes` and `max_records`
limit this inbox's retained Enterprise reservations, actual payloads, and
tombstones, separately from the consumer inbox. `max_concurrent_requests`
bounds Enterprise bodies and pending work; the listener's connection limit
and minimum free disk floor still apply to both routes. `upload_ttl_seconds`
is the absolute partial-upload deadline; `retention_seconds` is the completed
payload lifetime; `metadata_retention_seconds` sets the additional finite
tombstone horizon. Expiry clears raw text, outcome and audio. Retained receipt
and employee identifiers remain until their final tombstone is removed. The
daemon cleans incrementally and logs released payload counts without IDs.
Offline `expire` performs one bounded Enterprise pass. Neither process erases
external exports or backups.

Use one inbox per organization/receiver. If an old read transaction pins a
large SQLite WAL, close it rather than increasing media budgets; the receiver
pauses Enterprise writes at the shared WAL pressure fence.

Customer-owned SQLite exposes `enterprise_sessions` (verified org, receiver,
session and employee), `enterprise_receipts` (event/revision, manifest and
artifact digest, retention and transfer state), and related internal replay
and accounting tables. Use an OS-authorized read-only connection or the
explicit offline `enterprise-export` command. Only complete, unexpired
receipts are exportable; never treat partial or expired records as sale
updates. Pick the latest completed outcome-bearing revision for a session,
independent of the latest revision containing text or audio. Keep verified
receipt context outside any text/agent-generated fields.

For example, this read-only query returns the latest retained sale answer per
verified employee/session; only a row with `outcome = 'won'` counts as a sale:

```sql
WITH answers AS (
  SELECT e.org_id, e.receiver_id, e.session_id, s.user_id, e.event_id, e.outcome,
         row_number() OVER (
           PARTITION BY e.org_id, e.receiver_id, e.session_id ORDER BY e.revision DESC
         ) AS newest
  FROM enterprise_receipts AS e
  JOIN enterprise_sessions AS s USING (org_id, receiver_id, session_id)
  WHERE e.state = 'complete' AND e.expires_at > unixepoch() AND e.outcome IS NOT NULL
)
SELECT org_id, receiver_id, session_id, user_id, event_id, outcome
FROM answers WHERE newest = 1;
```
