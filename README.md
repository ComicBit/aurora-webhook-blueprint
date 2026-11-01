<p align="center">
  <a href="https://auroraisyours.com/">
    <img src="https://auroraisyours.com/assets/brand/og-aurora-2026-10.jpg" alt="Aurora — on-device AI meeting notes" width="760">
  </a>
</p>

<h1 align="center">Aurora Webhook Blueprint</h1>

<p align="center">
  <strong>Your meeting notes. Your next step.</strong><br>
  Connect Aurora to a private inbox and the agent workflow you choose.
</p>

<p align="center">
  <a href="prompts/connect-aurora.md"><img src="https://img.shields.io/badge/Copy_agent_setup_prompt-386650?style=for-the-badge" alt="Copy agent setup prompt"></a>
  <a href="docs/getting-started.md"><img src="https://img.shields.io/badge/Set_it_up_yourself-252525?style=for-the-badge&amp;logo=github&amp;logoColor=white" alt="Set it up yourself"></a>
  <a href="https://auroraisyours.com/"><img src="https://img.shields.io/badge/Discover_Aurora-EEEAE2?style=for-the-badge&amp;logoColor=252525" alt="Discover Aurora"></a>
</p>

<p align="center">
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/built_with-Rust-386650?logo=rust" alt="Built with Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-386650" alt="MIT license"></a>
</p>

## Take your Aurora notes further

[Aurora](https://auroraisyours.com/) transcribes and summarizes meetings,
interviews and lectures on your iPhone. No meeting bot, no Aurora account;
on-device processing works offline after model setup.

This open-source blueprint is the next part of that workflow: a small,
self-hosted receiver for the transcripts, summaries and action items you choose
to export. Start with a private inbox. Add agent processing only when you know
what you want it to do and where the data may go. **No audio is sent.**

> **Looking for the app?** Aurora's iPhone beta is available by personal
> TestFlight invitation. [Join the waitlist](https://auroraisyours.com/#waitlist)
> or read [how invitations work](https://auroraisyours.com/beta/).
> The Mac app is in development; there is no public TestFlight install link yet.

## What you can build

- **A private meeting inbox.** Keep selected exports on your own host, inspect
  the original JSON and decide what to retain. No model account required.
- **A folder of portable notes.** Save exact JSON and literal Markdown with
  verifiable receipts—without asking a model to rewrite your meeting.
- **Digests and task proposals.** Opt into Claude Code, Codex or Hermes, or
  hand off to an approved Hermes Runs endpoint or existing Grok Bot routine.
  Choose a fresh task or an explicitly supported existing target; review its
  access before sharing content.

The default is **inbox-only**. You do not need a model account to get started.

## Get started

### Let your coding agent help

Tell your agent what you want to happen after a meeting: save notes to a folder,
prepare a digest, or bring the context into a workflow you already use.

<p>
  <a href="prompts/connect-aurora.md"><img src="https://img.shields.io/badge/Open_%26_copy_the_setup_prompt-386650?style=for-the-badge" alt="Open and copy the setup prompt"></a>
  <a href="skills/connect-aurora/SKILL.md"><img src="https://img.shields.io/badge/Read_the_agent_skill-EEEAE2?style=for-the-badge" alt="Read the agent skill"></a>
</p>

Open the prompt, use the copy button on its text block, and paste it into your
coding agent. It will help you choose the destination, set up the receiver and
check a fictional meeting before you share anything real.

### Keep the connection private

Already have a trusted VPN? Keep it. Otherwise, **Tailscale Serve** gives your
receiver a private HTTPS address without opening it to the public internet.
Your receiving host needs to stay awake and connected.

<p>
  <a href="https://tailscale.com/"><img src="https://img.shields.io/badge/Meet_Tailscale-242424?style=for-the-badge&amp;logo=tailscale&amp;logoColor=white" alt="Meet Tailscale"></a>
  <a href="docs/networking.md"><img src="https://img.shields.io/badge/Private_connection_guide-EEEAE2?style=for-the-badge" alt="Private connection guide"></a>
</p>

Use Serve, not Funnel: this setup is for private access. The
[networking guide](docs/networking.md) walks through the choices.

### Set it up yourself

Use the **[getting-started guide](docs/getting-started.md)** for the complete
signed, database-only setup, a synthetic delivery check and an optional folder
destination.

Build the current **Rust 0.2.1 source** with Rust 1.98 and a native C toolchain
(SQLite is bundled):

```sh
git clone https://github.com/ComicBit/aurora-webhook-blueprint.git
cd aurora-webhook-blueprint
cargo build --release
cp config.example.json config.local.json
target/release/aurora-webhook --config config.local.json plan
target/release/aurora-webhook --config config.local.json apply --approve
```

Then copy Aurora's signing secret into a protected local `.env`, generate a
separate bearer token, and start `serve` as described in the guide.
`plan` only reviews configuration; `apply` initializes or activates it with the
receiver stopped. Neither sends historical meetings.

**Prefer a binary?** The [Rust 0.2.1 preview](https://github.com/ComicBit/aurora-webhook-blueprint/releases/tag/v0.2.1)
includes the native-tested macOS arm64 CLI and setup resources.
The older [0.2.0 macOS arm64 preview](https://github.com/ComicBit/aurora-webhook-blueprint/releases/tag/v0.2.0)
does **not** load `.env` or provide `token-gen`. See the
[prebuilt setup](docs/getting-started.md#prebuilt-macos-arm64-preview).
Existing [v0.1.0 downloads](https://github.com/ComicBit/aurora-webhook-blueprint/releases/tag/v0.1.0)
are the earlier Go receiver. Do not mix their configuration or commands;
[migrate retained history explicitly](docs/getting-started.md#migrate-from-the-go-010-inbox).

## How it works

1. **Choose the content in Aurora:** transcript, summary and action items, or
   both. Use an Aurora build that exposes webhook settings.
2. **Send over private HTTPS:** through your existing VPN or an approved
   Tailscale Serve endpoint, to the receiver on your own host.
3. **Save before acknowledging:** signature verification precedes JSON parsing;
   the receiver atomically commits the exact event and selected destination
   intents in SQLite before returning success. Same-id/same-byte deliveries
   create no new work or retention deadline; changed bytes with that id conflict.
4. **Deliver independently:** each enabled destination has its own durable
   state. Folder exports and bounded agent results are reviewable; a remote
   handoff reports admission, not a finished task.

A connectivity-test event never writes a folder, starts an agent or creates
backfill work. Ambiguous process/network failures are held for review, not
blindly retried. Adding a destination affects new events only; historical
delivery needs a separate, frozen preview and explicit backfill approval.

## Private by default. Processing by choice.

Aurora's on-device notes and this receiver's downstream processing are separate
privacy decisions. **Running an agent on your computer does not make its
inference local.** Approve the provider, content and retention before enabling it.

The receiver binds only to loopback. Its SQLite inbox has configurable logical
storage limits and seven-day payload/result retention by default; finite
deduplication metadata remains for another 30 days. Folder copies, CLI profiles,
provider histories and backups have separate retention and deletion policies.

Your host must stay awake and connected. Hermes task wake-up starts work on a
running host; it does not wake a sleeping computer. Private ingress is not an
offline transport, and prompt instructions are not a security boundary. Read
[security and operations](docs/security.md) before using real meeting content.

## Documentation

| I want to… | Read |
| --- | --- |
| Run the inbox or export to a folder | [Getting started](docs/getting-started.md) |
| Set up private HTTPS with my VPN or Tailscale | [Private networking](docs/networking.md) |
| Choose an agent operation and review its authority | [Agent guide](docs/agents.md) |
| Understand the event fields and their limits | [Payload contract](docs/payload.md) |
| Plan storage, consent, deletion and recovery | [Security and operations](docs/security.md) |
| Check what has actually been exercised | [Verification evidence](docs/verification.md) |

## License

The blueprint is [MIT licensed](LICENSE). See the [changelog](CHANGELOG.md) for
release history. Aurora's linked brand artwork is not included in the code's
MIT license grant.
