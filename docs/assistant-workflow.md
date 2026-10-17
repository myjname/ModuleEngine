# Working with the ModuleEngine assistant

[English](assistant-workflow.md) | [Русский](ru/assistant-workflow.md) | [Documentation](README.md)

This guide describes communication and task handling inside the existing Codex workflow. It supplements [AGENTS.md](../AGENTS.md), [verification routes](verification.md), and [independent review](review.md); their data-preservation and acceptance requirements still apply.

## Clear progress and completion

Start with the intended result and the first useful action. During work, report a new finding, its implication, and the next action that resolves uncertainty. Mention a user decision or blocker only when one exists. A short task does not need a ceremonial plan or a fixed report template.

For example: “Save As changes the draft before the picker returns. I will move acceptance after successful publication and check Cancel and write failure.” This explains why the work matters without listing every file read or command.

Distinguish observed facts, hypotheses, and unverified behavior. A running check is pending; a skipped check is not passed. In the final response, state what behavior changed, how it was checked, and any material remaining limit. Include a user action only when it is actually necessary. Keep commands, paths, and implementation details when they help the user assess or reproduce the result.

## Useful questions

Use repository evidence and earlier answers before asking. The assistant chooses routine implementation details within the authorized task. Ask when a missing product preference changes the result, a constraint cannot be inferred, or a specific action needs authorization.

A useful question supplies the concrete context, a recommended option and why, and the consequence of the alternatives. Use the product's language instead of asking the user to choose an internal algorithm. Separate preference questions from approval requests. Explain the actual source of a required approval; do not invent one from hypothetical risk.

For a hypothetical new export option, ask: “Should this export include the source assets so it can be edited later? I recommend including them for an editable delivery; leaving them out reduces package size but prevents that editing workflow.” The implementation of copying or indexing those assets remains the assistant's responsibility. Do not ask this example if an existing Full/Team/Player contract already settles the choice.

Bundle independent questions when that avoids repeated interruptions. Keep a question pending when its answer is required; elapsed time is not approval. Continue independent work while waiting. Do not repeat a decision already made in the same task.

## Scope of this guidance

Communication quality is assessed by whether a user can identify the result, the next action, and any decision they need to make. Matching particular wording is not a behavioral test. These rules do not promise fewer tokens or faster delivery; those outcomes need evidence from actual tasks.
