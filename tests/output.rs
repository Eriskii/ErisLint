mod common;

use std::collections::BTreeSet;

use common::{Project, response, rule};
use erislint::{
    jev::Response,
    output::{TextOptions, write_text},
    policy::Level,
    runner::{Plan, Report, diagnostics, rule_answers},
};
use serde_json::json;

fn fixture(confidences: [f64; 2]) -> (Project, Plan, Report) {
    let project = Project::new();
    let path = project.write("src/lib.rs", "fn saved_on_disk() {}\n");
    let config = project.config(json!({ "rules": [rule("quality")] }));
    let plan = Plan::from_source(&config, &path, "// 🦀\nfn café() {}\nfn broken() {}\n").unwrap();
    let mut emitted = Vec::new();
    let mut answers = Vec::new();
    for (evaluation, confidence) in plan.evaluations.iter().zip(confidences) {
        let response: Response = serde_json::from_value(response("quality", confidence)).unwrap();
        answers.extend(rule_answers(evaluation, &response));
        emitted.extend(diagnostics(&config, evaluation, response).unwrap());
    }
    let report = Report {
        files: 1,
        evaluations: 2,
        questions: 2,
        models: BTreeSet::from(["jev-test-pinned".into()]),
        warnings: emitted.iter().filter(|d| d.level == Level::Warn).count(),
        errors: emitted.iter().filter(|d| d.level == Level::Error).count(),
        diagnostics: emitted,
        answers,
    };
    (project, plan, report)
}

fn render(report: &Report, plan: &Plan, options: TextOptions) -> String {
    let mut output = Vec::new();
    write_text(&mut output, report, plan, options).unwrap();
    String::from_utf8(output).unwrap()
}

#[test]
fn human_output_uses_the_evaluated_snapshot_without_model_metadata() {
    let (_project, plan, report) = fixture([0.7, 0.95]);
    let text = render(&report, &plan, TextOptions::default());
    for expected in [
        "warning[quality]",
        "error[quality]",
        "src/lib.rs:2:4",
        "fn café() {}",
        "^^^^",
    ] {
        assert!(text.contains(expected), "missing {expected:?}:\n{text}");
    }
    for absent in [
        "saved_on_disk",
        "confidence",
        "probabilities",
        "jev-test-pinned",
        "\x1b[",
    ] {
        assert!(!text.contains(absent), "unexpected {absent:?}:\n{text}");
    }
    assert!(text.ends_with("Found 1 error, 1 warning.\n"));
    assert!(
        serde_json::to_value(&plan)
            .unwrap()
            .get("sources")
            .is_none()
    );
}

#[test]
fn errors_only_filters_both_diagnostics_and_json_answers() {
    let (_project, plan, mut report) = fixture([0.7, 0.95]);
    report.retain_errors();
    let text = render(
        &report,
        &plan,
        TextOptions {
            errors_only: true,
            ..Default::default()
        },
    );
    assert!(!text.contains("warning["));
    assert!(text.contains("error[quality]: Simplify broken"));
    assert!(text.ends_with("Found 1 error.\n"));
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["warnings"], 0);
    assert_eq!(json["errors"], 1);
    assert_eq!(json["answers"].as_array().unwrap().len(), 1);
    assert_eq!(json["answers"][0]["target"], "broken");
    assert_eq!(json["questions"], 2);
}

#[test]
fn errors_only_reports_success_when_only_warnings_were_found() {
    let (_project, plan, mut report) = fixture([0.7, 0.7]);
    report.retain_errors();
    assert_eq!(
        render(
            &report,
            &plan,
            TextOptions {
                errors_only: true,
                ..Default::default()
            }
        ),
        "No errors found.\n"
    );
}

#[test]
fn color_and_probability_details_are_explicit() {
    let (_project, plan, report) = fixture([0.7, 0.95]);
    let text = render(
        &report,
        &plan,
        TextOptions {
            all_answers: true,
            color: true,
            ..Default::default()
        },
    );
    assert!(text.contains("\x1b["));
    assert!(text.contains("confidence: 70.0%"));
    assert!(text.contains("  bad: 90.00%"));
}
