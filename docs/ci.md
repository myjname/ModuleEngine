<a id="ci-moduleengine"></a>

# ModuleEngine CI

[English](ci.md) | [Русский](ru/ci.md) | [Documentation](README.md)

Goal: every PR receives `contracts` on Windows and Linux; changes to executable
sources add native and package checks. Documentation PRs do not receive
expensive native/package builds. Non-goals: performance, desktop dialogs, hardware GPU,
and full Full/Team/Player delivery. Criterion: `CI required` succeeds only after
successful planning and both platforms, including selected additional routes.

<a id="что-выполняется"></a>

## What Runs

The [workflow](../.github/workflows/ci.yml) runs for all PRs, pushes to `main`, the merge queue,
and manual invocation; there are no workflow-level path filters. [ci_plan.py](../scripts/ci_plan.py)
reads the Git diff, including deleted files and both sides of renames. Only changes
to `docs/`, Markdown, LICENSE, and `.gitignore` skip additional routes;
an unknown new source path enables them. An initial push/manual run enables everything.
Failure to determine the diff fails planning.

| Runner | Always | When sources change |
| --- | --- | --- |
| `windows-2022` | Python unit checks, `contracts` | `native-tools`, then `package` |
| `ubuntu-24.04` | Python unit checks, `contracts` | `native` on explicitly selected Mesa lavapipe, then `package` |

`native-tools` comprises three existing checks of native linker/cache/executable
spellings without GPU or UI. This is part of `native`, not a replacement for its two GameUI tests.
Linux full `native` runs offscreen tests with software Vulkan, without a desktop
session. Windows GPU native acceptance remains a targeted local route.
Successful CI does not confirm hardware Linux, strict 60 FPS, cold-start, or
interactive desktop acceptance. `package` uses minimal Cargo fixtures,
not a real game export.

The two platforms run in parallel on separate hosted VMs; commands within
each VM run sequentially. Each job receives a fresh hosted checkout
and separate caches/TEMP; the developer's authored working directory is not used.
Rust version/native target come from the existing Windows/Linux manifests.
Before the offline launcher, CI explicitly installs pinned Rust/rustfmt and runs
`cargo fetch --locked`. Linux explicitly installs system link libraries;
the native route additionally installs Mesa and restores DXC using the
existing archive SHA and all file SHAs from `scripts/linux-toolchain.json`.
An installation, fetch, compilation, or test failure fails CI, without a fallback version.

The launcher uses a short checkout-local `.work/v/<run-token>` and records
its resolved path in the report. The first Windows hosted run found LNK1181 in a nested
snapshot fixture with a long TEMP; the test and its criteria were preserved. Cargo hides the exact name
of the problematic object, so path length is a testable hypothesis,
and confirming the fix requires a new successful `package` on hosted Windows.

Reports and command logs from `outputs/verification` are published as an artifact for
each platform even after failure; if verification did not start, the GitHub job log
contains the preparation error. Retention is 14 days. Full source/artifact hashes and
scope remain in the launcher's normal `result.json`. CI caches have not yet been added:
first, the real hosted run must be measured without carrying over local tool paths.

<a id="включение-обязательности"></a>

## Making CI Required

The workflow file itself does not change GitHub branch protection. After the
workflow appears in the target GitHub repository and the first successful PR/push run:

1. Enable a PR requirement before merging in the ruleset/protected branch.
2. Add the required status check **`CI required`**, source GitHub Actions.
3. Check a test failure in a separate PR: a failed prerequisite must block
   `CI required`. A new PR version must pass again; the old SHA is insufficient.

The aggregate with `if: always()` explicitly rejects `failure`, `cancelled`, `skipped`, and
missing route outputs. Individual conditional native/package steps are not declared
passed when their route is not selected. If branch protection/ruleset has not yet been
applied, CI provides a signal but does not technically prohibit merging. Enabling the rule
requires a known repository and appropriate permissions; local unit checks
do not prove execution of GitHub Actions.

The workflow token has only `contents: read`; checkout does not persist credentials.
There are no secrets, `pull_request_target`, privileged PR execution, or
unprepared self-hosted jobs. All external actions are pinned to full commit SHAs.
Fresh CI tools are downloaded only inside an explicitly started hosted job.

For [myjname/ModuleEngine](https://github.com/myjname/ModuleEngine), public
access is enabled by the owner's decision. `main` has required PRs, an up-to-date branch,
**`CI required` with GitHub Actions app ID 15368**, administrator protection, and resolution
of review discussions; force pushes and branch deletion are prohibited. Required human approvals = 0:
separate independent AI review is not presented as approval from another GitHub account.
In [service PR #2](https://github.com/myjname/ModuleEngine/pull/2), an invalid base SHA
caused planning failure, a skipped matrix, and failed aggregate; GitHub showed
`mergeable_state=blocked`. The PR was closed without merging. This proves prerequisite
failure, not a separate artificial Rust-test failure. Actual results
for each version must be checked in its [CI run](https://github.com/myjname/ModuleEngine/actions).

<a id="локальная-проверка-политики"></a>

## Local Policy Verification

```sh
python -B -m unittest discover -s scripts -p 'test_*.py' -v
python scripts/ci_plan.py --base <full-base-SHA>
python scripts/verify.py --profile native-tools --plan
```

Policy fixtures check path selection, safe Git argv, and aggregate rejection on
error/skip/cancel. This is not hosted CI acceptance. A changed workflow must be checked
in GitHub, including fresh dependency restoration on both systems.

References: [required checks and skipped jobs](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks),
[minimal permissions and SHA pins](https://docs.github.com/en/actions/reference/security/secure-use),
[hosted runner images](https://github.com/actions/runner-images).
