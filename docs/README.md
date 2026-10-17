# ModuleEngine documentation

[English](README.md) | [Русский](ru/README.md)

English is the primary documentation language. Every document below has a complete Russian counterpart, available through its language links. Start with the current status; historical reports establish only the snapshots and checks they name.

## Start here

- [Working with the ModuleEngine assistant](assistant-workflow.md)
- [Debug ModuleEngine behavior](../.agents/skills/moduleengine-debug/SKILL.md)

- [ModuleEngine 0.4](../README.md)
- [Working on ModuleEngine](../AGENTS.md)
- [Current ModuleEngine Status](current-status.md)
- [Architecture 0.4](architecture.md)
- [Decisions and Working Assumptions](decisions.md)
- [Open Questions After 0.4](questions.md)
- [Game stage: from authored scene to Player](next-stage.md)

## Editor and authoring

- [Editor Menus and Project Workflow](editor-workflow.md)
- [Authored scenes and the gameplay system graph](editor-game.md)
- [Scene and Assets](scene-assets.md)
- [Authored scenes, templates and Play copies](scenes.md)
- [Engine and Project settings](settings.md)
- [History, merging and assets](history-assets.md)
- [Assets: Identity, References, and File Operations](asset-workspace-api.md)
- [Source component schemas and composition](component-schemas.md)
- [Melee combat visual feedback](combat-feedback.md)

## Runtime, modules, and rendering

- [Gameplay ECS storage](ecs-world.md)
- [Gameplay storage decision](ecs-storage-decision.md)
- [Executable systems and the authored schedule](module-execution.md)
- [Project Format 0.4](graph-format.md)
- [Reusable gameplay modules and the acceptance game](gameplay.md)
- [Optional gameplay packages](optional-gameplay.md)
- [Crate puzzle project](puzzle-project.md)
- [Shared authored-game host](shared-game-host.md)
- [Running Game outside the UI](game-worker.md)
- [Custom Rust 0.4](custom-rust.md)
- [Rust Custom State and Standalone Workers](custom-state.md)
- [Reusing Custom Rust](custom-program-store.md)
- [Static Models, Textures, and Materials](content.md)
- [Vulkan and Custom HLSL](gpu-hlsl.md)
- [Adding a module](extending.md)

## Build, delivery, and verification

- [Pinned Windows tools](toolchain.md)
- [ModuleEngine CI](ci.md)
- [Verification Routes](verification.md)
- [Independent Change Review](review.md)
- [Review: <task>](review-template.md)
- [End-to-end game acceptance and timing evidence](game-verification.md)
- [Engine profiles, compatibility and a standalone Player](engine-profiles.md)
- [Full, Team and a separate Player](distribution.md)
- [Standalone Game and Verification](player.md)
- [Linux setup and verification boundaries](linux-support.md)
- [Reproducible artifact maintenance](artifact-maintenance.md)
- [Optional host services](../crates/host-services/README.md)
- [Standalone Player](../crates/player/README.md)

## Stage reports and historical evidence

- [Results of the 0.4 Game Stage](release-v04.md)
- [UI stage: menus and project workflow](release-ui.md)
- [Scene, Assets, and responsive editing](release-scene-assets.md)
- [Functionality and stability 0.4 — October 15, 2026](stability-audit.md)
- [Modular runtime migration](modular-runtime-progress.md)
- [Linux preparation: verification on 15.10.2026](linux-support-20261015.md)
- [Desktop stalls when launching ModuleEngine — 15.10.2026](desktop-stalls-20261015.md)
- [Review fixes and Custom repreparation — 16.10.2026](review-fixes-20261016.md)
- [Four additional P2 findings after review 17fb340](postfix-p2-fixes-20261016.md)
- [PropertySchema assets and Inspector preservation after review 5e3f064](remaining-authoring-fixes-20261016.md)
- [System dialog paths and the Save As transaction](dialog-path-fixes-20261016.md)
