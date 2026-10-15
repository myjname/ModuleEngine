#!/bin/sh
# Uses installed native tools only. Fetch dependencies explicitly before offline use.
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
if [ "$(uname -s)" != Linux ]; then
    echo 'run.sh requires native Linux; use run.ps1 on Windows.' >&2
    exit 2
fi
command -v cargo >/dev/null 2>&1 || { echo 'Install Rust/Cargo explicitly; see docs/linux-support.md.' >&2; exit 2; }
host=$(rustc -vV | sed -n 's/^host: //p')
case "$host" in *linux*) ;; *) echo 'A native Linux Rust toolchain is required.' >&2; exit 2 ;; esac
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/.work/linux-target/$host}"
project=${1:-}
if [ -z "$project" ]; then
    if [ -f game-project-v4.json ]; then project=game-project-v4.json
    elif [ -f game-project.json ]; then project=game-project.json
    else project=project.json; fi
fi
exec cargo run --offline --locked --release --target "$host" -p module-engine-editor -- "$project"
