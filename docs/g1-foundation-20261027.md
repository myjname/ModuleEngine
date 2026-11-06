# G1 foundation: modules and verification

[English](g1-foundation-20261027.md) | [Русский](ru/g1-foundation-20261027.md) | [Documentation](README.md)

This is the first goal of the [approved G1 plan](g1-modular-plan.md), not a finished G1 or portfolio-game acceptance. The independent final decision and final source binding belong to the [external review](../outputs/reviews/g1-foundation-20261026/final-review.md). The second goal builds the ten-level game on these contracts; its gameplay, presentation, shipping package and performance need their own evidence.

## Module boundaries implemented

`module-engine-game-kit` has no Core, renderer, filesystem or window dependency. Its four independent features provide validated layered grids and projection, typed serializable Undo, stable sequential campaigns with configurable score ordering, and named physical/UI actions. These are actual library features, not dummy executable graph nodes. The [complete API](../crates/game-kit/README.md) explains consumer-owned rules and validation. An external formation fixture uses all four through a real registered system and GameSession in both world implementations; it is a bounded non-puzzle consumer, not another finished game.

Optional `persistence.json` stores a versioned namespaced opaque payload with bounded atomic publication. Applications choose writable roots; Editor Play uses a separate project personal root rather than the shipping profile. Core only accepts bootstrap data before simulation; games own the profile/attempt schema. A real Host test commits an event, writes a file, reconstructs a new Host from it, and verifies that a rejected tick leaves the file unchanged. Headless/omitted services still perform no IO. [Service contracts](../crates/host-services/README.md) specify failure and version behavior.

Optional `audio.wav` now owns a concurrent mixer and Windows waveOut output per service owner. It supports looping music, overlapping effects, separate gains, bounded voice allocation and owned shutdown. Legacy string cues remain valid; common PCM8/16/24/32, finite float32 and standard extensible PCM/float WAVs decode. The final float64 interpolation/accumulation preserves admitted extreme float32 values until the final output clamp. Native tests exercise both two-owner lifetime isolation and a registered service loading a real PCM8 asset through a legacy cue and event. Device completion is not a listening test.

The [UI/tool contract](game-ui-tools.md) lets source controls return complete GameInput and lets registered immutable editor tools propose authored candidates. Applications dispatch one collected input packet; Editor owns composition availability, permissions, validation, history and publication. A scheduled owner needs no redundant root toggle. Native export now resolves contract owners, presentation IDs and service IDs through composition-root package metadata rather than identifying ARPG/Puzzle in generic export code. Nested GPU pass owners participate in that mapping. Full-source guards include the new production sources.

## Evidence and source boundaries

Base HEAD is `f056a30e29b8ce3d73b40bbd40366e3dcbba6682`; work is uncommitted on `TechArt/g1-foundation`. Native-code candidate03 has digest `dfab3533ae0639d74871ed839befc87e3c2438f4a1fb99f5263e43c211d59157`. The final documentation snapshot is separately bound by review. No earlier P0 EXE, FPS or cold-start result is relabeled for these edits.

| Check | Actual scope/result |
| --- | --- |
| Pure modules | [12 ordinary tests and five smaller feature builds](../outputs/verification/g1-20261026/game-kit/feature-checks.json); independent tactical/drawing/racing/flight cases, not a single native game. |
| External registered consumer | [2 tests](../outputs/verification/g1-20261026/external-consumer01.log): both worlds, named/physical input, world changes, Undo/campaign/resume, rollback and omitted source rejection. |
| Current Host | [82 ordinary passed, 1 historical diagnostic ignored](../outputs/verification/g1-20261026/host-lib03.log), including the real registered persistence path. |
| Current services | [20 ordinary passed and strict scoped Clippy](../outputs/verification/g1-20261026/services/float03-result.json); two native cases executed separately. |
| Current Windows audio | [2 actual native cases](../outputs/verification/g1-20261026/services/native-audio03-binding.json), exact test artifact SHA `abaadfc1563caf0737631ee082a3cf4bcf8349a038f023e97e5c39deb6e0143a`; retained [test executable](../outputs/verification/g1-20261026/native-services03.exe). |
| Player compositions | [39 all-feature tests](../outputs/verification/g1-20261026/player-all03.log) and [13 no-default-feature tests](../outputs/verification/g1-20261026/player-minimal03.log), including positive and absent-renderer mapping. |
| Editor tools | [2 candidate/publication cases](../outputs/verification/g1-20261026/editor-tools01.log), [scheduled owner](../outputs/verification/g1-20261026/editor-scheduled01.log), [Engine versus Project/Team](../outputs/verification/g1-20261026/editor-engine-tool01.log). UI code did not change after these cases; later changes were audio, exporter metadata and documentation. |
| Export planning | [10 passed, 2 explicit native-builder cases ignored](../outputs/verification/g1-20261026/editor-package03.log). This is not a new finished-game export or a clean-machine check. |
| Selected contracts | [16 steps / 37 commands passed](../outputs/verification/g1-20261026/contracts01/20261026T233215Z-61f81f9c/result.json): Core, Project, compiler-product selection, Inspector and file/history preservation. This run is candidate02; subsequent native changes are only the mixer precision/goldens covered by current service/native checks. No new full-workspace test invocation is claimed. |
| Strict workspace Clippy | [All targets/features passed](../outputs/verification/g1-20261026/clippy03/20261026T234103Z-7c62aea6/result.json) on pinned native Rust. |

These sets overlap and must not be summed into a unique total. Source snapshots include new untracked sources; the generic verification launcher's tracked dirty digest alone does not. The external final review checks that mapping and the preservation proof.

## Problems found and repaired

Independent review caught first-error starvation between services, loss of accepted-prefix events after a later fixed tick failed, duplicate input callback execution, root-only tool availability, and inconsistent Engine tool publication. During the exporter refactor, actual package tests found missing nested GPU owners; their initial three failures remain recorded and the route was retaken successfully. Review also caught new WAV admission narrowing; common formats were restored. Extreme finite-float interpolation had an actual failing golden before the wider arithmetic fix. These implementation-stage defects are not all claimed as inherited product bugs.

Failed sandbox TEMP rename probes, incorrect empty exact-test discovery and the initial missing portable toolchain attempt remain evidence of fixture/environment mistakes. None counts as a product pass or justifies weakening runtime permissions. The [debugging skill](../.agents/skills/moduleengine-debug/SKILL.md) retains the verified TEMP/work distinction. Authored `game-project-v4.json`, prior assets/SDK/layout data and unrelated notes remain preserved; no user rollback was needed. The preservation receipt compares current protected data with the older EN baseline and the fresh contracts01 baseline. A new ignored-data/untracked baseline was not captured immediately before G1; this does not certify any unobserved intervening state.

## Limits and next consumer

Audio remains Windows-only, with no compressed streaming, spatial mixer or physical-audibility certification. The save service rejects incompatible versions and preserves bytes; it does not invent migrations for opaque game data. Save timing, multiple-process profile coordination and game-specific recovery UI need assessment in concrete consumption. No arbitrary binary plugin ABI or universal root/game scheduler has been added. Source packages still require native compilation and composition-root registration.

The next goal must create G1's crate rules, ten solvable layouts, menus, grid palette/painting policy, attempt validation, original art/audio and complete shipping workflow. It must check real authoring/input/visuals, save recovery, relocated tools-free Player, the agreed 1080p frame budget and chosen memory budget. Clean-machine and hardware Linux coverage remain separate, unverified environments. Reusable modules make those tasks possible; they do not establish mass-market quality without an actual finished game and inspection.
