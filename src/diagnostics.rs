use napi_derive::napi;
use serde::{Deserialize, Serialize};

use crate::ast::Span;

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
    /// Column within the line in UTF-16 code units (0-indexed), as editors count it
    pub column: u32,
    /// Byte offset in the UTF-8 source string
    pub offset: u32,
}

impl Position {
    pub fn from_offset(source: &str, offset: usize) -> Self {
        let mut offset = offset.min(source.len());
        while !source.is_char_boundary(offset) {
            offset -= 1;
        }
        let before = &source[..offset];
        let line = before.matches('\n').count() as u32;
        let line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
        let column = before[line_start..].encode_utf16().count() as u32;
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
            end: Position::from_offset(source, end.max(start)),
        }
    }

    pub fn from_span(source: &str, span: Span) -> Self {
        Self::from_offsets(source, span.start, span.end)
    }

    pub fn whole(source: &str) -> Self {
        Self::from_offsets(source, 0, source.len())
    }
}

/// A text edit an editor can apply to resolve the diagnostic
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fix {
    /// Short action label, e.g. "Insert AND"
    pub title: String,
    /// Text to replace (empty range = insertion)
    pub range: Range,
    /// Replacement text (empty = deletion)
    pub replacement: String,
}

/// Diagnostic message with location and severity info
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub range: Range,
    /// Stable machine-readable rule id (e.g. `mixed-and-or`)
    pub code: Option<String>,
    pub source: Option<String>,
    /// Longer explanation or a suggested fix
    pub related_info: Option<String>,
    /// Machine-applicable repair, when the fix is unambiguous
    pub fix: Option<Fix>,
}

const SOURCE: &str = "alidade-query-parser";

impl Diagnostic {
    fn new(severity: DiagnosticSeverity, message: impl Into<String>, range: Range) -> Self {
        Self {
            message: message.into(),
            severity,
            range,
            code: None,
            source: Some(SOURCE.to_string()),
            related_info: None,
            fix: None,
        }
    }

    pub fn error(message: impl Into<String>, range: Range) -> Self {
        Self::new(DiagnosticSeverity::Error, message, range)
    }

    pub fn warning(message: impl Into<String>, range: Range) -> Self {
        Self::new(DiagnosticSeverity::Warning, message, range)
    }

    pub fn info(message: impl Into<String>, range: Range) -> Self {
        Self::new(DiagnosticSeverity::Info, message, range)
    }

    pub fn hint(message: impl Into<String>, range: Range) -> Self {
        Self::new(DiagnosticSeverity::Hint, message, range)
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_related_info(mut self, info: impl Into<String>) -> Self {
        self.related_info = Some(info.into());
        self
    }

    pub fn with_fix(
        mut self,
        title: impl Into<String>,
        range: Range,
        replacement: impl Into<String>,
    ) -> Self {
        self.fix = Some(Fix {
            title: title.into(),
            range,
            replacement: replacement.into(),
        });
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

    pub fn codes(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect()
    }

    /// Order by position so editors and tests see a stable list.
    pub fn sort(&mut self) {
        self.items.sort_by_key(|d| {
            (
                d.range.start.offset,
                d.range.end.offset,
                std::cmp::Reverse(d.severity as u8),
            )
        });
    }
}
