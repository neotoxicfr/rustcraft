//! Tree built from the `commands` packet (§ 1.2).

use hashbrown::HashMap;

use rc_proto_v777::commands::{ArgType, Commands, Kind};

pub type NodeId = u32;

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: Kind,
    pub name: String,
    pub arg: Option<ArgType>,
    pub suggestions: Option<String>,
    pub executable: bool,
    pub restricted: bool,
    pub redirect: Option<NodeId>,
    /// Children in packet order (those that resolve to the root are removed).
    pub children: Vec<NodeId>,
    pub literals: HashMap<String, NodeId>,
    /// Argument children, in order.
    pub arguments: Vec<NodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: NodeId,
}

impl Tree {
    pub fn new(c: &Commands) -> Tree {
        let mut nodes: Vec<Node> = c
            .nodes
            .iter()
            .map(|n| Node {
                kind: n.kind,
                name: n.name.clone(),
                arg: n.arg.clone(),
                suggestions: n.suggestions.clone(),
                executable: n.executable,
                restricted: n.restricted,
                redirect: n.redirect,
                children: Vec::new(),
                literals: HashMap::new(),
                arguments: Vec::new(),
            })
            .collect();
        for (i, n) in c.nodes.iter().enumerate() {
            for &ch in &n.children {
                let k = nodes[ch as usize].kind;
                if k == Kind::Root {
                    continue;
                }
                let name = nodes[ch as usize].name.clone();
                let node = &mut nodes[i];
                // Same name already present: the first stays (the server sends none).
                if node.children.iter().any(|&o| c.nodes[o as usize].name == name) {
                    continue;
                }
                node.children.push(ch);
                if k == Kind::Literal {
                    node.literals.insert(name, ch);
                } else {
                    node.arguments.push(ch);
                }
            }
        }
        Tree { nodes, root: c.root }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Usage text of a node: literal = name, argument = `<name>`.
    pub fn usage_text(&self, id: NodeId) -> String {
        let n = self.node(id);
        match n.kind {
            Kind::Argument => format!("<{}>", n.name),
            _ => n.name.clone(),
        }
    }
}
