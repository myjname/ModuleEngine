# Game controls and module authoring tools

[English](game-ui-tools.md) | [Русский](ru/game-ui-tools.md) | [Documentation](README.md)

The installed source composition selects game presentations by contribution ID.
Player and Editor use the same `UiContribution` callbacks; dispatch contains no
game-specific action names. An absent contribution supplies no gameplay controls.
Common keyboard restart remains available without an installed gameplay
presentation. An installed input callback owns its keyboard bindings, including
the meaning of R; a namespaced level-reset action does not imply host restart.

## Game input from controls

`UiContribution.controls`, when present, replaces the legacy `hud` callback and
returns `GameInput`: pressed `actions`, current `values`, occurrence-specific
`action_values`, and session `restart`. `draw_controls_for` returns that packet.
When no richer controls callback is installed, the original boolean HUD is adapted to `restart`.
`draw_hud_for` remains the restart-only compatibility entry point; callers needing
game actions must use `draw_controls_for`.

## Application status and close capability

`UiContribution.controls_with_status` is the preferred callback when installed.
It receives Core `GameUiStatus` alongside the same immutable game state and
presentation arguments, and returns Core `GameUiResponse`. The status contains
`can_close_application`, current `services` diagnostics and current host `errors`.
It is application context, separate from authored data, gameplay state and the
Custom wire protocol. Service diagnostics expose current recovery/error state;
historical Activity failures do not become permanently latched game UI errors.

`GameUiResponse.input` carries ordinary `GameInput`; `close_application` is a
separate application request. Native Player grants the close capability and
handles an allowed request with the normal window-close command. Editor passes
`can_close_application:false` and rejects game close requests. Applications enforce
the capability even if a source callback requests closing without permission.
An accepted close suppresses gameplay input from the same gesture before dispatch.
No game action name or additional `GameInput` wire field represents closing.

Interactive paths use `collect_input_with_status_for`; the compatible
`collect_input_for` supplies a default status without a close capability.
The existing `controls -> GameInput` and boolean HUD callbacks remain supported.
A status-aware presentation owns application shortcut interpretation, allowing
Escape to pause its game instead of closing the window. Presentations using the
legacy callbacks keep Player's existing Escape-to-close behavior. Source-owned
R bindings and the absent-contribution R fallback remain unchanged.

Applications use `collect_input_for` to draw controls before collecting world
input and merge the two packets once. The source input callback executes at most
once for each collection; it may consume key events or retain its own action
state. Preview aim uses the previous accepted packet, so its pointer preview may
lag by one UI frame without consuming the current action. Control values override matching world values; action names are
unioned and control occurrence context overrides matching keys. Values-only
packets are input, including sliders without an action. A restart cancels all
same-frame gameplay actions and values and takes priority over queued old edges.
Editor combines visible panes before consuming its frame input; Player dispatches
once per UI frame. Queued action packets retain their captured context while held
values remain current. Paused Editor does not dispatch gameplay controls.

Interactive HUD widget ownership prevents its pointer gesture from reaching the
world callback. A nonempty controls packet during a pointer gesture also suppresses
the world packet. World held input may pause while that pointer gesture belongs
to controls. Callbacks should return actions only for a completed widget action,
and current values while a held control is being manipulated. Merely drawing a
HUD must not generate actions. Optional package availability and the host's active
presentation ID control whether its callbacks participate.

## Immutable source tools

`Editor::register_module_panel` retains its read-only callback API.
`Editor::register_module_tool` registers a callback with the same immutable
`GraphDocument` and `ModuleRegistry` inputs. It returns
`Result<Option<GraphDocument>, String>`: no completed edit, a candidate document,
or a diagnostic. A callback receives no Editor internals, mutable authored document,
history, Play state, file publication handle, or save authority.

Multi-frame tools use the optional `Editor::register_stateful_module_tool`
callback instead. It receives the same immutable inputs plus Core
`ModuleToolContext { storage_id, document_instance, revision }` and returns
`Result<ModuleToolResponse, String>`, whose fields are `candidate` and `dirty`.
The legacy registration remains compatible for immediate proposals and read-only
tools; it does not declare private pending edits to the application.

`storage_id` identifies one registered tool in one Editor, independently of its
visible pane. Keep one tool cache under that identity and store the current
document/revision inside it. Successful Open, New, reload or different-file
Save As adopts a fresh `document_instance`, even for identical game data. Save
and same-file Save As retain the instance; cancellation, failed reading or failed
publication retain the original instance and draft. Closing/reopening a pane
does not discard pending work. These identities are transient, not authored JSON.

Report `dirty` while local edits remain unresolved, including an invalid local
draft. A returned proposal is acknowledged only after normal Editor validation
and publication. Accepted Apply, including a no-op, and explicit Discard advance
only that tool's `revision` and clear only its pending flag. Rejected proposals
and callback errors retain the last pending status and acknowledgement. A source
must return its dirty status after editing; a callback error cannot report new
private work. On a changed document/revision, rebase the cache from the current
authored document. This handles Apply followed by Undo before the next tool draw;
another tool publishing identical data is not an acknowledgement of this tool.

Pending stateful tools participate in the unsaved guard. Save/Save As, Check,
export and starting Play require Apply or Discard first. Open/New/Close cannot
bypass them through Don't save. The guard and operation report offer per-tool
`Discard module draft`, including when its owner is inactive or its pane closed;
no unavailable source callback runs to discard it. Cancel returns to the current
project. Other module, JSON, Rust and execution drafts are not cleared. These
guards prevent a first gameplay tick and its effects; they do not remove existing
stopped-preview preparation reads. They do not persist a draft across a crash.

A tool is available only when its owning module is installed and active according
to shared `CompositionContext::activity`, including scheduled game systems without
a root-graph node or redundant Engine feature toggle.
`engine_only` also requires Full authoring with matching source availability;
Team cannot acquire this permission through a tool. Editor rejects application
during Play (including a paused Play session), active document/execution drafts,
or an unfinished scene gesture. Project candidates retain the ordinary Full/Team
project policy and scene/resource/preview validation. An `engine_only` tool uses
explicit Engine validation for game and legacy documents alike, may change the
Full blueprint, and establishes its new Project policy baseline after successful
publication. It still passes the same scene/resource/preview checks. A Project
tool cannot acquire Engine permissions through a candidate. Rejected and unchanged
candidates preserve the original document and history. Callback errors publish
only a diagnostic.

Each returned candidate denotes one completed authoring gesture. Editor validates
it before publication and records one Undo boundary, independently of history's
usual debounce. A tool implementing a multi-frame drag uses the stateful contract
above and returns a candidate at gesture completion. Scene edits share
the existing Inspector publication boundary. Editor alone owns dirty state,
Undo/Redo, Save/Save As, validation and file publication. Accepting a candidate
changes authored memory; it does not save a file. Save cancellation and errors
retain their existing transactional contract.

Synthetic contribution tests cover full control packets, the boolean adapter,
values-only detection, consuming source callbacks, source-owned R bindings and
HUD pointer isolation. Editor tests exercise a source
tool scene edit, one Undo, no-op/error preservation, inactive ownership and
Full/Team/Play rejection, including an independently scheduled source system
owner without a root node or Engine toggle. These tests are contract coverage; they do not establish
native foreground interaction or a particular game's acceptance.
