# UI sounds

These 22 sounds are rendered from the web prototype's own synthesis code (`createSFX()` in the
design prototypes: moment.html, working.html, home-v2.html and core-open.html share the same
engine). The sounds are built from sine, FM, sawtooth and triangle oscillators, filtered noise, and a
generated reverb impulse with a ping-pong echo. No samples are used. Each one went through the engine's
`renderMix()`, which uses the same master chain as live playback (reverb and echo sends, compressor,
master gain 0.9, soft limiter), with default options and the engine's fixed random seeds. Files are
16-bit stereo PCM at 44.1 kHz. They include the reverb tail, which is cut where it rounds to digital
silence. `listen` is a sustained sound: the prototype holds it while the mic is open and fades it out
over 0.4 s. This file renders it with a 2.0 s hold, so it has a 0.3 s fade-in, a hold, the 0.4 s
release and a tail. It is not a seamless loop.

| Sound | File length | What the prototype plays it on |
|---|---|---|
| hover | 0.50 s | pointer moves onto a button, link or other clickable (mouse only) |
| tick | 0.47 s | arrow/Home/End in the theme menu, rolodex step, opening a card that has a kind, switching cards in a moment |
| click | 0.71 s | any click on a clickable (skipped if another sound played in the last 90 ms) |
| toggle | 0.95 s | sound turned back on, theme menu opens |
| progress | 0.52 s | each 10% step of a progress bar (command run, test run, image download, "why" evidence lighting up) |
| toast | 0.85 s | a plain toast notification appears |
| open | 1.59 s | a panel or card opens, a moment card rises |
| close | 1.45 s | a panel closes (working view) |
| whoosh | 1.67 s | view transform: fit view, mode change, entering a moment, a card flying to its destination |
| collapse | 1.70 s | the dashboard collapses into the core, leaving a moment, a card folding away |
| expand | 1.95 s | the dashboard expands out of the core, the first moment card rising |
| theme | 1.79 s | accent or theme colour changes |
| wake | 1.55 s | voice state WAKE (wake word heard / talk pressed) |
| listen | 2.82 s | voice state LISTENING; held until the state changes, then faded out |
| think | 0.86 s | voice state THINKING |
| confirm | 1.73 s | approving a draft or a moment ("yes"), resuming a run |
| discard | 0.78 s | discarding a draft, "later" on a moment, pausing or stopping a run |
| error | 1.01 s | an error toast |
| done | 1.78 s | a success toast, a run or download finishing, the last moment answered |
| boot | 2.75 s | start-up and reset of the whole view, replaying a run |
| grab | 0.85 s | starting to drag the scene or the core |
| drop | 0.87 s | releasing a drag, the last ring landing at start-up |
