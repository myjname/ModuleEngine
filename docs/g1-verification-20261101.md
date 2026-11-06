# Crate Garden: foreground verification and audio repair

[English](g1-verification-20261101.md) | [Русский](ru/g1-verification-20261101.md) | [Game guide](g1-game.md) | [Engine assessment](g1-engine-assessment.md)

The remaining Windows G1 routes have now been exercised with real OS input,
including Editor authoring, standalone play, recovery and the agreed 1080p
workload. The [independent final review](../outputs/reviews/g1-game-20261101/final-review.md)
governs Goal2 acceptance. Goal1 retains its historical independent acceptance;
the [two-goal agreement](g1-modular-plan.md) is unchanged. This continuation
supersedes the open foreground gates in the [October report](g1-delivery-20261027.md),
without changing that report's historical results or starting G2.

## Sources and deliveries

The measured implementation is [Source16](../outputs/reviews/g1-game-20261101/candidate16.json),
digest `26526ab52b5516ca3dc576f0776af15965c47064de6a01be79888c3eb8c062b1`,
at HEAD `f056a30e29b8ce3d73b40bbd40366e3dcbba6682`, with local uncommitted work.
The normal Editor SHA-256 is
`e8f3ab54ae01a9843bfe369417227059a4a3a61bbe932d0e050c9d60f99bfd3c`;
the selected Player and checked-prebuilt Team Player SHA-256 is
`61d583b291179a79316c96b22dfbc10afd50a6323f505a67b22fd9cddeae80ec`.

[Native16](../outputs/verification/g1-20261027/native-stage16/result.json) verifies
the normal Full/New G1/ten-level functional route, 184 native inputs and 430
exported source files. [Package08](../outputs/verification/g1-20261027/package-stage08/result.json)
passes all 11 actual export/rendered/headless/relocation/Team commands. Run the
retained [Player08](../outputs/verification/g1-20261027/g1-player-relocated08/Player.exe):
24 sealed files, 26,192,550 bytes, features `audio,g1,persistence,ui,vulkan`,
14 selected assets and six precompiled SPIR-V shaders. ARPG, Editor and compilers
are absent from this selected game package. Same-host tools-off relocation is
not a clean-machine installation check.

The final documentation-only Full delivery is recorded separately by
[Native17](../outputs/verification/g1-20261027/native-stage17/result.json).
Use its [Full13 Editor](../outputs/verification/g1-20261027/source-full13/ModuleEngine.exe)
and associated [editable project](../outputs/verification/g1-20261027/g1-authored13/project.json).
Its unchanged native code, assets and retained normal binary do not create a new
UI or timing measurement; [Source17](../outputs/reviews/g1-game-20261101/candidate17.json)
and the final review bind the documentation separately. Keep the project's assets
and Engine binding together. The old published checkout EXE was not replaced.

## Defects found and repaired

- Direction buttons displayed missing font glyphs. They now paint vector arrows,
  retain all four actions and accessible names, and scale with the game canvas.
  The one-move victory message now uses the singular.
- Expanded save diagnostics escaped a fixed-height warning background. The
  panel now covers its measured contents; long details wrap and scroll within
  the viewport. Actual corrupt-save Menu and Settings captures verify readability.
- The common Windows audio module fed only about 75% of real-time audio with
  three queued 10 ms buffers on this endpoint. A quiet native regression measured
  3.750 seconds of source audio during 5.003 seconds of wall time. Four buffers
  restored 5.010 seconds during 5.003 seconds; repeated native checks agree.
  The bounded queue now holds 40 ms of generated audio. Sample rates, system
  settings and ownership/retirement rules were preserved.

The [audio review](../outputs/reviews/g1-game-20261101/audio-cadence-review.md)
distinguishes the production mixer's source-clock measurement from device pitch,
physical latency or universal scheduling guarantees. Nonzero completion counters
had missed the slowdown. The user confirmed normal native sound after the fix;
Source16 retains that Source15 audio implementation and soundtrack unchanged.
The separate game-owned soundtrack was also revised to an original 20-second
major-key keyboard phrase. All seven effects and the other assets remain byte-identical;
15 original assets now total 1,149,976 bytes. Track replacement alone did not fix
the playback defect. No game-specific workaround was added to the audio service.

## Functional and performance evidence

Source16 G1 checks pass **42 ordinary tests / 1 explicit timing ignore**, including
real egui clicks, arrow geometry, warning bounds and reset confirmation. The
unchanged audio implementation passes **30 Host-service ordinary tests / 4 native
ignores**, with all **four native audio cases separately executed and passed**.
Format and affected strict Clippy checks pass. These scoped checks are not a new
full workspace run; October's broader suites retain their original bindings.

Actual [Editor03](../outputs/verification/g1-20261101/foreground-editor03/result.json)
on Source15 covers Windows→palette, painting one wall, Save's pending-draft guard,
Apply, Undo/Redo, Save, Recent-project reopen, Play/victory and Stop. Its 23 retained
PNGs and saved JSON prove exactly one added wall at cell `(1,1)`; other authored
values and original resource files are unchanged. Save's backup equals the
original JSON, and the separate Editor Play profile is explicitly accounted for.
Source16 changes only the game's warning branch; that unaffected authoring route
is retained as Source15 evidence, not relabeled as a new run.

Player02/03 verify all four onscreen directions, goal tint, Undo, saved continuation
and Undo after relaunch, Restart and independent persisted volumes. Their 13 PNGs
retain Source11 binding. [Recovery05](../outputs/verification/g1-20261101/foreground-recovery05/result.json)
uses the current Source16 Player and an owned corrupt profile: readable warning,
Settings, separate confirmation, authorized Reset, cleared warning and newly
saved victory. The recovery copy is byte-identical to the 50-byte corrupt fixture;
the new valid slot records unlocked level 2 and a one-move level01 best. Original
seed/user profiles remain unchanged. [Author observations and data proofs](../outputs/verification/g1-20261101/final-resume-evidence.json)
bind these routes; the reviewer separately inspects the artifacts.

[Perf10](../outputs/verification/g1-20261027/perf-playing10/result.json) measures
the actual Source16 Player at 1920×1080 on Garden Master. Continue→F9→onscreen
push→keyboard Undo stays in Playing throughout, with music, effects, UI and saves.
All 11 recorded gates pass; 2,400 genuine distinct frames and 2,928 UI callbacks
are retained without filtering.

| Interval | Mean, ms | p95, ms |
| --- | ---: | ---: |
| UI callback | 6.9442 | 7.8540 |
| Producer completion | 8.4730 | 9.8971 |
| UI-observed distinct frame | 8.4723 | 14.0901 |

Both mean and p95 meet 16.67 ms. Maximum UI-distinct interval is 31.148 ms;
the criterion does not say every frame is below budget. Peak working set is
338,776,064 bytes (323.08 MiB), below the chosen 1 GiB budget. Normal presentation
has zero measured CPU pixel readback/upload and no measured render/service errors.
The current [hardware observation](../outputs/verification/g1-20261101/hardware02.json)
records RTX 4080 SUPER, driver `32.0.16.1742`, desktop 3440×1440 at 144 Hz;
the benchmark's physical render resolution is separately 1920×1080.
Process-to-GUI-initialized log is 0.922 seconds, not cold startup, first scanout
or end-to-end hardware-input latency. This is a fixed-content acceptance check,
not a causal A/B claim or a guarantee for arbitrary projects.

## Preservation and limits

Computer Use injects actual OS input; it is not human hardware input. Sessions
were reset between batches and before non-UI work. Earlier approval timeouts,
unarmed Perf06/07, the zero-test filter attempt and the incomplete onscreen route
in Perf08 remain explicit evidence, not successful replacements for Perf10.
Editor01's missing diagnostic directory was a fixture error; Editor02's timeout
was an access/verification failure. Neither was repaired by weakening product checks.

Original `game-project-v4.json`, unrelated notes, personal data, old packages,
root metadata and the published EXE were preserved. Only owned verification
projects/profiles were edited or reset. No user rollback, commit, push or PR was
performed. Work remains on `TechArt/g1-foundation`.

This closes concrete Windows G1 functionality and budget evidence, subject to
the final independent decision. Hardware Linux, native Linux audio, mobile ports,
clean-machine installation, broad user testing and arbitrary-game suitability
remain outside this acceptance. G1 proves a small finished project on the shared
modules; the [engine assessment](g1-engine-assessment.md) retains the architectural
limits. Agree G2 separately before starting its implementation.
