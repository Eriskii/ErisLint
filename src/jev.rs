// Copyright (C) 2026 Eriskii
// SPDX-License-Identifier: AGPL-3.0-only
// See LICENSE for the full license text.

use std::{
    collections::BTreeMap,
    fmt,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, ensure};
use backon::{ExponentialBuilder, Retryable};
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, HeaderMap, HeaderValue, RETRY_AFTER},
    redirect::Policy,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::Probability;

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Question {
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
}

impl Question {
    pub fn choices(&self) -> &BTreeMap<String, String> {
        match self {
            Self::Choice { criteria, .. } => criteria,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Choice {
                instructions,
                criteria,
            } => {
                ensure!(
                    !instructions.trim().is_empty(),
                    "question instructions must not be empty"
                );
                ensure!(
                    (2..=255).contains(&criteria.len()),
                    "a choice question requires 2 to 255 choices"
                );
                ensure!(
                    criteria.keys().all(|choice| !choice.trim().is_empty()),
                    "choice names must not be empty"
                );
                Ok(())
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Request {
    pub model: String,
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
}

#[derive(Debug, Deserialize)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, ChoiceAnswer>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceType {
    Choice,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChoiceAnswer {
    pub r#type: ChoiceType,
    pub choice: String,
    pub confidence: Probability,
    pub probabilities: BTreeMap<String, Probability>,
}

impl Response {
    pub fn validate(&self, request: &Request) -> Result<()> {
        ensure!(
            !self.model.trim().is_empty(),
            "Jev returned an empty model identifier"
        );
        ensure!(
            self.answers.keys().eq(request.questions.keys()),
            "Jev returned missing or unexpected question ids"
        );
        for (id, question) in &request.questions {
            let answer = &self.answers[id];
            let choices = question.choices();
            ensure!(
                choices.contains_key(&answer.choice),
                "Jev returned unknown choice {:?} for {id}",
                answer.choice
            );
            ensure!(
                answer.probabilities.keys().eq(choices.keys()),
                "Jev returned missing or unexpected probabilities for {id}"
            );
        }
        Ok(())
    }
}

pub struct JevClient {
    client: Client,
    endpoint: String,
}

impl JevClient {
    pub fn new(key: &str) -> Result<Self> {
        Self::at_endpoint(key, ENDPOINT)
    }

    fn at_endpoint(key: &str, endpoint: &str) -> Result<Self> {
        ensure!(!key.trim().is_empty(), "jev_key must not be empty");
        let mut authorization = HeaderValue::from_str(&format!("Bearer {key}"))
            .context("jev_key contains invalid header characters")?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        let client = Client::builder()
            .default_headers(headers)
            .user_agent(concat!("erislint/", env!("CARGO_PKG_VERSION")))
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .context("cannot initialize Jev client")?;
        Ok(Self {
            client,
            endpoint: endpoint.into(),
        })
    }

    pub async fn evaluate(&self, request: &Request) -> Result<Response> {
        let mut attempts = 0;
        let body = (|| {
            attempts += 1;
            self.fetch(request)
        })
        .retry(
            ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_factor(2.0)
                .with_max_times(3)
                .with_jitter(),
        )
        .adjust(retry_delay)
        .notify(|error, delay| {
            eprintln!(
                "erislint: {error:#}; retrying in {:.1}s",
                delay.as_secs_f64()
            );
        })
        .await
        .with_context(|| {
            format!(
                "Jev evaluation failed after {attempts} attempt{}",
                if attempts == 1 { "" } else { "s" }
            )
        })?;
        let response: Response = serde_json::from_slice(&body).context("invalid Jev response")?;
        response.validate(request)?;
        Ok(response)
    }

    // Keep decoding outside the retried operation: a broken connection can be
    // retried, but a complete response with invalid JSON or answers must fail.
    async fn fetch(&self, request: &Request) -> Result<Vec<u8>> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(request)
            .send()
            .await
            .context("cannot reach Jev")?;
        let status = response.status();
        if !status.is_success() {
            return Err(HttpFailure {
                status,
                retry_after: response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| retry_after(value, SystemTime::now())),
            }
            .into());
        }
        response
            .bytes()
            .await
            .map(|body| body.to_vec())
            .context("cannot read Jev response")
    }
}

#[derive(Debug)]
struct HttpFailure {
    status: StatusCode,
    retry_after: Option<Duration>,
}

impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Jev returned HTTP {}", self.status)?;
        if self
            .retry_after
            .is_some_and(|delay| delay > MAX_RETRY_DELAY)
        {
            write!(f, "; Retry-After exceeds the 60s retry limit")?;
        }
        Ok(())
    }
}

impl std::error::Error for HttpFailure {}

fn retry_delay(error: &anyhow::Error, backoff: Option<Duration>) -> Option<Duration> {
    let backoff = backoff?;
    if let Some(failure) = error.downcast_ref::<HttpFailure>() {
        return matches!(failure.status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
            .then(|| backoff.max(failure.retry_after.unwrap_or_default()))
            .filter(|delay| *delay <= MAX_RETRY_DELAY);
    }
    error
        .downcast_ref::<reqwest::Error>()
        .filter(|error| {
            error.is_timeout()
                || error.is_connect()
                || error.is_request()
                || error.is_body()
                || error.is_decode()
        })
        .map(|_| backoff)
}

fn retry_after(value: &HeaderValue, now: SystemTime) -> Option<Duration> {
    let value = value.to_str().ok()?.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or_default())
}

#[cfg(test)]
mod tests;
