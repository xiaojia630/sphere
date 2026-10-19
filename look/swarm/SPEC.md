# Agent swarm: implementation spec (extracted from swarm-proto.html)

Source: the approved "Agent swarm" design prototype (a single HTML/three.js page, 4703 lines). "L1234" = prototype line number. Numbers and strings are verbatim. Units: "stage px" = the 1600x1000 stage; "world units" = three.js units where 1 stage px = 1 world unit at z=0 (stageToWorld: `x = sx - 800, y = 500 - sy`, L3002).
Screenshots for comparison: `proto-overview.jpg`, `proto-hover.jpg`, `proto-focus.jpg`, `proto-needs.jpg`, `proto-ready-focus.jpg`, `proto-conflict.jpg`, `proto-merge.jpg` (same folder; `sbs-*.jpg` put the native view beside them, `live-*.jpg` are from a run on fake agents). `window.__swarm` (L4695) was used to reach states.

Prototype tech: three.js 0.160, CSS3DRenderer for all panels/labels (DOM planes in the 3D scene), UnrealBloom (strength theme `bloom` .6 balanced), additive blending everywhere (`ADD` = transparent, depthWrite false, depthTest false, additive; L2530). Camera: PerspectiveCamera fov 40, near 10, far 40000 (L2604). `view.D` = distance such that the stage fits; camera at (view.px, baseY+view.py, D*zoom).

---------------------------------------------------------------------------------------------------

## 1. Screen layout

### 1.0 View fit (L3017-3033)
- Layout `wide` unless `W < 820 || W/H < 1.05` -> `compact` (stage 420x860, phone). Stage sizes: wide `{w:1600,h:1000}`, compact `{w:420,h:860}` (L2980).
- `reserved = (H - dock.top) + 10`, `topPad = 6`; `availH = max(160, H - reserved - topPad)`, `availW = max(160, W - 24)`; `view.k = min(availW/1600, availH/1000)`. Stage is centred horizontally, top-aligned (`baseY = ((topPad + availH/2) - H/2)/k`).
- Label safe rectangle in screen px (L3030): `x0 = 36, x1 = W-36, y0 = strip.bottom + 10, y1 = dock.top - 12`.
- Zoom: `view.zoom` Spring(1,80,18), clamp .4..2.4. "Fit" (F / dblclick / fit button) resets zoom, pan, core user offset, orientation, panel user offsets (L3070-3076).

### 1.1 Screen-pinned chrome (HTML overlay, not on the stage) (L1019-1075, CSS L302-374, 835-863)
| Element | Position / size | Contents |
|---|---|---|
| Corner brackets `.brk` | 56x56 (compact 30), 1px `--hot`, opacity .7, at 14px from top corners, 38px above bottom (bottom ones hidden compact) | decoration |
| `.edge.l` rotated -90deg at left | font 9.5px, .5em tracking, `--dim` | `Sphere · agent swarm online` |
| `.edge.r` rotated 90deg at right | same | `Prototype · example data · core rings after A.L.I.C.E. (MIT)` (replace with product text) |
| **Mission strip** `#strip` | `left:50%` translateX(-50%), `top:16px`, height 42px, padding `0 16px 0 18px`, gap 16px, glass+frame (14px chamfer TL & BR), font `600 11px/1 Rajdhani`, .2em, uppercase, z 3 | see 1.2 |
| `#toasts` | centred, `bottom = (H - dock.top + 14)px` (compact swarm: `top:120px`) | max 3 stacked, see 6.3 |
| Dock `#dock` | centred, bottom 34px, width `min(1300px, 100% - 32px)` | command bar `#cmd` (h 48, flex 1 1 440), then controls: `Tilt` group, theme button, tools |
| Ticker `#ticker` | bottom 0, height 24px, `500 10.5px/24px JetBrains Mono`, top border `--g3`, edge-faded mask | see 6.2 |
| Tooltip `#tip`, reticle `#reticle` | follows pointer | see 4.1 |
| `#sweep` | full width, 220px tall band | scan flash on merge/theme/WAKE/SPEAKING |
| `#tint`, `#grid`, `#scan` | radial tint, 55px grid at .11 opacity, scanlines .05 | background |

Dock details: mic button 32px circle; placeholder (wide) `Ask Sphere anything, or try “next”, “merge”, “new fix the 404 page”`, compact `Ask Sphere anything`; hint key `Enter`. Tilt group: label `Tilt`, buttons `Low`/`Mid`/`High` (aria-pressed; Mid default) and `Tracks` (title "Show every agent's step track (T)"). Theme dropdown (`Balanced`, `Full color`, `Cyan`, `Black`, `Red`). Tools: bell `Needs you (N)` (amber 5px dot top-right when any agent needs you: `#btn-needs.has::after`, pulses `mo-badge` 1.7s; pressed while request mode), fit `Fit to screen (F)`, reset `Reset everything (R)`, sound `Sound on (M)` / `Sound off (M)`.

### 1.2 Mission strip text (renderStrip L3709-3721; CSS L842-863)
HTML, rebuilt only when it changes:
```
<seg>{n} agent|agents</seg> · <seg>{w} working</seg> · <seg.need>{nd} needs you</seg> · <seg.ready>{d} ready</seg> · <seg>{q} queued</seg> · <cost>$X.XX<i>today</i></cost>
```
- Each seg = `<b>N</b>label` (b: `500 12px JetBrains Mono`, `--hot`; gap 6px). Label singular only for the first seg (`agent` when n===1).
- `.need` is `--acc` (b gets amber text-shadow 0 0 8px acc 45%); `.ready` is `--ok` mixed by `--sem` into `--hot`; if value is 0 and seg has a class (need, ready) it also gets `.zero` -> `--dim`. Separators `·` `--dim`, gap 9px.
- Cost: `$` + `todayCost().toFixed(2)`, `todayCost = 0.57 + sim.merged + sum(agent.cost)`; trailing `<i>today</i>` 10.5px Rajdhani `--dim`, margin-left 6px.
- **Dots** `#st-dots` (to the right, `border-left: 1px solid --g2`, padding-left 14px): one `<button class="st-dot" data-id data-s="state">` per agent in AG order, 17x24 hit area, 7px glyph; `title="{i+1} · {title}"`, aria-label `"{i+1}. {title}, needs you|ready to merge|working|queued"`.
  - working: filled `--fill` circle breathing (`mo-breathe 2.6s`, staggered delays -.8/-1.6/-.4s on children 2/3/4)
  - needs: `--acc` circle, glow 0 0 7px, blinking `mo-badge 1.5s`
  - done: 7px `--ok` square rotated 45deg (diamond)
  - queued: transparent with `1px dashed --line`, opacity .75
  - hover/focus/`.on` (agent currently in focus or request): scale 1.5 (done: 1.4 rotated); `.on` also adds a 1px `--hot` ring (`inset 3px 1px`, opacity .8). Transition `transform .25s cubic-bezier(.2,1.2,.3,1)`.
  - Click -> `selectAgent(id)` (3.2/4.2). Compact: strip wraps, centered, top 10px.

### 1.3 Stage panels (CSS3D glass slabs) (LAYOUT L2981-2995; Panel class L2832; HTML L1085-1155)
Stage positions: `ax` l/c/r = x is left edge / centre / right edge; `ay` t/m/b = y is top / middle / bottom.

| Panel id | Wide stage pos | Width | Header / content |
|---|---|---|---|
| `p-brand` | left edge x=54, top y=26 | auto (`.brand`, left aligned) | wordmark `SPHERE` + chip `OS` (19px Michroma, .5em tracking), sub `Agent swarm` (10px, .44em). Not framed, no glass. Hidden compact. |
| `p-repos` ("Orbits") | left x=54, top y=92 | 316 | header `Orbits` / right `3 repos` (`REPOS.length + ' repos'`); body = rows, see 4.7 |
| `p-slots` ("Run slots") | right edge x=1546, top y=70 | 316 | header `Run slots` / right `{run} of {LIMIT}` e.g. `3 of 3`; body = 3 slot rows + queue list, see 4.8 |
| `p-say` caption | centre x=800, top y=892 | 780 (compact 400) | `#say-q` (question, 14px/600/.3em) + `#say-a` (answer 23px; idle hint 12px uppercase .28em `--dim`). Inert (no pointer). Initial text: `Press Space or click the core to talk`, then `IDLE_HINT`. |
| `p-moment` (request card) | centre (800, 520 middle) -> compact (210,520, scale .9) | 600 (compact 400) | see 3.2 |
| `p-focus` (focus card) | left edge x=96, middle y=500 | 478 (compact 400) | see 4.3 |
| `p-core` readout | child of core deck at z=112 | 230 | `Sphere core` / state label (`Standby`, `Attention`, `Listening`, `Thinking`, `Responding`) / sub: IDLE = summary `"{w} working · {n} needs you"` or `"{w} working · all clear"`; else `Wake word heard`, `Speak now`, `Working on it`, `Answering`. Opacity = `(1 - focAmt*1.4) * reveal * (wide ? 1 - moAmt : 1)` |

- Core (the "Sphere") is placed at stage (800, 486), scale `.58` (compact (210,304) .26) (CORE_AT L2996). Core layer list/geometry is in the existing home spec, unchanged.
- Panel `p.user` offsets: panels are draggable (pointerdown on empty area, spring k420 c40 while dragging) and wheel moves them in z (`user.z = clamp(user.z - deltaY*.7, -700, 420)`), reset by Fit/Reset. Moment and focus panels are `locked` (no drag).
- Panel idle motion (L2898-2911): yaw `clamp((pointer.x - screen.x)/W, -1, 1) * 8deg`, pitch `clamp((pointer.y - screen.y)/H,-1,1) * 6deg`, y bob `sin(T*.8 + home.x*.004) * 2.2`, hover lift 8px, drag lift 18px.
- Panel reveal at boot: `shown` set after `420 + i*90` ms per panel (skips moment & focus); framed panels get class `drawing` for 1250 ms (frame stroke draw `1.15s cubic-bezier(.65,.05,.25,1)`).
- Panel frame: chamfer 14px (top-left and bottom-right), path `M14 .5H{w-.5}V{h-14}L{w-14} {h-.5}H.5V14Z`, ticks along top right (`x` from w-58 to w-18 step 8, length 4.5) and left (y from 30 to 54 step 6, length 4), LED triangle bottom-right (`--fill`, pulses), bar bottom-left 30x2.5 at x=18 (L2856-2867). Back frame dashed `3 4` at z=-lift when lifted.
- Glass: gradient `--line` 9% -> 2.5% -> 4% over transparent; card/focus glass is darker (`--line` 13% over rgba(0,0,0,.6) -> 5% over .78 -> 6% over .78) "smoked glass".

### 1.4 What exists on home vs swarm-only
The swarm prototype markup contains NO home panels (no system, apps, tele, clock, rail, agents, rolo, or home `p-agents`); the CSS for them is still in the file (L148-300) as inherited. Swarm view = brand (retitled `Agent swarm`) + repos + slots + say + core + moment + focus. The home "Agents" panel is replaced by the satellites/orbits. Shared with home and unchanged: core, visor brackets, edge text (changed strings), toasts, dock, ticker, theme system, voice state machine, say panel. The `#app.swarm` class only adds `.say-a.idle {white-space: nowrap}` (L993) and compact toast offset. What home panels should do when swarm opens is NOT defined by the prototype; suggested: hide them with the same dim/exit transform as `ui.dim` (`tx -= side*120*k; tz -= 200*k; opacity *= 1 - .9k`, L2902-2903).

---------------------------------------------------------------------------------------------------

## 2. 3D scene

### 2.1 Groups and relation to the core (L3225-3228, 4612-4617, 4541-4546)
- `coreRoot` at stage-core position + user drag, `z = -70 * moAmt`, scale `C.s * (1 - .1*focAmt - .06*moAmt)` (C.s = .58 wide). Core x slides `+210 * focAmt` (wide only) toward the right while a focus panel is open (spring `coreX/coreY` k90 c19).
- `swarmG` (rings, in core-centred orbit plane): position = coreRoot.position; scale `C.sw * (1 - .08*focAmt)` (C.sw = 1 wide, .3 compact); quaternion = `coreTilt.quaternion * (Ry(sin(T*.13)*.05) * Rx(-orbitTilt deg))`. So the orbit plane follows the core's mouse-parallax tilt (+-5deg/+-7deg) and user rotation, plus a slow yaw sway of +-.05 rad at .13 rad/s (0 if reduced motion).
- Ring radii are in world units NOT scaled by the core (core's outermost deck is 412*.58 = 239). Rings are 292/372/452 and ready 540, so the swarm sits outside the core, face-on tilt 60deg.
- `orbitTilt` Spring(60, 40, 11) in degrees from face-on. TILTS: `low: 71, mid: 60, high: 46` (L3228). Boot intro starts at `TILTS.mid - 26 = 34` and springs to 60 (L4679); boot also sets `spacing` 165, `depthS` 80, `view.zoom` 1.22 -> 1, `qUser` = Euler(30deg, -26deg, 0, 'YXZ').
- `satLayer` is a separate world-space group (nodes can fly to the camera). Each ring group `R.g` is a child of `swarmG` with quaternion `Euler(R.tx, R.ty, 0, 'XYZ')`.
- Ring point function (L3247): `ringPoint(R, a, out, dz) = swarmG.localToWorld( (cos a * R.rad, sin a * R.rad, dz) rotated by R.q )`. `a` is a math angle (ccw from ring +x).

### 2.2 Per-repo rings (REPOS L3219-3224; geometry L3231-3245)
```
{ key:'api-server',  branch:'main', rad:292, tx:  0deg, ty:  0deg, w:  .074 }
{ key:'web-app',     branch:'main', rad:372, tx:  8deg, ty:  7deg, w: -.061 }
{ key:'sphere-cli',  branch:'main', rad:452, tx: -7deg, ty: -9deg, w:  .052 }
READY = { key:'ready', rad:540, tx:4deg, ty:-5deg, w:.027 }
```
`w` = angular speed (rad/s) of agents on that ring and ring decoration spin. Agent -> ring: `DEFS[i].repo` index (explicit, 0/1/2); `newAgent`: `repo = AG.length % 3`.
Per repo ring meshes (all `holoMat` with uDepth 0 so flat, walls 0, additive, uFade 1):
1. thin ring: r = rad, width .8, role `line`, alpha .26
2. glow band: r = rad, width 7, `line`, alpha .02
3. ticks: r = rad-1, 72 ticks, len 4.5, w .9, every 6th long (len 8), `line`, alpha .22, spin `R.w*.6` rad/s
4. arcs: r = rad, 3 segments, width 1.5, gap 74deg, role `fill`, alpha .32, spin `R.w*2.4` rad/s
Spin applied each frame `rotation.z += spin * dt * spinMul`; `spinMul` lerps (rate 1-exp(-3dt)) to 1 (idle) / 1.7 (request open or LISTENING) / 2.2 (WAKE) / 3.4 (THINKING); .15 reduced motion.
Ring "hi" (`uHi`, alpha x(1+uHi*.8)): lerps to .9 when the hovered agent is on that ring and its state != done, else 0 (rate 1-exp(-8dt)) (L4407).

Shared outer "Ready to merge" ring (READY, L3235-3238), role `ok`:
- dash ring r=540, w .9, dash 7 gap 9, alpha .42
- ring r=548, w .7, alpha .12
- ticks r=539, 48 ticks, len 4.5, w .9, alpha .24, spin TAU/160
- Its `uFade` lerps (rate 1-exp(-3dt)) to 1 when any agent is `done`, else .4 (L4406).

Ring labels (L3330-3334, 4397-4405; CSS .sw-ring L908-909):
- repo ring label text: `{R.key}<i>{n} agent(s)</i>` e.g. `api-server` + `2 agents` (`1 agent` singular). Uppercase via CSS, `600 9.5px/1 Rajdhani`, .34em tracking, color `--mo-mute` (= mix(--dim 52%, --line)); the `<i>` is `500 9.5px JetBrains Mono`, `--dim`, margin-left 8px, tracking .04em, not uppercased in data font. Text-shadow 3 layers of `--ink`.
- placed at ring angle 100deg, dz 0, then +14 world units up in y. Only when layout is wide (hidden compact).
- opacity `(1 - .7*ui.dim) * .85`.
- ready label: `Ready to merge<i>{n} ready | none yet</i>` at ring angle 78deg, +14 up; opacity `((n ? .85 : .5)) * (1 - .7*dim)`.

### 2.3 Satellite node (makeSat L3287-3317; per-frame placeSats L4258-4332)
One `THREE.Group g` in `satLayer`, all additive, in this build order (all share colour `col`, lerped per state):
| Part | Geometry | Material |
|---|---|---|
| shell | SphereGeometry r 10.5 (28x18) | fresnel (rim glow) `pow(1 - |n.v|, 2.4)`, alpha uniform `(.62 + (needs ? .25 : 0)) * (1 - .4*q)` |
| ico edges | IcosahedronGeometry r 6.2 detail 0, EdgesGeometry | `sphMat` role hot, alpha .85 |
| ico fill | same icosahedron, faces | role fill, alpha .07 |
| gyro 1 `gy1` | ring r 16.5, w .9 | line, alpha .75 |
| gyro 2 `gy2` | dashed ring r 21, w .9, dash 3.2, gap 4.4 | line, alpha .6 (lerps to 1 when queued) |
| track | ring r 26, w .8 | line, alpha .26 (0 when queued) |
| progress arc | ring r 26, w 2.3, 72 segs, full 2pi | role fill; `uReveal = clamp(pr,0,1)` where `pr` Spring(0,50,12) targets `a.p` (0 when queued). Reveals clockwise from 12 o'clock (angle measured clockwise from top). Hidden when queued. |
| pulse ring | ring r 13, w 1.3 | hot; on step: scale `1 + pl*2.5`, fade `(1-pl)^1.5 * .9 * fade`, `pl` runs 0..1 over .95 s |
| hover ring `hov` | 4 arcs r 35, w 1.1, gap 58deg | hot alpha .9, rotates `+.55 rad/s`, fade `hv*.85*fade` |
| halo | 1 point, shader `pow(1-r,1.8)*vA*uFade`, size 78 px * devicePixelRatio | per-vertex alpha `aA = .26 + hv*.1 + fly*.2`; `uHalo = fade * (needs 1.5 | queued .3 | else .7 + hv*.5)` |
| trail | 46 quads over 104deg behind the node | see 4.1 |
| marks | up to 9 points size 11 px (`NMARK = 9`) | see 4.1 |
Colour by state (lerp `1 - exp(-6dt)`): needs = `acc`; done/merging = `ok`; queued = `dim`; working = `line` lerped .3 toward `hot`.
Gyro motion (not reduced) with `lazy = working 1 | queued .25 | else .55`: `ico.rotation.y += dt*.8*lazy`, `ico.rotation.x += dt*.37*lazy`; `gy1.rotation.x = 1.12 + sin(T*.3 + phase)*.14`, `gy1.rotation.z += dt*.7*lazy`; `gy2.rotation.y = .5 + sin(T*.22 + phase)*.2`, `gy2.rotation.z -= dt*.45*lazy`, `gy2.rotation.x = 1.1`. `phase` = random in [0, 2pi) per node.
Position on its ring: `ringPoint(R, a.ang + wob, ..., bob)` with `bob = sin(T*.9 + phase) * 5` (dz, out of plane) and `wob = sin(T*.45 + phase*1.7) * .012` (angle). If done: lerp toward `ringPoint(READY, a.angR + wob, ..., bob)` by spring `rb`. Then `pos = lerp(orb, frontTarget, fly)`, `lerp(.., focusTarget, foc)`, `lerp(.., coreCentre, clamp(mg,0,1.05))`.
Scale: `sc = nodeS * (1 + hv*.22 + max(0,kick)*.1 + fly*.35 + foc*.25) * (1 - .84*smoothstep(0,1,mg)) * (1 - .12*q)`, `nodeS = lerp(1, swS.v, .55)` (wide 1). Group visible when `vis > .01 && mg < .99`.
Fade = `clamp(vis,0,1) * (1 - .3*q) * dimO * (1 - smoothstep(.5,1,mg))` with `dimO = .52` when a request card or focus panel is open and this is not that agent's node, else 1.
Springs per satellite (k, c): `fly` (34, 9.5), `foc` (40, 10.5), `mg` (30, 12), `rb` (22, 7.5), `q` (36, 12), `vis` (60, 13) (stopped: k=40), `hv` (90, 15), `trk` (60, 14), `kick` (220, 10), `pr` (50, 12).
Screen hit circle: centre = node projected; radius = `27 * k * sc + 12` px where `k = H/(2*tan(20deg)*max(60, camZ - z))` (px per world unit at depth z) (L4226, 4329).
Node colours come from theme roles (`line`, `hot`, `fill`, `acc`, `ok`, `dim`).

### 2.4 Label (HUD plate) per satellite (CSS L866-905, makeLabel L3323-3329, patchLabel L3755-3767, layoutLabels L4335-4391)
DOM: `<div class="mo-sat sw-sat" data-act="sw-sat" data-id tabindex=0 role=button>` containing
```
<b>{title}</b>
<span class="sw-m"><em>{repo key}</em><i class="chip">{chip}</i><s>{pct}</s></span>
<span class="sw-st"><u>{step}</u><i>{cost}</i></span>
<i class="sw-bar"><i></i></i>
```
- Width 268 (compact 206), padding `7px 12px 8px 13px`, row gap 3px, text left aligned; 1px left rule (`--sc` gradient, `--sc` = `--g1`; done = ok mix; needs = acc, 2px wide + amber glow). Background gradient 90deg: `--ink 74%` -> `52%` at 62% -> transparent (mirrored 270deg for `.l`).
- **Left/right side variant `.l`** (mirrors everything: text-align right, rule on right, `.sw-m` and `.sw-st` flex-direction row-reverse, bar fills from right): rule in `layoutLabels` (L4347-4349): default `side = +1` (label on the right of its node). If `side > 0 && nodeScreenX - coreScreenX < -80` -> `side = -1`; if `side < 0 && dx > 80` -> `side = +1` (80 px hysteresis). `.l` class toggled when `side < 0`.
- Fields:
  - `b` title: `700 10.5px Rajdhani`, .15em, uppercase, `--hot` (needs: `--acc`; queued: `--line`)
  - `em` repo key (e.g. `api-server`), `600 10px` .16em uppercase `--mo-mute`
  - chip text by state: working `Working` (class `chip work`, blinking 5px square), needs `Needs you` (`chip acc`, filled amber), done/merging `Ready to merge` (`chip done`, `--ok`), queued `Queued` (`chip dim q`, dashed border). Chip: `9px`, .18em, padding `3px 6px 2px`.
  - `s` percent: `Math.round(a.p*100) + '%'`, queued `—`; `500 11px JetBrains Mono`, margin-left auto (right aligned; in `.l` flows left); done colour = ok.
  - `u` current step: `a.step`; queued: exact string `Queued · starts when a slot frees`. 12px Rajdhani with a 5x7 right-pointing triangle bullet (`--dim`, hidden when queued); ellipsis overflow. Queued colour `--mo-mute`.
  - `.sw-st > i` cost: `'$' + a.cost.toFixed(2)` (queued: empty), `500 10.5px JetBrains Mono`, `--mo-mute`.
  - bar `.sw-bar`: 2px high `--g3` track; fill `width = pct%` (queued 0), `transition: width .8s cubic-bezier(.2,.8,.2,1)`; fill = `--fill` (needs `--acc` + glow; done `--ok`). Queued bar: 1px dashed (`repeating-linear-gradient 90deg --g2 0 4px, transparent 4px 8px`) and left rule dashed 1px.
  - aria-label: `"{title}, {repo key}, {chip}. Open"`.
- States toggled via `data-state` (working | needs | done | queued) and classes `hot` (needs), `hv` (hover: background `--line 5%`, title glow), `sel` (focused/requesting).
- Placement (screen px; label is a CSS3DObject at world z = sat.z+1 so it perspective-scales with `k`): `gap = 27*k*(1 + hv*.1) + 10`; label centre = `(node.x + side*(w*k/2 + gap), node.y - 2*k)` with `w = 268`, `h` = measured offsetHeight (default 78). Then 5 relaxation iterations (L4355-4368): for each pair of visible labels, overlap `ox = (pw+qw)/2 + 6 - |dx|`, `oy = (ph+qh)/2 + 4 - |dy|`; if both > 0: if `oy <= ox*1.4` push apart vertically by `oy/2` each, else horizontally by `ox/2`; then clamp each into the safe rect. Offsets ease toward targets at `1 - exp(-9 dt)`.
- Opacity: `op = clamp( L.op * clamp(vis,0,1) * (.46 + .54*depth) * dimO * (1 - hide) * (1 - .2*q) * (1 + hv*.4), 0, 1)` where `depth = .5 + .5*smoothstep(-420, 260, sat.z - core.z)`, `dimO = .5` when card/focus open and not this agent, `hide` -> 1 when the label rect (+-4 px) overlaps the open card/focus panel's screen rect (ease rate 10), `L.op` eases to `on ? 1 : 0` at rate 8, where `on = vis > .02 && mg < .6 && !(this agent is in focus mode)` (the focused agent's label is hidden). `pointer-events` off when op <= .3.

### 2.5 Other scene pieces
- Conflict chip + arcs: section 5.
- Request halo/tethers/sheet: section 3.2.
- Core dynamic layers affected by agents (kept from the existing core spec): `dAgents` deck has 6 sectors of 52deg (`SECT = 52*DEG`, `NSEC = 6`): track ring r247 w1 per agent; **working** arc r247 w4.6 `fill` filling `p` of the sector; **needs** r247 w4.6 `acc` (alpha `.62 + .38*sin(T*4.2)`); **done/merging** r247 w2.4 `ok`. Rebuilt on swarmDirty or every 1.2 s (L4186-4195, 4538, 4583). Hovering a core sector gives tooltip rows (section 4.1).
- Core flare/pulses: `sphPulse` Spring(0,150,9); `mo.fl` core-amber flare decays `exp(-1.1 dt)` (while > .01, deck colours lerp toward `acc`: `hot`,`fill` weight `a`, `line` .55a; bloom `+= .5*fl`).

---------------------------------------------------------------------------------------------------

## 3. States and transitions

Agent state values: `working`, `needs`, `done`, `queued` (+ transient `merging`, `stopped`). No `failed` state exists in the prototype. `LIMIT = 3` running agents (working or needs) at once (L3494). Max agents 9.

### 3.1 working
- Moves around its ring: `a.ang += R.w * dt` (needs: `* .7`).
- Every `a.tNext` seconds (`iv*(.88 + rnd*.24)`; first `iv*(.7+.5*rnd)`) `landStep` (L3585): pops next script step (`a.step = s.t`, `a.editing = s.f`), file add `+= 3 + floor(rnd*11)`, del `+= rnd<.5 ? floor(rnd*3) : 0`; `a.p = min(.985, a.p + a.dpv)` with `dpv = max(.008, (1 - p - .03)/script.length)`; `cost += .006 + rnd*.016`; unshift into `a.steps` (max 10, pop oldest) with timestamp and the ring angle at that moment; kick spring `+4` (strong `+7`), pulse ring plays. If script exhausted -> `finishAgent`.
- Visual: bright node, trail faint (.24), progress arc, label `Working`.

### 3.2 needs (fly forward, flare, opens request card, others dim) (askAgent L3622, reqOpen L3870, reqRise L3886)
Trigger: sim schedule `[{t:6.0,id:'checkout'},{t:47,id:'redis'}]`. If the agent was not `working`, it is rescheduled `+3 s`. State -> `needs`, `a.step = R.step` (`waiting to run pnpm test:e2e` / `waiting · migrate live sessions?`), ticker. If UI mode is overview -> `reqOpen(id, true)` immediately (no click needed); otherwise only toast `{title} · needs you` + sfx `toast`.
`reqOpen` sequence (a coroutine "flow", times in s):
1. t=0: sfx `whoosh` gain .7, buzz `transform`, `flash(.95)`, `shock('acc')` (amber shock ring), `sphPulse += 6`, `mo.fl = 1` (core flare), `ui.mode='request'`, `ui.req=id`, `ui.reqAmt.t = 1`, `ui.dim.t = 1`, `mo.halo.t = 1`, input locked.
2. wait .55 (the satellite flies toward you meanwhile).
3. `reqRise`: fill the card, `mo.tsrc = sat pos`, `mc.launch(satPos)`, sfx `open` .8, `expand` .5 (first), buzz `transform`; card gets CSS class `mo-in` for 1.7 s and `drawing` for 1.3 s.
4. wait .8, unlock, `say(REQS[id].intro)`.
Flight: satellite's `fly` spring (k34 c9.5) -> world position `frontTarget = (core.x - 20, min(card.home.y + cardH*homeScale/2 + 78, core.y + 330), core.z + 330)` (above the card, toward camera); node scale `+.35*fly`; label `sel`; on needs the shell alpha +.25, halo fade 1.5, colour `acc`.
Dim (`ui.dim` Spring(0,40,12) -> 1): every panel except brand and say: `x -= side*120*k`, `z -= 200*k`, opacity `*= 1 - .9k`; other satellites fade x.52, other labels x.5; ring labels x`(1 - .7k)`; conflict chips x`(1 - .55k)`; core: `z = -70*moAmt` (ui.reqAmt Spring(0,24,9.5)), scale `x(1 - .06*moAmt)`, deck `uFade = 1 - .12*moAmt`; core readout hidden in wide.
Halo around the core (`moHalo`, in coreTilt, radius `MO_R = 440`, spring `mo.halo` k26 c9, z = time-deck z - 6): ring r440 w1.7 (alpha fade `ha*(.7+.3*pulse)`), band r440 w20 alpha .1 (`ha*(.5+.5*pulse)`), 4 ticks at r426 len 26 w2.6 rotated 45deg (fade `ha*.85`), dashed ring r457 w1.1 dash 10 gap 9 rotating `-.13 rad/s` (fade `ha*.7`), ping ring r440 w2.2 expanding scale `1 + f*.3` over 2.7 s (fade `ha * (1-f)^1.6 * .8`). `pulse = .5 + .5*sin(T*3.1)`. Colour amber `acc` -> lerps to `ok` when answered (`mo.haloOk`). Halo scale `1 + .007*sin(T*3.1)`.
Tethers (lateDraw L4472-4501): `mo.tk` spring(0,60,13) -> 1 while mode request && card goal home. 4 constant-pixel-width lines from the halo ticks (angles `k*90 - 45`deg on the r440 ring) to the 4 card corners: width 1.6 px, alpha `min(1, .66*boost)*tk` -> `.32*tk`, dash period 11 px, flow `lerp(3.2, .55, rise)`, packet 1, phase `k*.27`; extensions 1.3 px beyond each corner alpha .55 -> 0 over `min(.34, ext/len)` (`ext` 130 wide / 60 compact); faint additive "sheet" quads between neighbouring tethers (alpha `beam*.085 + spill*.13`, bands `.72 + .28 sin(ta*52 - T*3.4)`); thread from satellite to card top-centre (1.2 px, alpha .75 -> .2, dash 9, flow 1.1). Before the card rises, tether starts come from the satellite (`mo.ta` spring(1,55,12) lerp from `tsrc`).
Card flight (`MomentPanel`): `launch`: start at satellite position, scale .08, opacity 0, `rx = -50deg`; springs for the first 1.5 s: `sc` k105 c12.5, `pz` k88 c13, `op` k80 c15, `rx` k80 c12.5; afterwards normal panel springs (`sc` 120/19, `pz` 60/14, `op` 50/13, `rx` 90/15). Target: stage (800,520) centre, `z = home.z + 190` (`cardZ`), lift 10, scale 1, pointer tilt yaw 5deg pitch 4deg, bob `sin(T*.9)*2.4` y, `sin(T*.7+1)*3.5` z, `sin(T*.6)*.5deg` pitch.
Card layout (CSS L579-696, HTML L1124-1138): chip `Needs you` (amber filled, 5px square blinking), title, right `src` text; ask row (52px avatar column; avatar `.mo-av` 40px circle with spinning ticks, `sys` icon = ringed planet SVG); `from` name (`700 12.5px`, `--hot`) + clock time (`HH:MM`, JetBrains Mono 11px) + question `p` (17px); draft block (`Reply to the agent` label + `by Sphere`, 20px text, 2px amber left rule + 22px bright tab at top); optional "you" row; `Why` evidence chips (3 mono 10.5px chips); actions `Send` (amber fill), `Change`, `Not now` (ghost) + keys hint `Enter · E · L`; footer `{k} of {n}` (`reqCount`) and `← → for the others` (if >1 waiting) else `Esc puts it back`. Card 600 wide, padding `16px 28px 17px`.
Request data (`REQS`, L3531-3544):
- checkout: title `Allow an e2e run`, src `web-app · waiting`, av `sys`, from `Fix flaky checkout test`, ask `wants to run pnpm test:e2e checkout --repeat-each 20 and start the dev server on :3000.`, dlbl `Reply to the agent`, draft `Yes, go ahead. Run it with --repeat-each 20, and stop the dev server when it finishes.`, why [`sandboxed · no network`, `writes 0 files`, `about 90 s`], yes `Send`, done `Sent`, toast `Sent to Fix flaky checkout test`, intro `The checkout agent wants to run the end-to-end tests twenty times. I drafted a yes. Send it?`, said `Only ten repeats, and don’t leave the server running.`, draft2 `Yes, go ahead, but use --repeat-each 10, and stop the dev server as soon as it finishes.`, lit 3, after `Ten repeats, and the server stops when it finishes. Send it?`, step `waiting to run pnpm test:e2e`, short `e2e run`.
- redis: title `Migrate old sessions?`, src `api-server · waiting`, av `sys`, from `Move sessions to Redis`, ask `Switching stores will log everyone out unless I migrate the 4,812 live sessions. Migrate them?`, draft `Yes, migrate the live sessions, keep the old store readable for 24 hours, then drop it.`, why [`4,812 live sessions`, `cookie sid v1`, `rollback ready`], toast `Sent to Move sessions to Redis`, intro `The Redis agent asks whether to migrate the four thousand live sessions. I drafted a yes. Send it?`, said `Keep the old store for a week, not a day.`, draft2 `Yes, migrate the live sessions, keep the old store readable for a week, then drop it.`, lit 1, after `It keeps the old store for a week. Send it?`, step `waiting · migrate live sessions?`, short `sessions`.
Card actions:
- **Send / Enter** (`reqYes`): sfx `confirm` .7 + buzz; chip text -> `Sent` (green check, `mo-done`); agent goes back to `working` immediately with step `continuing · running pnpm test:e2e` (checkout) or `continuing · migrating sessions` (redis), `tNext = 1.8`; wait .55; class `mo-fold` (.35 s collapse to a bright line: scaleY 1 -> .014 at 70% -> .004, opacity -> 0, `cubic-bezier(.6,.02,.72,.46)`, a horizontal line flashes at 68%) + sfx `collapse` .45; wait .46; card `goal='gone'`; "answer" streak flies from card to the satellite in .5 s (`1 - (1-f)^3`), line width 2.4 px, sfx `whoosh` .7 pitch +5, flash .5; wait .38; `shock('ok')`, `flash(.4)`, node kick `+8`, toast `r.toast` (ok), ticker; wait .5; `reqNext`.
- `reqNext`: if another agent needs you -> `reqOpen(next)` immediately; else halo -> ok, `shock('ok')`, `flash(.4)`, `sphPulse += 4`, say `Answered. Nothing else needs you right now.`
- **Change / E**: `reqChange` (voice rewrite): once per request (`Draft already updated · send it when it reads right` toast on repeat). Flow: card `mo-busy` (actions at .3 opacity, key hint reads `Listening` then `Rewriting`), types `said` at 34 cps into the "You" row (mic glyph live), .42 pause, THINKING .62 s, then word-diff rewrite (old words struck for .6 s, each old run collapses .3 s while the new run types at 52 cps with caret; final .12 s settle), lights evidence chip `lit` (sfx `progress` .5), wait .35, say `after`.
- **Not now / L** (`reqLater`): `mo-later` (card dims to opacity .26 / saturate .4 in .2 s), sfx `discard` .5, toast `Saved for later · {title}`, card goal `sat` (flies back into the node: scale .09, opacity 0, rx 30deg, 0.5 s) then if others wait, after .3 s opens the next. The agent stays `needs`.
- **Esc** (`reqPutBack`): card flies back to the satellite (same as above), sfx `collapse` .7 + buzz `collapse`, request remains, `ui.dim -> 0`.
- **← / →** (`reqStep`): `reqSwap`: sfx `tick` .5, old card returns to its satellite (.4 s), new card rises (.5 s), say new intro. Single request: toast `That is the only one waiting`.

### 3.3 done (move to outer ring, "Ready to merge")
- On script end (`finishAgent` L3603): state `done`, `p = 1`, `step = doneStep` (e.g. `PR #482 · CI green`), a new `angR = readySlot()` (candidate angles `k*TAU/12 + .4` for k 0..11; pick the one farthest from the other ready nodes; first one `160deg`), `rb.t = 1` (spring k22 c7.5 slides the node from its repo ring to the READY ring, rb overshoots slightly), strong pulse kick, sfx `done` .8 + buzz `confirm`, `shock('ok')`, `flash(.3)`, toast `{title} · ready to merge` (kind ok), ticker `{REPO} / {title lower} · {doneStep} · ready to merge`, say `{title} is ready to merge.` (overview and not busy).
- On the READY ring: `angR += READY.w * dt` (.027 rad/s, ccw), keeps `ang += R.w*dt*.5`. Node `ok` coloured, bar/pct green, chip `Ready to merge`. Progress arc stays full. The ring label counts `{n} ready`.
- Click opens the focus card with a green `Merge` button (4.3).

### 3.4 merged (collapse into core) (mergeAgent L3659)
Trigger: focus card `Merge` button, Enter on a focused done agent, or command `merge` (uses the focused agent if done, else first done; `Nothing is ready to merge yet` toast if none).
1. state `merging`, sfx `confirm` .7 + buzz; if in focus, close focus quietly; `sat.mg.t = 1` (spring k30 c12) sfx `whoosh` .6 -> node flies from READY ring to core centre shrinking (scale x `(1 - .84*smoothstep(mg))`, fade `(1 - smoothstep(.5,1,mg))`); label fades (hidden at mg>.6).
2. wait .62: `sweep()` (CSS band), `shock('ok')`, `flash(.55)`, `sphPulse += 6`, sfx `done` .8 + buzz.
3. wait .3: toast `Merged into main` (ok), ticker `GIT` / `main ← {title lower} · {doneStep}`, `sim.merged += a.cost` (added to the strip's cost), `removeAgent(a)`, say `Merged into main.`
Reduced motion: waits .1 and .1.

### 3.5 queued
Dashed look: node scale x.88, fade x.7, halo fade x.3, dim colour, dashed gyro ring alpha 1, progress arc and track hidden, slow gyro (lazy .25); label `Queued` chip with dashed border, text `Queued · starts when a slot frees`, pct `—`, cost empty, dashed bar. Position: fixed slot `a.slot = initial ang` with sway `sin(sim.t*.35 + qorder*2)*.07` rad.
`startQueued()` (every tick, L3613): while `running() < LIMIT`, take the lowest `qorder` queued agent -> `working`, `p = .03`, `tNext = 1.6`, step `first.t`; sfx `open` .6; toast `{title} · started` (ok); ticker `{REPO} / {title lower} · started in a free slot`; node `q.t = 0`.
New agent (`new <text>` command, or any unrecognised text, L3685): title capped at 34 chars (`slice(0,33) + '…'`), first letter upper-cased; branch `agent/{slug}` (slug: lowercase, non-alnum -> `-`, max 24); repo `AG.length % 3`; angle `(repo*70 + AG.length*53) % 360`; `iv 3.1`; `first S('planning the first steps')`; script `reading the repo`, `editing src/lib.rs`, `running the tests`, `editing src/lib.rs`, `running the tests`, `opening a PR`; done `PR opened · CI green`. Effects: `shock('fill')`, `flash(.4)`, sfx `open` .7, toast `Agent started` if a slot is free else `Queued · starts when a slot frees` (note: it is always created queued and then started by startQueued, so a free slot starts it next tick), ticker `{title lower} · queued`. If 9 agents: toast `The swarm is full · merge or stop one first`.

### 3.6 stopped (no failed state)
`stopAgent` (focus `Stop` button or `stop`/`kill` command with a focused agent; otherwise toast `Open an agent first, then say stop`): closes focus/request, sfx `discard` .6, toast `Stopped · {title} · worktree kept`, ticker (warn) `{title lower} · stopped, worktree kept`, node `vis.t = 0` with `vis.k = 40` (fade out), removed after 700 ms.

---------------------------------------------------------------------------------------------------

## 4. Interactions

### 4.1 Hover
- Detect: label hover (`ui.hoverLab`, pointerover on `.sw-sat`) or canvas hover (`hoverSatAt`, node screen circle radius `27*k*sc + 12`, nearest wins, needs `vis >= .3`, not merging; fine pointers only, not while grabbing). `ui.hover = hoverLab || hoverCanvas`.
- Effects (spring `hv` k90 c15 -> 1): node scale +22%, hover ring (4 arcs r35) fades in, halo brighter, label gains `.hv` bg and opacity x1.4, bright trail + step marks (`trk` target `+.8*hv`), the ring's `uHi` .9, hovered agent's tooltip.
- Tooltip `#tip` (frame L4653-4661): `t1` = agent title (uppercase, `700 10.5px`, .26em), `t2` = `{repo key} · {status}`; status:
  - needs: `Needs you · click to answer`
  - done: `Ready to merge · click to open`
  - queued: `Queued · starts when a slot frees`
  - working: `Step {si+1} · {step} · {m:ss} running` (e.g. `Step 3 · wiring the limiter into src/routes.rs · 3:18 running`).
  Box: bg `#000` 76%, left border 2px `--fill`, padding `7px 12px 8px 11px`; position `translate(pointer.x + 22, pointer.y + 18)`, flips to the left of the cursor if it would overflow `W - 12`; fade .15 s.
  Core deck hover tooltips (`deckInfo`, L4207): time: `Timeline · agent hours today` / `Now HH:MM · $X.XX spent` (or event name + `HH:MM to HH:MM`); load: `Run slots` / `{running} of 3 slots busy · {q} queued`; agents: sector agent title / `Needs you | Ready to merge | Queued | Working · N%` + ` · click to open`; signal: `Signal` / `Uplink 42 ms · 5 of 6 services up`; voice: `Voice` / `Click the core or press Space to talk`; core: `Sphere core` / `Drag rotate · click to talk · double-click to fit`.
- **Step track** trailing the satellite (trailGeo L3274-3285, placeSats L4307-4325, layoutExtras L4432-4451):
  - Trail ribbon: 46 quads along the ring over `TR_SPAN = 104deg` BEHIND the node (angle `-TR_SPAN*f`, mirrored `tg.scale.y = -1` when `R.w < 0`), half-width `.8 + 1.5*(1 - f)`, alpha `(1-f)^1.7`, role `fill`, material alpha .9. The tail fans out of the ring plane toward the viewer: `z = 22 * f^.85 * tg.scale.z`, with `tg.scale.z = .2 + .8*hv` (flat when idle, full 22-unit fan when hovered). Group rotation.z = `a.ang`.
  - Trail fade = `trk * onRing * fade` where `trk` target `= clamp((tracksOn ? .6 : 0) + (working .24 | needs .2) + hv*.8, 0, 1)` and `onRing = (1 - clamp(max(fly,foc,rb,mg),0,1)) * (queued|stopped ? 0 : 1)`.
  - Step marks: up to `NMARK = 9` points at the ring angle where each of the agent's last 9 steps landed (`a.steps[i].ang`), 11 px (x1.35 for the newest, i=0), colour `hot`, radial position `rad`, z = same fan function; shown if `back = (a.ang - step.ang) * sign(R.w)` is within `0..104deg`; alpha `(1 - back/TR_SPAN)*.95 + .05`; visibility `mk = clamp((tracksOn ? .85 : 0) + hv, 0, 1) * onRing`.
  - Step text callouts (pool of 4, L4434-4451): only for the hovered **working** agent when mode != request: texts for `steps[1..4]` (the older steps; step 0 is already in the label) placed at the step's ring point, on the side opposite to the label (`dir = -label.side`, offset `w/2 + 12`, y -10), content `<time>{ago}</time>{text}` (mono 10.5px, left rule `--g1`, bg `--ink 66%`; `time` `--dim`), opacity `(1 - back/TR_SPAN)*.95`; visible only if `0 < back < .95*TR_SPAN`.
  - Spacing: steps land every ~3 s; a ring moves .052-.074 rad/s so adjacent steps sit ~9-13deg apart (the 104deg track holds ~8-9 steps at default speed).
  - `Tracks` button / T key: `tracksOn` -> trails .6 and marks .85 for all agents (toast `Step tracks on` / `Step tracks follow the pointer`).

### 4.2 Select (click / 1-9 / Tab / strip dot)
`selectAgent(id)` (L3817): needs -> `reqOpen` (the request card), merging -> ignored, otherwise `focusOpen`.
- Click a label, strip dot, or the node on canvas (click = pointer up within 450 ms and under 4 px movement, `clickCanvas`; if no node hit and focus open -> `focusClose()`; hit on voice/core deck -> `voiceTurn()`).
- Keys: `1`..`9` -> `jump(i)` (the i-th agent in AG order; toast `No agent {i+1}` if none); `Tab`/`Shift+Tab` -> `cycle(+1/-1)` (only when focus is on body/a satellite label/a strip dot; wraps; starts from first/last when nothing selected); Enter/Space on a focused label also selects.
- `N` -> `nextNeeding`: list of `needs` agents; opens the one after the current request (cyclic); toasts `Nothing needs you right now` / `That is the only one waiting`. If in a request with < 2 waiting, N puts the card back.

### 4.3 Focus (glide front-left) (focusOpen L3802, placeSats, FocusPanel)
- On open: `ui.mode='focus'`, `ui.dim.t = 1`, `ui.focAmt.t = 1` (Spring(0,40,12)); sfx `open` .8 (or `tick` .8 if switching agents) + buzz `tap`; focus card gets `drawing` frame animation 1.25 s; `aria-hidden=false`.
- Node target (`focusTarget`): screen point `sx = max(96, W*.10)`, `sy = max(safe.y0 + 74, H*.17)` (compact: `W*.5`, `max(safe.y0+56, H*.2)`) projected onto the plane `z = 235` (fallback `(-600, 250, 235)`); node scale `+.25*foc` spring k40 c10.5; its label hidden; a thread line is NOT drawn (the visible link in the screenshot is the trail/conflict arc only).
- Core slides right `+210` stage px and shrinks 10%; the orbit group shrinks 8%.
- Panel: `FocusPanel` home = stage left x=96, centre y=500, width 478. Open: `x = home.x`, `z = home.z + 150`, lift 10, scale 1, opacity 1; closed: `x = home.x - 90`, `z = home.z - 110`, opacity 0, scale .94, yaw 16deg. Pointer tilt yaw +-4deg / pitch +-3deg; bob `sin(T*.9)*2.2` y, `sin(T*.7+1)*3` z. Springs as normal panel (`px/py` 120/20, `pz` 60/14, `op` 50/13, `sc` 120/19, `lift` 150/17).
- Contents (`foBuild/foPatch` L3770-3801, HTML L1145-1155). Updated on every step (`a.rev`) and once per sim-second.
  1. Header: chip (`Working` | `Needs you` | `Ready to merge` | `Queued`, classes `work | acc | done | dim q`), title (agent title, 14px, ellipsis), right = repo key.
  2. Meta row: `Branch` `{a.branch}` / `Running` `{m:ss}` (`fmtDur(sim.t - a.t0)`, `queued` if queued) / `Cost` `$0.00` (cost 2 decimals). Labels 9px uppercase `--dim`; values `--hot` mono 11.5px.
  3. `Progress` meter, right value `{pct}%` (queued `—`), track of 30 segments, first `round(p*30)` lit (`.on`).
  4. `Steps` with right label `waiting for a slot` (queued) | `{n} recent` (done) | `step {si+1}`; list = last 5 of `a.steps`, row `<time>{ago}</time><span>{text}</span>`; `ago(s, first)`: `now` if `s < 2 && first`, else `{round(s)}s` under 60, else `{round(s/60)}m`; first row of a working agent has a pulsing 5px square (`.now`). Empty: `—` / `Not started`.
  5. `Files touched` with right label `{n} file(s)`; rows `name` | `conflict` (amber `em`, only if this file is in an active conflict, left bar amber) | `+{add}` (ok colour) | `−{del}` (U+2212, bad colour). Row being edited now (`a.editing`, working) has a pulsing `--fill` left bar. Empty: `No files yet`.
  6. `Diff`: `+{add} −{del} · {n} file(s)` or `no changes yet`, plus a 4 px two-segment bar (ok / bad) with `flex-grow` = add / del (transition .6 s).
  7. Actions (top border `--g3`, gap 12): working/needs/queued -> `Open in Code` (fill), `Ask`, `Stop` (ghost), key hint `Esc back`. done -> `Merge` (`fill ok`, green) shown, `Open in Code` not filled, `Stop` hidden.
- Buttons: `Open in Code` -> toast `Opening {repo} in Code`, ticker `CODE` / `{repo} · {branch} opened`, sfx `confirm` .5. `Ask` -> `foAsk`: command bar gets `.ask` with chip `Ask · {repo}`, placeholder `Message “{title}”`, focuses input, sfx `toggle`; submitting text (runCommand) sets the agent step to `reading your note`, prepends step `you: {text cut to 60 chars + …}`, strong pulse, toast `Sent to {title}` (ok), sfx `confirm` .6, ticker `{title lower} · note from you`. `Merge` -> `mergeAgent` (3.4). `Stop` -> `stopAgent` (3.6).
- Esc / click on empty canvas / command `back|home|close|exit` -> `focusClose`: sfx `close` .7, `ui.dim.t = 0`, `focAmt.t = 0`. While in focus: Enter = merge (done) or open in Code, `A` = ask.
- `idleHint()` captions: request mode `Enter to send · E to change · L for later · ← → for the others · Esc back` (touch: `Tap a button, or the core to talk`); focus mode `Enter merges or opens in Code · A to ask · Tab for the next agent · Esc back` (touch: `Tap the core or press Esc to go back`); overview `IDLE_HINT` = `1 to 9 jump to an agent · Tab cycles · N for the next one that needs you · Esc back · F to fit` (touch: `Tap a satellite · the bell shows what needs you`). Idle text returns 9 s after a spoken line.

### 4.4 Key list (keydown capture, L4110-4138; mute at L2393)
| Key | Action |
|---|---|
| Esc | in input: blur (and cancel ask); theme menu open: close; request: put back; focus: close |
| Enter (request open, not on button) | Send |
| E / L / ← / → (request open) | Change / Not now / previous / next request |
| Enter or Space on a label | select that agent |
| N | next agent needing you (or put the card back if < 2 waiting) |
| 1-9 | jump to agent index |
| Tab / Shift+Tab | cycle agents |
| Enter (focus mode) | Merge if done else Open in Code; A = Ask |
| Space | `voiceTurn()` (status report) when not on a button |
| T | toggle step tracks |
| F | fit view |
| C | cycle theme (balanced, full, cyan, black, red) |
| R | reset everything (`resetAll`) |
| / | focus the command input |
| M | mute/unmute (toast `Sound off` / `Sound on`, sfx `toggle`) |
Ignored with Meta/Ctrl/Alt, when typing in an input, or while the theme menu is open. Repeat events ignored (except nothing).

### 4.5 Mouse
- Drag background = rotate the core and orbit plane (0.0062 rad/px; inertia, damping `exp(-2.4 dt)`; auto-orient stops until Fit/Reset); right-drag or Alt-drag = move the core; Shift-drag or middle = pan; wheel = zoom `exp(deltaY * .0011)` (ctrl/pinch .01), zoom clamp .4-2.4, two-finger pinch zooms/pans; double-click = Fit. Pointer parallax tilts the core `+-5deg x / +-7deg y` (`py*5deg`, `px*7deg`).

### 4.6 Command bar (runCommand L4042-4075; regexes on lowercased text)
In ask mode everything is the note. In request mode: `send|yes|yep|approve|go ahead|do it|confirm|ok|okay|sure` -> Send; `not now|later|skip|snooze|dismiss` -> Not now; `next|forward` / `previous|prev` -> step; `back|home|close|exit` -> put back; `change|make it|make this|shorter|rewrite|reword|edit|tweak|only|keep|say` -> Change. Otherwise: `back|home|close|exit` (closes focus); `needs?|what needs (me|you)|waiting|inbox|approvals?|requests?` or `next` -> next needing; `merge` (+anything); `stop|kill`; `new|start|add|queue <text>` -> new agent; `status|how|swarm|report` -> voice status turn; `track(s)`; `low|mid|high`; theme words (`balanced|default|calm`, `full color|colour|rainbow`, `cyan|blue`, `black|mono|dark`, `red`); `fit|frame`; `reset|restart|replay`; `[open |focus |jump |go to ]<digit>`; text (len > 2) matching an agent title substring or id prefix -> select it; anything else -> create a new agent with that text.

### 4.7 Orbits panel (`renderRepos` L3722-3733, CSS L921-931)
Rebuilt when agent ids/states change. Per repo a row (grid `16px 1fr auto`, padding 7px 0, bottom hairline):
- icon: 16x16 svg, ellipse `rx=7`, `ry = 2.6 + idx*.9`, rotated `(idx - 1) * 14` deg, plus 1.2 centre dot
- name `api-server` (13px 600 .16em uppercase `--hot`) and sub line `main · 2 agents` (`{branch} · {n} agent|agents`, mono 10.5px `--dim`, not uppercase)
- right: one 7x7 chip per agent in that repo, 4 px gap: working `--fill` square; needs `--acc` + glow; done `--ok` diamond (rotate 45 scale .85); queued dashed outline.
Header right text `3 repos`.

### 4.8 Run slots panel (`renderSlots` L3734-3747, CSS L932-944)
3 slots (`LIMIT`). Running agents (state working or needs, AG order) fill slots 1-3.
- row grid `14px 1fr 40px`: index `1` (mono dim), name = agent title (13.5px 600, not uppercase, ellipsis, `--hot`; needs -> `--acc`), right: `{pct}%` or `WAIT` for needs (mono 11px). Below each row a 2px bar (margin `-1px 0 5px 24px`) filled `--fill` (needs `--acc`) with `width: {pct}%` transition .8 s.
- empty slot: `Free` and `—` in `--dim`, empty bar.
- queue block (top border): heading `Queued · {n}` (10px .34em uppercase) followed by one line per queued agent (title, 5px dashed square bullet), ordered by qorder; or heading `Queue empty`.
- Header right `{running} of 3` (e.g. `3 of 3`).

---------------------------------------------------------------------------------------------------

## 5. Conflict detection and visuals (updateConflicts L3630-3643; layoutExtras L4409-4431; CSS L910-914)
Rule (checked every tick): for each pair `(a, b)` of agents, `a.state === 'working' && b.state === 'working' && a.repo === b.repo && a.editing && a.editing === b.editing` -> conflict keyed `a.id|b.id`, `file = a.editing`. `editing` is the file of the current step (set by `S(text, file)`), null for non-file steps; needs/done/queued agents do not conflict. In the example data it fires at start (rate + redis both on `src/routes.rs`) and clears when one moves to a different file.
- On new conflict: sfx `think` .6, toast `Conflict · both editing {file}`, ticker (warn) `{REPO} / conflict · {a title lower} and {b title lower} both editing {file}`, say `Heads up: two agents in {repo key} are both editing {file}.` (overview, not busy).
- On clear: toast `Conflict cleared · {file}` (ok). Chip pool reused; conflicts removed when an agent is removed.
- Arc geometry (every frame): quadratic Bezier from node A to node B with control point `M = mid + dirOut * (bulge*2*age)`, `M.z += 44*age`, where `mid = (A+B)/2`, `dirOut = normalize((mid - coreCentre) with z=0)` (fallback (0,1,0) if length^2 < 1), `bulge = 38 + dist(A,B)*.16`, `age = clamp((T - birth)/.6, 0, 1)` (grows over .6 s). 20 segments; each segment is a constant-pixel line (`mbLine`, 1.5 px, no dash) with alpha at t: `(.18 + .82*sin(pi*t)) * pulse * age`, `pulse = .72 + .28*sin(T*3.1)` (.8 reduced). Colour: `moBeamCol` = `acc` lerped .3 toward `hot` (so amber-white), shifting toward `ok` only right after answering a request.
- Chip: CSS3D element `.sw-conf` = amber `■` (6px, pulsing `mo-pulse 1.4s`) + `BOTH EDITING` (`700 9.5px`, .16em, uppercase, `--acc`) + file name in mono 10.5px `--hot` (not uppercase). Border `1px solid acc 62%`, bg `--ink 72%`, glow `0 0 14px -2px acc 38%`, padding `5px 9px 4px 8px`, gap 8px. Placement: `0.25*A + 0.5*M + 0.25*B + (0, 20, 2)` (the crest of the arc, 20 world units above). Opacity `clamp(age*1.2) * (1 - .55*max(ui.dim, 0)) * (1 - under)` where `under` eases to 1 when the chip's screen position is within the open card/focus panel rect (+40 px x, +14 px y).
- Focus card marks the conflicting file row (`conflict` tag + amber bar).

---------------------------------------------------------------------------------------------------

## 6. Sound, speech, ticker, toasts

### 6.1 sfx calls (engine: `createSFX`, names: hover tick click toggle open close whoosh collapse expand theme wake listen think confirm discard toast error progress done boot grab drop; buzz: tap confirm transform collapse error). `sfx(name,{gain,pitch})`; `sfxQ(name,o,gapMs)` only plays if no sfx in last `gap` ms (default 150).
| Trigger | Call |
|---|---|
| click on any button/`[data-act]`/menu item | `sfxQ('click', , 90)`; touch also buzz `tap` |
| pointerover clickable (non-touch) | `hover` |
| drag start / end (panel/core) | `grab` / `drop` gain .8 |
| sound toggle on | `toggle` |
| theme change | `theme` (+ sweep, shock fill, toast) ; theme menu open `toggle`, close `sfxQ toggle`, arrow keys `tick` |
| `toast()` (if no sfx in last 150 ms) | kind `bad` -> `error` + buzz `error`; kind `ok` -> `done`; else `toast` |
| step lands on hovered/focused agent | `sfxQ('tick', {gain:.12}, 600)` |
| agent finishes | `done` .8 + buzz `confirm` |
| queued agent starts | `open` .6 |
| new request while not overview | `toast` |
| conflict created | `think` .6 |
| merge click | `confirm` .7 + buzz; at collapse start `whoosh` .6; at impact `done` .8 + buzz `confirm` |
| stop | `discard` .6 |
| new agent | `open` .7 |
| focus open | `open` .8 (`tick` .8 if switching) + buzz `tap`; close `close` .7 |
| Ask | `toggle` |
| Open in Code | `confirm` .5 |
| request flare | `whoosh` .7 + buzz `transform`; rise `open` .8 (+ `expand` .5 first) + buzz |
| put back | `collapse` .7 + buzz `collapse` |
| Send | `confirm` .7 + buzz; fold `collapse` .45; flight `whoosh` .7 pitch 5 |
| Not now | `discard` .5 |
| card swap | `tick` .5 |
| Change flow | voice state sounds `wake` / `listen` (sustained until state changes) / `think`; typing `sfxQ('tick', {gain:.1}, 70)` every 4 chars; rewrite strikes `sfxQ('tick', {gain:.08}, 0)`; evidence lit `progress` .5 |
| note sent in Ask mode | `confirm` .6 |
| tilt change | `whoosh` .6 ; tracks toggle `toggle` ; fit `whoosh` .6 ; reset `boot` .7 |
Voice states (`ST`, energy `e`): `IDLE` Standby .3; `WAKE` Attention "Wake word heard" .95; `LISTENING` "Speak now" .8; `THINKING` "Working on it" .45; `SPEAKING` "Responding" / "Answering" .9. `sfxState`: WAKE -> `wake`; LISTENING -> `listen` held; THINKING -> `think`.

### 6.2 Spoken lines (`say()`, TTS via `SFX.speak`, caption types at 26 ms/char; waits for any current line (max 20 s), +.5 s after; idle hint 9 s later)
- finish: `{title} is ready to merge.` (overview, not busy)
- conflict: `Heads up: two agents in {repo key} are both editing {file}.`
- merge: `Merged into main.`
- request open / swap: `REQS[id].intro` (see 3.2)
- after rewrite: `REQS[id].after`
- all answered: `Answered. Nothing else needs you right now.`
- status (Space / mic / `status|how|swarm|report`): caption question `“How is the swarm doing?”` typed at 26 cps; WAKE 450 ms (flash .9), LISTENING (+420 ms), THINKING 1000 ms, SPEAKING: `{AG.length} agents: {w} working[, {n} needs you][, {d} ready to merge][, {q} queued]. Spent ${cost} today.` e.g. `6 agents: 3 working, 1 ready to merge, 2 queued. Spent $1.89 today.` then +500 ms and (if anything needs you) after 300 ms opens the first request.

### 6.3 Ticker (L3697-3704, 80 s linear scroll `translateX(-50%)`, items duplicated)
Item = `<span class="w?"><b>KEY</b>value</span>`; padding 0 24px; key `--hot`, `500`, margin-right 9px; warn items (`w`) are `--acc` incl. key. New events `unshift`, list capped at 14, re-render debounced 400 ms. Initial list: `API-SERVER`/`rate limiting · editing src/auth.rs`; `WEB-APP`/`checkout.spec · flaky 3 / 20`; `SPHERE-CLI`/`PR #77 · CI green · ready to merge`; `SWARM`/`3 of 3 slots busy · 2 queued`; `COST`/`$1.84 today · budget $10`; `GIT`/`main ← 2 commits from Ada`; `API-SERVER`/`sessions · editing src/routes.rs` (warn); `CI`/`login.spec #482 passed · 2m 14s`; `SPHERE-CLI`/`cold start · queued`; `VOICE`/`wake word armed`.
Events (key = `REPO.toUpperCase()` unless noted):
| Trigger | Value (warn?) |
|---|---|
| done | `{title lower} · {doneStep} · ready to merge` |
| started | `{title lower} · started in a free slot` |
| needs you | `{title lower} · {request step}` (warn) |
| conflict | `conflict · {a lower} and {b lower} both editing {file}` (warn) |
| merged | key `GIT`: `main ← {title lower} · {doneStep}` |
| stopped | `{title lower} · stopped, worktree kept` (warn) |
| new | `{title lower} · queued` |
| answered | `{title lower} · answered, continuing` |
| note | `{title lower} · note from you` |
| open in code | key `CODE`: `{repo} · {branch} opened` |

### 6.4 Toasts (stack of max 3, `600 11px` .24em uppercase, bg black 72%, border `--g3`, left border 3px `--fill` (ok: `--ok` mix, bad: `--bad` mix), in `.35 s`, auto-out after 2400 ms then out `.42 s`)
`{title} · ready to merge` (ok), `{title} · started` (ok), `{title} · needs you`, `Conflict · both editing {file}`, `Conflict cleared · {file}` (ok), `Merged into main` (ok), `Nothing is ready to merge yet`, `Stopped · {title} · worktree kept`, `Open an agent first, then say stop`, `The swarm is full · merge or stop one first`, `Agent started`, `Queued · starts when a slot frees`, `Sent to {title}` (ok), `Sent to Fix flaky checkout test` / `Sent to Move sessions to Redis` (ok), `Saved for later · {request title}`, `That is the only one waiting`, `Nothing needs you right now`, `Draft already updated · send it when it reads right`, `No agent {n}`, `Opening {repo} in Code`, `Orbit tilt · {low|mid|high}`, `Step tracks on`, `Step tracks follow the pointer`, `Fit to screen`, `View reset`, `Swarm reset`, `Theme · {label}`, `Sound on`, `Sound off`.

---------------------------------------------------------------------------------------------------

## 7. Timings, springs, easings

Spring model (L2417): semi-implicit Euler, substeps `ceil(dt*120)`, `a = k*(target - x) - c*v`. `Spring(x, k, c)`; default `Spring(0,140,22)`. Frame `dt = min(1/30, ...)`.
| Spring | (k, c) |
|---|---|
| Panel px/py | 120, 20 (drag 420, 40) |
| Panel pz | 60, 14 |
| Panel rx / ry | 90, 15 |
| Panel scale | 120, 19 |
| Panel lift | 150, 17 |
| Panel opacity | 50, 13 |
| `ui.dim` / `focAmt` | 40, 12 |
| `ui.reqAmt` | 24, 9.5 |
| `mo.halo` | 26, 9 |
| `mo.tk` / `mo.ta` | 60,13 / 55,12 |
| `mc.foldK` | 60, 14 |
| `orbitTilt` | 40, 11 |
| `view.zoom/px/py` | 80, 18 |
| `coreX/coreY` | 90, 19 |
| `spacing` / `depthS` | 46, 11.5 / 60, 13 |
| `sphPulse` | 150, 9 |
| Sat `fly` / `foc` / `mg` / `rb` / `q` | 34,9.5 / 40,10.5 / 30,12 / 22,7.5 / 36,12 |
| Sat `vis` / `hv` / `trk` / `kick` / `pr` | 60,13 / 90,15 / 60,14 / 220,10 / 50,12 |
| Card launch (first 1.5 s) | `sc` 105,12.5; `pz` 88,13; `op` 80,15; `rx` 80,12.5 |
| Deck `hi` | 170, 24 |
Exponential smoothing constants: theme colour lerp `1 - exp(-4.5 dt)`; node colour `-6`; labels offset `-9`, opacity `-8`, hide `-10`; ring uHi `-8`; ready ring fade `-3`; orientation slerp `-3.2`; pointer parallax `-3`.
Fixed durations: step interval 2.9-3.4 s per agent (`iv` in DEFS: 3.0, 3.4, 2.9, 3.1, 3.2, 3.3 with +-12% jitter); pulse ring .95 s; shock ring .95 s (`scale lerp(1.1, 4.4, outCubic(t))`, alpha `(1-t)*.9`, ring r100 w1.6 in the core's space, colour role `hot` default / `ok` / `acc` / `fill`); `flash(a)` decays `exp(-3.2 dt)` and lifts decks; `#sweep` 1.1 s `cubic-bezier(.3,.7,.3,1)`; toast 2.4 s + .35/.42 s; request flare .55 s -> rise .8 s; Send: .55 + .46 + .38 + .5 s; Later .5 (+.3); swap .4 + .5; merge .62 + .3; stop 0.7; frame draw 1.15 s; panel stagger `420 + i*90` ms; boot reveal `outCubic((bootT - .15 - revealAt) / .9)` per core layer; CSS: sw-bar `width .8s cubic-bezier(.2,.8,.2,1)`, st-dot `.25s cubic-bezier(.2,1.2,.3,1)`, mo-fold `.35s cubic-bezier(.6,.02,.72,.46)`, mo-rise `.6s cubic-bezier(.2,.8,.2,1)`, caption speak 26 ms/char, idle hint 9 s, theme transition .9 s ease.
Reduced motion (`#still` hash or prefers-reduced-motion): springs unchanged but all waits become 0-.15 s, no bob/wob/tilt/sway, spokes const, card pops instantly (no flight), no sweep. Map "Fast" to the reduced waits plus stiffer springs; "Cinematic" = the values above.

---------------------------------------------------------------------------------------------------

## 8. Theme tokens per element
Tokens: `--line`, `--dim`, `--hot`, `--fill`, `--acc` (#ffb347 amber = "needs you"), `--ok` (#5fe3a1), `--bad` (#ff5a4f), `--ink` (#000), `--g1/g2/g3` (line at 72-78% / 34-38% / 13-15% alpha), `--fg2` (fill 45%), `--sem` (1 in Balanced/Full color: done = `--ok`, bad = `--bad`; 0 in Cyan/Black/Red: mono), `--fx` (1 only in Full color: per-panel hue), `--tint`. `--mo-ok = mix(--ok --sem%, --hot)`, `--mo-bad` likewise, `--mo-mute = mix(--dim 52%, --line)`. Balanced values: line `#a6d3e6`, dim `#3e6678`, hot `#f1f9fc`, fill `#dff4ff`, acc `#ffb347`, ok `#5fe3a1`, bad `#ff5a4f`, tint `#03141a`; theme `PAL` (L2434): balanced/full/cyan/black/red with `bloom` .6/.72/.68/.34/.58. `deep` (ring accents in core) from PAL.
| Element | Tokens |
|---|---|
| Strip | glass `--line 9% over rgba(0,0,0,.5)`; frame `--hot` 1px + drop-shadow `--g1`,`--g3`; text `--line`; numbers `--hot`; need `--acc`; ready `--ok`; zero/sep `--dim`; cost `--hot`, `today` `--dim`; dots: working `--fill`, needs `--acc`, done `--ok`, queued dashed `--line` |
| Label | text `--line`; title `--hot` (needs `--acc`, queued `--line`); repo `--mo-mute`; chip: working `--line` border + `--fill` blink; needs `--acc` fill with `--ink` text; done `--ok`; queued `--dim` dashed; percent `--hot` (done `--ok`); step `--line` (queued `--mo-mute`), bullet `--dim`; cost `--mo-mute`; bar track `--g3`, fill `--fill` (needs `--acc`, done `--ok`); rule `--g1` (needs `--acc`, done `--ok`) |
| Rings | repo rings `line` (alpha .26 / .02 / .22) + `fill` arcs .32; ready ring `ok` |
| Node | shell/halo/arc/gyros from state colour: working `line->hot 30%`, needs `acc`, done `ok`, queued `dim`; icosa edges `hot`, fill `fill`; pulse/hover ring `hot`; trail `fill`; marks `hot` |
| Ring/ready label | `--mo-mute`, sub `--dim`, shadow `--ink` |
| Conflict | `--acc` text and border, bg `--ink 72%`, file `--hot`; arc `acc` lerp `hot` .3 |
| Request card | `--mo-a` = `--acc` (becomes `--mo-ok` when sent): chip, header rule, draft tab, led/bar, glow; text `--line`/`--hot`; time/labels `--mo-mute`; evidence border `--g2`, lit `--hot`; buttons `Send` `--acc` fill; frame `--hot` |
| Focus card | same `.card` glass; chip colours as label; led/bar `--fill` (needs `--acc`, done `--ok`); meta labels `--dim`, values `--hot`; step times `--dim`; files border `--g3`, bars `--g2` (editing `--fill`, conflict `--acc`); `+N` `--ok`, `−N` `--bad` (mixed to `--hot` sem 0); diff bar ok/bad; `Merge` fill `--ok` |
| Orbits panel | name `--hot`, branch `--dim`, ring icon `--line`, chips `--fill`/`--acc`/`--ok`/dashed `--line` |
| Run slots | idx `--dim`, name `--hot`, free `--dim`, need `--acc`, bar `--g3`/`--fill`; queue head `--line`, items `--line`, bullet dashed |
| Toasts | bg `#000 72%`; border `--g3`; left 3px `--fill` / ok / bad |
| Tooltip | bg `#000 76%`, left 2px `--fill`, title `--hot`, sub `--line` |
| Ticker | text `--line`, key `--hot`, warn `--acc`, top border `--g3` |
| Core flare | `acc` lerp into core `hot/fill/line` |
Full color: `.mo-sat` (labels) and `#p-moment` stay neutral blue-white (`--pl #e6f1f7`, `--pd #8aa1ad`, `--ph #fff`, `--pf #fff`); amber is the only hue.

---------------------------------------------------------------------------------------------------

## 9. Example data (DEFS L3498-3530) for demo mode
`initial ang` (deg on ring), `age` s since start, `iv` step interval s. `todayCost` base `sim.today0 = .57`. RNG: LCG `seed = seed*16807 % 2147483647`, seed 11, value `seed/2147483647`.
| # | id | repo | title | branch | state | p | cost | age | ang | iv | files [name, add, del] | first step | script (S(text, file)) | doneStep |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | rate | api-server | Add rate limiting to /login | feat/login-rate-limit | working | .34 | .12 | 192 | 22 | 3.0 | src/auth.rs 42/3; src/middleware/limit.rs 31/0 | `editing src/auth.rs` (src/auth.rs) | `editing src/routes.rs` (f), `wiring the limiter into src/routes.rs` (f), `editing src/routes.rs` (f), `running cargo test · 18 / 31`, `fixing a clippy warning in src/auth.rs` (src/auth.rs), `running cargo test · 31 / 31`, `writing tests/login_rate.rs` (f), `committing 4 files`, `opening PR #482` | `PR #482 · CI green` |
| 2 | redis | api-server | Move sessions to Redis | feat/redis-sessions | working | .58 | .31 | 410 | 205 | 3.4 | src/session.rs 64/21; src/routes.rs 12/4 | `editing src/routes.rs` (f) | `editing src/routes.rs`(f), `editing src/routes.rs`(f), `reading src/session.rs`, `editing src/session.rs`(f), `adding redis to Cargo.toml`(Cargo.toml), `running cargo check`, `editing src/store/redis.rs`(f), `running migration 0007`, `running cargo test · 12 / 31`, `editing src/session.rs`(f), `running cargo test · 31 / 31`, `opening PR #483` | `PR #483 · CI green` |
| 3 | checkout | web-app | Fix flaky checkout test | fix/checkout-flake | working | .71 | .18 | 275 | 312 | 2.9 | e2e/checkout.spec.ts 18/9 | `running checkout.spec · repeat 4 / 20` | `reading e2e/checkout.spec.ts`, `editing e2e/checkout.spec.ts`(f), `running pnpm test:e2e · repeat 6 / 20`, `replacing a fixed wait in e2e/helpers.ts`(e2e/helpers.ts), `running pnpm test:e2e · repeat 13 / 20`, `running pnpm test:e2e · repeat 20 / 20`, `committing 2 files`, `opening PR #232` | `20 / 20 pass · PR #232` |
| 4 | react | web-app | Upgrade React to 19 | chore/react-19 | queued | 0 | 0 | - | 118 | 3.1 | - | `creating the worktree` | `bumping react and react-dom to 19`(package.json), `running the codemod`, `editing src/components/Cart.tsx`(f), `fixing 3 type errors`, `running pnpm test · 112 / 240`, `editing src/hooks/useCheckout.ts`(f), `running pnpm test · 240 / 240`, `opening PR #233` | `PR #233 · CI green` |
| 5 | json | sphere-cli | Add --json flag to sphere run | feat/run-json | done | 1 | .66 | 1210 | 150 (angR 160) | 3.2 | src/cmd/run.rs 48/6; src/output.rs 22/2; tests/run_json.rs 31/0 | step `PR #77 · CI green` | none | `PR #77 · CI green` |
| 6 | cold | sphere-cli | Speed up cold start | perf/cold-start | queued | 0 | 0 | - | 64 | 3.3 | - | `profiling the first run` | `reading src/boot.rs`, `editing src/boot.rs`(f), `lazy-loading the plugin index`(src/plugins.rs), `running the benchmark · 410 ms`, `editing src/plugins.rs`(f), `running the benchmark · 210 ms`, `opening PR #78` | `PR #78 · 410 → 210 ms` |
Seed step history `hist` (older first shown at ages 21, 37, 53 s etc: `16*(i+1)+5` s ago): rate [`planning the limiter`, `reading src/auth.rs`, `creating branch feat/login-rate-limit`]; redis [`migrating the session type`, `reading src/session.rs`, `creating branch feat/redis-sessions`]; checkout [`reproduced the flake 3 / 20`, `reading e2e/helpers.ts`, `creating branch fix/checkout-flake`]; json [`opening PR #77`, `committing 3 files`, `running cargo test · 24 / 24`]. Active agents start with `steps = [first, ...hist]`.
Schedule: request from `checkout` at sim t=6.0 s, from `redis` at t=47 s. With the defaults a conflict (`src/routes.rs`, rate + redis, api-server) is live around t=5 s. Queued agents start when `running() < 3` (a request answer keeps the slot; the first free slot appears when an agent finishes).
Ticker/idle strings: see 6.2/6.3. Bell, strip counts and slot panel update from the same state.

## Gaps / not determinable
- Home-panel behaviour on entering swarm (not in the prototype; see 1.4).
- Heights of stage panels are content-driven (measured via `offsetHeight`); exact px vary with fonts.
- Voice (TTS) uses the browser `speechSynthesis` through `SFX.speak`; synthesized sound specs live in the `createSFX` engine (L1206-2336), not repeated here.
