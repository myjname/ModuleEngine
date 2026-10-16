# Review: <task>

[English](review-template.md) | [Русский](ru/review-template.md) | [Documentation](README.md)

Copy to `outputs/reviews/<task-id>/review.md`. Fill in the result with facts;
the template and a successful snapshot comparison are not independent review.

<a id="задача-и-границы--автор"></a>

## Task and scope — author

- Goal and expected behavior:
- Non-goals:
- Acceptance criteria and how to verify them:
- Contracts/documents:
- Initial user changes and where they are preserved; which files belong to the task:
- Diff/base and new files to inspect:

<a id="проверяемая-версия--автор-сверяет-reviewer"></a>

## Version under review — author, verified by reviewer

- `source.json`, HEAD, `source_digest`:
- Additional ignored/external inputs and artifact SHA, if needed:
- Checks: exact commands, environment/target/features, exit/result, links to logs:
- Unverified areas, omissions and known limitations:

<a id="независимое-заключение--заполняет-reviewer"></a>

## Independent conclusion — completed by reviewer

- Reviewer, their role and independence from the implementation author; clean agent context:
- Verified `source_digest`, files and contracts actually inspected:
- Reviewer's own runs, separately from the author's logs they read:
- Confirmed findings: ID, priority, file/line, trigger, consequence,
  violated criterion and reproduction. If there are none, explicitly state “no confirmed findings
  were found within the reviewed scope”; this does not prove that all errors are absent.
- Assumptions/remaining risks and gaps:
- Conclusion: what can be accepted and what remains open. If a reviewer is unavailable,
  explicitly state “has not undergone independent review” with the reason.

<a id="ответ-и-повторная-приёмка--автор-и-reviewer"></a>

## Response and repeat acceptance — author and reviewer

- For each finding: the author's fix or verifiable objection, repeat-check
  result, reviewer's decision; preserve unresolved disagreements.
- When sources change: new snapshot/digest, additional diff and reviewer
  reinspection. If there are no changes, specify the original snapshot.
- Final snapshot comparison: command/result, date; actual acceptance results
  and limitations. Do not replace them with `review_passed=true` or a CI checkmark.
