use napi_derive::napi;
use serde::{Deserialize, Serialize};

/// Byte range in the source query (start inclusive, end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

/// Fuzzy modifier on a term: `term~N` or `term~P:N`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fuzzy {
    pub distance: u32,
    pub prefix: Option<u32>,
}

/// Query syntax tree. Every node carries the byte span it was parsed from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Node {
    /// Bare word. `wildcard` is set when the value holds an unescaped `*`/`?`;
    /// escaped ones stay as `\*` / `\?` in `value`.
    Term {
        value: String,
        wildcard: bool,
        fuzzy: Option<Fuzzy>,
        span: Span,
    },
    /// Quoted phrase with its raw inner text and optional `~N` slop.
    Phrase {
        text: String,
        slop: u32,
        span: Span,
    },
    /// Standalone `*`.
    All {
        span: Span,
    },
    /// Parenthesised sub-expression.
    Group {
        child: Box<Node>,
        span: Span,
    },
    /// `NOT x` / `-x`.
    Not {
        child: Box<Node>,
        span: Span,
    },
    /// `a NEAR/N b` (unordered) or `a THEN/N b` (ordered).
    Proximity {
        left: Box<Node>,
        right: Box<Node>,
        gap: u32,
        ordered: bool,
        span: Span,
    },
    And {
        children: Vec<Node>,
        span: Span,
    },
    Or {
        children: Vec<Node>,
        span: Span,
    },
    /// `x^N`; parsed for compatibility, ignored by the emitter.
    Boost {
        factor: f64,
        child: Box<Node>,
        span: Span,
    },
}

impl Node {
    pub fn span(&self) -> Span {
        match self {
            Node::Term { span, .. }
            | Node::Phrase { span, .. }
            | Node::All { span }
            | Node::Group { span, .. }
            | Node::Not { span, .. }
            | Node::Proximity { span, .. }
            | Node::And { span, .. }
            | Node::Or { span, .. }
            | Node::Boost { span, .. } => *span,
        }
    }

    /// The node with any wrapping groups and boosts peeled off.
    pub fn unwrapped(&self) -> &Node {
        match self {
            Node::Group { child, .. } | Node::Boost { child, .. } => child.unwrapped(),
            other => other,
        }
    }

    pub fn is_negation(&self) -> bool {
        matches!(self.unwrapped(), Node::Not { .. })
    }

    pub fn children(&self) -> Vec<&Node> {
        match self {
            Node::Term { .. } | Node::Phrase { .. } | Node::All { .. } => Vec::new(),
            Node::Group { child, .. } | Node::Not { child, .. } | Node::Boost { child, .. } => {
                vec![child]
            }
            Node::Proximity { left, right, .. } => vec![left, right],
            Node::And { children, .. } | Node::Or { children, .. } => children.iter().collect(),
        }
    }
}

/// Summary statistics about a parsed query
#[napi(object)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryStats {
    /// Number of terms and phrases
    pub term_count: u32,
    /// Maximum nesting depth
    pub max_depth: u32,
    /// Whether the query contains a match-all (*)
    pub has_match_all: bool,
    /// Whether the query contains any boosted terms
    pub has_boost: bool,
    /// Whether the query contains any negations
    pub has_negation: bool,
    /// Whether the query contains any phrase queries
    pub has_phrase: bool,
    /// Whether the query contains any wildcard terms
    pub has_wildcard: bool,
    /// Whether the query contains any fuzzy terms
    pub has_fuzzy: bool,
    /// Whether the query contains any NEAR/THEN proximity operators
    pub has_proximity: bool,
}

impl QueryStats {
    pub fn from_node(node: &Node) -> Self {
        let mut stats = QueryStats::default();
        Self::collect(node, &mut stats, 1);
        stats
    }

    fn collect(node: &Node, stats: &mut QueryStats, depth: u32) {
        stats.max_depth = stats.max_depth.max(depth);
        match node {
            Node::Term {
                wildcard, fuzzy, ..
            } => {
                stats.term_count += 1;
                stats.has_wildcard |= *wildcard;
                stats.has_fuzzy |= fuzzy.is_some();
            }
            Node::Phrase { .. } => {
                stats.term_count += 1;
                stats.has_phrase = true;
            }
            Node::All { .. } => stats.has_match_all = true,
            Node::Not { .. } => stats.has_negation = true,
            Node::Proximity { .. } => stats.has_proximity = true,
            Node::Boost { .. } => stats.has_boost = true,
            Node::Group { .. } | Node::And { .. } | Node::Or { .. } => {}
        }
        for child in node.children() {
            Self::collect(child, stats, depth + 1);
        }
    }
}
