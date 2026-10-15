# Isolated regression tests: every Rust/DXC command is a fixture PowerShell script.
param()
$ErrorActionPreference = 'Stop'
$repositoryRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$fixtureParent = Join-Path $repositoryRoot '.work'
$fixtureRoot = Join-Path $fixtureParent ("bootstrap-fixture-" + [Guid]::NewGuid().ToString('N'))
$shellPath = (Get-Process -Id $PID).Path
$checks = 0
function Assert-True([bool]$condition, [string]$message) {
    if (-not $condition) { throw $message }
}
$mockTool = @'
$fixtureRoot = $env:MODULE_ENGINE_BOOTSTRAP_FIXTURE_ROOT
$settings = Get-Content -LiteralPath (Join-Path $fixtureRoot 'fixture.json') -Raw | ConvertFrom-Json
$tool = [IO.Path]::GetFileNameWithoutExtension($PSCommandPath)
$global:LASTEXITCODE = 0
if ($tool -eq 'rustup') {
    @{tool=$tool;arguments=@($args);rustup_home=$env:RUSTUP_HOME;cargo_home=$env:CARGO_HOME} | ConvertTo-Json -Compress | Add-Content -LiteralPath (Join-Path $fixtureRoot 'calls.jsonl')
    if ($args[0] -eq 'toolchain' -and $args[1] -eq 'install') {
        New-Item -ItemType File -Path (Join-Path $fixtureRoot 'restored') -Force | Out-Null
    }
    return
}
if ($tool -eq 'cargo' -and $args[0] -eq 'fetch') {
    @{tool=$tool;arguments=@($args);cargo_home=$env:CARGO_HOME} | ConvertTo-Json -Compress | Add-Content -LiteralPath (Join-Path $fixtureRoot 'calls.jsonl')
    return
}
if (-not (Test-Path -LiteralPath (Join-Path $fixtureRoot 'restored')) -and $settings.tools.$tool -eq 'broken') {
    $global:LASTEXITCODE = 1
    return
}
$version = $settings.version
if (-not (Test-Path -LiteralPath (Join-Path $fixtureRoot 'restored')) -and $settings.tools.$tool -eq 'wrong') { $version = '0.0.0' }
switch ($tool) {
    'rustc' { if ($args[0] -eq '-vV') { "host: $($settings.target)" } else { "rustc $version (fixture)" } }
    'cargo' { "cargo $version (fixture)" }
    'rustfmt' { 'rustfmt 9.8.7-fixture' }
    'cargo-fmt' { 'rustfmt 9.8.7-fixture' }
    'cargo-clippy' { 'clippy 0.9.8 (fixture)' }
    'clippy-driver' { 'clippy 0.9.8 (fixture)' }
    'dxc' { 'fixture-dxc 1.2.3' }
    default { throw "Unexpected mock tool: $tool" }
}
'@
$harness = @'
param([string]$FixtureRoot, [switch]$Restore)
$ErrorActionPreference = 'Stop'
$env:MODULE_ENGINE_BOOTSTRAP_FIXTURE_ROOT = $FixtureRoot
function Get-Command([string]$Name, $ErrorAction) {
    if ($Name -notin @('rustc.exe','cargo.exe','rustfmt.exe','cargo-fmt.exe','cargo-clippy.exe','clippy-driver.exe','rustup.exe')) { throw "Unexpected command lookup: $Name" }
    $tool = $Name -replace '\.exe$', ''
    $settings = Get-Content -LiteralPath (Join-Path $FixtureRoot 'fixture.json') -Raw | ConvertFrom-Json
    if (-not (Test-Path -LiteralPath (Join-Path $FixtureRoot 'restored')) -and $settings.tools.$tool -eq 'missing') { return $null }
    return [pscustomobject]@{Source=(Join-Path $FixtureRoot "mock-tools/$tool.ps1")}
}
function Invoke-WebRequest { throw 'Tests must not download tools' }
function Expand-Archive { throw 'Tests must not unpack real tools' }
& (Join-Path $FixtureRoot 'scripts/bootstrap.ps1') -Json -Restore:$Restore
exit $LASTEXITCODE
'@
function New-Fixture([string]$name, [hashtable]$toolStates) {
    $root = Join-Path $fixtureRoot $name
    New-Item -ItemType Directory -Path (Join-Path $root 'scripts'), (Join-Path $root 'mock-tools'), (Join-Path $root 'tools/dxc') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'bootstrap.ps1'), (Join-Path $PSScriptRoot 'env.ps1') -Destination (Join-Path $root 'scripts')
    foreach ($tool in @('rustc','cargo','rustfmt','cargo-fmt','cargo-clippy','clippy-driver','rustup')) {
        $mockTool | Set-Content -LiteralPath (Join-Path $root "mock-tools/$tool.ps1") -Encoding utf8
    }
    $dxcPath = Join-Path $root 'tools/dxc/dxc.ps1'
    $mockTool | Set-Content -LiteralPath $dxcPath -Encoding utf8
    $settings = @{version='8.7.6';target='x86_64-fixture-msvc';tools=$toolStates}
    $settings | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $root 'fixture.json') -Encoding utf8
    @{
        schema_version=1
        rust=@{version=$settings.version;target=$settings.target}
        rustup=@{url='https://fixture.invalid/never-download';sha256=('0' * 64)}
        dxc=@{relative_executable='tools/dxc/dxc.ps1';version='fixture-dxc 1.2.3';files=@{'dxc.ps1'=(Get-FileHash -LiteralPath $dxcPath -Algorithm SHA256).Hash.ToLowerInvariant()}}
        native_requirements=@('fixture native requirements')
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $root 'toolchain-manifest.json') -Encoding utf8
    '[workspace]' | Set-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Encoding utf8
    $harness | Set-Content -LiteralPath (Join-Path $root 'harness.ps1') -Encoding utf8
    return $root
}
function Invoke-Fixture([string]$root, [switch]$Restore) {
    $arguments = @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',(Join-Path $root 'harness.ps1'),'-FixtureRoot',$root)
    if ($Restore) { $arguments += '-Restore' }
    $output = @(& $shellPath @arguments 2>&1)
    $code = $LASTEXITCODE
    try { $report = ($output -join [Environment]::NewLine) | ConvertFrom-Json } catch { throw "Fixture failed with exit $code`: $output" }
    return [pscustomobject]@{code=$code;report=$report}
}
try {
    New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
    $root = New-Fixture 'complete' @{}
    $result = Invoke-Fixture $root
    Assert-True ($result.code -eq 0 -and $result.report.ok) 'Complete toolchain must pass doctor'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $root '.tools'))) 'Doctor must not create portable tools'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $root 'calls.jsonl'))) 'Doctor must not install or fetch'
    $checks++

    foreach ($case in @(
        @{tool='cargo';key='cargo';state='missing'}
        @{tool='rustfmt';key='rustfmt';state='missing'}
        @{tool='cargo-fmt';key='cargo_fmt';state='missing'}
        @{tool='cargo-clippy';key='clippy';state='missing'}
        @{tool='clippy-driver';key='clippy_driver';state='missing'}
        @{tool='cargo';key='cargo';state='broken'}
        @{tool='rustfmt';key='rustfmt';state='broken'}
        @{tool='cargo-clippy';key='clippy';state='broken'}
        @{tool='clippy-driver';key='clippy_driver';state='broken'}
        @{tool='cargo';key='cargo';state='wrong'}
    )) {
        $root = New-Fixture ("$($case.tool)-$($case.state)") @{$case.tool=$case.state}
        $result = Invoke-Fixture $root
        Assert-True ($result.code -eq 1 -and -not $result.report.ok -and $result.report.rust.ok -and -not $result.report.($case.key).ok) "Doctor missed $($case.tool) $($case.state) with matching rustc"
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $root 'calls.jsonl'))) 'Doctor must remain read-only on failure'
        $restored = Invoke-Fixture $root -Restore
        Assert-True ($restored.code -eq 0 -and $restored.report.ok) "Restore did not repair $($case.tool) $($case.state)"
        $calls = @(Get-Content -LiteralPath (Join-Path $root 'calls.jsonl') | ForEach-Object { $_ | ConvertFrom-Json })
        $install = @($calls | Where-Object { $_.tool -eq 'rustup' -and $_.arguments[0] -eq 'toolchain' })
        Assert-True ($install.Count -eq 1 -and $install[0].arguments[2] -eq '8.7.6-x86_64-fixture-msvc') 'Restore must use the manifest host toolchain'
        Assert-True ($install[0].arguments -contains 'rustfmt' -and $install[0].arguments -contains 'clippy') 'Restore must include required components'
        Assert-True ($install[0].rustup_home -eq (Join-Path $root '.tools/rustup') -and $install[0].cargo_home -eq (Join-Path $root '.tools/cargo')) 'Restore must isolate toolchain homes'
        $fetch = @($calls | Where-Object { $_.tool -eq 'cargo' })
        Assert-True ($fetch.Count -eq 1 -and $fetch[0].arguments[0] -eq 'fetch' -and $fetch[0].arguments -contains '--locked' -and $fetch[0].cargo_home -eq (Join-Path $root '.work/cargo-home')) 'Restore must use the project dependency cache'
        $doctor = Invoke-Fixture $root
        Assert-True ($doctor.code -eq 0 -and $doctor.report.ok) 'Reopened restored environment must pass doctor'
        Assert-True (@(Get-Content -LiteralPath (Join-Path $root 'calls.jsonl')).Count -eq $calls.Count) 'Reopened doctor must not install or fetch'
        $checks++
    }

    $root = New-Fixture 'environment' @{}
    $environmentProbe = @'
param([string]$FixtureRoot)
$env:PATH = "C:\fixture-first;$(Join-Path $FixtureRoot '.tools/cargo/bin');$env:PATH"
. (Join-Path $FixtureRoot 'scripts/env.ps1')
. (Join-Path $FixtureRoot 'scripts/env.ps1')
@{toolchain=$env:RUSTUP_TOOLCHAIN;rustup=$env:RUSTUP_HOME;cargo=$env:CARGO_HOME;first_path=($env:PATH -split ';')[0];bin_count=@(($env:PATH -split ';') | Where-Object { $_ -eq (Join-Path $FixtureRoot '.tools/cargo/bin') }).Count} | ConvertTo-Json
'@
    $probePath = Join-Path $root 'environment-probe.ps1'
    $environmentProbe | Set-Content -LiteralPath $probePath -Encoding utf8
    $environment = (& $shellPath -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $probePath -FixtureRoot $root) | ConvertFrom-Json
    Assert-True ($environment.toolchain -eq '8.7.6-x86_64-fixture-msvc' -and $environment.bin_count -eq 1) 'Environment must derive pins and avoid duplicate PATH entries'
    Assert-True ($environment.first_path -eq (Join-Path $root '.tools/cargo/bin')) 'Portable tools must precede other PATH entries'
    Assert-True ($environment.rustup -eq (Join-Path $root '.tools/rustup') -and $environment.cargo -eq (Join-Path $root '.tools/cargo')) 'Environment homes must remain project-local'
    $checks++
    Write-Output "Bootstrap regression checks passed: $checks scenarios; all external tools mocked."
} finally {
    # Only remove the unique fixture directory created under this workspace.
    $resolvedFixture = [IO.Path]::GetFullPath($fixtureRoot)
    $resolvedParent = [IO.Path]::GetFullPath($fixtureParent).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolvedFixture.StartsWith($resolvedParent, [StringComparison]::OrdinalIgnoreCase)) { throw 'Fixture cleanup leaves the workspace' }
    if (Test-Path -LiteralPath $resolvedFixture) { Remove-Item -LiteralPath $resolvedFixture -Recurse -Force }
}
