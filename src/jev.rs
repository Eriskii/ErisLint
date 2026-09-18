use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, ensure};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
    redirect::Policy,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::Probability;

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

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
        let response = self
            .client
            .post(&self.endpoint)
            .json(request)
            .send()
            .await
            .context("cannot reach Jev")?;
        let status = response.status();
        ensure!(status.is_success(), "Jev request failed with HTTP {status}");
        let response: Response = response.json().await.context("invalid Jev response")?;
        response.validate(request)?;
        Ok(response)
    }
}
