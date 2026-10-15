# Dot-source this file to select this project's portable, manifest-pinned Rust.
$moduleEnvironmentRoot = Split-Path -Parent $PSScriptRoot
$moduleToolchainPins = Get-Content -LiteralPath (Join-Path $moduleEnvironmentRoot 'toolchain-manifest.json') -Raw | ConvertFrom-Json
if ($moduleToolchainPins.schema_version -ne 1) { throw 'Unsupported tool manifest' }
$modulePortableRoot = Join-Path $moduleEnvironmentRoot '.tools'
$env:RUSTUP_HOME = Join-Path $modulePortableRoot 'rustup'
$env:CARGO_HOME = Join-Path $modulePortableRoot 'cargo'
$env:RUSTUP_TOOLCHAIN = "$($moduleToolchainPins.rust.version)-$($moduleToolchainPins.rust.target)"
$moduleCargoBin = Join-Path $env:CARGO_HOME 'bin'
$moduleOtherPaths = @($env:PATH -split ';' | Where-Object { $_ -ne $moduleCargoBin })
$env:PATH = (@($moduleCargoBin) + $moduleOtherPaths) -join ';'
