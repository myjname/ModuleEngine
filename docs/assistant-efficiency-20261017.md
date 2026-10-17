# Assistant workflow rollout and Serena pilot — 17 October 2026

[English](assistant-efficiency-20261017.md) | [Русский](ru/assistant-efficiency-20261017.md) | [Documentation](README.md)

The communication, context-handling and debugging changes are implemented in three separate commits. Serena also completed a read-only navigation pilot. Symbol navigation worked after workspace-loading configuration was corrected; overall development speed, token savings and account cost were not measured. No permanent Serena MCP registration was added to Codex.

## Scope and preservation

The starting revision was `1fc13b3d12b1977b4e55c99e0daee754b0dc41bd`. Thirteen pre-existing dirty tracked files, including `game-project-v4.json`, were recorded and backed up separately. Their bytes were checked after each stage and the pilot; they were excluded from the implementation commits. Thirty-three protected files/artifacts and the user's global Codex configuration were also checked against the initial hashes.

This rollout changes assistant instructions, a report-reading helper, a project skill and subagent defaults. It does not change engine production code, Cargo dependencies or authored project data. Python fixtures and navigation checks are not engine, GPU, packaging or published-EXE acceptance.

## Stage 1: understandable communication

Commit `de0b55c` adds the [communication guide](assistant-workflow.md) and concise EN/RU root instructions. Progress explains a new finding and its next useful action. A question supplies the actual product choice or permission needed, a recommended option and consequences; routine implementation decisions stay with the assistant. Completion states changed behavior, checks and material limits.

An independent agent tested response behavior on a trivial edit, a genuinely unresolved export preference, and an interrupted native check. The check exposed an ambiguity about unnecessary edits to an already correct translation; both guides were clarified. Relative links, EN/RU heading structure and preservation checks passed. This tests examples of behavior, not a guarantee about every future answer.

## Stage 2: bounded context and diagnostic evidence

Commit `14b865b` adds [summarize_verification.py](../scripts/summarize_verification.py) and its tests. It summarizes an existing version-1 verification report without running its commands. Default output is bounded to 6000 characters; failure excerpts are optional and bounded. Original evidence remains available. Empty, all-skipped and commandless reports cannot become a successful summary; malformed input returns an input error. See [the full contract and limits](assistant-workflow.md#context-and-diagnostic-budget).

One historical report of 12,163 input bytes produced 737 output characters in approximately 0.12 seconds. This demonstrates useful compression of that report, not a measured token/billing saving or acceptance of current sources. Independent review found empty-report and oversized-timeout cases; both were corrected and rechecked. The stage's 16 tests ran with 15 passes and one Windows symlink-permission skip.

## Stage 3: debugging and adaptive task settings

Commit `9ad4393` adds the repository [moduleengine-debug skill](../.agents/skills/moduleengine-debug/SKILL.md), its [complete Russian manual copy](ru/debugging-skill.md), and [project subagent defaults](../.codex/config.toml). Debugging follows an observed failure, a distinguishing hypothesis/probe, the smallest demonstrated correction, regression checks and an honest statement of remaining uncertainty. General feature design does not trigger the skill.

The skill validator passed. A separate agent investigated deliberately incomplete report fixtures using the real summarizer: independent changes distinguished missing steps, missing command evidence, actual nonzero process exit and invalid schema. It retained the failing input and checked successful and negative controls without patching production code. The skill's scope was clarified to include project verification tools and independently rechecked. This exercise does not establish performance on difficult engine bugs or automatic skill selection in every conversation.

Installed `codex-cli 0.163.0-alpha.10.2` was checked with an isolated app-server: strict `config/read` attributed `gpt-6.1-sol` / `medium` to this repository's `.codex/config.toml`; `skills/list` discovered `moduleengine-debug` with repo scope, enabled, without scan errors. No model turns were started by these probes. App-server startup attempted its own featured-plugin cache request, which failed in the sandbox; the probes were not completely offline.

The assistant is authorized to choose supported settings for delegated work. The [routing policy](assistant-workflow.md#adaptive-task-configuration) uses low effort for narrow lookups, medium for bounded ordinary work, high for independent review or difficult/data-sensitive work, and higher supported levels when justified. This rollout actually used explicit low, medium and high launches. The running parent turn cannot change its assigned settings through that spawn operation. The user's global `ultra` setting was preserved; project defaults are not an automatic switch for every desktop response.

## Serena setup and successful checks

The pilot used Serena commit [`d9e4c796fae0207ba6848d1df4b331122a5567b5`](https://github.com/oraios/serena/tree/d9e4c796fae0207ba6848d1df4b331122a5567b5), installed as `serena-agent 2.0.1.dev0` in a separate Python 3.12.14 environment under ignored task storage. The downloaded source archive SHA-256 was `7d8aafe15e9741c4ccedc26473cfd25a27f379f38c31c3f13135f06e9f5c482f`. The installed archive's commit is established by this pinned download, not a version suffix that may refer to the working directory's Git revision.

The official [rust-analyzer release 2026-10-12](https://github.com/rust-lang/rust-analyzer/releases/tag/2026-10-12) supplied `rust-analyzer 0.3.3081-standalone`. Its Windows ZIP matched the release API's published SHA-256 `77f7a907f0e6f179281bac38e2d08435c974db9a5b99de52c5c963aff52883ec`. No global language-server installation was performed.

Serena home, project metadata, environment and logs stayed under `work/workflow-efficiency-20261017/`. The pilot used the real MCP stdio protocol through a temporary Python client; this does not establish native integration in an already running Codex conversation. GUI and dashboard were disabled. The project was read-only; the actual MCP tool list was checked to contain only `get_symbols_overview`, `find_symbol` and `find_referencing_symbols`. A project-only tool restriction initially still exposed other tool descriptions; setting the allowlist in the isolated global Serena configuration reduced the exposed set to three.

The working Rust configuration included an explicit language-server executable, existing project `CARGO_HOME=.work/cargo-home`, offline Cargo, `linkedProjects: [E:/ModuleEngine/Cargo.toml]` and `cargo.autoreload: true`. `checkOnSave`, Cargo build scripts and proc macros stayed disabled; threads were bounded. Cargo metadata and compiler-introspection processes ran. No engine acceptance suite was run, and caches were not cleared.

The initial configuration returned definitions but empty references, even for positive controls. Using the project cache alone did not fix that. Explicit `linkedProjects` and `cargo.autoreload` were then changed together: their individual causal effects were not isolated. In that configuration, controls returned two `same_path` references and three `load_workflow` references. The final LSP trace reported `health: ok`, `quiescent: true`, and no message.

The successful `GameWorker::spawn` query returned its signature and nine references with containing function names. An independent reviewer compared all nine against fresh lexical matches: files and lines agreed after converting Serena's zero-based locations to one-based source lines. This proves those handwritten references on this snapshot; disabled generated-code support still limits completeness.

## Retrieval measurements and decision

Both approaches received known file paths. The table counts each MCP `content[].text` once and compares it with `rg` stdout; it excludes the duplicated structured result and JSON envelope. It describes successful selected retrievals from different sessions, not one end-to-end run. The `rg` ranges were refined after the initial reads, so this is not a blinded experiment.

| Task | Serena result characters | Refined `rg` characters |
| --- | ---: | ---: |
| Complete shared `GameHost` structure | 1,339 | 1,265 |
| `GameWorker::spawn` signature and nine callers | 3,232 | 3,980 |
| Complete Save As dialog handler | 4,787 | 4,918 |
| Total selected retrievals | 9,358 | 10,163 |

The refined four `rg` commands took about 0.106 seconds of subprocess time, excluding planning and tool overhead. One successful MCP run took about 3.25 seconds to initialize, 8.51 seconds for its first definition request and 5.19 seconds for references; it included diagnostic tracing. An earlier warm definition request took 0.096 seconds. These are individual observations, not stable latency estimates or an end-to-end speed comparison. MCP tool descriptions added 7,263 serialized characters at discovery; their actual treatment by a native client was not measured.

For the Save As adoption follow-up, whole-symbol retrieval produced 7,538 text characters, while two targeted `rg` excerpts produced 1,064. Symbol tools do not automatically minimize context: retrieve a whole method only when the question requires it. Neither navigation route executed saving or proved transaction correctness.

**Decision:** selective semantic navigation is supported by evidence, particularly caller attribution and symbol overviews. Retain bounded `rg`/source reads for literals and small known regions. Do not register Serena as an always-on project MCP service on the strength of this pilot alone: it has setup/startup costs and no demonstrated overall speed or account-cost advantage here. Ambiguous names, aliases, generated code, a larger unfamiliar feature and long-lived-session amortization remain untested. If it is integrated later, retain the working workspace configuration, narrow tools, positive controls and a fallback; a successful startup or empty result alone is insufficient evidence.

## Evidence and final limits

Local evidence is retained under `work/workflow-efficiency-20261017/`: `baseline.json`, preservation checks, `config-probe/`, `debug-forward/`, `navigation-baseline/`, `pilot-summary.json`, per-attempt MCP results/logs, downloads and the installation manifest. These ignored files are not published repository dependencies. Full final review bindings belong in `outputs/reviews/workflow-efficiency-20261017/`.

The combined local Python suite ran 52 tests: 50 passed and two symlink tests were skipped because Windows did not permit creating the required links. The skipped behavior remains unverified locally. Independent review and GitHub CI must be assessed for their actual final source snapshot; historical results and a source digest comparison do not replace them.
