use super::*;

/// Portable events read by game UI, audio and custom modules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ArpgEvent {
    Attack {
        attacker: u64,
    },
    Hit {
        attacker: u64,
        target: u64,
        damage: f32,
    },
    Death {
        object: u64,
    },
    SceneChanged {
        from: String,
        to: String,
    },
    Victory,
    Sound {
        cue: String,
        object: Option<u64>,
    },
    Custom {
        name: String,
        payload: Value,
    },
}

impl From<ArpgEvent> for GameEvent {
    fn from(event: ArpgEvent) -> Self {
        serde_json::from_value(serde_json::to_value(event).expect("ARPG event serialization"))
            .expect("Generic event accepts an externally tagged event")
    }
}
impl TryFrom<&GameEvent> for ArpgEvent {
    type Error = String;
    fn try_from(event: &GameEvent) -> Result<Self, Self::Error> {
        serde_json::from_value(serde_json::to_value(event).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
}
impl TryFrom<GameEvent> for ArpgEvent {
    type Error = String;
    fn try_from(event: GameEvent) -> Result<Self, Self::Error> {
        Self::try_from(&event)
    }
}

/// Validate this package's event contracts while allowing unrelated module
/// events to retain their own schemas.
pub fn validate_event(event: &GameEvent) -> Result<(), String> {
    match event.name.as_str() {
        "Attack" | "Hit" | "Death" | "SceneChanged" | "Victory" | "Sound" | "Custom" => {
            ArpgEvent::try_from(event)
                .map(|_| ())
                .map_err(|error| format!("Invalid ARPG {} event: {error}", event.name))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_events_preserve_the_established_wire_shape() {
        for event in [
            ArpgEvent::Attack { attacker: 7 },
            ArpgEvent::Hit {
                attacker: 7,
                target: 8,
                damage: 12.,
            },
            ArpgEvent::Death { object: 8 },
            ArpgEvent::SceneChanged {
                from: "arena".into(),
                to: "boss".into(),
            },
            ArpgEvent::Victory,
            ArpgEvent::Sound {
                cue: "hit".into(),
                object: Some(8),
            },
            ArpgEvent::Custom {
                name: "source".into(),
                payload: json!({"count":2}),
            },
        ] {
            let generic: GameEvent = event.clone().into();
            assert_eq!(
                serde_json::to_value(&generic).unwrap(),
                serde_json::to_value(&event).unwrap()
            );
            assert_eq!(ArpgEvent::try_from(&generic).unwrap(), event);
        }
        let unrelated = GameEvent::new("puzzle.opened", json!({"door":9}));
        assert!(ArpgEvent::try_from(&unrelated).is_err());
        assert_eq!(unrelated.name, "puzzle.opened");
    }

    #[test]
    fn known_event_contracts_reject_malformed_payloads_and_allow_other_modules() {
        for event in [
            GameEvent::new("Attack", json!({"attacker":"invalid"})),
            GameEvent::new("Hit", json!({"attacker":1,"target":2})),
            GameEvent::new("Death", json!({"object":null})),
            GameEvent::new("SceneChanged", json!({"from":"arena","to":3})),
            GameEvent::new("Victory", json!({"unexpected":true})),
            GameEvent::new("Sound", json!({"cue":3,"object":1})),
            GameEvent::new("Custom", json!({"name":3,"payload":{}})),
        ] {
            assert!(validate_event(&event).is_err(), "accepted {}", event.name);
        }
        assert!(
            validate_event(
                &ArpgEvent::Sound {
                    cue: "hit".into(),
                    object: Some(2)
                }
                .into()
            )
            .is_ok()
        );
        assert!(validate_event(&GameEvent::new("puzzle.opened", json!({"cue":3}))).is_ok());
    }
}
