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
  <a href="https://github.com/ComicBit/aurora-webhook-blueprint/releases"><img src="https://img.shields.io/github/v/release/ComicBit/aurora-webhook-blueprint?label=release&amp;color=386650" alt="Latest release"></a>
  <a href="go.mod"><img src="https://img.shields.io/badge/Go-1.25%2B-386650" alt="Go 1.25 or newer"></a>
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
  the original JSON and decide what to retain.
- **Digests and task proposals.** Opt into restricted Claude Code, Codex or
  Hermes processing; retrieve the result for review alongside the event.
- **A handoff to your own tools.** Use the documented payload as a starting
  point for a notes, knowledge-base or task workflow. You choose and implement
  the destination—this kit does not silently write to a CRM or message a team.

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

Use the **[getting-started guide](docs/getting-started.md)** for signing-secret
setup, a synthetic delivery check and the private Aurora connection.

To build from source:

```sh
git clone https://github.com/ComicBit/aurora-webhook-blueprint.git
cd aurora-webhook-blueprint
go build -o bin/aurora-webhook ./cmd/aurora-webhook
cp config.example.json config.local.json
```

Prefer a binary? **[Download a release](https://github.com/ComicBit/aurora-webhook-blueprint/releases)**
for macOS or Linux, arm64 or amd64. Releases include checksums; binaries are
unsigned. Follow the setup guide before starting the receiver.

## How it works

1. **Choose the content in Aurora:** transcript, summary and action items, or
   both. Use an Aurora build that exposes webhook settings.
2. **Send over private HTTPS:** through your existing VPN or an approved
   Tailscale Serve endpoint, to the receiver on your own host.
3. **Save before acknowledging:** the receiver checks the signature and commits
   the event durably. Repeated delivery of the same id and body is deduplicated.
4. **Process only if enabled:** an approved agent receives the event over stdin.
   Its bounded result stays with the event for explicit retrieval and review.

A connectivity-test event never starts an agent. Failed or interrupted work is
held for review rather than automatically rerun.

## Private by default. Processing by choice.

Aurora's on-device notes and this receiver's downstream processing are separate
privacy decisions. **Running an agent on your computer does not make its
inference local.** Approve the provider, content and retention before enabling it.

The receiver binds only to loopback and verifies signed requests before
interpreting JSON. It stores events in a local transactional database, with
configurable limits and retention. The default retention is seven days.

Your host must stay awake and connected. Hermes task wake-up starts work on a
running host; it does not wake a sleeping computer. Private ingress is not an
offline transport, and prompt instructions are not a security boundary. Read
[security and operations](docs/security.md) before using real meeting content.

## Documentation

| I want to… | Read |
| --- | --- |
| Run the inbox and send my first test | [Getting started](docs/getting-started.md) |
| Set up private HTTPS with my VPN or Tailscale | [Private networking](docs/networking.md) |
| Configure Claude Code, Codex or Hermes | [Agent guide](docs/agents.md) |
| Understand the event fields and their limits | [Payload contract](docs/payload.md) |
| Plan storage, consent, deletion and recovery | [Security and operations](docs/security.md) |
| Check what has actually been exercised | [Verification evidence](docs/verification.md) |

## Project status

The receiver, onboarding kit and release binaries are available for testing.
Local race-enabled tests, vet, build and signed synthetic process checks passed.
**Real Aurora delivery over a VPN and real model runs remain unverified.**
Hosted macOS/Linux CI could not start because of an account-level runner
restriction; Linux runtime checks are not yet verified. See the
[verification record](docs/verification.md) for the exact scope and limits.

This repository does not modify Aurora. App settings UX changes and a future
copy-prompt button belong to a later app release, tracked in the
[release notes for app integration](docs/payload.md#future-app-release-gates-not-implemented-here).

## License

The blueprint is [MIT licensed](LICENSE). See the [changelog](CHANGELOG.md) for
release history. Aurora's linked brand artwork is not included in the code's
MIT license grant.
