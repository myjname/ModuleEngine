//! Ordered level selection and progress; level content and attempt data stay opaque.
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScoreOrder {
    LowerIsBetter,
    HigherIsBetter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub new_best: bool,
    pub newly_unlocked: Option<String>,
}

/// One current attempt, an ordered stable level catalog and sequential progress.
/// The attempt may itself contain an `UndoJournal`; its rules are consumer-owned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Campaign<A> {
    levels: Vec<String>,
    score_order: ScoreOrder,
    unlocked: usize,
    current: usize,
    best: BTreeMap<String, u64>,
    attempt: Option<A>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CampaignData<A> {
    levels: Vec<String>,
    score_order: ScoreOrder,
    unlocked: usize,
    current: usize,
    best: BTreeMap<String, u64>,
    attempt: Option<A>,
}

impl<'de, A: Deserialize<'de>> Deserialize<'de> for Campaign<A> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let data = CampaignData::<A>::deserialize(deserializer)?;
        let result = Self {
            levels: data.levels,
            score_order: data.score_order,
            unlocked: data.unlocked,
            current: data.current,
            best: data.best,
            attempt: data.attempt,
        };
        result.validate().map_err(serde::de::Error::custom)?;
        Ok(result)
    }
}

fn validate_ids(levels: &[String]) -> Result<(), String> {
    if levels.is_empty() {
        return Err("Campaign needs at least one level".into());
    }
    let mut unique = BTreeSet::new();
    for id in levels {
        if id.is_empty()
            || id.trim() != id
            || id.chars().any(char::is_control)
            || !unique.insert(id)
        {
            return Err("Campaign level IDs must be unique nonempty stable names".into());
        }
    }
    Ok(())
}

impl<A> Campaign<A> {
    pub fn new(levels: Vec<String>, score_order: ScoreOrder) -> Result<Self, String> {
        validate_ids(&levels)?;
        Ok(Self {
            levels,
            score_order,
            unlocked: 1,
            current: 0,
            best: BTreeMap::new(),
            attempt: None,
        })
    }
    pub fn levels(&self) -> &[String] {
        &self.levels
    }
    pub fn current(&self) -> &str {
        &self.levels[self.current]
    }
    pub fn unlocked_count(&self) -> usize {
        self.unlocked
    }
    pub fn score_order(&self) -> ScoreOrder {
        self.score_order
    }
    pub fn is_unlocked(&self, level: &str) -> bool {
        self.levels[..self.unlocked].iter().any(|id| id == level)
    }
    pub fn best(&self, level: &str) -> Option<u64> {
        self.best.get(level).copied()
    }
    pub fn attempt(&self) -> Option<&A> {
        self.attempt.as_ref()
    }
    pub fn set_attempt(&mut self, attempt: A) {
        self.attempt = Some(attempt);
    }
    pub fn clear_attempt(&mut self) {
        self.attempt = None;
    }
    /// Selecting the same level preserves its current attempt. Selecting another
    /// unlocked level discards that single current attempt, preserving progress.
    pub fn select(&mut self, level: &str) -> Result<(), String> {
        let index = self
            .levels
            .iter()
            .position(|id| id == level)
            .ok_or_else(|| format!("Unknown campaign level '{level}'"))?;
        if index >= self.unlocked {
            return Err(format!("Campaign level '{level}' is locked"));
        }
        if self.current != index {
            self.current = index;
            self.attempt = None;
        }
        Ok(())
    }
    /// Record a successful result without automatically navigating. Replaying an
    /// earlier level cannot unlock beyond its successor or worsen a best score.
    /// The completed attempt is retained until the consumer clears/replaces it.
    pub fn complete(&mut self, score: u64) -> Completion {
        let level = self.current().to_owned();
        let new_best = self
            .best(&level)
            .is_none_or(|previous| match self.score_order {
                ScoreOrder::LowerIsBetter => score < previous,
                ScoreOrder::HigherIsBetter => score > previous,
            });
        if new_best {
            self.best.insert(level, score);
        }
        let newly_unlocked =
            if self.current + 1 == self.unlocked && self.unlocked < self.levels.len() {
                let next = self.levels[self.unlocked].clone();
                self.unlocked += 1;
                Some(next)
            } else {
                None
            };
        Completion {
            new_best,
            newly_unlocked,
        }
    }
    /// A save cannot replace or reorder the authored catalog. Content changes
    /// require an explicit consumer migration rather than accidental remapping.
    pub fn validate_catalog(&self, authored_levels: &[String]) -> Result<(), String> {
        if self.levels == authored_levels {
            Ok(())
        } else {
            Err("Saved campaign level catalog differs from authored content".into())
        }
    }
    pub fn validate_attempt(
        &self,
        validate: impl Fn(&A) -> Result<(), String>,
    ) -> Result<(), String> {
        self.attempt.as_ref().map_or(Ok(()), validate)
    }
    fn validate(&self) -> Result<(), String> {
        validate_ids(&self.levels)?;
        let completed = self
            .levels
            .iter()
            .take_while(|id| self.best.contains_key(*id))
            .count();
        if completed != self.best.len() {
            return Err("Campaign scores must name a completed sequential prefix of levels".into());
        }
        let expected_unlocked = (completed + 1).min(self.levels.len());
        if self.unlocked != expected_unlocked || self.current >= self.unlocked {
            return Err("Campaign unlock/current state disagrees with completed levels".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn levels() -> Vec<String> {
        ["first", "second", "third"].map(str::to_owned).to_vec()
    }

    #[test]
    fn move_scores_unlock_in_order_replay_and_preserve_attempt_in_save() {
        let mut campaign = Campaign::new(levels(), ScoreOrder::LowerIsBetter).unwrap();
        assert!(campaign.select("third").is_err());
        campaign.set_attempt(vec!["north", "east"]);
        campaign.select("first").unwrap();
        assert!(campaign.attempt().is_some());
        assert_eq!(
            campaign.complete(12).newly_unlocked.as_deref(),
            Some("second")
        );
        assert!(!campaign.complete(14).new_best);
        assert!(campaign.complete(10).new_best);
        assert_eq!(campaign.best("first"), Some(10));
        assert!(!campaign.is_unlocked("third"));
        campaign.select("second").unwrap();
        assert!(campaign.attempt().is_none());
        campaign.set_attempt(vec!["west"]);
        let saved = serde_json::to_string(&campaign).unwrap();
        let loaded: Campaign<Vec<String>> = serde_json::from_str(&saved).unwrap();
        loaded.validate_catalog(&levels()).unwrap();
        assert_eq!(loaded.current(), "second");
        assert_eq!(loaded.attempt().unwrap(), &vec!["west".to_owned()]);
    }

    #[test]
    fn racing_consumer_uses_high_score_order_and_finishes_last_stage() {
        let mut season: Campaign<u64> =
            Campaign::new(levels(), ScoreOrder::HigherIsBetter).unwrap();
        for id in levels() {
            season.select(&id).unwrap();
            assert!(season.complete(100).new_best);
            assert!(!season.complete(90).new_best);
            assert!(season.complete(110).new_best);
        }
        assert_eq!(season.unlocked_count(), 3);
        assert!(season.complete(u64::MAX).newly_unlocked.is_none());
        assert_eq!(season.best("third"), Some(u64::MAX));
        let restored: Campaign<u64> =
            serde_json::from_value(serde_json::to_value(&season).unwrap()).unwrap();
        assert_eq!(restored, season);
    }

    #[test]
    fn invalid_saved_progress_and_catalog_changes_are_rejected() {
        let state: Campaign<()> = Campaign::new(levels(), ScoreOrder::LowerIsBetter).unwrap();
        for (key, value) in [
            ("unlocked", serde_json::json!(3)),
            ("current", serde_json::json!(1)),
            ("levels", serde_json::json!(["same", "same"])),
            ("best", serde_json::json!({"second": 1})),
            ("best", serde_json::json!({"foreign": 1})),
        ] {
            let mut malformed = serde_json::to_value(&state).unwrap();
            malformed[key] = value;
            assert!(
                serde_json::from_value::<Campaign<()>>(malformed).is_err(),
                "{key}"
            );
        }
        let mut reordered = levels();
        reordered.swap(0, 1);
        assert!(state.validate_catalog(&reordered).is_err());
        assert!(Campaign::<()>::new(vec![], ScoreOrder::LowerIsBetter).is_err());
    }
}
