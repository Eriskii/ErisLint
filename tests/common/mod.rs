#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use erislint::config::Config;
use serde_json::{Value, json};
use tempfile::TempDir;

pub struct Project(pub TempDir);

impl Project {
    pub fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    pub fn root(&self) -> &Path {
        self.0.path()
    }

    pub fn write(&self, path: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    pub fn json(&self, path: &str, value: &Value) -> PathBuf {
        self.write(path, serde_json::to_vec_pretty(value).unwrap())
    }

    pub fn config(&self, value: Value) -> Config {
        Config::load(&self.json("erislint.json", &value)).unwrap()
    }
}

pub fn rule(id: &str) -> Value {
    json!({
        "id": id,
        "where": { "kind": "function", "has_body": true },
        "question": {
            "type": "choice",
            "instructions": "Is the function simple?",
            "criteria": { "good": "Simple", "bad": "Unnecessarily complex", "unknown": "Insufficient context" }
        },
        "diagnostics": [
            { "when": { "choice": "bad", "min_confidence": 0.9 }, "level": "error", "message": "Simplify {name}" },
            { "when": { "choice": "bad", "min_confidence": 0.65 }, "level": "warn", "message": "Consider simplifying {name}" }
        ]
    })
}

pub fn response(id: &str, confidence: f64) -> Value {
    json!({
        "model": "jev-test-pinned",
        "answers": {
            id: {
                "type": "choice", "choice": "bad", "confidence": confidence,
                "probabilities": { "good": 0.08, "bad": 0.9, "unknown": 0.02 }
            }
        }
    })
}
