use super::*;
use crate::components::{Actor, Health, Melee};
use crate::state::read_frame;

pub trait GameStateExt {
    fn arpg(&self) -> ArpgFrame;
    fn set_arpg(&mut self, frame: ArpgFrame) -> Result<(), String>;
    fn player(&self) -> Option<GameEntity>;
    fn living_enemies(&self) -> usize;
    fn melee_aim(&self, attacker: u64, aim_world: [f32; 3]) -> Option<MeleeArea>;
    fn melee_targets(&self, attacker: u64, area: &MeleeArea) -> Vec<u64>;
}
impl GameStateExt for GameState {
    fn arpg(&self) -> ArpgFrame {
        read_frame(self).unwrap_or_default()
    }
    fn set_arpg(&mut self, frame: ArpgFrame) -> Result<(), String> {
        self.module_data.insert(
            FRAME_KEY.into(),
            serde_json::to_value(frame).map_err(|e| e.to_string())?,
        );
        Ok(())
    }
    fn player(&self) -> Option<GameEntity> {
        self.world
            .entities_with(&["player"])
            .first()
            .and_then(|id| self.world.entity(*id))
    }
    fn living_enemies(&self) -> usize {
        self.world
            .entities_with(&["actor"])
            .into_iter()
            .filter(|id| {
                world_component::<Actor>(&self.world, *id, "actor")
                    .is_some_and(|a| a.team == "enemy")
                    && world_component::<Health>(&self.world, *id, "health")
                        .is_none_or(|health| health.current > 0.)
            })
            .count()
    }
    /// Prospective attack region, using the same aim fallback as a real swing.
    fn melee_aim(&self, attacker: u64, aim_world: [f32; 3]) -> Option<MeleeArea> {
        world_component::<Actor>(&self.world, attacker, "actor")?;
        let melee = world_component::<Melee>(&self.world, attacker, "melee")?;
        if world_component::<Health>(&self.world, attacker, "health")
            .is_some_and(|health| health.current <= 0.)
        {
            return None;
        }
        let center = self.world.object(attacker)?.transform.translation;
        Some(MeleeArea {
            center,
            direction: attack_direction(center, aim_world),
            range: melee.range,
            arc: melee.arc,
        })
    }
    /// Living opposing targets eligible for a fresh swing in the supplied area.
    fn melee_targets(&self, attacker: u64, area: &MeleeArea) -> Vec<u64> {
        let Some(actor) = world_component::<Actor>(&self.world, attacker, "actor") else {
            return vec![];
        };
        self.world
            .entities_with(&["actor", "health"])
            .into_iter()
            .filter_map(|id| {
                let other = world_component::<Actor>(&self.world, id, "actor")?;
                let hp = world_component::<Health>(&self.world, id, "health")?;
                (id != attacker
                    && other.team != actor.team
                    && hp.current > 0.
                    && area.contains(self.world.object(id)?.transform.translation, other.radius))
                .then_some(id)
            })
            .collect()
    }
}

fn world_component<T: for<'de> Deserialize<'de>>(
    world: &EcsWorld,
    id: u64,
    name: &str,
) -> Option<T> {
    serde_json::from_value(world.component(id, name)?.clone()).ok()
}
pub(crate) fn normalized(v: [f32; 2]) -> [f32; 2] {
    let length = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if length > 1e-6 {
        [v[0] / length, v[1] / length]
    } else {
        [0., 0.]
    }
}
pub(crate) fn attack_direction(center: [f32; 3], aim: [f32; 3]) -> [f32; 2] {
    let direction = normalized([aim[0] - center[0], aim[2] - center[2]]);
    if direction == [0., 0.] {
        [0., -1.]
    } else {
        direction
    }
}
pub(crate) fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
