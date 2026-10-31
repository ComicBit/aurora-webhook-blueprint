# Aurora webhook blueprint

A private-first, self-hosted receiver for finalized Aurora meeting transcripts
and summaries. Durable inbox first; agent processing only when you enable it.
No Aurora app modification, public relay, audio transfer, or model account is
required for inbox-only use.

**Status:** standalone reference implementation. Local synthetic verification
is recorded in [verification](docs/verification.md). Live Aurora iOS/macOS VPN
delivery and real model runs have not been verified. Test with your app before
relying on it. Do not treat this kit as a claim of audited compliance.

## Start with your goal

Give an agent the [copyable prompt](prompts/connect-aurora.md), or install/use
[`skills/connect-aurora/SKILL.md`](skills/connect-aurora/SKILL.md) with this
repository available. The skill asks what outcome and destinations you want,
what data may leave the host, which runtime you approve, and how you connect
privately. It then implements and verifies the agreed workflow. Copying just
the skill requires access to this public repository's referenced docs.

A private inbox, a reviewed meeting digest and autonomous writes to a CRM are
not the same risk. Start with an inbox and minimum content scope. A locally
launched Claude, Codex or Hermes process is **not necessarily local inference**.

Prefer a prebuilt executable? Download your platform's unsigned binary and
`SHA256SUMS` from [Releases](https://github.com/ComicBit/aurora-webhook-blueprint/releases),
verify its checksum, and use it with `config.example.json` and the protected
environment below. macOS may require local executable approval; do not disable
system protections globally. Building from source is also supported.

## Local quickstart

Requirements: macOS or Linux, Go 1.25+, Python 3.9+ for the optional synthetic
sender. Do this in your own repository/template copy or a local clone:

```sh
git clone https://github.com/ComicBit/aurora-webhook-blueprint.git
cd aurora-webhook-blueprint
go build -o bin/aurora-webhook ./cmd/aurora-webhook
cp config.example.json config.local.json
```

Set `AURORA_WEBHOOK_SECRET` to the same signing secret configured in Aurora.
It must be `whsec_` plus base64-encoded 32 random bytes (bare base64 also
accepted). Use protected service credentials; do not paste secrets into shell
history. For a temporary interactive Bash session, read it without echo:

```bash
read -r -s -p 'Webhook signing secret: ' AURORA_WEBHOOK_SECRET
printf '\n'
export AURORA_WEBHOOK_SECRET
bin/aurora-webhook serve -config config.local.json
```

For a purely synthetic local run you can generate an ephemeral key instead:
`export AURORA_WEBHOOK_SECRET="whsec_$(openssl rand -base64 32)"`.
This does not configure Aurora; real app and receiver must share the same key.
If Aurora is configured for optional bearer authentication, also provide
`AURORA_BEARER_TOKEN` through protected credentials (32–4096 bytes, no whitespace).

In another terminal with the same secret environment:

```sh
python3 scripts/send-synthetic.py --test
python3 scripts/send-synthetic.py --event-id evt_00000000-0000-4000-8000-000000000003
# Repeating the exact command demonstrates same-id deduplication.
python3 scripts/send-synthetic.py --event-id evt_00000000-0000-4000-8000-000000000003
```

The sender uses only the bundled fictional fixture and refuses redirects. Do
not reuse one id for different bodies (`--test` changes the body). Real app
connectivity checks have their own ids. Stop the receiver before local admin
commands: one process owns the database lock.

```sh
bin/aurora-webhook status -config config.local.json
# Explicitly print sensitive payload/result only to a trusted terminal or file:
bin/aurora-webhook show -config config.local.json EVENT_ID
bin/aurora-webhook result -config config.local.json EVENT_ID
bin/aurora-webhook delete -config config.local.json EVENT_ID
# Only after reconciling any possible prior downstream effects:
bin/aurora-webhook retry -config config.local.json --acknowledge-duplicate-risk EVENT_ID
```

## Connect the app privately

Read [private networking](docs/networking.md) before changing network policy.
Use an existing suitable VPN with trusted HTTPS DNS. Without one, recommend
Tailscale **Serve**, never Funnel. With the receiver running on loopback:

```sh
tailscale serve --bg http://127.0.0.1:8787
tailscale serve status
```

Use the reported `https://<machine>.<tailnet>.ts.net/webhooks/aurora` URL in
Aurora, keep signing on, choose the least content needed and use Send test.
The actual sending phone/Mac must have active authorized private connectivity.
Certificate issuance publishes the hostname in CT logs. Serve does not start
the receiver, keep a computer awake or make cloud inference private.

A hostname passing Aurora's URL validator is not delivery evidence. Verify
Send test on **both iOS and macOS**, then a deliberate synthetic meeting send,
then optional automatic export. Do not change Aurora during this first kit
trial. The later default-OFF settings correction and copy-prompt button are
tracked in [deferred app release gates](docs/payload.md#future-app-release-gates-not-implemented-here).

## Configuration and agent mode

[`config.example.json`](config.example.json) is inbox-only. Paths for local
storage are relative to the config file. The receiver rejects nonloopback
listeners and unknown config keys. Defaults: seven-day retention, 100 pending
jobs, 256 MiB logical storage budget, eight concurrent requests. The database
file can retain its high-water allocation after deletion; logical limits are
not filesystem quotas. Each non-test record reserves 64 KiB for a future agent
result, even in inbox-only mode. Leave capacity for database overhead and backups.

To enable processing, read [supported runtimes and isolation](docs/agents.md).
Set `worker.agent`, an absolute `binary` path, a dedicated absolute `workspace`,
an approved `task`, `timeout_seconds`, and `cloud_approved: true`. This flag
records the operator's egress approval; it does not constrain the provider's
network access. Do not enable it to circumvent a fully local requirement.
No arbitrary shell command or webhook-selected executable is supported.
Hermes additionally requires explicit `HERMES_INFERENCE_MODEL` and
`HERMES_INFERENCE_PROVIDER`; a custom route also requires `CUSTOM_BASE_URL`.
See [environment reference](.env.example); dotenv files are not automatically loaded.

The worker gets the original event over stdin, never transcript argv. Test
events never invoke it. Success is process completion, not truth of extracted
facts. Successful bounded stdout is stored with the event and can be retrieved
explicitly with `result`; it is never printed to receiver logs. The result shares
the event's retention and deletion. Failure or interrupted work becomes
`needs_review`; no blind automatic agent retries. Reconcile first, then
explicitly retry. Configure your intended destination separately; no implicit
CRM, messaging or external task writes are enabled.

## Guarantees and limits

- Exact-body Standard Webhooks HMAC verification before JSON interpretation;
  timestamp freshness, optional bearer auth, 8 MiB body limit and bounded HTTP
  timeouts/concurrency. Unknown additive schema fields are tolerated.
- A 2xx means durable inbox acceptance, not completed agent work. Matching-id
  retries dedupe; same-id different-body requests conflict.
- One local transactional bbolt database; sequential worker claim persists
  before process launch. A restarted in-flight job needs human review.
- Deletion removes payload but retains a dedupe tombstone until expiry.
  Retention eventually removes both: dedupe is finite, not exactly-once.
- No encryption-at-rest implementation, downstream authorization, guaranteed
  prompt-injection prevention, machine wake, public ingress or cloud relay.

See [payload quality and contract](docs/payload.md), [security and operations](docs/security.md),
[agent research](docs/agents.md), and [verification evidence](docs/verification.md).
MIT licensed. Examples contain no real meetings. Run `go test -race ./...` and
`go vet ./...` when adapting the receiver.
