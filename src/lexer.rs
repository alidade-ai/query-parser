use crate::ast::{Fuzzy, Span};
use crate::diagnostics::{Diagnostic, DiagnosticList, Range};

/// Legacy field prefixes accepted and dropped with a warning.
const KNOWN_FIELDS: [&str; 2] = ["content_exact", "content"];

const LINE_COMMENT: &str = "//";
const BLOCK_COMMENT_OPEN: &str = "<<<";
const BLOCK_COMMENT_CLOSE: &str = ">>>";

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Word {
        text: String,
        wildcard: bool,
        fuzzy: Option<Fuzzy>,
    },
    Phrase {
        text: String,
        slop: Option<u32>,
    },
    All,
    And,
    Or,
    Not,
    Near(Option<u32>),
    Then(Option<u32>),
    Plus,
    Minus,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Boost(f64),
}

impl TokenKind {
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Word { text, .. } => text.clone(),
            TokenKind::Phrase { .. } => "phrase".into(),
            TokenKind::All => "*".into(),
            TokenKind::And => "AND".into(),
            TokenKind::Or => "OR".into(),
            TokenKind::Not => "NOT".into(),
            TokenKind::Near(Some(n)) => format!("NEAR/{n}"),
            TokenKind::Near(None) => "NEAR".into(),
            TokenKind::Then(Some(n)) => format!("THEN/{n}"),
            TokenKind::Then(None) => "THEN".into(),
            TokenKind::Plus => "+".into(),
            TokenKind::Minus => "-".into(),
            TokenKind::LParen => "(".into(),
            TokenKind::RParen => ")".into(),
            TokenKind::LBracket => "[".into(),
            TokenKind::RBracket => "]".into(),
            TokenKind::Boost(_) => "^".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub comments: Vec<Span>,
    pub diagnostics: DiagnosticList,
}

fn is_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | '[' | ']' | '"' | '~' | '^')
}

/// Characters a backslash escapes inside a bare term. `*` and `?` keep their
/// backslash so the emitter can tell literal from wildcard.
fn is_term_escapable(c: char) -> bool {
    matches!(
        c,
        '*' | '?' | '(' | ')' | '[' | ']' | '"' | '~' | '^' | '\\' | ' '
    )
}

struct Lexer<'a> {
    source: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    comments: Vec<Span>,
    diagnostics: DiagnosticList,
}

pub fn lex(source: &str) -> Lexed {
    let mut lexer = Lexer {
        source,
        pos: 0,
        tokens: Vec::new(),
        comments: Vec::new(),
        diagnostics: DiagnosticList::new(),
    };
    lexer.run();
    Lexed {
        tokens: lexer.tokens,
        comments: lexer.comments,
        diagnostics: lexer.diagnostics,
    }
}

/// The query with its comments removed. A block comment becomes one space so
/// the words around it stay apart; a line comment keeps its newline.
pub fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut last = 0;
    for span in lex(source).comments {
        out.push_str(&source[last..span.start]);
        if source[span.start..].starts_with(BLOCK_COMMENT_OPEN) {
            out.push(' ');
        }
        last = span.end;
    }
    out.push_str(&source[last..]);
    out
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.source[self.pos..].chars().nth(offset)
    }

    fn rest(&self) -> &'a str {
        &self.source[self.pos..]
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn error(&mut self, code: &str, message: impl Into<String>, start: usize, end: usize) {
        self.diagnostics.push(
            Diagnostic::error(message, Range::from_offsets(self.source, start, end))
                .with_code(code),
        );
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
    }

    fn run(&mut self) {
        while let Some(c) = self.peek() {
            let start = self.pos;
            if c.is_whitespace() {
                self.bump();
                continue;
            }
            if self.rest().starts_with(LINE_COMMENT) {
                self.line_comment();
                continue;
            }
            if self.rest().starts_with(BLOCK_COMMENT_OPEN) {
                self.block_comment();
                continue;
            }
            if self.rest().starts_with(BLOCK_COMMENT_CLOSE) {
                self.stray_comment_close();
                continue;
            }
            match c {
                '(' => {
                    self.bump();
                    self.push(TokenKind::LParen, start);
                }
                ')' => {
                    self.bump();
                    self.push(TokenKind::RParen, start);
                }
                '[' => {
                    self.bump();
                    self.push(TokenKind::LBracket, start);
                }
                ']' => {
                    self.bump();
                    self.push(TokenKind::RBracket, start);
                }
                '"' => self.phrase('"'),
                '\'' if self.single_quote_opens_phrase() => self.phrase('\''),
                '^' => self.boost(),
                '~' => {
                    self.bump();
                    self.scan_digits();
                    self.error(
                        "dangling-modifier",
                        "~ must directly follow a term (fuzzy) or a closing quote (proximity)",
                        start,
                        self.pos,
                    );
                }
                '+' | '-' if self.sign_starts_operand() => {
                    self.bump();
                    let kind = if c == '+' {
                        TokenKind::Plus
                    } else {
                        TokenKind::Minus
                    };
                    self.push(kind, start);
                }
                _ => self.word(),
            }
        }
    }

    /// `//` at a token start runs to the end of the line; inside a word
    /// (`https://…`) it is term text.
    fn line_comment(&mut self) {
        let start = self.pos;
        self.pos += self.rest().find('\n').unwrap_or(self.rest().len());
        self.comments.push(Span::new(start, self.pos));
    }

    /// Brandwatch-style `<<< … >>>`, may span lines, does not nest.
    fn block_comment(&mut self) {
        let start = self.pos;
        let body = start + BLOCK_COMMENT_OPEN.len();
        match self.source[body..].find(BLOCK_COMMENT_CLOSE) {
            Some(close) => {
                self.pos = body + close + BLOCK_COMMENT_CLOSE.len();
                self.comments.push(Span::new(start, self.pos));
            }
            None => {
                self.pos = self.source.len();
                let end = Range::from_offsets(self.source, self.pos, self.pos);
                self.diagnostics.push(
                    Diagnostic::error(
                        "Missing >>> to close this comment",
                        Range::from_offsets(self.source, start, body),
                    )
                    .with_code("unterminated-comment")
                    .with_fix("Close comment", end, BLOCK_COMMENT_CLOSE),
                );
            }
        }
    }

    fn stray_comment_close(&mut self) {
        let start = self.pos;
        self.pos += BLOCK_COMMENT_CLOSE.len();
        let range = Range::from_offsets(self.source, start, self.pos);
        self.diagnostics.push(
            Diagnostic::error("Comment end >>> has no opening <<<", range)
                .with_code("stray-comment-end")
                .with_fix("Remove >>>", range, ""),
        );
    }

    /// A single quote opens a phrase only at a token start; apostrophes inside
    /// words (o'brien) are term characters.
    fn single_quote_opens_phrase(&self) -> bool {
        let rest = &self.source[self.pos + 1..];
        rest.contains('\'')
    }

    fn sign_starts_operand(&self) -> bool {
        match self.peek_at(1) {
            None => false,
            Some(next) => !next.is_whitespace() && !matches!(next, ')' | ']' | '~' | '^' | '['),
        }
    }

    fn scan_digits(&mut self) -> Option<u32> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.bump();
        }
        if start == self.pos {
            return None;
        }
        Some(
            self.source[start..self.pos]
                .parse::<u32>()
                .unwrap_or(u32::MAX),
        )
    }

    fn phrase(&mut self, quote: char) {
        let start = self.pos;
        self.bump();
        let mut text = String::new();
        let mut terminated = false;
        while let Some(c) = self.bump() {
            if c == quote {
                terminated = true;
                break;
            }
            if c == '\\' {
                if let Some(escaped) = self.bump() {
                    text.push(escaped);
                }
                continue;
            }
            text.push(c);
        }
        if !terminated {
            self.error(
                "unterminated-phrase",
                format!("Missing closing {quote} for this phrase"),
                start,
                self.pos,
            );
        }
        let mut slop = None;
        if terminated && self.peek() == Some('~') {
            let tilde = self.pos;
            self.bump();
            match self.scan_digits() {
                Some(n) => slop = Some(n),
                None => self.error(
                    "invalid-slop",
                    "Proximity needs a number after ~, for example \"health care\"~3",
                    tilde,
                    self.pos,
                ),
            }
        }
        self.push(TokenKind::Phrase { text, slop }, start);
    }

    fn boost(&mut self) {
        let start = self.pos;
        self.bump();
        let number_start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
            self.bump();
        }
        match self.source[number_start..self.pos].parse::<f64>() {
            Ok(factor) if number_start != self.pos => self.push(TokenKind::Boost(factor), start),
            _ => self.error(
                "invalid-boost",
                "Boost needs a number after ^, for example term^2",
                start,
                self.pos,
            ),
        }
    }

    fn field_ignored(&mut self, start: usize, len: usize) {
        let range = Range::from_offsets(self.source, start, start + len);
        self.diagnostics.push(
            Diagnostic::warning(
                format!(
                    "Field prefix \"{}\" is ignored; all terms search post content",
                    &self.source[start..start + len]
                ),
                range,
            )
            .with_code("field-ignored")
            .with_fix("Remove field prefix", range, ""),
        );
    }

    fn word(&mut self) {
        let start = self.pos;
        let mut text = String::new();
        let mut wildcard = false;
        let mut escaped_any = false;
        while let Some(c) = self.peek() {
            if is_boundary(c) || self.rest().starts_with(BLOCK_COMMENT_OPEN) {
                break;
            }
            self.bump();
            match c {
                '\\' => match self.peek() {
                    Some(next) if is_term_escapable(next) => {
                        self.bump();
                        escaped_any = true;
                        if matches!(next, '*' | '?') {
                            text.push('\\');
                        }
                        text.push(next);
                    }
                    _ => text.push('\\'),
                },
                '*' | '?' => {
                    wildcard = true;
                    text.push(c);
                }
                _ => text.push(c),
            }
        }
        let raw = &self.source[start..self.pos];

        if !escaped_any {
            match raw {
                "*" => {
                    self.push(TokenKind::All, start);
                    return;
                }
                "AND" => {
                    self.push(TokenKind::And, start);
                    return;
                }
                "OR" => {
                    self.push(TokenKind::Or, start);
                    return;
                }
                "NOT" => {
                    self.push(TokenKind::Not, start);
                    return;
                }
                _ => {}
            }
            for (keyword, make) in [
                ("NEAR", TokenKind::Near as fn(Option<u32>) -> TokenKind),
                ("THEN", TokenKind::Then as fn(Option<u32>) -> TokenKind),
            ] {
                if raw == keyword || raw.starts_with(&format!("{keyword}/")) {
                    let gap = raw
                        .get(keyword.len() + 1..)
                        .filter(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()))
                        .map(|g| g.parse::<u32>().unwrap_or(u32::MAX));
                    if gap.is_none() {
                        self.error(
                            "invalid-proximity",
                            format!(
                                "{keyword} needs a distance, for example {keyword}/3; \
                                 quote \"{keyword}\" to search for the word"
                            ),
                            start,
                            self.pos,
                        );
                    }
                    self.push(make(gap), start);
                    return;
                }
            }
        }

        if let Some(colon) = raw.find(':')
            && KNOWN_FIELDS.contains(&&raw[..colon])
        {
            self.field_ignored(start, colon + 1);
            if colon + 1 == raw.len() {
                // `content:"x y"`, `content:(a OR b)`, `content: apple`: the
                // prefix stands alone and attaches to whatever follows.
                return;
            }
            text = text[colon + 1..].to_string();
        }

        let mut fuzzy = None;
        if self.peek() == Some('~') {
            let tilde = self.pos;
            self.bump();
            match self.scan_digits() {
                Some(first) => {
                    if self.peek() == Some(':') {
                        self.bump();
                        match self.scan_digits() {
                            Some(distance) => {
                                fuzzy = Some(Fuzzy {
                                    distance,
                                    prefix: Some(first),
                                })
                            }
                            None => self.error(
                                "invalid-fuzzy",
                                "Fuzzy prefix form is term~PREFIX:DISTANCE, for example apple~1:2",
                                tilde,
                                self.pos,
                            ),
                        }
                    } else {
                        fuzzy = Some(Fuzzy {
                            distance: first,
                            prefix: None,
                        });
                    }
                }
                None => self.error(
                    "invalid-fuzzy",
                    "Fuzzy match needs a number after ~, for example apple~1",
                    tilde,
                    self.pos,
                ),
            }
        }

        self.push(
            TokenKind::Word {
                text,
                wildcard,
                fuzzy,
            },
            start,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source).tokens.into_iter().map(|t| t.kind).collect()
    }

    fn word(text: &str) -> TokenKind {
        TokenKind::Word {
            text: text.into(),
            wildcard: false,
            fuzzy: None,
        }
    }

    #[test]
    fn words_and_keywords() {
        assert_eq!(
            kinds("apple AND banana OR NOT cherry"),
            vec![
                word("apple"),
                TokenKind::And,
                word("banana"),
                TokenKind::Or,
                TokenKind::Not,
                word("cherry")
            ]
        );
    }

    #[test]
    fn lowercase_keywords_are_words() {
        assert_eq!(
            kinds("rock and roll"),
            vec![word("rock"), word("and"), word("roll")]
        );
    }

    #[test]
    fn newline_and_tab_are_separators() {
        assert_eq!(
            kinds("apple AND\n(banana\tOR\r\ncherry)"),
            vec![
                word("apple"),
                TokenKind::And,
                TokenKind::LParen,
                word("banana"),
                TokenKind::Or,
                word("cherry"),
                TokenKind::RParen
            ]
        );
    }

    #[test]
    fn keyword_followed_by_paren_is_keyword() {
        assert_eq!(
            kinds("a AND(b)"),
            vec![
                word("a"),
                TokenKind::And,
                TokenKind::LParen,
                word("b"),
                TokenKind::RParen
            ]
        );
    }

    #[test]
    fn phrases_with_slop_and_escapes() {
        assert_eq!(
            kinds(r#""health care"~3 'single quoted' "say \"hi\"""#),
            vec![
                TokenKind::Phrase {
                    text: "health care".into(),
                    slop: Some(3)
                },
                TokenKind::Phrase {
                    text: "single quoted".into(),
                    slop: None
                },
                TokenKind::Phrase {
                    text: "say \"hi\"".into(),
                    slop: None
                },
            ]
        );
    }

    #[test]
    fn apostrophes_inside_words_stay() {
        assert_eq!(
            kinds("o'brien rock'n'roll"),
            vec![word("o'brien"), word("rock'n'roll")]
        );
    }

    #[test]
    fn unterminated_phrase_is_reported() {
        let lexed = lex("apple \"banana cherry");
        assert_eq!(lexed.diagnostics.codes(), vec!["unterminated-phrase"]);
        assert_eq!(lexed.diagnostics.items[0].range.start.offset, 6);
        assert_eq!(lexed.tokens.len(), 2);
    }

    #[test]
    fn signs_only_before_operands() {
        assert_eq!(
            kinds("-apple +\"pie\" -(a) c++ covid-19 - x"),
            vec![
                TokenKind::Minus,
                word("apple"),
                TokenKind::Plus,
                TokenKind::Phrase {
                    text: "pie".into(),
                    slop: None
                },
                TokenKind::Minus,
                TokenKind::LParen,
                word("a"),
                TokenKind::RParen,
                word("c++"),
                word("covid-19"),
                word("-"),
                word("x"),
            ]
        );
    }

    #[test]
    fn wildcards_fuzzy_and_escapes() {
        assert_eq!(
            kinds(r"appl* p?ach file\*name apple~2 apple~0:2"),
            vec![
                TokenKind::Word {
                    text: "appl*".into(),
                    wildcard: true,
                    fuzzy: None
                },
                TokenKind::Word {
                    text: "p?ach".into(),
                    wildcard: true,
                    fuzzy: None
                },
                TokenKind::Word {
                    text: r"file\*name".into(),
                    wildcard: false,
                    fuzzy: None
                },
                TokenKind::Word {
                    text: "apple".into(),
                    wildcard: false,
                    fuzzy: Some(Fuzzy {
                        distance: 2,
                        prefix: None
                    })
                },
                TokenKind::Word {
                    text: "apple".into(),
                    wildcard: false,
                    fuzzy: Some(Fuzzy {
                        distance: 2,
                        prefix: Some(0)
                    })
                },
            ]
        );
    }

    #[test]
    fn match_all_and_proximity_keywords() {
        assert_eq!(
            kinds("* a NEAR/3 b THEN/0 c"),
            vec![
                TokenKind::All,
                word("a"),
                TokenKind::Near(Some(3)),
                word("b"),
                TokenKind::Then(Some(0)),
                word("c")
            ]
        );
    }

    #[test]
    fn proximity_without_distance_is_an_error() {
        let lexed = lex("a NEAR b");
        assert_eq!(lexed.diagnostics.codes(), vec!["invalid-proximity"]);
        assert_eq!(lexed.tokens[1].kind, TokenKind::Near(None));
    }

    #[test]
    fn field_prefix_is_stripped_with_warning() {
        let lexed = lex("content_exact:apple content:\"x\" user@host:port");
        assert_eq!(
            lexed.diagnostics.codes(),
            vec!["field-ignored", "field-ignored"]
        );
        assert_eq!(lexed.tokens[0].kind, word("apple"));
        assert_eq!(
            lexed.tokens[1].kind,
            TokenKind::Phrase {
                text: "x".into(),
                slop: None
            }
        );
        assert_eq!(lexed.tokens[2].kind, word("user@host:port"));
    }

    #[test]
    fn detached_field_prefix_attaches_to_what_follows() {
        assert_eq!(
            kinds("content:(apple OR pie) content: cherry"),
            vec![
                TokenKind::LParen,
                word("apple"),
                TokenKind::Or,
                word("pie"),
                TokenKind::RParen,
                word("cherry")
            ]
        );
        let lexed = lex("content:");
        assert!(lexed.tokens.is_empty());
        assert_eq!(lexed.diagnostics.codes(), vec!["field-ignored"]);
    }

    #[test]
    fn boost_and_dangling_modifiers() {
        let lexed = lex("apple^2 ~3 pie^");
        assert_eq!(lexed.tokens[1].kind, TokenKind::Boost(2.0));
        assert_eq!(
            lexed.diagnostics.codes(),
            vec!["dangling-modifier", "invalid-boost"]
        );
    }

    #[test]
    fn line_comments_run_to_end_of_line() {
        assert_eq!(
            kinds("apple // fruit\n// banana OR\n(pie//x\n)"),
            vec![
                word("apple"),
                TokenKind::LParen,
                word("pie//x"),
                TokenKind::RParen
            ]
        );
        assert_eq!(
            kinds("https://example.com"),
            vec![word("https://example.com")]
        );
        assert_eq!(
            kinds("\"a // b\""),
            vec![TokenKind::Phrase {
                text: "a // b".into(),
                slop: None
            }]
        );
    }

    #[test]
    fn block_comments_span_lines_and_split_words() {
        let lexed = lex("apple<<<note>>>AND <<<multi\nline OR>>> pie");
        assert!(lexed.diagnostics.is_empty());
        assert_eq!(
            lexed
                .tokens
                .iter()
                .map(|t| t.kind.clone())
                .collect::<Vec<_>>(),
            vec![word("apple"), TokenKind::And, word("pie")]
        );
        assert_eq!(lexed.tokens[2].span, Span::new(39, 42));
    }

    #[test]
    fn strip_comments_keeps_words_apart() {
        assert_eq!(
            strip_comments("apple<<<x>>>pie // note\nOR \"a // b\" https://x.com"),
            "apple pie \nOR \"a // b\" https://x.com"
        );
        assert_eq!(strip_comments("apple <<<open"), "apple <<<open");
    }

    #[test]
    fn unbalanced_block_comments_are_errors() {
        let lexed = lex("apple <<<oops");
        assert_eq!(lexed.diagnostics.codes(), vec!["unterminated-comment"]);
        assert_eq!(lexed.tokens.len(), 1);
        let lexed = lex("apple >>> pie");
        assert_eq!(lexed.diagnostics.codes(), vec!["stray-comment-end"]);
        assert_eq!(lexed.tokens.len(), 2);
    }

    #[test]
    fn spans_are_byte_offsets() {
        let lexed = lex("héllo \"wörld\"~1");
        assert_eq!(lexed.tokens[0].span, Span::new(0, 6));
        assert_eq!(lexed.tokens[1].span, Span::new(7, 17));
    }
}
