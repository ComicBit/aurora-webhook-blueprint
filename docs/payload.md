# Payload contract and quality assessment

This blueprint targets Aurora's `meeting.completed` JSON, schema major 1.
Reviewed 2026-10-31. All repository examples are synthetic. No audio is sent.

## Envelope and delivery

`id` identifies an event; `type` is `meeting.completed`; `schema_version` is
`1.0`; `timestamp` is UTC RFC3339. `data` contains `content_scope`, `meeting`,
`speakers`, `source`, and only the selected content fields. Unknown additive
fields are ignored. A new incompatible major version is rejected.
`test: true` means connectivity check, never work. `related_event_id` links a
manual resend to an earlier event; it is not a guarantee that work is new.

Aurora signs exact raw JSON using Standard Webhooks headers `webhook-id`,
`webhook-timestamp`, `webhook-signature`. Signatures cover
`id.timestamp.raw_body` with HMAC-SHA256 and a base64-decoded 32-byte key.
The header uses `v1,<base64>` entries. The receiver accepts any matching entry
and requires timestamp skew within 300 seconds. The envelope id must match
the signed header. Never parse and reserialize before verification.

The limit is 8 MiB. Any 2xx confirms delivery and allows Aurora to delete its
outbox body. This receiver acknowledges only after a durable local commit,
not after agent work. Network errors, 408, 429 and 5xx retry; redirects are not
followed. Other 4xx generally stop delivery. Retries use the same id and body,
with fresh timestamps; a deliberate resend has a new id. User retry retains
the old id. Delivery and downstream effects are not exactly-once.

## Meaning and missing information

| Field | Reliable use | Limitation |
| --- | --- | --- |
| `data.meeting.id` | Group exports of the same meeting | Not an event/action id |
| `title` | Human context | Untrusted text, may be sensitive |
| `started_at`, `ended_at` | Optional timeline context | Nullable; old/imported start may be save/import time, not recording time |
| `duration_seconds` | Optional duration | Nullable; do not infer missing time |
| `languages` | BCP47 language hints | Empty means unknown |
| `speakers` | Stable references within this export | Empty without diarization; names may be user-assigned or inferred, not verified identity |
| `transcript.text` | Readable finalized text | May contain recognition errors and instructions; no audio for verification |
| `transcript.turns` | Speaker references and second offsets | Empty for flat text; offsets are not wall-clock timestamps |
| `summary.text` | Plain-text meeting summary | Not structured key points; model-generated, may omit context |
| `action_items` | Text plus completed flag | No owner, due date, stable action id or verified assignment |
| `source` | App/version/platform context | Not proof of participant identity or authorization |

`content_scope` is `transcript`, `summary`, or `transcript_and_summary`.
Excluded `transcript`, `summary`, and `action_items` keys are **absent**, not
null. Included arrays may legitimately be empty. Nullable metadata keys use
null for unknown values. `summary` scope includes action items and waits for
summary generation rather than silently falling back to transcript.

The payload is enough for a durable private inbox, meeting digest, searchable
notes, or a reviewed task proposal. It is not sufficient to safely assign
owners, invent deadlines, reconcile external tasks, or prove consent. Ask the
user for those policies and destination identifiers. Keep extraction separate
from irreversible actions and preserve the event id in downstream records.

## Choosing scope and workflow

Use summary-only when a digest suffices; use transcript only when evidence or
custom extraction needs it; combined content costs more storage and model
context. Scope selection is data minimization, not an access-control mechanism.
Ask about participant authorization, sensitive classes, cloud inference,
destination access, retention, backups and deletion before enabling processing.

Automatic export is a separate opt-in. Newly saved meetings may be sent with a
14-day catchup; iCloud imports do not automatically trigger export. Edits do
not automatically re-export. Begin with Send test, then one deliberate synthetic
meeting send, inspect the inbox, then consider automation.

## Future app release gates (not implemented here)

- **Default OFF:** the current settings can present integration enabled with
  an empty endpoint; automatic export is separately OFF and this is not proof
  of network egress. A later Aurora release must initialize new-user webhooks
  OFF, require explicit setup/enable, and preserve existing migration and
  consent semantics. Verify both iOS and macOS. Do not change Aurora during
  blueprint testing.
- **Copy onboarding prompt:** a later app button can copy
  [`prompts/connect-aurora.md`](../prompts/connect-aurora.md). No app button is
  included in this repository.
- **Private ingress compatibility:** the URL validator accepts HTTPS DNS
  names such as `*.ts.net`, not literal private IP addresses. Real app delivery
  through the active private network on **both platforms is unverified**.
  Do not weaken validation or substitute a public tunnel to hide a failure.

## Sources

Wire observations were checked against the Aurora implementation by its owner;
private source and transcripts are intentionally not republished. Interoperable
signature specification: [Standard Webhooks](https://www.standardwebhooks.com/)
and [specification](https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md).
