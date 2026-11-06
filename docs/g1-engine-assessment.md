# Engine assessment from G1

[English](g1-engine-assessment.md) | [Русский](ru/g1-engine-assessment.md) | [Documentation](README.md)

G1 demonstrates a useful source-built modular boundary: a separate game package
supplies rules, content, UI, tools and service requests through shared contracts.
Goal1's [foundation](g1-foundation-20261027.md) is historically accepted. The
[Source9 architecture review](../outputs/reviews/g1-game-20261027/architecture-final-source09.md)
retains its scoped decision. The [1 November continuation](g1-verification-20261101.md)
adds actual foreground authoring/play/recovery, corrected native audio and the
completed 1080p Playing budget. Its final independent decision governs Goal2.
Earlier Full/Player/Team, Menu and diagnostic results keep their original
bindings. This is evidence of concrete reuse, not a universal engine or
arbitrary visual composition of every genre.

## Reuse and ownership

| Shared engine contract | Source-owned G1 policy |
| --- | --- |
| Core ECS, transactional execution, Host and authored/Play isolation | Push/blocked/solved policy in `g1.rules` |
| Independently enabled Game Kit grid/history/campaign/actions | Cell kinds, attempts, ten layouts, score/unlock rules |
| Requested `audio.wav` and `persistence.json` services | Eight cues/events, profile validation and explicit recovery |
| Registered UI/input and application capabilities | Menus/theme/actions/camera and standalone-only Quit |
| Immutable stateful tool context/candidates, validation/history and Full/Team policy | Grid palette, draft painting and complete-board validation |
| Native-owner feature mapping and selected Player export | Game's selected source composition and resources |
| Optional pre-interpolation owner model projection | Procedural gardener breathing/heading sway and turning flowers |

Game Kit is a serde-only library with no Core, renderer, window or IO dependency.
Its formation consumer executes a real registered non-puzzle system, including
rejection preservation, resume/Undo and progression. That is reuse evidence,
not another finished portfolio game. Legacy Puzzle and ARPG remain separate
regression consumers; generic dispatch does not interpret crate/goal actions.

Native source installation, authored scheduling and requested services are
distinct. Cargo features compile implementations and source installers register
them; `game.execution` schedules installed systems; active contributions request
application services. JSON cannot supply missing binary code. G1 deliberately
compiles all four helpers into one coarse game-owned `g1.rules` executable.
Removing an arbitrary JSON flag does not remove its Undo implementation. These
libraries are not decorative graph nodes or a dynamic binary-plugin ABI.

The root scalar/clock/Custom starter graph and demo-world factories are different
entry points from authored Game execution with resolved scenes and a system
schedule. Root graph world/render/image dataflow still has its own contract;
deleting a demo node is not disabling a Game system. Core ECS remains canonical
gameplay storage. Objects/custom providers project shared object data rather than
demonstrating a replacement memory model or scheduler. GPU image/Scene3D pins
do not supply general buffer/geometry/lifetime contracts for arbitrary streaming
or virtualized geometry. Different G1 art does not establish that extensibility.

## Reusable pose and failure boundaries

The optional `GamePoseProjection` registry seam reads an immutable raw snapshot
and state and returns object-ID/model matrices before interpolation and content
expansion. `GameSession.snapshot()` remains canonical; presentation uses a fresh
local candidate. Unknown/ambiguous raw IDs, conflicting active writers and
nonfinite/projective models reject that candidate. No mandatory GameContribution field, extra native feature/dependency, or
relaxation of frame-identity validation was introduced by this seam. No fake
scheduled node was added.

Host captures owner poses before interpolation, then attaches imported
primitive-local transforms. G1 uses SRT breathing/heading sway with anchored
feet and slow flower rotation; late decoration supplies only occupied-crate
tint. Invalid clocks/singular frames retain ordinary geometry. This is
procedural static-mesh presentation, not skeletal animation. Shared material/
texture ownership and canonical World/profile/history/events remain unchanged.

Pose-capture failure resets visual history and reports rendering failure while
preserving the previous completed frame/provenance. Already committed gameplay
events still reach services; visual failure cannot swallow an accepted save.
Rejected gameplay keeps its rollback rules. Neutral consumers and real service
delivery tests cover this isolation. Native source callbacks are trusted code,
not a sandbox or a general geometry API. Source6's 289 scoped passes and later
native images retain their own bindings; they are not a new full 815-pass run.

## Authoring ownership and integration friction

Source9 fixes the real equal-content-project draft leak through optional Core
`ModuleToolContext`/`ModuleToolResponse`, one cache per registered tool and an
Editor-owned document-instance/revision acknowledgement. Accepted/no-op Apply
and Discard resolve only that tool; reject/error preserve work. Clean caches
follow Undo/Redo; dirty external changes stay stale. Pending tools participate
in Save/Save As, Check/export, Play and project/close guards, with independent
Discard available even while hidden/inactive. Legacy immediate callbacks remain
compatible but cannot report private drafts. No authored schema or mutable Editor
capability is exposed. Stopped-preview reads remain; guards prevent gameplay
ticks/effects, not every preview IO. This is not crash recovery.

[Independent Source9 lifecycle review](../outputs/reviews/g1-game-20261027/source9-lifecycle-review.md)
closes G1-DRAFT01 and G1-STATE-ROOT01 in their scoped tests. Automatic save roots
now reject nonabsolute environment candidates, without changing explicit paths
or claiming filesystem writability. The real Team-input CPU case preserves
frozen policy/resources; it is not native foreground acceptance.

Actual consumers exposed double input reads, service starvation, lost accepted
prefix effects, incomplete GPU feature ownership, stale geometry, invisible
service errors and the draft/root defects. Their fixes and independent checks
show why module diagrams alone are insufficient. Primary presentation/camera
roles remain coarse. Tools propose full documents, which can grow expensive.
G1's flat objects do not establish nested-prefab production or a general tile importer.

Within one Host, requested service IDs must be unique: independent contributions
both requesting `audio.wav` or `persistence.json` reject instead of merging or
creating named instances. Separate Hosts have independent owners. G1 requests
each once; this is future composition friction, not a current G1 defect. A consumer needing independent configurations would justify an explicit
aggregation or named-instance contract.

## Resource limits and next pressure

Unlimited Undo means no artificial move-count cap, not infinite memory/storage.
Full-state history, transactional copies and synchronous bounded JSON publication
cost CPU and memory. A historical release CPU probe after 1000 moves produced a
68,425-byte profile; removing idle decoding/republication changed scoped idle
mean/p95 from 5.508/7.349 to 3.088/4.593 ms. These are not native FPS or universal
savings. The 8 MiB envelope limit rejects instead of truncating history.
Content revisions validate saved attempts and every Undo edge; automatic
migration and cross-process locking/merge are absent. Invalid/unreadable files
remain preserved until explicit read-only/reset/quarantine recovery.

Windows provides owned multi-voice music/SFX WAV output. Real G1 listening exposed
a shared backend defect: a 30 ms queue supplied only about 75% of wall-clock
audio on the tested endpoint, despite successful completion counters. A bounded
40 ms queue and a real sustained source-clock regression correct the demonstrated
cause; owner stop/retry/independence tests pass. The fix belongs to the reusable
audio module, while the separate soundtrack revision belongs to G1. The user
confirmed normal native playback after repair. This does not establish native
Linux audio, compressed streaming/spatial audio, device pitch measurement or
end-to-end sound latency. Already mixed audio can delay gain/stop effects until
its queued portion is processed.

The actual Source16 Player passes the agreed 1080p Playing timing/memory route
with movement, Undo, onscreen UI, sound and persistence. Earlier Menu passes
remain narrower historical observations; clean-machine/hardware Linux/phone
coverage is absent. See the [continuation report](g1-verification-20261101.md)
for exact artifacts, independent review and preservation.

The next separate G2 brief should pressure reusable actions/audio/save/UI with
real-time ARPG behavior and required character animation. Later consumers should
drive true 2D authoring, spatial contacts and executable PBR/lighting/texture
budgets. Static registration/rebuilding, coarse camera/presentation ownership,
full-document tool edits and limited geometry contracts above remain practical
friction; G1 did not remove them by becoming playable. No universal scheduler,
networking, mobile port or binary-plugin subsystem starts implicitly. See the
[roadmap](portfolio-roadmap.md).
