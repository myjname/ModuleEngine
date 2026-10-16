<a id="подготовка-linux-и-границы-проверки"></a>

# Linux setup and verification boundaries

[English](linux-support.md) | [Русский](ru/linux-support.md) | [Documentation](README.md)

GUI Editor and Player share one policy: **Windows — WGPU/DX12, Linux — WGPU/Vulkan**. The game renderer is a separate Vulkan module. The GUI using one API does not determine the game graph's format. The Windows DWM-freeze fix is preserved; diagnostic Glow is enabled only explicitly.

Policy checks with a Linux flag on Windows do not replace native building and execution. Native Rust/Cargo, a C compiler, pkg-config and DXC were installed in the existing Ubuntu/WSL2 with separate authorization. [Current status](current-status.md) ties completed fixes and open boundaries to subsequent checks; [linux-support-20261015.md](linux-support-20261015.md) describes the first run and is retained as history.

<a id="требования-к-среде"></a>

## Environment requirements

* Native Linux Rust/Cargo 1.99.0 is the verified version; earlier versions were not checked in this run. rustfmt/Clippy components, a C compiler/linker and pkg-config are required. The commands below do not install tools.
* For GUI: a working Wayland or X11 session, window-system and xkbcommon runtime libraries, a Vulkan loader and compatible ICD. Development/linking may require dev packages for the selected distribution. Install them explicitly through your system's tools.
* For new HLSL: native Linux DXC. Windows `dxc.exe` is not used through Wine/interop. A precompiled Player needs neither DXC nor Rust, but needs system runtime libraries and a GPU driver.
* Native packages are built on their own OS/architecture. A Windows export does not become a Linux package by renaming the EXE. The `native_platform` tag does not prove glibc/driver compatibility with every distribution.

Read-only environment check, including real `vkCreateInstance` and Vulkan device enumeration:

```sh
python3 scripts/linux_prerequisites.py
python3 scripts/linux_prerequisites.py --require build
python3 scripts/linux_prerequisites.py --require gui
python3 scripts/linux_prerequisites.py --require hlsl
```

Code 2 means the selected mode's requirements are missing. A successful probe does not mean ModuleEngine has been built or a window successfully created. Device type 4/`software=true` means software Vulkan; this does not measure discrete-GPU performance. A socket check does not prove X11 authorization or Vulkan surface creation.

<a id="сборка-и-запуск"></a>

## Building and running

Use the existing checkout through a native Linux environment or an explicitly created Full acceptance package, after preparing tools. Ordinary development does not require a new checkout. For GUI, choose a format 4 project with no active WAV request; `/path/to/linux-project-v4.json` below means its actual path. This checkout's root `project.json` has the old format 3 and is preserved without automatic migration. `game-project-v4.json` has the current format, but its audio request requires an explicit change to a separate verification project.

```sh
rustc -vV
cargo --version
cargo fetch --locked --target "$(rustc -vV | sed -n 's/^host: //p')"
sh verify.sh
sh run.sh /path/to/linux-project-v4.json
```

`run.sh` and `verify.sh` use offline Cargo with `Cargo.lock`. Restoring dependencies is a separate explicit `cargo fetch` command. The Linux target is in `.work/linux-target/<host>`; Windows target and MSVC settings are not reused. `CARGO_TARGET_DIR` can be explicitly replaced. Do not load Windows `.tools/env.ps1` on Linux. Windows toolchain-manifest/bootstrap remain Windows tools; automatic Linux SDK installation is not yet available.

`verify.sh` runs the entire workspace suite, including real Vulkan rendering and the offscreen GameUI test, so it requires a usable Vulkan device and native DXC. It is not merely a CPU/build check. To check compilation alone, use `cargo check --offline --locked --release --target "$(rustc -vV | sed -n 's/^host: //p')" --workspace --all-targets`. A Wayland/X11 desktop session is unnecessary for the offscreen test; `--require gui` is not a prerequisite for it. Native window creation and export are checked separately below.

Full source export includes the shell scripts and Python probe. Export performed on Unix preserves execute bits. Scripts can be run through `sh` if file transfer or the filesystem lost those bits. Native `Player` must remain executable.

A separate native environment is prepared on the test PC: `. .tools/linux-x86_64/env.sh` in Ubuntu. This local file contains absolute paths for the current machine and is not shipped as a portable SDK. It sets separate Linux Cargo/rustup homes and an ext4 target; it does not change Windows `.tools/cargo`, `.tools/rustup` or shell profiles.

Verified versions, official URLs and SHA-256 hashes of Linux Rust/DXC are saved in `scripts/linux-toolchain.json`; this file is delivered with `scripts/` in Full/Team. It is metadata for explicit restoration, not an automatically executed installer. The Windows manifest and its pins are preserved.

Verification CLI commands after `verify.sh`:

```sh
host=$(rustc -vV | sed -n 's/^host: //p')
editor="${CARGO_TARGET_DIR:-$PWD/.work/linux-target/$host}/$host/release/module-engine-editor"
"$editor" --integration-check outputs/linux-verification/integration
"$editor" --game-worker-check game-project-v4.json outputs/linux-verification/worker
"$editor" --scene-assets-check game-project-v4.json outputs/linux-verification/scene-assets
"$editor" --editor-ui-check game-project-v4.json outputs/linux-verification/workflow
```

These commands require the corresponding runtime/SDK/DXC. Native Linux acceptance was performed for snapshots specified in the [reports](current-status.md#choosing-evidence); the commands alone do not confirm the current HEAD. Acceptance can create temporary Full/Team/Player packages; run sequentially. An active request for unsupported native audio must first be explicitly removed in a separate verification project; the production project is not changed automatically.

<a id="wayland-x11-и-диагностика"></a>

## Wayland, X11 and diagnostics

The window system is selected independently of the GUI GPU backend:

```sh
MODULE_ENGINE_WINDOW_SYSTEM=wayland sh run.sh /path/to/linux-project-v4.json
MODULE_ENGINE_WINDOW_SYSTEM=x11 sh run.sh /path/to/linux-project-v4.json
MODULE_ENGINE_WINDOW_SYSTEM=auto sh run.sh /path/to/linux-project-v4.json
```

Auto uses winit's selection: a nonempty Wayland environment takes priority. An automatic retry through X11 after a failed Wayland connection is not promised. On error, check `WAYLAND_DISPLAY`, `WAYLAND_SOCKET`, `XDG_RUNTIME_DIR` and `DISPLAY`, or explicitly select X11. A faulty configuration reports the reason; there is no hidden WGPU-to-Glow fallback.

In the detected WSLg, `WAYLAND_DISPLAY=wayland-0`, but the socket is in `/mnt/wslg/runtime-dir/wayland-0`, and `XDG_RUNTIME_DIR=/run/user/0` does not point to it. An absolute path is allowed for a separate process:

```sh
WAYLAND_DISPLAY=/mnt/wslg/runtime-dir/wayland-0 python3 scripts/linux_prerequisites.py
```

This address is specific to the detected environment, not a universal Linux setting. The probe does not repair global variables/system settings.

To start Editor itself in this WSLg, `XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir` was used only in the child process environment. This was verified by actual Wayland window creation; X11 was checked separately with `MODULE_ENGINE_WINDOW_SYSTEM=x11`.

In a shared Windows/WSL checkout, the project binding can contain a Windows path such as `E:\ModuleEngine` or its verbatim form. On Linux, such an absolute drive path is resolved through `/usr/bin/wslpath -u`, without a shell or sidecar write. Ordinary checks of the selected Engine, manifest, SDK and profile then follow. UNC/device/relative Windows paths are not converted. On ordinary Linux without `wslpath`, explicitly select an accessible native Engine; rejection does not authorize another architecture or access to absent sources.

Short native-window check without a project, GameHost or layout saving:

```sh
host=$(rustc -vV | sed -n 's/^host: //p')
MODULE_ENGINE_PROBE_SECONDS=5 MODULE_ENGINE_WINDOW_SYSTEM=x11 \
  cargo run --offline --locked --release --target "$host" -p module-engine-editor --example desktop_presentation_probe
# Then repeat separately with MODULE_ENGINE_WINDOW_SYSTEM=wayland in a working session.
```

Running with `--target` without `CARGO_TARGET_DIR` uses standard `target/<host>`; set the same variable as in `run.sh` if needed. Do not run multiple GPU checks simultaneously.

Logs distinguish `requested` parameters from the actually `initialized` adapter/backend/window-system. `WGPU_BACKEND` does not override Windows DX12 or Linux Vulkan. `MODULE_ENGINE_GUI_BACKEND=glow` is explicit diagnostics requiring a separate decision on a machine with the previous DWM defect. `EFRAME_SCREENSHOT_TO` no longer enables Glow implicitly: for Editor, use `MODULE_ENGINE_UI_SCREENSHOT_TO` with WGPU. The historical DWM report describes the previous capture behavior.

<a id="dxc-пакеты-и-процессы"></a>

## DXC, packages and processes

DXC search order: explicit `MODULE_ENGINE_DXC`, native tools in the backend package, then `PATH`. Recommended structure:

```text
tools/dxc/bin/dxc
tools/dxc/lib/libdxcompiler.so
```

`tools/dxc/dxc` with `tools/dxc/lib` is also allowed. ELF and execute bit are checked. The adjacent `lib` is added to `LD_LIBRARY_PATH` only for the child DXC; the editor/system environment is unchanged. The package must include the actual shared-library dependencies of its DXC version. The native library list inside a Linux package requires verification on Linux. A connected Vulkan Team requires DXC; a graph without this backend does not acquire that dependency.

Native Player is named `Player` on Linux and `Player.exe` on Windows. New manifests write `native_platform=<os>-<arch>` and reject a foreign platform before creating Host. Old schema2 manifests without the tag remain compatible; the tag itself does not replace binary/ABI checks. Graph schemas and the public SDK did not change.

Native build/cache on Unix is separated by platform; builds are serialized with a file lock. Linux Player publication uses `renameat2(RENAME_NOREPLACE)`; an unsupported syscall/filesystem causes rejection, not a nonatomic overwrite of the destination.

In the verified WSL, the shared Windows drive `/mnt/e` (DrvFS) returns `EINVAL` for this operation even when the destination is free. Player publication there is therefore rejected with diagnostics; use a destination on a native Linux filesystem. Export to ext4 was verified. Sources/resources can still be on the Windows drive: the requirement concerns the destination filesystem where staging is created and publication performed.

Export of the selected Player uses its own cache `<Engine>/.work/player-native-target/linux-x86_64`, separate from the Editor build's `CARGO_TARGET_DIR`. Therefore, the first export may rebuild GUI dependencies even after Editor is built. In the verification Full package, only the build cache was explicitly reused through a symlink; the report describes this, and it does not imply a cold build on a clean machine.

Rust Custom uses the `custom-workers` worker target inside the selected `CARGO_TARGET_DIR`. On Linux, the ready worker starts from a unique directory `executables/<candidate>/worker` in this target; sources, lock and diagnostics remain in the project's original working directory. The last owner closes the process and releases only its executable; shared cache and neighboring candidates are preserved. For a target nested directly in the candidate directory, the worker stays beside the sources, outside deleted build outputs. Windows placement is preserved.

In a shared Windows/WSL checkout, select native ext4 for `CARGO_TARGET_DIR` and `TMPDIR`. In the measured case, executing ELF from DrvFS added about 48 ms to Custom's first response and caused code with an 80 ms delay to exceed the unchanged 150 ms IPC limit. Placing the executable on ext4 eliminates this measured cause; it does not guarantee execution of arbitrary user code within 150 ms.

With the [compiled-program store](custom-program-store.md) enabled, the installed toolchain and Cargo registry also need a native filesystem: verification reads compiler/link input contents rather than trusting mtime alone. In the 16.10.2026 check, one full proof with Rust/Cargo on `/mnt/e` took 102.04 s. After exact relocation of the pinned installation to Ubuntu's native cache with bytes/modes/symlinks checked, the same production context passed in 2.26 s. This removes a measured IO bottleneck, not a timing promise for every project; versions, watchdog and input checks are preserved. Conditions, paths and other native results are in the [stage report](review-fixes-20261016.md).

Rust Custom compilation and execution processes on Linux, including this subsystem's Cargo, belong to separate process groups. Cancellation and shutdown signal inheriting descendants and wait for the leader; `waitid(WNOWAIT)` preserves the PID until the group finishes. A deliberate `setsid` can escape the group; no cleanup guarantee after a crash of the host itself was added. This manages lifecycle, not a sandbox. Windows Job Object is preserved. A forced timeout of an external verifier/relocation process still kills only the immediate child; complete descendant cleanup remains a limitation.

Reusing ready scheduled Rust Custom after Stop is implemented in the [compiled-program store](custom-program-store.md). With an unchanged supported compile context, instances use the same verified code while retaining independent state and worker PIDs; Cargo freshness proof does not mean a new root compilation. This is confirmed by subsequent [Windows/Linux checks](remaining-authoring-fixes-20261016.md), while the previous failed 45-second run remains a historical result. Cold Editor preparation within 45 seconds and the strict Player 60 FPS target are still open; a successful prewarmed benchmark does not close them. See [current status](current-status.md).

<a id="опциональные-сервисы"></a>

## Optional services

Native WAV is implemented on Windows. On Linux, an active `audio.wav` request produces an error before assets load; export is also rejected. An absent/disabled service is allowed. Export does not silently remove sound or alter authored data. A passed silent/headless check cannot be treated as proof of audio operation.

For Linux GUI/export verification, use a project that explicitly does not request WAV, for example Puzzle or your own profile without audio. Current ARPG settings with sound cues require an explicit change to a separate project copy. A complete Linux audio backend is the next separate stage.

Native File/Folder dialogs are currently implemented only on Windows. On Linux, file/folder selection commands report this limitation. Opening a graph through a CLI path, saving an already selected file and CLI export do not depend on these dialogs; interactive Import/Open/Save As require the next stage. A window-launch check does not prove parity of all menus with Windows.
