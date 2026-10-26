//! Suggestions computed by the server (`command_suggestion` → `command_suggestions`,
//! `commands.md` § 3.2): all children of the parent node at the cursor, providers specific to the
//! server for arguments marked `ask_server` (objectives, score holders, functions,
//! tags, storages, time marks…).

use rc_client::Event;
use rc_commands::suggest::{Builder, Suggestions, Tooltip};
use rc_proto::nbt::Nbt;
use rc_proto_v777::commands::{ArgType, Kind};

use crate::server::Server;

impl Server {
    /// Answer to a request (at most one per tick, the most recent).
    pub(crate) fn answer_suggestions(&mut self, tx: i32, text: &str) {
        let op = self.cmd.op;
        if op < 2 && text.chars().count() > 256 {
            return;
        }
        let sugg = self.compute_suggestions(text, op);
        let list = sugg
            .list
            .iter()
            .take(1000)
            .map(|s| {
                let tip = match &s.tooltip {
                    Some(Tooltip::Key(k)) => Some(Nbt::Compound(vec![("translate".into(), Nbt::String(k.clone()))])),
                    _ => None,
                };
                (s.text.clone(), tip)
            })
            .collect();
        let _ = self.send(Event::CommandSuggestions(rc_proto_v777::commands::ServerSuggestions {
            id: tx,
            start: sugg.start as i32,
            length: (sugg.end - sugg.start) as i32,
            list,
        }));
    }

    /// Complete suggestions at the end of the text.
    pub fn compute_suggestions(&mut self, text: &str, op: u8) -> Suggestions {
        let Some(lvl) = self.cmd.level(op) else { return Suggestions::default() };
        let tree = lvl.tree.clone();
        let input: Vec<char> = text.chars().collect();
        let src = rc_commands::parse::Source { restricted: true, selectors: op >= 2 };
        let parse = rc_commands::parse(&tree, &input, &self.cmd.data, src);
        let cursor = input.len();
        let (ctx, parent, start) = parse.ctx.suggestion_context(cursor);
        let start = start.min(cursor);
        // Arguments already read (for providers that depend on them: `tag … remove`).
        let read: Vec<(String, String)> = ctx
            .nodes
            .iter()
            .filter(|(id, _)| tree.node(*id).kind == Kind::Argument)
            .map(|(id, (a, b))| (tree.node(*id).name.clone(), input[*a..(*b).min(input.len())].iter().collect()))
            .collect();
        let path: Vec<String> = ctx
            .nodes
            .iter()
            .map(|(id, _)| {
                let n = tree.node(*id);
                if n.kind == Kind::Argument { format!("<{}>", n.name) } else { n.name.clone() }
            })
            .collect();
        let root_name = if ctx.root != tree.root { tree.node(ctx.root).name.clone() } else { String::new() };
        let mut all = Vec::new();
        for &child in &tree.node(parent).children {
            let n = tree.node(child);
            let mut b = Builder::new(&input, start);
            match n.kind {
                Kind::Literal => {
                    if n.name.to_lowercase().starts_with(&b.remaining_lower) {
                        b.suggest(n.name.clone());
                    }
                }
                _ => {
                    let Some(ty) = &n.arg else { continue };
                    match n.suggestions.as_deref() {
                        Some("minecraft:ask_server") => {
                            self.provider(&root_name, &path, &n.name, ty, &read, &mut b);
                        }
                        Some(id) => {
                            rc_commands::args::custom_suggestions(id, &mut b, &self.cmd.data);
                        }
                        None => {
                            if rc_commands::args::suggest(ty, &mut b, &self.cmd.data, src) {
                                self.type_suggestions(ty, &mut b);
                            }
                        }
                    }
                }
            }
            all.push(b.build());
        }
        Suggestions::merge(&input, all)
    }

    /// Suggestions of a type the client cannot compute alone.
    fn type_suggestions(&self, ty: &ArgType, b: &mut Builder<'_>) {
        match ty {
            ArgType::Objective => {
                let names: Vec<String> = self.cmd.scores.objectives.iter().map(|o| o.name.clone()).collect();
                b.suggest_matching(names);
            }
            ArgType::Resource(reg) | ArgType::ResourceKey(reg) | ArgType::ResourceSelector(reg) => {
                if let Some(e) = self.cmd.server_regs.get(reg) {
                    b.suggest_resources(e);
                }
            }
            _ => {}
        }
    }

    /// `ask_server` providers according to the command and the argument name.
    fn provider(
        &mut self,
        root: &str,
        path: &[String],
        name: &str,
        ty: &ArgType,
        read: &[(String, String)],
        b: &mut Builder<'_>,
    ) {
        let head = if root.is_empty() { path.first().map(String::as_str).unwrap_or("") } else { root };
        match (head, name) {
            ("trigger", "objective") => {
                let holder = self.name.clone();
                let names: Vec<String> = self
                    .cmd
                    .scores
                    .objectives
                    .iter()
                    .filter(|o| o.criteria == "trigger")
                    .filter(|o| self.cmd.scores.score(&holder, &o.name).is_some_and(|s| !s.locked))
                    .map(|o| o.name.clone())
                    .collect();
                b.suggest_matching(names);
            }
            (_, "name" | "function") if matches!(ty, ArgType::Function) => {
                let mut v: Vec<String> = self.cmd.function_tags.keys().map(|k| format!("#{k}")).collect();
                v.extend(self.cmd.functions.keys().cloned());
                b.suggest_resources(v);
            }
            ("tag", "name") => {
                let t = read.iter().find(|(n, _)| n == "targets").map(|(_, v)| v.clone()).unwrap_or_default();
                let src = self.player_source();
                let mut tags: Vec<String> =
                    self.arg_entities_opt(&src, &t).into_iter().flat_map(|e| self.ent_tags(e)).collect();
                tags.sort();
                tags.dedup();
                b.suggest_matching(tags);
            }
            (_, "target" | "source") if matches!(ty, ArgType::ResourceLocation) => {
                let keys: Vec<String> = self.cmd.storage.keys().cloned().collect();
                b.suggest_resources(keys);
            }
            ("time", "timemarker") => {
                let v: Vec<String> = self.cmd.time_markers.iter().filter(|m| m.show).map(|m| m.id.clone()).collect();
                b.suggest_resources(v);
            }
            ("time", "timeline") => {
                let v: Vec<String> = self.data.timelines.iter().map(|t| t.name.clone()).collect();
                b.suggest_resources(v);
            }
            ("tick", "rate") => b.suggest_matching(["20"]),
            ("tick", "time") => {
                if path.iter().any(|p| p == "step") {
                    b.suggest_matching(["1t", "1s"]);
                } else {
                    b.suggest_matching(["60s", "1d", "3d"]);
                }
            }
            ("schedule", "function") if path.iter().any(|p| p == "clear") => {
                let v: Vec<String> = self.cmd.scheduled.iter().map(|s| s.1.clone()).collect();
                b.suggest_matching(v);
            }
            ("place", "template") => {
                if let Some(v) = self.cmd.server_regs.get("minecraft:structure") {
                    b.suggest_resources(v);
                }
            }
            _ => {
                // Score holders: online players and selectors.
                if matches!(ty, ArgType::ScoreHolder { .. } | ArgType::Entity { .. } | ArgType::GameProfile) {
                    let src = rc_commands::parse::Source { restricted: true, selectors: self.cmd.op >= 2 };
                    rc_commands::args::suggest(&ArgType::ScoreHolder { multiple: true }, b, &self.cmd.data, src);
                }
            }
        }
    }
}
