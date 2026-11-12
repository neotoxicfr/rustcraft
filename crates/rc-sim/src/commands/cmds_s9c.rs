//! S9: command predicates (`execute if predicate`, `advancements=` option of
//! selectors), simulated command block minecarts, `place`.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use super::ents::E;
use super::src::{Fail, Origin, Src};
use super::{Parsed, R, Seg, args};
use crate::progress::pred::{Ctx, Who};
use crate::server::Server;
use crate::tr;

impl Server {
    /// `execute if predicate <predicate>`: named or inline predicate, "command" context
    /// (source entity, source position), read in the source's world. The source's entity is
    /// `this` wherever it lives, like the game: when it lives in another world, the conditions
    /// reading it are answered there first, the rest of the predicate here.
    pub(crate) fn command_predicate(&mut self, s: &Src, text: &str) -> Result<bool, Fail> {
        let t = text.trim();
        let v: Value = if t.starts_with('{') || t.starts_with('[') {
            let n = super::snbt::parse(t).map_err(|_| Fail(tr!("argument.nbt.expected.compound")))?;
            super::cmds_s9b::nbt_to_json(&n)
        } else {
            let id = args::id(t);
            match self.life.pack.get("predicate", &id) {
                Some(p) => (*p).clone(),
                None => return Err(Fail(tr!("argument.resource_or_id.failed_to_parse", id))),
            }
        };
        let origin = Some(s.pos);
        let elsewhere = s.entity.and_then(|e| self.other_world_holding(e).map(|world| (e, world.to_owned())));
        let answers_about_this = match elsewhere {
            Some((e, world)) => self.in_world(&world, |sv| {
                sv.answers_about_this(&v, &Ctx { this: Some(Who::E(e)), origin, ..Ctx::default() })
            }),
            None => Vec::new(),
        };
        let ctx = Ctx { this: s.entity.map(Who::E), origin, answers_about_this: &answers_about_this, ..Ctx::default() };
        Ok(self.p_condition(&v, &ctx))
    }

    /// `advancements={…}` option of a selector, for an entity.
    pub(crate) fn selector_advancements(&self, list: &[(String, Value)], e: E) -> bool {
        if list.is_empty() {
            return true;
        }
        if e != E::Player {
            return false;
        }
        list.iter().all(|(id, want)| self.adv_predicate(id, want))
    }

    /// Activation of a command block minecart (the activation of a command block minecart →
    /// the execution of a command): "minecart" source at its position and its current rotation,
    /// level 2; success count written into its compound (`SuccessCount`).
    pub(crate) fn command_cart_perform(&mut self, id: i32, pos: [f64; 3], rot: [f32; 2]) {
        let Some(c) = self.cart_ref(id) else { return };
        let cmd = c.nbt.get("Command").and_then(Nbt::as_str).unwrap_or("").to_owned();
        if cmd.is_empty() || !self.rule("minecraft:command_blocks_work", true) {
            return;
        }
        let e = E::Other(id);
        let mut src = self.server_source();
        src.origin = Origin::Minecart(id);
        src.entity = Some(e);
        src.pos = pos;
        src.rot = rot;
        src.dim = self.dim.name.clone();
        src.level = 2;
        src.silent = false;
        src.name = "@".into();
        src.display = self.ent_display(e);
        self.cmd.successes = 0;
        let (ok, _) = self.perform(src, &cmd);
        let count = if ok { self.cmd.successes.max(1) } else { 0 };
        if let Some(c) = self.cart_mut(id) {
            crate::nbtio::set(&mut c.nbt, "SuccessCount", Nbt::Int(count));
        }
    }

    /// `place feature|structure|template|jigsaw`.
    pub(crate) fn cmd_place(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let kind = seg.path.get(1).map(String::as_str).unwrap_or("");
        match kind {
            "template" => self.place_template(s, p, seg),
            "feature" => self.place_feature_cmd(s, p, seg),
            "structure" => self.place_structure_cmd(s, p, seg),
            _ => self.place_jigsaw_cmd(s, p, seg),
        }
    }
}
