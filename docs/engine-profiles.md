<a id="профили-engine-совместимость-и-самостоятельный-player"></a>

# Engine profiles, compatibility and a standalone Player

[English](engine-profiles.md) | [Русский](ru/engine-profiles.md) | [Documentation](README.md)

An Engine profile defines the published configuration and available module library.
A format 4 project saves the profile itself and exact contract versions in `engine_lock`:
module implementations, component-schema owners and structure, system
state schemas and the Custom SDK identifier. `module_versions` remains the sole map
of module versions.

A module's availability in a profile does not enable it automatically. Activity is determined
by connected nodes, settings and the system schedule. Inactive pins are preserved
as an archive; they do not by themselves require enabling the module in the native Player build.
Inspector uses active component contracts.

<a id="новый-проект-и-смена-контрактов"></a>

## New project and contract changes

In **File → New Project**, select **Inherit current Engine** or, in Full,
**Choose Neutral Engine**, **Choose ARPG Engine** or **Choose Puzzle Engine**.
Then select **Empty scene**, **Two arenas example** or **Crate puzzle example**.
The template contents must be compatible with the selected profile.

In Full, a new Empty uses the profile's saved schedule; stock profiles
publish an empty schedule without ARPG. Inheritance preserves the current Engine
graph and its settings. Team preserves the pinned Engine schedule,
and replaces editable Project Rust code with an empty `update`.
If pinned systems require a particular scene, an incompatible template
is rejected before the project is created. For example, Team with Puzzle Engine does not create
an empty scene without the required board/pawn/crate.

Ordinary Open, Save, Play and export validate contracts and do not update them silently.
In Full, **Tools → Publish Engine profile / update contracts** explicitly accepts
the installed active contracts and creates a new profile version in the working document.
**Save Project** saves this change. Local edits to the Full graph and settings
are allowed within available modules and pinned contracts. In Team, the Engine
architecture is frozen by the delivery. These are authoring rules, not isolation of untrusted code.

<a id="миграция-существующего-проекта"></a>

## Migrating an existing project

Formats 1–3 cannot be opened with an automatic update. Create a separate copy,
explicitly specifying the target profile `neutral`, `arpg` or `puzzle`:
Commands are shown for the repository root; in a Team delivery, the EXE is at its root.

```powershell
.\outputs\ModuleEngine.exe --migrate-copy .\game-project.json .\game-project-v4.json arpg
```

Migration compares known source contracts against the shipped snapshot of target
contracts, checks resources and compiles the Custom code in use before publication.
The source file is not overwritten. The command prints a report with a hash of the source bytes
and a list of changes; save it with the project history.

The destination must be a distinct project file. On a case-sensitive Linux
filesystem, `project.json` and `PROJECT.json` can be separate source/copy names;
Windows retains case-insensitive path comparison. A true source alias or an
occupied destination is still rejected by path/publication checks. Migration
preserves the original JSON, Engine binding and resource bytes.

Unknown fields of typed records are preserved in `legacy_archive`, and unknown
inactive component JSON in authored data. An unknown active contract
or invalid known field rejects migration. The adapter supports known
first-party versions, not arbitrary third-party plugin formats. For
formats 1–3, it uses a common set of known fields, without a separate table
of the version in which each field appeared.

For the known old `boss` contract, migration preserves the previous behavior:
a missing `victory_on_death` becomes explicit `true`, while an authored `false`
is preserved. In the new reusable ARPG, the default is `false`;
the project selects the victory condition. Unknown inactive versions are not interpreted.

<a id="экспорт-windows-player"></a>

## Windows Player export

Use **File → Export → Windows Player…** or the command:

```powershell
.\outputs\ModuleEngine.exe --export-game .\game-project-v4.json .\outputs\my-game
.\outputs\my-game\Player.exe
```

The destination must be a new or empty folder. Full separately builds
`module-engine-player` with the required native features. Stock Puzzle uses
`ui,puzzle,vulkan`, and Two Arenas uses `ui,arpg,vulkan,audio`. Owners of
active schemas declared by Custom are also considered, even without an ARPG/Puzzle schedule. The editor
exe is not used as Player. Team takes a verified prebuilt variant; when
`game.audio` is pinned, the delivery contains variants with and without audio so
allowed changes to sound cues do not require Engine sources.

`player.json` v2 pins the exact native feature set, SDK data/API identifier
and hashes of resources, Rust workers, SPIR-V and `Player.exe`. Export validates the copied
Player and performs a trial run with rendering before publishing the folder. Running
the finished game requires no Rust/Cargo, DXC, SDK or source checkout; graphics
mode requires a compatible GPU driver.

```powershell
.\outputs\my-game\Player.exe --verify-package .\outputs\my-game .\outputs\player-check
.\outputs\my-game\Player.exe --headless-check .\outputs\my-game .\outputs\headless-check
```

`--verify-package` checks the package, execution and rendering. `--headless-check` executes
the world without creating a GPU or native audio. Choose a report folder separate from the package.
The Player library with default settings does not depend on UI, Vulkan, ARPG or audio.

Without scheduled arbitrary Rust, export keeps only resources
used by active schemas and services. Rust can form asset IDs
dynamically, so the entire declared manifest is retained with it. Without a request for the
audio service, audio is not created and cue files are not added. A dependency's absence
from a native build is confirmed by its Cargo features/dependency graph; a hidden
panel, inactive pin or unscheduled system does not prove it.

<a id="custom-sdk-и-фоновые-задачи"></a>

## Custom SDK and background tasks

Custom compilation checks `engine-sdk.json` before launching Cargo, then verifies
the identifier of the SDK actually linked. The same identifier is sent
in worker packets and prebuilt-code metadata. A mismatch
rejects the new code and preserves the last working version with its state.
The data/API fingerprint describes the data and API contract; it does not promise a stable Rust ABI
or compatible in-memory representation of Rust types.

Cancellation of stale preparation or compilation is requested without waiting in the UI.
Background owners finish and release processes and their temporary files
outside the window loop. Therefore, after the window closes, the process may still wait for
cancellable work or a GPU task and cleanup to finish; this wait does not block a running UI.
Cancellation does not guarantee an immediate stop of an operation with no cancellation point.

Manual cleanup of old reproducible artifacts is described in
[artifact-maintenance.md](artifact-maintenance.md). It does not run during ordinary
project opening or saving; inspect the dry run first.
