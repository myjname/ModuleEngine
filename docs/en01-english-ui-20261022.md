# EN-01: English engine-owned interface and messages

[English](en01-english-ui-20261022.md) | [Русский](ru/en01-english-ui-20261022.md) | [Documentation](README.md)

Status, 22 October 2026: the engine-owned Editor/Host/renderer messages are English, with public English documentation and complete Russian counterparts. Implementation and applicable checks pass within the scope below, including the source05 correction of the Custom toolbar overlap found during native image review. Source-bound final acceptance is governed by the external review. Historical P0 results retain their original sources and artifacts.

## Scope and preservation

EN-01 changes labels, help, validation/error wrappers, fallback names, units and progress messages in 18 Rust files. The Vulkan busy diagnostic and Editor's exact consumer prefix change together. The source05 follow-up changes the Custom toolbar from `horizontal` to `horizontal_wrapped` and adds a narrow-pane regression; it does not redesign execution or persistence.

No localization framework, dependency, locale switch or migration of existing authored projects was added. User-authored JSON, names, paths, identifiers, unknown/opaque payloads, Custom source, exact keys and module/schema/SDK pins remain unchanged. Unicode preservation fixtures remain intentional. OS, compiler and third-party diagnostics are retained verbatim, including their original language; their engine-owned wrappers are English. The current user's authored JSON is not replaced by a historical acceptance fixture. The [after-native preservation check](../outputs/reviews/en01-english-ui-20261018/preservation-20261022-after-native05.json) confirms the resume baseline's **29 protected entries and three unrelated untracked files** are unchanged.

## Source and artifact binding

All listed native sources have HEAD `a3b4f9a73ec2a29f15f03b27403edd9ec3ff45ba`. HEAD is the base of the modified working tree; the snapshots cover the uncommitted implementation bytes.

| Snapshot / artifact | Binding and use |
| --- | --- |
| [Source04](../outputs/reviews/en01-english-ui-20261018/source-20261022-candidate04.json) | Source digest `1e673e22dbe9d9bf91bcf81d75a0f913391fad67d4737d6989a740c5f5b1cba8`; the 18-file language change. |
| [Normal Editor04](../outputs/verification/en01-english-ui-20261022/editor04/editor-receipt.json) | SHA-256 `9a6564b622a26e5c8800c07feca8af49dd6337555c541caeb9b67900dddf15d2`; source04 CLI, native UI tests and initial captures. |
| [Source05](../outputs/reviews/en01-english-ui-20261018/source-20261022-candidate05.json) | Source digest `cb59a3bcb7efa98ddb06ea5cda3a2451c8163e8ed4bb235fabfbd45c5386b5f1`; only `app.rs` wrapping and the `panes.rs` regression differ from source04. |
| [Normal Editor05](../outputs/verification/en01-english-ui-20261022/editor05/editor-receipt.json) | SHA-256 `cf14c08d8140a4d9ecec3917c96659d461d64c04d2989125b5c013e24f0ef901`; source05 Full/workflow/Custom capture and current Editor checks. |
| Exported standalone Player | SHA-256 `1ed74607397c37accc8ecd92d9e6219ffcee19425d28e3b5112c979fb6616cf9`; the source04 Scene/Assets export, schema2 Windows package with `ui,vulkan` and ten sealed members. Player's native dependency inputs are unchanged at source05. |

The Editors are actual CargoJSON-selected default release executables, not separately instrumented GUI builds; Player uses the listed selected export features. Full04 and Full05 are new verification deliveries; the checkout marker and prior P0 deliveries were not resealed or replaced. Full05 contains source05 documentation; the final checkout documentation is newer. Reusing the source05 executable does not claim a new build or updated Full05 documentation.

## Requirements and evidence

| Requirement | Actual evidence and boundary |
| --- | --- |
| English owned text; preserve opaque data | [Independent source audit](../outputs/reviews/en01-english-ui-20261018/independent-source-audit-20261022.json); [source05 scoped recheck](../outputs/reviews/en01-english-ui-20261018/independent-source05-static-recheck.json). Public EN/RU counterparts preserve the full meaning, exact commands and English UI literals. |
| Default logical coverage | [Independent artifact/case map](../outputs/reviews/en01-english-ui-20261018/independent-artifact-map04-to05.json): **713 unique ordinary cases, nine default-ignored cases, 14 logical targets**. Thirteen unchanged non-Editor harnesses contribute 471 passes; current Editor05 contributes 242. The [source04 direct run](../outputs/verification/en01-english-ui-20261022/default-direct04/result.json) remains failed, with its one junction case covered by the [same-artifact exact retry](../outputs/verification/en01-english-ui-20261022/junction-union04b/result.json). This scoped nonoverlapping union is **not one passed full-workspace Cargo invocation**; the zero-test Player bin is explicit. |
| Play/Stop reuse and slow Custom UI | [Current Editor05 recheck](../outputs/verification/en01-english-ui-20261022/editor-layout05b/result.json): **242 ordinary cases pass, five remain default-ignored, and two separately executed exact native cases pass**. The narrow-pane geometry regression is already included in 242, not counted again. Earlier [source04 native UI results](../outputs/verification/en01-english-ui-20261022/native-ui04/result.json) retain their source binding. |
| CLI, authoring and export | [Five source04 CLI routes](../outputs/verification/en01-english-ui-20261022/native-cli04b/result.json): self, integration, settings, Scene/Assets and GameWorker. Their semantic reports pass; output/logs and actual binary identities are retained. |
| Full source delivery and static checks | [Full05 sealing/export/capabilities](../outputs/verification/en01-english-ui-20261022/native-stage05/result.json), [format](../outputs/verification/en01-english-ui-20261022/fmt05/result.json), [test build](../outputs/verification/en01-english-ui-20261022/tests-build05/result.json) and [strict current Clippy](../outputs/verification/en01-english-ui-20261022/clippy05/20261022T112450Z-7126aab7/result.json) pass on source05. |
| Native English views and menus | [Initial Scene/File workflow](../outputs/verification/en01-english-ui-20261022/diagnostics04/20261022T105104Z-7a0c09c65045/result.json) plus [nine other views](../outputs/verification/en01-english-ui-20261022/native-views04/result.json) on normal Editor04. English menus were visible; independent review found Custom overlap. [Normal Editor05 workflow and Custom/Edit capture](../outputs/verification/en01-english-ui-20261022/diagnostics05-custom/20261022T111242Z-3f1cea36c29b/result.json) pass; the [independent current visual proof](../outputs/reviews/en01-english-ui-20261018/independent-custom05-workflow-proof.json) and current tests close `UI-LAYOUT-01`. |
| Standalone Player | [Explicit standalone audit](../outputs/verification/en01-english-ui-20261022/standalone-player05b/result.json): actual exported Player help, rendered package verification and exact English unknown-option diagnostic pass. Verification confirms sealed hashes/contract lock, tick3, 960×540 geometry, no source compilation/services and unchanged authored bytes. Source04 export also verifies the actual exported binary. This is offscreen evidence, with no Player GUI capture or benchmark claim. |
| Linux compile/CPU scope | [Linux-check05b](../outputs/verification/en01-english-ui-20261022/linux-check05b/result.json) passes the source05 Editor compile check and **five CPU Activity tests** in the existing Ubuntu environment, with source/protected data preserved; the [independent Linux proof](../outputs/reviews/en01-english-ui-20261018/independent-linux05b-proof.json) accepts this limited scope. It is not a Linux GPU/UI or hardware run. |
| Puzzle Scene capture | [Current native Puzzle capture](../outputs/verification/en01-english-ui-20261022/diagnostics05-scene/20261022T112439Z-6ae279bb8015/result.json) passes capture/backend validation, but its actual image still shows **Preparing Scene**. Even without Rust compilation, this is **not ready-geometry proof**; no additional feature change was made. |

The source04 ten-view captures are not relabeled as source05 measurements. Source05's Editor checks address its exact changed scope; the independent artifact/case map establishes the 713-case logical union. Repeated native or ordinary passes are not added as new ordinary cases.

## Failed attempts and limitations

Original failed receipts/logs remain retained: early formatting/test routes, the initial offline-cache CLI attempt, denied owned-junction creation, helper preflight/parsing failures and the first WSL launch blocked before a Linux child started. Corrected attempts use new output paths; none of the original failures is relabeled as passed. The unchanged exact junction case passed with the reviewed permission and is accounted for once in the explicit union.

The first native images exposed the longer English Custom toolbar overlapping an adjacent pane. The normal source05 image and geometry regression confirm the correction without cropping or a different renderer. A PNG and initialized WGPU/DX12 record prove the captured application image/backend, not compositor/foreground input, Scene readiness or performance. Capture settlement alone is not a readiness gate; the preparing-state Puzzle image remains explicitly limited.

No cold-start/FPS matrix, hardware Linux, arbitrary ABI/driver, native Linux audio/dialog or universal layout acceptance is implied. WSL/software-Vulkan historical results remain historical. Exact authored/project behavior and external side effects retain their existing contracts.

## Final binding and review

Final source binding is recorded externally in the [final snapshot](../outputs/reviews/en01-english-ui-20261018/final-source.json) and [native-input equality record](../outputs/reviews/en01-english-ui-20261018/final-native-input-equality.json). Those records govern the binding of the final documentation snapshot to the 163 source05 native inputs and actual `cf14c08…` build; they do not imply a rebuild or a new Full05 documentation delivery. Final source-bound acceptance is governed by the exact scope and conclusion of the [independent final review](../outputs/reviews/en01-english-ui-20261018/final-review.md).
