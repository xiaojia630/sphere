# Sphere app screens: shared brief (design prototypes)

Gabriel approved four HUD screens (all in /mnt/project-files/sphere-hud/, see README.md there): v2 home, Core open (app orbit), Working, Agent moment. Now we design the APP screens that open from the Core open orbit tiles. These are web design prototypes (one self-contained HTML each), not the Rust app. He reviews them in the morning; quality bar is very high ("actually pretty", cinematic, JARVIS-like, nothing static, everything floats / zooms / moves). Desktop only: no phone layout.

## Style (do not drift)
- Built from a COPY of /mnt/project-files/sphere-hud/core-open/index.html (it already contains v2 + the orbit). Use the same build pattern as core-open/src/build.py and working/src/build.py: a build.py that applies assert-unique anchored patches to a base copy plus your own fragment.html + app.js. Never edit the approved files in place.
- Balanced theme default: clear glass, thin cool blue-white lines, colour only for meaning (amber = needs you, green = done/ok, red = error). Keep the five themes (key C) working on your screen. Fonts and type scale as in v2. Mostly clear glass with thin lines and some filled elements; NOT heavy glow cards.
- Sounds: use the existing createSFX() calls (open/close/confirm/toast etc.) for your interactions; no new sound engine.

## Shared app frame (every app uses this so they feel like one OS)
- Entry: the page boots in Core open with your app's tile focused, then auto-opens it after ~1.2 s (and `#app` hash starts directly inside the app; `#orbit` stays on the orbit). Enter or click on the tile opens it.
- Open transition: the tile flies toward the camera and unfolds into the app space: a large glass workspace ~74% of the stage, slightly tilted in 3D, that settles flat. The core shrinks and slides to the top-left as a small live sphere (as in Working) showing agent state; the other tiles fall back into a dim orbit arc behind.
- Inside the app: panels are separate floating glass planes at slightly different depths (parallax on mouse move), draggable, scroll-zoomable; double-click a panel pulls it forward (like T in Working).
- An "agent strip" at the bottom of every app: shows agents relevant to this app (e.g. an agent editing this repo) with working / needs you / done states; clicking a needs-you chip raises the Agent moment card (reuse the visual language; a simplified card is fine).
- Esc closes the app: it folds back into its tile and returns to the orbit. Cmd/Ctrl+K opens the command bar.
- A small key legend bottom-right (like the others).

## Content
Use realistic fake data for a developer named Gabriel working on repo `sphere` (Rust) plus `pragma` and `nook`. Credit the open-source project each app adapts in a small line in the app (e.g. "terminal engine: Alacritty (Apache-2.0)").

## Delivery
- Output: /mnt/project-files/sphere-hud/apps/<app>/index.html with sources in /mnt/project-files/sphere-hud/apps/<app>/src/. Only write under your own app folders.
- Test in headless Chromium the way /mnt/project-files/sphere-hud/test/*.mjs does (three@0.160.0 from the npm registry served via page.route; flags --use-gl=angle --use-angle=swiftshader --enable-unsafe-swiftshader; SwiftShader is ~2 fps so use page.clock pauseAt/runFor + CDP Page.captureScreenshot, not page.screenshot). Save 1600x1000 shots to /mnt/project-files/sphere-hud/apps/<app>/shots/ : the opening transition mid-way, the app settled, one interaction state, and Full color theme. Look at your shots yourself and fix anything ugly, overlapping, cropped or unreadable before reporting. No console errors.
- Do not install system packages. npm/pip packages for testing are fine (playwright is already present; check before installing). The shared folder is a network mount: re-read a shared file just before changing it; don't grep the whole folder.
- Report back: path, what the screen does, keys, which shots to look at, any known gaps. Concise.
