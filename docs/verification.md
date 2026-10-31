# Verification evidence

Date: 2026-10-31. All events were synthetic; no Aurora code, accounts, network
policy or power settings were changed. No real transcript or model inference
was used. The host was macOS arm64 with Go 1.27.1 and Python 3.9.6.

## Go checks and actual receiver process

Passed `go test -race ./...`, `go vet ./...`, and
`go build -o bin/aurora-webhook ./cmd/aurora-webhook` on the host.
The regression suite exercises authentication/freshness/header boundaries,
body/storage limits, finite dedupe, durable claim/recovery, test suppression,
result retention/deletion, permissions, output bounds and subprocess cancellation.
The initial run exposed an empty-data-directory validation error; it was fixed
before the passing run. Temporary test directories were also corrected to
exercise private data-directory creation rather than rely on temp-root modes.

A throwaway driver launched the compiled receiver on loopback, sent signed
HTTP events and exercised the actual administrative CLI. Observed:

- Valid test and real event acceptance; invalid HMAC, stale timestamp and wrong
  bearer rejection; over-8-MiB declared body rejection; same-id/body conflict.
- Duplicate suppression across a full process restart; explicit raw retrieval;
  payload deletion and retained tombstone suppression.
- A clearly labeled synthetic executable (not Codex inference) received the
  real-event JSON through stdin without transcript argv or webhook credentials.
  Test events did not invoke it. Successful output was retained and retrievable;
  deleting the event also removed its retrievable result.
- SIGKILL after worker start followed by restart produced `needs_review`, with
  no automatic second invocation. Retry without acknowledgement failed;
  explicit duplicate-risk acknowledgement returned the job to pending.

The driver, ephemeral database and process-double scaffolding are not shipped.
These checks prove the real receiver/process protocol with synthetic inputs,
not real provider behavior or Aurora app delivery.

Release builds also succeeded with `-trimpath` for macOS/Linux, arm64/amd64.
Only the native host received process-level verification; cross-compilation
does not establish runtime behavior on the other targets. Release checksums
are published separately from the unsigned binaries.

Hosted CI was configured for macOS and Linux, but the
[initial run](https://github.com/ComicBit/aurora-webhook-blueprint/actions/runs/41219953078)
was blocked by account-level runner admission before any steps executed.
It is not a passing CI run; Linux runtime checks remain unverified. Resolve
the account restriction and rerun the workflow before relying on hosted gates.

## Synthetic delivery helper

The actual `scripts/send-synthetic.py` process was exercised against an owned
loopback HTTP endpoint that independently verified its HMAC and synthetic test
body. Correct signed delivery returned 204. A redirect response was not followed.
Nonloopback HTTP and credential-bearing URL inputs were rejected before a
request. This proves the helper path, not private TLS or app compatibility.

## Source review and onboarding

Two independent read-only reviews covered receiver/storage/HTTP/CLI and
runtime/onboarding/documentation. Neither reported actionable findings in its
reviewed source. Source review is not a security audit or runtime proof.

The skill had isolated behavioral checks for an existing VPN with fully local
requirements, a sleeping host/Hermes request with limited consent, and an
unrelated Stripe near miss, plus a no-skill baseline. See
[skill evaluation](skill-evaluation.md) for observations and comparison limits.
Local resource checks confirmed frontmatter/name consistency and referenced
README, payload, networking, agent and security documents exist. The Python
sender compiled and the bundled fixture was confirmed synthetic.

## Not verified

- Actual Aurora Send test or meeting delivery on iOS or macOS.
- Active private VPN routing, HTTPS trust, Tailscale Serve, grants, reboot or
  unattended host behavior. No Tailscale identity or policy was changed.
- Real Claude, Codex or Hermes inference/authentication, response semantics,
  provider retention, or destination integration. CLI interfaces were researched
  and safe help/version inspected; process doubles do not establish model behavior.
- Hermes backend interruption/settlement. Accepted gateway/provider work may
  outlive a killed local CLI; reconcile before retrying.
- Fully local/offline model execution, downstream exactly-once effects,
  prompt-injection immunity, compliance certification, backup restoration or
  platform-independent filesystem isolation.

These are user deployment checks, not implied by passing local tests. Begin
with synthetic Send test and a deliberate manual send before automatic export;
keep Aurora changes for the separate release work in [payload notes](payload.md).
