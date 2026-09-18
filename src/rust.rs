// Copyright (C) 2026 Eriskii
// SPDX-License-Identifier: AGPL-3.0-only
// See LICENSE for the full license text.

use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail};
use ra_ap_syntax::{
    AstNode, AstToken, Edition, SourceFile, SyntaxKind, SyntaxNode, TextRange,
    ast::{self, HasTypeBounds},
    match_ast,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::InputContext;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    File,
}

impl TargetKind {
    fn of(node: &SyntaxNode) -> Option<Self> {
        Some(match node.kind() {
            SyntaxKind::FN => Self::Function,
            SyntaxKind::STRUCT => Self::Struct,
            SyntaxKind::ENUM => Self::Enum,
            SyntaxKind::TRAIT => Self::Trait,
            SyntaxKind::IMPL => Self::Impl,
            SyntaxKind::MODULE => Self::Module,
            SyntaxKind::SOURCE_FILE => Self::File,
            _ => return None,
        })
    }
}

/// Byte offsets are zero-based, end-exclusive. Lines and Unicode columns are one-based.
#[derive(Debug, Clone, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

pub struct Target {
    pub kind: TargetKind,
    pub name: String,
    pub span: Span,
    pub range: Span,
    pub has_body: bool,
    state: Value,
    enclosing: Vec<Value>,
}

impl Target {
    pub fn input(&self, context: InputContext, source: &str) -> Value {
        let mut state = self.state.clone();
        match context {
            InputContext::Target => {}
            InputContext::Enclosing => state["context"] = json!({ "enclosing": self.enclosing }),
            InputContext::File => {
                state["context"] = json!({ "enclosing": self.enclosing, "file": source })
            }
        }
        state
    }
}

pub fn extract(
    source: &str,
    edition: Edition,
    kinds: &BTreeSet<TargetKind>,
) -> Result<Vec<Target>> {
    let parsed = SourceFile::parse(source, edition);
    let lines = Lines::new(source);
    let errors = parsed.errors();
    if !errors.is_empty() {
        let errors = errors
            .iter()
            .map(|error| {
                let span = lines.span(error.range());
                format!("{}:{}: {error}", span.line, span.column)
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!("Rust syntax errors:\n{errors}");
    }
    parsed
        .tree()
        .syntax()
        .descendants()
        .filter_map(|node| {
            TargetKind::of(&node)
                .filter(|kind| kinds.contains(kind))
                .map(|kind| (node, kind))
        })
        .map(|(node, kind)| {
            let mut state = describe(&node);
            state["language"] = json!("rust");
            state["kind"] = json!(kind);
            let name = state["name"]
                .as_str()
                .unwrap_or(match kind {
                    TargetKind::File => "<file>",
                    TargetKind::Impl => "<impl>",
                    _ => "<unnamed>",
                })
                .to_owned();
            let has_body = !state["body"].is_null();
            let range = node
                .children()
                .find_map(ast::Name::cast)
                .map_or(node.text_range(), |name| name.syntax().text_range());
            Ok(Target {
                kind,
                name,
                has_body,
                span: lines.span(range),
                range: lines.span(node.text_range()),
                state,
                enclosing: enclosing(&node),
            })
        })
        .collect()
}

fn text<N: AstNode>(node: Option<N>) -> Option<String> {
    node.map(|node| node.syntax().text().to_string())
}

fn child_text<N: AstNode>(node: &SyntaxNode) -> Option<String> {
    text(node.children().find_map(N::cast))
}

fn common(node: &SyntaxNode) -> Value {
    json!({
        "name": child_text::<ast::Name>(node),
        "visibility": child_text::<ast::Visibility>(node),
        "generics": child_text::<ast::GenericParamList>(node),
        "where_clause": child_text::<ast::WhereClause>(node),
        "attributes": node.children().filter_map(ast::Attr::cast).map(|attr| attr.to_string()).collect::<Vec<_>>(),
        "docs": node.children_with_tokens().filter_map(|element| element.into_token())
            .filter_map(ast::Comment::cast).filter(ast::Comment::is_doc)
            .map(|comment| comment.syntax().text().to_owned()).collect::<Vec<_>>()
    })
}

fn describe(node: &SyntaxNode) -> Value {
    let mut state = common(node);
    match_ast! {
        match node {
            ast::Fn(function) => {
                let parameters = function.param_list();
                state["receiver"] = json!(text(parameters.as_ref().and_then(ast::ParamList::self_param)));
                state["params"] = json!(parameters.into_iter().flat_map(|params| params.params()).map(|param| {
                    json!({ "pattern": text(param.pat()), "type": text(param.ty()) })
                }).collect::<Vec<_>>());
                state["return_type"] = json!(text(function.ret_type().and_then(|ret| ret.ty())).unwrap_or_else(|| "()".into()));
                state["body"] = json!(text(function.body()));
                state["async"] = json!(function.async_token().is_some());
                state["unsafe"] = json!(function.unsafe_token().is_some());
                state["const"] = json!(function.const_token().is_some());
                state["abi"] = json!(text(function.abi()));
            },
            ast::Struct(item) => state["fields"] = fields(item.field_list()),
            ast::Enum(item) => {
                state["variants"] = json!(item.variant_list().into_iter().flat_map(|list| list.variants()).map(|variant| {
                    let mut value = common(variant.syntax());
                    value["fields"] = fields(variant.field_list());
                    value["discriminant"] = json!(text(variant.const_arg()));
                    value
                }).collect::<Vec<_>>());
            },
            ast::Trait(item) => {
                state["supertraits"] = json!(text(item.type_bound_list()));
                state["items"] = json!(item.assoc_item_list().into_iter().flat_map(|list| list.assoc_items())
                    .map(|item| item.to_string()).collect::<Vec<_>>());
            },
            ast::Impl(item) => {
                state["self_type"] = json!(text(item.self_ty()));
                state["trait"] = json!(text(item.trait_()));
                state["items"] = json!(item.assoc_item_list().into_iter().flat_map(|list| list.assoc_items())
                    .map(|item| item.to_string()).collect::<Vec<_>>());
            },
            ast::Module(item) => {
                state["contents"] = json!(text(item.item_list()));
                state["external"] = json!(item.item_list().is_none());
            },
            ast::SourceFile(_) => state["contents"] = json!(node.text().to_string()),
            _ => {}
        }
    }
    state
}

fn fields(list: Option<ast::FieldList>) -> Value {
    match list {
        Some(ast::FieldList::RecordFieldList(list)) => json!(
            list.fields()
                .map(|field| {
                    let mut value = common(field.syntax());
                    value["type"] = json!(text(field.ty()));
                    value
                })
                .collect::<Vec<_>>()
        ),
        Some(ast::FieldList::TupleFieldList(list)) => json!(
            list.fields()
                .enumerate()
                .map(|(index, field)| {
                    let mut value = common(field.syntax());
                    value["index"] = json!(index);
                    value["type"] = json!(text(field.ty()));
                    value
                })
                .collect::<Vec<_>>()
        ),
        None => json!([]),
    }
}

fn enclosing(node: &SyntaxNode) -> Vec<Value> {
    let mut contexts = node
        .ancestors()
        .skip(1)
        .filter_map(|node| {
            let kind = TargetKind::of(&node)?;
            match kind {
                TargetKind::Module
                | TargetKind::Function
                | TargetKind::Trait
                | TargetKind::Impl => {
                    let mut context = common(&node);
                    context["kind"] = json!(kind);
                    if let Some(item) = ast::Impl::cast(node.clone()) {
                        context["self_type"] = json!(text(item.self_ty()));
                        context["trait"] = json!(text(item.trait_()));
                    }
                    if let Some(item) = ast::Trait::cast(node) {
                        context["supertraits"] = json!(text(item.type_bound_list()));
                    }
                    Some(context)
                }
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    contexts.reverse();
    contexts
}

struct Lines<'a> {
    source: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(source: &'a str) -> Self {
        let starts = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(index, _)| index + 1))
            .collect();
        Self { source, starts }
    }

    fn position(&self, offset: usize) -> (usize, usize) {
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        (
            line + 1,
            self.source[self.starts[line]..offset].chars().count() + 1,
        )
    }

    fn span(&self, range: TextRange) -> Span {
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        let (line, column) = self.position(start);
        let (end_line, end_column) = self.position(end);
        Span {
            start,
            end,
            line,
            column,
            end_line,
            end_column,
        }
    }
}

/// Read Cargo's declared edition without building the project or expanding macros.
pub fn edition_for(path: &Path) -> Result<Edition> {
    for directory in path.parent().into_iter().flat_map(Path::ancestors) {
        let manifest = directory.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let manifest = read_manifest(&manifest)?;
        let Some(package) = manifest.get("package") else {
            continue;
        };
        return match package.get("edition") {
            None => Ok(Edition::Edition2015),
            Some(edition)
                if edition.get("workspace").and_then(toml::Value::as_bool) == Some(true) =>
            {
                let explicit = package
                    .get("workspace")
                    .and_then(toml::Value::as_str)
                    .map(|path| directory.join(path));
                let workspace_dirs = explicit
                    .into_iter()
                    .chain(directory.ancestors().map(Path::to_path_buf));
                for workspace_dir in workspace_dirs {
                    let path = workspace_dir.join("Cargo.toml");
                    if !path.is_file() {
                        continue;
                    }
                    let manifest = read_manifest(&path)?;
                    if let Some(workspace) = manifest.get("workspace") {
                        let edition = workspace.get("package").and_then(|package| package.get("edition"))
                            .context("workspace.package.edition is required for edition.workspace = true")?;
                        return parse_edition(edition);
                    }
                }
                bail!(
                    "cannot find workspace.package.edition for {}",
                    path.display()
                )
            }
            Some(edition) => parse_edition(edition),
        };
    }
    Ok(Edition::Edition2024)
}

fn read_manifest(path: &Path) -> Result<toml::Value> {
    let source =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    toml::from_str(&source).with_context(|| format!("invalid Cargo manifest {}", path.display()))
}

fn parse_edition(value: &toml::Value) -> Result<Edition> {
    match value.as_str() {
        Some("2015") => Ok(Edition::Edition2015),
        Some("2018") => Ok(Edition::Edition2018),
        Some("2021") => Ok(Edition::Edition2021),
        Some("2024") => Ok(Edition::Edition2024),
        _ => bail!("unsupported Rust edition {value}"),
    }
}
