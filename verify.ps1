param([switch]$NativeBenchmarks)
$ErrorActionPreference = "Stop"
$verificationOriginalTemp = $env:TEMP
$verificationOriginalTmp = $env:TMP
$verificationOriginalIncremental = $env:CARGO_INCREMENTAL
$verificationOriginalCargoHome = $env:CARGO_HOME
$verificationOriginalRustdocFlags = $env:RUSTDOCFLAGS
Push-Location $PSScriptRoot
try {
    $verificationTemp = Join-Path $PSScriptRoot '.work/test-temp'
    New-Item -ItemType Directory -Path $verificationTemp -Force | Out-Null
    $env:TEMP = (Resolve-Path -LiteralPath $verificationTemp).Path
    $env:TMP = $env:TEMP
    $env:CARGO_INCREMENTAL = '0'
    $localEnvironment=Join-Path $PSScriptRoot '.tools/env.ps1'
    if (Test-Path -LiteralPath $localEnvironment) { . $localEnvironment }
    $projectCargoCache=Join-Path $PSScriptRoot '.work/cargo-home'
    if (Test-Path -LiteralPath $projectCargoCache -PathType Container) { $env:CARGO_HOME=$projectCargoCache }
    & (Join-Path $PSScriptRoot 'scripts/bootstrap.ps1') -Json
    if ($LASTEXITCODE -ne 0) { throw "Pinned tools are missing. Run scripts/bootstrap.ps1 -Restore explicitly." }
    # Integration-style units invoke the same Cargo cache, GPU and hard wall
    # timeouts. Serialize these fixtures so setup does not distort tick timing.
    cargo test --offline --locked --workspace --lib --bins -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "Tests failed" }
    cargo test --offline --locked -p module-engine-editor game_ui::tests::interactive_game_ui_keeps_drawing_and_applying_rust_during_slow_ticks -- --ignored --exact --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "Isolated native Game UI/slow Rust acceptance failed" }
    cargo test --offline --locked --workspace --doc
    if ($LASTEXITCODE -ne 0) { throw "Doc-tests failed" }
    cargo clippy --offline --locked --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "Clippy failed" }
    $env:RUSTDOCFLAGS='-D warnings'
    cargo doc --offline --locked --workspace --no-deps
    if ($LASTEXITCODE -ne 0) { throw "Rustdoc failed" }
    cargo run --offline --locked -p module-engine-editor -- --self-check outputs/verification-v04/cpu
    if ($LASTEXITCODE -ne 0) { throw "CPU runtime self-check failed" }
    cargo run --offline --locked -p module-engine-editor -- --integration-check outputs/verification-v04/integration
    if ($LASTEXITCODE -ne 0) { throw "Vulkan/Rust/project integration check failed" }
    cargo run --offline --locked -p module-engine-editor -- --settings-check outputs/verification-v04/settings
    if ($LASTEXITCODE -ne 0) { throw "Settings/properties policy check failed" }
    $verificationGame = Join-Path $PSScriptRoot 'game-project-v4.json'
    if (-not (Test-Path -LiteralPath $verificationGame -PathType Leaf)) {
        $verificationGame = Join-Path $PSScriptRoot 'game-project.json'
    }
    if (-not (Test-Path -LiteralPath $verificationGame -PathType Leaf)) {
        $verificationGame = Join-Path $PSScriptRoot 'project.json'
    }
    if (-not (Test-Path -LiteralPath $verificationGame -PathType Leaf) -or
        $null -eq (Get-Content -LiteralPath $verificationGame -Raw | ConvertFrom-Json).game) {
        throw 'No authored game exists. Create the example with: cargo run --locked -p module-engine-editor -- --new-game game-project.json; then run verify.ps1 again.'
    }
    cargo run --offline --locked -p module-engine-editor -- --game-check $verificationGame outputs/verification-v04/game
    if ($LASTEXITCODE -ne 0) { throw "Authored game/Player pipeline check failed" }
    cargo run --offline --locked -p module-engine-editor -- --editor-ui-check $verificationGame outputs/verification-v04/editor-ui
    if ($LASTEXITCODE -ne 0) { throw "Editor workflow check failed" }
    cargo run --offline --locked -p module-engine-editor -- --scene-assets-check $verificationGame outputs/verification-v04/scene-assets
    if ($LASTEXITCODE -ne 0) { throw "Scene/Assets/export check failed" }
    cargo run --offline --locked -p module-engine-editor -- --game-worker-check $verificationGame outputs/verification-v04/game-worker
    if ($LASTEXITCODE -ne 0) { throw "Background simulation/lifecycle check failed" }
    if ($NativeBenchmarks) {
        # A real desktop is required; CPU processing and native cadence are
        # reported separately. Do not run other builds during these samples.
        cargo build --offline --locked --release -p module-engine-editor
        if ($LASTEXITCODE -ne 0) { throw "Release build failed" }
        $verificationMetadata=cargo metadata --offline --locked --no-deps --format-version 1 | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw "Cargo metadata failed" }
        $verificationExe=Join-Path $verificationMetadata.target_directory 'release/module-engine-editor.exe'
        & $verificationExe --editor-benchmark $verificationGame outputs/verification-v04/editor-native
        if ($LASTEXITCODE -ne 0) { throw "Native Editor benchmark failed" }
        & $verificationExe --player-benchmark $verificationGame outputs/verification-v04/player-native
        if ($LASTEXITCODE -ne 0) { throw "Native Player benchmark failed" }
    }
} finally {
    $env:TEMP = $verificationOriginalTemp
    $env:TMP = $verificationOriginalTmp
    $env:CARGO_INCREMENTAL = $verificationOriginalIncremental
    $env:CARGO_HOME = $verificationOriginalCargoHome
    $env:RUSTDOCFLAGS = $verificationOriginalRustdocFlags
    Pop-Location
}


