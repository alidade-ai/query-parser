use napi_derive::napi;
use serde::{Deserialize, Serialize};

/// Diagnostic severity levels
#[napi]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Hint = 1,
    Info = 2,
    Warning = 4,
    Error = 8,
}

/// A position in the source text (0-indexed)
#[napi(object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// Line number (0-indexed)
    pub line: u32,
    /// Column number (0-indexed)
    pub column: u32,
    /// Byte offset in the source string
    pub offset: u32,
}

impl Position {
    pub fn from_offset(source: &str, offset: usize) -> Self {
        let offset = offset.min(source.len());
        let before = &source[..offset];
        let line = before.chars().filter(|&c| c == '\n').count() as u32;
        let column = before
            .rfind('\n')
            .map(|pos| offset - pos - 1)
            .unwrap_or(offset) as u32;

        Position {
            line,
            column,
            offset: offset as u32,
        }
    }
}

#[napi(object)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    /// Start position (inclusive)
    pub start: Position,
    /// End position (exclusive)
    pub end: Position,
}

impl Range {
    pub fn from_offsets(source: &str, start: usize, end: usize) -> Self {
        Range {
            start: Position::from_offset(source, start),
            end: Position::from_offset(source, end),
        }
    }

    pub fn at_offset(source: &str, offset: usize) -> Self {
        let pos = Position::from_offset(source, offset);
        Range {
            start: pos,
            end: Position {
                line: pos.line,
                column: pos.column + 1,
                offset: pos.offset + 1,
            },
        }
    }
}

/// Diagnostic message with location and severity info
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub range: Range,
    pub code: Option<String>,
    pub source: Option<String>,
    pub related_info: Option<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, range: Range) -> Self {
        Self {
            message: message.into(),
            severity: DiagnosticSeverity::Error,
            range,
            code: None,
            source: Some("tantivy-query-parser".to_string()),
            related_info: None,
        }
    }

    pub fn warning(message: impl Into<String>, range: Range) -> Self {
        Self {
            message: message.into(),
            severity: DiagnosticSeverity::Warning,
            range,
            code: None,
            source: Some("tantivy-query-parser".to_string()),
            related_info: None,
        }
    }

    pub fn info(message: impl Into<String>, range: Range) -> Self {
        Self {
            message: message.into(),
            severity: DiagnosticSeverity::Info,
            range,
            code: None,
            source: Some("tantivy-query-parser".to_string()),
            related_info: None,
        }
    }

    pub fn hint(message: impl Into<String>, range: Range) -> Self {
        Self {
            message: message.into(),
            severity: DiagnosticSeverity::Hint,
            range,
            code: None,
            source: Some("tantivy-query-parser".to_string()),
            related_info: None,
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_related_info(mut self, info: impl Into<String>) -> Self {
        self.related_info = Some(info.into());
        self
    }
}

#[napi(object)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnosticList {
    pub items: Vec<Diagnostic>,
}

impl DiagnosticList {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.items.push(diagnostic);
    }

    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.items.extend(diagnostics);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        self.items
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Warning)
    }
}
