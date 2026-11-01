# Verification evidence

Evidence is version-specific. **Current Rust 0.2.1 source is not the historical
0.2.0 Rust or Go 0.1.0 binary.** Each record retains its own scope. A configured
workflow, local preflight or process double does not establish a passing hosted
build, actual model behavior or device delivery.

## Rust 0.2.1 credential verification

Native checks ran on **macOS arm64**, with Rust/Cargo 1.98.1:

- `cargo test --locked`: **47 passed**—the original 38 regressions plus nine
  isolated subprocess contracts for dotenv selection, independent signing/
  bearer enforcement, precedence, generator independence, admin availability,
  file security, redacted errors and non-execution of shell substitutions.
- Strict Clippy (`--all-targets -- -D warnings`), Rustfmt and the optimized
  release build passed.
- **Nine compiled-binary credential scenarios** independently exercised the
  generated bearer, config-adjacent and explicit files, both required
  credentials, restart-only reload, retained deduplication, inherited overrides,
  absent explicit/implicit files, unsafe-file rejection and error redaction.
  These used disposable credentials and fictional connectivity events.
- The revised skill and isolated previous revision were compared across four
  scenarios each. Local resource links and the single copyable prompt block
  were checked; see [skill evaluation](skill-evaluation.md).

An authorized private Mac deployment was upgraded without changing its approved
destination configuration or existing Tailscale Serve routes. A private inbox
backup preceded the upgrade. Both credentials were loaded from an ignored
owned 0600 `.env`, not a service environment export. HTTPS certificate
verification remained enabled. A fictional signed connectivity request with
the correct bearer returned **204**; a missing/wrong bearer or incorrect
signature returned **401**. Duplicate delivery retained one receipt and created
no outward work. The credential-sync/restart helper was also exercised.

This private HTTPS check is not an independent iOS/macOS app test, an audit of
tailnet grants or a production restore test. The user-reported app check below
predates enabling the new bearer credential; that app setting must be updated
before claiming the revised configuration works from the device. No new real
provider, Linux, other-architecture or hosted-CI qualification is implied.

## Rust 0.2.0 native verification

The Rust receiver was built and exercised on **macOS arm64 (Darwin 27.0.0)**,
using Rust/Cargo 1.98.1. All meeting content and credentials in the scenarios
were synthetic. Aurora, VPN/account configuration and real model providers
were not changed or invoked.

Passed:

- `cargo test --locked`: **38 regression tests**, including authentication,
  escaped/nullable/scoped payloads, literal export safety, durable fanout,
  quota counters, fair scheduling, pinned-reader WAL pressure, frozen history,
  migration results, retention fences and process-group cancellation.
- `cargo clippy --locked --all-targets -- -D warnings`, Rustfmt checks and the
  optimized native release build.
- **15 compiled receiver scenarios**: signed authentication before parsing,
  exact 8 MiB raw-body acceptance/oversize rejection, independent folder
  delivery despite another failed destination, duplicate/conflict handling,
  zero-outward tests, concurrent read-only status/exclusive mutation,
  deliberate new revisions, SIGKILL/restart persistence, consistent backup,
  deletion without erasing destination copies, no latent storage-only work,
  tamper-rejected explicit backfill, normalized JSONL, real-clock expiry and
  actual Go bbolt export/import as held history without new deliveries.
- **3 compiled receiver/process scenarios** using an explicitly synthetic
  executable double: bounded stdin/result transport with excluded webhook
  credentials; interrupted work becoming `needs_review` without resubmission;
  and actual listener file-descriptor exhaustion draining/reaping its active
  local task before error exit.

The process double is **not Claude inference or a runtime permission audit**.
SIGKILL cannot execute destructors; its orphan fixture process group was
explicitly removed during the crash scenario. These checks do not establish
hardware power-loss durability, native Linux behavior or remote completion.
Throwaway drivers/databases were removed rather than shipped as application
features. Permanent regression tests remain in the Rust modules.

Normal-profile Claude/Codex/Hermes argument/help invocations exited
successfully. With the receiver's cleared environment and empty private
profiles, Claude and Codex help also launched; the installed Hermes launcher
refused its unprovisioned dependency environment. No repair was performed.
Help is not authentication, effective-policy or live-model proof. Static
checks resolved the skill/prompt's nine local resources and parsed the default
configuration. The revised onboarding behavior was compared in two isolated
agents across four shared scenarios; see [skill evaluation](skill-evaluation.md).

The [CI workflow](../.github/workflows/ci.yml) targets Rust 1.98.0 on Ubuntu
and macOS. Its configuration and these **manual native results are not a
passing hosted CI result**. The prior account/billing runner-admission block
remains a known limitation; it was not rerun merely to reconfirm the failure.

## Go checks and actual receiver process

**Historical only — 2026-10-31, Go 0.1.0.** The host was macOS arm64 with Go
1.27.1 and Python 3.9.6. No Aurora code, accounts, network policy or power
settings were changed; no real transcript or model inference was used.

The earlier `go test -race ./...`, `go vet ./...` and receiver build passed.
Those commands refer to the old receiver tree, **not the current Rust build**.
The historical suite covered authentication/freshness/header boundaries,
body/storage limits, finite dedupe, durable claim/recovery, test suppression,
results/deletion, permissions, output bounds and subprocess cancellation.

A throwaway driver launched that compiled Go receiver and used signed HTTP
and its administrative CLI. Observed: valid test/fictional-event acceptance;
invalid HMAC/stale timestamp/wrong bearer rejection; oversized declared-body
rejection; same-id changed-body conflict; dedupe across restart; exact raw
retrieval; logical deletion/tombstone suppression; and synthetic executable
stdin transport with excluded webhook credentials, retained successful result
and no test-event invocation. SIGKILL after worker start followed by restart
produced `needs_review` without automatic second invocation. Retry required
duplicate-risk acknowledgement. The process double was **not Codex inference**.
The throwaway driver/database were not shipped.

Historical release cross-builds succeeded for macOS/Linux arm64/amd64. Only
native macOS arm64 had process-level verification; cross-compilation does not
prove Linux execution. Existing **v0.1.0 unsigned release artifacts/checksums**
are those Go builds, not Rust 0.2.0 distribution artifacts.

The [initial hosted run](https://github.com/ComicBit/aurora-webhook-blueprint/actions/runs/41219953078)
was blocked by account-level runner admission **before any step executed**.
It was not passing CI and provides no Linux runtime evidence. The Rust
workflow replaces those old Go gates but does not erase this evidence limit.

## Synthetic helper and onboarding history

In the historical check, the actual `scripts/send-synthetic.py` process sent to
an owned loopback endpoint that independently verified its HMAC/body. Correct
delivery returned 204; redirects were not followed, and nonloopback HTTP and
credential-bearing URLs were rejected. This is helper evidence, not real
private TLS, app delivery or a current Rust receiver process run.

Two independent read-only reviews covered the earlier Go receiver and its
runtime/onboarding docs; neither reported actionable findings in its reviewed
source. That is **not** an audit of the Rust cutover or the rewritten adapters.
The current onboarding entrypoint and references are available in this source
tree. Historical evaluations and the revised Rust comparison are recorded in
[skill evaluation](skill-evaluation.md), separately from runtime evidence.

## User-reported app check

The user reported a successful manual Aurora app test after correcting the
endpoint URL. The platform was not specified. This is a user report, not a
new independently measured iOS/macOS, VPN, provider or Linux qualification.

## Not independently verified

- Platform-specific Aurora Send test or meeting delivery on iOS and macOS.
- Device-side private VPN routing, effective tailnet grants, cold-boot/
  unattended supervision or host power behavior. The Mac-side private HTTPS
  checks above do not establish those properties.
- Real Claude/Codex/Hermes inference or authentication; actual new-task/saved-
  session/cloud follow-up behavior; effective tools/managed policy, provider
  persistence or real model result semantics.
- Live Hermes Runs or Grok Bot routine admission/completion, endpoint policy,
  target identity/authority, backend settlement or interruption. Accepted remote
  work may outlive a local process, and admission is not completed automation.
- Linux runtime behavior of Rust or an actual hosted Rust CI pass. Intended
  Unix targets are not a platform certification or multiuser support claim.
- Fully local/offline inference, downstream exactly-once effects,
  prompt-injection immunity, compliance certification, production backup
  restoration or platform-independent filesystem isolation.

Real provider/device/VPN checks were not authorized in the historical
implementation turn above and must not be inferred from those synthetic checks.
Use zero-outward Send test first, then a deliberate fictional meeting on the
approved route before real content/automatic export. Aurora changes remain
separate release work in the [payload notes](payload.md).
