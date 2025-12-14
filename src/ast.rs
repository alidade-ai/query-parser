use napi_derive::napi;
use serde::{Deserialize, Serialize};
use tantivy_query_grammar::{
    Occur as TantivyOccur, UserInputAst, UserInputBound as TantivyBound, UserInputLeaf,
    UserInputLiteral,
};

/// Occurrence modifier for a clause term
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Occur {
    /// Term must appear (AND, +)
    Must,
    /// Term must not appear (NOT, -)
    MustNot,
    /// Term should appear (OR, default)
    Should,
}

impl From<Option<TantivyOccur>> for Occur {
    fn from(occur: Option<TantivyOccur>) -> Self {
        match occur {
            Some(TantivyOccur::Must) => Occur::Must,
            Some(TantivyOccur::MustNot) => Occur::MustNot,
            Some(TantivyOccur::Should) | None => Occur::Should,
        }
    }
}

/// Bound type for range queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BoundType {
    /// Inclusive bound [value
    Inclusive,
    /// Exclusive bound {value
    Exclusive,
    /// Unbounded (*)
    Unbounded,
}

/// A bound value for range queries
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bound {
    /// The type of bound
    pub bound_type: BoundType,
    /// The bound value (None if unbounded)
    pub value: Option<String>,
}

impl From<&TantivyBound> for Bound {
    fn from(bound: &TantivyBound) -> Self {
        match bound {
            TantivyBound::Inclusive(v) => Bound {
                bound_type: BoundType::Inclusive,
                value: Some(v.clone()),
            },
            TantivyBound::Exclusive(v) => Bound {
                bound_type: BoundType::Exclusive,
                value: Some(v.clone()),
            },
            TantivyBound::Unbounded => Bound {
                bound_type: BoundType::Unbounded,
                value: None,
            },
        }
    }
}

/// Delimiter type for literals
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Delimiter {
    /// No delimiter (bare word)
    None,
    /// Double quotes
    DoubleQuotes,
    /// Single quotes
    SingleQuotes,
}

impl From<tantivy_query_grammar::Delimiter> for Delimiter {
    fn from(d: tantivy_query_grammar::Delimiter) -> Self {
        match d {
            tantivy_query_grammar::Delimiter::None => Delimiter::None,
            tantivy_query_grammar::Delimiter::DoubleQuotes => Delimiter::DoubleQuotes,
            tantivy_query_grammar::Delimiter::SingleQuotes => Delimiter::SingleQuotes,
        }
    }
}

/// A literal term in the query
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Literal {
    /// Optional field name
    pub field: Option<String>,
    /// The phrase/term value
    pub phrase: String,
    /// Delimiter used
    pub delimiter: Delimiter,
    /// Slop for phrase queries (distance between terms)
    pub slop: u32,
    /// Whether this is a prefix query
    pub prefix: bool,
}

impl From<&UserInputLiteral> for Literal {
    fn from(lit: &UserInputLiteral) -> Self {
        Literal {
            field: lit.field_name.clone(),
            phrase: lit.phrase.clone(),
            delimiter: lit.delimiter.into(),
            slop: lit.slop,
            prefix: lit.prefix,
        }
    }
}

/// A range query
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeQuery {
    /// Optional field name
    pub field: Option<String>,
    /// Lower bound
    pub lower: Bound,
    /// Upper bound
    pub upper: Bound,
}

/// A set/IN query
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetQuery {
    /// Optional field name
    pub field: Option<String>,
    /// Set elements
    pub elements: Vec<String>,
}

/// An exists query (field:*)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistsQuery {
    /// Field name
    pub field: String,
}

/// Types of leaf nodes in the AST
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LeafNode {
    /// A literal term or phrase
    Literal(Literal),
    /// Match all documents (*)
    All,
    /// A range query
    Range(RangeQuery),
    /// A set/IN query
    Set(SetQuery),
    /// An exists query
    Exists(ExistsQuery),
}

impl From<&UserInputLeaf> for LeafNode {
    fn from(leaf: &UserInputLeaf) -> Self {
        match leaf {
            UserInputLeaf::Literal(lit) => LeafNode::Literal(lit.into()),
            UserInputLeaf::All => LeafNode::All,
            UserInputLeaf::Range {
                field,
                lower,
                upper,
            } => LeafNode::Range(RangeQuery {
                field: field.clone(),
                lower: lower.into(),
                upper: upper.into(),
            }),
            UserInputLeaf::Set { field, elements } => LeafNode::Set(SetQuery {
                field: field.clone(),
                elements: elements.clone(),
            }),
            UserInputLeaf::Exists { field } => LeafNode::Exists(ExistsQuery {
                field: field.clone(),
            }),
        }
    }
}

/// A clause member (occur + node)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClauseMember {
    /// Occurrence modifier
    pub occur: Occur,
    /// The AST node
    pub node: QueryNode,
}

/// Boolean clause containing multiple members
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clause {
    pub members: Vec<ClauseMember>,
}

/// Main AST node type - can be a leaf, clause, or boost
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "nodeType", rename_all = "camelCase")]
pub enum QueryNode {
    /// Leaf node (literal, range, set, etc.)
    Leaf(LeafNode),
    /// Boolean clause
    Clause(Clause),
    /// Boosted node
    Boost {
        factor: f64,
        node: Box<QueryNode>,
    },
}

impl From<&UserInputAst> for QueryNode {
    fn from(ast: &UserInputAst) -> Self {
        match ast {
            UserInputAst::Clause(members) => {
                let members = members
                    .iter()
                    .map(|(occur, node)| ClauseMember {
                        occur: (*occur).into(),
                        node: QueryNode::from(node),
                    })
                    .collect();
                QueryNode::Clause(Clause { members })
            }
            UserInputAst::Leaf(leaf) => QueryNode::Leaf(LeafNode::from(leaf.as_ref())),
            UserInputAst::Boost(inner, factor) => QueryNode::Boost {
                factor: *factor,
                node: Box::new(QueryNode::from(inner.as_ref())),
            },
        }
    }
}

/// Summary statistics about a parsed query
#[napi(object)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryStats {
    /// Total number of terms
    pub term_count: u32,
    /// Maximum nesting depth
    pub max_depth: u32,
    /// Fields referenced in the query
    pub fields: Vec<String>,
    /// Whether the query contains a match-all (*)
    pub has_match_all: bool,
    /// Whether the query contains any boosted terms
    pub has_boost: bool,
    /// Whether the query contains any negations
    pub has_negation: bool,
    /// Whether the query contains any phrase queries
    pub has_phrase: bool,
    /// Whether the query contains any range queries
    pub has_range: bool,
}

impl QueryStats {
    pub fn from_node(node: &QueryNode) -> Self {
        let mut stats = QueryStats::default();
        Self::collect_stats(node, &mut stats, 1);
        stats.fields.sort();
        stats.fields.dedup();
        stats
    }

    fn collect_stats(node: &QueryNode, stats: &mut QueryStats, depth: u32) {
        stats.max_depth = stats.max_depth.max(depth);

        match node {
            QueryNode::Leaf(leaf) => {
                stats.term_count += 1;
                match leaf {
                    LeafNode::Literal(lit) => {
                        if let Some(field) = &lit.field {
                            stats.fields.push(field.clone());
                        }
                        if lit.phrase.contains(' ') || lit.slop > 0 {
                            stats.has_phrase = true;
                        }
                    }
                    LeafNode::All => {
                        stats.has_match_all = true;
                    }
                    LeafNode::Range(r) => {
                        stats.has_range = true;
                        if let Some(field) = &r.field {
                            stats.fields.push(field.clone());
                        }
                    }
                    LeafNode::Set(s) => {
                        if let Some(field) = &s.field {
                            stats.fields.push(field.clone());
                        }
                    }
                    LeafNode::Exists(e) => {
                        stats.fields.push(e.field.clone());
                    }
                }
            }
            QueryNode::Clause(clause) => {
                for member in &clause.members {
                    if member.occur == Occur::MustNot {
                        stats.has_negation = true;
                    }
                    Self::collect_stats(&member.node, stats, depth + 1);
                }
            }
            QueryNode::Boost { node, .. } => {
                stats.has_boost = true;
                Self::collect_stats(node, stats, depth + 1);
            }
        }
    }
}
