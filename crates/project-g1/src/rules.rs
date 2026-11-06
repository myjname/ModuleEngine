use crate::{
    components::{self, Catalog, CellData, Kind, Level},
    *,
};
use module_engine_core::*;
use module_engine_game_kit::{
    actions::{ActionMap, Binding, Trigger},
    campaign::ScoreOrder,
    grid::Occupancy,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const UI_KEY: &str = "g1.ui/1";
pub(crate) const CUES: &[&str] = &[
    "music", "move", "push", "goal", "blocked", "undo", "win", "ui",
];
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiState {
    phase: Phase,
    return_phase: Phase,
    warning: Option<String>,
    save_blocked: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeParams {
    catalog: Catalog,
}

pub fn action_map() -> ActionMap {
    let mut actions = [
        "play",
        "continue",
        "levels",
        "settings",
        "back",
        "menu",
        "pause",
        "resume",
        "undo",
        "restart",
        "next",
        "reset_progress",
        "select_level",
        "set_volumes",
        "north",
        "east",
        "south",
        "west",
    ]
    .map(|name| format!("g1.{name}"))
    .to_vec();
    actions.sort();
    let bindings = [
        ("ArrowUp", "north"),
        ("W", "north"),
        ("ArrowRight", "east"),
        ("D", "east"),
        ("ArrowDown", "south"),
        ("S", "south"),
        ("ArrowLeft", "west"),
        ("A", "west"),
        ("Z", "undo"),
        ("R", "restart"),
        ("Escape", "pause"),
    ]
    .into_iter()
    .map(|(key, action)| Binding {
        key: key.into(),
        action: format!("g1.{action}"),
        trigger: Trigger::Pressed,
    })
    .collect();
    ActionMap::new(actions, bindings).expect("Valid source-owned input map")
}
pub fn profile(game: &GameState) -> Result<Profile, String> {
    serde_json::from_value(
        game.module_data
            .get(PROFILE_KEY)
            .ok_or("Crate Garden has not initialized its profile")?
            .clone(),
    )
    .map_err(|e| format!("Invalid Crate Garden profile: {e}"))
}
pub fn view(game: &GameState) -> Result<View, String> {
    serde_json::from_value(
        game.module_data
            .get(VIEW_KEY)
            .ok_or("Crate Garden is preparing")?
            .clone(),
    )
    .map_err(|e| format!("Invalid Crate Garden view: {e}"))
}
fn default_profile(catalog: &Catalog) -> Result<Profile, String> {
    Ok(Profile {
        version: 1,
        catalog_revision: catalog.revision.clone(),
        campaign: Campaign::new(catalog.ids(), ScoreOrder::LowerIsBetter)?,
        settings: Settings::default(),
    })
}
fn new_attempt(level: &Level) -> Attempt {
    Attempt {
        level_revision: level.revision.clone(),
        history: UndoJournal::new(level.initial.clone()),
    }
}
fn settings_valid(settings: Settings) -> Result<(), String> {
    if [settings.music_volume, settings.sfx_volume]
        .iter()
        .all(|volume| volume.is_finite() && (0.0..=1.0).contains(volume))
    {
        Ok(())
    } else {
        Err("Music and sound volumes must be finite values from 0 to 1".into())
    }
}
fn validate_profile(profile: &Profile, catalog: &Catalog) -> Result<(), String> {
    if profile.version != 1
        || profile.catalog_revision != catalog.revision
        || profile.campaign.score_order() != ScoreOrder::LowerIsBetter
    {
        return Err("Saved profile version/content revision does not match this garden".into());
    }
    profile.campaign.validate_catalog(&catalog.ids())?;
    settings_valid(profile.settings)?;
    profile.campaign.validate_attempt(|attempt| {
        validate_attempt(attempt, catalog.level(profile.campaign.current())?)
    })
}
fn validate_attempt(attempt: &Attempt, level: &Level) -> Result<(), String> {
    if attempt.level_revision != level.revision {
        return Err("Saved attempt belongs to another level revision".into());
    }
    attempt
        .history
        .validate(|state| level.validate_position(state))?;
    if attempt.history.current().moves
        != u64::try_from(attempt.history.undo_len())
            .map_err(|_| "Undo length cannot fit a move count")?
    {
        return Err("Saved move count disagrees with Undo history".into());
    }
    let mut history = attempt.history.clone();
    while history.can_undo() {
        let next = history.current().clone();
        let previous = history.undo().ok_or("Missing saved Undo entry")?;
        if level.solved(previous) {
            return Err("Saved Undo history continues after a completed level".into());
        }
        let mut legal = false;
        for direction in [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ] {
            if move_position(level, previous, direction)?
                .is_some_and(|(position, _)| position == next)
            {
                legal = true;
                break;
            }
        }
        if !legal {
            return Err("Saved Undo history contains a noncanonical or disconnected move".into());
        }
    }
    if history.current() != &level.initial {
        return Err("Saved Undo history does not begin at the authored level".into());
    }
    Ok(())
}

/// Canonical one-cell movement: one crate may be pushed into an empty cell;
/// chains/pulls are never introduced by an alternative input or restore route.
pub(crate) fn move_position(
    level: &Level,
    state: &Position,
    direction: Direction,
) -> Result<Option<(Position, bool)>, String> {
    level.validate_position(state)?;
    let (dx, dy) = direction.delta();
    let Some(target) = state.pawn.offset(dx, dy) else {
        return Ok(None);
    };
    let bounds = level.bounds()?;
    if !bounds.contains(target) || level.walls.contains(&target) {
        return Ok(None);
    }
    let mut occupants = Occupancy::new(bounds);
    occupants.insert(state.pawn, "actor", level.pawn_id)?;
    for (id, cell) in &state.crates {
        occupants.insert(*cell, "actor", *id)?;
    }
    let crate_id = occupants.get(target, &"actor").copied();
    let mut relocations = vec![(level.pawn_id, target)];
    let mut next = state.clone();
    if let Some(id) = crate_id {
        let Some(destination) = target.offset(dx, dy) else {
            return Ok(None);
        };
        if !bounds.contains(destination)
            || level.walls.contains(&destination)
            || occupants.get(destination, &"actor").is_some()
        {
            return Ok(None);
        }
        relocations.push((id, destination));
        next.crates.insert(id, destination);
    }
    occupants.relocate(&relocations)?;
    next.pawn = target;
    next.moves = next
        .moves
        .checked_add(1)
        .ok_or("Move counter exhausted its integer representation")?;
    Ok(Some((next, crate_id.is_some())))
}

pub fn install(registry: &mut SystemRegistry) -> Result<(), String> {
    components::install(registry.components_mut())?;
    registry.register(
        SystemDescriptor {
            id: "g1.rules".into(),
            state_schema: "g1.rules/1".into(),
            components: vec![BOARD.into(), CELL.into(), "mesh".into()],
            requires: vec![],
            ports: vec![
                SystemPort {
                    name: "world".into(),
                    data_type: "GameWorld".into(),
                    access: ResourceAccess::ReadWrite,
                },
                SystemPort {
                    name: "input".into(),
                    data_type: "GameInput".into(),
                    access: ResourceAccess::Read,
                },
                SystemPort {
                    name: "events".into(),
                    data_type: "GameEvents".into(),
                    access: ResourceAccess::ReadWrite,
                },
            ],
        },
        |params| {
            let params: RuntimeParams =
                serde_json::from_value(params.clone()).map_err(|error| {
                    format!("Crate Garden needs its prepared authored runtime catalog: {error}")
                })?;
            if params.catalog.levels.len() != crate::level_catalog().len() {
                return Err("Invalid prepared garden catalog".into());
            }
            Ok(Box::new(Rules {
                catalog: params.catalog,
            }))
        },
    )?;
    registry.register_project_contract(
        "g1.rules",
        |project, _| {
            if project
                .execution
                .stages
                .iter()
                .flat_map(|stage| &stage.nodes)
                .any(|node| node.module == "g1.rules")
            {
                Catalog::from_project(project)?;
                let nodes = project
                    .execution
                    .stages
                    .iter()
                    .flat_map(|stage| &stage.nodes)
                    .filter(|node| node.module == "g1.rules")
                    .collect::<Vec<_>>();
                if nodes.len() != 1 {
                    return Err("Crate Garden requires one rules instance".into());
                }
                if !(nodes[0].params.is_null()
                    || nodes[0]
                        .params
                        .as_object()
                        .is_some_and(|value| value.is_empty()))
                {
                    // Older delivery projections may contain a derived cache.
                    // It is disposable, never an authored-data authority: the
                    // normal Host preparation always replaces it from scenes.
                    let fields = nodes[0]
                        .params
                        .as_object()
                        .ok_or("Garden params must be an object")?;
                    if fields.len() != 1 || !fields.contains_key("catalog") {
                        return Err("Garden configuration belongs to authored scenes".into());
                    }
                }
            }
            Ok(())
        },
        |project| {
            if !project
                .execution
                .stages
                .iter()
                .flat_map(|stage| &stage.nodes)
                .any(|node| node.module == "g1.rules")
            {
                return Ok(());
            }
            let catalog = Catalog::from_project(project)?;
            for node in project
                .execution
                .stages
                .iter_mut()
                .flat_map(|stage| &mut stage.nodes)
                .filter(|node| node.module == "g1.rules")
            {
                node.params = serde_json::to_value(RuntimeParams {
                    catalog: catalog.clone(),
                })
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        },
    )?;
    registry.register_game_contribution(GameContribution {
        id: "g1.presentation".into(),
        activation_modules: BTreeSet::from(["g1.rules".into()]),
        begin_tick: |_| Ok(()),
        initial_camera: |project| {
            let span = Catalog::from_project(project).map_or(12., |catalog| {
                catalog
                    .levels
                    .iter()
                    .map(|level| {
                        level.config.width.max(level.config.height) as f32 * level.config.cell_size
                    })
                    .fold(0., f32::max)
            });
            Camera {
                yaw: 0.42,
                pitch: 0.95,
                distance: span * 1.65,
                target: [0.; 3],
            }
        },
        follow_target: |game| {
            game.world
                .entities_with(&[BOARD])
                .first()
                .and_then(|id| game.world.object(*id))
                .map(|object| object.transform.translation)
        },
        decorate_snapshot: decorate_garden_snapshot,
        script_input: |input| serde_json::to_value(input).expect("Portable named input"),
        validate_event,
        validate_state,
        service_requests,
        diagnostics: crate::diagnostics,
    })?;
    registry.register_pose_projection("g1.presentation", project_garden_poses)
}
pub fn registry() -> SystemRegistry {
    let mut registry = SystemRegistry::builtin();
    install(&mut registry).expect("Valid Crate Garden contracts");
    registry
}

struct Rules {
    catalog: Catalog,
}
impl Rules {
    fn publish(&self, game: &mut GameState, profile: &Profile, ui: &UiState) -> Result<(), String> {
        let current = self.catalog.level(profile.campaign.current())?;
        let position = profile
            .campaign
            .attempt()
            .map_or(&current.initial, |attempt| attempt.history.current());
        let view = View {
            phase: ui.phase,
            current_level: self
                .catalog
                .levels
                .iter()
                .position(|level| level.id == current.id)
                .ok_or("Missing selected level")?,
            levels: self
                .catalog
                .levels
                .iter()
                .enumerate()
                .map(|(index, level)| LevelSummary {
                    id: level.id.clone(),
                    name: level.title.clone(),
                    index,
                    unlocked: profile.campaign.is_unlocked(&level.id),
                    best_moves: profile.campaign.best(&level.id),
                })
                .collect(),
            moves: position.moves,
            can_undo: profile
                .campaign
                .attempt()
                .is_some_and(|attempt| attempt.history.can_undo()),
            can_continue: profile.campaign.attempt().is_some(),
            settings: profile.settings,
            board: current.view(position),
            warning: ui.warning.clone(),
        };
        game.module_data.insert(
            PROFILE_KEY.into(),
            serde_json::to_value(profile).map_err(|e| e.to_string())?,
        );
        game.module_data.insert(
            UI_KEY.into(),
            serde_json::to_value(ui).map_err(|e| e.to_string())?,
        );
        game.module_data.insert(
            VIEW_KEY.into(),
            serde_json::to_value(view).map_err(|e| e.to_string())?,
        );
        Ok(())
    }
    fn apply_position(
        &self,
        game: &mut GameState,
        profile: &Profile,
        reset: bool,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), String> {
        let level = self.catalog.level(profile.campaign.current())?;
        let same_level = game
            .world
            .entities_with(&[BOARD])
            .first()
            .and_then(|id| game.world.component(*id, BOARD))
            .and_then(|value| value.get("level_id"))
            .and_then(Value::as_str)
            == Some(level.id.as_str());
        if !same_level {
            game.world = EcsWorld::from_entities(level.entities.clone())?;
        }
        if reset || !same_level {
            for id in game.world.entities_with(&[CELL]) {
                let data: CellData = serde_json::from_value(
                    game.world
                        .component(id, CELL)
                        .ok_or("Missing grid component")?
                        .clone(),
                )
                .map_err(|e| e.to_string())?;
                game.world
                    .transform_mut(id)
                    .ok_or("Missing grid transform")?
                    .translation = level.translation(data.cell(), data.kind)?;
            }
        }
        let state = profile
            .campaign
            .attempt()
            .map_or(&level.initial, |attempt| attempt.history.current());
        for (id, cell, kind) in std::iter::once((level.pawn_id, state.pawn, Kind::Pawn)).chain(
            state
                .crates
                .iter()
                .map(|(id, cell)| (*id, *cell, Kind::Crate)),
        ) {
            let fields = game
                .world
                .component_mut(id, CELL)
                .ok_or("Missing board actor")?;
            *fields = serde_json::to_value(CellData {
                x: cell.x,
                z: cell.y,
                kind,
            })
            .map_err(|e| e.to_string())?;
            game.world
                .transform_mut(id)
                .ok_or("Missing actor transform")?
                .translation = level.translation(cell, kind)?;
        }
        if reset || !same_level {
            events.push(GameEvent::presentation_reset());
        }
        Ok(())
    }
    fn outcome(&self, profile: &Profile) -> Result<Phase, String> {
        let level = self.catalog.level(profile.campaign.current())?;
        if profile
            .campaign
            .attempt()
            .is_some_and(|attempt| level.solved(attempt.history.current()))
        {
            Ok(
                if profile.campaign.current()
                    == self.catalog.levels.last().ok_or("Empty catalog")?.id
                {
                    Phase::Complete
                } else {
                    Phase::Victory
                },
            )
        } else {
            Ok(Phase::Playing)
        }
    }
    /// Settle an activated attempt, including a valid authored zero-move win.
    /// Call only while Playing: completed/menu/idle frames cannot replay it.
    fn settle_active_attempt(
        &self,
        profile: &mut Profile,
        ui: &mut UiState,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), String> {
        let level = self.catalog.level(profile.campaign.current())?;
        let attempt = profile
            .campaign
            .attempt()
            .ok_or("Playing without an attempt")?;
        if level.solved(attempt.history.current()) {
            let moves = attempt.history.current().moves;
            profile.campaign.complete(moves);
            ui.phase = self.outcome(profile)?;
            cue(events, "win");
        }
        Ok(())
    }
}
/// Presentation-only feedback. Shared materials, texture Arcs,
/// authored data and the canonical world/attempt remain untouched.
fn decorate_garden_snapshot(snapshot: &mut SceneSnapshot, game: &GameState) {
    let mut goals = BTreeSet::new();
    let mut crates = Vec::new();
    for id in game.world.entities_with(&[CELL]) {
        let Some(raw) = game.world.component(id, CELL) else {
            continue;
        };
        if !matches!(
            raw.get("kind").and_then(Value::as_str),
            Some("goal" | "crate" | "pawn")
        ) {
            continue;
        }
        let Ok(cell) = serde_json::from_value::<CellData>(raw.clone()) else {
            continue;
        };
        match cell.kind {
            Kind::Goal => {
                goals.insert(cell.cell());
            }
            Kind::Crate => crates.push((id, cell.cell())),
            _ => {}
        }
    }
    let occupied = crates
        .into_iter()
        .filter_map(|(id, cell)| goals.contains(&cell).then_some(id))
        .collect::<BTreeSet<_>>();
    for item in &mut snapshot.items {
        if occupied.contains(&item.id) {
            item.color = [80, 245, 130, item.color[3]];
        }
    }
}

/// Pure owner poses on raw objects. Host interpolates these before composing
/// imported primitive-local matrices; the late decorator only supplies color.
fn project_garden_poses(
    snapshot: &SceneSnapshot,
    game: &GameState,
) -> Result<BTreeMap<u64, [[f32; 4]; 4]>, String> {
    let mut models = BTreeMap::new();
    for item in &snapshot.items {
        let Some(raw) = game.world.component(item.id, CELL) else {
            continue;
        };
        let Ok(cell) = serde_json::from_value::<CellData>(raw.clone()) else {
            continue;
        };
        if let Some(delta) = ambient_pose(cell.kind, game.time)
            .and_then(|pose| owner_pose_delta(game, item.id, pose))
        {
            models.insert(item.id, multiply_matrices(delta, item.model));
        }
    }
    Ok(models)
}

fn ambient_pose(kind: Kind, time: f64) -> Option<[[f32; 4]; 4]> {
    if !time.is_finite() {
        return None;
    }
    // Reduce before trigonometry so even a huge finite simulation clock stays
    // bounded. No wall clock, frame counter or persistent animation state.
    let phase = |period: f64| (std::f64::consts::TAU * (time.rem_euclid(period) / period)) as f32;
    match kind {
        Kind::Pawn => {
            let breath = 1. + 0.03 * phase(3.6).sin();
            let sway = 0.045 * phase(5.).sin();
            // Original gardener geometry has its sole plane at local y=-0.42.
            // Breathing anchors the sole plane; a small local heading sway
            // keeps the source pose SRT so the neutral interpolator can blend it.
            let sole = -0.42;
            let pose = Transform {
                translation: [0., sole * (1. - breath), 0.],
                rotation: [0., sway, 0.],
                scale: [1., breath, 1.],
            }
            .matrix();
            Some(pose)
        }
        Kind::Goal => Some(
            Transform {
                rotation: [0., phase(24.), 0.],
                ..Default::default()
            }
            .matrix(),
        ),
        _ => None,
    }
}

/// Apply one owner-local pose to every imported primitive, including any
/// primitive-local offsets. Parent transforms and nonuniform scales retain
/// their axes; singular/invalid frames simply retain their ordinary appearance.
fn owner_pose_delta(game: &GameState, id: u64, pose: [[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    if pose == identity_matrix() {
        return Some(pose);
    }
    let mut frame = identity_matrix();
    let mut current = Some(id);
    let mut seen = BTreeSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return None;
        }
        let object = game.world.object(id)?;
        frame = multiply_matrices(object.transform.matrix(), frame);
        current = object.parent;
    }
    let inverse = inverse_pose_frame(frame)?;
    let delta = multiply_matrices(multiply_matrices(frame, pose), inverse);
    delta
        .iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(delta)
}

fn inverse_pose_frame(frame: [[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    let column = |index: usize| {
        [
            frame[0][index] as f64,
            frame[1][index] as f64,
            frame[2][index] as f64,
        ]
    };
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a.into_iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
    let [x, y, z] = [column(0), column(1), column(2)];
    let determinant = dot(x, cross(y, z));
    if !determinant.is_finite() || determinant == 0. {
        return None;
    }
    let mut inverse = identity_matrix();
    for (index, axis) in [cross(y, z), cross(z, x), cross(x, y)]
        .into_iter()
        .enumerate()
    {
        let row = axis.map(|value| value / determinant);
        for (column, value) in row.into_iter().enumerate() {
            inverse[index][column] = value as f32;
        }
        inverse[index][3] = -dot(row, column(3)) as f32;
    }
    inverse
        .iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(inverse)
}

fn cue(events: &mut Vec<GameEvent>, name: &str) {
    events.push(GameEvent::new("g1.audio", json!({"cue":name})));
}
fn volumes(events: &mut Vec<GameEvent>, settings: Settings) {
    events.push(GameEvent::new(
        "g1.audio.control",
        json!({"command":"volume","bus":"music","volume":settings.music_volume}),
    ));
    events.push(GameEvent::new(
        "g1.audio.control",
        json!({"command":"volume","bus":"sfx","volume":settings.sfx_volume}),
    ));
}
fn captured<'a>(input: &'a GameInput, key: &str) -> Option<&'a Value> {
    input
        .action_values
        .get(key)
        .or_else(|| input.values.get(key))
}

impl ExecutableSystem for Rules {
    fn initialize(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        let mut ui = UiState::default();
        let profile = if let Some(raw) = ctx.game.module_data.get(SAVED_KEY) {
            match serde_json::from_value::<Profile>(raw.clone())
                .map_err(|e| e.to_string())
                .and_then(|profile| {
                    validate_profile(&profile, &self.catalog)?;
                    Ok(profile)
                }) {
                Ok(profile) => profile,
                Err(error) => {
                    ui.warning = Some(format!(
                        "Saved progress could not be resumed: {error}. Your previous save is preserved. Reset progress explicitly to replace it."
                    ));
                    ui.save_blocked = true;
                    default_profile(&self.catalog)?
                }
            }
        } else {
            default_profile(&self.catalog)?
        };
        self.apply_position(ctx.game, &profile, true, ctx.events)?;
        self.publish(ctx.game, &profile, &ui)?;
        volumes(ctx.events, profile.settings);
        cue(ctx.events, "music");
        Ok(())
    }
    fn step(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        // No source-owned input means no game/profile/UI change. In particular,
        // an idle frame must not decode and republish a growing Undo journal.
        // The executor still validates the final world/state transaction.
        if !ctx
            .input_resource("input")?
            .actions
            .iter()
            .any(|action| action.starts_with("g1."))
        {
            return Ok(());
        }
        let input = ctx.input_resource("input")?.clone();
        let mut profile = profile(ctx.game)?;
        let previous = profile.clone();
        let mut ui: UiState = serde_json::from_value(
            ctx.game
                .module_data
                .get(UI_KEY)
                .ok_or("Missing garden UI state")?
                .clone(),
        )
        .map_err(|e| e.to_string())?;
        let mut reset = false;
        let mut events = vec![];
        if input.pressed("g1.reset_progress") {
            let settings = profile.settings;
            profile = default_profile(&self.catalog)?;
            profile.settings = settings;
            ui = UiState::default();
            reset = true;
            events.push(GameEvent::new("g1.storage.reset", json!({"confirm":true})));
            // Even an already-default profile must replace an explicitly reset
            // corrupt envelope. The save follows reset in this same batch.
            events.push(GameEvent::new("g1.save", json!({"data":profile})));
            cue(&mut events, "ui");
        } else {
            if input.pressed("g1.set_volumes") {
                let music = captured(&input, "g1.music_volume")
                    .and_then(Value::as_f64)
                    .ok_or("Missing music volume")?;
                let sfx = captured(&input, "g1.sfx_volume")
                    .and_then(Value::as_f64)
                    .ok_or("Missing sound volume")?;
                if ![music, sfx]
                    .iter()
                    .all(|volume| volume.is_finite() && (0.0..=1.0).contains(volume))
                {
                    return Err("Volumes must lie between zero and one".into());
                }
                let settings = Settings {
                    music_volume: music as f32,
                    sfx_volume: sfx as f32,
                };
                settings_valid(settings)?;
                profile.settings = settings;
                volumes(&mut events, settings);
            }
            if input.pressed("g1.menu") {
                ui.phase = Phase::Menu;
            } else if input.pressed("g1.pause") {
                ui.phase = match ui.phase {
                    Phase::Playing => Phase::Paused,
                    Phase::Paused => self.outcome(&profile)?,
                    _ => Phase::Menu,
                };
            } else if input.pressed("g1.back") {
                ui.phase = if matches!(ui.phase, Phase::Settings | Phase::LevelSelect) {
                    ui.return_phase
                } else {
                    Phase::Menu
                };
            } else if input.pressed("g1.settings") {
                ui.return_phase = if ui.phase == Phase::Paused {
                    Phase::Paused
                } else {
                    Phase::Menu
                };
                ui.phase = Phase::Settings;
            } else if input.pressed("g1.levels") {
                ui.return_phase = Phase::Menu;
                ui.phase = Phase::LevelSelect;
            } else if input.pressed("g1.play") && ui.phase == Phase::Menu {
                profile.campaign.select(&self.catalog.levels[0].id)?;
                profile
                    .campaign
                    .set_attempt(new_attempt(&self.catalog.levels[0]));
                ui.phase = Phase::Playing;
                reset = true;
            } else if input.pressed("g1.select_level") && ui.phase == Phase::LevelSelect {
                let id = captured(&input, "g1.level_id")
                    .and_then(Value::as_str)
                    .ok_or("Missing selected level ID")?;
                profile.campaign.select(id)?;
                profile
                    .campaign
                    .set_attempt(new_attempt(self.catalog.level(id)?));
                ui.phase = Phase::Playing;
                reset = true;
            } else if (input.pressed("g1.continue") || input.pressed("g1.resume"))
                && matches!(ui.phase, Phase::Menu | Phase::Paused)
            {
                if profile.campaign.attempt().is_none() {
                    profile
                        .campaign
                        .set_attempt(new_attempt(self.catalog.level(profile.campaign.current())?));
                }
                ui.phase = Phase::Playing;
                reset = true;
            } else if input.pressed("g1.next") && ui.phase == Phase::Victory {
                let index = self
                    .catalog
                    .levels
                    .iter()
                    .position(|level| level.id == profile.campaign.current())
                    .ok_or("Missing level")?;
                let next = self.catalog.levels.get(index + 1).ok_or("No next level")?;
                profile.campaign.select(&next.id)?;
                profile.campaign.set_attempt(new_attempt(next));
                ui.phase = Phase::Playing;
                reset = true;
            } else if input.pressed("g1.restart")
                && matches!(
                    ui.phase,
                    Phase::Playing | Phase::Paused | Phase::Victory | Phase::Complete
                )
            {
                profile
                    .campaign
                    .set_attempt(new_attempt(self.catalog.level(profile.campaign.current())?));
                ui.phase = Phase::Playing;
                reset = true;
                cue(&mut events, "ui");
            } else if input.pressed("g1.undo")
                && matches!(ui.phase, Phase::Playing | Phase::Victory | Phase::Complete)
            {
                if let Some(mut attempt) = profile.campaign.attempt().cloned()
                    && attempt.history.undo().is_some()
                {
                    profile.campaign.set_attempt(attempt);
                    ui.phase = self.outcome(&profile)?;
                    reset = true;
                    cue(&mut events, "undo");
                }
            } else if ui.phase == Phase::Playing {
                let directions = [
                    Direction::North,
                    Direction::East,
                    Direction::South,
                    Direction::West,
                ]
                .into_iter()
                .filter(|direction| input.pressed(direction.action()))
                .collect::<Vec<_>>();
                if directions.len() == 1 {
                    let level = self.catalog.level(profile.campaign.current())?;
                    let mut attempt = profile
                        .campaign
                        .attempt()
                        .cloned()
                        .ok_or("Playing without an attempt")?;
                    if let Some((next, pushed)) =
                        move_position(level, attempt.history.current(), directions[0])?
                    {
                        let moves = next.moves;
                        let solved = level.solved(&next);
                        let placed_on_goal = pushed
                            && attempt
                                .history
                                .current()
                                .crates
                                .iter()
                                .any(|(id, previous)| {
                                    next.crates.get(id).is_some_and(|cell| {
                                        cell != previous && level.goals.contains(cell)
                                    })
                                });
                        attempt.history.commit(next);
                        profile.campaign.set_attempt(attempt);
                        if !solved {
                            cue(
                                &mut events,
                                if placed_on_goal {
                                    "goal"
                                } else if pushed {
                                    "push"
                                } else {
                                    "move"
                                },
                            );
                        }
                        events.push(GameEvent::new(
                            "g1.moved",
                            json!({"moves":moves,"pushed":pushed,"solved":solved}),
                        ));
                    } else {
                        cue(&mut events, "blocked");
                    }
                } else if !directions.is_empty() {
                    cue(&mut events, "blocked");
                }
            }
            if ui.phase == Phase::Playing {
                self.settle_active_attempt(&mut profile, &mut ui, &mut events)?;
            }
            if profile != previous && !ui.save_blocked {
                events.push(GameEvent::new("g1.save", json!({"data":profile})));
            }
        }
        self.apply_position(ctx.game, &profile, reset, &mut events)?;
        self.publish(ctx.game, &profile, &ui)?;
        ctx.events_resource_mut("events")?.extend(events);
        Ok(())
    }
}

fn validate_state(game: &GameState) -> Result<(), String> {
    let level = components::world_level(&game.world)?;
    // Opaque bootstrap is intentionally not authoritative until the factory has
    // checked it against its prepared catalog. A bad source payload is recoverable.
    if !game.module_data.contains_key(PROFILE_KEY) {
        return Ok(());
    }
    let profile = profile(game)?;
    settings_valid(profile.settings)?;
    if profile.version != 1
        || profile.campaign.current() != level.id
        || profile.campaign.score_order() != ScoreOrder::LowerIsBetter
    {
        return Err("Active garden profile/world identity mismatch".into());
    }
    let position = profile
        .campaign
        .attempt()
        .map_or(&level.initial, |attempt| attempt.history.current());
    level.validate_position(position)?;
    if position.pawn != level.initial.pawn || position.crates != level.initial.crates {
        return Err("World actor positions disagree with the accepted attempt".into());
    }
    for id in game.world.entities_with(&[CELL]) {
        let data: CellData = serde_json::from_value(
            game.world
                .component(id, CELL)
                .ok_or("Missing accepted cell")?
                .clone(),
        )
        .map_err(|e| e.to_string())?;
        if game
            .world
            .object(id)
            .ok_or("Missing accepted grid object")?
            .transform
            .translation
            != level.translation(data.cell(), data.kind)?
        {
            return Err("Rendered grid transform disagrees with its cell coordinates".into());
        }
    }
    if let Some(attempt) = profile.campaign.attempt()
        && position.moves != attempt.history.undo_len() as u64
    {
        return Err("Move counter disagrees with accepted Undo history".into());
    }
    let view = view(game)?;
    if view.board != level.view(position)
        || view.moves != position.moves
        || view.settings != profile.settings
    {
        return Err("Published garden view disagrees with the accepted state".into());
    }
    if game.control.halted || game.control.transition.is_some() {
        return Err(
            "Garden menus require an active session without an external scene transition".into(),
        );
    }
    Ok(())
}
fn validate_event(event: &GameEvent) -> Result<(), String> {
    match event.name.as_str() {
        "g1.save" => {
            let profile: Profile = serde_json::from_value(
                event
                    .payload
                    .get("data")
                    .ok_or("Missing profile save payload")?
                    .clone(),
            )
            .map_err(|e| e.to_string())?;
            settings_valid(profile.settings)
        }
        "g1.storage.reset" => {
            if event.payload == json!({"confirm":true}) {
                Ok(())
            } else {
                Err("Storage reset requires explicit confirmation".into())
            }
        }
        "g1.audio" => {
            if event
                .payload
                .get("cue")
                .and_then(Value::as_str)
                .is_some_and(|cue| CUES.contains(&cue))
            {
                Ok(())
            } else {
                Err("Unknown garden audio cue".into())
            }
        }
        "g1.audio.control" => {
            if event.payload.get("command").and_then(Value::as_str) == Some("volume")
                && event
                    .payload
                    .get("bus")
                    .and_then(Value::as_str)
                    .is_some_and(|bus| ["music", "sfx"].contains(&bus))
                && event
                    .payload
                    .get("volume")
                    .and_then(Value::as_f64)
                    .is_some_and(|volume| volume.is_finite() && (0.0..=1.0).contains(&volume))
            {
                Ok(())
            } else {
                Err("Invalid garden audio control".into())
            }
        }
        "g1.moved" => {
            if event.payload.get("moves").and_then(Value::as_u64).is_some()
                && event
                    .payload
                    .get("pushed")
                    .and_then(Value::as_bool)
                    .is_some()
                && event
                    .payload
                    .get("solved")
                    .and_then(Value::as_bool)
                    .is_some()
            {
                Ok(())
            } else {
                Err("Invalid garden move event".into())
            }
        }
        _ => Ok(()),
    }
}
fn service_requests(project: &GameProject) -> Result<Vec<HostServiceRequest>, String> {
    let cues = CUES.iter().copied().map(|cue| {
        let path = project.audio_cues.get(cue).ok_or_else(|| format!("Missing garden audio cue '{cue}'"))?;
        Ok((cue.to_owned(), json!({"path":path,"bus":if cue == "music" {"music"} else {"sfx"},"looping":cue=="music","volume":1.})))
    }).collect::<Result<serde_json::Map<_, _>, String>>()?;
    let references = CUES
        .iter()
        .copied()
        .filter_map(|cue| project.audio_cues.get(cue).cloned())
        .collect();
    Ok(vec![
        HostServiceRequest {
            id: "audio.wav".into(),
            config: json!({"cues":cues,"event":"g1.audio","cue_field":"cue","control_event":"g1.audio.control","music_volume":Settings::default().music_volume,"sfx_volume":Settings::default().sfx_volume}),
            asset_references: references,
        },
        HostServiceRequest {
            id: "persistence.json".into(),
            config: json!({"namespace":"crate-garden/default","data_key":SAVED_KEY,"write_event":"g1.save","version":1,"slot":"state","payload_field":"data","invalid_save":"read_only","reset_event":"g1.storage.reset"}),
            asset_references: BTreeSet::new(),
        },
    ])
}
