use napi_derive::napi;
use serde::{Deserialize, Serialize};

use crate::ast::{Clause, Delimiter, LeafNode, Literal, Occur, QueryNode};
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};
use crate::linters::default_pipeline;
use crate::parser::parse_and_lint;

/// Options for tsquery emission
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct TsqueryOptions {
    /// Field assigned to bare (unscoped) terms. Defaults to "content_exact".
    pub default_field: Option<String>,
    /// If provided, field-scoped terms referencing other fields produce an error.
    pub allowed_fields: Option<Vec<String>>,
    /// Treat bare adjacent terms as AND (tantivy conjunction_mode). Defaults to true.
    pub conjunction_mode: Option<bool>,
    /// Maximum phrase slop expanded into `<N>` alternatives. Defaults to 5.
    pub max_slop: Option<u32>,
}

/// Result of transpiling a query to tsquery form
#[napi(object)]
pub struct TsqueryOutput {
    /// Whether transpilation succeeded (no error diagnostics)
    pub ok: bool,
    /// JSON expression tree (use JSON.parse() in JS). Nodes:
    /// {"type":"match","field":string,"tsquery":string}
    /// {"type":"and","children":[...]} | {"type":"or","children":[...]}
    /// {"type":"not","child":{...}}
    /// The `tsquery` string is intended for `to_tsquery(<config>, $1)`.
    pub expression: Option<String>,
    /// Diagnostics from parsing, linting, and emission
    pub diagnostics: DiagnosticList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TsExpr {
    Match { field: String, tsquery: String },
    And { children: Vec<TsExpr> },
    Or { children: Vec<TsExpr> },
    Not { child: Box<TsExpr> },
}

struct Emitter<'a> {
    source: &'a str,
    default_field: String,
    allowed_fields: Option<Vec<String>>,
    conjunction_mode: bool,
    max_slop: u32,
    diagnostics: DiagnosticList,
}

pub fn emit_tsquery(source: &str, options: TsqueryOptions) -> TsqueryOutput {
    let pipeline = default_pipeline();
    let parsed = parse_and_lint(source, &pipeline);
    let mut diagnostics = parsed.diagnostics;

    if diagnostics.has_errors() {
        return TsqueryOutput {
            ok: false,
            expression: None,
            diagnostics,
        };
    }

    let Some(ast) = parsed.ast else {
        return TsqueryOutput {
            ok: false,
            expression: None,
            diagnostics,
        };
    };

    // `a AND b OR c` and `(a AND b) OR c` parse to the same AST (the grammar
    // applies precedence grouping), so implicit precedence can only be caught
    // in the source text. Force explicit parentheses instead of making users
    // learn the implicit rules.
    if let Some(offset) = find_mixed_and_or(source) {
        diagnostics.push(
            Diagnostic::error(
                "Mixing AND and OR at the same level relies on implicit precedence; \
                 add parentheses to make the grouping explicit",
                Range::from_offsets(source, offset, offset + 2),
            )
            .with_code("mixed-and-or"),
        );
        return TsqueryOutput {
            ok: false,
            expression: None,
            diagnostics,
        };
    }

    let conjunction_mode = options.conjunction_mode.unwrap_or(true);

    // Bare-space adjacency is warned from the source text so each squiggle
    // lands on the two adjacent operands instead of the whole query (the
    // grammar's AST carries no positions).
    for (start, end) in find_implicit_adjacencies(source) {
        diagnostics.push(
            Diagnostic::warning(
                if conjunction_mode {
                    "Space-separated terms are combined with AND; \
                     use an explicit AND to make this clear"
                } else {
                    "Space-separated terms are combined with OR; \
                     use an explicit OR to make this clear"
                },
                Range::from_offsets(source, start, end),
            )
            .with_code("implicit-operator"),
        );
    }

    let mut emitter = Emitter {
        source,
        default_field: options
            .default_field
            .unwrap_or_else(|| "content_exact".to_string()),
        allowed_fields: options.allowed_fields,
        conjunction_mode,
        max_slop: options.max_slop.unwrap_or(5),
        diagnostics: DiagnosticList::new(),
    };

    let expr = emitter.emit(&ast);
    diagnostics.extend(emitter.diagnostics.items);

    let expr = match expr {
        Ok(Some(expr)) => Some(expr),
        Ok(None) => {
            diagnostics.push(
                Diagnostic::error(
                    "Query produced no searchable terms",
                    Range::from_offsets(source, 0, source.len()),
                )
                .with_code("empty-query"),
            );
            None
        }
        Err(()) => None,
    };

    let ok = !diagnostics.has_errors() && expr.is_some();
    TsqueryOutput {
        ok,
        expression: if ok {
            expr.map(|e| serde_json::to_string(&e).unwrap_or_else(|_| "null".to_string()))
        } else {
            None
        },
        diagnostics,
    }
}

impl Emitter<'_> {
    fn full_range(&self) -> Range {
        Range::from_offsets(self.source, 0, self.source.len())
    }

    // Best-effort localization: the AST carries no positions, so point the
    // diagnostic at the first occurrence of the offending text.
    fn range_of(&self, needle: &str) -> Range {
        match self.source.find(needle) {
            Some(offset) => Range::from_offsets(self.source, offset, offset + needle.len()),
            None => self.full_range(),
        }
    }

    fn emit(&mut self, node: &QueryNode) -> Result<Option<TsExpr>, ()> {
        match node {
            QueryNode::Leaf(leaf) => self.emit_leaf(leaf),
            QueryNode::Clause(clause) => self.emit_clause(clause),
            QueryNode::Boost { node, .. } => {
                self.diagnostics.push(
                    Diagnostic::warning(
                        "Boost (^) has no effect and is ignored",
                        self.range_of("^"),
                    )
                    .with_code("boost-ignored"),
                );
                self.emit(node)
            }
        }
    }

    fn emit_clause(&mut self, clause: &Clause) -> Result<Option<TsExpr>, ()> {
        let mut musts: Vec<TsExpr> = Vec::new();
        let mut shoulds: Vec<TsExpr> = Vec::new();
        let mut must_nots: Vec<TsExpr> = Vec::new();

        for member in &clause.members {
            let effective = match member.occur {
                Occur::Default => {
                    if self.conjunction_mode {
                        Occur::Must
                    } else {
                        Occur::Should
                    }
                }
                occur => occur,
            };
            let Some(expr) = self.emit(&member.node)? else {
                continue;
            };
            match effective {
                Occur::Must => musts.push(expr),
                Occur::Should => shoulds.push(expr),
                Occur::MustNot => must_nots.push(expr),
                Occur::Default => unreachable!(),
            }
        }

        // A clause mixing required (AND'd / bare) members with optional (OR'd)
        // members is ambiguous — tantivy would silently treat the OR'd terms as
        // optional. Reject and demand explicit grouping instead.
        if !musts.is_empty() && !shoulds.is_empty() {
            self.diagnostics.push(
                Diagnostic::error(
                    "Ambiguous mix of AND'd and OR'd terms; \
                     add parentheses to group the OR'd terms",
                    self.full_range(),
                )
                .with_code("mixed-and-or"),
            );
            return Err(());
        }

        let mut positive: Vec<TsExpr> = Vec::new();
        if !musts.is_empty() {
            positive.extend(musts);
        } else if !shoulds.is_empty() {
            positive.push(merge(shoulds, Op::Or));
        }

        if positive.is_empty() {
            if must_nots.is_empty() {
                return Ok(None);
            }
            // Negation-only groups emit `!(…)` (everything-except). Under
            // tantivy these silently matched nothing; the divergence is
            // deliberate and matches the obvious intent, so it is not warned.
            let negated = must_nots.into_iter().map(not_wrap).collect();
            return Ok(Some(merge(negated, Op::And)));
        }

        positive.extend(must_nots.into_iter().map(not_wrap));
        Ok(Some(merge(positive, Op::And)))
    }

    fn emit_leaf(&mut self, leaf: &LeafNode) -> Result<Option<TsExpr>, ()> {
        match leaf {
            LeafNode::Literal(lit) => self.emit_literal(lit),
            LeafNode::All => {
                self.diagnostics.push(
                    Diagnostic::error("Match-all (*) is not supported", self.full_range())
                        .with_code("no-wildcard"),
                );
                Err(())
            }
            LeafNode::Range(_) => {
                self.diagnostics.push(
                    Diagnostic::error(
                        "Range queries ([a TO b]) are not supported",
                        self.full_range(),
                    )
                    .with_code("no-range"),
                );
                Err(())
            }
            LeafNode::Exists(_) => {
                self.diagnostics.push(
                    Diagnostic::error(
                        "Exists queries (field:*) are not supported",
                        self.full_range(),
                    )
                    .with_code("no-wildcard"),
                );
                Err(())
            }
            LeafNode::Set(set) => {
                let field = self.resolve_field(set.field.as_deref())?;
                let members: Vec<TsExpr> = set
                    .elements
                    .iter()
                    .filter(|e| !e.trim().is_empty())
                    .map(|e| TsExpr::Match {
                        field: field.clone(),
                        tsquery: quote_lexeme(e.trim()),
                    })
                    .collect();
                if members.is_empty() {
                    return Ok(None);
                }
                Ok(Some(merge(members, Op::Or)))
            }
        }
    }

    fn emit_literal(&mut self, lit: &Literal) -> Result<Option<TsExpr>, ()> {
        let field = self.resolve_field(lit.field.as_deref())?;
        let words: Vec<&str> = lit.phrase.split_whitespace().collect();
        if words.is_empty() {
            return Ok(None);
        }

        let is_phrase = lit.delimiter != Delimiter::None || words.len() > 1;
        let tsquery = if !is_phrase {
            let mut term = quote_lexeme(words[0]);
            if lit.prefix {
                term.push_str(":*");
            }
            term
        } else if lit.slop == 0 {
            // A single quoted multi-word lexeme: to_tsquery() tokenizes it through
            // the text-search config and connects the lexemes with <-> itself,
            // keeping stemming/stopword positions consistent with the tsvector.
            quote_lexeme(&words.join(" "))
        } else if words.len() == 2 && lit.slop <= self.max_slop {
            // Tantivy slop N allows up to N extra positions between the two
            // (ordered) words; tsquery <D> is an exact distance, so expanding
            // to distances 1..=N+1 is an exact translation.
            let a = quote_lexeme(words[0]);
            let b = quote_lexeme(words[1]);
            let alternatives: Vec<String> = (1..=lit.slop + 1)
                .map(|d| {
                    if d == 1 {
                        format!("{a} <-> {b}")
                    } else {
                        format!("{a} <{d}> {b}")
                    }
                })
                .collect();
            format!("({})", alternatives.join(" | "))
        } else {
            let needle = format!("\"{}\"~{}", lit.phrase, lit.slop);
            let range = match self.source.find(&needle) {
                Some(offset) => Range::from_offsets(self.source, offset, offset + needle.len()),
                None => self.range_of(&format!("~{}", lit.slop)),
            };
            self.diagnostics.push(
                Diagnostic::error(
                    if words.len() > 2 {
                        format!(
                            "Proximity (~{}) is only supported for two-word phrases",
                            lit.slop
                        )
                    } else {
                        format!(
                            "Proximity distance ~{} exceeds the maximum of {}",
                            lit.slop, self.max_slop
                        )
                    },
                    range,
                )
                .with_code("slop-unsupported"),
            );
            return Err(());
        };

        Ok(Some(TsExpr::Match { field, tsquery }))
    }

    fn resolve_field(&mut self, field: Option<&str>) -> Result<String, ()> {
        let field = field.unwrap_or(&self.default_field).to_string();
        if let Some(allowed) = &self.allowed_fields
            && !allowed.contains(&field)
        {
            self.diagnostics.push(
                Diagnostic::error(
                    format!("Unknown field \"{field}\""),
                    self.range_of(&format!("{field}:")),
                )
                .with_code("unknown-field"),
            );
            return Err(());
        }
        Ok(field)
    }
}

enum Op {
    And,
    Or,
}

fn merge(mut children: Vec<TsExpr>, op: Op) -> TsExpr {
    if children.len() == 1 {
        return children.remove(0);
    }

    let same_field = match children.first() {
        Some(TsExpr::Match { field, .. }) => {
            let first = field.clone();
            children
                .iter()
                .all(|c| matches!(c, TsExpr::Match { field, .. } if *field == first))
        }
        _ => false,
    };

    if same_field {
        let separator = match op {
            Op::And => " & ",
            Op::Or => " | ",
        };
        let field = match &children[0] {
            TsExpr::Match { field, .. } => field.clone(),
            _ => unreachable!(),
        };
        let parts: Vec<String> = children
            .into_iter()
            .map(|c| match c {
                TsExpr::Match { tsquery, .. } => wrap_operand(&tsquery),
                _ => unreachable!(),
            })
            .collect();
        return TsExpr::Match {
            field,
            tsquery: parts.join(separator),
        };
    }

    match op {
        Op::And => TsExpr::And { children },
        Op::Or => TsExpr::Or { children },
    }
}

fn not_wrap(expr: TsExpr) -> TsExpr {
    match expr {
        TsExpr::Match { field, tsquery } => TsExpr::Match {
            field,
            tsquery: format!("!({tsquery})"),
        },
        other => TsExpr::Not {
            child: Box::new(other),
        },
    }
}

fn wrap_operand(tsquery: &str) -> String {
    let atomic = is_single_lexeme(tsquery)
        || is_fully_parenthesized(tsquery)
        || (tsquery.starts_with('!') && is_fully_parenthesized(&tsquery[1..]));
    if atomic {
        tsquery.to_string()
    } else {
        format!("({tsquery})")
    }
}

fn is_single_lexeme(tsquery: &str) -> bool {
    let inner = tsquery.strip_suffix(":*").unwrap_or(tsquery);
    inner.len() >= 2
        && inner.starts_with('\'')
        && inner.ends_with('\'')
        && !inner[1..inner.len() - 1].replace("''", "").contains('\'')
}

fn is_fully_parenthesized(tsquery: &str) -> bool {
    if !(tsquery.starts_with('(') && tsquery.ends_with(')')) {
        return false;
    }
    let mut depth = 0usize;
    for (i, c) in tsquery.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && i != tsquery.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn quote_lexeme(word: &str) -> String {
    format!("'{}'", word.replace('\'', "''"))
}

/// Scan the raw source for AND and OR keywords appearing at the same paren
/// depth. Returns the byte offset of the second operator kind if found.
/// Source-level scan for bare-space adjacency: two operands (terms, quoted
/// phrases, or parenthesized groups) with no operator keyword between them.
/// Returns one `(start, end)` span per gap, covering both adjacent operands,
/// so warnings land on the offending spot instead of the whole query.
/// Operands with an explicit `+`/`-` occur prefix on both sides don't count —
/// the operator is explicit there.
fn find_implicit_adjacencies(source: &str) -> Vec<(usize, usize)> {
    #[derive(Clone, Copy)]
    struct Operand {
        start: usize,
        bare: bool,
    }

    let bytes = source.as_bytes();
    let mut gaps: Vec<(usize, usize)> = Vec::new();
    // (start, bare) of the previous completed operand at the current depth
    let mut prev: Option<Operand> = None;
    // stack of `prev` values for enclosing paren levels + the group start
    let mut stack: Vec<(Option<Operand>, usize, bool)> = Vec::new();
    // a dangling `field:` or `+`/`-` prefix waiting for its operand
    let mut pending_prefix: Option<usize> = None;
    let mut pending_signed = false;
    let mut i = 0;

    let complete =
        |prev: &mut Option<Operand>, gaps: &mut Vec<(usize, usize)>, op: Operand, end: usize| {
            if let Some(p) = *prev
                && (p.bare || op.bare)
            {
                gaps.push((p.start, end));
            }
            *prev = Some(op);
        };

    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        match b {
            b'"' | b'\'' if b == b'"' || i == 0 || bytes[i - 1].is_ascii_whitespace() => {
                let quote = b;
                let start = pending_prefix.take().unwrap_or(i);
                let bare = !pending_signed;
                pending_signed = false;
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'~' {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                complete(&mut prev, &mut gaps, Operand { start, bare }, i);
            }
            b'(' => {
                let start = pending_prefix.take().unwrap_or(i);
                let bare = !pending_signed;
                pending_signed = false;
                stack.push((prev, start, bare));
                prev = None;
                i += 1;
            }
            b')' => {
                if let Some((outer, start, bare)) = stack.pop() {
                    prev = outer;
                    complete(&mut prev, &mut gaps, Operand { start, bare }, i + 1);
                }
                i += 1;
            }
            b'[' => {
                // set/range bracket blocks are their own syntax; skip wholesale
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                prev = None;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && bytes[i] != b'('
                    && bytes[i] != b')'
                    && bytes[i] != b'"'
                    && bytes[i] != b'['
                {
                    i += 1;
                }
                let token = &source[start..i];
                match token {
                    "AND" | "OR" | "NOT" | "TO" | "IN" => {
                        prev = None;
                        pending_prefix = None;
                        pending_signed = false;
                    }
                    "+" | "-" => {
                        pending_signed = true;
                        pending_prefix.get_or_insert(start);
                    }
                    _ if token.ends_with(':')
                        && i < bytes.len()
                        && (bytes[i] == b'"' || bytes[i] == b'(') =>
                    {
                        // field prefix attached to a following quote/group
                        pending_prefix.get_or_insert(start);
                    }
                    _ => {
                        let opstart = pending_prefix.take().unwrap_or(start);
                        let signed =
                            pending_signed || token.starts_with('+') || token.starts_with('-');
                        pending_signed = false;
                        complete(
                            &mut prev,
                            &mut gaps,
                            Operand {
                                start: opstart,
                                bare: !signed,
                            },
                            i,
                        );
                    }
                }
            }
        }
    }
    gaps
}

fn find_mixed_and_or(source: &str) -> Option<usize> {
    #[derive(Default, Clone, Copy)]
    struct Seen {
        and: bool,
        or: bool,
    }

    let bytes = source.as_bytes();
    let mut depth: usize = 0;
    let mut seen: Vec<Seen> = vec![Seen::default()];
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        if in_double_quote {
            if b == b'"' {
                in_double_quote = false;
            }
            i += 1;
            continue;
        }
        if in_single_quote {
            if b == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }

        match b {
            b'"' => {
                in_double_quote = true;
                i += 1;
            }
            // Only treat ' as a phrase delimiter at a token boundary, so
            // apostrophes inside words (o'brien) don't derail the scan.
            b'\'' if i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b'(' => {
                in_single_quote = true;
                i += 1;
            }
            b'(' => {
                depth += 1;
                if seen.len() <= depth {
                    seen.push(Seen::default());
                }
                seen[depth] = Seen::default();
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ if b.is_ascii_whitespace() => {
                i += 1;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && bytes[i] != b'('
                    && bytes[i] != b')'
                    && bytes[i] != b'"'
                {
                    i += 1;
                }
                match &source[start..i] {
                    "AND" => {
                        if seen[depth].or {
                            return Some(start);
                        }
                        seen[depth].and = true;
                    }
                    "OR" => {
                        if seen[depth].and {
                            return Some(start);
                        }
                        seen[depth].or = true;
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit(query: &str) -> TsqueryOutput {
        emit_tsquery(query, TsqueryOptions::default())
    }

    fn expression(output: &TsqueryOutput) -> TsExpr {
        assert!(
            output.ok,
            "expected ok output, diagnostics: {:?}",
            output.diagnostics.items
        );
        serde_json::from_str(output.expression.as_ref().unwrap()).unwrap()
    }

    fn match_parts(expr: &TsExpr) -> (&str, &str) {
        match expr {
            TsExpr::Match { field, tsquery } => (field, tsquery),
            other => panic!("expected match node, got {other:?}"),
        }
    }

    #[test]
    fn bare_terms_are_anded_in_conjunction_mode_with_warning() {
        let out = emit("apple banana");
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "'apple' & 'banana'"));
        assert!(
            out.diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("implicit-operator"))
        );
    }

    #[test]
    fn explicit_operators_do_not_warn() {
        let out = emit("apple AND banana");
        assert!(out.ok);
        assert!(out.diagnostics.items.is_empty());
    }

    #[test]
    fn explicit_or_stays_or() {
        let out = emit("apple OR banana");
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "'apple' | 'banana'"));
    }

    #[test]
    fn explicit_and() {
        let out = emit("apple AND banana");
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "'apple' & 'banana'"));
    }

    #[test]
    fn negation() {
        let out = emit("apple AND NOT banana");
        let expr = expression(&out);
        assert_eq!(
            match_parts(&expr),
            ("content_exact", "'apple' & !('banana')")
        );
    }

    #[test]
    fn quoted_phrase_stays_single_lexeme() {
        let out = emit("\"health care\"");
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "'health care'"));
    }

    #[test]
    fn nested_groups() {
        let out = emit("(apple OR banana) AND cherry");
        let expr = expression(&out);
        assert_eq!(
            match_parts(&expr),
            ("content_exact", "('apple' | 'banana') & 'cherry'")
        );
    }

    #[test]
    fn field_scoped_terms_split_by_field() {
        let out = emit("content:apple AND banana");
        let expr = expression(&out);
        match expr {
            TsExpr::And { children } => {
                assert_eq!(children.len(), 2);
                assert_eq!(match_parts(&children[0]), ("content", "'apple'"));
                assert_eq!(match_parts(&children[1]), ("content_exact", "'banana'"));
            }
            other => panic!("expected and node, got {other:?}"),
        }
    }

    #[test]
    fn unparenthesized_and_or_mix_errors() {
        let out = emit("apple AND banana OR cherry");
        assert!(!out.ok);
        assert!(
            out.diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("mixed-and-or"))
        );
    }

    #[test]
    fn parenthesized_and_or_mix_is_explicit_and_ok() {
        let out = emit("(apple AND banana) OR cherry");
        let expr = expression(&out);
        assert_eq!(
            match_parts(&expr),
            ("content_exact", "('apple' & 'banana') | 'cherry'")
        );
    }

    #[test]
    fn ambiguous_group_plus_bare_term_errors() {
        let out = emit("apple AND banana cherry");
        assert!(!out.ok);
        assert!(
            out.diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("mixed-and-or"))
        );
    }

    #[test]
    fn quoted_operators_are_not_operators() {
        let out = emit("\"apple AND banana\" OR \"cherry OR plum\"");
        assert!(out.ok, "diagnostics: {:?}", out.diagnostics.items);
        let expr = expression(&out);
        assert_eq!(
            match_parts(&expr),
            ("content_exact", "'apple AND banana' | 'cherry OR plum'")
        );
    }

    #[test]
    fn slop_expands_to_distance_alternatives() {
        let out = emit("\"health care\"~2");
        let expr = expression(&out);
        assert_eq!(
            match_parts(&expr),
            (
                "content_exact",
                "('health' <-> 'care' | 'health' <2> 'care' | 'health' <3> 'care')"
            )
        );
    }

    #[test]
    fn slop_on_long_phrase_errors() {
        let out = emit("\"one two three\"~2");
        assert!(!out.ok);
        assert!(
            out.diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("slop-unsupported"))
        );
    }

    #[test]
    fn slop_over_max_errors() {
        let out = emit("\"health care\"~9");
        assert!(!out.ok);
        assert!(
            out.diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("slop-unsupported"))
        );
    }

    #[test]
    fn max_slop_is_configurable() {
        let out = emit_tsquery(
            "\"health care\"~6",
            TsqueryOptions {
                max_slop: Some(10),
                ..Default::default()
            },
        );
        assert!(out.ok);
    }

    #[test]
    fn negation_only_query_emits_without_warning() {
        let out = emit("NOT apple");
        assert!(out.ok);
        assert!(out.diagnostics.items.is_empty());
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "!('apple')"));
    }

    #[test]
    fn negation_only_group_inside_or_emits_without_warning() {
        let out = emit("(apple NOT (banana OR cherry)) OR mango");
        assert!(out.ok);
        assert!(out.diagnostics.items.is_empty());
    }

    #[test]
    fn implicit_warning_is_localized_to_the_gap() {
        let source = r#"alpha OR "chat bot" "frontier model" OR beta"#;
        let out = emit(source);
        let warnings: Vec<_> = out
            .diagnostics
            .items
            .iter()
            .filter(|d| d.code.as_deref() == Some("implicit-operator"))
            .collect();
        assert_eq!(warnings.len(), 1);
        let range = &warnings[0].range;
        let start = source.find(r#""chat bot""#).unwrap();
        let end = source.find(r#""frontier model""#).unwrap() + r#""frontier model""#.len();
        assert_eq!(range.start.offset, start as u32);
        assert_eq!(range.end.offset, end as u32);
    }

    #[test]
    fn each_adjacency_gap_warns_separately() {
        let out = emit("alpha beta gamma");
        let count = out
            .diagnostics
            .items
            .iter()
            .filter(|d| d.code.as_deref() == Some("implicit-operator"))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn explicit_not_adjacency_does_not_warn() {
        let out = emit("apple NOT banana");
        assert!(out.ok);
        assert!(out.diagnostics.items.is_empty());
    }

    #[test]
    fn signed_operands_do_not_warn() {
        let out = emit("+apple +banana");
        assert!(out.ok);
        assert!(out.diagnostics.items.is_empty());
    }

    #[test]
    fn slop_error_is_localized() {
        let source = r#"apple AND "one two three"~2"#;
        let out = emit(source);
        assert!(!out.ok);
        let diag = out
            .diagnostics
            .items
            .iter()
            .find(|d| d.code.as_deref() == Some("slop-unsupported"))
            .unwrap();
        let start = source.find(r#""one two three"~2"#).unwrap();
        assert_eq!(diag.range.start.offset, start as u32);
        assert_eq!(diag.range.end.offset, source.len() as u32);
    }

    #[test]
    fn apostrophes_are_escaped() {
        let out = emit("\"o'brien\"");
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "'o''brien'"));
    }

    #[test]
    fn unknown_field_rejected_when_allowlist_given() {
        let out = emit_tsquery(
            "title:apple",
            TsqueryOptions {
                allowed_fields: Some(vec!["content".into(), "content_exact".into()]),
                ..Default::default()
            },
        );
        assert!(!out.ok);
        assert!(
            out.diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("unknown-field"))
        );
    }

    #[test]
    fn wildcard_still_rejected_by_lint() {
        let out = emit("appl*");
        assert!(!out.ok);
        assert!(out.expression.is_none());
    }

    #[test]
    fn range_rejected() {
        let out = emit("content_exact:[a TO b]");
        assert!(!out.ok);
        assert!(
            out.diagnostics
                .items
                .iter()
                .any(|d| d.code.as_deref() == Some("no-range"))
        );
    }

    #[test]
    fn set_query_becomes_or() {
        let out = emit("content_exact: IN [apple banana]");
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "'apple' | 'banana'"));
    }

    #[test]
    fn disjunction_mode_ors_bare_terms() {
        let out = emit_tsquery(
            "apple banana",
            TsqueryOptions {
                conjunction_mode: Some(false),
                ..Default::default()
            },
        );
        let expr = expression(&out);
        assert_eq!(match_parts(&expr), ("content_exact", "'apple' | 'banana'"));
    }
}
