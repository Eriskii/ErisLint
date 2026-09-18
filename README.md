# ErisLint

Define code-quality questions in JSON, evaluate Rust syntax with TypeSafe's Jev,
and map the resulting choices and confidence to warnings or errors.

## Run

Requires Rust 1.95 or newer. Build from this checkout:

```sh
cargo install --path . --locked
erislint --check-config
erislint --dry-run
erislint
```

Set the **`jev_key` environment variable** to your TypeSafe API key before a live
run. ErisLint reads this exact, case-sensitive name and sends it as a bearer token
to `https://api.typesafe.ai/v1/systemone`. Configuration files contain no key.
`.env` files are not loaded automatically; your shell or secret manager must
populate the environment.

`--check-config` validates configuration and referenced rule files offline.
`--dry-run` also parses Rust and prints the exact states and questions that would
be sent to Jev. Neither mode reads the secret or makes network requests.

```sh
erislint src/lib.rs src/domain
erislint --config ./erislint.json
erislint --format json --jobs 64
erislint --deny-warnings
erislint --errors-only
erislint --all-answers
```

Without input paths, ErisLint scans the configuration directory. Explicit input
paths are relative to the working directory and must be inside the configuration
directory. Output locations are relative to that configuration directory.
Warnings exit with **0**, emitted errors with **1**, and configuration, syntax,
filesystem, authentication, or API failures with **2**. `--deny-warnings` also
turns warnings into exit status 1. JSON diagnostics go to stdout; progress and
operational errors go to stderr.

Normal text output uses Rust-style diagnostics with a rule ID, source location,
the evaluated source excerpt, and a caret under the affected declaration. It
ends with a short error/warning summary. Confidence and probability details are
shown only with `--all-answers` or in JSON.

`--errors-only` shows only error diagnostics and their answers, in text or JSON.
It prints `No errors found.` when no errors were emitted. The filter affects
display, so `--deny-warnings` still fails a run that produced hidden warnings.
`--errors-only` and `--all-answers` are mutually exclusive. JSON evaluation counts
still describe the complete run.

Terminal diagnostics use color automatically; redirected files are plain text.
Use `--color auto|always|never` to override this. `NO_COLOR` disables automatic color.

`--format json` includes an `answers` array for **every evaluated rule**, even
when no warning or error is triggered, unless filtered by `--errors-only`.
Each entry includes the complete option
probabilities, selected choice, confidence, rubric, and model. `--all-answers`
prints those probabilities in human-readable text output.

## VS Code

The [VS Code extension](vscode/README.md) adds **Run ErisLint** buttons above
function declarations. Click to evaluate just that function, then hover its name
for all rules' probability distributions. It uses unsaved editor contents and
clears results when the document changes. Package it with `npm run package` from
`vscode/`, then install the generated VSIX locally.

Editor integrations can pipe source into `--stdin-file /absolute/path.rs`. Add
`--dry-run` for local AST discovery or `--format json --target-start <byte-offset>`
to evaluate only the function name at that exact UTF-8 offset. The file must
exist, but the supplied contents need not be saved. This obeys configuration
patterns and does not write the buffer to disk.

## Configuration

Put `erislint.json` alongside your workspace's root `Cargo.toml` and commit it.
Discovery searches the working directory, then its parents, stopping after
checking the repository root (identified by a `.git` file or directory).
`--config` bypasses discovery. The nearest config is selected; parent configs
are not implicitly merged.

```json
{
  "$schema": "./erislint.schema.json",
  "version": 1,
  "model": "jev-latest",
  "include": ["**/*.rs"],
  "exclude": ["generated/**"],
  "rule_files": [".erislint/rules/function-simplicity.json"],
  "rules": [],
  "overrides": [
    {
      "files": ["tests/**"],
      "rules": { "function-simplicity": "warn" }
    }
  ]
}
```

- `rules` contains inline rules; each `rule_files` entry contains one rule object
  or an array of rule objects. References are explicit file paths, not globs.
- `include` defaults to `["**/*.rs"]`; `exclude` defaults to `[]`. Only `.rs`
  files are scanned. `target` and `.git` directories are always skipped.
- File patterns use glob syntax (`*` stays within a directory; `**` spans
  directories). All source patterns are relative to the selected config's
  directory. `.gitignore`, `.ignore`, and `.erislintignore` are also respected
  during directory traversal. Explicit file arguments bypass ignore files but
  still obey config patterns and the `target`/`.git` exclusions.
- Overrides are applied in order; the last matching setting for a rule wins.
  `"off"` prevents evaluation; `"warn"` or `"error"` overrides the severity of
  a matching diagnostic without changing its thresholds.
- `edition` can explicitly be `"2015"`, `"2018"`, `"2021"`, or `"2024"`.
  Otherwise each source file uses its nearest package's Cargo edition, including
  `edition.workspace = true`. Cargo packages without an edition use 2015;
  standalone files without a package use 2024.
- Unknown fields, duplicate local rule IDs, unknown choices, unknown override
  IDs, and invalid thresholds are configuration errors.

Use `"extends": ["../shared/erislint.json"]` for explicit inheritance. Bases are
applied in order, then the current config. A child rule replaces the whole base
rule with the same ID. Scalar settings and include/exclude lists replace base
values; override lists append. `extends` and `rule_files` paths resolve relative
to the file declaring them. Inherited source globs still use the selected
config's directory, making shared rule sets reusable. Cycles are rejected.

The checked-in JSON Schemas provide editor completion. Regenerate them after
changing the configuration types:

```sh
cargo run --locked -- --schema config > erislint.schema.json
cargo run --locked -- --schema rule > erislint-rule.schema.json
```

## Rules and diagnostic conditions

See [the starter simplicity rule](.erislint/rules/function-simplicity.json).
The initial implementation supports Jev **Choice** questions. `question` uses
Jev's `type`, `instructions`, and `criteria` fields; choices and descriptions are
strings. Each rule needs 2–255 choices and at least one diagnostic policy.

```json
{
  "id": "clear-name",
  "where": { "kind": "function", "files": ["src/**"] },
  "question": {
    "type": "choice",
    "instructions": "Does the function name accurately describe its purpose?",
    "criteria": {
      "clear": "The name makes the purpose clear.",
      "unclear": "The name is misleading or too vague.",
      "unknown": "There is not enough information to judge."
    }
  },
  "diagnostics": [
    {
      "when": { "choice": "unclear", "min_confidence": 0.8 },
      "level": "warn",
      "message": "Choose a more descriptive name for {name}."
    }
  ]
}
```

Policies are evaluated top to bottom: **first match wins**, at most one
diagnostic per rule and target. No match emits nothing. The message is written
by the rule author; `{name}` expands to the target's name.

All fields in a `when` object are ANDed. Available conditions:

- `choice`: compare the selected choice.
- `min_confidence` / `max_confidence`: inclusive bounds on Jev's confidence.
- `probability`: bound a particular option, whether or not it was selected.
- `any` / `all`: nonempty arrays of nested conditions, combined with OR / AND.

An empty `when` object matches every answer. For example, this condition matches
either a confidently selected `unclear`, or an `unknown` probability of at least
0.7:

```json
{
  "any": [
    { "choice": "unclear", "min_confidence": 0.8 },
    { "probability": { "choice": "unknown", "min": 0.7 } }
  ]
}
```

All bounds must be between 0 and 1. `probability` accepts `min`, `max`, or both.
Confidence summarizes the distribution; it is **not** the probability of the
selected option. The starter thresholds are examples to tune on your code.

## Rust inputs

The parser is rust-analyzer's `ra_ap_syntax`. Targets retain the original source
text, including comments in bodies. Diagnostics carry zero-based, end-exclusive
byte offsets and one-based lines and Unicode columns. Named targets point to
their names; unnamed targets point to the whole node.

| `where.kind` | Target-specific input fields |
| --- | --- |
| `function` | `params` (pattern/type pairs), `receiver`, `return_type`, `body`, `async`, `unsafe`, `const`, `abi` |
| `struct` | `fields` (named or indexed tuple fields with types) |
| `enum` | `variants`, each with fields and discriminant |
| `trait` | `supertraits`, `items` |
| `impl` | `self_type`, `trait`, `items` |
| `module` | `contents`, `external` |
| `file` | `contents` |

Common fields include `language`, `kind`, `file`, `name`, `visibility`,
`generics`, `where_clause`, `attributes`, and `docs`; absent information is null
or an empty array. Functions include free functions, methods, and trait
declarations. `where.has_body` can filter functions with or without bodies.
Selectors also accept `files` and `exclude` globs.

Per-rule `context` controls surrounding input:

- `"target"`: the target's fields alone.
- `"enclosing"` (default): also include enclosing inline module, function, impl,
  and trait metadata in `context.enclosing`, ordered outermost first.
- `"file"`: also include the whole source text in `context.file`.

Rules sharing a target and context are batched into one Jev request. Requests run
with bounded concurrency, defaulting to 64. Diagnostics are sorted by source
location and rule ID. JSON output includes the reported model version, choices,
confidence, and probability values exactly as returned, even when they do not sum
to 1. The reported choice is retained even when another option has a higher
probability. Invalid or incomplete Jev responses
fail the run; they are never treated as passing lint results.

This is source-level analysis: macros and `cfg` branches are not expanded,
inferred types and imported symbols are not resolved, and module declarations
do not automatically load additional context from other files. Syntax errors
stop the run before any requests are sent. Jev supplies judgments, not proofs
of correctness or generated explanations.

## Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Keep `Cargo.lock`: the selected rust-analyzer lexer requires `unicode-ident`
1.0.24 to match `unicode-properties`' Unicode version. Dependency upgrades need
to preserve that compatibility.

References: [Jev API](https://docs.typesafe.ai/api),
[confidence](https://docs.typesafe.ai/confidence),
[Rust syntax library](https://docs.rs/ra_ap_syntax/0.0.349/ra_ap_syntax/).
