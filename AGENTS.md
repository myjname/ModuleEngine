<a id="работа-над-moduleengine"></a>

# Working on ModuleEngine

[English](AGENTS.md) | [Русский](docs/ru/agent-guide.md) | [Documentation](docs/README.md)

<a id="цели-и-точка-входа"></a>

## Goals and entry point

Start with the [current status](docs/current-status.md), then read the documents covering the affected contract. The [architecture](docs/architecture.md) defines boundaries; [decisions](docs/decisions.md) separate accepted scope from assumptions. A historical report provides evidence only for its stated snapshot.

ModuleEngine is a modular Rust engine and native editor. Core owns shared contracts and ECS; game rules and specific examples belong to separate packages. Engine defines the architecture and the changes permitted in Project. A separate Player runs the selected game without the editor or compilers.

[Open questions](docs/questions.md) list future decisions; they do not authorize adding them to every task. Do not add a subsystem, migration, dependency, or user-visible behavior change unrelated to the current goal. For a substantial change, briefly record the expected behavior, non-goals, and acceptance criterion; for a small fix, use its reproduction case.

<a id="рабочие-данные"></a>

## Working data

- Continue in the existing checkout. Do not create development copies, worktrees, or clones without a corresponding request. Full/Team/Player acceptance packages are verification deliveries.
- Establish the initial diff before editing. Preserve unrelated user changes, authored JSON, assets, layout, Recent, Engine bindings, and SDK metadata. Use separate fixtures for destructive scenarios; do not replace the authored project with them.
- Store intermediate material in `work/`, and completed builds and reports in `outputs/`. Do not clear shared caches or replace a published EXE to prepare a check.

<a id="инварианты-изменений"></a>

## Change invariants

- Keep the authored document separate from the Play copy. Rejecting a candidate preserves the last good version; Cancel and publication errors preserve the previous authored data and state. Account explicitly for permitted recovery files.
- Preserve exact JSON keys, unknown payloads, null, stable IDs, and other namespaces. A no-op Apply must not create local overrides or break prefab inheritance.
- Distinguish authored values from effective defaults, ComponentSchema from PropertySchema, and unknown live IDs from a known empty set. Use shared Core ownership and availability rules; respect exact owner/schema/SDK pins. See [schemas](docs/component-schemas.md) and the [asset API](docs/asset-workspace-api.md).
- Do not bypass Full/Team restrictions or source checks to make a test pass. Selected native features, the working graph, and Player assets must match the project; an absent optional module must not activate implicitly.
- Reusing compiled code does not imply shared state: Custom instances have independent state and workers. Release only processes and files owned by the operation. See the [Custom store](docs/custom-program-store.md).

<a id="проверки-и-результат"></a>

## Verification and delivery

Choose checks by the affected contracts: UI, persistence, runtime, and export may depend on the same schema. The [verification registry and routes](docs/verification.md) describe the explicit `quick`, `contracts`, `native`, `native-tools`, and `package` profiles, and the preserved no-argument legacy `verify.ps1`/`verify.sh` calls. Inspect `--plan` first; a profile's scope is not full acceptance. Run native/GPU/timing checks sequentially, without competing builds. Do not repeat the full suite after every small step without a new reason.

Before completing substantial changes, send them to an [independent reviewer](docs/review.md): an agent with a clean context or a person who did not write the implementation. The reviewer checks the goal, diff, new files, and evidence; the author fixes findings and repeats checks, and the reviewer verifies the fix. Bind the conclusion to HEAD and a source digest covering tracked and untracked files; after source changes, review the changed scope again. The author's own review or a successful snapshot check does not replace an independent conclusion. If a reviewer is unavailable, explicitly state that independent review was not performed and do not claim full acceptance. Details and the template are in the linked contract.

Distinguish product defects from fixture or environment failures. Tie results to the source snapshot, commands, target/features, and actual artifact; identify failed and unverified checks. Offscreen, prewarmed, or small Puzzle results do not establish cold-start behavior, hardware Linux support, or the ARPG 60 FPS goal. When behavior changes, update the relevant contract and current status; do not copy the entire history into these instructions.

## Context and evidence

Load the current status and affected contracts, then narrow source searches by path and symbol. Open generated `outputs/`, `work/`, historical reports, and full logs only for a specific question. Bound tool output; state truncation and fetch the relevant continuation instead of silently treating an excerpt as complete.

Summarize existing verification reports with `python scripts/summarize_verification.py <report-directory>`; add `--excerpts` for bounded failed-log context. Keep original logs as evidence. For a long task, keep a compact checkpoint in its existing notes: goal, decisions, current hypothesis, source state, checks and next action. Revalidate it against the current diff before resuming; do not create a second project history. See [context guidance](docs/assistant-workflow.md#context-and-diagnostic-budget).

## Communication

Lead with the outcome in the user's language. Progress updates explain a new finding and the next useful action; identify a blocker or user decision when one exists. Summarize routine commands rather than narrating them.

Ask only for missing decisions that materially affect the result or required authorization. Explain the concrete choice, recommend an option with its consequence, and distinguish a product decision from permission. Use existing answers and project contracts; resolve routine implementation details yourself. The final response states the changed behavior, verification and material limits, plus any necessary user action. See [examples](docs/assistant-workflow.md).

## Debugging and task configuration

For observed bugs or regressions, use the project [moduleengine-debug skill](.agents/skills/moduleengine-debug/SKILL.md). Load it for the failure being investigated, not routine explanations or every edit.

The user authorizes choosing supported model/effort settings for delegated tasks. Prefer `low` for narrow read-only scans, `medium` for bounded ordinary work, and `high` for independent review or complex/data-sensitive debugging; use higher levels when the difficulty justifies them. Honor explicit user choices, avoid delegation overhead for trivial commands, and do not lower acceptance criteria. [Project defaults](.codex/config.toml) apply to new delegated work; explicit launch settings take precedence. They do not switch an already running parent turn or change the user's global `ultra` preference. See [routing and limits](docs/assistant-workflow.md#adaptive-task-configuration).

## Documentation languages

English is the primary language of public documentation: root and crate `README.md` files, `AGENTS.md`, and `docs/*.md`. Maintain complete Russian counterparts in root and crate `README.ru.md` files and `docs/ru/*.md`; the Russian copy of this guide is `docs/ru/agent-guide.md`. Keep language links visible on both versions.

Review both language versions in the same change when adding or changing documentation; update each as needed to keep their full meaning aligned. Do not make empty edits to an already correct translation. Translate the full meaning, including limitations and evidence, without turning the counterpart into a summary. Keep commands, code, file paths, API and JSON identifiers, exact diagnostic/UI literals, and historical evidence accurate; explain technical literals in the surrounding language. Verify relative links and heading anchors in both versions. Conversation language follows the user's preference.
