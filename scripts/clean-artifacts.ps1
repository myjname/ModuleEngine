<#
Explicit workspace maintenance. Dry-run by default; no launch hook invokes it.
Only reproducible fixture directories and explicitly selected build caches qualify.
#>
[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$KeepBlocked,
    [switch]$PruneIncremental,
    [switch]$ResetCustomCache,
    [switch]$Json,
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-WorkspacePath([string]$Workspace, [string]$Target) {
    $workspacePath = [IO.Path]::GetFullPath($Workspace).TrimEnd('\', '/')
    $targetPath = [IO.Path]::GetFullPath($Target).TrimEnd('\', '/')
    if (-not $targetPath.StartsWith($workspacePath + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Target is not a child of the workspace: $targetPath"
    }
    # GetFullPath is lexical. Independently refuse every existing reparse-point
    # ancestor, including the workspace itself and its containing directories.
    $current = $targetPath
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Junction/symlink/reparse point refused: $current"
            }
        }
        $parent = [IO.Path]::GetDirectoryName($current)
        if ($parent -eq $current) { break }
        $current = $parent
    }
    return $targetPath
}

function Get-ArtifactKind([string]$Workspace, [string]$Target, [bool]$Incremental, [bool]$CustomCache) {
    $checkedPath = Assert-WorkspacePath $Workspace $Target
    $relative = $checkedPath.Substring([IO.Path]::GetFullPath($Workspace).TrimEnd('\', '/').Length + 1).Replace('/', '\')
    if ($relative -match '^\.work\\(state-test|script-test|authoring-state-test|player-plan-test)-[0-9]+(?:-[0-9]+)*$') { return 'historical-fixture' }
    if ($relative -match '^\.work\\authoring-asset-context-[0-9]+(?:-[0-9]+)*\\target$') { return 'historical-asset-fixture-build-cache' }
    if ($Incremental -and $relative -eq 'target\debug\incremental') { return 'incremental-cache' }
    if ($CustomCache -and $relative -eq 'target\custom-workers') { return 'custom-worker-cache' }
    throw "Path is outside the selected reproducible-artifact allowlist: $checkedPath"
}

function Read-NativeProcessInventory {
    try {
        if ($env:OS -ne 'Windows_NT') { throw 'Native Win32_Process inventory requires Windows.' }
        $processes = @(CimCmdlets\Get-CimInstance -ClassName Win32_Process -Property ProcessId, Name, ExecutablePath -ErrorAction Stop)
        if ($processes.Count -eq 0) { throw 'CIM returned an empty process inventory.' }
        return [pscustomobject]@{ available = $true; processes = $processes; error = $null }
    } catch {
        return [pscustomobject]@{
            available = $false; processes = @()
            error = "Native CIM process inventory unavailable; cleanup is blocked. Retry in a PowerShell session with CIM access: $($_.Exception.Message)"
        }
    }
}

function Get-ProcessBlockers([string]$Workspace, [object]$Inventory, [string]$Target = '') {
    $blockers = [Collections.Generic.List[object]]::new()
    if (-not $Inventory.available) {
        $blockers.Add([pscustomobject]@{ reason = 'inventory-unavailable'; process_id = $null; name = $null; executable = $null; detail = $Inventory.error })
        return $blockers.ToArray()
    }
    $prefix = [IO.Path]::GetFullPath($Workspace).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $ownerPid = $null
    $ownerName = [IO.Path]::GetFileName($Target)
    if ($ownerName -eq 'target') { $ownerName = [IO.Path]::GetFileName([IO.Path]::GetDirectoryName($Target)) }
    if ($Target -and $ownerName -match '^(?:state-test|script-test|authoring-state-test|player-plan-test|authoring-asset-context)-([0-9]+)(?:-[0-9]+)*$') {
        $ownerPid = [long]$Matches[1]
    }
    foreach ($process in $Inventory.processes) {
        $name = [string]$process.Name
        $executable = [string]$process.ExecutablePath
        $reason = $null
        if ($name -match '^(cargo(?:-clippy)?|rustc|rustdoc|clippy-driver)(?:\.exe)?$') { $reason = 'active-rust-build' }
        elseif ($name -match '^(module[-_]engine[-_]worker_.+|module-engine-editor|module-engine-player|ModuleEngine(?:\.Player)?|Player)(?:\.exe)?$') { $reason = 'active-worker-or-application' }
        elseif ($executable -and [IO.Path]::GetFullPath($executable).StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { $reason = 'active-workspace-executable' }
        elseif ($null -ne $ownerPid -and [long]$process.ProcessId -eq $ownerPid) { $reason = 'fixture-owner-pid-still-active' }
        if ($reason) {
            $blockers.Add([pscustomobject]@{ reason = $reason; process_id = [long]$process.ProcessId; name = $name; executable = $executable; detail = $null })
        }
    }
    return $blockers.ToArray()
}

function Measure-SafeArtifact([string]$Workspace, [string]$Target, [bool]$ProbeLocks = $true) {
    $checkedPath = Assert-WorkspacePath $Workspace $Target
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($checkedPath)
    $errors = [Collections.Generic.List[string]]::new()
    [long]$logicalBytes = 0
    [long]$fileCount = 0
    [long]$directoryCount = 0
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        try {
            $item = [IO.DirectoryInfo]::new($directory)
            $item.Refresh()
            if (-not $item.Exists) { throw "Expected readable directory: $directory" }
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse point refused: $directory" }
            $directoryCount++
            # Enumerate one directory through .NET, avoiding PowerShell provider
            # overhead. Inspect every entry before descending; never use a
            # recursive enumeration that could traverse a linked destination.
            foreach ($child in $item.EnumerateFileSystemInfos()) {
                $attributes = $child.Attributes
                if (($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    $errors.Add("Reparse point refused without traversing: $($child.FullName)")
                    continue
                }
                if (($attributes -band [IO.FileAttributes]::Directory) -ne 0) { $pending.Push($child.FullName); continue }
                $fileCount++
                $logicalBytes += $child.Length
                if ($ProbeLocks) {
                    try {
                        $stream = [IO.File]::Open($child.FullName, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
                        $stream.Dispose()
                    } catch { $errors.Add("Locked/unreadable file refused: $($child.FullName): $($_.Exception.Message)") }
                }
            }
        } catch { $errors.Add($_.Exception.Message) }
    }
    return [pscustomobject]@{
        logical_bytes = $logicalBytes; files = $fileCount; directories = $directoryCount
        complete = ($errors.Count -eq 0); diagnostic_count = $errors.Count
        diagnostics = @($errors | Select-Object -First 16)
    }
}

function Get-ArtifactCandidates([string]$Workspace, [bool]$Incremental, [bool]$CustomCache) {
    $candidates = [Collections.Generic.List[string]]::new()
    $work = Join-Path $Workspace '.work'
    if (Test-Path -LiteralPath $work) {
        $null = Assert-WorkspacePath $Workspace $work
        foreach ($item in Get-ChildItem -LiteralPath $work -Directory -Force -ErrorAction Stop) {
            if ($item.Name -match '^(state-test|script-test|authoring-state-test|player-plan-test)-[0-9]+(?:-[0-9]+)*$') { $candidates.Add($item.FullName) }
            if ($item.Name -match '^authoring-asset-context-[0-9]+(?:-[0-9]+)*$') { $candidates.Add((Join-Path $item.FullName 'target')) }
        }
    }
    if ($Incremental) { $candidates.Add((Join-Path $Workspace 'target/debug/incremental')) }
    if ($CustomCache) { $candidates.Add((Join-Path $Workspace 'target/custom-workers')) }
    return @($candidates | Sort-Object)
}

function New-ArtifactPlan([string]$Workspace, [bool]$Incremental, [bool]$CustomCache, [object]$Inventory) {
    $records = [Collections.Generic.List[object]]::new()
    foreach ($path in Get-ArtifactCandidates $Workspace $Incremental $CustomCache) {
        $record = [ordered]@{ path = $path; kind = $null; status = 'blocked'; logical_bytes = 0L; files = 0L; directories = 0L; complete = $false; blockers = @(); diagnostics = @() }
        try {
            $record.kind = Get-ArtifactKind $Workspace $path $Incremental $CustomCache
            $record.blockers = @(Get-ProcessBlockers $Workspace $Inventory $path)
            if (-not (Test-Path -LiteralPath $path)) { $record.status = 'missing'; $record.complete = $true }
            elseif ($record.blockers.Count -eq 0) {
                $measurement = Measure-SafeArtifact $Workspace $path
                $record.logical_bytes = $measurement.logical_bytes
                $record.files = $measurement.files
                $record.directories = $measurement.directories
                $record.complete = $measurement.complete
                $record.diagnostics = $measurement.diagnostics
                if ($measurement.complete) { $record.status = 'planned' }
            }
        } catch { $record.diagnostics = @($_.Exception.Message) }
        $records.Add([pscustomobject]$record)
    }
    return $records.ToArray()
}

function Read-AvailableBytes([string]$Workspace) {
    try { return [IO.DriveInfo]::new([IO.Path]::GetPathRoot($Workspace)).AvailableFreeSpace } catch { return $null }
}

function Get-ArtifactApplyPlan([object[]]$Records, [bool]$RetainBlocked, [bool]$InventoryAvailable) {
    $retained = @($Records | Where-Object status -eq 'blocked')
    $preflightBlocked = -not $InventoryAvailable -or ($retained.Count -gt 0 -and -not $RetainBlocked)
    $eligible = @()
    if (-not $preflightBlocked) { $eligible = @($Records | Where-Object status -eq 'planned') }
    return [pscustomobject]@{ preflight_blocked = $preflightBlocked; eligible = $eligible; retained_blocked = $retained }
}

function Invoke-CleanupSelfTest([string]$Workspace) {
    if ($Apply) { throw '-SelfTest cannot be combined with -Apply.' }
    $fixture = Join-Path $Workspace ('.work/clean-artifacts-tests-' + [Guid]::NewGuid().ToString('N'))
    $null = Assert-WorkspacePath $Workspace $fixture
    $sandbox = Join-Path $fixture 'workspace'
    $outside = Join-Path $fixture 'workspace-other'
    $safe = Join-Path $sandbox '.work/state-test-424242'
    New-Item -ItemType Directory -Path $safe, $outside -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $safe 'payload.bin'), 'reproducible')
    [IO.File]::WriteAllText((Join-Path $outside 'sentinel.bin'), 'must remain')
    $checks = [Collections.Generic.List[string]]::new()
    function Assert-Test([bool]$Condition, [string]$Description) { if (-not $Condition) { throw "Self-test failed: $Description" }; $checks.Add($Description) }
    $noProcesses = [pscustomobject]@{ available = $true; processes = @(); error = $null }
    $plan = @(New-ArtifactPlan $sandbox $false $false $noProcesses)
    Assert-Test ($plan.Count -eq 1 -and $plan[0].status -eq 'planned' -and $plan[0].logical_bytes -eq 12) 'allowlisted fixture is measured without deleting it'
    New-Item -ItemType Directory -Path (Join-Path $sandbox 'target/debug/incremental'), (Join-Path $sandbox 'target/custom-workers'), (Join-Path $sandbox '.work/state-test-user-project') -Force | Out-Null
    Assert-Test (@(Get-ArtifactCandidates $sandbox $false $false).Count -eq 1) 'ordinary dry-run excludes both caches and nonnumeric fixture suffixes'
    Assert-Test (@(Get-ArtifactCandidates $sandbox $true $true).Count -eq 3) 'incremental and Custom caches require explicit selection'
    $assetFixture = Join-Path $sandbox '.work/authoring-asset-context-424243'
    $assetTarget = Join-Path $assetFixture 'target'
    New-Item -ItemType Directory -Path $assetTarget -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $assetFixture 'failed-source.rs'), 'retained source')
    $assetPlan = @(New-ArtifactPlan $sandbox $false $false $noProcesses | Where-Object kind -eq 'historical-asset-fixture-build-cache')
    Assert-Test ($assetPlan.Count -eq 1 -and $assetPlan[0].path -eq $assetTarget -and (Test-Path -LiteralPath (Join-Path $assetFixture 'failed-source.rs'))) 'asset-context fixture selects only target and retains source/log evidence'
    $rejected = $false
    try { $null = Get-ArtifactKind $sandbox $assetFixture $true $true } catch { $rejected = $true }
    Assert-Test $rejected 'the asset-context fixture root is never a recursive target'
    $rejected = $false
    try { $null = Assert-WorkspacePath $sandbox $outside } catch { $rejected = $true }
    Assert-Test $rejected 'sibling with the same workspace prefix is outside containment'
    $rejected = $false
    try { $null = Assert-WorkspacePath $sandbox $sandbox } catch { $rejected = $true }
    Assert-Test $rejected 'the complete workspace cannot be a recursive target'
    $rejected = $false
    try { $null = Get-ArtifactKind $sandbox (Join-Path $sandbox 'outputs') $true $true } catch { $rejected = $true }
    Assert-Test $rejected 'output/project/export roots are outside the allowlist'
    $unavailable = [pscustomobject]@{ available = $false; processes = @(); error = 'fixture CIM access denied' }
    Assert-Test (@(New-ArtifactPlan $sandbox $false $false $unavailable)[0].status -eq 'blocked') 'unavailable native inventory fails closed'
    foreach ($name in @('cargo.exe', 'rustc.exe', 'module_engine_worker_1_2_3.exe')) {
        $active = [pscustomobject]@{ available = $true; error = $null; processes = @([pscustomobject]@{ Name = $name; ProcessId = 515151; ExecutablePath = 'C:\fixture\tool.exe' }) }
        Assert-Test (@(New-ArtifactPlan $sandbox $false $false $active)[0].status -eq 'blocked') "active $name blocks maintenance"
    }
    $active = [pscustomobject]@{ available = $true; error = $null; processes = @([pscustomobject]@{ Name = 'fixture-tests.exe'; ProcessId = 424242; ExecutablePath = 'C:\fixture\tests.exe' }) }
    $mixedPlan = @(New-ArtifactPlan $sandbox $false $false $active)
    Assert-Test (@($mixedPlan | Where-Object path -eq $safe)[0].status -eq 'blocked') 'a running fixture owner PID blocks its artifacts'
    $selection = Get-ArtifactApplyPlan $mixedPlan $false $true
    Assert-Test ($selection.preflight_blocked -and $selection.eligible.Count -eq 0) 'default apply planning remains all-or-nothing with a blocked fixture'
    $selection = Get-ArtifactApplyPlan $mixedPlan $true $true
    Assert-Test (-not $selection.preflight_blocked -and $selection.eligible.Count -eq 1 -and $selection.eligible[0].path -eq $assetTarget -and $selection.retained_blocked.Count -eq 1 -and $selection.retained_blocked[0].path -eq $safe -and (Test-Path -LiteralPath (Join-Path $safe 'payload.bin'))) 'KeepBlocked selects only already planned targets and retains the blocked fixture'
    $unavailablePlan = @(New-ArtifactPlan $sandbox $false $false $unavailable)
    $selection = Get-ArtifactApplyPlan $unavailablePlan $true $false
    Assert-Test ($selection.preflight_blocked -and $selection.eligible.Count -eq 0) 'KeepBlocked cannot bypass unavailable native inventory'
    $activeBuild = [pscustomobject]@{ available = $true; error = $null; processes = @([pscustomobject]@{ Name = 'cargo.exe'; ProcessId = 515151; ExecutablePath = 'C:\fixture\cargo.exe' }) }
    $selection = Get-ArtifactApplyPlan @(New-ArtifactPlan $sandbox $false $false $activeBuild) $true $true
    Assert-Test ($selection.eligible.Count -eq 0 -and $selection.retained_blocked.Count -eq $mixedPlan.Count) 'KeepBlocked cannot select a target while a global build blocker is active'
    $active = [pscustomobject]@{ available = $true; error = $null; processes = @([pscustomobject]@{ Name = 'fixture-tests.exe'; ProcessId = 424243; ExecutablePath = 'C:\fixture\tests.exe' }) }
    Assert-Test (@(Get-ProcessBlockers $sandbox $active $assetTarget).Count -eq 1) 'a running asset fixture owner PID blocks its target cache'
    $locked = [IO.File]::Open((Join-Path $safe 'payload.bin'), [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $plan = @(New-ArtifactPlan $sandbox $false $false $noProcesses | Where-Object path -eq $safe)
        Assert-Test ($plan[0].status -eq 'blocked' -and ($plan[0].diagnostics -join ' ') -match 'Locked/unreadable') 'locked files produce diagnostics before any deletion'
    } finally { $locked.Dispose() }
    New-Item -ItemType Junction -Path (Join-Path $safe 'outside-link') -Target $outside | Out-Null
    $plan = @(New-ArtifactPlan $sandbox $false $false $noProcesses | Where-Object path -eq $safe)
    Assert-Test ($plan[0].status -eq 'blocked' -and ($plan[0].diagnostics -join ' ') -match 'Reparse') 'nested junctions are refused without traversing their destination'
    Assert-Test (Test-Path -LiteralPath (Join-Path $outside 'sentinel.bin')) 'outside sentinel is unchanged'
    return [pscustomobject]@{ self_test = $true; checks = $checks.ToArray(); passed = $true; fixture = $fixture; deleted_paths = @(); note = 'Read-only cleanup tests: the small isolated fixture is retained; no Remove-Item was invoked.' }
}

$workspaceRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot)).TrimEnd('\', '/')
if ($SelfTest) {
    $selfTestReport = Invoke-CleanupSelfTest $workspaceRoot
    if ($Json) { $selfTestReport | ConvertTo-Json -Depth 8 } else { $selfTestReport | Format-List }
    exit 0
}
$inventory = Read-NativeProcessInventory
$beforeAvailable = Read-AvailableBytes $workspaceRoot
$records = @(New-ArtifactPlan $workspaceRoot $PruneIncremental.IsPresent $ResetCustomCache.IsPresent $inventory)
$applyPlan = Get-ArtifactApplyPlan $records $KeepBlocked.IsPresent $inventory.available
$blocked = $applyPlan.retained_blocked.Count -gt 0 -or $applyPlan.preflight_blocked
[long]$plannedLogicalBytes = 0
foreach ($record in $records) { $plannedLogicalBytes += $record.logical_bytes }
[long]$eligibleLogicalBytes = 0
foreach ($record in $applyPlan.eligible) { $eligibleLogicalBytes += $record.logical_bytes }
[long]$removedLogicalBytes = 0
if ($Apply -and -not $applyPlan.preflight_blocked) {
    foreach ($record in $applyPlan.eligible) {
        try {
            # Revalidate the exact absolute allowlist target, native process state
            # and complete tree immediately before the native recursive removal.
            $checkedPath = Assert-WorkspacePath $workspaceRoot $record.path
            $null = Get-ArtifactKind $workspaceRoot $checkedPath $PruneIncremental.IsPresent $ResetCustomCache.IsPresent
            $freshInventory = Read-NativeProcessInventory
            $blockers = @(Get-ProcessBlockers $workspaceRoot $freshInventory $checkedPath)
            if ($blockers.Count -gt 0) { $record.blockers = $blockers; throw 'Process state changed; remaining cleanup aborted.' }
            $measurement = Measure-SafeArtifact $workspaceRoot $checkedPath
            if (-not $measurement.complete) { $record.diagnostics = $measurement.diagnostics; throw 'Target became locked, unreadable or redirected; remaining cleanup aborted.' }
            Remove-Item -LiteralPath $checkedPath -Recurse -Force -ErrorAction Stop
            if (Test-Path -LiteralPath $checkedPath) { throw 'Target remains after removal.' }
            $record.status = 'removed'
            $removedLogicalBytes += $measurement.logical_bytes
        } catch {
            $record.status = 'failed-or-partial'
            $record.diagnostics += $_.Exception.Message
            $blocked = $true
            break
        }
    }
}
$afterAvailable = Read-AvailableBytes $workspaceRoot
$freeDelta = $null
if ($null -ne $beforeAvailable -and $null -ne $afterAvailable) { $freeDelta = $afterAvailable - $beforeAvailable }
$report = [pscustomobject]@{
    schema_version = 1; workspace = $workspaceRoot; apply_requested = $Apply.IsPresent
    keep_blocked = $KeepBlocked.IsPresent; apply_preflight_blocked = $applyPlan.preflight_blocked
    retained_blocked_count = $applyPlan.retained_blocked.Count
    eligible_logical_bytes = $eligibleLogicalBytes
    prune_incremental = $PruneIncremental.IsPresent; reset_custom_cache = $ResetCustomCache.IsPresent
    native_process_inventory_available = $inventory.available; inventory_error = $inventory.error
    blocked = $blocked; targets = $records
    planned_logical_bytes = $plannedLogicalBytes
    removed_logical_bytes = $removedLogicalBytes
    available_bytes_before = $beforeAvailable; available_bytes_after = $afterAvailable
    observed_available_bytes_delta = $freeDelta
    size_scope = 'Logical file lengths are not reclaimed disk space. Free-space delta is an observation affected by other activity, compression, hard links and delayed reclamation.'
    apply_scope = 'No changes unless -Apply. Default preflight is all-or-nothing. -KeepBlocked retains preflight-blocked targets and selects only planned targets; unavailable CIM never permits apply. Every selected target is checked again; a later failure stops remaining targets and can leave a partially removed target. Processes are never stopped or killed. Retained blockers remain reported and return exit code 2.'
}
if ($Json) { $report | ConvertTo-Json -Depth 10 } else {
    $records | Select-Object status, kind, logical_bytes, path | Format-Table -AutoSize
    $report | Select-Object workspace, apply_requested, keep_blocked, apply_preflight_blocked, retained_blocked_count, blocked, inventory_error, planned_logical_bytes, eligible_logical_bytes, removed_logical_bytes, observed_available_bytes_delta, size_scope | Format-List
}
if ($blocked) { exit 2 }
