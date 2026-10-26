//! "Smart" usage of a node's children (§ 4.4).

use rc_proto_v777::commands::Kind;

use crate::parse::Source;
use crate::tree::{NodeId, Tree};

fn can_use(tree: &Tree, id: NodeId, src: Source) -> bool {
    !tree.node(id).restricted || src.restricted
}

/// Usage of each usable child of `node`: (child, is a literal, text).
pub fn smart_usage(tree: &Tree, node: NodeId, src: Source) -> Vec<(NodeId, bool, String)> {
    if tree.is_empty() {
        return Vec::new();
    }
    let optional = tree.node(node).executable;
    let mut out = Vec::new();
    for &c in &tree.node(node).children {
        if let Some(u) = usage(tree, c, src, optional, false) {
            out.push((c, tree.node(c).kind == Kind::Literal, u));
        }
    }
    out
}

fn usage(tree: &Tree, id: NodeId, src: Source, optional: bool, deep: bool) -> Option<String> {
    if !can_use(tree, id, src) {
        return None;
    }
    let n = tree.node(id);
    let me = if optional { format!("[{}]", tree.usage_text(id)) } else { tree.usage_text(id) };
    let child_opt = n.executable;
    let (open, close) = if child_opt { ("[", "]") } else { ("(", ")") };
    if deep {
        return Some(me);
    }
    if let Some(t) = n.redirect {
        let r = if t == tree.root { "...".to_owned() } else { format!("-> {}", tree.usage_text(t)) };
        return Some(format!("{me} {r}"));
    }
    let children: Vec<NodeId> = n.children.iter().copied().filter(|&c| can_use(tree, c, src)).collect();
    if children.len() == 1 {
        if let Some(u) = usage(tree, children[0], src, child_opt, child_opt) {
            return Some(format!("{me} {u}"));
        }
    } else if children.len() > 1 {
        let mut set: Vec<String> = Vec::new();
        for &c in &children {
            if let Some(u) = usage(tree, c, src, child_opt, true)
                && !set.contains(&u)
            {
                set.push(u);
            }
        }
        if set.len() == 1 {
            let u = &set[0];
            return Some(format!("{me} {}", if child_opt { format!("[{u}]") } else { u.clone() }));
        }
        if set.len() > 1 {
            let parts: Vec<String> = children.iter().map(|&c| tree.usage_text(c)).collect();
            return Some(format!("{me} {open}{}{close}", parts.join("|")));
        }
    }
    Some(me)
}
