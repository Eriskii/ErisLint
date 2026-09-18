mod common;

use std::collections::BTreeSet;

use common::{Project, response, rule};
use erislint::{
    config::{Config, InputContext},
    jev::{ChoiceAnswer, Response},
    policy::{Condition, Level},
    runner::{Plan, diagnostics, rule_answers},
    rust::{self, TargetKind},
};
use ra_ap_syntax::Edition;
use serde_json::{Value, json};

#[test]
fn discovers_nearest_config_and_stops_at_repository_boundary() {
    let project = Project::new();
    let outer = project.json("erislint.json", &json!({ "rules": [rule("outer")] }));
    project.write("repo/.git", "gitdir: elsewhere");
    project.write("repo/src/lib.rs", "fn f() {}");
    assert!(Config::discover(&project.root().join("repo/src")).is_err());
    let inner = project.json("repo/erislint.json", &json!({ "rules": [rule("inner")] }));
    assert_eq!(
        Config::discover(&project.root().join("repo/src")).unwrap(),
        inner
    );
    assert_eq!(Config::discover(project.root()).unwrap(), outer);
}

#[test]
fn extends_resolves_rule_files_at_their_origin_and_replaces_rules_by_id() {
    let project = Project::new();
    project.json("shared/rules/quality.json", &rule("quality"));
    project.json(
        "shared/base.json",
        &json!({ "rule_files": ["rules/quality.json"], "model": "base-model" }),
    );
    project.write("app/src/lib.rs", "fn task() {}");
    let mut replacement = rule("quality");
    replacement["diagnostics"][0]["message"] = json!("Child rule");
    let path = project.json(
        "app/erislint.json",
        &json!({
            "extends": ["../shared/base.json"], "rules": [replacement], "include": ["src/**"]
        }),
    );
    let config = Config::load(&path).unwrap();
    let plan = Plan::build(&config, &[]).unwrap();
    assert_eq!(plan.evaluations.len(), 1);
    assert_eq!(plan.evaluations[0].request.model, "base-model");
    assert_eq!(
        config.rules["quality"].definition.diagnostics[0].message,
        "Child rule"
    );
}

#[test]
fn rejects_cycles_duplicates_and_invalid_rule_contracts() {
    let project = Project::new();
    project.json("a.json", &json!({ "extends": ["b.json"] }));
    project.json("b.json", &json!({ "extends": ["a.json"] }));
    assert!(
        Config::load(&project.root().join("a.json"))
            .unwrap_err_string()
            .contains("cycle")
    );

    let mut unknown_choice = rule("quality");
    unknown_choice["diagnostics"][0]["when"]["choice"] = json!("typo");
    let mut invalid_probability = rule("quality");
    invalid_probability["diagnostics"][0]["when"]["min_confidence"] = json!(1.1);
    let mut inverted_bounds = rule("quality");
    inverted_bounds["diagnostics"][0]["when"]["max_confidence"] = json!(0.3);
    let mut invalid_selector = rule("quality");
    invalid_selector["where"]["kind"] = json!("struct");
    for (config, expected) in [
        (
            json!({ "rules": [rule("same"), rule("same")] }),
            "duplicate",
        ),
        (json!({ "rules": [unknown_choice] }), "unknown choice"),
        (json!({ "rules": [invalid_probability] }), "between 0 and 1"),
        (json!({ "rules": [inverted_bounds] }), "exceeds"),
        (json!({ "rules": [invalid_selector] }), "has_body"),
        (
            json!({ "rules": [rule("quality")], "modle": "typo" }),
            "unknown field",
        ),
        (
            json!({ "rules": [rule("quality")], "overrides": [{ "files": ["**"], "rules": {"typo": "off"} }] }),
            "unknown rule",
        ),
    ] {
        let path = project.json("erislint.json", &config);
        let error = Config::load(&path).unwrap_err_string();
        assert!(
            error.contains(expected),
            "expected {expected:?}, got {error}"
        );
    }
    let mut invalid = rule("quality");
    invalid["diagnostics"][0]["when"]["min_confidence"] = json!(1.1);
    project.json("rules/invalid.json", &invalid);
    let path = project.json(
        "erislint.json",
        &json!({ "rule_files": ["rules/invalid.json"] }),
    );
    let error = Config::load(&path).unwrap_err_string();
    assert!(error.contains("rules/invalid.json") && error.contains("between 0 and 1"));
}

trait ErrorText {
    fn unwrap_err_string(self) -> String;
}

impl<T> ErrorText for anyhow::Result<T> {
    fn unwrap_err_string(self) -> String {
        match self {
            Ok(_) => panic!("expected an error"),
            Err(error) => format!("{error:#}"),
        }
    }
}

#[test]
fn preserves_function_fields_comments_enclosing_context_and_unicode_locations() {
    let source = r#"mod inventory {
    impl<'a, T> Store<'a, T> {
        /// Keep this documentation.
        #[inline]
        pub unsafe fn résumé<U>(&mut self, (left, right): (U, U), value: &'a T) -> Option<U>
        where U: Copy {
            // Keep this body comment.
            Some(left)
        }
    }
}"#;
    let targets = rust::extract(
        source,
        Edition::Edition2024,
        &BTreeSet::from([TargetKind::Function]),
    )
    .unwrap();
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    let state = target.input(InputContext::Enclosing, source);
    assert_eq!(state["name"], "résumé");
    assert_eq!(state["receiver"], "&mut self");
    assert_eq!(
        state["params"][0],
        json!({ "pattern": "(left, right)", "type": "(U, U)" })
    );
    assert_eq!(state["params"][1]["type"], "&'a T");
    assert_eq!(state["return_type"], "Option<U>");
    assert_eq!(state["generics"], "<U>");
    assert!(
        state["body"]
            .as_str()
            .unwrap()
            .contains("// Keep this body comment.")
    );
    assert_eq!(state["docs"][0], "/// Keep this documentation.");
    assert_eq!(state["context"]["enclosing"][0]["name"], "inventory");
    assert_eq!(
        state["context"]["enclosing"][1]["self_type"],
        "Store<'a, T>"
    );
    assert_eq!(&source[target.span.start..target.span.end], "résumé");
    assert_eq!((target.span.line, target.span.column), (5, 23));
    assert_eq!(target.span.end_column - target.span.column, 6);
    assert!(
        target
            .input(InputContext::Target, source)
            .get("context")
            .is_none()
    );
    assert_eq!(
        target.input(InputContext::File, source)["context"]["file"],
        source
    );
}

#[test]
fn extracts_structs_enums_traits_impls_modules_and_files() {
    let source = "struct Pair(pub u8, u8); struct Unit; enum State { Empty, Value { count: u32 }, Code = 2 } trait Read { fn read(&self); } impl Read for Pair { fn read(&self) {} } mod external;";
    let kinds = BTreeSet::from([
        TargetKind::Function,
        TargetKind::Struct,
        TargetKind::Enum,
        TargetKind::Trait,
        TargetKind::Impl,
        TargetKind::Module,
        TargetKind::File,
    ]);
    let targets = rust::extract(source, Edition::Edition2024, &kinds).unwrap();
    let state = |name: &str| {
        targets
            .iter()
            .find(|target| target.name == name)
            .unwrap()
            .input(InputContext::Target, source)
    };
    assert_eq!(state("Pair")["fields"][0]["type"], "u8");
    assert_eq!(state("Pair")["fields"][0]["visibility"], "pub");
    assert_eq!(state("Unit")["fields"], json!([]));
    assert_eq!(state("State")["variants"][1]["fields"][0]["name"], "count");
    assert_eq!(state("State")["variants"][2]["discriminant"], "2");
    assert_eq!(state("Read")["items"].as_array().unwrap().len(), 1);
    assert_eq!(state("<impl>")["trait"], "Read");
    assert_eq!(state("external")["external"], true);
    assert_eq!(state("<file>")["contents"], source);
    let bodies: Vec<_> = targets
        .iter()
        .filter(|target| target.kind == TargetKind::Function)
        .map(|target| target.has_body)
        .collect();
    assert_eq!(bodies, [false, true]);
}

#[test]
fn source_level_analysis_leaves_macros_unexpanded_and_reports_parse_errors() {
    let kinds = BTreeSet::from([TargetKind::Function]);
    let targets = rust::extract(
        "macro_rules! make { () => { fn generated() {} } } #[cfg(any())] fn present() {}",
        Edition::Edition2024,
        &kinds,
    )
    .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "present");
    let error = rust::extract("fn broken( {", Edition::Edition2024, &kinds).unwrap_err_string();
    assert!(error.contains("Rust syntax errors"));
}

#[test]
fn plans_batch_by_target_and_context_and_apply_globs_ignores_and_overrides() {
    let project = Project::new();
    for path in [
        "src/lib.rs",
        "src/ignored.rs",
        "generated/code.rs",
        "target/build/code.rs",
        "tests/test.rs",
    ] {
        project.write(path, "fn task() {}");
    }
    project.write(".gitignore", "/src/ignored.rs\n");
    project.write("src/info.txt", "fn not_rust() {}");
    let mut full = rule("full-file");
    full["context"] = json!("file");
    let config = project.config(json!({
        "rules": [rule("first"), rule("second"), full],
        "exclude": ["generated/**"],
        "overrides": [{ "files": ["tests/**"], "rules": { "first": "off", "second": "off", "full-file": "off" } }]
    }));
    let plan = Plan::build(&config, &[]).unwrap();
    assert_eq!(plan.files, 2);
    assert_eq!(plan.evaluations.len(), 2);
    assert_eq!(plan.evaluations[0].request.questions.len(), 2);
    assert_eq!(plan.evaluations[1].request.questions.len(), 1);
    assert_eq!(
        plan.evaluations[0].location.file.to_str().unwrap(),
        "src/lib.rs"
    );
    assert!(
        plan.evaluations[0].request.state["context"]
            .get("file")
            .is_none()
    );
    assert_eq!(
        plan.evaluations[1].request.state["context"]["file"],
        "fn task() {}"
    );
}

#[test]
fn honors_package_and_inherited_workspace_editions() {
    let project = Project::new();
    project.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\n[workspace.package]\nedition = \"2021\"\n",
    );
    project.write(
        "member/Cargo.toml",
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition.workspace = true\n",
    );
    let file = project.write("member/src/lib.rs", "fn gen() {}");
    assert_eq!(rust::edition_for(&file).unwrap(), Edition::Edition2021);
    let config = project.config(json!({ "rules": [rule("quality")] }));
    assert_eq!(
        Plan::build(&config, &[]).unwrap().evaluations[0].target,
        "gen"
    );
    project.write(
        "member/Cargo.toml",
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\n",
    );
    assert_eq!(rust::edition_for(&file).unwrap(), Edition::Edition2015);
}

#[test]
fn selects_the_first_matching_severity_at_inclusive_thresholds() {
    let project = Project::new();
    project.write("src/lib.rs", "fn task() {}");
    let config = project.config(json!({ "rules": [rule("quality")] }));
    let plan = Plan::build(&config, &[]).unwrap();
    for (confidence, expected) in [
        (0.649, None),
        (0.65, Some(Level::Warn)),
        (0.899, Some(Level::Warn)),
        (0.9, Some(Level::Error)),
    ] {
        let response: Response = serde_json::from_value(response("quality", confidence)).unwrap();
        let diagnostics = diagnostics(&config, &plan.evaluations[0], response).unwrap();
        assert_eq!(
            diagnostics.first().map(|diagnostic| diagnostic.level),
            expected
        );
        if let Some(diagnostic) = diagnostics.first() {
            assert!(diagnostic.message.ends_with("task"));
            assert_eq!(diagnostic.model, "jev-test-pinned");
        }
    }
}

#[test]
fn last_matching_override_changes_severity_without_changing_thresholds() {
    let project = Project::new();
    project.write("src/lib.rs", "fn task() {}");
    let config = project.config(json!({
        "rules": [rule("quality")],
        "overrides": [
            { "files": ["**/*.rs"], "rules": { "quality": "off" } },
            { "files": ["src/**"], "rules": { "quality": "warn" } }
        ]
    }));
    let plan = Plan::build(&config, &[]).unwrap();
    assert_eq!(plan.evaluations.len(), 1);
    for (confidence, expected) in [(0.5, None), (1.0, Some(Level::Warn))] {
        let response = serde_json::from_value(response("quality", confidence)).unwrap();
        let diagnostics = diagnostics(&config, &plan.evaluations[0], response).unwrap();
        assert_eq!(
            diagnostics.first().map(|diagnostic| diagnostic.level),
            expected
        );
    }
}

#[test]
fn confidence_probability_and_boolean_conditions_have_distinct_meanings() {
    let answer: ChoiceAnswer =
        serde_json::from_value(response("quality", 0.4)["answers"]["quality"].clone()).unwrap();
    for (when, expected) in [
        (json!({ "min_confidence": 0.8 }), false),
        (
            json!({ "probability": { "choice": "bad", "min": 0.8 } }),
            true,
        ),
        (
            json!({ "choice": "good", "probability": { "choice": "bad", "min": 0.8 } }),
            false,
        ),
        (
            json!({ "any": [{"choice": "good"}, {"choice": "bad"}] }),
            true,
        ),
        (
            json!({ "all": [{"choice": "bad"}, {"max_confidence": 0.4}] }),
            true,
        ),
        (
            json!({ "choice": "good", "any": [{"choice": "bad"}] }),
            false,
        ),
    ] {
        let condition: Condition = serde_json::from_value(when.clone()).unwrap();
        assert_eq!(condition.matches(&answer), expected, "{when}");
    }
}

#[test]
fn malformed_or_incomplete_responses_cannot_silently_pass() {
    let project = Project::new();
    project.write("src/lib.rs", "fn task() {}");
    let config = project.config(json!({ "rules": [rule("quality")] }));
    let plan = Plan::build(&config, &[]).unwrap();
    let good = response("quality", 0.9);
    let mutate = |path: &str, value: Value| {
        let mut response = good.clone();
        *response.pointer_mut(path).unwrap() = value;
        response
    };
    for bad in [
        json!({ "model": "test", "answers": {} }),
        mutate("/answers/quality/type", json!("score")),
        mutate("/answers/quality/choice", json!("other")),
        mutate("/answers/quality/confidence", json!(1.1)),
        mutate("/answers/quality/probabilities", json!({"bad": 1.0})),
        mutate("/answers/quality/probabilities/good", json!(1.1)),
    ] {
        let result = serde_json::from_value::<Response>(bad)
            .map_err(anyhow::Error::from)
            .and_then(|response| diagnostics(&config, &plan.evaluations[0], response));
        assert!(result.is_err());
    }
}

#[test]
fn preserves_reported_choices_and_probabilities_without_normalizing() {
    let project = Project::new();
    project.write("src/lib.rs", "fn task() {}");
    let config = project.config(json!({ "rules": [rule("quality")] }));
    let plan = Plan::build(&config, &[]).unwrap();
    let evaluation = &plan.evaluations[0];
    for (choice, probabilities) in [
        ("bad", json!({ "good": 0.1, "bad": 0.4, "unknown": 0.1 })),
        ("bad", json!({ "good": 0.6, "bad": 0.8, "unknown": 0.2 })),
        ("good", json!({ "good": 0.08, "bad": 0.9, "unknown": 0.02 })),
    ] {
        let mut value = response("quality", 0.9);
        value["answers"]["quality"]["choice"] = json!(choice);
        value["answers"]["quality"]["probabilities"] = probabilities.clone();
        let response: Response = serde_json::from_value(value).unwrap();
        response.validate(&evaluation.request).unwrap();
        let answers = rule_answers(evaluation, &response);
        assert_eq!(answers[0].answer.choice, choice);
        assert_eq!(
            serde_json::to_value(&answers[0].answer.probabilities).unwrap(),
            probabilities
        );
    }
}

#[test]
fn retains_every_probability_even_when_no_diagnostic_is_emitted() {
    let project = Project::new();
    project.write("src/lib.rs", "fn task() {}");
    let config = project.config(json!({ "rules": [rule("quality")] }));
    let plan = Plan::build(&config, &[]).unwrap();
    let response: Response = serde_json::from_value(response("quality", 0.2)).unwrap();
    let answers = rule_answers(&plan.evaluations[0], &response);
    assert!(
        diagnostics(&config, &plan.evaluations[0], response)
            .unwrap()
            .is_empty()
    );
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].answer.probabilities.len(), 3);
    assert_eq!(answers[0].answer.probabilities["unknown"].get(), 0.02);
}

#[test]
fn editor_snapshot_selects_one_exact_function_including_all_its_contexts() {
    let project = Project::new();
    let path = project.write("src/lib.rs", "fn old_name() {}");
    let mut full = rule("full");
    full["context"] = json!("file");
    let config = project.config(json!({ "rules": [rule("quality"), full] }));
    let snapshot = "// 🦀\nfn first() {} fn café() {}";
    let start = snapshot.find("café").unwrap();
    let mut plan = Plan::from_source(&config, &path, snapshot).unwrap();
    plan.select_function(start).unwrap();
    assert_eq!(plan.evaluations.len(), 2);
    assert!(
        plan.evaluations
            .iter()
            .all(|evaluation| evaluation.target == "café")
    );
    assert_eq!(
        plan.evaluations[1].request.state["context"]["file"],
        snapshot
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "fn old_name() {}");
    assert!(plan.select_function(start + 1).is_err());
}
