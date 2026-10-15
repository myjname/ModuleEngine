use super::*;
use crate::queries::{distance, normalized};

pub const FRAME_KEY: &str = "arpg.frame/1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameStatus {
    #[default]
    Playing,
    Defeat,
    Victory,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameIntent {
    pub movement: [f32; 2],
    pub aim: [f32; 3],
    pub attack: bool,
    #[serde(default)]
    pub lifetime: Option<u64>,
}

impl PartialEq for GameIntent {
    fn eq(&self, other: &Self) -> bool {
        self.movement == other.movement && self.aim == other.aim && self.attack == other.attack
    }
}

/// World-space telegraph for the UI/renderer. Filled from actual combat state.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttackTelegraph {
    pub attacker: u64,
    #[serde(default)]
    pub attacker_lifetime: Option<u64>,
    #[serde(default)]
    pub attacker_player: Option<bool>,
    pub center: [f32; 3],
    pub direction: [f32; 2],
    pub range: f32,
    pub arc: f32,
    pub progress: f32,
}

impl PartialEq for AttackTelegraph {
    fn eq(&self, other: &Self) -> bool {
        self.attacker == other.attacker
            && self.attacker_player == other.attacker_player
            && self.center == other.center
            && self.direction == other.direction
            && self.range == other.range
            && self.arc == other.arc
            && self.progress == other.progress
    }
}

/// The exact world-space melee region used by combat and an aim preview.
///
/// Target circles extend radial reach by their radius. The angular test uses
/// the target's center; it does not widen the sector by the target's radius.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeleeArea {
    pub center: [f32; 3],
    pub direction: [f32; 2],
    pub range: f32,
    pub arc: f32,
}

impl MeleeArea {
    /// Keep presentation and damage eligibility on the same XZ-plane test.
    pub fn contains(&self, position: [f32; 3], radius: f32) -> bool {
        let delta = normalized([position[0] - self.center[0], position[2] - self.center[2]]);
        let dot = delta[0] * self.direction[0] + delta[1] * self.direction[1];
        distance(self.center, position) <= self.range + radius
            && (self.arc >= std::f32::consts::TAU || dot >= (self.arc * 0.5).cos())
    }
}

/// Presentation of an actual damage-active swing, rather than a pressed edge.
/// Direction is captured when the attack starts and progress spans its duration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttackVisual {
    pub attacker: u64,
    #[serde(default)]
    pub attacker_lifetime: Option<u64>,
    #[serde(default)]
    pub attacker_player: Option<bool>,
    pub area: MeleeArea,
    pub progress: f32,
}

impl PartialEq for AttackVisual {
    fn eq(&self, other: &Self) -> bool {
        self.attacker == other.attacker
            && self.attacker_player == other.attacker_player
            && self.area == other.area
            && self.progress == other.progress
    }
}

/// A confirmed hit retained for 0.35 simulated seconds, including lethal hits.
/// Position is captured before the health system can remove the target.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HitVisual {
    pub attacker: u64,
    pub target: u64,
    /// Runtime incarnation prevents an old hit flashing a replacement object.
    #[serde(default)]
    pub target_lifetime: Option<u64>,
    #[serde(default)]
    pub target_player: Option<bool>,
    pub position: [f32; 3],
    pub damage: f32,
    pub started_at: f64,
    pub expires_at: f64,
}

impl PartialEq for HitVisual {
    fn eq(&self, other: &Self) -> bool {
        self.attacker == other.attacker
            && self.target == other.target
            && self.target_player == other.target_player
            && self.position == other.position
            && self.damage == other.damage
            && self.started_at == other.started_at
            && self.expires_at == other.expires_at
    }
}

/// Portable data owned by the optional ARPG package, stored in module_data.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArpgFrame {
    #[serde(default)]
    pub status: GameStatus,
    #[serde(default)]
    pub intentions: BTreeMap<u64, GameIntent>,
    #[serde(default)]
    pub telegraphs: Vec<AttackTelegraph>,
    #[serde(default)]
    pub attack_visuals: Vec<AttackVisual>,
    #[serde(default)]
    pub hit_visuals: Vec<HitVisual>,
    #[serde(default)]
    prepared_tick: Option<u64>,
}
impl ArpgFrame {
    pub(crate) fn ensure_tick(&mut self, game: &GameState) {
        if self.prepared_tick == Some(game.tick) {
            return;
        }
        self.intentions.clear();
        self.telegraphs.clear();
        self.attack_visuals.clear();
        self.hit_visuals.retain(|hit| hit.expires_at > game.time);
        self.prepared_tick = Some(game.tick);
    }
}
pub(crate) fn read_frame(game: &GameState) -> Result<ArpgFrame, String> {
    game.module_data.get(FRAME_KEY).map_or_else(
        || Ok(ArpgFrame::default()),
        |value| {
            serde_json::from_value(value.clone())
                .map_err(|error| format!("Invalid {FRAME_KEY}: {error}"))
        },
    )
}

pub(crate) fn begin_tick(game: &mut GameState) -> Result<(), String> {
    let mut frame = read_frame(game)?;
    frame.ensure_tick(game);
    game.set_arpg(frame)
}

/// Adapter for hosts that collect the original ARPG controls.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ArpgInput {
    pub movement: [f32; 2],
    pub aim_world: [f32; 3],
    pub attack: bool,
    pub interact: bool,
    pub restart: bool,
}
impl ArpgInput {
    pub fn into_input(self) -> GameInput {
        self.into()
    }
}
impl From<ArpgInput> for GameInput {
    fn from(input: ArpgInput) -> Self {
        let mut actions = BTreeSet::new();
        if input.attack {
            actions.insert("attack".into());
        }
        if input.interact {
            actions.insert("interact".into());
        }
        let action_values = if actions.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([("aim_world".into(), json!(input.aim_world))])
        };
        Self {
            values: BTreeMap::from([
                ("movement".into(), json!(input.movement)),
                ("aim_world".into(), json!(input.aim_world)),
            ]),
            actions,
            action_values,
            restart: input.restart,
        }
    }
}
pub trait GameInputExt {
    fn movement(&self) -> [f32; 2];
    fn aim_world(&self) -> [f32; 3];
    fn attack(&self) -> bool;
    fn interact(&self) -> bool;
    fn set_movement(&mut self, value: [f32; 2]);
    fn set_aim_world(&mut self, value: [f32; 3]);
    fn set_attack(&mut self, pressed: bool);
    fn set_interact(&mut self, pressed: bool);
}
impl GameInputExt for GameInput {
    fn movement(&self) -> [f32; 2] {
        self.values
            .get("movement")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
    fn aim_world(&self) -> [f32; 3] {
        self.action_values
            .get("aim_world")
            .or_else(|| self.values.get("aim_world"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
    fn attack(&self) -> bool {
        self.pressed("attack")
    }
    fn interact(&self) -> bool {
        self.pressed("interact")
    }
    fn set_movement(&mut self, value: [f32; 2]) {
        self.values.insert("movement".into(), json!(value));
    }
    fn set_aim_world(&mut self, value: [f32; 3]) {
        self.values.insert("aim_world".into(), json!(value));
        if !self.actions.is_empty() {
            self.action_values.insert("aim_world".into(), json!(value));
        }
    }
    fn set_attack(&mut self, pressed: bool) {
        if pressed {
            self.actions.insert("attack".into());
            capture_aim(self);
        } else {
            self.actions.remove("attack");
            if self.actions.is_empty() {
                self.action_values.remove("aim_world");
            }
        }
    }
    fn set_interact(&mut self, pressed: bool) {
        if pressed {
            self.actions.insert("interact".into());
            capture_aim(self);
        } else {
            self.actions.remove("interact");
            if self.actions.is_empty() {
                self.action_values.remove("aim_world");
            }
        }
    }
}

fn capture_aim(input: &mut GameInput) {
    let aim = input
        .values
        .get("aim_world")
        .cloned()
        .unwrap_or_else(|| json!([0., 0., 0.]));
    input.action_values.insert("aim_world".into(), aim);
}
/// Preserve the established custom-script input packet at the package boundary.
pub fn legacy_input(input: &GameInput) -> Value {
    json!({"movement":input.movement(), "aim_world":input.aim_world(), "attack":input.attack(), "interact":input.interact(), "restart":input.restart})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> GameState {
        GameState {
            scene: "source".into(),
            time: 0.,
            tick: 0,
            world: EcsWorld::from_objects(vec![]).unwrap(),
            module_data: BTreeMap::from([("another.module/1".into(), json!({"retained":7}))]),
            control: SessionControl::default(),
        }
    }

    #[test]
    fn frame_prepares_transients_once_per_tick_and_preserves_other_modules() {
        let mut game = state();
        let mut frame = game.arpg();
        frame.intentions.insert(1, GameIntent::default());
        frame.hit_visuals.push(HitVisual {
            attacker: 1,
            target: 2,
            target_lifetime: None,
            target_player: None,
            position: [0.; 3],
            damage: 1.,
            started_at: 0.,
            expires_at: 0.35,
        });
        frame.ensure_tick(&game);
        assert!(frame.intentions.is_empty());
        assert_eq!(frame.hit_visuals.len(), 1);
        frame.intentions.insert(1, GameIntent::default());
        game.set_arpg(frame).unwrap();
        let mut next_node = read_frame(&game).unwrap();
        next_node.ensure_tick(&game);
        assert_eq!(next_node.intentions.len(), 1);
        game.tick += 1;
        game.time = 0.4;
        next_node.ensure_tick(&game);
        assert!(next_node.intentions.is_empty());
        assert!(next_node.hit_visuals.is_empty());
        game.set_arpg(next_node).unwrap();
        assert_eq!(game.module_data["another.module/1"], json!({"retained":7}));
        game.module_data
            .insert(FRAME_KEY.into(), json!("malformed"));
        assert!(read_frame(&game).unwrap_err().contains(FRAME_KEY));
    }

    #[test]
    fn frame_without_combat_visual_fields_uses_compatible_defaults() {
        let frame: ArpgFrame = serde_json::from_value(json!({
            "status":"Playing", "intentions":{}, "telegraphs":[]
        }))
        .unwrap();
        assert!(frame.attack_visuals.is_empty());
        assert!(frame.hit_visuals.is_empty());
        assert_eq!(frame.status, GameStatus::Playing);
    }

    #[test]
    fn input_adapters_preserve_edges_and_the_legacy_script_packet() {
        let native = ArpgInput {
            movement: [0.5, -1.],
            aim_world: [3., 0., 7.],
            attack: true,
            interact: true,
            restart: true,
        };
        let expected = serde_json::to_value(&native).unwrap();
        let mut input = native.into_input();
        input.actions.insert("puzzle.use".into());
        input.values.insert("puzzle.value".into(), json!(42));
        assert_eq!(legacy_input(&input), expected);
        input.set_attack(false);
        input.set_interact(false);
        input.set_movement([1., 0.]);
        input.set_aim_world([4., 0., 8.]);
        assert!(!input.attack());
        assert!(!input.interact());
        assert!(input.pressed("puzzle.use"));
        assert_eq!(input.movement(), [1., 0.]);
        assert_eq!(input.aim_world(), [4., 0., 8.]);
        assert_eq!(input.values["puzzle.value"], json!(42));
    }

    #[test]
    fn pressed_aim_context_survives_latest_held_values() {
        let mut input = ArpgInput {
            aim_world: [1., 0., 2.],
            attack: true,
            ..Default::default()
        }
        .into_input();
        input.values.insert("aim_world".into(), json!([8., 0., 9.]));
        input.set_movement([1., 0.]);
        assert_eq!(input.aim_world(), [1., 0., 2.]);
        assert_eq!(input.movement(), [1., 0.]);
        input.set_attack(false);
        assert_eq!(input.aim_world(), [8., 0., 9.]);
        input.set_interact(true);
        assert_eq!(input.action_values["aim_world"], json!([8., 0., 9.]));
        input.set_aim_world([3., 0., 4.]);
        assert_eq!(input.action_values["aim_world"], json!([3., 0., 4.]));
        input.set_interact(false);
        assert!(input.action_values.is_empty());
    }
}
