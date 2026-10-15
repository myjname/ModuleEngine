# Standalone Player

This package contains the shared native Player UI and a separate executable. It
has no production dependency on the Editor or the Two Arenas project.

The default build enables no optional implementation. Select native packages
explicitly: `ui`, `arpg`, `puzzle`, `vulkan`, and `audio`. A puzzle window uses
`ui,puzzle,vulkan`; it does not require ARPG or audio. `ui` enables UI callbacks
only for gameplay packages already selected. Installation includes neutral core
contracts, the host-bound Custom contract, and the small generic source rotation
example; installed metadata alone never activates an authored module.

Applications share `composition_at`, `composition_with`, `service_registry`,
and the UI's `run_host`/`run_benchmark_host`. The Editor can add source contracts
through the composition callback before metadata is synchronized. The Player's
ordinary dispatch paths select source UI callbacks by contribution ID.

An exported package contains `player.json` schema 2, its authored format-4
project, the selected native executable, required assets, precompiled Custom
workers, and sealed Vulkan artifacts when used. The manifest records exact
native features, the SDK data/API fingerprint, and SHA-256 file hashes. Launch
requires an exact compiled feature match and the project's exact active
module/schema/state/SDK lock. Normal reads perform no migration and never fill
missing pins. The Editor's metadata validator can inspect a smaller installation
without pretending that its all-feature binary is that selected Player.

```powershell
Player.exe
Player.exe --verify-package <package-root> <evidence-folder>
Player.exe --headless-check <package-root> <evidence-folder>
Player.exe --benchmark <evidence-folder>
```

Normal launch and native benchmark locate `player.json` only beside the
executable. Verification accepts an explicit package root, advances three real
fixed ticks, and reports actual state and rendering statistics; the rendered
check also saves a 960×540 PNG. Headless verification reports `frame:null` and
constructs no renderer or optional service. Package execution uses no source
checkout locator, compiler root, or Editor distribution policy. Vulkan requires
the package's `.spirv` directory, so a missing artifact fails instead of falling
back to source compilation.

The existing native benchmark retains its 30-frame warmup, 120 UI samples and
120 distinct completed game frames, physical 1920×1080 requirement, 45-second
watchdog, and unmodified twenty-enemy workload criterion. The measured data and
any unmet target are reported directly; a different project does not satisfy
that workload criterion merely by rendering quickly.
