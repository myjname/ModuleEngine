param([switch]$Restore, [switch]$Json)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$pins = Get-Content -LiteralPath (Join-Path $projectRoot 'toolchain-manifest.json') -Raw | ConvertFrom-Json
if ($pins.schema_version -ne 1) { throw 'Unsupported tool manifest' }
if ($env:OS -ne 'Windows_NT') { throw 'This prototype supports Windows x64.' }
$localRoot = Join-Path $projectRoot '.tools'
$localRustup = Join-Path $localRoot 'rustup'
$localCargo = Join-Path $localRoot 'cargo'
$rustToolchain = "$($pins.rust.version)-$($pins.rust.target)"
if (Test-Path -LiteralPath (Join-Path $localRoot 'env.ps1')) { . (Join-Path $localRoot 'env.ps1') }
function Find-Tool([string]$name) { $command=Get-Command $name -ErrorAction SilentlyContinue; if ($command) { return $command.Source }; return $null }
function Read-Version([string]$path) { if (-not $path -or -not (Test-Path -LiteralPath $path -PathType Leaf)) { return '' }; try { $text=& $path --version 2>&1; if ($LASTEXITCODE -ne 0) { return '' }; return ($text -join [Environment]::NewLine) } catch { return '' } }
function Read-RustHost([string]$path) { if (-not $path -or -not (Test-Path -LiteralPath $path -PathType Leaf)) { return '' }; try { $details=& $path -vV 2>&1; if ($LASTEXITCODE -ne 0) { return '' }; $hostLine=$details | Where-Object { $_ -match '^host: ' } | Select-Object -First 1; if ($hostLine) { return $hostLine.Substring(6).Trim() }; return '' } catch { return '' } }
function Read-RustTools {
    $rustcPath=Find-Tool 'rustc.exe'; $version=Read-Version $rustcPath; $rustHost=Read-RustHost $rustcPath
    $tools=[ordered]@{
        rust=[ordered]@{ok=(($version -match ('^rustc ' + [regex]::Escape([string]$pins.rust.version) + ' ')) -and $rustHost -eq $pins.rust.target);path=$rustcPath;version=$version;host=$rustHost}
    }
    foreach ($tool in @(
        @{key='cargo';name='cargo.exe';pattern=('^cargo ' + [regex]::Escape([string]$pins.rust.version) + ' ')}
        @{key='rustfmt';name='rustfmt.exe';pattern='^rustfmt '}
        @{key='cargo_fmt';name='cargo-fmt.exe';pattern='^rustfmt '}
        @{key='clippy';name='cargo-clippy.exe';pattern='^clippy '}
        @{key='clippy_driver';name='clippy-driver.exe';pattern='^clippy '}
    )) {
        $path=Find-Tool $tool.name; $version=Read-Version $path
        $tools[$tool.key]=[ordered]@{ok=($version -match $tool.pattern);path=$path;version=$version}
    }
    $tools['ok']=@($tools.Values | Where-Object { -not $_.ok }).Count -eq 0
    return [pscustomobject]$tools
}
function Ensure-Download([string]$url,[string]$target,[string]$expected) {
    if ((Test-Path -LiteralPath $target) -and (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() -eq $expected.ToLowerInvariant()) { return }
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    $temporary="$target.download-$([Guid]::NewGuid().ToString('N'))"
    Invoke-WebRequest -Uri $url -OutFile $temporary
    if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected.ToLowerInvariant()) { throw "Download hash mismatch. File retained at $temporary" }
    if (Test-Path -LiteralPath $target) { Copy-Item -LiteralPath $target -Destination "$target.backup-$([Guid]::NewGuid().ToString('N'))" }
    Move-Item -LiteralPath $temporary -Destination $target -Force
}
$rustTools=Read-RustTools
if ($Restore -and -not $rustTools.ok) {
    New-Item -ItemType Directory -Path $localRoot -Force | Out-Null
    $env:RUSTUP_HOME=$localRustup; $env:CARGO_HOME=$localCargo
    $rustup=Find-Tool 'rustup.exe'
    if ($rustup) {
        & $rustup toolchain install $rustToolchain --profile minimal --target $pins.rust.target --component rustfmt --component clippy
        if ($LASTEXITCODE -ne 0) { throw 'Portable Rust toolchain restoration failed' }
        & $rustup default $rustToolchain
        if ($LASTEXITCODE -ne 0) { throw 'Portable default toolchain selection failed' }
        $localBin=Join-Path $localCargo 'bin'
        New-Item -ItemType Directory -Path $localBin -Force | Out-Null
        # rustup dispatches Cargo/Rust tools by argv[0]; keep these shims local
        # even when the installer used an existing host rustup executable.
        foreach ($shim in @('rustup.exe','rustc.exe','cargo.exe','rustdoc.exe','rustfmt.exe','cargo-fmt.exe','cargo-clippy.exe','clippy-driver.exe')) {
            $shimPath=Join-Path $localBin $shim
            if ([IO.Path]::GetFullPath($rustup) -eq [IO.Path]::GetFullPath($shimPath)) { continue }
            if (Test-Path -LiteralPath $shimPath) { Copy-Item -LiteralPath $shimPath -Destination "$shimPath.backup-$([Guid]::NewGuid().ToString('N'))" }
            Copy-Item -LiteralPath $rustup -Destination $shimPath -Force
        }

    } else {
        $init=Join-Path $localRoot 'bootstrap/rustup-init.exe'
        if ($pins.rustup.sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Malformed pinned rustup checksum' }
        Ensure-Download $pins.rustup.url $init $pins.rustup.sha256
        & $init -y --no-modify-path --profile minimal --default-host $pins.rust.target --default-toolchain $rustToolchain --component rustfmt --component clippy
        if ($LASTEXITCODE -ne 0) { throw 'Portable rustup initialization failed' }
    }
    $environment=". (Join-Path (Split-Path -Parent `$PSScriptRoot) 'scripts/env.ps1')"
    $environment | Set-Content -LiteralPath (Join-Path $localRoot 'env.ps1') -Encoding utf8
    . (Join-Path $localRoot 'env.ps1')
    $rustTools=Read-RustTools
}
$dxc=Join-Path $projectRoot $pins.dxc.relative_executable
$dxcRoot=Split-Path -Parent $dxc
$dxcValid=$true
foreach ($file in $pins.dxc.files.PSObject.Properties) {
    $path=Join-Path $dxcRoot $file.Name
    if (-not (Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.Value) { $dxcValid=$false }
}
if ($Restore -and -not $dxcValid) {
    $archive=Join-Path $localRoot "downloads/dxc-$($pins.dxc.release).zip"
    Ensure-Download $pins.dxc.url $archive $pins.dxc.sha256
    $expanded=Join-Path $localRoot "downloads/dxc-$($pins.dxc.release)-$([Guid]::NewGuid().ToString('N'))"
    Expand-Archive -LiteralPath $archive -DestinationPath $expanded
    New-Item -ItemType Directory -Path $dxcRoot -Force | Out-Null
    foreach ($file in $pins.dxc.files.PSObject.Properties) {
        $candidates=@(Get-ChildItem -LiteralPath $expanded -Recurse -File -Filter $file.Name | Where-Object { (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -eq $file.Value })
        if ($candidates.Count -eq 0) { throw "Pinned DXC component missing from verified archive: $($file.Name)" }
        $target=Join-Path $dxcRoot $file.Name
        if (Test-Path -LiteralPath $target) { Copy-Item -LiteralPath $target -Destination "$target.backup-$([Guid]::NewGuid().ToString('N'))" }
        Copy-Item -LiteralPath $candidates[0].FullName -Destination $target -Force
    }
    $dxcValid=$true
}
$dxcVersion=Read-Version $dxc
$cargo=$rustTools.cargo.path
if ($Restore -and $rustTools.cargo.ok) {
    $projectCargoCache=Join-Path $projectRoot '.work/cargo-home'
    New-Item -ItemType Directory -Path $projectCargoCache -Force | Out-Null
    $env:CARGO_HOME=$projectCargoCache
    $cargoManifest=Join-Path $projectRoot 'Cargo.toml'
    if (-not (Test-Path -LiteralPath $cargoManifest -PathType Leaf)) { $cargoManifest=Join-Path $projectRoot '.sdk/Cargo.toml' }
    if (-not (Test-Path -LiteralPath $cargoManifest -PathType Leaf)) { throw 'Full source workspace or Team SDK Cargo.toml is missing' }
    Push-Location $projectRoot
    try { & $cargo fetch --locked --manifest-path $cargoManifest; if ($LASTEXITCODE -ne 0) { throw 'Pinned Cargo dependency restoration failed' } } finally { Pop-Location }
}
$rustValid=$rustTools.rust.ok
$dxcValid=$dxcValid -and ($dxcVersion -match [regex]::Escape($pins.dxc.version))
$loader=Join-Path $env:WINDIR 'System32/vulkan-1.dll'
$report=[ordered]@{ok=($rustTools.ok -and $dxcValid);restoration_requested=[bool]$Restore;rust=$rustTools.rust;cargo=$rustTools.cargo;rustfmt=$rustTools.rustfmt;cargo_fmt=$rustTools.cargo_fmt;clippy=$rustTools.clippy;clippy_driver=$rustTools.clippy_driver;dxc=[ordered]@{ok=$dxcValid;path=$dxc;version=$dxcVersion};vulkan_loader_present=(Test-Path -LiteralPath $loader);native_requirements=$pins.native_requirements;note='Loader presence is not device/pass validation. Run the GPU self-check. Restore downloads only project-local Rust/DXC tools; native MSVC SDK and GPU driver require the host installation.'}
if ($Json) { $report | ConvertTo-Json -Depth 8 } else { Write-Output "Rust pinned: $rustValid; Cargo pinned: $($rustTools.cargo.ok); rustfmt: $($rustTools.rustfmt.ok); cargo-fmt: $($rustTools.cargo_fmt.ok); Clippy: $($rustTools.clippy.ok); Clippy driver: $($rustTools.clippy_driver.ok); DXC pinned + component hashes: $dxcValid; Vulkan loader: $($report.vulkan_loader_present)"; $pins.native_requirements | ForEach-Object { Write-Output $_ }; if (-not $report.ok) { Write-Output 'Invoke scripts/bootstrap.ps1 -Restore to restore pinned portable tools and Cargo dependencies.' } }
if (-not $report.ok) { exit 1 }







