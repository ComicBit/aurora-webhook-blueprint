# README presentation research

Observed **2026-10-31**. This is presentation research, not evidence that popularity causes good documentation.

## Reproducible sample

[GitHub REST search query](https://api.github.com/search/repositories?q=stars:%3E1+fork:false&sort=stars&order=desc&per_page=10):

```text
GET https://api.github.com/search/repositories?q=stars:%3E1+fork:false&sort=stars&order=desc&per_page=10
```

The response reported `total_count: 12624479` and `incomplete_results: false`; all ten returned repositories had `fork: false`. Ranks below are **positions in this query response**, not a certified all-time/global leaderboard. The unauthenticated public search excludes inaccessible repositories; GitHub documents search scope limits, a maximum of 1,000 returned results, and possible timeouts. `incomplete_results: false` is not a guarantee of exhaustive global coverage. Counts, ownership and default-branch README contents can change. See [GitHub's search documentation](https://docs.github.com/en/rest/search/search#about-search).

Exactly ten root READMEs were retrieved through `GET /repos/{owner}/{repo}/readme`; their opening presentation and navigation were inspected directly. No ranking article or README template was used as primary evidence.

| Query rank | Repository / inspected root README | Stars at snapshot | Kind | Concrete source-grounded pattern |
| --- | --- | ---: | --- | --- |
| 1 | [codecrafters-io/build-your-own-x](https://github.com/codecrafters-io/build-your-own-x/blob/master/README.md#build-your-own-insert-technology-here) | 557,833 | Educational tutorial index | A linked brand banner precedes a short explanation of the learning benefit, then topic jump links. Borrow the immediate proposition and navigation, not the large catalog. |
| 2 | [sindresorhus/awesome](https://github.com/sindresorhus/awesome/blob/main/readme.md) | 527,871 | Curated index of lists | A centered identity block includes a named macOS app, direct product link and benefit line; contribution/navigation links and the contents are separate. Product placement can be explicit, but Aurora needs less promotional vertical space. |
| 3 | [public-apis/public-apis](https://github.com/public-apis/public-apis/blob/master/README.md) | 493,738 | API directory with product promotion | The opening names concrete API jobs, links a product, then offers sign-up and a runnable Postman route. Borrow concrete jobs and actionable routes, not the advertisement-heavy lead-in. |
| 4 | [freeCodeCamp/freeCodeCamp](https://github.com/freeCodeCamp/freeCodeCamp/blob/main/README.md#freecodecamporgs-open-source-codebase-and-curriculum) | 458,300 | Learning platform and curriculum | A linked product banner and audience/value explanation come before the table of contents. Learning routes appear before bug reporting, security and contributing: users are not treated as maintainers first. |
| 5 | [morluto/rea](https://github.com/morluto/rea/blob/main/README.md#rea-reverse-engineer-anything) | 414,672 | Agent tooling product | A descriptive title and benefit lead into Website / Guides / Showcases, section links and a setup command. A plain-language scenario explains why the tools matter before the detailed quick start. |
| 6 | [EbookFoundation/free-programming-books](https://github.com/EbookFoundation/free-programming-books/blob/main/README.md) | 400,609 | Educational resource index | Search and an easier-to-read website are offered near the top; contributing is a separate section. Give readers a route suited to their goal rather than forcing everyone through source-level setup. |
| 7 | [openclaw/openclaw](https://github.com/openclaw/openclaw/blob/main/README.md#install) | 393,541 | Assistant product | The opening states the user-facing assistant proposition and links Website / Docs / Getting started. Install and onboarding precede “How it fits together,” which links architectural concepts to deeper docs. |
| 8 | [donnemartin/system-design-primer](https://github.com/donnemartin/system-design-primer/blob/master/README.md#motivation) | 376,315 | Educational guide | “Motivation” names two outcomes, learning system design and interview preparation; study guides and exercises have explicit routes. Explain the reader's outcome before the implementation inventory. |
| 9 | [nilbuild/developer-roadmap](https://github.com/nilbuild/developer-roadmap/blob/master/readme.md) | 370,875 | Educational platform and roadmap index | The logo and product name both link to roadmap.sh; a short proposition and category links lead to beginner/specialist paths, including a “get started” choice helper. Brand and route selection work together. |
| 10 | [jwasham/coding-interview-university](https://github.com/jwasham/coding-interview-university/blob/main/README.md#what-is-it) | 363,625 | Educational study plan | “What is it?” states the audience, prerequisites and scope exclusions; translation lists are collapsed with `details`. Keep important boundaries visible and secondary material out of the main reading path. |

## Decisions for Aurora's webhook blueprint

These are recommendations, not claims of features verified by this research. Most of the sample consists of educational indexes; their enormous lists are the product itself. This repository needs a product-led integration README, with the strongest structural parallels in REA, OpenClaw and freeCodeCamp.

1. **Lead with Aurora, then identify this repository.** Use a concise brand proposition explaining the user benefit, followed immediately by what the webhook blueprint adds. Put a verified Aurora website link in the hero, separate from technical documentation. Avoid an unexplained wall of badges.
2. **Show useful outcomes before configuration.** Give a few concrete scenarios such as retaining meeting exports in a private inbox and optionally generating proposed action items. Label scenarios as uses of the blueprint, not demonstrated end-to-end integrations.
3. **Offer distinct getting-started routes.** Make agent-guided onboarding and manual setup discoverable; link the actual onboarding skill/prompt rather than pasting every operational instruction into the introduction. Separate obtaining Aurora from deploying this receiver. A future TestFlight invitation should remain plain availability text until a real public URL is supplied; publish no invented TestFlight or App Store link.
4. **Make the capability boundary legible.** Explain that the default is a durable signed inbox with no model call, while Claude/Codex/Hermes processing is restricted and opt-in ([agent behavior](agents.md#default-durable-inbox-no-model-call)). Do not equate running a CLI locally with offline inference or promise unattended external actions.
5. **Keep status truthful and compact.** Distinguish available receiver/release artifacts from unverified real Aurora delivery, real model inference and Linux runtime behavior. The recorded hosted CI run did not execute; a green CI badge would mislead ([verification evidence](verification.md#not-verified), [hosted CI limitation](verification.md#go-checks-and-actual-receiver-process)).
6. **Progressively disclose operational detail.** After the proposition, use cases, start routes and status, link to [payload](payload.md), [networking](networking.md), [security](security.md), [agent configuration](agents.md) and [verification](verification.md). Keep signing, retention, permissions and deployment caveats accessible without making them the opening pitch.
7. **Create an original presentation.** Reuse information hierarchy, not other projects' prose, artwork, testimonials, numerical success claims or badges. Any visual should use only assets approved for this public repository; private Aurora source, assets and history are out of scope.

## Verified Aurora references

- [Official website](https://auroraisyours.com/): on-device transcription and
  summaries on iPhone; the Mac app is in development.
- [Beta invitation policy](https://auroraisyours.com/beta/): private TestFlight
  invitations sent in waves; no public enrollment link. The appropriate public
  call to action is [join the waitlist](https://auroraisyours.com/#waitlist),
  not an install or App Store button.
- The homepage publishes its
  [Aurora social banner](https://auroraisyours.com/assets/brand/og-aurora-2026-10.jpg)
  as the public promotional image. The README references that hosted image and
  links it to Aurora; it does not import private artwork or extend the
  blueprint's MIT license to the product branding.
