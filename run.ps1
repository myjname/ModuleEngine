param([string]$Project = "")
$ErrorActionPreference = "Stop"
Push-Location $PSScriptRoot
try {
    if ([string]::IsNullOrWhiteSpace($Project)) {
        $Project = if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'game-project-v4.json')) { 'game-project-v4.json' } elseif (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'game-project.json')) { 'game-project.json' } else { 'project.json' }
    }
    $localEnvironment=Join-Path $PSScriptRoot '.tools/env.ps1'
    if (Test-Path -LiteralPath $localEnvironment) { . $localEnvironment }
    $projectCargoCache=Join-Path $PSScriptRoot '.work/cargo-home'
    if (Test-Path -LiteralPath $projectCargoCache -PathType Container) { $env:CARGO_HOME=$projectCargoCache }
    & (Join-Path $PSScriptRoot 'scripts/bootstrap.ps1') -Json
    if ($LASTEXITCODE -ne 0) { throw "Pinned tools are missing. Run scripts/bootstrap.ps1 -Restore explicitly." }
    cargo run --locked -p module-engine-editor -- $Project
    if ($LASTEXITCODE -ne 0) { throw "ModuleEngine exited with code $LASTEXITCODE" }
} finally {
    Pop-Location
}

