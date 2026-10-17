# M2 look check: the app against the approved prototypes

Each image is the Rust app on the left and the prototype on the right, both at 1600x1000.

- The app side is rendered offscreen with `sphere-app --shot`: the demo data, or a test sphered running the fake agent for the Working view.
- The prototype side was captured from headless Chrome in real time, once it had settled.
- Clocks, telemetry, agent progress and the ticker are live or example data, so they differ between the two sides.
- The prototypes show example events and music. The app shows only real data, so where none exists yet (calendar, music) you see an empty state.

| Image | State |
| --- | --- |
| `home.jpg` | Home, Flat, Balanced |
| `home-depth.jpg` | Depth (2): the layers pulled apart, with the L1–L6 callouts |
| `home-orbit.jpg` | Orbit (3): the rings folded like a gyroscope |
| `home-collapsed.jpg` | Collapsed (X): the rings landed on the sphere |
| `home-full.jpg`, `home-cyan.jpg`, `home-red.jpg` | Themes (C) |
| `core-open.jpg` | Core open (A): the app orbit |
| `moment.jpg` | The Agent moment: the card, the halo, tethers and satellites |
| `working-end.jpg` | The Working view at the end of a run (the app's side is a fake agent that ran without tool steps) |
