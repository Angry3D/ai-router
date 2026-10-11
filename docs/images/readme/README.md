# README screenshot sources

<!-- provenance:readme-product-screenshots -->

This directory holds the AI Router product screenshots used by the root README. The AI Router UI
composition in the screenshots is project-owned content released under the root MIT License; the
Lucide icons rendered in the UI remain subject to the Lucide ISC License.

## Capture record

- Capture dates: 2026-08-20 to 2026-08-21
- Source version: `9dd77e99df4a5a5845a525a406a21e3b706b7f3c`
- App identity: `AI Router QA.app` / `com.relax.airouter.qa`
- Menu view: `384 x 364` pixels
- Route view: `920 x 640` pixels

The menu images come from an isolated QA run on 2026-08-20; the route images come from two
isolated QA runs on 2026-08-21. Every run used a fresh temporary QA data directory, the repository
fixture listening only on IPv4 loopback, four synthetic routes named `Synthetic A` through
`Synthetic D`, and 320 synthetic request records. The route screenshots are consistent in theme,
window geometry, route names, and ordering; because they come from independent runs, the temporary
loopback port differs between frames. The settings button in the dark menu screenshot is in a
hover state, and the light menu has no such state; that difference only affects the button
background and does not change app state or data.

The capture process only launched the QA bundle; it did not operate, quit, restart, or read the
production `AI Router.app`. After capture finished, the QA processes and fixture were stopped, and
the temporary data directories were deleted through the repository's QA identity validator.

## Pre-release checks

Every image passed the following checks:

- Reviewing the full frame to confirm it contains only the AI Router QA window and synthetic data;
- Checking for sensitive information such as API keys, Authorization headers, user paths, production
  route names, and real request content;
- Re-encoding as PNG and checking the file type, dimensions, alpha behavior, the Display P3 color
  profile, and PNG metadata;
- Computing SHA-256 over the final bytes and binding it through `scripts/license-policy.json`.

Final files:

| File                       | SHA-256                                                            |
| -------------------------- | ------------------------------------------------------------------ |
| `menu-overview-light.png`  | `13fd3aed0e73babaf668c1df8615398a62f0b9853ce0817f57b1b9b7325806a9` |
| `menu-overview-dark.png`   | `6d742befee5a4f5cbae058006a2eb621d89f7bd22c5ed76359f2e45aa8c62a6b` |
| `route-overview-light.png` | `4671029bbad6291584a8792ad86d2802833ccc05b34930d83aa051f52d83d9aa` |
| `route-overview-dark.png`  | `65fca16ffdeb0c10c9bcc5fdf50f3695cac9e9b3eff9769e1252125e4819209b` |

Lucide upstream information and the full ISC license text are in the
[third-party notices](../../../THIRD_PARTY_NOTICES.md) and
[`third-party/licenses/ISC-Lucide.txt`](../../../third-party/licenses/ISC-Lucide.txt).
