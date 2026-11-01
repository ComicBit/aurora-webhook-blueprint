# Connect my Aurora meetings to my workflow

Copy the text below into your coding agent using the copy button at the top-right
of the block. Tell it what you want your meeting notes to become.

```text
Use the public Aurora webhook blueprint:
https://github.com/ComicBit/aurora-webhook-blueprint

Read `skills/connect-aurora/SKILL.md` and the relevant kit resources. Help me
implement and verify my intended result; continue through authorized local
work, not just a plan. Do not modify Aurora, create another repository, publish
my data or open issues unless I separately request it.

Use facts I already supplied. Ask only meaningful missing decisions, a small
related group at a time: what I want to produce and where it should live;
review-only versus actions; participant/data authorization and minimum export
scope; local-only versus approved provider disclosure; actual target authority;
retention/backups; and the receiving host/private connectivity. Don't quiz me
on database internals, queues, flags or choices the kit already makes safely.

Start with the current Rust 0.2.1 source, signed database-only receipt and
`destinations: []`, no model account or latent jobs. Offer a private folder
export if original JSON/literal Markdown meets my goal without inference.
Keep historical Go 0.1.0 release binaries and their migration separate. Use
`plan`, then offline `apply --approve`; editing a file alone is not activation.
`doctor` checks local prerequisites without network/auth/model calls and is
not proof of a real provider or policy boundary.

Explain signing (exact-body authenticity/integrity and freshness) versus bearer
(shared authorization), and recommend both. This receiver requires signing;
if I decline it, explain the limitation rather than silently disabling it.
Aurora generates the signing secret: guide Generate only if missing, then Copy
from the app into protected capture. Never substitute receiver `keygen`.
Generate a separate bearer with `token-gen`; with my approval store both in
ignored private 0600 `.env` as AURORA_WEBHOOK_SECRET and AURORA_BEARER_TOKEN.
Use protected masked input/catalog where available or a trusted local editor.
Never echo either secret into chat, argv, Git or logs. I paste ONLY bearer into
Aurora Authentication > Bearer. Explain clipboard/editor-backup, OS, same-user,
backup and synced-folder risks.

In 0.2.1 serve/doctor load .env beside the selected config; --env-file PATH
selects an explicit required file. Inherited process values override it and a
restart reloads it. Other commands do not load .env; only key generators are
independent of configuration too.
Old 0.2.0 binaries do not implement this feature; use 0.2.1 source or its preview.
The Python synthetic sender uses process environment only: securely capture
its values without shell-sourcing .env or inventing a secret-capture tool.

Enable only a runtime, provider, operation and exact destination I approve.
Separate fresh tasks, saved-session continuation and existing remote targets:
`continue` is not a generic live-chat notification. Review the selected
session/routine's broader tools, participants and credentials before sharing
content. A local CLI does not imply local inference, and a proposal-only prompt
does not isolate server-owned authority or solve prompt injection. Don't invent
owners, due dates, integrations, credentials or unsupported bridges/fallbacks.

Reuse my trusted VPN if it provides private HTTPS DNS/TLS. If none exists,
explain Tailscale Serve (private encrypted overlay, not offline) and get approval
for installation/sign-in/HTTPS/grants. Never use Funnel, a public tunnel/bind
or weaker Aurora URL checks. The host must already be awake/reachable; starting
agent work is not OS wake-on-LAN. Don't change service/power policy without
permission, but continue the local work I have authorized.
Inspect existing Serve routes; never overwrite them. Select an explicit free
HTTPS port if the default is occupied. Use the full reported private HTTPS
hostname, port and /webhooks/aurora path. Distinguish GET /healthz from POST
/webhooks/aurora: diagnose 405 as route/method and 401 on the correct POST as
credential/timestamp checks, without guessing URLs or disabling protections.

New routes must not silently send my retained meetings. If I want history,
establish the receipt-time interval, latest-per-meeting versus all-revisions
choice, and target disclosure/action consent. Create and inspect a private
frozen backfill preview, then obtain approval of its exact selection before
applying it. Never reset retention or resurrect deleted/expired content. Treat
Go migration as held history with zero jobs; preserve backups and reconcile
old effects before any historical delivery.

Use fictional signed delivery checks first. Connectivity tests must cause zero
outward work, including folder writes, model/remote calls and backfill. A local
folder or approved provider check uses a fictional non-test event; don't run a
model merely to test connectivity. Verify duplicate/conflict, durable restart,
output, retention and reviewable ambiguous failures. Explicit repeat requires
reconciliation and duplicate-risk acknowledgement, not blind retries.

Only when device/network use is authorized, guide actual Send test and one
deliberate fictional meeting before optional automatic export. Don't claim
app/VPN compatibility from a hostname validator or loopback test. Report exactly
what ran, distinguish doubles from real models, and name unperformed platform,
provider and device checks. Do not send real content or change private-network
policy without my explicit approval. Preserve Aurora's invitation-only beta
caveat and website/waitlist links; do not invent public install availability.
```
