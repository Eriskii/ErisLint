// Copyright (C) 2026 Eriskii
// SPDX-License-Identifier: AGPL-3.0-only
// See LICENSE for the full license text.

use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A probability or confidence in the closed interval [0, 1].
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "f64")]
#[schemars(!try_from)]
pub struct Probability(#[schemars(range(min = 0.0, max = 1.0))] f64);

impl Probability {
    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Probability {
    type Error = &'static str;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        (value.is_finite() && (0.0..=1.0).contains(&value))
            .then_some(Self(value))
            .ok_or("probability/confidence must be a finite number between 0 and 1")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticPolicy {
    pub when: Condition,
    pub level: Level,
    pub message: String,
}

/// Fields are ANDed. `any` and `all` compose nested conditions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<Probability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_confidence: Option<Probability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probability: Option<ChoiceProbability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<Vec<Condition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Condition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChoiceProbability {
    pub choice: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Probability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Probability>,
}

fn in_bounds(value: Probability, min: Option<Probability>, max: Option<Probability>) -> bool {
    min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
}

fn ordered(min: Option<Probability>, max: Option<Probability>) -> bool {
    min.zip(max).is_none_or(|(min, max)| min <= max)
}

impl Condition {
    pub fn validate(&self, choices: &BTreeMap<String, String>) -> Result<()> {
        if let Some(choice) = &self.choice {
            ensure!(choices.contains_key(choice), "unknown choice {choice:?}");
        }
        ensure!(
            ordered(self.min_confidence, self.max_confidence),
            "min_confidence exceeds max_confidence"
        );
        if let Some(probability) = &self.probability {
            ensure!(
                choices.contains_key(&probability.choice),
                "unknown probability choice {:?}",
                probability.choice
            );
            ensure!(
                probability.min.is_some() || probability.max.is_some(),
                "probability needs min or max"
            );
            ensure!(
                ordered(probability.min, probability.max),
                "probability min exceeds max"
            );
        }
        for group in [&self.all, &self.any].into_iter().flatten() {
            ensure!(
                !group.is_empty(),
                "all/any must contain at least one condition"
            );
            for condition in group {
                condition.validate(choices)?;
            }
        }
        Ok(())
    }

    pub fn matches(&self, answer: &crate::jev::ChoiceAnswer) -> bool {
        self.choice
            .as_ref()
            .is_none_or(|choice| choice == &answer.choice)
            && in_bounds(answer.confidence, self.min_confidence, self.max_confidence)
            && self.probability.as_ref().is_none_or(|condition| {
                answer
                    .probabilities
                    .get(&condition.choice)
                    .is_some_and(|&p| in_bounds(p, condition.min, condition.max))
            })
            && self
                .all
                .as_ref()
                .is_none_or(|all| all.iter().all(|condition| condition.matches(answer)))
            && self
                .any
                .as_ref()
                .is_none_or(|any| any.iter().any(|condition| condition.matches(answer)))
    }
}
