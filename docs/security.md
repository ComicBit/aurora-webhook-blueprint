# Security and operations

## Trust model

The endpoint receives confidential, untrusted meeting text. A valid HMAC proves
possession of the signing key, not truth of the text, participant consent, or
permission to act. Timestamp validation limits signature replay; event-id
storage limits duplicate work only within its retention window. A new manual
resend id can represent old content. No prompt can guarantee prevention of
prompt injection. Use read-only extraction and human approval before external
actions; apply destination-side idempotency using event id and action identity.

Private ingress is required in the supported deployment. Bind to loopback and
use an authenticated private HTTPS proxy (see [networking](networking.md)).
Do not expose the port on a public interface, use Tailscale Funnel, or rely on
a secret URL. Keep signing enabled. A separate bearer token is optional defense
in depth; it is not a replacement for signatures. Never commit credentials.

## Local storage and host

Run under a dedicated nonadministrator account on an encrypted, patched,
always-on host. The database contains raw text and successful bounded agent
results until deletion/retention. Explicit `show` and `result` commands print
sensitive data; use trusted terminals/files, never shared build logs.
Private filesystem permissions are not encryption: FileVault/LUKS and protected
backups are separate deployment requirements. Root, same-user processes,
debuggers and compromised model CLIs can read that account's files. Do not
mount your home, email, browser profiles, SSH keys or production credentials
into an agent workspace. Configure model authentication separately, then
review [agent-specific isolation limits](agents.md).

Use a private data directory outside synced folders. Do not put transcripts,
.env files, data files or agent outputs in Git. Export environment secrets
through a service manager's protected configuration, not shell history. Use a
random 32-byte signing secret and set the same `whsec_` value in Aurora. Rotate
by coordinating app and receiver; this blueprint has one active signing key,
so schedule rotation and retry interrupted events after the switch.
The data directory must be 0700 and the database 0600. Symlinks anywhere in the
data path are rejected; use the real directory path (for example `/private/tmp`
rather than macOS `/tmp`) and keep its ancestors under trusted administration.

The queue is finite. Pending-capacity and byte-budget exhaustion return 503
so Aurora can retry; monitor disk capacity and receiver availability. The app
has a finite retry budget; an offline host is not a durable cloud relay.
Back up with the receiver stopped (bbolt locks the live database); test a
restore offline before relying on it. A restored old backup may forget recent
dedupe decisions. Reconcile external effects before enabling workers.

## Processing and failure semantics

A 2xx means committed inbox data, not successful automation. Worker mode is
opt-in and sequential. A job is recorded as running before launching the
agent. Failure, timeout or a crash after claim becomes `needs_review`, not an
automatic retry: a process may have changed a remote system before failing.
On restart, previously running jobs are marked for review. Inbox-only mode
leaves real events pending; test events never run.

Inspect status before explicitly retrying. Acknowledge duplicate-effect risk
only after reconciling the destination. Delete logically removes the raw body
and agent result and leaves a dedupe tombstone until expiry. Retention expiry
eventually removes the id; a later replay may be accepted again. Logical
deletion does **not** scrub bbolt free pages or securely erase the database
file, backups, SSD history, provider logs, downstream copies or external agent
memory. Use encrypted storage and define those policies separately. Do not
assume a completed job means the model's answer was correct.

## Deployment checklist

Before enabling automatic export, confirm private HTTPS from the sending
device, tailnet/VPN grants, signing rejection, a synthetic Send test without
agent invocation, real-event durable delivery, duplicate suppression, restart
recovery, storage capacity, deletion and destination-side approvals. Run the
receiver as a supervised service with restart-on-failure and an awake host.
Supervision and service startup are OS-specific: review your service file
before installation; this kit does not silently install a daemon or change
power settings.

Report vulnerabilities privately using the repository owner's GitHub contact;
never post webhook secrets or meeting content in a public issue.
