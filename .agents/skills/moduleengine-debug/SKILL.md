---
name: moduleengine-debug
description: Diagnose and fix observed ModuleEngine bugs, regressions, crashes, or failing contracts using a minimal reproduction and evidence-driven probes. Use for existing behavior, not feature design, general explanations, or unrelated environment setup.
---

# Debug ModuleEngine behavior

[English](SKILL.md) | [Русский](../../../docs/ru/debugging-skill.md) | [Documentation](../../../docs/README.md)

Use this procedure for an observed failure in the existing engine, editor, Player or project verification tools. Follow [project preservation and review rules](../../../AGENTS.md) and select checks from [verification routes](../../../docs/verification.md). It does not authorize unrelated changes or external actions.

## Establish the failure

Record expected versus observed behavior, the smallest reliable reproduction, relevant source state and evidence. Inspect the affected contract and actual production path. Protect the initial diff and authored project; use a separate fixture for destructive operations. If the failure is not reproducible, identify the missing observation or environmental condition before proposing a product fix. A loader, fixture, or toolchain failure is not by itself an engine defect.

## Test a hypothesis

Keep the current hypothesis and a predicted observation in the existing task notes. Choose the smallest probe that distinguishes it from plausible alternatives: a targeted regression, exact input/output comparison, call-site inspection, or bounded diagnostic capture. Separate facts from assumptions and check the real ownership, source snapshot, target/features and artifact when they matter.

After each observation, update or discard the hypothesis. When repeated edits provide no new distinguishing evidence, stop editing and improve the reproduction or instrumentation; ask only for genuinely unavailable user observations or product decisions. More reasoning effort cannot replace a missing measurement. Put temporary probes in `work/`, retain full evidence, and summarize logs with `scripts/summarize_verification.py` when they are existing verification reports. Read further excerpts only for a specific unresolved question.

## Fix and verify

For Windows AppContainer fixtures, TEMP can permit file creation but reject rename
or junction operations. Before changing atomic-publication code, compare a tiny
write/rename probe in that TEMP with the same probe in a new owned `work/` directory.
If the filesystem probe reproduces the failure, retain the failed evidence and
move only the disposable fixture to `work/`; do not redirect production user data
or weaken its publication guarantees. A passing workspace fixture does not prove
that an arbitrary user-selected destination is writable.

When UI callbacks are fast but distinct game frames are slow, compare callback,
producer-completion and UI-observed frame intervals, then inspect presentation
origins and the raw/pose/content/late-decoration order. A pose added only after
eligibility cannot produce interpolated samples. Check the boundary with a
neutral renderer-backed Host regression and actual native frame provenance;
never make time, alpha or serial alone sufficient for a new frame. Keep a Menu
probe or scripted workload distinct from required foreground/gameplay evidence.

For slowed or distorted native audio, compare production source duration and completed buffers with monotonic wall time after warmup. Nonzero completions alone can hide sustained starvation. Compare offline decoded/mixed PCM with native feeding before changing the soundtrack or sample rate. Verify queue changes with the same cadence probe, actual game load and listening; a passing producer clock does not measure device pitch or physical sound quality. Keep any buffer horizon justified by that endpoint's evidence rather than treating it as a universal constant.

Change the smallest demonstrated cause. Where useful, add a regression whose expectation is the contract or original data, not a second implementation of the same algorithm. Verify that it detects the failure, that the corrected path passes, and that relevant neighboring behavior and positive controls still work. Do not repeatedly rebuild the entire workspace without a new reason.

Check the selected profile's plan first. Coordinate native/GPU/timing work and release only processes/files owned by the probe. Preserve failed candidates and last-good behavior according to the relevant contract. Send substantial fixes to the independent reviewer required by AGENTS, and repeat affected checks and review after corrections.

## Report the outcome

Explain the demonstrated cause, changed behavior, actual verification and remaining uncertainty in the user's language. An interrupted or skipped check remains unverified; a historical report does not certify today's source. If no cause is established, report what was ruled out and the next useful observation instead of presenting a speculative patch as a fix.
