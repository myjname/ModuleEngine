# Pinned Windows tools

`toolchain-manifest.json` owns the Rust version and host target. Run
`./scripts/bootstrap.ps1 -Json` for a read-only doctor. Its overall `ok` requires
the pinned Rust compiler and Cargo, working rustfmt/cargo-fmt and Clippy commands
(both cargo-clippy and clippy-driver),
and the pinned DXC version/component hashes. Rustfmt and Clippy have their own
version numbering; their reported versions are checked for working tool commands,
while Rust/Cargo use the manifest's exact release. Vulkan loader presence remains
a separate observation; use the GPU self-check for device capabilities.

`./scripts/bootstrap.ps1 -Restore` also repairs missing or broken Cargo, rustfmt
and Clippy when `rustc` already matches. It installs the explicit manifest
`version-target` Rust toolchain with rustfmt and Clippy into `.tools`, then fetches
locked dependencies into `.work/cargo-home`. It does not alter the host Rust
installation. Network access is needed when pinned tools/dependencies are absent.

After portable restoration, dot-source `./.tools/env.ps1` in a new PowerShell
session. That generated wrapper loads `scripts/env.ps1`, which derives the
toolchain from the manifest and sets the project-local Rust homes. The shared
helper can also be dot-sourced directly.

Run `./scripts/test-bootstrap.ps1` for isolated regression checks. The fixture
uses PowerShell mock tools, an alternate manifest release and temporary files
under `.work`; it invokes no installed Rust/Cargo, performs no downloads and
removes its own fixture directory. It covers matching `rustc` with missing,
broken or mismatched required tools, restoration and subsequent doctor, and
manifest-driven environment selection.
