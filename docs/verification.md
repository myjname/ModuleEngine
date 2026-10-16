<a id="маршруты-проверок"></a>

# Verification Routes

[English](verification.md) | [Русский](ru/verification.md) | [Documentation](README.md)

The [scripts/verification.json](../scripts/verification.json) registry and stdlib launcher
[scripts/verify.py](../scripts/verify.py) select existing checks. They do not
change Rust, Cargo profiles, product watchdogs, or performance criteria.
The new route does not replace full acceptance from historical reports.

<a id="запуск-и-окружение"></a>

## Launch and Environment

New profiles require Python 3.9+ without third-party packages and an already configured native
Rust/Cargo toolchain. Nothing is installed or downloaded. Cargo runs
with `--offline --locked`. Verify pinned tools using the existing
`scripts/bootstrap.ps1 -Json`; restoration remains a separate explicit operation.
On Linux, source the existing `.tools/linux-x86_64/env.sh` and check the conditions in the
[Linux guide](linux-support.md). The launcher inherits the toolchain and normal loader
environment, including legitimate DXC `LD_LIBRARY_PATH`; it does not configure them again.
The Windows wrapper for new profiles also does not replace `CARGO_HOME`: if the ready
offline registry is in `.work/cargo-home`, before running set
`$env:CARGO_HOME = (Resolve-Path .work/cargo-home).Path`. Use the actual
installed cache; Cargo diagnoses missing packages, rather than fixing them
through downloading or cleanup.

```powershell
$python = 'C:\path\to\python.exe'
.\verify.ps1 -List -Python $python
.\verify.ps1 -Profile contracts -Plan -Python $python
.\verify.ps1 -Profile quick -Python $python
```

`-Python` takes precedence over `MODULE_ENGINE_VERIFY_PYTHON`. The wrapper then checks
available Python/py. WindowsApps aliases are not launched; a personal Python path
is not stored in the repository. `-Plan`/`-List` do not run Cargo/bootstrap or create
run directories. Direct Python execution uses the same CLI:

```sh
python3 scripts/verify.py --list
python3 scripts/verify.py --profile native --plan
./verify.sh --profile quick
```

`MODULE_ENGINE_VERIFY_PYTHON` can also specify the shell wrapper's interpreter.
Direct `verify.py` defaults to `quick`; **old wrappers without
arguments retain the previous heavy legacy suite**.

<a id="профили"></a>

## Profiles

| Profile | Scope | What it does not confirm |
| --- | --- | --- |
| `quick` | `cargo fmt --all -- --check`, all lib tests of `module-engine-core` and `module-engine-project`. | Not the workspace, Editor, Vulkan runtime, secondary Custom builds, or ignored acceptance. Cold compilation can still be slow. |
| `contracts` | Quick plus `cargo_build::tests::`, four Inspector regressions, 99 Inspector property scenarios, four previous Save As tests, and 28 workflow preservation scenarios. | Not all UI, native dialogs, or the full authoring suite. |
| `native` | Five explicit ignored tests: Stop/reuse, slow-UI/hot-apply, native linker, cache context, and Windows executable spellings. | Not desktop benchmark, screen latency, Full export, or strict 60 FPS. A Windows-only test is marked `skipped` on Linux, not passed. |
| `native-tools` | Three native linker/cache/executable-spelling checks from `native`, without GameUI or GPU. | Not full `native`; a Windows-only test is marked `skipped` on Linux. Used by Windows CI. |
| `package` | Two ignored `game_package` checks: native target modes and concurrent immutable snapshots. They build minimal Cargo fixtures and execute their products. | **Not a real Full/Team/Player game export, relocation, or full delivery acceptance.** These CLI checks remain a targeted/legacy route. |

Checks run sequentially. For Rust tests, the launcher first obtains
`cargo metadata`, then `cargo test --no-run --message-format=json` for the selected
package/target. `contracts` uses a normal test build; `native` and `package` use
release. The actual native host is explicitly specified through `--target`; a different explicit
`CARGO_BUILD_TARGET` is rejected. The target directory comes from Cargo metadata,
not an assumed `target/debug` or `target/release` path.

The direct test executable is selected by package ID, canonical manifest, target
name/kind, and `profile.test=true`. Requirements are a successful Cargo process, exactly one
`build-finished.success=true`, one distinct executable, and an existing native
PE/ELF inside the target directory. `fresh=true` is allowed. The path and SHA are recorded;
file substitution between steps is rejected.

The test binary runs **directly in the normal loader environment**, not through
`cargo test` or `cargo run`. Exact tests first require a unique
`--list --ignored --exact <name>` result; zero tests never counts as
success. After execution, the libtest summary is checked against the discovered set. This
preserves the important Stop/reuse condition from the [previous loader check](postfix-p2-fixes-20261016.md).

## CI

[GitHub Actions and the required check](ci.md) run `contracts` on Windows and
Linux and add native/package routes for source changes. Before offline
checks, CI explicitly restores tools and locked dependencies. Enforcing
merge requirements needs an applied branch rule; having YAML is not
proof of a first successful hosted run or configured protection.

<a id="проверки-свойств-inspector"></a>

## Inspector Property Checks

Three `game_ui::tests::inspector_generated_*` tests in
[game_ui.rs](../crates/editor/src/game_ui.rs) are included in `contracts` as separate
exact tests. This is a finite reproducible matrix without a new dependency:
six keys (`literal.key`, `0`, empty, `/~`, `a/b`, `a~b`) and eight payloads
(null, bool, number, string, empty object/array, nested object, and mixed array).

- 48 cases check that Apply without editing preserves the authored document
  and does not rename an existing local opaque member after save/reopen.
- 48 cases check the absence of a local delta for inherited data and
  receipt of future prefab/defaulted-field changes after save/reopen.
- Three positive controls check a real registered edit, preservation
  of an existing override, and an intentional local opaque-aggregate edit.

They use real egui Apply, production save/read, and document-contract
validation. Expectations are the original authored document and explicitly specified subsequent
values; tests do not reproduce the flatten/merge/ownership algorithm. The
`get(key)` check distinguishes present null from an absent member; inactive unknown
payload and neighboring namespace `foo.bar` must survive. A positive edit
control prevents accepting a no-op Apply as successful verification.

The matrix is not random fuzzing and does not prove all possible JSON or
Assets operations. For a new defect, add its minimal case and extend
the corresponding property while retaining existing typed-reference/default checks.

<a id="сохранность-save-as-и-файловой-истории"></a>

## Save As and File History Preservation

Three exact tests `app::editor_workflow::workflow_tests::preservation_*` in
[editor_workflow.rs](../crates/editor/src/editor_workflow.rs) use real
commands, worker, save/read, and file history. The matrix is finite, without fuzzing or
new dependencies:

- 18 cases: six exact keys/values and binary payloads through Cancel,
  an occupied destination resource, and successful Save As. They check original bytes,
  dirty JSON, Play/history/Recent, opaque payload, stable asset ID, and Engine binding.
- Four Rename/Delete × Undo/Redo cases: JSON publication failure must roll back
  files and preserve history. The only permitted staged recovery JSON
  is compared with the candidate; retry after restoring the path must pass.
- Six history-branching cases: Rename → Undo → Delete prevents returning
  to the discarded branch through Redo. Stable IDs, unknown/null payload,
  binary bytes, the neighboring resource, and original binding are preserved.

Expectations are the original document, saved bytes/hash, and strict reopen,
rather than repeating the rollback algorithm. Successful Save As is a positive control.
Four existing `workflow_tests::save_as_*` tests also belong to `contracts`:
picker error, same-path publication failure, copy, and invalid JSON.

This does not cover crash/power-loss recovery, every possible I/O failure, or GUI dialogs.
A resource conflict checks rejection before publication, not rollback of partial completion.
Actual write failure is separately checked in Undo/Redo. On WSL/DrvFS, Full fixtures
are slower; the limit for the two large matrices is 300 seconds, without changing product watchdogs.

<a id="выбор-по-изменению"></a>

## Selection by Change

| What changes | Initial route | Additional evidence |
| --- | --- | --- |
| Only neutral Core or Project data/history | `quick` | Targeted regression for the changed contract. |
| Inspector, schemas/defaults, prefab, authored JSON | `contracts` | A separate scenario for new UI behavior; four previous regressions do not check every window. |
| Save As, Assets, and file Undo/Redo history | `contracts` | A targeted scenario for the new failure; bounded matrices do not replace other export/authoring contracts. |
| Custom compiler, toolchain, store, lifecycle/slow ticks | `contracts`, then `native` | Measurements without competing native/GPU/build operations; desktop/cold-start are checked separately when those requirements change. |
| Cargo product selection or Player packaging | `contracts`, then `package` | For changed export/delivery, separate Full/Team/Player acceptance of the real game. |

This is an initial set, not permission to skip checking new behavior. For a
fix, reproduce the error first; after editing, repeat that scenario
and affected contracts. Repeating the entire legacy suite after a documentation edit
or a successful small test change without a new reason is unnecessary.

<a id="inventory-ignored-tests"></a>

## Ignored Test Inventory

All nine `#[ignore]` entries are listed in the registry with their purpose. Seven are included in
`native`/`package`; two are intentionally excluded:

- `runtime_delivery::tests::emit_stage4_software_inactive_vulkan_fixture` — only
  fixture preparation; requires a new absolute `MODULE_ENGINE_STAGE4_FIXTURE_PATH`.
- `compat_tests::audit_slow_custom_catchup_wall_time` — historical diagnostics;
  not a gate for new behavior or performance.

The registry does not provide an arbitrary `--step` that could silently enable the emitter.
A new ignored test must be deliberately added to the inventory and appropriate profile.

<a id="сохранность-сериализация-и-результат"></a>

## Preservation, Serialization, and Result

Each run creates a new `outputs/verification/<UTC-time>-<nonce>/` with
`result.json` and separate stdout/stderr logs for every command. Launcher-owned
TEMP/TMP/TMPDIR point to a unique short `.work/v/<run-token>` only in the child
environment. Existing tests may create their own isolated fixtures
in `.work/test-temp` and use caches according to their contract. A short TEMP
reduces nested MSVC object paths; the resolved path must remain within the checkout,
and the actual `temporary_directory` is recorded in the report. `--output-root` is allowed only inside this checkout's
`outputs`; existing reports are not overwritten.

One OS file lock, `work/verification/launcher.lock`, serializes all executions
of the new launcher in this checkout, including shared GPU/native setup and
competing build steps. If the lock is occupied, a second launch exits with an explicit
error. The OS releases the lock when the process closes; the stale file need not be deleted.
This lock **does not block legacy wrappers, external programs, or manually invoked
Cargo**. Continue running native/GPU/timing scenarios without such competitors.

Before and after steps, the inventory of root JSON, `assets`, `game-assets`,
`.user`, `.history`, `.asset-history`, and `.asset-trash` is checked. New/changed/deleted
protected files mean failure. The launcher does not reset, clean up, or
automatically restore user data. This monitors changes; it is not a filesystem
sandbox, and does not protect external linked resources beyond these directories.
Selected tests themselves use their own fixtures, not authored game JSON.

The report records UTC timestamps, profile/scope, SHA of registry/launcher/wrappers, available HEAD and
a digest of tracked dirty diff, native host/toolchain, commands/cwd/exit codes/timeouts,
logs, selected executable paths/SHA, and test names. If Git is unavailable, a
snapshot error is recorded; untracked source is outside the dirty-diff digest. The protected author
inventory is recorded separately. This is not new certification of a published EXE.

Statuses `passed`, `failed`, `skipped`, and `not_run` are distinct. After an error, subsequent
steps do not run; a platform skip contains a reason. A preflight error leaves
steps `not_run`. A nonzero exit or missing program cannot inherit
success from a previous process. On timeout, only the process tree started by the launcher
is terminated; cleanup results are recorded. Windows uses `taskkill /PID /T`,
Linux its own process group; this does not manage unrelated processes.
`--timeout <seconds>` is an explicit per-command limit, including compilation; it does not
change the product's internal watchdog. The default build timeout is 1200 seconds; test
timeouts are specified in the registry.

<a id="legacy-и-performance"></a>

## Legacy and Performance

`verify.ps1` and `verify.sh` without arguments retain their previous scope, order, and
settings. `verify.ps1 -NativeBenchmarks` still does not require Python. Only
normal release Editor selection was fixed: the executable comes from CargoJSON with
identity/unique successful product and native PE checks rather than a manually constructed path.

There is no new separate `performance` profile. Legacy `-NativeBenchmarks` runs
the entire previous suite, then native Editor/Player benchmarks on the selected legacy
project; its historical outputs are reused. This legacy route does not
automatically gain new author-file guards, timestamp reports, or the lock. Editor
benchmark requires an isolated Full package: creating Editor may write Recent.
For strict acceptance, use a separate verified Full package/environment and
targeted normal executable commands from the relevant report; do not run
the legacy benchmark on the only authored project.

A single legacy script exit 0 does not confirm Player `target_met=true`: read
`native-frame-report.json`. The distinctions between functional completion,
`measurements_valid`, cold/prewarmed, hardware/software Vulkan, and strict cadence remain.
New native/package profiles also do not complete the open goals from
[current-status](current-status.md).

<a id="проверка-launcher-без-cargo-и-gpu"></a>

## Launcher Verification Without Cargo or GPU

```powershell
& $python -B -m unittest discover -s scripts -p test_verify.py -v
```

Fixtures are created only in `work/verification-tests`. Tests use synthetic
CargoJSON/PE/ELF, mocked test processes, and their own short Python children
for exit/timeout; Rust/Cargo/native engine are not launched. They check profile
isolation, all nine ignored entries, OS skip, selector failures, zero-test
rejection, normal environment, lock, failure reporting, and author-file preservation.
This is unit verification of the launcher; real profiles must pass separately in the
target Windows/Linux environment on the current source snapshot.

<a id="проверка-внедрения-16-октября-2026"></a>

## Rollout Verification on 16 October 2026

The work was performed on top of `c64bc34ffd28419cfd851838c16a2a48c9c2fcf8`, preserving
existing user changes. Stages were checked sequentially:

| Check | Result |
| --- | --- |
| Project context | Current contracts and open goals clarified; links and UTF-8/LF checked. |
| Launcher and wrappers | 18 Python unit tests; separate dispatch/legacy/shell fixtures and 20 mocked CargoJSON/PE cases. |
| Windows, routing stage | `quick`, `contracts`, `native` (5 tests), and `package` (2 tests) passed. |
| Linux Ubuntu/WSL, routing stage | `quick` passed: 102 Core + 59 Project tests. |
| Windows after new properties | `contracts` passed: 102 Core + 60 Project + 9 Cargo + 7 Inspector = 178 tests. Three Inspector tests ran 99 new scenarios. |
| Linux after new properties | `contracts` passed: 102 Core + 59 Project + 10 Cargo + 7 Inspector = 178 tests, including the same matrix. The platform-specific test set differs from Windows. |
| Defect sensitivity | Temporarily reverting to dot-path writes instead of exact members produced the expected failure on `key-0-null`; production code was restored byte for byte, then Windows/Linux contracts passed. |
| User work | All 11 initially protected files retained SHA-256, including `game-project-v4.json`, service data, and four previous EXEs. |

Commands, actual test executable/SHA, and logs are in separate
`outputs/verification/<run-id>/result.json`; run indexes and checksums
are collected in the [rollout results](../outputs/development-rollout-20261016/acceptance.json).
The Rust diff of this stage adds tests only; Cargo dependencies did not change.

The first manual Windows baseline without TEMP setup received access failures;
a repeat with local TEMP passed. New profiles use their own TEMP.
The Git Bash mock harness was also repeated outside the restricted sandbox after MSYS
failed to create a service object. These are preserved environment failures, not silently declared
successes. In WSL, file tests on `/mnt/e` are noticeably slower than on a native filesystem;
their duration is not an engine measurement.

Full legacy suites without arguments, desktop/performance benchmarks, and real
Full/Team/Player delivery acceptance were not repeated in this rollout. Linux
`native`/`package` were not run by the new launcher. Open cold-start/60 FPS and
hardware Linux goals are not completed by these checks.
