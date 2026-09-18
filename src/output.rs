use std::io::{self, Write};

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet};

use crate::{
    policy,
    runner::{Plan, Report},
};

#[derive(Default)]
pub struct TextOptions {
    pub errors_only: bool,
    pub all_answers: bool,
    pub color: bool,
}

pub fn write_text(
    mut output: impl Write,
    report: &Report,
    plan: &Plan,
    options: TextOptions,
) -> io::Result<()> {
    let renderer = if options.color {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
    for diagnostic in &report.diagnostics {
        let level = match diagnostic.level {
            policy::Level::Warn => Level::WARNING,
            policy::Level::Error => Level::ERROR,
        };
        let file = diagnostic.location.file.display().to_string();
        let span = &diagnostic.location.span;
        let title = level
            .primary_title(&diagnostic.message)
            .id(&diagnostic.rule);
        let group = match plan.source(&diagnostic.location.file) {
            Some(source) => title.element(
                Snippet::source(source)
                    .path(&file)
                    .fold(true)
                    .annotation(AnnotationKind::Primary.span(span.start..span.end)),
            ),
            None => {
                title.element(Level::NOTE.message(format!("{file}:{}:{}", span.line, span.column)))
            }
        };
        writeln!(output, "{}\n", renderer.render(&[group]))?;
    }
    if options.all_answers {
        for result in &report.answers {
            writeln!(
                output,
                "{}:{}: {} [{}] — {} (confidence: {:.1}%)",
                result.location.file.display(),
                result.location.span.line,
                result.target,
                result.rule,
                result.answer.choice,
                result.answer.confidence.get() * 100.0,
            )?;
            for (choice, probability) in &result.answer.probabilities {
                writeln!(output, "  {choice}: {:.2}%", probability.get() * 100.0)?;
            }
        }
        if !report.answers.is_empty() {
            writeln!(output)?;
        }
    }
    let counts = [(report.errors, "error"), (report.warnings, "warning")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, kind)| format!("{count} {kind}{}", if count == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(", ");
    if !counts.is_empty() {
        writeln!(output, "Found {counts}.")
    } else if options.errors_only {
        writeln!(output, "No errors found.")
    } else {
        writeln!(output, "No issues found.")
    }
}
