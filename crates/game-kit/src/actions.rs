//! Named actions and physical edge tracking without a windowing dependency.
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    Pressed,
    Held,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub key: String,
    pub action: String,
    pub trigger: Trigger,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ActionMap {
    actions: Vec<String>,
    bindings: Vec<Binding>,
}

impl<'de> Deserialize<'de> for ActionMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Data {
            actions: Vec<String>,
            bindings: Vec<Binding>,
        }
        let data = Data::deserialize(deserializer)?;
        Self::new(data.actions, data.bindings).map_err(serde::de::Error::custom)
    }
}

/// The adapter reports platform repeats explicitly. Repeats can restore held
/// state after focus returns, but never manufacture a fresh edge action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhysicalEvent {
    Pressed(String),
    Released(String),
    Repeated(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionState {
    down: BTreeSet<String>,
}
impl ActionState {
    pub fn reset(&mut self) {
        self.down.clear();
    }
}

/// Pressed actions retain physical event order. Distinct fresh presses can
/// produce the same action more than once in a frame; the consumer chooses its
/// queue/one-action-per-tick policy. Held actions are deduplicated named values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionFrame {
    pub pressed: Vec<String>,
    pub held: BTreeSet<String>,
}
impl ActionFrame {
    pub fn has_input(&self) -> bool {
        !self.pressed.is_empty() || !self.held.is_empty()
    }
    pub fn contains(&self, action: &str) -> bool {
        self.pressed.iter().any(|name| name == action) || self.held.contains(action)
    }
    pub fn extend(&mut self, other: Self) {
        self.pressed.extend(other.pressed);
        self.held.extend(other.held);
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.trim() == name && !name.chars().any(char::is_control)
}
impl ActionMap {
    /// Physical key names are opaque stable strings supplied by the platform
    /// adapter. One key has one binding; multiple keys may share one action.
    /// Declared actions without a key remain available to on-screen controls.
    pub fn new(actions: Vec<String>, bindings: Vec<Binding>) -> Result<Self, String> {
        let mut declared = BTreeSet::new();
        for action in &actions {
            if !valid_name(action) || !declared.insert(action) {
                return Err("Action names must be unique nonempty stable names".into());
            }
        }
        let mut keys = BTreeSet::new();
        for binding in &bindings {
            if !valid_name(&binding.key) || !keys.insert(&binding.key) {
                return Err("Physical keys must have unique nonempty bindings".into());
            }
            if !declared.contains(&binding.action) {
                return Err(format!(
                    "Binding references undeclared action '{}'",
                    binding.action
                ));
            }
        }
        Ok(Self { actions, bindings })
    }
    pub fn actions(&self) -> &[String] {
        &self.actions
    }
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
    pub fn evaluate(
        &self,
        state: &mut ActionState,
        events: &[PhysicalEvent],
        focused: bool,
    ) -> ActionFrame {
        if !focused {
            state.reset();
            return ActionFrame::default();
        }
        // Removed bindings cannot leave stale keys behind. Consumers replacing
        // mappings can explicitly reset state to discard all held keys too.
        state
            .down
            .retain(|key| self.bindings.iter().any(|binding| &binding.key == key));
        let mut frame = ActionFrame::default();
        for event in events {
            let (key, fresh) = match event {
                PhysicalEvent::Pressed(key) => (key, true),
                PhysicalEvent::Repeated(key) => (key, false),
                PhysicalEvent::Released(key) => {
                    state.down.remove(key);
                    continue;
                }
            };
            let Some(binding) = self.bindings.iter().find(|binding| &binding.key == key) else {
                continue;
            };
            let newly_down = state.down.insert(key.clone());
            if fresh && newly_down && binding.trigger == Trigger::Pressed {
                frame.pressed.push(binding.action.clone());
            }
        }
        for binding in &self.bindings {
            if binding.trigger == Trigger::Held && state.down.contains(&binding.key) {
                frame.held.insert(binding.action.clone());
            }
        }
        frame
    }
    /// Buttons use the same validated action namespace as physical bindings.
    /// This method creates one pressed action and does not mutate held keys.
    pub fn named_press(&self, action: &str) -> Result<ActionFrame, String> {
        if !self.actions.iter().any(|name| name == action) {
            return Err(format!("Unknown named action '{action}'"));
        }
        Ok(ActionFrame {
            pressed: vec![action.into()],
            held: BTreeSet::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding(key: &str, action: &str, trigger: Trigger) -> Binding {
        Binding {
            key: key.into(),
            action: action.into(),
            trigger,
        }
    }

    #[test]
    fn turn_controls_preserve_event_order_ignore_repeat_and_match_screen_button() {
        let map = ActionMap::new(
            vec!["north".into(), "east".into(), "undo".into()],
            vec![
                binding("ArrowUp", "north", Trigger::Pressed),
                binding("W", "north", Trigger::Pressed),
                binding("ArrowRight", "east", Trigger::Pressed),
            ],
        )
        .unwrap();
        let mut state = ActionState::default();
        let frame = map.evaluate(
            &mut state,
            &[
                PhysicalEvent::Pressed("ArrowRight".into()),
                PhysicalEvent::Pressed("W".into()),
                PhysicalEvent::Repeated("W".into()),
                PhysicalEvent::Pressed("W".into()),
            ],
            true,
        );
        assert_eq!(frame.pressed, ["east", "north"]);
        assert!(!map.evaluate(&mut state, &[], true).has_input());
        let frame = map.evaluate(
            &mut state,
            &[
                PhysicalEvent::Released("W".into()),
                PhysicalEvent::Pressed("W".into()),
            ],
            true,
        );
        assert_eq!(frame, map.named_press("north").unwrap());
        assert!(map.named_press("undo").unwrap().contains("undo"));
        assert!(map.named_press("foreign").is_err());
    }

    #[test]
    fn flight_consumer_holds_thrust_with_aliases_and_handles_focus_loss() {
        let map = ActionMap::new(
            vec!["thrust".into(), "camera".into()],
            vec![
                binding("W", "thrust", Trigger::Held),
                binding("ArrowUp", "thrust", Trigger::Held),
                binding("C", "camera", Trigger::Pressed),
            ],
        )
        .unwrap();
        let mut state = ActionState::default();
        let frame = map.evaluate(
            &mut state,
            &[
                PhysicalEvent::Pressed("W".into()),
                PhysicalEvent::Pressed("ArrowUp".into()),
                PhysicalEvent::Pressed("C".into()),
            ],
            true,
        );
        assert_eq!(frame.held.len(), 1);
        assert_eq!(frame.pressed, ["camera"]);
        assert!(
            map.evaluate(&mut state, &[PhysicalEvent::Released("W".into())], true)
                .contains("thrust")
        );
        assert!(!map.evaluate(&mut state, &[], false).has_input());
        assert!(!map.evaluate(&mut state, &[], true).has_input());
        let frame = map.evaluate(
            &mut state,
            &[
                PhysicalEvent::Repeated("C".into()),
                PhysicalEvent::Repeated("W".into()),
            ],
            true,
        );
        assert!(frame.pressed.is_empty());
        assert!(frame.contains("thrust"));
    }

    #[test]
    fn malformed_bindings_reject_before_publication_and_serialization_keeps_policy() {
        assert!(ActionMap::new(vec!["move".into(), "move".into()], vec![]).is_err());
        assert!(
            ActionMap::new(
                vec!["move".into()],
                vec![binding("W", "foreign", Trigger::Pressed)]
            )
            .is_err()
        );
        assert!(
            ActionMap::new(
                vec!["move".into()],
                vec![
                    binding("W", "move", Trigger::Pressed),
                    binding("W", "move", Trigger::Held)
                ]
            )
            .is_err()
        );
        let map = ActionMap::new(
            vec!["move".into()],
            vec![binding("W", "move", Trigger::Pressed)],
        )
        .unwrap();
        assert_eq!(
            serde_json::from_value::<ActionMap>(serde_json::to_value(&map).unwrap()).unwrap(),
            map
        );
        let mut invalid = serde_json::to_value(&map).unwrap();
        invalid["bindings"][0]["action"] = "foreign".into();
        assert!(serde_json::from_value::<ActionMap>(invalid).is_err());
    }
}
