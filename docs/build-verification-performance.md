# Build and verification performance

[English](build-verification-performance.md) | [Русский](ru/build-verification-performance.md) | [Documentation](README.md)

This contract separates compilation/linking, direct test execution, launcher work
and hosted cache transfer. Faster local reuse does not establish a hosted CI gain.
Tests, feature/target selection and artifact validation retain their existing scope.

## Measurement method and local evidence

The [paired report](../outputs/ci-editor-builds-20261017/measurements/20261017T022121Z-8eb11023/result.json)
and its `cold/`, `repeat/` and archived `instrumentation/` evidence describe Windows
`contracts` on HEAD `0d3eb5285bc9674b2edb50e7830c8a69f7053d23`, source digest
`fad4e5438f83cbcc5cf0847b66cabb41d25f681532702ac0a16b2840d145c06a`.
Both cycles passed 187 Rust tests, preserved all 33 protected files, and kept the
source unchanged. The new target directory `.work/b/8eb11023` did not exist before
the first cycle; the second reused it. Existing offline dependency and OS/filesystem
caches were warm/uncontrolled. This is target-cold versus repeat, not machine-cold.

Both cycles used the normal `contracts` plan, `--offline --locked`,
`CARGO_INCREMENTAL=0`, and a fresh child TEMP for each run. No compiler flags,
wrappers, profiles or tests were changed, and no shared cache was cleared.

| Wall time, seconds | Target-cold | Repeat |
| --- | ---: | ---: |
| Whole cycle | 182.375 | 89.906 |
| Cargo compile + link commands | 92.376 | 3.905 |
| Direct tests | 81.641 | 81.250 |
| Test discovery | 2.296 | 0.499 |
| Formatting | 1.859 | 1.859 |
| Setup commands | 0.187 | 0.187 |
| Outside commands | 4.016 | 2.206 |

Outside-command time includes source/protected hashing, lease/setup, observer
finalization and reporting; it is not pure Cargo overhead. The repeated test cost
stayed close to 81 seconds even when compilation fell sharply. This supports
investigating compiled-product reuse; it does not justify removing tests or changing
profiles. No sccache or nextest dependency was introduced.

A read-only Windows Toolhelp/GetProcessTimes observer sampled owned rustc/linker
processes every 50 ms. Within the cold compile/link phase, the approximate exclusive
compiler union was 79.199 seconds and the observed linker union 10.343 seconds;
no compiler/linker was observed in the repeat compile/link phase. Polls can miss
short processes or ancestry, so these are estimates, not exact compiler timings.
CPU totals from parallel processes are separate from wall time. The repeat setup
recorded one OpenProcess race error; compile/link observations had no errors or
sample-cap hits. Probe/source hashes and the archived observer sources are in the
report. These measurements do not assess Editor cold-start, hardware Linux or
hosted acceptance.

## Hosted Cargo cache contract

[scripts/ci_cache.py](../scripts/ci_cache.py) is invoked as
`python scripts/ci_cache.py` in the existing [CI checks job](ci.md). It prints a JSON
admission decision and, when `GITHUB_OUTPUT` is set, emits `enabled`, `key`,
`restore_prefix` and `native_host`. It does not change environment variables or
repair an unsupported context. Unknown flags/wrappers, effective Cargo configs,
unverified toolchain/runner identity or other unaccounted compiler environment
disable caching while the normal checks continue against that original environment.

The workflow uses one combined `actions/cache` v6 restore/save action, pinned to
`55cc8345863c7cc4c66a329aec7e433d2d1c52a9`. Allowed paths are only:

- `$CARGO_HOME/registry` and `$CARGO_HOME/git`;
- `target/debug` and `target/release`;
- `target/<verified-native-host>/debug` and `target/<verified-native-host>/release`.

Custom worker and `sdk-v3` namespaces, authored/root data, generated fixtures,
`.tools`, reports and credentials are outside this cache scope. It is not an SDK,
Custom-program-store or user-project backup. Changing the list requires checking
these boundaries again.

The compatibility prefix hashes the verified verbose Rust/Cargo identity, native
host, runner OS/architecture/image and paths, toolchain pins, lockfile, manifests
and profiles, CI/verification policy and guarded configuration inputs. The script
also includes its conservative source-policy inputs, including Core/runtime-script
sources. The exact key appends the source SHA. There is one restore prefix: the
complete compatibility prefix, with no broader fallback across incompatible
contexts. A miss or rejected cache admission is an ordinary uncached run.

Every locked dependency fetch, Clippy, contracts, selected native/package route and
artifact check still runs after restoration. A hit does not skip a verification
step or inherit an earlier pass. Existing Cargo freshness and artifact-selection
checks continue to decide whether compiled products can be reused.

## Assessing hosted benefit

Compare hosted runs with the same routes, toolchain, runner image and relevant
sources; distinguish an uncached baseline, compatible miss and compatible hit.
Record cache decisions, transfer size/time, restore and save work, compilation,
tests and total job duration. Include post-job save time and failures rather than
comparing only Cargo's compilation line. Keep original GitHub logs and current-run
verification artifacts with each report.

A local repeat alone does not establish hosted benefit; actual run results belong
with their reports. The uncached baseline is
[PR #6 run 38994338494](https://github.com/myjname/ModuleEngine/actions/runs/38994338494).
This guide does not claim a cache speedup from that baseline or predict a miss/hit
result. Cache transfer may cost more than the work it avoids. No change to the
fail-closed `CI required` aggregate or full acceptance boundaries follows from caching.
