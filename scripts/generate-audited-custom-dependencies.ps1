#requires -Version 7.3
<#
.SYNOPSIS
Regenerate the audited Custom worker registry closure from checksum-verified .crate archives.
.DESCRIPTION
Use an actual generated Custom worker Cargo.lock, not the full workspace lock.
Review dependency versions, build scripts and proc macros before updating the audited set.
No network, extraction or Cargo execution is performed by this script.
#>
param(
    [Parameter(Mandatory)][string]$LockPath,
    [Parameter(Mandatory)][string]$CargoHome,
    [string]$RustOutput = (Join-Path $PSScriptRoot '../crates/runtime-script/src/freshness/audited_dependencies.rs'),
    [Parameter(Mandatory)][string]$EvidenceOutput,
    [int]$ExpectedPackageCount = 23
)
$ErrorActionPreference = 'Stop'
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$rows = [Collections.Generic.List[object]]::new()
foreach ($block in ([IO.File]::ReadAllText($LockPath) -split '\[\[package\]\]')) {
    if ($block -notmatch '(?m)^source = "registry\+') { continue }
    $name = [regex]::Match($block, '(?m)^name = "([^"]+)"').Groups[1].Value
    $version = [regex]::Match($block, '(?m)^version = "([^"]+)"').Groups[1].Value
    $source = [regex]::Match($block, '(?m)^source = "([^"]+)"').Groups[1].Value
    $expected = [regex]::Match($block, '(?m)^checksum = "([a-f0-9]{64})"').Groups[1].Value
    if (-not $expected) { throw "No archive checksum for $name" }
    $archives = @(Get-ChildItem -LiteralPath (Join-Path $CargoHome 'registry/cache') -Directory | ForEach-Object {
        $path = Join-Path $_.FullName "$name-$version.crate"
        if ([IO.File]::Exists($path)) { $path }
    })
    if ($archives.Count -ne 1) { throw "Ambiguous/missing archive for $name-$version" }
    $actual = (Get-FileHash -LiteralPath $archives[0] -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw "Archive SHA differs from lock: $name-$version" }
    $files = [Collections.Generic.Dictionary[string,byte[]]]::new([StringComparer]::Ordinal)
    $archive = [IO.File]::OpenRead($archives[0])
    $gzip = [IO.Compression.GZipStream]::new($archive, [IO.Compression.CompressionMode]::Decompress)
    $reader = [System.Formats.Tar.TarReader]::new($gzip)
    try {
        while ($entry = $reader.GetNextEntry()) {
            if ($entry.EntryType -eq [System.Formats.Tar.TarEntryType]::Directory) { continue }
            if ($entry.EntryType -notin @([System.Formats.Tar.TarEntryType]::RegularFile, [System.Formats.Tar.TarEntryType]::V7RegularFile)) { throw 'Non-regular tar input' }
            $prefix = "$name-$version/"
            if (-not $entry.Name.StartsWith($prefix, [StringComparison]::Ordinal)) { throw 'Archive path escapes package' }
            $relative = $entry.Name.Substring($prefix.Length)
            if (-not $relative -or $relative.Contains('\') -or $relative.Contains([char]0xfffd) -or @($relative.Split('/') | Where-Object { -not $_ -or $_ -in @('.','..') }).Count) { throw 'Unsupported archive path' }
            if ($relative -eq '.cargo-ok') { throw 'Archive unexpectedly supplies generated Cargo marker' }
            $buffer = [IO.MemoryStream]::new()
            try { $entry.DataStream.CopyTo($buffer); $files.Add($relative, $buffer.ToArray()) } finally { $buffer.Dispose() }
        }
    } finally { $reader.Dispose(); $gzip.Dispose(); $archive.Dispose() }
    $ordered = @($files.Keys | ForEach-Object { [pscustomobject]@{path=$_;key=[Convert]::ToHexString($utf8.GetBytes($_))} } | Sort-Object -Property key)
    $hash = [Security.Cryptography.IncrementalHash]::CreateHash([Security.Cryptography.HashAlgorithmName]::SHA256)
    $hash.AppendData($utf8.GetBytes("ModuleEngine audited registry tree v1`0"))
    $hash.AppendData([BitConverter]::GetBytes([UInt64]$ordered.Count))
    $total = 0L
    foreach ($item in $ordered) {
        $pathBytes = $utf8.GetBytes($item.path)
        $bytes = $files[$item.path]
        $hash.AppendData([BitConverter]::GetBytes([UInt64]$pathBytes.Length))
        $hash.AppendData($pathBytes)
        $hash.AppendData([BitConverter]::GetBytes([UInt64]$bytes.Length))
        $hash.AppendData($bytes)
        $total += $bytes.Length
    }
    $digest = [Convert]::ToHexString($hash.GetHashAndReset()).ToLowerInvariant()
    $hash.Dispose()
    $rows.Add([pscustomobject]@{name=$name;version=$version;source=$source;archive_sha256=$actual;files=$ordered.Count;bytes=$total;tree_sha256=$digest})
}
if ($rows.Count -ne $ExpectedPackageCount) { throw "Expected audited closure of $ExpectedPackageCount registry packages, got $($rows.Count)" }
$lines = [Collections.Generic.List[string]]::new()
$lines.Add('// Generated from archive-SHA-verified Cargo packages; do not hand-edit digests.')
$lines.Add('// Generator/evidence: scripts/generate-audited-custom-dependencies.ps1')
$lines.Add('// Framing: magic+NUL, u64 LE file count, sorted UTF-8 slash paths:')
$lines.Add('// u64 LE path byte length, path bytes, u64 LE content byte length, content bytes.')
$lines.Add('// Only extracted-tree root .cargo-ok is excluded (Cargo-generated marker).')
$lines.Add('pub(super) struct AuditedPackage { pub name: &''static str, pub version: &''static str, pub source: &''static str, pub archive: &''static str, pub files: usize, pub bytes: u64, pub tree: &''static str }')
$lines.Add('pub(super) const PACKAGES: &[AuditedPackage] = &[')
foreach ($row in $rows) {
    $lines.Add(('    AuditedPackage {{ name: "{0}", version: "{1}", source: "{2}", archive: "{3}", files: {4}, bytes: {5}, tree: "{6}" }},' -f $row.name,$row.version,$row.source,$row.archive_sha256,$row.files,$row.bytes,$row.tree_sha256))
}
$lines.Add('];')
[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($RustOutput)) | Out-Null
[IO.File]::WriteAllText($RustOutput, ($lines -join "`n")+"`n", $utf8)
$evidence = [ordered]@{schema_version=1;algorithm='SHA-256';framing='ModuleEngine audited registry tree v1 NUL + u64LE count + sorted UTF8 slash relative paths: u64LE path length, path, u64LE file length, bytes';excluded_extracted_file='.cargo-ok';lock_sha256=(Get-FileHash -LiteralPath $LockPath -Algorithm SHA256).Hash.ToLowerInvariant();packages=$rows}
[IO.File]::WriteAllText($EvidenceOutput, ($evidence | ConvertTo-Json -Depth 8)+"`n", $utf8)
$rows | Select-Object name,version,files,bytes,tree_sha256 | Format-Table -AutoSize
