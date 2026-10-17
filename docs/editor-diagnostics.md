# Editor diagnostics

[English](editor-diagnostics.md) | [Русский](ru/editor-diagnostics.md) | [Documentation](README.md)

[scripts/editor_diagnostics.py](../scripts/editor_diagnostics.py) runs bounded diagnostics
with an existing normal native Editor executable. It creates its own fixture, reuses
`--editor-ui-check` for workflow checks, and records native WGPU captures. The wrapper
does not build an Editor and accepts no authored project JSON as input. The Windows
runs below confirm this bounded route; Linux native capture and hosted CI for this
change remain unverified.

## Run

Use an installed Python supporting the launcher and an absolute path to the existing
normal Editor executable for the current OS/architecture. Supply its build report so
that the executable SHA can be matched to the intended source snapshot. A test harness
or a stale executable lacking the diagnostic protocol is unsuitable. Linux needs the
normal loader/toolchain environment and a working X11 or Wayland display; see the
[Linux requirements](linux-support.md). Workflow export may use existing build tools
and caches; the wrapper itself does not install tools or restore dependencies.

```powershell
& $python scripts/editor_diagnostics.py --editor 'E:\ModuleEngine\outputs\YourVerifiedEditor.exe' --scenario capture --view scene --menu File
& $python scripts/editor_diagnostics.py --editor 'E:\ModuleEngine\outputs\YourVerifiedEditor.exe' --scenario both --timeout 1800
```

Replace the example path with the actual verified executable. Linux example:

```sh
python3 scripts/editor_diagnostics.py --editor /absolute/path/to/module-engine-editor --scenario capture --window-system x11
```

| Option | Contract |
| --- | --- |
| `--editor` | Required absolute path to an existing native PE/ELF executable of the current architecture. |
| `--scenario capture\|workflow\|both` | Default `capture`. Capture creates a fresh Puzzle with `--new-puzzle`; workflow/both create a fresh Two Arenas fixture with `--new-game`. `both` runs workflow before capture. |
| `--view` | Capture view: `render`, `shader`, `history`, `engine`, `settings`, `game`, `scene`, `assets`, `systems`, `custom`; default `scene`. |
| `--menu` | Optional capture menu: `File`, `Edit`, `Tools`, `Windows`. |
| `--window-system auto\|x11\|wayland` | Default `auto`. Explicit selections must match the initialized window system. Windows capture must initialize the Windows window system. |
| `--timeout` | Positive finite seconds per command, default 180. Use `--timeout 1800` explicitly for workflow builds; it does not change product watchdogs. |
| `--output-root` | Optional report root inside this checkout's `outputs/`; default `outputs/editor-diagnostics`. Each invocation creates a fresh timestamp/nonce child, preserving old evidence. |

The example `$python` must name an actual Python interpreter. The commands do not
select a build automatically. Record the chosen path and build evidence with the run.

## Isolation and preservation

Each run places its generated fixture and isolated personal directory under
`work/verification/editor-diagnostics/<run-id>/`. Child TEMP/TMP/TMPDIR use a unique
short `.work/v/<token>` directory. The wrapper removes inherited capture controls
and explicitly selects WGPU and the requested window system while retaining the
normal loader environment. The requested window size is 1580×940 with a minimum
1100×720; the actual PNG dimensions are validated and reported.

The opt-in `MODULE_ENGINE_DIAGNOSTIC_USER_ROOT` must point to an existing absolute
isolated directory. The wrapper creates and supplies this directory for its children.
It redirects only Recent and layout persistence. It does not override package/source
validation, SDK pins, Engine binding or Full/Team permissions. Normal Editor behavior
is unchanged when the variable is absent; invalid explicit roots fail rather than
falling back to the installed personal files.

The route shares `work/verification/launcher.lock` with the verification launcher;
a competing launcher operation fails to acquire the lease. It does not serialize
manual Cargo or other applications. Avoid concurrent builds/native work and source
edits during a run. Protected authored paths, including personal Recent/layout,
are inventoried before and after execution, including failure/timeout. Changes fail
the run; there is no automatic restore or cleanup of user files. The supplied
executable and generated fixture must retain their SHA throughout the run.

## Evidence and failure

`result.json` records commands, stdout/stderr paths, exit status, per-command timing,
timeout/owned-process cleanup evidence, selected environment, executable path/SHA,
generated fixture path/SHA, protected-file inventories and preservation results.
It also records checkout HEAD and a tracked/untracked source inventory digest before
and after the run. This inventory describes the checkout; it does not prove that
the supplied executable was built from it. Match the executable to separate build
evidence before making a source-specific conclusion.

Editor exit 0 alone is insufficient. Capture requires a fresh complete RGBA8 PNG
with bounded positive dimensions, valid chunks/CRC, terminated zlib data and complete
scanlines. It also requires exactly one actual initialized graphics record: WGPU
with `Dx12` on Windows or `Vulkan` on Linux and a compatible actual window system.
Requested environment values alone are not graphics evidence. `native-ui.png` is
an application WGPU readback, not a compositor screenshot.

Workflow requires a fresh `workflow-report.json`, `passed=true`, positive integer
`model_pixels_changed` and `model_primitives`, and all four `menus_clicked` values
true. It also requires true `new_templates_preserve_engine`,
`copy_retains_author_document`, `unassigned_asset_copied`,
`windows_reuses_current_workspace`, `exported_player_pixels_equal` and
`original_project_unchanged`, plus validated `relocated-model.png` and
`player-model.png`. This checks existing services, egui menu operations and
offscreen model/Player rendering, rather than native file-picker interaction.

The final status distinguishes `passed`, `failed`, and `timed_out`. A failed command,
missing/invalid evidence or preservation failure rejects the run. Timeout cleanup
terminates only the launched process tree/group using the shared launcher executor;
its result is recorded. After report-directory creation, failures retain logs and
`result.json`. Invalid initial arguments/executable paths can fail before a report
exists. Exit codes are 0 for a passed report, 1 for a failed/timed-out report, and 2
for initial setup/argument validation failure.

## Windows evidence on 17 October 2026

The [build report](../outputs/ci-editor-builds-20261017/stage2-editor-build/result.json)
identifies the normal Editor selected from CargoJSON, executable SHA-256
`5ccefd946b718ca523f732ff54d8c6cbe793ecb6afc06f19208dd1877da8c388`.
The [input comparison](../outputs/ci-editor-builds-20261017/stage2-executable-input-check.json)
confirms that all 143 Rust/Cargo source inputs still match that build. The first
[Puzzle capture](../outputs/ci-editor-builds-20261017/diagnostics/20261017T020736Z-ae5f9d5f527c/result.json)
passed with File open and a rendered Puzzle board at 1580×940 on WGPU/Dx12, NVIDIA RTX 4080 SUPER.

A workflow initially completed inside the Editor but was falsely rejected by the
Python wrapper's handling of Windows `\\?\` canonical paths. A minimal probe confirmed
the same parent directory. The containment check now falls back to same-file
ancestor comparison; its regression failed before the fix and passed after it,
while outside paths remain rejected. The fresh complete
[both run](../outputs/ci-editor-builds-20261017/diagnostics/20261017T020938Z-e9e189aad466/result.json)
passed three commands and three captures, including menus, Save As, import, export,
30,719 changed model pixels in the separate workflow model PNGs and a native
Scene-pane capture during Scene/Rust Custom preparation. A valid screenshot and
initialized backend do not prove that this Scene finished preparation or rendered.

Negative runs rejected an
[unsupported Windows Wayland request](../outputs/ci-editor-builds-20261017/diagnostics/20261017T020954Z-1c2e019218a1/result.json)
and recorded explicit timeouts for
[fixture creation at 0.01 s](../outputs/ci-editor-builds-20261017/diagnostics/20261017T020955Z-f624acdcac14/result.json)
(one command) and
[native capture at 0.2 s](../outputs/ci-editor-builds-20261017/diagnostics/20261017T021008Z-e53a1235b6ac/result.json)
(two commands), with owned-process cleanup recorded. The native-capture timeout
has `cleanup_exit_code=0`. The 0.01 s fixture timeout raced a normal child exit;
its cleanup reported exit 128/error, and the run correctly remained `timed_out`.
Each linked diagnostic report
has `protected_unchanged=true` and `source_unchanged=true`; the supplied executable
and any generated fixture retained their SHA. These source comparisons apply within
each run, not across subsequent documentation edits.

The two actual Rust regressions and 11 Python launcher tests passed, including the
Windows canonical-path regression. Final Windows
[`contracts`](../outputs/ci-editor-builds-20261017/verification/20261017T021011Z-9d643200/result.json)
passed 187 Rust tests across 16 steps, and
[`clippy`](../outputs/ci-editor-builds-20261017/verification/20261017T021244Z-aad65aea/result.json)
passed. The final full
[Python suite](../outputs/ci-editor-builds-20261017/stage2-final-python-tests.json)
passed with 66 tests and two explicit platform skips; its logs are linked from the
report. Linux native capture and hosted CI remain unverified. The scope limits below
continue to apply.

## Scope and verification

Two Rust `diagnostic_personal` tests in `contracts` cover explicit-root validation
and isolation of Recent/layout while retaining other Editor state and source
permissions. They do not run native capture. Wrapper unit fixtures check protocol
validation and failure handling; they do not replace actual runs using the supplied
Windows/Linux Editor. Historical contract counts remain tied to their original
snapshots; adding these tests does not update those old reports.

This route does not establish DWM/compositor responsiveness, foreground input latency,
cold-start time, native picker behavior, hardware Linux support, pixel-perfect UI
appearance or full Full/Team/Player acceptance. Read captures and reports against the
specific scenario and artifact. See [verification routes](verification.md) and
[Editor workflow](editor-workflow.md).
