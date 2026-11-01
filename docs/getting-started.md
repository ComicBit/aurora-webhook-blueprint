# Run your first Aurora webhook inbox

Start with a private inbox, then add a folder or an approved agent destination.
**The default makes no model call.** You do not need a model account to receive
and inspect events. This guide is for the Rust **0.2.1 source**, not older
0.2.0 Rust binaries or Go 0.1.0 release binaries.

## What you need

- An awake host and a trusted local account. macOS and Linux are the intended
  Unix targets; consult [verification evidence](verification.md) for what has
  actually run. Windows is not supported by the current Unix implementation.
- Building from source requires Rust **1.98**, Cargo and a native C compiler/
  linker for bundled SQLite. On macOS this normally means Command Line Tools;
  on Linux, a distribution's C toolchain. Installation is an explicit action.
- Python 3.9+ for the optional bundled synthetic sender.
- For app delivery later: an invited Aurora beta build exposing webhook
  settings, its signing key, and private HTTPS reachable from the sending
  device. [Aurora's waitlist](https://auroraisyours.com/#waitlist) is not a
  public TestFlight installation link.

Do the local check first. VPN installation, account sign-in, provider access
and automatic export are separate decisions, not prerequisites for it.

## Prebuilt macOS arm64 preview

The [Rust 0.2.1 preview](https://github.com/ComicBit/aurora-webhook-blueprint/releases/tag/v0.2.1)
includes the CLI and setup resources for macOS arm64. Download
`aurora-webhook-v0.2.1-darwin-arm64.tar.gz` and `SHA256SUMS` into a trusted
directory, verify the checksum, then extract into a new directory:

```sh
shasum -a 256 -c SHA256SUMS
mkdir aurora-preview
tar -xzf aurora-webhook-v0.2.1-darwin-arm64.tar.gz -C aurora-preview
cd aurora-preview/aurora-webhook
cp config.example.json config.local.json
./aurora-webhook --config config.local.json plan
./aurora-webhook --config config.local.json apply --approve
```

Use `./aurora-webhook` in place of `target/release/aurora-webhook` in the
remaining examples. The bundle also contains the source and setup resources.
It needs no Go or external SQLite server. Source builds need the toolchain
above; this prebuilt path does not.

The binary is **unsigned and not notarized**. Checksums detect corruption;
they do not constitute a publisher signature. Follow your Mac's security
policy or build from the tagged source rather than blindly bypassing a
quarantine warning. Consult [verification evidence](verification.md) for measured
qualification; no Linux or other-architecture qualification is implied here.
The historical 0.2.0 macOS arm64 preview does not implement `.env` loading or
`token-gen`. Old v0.1.0 bundles are Go, not this Rust build.

## 1. Build and initialize a database-only inbox

```sh
git clone https://github.com/ComicBit/aurora-webhook-blueprint.git
cd aurora-webhook-blueprint
cargo build --release
cp config.example.json config.local.json
target/release/aurora-webhook --config config.local.json plan
target/release/aurora-webhook --config config.local.json apply --approve
```

Review the `plan` output before approving it. `plan` validates and displays the
proposed configuration without activating it. `apply --approve` initializes
`data/inbox.sqlite3` or activates a new configuration **with the receiver
stopped**. It never creates historical deliveries. The database's approved
snapshot is authoritative: editing the file alone does not activate changes,
and `serve` refuses a file that differs from the approved snapshot.

The example binds to `127.0.0.1:8787`, uses `./data` **relative to the config
file**, and has `"destinations": []`. It stores exact verified request bytes
without leaving unfinished agent jobs. Keep the config, data and any exports
outside shared/synced directories. Existing managed directories must be
private; paths containing symlinks are rejected. Use real paths (for example
`/private/tmp`, not macOS `/tmp`) where applicable.

`--config` is a global option: it may precede or follow the subcommand. The
examples consistently put it first. If omitted, the file is `config.json`.

## 2. Capture Aurora's signing secret and a separate bearer token

Use both protections. Signing verifies the exact body's authenticity/integrity
and timestamp freshness (±5 minutes); bearer authentication is a separate shared
authorization credential. Neither establishes participant consent. **This
receiver requires signing.** If you decline signing, stop app setup rather than
silently disabling verification.

In Aurora's webhook settings, use **Signing secret → Generate** only if the app
has no key, then **Copy**. Aurora is the source of truth: do not generate a
replacement receiver key and assume the app will use it. The receiver accepts
`whsec_` plus base64-encoded 32 bytes (or bare base64).

With your approval, store both credentials in an ignored private `.env` beside
`config.local.json`. `.env.example` is a tracked placeholder reference, not a
credential store. Use a trusted local editor or protected masked input/credential
catalog where available; never put either secret into chat, command arguments,
Git, shared logs or screenshots. A local editor can still create swap/backup
copies: review its behavior and keep the directory outside shared/synced folders.

For a new file, this interactive **Bash** example captures the app key and
generates a distinct CSPRNG 32-byte URL-safe, unpadded base64 bearer token without
printing either. Run it only after approving local credential storage; it
refuses to overwrite an existing file:

```bash
(
  umask 077
  set -o noclobber
  read -r -s -p 'Copy signing secret from Aurora: ' signing
  printf '\n'
  bearer="$(target/release/aurora-webhook token-gen)" || exit
  printf 'AURORA_WEBHOOK_SECRET=%s\nAURORA_BEARER_TOKEN=%s\n' \
    "$signing" "$bearer" > .env || exit
)
```

The file must be owned by your account, regular, single-link and exactly 0600,
with no symlinks or untrusted-writer ancestors. Private permissions are not
encryption or protection from same-user processes. Protect OS storage, backups
and clipboard history too. Using a protected local editor/catalog, copy **only**
`AURORA_BEARER_TOKEN`'s value into Aurora **Authentication → Bearer**. Do not paste
the signing secret there. Clear the clipboard when done.

```sh
target/release/aurora-webhook --config config.local.json doctor
target/release/aurora-webhook --config config.local.json serve
```

In 0.2.1, only `serve` and `doctor` load `.env` automatically, from **beside the
selected config**, not a recursive search of the current directory. A missing
implicit `.env` is optional; an explicit global `--env-file PATH` must exist:

```sh
target/release/aurora-webhook --config config.local.json \
  --env-file /absolute/private/receiver.env serve
```

The whole valid file is parsed before changing the environment, before runtime
threads start; it is never shell-sourced or executed. Reads are bounded to 64 KiB.
Inherited process variables take precedence, so stale exported credentials can
override edits to `.env`. Restart the receiver after credential changes; there
is no per-request reload. Adapter variables in the same file work through the
loaded process environment, with the existing adapter allowlists unchanged.
Other commands do not load `.env`; administration still needs its configuration
and inbox. `token-gen` and `keygen` need neither configuration nor credentials.
`keygen` is only for isolated synthetic/receiver tooling, never app signing-key
setup. Existing 0.2.0 binaries lack this dotenv feature.

## 3. Check delivery with fictional content

The Python helper reads **process environment only**, not the receiver's `.env`.
In a second interactive Bash terminal, securely copy the same two values from
their protected source (do not shell-source `.env`):

```bash
read -r -s -p 'Webhook signing secret: ' AURORA_WEBHOOK_SECRET
printf '\n'
read -r -s -p 'Bearer token: ' AURORA_BEARER_TOKEN
printf '\n'
export AURORA_WEBHOOK_SECRET AURORA_BEARER_TOKEN
```

Then run:

```sh
python3 scripts/send-synthetic.py --test
python3 scripts/send-synthetic.py --event-id evt_00000000-0000-4000-8000-000000000003
python3 scripts/send-synthetic.py --event-id evt_00000000-0000-4000-8000-000000000003
target/release/aurora-webhook --config config.local.json status
target/release/aurora-webhook --config config.local.json list --limit 20
target/release/aurora-webhook --config config.local.json show evt_00000000-0000-4000-8000-000000000003
```

Successful delivery prints `Synthetic event HTTP 204`. The test is retained
for connectivity inspection but causes **zero outward delivery**, including
folders, agents, remote requests and later backfills. The two identical real
fictional sends create one receipt, not two. Reusing an id with changed bytes
returns **409**; an identical duplicate creates no new intents and does not
extend the original deadline. A manual resend with a **new** id is new work
under the then-active configuration, even if its meeting text is old.

`status`, `list`, `show` and `result` work while the daemon runs. `status`
contains redacted operational metadata and up to 1,000 recent delivery rows;
`list` paginates receipt metadata (`--limit` 1–100, `--after RECEIPT_ID`).
`show EVENT_ID` prints sensitive, exact original bytes. Use a trusted terminal.
A successful receipt is not a successful downstream task.

## 4. Optional: save a portable folder copy

Stop `serve` with Ctrl-C. Replace the empty `destinations` array in
`config.local.json` with the following, using your own **absolute private
folder path outside the database tree**:

```json
"destinations": [
  {
    "id": "notes",
    "enabled": true,
    "kind": "folder",
    "root": "/absolute/private/aurora-notes",
    "formats": ["original_json", "markdown"]
  }
]
```

This is a replacement JSON property, not a complete config file. Do not
literally use the placeholder path. The receiver can create the folder with
private permissions; existing directories must be private and have trusted,
non-symlink ancestors. Folder roots must not overlap each other or `data_dir`.

```sh
target/release/aurora-webhook --config config.local.json plan
target/release/aurora-webhook --config config.local.json doctor
target/release/aurora-webhook --config config.local.json apply --approve
target/release/aurora-webhook --config config.local.json serve
```

`doctor` only checks local configuration/files/environment. It can create a
configured folder root, but makes **no network, authentication or inference
probe**. It does not prove a live agent will obey policy.

Send a **new** fictional event id to see a folder export. Each event gets a
subdirectory named by the full SHA-256 of its event id. The selected files are
`event.json` (unchanged bytes) and `meeting.md` (readable source sections, not an
AI digest), followed by `receipt.json` containing hashes and the configuration
revision. Markdown organizes meeting/event metadata, summary, action items,
transcript/turns, speakers and source metadata; source text stays literal inside
data-dependent fences. Unknown additive fields retain their source paths in
Extra source fields. Excluded scope is distinct from included-empty text/lists;
no owner or deadline is inferred. Files publish without replacement; the receipt is
written last. Matching partial bundles can be reconciled after explicit retry;
conflicting or edited artifacts and symlinks fail for review. A bundle is not
one atomic filesystem operation. Do not edit receiver-owned files in place.

Only future newly accepted events use the new destination. Existing receipts
stay in the database until you explicitly approve a [backfill](#send-retained-history-with-a-reviewed-backfill).
Database deletion does **not** remove folder copies.

## 5. Connect Aurora over private HTTPS

Follow the [private networking guide](networking.md). Keep an existing trusted
VPN if it provides reachable HTTPS DNS and trusted TLS. If you need private
connectivity, use approved Tailscale **Serve**—never Funnel or a public tunnel.
Installation, sign-in, HTTPS and access-policy changes need authorization.
Inspect existing Serve routes first. Never overwrite another service's route.
If the default HTTPS port is occupied, choose an explicit free port as described
in the networking guide. With Tailscale approved/configured and an unused 443:

```sh
tailscale serve status
tailscale serve --bg --https=443 http://127.0.0.1:8787
tailscale serve status
```

Set Aurora's endpoint to the **full reported HTTPS hostname, port and path**:
`https://<machine>.<tailnet>.ts.net:443/webhooks/aurora` (or your chosen port).
Do not guess it or omit `/webhooks/aurora`. `GET /healthz` is a health check;
Aurora sends `POST /webhooks/aurora`. A **405** suggests a route/method mismatch,
not a reason to rotate credentials; a **401** on the correct POST suggests
checking the copied signing/bearer values and timestamp. Never disable
protections to troubleshoot. A source-accepted URL is not delivery proof.
Select the minimum content scope and use **Send test** from the actual sending
device on the intended VPN. Then send one deliberate fictional meeting before
considering automatic export. Test each intended platform separately.

Serve does not start the receiver or keep the host awake. Certificate issuance
publishes the hostname in Certificate Transparency logs. Private ingress is not
offline transport; local CLI execution is not necessarily local inference.

## 6. Add an approved agent only if you need it

Choose the outcome, recipient/provider, exact operation and target authority
before sharing meeting content. The [agent guide](agents.md) documents local
Claude/Codex/Hermes operations and remote Hermes Runs/Grok Bot handoffs.
A fresh task, a saved-session continuation and a notification to an existing
remote target are different operations, not interchangeable modes.

Configure a destination, review `plan`, run local-only `doctor`, then stop the
receiver and `apply --approve`. Validate it with fictional **non-test** content
only when actual provider use is authorized. A connectivity test never calls
the provider, and `doctor` is not a substitute for that separate verification.
Retrieve a completed local result using its **delivery id**, from `status`:

```sh
target/release/aurora-webhook --config config.local.json result DELIVERY_ID
```

A remote `handed_off` receipt reports admission, not task completion or a
retrievable answer. No adapter silently adds a CRM/task/message integration.

## Send retained history with a reviewed backfill

Adding/enabling/changing a destination does not send old content. Backfill is
a separate disclosure/action approval. Stop the receiver, apply the intended
configuration first, and choose a **receipt-time** UTC interval `[start, end)`:

```sh
target/release/aurora-webhook --config config.local.json backfill preview notes \
  --start 2026-11-01T00:00:00Z --end 2026-11-02T00:00:00Z \
  --output /absolute/private/backfill-notes.json
# Review the exact private plan and its destination/authority before approving:
target/release/aurora-webhook --config config.local.json backfill apply \
  --plan /absolute/private/backfill-notes.json --approve
```

Replace dates and paths with the intended selection. Output must be a **new**
private file; preview also prints metadata to the terminal. Preview records an
immutable local plan: receipt incarnation/id, event id, body hash, original
expiry, receiver, config and destination fingerprints. By default it selects
the latest received revision **within the range** per meeting; explicitly add
`--all-revisions` only if every eligible revision is intended. Tests are always
excluded. Unavailable latest revisions are counted, not replaced with older
content; already-routed initial deliveries to that destination are skipped.

Approval must match the stored plan, unchanged configuration and still-retained
bodies. Plans expire after at most **15 minutes**, earlier if selected content
expires; at most **1,000** new intents are allowed per preview. Narrow the
interval rather than silently reducing selection. Neither preview nor apply
contacts a destination; approved intents run after `serve` restarts. Backfill
cannot resurrect deleted/expired bytes or reset retention. Inspect the
selection and obtain consent before approval, not after delivery.

## Operate, delete and recover

| Command | While `serve` runs? | Meaning |
| --- | --- | --- |
| `plan`, `doctor` | Yes | Proposed-file review/local preflight; no activation or model call |
| `status`, `list`, `show`, `result`, `legacy-result`, `export` | Yes | Read-only database inspection; content commands are sensitive |
| `apply --approve` | No | Initialize/activate the reviewed config |
| `delete`, `retry`, `expire`, `backup`, `import-go`, both `backfill` commands | No | Offline administration under the exclusive owner lock |

Defaults: seven-day payload/result retention, another 30 days of finite
deduplication metadata, 256 MiB **logical** storage, 1,000 pending/in-flight
deliveries, 100,000 receipts, eight intake requests, four active deliveries,
64 connections, 256 MiB minimum free space and at most 16 destinations. The
byte budget includes receipt/intent overhead and bounded per-destination result
reservation; it is not a cap on SQLite/WAL disk allocation, exports or backups.
Capacity/headroom exhaustion returns 503 rather than acknowledging lost work.

Independent destinations make progress with at most one in-flight delivery
per destination. Safe automatic retries with backoff occur only when the
adapter positively reports **not admitted**. Ambiguous network/process errors,
timeouts, output failures and interrupted in-flight work become `needs_review`.
Startup does not blindly repeat them. Review downstream state first:

```sh
# Stop serve first:
target/release/aurora-webhook --config config.local.json delete EVENT_ID
# For a different retained delivery, only after reconciling possible effects:
target/release/aurora-webhook --config config.local.json retry DELIVERY_ID --acknowledge-duplicate-risk
target/release/aurora-webhook --config config.local.json expire
target/release/aurora-webhook --config config.local.json backup --output /absolute/private/backup-inbox.sqlite3
# This read-only normalized export may also run while serve runs:
target/release/aurora-webhook --config config.local.json export --output /absolute/private/events.jsonl
```

`retry` creates an explicitly authorized repeat for an eligible terminal
delivery under the same enabled destination revision, **without extending its
original expiry**. It can duplicate external effects. Deleted/expired/test
content is not retryable. `delete` logically removes local raw/results, not
folder copies, provider history, profiles or backups. Tombstones expire 30 days
after the original payload deadline by default; deduplication is not forever.

`backup` creates a consistent private SQLite backup in a **new** file.
`export` emits normalized JSONL, **not exact signed-raw backup**, and is not the
Go migration format. Define restore/reconciliation and external retention
policies before using real content; see [security and operations](security.md).

## Migrate from the Go 0.1.0 inbox

Do not run an old binary against Rust storage, rename a bbolt file to SQLite,
or reuse the old `worker`/`retention_days` configuration. Stop the Go receiver,
keep an untouched private backup of its database, and reconcile any prior
external work. Use a **new** Rust data directory with `destinations: []`, then
`plan` and `apply --approve` it as above.

The optional read-only migration exporter alone requires **Go 1.25+**. From
the repository root, with real absolute private paths:

```sh
(cd tools/export-go-inbox && go run . \
  --input /absolute/private/stopped-old-inbox.db \
  --output /absolute/private/migration-go.jsonl)
target/release/aurora-webhook --config config.local.json import-go \
  --input /absolute/private/migration-go.jsonl --approve
```

The exporter locks bbolt read-only and creates a new 0600 JSONL file. Records
carry version 1, event id, original receipt/expiry times, prior state, SHA-256,
base64 original body/result (or null), and test marker. Rust verifies records
and imports available bodies, bounded original results and original delivery
state as **held history with zero delivery jobs**. `status` reports original
state metadata for reconciliation. Normal `result DELIVERY_ID` is for new
destination results; use the read-only command below for a retained old result:

```sh
target/release/aurora-webhook --config config.local.json legacy-result EVENT_ID
```

It prints sensitive original bytes, not a new inference result. Original
deadlines survive; deleted/expired bodies and results stay unavailable, and
records beyond the finite metadata horizon are rejected rather than rejuvenated.

Export/import can fail after writing earlier records; this is not an atomic
whole-inbox migration. Preserve the old backup and inspect the error/counts.
Do not treat a partial export as complete; rerun the exporter to a new file
after correcting the cause. Importing the same matching receipt metadata is
idempotent; conflicting records stop import. Do not automatically rerun old
pending/running work. New downstream delivery requires separate route approval
and the exact reviewed backfill flow above. Keep migration JSONL confidential
and apply its own deletion policy.

## Before using real meetings

Read [verification evidence](verification.md), [payload limits](payload.md)
and [security](security.md). Synthetic process tests are not proof of real
Aurora/VPN delivery or real model behavior. Confirm participant authorization,
minimum scope, destination access, provider terms and all retention/backups.
There is no downstream exactly-once or prompt-injection-immunity guarantee.

[Back to the project overview](../README.md)
