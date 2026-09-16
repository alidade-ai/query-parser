use crate::ast::{Delimiter, LeafNode, QueryNode};
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};
use crate::parser::Linter;

/// `AND` / `OR` only act as operators between two operands; anywhere else
/// (`a AND`, `OR b`, `a AND OR b`) the grammar quietly parses the word as a
/// search term. Flag those so the query does not silently search for "AND".
pub struct BareOperator;

impl Linter for BareOperator {
    fn lint(&self, source: &str, ast: &QueryNode) -> DiagnosticList {
        let mut diagnostics = DiagnosticList::new();
        let literal_count = count_literal_operators(ast);
        if literal_count == 0 {
            return diagnostics;
        }

        let bare = find_bare_operators(source);
        if bare.is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "AND/OR is being searched as a literal word; \
                     put a search term on both sides of every operator",
                    Range::from_offsets(source, 0, source.len()),
                )
                .with_code("bare-operator"),
            );
            return diagnostics;
        }

        for (start, end) in bare {
            let token = &source[start..end];
            diagnostics.push(
                Diagnostic::error(
                    format!(
                        "{token} has no term on both sides, so it is searched as the \
                         literal word \"{token}\"; complete the expression or quote it"
                    ),
                    Range::from_offsets(source, start, end),
                )
                .with_code("bare-operator"),
            );
        }
        diagnostics
    }
}

fn count_literal_operators(node: &QueryNode) -> usize {
    match node {
        QueryNode::Leaf(LeafNode::Literal(lit)) => usize::from(
            lit.field.is_none()
                && matches!(lit.delimiter, Delimiter::None)
                && matches!(lit.phrase.as_str(), "AND" | "OR"),
        ),
        QueryNode::Leaf(_) => 0,
        QueryNode::Clause(clause) => clause
            .members
            .iter()
            .map(|member| count_literal_operators(&member.node))
            .sum(),
        QueryNode::Boost { node, .. } => count_literal_operators(node),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Prev {
    Operand,
    Operator,
}

/// Byte ranges of `AND` / `OR` tokens the grammar treats as words: an operator
/// needs an operand before it and whitespace after it.
fn find_bare_operators(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut bare = Vec::new();
    let mut prev: Option<Prev> = None;
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\''
                if bytes[i] == b'"'
                    || i == 0
                    || bytes[i - 1].is_ascii_whitespace()
                    || bytes[i - 1] == b'(' =>
            {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                i += 1;
                prev = Some(Prev::Operand);
            }
            b'(' => {
                prev = None;
                i += 1;
            }
            b')' => {
                prev = Some(Prev::Operand);
                i += 1;
            }
            b'[' => {
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                i += 1;
                prev = Some(Prev::Operand);
            }
            b if b.is_ascii_whitespace() => i += 1,
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'(' | b')' | b'"' | b'[')
                {
                    i += 1;
                }
                match &source[start..i] {
                    "AND" | "OR" => {
                        let followed_by_space = i < bytes.len() && bytes[i].is_ascii_whitespace();
                        if prev == Some(Prev::Operand) && followed_by_space {
                            prev = Some(Prev::Operator);
                        } else {
                            bare.push((start, i));
                            prev = Some(Prev::Operand);
                        }
                    }
                    "NOT" => prev = Some(Prev::Operator),
                    _ => prev = Some(Prev::Operand),
                }
            }
        }
    }
    bare
}
