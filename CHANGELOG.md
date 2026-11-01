# Changelog

## Unreleased

- Refreshed the README with prominent setup, Aurora and Tailscale links, a
  one-block copyable agent prompt, and a shorter introduction to private setup.
  Kept detailed qualification and release evidence in the documentation.

## 0.2.0 — Rust source cutover

- Replaced the receiver with Rust/Cargo and bundled SQLite WAL/FULL storage;
  exact raw receipt and independent selected destination intents commit before
  acknowledgement. Added bounded writer/connection/intake/dispatch capacity,
  finite deduplication, unchanged duplicate deadlines and per-destination state.
- Added reviewed `plan`/offline `apply --approve` activation, local-only
  `doctor`, concurrent read-only inspection, explicit duplicate-risk retry,
  logical deletion/expiry, consistent offline backup and normalized JSONL export.
- Added private folder destinations with exact JSON, literal Markdown and
  hash receipts published last without replacement; separate copy retention
  and matching-partial/conflict semantics are explicit.
- Added fixed Claude/Codex new-task and saved-history operations, Claude cloud
  follow-up, Hermes finite task/official Runs handoff and existing Grok Bot
  routine handoff. Disclosure and target authority require distinct approval;
  admission is not completion and ambiguous effects require review.
- Added immutable, bounded receipt-time backfill preview/approval with original
  expiry and configuration/target fingerprints. Route edits do not auto-process
  history; connectivity tests never cause any outward work.
- Added explicit offline Go 0.1.0 export/import as held history with no runnable
  jobs. Preserved original identity/deadlines and available raw/result/state
  history, with `legacy-result EVENT_ID` for retained old results; prior
  downstream effects still require reconciliation.
- Updated public onboarding, safe database-only config and staged folder/agent
  consent; retained Aurora's actual banner, website/waitlist and invitation-only
  beta caveat. No Aurora application changes or public install links were added.
- Replaced hosted Go gates with a Rustfmt/Clippy/test/release-build workflow for
  Ubuntu/macOS. Workflow configuration is not a passing run. Native verification
  is recorded separately in [verification evidence](docs/verification.md);
  real-provider, app/device, VPN and Linux runtime checks remain unverified.

The Rust 0.2.0 GitHub release is an **unsigned macOS arm64 preview**, qualified
by 38 native regression tests and 18 compiled receiver/process scenarios.
Existing 0.1.0 binaries remain the Go receiver. Real providers/device delivery
and native Linux remain unverified; this is not a stable SDK or multiuser claim.

## 0.1.0 — 2026-10-31

- Initial standalone private-ingress Aurora webhook inbox with durable delivery,
  authenticated raw-body verification, bounded storage, and deliberate worker
  dispatch.
- Onboarding skill and copyable prompt covering workflow intent, data consent,
  private networking, runtime selection and staged verification.
- Payload assessment and explicit deferred Aurora release gates. No Aurora
  application changes are included.
