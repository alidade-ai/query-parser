use napi_derive::napi;

use crate::ast::{Node, QueryStats};
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};
use crate::lint::{LintOptions, lint};
use crate::parser::{ImplicitOp, parse};

/// Every UPPER CASE word TINQL reserves; emitted quoted when searched literally.
pub const TIN_KEYWORDS: [&str; 24] = [
    "AND",
    "OR",
    "NOT",
    "THEN",
    "NEAR",
    "WITHIN",
    "ENCLOSES",
    "ENCLOSED",
    "BY",
    "OVERLAPPING",
    "BEFORE",
    "AFTER",
    "TO",
    "IN",
    "FIRST",
    "LAST",
    "MIDDLE",
    "WORDS",
    "CONTAINS",
    "MATCHES",
    "AT",
    "LEAST",
    "OF",
    "ALL",
];

/// Options for analysis and TINQL emission
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct TinqlOptions {
    /// Treat bare adjacent terms as AND (true, default) or OR (false).
    pub conjunction_mode: Option<bool>,
    /// Maximum phrase proximity `"a b"~N`. Defaults to 20.
    pub max_slop: Option<u32>,
    /// Maximum fuzzy edit distance `term~N`. Defaults to 2.
    pub max_fuzzy_distance: Option<u32>,
}

/// Result of transpiling a query to TINQL
#[napi(object)]
pub struct TinqlOutput {
    /// Whether transpilation succeeded (no error diagnostics)
    pub ok: bool,
    /// The TINQL string for `content ==> $1`, present only when `ok`
    pub tinql: Option<String>,
    /// Diagnostics from parsing, linting, and emission
    pub diagnostics: DiagnosticList,
}

pub struct Analysis {
    pub ast: Option<Node>,
    pub stats: Option<QueryStats>,
    pub diagnostics: DiagnosticList,
    pub tinql: Option<String>,
}

impl Analysis {
    pub fn is_ok(&self) -> bool {
        !self.diagnostics.has_errors()
    }
}

/// The single pipeline behind every public function: lex, parse, lint, emit.
pub fn analyze(source: &str, options: &TinqlOptions) -> Analysis {
    let implicit = if options.conjunction_mode.unwrap_or(true) {
        ImplicitOp::And
    } else {
        ImplicitOp::Or
    };
    let parsed = parse(source, implicit);
    let mut diagnostics = parsed.diagnostics;

    let Some(ast) = parsed.ast else {
        diagnostics.push(
            Diagnostic::error("Query has no search terms", Range::whole(source))
                .with_code("empty-query"),
        );
        return Analysis {
            ast: None,
            stats: None,
            diagnostics,
            tinql: None,
        };
    };

    let lint_options = LintOptions {
        max_slop: options.max_slop.unwrap_or(20),
        max_fuzzy_distance: options.max_fuzzy_distance.unwrap_or(2),
    };
    diagnostics.extend(lint(source, &ast, &lint_options).items);
    diagnostics.sort();

    let stats = QueryStats::from_node(&ast);
    let tinql = if diagnostics.has_errors() {
        None
    } else {
        Some(emit(&ast).text)
    };

    Analysis {
        ast: Some(ast),
        stats: Some(stats),
        diagnostics,
        tinql,
    }
}

pub fn to_tinql(source: &str, options: &TinqlOptions) -> TinqlOutput {
    let analysis = analyze(source, options);
    TinqlOutput {
        ok: analysis.is_ok() && analysis.tinql.is_some(),
        tinql: analysis.tinql,
        diagnostics: analysis.diagnostics,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    Or,
    And,
    AndNot,
    Proximity,
    Primary,
}

struct Piece {
    text: String,
    prec: Prec,
}

impl Piece {
    fn primary(text: String) -> Self {
        Piece {
            text,
            prec: Prec::Primary,
        }
    }

    /// Anything that is not a single term or phrase gets parentheses, so the
    /// emitted string never relies on TINQL precedence.
    fn wrapped(&self) -> String {
        if self.prec == Prec::Primary {
            self.text.clone()
        } else {
            format!("({})", self.text)
        }
    }
}

fn emit(node: &Node) -> Piece {
    match node {
        Node::Term {
            value,
            wildcard,
            fuzzy,
            ..
        } => {
            if *wildcard {
                return Piece::primary(value.clone());
            }
            if let Some(fuzzy) = fuzzy {
                let suffix = match fuzzy.prefix {
                    Some(prefix) => format!("~{prefix}:{}", fuzzy.distance),
                    None => format!("~{}", fuzzy.distance),
                };
                return Piece::primary(format!("{value}{suffix}"));
            }
            if needs_quoting(value) || TIN_KEYWORDS.contains(&value.as_str()) {
                let literal = value.replace("\\*", "*").replace("\\?", "?");
                Piece::primary(quote_phrase(&literal, 0))
            } else {
                Piece::primary(value.clone())
            }
        }
        Node::Phrase { text, slop, .. } => Piece::primary(quote_phrase(text, *slop)),
        Node::All { .. } => Piece::primary("*".to_string()),
        Node::Group { child, .. } | Node::Boost { child, .. } => emit(child),
        Node::Not { child, .. } => Piece {
            text: format!("* AND NOT {}", emit(child).wrapped()),
            prec: Prec::AndNot,
        },
        Node::Proximity {
            left,
            right,
            gap,
            ordered,
            ..
        } => {
            let op = if *ordered { "THEN" } else { "NEAR" };
            Piece {
                text: format!(
                    "{} {op}/{gap} {}",
                    emit(left).wrapped(),
                    emit(right).wrapped()
                ),
                prec: Prec::Proximity,
            }
        }
        Node::And { children, .. } => {
            let mut positives: Vec<Piece> = Vec::new();
            let mut negatives: Vec<Piece> = Vec::new();
            for child in children {
                match child.unwrapped() {
                    Node::Not { child: inner, .. } => negatives.push(emit(inner)),
                    _ => positives.push(emit(child)),
                }
            }
            let positive = match positives.len() {
                0 => Piece::primary("*".to_string()),
                1 => positives.pop().unwrap(),
                _ => Piece {
                    text: positives
                        .iter()
                        .map(Piece::wrapped)
                        .collect::<Vec<_>>()
                        .join(" AND "),
                    prec: Prec::And,
                },
            };
            if negatives.is_empty() {
                return positive;
            }
            let negative = match negatives.len() {
                1 => negatives[0].wrapped(),
                _ => format!(
                    "({})",
                    negatives
                        .iter()
                        .map(Piece::wrapped)
                        .collect::<Vec<_>>()
                        .join(" OR ")
                ),
            };
            Piece {
                text: format!("{} AND NOT {negative}", positive.wrapped()),
                prec: Prec::AndNot,
            }
        }
        Node::Or { children, .. } => Piece {
            text: children
                .iter()
                .map(|c| emit(c).wrapped())
                .collect::<Vec<_>>()
                .join(" OR "),
            prec: Prec::Or,
        },
    }
}

/// A bare TINQL term may hold anything except whitespace and `( ) [ ] " ~ ^`;
/// a backslash is only meaningful before `*` / `?`.
pub fn needs_quoting(value: &str) -> bool {
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if !matches!(chars.peek(), Some('*') | Some('?')) {
                    return true;
                }
                chars.next();
            }
            c if c.is_whitespace() => return true,
            '(' | ')' | '[' | ']' | '"' | '~' | '^' => return true,
            _ => {}
        }
    }
    value.starts_with(':')
}

fn quote_phrase(text: &str, slop: u32) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    out.push('"');
    for c in text.chars() {
        if matches!(c, '\\' | '"' | '_' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    if slop > 0 {
        out.push('~');
        out.push_str(&slop.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tinql(query: &str) -> String {
        let out = to_tinql(query, &TinqlOptions::default());
        assert!(out.ok, "{query:?}: {:?}", out.diagnostics.items);
        out.tinql.unwrap()
    }

    fn codes(query: &str) -> Vec<String> {
        to_tinql(query, &TinqlOptions::default())
            .diagnostics
            .codes()
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn errors(query: &str) -> Vec<String> {
        to_tinql(query, &TinqlOptions::default())
            .diagnostics
            .errors()
            .filter_map(|d| d.code.clone())
            .collect()
    }

    #[test]
    fn boolean_shapes() {
        assert_eq!(tinql("apple AND banana"), "apple AND banana");
        assert_eq!(tinql("apple OR banana"), "apple OR banana");
        assert_eq!(
            tinql("(apple OR banana) AND cherry"),
            "(apple OR banana) AND cherry"
        );
        assert_eq!(
            tinql("(apple AND banana) OR cherry"),
            "(apple AND banana) OR cherry"
        );
        assert_eq!(tinql("a AND b AND c"), "a AND b AND c");
    }

    #[test]
    fn negation_shapes() {
        assert_eq!(tinql("apple AND NOT banana"), "apple AND NOT banana");
        assert_eq!(tinql("apple NOT banana"), "apple AND NOT banana");
        assert_eq!(tinql("apple -banana"), "apple AND NOT banana");
        assert_eq!(
            tinql("apple NOT banana NOT cherry"),
            "apple AND NOT (banana OR cherry)"
        );
        assert_eq!(
            tinql("apple AND banana NOT cherry"),
            "(apple AND banana) AND NOT cherry"
        );
        assert_eq!(
            tinql("apple NOT (banana OR cherry)"),
            "apple AND NOT (banana OR cherry)"
        );
        assert_eq!(tinql("NOT apple"), "* AND NOT apple");
        assert_eq!(
            tinql("NOT (apple OR banana)"),
            "* AND NOT (apple OR banana)"
        );
        assert_eq!(tinql("apple OR NOT banana"), "apple OR (* AND NOT banana)");
        assert_eq!(
            tinql("(apple NOT (banana OR cherry)) OR mango"),
            "(apple AND NOT (banana OR cherry)) OR mango"
        );
        assert_eq!(tinql("NOT NOT apple"), "* AND NOT (* AND NOT apple)");
    }

    #[test]
    fn negation_only_query_has_no_warning() {
        assert!(codes("NOT apple").is_empty());
        assert!(codes("(apple NOT (banana OR cherry)) OR mango").is_empty());
    }

    #[test]
    fn implicit_terms_are_anded_with_warning() {
        assert_eq!(tinql("apple banana"), "apple AND banana");
        assert_eq!(codes("apple banana"), vec!["implicit-operator"]);
        let out = to_tinql(
            "apple banana",
            &TinqlOptions {
                conjunction_mode: Some(false),
                ..Default::default()
            },
        );
        assert_eq!(out.tinql.as_deref(), Some("apple OR banana"));
    }

    #[test]
    fn mixed_and_or_without_parentheses_errors() {
        assert_eq!(errors("apple AND banana OR cherry"), vec!["mixed-and-or"]);
        assert_eq!(errors("apple banana OR cherry"), vec!["mixed-and-or"]);
        assert_eq!(errors("apple OR banana AND cherry"), vec!["mixed-and-or"]);
        assert_eq!(errors("apple OR banana NOT cherry"), vec!["mixed-and-or"]);
        assert!(errors("(apple OR banana) NOT cherry").is_empty());
    }

    #[test]
    fn missing_operator_inside_or_list_names_the_gap() {
        let out = to_tinql(
            "alpha OR \"chat bot\" \"frontier model\" OR beta",
            &TinqlOptions::default(),
        );
        let error = out.diagnostics.errors().next().unwrap();
        assert_eq!(error.code.as_deref(), Some("mixed-and-or"));
        assert!(
            error
                .message
                .starts_with("Missing operator between \"chat bot\" and \"frontier model\""),
            "{}",
            error.message
        );
        assert_eq!(error.range.start.offset, 9);
        assert_eq!(error.range.end.offset, 36);
    }

    #[test]
    fn mixed_and_or_error_points_at_the_and_group() {
        let out = to_tinql("cherry OR apple AND banana", &TinqlOptions::default());
        let error = out.diagnostics.errors().next().unwrap();
        assert_eq!(error.range.start.offset, 10);
        assert_eq!(error.range.end.offset, 26);
    }

    #[test]
    fn phrases() {
        assert_eq!(tinql("\"health care\""), "\"health care\"");
        assert_eq!(tinql("'health care'"), "\"health care\"");
        assert_eq!(tinql("\"health care\"~3"), "\"health care\"~3");
        assert_eq!(tinql("\"one two three\"~7"), "\"one two three\"~7");
        assert_eq!(tinql("\"o'brien\""), "\"o'brien\"");
        assert_eq!(tinql("\"say \\\"hi\\\"\""), "\"say \\\"hi\\\"\"");
        assert_eq!(tinql("\"snake_case [x]\""), "\"snake\\_case \\[x\\]\"");
        assert_eq!(
            tinql("\"apple AND banana\" OR \"cherry OR plum\""),
            "\"apple AND banana\" OR \"cherry OR plum\""
        );
    }

    #[test]
    fn slop_limits() {
        assert_eq!(errors("\"health care\"~21"), vec!["slop-too-large"]);
        let out = to_tinql(
            "\"health care\"~21",
            &TinqlOptions {
                max_slop: Some(30),
                ..Default::default()
            },
        );
        assert!(out.ok);
    }

    #[test]
    fn term_quoting() {
        assert_eq!(
            tinql("#tag AND c++ AND covid-19 AND nytimes.com AND @potus"),
            "#tag AND c++ AND covid-19 AND nytimes.com AND @potus"
        );
        assert_eq!(tinql("rock and roll"), "rock AND and AND roll");
        assert_eq!(
            tinql("TO AND IN AND WITHIN"),
            "\"TO\" AND \"IN\" AND \"WITHIN\""
        );
        assert_eq!(tinql("\"AND\" OR banana"), "\"AND\" OR banana");
        assert_eq!(tinql("foo\\(bar\\)"), "\"foo(bar)\"");
        assert_eq!(tinql("file\\*name"), "file\\*name");
        assert_eq!(
            tinql("content_exact:apple AND content:banana"),
            "apple AND banana"
        );
        assert_eq!(codes("content:apple"), vec!["field-ignored"]);
        assert_eq!(
            tinql("content:\"x y\" AND content:(a OR b)"),
            "\"x y\" AND (a OR b)"
        );
        assert_eq!(apply("content:\"x y\"", "field-ignored"), "\"x y\"");
    }

    #[test]
    fn wildcards() {
        assert_eq!(tinql("appl*"), "appl*");
        assert_eq!(tinql("p?ach AND *house"), "p?ach AND *house");
        assert_eq!(codes("*house"), vec!["leading-wildcard"]);
        assert_eq!(codes("a*"), vec!["short-wildcard"]);
        assert!(codes("ab*").is_empty());
        assert_eq!(errors("**"), vec!["empty-wildcard"]);
        assert_eq!(errors("@*"), vec!["unsearchable-term"]);
        assert_eq!(errors("*"), vec!["match-all"]);
        assert_eq!(errors("apple AND *"), vec!["match-all"]);
        assert_eq!(codes("\"appl*\""), vec!["wildcard-in-phrase"]);
    }

    #[test]
    fn fuzzy() {
        assert_eq!(tinql("apple~1"), "apple~1");
        assert_eq!(tinql("apple~0:2"), "apple~0:2");
        assert_eq!(errors("apple~3"), vec!["fuzzy-too-large"]);
        assert_eq!(errors("appl*~1"), vec!["invalid-fuzzy"]);
    }

    #[test]
    fn proximity() {
        assert_eq!(tinql("apple NEAR/3 pie"), "apple NEAR/3 pie");
        assert_eq!(tinql("apple THEN/0 pie"), "apple THEN/0 pie");
        assert_eq!(
            tinql("(apple OR pear) NEAR/5 \"hot pie\""),
            "(apple OR pear) NEAR/5 \"hot pie\""
        );
        assert_eq!(
            tinql("apple NEAR/3 pie AND crust"),
            "(apple NEAR/3 pie) AND crust"
        );
        assert_eq!(tinql("a NEAR/3 b THEN/1 c"), "(a NEAR/3 b) THEN/1 c");
        assert_eq!(tinql("NOT (a NEAR/3 b)"), "* AND NOT (a NEAR/3 b)");
        assert_eq!(errors("a NEAR/3 NOT b"), vec!["negation-in-proximity"]);
        assert_eq!(errors("a NEAR b"), vec!["invalid-proximity"]);
    }

    #[test]
    fn unsearchable_terms() {
        assert_eq!(errors("apple AND -"), vec!["unsearchable-term"]);
        assert_eq!(errors("apple AND ..."), vec!["unsearchable-term"]);
        assert!(errors("apple AND 😀").is_empty());
        assert!(errors("apple AND 日本語").is_empty());
    }

    #[test]
    fn empty_queries() {
        assert_eq!(errors(""), vec!["empty-query"]);
        assert_eq!(errors("   "), vec!["empty-query"]);
        assert_eq!(errors("\"\""), vec!["empty-phrase"]);
    }

    #[test]
    fn operators_across_newlines_and_tabs() {
        for sep in ["\n", "\t", "\r\n", "\n\n", " \n "] {
            assert_eq!(
                tinql(&format!("(test OR test){sep}AND{sep}(testing OR testing)")),
                "(test OR test) AND (testing OR testing)",
                "sep {sep:?}"
            );
            assert!(
                codes(&format!("apple OR{sep}banana")).is_empty(),
                "sep {sep:?}"
            );
        }
    }

    #[test]
    fn diagnostics_keep_line_and_utf16_column() {
        let out = to_tinql("apple AND\nbanana cherry", &TinqlOptions::default());
        let warning = &out.diagnostics.items[0];
        assert_eq!(warning.code.as_deref(), Some("implicit-operator"));
        assert_eq!(warning.range.start.line, 1);
        assert_eq!(warning.range.start.column, 0);
        assert_eq!(warning.range.start.offset, 10);
        assert_eq!(warning.range.end.offset, 23);

        let out = to_tinql("日本 AND 😀x *", &TinqlOptions::default());
        let error = out.diagnostics.errors().next().unwrap();
        assert_eq!(error.code.as_deref(), Some("match-all"));
        assert_eq!(error.range.start.column, 11);
    }

    fn fix_of(query: &str, code: &str) -> (u32, u32, String, String) {
        let out = to_tinql(query, &TinqlOptions::default());
        let diag = out
            .diagnostics
            .items
            .iter()
            .find(|d| d.code.as_deref() == Some(code))
            .unwrap_or_else(|| panic!("{query:?}: no {code} in {:?}", out.diagnostics.items));
        let fix = diag.fix.as_ref().expect("fix");
        (
            fix.range.start.offset,
            fix.range.end.offset,
            fix.replacement.clone(),
            fix.title.clone(),
        )
    }

    fn apply(query: &str, code: &str) -> String {
        let (start, end, replacement, _) = fix_of(query, code);
        format!(
            "{}{}{}",
            &query[..start as usize],
            replacement,
            &query[end as usize..]
        )
    }

    #[test]
    fn fixes_are_applicable_edits() {
        assert_eq!(
            apply("apple banana", "implicit-operator"),
            "apple AND banana"
        );
        assert_eq!(
            apply("apple\n  banana", "implicit-operator"),
            "apple AND banana"
        );
        assert_eq!(
            apply("apple and banana", "lowercase-operator"),
            "apple AND banana"
        );
        assert_eq!(
            apply("apple near/3 banana", "lowercase-operator"),
            "apple NEAR/3 banana"
        );
        assert_eq!(apply("apple^2 AND pie", "boost-ignored"), "apple AND pie");
        assert_eq!(
            apply("\"apple\"~3 AND pie", "slop-no-effect"),
            "\"apple\" AND pie"
        );
        assert_eq!(apply("content:apple", "field-ignored"), "apple");
        assert_eq!(apply("(apple OR pie", "unbalanced-paren"), "(apple OR pie)");
        assert_eq!(fix_of("apple banana", "implicit-operator").3, "Insert AND");
        let out = to_tinql(
            "apple banana",
            &TinqlOptions {
                conjunction_mode: Some(false),
                ..Default::default()
            },
        );
        assert_eq!(
            out.diagnostics.items[0].fix.as_ref().unwrap().replacement,
            " OR "
        );
    }

    #[test]
    fn lowercase_operator_is_a_hint_only() {
        let out = to_tinql("rock and roll", &TinqlOptions::default());
        assert!(out.ok);
        let hint = out
            .diagnostics
            .items
            .iter()
            .find(|d| d.code.as_deref() == Some("lowercase-operator"))
            .unwrap();
        assert_eq!(hint.severity, crate::DiagnosticSeverity::Hint);
        assert_eq!(hint.range.start.offset, 5);
        assert_eq!(hint.range.end.offset, 8);
        assert!(codes("rock AND roll").is_empty());
        assert!(!codes("\"rock and roll\"").contains(&"lowercase-operator".to_string()));
    }

    #[test]
    fn slop_on_single_word_phrase_warns() {
        assert_eq!(codes("\"apple\"~3"), vec!["slop-no-effect"]);
        assert!(codes("\"apple pie\"~3").is_empty());
        assert_eq!(tinql("\"apple\"~3"), "\"apple\"~3");
    }

    #[test]
    fn boost_is_dropped_with_warning() {
        assert_eq!(tinql("apple^2 AND pie"), "apple AND pie");
        assert_eq!(codes("apple^2 AND pie"), vec!["boost-ignored"]);
    }

    #[test]
    fn ok_is_false_whenever_there_is_an_error() {
        for query in [
            "apple AND",
            "(apple",
            "apple AND banana OR cherry",
            "[a b]",
            "appl* AND *",
        ] {
            let out = to_tinql(query, &TinqlOptions::default());
            assert!(!out.ok, "{query:?}");
            assert!(out.tinql.is_none(), "{query:?}");
        }
    }
}
