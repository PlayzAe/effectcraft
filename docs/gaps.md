# Where EffectCraft falls short

An honest assessment of how far EffectCraft is from being a real replacement for After Effects,
and the work that closes the gap. This document is meant for contributors and agents choosing
what to work on. The [ROADMAP](../ROADMAP.md) summarises it; [parity.md](parity.md) is the
feature-by-feature checklist it builds on.

*Assessed 5 October 2026, user issues updated 7 October. Estimates marked "≈" are judgements from the evidence listed, not
measurements. Update this file when the evidence changes.*

## Two different questions

[parity.md](parity.md) reports **≈ 99%**. That number answers one question: *does each After
Effects feature exist?* It scores a 92-item catalogue we wrote ourselves, graded by the same agents
that built the features, with partial features counting half. As a measure of breadth it is fair,
and the breadth is real: every one of After Effects' effects exists by name, along with the
menus, panels, 3D, tracking, expressions, scripting and export.

It does not answer the question users care about: *can someone who uses After Effects for a living
do real work in EffectCraft?* Nobody has measured that yet. Our best estimate is **≈ 30–50%**,
limited mainly by four things: projects we can't open, behaviour nobody has checked against After
Effects, reliability on platforms other than macOS, and the third-party plug-ins professional
projects depend on.

The checklist is also internally inconsistent: parity.md's per-area table still shows Interface
75%, Project 68%, Animation 70% and Paint 0% next to the 99% headline. Some rows are stale; none
of them have been reconciled.

## By dimension

| Dimension | Estimate | Evidence |
|---|---|---|
| Breadth of features | ≈ 95%+ | All 306 effects, the After Effects menus and panels, Classic and Advanced 3D, tracking, expressions, scripting, render queue (parity.md) |
| Behaves like After Effects | largely unmeasured, ≈ 60–80% | One outside contributor found four bugs in features marked done within a day (PRs #6–#10): Hold keyframes eased the motion into them, `keyInSpatialTangent` / `keyOutSpatialTangent` had the wrong names, a zero frame rate crashed, Find and Enter Full Screen shared Ctrl+F off macOS. There are about 10 After Effects reference captures in total. The first live G1 comparison now checks eight scalar Rotation/ease cases (168 samples) against AE 26.3x87 via AEsync 2.0.4: all pass after fixing overlapping temporal influences, with maximum error below 3e-11 degrees ([scores and reproduction](fidelity/README.md)). Other animation behavior and rendered frames remain unmeasured. User reports of 6 October found five more in features marked done (the Render Queue's template menu, angle revolutions, switches wiping the RAM preview, audio playback skipping frames, preview crashing when video memory ran out; all fixed 7 October) |
| Opening existing After Effects work | expanding | `.aep` binary RIFX projects and `.aepx` XML projects can be opened and imported directly, loading compositions, folder trees, layers, solids, text, cameras, markers and transforms. Third-party binary After Effects plug-ins cannot run. Expressions and the scripting object model are strong, so scripts and expressions carry over |
| Stability | improving | 23 never-crash PRs landed on 4 October; community PRs on 7 October fixed a hang on projects whose parent chains loop (#142), a panic on non-ASCII label colours (#135), and contained GPU initialization and device failures (#139); [AGENTS.md](../AGENTS.md) "Never crash" now binds every crate. On 5 October `cargo xtask ci` failed on main under Rust 1.99's clippy (a fix is in progress) |
| Real-user experience | ≈ 70%, uneven by platform | Issues #41–#47 (all from the Linux AppImage 0.1.1, 5 October): panning the viewer snaps back, panels can't be resized or rearranged, drag-and-drop and double-click import don't work, layer rename gets stuck, the Layer Settings arrow does nothing, the Project panel clips, the Wayland window icon is generic. Issues #63–#68 (macOS, 5 October): scaling a layer by its handles goes wrong and can stick at 0, two 3D compasses, the viewer lags while a layer is dragged (a frame renders in 20 ms but showed after ≈ 100 ms, behind RAM preview prefetch), the Discord and other links do nothing, Delete doesn't delete in the Project panel, hidden layers can be selected in the viewer. All but the last two were fixed or confirmed fixed with a test on 5–6 October (those two have community PRs); file drops still can't work on Wayland (winit has no support). The panning, rename and arrow bugs had been fixed in v0.2.0 already; nobody had told the reporter. A Windows user (6 October, on Discord) found the font menus listed only the three bundled fonts: installed fonts were read only when a project asked for one, on every platform. Fixed with a regression test on 6 October; the menus now list every installed family and its own styles, and agents get `text.fonts` / `list_fonts`. Seven reports of 6 October (Linux .deb and Windows): Project items and files couldn't be dropped on the viewer (#85), nor effects on a layer there (#88); drops in the Timeline ignored where they landed (#89); an angle's revolutions couldn't be edited (#93); toggling Audio, Lock or Shy threw away the RAM preview and playback with audio skipped frames (#103); running out of video memory panicked every frame thread and left frames stuck (#106); choosing a Render Settings template in the Render Queue did nothing (#117: any popup menu moved up to fit the window closed on the press). All seven were fixed with regression tests on 7 October. Most checking happens on macOS. No localisation, no accessibility work |
| Performance | unknown against After Effects | Internal numbers only (e.g. Advanced 3D 290 ms/frame at 1080p on the GPU, an M4 Pro under load). Nothing benchmarked against After Effects; no large real projects (4K footage, hundreds of layers) tested |
| Media formats | ≈ 80% | H.264, ProRes, HEVC, AV1, image sequences and audio exist. The new HEVC / AV1 encoders have no B-frames, multi-reference or SAO / CDEF, so files are larger than from mature encoders. Camera formats (BRAW, R3D, ProRes RAW, variable-frame-rate phone video) are unverified |
| AI-assisted tools | ≈ 50% | Both tools can use trained models (pure-Rust inference, optional downloads), but nothing compares them with After Effects yet. Roto Brush 2.0 / 3.0 with MobileSAM (M13.35) scores IoU 0.989 on the base frame of our synthetic moving-disc test and ≥ 0.980 over 20 propagated frames (classic: 0.973). Face tracking with MediaPipe Face Landmarker (M13.36) matches Google's own pipeline to 0.85 px on average on a test portrait; on our synthetic clip the eyes and chin stay within 4% of the face height |
| Maturity | early | First commit 1 October 2026. ≈ 285,000 lines of almost entirely agent-written Rust, ≈ 2,050 tests, about 30 external issue reports so far. After Effects has around 30 years of edge cases behind it |

## Where we're going: workstreams in priority order

Each workstream lists what "done" means, so progress can be measured rather than self-graded.

### G1. Measure fidelity against After Effects

The most important missing piece: it turns every other estimate here into a measurement.

- Build a corpus of original test projects (our own content, no Adobe assets), one or more per
  feature id in `plan/aftereffects/feature-catalog.md`: keyframe interpolation of every kind,
  blend modes, track mattes, each effect at default and at non-default settings, text animators,
  expressions, 3D, motion blur, time remapping.
- Drive After Effects through ExtendScript (see CLAUDE.md) to record property values at sampled
  times and render reference frames; render the same projects in EffectCraft headless.
- Compare values exactly and frames with a perceptual metric; publish a per-feature fidelity
  score and a fidelity column in parity.md.
- Clean room: After Effects output stays local in `plan/aftereffects/ref/` (gitignored). Commit
  only our projects, the harness and the scores, never After Effects frames.
- Done when: every P0 and P1 feature has at least one corpus project, scores are reproducible from
  one command, and parity.md reports breadth and measured fidelity side by side.

### G2. Real-user reliability on every platform

- Fix user issues as they come in, each with a regression test, and answer the reporter (#41–#47
  and #63–#68 are handled, as are the Windows report that the font menus missed installed fonts
  and the reports of 6 October, #85, #88, #89, #93, #103, #106 and #117; Wayland file drops wait
  on winit).
- Run the headless snapshot and control-channel checks on Linux (X11 and Wayland) and Windows, not
  only macOS. Interactions that only fail with real input (docking drags, viewer pan, drag-and-drop
  import, inline rename) need scripted input tests.
- Regression evidence for [#66](https://github.com/storytold/effectcraft/issues/66): scripted
  pointer tests in `ui_viewer` and `ui_text_edit`, checked headlessly on Linux, cover viewer clicks,
  marquee selection and text picking when layers are hidden or excluded by solo. They also check
  hidden/locked solo layers and solo layers outside their active time range.
- Regression evidence for [#67](https://github.com/storytold/effectcraft/issues/67): scripted
  input tests in `ui_project_delete`, checked headlessly on Linux, cover Delete/Backspace on
  Project items, multiple selection and undo. They check that an unrelated Timeline layer stays,
  an empty Project selection does nothing, viewer focus still deletes layers, and typing or
  dialogs do not delete items.
- Regression evidence for the reports of 6 October, checked headlessly on Windows: real pointer
  drags in `ui_drag_drop` (Project items into the Timeline between layers and at a time, and onto
  the viewer; an effect onto a layer in the viewer: #85, #88, #89), `ui_render_queue` (a template
  chosen from a menu moved to fit the window, #117), `angle_revolutions_scrub_and_take_typing`
  (#93), `audio_lock_and_shy_switches_keep_the_frames` and
  `preview_with_audio_shows_every_frame_and_sounds_once_cached` (#103), and
  `a_frame_whose_render_panics_is_released` (#106).
- Follow-up angle-input regressions in `ui_angles` (#141), checked headlessly on Linux and
  Windows, verify that Escape cancels edits and non-finite entries leave the value and undo
  history unchanged in Effect Controls, Timeline and Properties. Valid entries still commit on
  Enter or click-away.
- Done when: no open bug blocks a basic workflow (import, arrange, animate, preview, render) on
  any of the three desktop platforms, and every bug users report gets triaged within a day.

### G3. Stability and a green main

- Keep `cargo xtask ci` green on the current stable toolchain. A toolchain release that breaks the
  gate is a P0.
- Fuzz the inputs we don't control: project files, imported media and documents, expressions,
  scripts, control-channel and MCP requests. Every crash found becomes a regression test
  (AGENTS.md "Never crash").
- Done when: the gate is green on stable and the fuzzers run regularly without new crashes.

### G4. Open After Effects projects

- `.aep` / `.aepx` import is the largest single barrier to switching. It needs an **owner
  decision** on clean-room scope (whether and how the format may be studied), like the open
  `.prproj` question in `plan/STATUS.md`.
- Also in this area: relative footage paths and relinking when a project moves (parity.md, Project).
- Done when: the decision is recorded and, if approved, a corpus of real-world-shaped projects
  opens with its comps, layers, keyframes, effects and expressions intact.

### G5. Performance at real-world scale

- Benchmark projects at 1080p and 4K with many layers, heavy effects, long footage and nested
  comps; record preview frame times, RAM preview fill and render times on reference machines.
- Compare with After Effects on the same machine where possible.
- Done when: benchmark numbers are tracked over time and regressions fail a check.

### G6. Media depth

- Encoder efficiency: B-frames and multi-reference for HEVC / AV1, SAO / CDEF / restoration, Opus
  FEC / DTX.
- Verify decode of the camera and phone formats people actually bring, including variable frame
  rate; document what is not supported.

### G7. Learned models for Roto Brush and face tracking

- Decided on 5 October 2026 for Roto Brush: licensed weights with pure-Rust inference, if the
  impact on the system is low, in a composable, swappable module. Done in M13.35: the
  `effectcraft-segment` crate (a `MaskModel` interface, a registry that only accepts
  open-source licences, MobileSAM under Apache-2.0), weights downloaded on demand and verified,
  Settings ▸ Roto Brush to choose. Next: measure it against After Effects (G1); faster encoders
  (GPU) for long shots.
- Face tracking: decided the same way on 5 October 2026, done in M13.36: MediaPipe Face
  Landmarker (Google, Apache-2.0: BlazeFace detector and the 478-point Face Mesh V2) on a pure-Rust
  TensorFlow Lite interpreter, behind a `FaceModel` interface in the same crate, a 3.8 MB optional
  download chosen in Settings ▸ Face Tracking. The classical fitter stays as the fallback. Next:
  measure both against After Effects (G1).

### G8. Plug-in ecosystem

- After Effects SDK plug-ins cannot run in EffectCraft. Our own WebAssembly plug-in API exists
  ([plugins.md](plugins.md)). Grow it: documentation, examples, and original effects that cover
  what the most common third-party plug-ins are used for (particles, glows, 3D objects, sabers).

### G9. Reach

- Localisation of the interface, accessibility (keyboard navigation, screen readers, contrast),
  user documentation and tutorials.

## For agents choosing work

1. Read `plan/STATUS.md` for owner blockers and running work, then this file.
2. Prefer G1–G3 over new features. A feature that exists but behaves differently from After
   Effects is not done.
3. When you fix a behaviour bug in a feature parity.md marks as done, add it to the evidence
   above. That is how this assessment stays honest.
4. Don't raise parity.md's numbers without evidence: a G1 score, a user-facing check on more than
   one platform, or both.
