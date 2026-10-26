//! Tree walk (§ 2.2), suggestion context (§ 3.1), signable arguments (§ 2.6).

use rc_proto_v777::commands::{ArgType, Kind};

use crate::reader::Reader;
use crate::suggest::{Builder, Suggestions};
use crate::tree::{NodeId, Tree};
use crate::{CmdError, Data, LITERAL_INCORRECT};

/// Parsing source: permissions.
#[derive(Debug, Clone, Copy)]
pub struct Source {
    /// Usable "restricted" nodes (typing in chat: yes, § 4.5).
    pub restricted: bool,
    /// Allowed `@…` selectors.
    pub selectors: bool,
}

/// Parsing context: nodes read with their range, and child context after a redirect.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub root: NodeId,
    pub range: (usize, usize),
    pub nodes: Vec<(NodeId, (usize, usize))>,
    /// The last node read is executable.
    pub executable: bool,
    pub child: Option<Box<Ctx>>,
}

impl Ctx {
    fn new(root: NodeId, start: usize) -> Ctx {
        Ctx { root, range: (start, start), nodes: Vec::new(), executable: false, child: None }
    }

    fn with_node(&mut self, id: NodeId, range: (usize, usize)) {
        self.nodes.push((id, range));
        self.range = (self.range.0.min(range.0), self.range.1.max(range.1));
    }

    /// Deepest context.
    pub fn last_child(&self) -> &Ctx {
        let mut c = self;
        while let Some(ch) = &c.child {
            c = ch;
        }
        c
    }

    /// Parent node at the cursor and start of the next word.
    pub fn suggestion_context(&self, cursor: usize) -> (&Ctx, NodeId, usize) {
        if self.range.0 <= cursor {
            if self.range.1 < cursor {
                if let Some(ch) = &self.child {
                    return ch.suggestion_context(cursor);
                }
                if let Some(&(last, r)) = self.nodes.last() {
                    return (self, last, r.1 + 1);
                }
                return (self, self.root, self.range.0);
            }
            let mut prev = self.root;
            for &(id, r) in &self.nodes {
                if r.0 <= cursor && cursor <= r.1 {
                    return (self, prev, r.0);
                }
                prev = id;
            }
            return (self, prev, self.range.0);
        }
        (self, self.root, self.range.0)
    }
}

/// Result: context, position where reading stopped, errors per node tried.
#[derive(Debug, Clone)]
pub struct Parse {
    pub ctx: Ctx,
    pub cursor: usize,
    pub errors: Vec<(NodeId, CmdError)>,
}

impl Parse {
    /// The input was read to the end.
    pub fn complete(&self, input: &[char]) -> bool {
        self.cursor >= input.len()
    }

    /// Valid command: everything read, no error, last node executable.
    pub fn valid(&self, input: &[char]) -> bool {
        self.complete(input) && self.errors.is_empty() && self.ctx.last_child().executable
    }
}

struct Parser<'a> {
    tree: &'a Tree,
    input: &'a [char],
    data: &'a Data,
    src: Source,
}

/// Parses the full input (`/` included: reading starts after it).
pub fn parse(tree: &Tree, input: &[char], data: &Data, src: Source) -> Parse {
    let start = usize::from(input.first() == Some(&'/'));
    let p = Parser { tree, input, data, src };
    if tree.is_empty() {
        return Parse { ctx: Ctx::new(0, start), cursor: start, errors: Vec::new() };
    }
    p.parse_nodes(tree.root, start, &Ctx::new(tree.root, start))
}

impl Parser<'_> {
    fn can_use(&self, id: NodeId) -> bool {
        !self.tree.node(id).restricted || self.src.restricted
    }

    /// Relevant children: the exact literal at the cursor if it exists, otherwise the arguments.
    fn relevant(&self, node: NodeId, cursor: usize) -> Vec<NodeId> {
        let n = self.tree.node(node);
        if n.literals.is_empty() {
            return n.arguments.clone();
        }
        let end = self.input[cursor..].iter().position(|&c| c == ' ').map_or(self.input.len(), |p| cursor + p);
        let word: String = self.input[cursor..end].iter().collect();
        match n.literals.get(&word) {
            Some(&l) => vec![l],
            None => n.arguments.clone(),
        }
    }

    fn parse_child(&self, id: NodeId, r: &mut Reader<'_>, ctx: &mut Ctx) -> Result<(), CmdError> {
        let n = self.tree.node(id);
        let start = r.cursor;
        match n.kind {
            Kind::Literal => {
                let lit: Vec<char> = n.name.chars().collect();
                let end = start + lit.len();
                let ok = end <= self.input.len()
                    && self.input[start..end] == lit[..]
                    && (end == self.input.len() || self.input[end] == ' ');
                if !ok {
                    return Err(r.err(LITERAL_INCORRECT, vec![n.name.clone()]));
                }
                r.cursor = end;
            }
            _ => {
                let Some(ty) = &n.arg else { return Ok(()) };
                crate::args::parse(ty, r, self.data, self.src)?;
            }
        }
        ctx.with_node(id, (start, r.cursor));
        if r.can_read() && r.peek() != ' ' {
            return Err(r.err0("command.expected.separator"));
        }
        Ok(())
    }

    fn parse_nodes(&self, node: NodeId, cursor: usize, so_far: &Ctx) -> Parse {
        let mut errors = Vec::new();
        let mut potentials: Vec<Parse> = Vec::new();
        for child in self.relevant(node, cursor) {
            if !self.can_use(child) {
                continue;
            }
            let mut ctx = so_far.clone();
            let mut r = Reader::new(self.input, cursor);
            if let Err(e) = self.parse_child(child, &mut r, &mut ctx) {
                errors.push((child, e));
                continue;
            }
            let c = self.tree.node(child);
            ctx.executable = c.executable;
            let need = if c.redirect.is_none() { 2 } else { 1 };
            if r.can_read_n(need) {
                r.skip();
                if let Some(target) = c.redirect {
                    let sub = self.parse_nodes(target, r.cursor, &Ctx::new(target, r.cursor));
                    ctx.child = Some(Box::new(sub.ctx));
                    return Parse { ctx, cursor: sub.cursor, errors: sub.errors };
                }
                potentials.push(self.parse_nodes(child, r.cursor, &ctx));
            } else {
                potentials.push(Parse { ctx, cursor: r.cursor, errors: Vec::new() });
            }
        }
        if !potentials.is_empty() {
            let len = self.input.len();
            potentials.sort_by_key(|p| (p.cursor < len, !p.errors.is_empty()));
            return potentials.swap_remove(0);
        }
        Parse { ctx: so_far.clone(), cursor, errors }
    }
}

/// Result of local suggestions; `server`: server request needed (§ 3.2).
#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub local: Vec<Suggestions>,
    pub server: bool,
    /// Start of the word at the cursor (to place the usage).
    pub start: usize,
    /// Parent node at the cursor.
    pub parent: NodeId,
}

/// Suggestions at the cursor: one source per child of the parent.
pub fn completions(tree: &Tree, parse: &Parse, input: &[char], cursor: usize, data: &Data, src: Source) -> Completion {
    let (_, parent, start) = parse.ctx.suggestion_context(cursor);
    let start = start.min(cursor);
    let truncated = &input[..cursor.min(input.len())];
    let mut out = Completion { local: Vec::new(), server: false, start, parent };
    if tree.is_empty() {
        return out;
    }
    for &child in &tree.node(parent).children {
        let n = tree.node(child);
        let mut b = Builder::new(truncated, start);
        match n.kind {
            Kind::Literal => {
                if n.name.to_lowercase().starts_with(&b.remaining_lower) {
                    let name = n.name.clone();
                    b.suggest(name);
                }
            }
            _ => {
                let Some(ty) = &n.arg else { continue };
                let asked = match n.suggestions.as_deref() {
                    Some(id) => crate::args::custom_suggestions(id, &mut b, data),
                    None => crate::args::suggest(ty, &mut b, data, src),
                };
                if asked {
                    out.server = true;
                    continue;
                }
            }
        }
        out.local.push(b.build());
    }
    out
}

/// Signable arguments (§ 2.6): (name, text) of the `message` arguments read, child contexts
/// included, without following a redirect to the root.
pub fn signable(tree: &Tree, parse: &Parse, input: &[char]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut ctx = Some(&parse.ctx);
    while let Some(c) = ctx {
        let mut redirect_root = false;
        for &(id, (a, b)) in &c.nodes {
            let n = tree.node(id);
            if matches!(n.arg, Some(ArgType::Message)) {
                out.push((n.name.clone(), input[a..b.min(input.len())].iter().collect()));
            }
            if n.redirect == Some(tree.root) {
                redirect_root = true;
            }
        }
        ctx = if redirect_root { None } else { c.child.as_deref() };
    }
    out
}
