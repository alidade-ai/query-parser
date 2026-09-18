use crate::ast::{Node, Span};
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};
use crate::tinql::{TIN_KEYWORDS, needs_quoting};

pub struct LintOptions {
    pub max_slop: u32,
    pub max_fuzzy_distance: u32,
}

impl Default for LintOptions {
    fn default() -> Self {
        Self {
            max_slop: 20,
            max_fuzzy_distance: 2,
        }
    }
}

struct Linter<'a> {
    source: &'a str,
    options: &'a LintOptions,
    diagnostics: DiagnosticList,
}

pub fn lint(source: &str, ast: &Node, options: &LintOptions) -> DiagnosticList {
    let mut linter = Linter {
        source,
        options,
        diagnostics: DiagnosticList::new(),
    };
    linter.visit(ast, false);
    linter.diagnostics
}

/// True when the tokenizer would keep at least one token: any letter, digit,
/// or non-ASCII character (emoji and other scripts are searchable terms).
pub fn is_searchable(text: &str) -> bool {
    text.chars().any(|c| c.is_alphanumeric() || !c.is_ascii())
}

impl Linter<'_> {
    fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    fn error(&mut self, code: &str, message: impl Into<String>, node: &Node) {
        self.push(
            Diagnostic::error(message, Range::from_span(self.source, node.span())).with_code(code),
        );
    }

    fn warning(&mut self, code: &str, message: impl Into<String>, node: &Node) {
        self.push(
            Diagnostic::warning(message, Range::from_span(self.source, node.span()))
                .with_code(code),
        );
    }

    fn visit(&mut self, node: &Node, in_proximity: bool) {
        match node {
            Node::Term {
                value,
                wildcard,
                fuzzy,
                ..
            } => self.term(node, value, *wildcard, fuzzy.is_some()),
            Node::Phrase { text, slop, .. } => self.phrase(node, text, *slop),
            Node::All { .. } => self.error(
                "match-all",
                "Match-all (*) is not supported; add a search term",
                node,
            ),
            Node::Or { children, .. } => {
                for child in children {
                    if let Node::And {
                        children: parts, ..
                    } = child
                    {
                        self.mixed_and_or(child, parts);
                    }
                }
            }
            Node::Proximity { left, right, .. } => {
                for operand in [left.as_ref(), right.as_ref()] {
                    if let Node::Not { .. } = operand.unwrapped() {
                        self.error(
                            "negation-in-proximity",
                            "NOT cannot be used inside NEAR/THEN; exclude the term with AND NOT outside the proximity expression",
                            operand,
                        );
                    }
                }
            }
            Node::Group { .. } | Node::Not { .. } | Node::And { .. } | Node::Boost { .. } => {}
        }
        let inner_proximity = in_proximity || matches!(node, Node::Proximity { .. });
        for child in node.children() {
            self.visit(child, inner_proximity);
        }
    }

    /// An AND directly under an OR relies on precedence. When the AND came from
    /// a bare space, the user most likely dropped an operator, so point at the
    /// gap and say so; otherwise ask for parentheses around the whole AND.
    fn mixed_and_or(&mut self, and: &Node, parts: &[Node]) {
        for pair in parts.windows(2) {
            let gap = Span::new(pair[0].span().end, pair[1].span().start);
            if self.source[gap.start..gap.end].trim().is_empty() && !pair[1].is_negation() {
                let left = &self.source[pair[0].span().start..pair[0].span().end];
                let right = &self.source[pair[1].span().start..pair[1].span().end];
                self.push(
                    Diagnostic::error(
                        format!(
                            "Missing operator between {left} and {right}: a space means AND, \
                             which is ambiguous inside an OR list; write OR or AND explicitly"
                        ),
                        Range::from_span(self.source, pair[0].span().to(pair[1].span())),
                    )
                    .with_code("mixed-and-or"),
                );
                return;
            }
        }
        self.error(
            "mixed-and-or",
            "Mixing AND (or NOT) with OR at the same level relies on operator precedence; \
             add parentheses to make the grouping explicit",
            and,
        );
    }

    fn term(&mut self, node: &Node, value: &str, wildcard: bool, fuzzy: bool) {
        if wildcard {
            self.wildcard(node, value, fuzzy);
            return;
        }
        if !is_searchable(value) {
            self.error(
                "unsearchable-term",
                format!("\"{value}\" contains no letters or digits, so it can never match a post"),
                node,
            );
            return;
        }
        if fuzzy {
            if needs_quoting(value) {
                self.error(
                    "invalid-fuzzy",
                    "Fuzzy matching only works on a single plain word",
                    node,
                );
            }
            if let Node::Term {
                fuzzy: Some(fuzzy), ..
            } = node
                && fuzzy.distance > self.options.max_fuzzy_distance
            {
                self.error(
                    "fuzzy-too-large",
                    format!(
                        "Fuzzy distance ~{} exceeds the maximum of {}",
                        fuzzy.distance, self.options.max_fuzzy_distance
                    ),
                    node,
                );
            }
        }
        if TIN_KEYWORDS.contains(&value) {
            self.push(
                Diagnostic::info(
                    format!("{value} is searched as a literal word"),
                    Range::from_span(self.source, node.span()),
                )
                .with_code("literal-keyword"),
            );
        }
    }

    fn wildcard(&mut self, node: &Node, value: &str, fuzzy: bool) {
        if fuzzy {
            self.error(
                "invalid-fuzzy",
                "A term cannot be both a wildcard and a fuzzy match",
                node,
            );
        }
        if needs_quoting(value) {
            self.error(
                "invalid-wildcard",
                "Wildcard terms cannot contain spaces or the characters ( ) [ ] \" ~ ^ \\",
                node,
            );
            return;
        }
        let mut literal_prefix = 0usize;
        let mut literal_total = 0usize;
        let mut seen_wildcard = false;
        let mut chars = value.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                    literal_total += 1;
                    if !seen_wildcard {
                        literal_prefix += 1;
                    }
                }
                '*' | '?' => seen_wildcard = true,
                _ => {
                    literal_total += 1;
                    if !seen_wildcard {
                        literal_prefix += 1;
                    }
                }
            }
        }
        if literal_total == 0 {
            self.error(
                "empty-wildcard",
                "A wildcard needs at least one literal character, for example appl*",
                node,
            );
        } else if !is_searchable(&value.replace(['*', '?', '\\'], "")) {
            self.error(
                "unsearchable-term",
                format!("\"{value}\" contains no letters or digits, so it can never match a post"),
                node,
            );
        } else if literal_prefix == 0 {
            self.warning(
                "leading-wildcard",
                "A leading wildcard has to scan every indexed word and can be slow",
                node,
            );
        } else if literal_prefix < 2 {
            self.warning(
                "short-wildcard",
                "A one-character prefix before a wildcard matches a very large number of words",
                node,
            );
        }
    }

    fn phrase(&mut self, node: &Node, text: &str, slop: u32) {
        if text.trim().is_empty() {
            self.error("empty-phrase", "Empty phrase", node);
            return;
        }
        if !is_searchable(text) {
            self.error(
                "unsearchable-term",
                format!("\"{text}\" contains no letters or digits, so it can never match a post"),
                node,
            );
            return;
        }
        if slop > self.options.max_slop {
            self.error(
                "slop-too-large",
                format!(
                    "Proximity distance ~{slop} exceeds the maximum of {}",
                    self.options.max_slop
                ),
                node,
            );
        }
        if text.contains(['*', '?']) {
            self.warning(
                "wildcard-in-phrase",
                "* and ? inside quotes are matched literally, not as wildcards",
                node,
            );
        }
    }
}
