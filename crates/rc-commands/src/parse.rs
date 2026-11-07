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

/// What a failed child try may have changed in a context: put back by `Ctx::rewind`.
#[derive(Clone, Copy)]
struct CtxMark {
    nodes: usize,
    range: (usize, usize),
}

impl Ctx {
    fn new(root: NodeId, start: usize) -> Ctx {
        Ctx { root, range: (start, start), nodes: Vec::new(), executable: false, child: None }
    }

    fn with_node(&mut self, id: NodeId, range: (usize, usize)) {
        self.nodes.push((id, range));
        self.range = (self.range.0.min(range.0), self.range.1.max(range.1));
    }

    fn mark(&self) -> CtxMark {
        CtxMark { nodes: self.nodes.len(), range: self.range }
    }

    /// Back to `mark`: a child that fails after reading its node (missing separator) leaves that
    /// node and its range behind, nothing else.
    fn rewind(&mut self, mark: CtxMark) {
        self.nodes.truncate(mark.nodes);
        self.range = mark.range;
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
    /// `RUSTCRAFT_OPT_PARSERNOALLOC`: walk without copying contexts, child lists and words.
    no_alloc: bool,
}

/// `RUSTCRAFT_OPT_PARSERNOALLOC` (on by default, `=0` = the former walk), read once per process.
pub fn no_alloc_switch() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_PARSERNOALLOC")
}

/// Parses the full input (`/` included: reading starts after it).
pub fn parse(tree: &Tree, input: &[char], data: &Data, src: Source) -> Parse {
    parse_with(tree, input, data, src, no_alloc_switch())
}

/// `parse` with the walk chosen by the caller (`no_alloc`: the walk without copies); both walks
/// give the same result.
pub fn parse_with(tree: &Tree, input: &[char], data: &Data, src: Source, no_alloc: bool) -> Parse {
    let start = usize::from(input.first() == Some(&'/'));
    let p = Parser { tree, input, data, src, no_alloc };
    if tree.is_empty() {
        return Parse { ctx: Ctx::new(0, start), cursor: start, errors: Vec::new() };
    }
    if no_alloc {
        p.walk(tree.root, start, Ctx::new(tree.root, start))
    } else {
        p.parse_nodes(tree.root, start, &Ctx::new(tree.root, start))
    }
}

/// Longest word looked up among the literals without a heap copy (UTF-8 bytes); a longer word
/// goes through a `String`, so a server tree with longer literal names still parses.
const STACK_WORD: usize = 64;

impl<'a> Parser<'a> {
    fn can_use(&self, id: NodeId) -> bool {
        !self.tree.node(id).restricted || self.src.restricted
    }

    /// End of the word at `cursor` (next space or end of input).
    fn word_end(&self, cursor: usize) -> usize {
        self.input[cursor..].iter().position(|&c| c == ' ').map_or(self.input.len(), |p| cursor + p)
    }

    /// Relevant children: the exact literal at the cursor if it exists, otherwise the arguments.
    fn relevant(&self, node: NodeId, cursor: usize) -> Vec<NodeId> {
        let n = self.tree.node(node);
        if n.literals.is_empty() {
            return n.arguments.clone();
        }
        let end = self.word_end(cursor);
        let word: String = self.input[cursor..end].iter().collect();
        match n.literals.get(&word) {
            Some(&l) => vec![l],
            None => n.arguments.clone(),
        }
    }

    /// `relevant` borrowed from the tree: no list copied, the word encoded on the stack.
    fn relevant_borrowed(&self, node: NodeId, cursor: usize) -> &'a [NodeId] {
        let n = self.tree.node(node);
        if n.literals.is_empty() {
            return &n.arguments;
        }
        let word = &self.input[cursor..self.word_end(cursor)];
        let mut bytes = [0u8; STACK_WORD];
        let found = match encode_word(word, &mut bytes) {
            Some(w) => n.literals.get(w),
            None => n.literals.get(&word.iter().collect::<String>()),
        };
        found.map_or(&n.arguments[..], std::slice::from_ref)
    }

    /// End of literal `name` read at `start` (followed by a space or the end), compared in place.
    fn literal_end(&self, start: usize, name: &str) -> Option<usize> {
        if self.no_alloc {
            let mut end = start;
            for c in name.chars() {
                if self.input.get(end) != Some(&c) {
                    return None;
                }
                end += 1;
            }
            return (end == self.input.len() || self.input[end] == ' ').then_some(end);
        }
        let lit: Vec<char> = name.chars().collect();
        let end = start + lit.len();
        let ok = end <= self.input.len()
            && self.input[start..end] == lit[..]
            && (end == self.input.len() || self.input[end] == ' ');
        ok.then_some(end)
    }

    fn parse_child(&self, id: NodeId, r: &mut Reader<'_>, ctx: &mut Ctx) -> Result<(), CmdError> {
        let n = self.tree.node(id);
        let start = r.cursor;
        match n.kind {
            Kind::Literal => {
                let Some(end) = self.literal_end(start, &n.name) else {
                    return Err(r.err(LITERAL_INCORRECT, vec![n.name.clone()]));
                };
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
            return self.best(potentials);
        }
        Parse { ctx: so_far.clone(), cursor, errors }
    }

    /// Best reading among the children that parsed: read to the end first, then without error,
    /// then the first tried.
    fn best(&self, mut potentials: Vec<Parse>) -> Parse {
        let len = self.input.len();
        potentials.sort_by_key(|p| (p.cursor < len, !p.errors.is_empty()));
        potentials.swap_remove(0)
    }

    /// `parse_nodes` without copies: the last usable child gets the context itself (put back as
    /// it was if that child fails, the fallback result needs it), only the earlier ones a copy;
    /// the child list is borrowed from the tree. A node usually has one relevant child, so most
    /// nodes copy nothing.
    fn walk(&self, node: NodeId, cursor: usize, so_far: Ctx) -> Parse {
        let children = self.relevant_borrowed(node, cursor);
        let last_usable = children.iter().rposition(|&c| self.can_use(c));
        let mut kept = Some(so_far);
        let mut errors = Vec::new();
        let mut potentials: Vec<Parse> = Vec::new();
        for (i, &child) in children.iter().enumerate() {
            if !self.can_use(child) {
                continue;
            }
            // Only the last usable child takes `kept`: the earlier ones always find it.
            let lent = if Some(i) == last_usable { kept.take() } else { kept.clone() };
            let Some(mut ctx) = lent else { break };
            let mark = ctx.mark();
            let mut r = Reader::new(self.input, cursor);
            if let Err(e) = self.parse_child(child, &mut r, &mut ctx) {
                errors.push((child, e));
                if kept.is_none() {
                    ctx.rewind(mark);
                    kept = Some(ctx);
                }
                continue;
            }
            let c = self.tree.node(child);
            ctx.executable = c.executable;
            let need = if c.redirect.is_none() { 2 } else { 1 };
            if r.can_read_n(need) {
                r.skip();
                if let Some(target) = c.redirect {
                    let sub = self.walk(target, r.cursor, Ctx::new(target, r.cursor));
                    ctx.child = Some(Box::new(sub.ctx));
                    return Parse { ctx, cursor: sub.cursor, errors: sub.errors };
                }
                potentials.push(self.walk(child, r.cursor, ctx));
            } else {
                potentials.push(Parse { ctx, cursor: r.cursor, errors: Vec::new() });
            }
        }
        if !potentials.is_empty() {
            return self.best(potentials);
        }
        // No child parsed: the context was put back (or never lent). A child that parsed always
        // left a potential, so `kept` is there.
        let ctx = kept.expect("context lent to a child that parsed, yet no potential");
        Parse { ctx, cursor, errors }
    }
}

/// `word` as UTF-8 in `buf`, if it fits.
fn encode_word<'b>(word: &[char], buf: &'b mut [u8; STACK_WORD]) -> Option<&'b str> {
    let mut len = 0;
    for &c in word {
        let n = c.len_utf8();
        if len + n > STACK_WORD {
            return None;
        }
        c.encode_utf8(&mut buf[len..len + n]);
        len += n;
    }
    std::str::from_utf8(&buf[..len]).ok()
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
