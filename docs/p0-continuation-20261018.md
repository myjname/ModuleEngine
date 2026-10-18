# P0 continuation: GPU presentation evidence — 18 October 2026

[English](p0-continuation-20261018.md) | [Русский](ru/p0-continuation-20261018.md) | [Documentation](README.md)

**The first eligible restored-normal Windows cold attempt and the current scoped Windows ARPG, Windows/Linux offscreen and Windows authoring routes pass. The clean Linux plain-window close and synthetic release-failure pair also pass within the scope below; earlier failed fixture receipts remain preserved. This is not full P0 acceptance.** This continuation updates the evidence after the [original GPU report](gpu-presentation-20261018.md); its failed runs and the historical [17 October P0 report](p0-stabilization-20261017.md) retain their original status.

## Source and artifact binding

HEAD is `cbe97de7a974d08e391b4e3b21a18656d08ff6f7`. Windows normal source is `cedf173e70f531c8f1784a14ec02830a4aea905f7ff0ebd3aea5a0a07484a6cb`; [inventory](../outputs/verification/p0-continuation-20261018/normal-source-01.json), [Editor receipt](../outputs/verification/p0-continuation-20261018/normal-editor-receipt-01.json). Normal release Editor SHA256 is `d061c7f69b0b1df38b6564875e9a92c4b1deec56229c5c7f9a83a0ef7932548e`.

[Restoration receipt](../outputs/verification/p0-continuation-20261018/probe-restoration-01.json) records restoration of the exact original runtime and removal of the temporary diagnostic module. Production admission default remains 2; no scheduling optimization or timeout repair was retained. After restoration, only the adapter test example changed: production adapter rejection/preservation is exercised with wrong Windows LUID or Linux device/driver UUIDs. Other source15 native inputs are unchanged. ME-01 documentation reconciliation has its [separate receipt](../outputs/verification/p0-continuation-20261018/me01-receipt.json).

Linux inventory `6cb94aa78ff59df58ce2ee234169d6094b26799e97deb229c1293edff3453f10` contains the same 162 native inputs, with only an additional unrelated ignored configuration entry. The [inventory binding](../outputs/verification/p0-continuation-20261018/linux-normal-inventory-binding-01.json) distinguishes inventory-policy variation from product changes. Linux remains Ubuntu/WSL llvmpipe software Vulkan, not hardware Linux certification.

## First eligible normal target-cold attempt

[Cold normal01](../outputs/verification/p0-continuation-20261018/cold-normal-01/launch.json) passes unchanged ≤45 s process-wall and internal clocks: **40.906 / 39.9128549 s**. This is the first eligible normal attempt after restoration: new process, fresh actual Full export and previously absent `work/ctN01`, existing tools/offline registry, uncontrolled OS caches. It is not a warm substitute or guaranteed startup bound.

The [native report](../outputs/verification/p0-continuation-20261018/cold-normal-01/report.json) completes 90+90 measured frames and 30+30 warmups, actual graph pans and Graph/Scene/Game readiness. Play/Stop releases the original owner, creates actor 2 after actor 1 and reaches stopped readiness at time zero. Both clocks include this final readiness. Source/protected/Full checks pass, preserving all 33 protected hashes; the [independent cold check](../outputs/verification/p0-continuation-20261018/cold-normal-01-independent-check.json) confirms this exact case. Injected pointer movements and command dispatch are scripted native UI evidence, not physical input or scanout certification.

Diagnostic baseline cold-trace01 passed 41.343/40.3415648 s; temporary limit-1 control passed 40.313/39.3508023 s. These are diagnostic builds, not shipping acceptance. The control removes a redundant initial preparation attempt but a single pair with differing compilation/cache/scheduling times cannot assign its full wall delta to admission. The final Stop path still performs an independent warm preparation. Earlier cold01 os5 and final-Stop watchdog failures remain failed; their cause is unknown. This work localizes successful preparation and records a normal successful attempt; it does not establish a repair or GPU causation.

## GPU transport correctness

[Windows fixture](../outputs/verification/p0-continuation-20261018/windows-gpu-fixture-01/result.json) passes 22 cases/16 exact-image comparisons on RTX 4080 SUPER/DX12. [Linux fixture](../outputs/verification/p0-continuation-20261018/linux-gpu-fixture-01/result.json) passes 23/16 on WSL llvmpipe/Vulkan, including separate wrong-device-UUID and wrong-driver-UUID rejection. Normal CPU readback/upload are zero; explicit comparison readbacks are diagnostic. Windows fixture SHA256 is `0c879d73721ea5af4884b43c3f33d6f0beb1adb2e706bd3298895ff7c712c211`; Linux ELF is `7554cf26fa2dc384257ae98ea7324d782935c5f9b6d0e97a179c1a6eef704c2e`.

These production-bridge tests are offscreen: no surface, eframe paint, vsync or compositor. They do not certify window cadence, physical scanout, arbitrary drivers/ABIs or the exceptional Linux ownership-release failure path. Current ordinary Linux Editor also builds successfully; compilation is not window acceptance.

## Current Windows ARPG and standalone delivery

[Current ARPG relocation](../outputs/verification/p0-continuation-20261018/arpg-relocation-01/result.json) passes all five actual commands: normal Editor benchmark, supported selected export, relocated package verification, headless check and standalone native benchmark. The unchanged 1080p/20-living-enemy/Playing criterion passes: Editor **119.05020347 FPS / worker p95 9.4335 ms**, Player **119.002023 FPS / worker p95 9.2239 ms**, with zero frame/wait errors. These are separate presentation and worker series; simulation remains fixed at 60 Hz.

The relocated 18-file Player has exact features `arpg,audio,ui,vulkan`, SHA256 `9887f1587da193eb9eca5dce935239e08f588b177a6435b8a4239b3cbc8eda70`, and manifest SHA256 `0649cd88b458a73bb58bfdf8aa75dcae2fc524fd933b57ee43702f6353fa2807`. Tools-free launch and preservation pass. This source-bound run is not a new A/B distribution, universal performance gain or clean-OS certification; prior source15 A/B evidence stays separately bound.

## Scoped Windows authoring route

[UI action receipt](../outputs/verification/p0-continuation-20261018/authoring-ui-01/ui-actions.json) and four actual JPEG captures record New Empty → add object → Scene X-gizmo drag → Save → native Open → Play/Stop → Windows Player export. Editor exits normally 0 with source/protected/Full/EXE preservation. Saved object ID1 has translation `[3.102818,0,0]`; authored SHA256 is `27acc01e0943e717102425cd3d11875f71f6f22b48a4abd6b41f67997165151a`.

The [relocated authored Player](../outputs/verification/p0-continuation-20261018/authoring-player-02/result.json) passes actual `--verify-package` and `--benchmark`, 120 distinct frames, advancing simulation and no frame/wait errors with tools removed from the launch environment. Its 10-file `ui,vulkan` package has Player SHA256 `77aad97859f90145d6a484431ae86676e5ff4a5c99b2ac9b3022676fdd5a642c`. Complete JSON-type-aware comparison proves semantic equality despite serialization byte differences. The receipt is author-transcribed from agent Computer Use; the independent reviewer examined captures/artifacts and did not repeat every input. Numeric text editing and initial tool input-geometry misses are unverified, not silently accepted or diagnosed as product defects. Earlier unsupported checker flag/byte-equality fixtures remain failed fixture evidence.

## Linux window route and remaining evidence

[Window01](../outputs/verification/p0-continuation-20261018/me04-linux-window-01/result.json) passes fresh Full export/capabilities, audio-free Puzzle and actual resized Editor benchmark. Its plain WM_DELETE attempt and the subsequent capture time out because the generated project displays an **Unsaved Changes** migration guard. The actual capture establishes the modal; source inspection explains adoption of default asset metadata from `asset_manifest=null`. This is not evidence of a GPU hang. The capture v3 cleanup receipt remains unconfirmed despite later PID absence; no later success relabels it.

The [clean plain-close case](../outputs/verification/p0-continuation-20261018/me04-clean-plain-close-01/result.json) changes only the owned generated fixture's `asset_manifest` from null to its canonical default, preserving the original, identical binding and absence of an assets directory. Actual current Editor receives WM_DELETE and exits0 in **5.401733667 s**, its window disappears and its owned process group is absent. All source/protected/Full/Puzzle/clean-fixture/ELF/helper checks pass; retirement counters are0/0/0/0/0. The [independent window review](../outputs/reviews/p0-continuation-20261018/me04-window-review.md) accepts this case. One recorded X11 BadWindow (error3/request3) comes from the post-exit query of the remembered window ID, not a zero-X11-error result. This is an X11 window-close functional case without clicks or Computer Use, not a GPU workload, hardware/Wayland/cold/performance acceptance or bypass of an unsaved-work guard.

Native Linux WAV/dialog services, hardware/ABI portability and physical foreground/scanout certification remain outside this evidence. Formatting and selected-example Clippy success do not replace runtime acceptance. [Independent scoped review](../outputs/reviews/p0-continuation-20261018/newnormal-review.md) distinguishes own checks from author-supplied execution evidence. Final continuation review/source binding follows the completed evidence and frozen documentation; no full-workspace rerun or full P0 closure is claimed.

## Linux synthetic release-failure pair — actual PASS

[Operation06](../outputs/verification/p0-continuation-20261018/linux-fault-operation-06.json) and [pair06](../outputs/verification/p0-continuation-20261018/linux-fault-pair-06/result.json) pass on the separately bound Linux source6cb. The fixture driver invokes the actual production bridge; an explicit Vulkan layer intercepts the matching release submission and returns pre-submit `ERROR_DEVICE_LOST` (-4) without forwarding it. Each normal/fault run has exactly one matching release submit and `invalid=false`. Both actual processes exit0 and prove their owned process groups absent. This is a synthetic API-failure experiment on WSL llvmpipe, not real hardware device loss.

| Observation | Normal | Synthetic fault |
| --- | --- | --- |
| Registered/joined/completed/unconfirmed/failed retirement |1/1/1/0/0|1/1/0/1/1|
| Weak lease after join/owner drop |dead|live|
| Usable slots after releasing other frames |4|3|
| Target destroys observed |5|0|
| Normal CPU readback/upload and diagnostic readback bytes |0/0/0|0/0/0|

The fault driver deliberately observes production retirement `ok=false` and `GPU presentation ownership release failed; source lease remains pinned`; fixture PASS means the expected fail-safe/error/quarantine was verified. It does not mean rendering succeeded, the error was repaired, or real device loss is certified. Initial four-slot/backpressure behavior is exercised; the fault keeps the affected lease/device pinned through normal owner drop rather than reusing the slot. Source/protected/control after-checks pass. [Independent actual-fault review](../outputs/reviews/p0-continuation-20261018/me04-fault-review.md) accepts this bounded experiment; the final decision is recorded separately. Earlier header/dependency-pin/offline-metadata setup failures remain preserved fixture/environment failures; the final pair does not relabel them.

The clean ordinary close result above completes this scoped functional route. Shared-v4 pure leader-Z ownership evidence validates the helper boundary separately. Earlier header/dependency-pin/offline-metadata setup failures remain fixture/environment failures, with their original receipts intact.

## Final binding and review

The [final external source inventory](../outputs/verification/p0-continuation-20261018/final-source-02.json) and [independent final decision](../outputs/reviews/p0-continuation-20261018/final-review.md) bind the completed documentation snapshot. At authoring time final review is pending; those records must supply the final conclusion. Native results retain their build/source identities; later documentation does not relabel binaries or constitute a full-workspace rerun. No full P0, hardware Linux, real-device-loss, universal performance or physical scanout certification is claimed.
