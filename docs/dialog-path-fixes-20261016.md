<a id="пути-системных-диалогов-и-транзакция-save-as"></a>

# System dialog paths and the Save As transaction

[English](dialog-path-fixes-20261016.md) | [Русский](ru/dialog-path-fixes-20261016.md) | [Documentation](README.md)

Work began from `f9cff695229e78382e1e0b16177c310c6df0478f` on branch `codex/dialog-path-hints`. User changes to `game-project-v4.json` are excluded from the fix. Working data, layouts and the three previous EXEs are checked against the original SHA-256 hashes in `outputs/dialog-path-fixes-f9cff69/stage0-baseline.json`.

<a id="подтверждённые-ошибки"></a>

## Confirmed defects

1. After New/Save As, the project path can be a canonical Windows path, `\\?\E:\…`. Its parent, a path with `/` or `.`/`..` was passed directly to `SHCreateItemFromParsingName`. Shell returned `0x80070057` before opening the window. The exact original implementation from the base commit reproduced this in `baseline-dialog-probe.log`; ordinary `E:\ModuleEngine` was accepted. This probe did not open a modal window.
2. An independent Codex reviewer found that Save As applies JSON before file selection. Cancellation could already have replaced the document, cleared the draft and stopped Play. The new regression actually failed on the old behavior: `cancel: dispatch applied JSON`, 0 passed / 1 failed (`red-save-as-v2.log`). The first preparatory attempt, `red-save-as.log`, did not compile and is not evidence of this defect.

<a id="принятые-границы"></a>

## Accepted boundaries

The initial folder is used only as a Windows Shell hint. The source path is canonicalized without first changing its semantics; an ordinary drive/UNC form is then allowed after verifying that it points to the same folder. UTF-16 is preserved losslessly. Device/volume namespaces and names with significant trailing dots/spaces are not converted into another path. An inaccessible or unrepresentable folder, Shell parser error or SetFolder failure does not prevent choosing another folder.

Errors in dialog creation, required settings, Show and result reading remain errors. Cancel returns no selected path. New, Open, Save As, Export and Import use shared dialog configuration. Normalization does not apply to the returned path, Recent, Engine binding or file IO.

Save As receives a validated candidate without applying it to the live editor. Writing the copy publishes JSON, binding and resources through the existing file service; only success switches the document. Publication failure also leaves the original work open. For the same physical file, the candidate is saved first, then applied; history is not replaced with new load history. If file replacement fails, the existing writer may leave a `*.tmp-*` recovery file with the candidate; original files and editor state remain unchanged.

Linux retains its explicit lack of native dialogs. The editor's graphics backend is unchanged: Windows WGPU/DX12, Linux WGPU/Vulkan through X11 or Wayland. WSLg checks characterize software llvmpipe, not physical Linux GPU performance.

<a id="проверка"></a>

## Verification

Fix sources: **`0fbfb9ca74a139253b4e9d611b2b8877cfb4166d`**, branch **`codex/dialog-path-hints`**. All results below were obtained on this version of the three changed Rust files; their SHAs are recorded in `frozen-source-v2-hashes.json`. Subsequent edits concern documentation and its inventory. Evidence is stored separately in `outputs/dialog-path-fixes-f9cff69/`; previous reports are not treated as results for the new version.

| Check | Windows | Ubuntu / WSL2 |
| --- | --- | --- |
| Formatting | `cargo fmt --all -- --check`: passed | The same unchanged Rust files |
| Strict Editor lint, all targets, release | Passed, `-D warnings` | Passed, `-D warnings` |
| Full Editor test suite, all targets, release | **219 passed / 0 failed / 5 ignored** | **216 passed / 0 failed / 5 ignored** |
| Native Play/Stop, shared compiled code and independent workers | Separate ignored test: **1 passed** | Separate ignored test: **1 passed** |
| Editor release build | Passed | Passed, native x86_64 ELF |
| Isolated Full export and resource verification | Passed, Puzzle | Passed, Puzzle |
| Real Editor window, Game/Scene, graph panning, Play/Stop | **WGPU / DX12**, RTX 4080 SUPER | **WGPU / Vulkan**, X11 and Wayland, llvmpipe |

All three window scenarios completed 90 measured frames each in Stop and Play, confirmed simulation progress, executor retention during panning, correct Stop, and unchanged authored document and file. Each scenario's `report.json` has `complete = true`, `measurements_valid = true`, `errors = []`. The original internal 45-second and external 90-second timeouts were not increased. This checks startup on a small Puzzle scene, not a new ARPG performance measurement with 20 enemies. Builds and GPU checks ran sequentially; existing tools and offline caches were used without installing software or changing the OS or drivers.

Windows separately passed 17 workflow tests and eight native-dialog tests with detailed output (`windows-v2-workflow-details.log`, `windows-v2-native-dialogs-details.log`). Coverage includes Russian names/emoji, `/`, canonical `\\?\` paths, relative paths, `.`/`..`, all four COM factories/configurations, optional SetFolder and strict errors in required settings/Show/GetResult. For an existing folder longer than 320 UTF-16 code units, ShellItem confirmed physical identity; SetFolder remains best-effort.

Save As regressions run real dispatch and workflow completion with substituted picker outcomes. They check cancellation, picker error, publication failure, write failure at the current path, successful write to the current file with history preserved, candidate copying with resources and Engine binding, invalid JSON and repeat selection after New → Save As → Recent. Each failure compares the document, exact draft, history, Play, Recent and SHA of every previous file. Only failure to replace the current file permits exactly one recovery file from the existing writer, with its name, location and candidate contents checked. Resource conflicts and absence of partial publication are additionally checked by the full `workspace_files` suite.

Native ignored tests ran directly from products reported by Cargo JSON after PE/ELF and SHA checks. The Linux test ran with ordinary `LD_LIBRARY_PATH` for DXC, without the Cargo test-runner environment. JSON with test-executable paths and SHAs is saved beside the logs.

The first full Windows run retained **216 passed / 3 failed / 5 ignored**: two new fixtures used a path containing `..` rejected by the service, and the third check did not account for the intended recovery file. Only fixtures/assertions were fixed, after which the full rerun passed. The independent reviewer confirmed that source-file and state checks were not weakened. The first window smoke stopped during junction creation because of sandbox restrictions; after creating the junction only in the Full project owned by the check, it continued through `-Resume`. This was a verification-setup failure, not a window-launch result. Old logs are preserved.

<a id="независимое-codex-review"></a>

## Independent Codex review

A separate reviewer discovered premature JSON application in Save As before the fix was implemented. After the fix, they rechecked the production diff and then test changes: no new confirmed defects were found. The reviewer did not edit the implementation or run Cargo/UI; the runs above were performed by the primary agent. A brief record is in `independent-codex-review.txt`.

<a id="готовая-сборка-и-сохранность-данных"></a>

## Ready build and data preservation

New file: **`outputs/ModuleEngine-dialogs-fixed.exe`**, 32 026 112 bytes.

SHA-256: **`472B6E8494F1C82734E63497A69EB2D687CF0B2D90D3BBC6751BAAB79091547D`**.

`ModuleEngine-dialogs-fixed.exe.sha256` and `ModuleEngine-dialogs-fixed.code-commit.txt` are saved beside it; provenance details are in `windows-delivery-artifact.json`. The EXE's SHA matches the verified Cargo release product. `ModuleEngine.exe`, `ModuleEngine-review-fixed.exe` and `ModuleEngine-authoring-fixed.exe` are preserved.

The user `game-project-v4.json` is preserved with its original SHA **`16E272712A31164CEC081E37B3327274AC2282CAF10CEE1BDDF410E0C531B34D`** and excluded from commits. Original projects, Engine binding, both Full layouts, Recent, SDK and three old EXEs are compared against `stage0-baseline.json`. Final SHAs, HEAD, branch and Git status are saved in `final-state.json`; the only expected remaining change is the user's `game-project-v4.json`.

<a id="пределы-проверки"></a>

## Verification limits

Real COM factories/configuration do not mean manual verification of interactive Show, file selection and overwrite prompts. Show/Cancel/error flow was checked through a controlled COM object; result decoding uses a real ShellItem. An accessible network UNC resource was not checked: tests cover string conversion and safe rejection of unrepresentable namespaces without network access. Linux windows were checked through WSLg and software llvmpipe; this run does not confirm operation on a physical Linux GPU. Native file/folder dialogs remain explicitly unavailable on Linux, checked by a separate test of all four public picker APIs.
