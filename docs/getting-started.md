# Run your first Aurora webhook inbox

This guide takes you from a local receiver to a private Aurora connection.
Agent processing stays off until you explicitly configure it. You do not need
a model account to receive and inspect events.

## What you need

- An awake macOS or Linux host that will run the receiver.
- Go 1.25+ to build from source, or a matching binary from
  [Releases](https://github.com/ComicBit/aurora-webhook-blueprint/releases).
- Python 3.9+ for the optional synthetic delivery helper.
- An Aurora build with webhook settings and its signing secret when connecting
  the app. Public beta availability may differ from the build under test.
- Private HTTPS connectivity from your sending device: an existing suitable
  VPN, or an approved Tailscale Serve setup.

Do the local check first. Installing a VPN, changing access rules and enabling
model processing are separate decisions, not prerequisites for that check.

## 1. Get the receiver

```sh
git clone https://github.com/ComicBit/aurora-webhook-blueprint.git
cd aurora-webhook-blueprint
go build -o bin/aurora-webhook ./cmd/aurora-webhook
cp config.example.json config.local.json
```

Alternatively, download your platform's unsigned release binary and
`SHA256SUMS`, verify its checksum, and mark it executable with `chmod u+x`.
Place it at `bin/aurora-webhook` to use the commands below. macOS may require
local approval for an unsigned executable; do not disable system protections
globally. You still need the repository's config, docs and optional helper.

The example config binds to `127.0.0.1:8787` and stores data in `./data` relative
to the config file. It is inbox-only. The receiver creates private storage and
rejects symlinks in the data path; existing directories must be private.

## 2. Supply the signing secret and start

Use the same signing secret in Aurora and the receiver. It is `whsec_` followed
by base64-encoded 32 random bytes; bare base64 is also accepted. Do not commit
it or paste it into shell history. Use protected service credentials for a
persistent deployment. For a temporary interactive **Bash** session:

```bash
read -r -s -p 'Webhook signing secret: ' AURORA_WEBHOOK_SECRET
printf '\n'
export AURORA_WEBHOOK_SECRET
bin/aurora-webhook serve -config config.local.json
```

For a local-only trial, you may generate a temporary key instead of using your
app's key: `export AURORA_WEBHOOK_SECRET="whsec_$(openssl rand -base64 32)"`.
A generated key does not configure Aurora. If you use it in a second terminal,
transfer it through a protected method; shell exports are not automatically
shared between terminals. Never include it in screenshots or issue reports.

If Aurora uses optional bearer authentication, also provide the matching
`AURORA_BEARER_TOKEN` (32–4096 bytes, no whitespace). See
[`.env.example`](../.env.example) for variable names. Dotenv files are a
reference format here; the receiver does **not** load them automatically.

## 3. Check delivery with fictional content

In a second terminal at the repository root, supply the **same** signing secret
(and bearer token, if enabled) through protected input, then run:

```sh
python3 scripts/send-synthetic.py --test
python3 scripts/send-synthetic.py --event-id evt_00000000-0000-4000-8000-000000000003
python3 scripts/send-synthetic.py --event-id evt_00000000-0000-4000-8000-000000000003
```

Successful delivery prints `Synthetic event HTTP 204`. The first event is a
connectivity test and never starts an agent. The next two commands send the
same fictional meeting twice with one id; the inbox stores it once. Do not
reuse an id for a changed body: adding `--test`, for example, changes the body
and produces a conflict. The helper never reads real meeting data and refuses
redirects.

Stop the receiver with Ctrl-C before inspecting its database. Administrative
commands use the same exclusive database lock:

```sh
bin/aurora-webhook status -config config.local.json
bin/aurora-webhook show -config config.local.json evt_00000000-0000-4000-8000-000000000003
```

`status` prints ids and processing states. `show` explicitly prints the raw
payload, so use a trusted terminal or protected output file.

## 4. Connect Aurora over private HTTPS

Follow the [private networking guide](networking.md). Keep an existing trusted
VPN if it provides reachable HTTPS DNS and trusted TLS. If you need a VPN,
Tailscale **Serve** is the recommended option; get approval for installation,
sign-in, HTTPS and access policy. Never use Funnel or a public tunnel as a
fallback.

With the receiver running and Tailscale already approved/configured:

```sh
tailscale serve --bg http://127.0.0.1:8787
tailscale serve status
```

Set Aurora's endpoint to the reported HTTPS hostname plus
`/webhooks/aurora`. Keep signing enabled, choose the minimum content scope and
use **Send test** from the actual sending device on the intended VPN. Verify
both iOS and macOS before relying on automatic export. Then send one deliberate
synthetic meeting before considering automatic export.

Serve does not start the receiver or keep your host awake. Certificate issuance
publishes the hostname in Certificate Transparency logs. Private transport is
not offline delivery and says nothing about where a model performs inference.

## 5. Add processing only when you are ready

Decide the intended outcome, review destination, approved runtime/provider and
data policy first. The [onboarding skill](../skills/connect-aurora/SKILL.md)
helps with these decisions. Follow the [agent guide](agents.md) for Claude,
Codex or Hermes configuration, required environment and isolation limitations.
A local CLI can still send content to a cloud provider.

Successful bounded output is retained with the event. Retrieve it explicitly:

```sh
bin/aurora-webhook result -config config.local.json EVENT_ID
```

The adapters generate digests/proposals; they do not implicitly create CRM
records, assign tasks or message your team. Those destinations and approvals
need their own implementation. Test events never invoke an agent. Hermes task
wake-up starts work on an awake host; it cannot wake a sleeping computer.

## Operate the inbox

The defaults are seven-day retention, 100 pending events, a 256 MiB logical
budget and eight concurrent requests. Every non-test record reserves 64 KiB for
a future result, even in inbox-only mode. The database can keep its high-water
allocation after deletion; leave room for overhead and backups. Configuration
rejects unknown keys and nonloopback listeners.

Stop the receiver before these commands:

```sh
bin/aurora-webhook delete -config config.local.json EVENT_ID
# Only after reconciling any possible earlier downstream effects:
bin/aurora-webhook retry -config config.local.json --acknowledge-duplicate-risk EVENT_ID
```

An interrupted/failed job becomes `needs_review`, not an automatic agent retry.
Deletion logically removes payload/results and retains a dedupe tombstone;
retention eventually removes the id too. Neither operation securely erases
bbolt free pages, backups or provider history. See [security and operations](security.md).

## Before using real meetings

Read the [verification evidence](verification.md) and [payload contract](payload.md).
Local synthetic checks are not evidence of real Aurora VPN delivery or real
model behavior. Confirm participant authorization, scope, destination access,
provider terms, retention and backup policy. There is no exactly-once guarantee
for downstream effects and no guarantee of prompt-injection prevention.

[Back to the project overview](../README.md)
