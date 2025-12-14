mod ast;
mod diagnostics;
mod parser;

pub use ast::*;
pub use diagnostics::*;
pub use parser::{parse_and_lint, parse_query, validate_query, Linter, LinterPipeline};

use napi_derive::napi;

/// Result of parsing a query, exposed to JavaScript
#[napi(object)]
pub struct ParseOutput {
    /// Whether parsing was successful (no errors)
    pub ok: bool,
    /// The parsed AST as JSON (use JSON.parse() in JS to get the typed object)
    pub ast: Option<String>,
    /// List of diagnostics
    pub diagnostics: DiagnosticList,
    /// Query statistics
    pub stats: Option<QueryStats>,
}

/// Parse a tantivy query string.
///
/// Returns a ParseOutput containing:
/// - `ok`: whether parsing succeeded without errors
/// - `ast`: the parsed AST as a JSON string
/// - `diagnostics`: array of diagnostic messages with positions
/// - `stats`: statistics about the query
#[napi]
pub fn parse(query: String) -> ParseOutput {
    let result = parse_query(&query);

    ParseOutput {
        ok: result.is_ok(),
        ast: result.ast.as_ref().map(|ast| {
            serde_json::to_string(ast).unwrap_or_else(|_| "null".to_string())
        }),
        diagnostics: result.diagnostics,
        stats: result.stats,
    }
}

/// Validate a tantivy query string and return only diagnostics.
/// More efficient than `parse` when you only need to check validity.
#[napi]
pub fn validate(query: String) -> DiagnosticList {
    validate_query(&query)
}

/// Check if a query string is valid (no syntax errors).
/// Returns true if the query can be parsed without errors.
#[napi]
pub fn is_valid(query: String) -> bool {
    !validate_query(&query).has_errors()
}

/// Get statistics about a query without returning the full AST.
#[napi]
pub fn get_stats(query: String) -> Option<QueryStats> {
    parse_query(&query).stats
}

/// Format a parsed AST back to a query string (normalized form).
/// Returns None if the query cannot be parsed.
#[napi]
pub fn format(query: String) -> Option<String> {
    let result = parse_query(&query);
    if result.is_ok() {
        result.ast.map(|ast| format_node(&ast))
    } else {
        None
    }
}

fn format_node(node: &QueryNode) -> String {
    match node {
        QueryNode::Leaf(leaf) => format_leaf(leaf),
        QueryNode::Clause(clause) => {
            let parts: Vec<String> = clause
                .members
                .iter()
                .map(|m| {
                    let prefix = match m.occur {
                        Occur::Must => "+",
                        Occur::MustNot => "-",
                        Occur::Should => "",
                    };
                    let inner = format_node(&m.node);
                    if prefix.is_empty() {
                        inner
                    } else {
                        format!("{}{}", prefix, inner)
                    }
                })
                .collect();
            if parts.len() == 1 {
                parts.into_iter().next().unwrap()
            } else {
                format!("({})", parts.join(" "))
            }
        }
        QueryNode::Boost { factor, node } => {
            format!("{}^{}", format_node(node), factor)
        }
    }
}

fn format_leaf(leaf: &LeafNode) -> String {
    match leaf {
        LeafNode::Literal(lit) => {
            let mut s = String::new();
            if let Some(field) = &lit.field {
                s.push_str(field);
                s.push(':');
            }
            if lit.phrase.contains(' ') || lit.delimiter != Delimiter::None {
                s.push('"');
                s.push_str(&lit.phrase);
                s.push('"');
            } else {
                s.push_str(&lit.phrase);
            }
            if lit.slop > 0 {
                s.push('~');
                s.push_str(&lit.slop.to_string());
            }
            if lit.prefix {
                s.push('*');
            }
            s
        }
        LeafNode::All => "*".to_string(),
        LeafNode::Range(r) => {
            let mut s = String::new();
            if let Some(field) = &r.field {
                s.push_str(field);
                s.push(':');
            }
            s.push(match r.lower.bound_type {
                BoundType::Inclusive => '[',
                BoundType::Exclusive => '{',
                BoundType::Unbounded => '[',
            });
            s.push_str(r.lower.value.as_deref().unwrap_or("*"));
            s.push_str(" TO ");
            s.push_str(r.upper.value.as_deref().unwrap_or("*"));
            s.push(match r.upper.bound_type {
                BoundType::Inclusive => ']',
                BoundType::Exclusive => '}',
                BoundType::Unbounded => ']',
            });
            s
        }
        LeafNode::Set(set) => {
            let mut s = String::new();
            if let Some(field) = &set.field {
                s.push_str(field);
                s.push(':');
            }
            s.push_str("IN [");
            s.push_str(&set.elements.join(" "));
            s.push(']');
            s
        }
        LeafNode::Exists(e) => {
            format!("{}:*", e.field)
        }
    }
}

/// Get the version of the parser library
#[napi]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
