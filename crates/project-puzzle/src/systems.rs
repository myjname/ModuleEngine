use crate::components::{self, CELL};
use crate::{Direction, PuzzleOutcome, frame, store_frame};
use module_engine_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    direction: Option<Direction>,
    moved: bool,
    pushed: bool,
    turns: u64,
}

fn descriptor(
    id: &str,
    ports: Vec<(&str, &str, ResourceAccess)>,
    requires: Vec<String>,
) -> SystemDescriptor {
    SystemDescriptor {
        id: id.into(),
        state_schema: format!("{id}/1"),
        components: vec![components::BOARD.into(), CELL.into()],
        ports: ports
            .into_iter()
            .map(|(name, data_type, access)| SystemPort {
                name: name.into(),
                data_type: data_type.into(),
                access,
            })
            .collect(),
        requires,
    }
}
pub(crate) fn install(registry: &mut SystemRegistry) -> Result<(), String> {
    registry.register(
        descriptor(
            "puzzle.turn",
            vec![
                ("world", "GameWorld", ResourceAccess::ReadWrite),
                ("input", "GameInput", ResourceAccess::Read),
                ("events", "GameEvents", ResourceAccess::ReadWrite),
                ("turn", "PuzzleTurn/1", ResourceAccess::Write),
            ],
            vec![],
        ),
        |params| {
            empty_params(params)?;
            Ok(Box::new(TurnSystem))
        },
    )?;
    registry.register(
        descriptor(
            "puzzle.result",
            vec![
                ("world", "GameWorld", ResourceAccess::ReadWrite),
                ("events", "GameEvents", ResourceAccess::ReadWrite),
                ("turn", "PuzzleTurn/1", ResourceAccess::Read),
            ],
            vec!["puzzle.turn".into()],
        ),
        |params| {
            empty_params(params)?;
            Ok(Box::new(ResultSystem))
        },
    )?;
    registry.require_module("puzzle.result", "puzzle.turn")
}
fn empty_params(params: &Value) -> Result<(), String> {
    if params.is_null() || params.as_object().is_some_and(|object| object.is_empty()) {
        Ok(())
    } else {
        Err("Puzzle configuration belongs to authored components".into())
    }
}
struct TurnSystem;
impl ExecutableSystem for TurnSystem {
    fn initialize(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        let layout = components::world_layout(&ctx.world("world")?.world)?;
        for (id, cell) in &layout.cells {
            *ctx.game
                .world
                .component_mut(*id, CELL)
                .ok_or("Missing puzzle cell")? = serde_json::to_value(cell).unwrap();
            ctx.game
                .world
                .transform_mut(*id)
                .ok_or("Missing puzzle transform")?
                .translation = layout.translation(cell);
        }
        let initial = frame(ctx.game)?;
        store_frame(ctx.game, initial)
    }
    fn step(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        let layout = components::world_layout(&ctx.world("world")?.world)?;
        let mut current = frame(ctx.game)?;
        let input = ctx.input_resource("input")?;
        let selected: Vec<_> = [
            Direction::North,
            Direction::East,
            Direction::South,
            Direction::West,
        ]
        .into_iter()
        .filter(|direction| input.pressed(direction.action()))
        .collect();
        let mut turn = Turn {
            turns: current.turns,
            ..Default::default()
        };
        if current.outcome != PuzzleOutcome::Playing || selected.is_empty() {
            return ctx.set_resource("turn", serde_json::to_value(turn).unwrap());
        }
        if selected.len() > 1 {
            ctx.events_resource_mut("events")?.push(GameEvent::new(
                "puzzle.blocked",
                json!({"reason":"ambiguous direction"}),
            ));
            return ctx.set_resource("turn", serde_json::to_value(turn).unwrap());
        }
        let direction = selected[0];
        turn.direction = Some(direction);
        let (dx, dz) = direction.delta();
        let pawn = &layout.cells[&layout.pawn];
        let destination = (pawn.x + dx, pawn.z + dz);
        let crate_cell = &layout.cells[&layout.crate_id];
        let push = destination == (crate_cell.x, crate_cell.z);
        let crate_destination = (crate_cell.x + dx, crate_cell.z + dz);
        if !layout.free(destination) || (push && !layout.free(crate_destination)) {
            ctx.events_resource_mut("events")?.push(GameEvent::new(
                "puzzle.blocked",
                json!({"reason":"occupied cell"}),
            ));
        } else {
            let next_turn = current
                .turns
                .checked_add(1)
                .ok_or("Puzzle turn counter overflow")?;
            let mut moved = pawn.clone();
            moved.x = destination.0;
            moved.z = destination.1;
            *ctx.game
                .world
                .component_mut(layout.pawn, CELL)
                .ok_or("Missing pawn cell")? = serde_json::to_value(&moved).unwrap();
            ctx.game
                .world
                .transform_mut(layout.pawn)
                .ok_or("Missing pawn transform")?
                .translation = layout.translation(&moved);
            if push {
                let mut moved = crate_cell.clone();
                moved.x = crate_destination.0;
                moved.z = crate_destination.1;
                *ctx.game
                    .world
                    .component_mut(layout.crate_id, CELL)
                    .ok_or("Missing crate cell")? = serde_json::to_value(&moved).unwrap();
                ctx.game
                    .world
                    .transform_mut(layout.crate_id)
                    .ok_or("Missing crate transform")?
                    .translation = layout.translation(&moved);
            }
            current.turns = next_turn;
            turn.turns = current.turns;
            turn.moved = true;
            turn.pushed = push;
            store_frame(ctx.game, current)?;
            ctx.events_resource_mut("events")?.push(GameEvent::new(
                "puzzle.moved",
                json!({"direction":direction,"turns":turn.turns,"pushed":push}),
            ));
        }
        ctx.set_resource("turn", serde_json::to_value(turn).unwrap())
    }
}
struct ResultSystem;
impl ExecutableSystem for ResultSystem {
    fn step(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        let turn: Turn = serde_json::from_value(ctx.resource("turn")?.clone())
            .map_err(|error| error.to_string())?;
        let layout = components::world_layout(&ctx.world("world")?.world)?;
        let mut current = frame(ctx.game)?;
        if turn.turns != current.turns {
            return Err("Puzzle turn resource disagrees with frame".into());
        }
        if current.outcome != PuzzleOutcome::Playing {
            return Ok(());
        }
        let crate_cell = &layout.cells[&layout.crate_id];
        let event = if (crate_cell.x, crate_cell.z) == layout.goal {
            current.outcome = PuzzleOutcome::Solved;
            Some("puzzle.solved")
        } else if current.turns >= layout.board.max_turns as u64 {
            current.outcome = PuzzleOutcome::Failed;
            Some("puzzle.failed")
        } else {
            None
        };
        if let Some(name) = event {
            ctx.game.control.halted = true;
            ctx.events_resource_mut("events")?
                .push(GameEvent::new(name, json!({"turns":current.turns})));
            store_frame(ctx.game, current)?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveEvent {
    direction: Direction,
    turns: u64,
    pushed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockedEvent {
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeEvent {
    turns: u64,
}
pub(crate) fn validate_event(event: &GameEvent) -> Result<(), String> {
    let result: Result<(), serde_json::Error> = match event.name.as_str() {
        "puzzle.moved" => serde_json::from_value::<MoveEvent>(event.payload.clone()).map(|value| {
            let _ = (value.direction, value.turns, value.pushed);
        }),
        "puzzle.blocked" => {
            serde_json::from_value::<BlockedEvent>(event.payload.clone()).map(|value| {
                let _ = value.reason;
            })
        }
        "puzzle.solved" | "puzzle.failed" => {
            serde_json::from_value::<OutcomeEvent>(event.payload.clone()).map(|value| {
                let _ = value.turns;
            })
        }
        _ => return Ok(()),
    };
    result.map_err(|error| format!("Invalid {} event: {error}", event.name))
}
