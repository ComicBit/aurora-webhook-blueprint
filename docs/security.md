# Security and operations

## Trust model

The endpoint receives confidential, **untrusted** meeting text. A valid HMAC
proves possession of a signing key, not truth, participant consent or authority
to act. Signature freshness is ±5 minutes. Event ids limit duplicate work only
inside a finite metadata horizon. A manual resend with a new id may contain
old text and be routed under a new configuration.

Use private HTTPS through an authenticated VPN proxy; the listener must stay
on a literal loopback address. See [private networking](networking.md). Never
use Funnel, a public bind/tunnel or a secret URL as the security boundary.
Keep signing enabled. Optional bearer authentication is defense in depth, not
a replacement for raw-body signatures. Preserve exact bytes and signature
headers through the proxy. A source-accepted `https://…ts.net` URL is not actual
Aurora device/VPN delivery proof.

A prompt cannot guarantee prevention of prompt injection. Keep the default
`destinations: []` unless a destination is wanted. For processing, prefer
read-only proposals and human approval before effects. Enforce action identity
and idempotency **at the destination**. The payload does not establish owners,
deadlines or stable action-item ids; absence is unknown, not permission to infer.

## Configuration, disclosure and authority

`plan` reviews a proposed file; `apply --approve` activates it offline. The
SQLite-approved configuration is canonical. `serve` refuses an edited file
that has not been applied. Configuration changes never automatically grant
historical work. An accepted real event commits its exact bytes and the
selected destination intents together; a 204 means durable receipt, **not
completed automation**. If any selected intent cannot fit, the whole new
receipt is rejected rather than partially routed.

Each agent/remote destination needs separate `disclosure_approved` and
`authority_approved` decisions. Review both **where content goes** and **what
the selected target can do**. New tasks, saved local session continuations,
Claude cloud follow-ups, Hermes API-owned sessions and Grok Bot routines have
different authority. An existing target may have tools, history, credentials,
other participants and broader permissions than a fresh tool-disabled task.
The connector's request controls do not rewrite that target's policy.

Enabling a local CLI does not establish local inference. Review the actual
provider, routing/fallback behavior, telemetry and persistence before sharing
meeting content. For fully local requirements, reject a hosted fallback.
`doctor` checks local files/configuration/environment only—no network,
authentication, model, app or VPN probe. A passed check is not a sandbox audit.

Backfill needs **new historical disclosure/action consent**, a private preview
and explicit approval of the exact selection. Selection uses retained receipt
time, not the meeting date. The frozen plan binds receipt incarnation, event
id/hash/expiry, receiver and config/destination fingerprints. It expires after
at most 15 minutes; no more than 1,000 new intents per plan. Approval cannot
revive expired/deleted bodies, substitute changed targets or renew retention.
Test events are excluded from every outward path, including folders/backfill.

## Host, secrets and storage

Run under a dedicated nonadministrator account on an encrypted, patched,
awake host. Protect the service's installation and configuration from other
writers. Use a private data directory outside synced folders. Existing managed
directories must be 0700 and files 0600; symlinks in managed paths are rejected.
Use real paths and trusted ancestors, such as `/private/tmp` rather than the
macOS `/tmp` symlink. Private permissions are not encryption. FileVault/LUKS
and protected backups are separate deployment requirements.

Root, same-user processes, debuggers and a compromised agent binary can still
read that account's files. Read-only CLI flags are not a filesystem read
sandbox. Do not expose personal homes, email/browser profiles, SSH keys,
production credentials or personal agent sessions to the workflow. Provision
separate private workspaces and runtime profiles. The receiver is not a
multiuser service and does not offer tenant isolation.

Keep signing/bearer keys out of Git, argv, issues, prompts and shared logs.
Use protected service environment credentials; dotenv files are not loaded
automatically. `keygen` outputs a sensitive key. This implementation has one
active signing key: coordinate Aurora/receiver rotation and interrupted sends.
Adapters clear the process environment; only fixed locale/path/private-home
values and the explicitly selected runtime credentials are provided. Runtime
profiles and provider-side authentication remain sensitive disk/remote state.
See [agent boundaries](agents.md) for exactly what is and is not inherited.

SQLite uses bundled `rusqlite`, WAL and FULL synchronization, with one daemon
writer behind a bounded command channel. Durability still depends on the
filesystem, OS and storage device honoring synchronization. Do not place the
live database on a shared/synced filesystem. Do not edit the database or
receiver-owned export artifacts directly.

The default **logical** budget is 256 MiB, with 1,000 pending/in-flight
deliveries, 100,000 receipts, eight intake slots, four active deliveries,
64 connections and 256 MiB free-disk headroom. There are at most 16 destinations.
Bodies are capped at 8 MiB. Delivery result capacity is reserved per destination;
logical accounting does not cap database/WAL high-water allocation, folder
copies, profile state, backups or migration files. Capacity/storage admission
failures return 503. Monitor real free space and availability. An offline host
is not a durable cloud relay; Aurora has a finite retry budget.

## Independent delivery and ambiguous failures

Each destination owns its own durable delivery state; a stalled destination
does not serialize every other destination. Scheduling permits at most one
in-flight delivery per destination and a bounded global total. The intent
records a destination revision. Changed or disabled destinations do not
silently redirect already-created intents; inspect `blocked_revision` status.

Automatic retry with bounded backoff is reserved for **positive non-admission**.
A timeout, process exit, malformed/overflowed result, transport failure or
crash after claim may have occurred after external work started. Such cases
become `needs_review`; in-flight work found on restart also needs review.
A `handed_off` state establishes admission only, not completion. Even
`completed` does not establish correctness of a model's answer.

After reconciling the selected target, an offline
`retry DELIVERY_ID --acknowledge-duplicate-risk` can authorize a repeat of an
eligible terminal delivery under the same enabled revision. This creates a
new repeat identity and can duplicate effects; it does not extend expiry.
Do not blindly retry to make a status green. Local process-group cancellation
does not retract admitted cloud work, a detached Hermes gateway task, an
existing routine's effects or a provider charge.

Graceful shutdown and fatal listener errors drain local task guards. A receiver
SIGKILL, kernel crash or power failure cannot execute those guards: a separate
local child may survive. Use an approved service supervisor that owns the
process tree; reconcile interrupted work before retrying. No service or power
configuration is installed automatically.

## Retention, logical deletion and export copies

Default payload/result retention is **604800 seconds (7 days)** from original
receipt. Deduplication metadata/tombstones expire
**2592000 seconds (30 days) after that original payload deadline**. Duplicates,
explicit retry, backfill and import never extend those immutable deadlines.
After the finite metadata horizon, an old id may be accepted as new work.

Minimal delivery metadata separately records `submission_state` and bounded
external identifiers until metadata expiry. Thus `expired` or `deleted` local
content does not falsely imply that an already-admitted remote task stopped
or was erased. Late observed receipts may update that metadata without
restoring raw notes or result content.

`delete EVENT_ID` logically removes local raw content and delivery results;
`expire` removes overdue content and eventually finite metadata. Neither
operation is forensic erasure. SQLite/WAL/free pages, filesystem snapshots,
SSDs, backups, runtime profiles, provider histories and folder copies can
retain data. Even SQLite's configured secure-delete behavior cannot guarantee
physical erasure. Define encryption, key management and retention separately.

Folder exports are **separate retained copies**, not a database view. The
bundle uses an event-id SHA-256 directory, no-replace private files and a
hash receipt published last. It can reconcile matching partial files but
refuses edited/conflicting/symlink artifacts. The bundle is not atomically
published as a whole. Do not claim that database deletion removes a folder,
or that editing a generated Markdown file updates the source inbox.

`show EVENT_ID`, `result DELIVERY_ID` and imported-history
`legacy-result EVENT_ID` deliberately print sensitive bytes.
`status` and `list` expose operational ids/hashes/times, not transcripts; those
metadata can still be identifying. `export --output NEW_FILE` creates sensitive
**normalized JSONL**, not an exact signed-raw backup or a transactional
whole-inbox snapshot while the receiver is changing. All exports have their
own access/deletion policy. Never attach real exports to public bug reports.

## Offline maintenance, backup and migration

Read-only `status/list/show/result/legacy-result/export` can coexist with `serve`.
`apply/delete/retry/expire/backup/import-go/backfill preview/backfill apply`
require the exclusive owner lock: stop the receiver first. `plan` and `doctor`
do not activate configuration. A command failure is not authorization to
remove a live lock or weaken private-file checks.

Use `backup --output NEW_PRIVATE_FILE` for a consistent SQLite backup, not a
casual copy of only the database while WAL is active. The backup command is
offline and verifies SQLite integrity, not external effects. Test restoration
in isolation with all destinations disabled. An old backup can forget newer
deduplication/approval decisions; reconcile downstream state before enabling
workers. Restore itself has no dedicated CLI command: deployment-specific
file replacement, private permissions and absolute config paths must be
reviewed offline. Never run two receivers against a restored clone with
active destinations.

For Go 0.1.0 migration, keep an untouched stopped bbolt backup. Use the
read-only exporter and explicit `import-go --approve` procedure in
[getting started](getting-started.md#migrate-from-the-go-010-inbox). Import
preserves original ids, hashes, deadlines, available bounded results and prior
delivery state as held history and creates zero jobs. Original state is
reconciliation metadata, not a runnable status; `legacy-result EVENT_ID`
retrieves an unexpired historical result separately from new delivery results.
Export/import can stop partway. Inspect errors/counts and correct the source
issue rather than assuming completion. Preserve old evidence until external
work is reconciled; only an approved frozen backfill may create new intents.

## Deployment checklist and evidence

Before automatic export, establish participant authorization, private HTTPS
from the actual device, VPN grants, signing rejection, test-event zero-outward
behavior, durable receipt, duplicate suppression, restart recovery, capacity,
delete/expiry behavior and intended target permissions. Run an approved
supervised service on an awake host. This kit does not install a service,
change power policy, sign in, configure a VPN or provision accounts for you.

Read the [verification record](verification.md). Source controls, process
doubles and historical Go tests are not evidence of current real-model,
Aurora device or Linux runtime behavior. Report vulnerabilities privately
through the repository owner's GitHub contact; never publish credentials or
meeting content in an issue.
