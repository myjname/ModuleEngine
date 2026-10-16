param(
    [switch]$NativeBenchmarks,
    [ValidateSet('quick', 'contracts', 'native', 'native-tools', 'package')]
    [string]$Profile,
    [string]$Python,
    [switch]$Plan,
    [switch]$List,
    [string]$OutputRoot,
    [ValidateScript({ $_ -gt 0 -and -not [double]::IsInfinity($_) -and -not [double]::IsNaN($_) })]
    [double]$Timeout
)
$ErrorActionPreference = "Stop"

# The opt-in runner dispatches before legacy environment/bootstrap work. In
# particular --plan and --list must remain read-only on an unprepared checkout.
$verificationNewArguments = @(@('Profile', 'Python', 'Plan', 'List', 'OutputRoot', 'Timeout') |
    Where-Object { $PSBoundParameters.ContainsKey($_) })
if ($verificationNewArguments.Count -gt 0) {
    if ($NativeBenchmarks) { throw '-NativeBenchmarks cannot be combined with profile runner arguments.' }
    if ($Plan -and $List) { throw '-Plan and -List cannot be combined.' }
    if (-not $Profile -and -not $List) { throw 'Choose -Profile, or use -List to list the available profiles.' }
    if (-not $Profile -and ($PSBoundParameters.ContainsKey('OutputRoot') -or $PSBoundParameters.ContainsKey('Timeout'))) {
        throw '-OutputRoot and -Timeout require -Profile.'
    }

    $verificationCandidates = @()
    $verificationExplicitPython = $Python
    if (-not $PSBoundParameters.ContainsKey('Python')) { $verificationExplicitPython = $env:MODULE_ENGINE_VERIFY_PYTHON }
    if ($PSBoundParameters.ContainsKey('Python') -or $env:MODULE_ENGINE_VERIFY_PYTHON) {
        if (-not $verificationExplicitPython -or -not [System.IO.Path]::IsPathRooted($verificationExplicitPython)) {
            throw '-Python / MODULE_ENGINE_VERIFY_PYTHON must name an absolute Python executable.'
        }
        $verificationCandidates += [pscustomobject]@{ Path = $verificationExplicitPython; Prefix = @() }
    } else {
        foreach ($verificationCommandName in @('py', 'python3', 'python')) {
            foreach ($verificationCommand in @(Get-Command $verificationCommandName -CommandType Application -All -ErrorAction SilentlyContinue)) {
                $verificationPrefix = @()
                if ($verificationCommandName -eq 'py') { $verificationPrefix = @('-3') }
                $verificationCandidates += [pscustomobject]@{ Path = $verificationCommand.Source; Prefix = $verificationPrefix }
            }
        }
    }
    $verificationInterpreter = $null
    foreach ($verificationCandidate in $verificationCandidates) {
        # Never invoke the Windows Store execution aliases while probing Python.
        if ($verificationCandidate.Path -match '(?i)[\\/]WindowsApps[\\/]' -or
            -not (Test-Path -LiteralPath $verificationCandidate.Path -PathType Leaf)) { continue }
        try {
            $verificationPrefix = @($verificationCandidate.Prefix)
            $LASTEXITCODE = 0
            $verificationProbe = @(& $verificationCandidate.Path @verificationPrefix -c 'import json, sys; print(json.dumps(dict(executable=sys.executable, version=list(sys.version_info[:2]))))' 2>$null)
            $verificationProbeExit = $LASTEXITCODE
            if ($verificationProbeExit -ne 0) { continue }
            $verificationPythonInfo = ($verificationProbe -join "`n") | ConvertFrom-Json
            if ($verificationPythonInfo.version.Count -ne 2 -or
                [int]$verificationPythonInfo.version[0] -ne 3 -or [int]$verificationPythonInfo.version[1] -lt 9 -or
                -not [System.IO.Path]::IsPathRooted([string]$verificationPythonInfo.executable) -or
                $verificationPythonInfo.executable -match '(?i)[\\/]WindowsApps[\\/]' -or
                -not (Test-Path -LiteralPath $verificationPythonInfo.executable -PathType Leaf)) { continue }
            $verificationInterpreter = $verificationCandidate
            break
        } catch {
            # An unavailable launcher/candidate is not a usable Python runtime.
        }
    }
    if ($null -eq $verificationInterpreter) {
        throw 'Python 3.9+ was not found. Pass -Python <absolute-python.exe> or set MODULE_ENGINE_VERIFY_PYTHON; WindowsApps aliases are ignored.'
    }
    $verificationRunnerArguments = @()
    if ($Profile) { $verificationRunnerArguments += @('--profile', $Profile) }
    if ($Plan) { $verificationRunnerArguments += '--plan' }
    if ($List) { $verificationRunnerArguments += '--list' }
    if ($PSBoundParameters.ContainsKey('OutputRoot')) { $verificationRunnerArguments += @('--output-root', $OutputRoot) }
    if ($PSBoundParameters.ContainsKey('Timeout')) {
        $verificationRunnerArguments += @('--timeout', $Timeout.ToString([System.Globalization.CultureInfo]::InvariantCulture))
    }
    $verificationPrefix = @($verificationInterpreter.Prefix)
    $LASTEXITCODE = 0
    & $verificationInterpreter.Path @verificationPrefix (Join-Path $PSScriptRoot 'scripts/verify.py') @verificationRunnerArguments
    exit $LASTEXITCODE
}

# These helpers are used only by the Python-free legacy native benchmark path.
function Get-VerificationCanonicalPath([string]$Path, [switch]$Directory) {
    if (-not ('ModuleEngineVerificationNativePath' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class ModuleEngineVerificationNativePath {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern uint GetFinalPathNameByHandleW(SafeFileHandle handle, StringBuilder path, uint size, uint flags);
    public static string Resolve(string path, bool directory) {
        using (SafeFileHandle handle = CreateFileW(path, 0, 7, IntPtr.Zero, 3, directory ? 0x02000000U : 0U, IntPtr.Zero)) {
            if (handle.IsInvalid) throw new Win32Exception(Marshal.GetLastWin32Error());
            StringBuilder buffer = new StringBuilder(32768);
            uint length = GetFinalPathNameByHandleW(handle, buffer, (uint)buffer.Capacity, 0);
            if (length == 0 || length >= buffer.Capacity) throw new Win32Exception(Marshal.GetLastWin32Error());
            string result = buffer.ToString();
            if (result.StartsWith(@"\\?\UNC\")) return @"\\" + result.Substring(8);
            if (result.StartsWith(@"\\?\")) return result.Substring(4);
            return result;
        }
    }
}
'@
    }
    return [ModuleEngineVerificationNativePath]::Resolve([System.IO.Path]::GetFullPath($Path), [bool]$Directory)
}

function Get-VerificationLegacyEditorExecutable {
    $LASTEXITCODE = 0
    $verificationHostLines = @(rustc -vV)
    if ($LASTEXITCODE -ne 0) { throw 'Native Rust host discovery failed.' }
    $verificationHostMatches = @($verificationHostLines | Where-Object { $_ -match '^host: ' })
    if ($verificationHostMatches.Count -ne 1) { throw 'Rust did not identify exactly one host.' }
    $verificationHost = $verificationHostMatches[0].Substring(6).Trim()
    $verificationMachine = switch -Regex ($verificationHost) {
        '^x86_64-pc-windows-(msvc|gnu|gnullvm)$' { 0x8664; break }
        '^i686-pc-windows-(msvc|gnu|gnullvm)$' { 0x014c; break }
        '^aarch64-pc-windows-(msvc|gnu|gnullvm)$' { 0xaa64; break }
        default { throw "A native Windows Rust host is required; found $verificationHost." }
    }
    $verificationOsArchitecture = $env:PROCESSOR_ARCHITEW6432
    if (-not $verificationOsArchitecture) { $verificationOsArchitecture = $env:PROCESSOR_ARCHITECTURE }
    $verificationOsMachine = switch ($verificationOsArchitecture.ToUpperInvariant()) {
        'AMD64' { 0x8664 }
        'X86' { 0x014c }
        'ARM64' { 0xaa64 }
        default { throw "Unsupported Windows architecture: $verificationOsArchitecture." }
    }
    if ($verificationMachine -ne $verificationOsMachine) { throw "Rust host $verificationHost does not match the native Windows architecture." }

    $LASTEXITCODE = 0
    $verificationMetadataLines = @(cargo metadata --offline --locked --no-deps --format-version 1)
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $verificationMetadata = ($verificationMetadataLines -join "`n") | ConvertFrom-Json
    $verificationExpectedManifest = Get-VerificationCanonicalPath (Join-Path $PSScriptRoot 'crates/editor/Cargo.toml')
    $verificationPackages = @($verificationMetadata.packages | Where-Object {
        $_.name -eq 'module-engine-editor' -and
        (Get-VerificationCanonicalPath $_.manifest_path) -eq $verificationExpectedManifest
    })
    if ($verificationPackages.Count -ne 1) { throw 'Cargo metadata did not identify exactly one Editor package/manifest.' }
    $verificationPackage = $verificationPackages[0]
    $verificationTargets = @($verificationPackage.targets | Where-Object {
        $_.name -eq 'module-engine-editor' -and @($_.kind).Count -eq 1 -and $_.kind[0] -eq 'bin'
    })
    if ($verificationTargets.Count -ne 1) { throw 'Cargo metadata did not identify exactly one Editor binary target.' }
    $verificationTarget = $verificationTargets[0]
    $LASTEXITCODE = 0
    $verificationBuildLines = @(cargo build --offline --locked --release --target $verificationHost -p module-engine-editor --bin module-engine-editor --message-format=json)
    $verificationBuildExit = $LASTEXITCODE
    $verificationFinished = @()
    $verificationExecutables = @()
    foreach ($verificationBuildLine in $verificationBuildLines) {
        $verificationBuildText = [string]$verificationBuildLine
        if (-not $verificationBuildText.TrimStart().StartsWith('{')) {
            Write-Host $verificationBuildText
            continue
        }
        $verificationMessage = $verificationBuildText | ConvertFrom-Json
        if ($verificationMessage.reason -eq 'compiler-message' -and $verificationMessage.message.rendered) {
            Write-Host -NoNewline $verificationMessage.message.rendered
        }
        if ($verificationMessage.reason -eq 'build-finished') { $verificationFinished += $verificationMessage }
        if ($verificationMessage.reason -eq 'compiler-artifact' -and
            $verificationMessage.package_id -eq $verificationPackage.id -and
            $verificationMessage.target.name -eq $verificationTarget.name -and
            @($verificationMessage.target.kind).Count -eq 1 -and $verificationMessage.target.kind[0] -eq 'bin' -and
            $verificationMessage.target.src_path -eq $verificationTarget.src_path -and
            $verificationMessage.profile.test -is [bool] -and -not $verificationMessage.profile.test -and $verificationMessage.executable -and
            (Get-VerificationCanonicalPath $verificationMessage.manifest_path) -eq $verificationExpectedManifest) {
            $verificationExecutables += [string]$verificationMessage.executable
        }
    }
    if ($verificationBuildExit -ne 0) { throw 'Release build failed.' }
    if ($verificationFinished.Count -ne 1 -or $verificationFinished[0].success -isnot [bool] -or -not $verificationFinished[0].success) {
        throw 'CargoJSON must contain exactly one successful build-finished message.'
    }
    $verificationExecutables = @($verificationExecutables | Sort-Object -Unique)
    if ($verificationExecutables.Count -ne 1) { throw 'CargoJSON did not identify exactly one Editor executable.' }
    if (-not [System.IO.Path]::IsPathRooted($verificationExecutables[0]) -or
        -not [System.IO.Path]::IsPathRooted([string]$verificationMetadata.target_directory)) {
        throw 'CargoJSON executable and target directory must be absolute paths.'
    }
    $verificationExecutable = Get-VerificationCanonicalPath $verificationExecutables[0]
    $verificationTargetDirectory = (Get-VerificationCanonicalPath $verificationMetadata.target_directory -Directory).TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if (-not $verificationExecutable.StartsWith($verificationTargetDirectory, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Cargo executable escapes the actual target directory.'
    }
    $verificationStream = [System.IO.File]::OpenRead($verificationExecutable)
    $verificationReader = New-Object System.IO.BinaryReader($verificationStream)
    try {
        if ($verificationStream.Length -lt 64 -or $verificationReader.ReadUInt16() -ne 0x5a4d) { throw 'Editor artifact is not a PE executable.' }
        $verificationStream.Position = 0x3c
        $verificationPeOffset = $verificationReader.ReadUInt32()
        if ($verificationPeOffset -lt 64 -or $verificationPeOffset -gt $verificationStream.Length - 26) { throw 'Editor artifact has an invalid PE header offset.' }
        $verificationStream.Position = $verificationPeOffset
        if ($verificationReader.ReadUInt32() -ne 0x00004550 -or $verificationReader.ReadUInt16() -ne $verificationMachine) {
            throw 'Editor artifact is not a PE executable for the native Rust host.'
        }
        $verificationSectionCount = $verificationReader.ReadUInt16()
        $verificationStream.Position = $verificationPeOffset + 20
        $verificationOptionalSize = $verificationReader.ReadUInt16()
        $verificationStream.Position = $verificationPeOffset + 22
        $verificationCharacteristics = $verificationReader.ReadUInt16()
        if (($verificationCharacteristics -band 0x0002) -eq 0 -or ($verificationCharacteristics -band 0x2000) -ne 0) {
            throw 'Editor PE artifact must be an executable, not a DLL.'
        }
        $verificationMagic = $verificationReader.ReadUInt16()
        $verificationExpectedMagic = if ($verificationMachine -eq 0x014c) { 0x010b } else { 0x020b }
        $verificationMinimumOptionalSize = if ($verificationMachine -eq 0x014c) { 224 } else { 240 }
        if ($verificationMagic -ne $verificationExpectedMagic -or $verificationSectionCount -lt 1 -or
            $verificationOptionalSize -lt $verificationMinimumOptionalSize -or
            $verificationPeOffset + 24 + $verificationOptionalSize + 40 * $verificationSectionCount -gt $verificationStream.Length) {
            throw 'Editor PE optional header/sections do not match its architecture.'
        }
    } finally {
        $verificationReader.Dispose()
    }
    $verificationHash = (Get-FileHash -LiteralPath $verificationExecutable -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "Native Editor artifact: $verificationExecutable (SHA-256 $verificationHash)"
    return $verificationExecutable
}

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
        $verificationExe = Get-VerificationLegacyEditorExecutable
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


