use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use futures_util::{StreamExt, TryStreamExt, stream};
use ignore::WalkBuilder;
use serde::Serialize;

use crate::{
    config::{Config, InputContext, RuleSetting},
    jev::{ChoiceAnswer, JevClient, Question, Request, Response},
    policy::Level,
    rust::{self, Span, TargetKind},
};

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub file: PathBuf,
    #[serde(flatten)]
    pub span: Span,
}

#[derive(Debug, Serialize)]
pub struct Evaluation {
    pub location: Location,
    pub range: Span,
    pub target: String,
    pub kind: TargetKind,
    pub request: Request,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub files: usize,
    pub evaluations: Vec<Evaluation>,
    #[serde(skip)]
    sources: BTreeMap<PathBuf, String>,
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub rule: String,
    pub level: Level,
    pub message: String,
    pub target: String,
    pub location: Location,
    pub model: String,
    pub answer: ChoiceAnswer,
}

/// Every rule answer, including those which emit no diagnostic.
#[derive(Debug, Serialize)]
pub struct RuleAnswer {
    pub rule: String,
    pub target: String,
    pub location: Location,
    pub model: String,
    pub question: Question,
    pub answer: ChoiceAnswer,
}

pub fn rule_answers(evaluation: &Evaluation, response: &Response) -> Vec<RuleAnswer> {
    response
        .answers
        .iter()
        .map(|(rule, answer)| RuleAnswer {
            rule: rule.clone(),
            target: evaluation.target.clone(),
            location: evaluation.location.clone(),
            model: response.model.clone(),
            question: evaluation.request.questions[rule].clone(),
            answer: answer.clone(),
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub files: usize,
    pub evaluations: usize,
    pub questions: usize,
    pub models: BTreeSet<String>,
    pub warnings: usize,
    pub errors: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub answers: Vec<RuleAnswer>,
}

impl Report {
    /// Restrict displayed results while retaining the work performed by the run.
    pub fn retain_errors(&mut self) {
        self.diagnostics
            .retain(|diagnostic| diagnostic.level == Level::Error);
        let errors: BTreeSet<_> = self
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    &diagnostic.location.file,
                    diagnostic.location.span.start,
                    &diagnostic.rule,
                )
            })
            .collect();
        self.answers.retain(|answer| {
            errors.contains(&(
                &answer.location.file,
                answer.location.span.start,
                &answer.rule,
            ))
        });
        self.warnings = 0;
    }
}

impl Plan {
    /// Finish parsing and validation before sending any source to Jev.
    pub fn build(config: &Config, paths: &[PathBuf]) -> Result<Self> {
        let files = source_files(config, paths)?;
        ensure!(
            !files.is_empty(),
            "no Rust source files matched the configured paths"
        );
        let mut evaluations = Vec::new();
        let mut editions = BTreeMap::new();
        let mut sources = BTreeMap::new();
        for path in &files {
            let source = fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            let edition = if let Some(edition) = config.edition {
                edition.into()
            } else {
                let directory = path
                    .parent()
                    .context("source file has no parent")?
                    .to_path_buf();
                match editions.entry(directory) {
                    std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        *entry.insert(rust::edition_for(path)?)
                    }
                }
            };
            evaluations.extend(Self::source_evaluations(config, path, &source, edition)?);
            sources.insert(path.strip_prefix(&config.root)?.to_path_buf(), source);
        }
        Ok(Self {
            files: files.len(),
            evaluations,
            sources,
        })
    }

    /// Analyze an editor snapshot without writing it to disk.
    pub fn from_source(config: &Config, path: &Path, source: &str) -> Result<Self> {
        let path = path
            .canonicalize()
            .with_context(|| format!("cannot open input {}", path.display()))?;
        let relative = path
            .strip_prefix(&config.root)
            .context("input is outside the configuration directory")?;
        ensure!(
            path.extension().is_some_and(|ext| ext == "rs"),
            "editor input must be a Rust file"
        );
        let mut evaluations = Vec::new();
        if config.filter.matches(relative)
            && !relative
                .components()
                .any(|part| matches!(part.as_os_str().to_str(), Some("target" | ".git")))
        {
            let edition = match config.edition {
                Some(edition) => edition.into(),
                None => rust::edition_for(&path)?,
            };
            evaluations = Self::source_evaluations(config, &path, source, edition)?;
        }
        Ok(Self {
            files: 1,
            evaluations,
            sources: BTreeMap::from([(relative.to_path_buf(), source.to_owned())]),
        })
    }

    /// The exact source that was evaluated, including unsaved editor snapshots.
    pub fn source(&self, path: &Path) -> Option<&str> {
        self.sources.get(path).map(String::as_str)
    }

    /// Select all questions for exactly one function declaration, by UTF-8 byte offset.
    pub fn select_function(&mut self, start: usize) -> Result<()> {
        self.evaluations.retain(|evaluation| {
            evaluation.kind == TargetKind::Function && evaluation.location.span.start == start
        });
        ensure!(
            !self.evaluations.is_empty(),
            "no configured function rules at byte offset {start}"
        );
        Ok(())
    }

    fn source_evaluations(
        config: &Config,
        path: &Path,
        source: &str,
        edition: ra_ap_syntax::Edition,
    ) -> Result<Vec<Evaluation>> {
        let relative = path.strip_prefix(&config.root)?;
        let kinds = config
            .rules
            .values()
            .map(|rule| rule.definition.r#where.kind)
            .collect();
        let mut evaluations = Vec::new();
        for target in rust::extract(source, edition, &kinds)
            .with_context(|| format!("cannot parse {}", relative.display()))?
        {
            let mut questions_by_context = BTreeMap::<InputContext, BTreeMap<_, _>>::new();
            for (id, rule) in &config.rules {
                let selector = &rule.definition.r#where;
                if selector.kind == target.kind
                    && selector
                        .has_body
                        .is_none_or(|has_body| has_body == target.has_body)
                    && rule.filter.matches(relative)
                    && config.setting(relative, id) != Some(RuleSetting::Off)
                {
                    questions_by_context
                        .entry(rule.definition.context)
                        .or_default()
                        .insert(id.clone(), rule.definition.question.clone());
                }
            }
            for (context, questions) in questions_by_context {
                let mut state = target.input(context, source);
                state["file"] = serde_json::json!(relative);
                evaluations.push(Evaluation {
                    location: Location {
                        file: relative.to_path_buf(),
                        span: target.span.clone(),
                    },
                    target: target.name.clone(),
                    range: target.range.clone(),
                    kind: target.kind,
                    request: Request {
                        model: config.model.clone(),
                        state,
                        questions,
                    },
                });
            }
        }
        Ok(evaluations)
    }

    pub async fn run(
        &self,
        config: &Config,
        client: &JevClient,
        jobs: NonZeroUsize,
    ) -> Result<Report> {
        let results: Vec<_> = stream::iter(&self.evaluations)
            .map(|evaluation| async move {
                let response = client
                    .evaluate(&evaluation.request)
                    .await
                    .with_context(|| {
                        format!(
                            "evaluating {}:{} ({})",
                            evaluation.location.file.display(),
                            evaluation.location.span.line,
                            evaluation.target
                        )
                    })?;
                let model = response.model.clone();
                let answers = rule_answers(evaluation, &response);
                let diagnostics = diagnostics(config, evaluation, response)?;
                Ok::<_, anyhow::Error>((model, diagnostics, answers))
            })
            .buffer_unordered(jobs.get())
            .try_collect()
            .await?;
        let (mut models, mut diagnostics, mut answers) = (BTreeSet::new(), Vec::new(), Vec::new());
        for (model, emitted, all_answers) in results {
            models.insert(model);
            diagnostics.extend(emitted);
            answers.extend(all_answers);
        }
        Ok(self.report(models, diagnostics, answers))
    }

    pub fn empty_report(&self) -> Report {
        self.report(BTreeSet::new(), Vec::new(), Vec::new())
    }

    fn report(
        &self,
        models: BTreeSet<String>,
        mut diagnostics: Vec<Diagnostic>,
        mut answers: Vec<RuleAnswer>,
    ) -> Report {
        answers.sort_by(|left, right| {
            (&left.location.file, left.location.span.start, &left.rule).cmp(&(
                &right.location.file,
                right.location.span.start,
                &right.rule,
            ))
        });
        diagnostics.sort_by(|left, right| {
            (&left.location.file, left.location.span.start, &left.rule).cmp(&(
                &right.location.file,
                right.location.span.start,
                &right.rule,
            ))
        });
        Report {
            files: self.files,
            evaluations: self.evaluations.len(),
            questions: self
                .evaluations
                .iter()
                .map(|evaluation| evaluation.request.questions.len())
                .sum(),
            models,
            warnings: diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.level == Level::Warn)
                .count(),
            errors: diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.level == Level::Error)
                .count(),
            diagnostics,
            answers,
        }
    }
}

pub fn diagnostics(
    config: &Config,
    evaluation: &Evaluation,
    response: Response,
) -> Result<Vec<Diagnostic>> {
    response.validate(&evaluation.request)?;
    response
        .answers
        .into_iter()
        .filter_map(|(id, answer)| {
            let rule = &config.rules.get(&id)?.definition;
            let policy = rule
                .diagnostics
                .iter()
                .find(|policy| policy.when.matches(&answer))?;
            let level = match config.setting(&evaluation.location.file, &id) {
                Some(RuleSetting::Off) => return None,
                Some(RuleSetting::Warn) => Level::Warn,
                Some(RuleSetting::Error) => Level::Error,
                None => policy.level,
            };
            Some(Ok(Diagnostic {
                rule: id,
                level,
                message: policy.message.replace("{name}", &evaluation.target),
                target: evaluation.target.clone(),
                location: evaluation.location.clone(),
                model: response.model.clone(),
                answer,
            }))
        })
        .collect()
}

fn source_files(config: &Config, paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let roots = if paths.is_empty() {
        vec![config.root.clone()]
    } else {
        paths.to_vec()
    };
    let mut files = BTreeSet::new();
    for root in roots {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot open input {}", root.display()))?;
        ensure!(
            root.starts_with(&config.root),
            "input {} is outside configuration directory {}",
            root.display(),
            config.root.display()
        );
        let mut walker = WalkBuilder::new(&root);
        walker
            .hidden(false)
            .follow_links(false)
            .require_git(false)
            .git_global(false)
            .add_custom_ignore_filename(".erislintignore")
            .filter_entry(|entry| !matches!(entry.file_name().to_str(), Some(".git" | "target")));
        for entry in walker.build() {
            let entry = entry.with_context(|| format!("cannot walk {}", root.display()))?;
            if !entry.file_type().is_some_and(|kind| kind.is_file())
                || entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "rs")
            {
                continue;
            }
            let path = entry.path().canonicalize()?;
            let relative = path.strip_prefix(&config.root)?;
            if !relative
                .components()
                .any(|part| matches!(part.as_os_str().to_str(), Some("target" | ".git")))
                && config.filter.matches(relative)
            {
                files.insert(path);
            }
        }
    }
    Ok(files)
}
