use crate::ast::Node;

/// Canonical text for a query: explicit operators, original grouping.
pub fn format_node(node: &Node) -> String {
    match node {
        Node::Term { value, fuzzy, .. } => {
            let mut s = value.clone();
            if let Some(fuzzy) = fuzzy {
                s.push('~');
                if let Some(prefix) = fuzzy.prefix {
                    s.push_str(&prefix.to_string());
                    s.push(':');
                }
                s.push_str(&fuzzy.distance.to_string());
            }
            s
        }
        Node::Phrase { text, slop, .. } => {
            let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
            if *slop > 0 {
                format!("\"{escaped}\"~{slop}")
            } else {
                format!("\"{escaped}\"")
            }
        }
        Node::All { .. } => "*".to_string(),
        Node::Group { child, .. } => format!("({})", format_node(child)),
        Node::Not { child, .. } => match child.as_ref() {
            Node::Proximity { .. } | Node::And { .. } | Node::Or { .. } => {
                format!("NOT ({})", format_node(child))
            }
            _ => format!("NOT {}", format_node(child)),
        },
        Node::Proximity {
            left,
            right,
            gap,
            ordered,
            ..
        } => {
            let op = if *ordered { "THEN" } else { "NEAR" };
            let wrap = |n: &Node| match n {
                Node::Proximity { .. } => format!("({})", format_node(n)),
                _ => format_node(n),
            };
            format!("{} {op}/{gap} {}", wrap(left), wrap(right))
        }
        Node::And { children, .. } => children
            .iter()
            .map(format_node)
            .collect::<Vec<_>>()
            .join(" AND "),
        Node::Or { children, .. } => children
            .iter()
            .map(format_node)
            .collect::<Vec<_>>()
            .join(" OR "),
        Node::Boost { factor, child, .. } => format!("{}^{factor}", format_node(child)),
    }
}
