//! Commands added by G3-F08: `compute` (value of a numeric provider), `posteffect` (screen
//! effects imposed on a player), `fetchprofile` (player profile resolved by the session
//! services, or carried by a player or a mannequin).

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rc_client::Event;
use rc_client::session::ProfileResult;
use rc_proto::nbt::Nbt;
use rc_text::{ClickEvent, Component, Content, ObjectInfo};

use super::cmds_basic::Track;
use super::ents::E;
use super::src::{Fail, Src};
use super::text::{self, GREEN, jf, lit};
use super::{Parsed, R, Seg, args, snbt};
use crate::server::Server;
use crate::tr;

/// Profile services used by `/fetchprofile`: identifier of a name (profile service), signed
/// profile of an identifier (session service). Replaced in the tests (no network).
#[derive(Clone, Copy)]
pub struct Services {
    pub id_by_name: fn(&str) -> Option<u128>,
    pub profile: fn(u128) -> Option<ProfileResult>,
}

impl Default for Services {
    fn default() -> Services {
        Services { id_by_name: network_id_by_name, profile: |id| rc_client::session::fetch_profile(id, true) }
    }
}

impl std::fmt::Debug for Services {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Services")
    }
}

/// Resolved profiles kept 10 minutes after their last use, 256 at most per kind (failures
/// included), like the game's resolver: by identifier, and by name as typed.
#[derive(Debug, Default)]
struct Cache {
    by_id: HashMap<u128, (Instant, Option<ProfileResult>)>,
    by_name: HashMap<String, (Instant, Option<ProfileResult>)>,
}

const CACHE_TTL: Duration = Duration::from_secs(600);
const CACHE_MAX: usize = 256;

fn cached<K: std::hash::Hash + Eq + Clone>(
    map: &mut HashMap<K, (Instant, Option<ProfileResult>)>,
    key: &K,
    load: impl FnOnce() -> Option<ProfileResult>,
) -> Option<ProfileResult> {
    let now = Instant::now();
    if let Some((at, v)) = map.get_mut(key)
        && now.duration_since(*at) < CACHE_TTL
    {
        *at = now;
        return v.clone();
    }
    let v = load();
    map.insert(key.clone(), (now, v.clone()));
    if map.len() > CACHE_MAX
        && let Some(old) = map.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone())
    {
        map.remove(&old);
    }
    v
}

/// Lookup asked by `/fetchprofile name|id`.
#[derive(Debug, Clone)]
enum Ask {
    Name(String),
    Id(u128),
}

/// Lookup in progress (answered on the server thread at a following tick, like the game).
#[derive(Debug)]
struct Pending {
    src: Src,
    ask: Ask,
    rx: Receiver<Option<ProfileResult>>,
}

/// State of `/fetchprofile`.
#[derive(Debug, Default)]
pub struct Profiles {
    pub services: Services,
    cache: Arc<Mutex<Cache>>,
    pending: Vec<Pending>,
}

/// Name accepted by the profile lookup: 16 characters at most, none of them a space or a
/// control character, nor beyond `~`.
fn valid_player_name(name: &str) -> bool {
    name.encode_utf16().count() <= 16 && !name.encode_utf16().any(|c| c <= 32 || c >= 127)
}

/// Identifier of a name through the profile service (name lowercased, inserted as it is in
/// the address, like the game). `None`: unknown or service unreachable.
fn network_id_by_name(name: &str) -> Option<u128> {
    let url = rc_client::session::endpoint("profiles", "getByName")?.replace("{name}", name);
    let mut r = rc_client::session::agent().get(&url).call().ok()?;
    if r.status().as_u16() >= 400 {
        return None;
    }
    let body = r.body_mut().with_config().limit(1 << 20).read_to_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    rc_client::auth::parse_uuid(v["id"].as_str()?)
}

/// Lookup on a worker thread: by identifier; by name, the name's identifier (lowercased; an
/// unknown name gets its offline identifier, looked up all the same), then by identifier.
fn resolve(services: Services, cache: &Mutex<Cache>, ask: &Ask) -> Option<ProfileResult> {
    let by_id = |id: u128| {
        let mut c = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        cached(&mut c.by_id, &id, || (services.profile)(id))
    };
    match ask {
        Ask::Id(id) => by_id(*id),
        Ask::Name(name) => {
            if !valid_player_name(name) {
                return None;
            }
            let load = || {
                let lower = name.to_ascii_lowercase();
                let id = (services.id_by_name)(&lower).unwrap_or_else(|| rc_client::offline_uuid(&lower));
                by_id(id)
            };
            // The name entry is read without holding the lock during the network access.
            let hit = {
                let mut c = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                c.by_name.get_mut(name).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(at, v)| {
                    *at = Instant::now();
                    v.clone()
                })
            };
            match hit {
                Some(v) => v,
                None => {
                    let v = load();
                    let mut c = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    cached(&mut c.by_name, name, || v)
                }
            }
        }
    }
}

/// Stored form of a resolved profile: identifier (4 integers), name, properties if any.
fn profile_nbt(p: &ProfileResult) -> Nbt {
    let mut f = vec![("id".to_owned(), crate::nbtio::uuid_nbt(p.id)), ("name".to_owned(), Nbt::String(p.name.clone()))];
    if !p.properties.is_empty() {
        let props = p
            .properties
            .iter()
            .map(|q| {
                let mut e =
                    vec![("name".to_owned(), Nbt::String(q.name.clone())), ("value".to_owned(), Nbt::String(q.value.clone()))];
                if let Some(s) = &q.signature {
                    e.push(("signature".to_owned(), Nbt::String(s.clone())));
                }
                Nbt::Compound(e)
            })
            .collect();
        f.push(("properties".to_owned(), Nbt::List(props)));
    }
    Nbt::Compound(f)
}

fn button(key: &str, args: Vec<text::A>, action: &str, value: String) -> Component {
    let mut c = text::tr(key, args);
    c.style.color = Some(GREEN);
    c.style.click = Some(Arc::new(ClickEvent { action: action.to_owned(), value, nbt: None }));
    text::brackets(c)
}

impl Server {
    /// Dispatch of the G3-F08 commands; `None` if the command is not one of them.
    pub(crate) fn dispatch_s9e(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> Option<R> {
        Some(match seg.path.first().map(String::as_str).unwrap_or("") {
            "compute" => self.cmd_compute(s, p, seg),
            "posteffect" => self.cmd_posteffect(s, p, seg),
            "fetchprofile" => self.cmd_fetchprofile(s, p, seg),
            _ => return None,
        })
    }

    // ---- compute ----------------------------------------------------------------------------

    /// `compute <default|block <pos>|entity <target>> integer <provider>` or `… float <provider>
    /// [scale]`: unchecked value; an arithmetic error (or a float that is not finite) fails with
    /// its text. A float is scaled then floored; "exact" only if the floored value equals the
    /// unscaled one (the game compares with the original, so a scale often reads "rounded").
    fn cmd_compute(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let ctx_kind = seg.path.get(1).map_or("default", String::as_str).to_owned();
        let float = seg.path.iter().any(|x| x == "float");
        let (name, value) = self.compute_eval(s, p, seg, &ctx_kind, float)?;
        let invalid = |v: String| {
            Fail(match &name {
                Some(id) => tr!("command.compute.result.named.invalid", id.clone(), v),
                None => tr!("command.compute.result.unnamed.invalid", v),
            })
        };
        let exact = |result: i32| match &name {
            Some(id) => tr!("command.compute.result.named.exact", id.clone(), result),
            None => tr!("command.compute.result.unnamed.exact", result),
        };
        if !float {
            let result = value.map_err(invalid)? as i32;
            self.ok(s, exact(result));
            return Ok(result);
        }
        let original = value.map_err(invalid)? as f32;
        if !original.is_finite() {
            return Err(invalid(jf(original)));
        }
        let scale: f32 = p.arg(seg, "scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let result = f64::from(original * scale).floor() as i32;
        let msg = if result as f32 == original {
            exact(result)
        } else {
            match &name {
                Some(id) => tr!("command.compute.result.named.rounded", id.clone(), jf(original), result),
                None => tr!("command.compute.result.unnamed.rounded", jf(original), result),
            }
        };
        self.ok(s, msg);
        Ok(result)
    }

    // ---- posteffect -------------------------------------------------------------------------

    /// Screen effects imposed on the player (player file `post_effects`, in order).
    pub(crate) fn post_effects(&self) -> Vec<String> {
        crate::nbtio::list(self.pdata.root.get("post_effects")).iter().filter_map(Nbt::as_str).map(str::to_owned).collect()
    }

    fn set_post_effects(&mut self, list: Vec<String>) {
        crate::nbtio::set(&mut self.pdata.root, "post_effects", Nbt::List(list.into_iter().map(Nbt::String).collect()));
        self.pdata.post_effects_dirty = true;
    }

    /// New player (respawn, return from the End): the game does not carry the list over, the
    /// effects disappear and the empty list goes out at the next tick.
    pub(crate) fn post_effects_reset(&mut self) {
        crate::nbtio::remove(&mut self.pdata.root, "post_effects");
        self.pdata.post_effects_dirty = true;
    }

    /// Player tick: the list goes out if it changed (and once after joining, even empty).
    pub(crate) fn post_effects_tick(&mut self) {
        if std::mem::take(&mut self.pdata.post_effects_dirty) {
            let list = self.post_effects();
            let _ = self.send(Event::Misc(rc_client::misc::Misc::PostEffects(list)));
        }
    }

    fn cmd_posteffect(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let sub = seg.path.get(1).map_or("", String::as_str);
        if sub == "list" {
            let e = self.arg_entity(s, &p.arg(seg, "target").unwrap_or_default(), true)?;
            let list = self.post_effects();
            let msg = if list.is_empty() {
                tr!("commands.posteffect.list.empty", self.ent_display(e))
            } else {
                tr!("commands.posteffect.list.success", self.ent_display(e), list.len(), list.join(", "))
            };
            self.ok(s, msg);
            return Ok(list.len() as i32);
        }
        let targets = self.arg_players(s, &p.arg(seg, "targets").unwrap_or_default())?;
        let id = p.arg(seg, "posteffect").map(|x| args::id(&x)).unwrap_or_default();
        let mut t: Track<E> = Track::default();
        for e in targets {
            let mut list = self.post_effects();
            let changed = match sub {
                "add" if !list.contains(&id) => {
                    list.push(id.clone());
                    true
                }
                "remove" if list.contains(&id) => {
                    list.retain(|x| *x != id);
                    true
                }
                "clear" if !list.is_empty() => {
                    list.clear();
                    true
                }
                _ => false,
            };
            if changed {
                self.set_post_effects(list);
            }
            t.track(e, i32::from(changed));
        }
        let key = |k: &str| format!("commands.posteffect.{sub}.{k}");
        let (fail, single, multiple) = (key("failed"), key("success.single"), key("success.multiple"));
        if sub == "clear" {
            return self.track_feedback(
                s,
                &t,
                false,
                Some(Fail(tr!(&fail))),
                |sv, e| tr!(&single, sv.ent_display(*e)),
                |n| tr!(&multiple, n),
            );
        }
        self.track_feedback(
            s,
            &t,
            false,
            Some(Fail(tr!(&fail))),
            |sv, e| tr!(&single, lit(id.clone()), sv.ent_display(*e)),
            |n| tr!(&multiple, lit(id.clone()), n),
        )
    }

    // ---- fetchprofile -----------------------------------------------------------------------

    /// `fetchprofile name <name>` / `id <uuid>`: lookup on a worker thread, answer at a
    /// following tick (success 1 at once); `entity <entity>`: profile of a player or a
    /// mannequin, otherwise "no profile".
    fn cmd_fetchprofile(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let ask = match seg.path.get(1).map_or("", String::as_str) {
            "name" => Ask::Name(p.arg(seg, "name").unwrap_or_default()),
            "id" => {
                let t = p.arg(seg, "id").unwrap_or_default();
                Ask::Id(snbt::parse_uuid(&t).ok_or_else(|| Fail(tr!("argument.uuid.invalid")))?)
            }
            _ => {
                let e = self.arg_entity(s, &p.arg(seg, "entity").unwrap_or_default(), false)?;
                let profile = match e {
                    E::Player => {
                        let props = self.profile_properties(self.uuid);
                        let r = ProfileResult {
                            id: self.uuid,
                            name: self.name.clone(),
                            properties: props,
                            forced_name_change: false,
                            banned_skin: false,
                        };
                        profile_nbt(&r)
                    }
                    E::Other(_) if self.ent_type(e) == "minecraft:mannequin" => {
                        // Stored profile as the game re-encodes it: a bare name becomes `{name}`,
                        // an unreadable or absent one the empty profile.
                        match self.ent_nbt(e).get("profile") {
                            Some(Nbt::String(n)) if valid_player_name(n) => {
                                Nbt::Compound(vec![("name".to_owned(), Nbt::String(n.clone()))])
                            }
                            Some(c @ Nbt::Compound(_)) => {
                                rc_text::canonical_profile(c).unwrap_or(Nbt::Compound(Vec::new()))
                            }
                            _ => Nbt::Compound(Vec::new()),
                        }
                    }
                    _ => return Err(Fail(tr!("commands.fetchprofile.no_profile", self.ent_display(e)))),
                };
                let who = self.ent_display(e);
                self.report_profile(s, "commands.fetchprofile.entity.success", who, profile);
                return Ok(1);
            }
        };
        let (tx, rx) = channel();
        let (services, cache) = (self.cmd.s9.profiles.services, self.cmd.s9.profiles.cache.clone());
        let job = ask.clone();
        let spawned = std::thread::Builder::new()
            .name("rc-fetchprofile".into())
            .spawn(move || {
                let _ = tx.send(resolve(services, &cache, &job));
            })
            .is_ok();
        if spawned {
            self.cmd.s9.profiles.pending.push(Pending { src: s.clone(), ask, rx });
        }
        Ok(1)
    }

    /// Answers of the finished lookups (server thread, start of the commands phase).
    pub(crate) fn fetchprofile_poll(&mut self) {
        if self.cmd.s9.profiles.pending.is_empty() {
            return;
        }
        let mut done = Vec::new();
        self.cmd.s9.profiles.pending.retain(|q| match q.rx.try_recv() {
            Ok(r) => {
                done.push((q.src.clone(), q.ask.clone(), r));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                done.push((q.src.clone(), q.ask.clone(), None));
                false
            }
        });
        for (src, ask, r) in done {
            let (arg, kind) = match &ask {
                Ask::Name(n) => (lit(n.clone()), "name"),
                Ask::Id(id) => (lit(crate::nbtio::uuid_text(*id)), "id"),
            };
            match r {
                Some(profile) => {
                    self.report_profile(&src, &format!("commands.fetchprofile.{kind}.success"), arg, profile_nbt(&profile))
                }
                None => self.fail(&src, tr!(&format!("commands.fetchprofile.{kind}.failure"), arg)),
            }
        }
    }

    /// Message of a resolved profile: its stored form, then four green buttons (copy the
    /// profile, give a head, summon a mannequin, copy the head component).
    fn report_profile(&mut self, s: &Src, key: &str, arg: Component, profile: Nbt) {
        let encoded = snbt::to_snbt(&profile);
        let head_nbt = Nbt::Compound(vec![("player".to_owned(), profile.clone())]);
        let encoded_head = snbt::to_snbt(&head_nbt);
        let mut head = lit("");
        head.content = Content::Object { info: ObjectInfo::Player { profile: Box::new(profile), hat: true }, fallback: None };
        head.style.color = Some(0xFF_FFFF);
        let buttons = vec![
            button("commands.fetchprofile.copy_component", Vec::new(), "copy_to_clipboard", encoded.clone()),
            button(
                "commands.fetchprofile.give_item",
                Vec::new(),
                "run_command",
                format!("give @s minecraft:player_head[profile={encoded}]"),
            ),
            button(
                "commands.fetchprofile.summon_mannequin",
                Vec::new(),
                "run_command",
                format!("summon minecraft:mannequin ~ ~ ~ {{profile:{encoded}}}"),
            ),
            button("commands.fetchprofile.copy_text", vec![text::A::C(head)], "copy_to_clipboard", encoded_head),
        ];
        self.ok(s, tr!(key, arg, text::list_sep(buttons, lit(" "))));
    }
}
