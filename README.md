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
  <a href="https://auroraisyours.com/">Explore Aurora</a> ·
  <a href="https://auroraisyours.com/#waitlist">Join the waitlist</a> ·
  <a href="#get-started">Get started</a> ·
  <a href="#documentation">Documentation</a>
</p>

<p align="center">
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/Rust-source%200.2.0-386650" alt="Rust source 0.2.0"></a>
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

Open the **[copyable setup prompt](prompts/connect-aurora.md)** and give it to
your coding agent. It points to the **[Aurora onboarding skill](skills/connect-aurora/SKILL.md)**
and guides setup around your actual goal: what to produce, where it should go,
which runtime you approve, and what data may leave the host.

Already use a trusted VPN? Keep it. If you need private connectivity, the guide
recommends Tailscale **Serve**—not a public tunnel.

### Set it up yourself

Use the **[getting-started guide](docs/getting-started.md)** for the complete
signed, database-only setup, a synthetic delivery check and an optional folder
destination.

Build the current **Rust 0.2.0 source** with Rust 1.98 and a native C toolchain
(SQLite is bundled):

```sh
git clone https://github.com/ComicBit/aurora-webhook-blueprint.git
cd aurora-webhook-blueprint
cargo build --release
cp config.example.json config.local.json
target/release/aurora-webhook --config config.local.json plan
target/release/aurora-webhook --config config.local.json apply --approve
```

Then supply your signing secret and start `serve` as described in the guide.
`plan` only reviews configuration; `apply` initializes or activates it with the
receiver stopped. Neither sends historical meetings.

**Prefer a binary?** The [Rust 0.2.0 preview](https://github.com/ComicBit/aurora-webhook-blueprint/releases/tag/v0.2.0)
includes a native-tested **macOS arm64** bundle and checksums. It is unsigned;
see the [prebuilt setup](docs/getting-started.md#prebuilt-macos-arm64-preview).
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

## Project status

Version **0.2.0 is the Rust source cutover**: a signed SQLite inbox, independent
destinations and explicit offline administration. It is a self-hosted blueprint,
not a stable SDK, managed service or multiuser application.

Native macOS arm64 verification passed **38 regression tests and 18 compiled
receiver/process scenarios**, plus strict Clippy. The Rust macOS/Linux CI
workflow is configured, not a passing hosted run. See the
[verification record](docs/verification.md) for exact scope and limits.
**Real agent/provider runs, actual Aurora device delivery and Linux runtime
behavior remain unverified.**

This repository does not modify Aurora. App integration and release gates
remain separate in the [payload notes](docs/payload.md#future-app-release-gates-not-implemented-here).

## License

The blueprint is [MIT licensed](LICENSE). See the [changelog](CHANGELOG.md) for
release history. Aurora's linked brand artwork is not included in the code's
MIT license grant.
