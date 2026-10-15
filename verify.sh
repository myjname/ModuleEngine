#!/bin/sh
# Full native Linux suite, including offscreen Vulkan/GameUI tests; no package
# manager, tool installation or native desktop-window acceptance in this script.
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
if [ "$(uname -s)" != Linux ]; then echo 'Use verify.ps1 on Windows.' >&2; exit 2; fi
python3 scripts/linux_prerequisites.py --require build
python3 scripts/linux_prerequisites.py --require hlsl
host=$(rustc -vV | sed -n 's/^host: //p')
case "$host" in *linux*) ;; *) echo 'A native Linux Rust toolchain is required.' >&2; exit 2 ;; esac
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/.work/linux-target/$host}"
export CARGO_INCREMENTAL=0
cargo fmt --all -- --check
cargo check --offline --locked --release --target "$host" --workspace --all-targets
cargo test --offline --locked --release --target "$host" --workspace --lib --bins -- --test-threads=1
cargo test --offline --locked --release --target "$host" -p module-engine-editor game_ui::tests::interactive_game_ui_keeps_drawing_and_applying_rust_during_slow_ticks -- --ignored --exact --test-threads=1
cargo clippy --offline --locked --release --target "$host" --workspace --all-targets -- -D warnings
cargo test --offline --locked --release --target "$host" --workspace --doc
cargo build --offline --locked --release --target "$host" -p module-engine-editor
"$CARGO_TARGET_DIR/$host/release/module-engine-editor" --self-check outputs/linux-verification/cpu
echo 'Build, workspace Vulkan/offscreen tests and CPU self-check passed. Native desktop-window/export checks remain separate; see docs/linux-support.md.'
