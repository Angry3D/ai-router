# AI Router Icons

<!-- provenance:lucide-icons -->

`app-icon.svg` is the production source and `app-icon-qa.svg` adds the static
QA corner badge. Both use the same Route geometry as `tray-route.svg`.

The Route, TriangleAlert, and CircleX geometry comes from Lucide
`lucide-react 0.468.0` under the ISC license. AI Router changes scale, stroke,
color, and composition; the application backgrounds and QA badge are
project-authored. The upstream copyright and license text is retained in
`third-party/licenses/ISC-Lucide.txt` and indexed by
`THIRD_PARTY_NOTICES.md`.

Run `pnpm icons:generate` after changing an icon source. It updates the tracked
macOS bundle assets (`.icns`) and 512 px PNG review previews. Tauri bundle
configuration consumes the `.icns` files directly. The command also refreshes
`icon.png`: Tauri's compile-time application context requires that production
default PNG even though the macOS bundle uses `app-icon.icns`.
The command delegates to `scripts/generate-app-icons.mjs`, which is the
versioned source-to-output generation chain.
When an SVG produces the same PNG as the tracked preview, the generator keeps
the existing valid `.icns` instead of introducing binary-only churn.

The `tray-*.svg` and `tray-*.png` files are separate macOS template assets.
`tray-active-a` through `tray-active-d` and `tray-active-static` use the same
Route geometry plus the approved radius-3 activity bead. The moving frames
advance from the solid head through two middle beads to the solid tail every
300 ms; the static center bead is shared by Waiting, Reduce Motion, and the
disabled-animation projection.
`pnpm icons:generate` rebuilds these 44 px PNGs from their SVG sources on macOS.
Do not substitute the coloured application icon for `tray-route.png`: the
native status item requires transparent, monochrome template pixels.
