use crate::ast::QueryNode;
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};
use crate::parser::Linter;

pub struct NoWildcard;

impl Linter for NoWildcard {
    fn lint(&self, source: &str, _ast: &QueryNode) -> DiagnosticList {
        let mut diagnostics = DiagnosticList::new();

        let bytes = source.as_bytes();
        let mut in_quotes = false;
        let mut i = 0;

        while i < bytes.len() {
            let b = bytes[i];

            if b == b'"' {
                in_quotes = !in_quotes;
                i += 1;
                continue;
            }

            if in_quotes {
                i += 1;
                continue;
            }

            if b == b'*' {
                let has_word_before = i > 0
                    && matches!(
                        bytes[i - 1],
                        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'
                    );

                let range = Range::from_offsets(source, i, i + 1);

                if has_word_before {
                    let term_start = source[..i]
                        .rfind(|c: char| c.is_whitespace() || "()+-:".contains(c))
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    let term = &source[term_start..i];
                    diagnostics.push(
                        Diagnostic::error(
                            format!(
                                "Wildcard operator (*) is not supported in prefix query \"{term}*\""
                            ),
                            range,
                        )
                        .with_code("no-wildcard")
                        .with_related_info(
                            "Prefix/wildcard queries (term*) are not supported. \
                             Use the full search term instead.",
                        ),
                    );
                } else {
                    diagnostics.push(
                        Diagnostic::error(
                            "Wildcard operator (*) is not supported".to_string(),
                            range,
                        )
                        .with_code("no-wildcard")
                        .with_related_info(
                            "The match-all wildcard (*) cannot be used. \
                             Use specific search terms instead.",
                        ),
                    );
                }
            }

            i += 1;
        }

        diagnostics
    }
}
