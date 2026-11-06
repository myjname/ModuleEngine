//! Serializable accepted-state history, independent of an ECS or game rules.
use serde::{Deserialize, Serialize};

/// The current accepted state and all previous accepted states, oldest first.
/// There is no fixed history length. Allocation and serialized input limits are
/// the consumer/storage boundary's responsibility.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoJournal<T> {
    current: T,
    past: Vec<T>,
}

impl<T> UndoJournal<T> {
    pub fn new(initial: T) -> Self {
        Self {
            current: initial,
            past: Vec::new(),
        }
    }
    pub fn current(&self) -> &T {
        &self.current
    }
    pub fn undo_len(&self) -> usize {
        self.past.len()
    }
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }
    /// Store only a new accepted state; equal/no-op candidates add no history.
    pub fn commit(&mut self, next: T) -> bool
    where
        T: PartialEq,
    {
        if next == self.current {
            return false;
        }
        self.past.push(std::mem::replace(&mut self.current, next));
        true
    }
    pub fn undo(&mut self) -> Option<&T> {
        self.current = self.past.pop()?;
        Some(&self.current)
    }
    pub fn reset(&mut self, initial: T) {
        self.current = initial;
        self.past.clear();
    }
    /// Validate every restored state before accepting an imported journal.
    /// Deserialization alone cannot know the consumer's state contract.
    pub fn validate(&self, validate: impl Fn(&T) -> Result<(), String>) -> Result<(), String> {
        validate(&self.current).map_err(|error| format!("Current history state: {error}"))?;
        for (index, state) in self.past.iter().enumerate() {
            validate(state).map_err(|error| format!("History state {index}: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_turns_round_trip_and_undo_without_recording_blocked_moves() {
        let mut turns = UndoJournal::new((0_i32, 0_u64));
        assert!(!turns.commit((0, 0)));
        for n in 1..=2048 {
            assert!(turns.commit((n, n as u64)));
        }
        let saved = serde_json::to_vec(&turns).unwrap();
        let mut loaded: UndoJournal<(i32, u64)> = serde_json::from_slice(&saved).unwrap();
        loaded
            .validate(|(position, moves)| {
                if *position as u64 == *moves {
                    Ok(())
                } else {
                    Err("Inconsistent turn".into())
                }
            })
            .unwrap();
        assert_eq!(loaded.undo_len(), 2048);
        for n in (0..2048).rev() {
            assert_eq!(loaded.undo(), Some(&(n, n as u64)));
        }
        assert_eq!(loaded.undo(), None);
        assert_eq!(loaded.current(), &(0, 0));
    }

    #[test]
    fn drawing_document_consumer_branches_after_undo_and_reset_starts_new_history() {
        let mut drawing = UndoJournal::new(vec!["background".to_owned()]);
        drawing.commit(vec!["background".into(), "circle".into()]);
        drawing.commit(vec!["background".into(), "circle".into(), "text".into()]);
        drawing.undo();
        drawing.commit(vec!["background".into(), "circle".into(), "line".into()]);
        assert_eq!(drawing.undo_len(), 2);
        assert_eq!(
            drawing.undo().unwrap(),
            &vec!["background".to_owned(), "circle".to_owned()]
        );
        drawing.reset(vec![]);
        assert!(!drawing.can_undo());
        assert!(drawing.current().is_empty());
    }

    #[test]
    fn consumer_validation_checks_old_entries_too() {
        let journal: UndoJournal<i32> =
            serde_json::from_str(r#"{"current":2,"past":[-1,1]}"#).unwrap();
        assert!(
            journal
                .validate(|n| if *n >= 0 {
                    Ok(())
                } else {
                    Err("Negative".into())
                })
                .unwrap_err()
                .contains("History state 0")
        );
    }
}
