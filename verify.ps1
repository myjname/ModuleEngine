$ErrorActionPreference = "Stop"
$verificationOriginalTemp = $env:TEMP
$verificationOriginalTmp = $env:TMP
$verificationOriginalIncremental = $env:CARGO_INCREMENTAL
$verificationOriginalCargoHome = $env:CARGO_HOME
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
    cargo test --locked --workspace
    if ($LASTEXITCODE -ne 0) { throw "Tests failed" }
    cargo clippy --locked --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "Clippy failed" }
    cargo run --locked -p module-engine-editor -- --self-check outputs/verification-v04/cpu
    if ($LASTEXITCODE -ne 0) { throw "CPU runtime self-check failed" }
    cargo run --locked -p module-engine-editor -- --integration-check outputs/verification-v04/integration
    if ($LASTEXITCODE -ne 0) { throw "Vulkan/Rust/project integration check failed" }
    cargo run --locked -p module-engine-editor -- --settings-check outputs/verification-v04/settings
    if ($LASTEXITCODE -ne 0) { throw "Settings/properties policy check failed" }
    $verificationGame = Join-Path $PSScriptRoot 'game-project.json'
    if (-not (Test-Path -LiteralPath $verificationGame -PathType Leaf)) {
        $verificationGame = Join-Path $PSScriptRoot 'project.json'
    }
    if (-not (Test-Path -LiteralPath $verificationGame -PathType Leaf) -or
        $null -eq (Get-Content -LiteralPath $verificationGame -Raw | ConvertFrom-Json).game) {
        throw 'No authored game exists. Create the example with: cargo run --locked -p module-engine-editor -- --new-game game-project.json; then run verify.ps1 again.'
    }
    cargo run --locked -p module-engine-editor -- --game-check $verificationGame outputs/verification-v04/game
    if ($LASTEXITCODE -ne 0) { throw "Authored game/Player pipeline check failed" }
} finally {
    $env:TEMP = $verificationOriginalTemp
    $env:TMP = $verificationOriginalTmp
    $env:CARGO_INCREMENTAL = $verificationOriginalIncremental
    $env:CARGO_HOME = $verificationOriginalCargoHome
    Pop-Location
}


