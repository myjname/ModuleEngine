# Reproducible artifact maintenance

[English](artifact-maintenance.md) | [Русский](ru/artifact-maintenance.md) | [Documentation](README.md)

Run `scripts/clean-artifacts.ps1` manually when disk usage warrants maintenance.
It defaults to a dry run. It is never called by ordinary application startup,
project open/save, or every Cargo build.

```powershell
# Inspect the default historical fixture candidates; changes nothing.
.\scripts\clean-artifacts.ps1 -Json

# Also inspect the two optional local caches; still changes nothing.
.\scripts\clean-artifacts.ps1 -PruneIncremental -ResetCustomCache -Json

# After reviewing the dry run and closing all builds/apps/workers, apply the
# same explicitly selected policy. This command deletes the listed artifacts.
.\scripts\clean-artifacts.ps1 -Apply -PruneIncremental -ResetCustomCache -Json

# Optional: retain targets already blocked in preflight and apply only the
# planned targets. Retained blockers stay in the report and return exit code 2.
.\scripts\clean-artifacts.ps1 -Apply -KeepBlocked -PruneIncremental -ResetCustomCache -Json
```

The workspace is the parent of the installed script directory, independent of
the shell's current directory. There is no arbitrary target-path parameter.
Default candidates are direct child directories of `.work` with these names:

* `state-test-<PID>[-<number>...]`
* `script-test-<PID>[-<number>...]`
* `authoring-state-test-<PID>[-<number>...]`
* `player-plan-test-<PID>[-<number>...]`

The default also selects only the `target` child inside historical
`.work/authoring-asset-context-<PID>[-<number>...]` fixtures. These were generated
by the editor's Custom asset-context relocation test. Their `compile` source,
logs, and `first`/`second` test data remain intact; their complete fixture roots
are never cleanup targets. Current tests use an owned fixture guard instead.

Only numeric suffixes qualify. This covers historical generated fixtures and
excludes similarly prefixed hand-named directories. Removing these fixtures
means losing their generated binaries/test data; rerunning the corresponding
tests reconstructs them. Retain a fixture elsewhere if its failure diagnostics
are still needed before choosing `-Apply`.

`-PruneIncremental` additionally selects exactly `target/debug/incremental`.
`-ResetCustomCache` additionally selects exactly `target/custom-workers`.
Neither option follows an external `CARGO_TARGET_DIR`, a Team SDK's independent
target directory, or another installation. The script never selects the entire
`.work` or `target`, `.work/game-scripts`, Cargo download homes, application
projects, output reports, exports, asset libraries, backup directories, SDKs or
installed tools.

## Preflight and failures

Native `CimCmdlets\Get-CimInstance Win32_Process` is required. Inventory failure
blocks cleanup, including when CIM is denied by the execution sandbox. Run the
reviewed script in a PowerShell session with native CIM access; an elevated
session may be required. There is no process-inventory override or fallback to
an incomplete process list.

Any active Cargo/rustc/rustdoc/Clippy process blocks maintenance, even if its
workspace cannot be determined. A running Custom worker, editor/Player or
workspace executable also blocks it. A historical fixture whose PID is still
present is retained; PID reuse can therefore conservatively block an old
fixture. The script never kills a process. Close it normally and rerun the dry
run; do not start new builds or applications during an apply pass.

Every recursive target is an absolute child of the resolved workspace and must
match the fixed allowlist. Reparse points, junctions and symbolic links are
rejected in the workspace ancestors, candidate ancestors and candidate trees.
Traversal checks attributes before descending, so linked destinations are not
enumerated. Locked/unreadable files are diagnosed using an exclusive read-open
probe. A blocked target prevents the entire initial apply pass by default.

`-KeepBlocked` explicitly retains those preflight-blocked targets and selects
only targets already marked `planned`. For example, an unrelated process can
reuse an old fixture's PID; that fixture remains untouched while other safe
fixtures can be removed. No process protection is disabled. Unavailable CIM
still prevents the whole apply pass, and a global build/application blocker
leaves no target eligible. The flag can also be supplied during a dry run to
inspect its selection. `keep_blocked`, `apply_preflight_blocked`,
`retained_blocked_count` and `eligible_logical_bytes` make that choice explicit
in the report. Retained targets keep their `blocked` status and diagnostics.

Before each selected deletion the script repeats path, native process and tree
checks. A new blocker, later access failure or process change aborts the
remaining targets even with `-KeepBlocked`; it never skips a newly blocked
target and continues deleting.
Filesystem removal is not transactional: `failed-or-partial` means a target can
already be partly removed. Such a report must not be described as a rollback or
a completed cleanup. Previously removed targets remain reported as `removed`.

Exit code 0 means the dry-run/apply completed without blockers. Exit code 2 means
cleanup has retained blockers or an apply was incomplete. This includes a
successful removal of eligible targets with `-KeepBlocked`; inspect the
individual `removed`/`blocked` statuses. Invalid paths/parameters and
other terminating PowerShell errors also fail the command without authorizing
further deletion. `-Json` emits paths, statuses, process blockers and diagnostics
for review; it never includes process command lines.

## Size and cache policy

`logical_bytes` sums file lengths. It is useful for comparing candidate groups,
but compression, sparse files, hard links, open handles and filesystem behavior
mean it is not the number of bytes freed. `removed_logical_bytes` counts only
fully removed targets. `available_bytes_before`, `available_bytes_after` and
`observed_available_bytes_delta` report actual drive free-space observations;
other activity can affect that delta, including making it negative. Preserve
the before/after reports instead of relabeling logical sizes as reclaimed space.

Keep Cargo's dependency/download caches and ordinary build outputs between
development sessions. They avoid repeated offline dependency restoration and
recompilation. Prune debug incremental state only for an explicit disk-pressure
or invalid-incremental-cache maintenance operation; the next debug build can be
slower. Reset the isolated Custom build cache only when its disk use warrants a
full worker rebuild and all relevant workers/builds have exited. Current Custom
compilation uses its own target and disables worker incrementality. Its owned
candidate/test cleanup is separate from this historical maintenance script;
failed source/log evidence is not swept by `-ResetCustomCache`.

After a completed release audit, an explicit native dev-cache reset can use
`cargo clean --offline --locked --profile dev --target-dir E:\ModuleEngine\target`.
This is outside the script's fixed allowlist: verify the exact target, reparse
entries and absence of owned Cargo/app/worker processes first. It removes debug
executables and dependency artifacts; the next debug build is cold. Release
executables, source/project data and reports outside target/debug are retained.
The 2026-10-15 audit recorded this one-off operation in
`stage8-native-dev-cache-clean.json`, with preserved project/release hashes.

## Isolated safety checks

```powershell
.\scripts\clean-artifacts.ps1 -SelfTest -Json
```

The self-test cannot be combined with `-Apply`. It creates a small unique
`.work/clean-artifacts-tests-*` fixture, uses synthetic process inventories, and
tests containment, allowlist/opt-in policy, unavailable inventory, active builds,
active workers/PIDs, locked files and a junction to an outside sentinel. It runs
no Cargo/native worker and calls no deletion path. Junction creation requires
native Windows filesystem permission and can be denied by a sandbox. The tiny
fixture, including its junction/sentinel, is deliberately retained as evidence;
its path is in the report and is outside the cleanup allowlist.
