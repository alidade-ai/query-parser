use crate::ast::{Node, Span};
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};
use crate::lexer::{Lexed, Token, TokenKind, lex};

pub struct Parsed {
    pub ast: Option<Node>,
    pub diagnostics: DiagnosticList,
}

/// Operator inserted between adjacent operands with no explicit keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplicitOp {
    And,
    Or,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    implicit: ImplicitOp,
    diagnostics: DiagnosticList,
}

/// An operand plus whether it carried its own `+`/`-`/`NOT` marker, which
/// decides whether adjacency to it deserves an implicit-operator warning.
struct Operand {
    node: Node,
    marked: bool,
}

pub fn parse(source: &str, implicit: ImplicitOp) -> Parsed {
    let Lexed {
        tokens,
        diagnostics,
    } = lex(source);
    let mut parser = Parser {
        source,
        tokens,
        pos: 0,
        implicit,
        diagnostics,
    };
    let ast = parser.parse_query();
    parser.diagnostics.sort();
    Parsed {
        ast,
        diagnostics: parser.diagnostics,
    }
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos).map(|t| &t.kind)
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        self.pos += 1;
        token
    }

    fn error(&mut self, code: &str, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(message, Range::from_span(self.source, span)).with_code(code));
    }

    fn starts_operand(kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Word { .. }
                | TokenKind::Phrase { .. }
                | TokenKind::All
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::Plus
                | TokenKind::Minus
                | TokenKind::Not
        )
    }

    fn parse_query(&mut self) -> Option<Node> {
        let mut parts: Vec<Node> = Vec::new();
        while self.pos < self.tokens.len() {
            let before = self.pos;
            if let Some(node) = self.parse_or() {
                parts.push(node);
            }
            match self.peek() {
                None => break,
                Some(TokenKind::RParen) => {
                    let span = self.bump().span;
                    self.error("unbalanced-paren", "Unmatched closing parenthesis", span);
                }
                Some(TokenKind::RBracket) => {
                    let span = self.bump().span;
                    self.error("unsupported-syntax", "Unmatched closing bracket", span);
                }
                Some(_) if self.pos == before => {
                    let token = self.bump();
                    self.error(
                        "unexpected-token",
                        format!("Unexpected {}", token.kind.describe()),
                        token.span,
                    );
                }
                Some(_) => {}
            }
        }
        combine(parts, self.implicit)
    }

    fn parse_or(&mut self) -> Option<Node> {
        let mut children: Vec<Node> = Vec::new();
        if let Some(first) = self.parse_and() {
            children.push(first.node);
        }
        loop {
            match self.peek() {
                Some(TokenKind::Or) => {
                    let span = self.bump().span;
                    let before = self.diagnostics.items.len();
                    match self.parse_and() {
                        Some(operand) => children.push(operand.node),
                        None if self.diagnostics.items.len() == before => {
                            self.error("bare-operator", "OR has no search term after it", span)
                        }
                        None => {}
                    }
                }
                Some(kind) if self.implicit == ImplicitOp::Or && Self::starts_operand(kind) => {
                    let Some(operand) = self.parse_and() else {
                        break;
                    };
                    self.warn_implicit(children.last(), &operand);
                    children.push(operand.node);
                }
                _ => break,
            }
        }
        combine(children, ImplicitOp::Or)
    }

    fn parse_and(&mut self) -> Option<Operand> {
        let first = self.parse_not()?;
        let mut marked = first.marked;
        let mut children = vec![first.node];
        loop {
            match self.peek() {
                Some(TokenKind::And) => {
                    let span = self.bump().span;
                    let before = self.diagnostics.items.len();
                    match self.parse_not() {
                        Some(operand) => children.push(operand.node),
                        None if self.diagnostics.items.len() == before => {
                            self.error("bare-operator", "AND has no search term after it", span)
                        }
                        None => {}
                    }
                }
                Some(kind) if self.implicit == ImplicitOp::And && Self::starts_operand(kind) => {
                    let Some(operand) = self.parse_not() else {
                        break;
                    };
                    self.warn_implicit(children.last(), &operand);
                    children.push(operand.node);
                }
                _ => break,
            }
        }
        if children.len() > 1 {
            marked = false;
        }
        combine(children, ImplicitOp::And).map(|node| Operand { node, marked })
    }

    /// `apple banana` warns; `apple NOT banana`, `apple -banana` and
    /// `+apple +banana` carry explicit markers and do not.
    fn warn_implicit(&mut self, previous: Option<&Node>, next: &Operand) {
        let Some(previous) = previous else { return };
        if next.node.is_negation() || next.marked {
            return;
        }
        let span = previous.span().to(next.node.span());
        let (message, operator) = match self.implicit {
            ImplicitOp::And => (
                "Space-separated terms are combined with AND; use an explicit AND to make this clear",
                "AND",
            ),
            ImplicitOp::Or => (
                "Space-separated terms are combined with OR; use an explicit OR to make this clear",
                "OR",
            ),
        };
        let gap = Range::from_offsets(self.source, previous.span().end, next.node.span().start);
        self.diagnostics.push(
            Diagnostic::warning(message, Range::from_span(self.source, span))
                .with_code("implicit-operator")
                .with_fix(format!("Insert {operator}"), gap, format!(" {operator} ")),
        );
    }

    fn parse_not(&mut self) -> Option<Operand> {
        match self.peek() {
            Some(TokenKind::Not | TokenKind::Minus) => {
                let token = self.bump();
                let before = self.diagnostics.items.len();
                match self.parse_not() {
                    Some(operand) => {
                        let span = token.span.to(operand.node.span());
                        Some(Operand {
                            node: Node::Not {
                                child: Box::new(operand.node),
                                span,
                            },
                            marked: true,
                        })
                    }
                    None => {
                        if self.diagnostics.items.len() == before {
                            self.error(
                                "bare-operator",
                                format!("{} has no search term after it", token.kind.describe()),
                                token.span,
                            );
                        }
                        None
                    }
                }
            }
            Some(TokenKind::Plus) => {
                let token = self.bump();
                let before = self.diagnostics.items.len();
                match self.parse_not() {
                    Some(operand) => Some(Operand {
                        node: operand.node,
                        marked: true,
                    }),
                    None => {
                        if self.diagnostics.items.len() == before {
                            self.error(
                                "bare-operator",
                                "+ has no search term after it",
                                token.span,
                            );
                        }
                        None
                    }
                }
            }
            _ => self.parse_proximity().map(|node| Operand {
                node,
                marked: false,
            }),
        }
    }

    fn parse_proximity(&mut self) -> Option<Node> {
        let mut left = self.parse_postfix()?;
        loop {
            let (gap, ordered) = match self.peek() {
                Some(TokenKind::Near(gap)) => (gap.unwrap_or(0), false),
                Some(TokenKind::Then(gap)) => (gap.unwrap_or(0), true),
                _ => break,
            };
            let token = self.bump();
            let before = self.diagnostics.items.len();
            match self.parse_postfix() {
                Some(right) => {
                    let span = left.span().to(right.span());
                    left = Node::Proximity {
                        left: Box::new(left),
                        right: Box::new(right),
                        gap,
                        ordered,
                        span,
                    };
                }
                None if self.diagnostics.items.len() == before => self.error(
                    "bare-operator",
                    format!("{} has no search term after it", token.kind.describe()),
                    token.span,
                ),
                None => {}
            }
        }
        Some(left)
    }

    fn parse_postfix(&mut self) -> Option<Node> {
        let mut node = self.parse_primary()?;
        while let Some(TokenKind::Boost(factor)) = self.peek() {
            let factor = *factor;
            let token = self.bump();
            self.diagnostics.push(
                Diagnostic::warning(
                    "Boost (^) has no effect on matching and is ignored",
                    Range::from_span(self.source, token.span),
                )
                .with_code("boost-ignored")
                .with_fix(
                    "Remove boost",
                    Range::from_span(self.source, token.span),
                    "",
                ),
            );
            let span = node.span().to(token.span);
            node = Node::Boost {
                factor,
                child: Box::new(node),
                span,
            };
        }
        Some(node)
    }

    fn parse_primary(&mut self) -> Option<Node> {
        let kind = self.peek()?.clone();
        match kind {
            TokenKind::Word {
                text,
                wildcard,
                fuzzy,
            } => {
                let span = self.bump().span;
                Some(Node::Term {
                    value: text,
                    wildcard,
                    fuzzy,
                    span,
                })
            }
            TokenKind::Phrase { text, slop } => {
                let span = self.bump().span;
                Some(Node::Phrase {
                    text,
                    slop: slop.unwrap_or(0),
                    span,
                })
            }
            TokenKind::All => {
                let span = self.bump().span;
                Some(Node::All { span })
            }
            TokenKind::LParen => {
                let open = self.bump().span;
                let inner = self.parse_or();
                let close = match self.peek() {
                    Some(TokenKind::RParen) => Some(self.bump().span),
                    _ => {
                        let end = self.source.len();
                        self.diagnostics.push(
                            Diagnostic::error(
                                "Missing closing parenthesis",
                                Range::from_span(self.source, open),
                            )
                            .with_code("unbalanced-paren")
                            .with_fix(
                                "Add closing parenthesis",
                                Range::from_offsets(self.source, end, end),
                                ")",
                            ),
                        );
                        None
                    }
                };
                let end = close.unwrap_or_else(|| inner.as_ref().map_or(open, |n| n.span()));
                let span = open.to(end);
                match inner {
                    Some(child) => Some(Node::Group {
                        child: Box::new(child),
                        span,
                    }),
                    None => {
                        self.error("empty-group", "Empty parentheses", span);
                        None
                    }
                }
            }
            TokenKind::LBracket => {
                let open = self.bump().span;
                let mut depth = 1usize;
                let mut end = open;
                while let Some(kind) = self.peek() {
                    match kind {
                        TokenKind::LBracket => depth += 1,
                        TokenKind::RBracket => depth -= 1,
                        _ => {}
                    }
                    end = self.bump().span;
                    if depth == 0 {
                        break;
                    }
                }
                self.error(
                    "unsupported-syntax",
                    "Square brackets are not supported; use parentheses with OR, for example (apple OR pear)",
                    open.to(end),
                );
                self.parse_primary()
            }
            TokenKind::And | TokenKind::Or | TokenKind::Near(_) | TokenKind::Then(_) => {
                let token = self.bump();
                self.error(
                    "bare-operator",
                    format!("{} has no search term before it", token.kind.describe()),
                    token.span,
                );
                self.parse_primary()
            }
            TokenKind::Not | TokenKind::Plus | TokenKind::Minus => {
                self.parse_not().map(|operand| operand.node)
            }
            TokenKind::Boost(_) => {
                let token = self.bump();
                self.error(
                    "dangling-modifier",
                    "^ must directly follow a term, phrase or closing parenthesis",
                    token.span,
                );
                self.parse_primary()
            }
            TokenKind::RParen | TokenKind::RBracket => None,
        }
    }
}

fn combine(mut children: Vec<Node>, op: ImplicitOp) -> Option<Node> {
    match children.len() {
        0 => None,
        1 => children.pop(),
        _ => {
            let span = children[0].span().to(children[children.len() - 1].span());
            Some(match op {
                ImplicitOp::And => Node::And { children, span },
                ImplicitOp::Or => Node::Or { children, span },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::format_node;

    fn p(source: &str) -> Parsed {
        parse(source, ImplicitOp::And)
    }

    fn shape(source: &str) -> String {
        let parsed = p(source);
        assert!(
            !parsed.diagnostics.has_errors(),
            "{source:?}: {:?}",
            parsed.diagnostics.items
        );
        format_node(&parsed.ast.expect("ast"))
    }

    fn codes(source: &str) -> Vec<String> {
        p(source)
            .diagnostics
            .codes()
            .into_iter()
            .map(String::from)
            .collect()
    }

    #[test]
    fn precedence_and_binds_tighter_than_or() {
        assert_eq!(shape("a AND b OR c"), "a AND b OR c");
        let ast = p("a AND b OR c").ast.unwrap();
        assert!(
            matches!(ast, Node::Or { ref children, .. } if matches!(children[0], Node::And { .. }))
        );
    }

    #[test]
    fn same_operator_chains_flatten() {
        let ast = p("a AND b AND c").ast.unwrap();
        assert!(matches!(ast, Node::And { ref children, .. } if children.len() == 3));
        let ast = p("a OR b OR c").ast.unwrap();
        assert!(matches!(ast, Node::Or { ref children, .. } if children.len() == 3));
    }

    #[test]
    fn groups_keep_parentheses() {
        assert_eq!(shape("(a OR b) AND c"), "(a OR b) AND c");
        assert_eq!(shape("NOT (a OR b)"), "NOT (a OR b)");
    }

    #[test]
    fn not_forms() {
        assert_eq!(shape("a NOT b"), "a AND NOT b");
        assert_eq!(shape("a AND NOT b"), "a AND NOT b");
        assert_eq!(shape("a -b"), "a AND NOT b");
        assert_eq!(shape("+a +b"), "a AND b");
        assert_eq!(shape("NOT a"), "NOT a");
        assert_eq!(shape("NOT a OR b"), "NOT a OR b");
    }

    #[test]
    fn implicit_adjacency_warns_once_per_gap() {
        let parsed = p("alpha beta gamma");
        let warnings: Vec<_> = parsed
            .diagnostics
            .items
            .iter()
            .filter(|d| d.code.as_deref() == Some("implicit-operator"))
            .collect();
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0].range.start.offset, 0);
        assert_eq!(warnings[0].range.end.offset, 10);
        assert_eq!(warnings[1].range.start.offset, 6);
        assert_eq!(warnings[1].range.end.offset, 16);
    }

    #[test]
    fn marked_operands_do_not_warn() {
        for query in [
            "apple NOT banana",
            "apple -banana",
            "+apple +banana",
            "apple AND banana",
        ] {
            assert!(codes(query).is_empty(), "{query:?}: {:?}", codes(query));
        }
        assert_eq!(codes("+apple banana"), vec!["implicit-operator"]);
    }

    #[test]
    fn implicit_or_mode() {
        let parsed = parse("apple banana", ImplicitOp::Or);
        assert!(matches!(parsed.ast, Some(Node::Or { .. })));
        let parsed = parse("apple banana AND cherry", ImplicitOp::Or);
        assert_eq!(
            format_node(&parsed.ast.unwrap()),
            "apple OR banana AND cherry"
        );
    }

    #[test]
    fn bare_operators_are_localized_errors() {
        for (query, start, end) in [
            ("apple AND", 6, 9),
            ("apple AND OR banana", 10, 12),
            ("(apple OR) AND banana", 7, 9),
            ("OR banana", 0, 2),
            ("apple NOT", 6, 9),
            ("apple NEAR/2", 6, 12),
        ] {
            let parsed = p(query);
            let errors: Vec<_> = parsed
                .diagnostics
                .items
                .iter()
                .filter(|d| d.code.as_deref() == Some("bare-operator"))
                .collect();
            assert_eq!(errors.len(), 1, "{query:?}: {:?}", parsed.diagnostics.items);
            assert_eq!(errors[0].range.start.offset, start, "{query:?}");
            assert_eq!(errors[0].range.end.offset, end, "{query:?}");
        }
    }

    #[test]
    fn recovery_keeps_the_rest_of_the_query() {
        let parsed = p("apple AND OR banana");
        assert_eq!(format_node(&parsed.ast.unwrap()), "apple AND banana");
    }

    #[test]
    fn unbalanced_parentheses() {
        assert_eq!(codes("(apple OR banana"), vec!["unbalanced-paren"]);
        assert_eq!(codes("apple OR banana)"), vec!["unbalanced-paren"]);
        assert_eq!(codes("()"), vec!["empty-group"]);
        let parsed = p("(apple OR banana");
        assert_eq!(parsed.diagnostics.items[0].range.start.offset, 0);
        assert_eq!(parsed.diagnostics.items[0].range.end.offset, 1);
    }

    #[test]
    fn brackets_are_unsupported() {
        assert_eq!(codes("apple AND [a b c]"), vec!["unsupported-syntax"]);
        let parsed = p("apple AND [a b c]");
        assert_eq!(parsed.diagnostics.items[0].range.start.offset, 10);
        assert_eq!(parsed.diagnostics.items[0].range.end.offset, 17);
    }

    #[test]
    fn proximity_chains_left_to_right() {
        assert_eq!(shape("a NEAR/3 b THEN/0 c"), "(a NEAR/3 b) THEN/0 c");
        assert_eq!(shape("a NEAR/3 b AND c"), "a NEAR/3 b AND c");
        assert_eq!(shape("NOT a NEAR/3 b"), "NOT (a NEAR/3 b)");
        assert_eq!(shape("(a OR b) NEAR/5 \"c d\""), "(a OR b) NEAR/5 \"c d\"");
    }

    #[test]
    fn boost_is_parsed_and_warned() {
        assert_eq!(codes("apple^2 AND pie"), vec!["boost-ignored"]);
        assert_eq!(shape("apple^2 AND pie"), "apple^2 AND pie");
    }

    #[test]
    fn empty_and_whitespace_queries_have_no_ast() {
        assert!(p("").ast.is_none());
        assert!(p("   \n ").ast.is_none());
        assert!(p("").diagnostics.is_empty());
    }

    #[test]
    fn quoted_operator_words_are_phrases() {
        assert_eq!(
            shape("\"AND\" OR \"apple AND banana\""),
            "\"AND\" OR \"apple AND banana\""
        );
    }

    #[test]
    fn lowercase_operators_are_terms() {
        let parsed = p("rock and roll");
        assert_eq!(format_node(&parsed.ast.unwrap()), "rock AND and AND roll");
    }
}
