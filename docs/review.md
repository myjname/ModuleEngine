<a id="независимое-ревью-изменений"></a>

# Independent Change Review

[English](review.md) | [Русский](ru/review.md) | [Documentation](README.md)

A substantial change goes through a separate reviewer before completion. This is another
agent or person who did not write the implementation being reviewed; the author's own second
pass does not count as independent. A new agent receives the task with a clean
context, the goal, criteria, contract paths, initial diff, and evidence.
Providing the conclusion “everything is correct” instead of these materials is not allowed.

<a id="когда-и-что-проверять"></a>

## When and What to Review

Review is mandatory for changes to saving/loading/history/Assets, authored data
and schemas, the Play/Custom lifecycle, Full/Team/Player delivery, executable selection,
CI/verification routes, and the acceptance criteria themselves. Other changes need it
when several contracts are affected or there is a substantial regression risk. A minor
documentation typo does not require an independent reviewer.

Before work, the author briefly records the goal, non-goals, criteria, and existing user
changes. Before review, they complete the [template](review-template.md) in a new
`outputs/reviews/<task-id>/` and save a source snapshot. The reviewer independently
reads AGENTS, affected contracts, the complete task-related diff, and new files,
traces actual execution paths, and assesses the checks. Existing user
changes must be visible and preserved; they must not be attributed to the current implementation.

Review covers alignment with the goal and architectural boundaries, error/Cancel scenarios,
data preservation, suitability of fixtures and assertions, zero/missing tests,
the relationship between logs and reviewed sources, and explicitly remaining gaps. Passing CI
alone does not establish correctness. A test that repeats the implementation algorithm,
or a successful bypass of a criterion, is not evidence of the property.

The reviewer does not change the implementation or share an active native/GPU/Cargo
run with the author. They may perform a separate targeted check after coordinating the queue
within the current task. The report distinguishes logs read from the author from the reviewer's own
runs. This process does not require connecting an external AI service, writing to other chats, or publishing
a review on GitHub.

<a id="замечания-и-повторная-проверка"></a>

## Findings and Rechecks

Each confirmed finding contains the file/line, triggering condition,
observed consequence, and violated criterion; where possible, a reproduction.
An assumption is marked separately. The author fixes the finding or supplies a verifiable
objection. After a fix, they rerun the relevant checks, and the reviewer
checks the fix and newly affected paths. An unresolved disagreement remains
open; the author cannot close it with their own “accepted” mark.

The final report identifies the reviewer, reviewed snapshot, resolved and open
findings, actual checks, and limitations. Without an independent reviewer,
the result explicitly has the status “has not passed independent review,” with a reason; do not declare
the task fully accepted. New implementation/fixture/criterion changes make the
previous review stale: a new snapshot and a re-examination of the changed
scope by the same or another independent reviewer are needed. A new commit with the same files also
requires rebinding evidence to HEAD. Records only in `outputs/` do not change source.

<a id="привязка-к-исходникам"></a>

## Binding to Sources

The stdlib helper [review_snapshot.py](../scripts/review_snapshot.py) requires Python
3.9+ and Git. It only saves/compares bytes, **does not perform a review or approve
a change**. Capture runs after the last edits, before checks pertaining to that
snapshot; files are then not edited in parallel with review.

```powershell
& $python scripts/review_snapshot.py capture --output outputs/reviews/<task-id>/source.json
# Checks and independent review of this source snapshot.
& $python scripts/review_snapshot.py check outputs/reviews/<task-id>/source.json
```

Linux uses the same CLI with `python3`. Check returns 0 only when the
snapshot matches, 1 on a change, and 2 on an error; none of these codes is a
reviewer decision. Capture does not overwrite an existing file. Before final handoff, the author
repeats check, and the reviewer compares the source digest in their record with the snapshot.

The snapshot contains HEAD and SHA-256 of every tracked file, including current dirty
user data, plus untracked files outside Git ignore and generated `work/`
and `outputs/`. This includes sources, tests, workflows,
and docs not yet added to Git. Tracked files in those directories are still included. A deleted tracked file
is recorded as missing; a symlink is recorded by its link text, without reading external content.
The digest is calculated from the sorted inventory and HEAD; capture time has no effect.

Ignored/external sources, linked assets, toolchains, and executable artifacts are not
covered by this inventory: required input/artifact SHA, target/features, and environment
are added explicitly to evidence. A snapshot is not a sandbox, signature, or protection against
deliberate report substitution. The helper does not check access/execution attributes or
submodule contents; an unsupported Git item produces an error. New user work
after capture also changes the snapshot: preserve it and update the binding; do not reset.

Helper unit checks, without Cargo/GPU:

```powershell
& $python -B -m unittest discover -s scripts -p test_review_snapshot.py -v
```

Fixtures live in `work/review-snapshot-tests`. They include a positive comparison and rejections
for changing a tracked/untracked file, adding/deleting files, a different HEAD, a damaged
snapshot, and a Git error. This checks evidence binding, not the quality of engine review.
