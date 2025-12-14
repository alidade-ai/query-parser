use crate::ast::{QueryNode, QueryStats};
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};

/// Result of parsing a query
pub struct ParseResult {
    /// The parsed AST (if parsing succeeded enough to produce one)
    pub ast: Option<QueryNode>,
    /// Diagnostics collected during parsing
    pub diagnostics: DiagnosticList,
    /// Query statistics
    pub stats: Option<QueryStats>,
}

impl ParseResult {
    /// Returns true if parsing was successful (no errors)
    pub fn is_ok(&self) -> bool {
        !self.diagnostics.has_errors()
    }
}

/// Parse a query string and return the result with diagnostics.
/// Uses lenient parsing to provide as much information as possible
/// even when the query has errors.
pub fn parse_query(source: &str) -> ParseResult {
    let (ast, errors) = tantivy_query_grammar::parse_query_lenient(source);

    let mut diagnostics = DiagnosticList::new();

    // Convert tantivy errors to our diagnostic format
    for error in errors {
        let range = Range::at_offset(source, error.pos);
        let diagnostic = Diagnostic::error(&error.message, range)
            .with_code("parse-error")
            .with_related_info(get_syntax_help(&error.message));
        diagnostics.push(diagnostic);
    }

    let query_node = QueryNode::from(&ast);
    let stats = QueryStats::from_node(&query_node);

    ParseResult {
        ast: Some(query_node),
        diagnostics,
        stats: Some(stats),
    }
}

/// Validate a query string and return only diagnostics (no AST).
/// More efficient when you only need to check validity.
pub fn validate_query(source: &str) -> DiagnosticList {
    parse_query(source).diagnostics
}

/// Get contextual help for an error message
fn get_syntax_help(error_msg: &str) -> String {
    let msg = error_msg.to_lowercase();

    if msg.contains("expected word") || msg.contains("expected term") {
        return "A search term was expected. Examples: apple, title:hello, \"phrase query\"".into();
    }

    if msg.contains("range") {
        return "Range syntax: field:[start TO end] or field:{start TO end}".into();
    }

    if msg.contains("quote") || msg.contains("unterminated") {
        return "Phrases must be enclosed in matching quotes: \"like this\" or 'like this'".into();
    }

    "Check your query syntax. Common patterns:\n\
     - term AND term\n\
     - term OR term\n\
     - +required -excluded\n\
     - field:value\n\
     - \"exact phrase\"\n\
     - field:[a TO z]"
        .into()
}

/// Linter trait for custom validation rules.
pub trait Linter {
    /// Lint a parsed query and return additional diagnostics
    fn lint(&self, source: &str, ast: &QueryNode) -> DiagnosticList;
}

/// A collection of linters that can be applied to queries
pub struct LinterPipeline {
    linters: Vec<Box<dyn Linter + Send + Sync>>,
}

impl Default for LinterPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl LinterPipeline {
    pub fn new() -> Self {
        Self {
            linters: Vec::new(),
        }
    }

    /// Add a linter to the pipeline
    pub fn add<L: Linter + Send + Sync + 'static>(&mut self, linter: L) {
        self.linters.push(Box::new(linter));
    }

    /// Run all linters and collect diagnostics
    pub fn lint(&self, source: &str, ast: &QueryNode) -> DiagnosticList {
        let mut diagnostics = DiagnosticList::new();
        for linter in &self.linters {
            diagnostics.extend(linter.lint(source, ast).items);
        }
        diagnostics
    }
}

/// Parse and lint a query with a custom linter pipeline
pub fn parse_and_lint(source: &str, linters: &LinterPipeline) -> ParseResult {
    let mut result = parse_query(source);

    // Only run linters if we have an AST
    if let Some(ast) = &result.ast {
        let lint_diagnostics = linters.lint(source, ast);
        result.diagnostics.extend(lint_diagnostics.items);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_query() {
        let result = parse_query("apple OR orange");
        assert!(result.is_ok());
        assert!(result.ast.is_some());
    }

    #[test]
    fn test_parse_invalid_query() {
        let result = parse_query("title:");
        assert!(!result.is_ok());
        assert!(result.diagnostics.has_errors());
    }

    #[test]
    fn test_parse_complex_query() {
        let result = parse_query("(apple OR orange) AND title:fruit -rotten");
        assert!(result.is_ok());
        let stats = result.stats.unwrap();
        assert!(stats.has_negation);
        assert!(stats.fields.contains(&"title".to_string()));
    }
}
