mod ast;
mod diagnostics;
mod format;
mod lexer;
mod lint;
mod parser;
mod tinql;

pub use ast::*;
pub use diagnostics::*;
pub use format::format_node;
pub use lint::{LintOptions, lint};
pub use parser::{ImplicitOp, Parsed, parse as parse_query};
pub use tinql::{Analysis, TinqlOptions, TinqlOutput, analyze, to_tinql as emit_tinql};

use napi_derive::napi;

/// Result of parsing a query, exposed to JavaScript
#[napi(object)]
pub struct ParseOutput {
    /// Whether the query is valid (no error diagnostics)
    pub ok: bool,
    /// The syntax tree as JSON (use JSON.parse() in JS); nodes carry byte spans
    pub ast: Option<String>,
    /// Diagnostics from parsing, linting and emission checks, ordered by position
    pub diagnostics: DiagnosticList,
    /// Query statistics
    pub stats: Option<QueryStats>,
}

/// Parse and lint a boolean query.
///
/// Runs the same pipeline as `toTinql`, so a query that parses here is
/// guaranteed to transpile.
#[napi]
pub fn parse(query: String, options: Option<TinqlOptions>) -> ParseOutput {
    let analysis = analyze(&query, &options.unwrap_or_default());
    ParseOutput {
        ok: analysis.is_ok(),
        ast: analysis
            .ast
            .as_ref()
            .map(|ast| serde_json::to_string(ast).unwrap_or_else(|_| "null".to_string())),
        diagnostics: analysis.diagnostics,
        stats: analysis.stats,
    }
}

/// Validate a boolean query and return only diagnostics.
#[napi]
pub fn validate(query: String, options: Option<TinqlOptions>) -> DiagnosticList {
    analyze(&query, &options.unwrap_or_default()).diagnostics
}

/// Check if a query string is valid (no error diagnostics).
#[napi]
pub fn is_valid(query: String, options: Option<TinqlOptions>) -> bool {
    analyze(&query, &options.unwrap_or_default()).is_ok()
}

/// Get statistics about a query without returning the full AST.
#[napi]
pub fn get_stats(query: String, options: Option<TinqlOptions>) -> Option<QueryStats> {
    analyze(&query, &options.unwrap_or_default()).stats
}

/// Rewrite a query in canonical form: explicit AND/OR/NOT, original grouping.
/// Returns null when the query has errors.
#[napi]
pub fn format(query: String, options: Option<TinqlOptions>) -> Option<String> {
    let analysis = analyze(&query, &options.unwrap_or_default());
    if analysis.is_ok() {
        analysis.ast.map(|ast| format_node(&ast))
    } else {
        None
    }
}

/// Transpile a boolean query to a TINQL string for `content ==> $1`.
///
/// Parses and lints the query (same pipeline as `parse`), then emits one
/// TINQL string with explicit parentheses, `AND NOT` for negation and
/// `* AND NOT (…)` for negation-only queries. Returns `ok: false` with
/// diagnostics when the query has any error.
#[napi]
pub fn to_tinql(query: String, options: Option<TinqlOptions>) -> TinqlOutput {
    emit_tinql(&query, &options.unwrap_or_default())
}

/// Get the version of the parser library
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
