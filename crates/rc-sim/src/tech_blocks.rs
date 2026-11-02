//! Game master blocks of the integrated server (G6-F14): structure block (settings, saving,
//! loading and size detection of templates, redstone trigger, author), jigsaw block (settings),
//! test block (mode, message, redstone edge, signal of the start mode) and test instance block
//! (settings); the world's templates (cache, `generated/<ns>/structure/<path>.nbt`) and their
//! placement into the live world, shared with `place template`.

use std::sync::Arc;

use hashbrown::HashMap;
use rc_proto::nbt::Nbt;
use rc_proto::varint::{read_varint, read_varlong};
use rc_proto::{BlockPos, Decode, Reader, read_str};
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::template::{Mirror, Rot, StateTransforms, Template, transform, transform_f};

use crate::blocks::{Dir, Kind, P, flag};
use crate::commands::ents::E;
use crate::mobs::jrand::JRand;
use crate::nbtio::{int, set};
use crate::server::Server;
use crate::tr;

/// Largest offset and size per axis of a structure block.
const MAX_AXIS: i32 = 48;
/// Half side of the area searched for corner blocks (size detection).
const SCAN_RANGE: i32 = 80;

/// A template known to the server, with its author (kept in memory only: the file does not carry
/// it, a loaded template reads "?").
#[derive(Debug, Clone)]
pub(crate) struct Tpl {
    pub t: Arc<Template>,
    pub author: String,
}

/// Templates of the server, like the game's manager: each identifier is looked up once (world
/// folder, then the game's files) and its absence remembered; a capture by a structure block
/// replaces the entry, saved to disk or not. Emptied by a data reload.
#[derive(Debug, Default)]
pub struct Templates {
    map: HashMap<String, Option<Tpl>>,
}

impl Templates {
    pub(crate) fn clear(&mut self) {
        self.map.clear();
    }
}

/// Identifier of a template (`ns:path`, default namespace), or `None` if it is not a valid
/// identifier (allowed characters of the game).
pub(crate) fn template_id(text: &str) -> Option<String> {
    let (ns, path) = text.split_once(':').unwrap_or(("minecraft", text));
    let ns = if ns.is_empty() { "minecraft" } else { ns };
    let ns_ok = ns.bytes().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.'));
    let path_ok = path.bytes().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/'));
    (ns_ok && path_ok).then(|| format!("{ns}:{path}"))
}

/// File of a template in the world (`generated/<ns>/structure/<path>.nbt`), or `None` if the
/// path is not portable (empty part, `.` or `..`, reserved name).
pub(crate) fn template_file(world: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    let (ns, path) = id.split_once(':')?;
    let mut f = world.join("generated").join(ns).join("structure");
    let parts: Vec<&str> = path.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        if part.is_empty() || *part == "." || *part == ".." || reserved {
            return None;
        }
        if i + 1 == parts.len() {
            f.push(format!("{part}.nbt"));
        } else {
            f.push(part);
        }
    }
    Some(f)
}

/// Randomness of a structure block or of `place template`: the given seed, or the clock for 0.
fn create_random(seed: i64) -> JRand {
    if seed == 0 { JRand::from_time(0x7e57) } else { JRand::new(seed) }
}

/// Settings of a placement into the live world (`place template`, structure block).
#[derive(Debug, Clone, Copy)]
pub(crate) struct LivePlace {
    pub mirror: Mirror,
    pub rot: Rot,
    pub ignore_entities: bool,
    /// Known shape: neither the edge shape pass nor the neighbour updates after placement.
    pub known_shape: bool,
    /// Below 1: each block is kept with this probability, drawn on the settings' own random.
    pub integrity: f32,
    pub seed: i64,
}

/// Mode of a structure block (state property and data value).
const MODES: [(&str, &str); 4] = [("save", "SAVE"), ("load", "LOAD"), ("corner", "CORNER"), ("data", "DATA")];
const MIRRORS: [(Mirror, &str); 3] =
    [(Mirror::None, "NONE"), (Mirror::LeftRight, "LEFT_RIGHT"), (Mirror::FrontBack, "FRONT_BACK")];
const TEST_MODES: [&str; 4] = ["start", "log", "fail", "accept"];

/// Data of a structure block, read like the game loads it (offsets and sizes clamped,
/// unknown mode = data).
#[derive(Debug, Clone, PartialEq)]
struct Sb {
    name: Option<String>,
    author: String,
    metadata: String,
    pos: [i32; 3],
    size: [i32; 3],
    rot: Rot,
    mirror: Mirror,
    mode: usize,
    ignore_entities: bool,
    strict: bool,
    powered: bool,
    show_air: bool,
    show_box: bool,
    integrity: f32,
    seed: i64,
}

impl Sb {
    fn read(be: &Nbt) -> Sb {
        let s = |k: &str| be.get(k).and_then(Nbt::as_str).unwrap_or("").to_owned();
        let i = |k: &str, d: i32| int(be, k).map_or(d, |v| v as i32);
        let b = |k: &str, d: bool| int(be, k).map_or(d, |v| v != 0);
        let name = s("name");
        Sb {
            name: if name.is_empty() { None } else { template_id(&name) },
            author: s("author"),
            metadata: s("metadata"),
            pos: [
                i("posX", 0).clamp(-MAX_AXIS, MAX_AXIS),
                i("posY", 1).clamp(-MAX_AXIS, MAX_AXIS),
                i("posZ", 0).clamp(-MAX_AXIS, MAX_AXIS),
            ],
            size: [
                i("sizeX", 0).clamp(0, MAX_AXIS),
                i("sizeY", 0).clamp(0, MAX_AXIS),
                i("sizeZ", 0).clamp(0, MAX_AXIS),
            ],
            rot: Rot::ALL.into_iter().find(|r| r.name() == s("rotation")).unwrap_or(Rot::None),
            mirror: MIRRORS.iter().find(|m| m.1 == s("mirror")).map_or(Mirror::None, |m| m.0),
            mode: MODES.iter().position(|m| m.1 == s("mode")).unwrap_or(3),
            ignore_entities: b("ignoreEntities", true),
            strict: b("strict", false),
            powered: b("powered", false),
            show_air: b("showair", false),
            show_box: b("showboundingbox", true),
            integrity: crate::nbtio::float(be, "integrity").unwrap_or(1.0),
            seed: int(be, "seed").unwrap_or(0),
        }
    }

    fn write(&self, be: &mut Nbt) {
        let mirror = MIRRORS.iter().find(|m| m.0 == self.mirror).map_or("NONE", |m| m.1);
        for (k, v) in [
            ("name", Nbt::String(self.name.clone().unwrap_or_default())),
            ("author", Nbt::String(self.author.clone())),
            ("metadata", Nbt::String(self.metadata.clone())),
            ("posX", Nbt::Int(self.pos[0])),
            ("posY", Nbt::Int(self.pos[1])),
            ("posZ", Nbt::Int(self.pos[2])),
            ("sizeX", Nbt::Int(self.size[0])),
            ("sizeY", Nbt::Int(self.size[1])),
            ("sizeZ", Nbt::Int(self.size[2])),
            ("rotation", Nbt::String(self.rot.name().to_owned())),
            ("mirror", Nbt::String(mirror.to_owned())),
            ("mode", Nbt::String(MODES[self.mode].1.to_owned())),
            ("ignoreEntities", Nbt::Byte(i8::from(self.ignore_entities))),
            ("strict", Nbt::Byte(i8::from(self.strict))),
            ("powered", Nbt::Byte(i8::from(self.powered))),
            ("showair", Nbt::Byte(i8::from(self.show_air))),
            ("showboundingbox", Nbt::Byte(i8::from(self.show_box))),
            ("integrity", Nbt::Float(self.integrity)),
            ("seed", Nbt::Long(self.seed)),
        ] {
            set(be, k, v);
        }
    }

    fn name_text(&self) -> String {
        self.name.clone().unwrap_or_default()
    }
}

fn be_id(be: Option<&Nbt>) -> Option<&str> {
    be.and_then(|b| b.get("id")).and_then(Nbt::as_str)
}

impl Server {
    // ---------- Packets ----------

    /// Packets of the game master blocks; `true` if consumed.
    pub(crate) fn tech_packet(&mut self, id: i32, r: &mut Reader<'_>) -> bool {
        let ids = self.peer.ids();
        if id == ids.set_structure_block {
            let _ = self.on_set_structure_block(r);
        } else if id == ids.set_jigsaw_block {
            let _ = self.on_set_jigsaw_block(r);
        } else if id == ids.jigsaw_generate {
            let _ = self.on_jigsaw_generate(r);
        } else if id == ids.set_test_block {
            let _ = self.on_set_test_block(r);
        } else if id == ids.test_instance_block_action {
            let _ = self.on_test_instance_action(r);
        } else {
            return false;
        }
        true
    }

    fn on_set_structure_block(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let bp = BlockPos::decode(r)?;
        let update = read_varint(r)?;
        let mode = read_varint(r)?;
        let name = read_str(r, 32767)?.to_owned();
        let mut off = [0i32; 3];
        for v in &mut off {
            *v = i32::from(r.u8()? as i8).clamp(-MAX_AXIS, MAX_AXIS);
        }
        let mut size = [0i32; 3];
        for v in &mut size {
            *v = i32::from(r.u8()? as i8).clamp(0, MAX_AXIS);
        }
        let mirror = read_varint(r)?;
        let rot = read_varint(r)?;
        let data = read_str(r, 128)?.to_owned();
        let integrity = f32::decode(r)?.clamp(0.0, 1.0);
        let seed = read_varlong(r)?;
        let flags = r.u8()?;
        // Out-of-range enumerations: the game fails to decode the packet.
        if !(0..4).contains(&update) || !(0..4).contains(&mode) || !(0..3).contains(&mirror) {
            return Ok(());
        }
        let p = [bp.x, bp.y, bp.z];
        if !self.can_use_game_master_blocks() || be_id(self.be_get(p)) != Some("minecraft:structure_block") {
            return Ok(());
        }
        let Some(mut sb) = self.be_get(p).map(Sb::read) else { return Ok(()) };
        self.structure_set_mode(p, &mut sb, mode as usize);
        sb.name = if name.is_empty() { None } else { template_id(&name) };
        sb.pos = off;
        sb.size = size;
        sb.mirror = MIRRORS[mirror as usize].0;
        sb.rot = Rot::from_index(rot);
        sb.metadata = data;
        sb.ignore_entities = flags & 1 != 0;
        sb.strict = flags & 8 != 0;
        sb.show_air = flags & 2 != 0;
        sb.show_box = flags & 4 != 0;
        sb.integrity = integrity;
        sb.seed = seed;
        self.structure_write(p, &sb);
        if sb.name.is_some() {
            let shown = sb.name_text();
            match update {
                1 => {
                    let ok = sb.mode == 0 && self.structure_save(p, &sb, true);
                    let k = if ok { "structure_block.save_success" } else { "structure_block.save_failure" };
                    self.system(tr!(k, shown));
                }
                2 => {
                    let loadable = sb.mode == 1 && sb.name.as_deref().is_some_and(|n| self.template_get(n).is_some());
                    if !loadable {
                        self.system(tr!("structure_block.load_not_found", shown));
                    } else if self.structure_place_if_same_size(p, &mut sb) {
                        self.system(tr!("structure_block.load_success", shown));
                    } else {
                        self.system(tr!("structure_block.load_prepare", shown));
                    }
                }
                3 => {
                    if self.structure_detect_size(p, &mut sb) {
                        self.system(tr!("structure_block.size_success", shown));
                    } else {
                        self.system(tr!("structure_block.size_failure"));
                    }
                }
                _ => {}
            }
        } else {
            self.system(tr!("structure_block.invalid_structure_name", name));
        }
        self.structure_write(p, &sb);
        self.send_block_entity(p);
        Ok(())
    }

    fn on_set_jigsaw_block(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let bp = BlockPos::decode(r)?;
        let mut ids = [String::new(), String::new(), String::new()];
        for v in &mut ids {
            // Identifier of the protocol: an invalid one fails the decoding.
            let Some(id) = template_id(read_str(r, 32767)?) else { return Ok(()) };
            *v = id;
        }
        let final_state = read_str(r, 32767)?.to_owned();
        let joint = if read_str(r, 32767)? == "rollable" { "rollable" } else { "aligned" };
        let selection = read_varint(r)?;
        let placement = read_varint(r)?;
        let p = [bp.x, bp.y, bp.z];
        if !self.can_use_game_master_blocks() || be_id(self.be_get(p)) != Some("minecraft:jigsaw") {
            return Ok(());
        }
        let [name, target, pool] = ids;
        if let Some(be) = self.be_get_mut(p) {
            for (k, v) in [
                ("name", Nbt::String(name)),
                ("target", Nbt::String(target)),
                ("pool", Nbt::String(pool)),
                ("final_state", Nbt::String(final_state)),
                ("joint", Nbt::String(joint.to_owned())),
                ("placement_priority", Nbt::Int(placement)),
                ("selection_priority", Nbt::Int(selection)),
            ] {
                set(be, k, v);
            }
        }
        self.send_block_entity(p);
        Ok(())
    }

    /// Generation from a jigsaw block: assembly of its pool in front of it (the cell its front
    /// faces), joined by its target name, the jigsaw blocks kept on request.
    fn on_jigsaw_generate(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let bp = BlockPos::decode(r)?;
        let levels = read_varint(r)?;
        let keep = bool::decode(r)?;
        let p = [bp.x, bp.y, bp.z];
        if !self.can_use_game_master_blocks() || be_id(self.be_get(p)) != Some("minecraft:jigsaw") {
            return Ok(());
        }
        let s = self.get(p);
        let front = self.v().get(s, "orientation").and_then(|o| o.split('_').next()).and_then(Dir::parse);
        let Some(front) = front else { return Ok(()) };
        let be = self.be_get(p).cloned().unwrap_or(Nbt::Compound(Vec::new()));
        let text = |k: &str| be.get(k).and_then(Nbt::as_str).unwrap_or("minecraft:empty").to_owned();
        let at = crate::blocks::rel(p, front);
        let _ = self.generate_jigsaw(&text("pool"), &text("target"), levels, at, keep);
        Ok(())
    }

    fn on_set_test_block(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let bp = BlockPos::decode(r)?;
        let mode = read_varint(r)?;
        let message = read_str(r, 32767)?.to_owned();
        let p = [bp.x, bp.y, bp.z];
        if !self.can_use_game_master_blocks() || be_id(self.be_get(p)) != Some("minecraft:test_block") {
            return Ok(());
        }
        // Unknown identifier: the first mode (start), like the game's table.
        let mode = TEST_MODES.get(mode as usize).copied().unwrap_or("start");
        self.test_block_set_mode(p, mode);
        if let Some(be) = self.be_get_mut(p) {
            set(be, "message", Nbt::String(message));
        }
        self.send_block_entity(p);
        Ok(())
    }

    /// Test instance block: the initial and size queries answer with the test's description
    /// (and its structure size); the other actions take the screen's settings (test, size,
    /// rotation, entities, status), then reset, save, export or run the test.
    fn on_test_instance_action(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let bp = BlockPos::decode(r)?;
        // Unknown action: the first one (initial query), like the game's table.
        let action = Some(read_varint(r)?).filter(|a| (0..=6).contains(a)).unwrap_or(0);
        let test = if bool::decode(r)? {
            let Some(id) = template_id(read_str(r, 32767)?) else { return Ok(()) };
            Some(id)
        } else {
            None
        };
        let size = [read_varint(r)?, read_varint(r)?, read_varint(r)?];
        let rot = Rot::from_index(read_varint(r)?);
        let ignore_entities = bool::decode(r)?;
        let status = read_varint(r)?;
        // An error message (text component) is never sent by the game's screen.
        if bool::decode(r)? {
            return Ok(());
        }
        let p = [bp.x, bp.y, bp.z];
        if !self.can_use_game_master_blocks() || be_id(self.be_get(p)) != Some("minecraft:test_instance_block") {
            return Ok(());
        }
        if matches!(action, 0 | 1) {
            let status = match test.as_deref().and_then(|t| self.cmd.tests.test(t)).cloned() {
                Some(t) => self.test_description(&t),
                None => crate::commands::text::colored(tr!("test_instance.description.no_test"), crate::commands::text::RED),
            };
            let size = if action == 1 { test.as_deref().and_then(|t| self.test_structure_size(t)) } else { None };
            let _ = self.send(rc_client::Event::TestInstanceStatus { status: Some(status), size });
            return Ok(());
        }
        let status = u8::try_from(status).ok().filter(|s| *s <= 2).unwrap_or(0);
        let data = crate::gametest::Data { test, size, rot, ignore_entities, status, error: None };
        self.ti_set(p, &data);
        let mut to_player = |sv: &mut Server, c: rc_text::Component| sv.system(c);
        match action {
            3 => self.ti_reset(p, &mut to_player),
            4 => {
                self.ti_save(p, &mut to_player);
            }
            // Saved to the world's folder; the export of test structures only exists in the
            // game's development environment.
            5 => {
                if self.ti_save(p, &mut to_player).is_some() {
                    let msg = rc_text::Component::text("Test structure exporting is disabled");
                    self.system(crate::commands::text::colored(msg, crate::commands::text::RED));
                }
            }
            6 => self.ti_run(p, &mut to_player),
            _ => {}
        }
        self.send_block_entity(p);
        Ok(())
    }

    /// Description of a test (its screen): type, function, structure, environment, dimension,
    /// one line each, values in blue.
    pub(crate) fn test_description(&self, t: &crate::gametest::TestDef) -> rc_text::Component {
        use crate::commands::text::{colored, lit};
        const BLUE: u32 = 0x5555FF;
        let row = |key: &str, v: rc_text::Component| {
            let mut c = tr!(key, colored(v, BLUE));
            c.extra.push(lit("\n"));
            c
        };
        let ty = match t.kind {
            crate::gametest::Kind::Function(_) => "test_instance.type.function",
            crate::gametest::Kind::TestBlocks => "test_instance.type.block_based",
        };
        let mut info = row("test_instance.description.structure", lit(t.structure.clone()));
        info.extra.push(row("test_instance.description.batch", lit(t.env.name())));
        info.extra.push(row("test_instance.description.dimension", lit(t.dimension.clone())));
        let mut c = row("test_instance.description.type", tr!(ty));
        if let crate::gametest::Kind::Function(f) = &t.kind {
            c.extra.push(row("test_instance.description.function", lit(f.clone())));
        }
        c.extra.push(info);
        c
    }

    // ---------- Block life ----------

    /// Default data of a game master block entity just created (values of the game's empty
    /// block entity; the mode comes from the block state).
    pub(crate) fn tech_created(&mut self, p: P, s: u32) {
        // A new test block entity starts untriggered.
        if self.v().kind(s) == Kind::Test {
            self.test_block_mark(p, false);
        }
        self.tech_defaults(p, s, true);
    }

    /// Data loaded into a game master block entity (`setblock` or `data` with NBT): the missing
    /// fields take the game's loading defaults (jigsaw joint from its front, test block in start
    /// mode, structure block offsets and sizes clamped), as its next save writes them.
    pub(crate) fn tech_loaded(&mut self, p: P) {
        let s = self.get(p);
        if self.v().kind(s) == Kind::Structure {
            if let Some(be) = self.be_get_mut(p) {
                let sb = Sb::read(be);
                sb.write(be);
            }
            return;
        }
        self.tech_defaults(p, s, false);
    }

    fn tech_defaults(&mut self, p: P, s: u32, fresh: bool) {
        let v = self.v();
        let fields: Vec<(&str, Nbt)> = match v.kind(s) {
            Kind::Structure => {
                let mode = v.get(s, "mode").and_then(|m| MODES.iter().position(|x| x.0 == m)).unwrap_or(3);
                let sb = Sb {
                    name: None,
                    author: String::new(),
                    metadata: String::new(),
                    pos: [0, 1, 0],
                    size: [0; 3],
                    rot: Rot::None,
                    mirror: Mirror::None,
                    mode,
                    ignore_entities: true,
                    strict: false,
                    powered: false,
                    show_air: false,
                    show_box: true,
                    integrity: 1.0,
                    seed: 0,
                };
                if let Some(be) = self.be_get_mut(p)
                    && be.get("mode").is_none()
                {
                    sb.write(be);
                }
                return;
            }
            Kind::Test => vec![
                ("mode", Nbt::String(if fresh { v.get(s, "mode").unwrap_or("start") } else { "start" }.to_owned())),
                ("message", Nbt::String(String::new())),
                ("powered", Nbt::Byte(0)),
            ],
            Kind::Jigsaw => {
                // Joint: rollable for a new block entity; data loaded without one: aligned for a
                // horizontal front, rollable otherwise.
                let front = v.get(s, "orientation").and_then(|o| o.split('_').next()).unwrap_or("north");
                let joint = if fresh || matches!(front, "up" | "down") { "rollable" } else { "aligned" };
                vec![
                    ("name", Nbt::String("minecraft:empty".into())),
                    ("target", Nbt::String("minecraft:empty".into())),
                    ("pool", Nbt::String("minecraft:empty".into())),
                    ("final_state", Nbt::String("minecraft:air".into())),
                    ("joint", Nbt::String(joint.into())),
                    ("placement_priority", Nbt::Int(0)),
                    ("selection_priority", Nbt::Int(0)),
                ]
            }
            // No test, empty size, no rotation, entities kept, cleared.
            Kind::TestInstance => vec![(
                "data",
                Nbt::Compound(vec![
                    ("size".to_owned(), Nbt::IntArray(vec![0, 0, 0])),
                    ("rotation".to_owned(), Nbt::String("none".to_owned())),
                    ("ignore_entities".to_owned(), Nbt::Byte(0)),
                    ("status".to_owned(), Nbt::String("cleared".to_owned())),
                ]),
            )],
            _ => return,
        };
        if let Some(be) = self.be_get_mut(p) {
            for (k, val) in fields {
                if be.get(k).is_none() {
                    set(be, k, val);
                }
            }
        }
    }

    /// Placed by the player: the structure block records its author (plain name).
    pub(crate) fn tech_placed_by(&mut self, p: P, s: u32) {
        if self.v().kind(s) == Kind::Structure && be_id(self.be_get(p)) == Some("minecraft:structure_block") {
            let name = self.name.clone();
            if let Some(be) = self.be_get_mut(p) {
                set(be, "author", Nbt::String(name));
            }
        }
    }

    /// Used by an operator in creative: the client opens the block's screen (nothing to send).
    pub(crate) fn tech_block_use(&self, p: P) -> bool {
        let s = self.get(p);
        let id = match self.v().kind(s) {
            Kind::Structure => "minecraft:structure_block",
            Kind::Jigsaw => "minecraft:jigsaw",
            Kind::Test => "minecraft:test_block",
            Kind::TestInstance => "minecraft:test_instance_block",
            _ => return false,
        };
        be_id(self.be_get(p)) == Some(id) && self.can_use_game_master_blocks()
    }

    /// Neighbour changed: the structure and test blocks follow the redstone signal (edges);
    /// `false` if the block is neither.
    pub(crate) fn tech_neighbor_changed(&mut self, s: u32, p: P) -> bool {
        match self.v().kind(s) {
            Kind::Structure => {
                if be_id(self.be_get(p)) != Some("minecraft:structure_block") {
                    return true;
                }
                let Some(mut sb) = self.be_get(p).map(Sb::read) else { return true };
                let signal = self.has_neighbor_signal(p);
                if signal && !sb.powered {
                    sb.powered = true;
                    self.structure_write(p, &sb);
                    match sb.mode {
                        // Save: captured into memory, not written to disk.
                        0 => {
                            self.structure_save(p, &sb, false);
                        }
                        1 => {
                            if let Some(t) = sb.name.clone().and_then(|n| self.template_get(&n)) {
                                self.structure_place(p, &mut sb, &t);
                                self.structure_write(p, &sb);
                            }
                        }
                        // Corner: the template is forgotten by the server.
                        2 => {
                            if let Some(n) = &sb.name {
                                self.cmd.templates.map.remove(n);
                            }
                        }
                        _ => {}
                    }
                } else if !signal && sb.powered {
                    sb.powered = false;
                    self.structure_write(p, &sb);
                }
                true
            }
            Kind::Test => {
                let Some(be) = self.be_get(p) else { return true };
                if be_id(Some(be)) != Some("minecraft:test_block") {
                    return true;
                }
                let mode = be.get("mode").and_then(Nbt::as_str).unwrap_or("fail").to_owned();
                if mode == "start" {
                    return true;
                }
                let powered = int(be, "powered").is_some_and(|v| v != 0);
                let signal = self.has_neighbor_signal(p);
                if signal != powered {
                    if let Some(be) = self.be_get_mut(p) {
                        set(be, "powered", Nbt::Byte(i8::from(signal)));
                    }
                    if signal {
                        self.test_block_trigger(p);
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Trigger of a test block: the start mode powers itself and warns its neighbours (the game
    /// only asks whether a tick is due, so nothing stops its signal before the test ends); the
    /// others are noted as triggered for the test runner (the log mode also writes its message
    /// to the server log, which the integrated server does not keep).
    pub(crate) fn test_block_trigger(&mut self, p: P) {
        let s = self.get(p);
        let start = self.be_get(p).and_then(|b| b.get("mode")).and_then(Nbt::as_str) == Some("start");
        if start {
            if let Some(be) = self.be_get_mut(p) {
                set(be, "powered", Nbt::Byte(1));
            }
            let b = self.v().block(s);
            self.update_neighbors_at(p, b);
        } else {
            self.test_block_mark(p, true);
        }
    }

    /// Scheduled tick of a test block: reset (no longer triggered, a start block stops emitting).
    pub(crate) fn test_block_tick(&mut self, _s: u32, p: P) {
        self.test_block_reset(p);
    }

    /// Signal of a test block: 15 in start mode while powered, otherwise nothing.
    pub(crate) fn test_block_signal(&self, s: u32, p: P) -> i32 {
        if self.v().get(s, "mode") != Some("start") {
            return 0;
        }
        let powered = self.be_get(p).and_then(|b| int(b, "powered")).is_some_and(|v| v != 0);
        if powered { 15 } else { 0 }
    }

    // ---------- Structure block ----------

    fn structure_write(&mut self, p: P, sb: &Sb) {
        if let Some(be) = self.be_get_mut(p) {
            sb.write(be);
        }
    }

    /// Mode change: the block state follows (clients flag).
    fn structure_set_mode(&mut self, p: P, sb: &mut Sb, mode: usize) {
        sb.mode = mode;
        let s = self.get(p);
        if self.v().kind(s) == Kind::Structure {
            let ns = self.v().set(s, "mode", MODES[mode].0);
            if ns != s {
                self.set_block(p, ns, flag::CLIENTS);
            }
        }
    }

    fn test_block_set_mode(&mut self, p: P, mode: &str) {
        if let Some(be) = self.be_get_mut(p) {
            set(be, "mode", Nbt::String(mode.to_owned()));
        }
        let s = self.get(p);
        if self.v().kind(s) == Kind::Test {
            let ns = self.v().set(s, "mode", mode);
            if ns != s {
                self.set_block(p, ns, flag::CLIENTS);
            }
        }
    }

    /// Capture of the area into the named template (`to_disk`: written to the world's folder);
    /// `false` without a name, with an invalid path or if the writing fails.
    fn structure_save(&mut self, p: P, sb: &Sb, to_disk: bool) -> bool {
        let Some(id) = sb.name.clone() else { return false };
        let origin = [p[0] + sb.pos[0], p[1] + sb.pos[1], p[2] + sb.pos[2]];
        self.template_capture(&id, origin, sb.size, !sb.ignore_entities, &sb.author, to_disk, false)
    }

    /// Capture of an area into the named template (structure voids left out, air too if
    /// `skip_air`), kept by the server and written to the world's folder if `to_disk`. An area
    /// without volume leaves the template as it was (created empty if new).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn template_capture(
        &mut self,
        id: &str,
        origin: P,
        size: [i32; 3],
        entities: bool,
        author: &str,
        to_disk: bool,
        skip_air: bool,
    ) -> bool {
        let Some((wd, _)) = self.place_data() else { return false };
        let mut entry = match self.template_get(id) {
            Some(e) => e,
            None => {
                let empty = Template::parse(&self.empty_template_root(), &wd.blocks).ok().map(Arc::new);
                let Some(t) = empty else { return false };
                Tpl { t, author: "?".into() }
            }
        };
        if size.iter().all(|&v| v >= 1) {
            let root = self.capture_area(origin, size, entities, skip_air, &wd.blocks);
            match Template::parse(&root, &wd.blocks) {
                Ok(t) => entry.t = Arc::new(t),
                Err(_) => return false,
            }
        }
        entry.author = author.to_owned();
        let t = entry.t.clone();
        self.cmd.templates.map.insert(id.to_owned(), Some(entry));
        if !to_disk {
            return true;
        }
        self.template_write(id, &t)
    }

    /// Writes a template to `generated/<ns>/structure/<path>.nbt` (compressed NBT).
    pub(crate) fn template_write(&self, id: &str, t: &Template) -> bool {
        let Some(path) = template_file(&self.world, id) else { return false };
        let Some(parent) = path.parent() else { return false };
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
        let root = self.template_to_nbt(t);
        std::fs::write(&path, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).is_ok()
    }

    /// Loading by a structure block of the same size: placed; otherwise only the size and the
    /// author are taken from the template (`false`).
    fn structure_place_if_same_size(&mut self, p: P, sb: &mut Sb) -> bool {
        if sb.mode != 1 {
            return false;
        }
        let Some(t) = sb.name.clone().and_then(|n| self.template_get(&n)) else { return false };
        if t.t.size == sb.size {
            self.structure_place(p, sb, &t);
            true
        } else {
            sb.author = if t.author.is_empty() { String::new() } else { t.author.clone() };
            sb.size = t.t.size;
            false
        }
    }

    /// Placement of the template by the structure block (author and size taken first).
    fn structure_place(&mut self, p: P, sb: &mut Sb, t: &Tpl) {
        sb.author = t.author.clone();
        sb.size = t.t.size;
        let origin = [p[0] + sb.pos[0], p[1] + sb.pos[1], p[2] + sb.pos[2]];
        let set = LivePlace {
            mirror: sb.mirror,
            rot: sb.rot,
            ignore_entities: sb.ignore_entities,
            known_shape: sb.strict,
            integrity: sb.integrity,
            seed: sb.seed,
        };
        let flags = flag::CLIENTS | if sb.strict { 816 } else { 0 };
        self.place_template_live(&t.t.clone(), origin, &set, flags);
    }

    /// Size detection (save mode): box of the corner blocks of the same name within 80
    /// blocks (the block itself counted when there is only one corner); each side must exceed
    /// one block, the interior becomes the area.
    fn structure_detect_size(&mut self, p: P, sb: &mut Sb) -> bool {
        if sb.mode != 0 {
            return false;
        }
        let corner_state = |s: &Server, st: u32| s.v().kind(st) == Kind::Structure;
        let (min_y, top) = (self.dim.height.min_y, self.top_y());
        let (mut lo, mut hi, mut n) = ([i32::MAX; 3], [i32::MIN; 3], 0usize);
        let (x0, x1, z0, z1) = (p[0] - SCAN_RANGE, p[0] + SCAN_RANGE, p[2] - SCAN_RANGE, p[2] + SCAN_RANGE);
        let mut found: Vec<P> = Vec::new();
        for cx in (x0 >> 4)..=(x1 >> 4) {
            for cz in (z0 >> 4)..=(z1 >> 4) {
                let Some(Some(c)) = self.chunks.get(&(cx, cz)) else { continue };
                for (si, sec) in c.sections.iter().enumerate() {
                    if !sec.blocks.may_contain(|st| corner_state(self, st)) {
                        continue;
                    }
                    let base_y = min_y + si as i32 * 16;
                    for ly in 0..16 {
                        let y = base_y + ly;
                        if y >= top {
                            break;
                        }
                        for lz in 0..16 {
                            let z = cz * 16 + lz;
                            if z < z0 || z > z1 {
                                continue;
                            }
                            for lx in 0..16 {
                                let x = cx * 16 + lx;
                                if x < x0 || x > x1 {
                                    continue;
                                }
                                let st = sec.blocks.get(rc_world::block_index(lx as usize, ly as usize, lz as usize));
                                if corner_state(self, st) {
                                    found.push([x, y, z]);
                                }
                            }
                        }
                    }
                }
            }
        }
        for q in found {
            let Some(be) = self.be_get(q) else { continue };
            if be_id(Some(be)) != Some("minecraft:structure_block") {
                continue;
            }
            let other = Sb::read(be);
            if other.mode == 2 && other.name == sb.name {
                n += 1;
                for i in 0..3 {
                    lo[i] = lo[i].min(q[i]);
                    hi[i] = hi[i].max(q[i]);
                }
            }
        }
        if n == 0 {
            return false;
        }
        if n == 1 {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        if d.iter().all(|&v| v > 1) {
            sb.pos = [lo[0] - p[0] + 1, lo[1] - p[1] + 1, lo[2] - p[2] + 1];
            sb.size = [d[0] - 1, d[1] - 1, d[2] - 1];
            true
        } else {
            false
        }
    }

    // ---------- Templates ----------

    /// Template of the server by identifier: memory, then the world's folder, then the game's
    /// files; the absence is remembered.
    pub(crate) fn template_get(&mut self, id: &str) -> Option<Tpl> {
        if let Some(e) = self.cmd.templates.map.get(id) {
            return e.clone();
        }
        let loaded = self.template_load(id);
        self.cmd.templates.map.insert(id.to_owned(), loaded.clone());
        loaded
    }

    /// The world's `generated` folder (an unreadable file passes on to the next source), then the
    /// chosen packs: the top pack holding the file (`data/<ns>/structure/<path>.nbt`), the game's
    /// base pack last.
    fn template_load(&mut self, id: &str) -> Option<Tpl> {
        let (wd, _) = self.place_data()?;
        let world = template_file(&self.world, id)
            .and_then(|f| crate::nbtio::read_dat(&f).ok())
            .and_then(|root| Template::parse(&root, &wd.blocks).ok())
            .map(Arc::new);
        let t = match world {
            Some(t) => t,
            None => match self.pack_template_file(id)? {
                crate::packs::StackFile::Base => wd.templates.get(id, &wd.blocks)?,
                crate::packs::StackFile::Bytes(b) => {
                    let raw = crate::nbtio::decompress_auto(&b).ok()?;
                    let root = crate::nbtio::parse_root(&raw).ok()?;
                    Arc::new(Template::parse(&root, &wd.blocks).ok()?)
                }
            },
        };
        Some(Tpl { t, author: "?".into() })
    }

    /// Template file in the chosen packs; only the base pack when no other is chosen (no folder
    /// listed).
    fn pack_template_file(&self, id: &str) -> Option<crate::packs::StackFile> {
        let enabled = crate::packs::config_of(self.level.root.get("Data")).0;
        if enabled.len() <= 1 && enabled.first().is_none_or(|e| e == "vanilla") {
            return Some(crate::packs::StackFile::Base);
        }
        // Same path checks as the world's folder: no empty, `.` or `..` part (a pack folder is
        // never left), no reserved name.
        template_file(&self.world, id)?;
        let (ns, path) = id.split_once(':')?;
        let key = format!("data/{ns}/structure/{path}.nbt");
        let repo = crate::packs::with_world(&self.cmd.builtin_packs, &self.world);
        let jar = crate::registries::client_jar(&self.data).and_then(|j| rc_data::jar::index(&j).ok());
        crate::packs::stack_file(&repo, &enabled, &key, |name| jar.as_ref()?.get(name).map(rc_data::jar::Blob::bytes))
    }

    fn empty_template_root(&self) -> Nbt {
        Nbt::Compound(vec![
            ("size".into(), Nbt::List(vec![Nbt::Int(0), Nbt::Int(0), Nbt::Int(0)])),
            ("blocks".into(), Nbt::List(Vec::new())),
            ("palette".into(), Nbt::List(Vec::new())),
            ("entities".into(), Nbt::List(Vec::new())),
        ])
    }

    /// Palette entry of a state (`Name`, `Properties` if any).
    pub(crate) fn state_nbt(&self, s: u32) -> Nbt {
        let name = self.data.block_of(s).map_or("minecraft:air", |b| b.name.as_str()).to_owned();
        let props: Vec<(String, Nbt)> = self
            .v()
            .props_sorted(s)
            .into_iter()
            .filter_map(|(k, vals, i)| vals.get(i).map(|v| (k.to_owned(), Nbt::String(v.clone()))))
            .collect();
        let mut f = vec![("Name".to_owned(), Nbt::String(name))];
        if !props.is_empty() {
            f.push(("Properties".to_owned(), Nbt::Compound(props)));
        }
        Nbt::Compound(f)
    }

    /// The area as the game's template file describes it: every block except the structure
    /// voids (air included, cells beyond the build height read as void air), in placement order
    /// (full blocks, others, block entities; each by y, x, z), palette in order of first use;
    /// the entities (players excepted) if asked.
    fn capture_area(&self, origin: P, size: [i32; 3], entities: bool, skip_air: bool, wb: &Blocks) -> Nbt {
        let far = [origin[0] + size[0] - 1, origin[1] + size[1] - 1, origin[2] + size[2] - 1];
        let lo = [origin[0].min(far[0]), origin[1].min(far[1]), origin[2].min(far[2])];
        let hi = [origin[0].max(far[0]), origin[1].max(far[1]), origin[2].max(far[2])];
        let void = self.tb().block_id("minecraft:structure_void");
        let air = self.tb().block_id("minecraft:air");
        // (y, x, z) order inside each category comes from the loops.
        let (mut full, mut other, mut with) = (Vec::new(), Vec::new(), Vec::new());
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                for z in lo[2]..=hi[2] {
                    let q = [x, y, z];
                    let st = self.get(q);
                    let b = self.v().block(st);
                    if b == void || (skip_air && b == air) {
                        continue;
                    }
                    let rel = [x - lo[0], y - lo[1], z - lo[2]];
                    let be = self.be_get(q).cloned().map(|mut n| {
                        for k in ["x", "y", "z"] {
                            crate::nbtio::remove(&mut n, k);
                        }
                        n
                    });
                    match be {
                        Some(n) => with.push((rel, st, Some(n))),
                        None if wb.has_flag(st, rc_data::flags::FULL_COLLISION) => full.push((rel, st, None)),
                        None => other.push((rel, st, None)),
                    }
                }
            }
        }
        let mut palette: Vec<u32> = Vec::new();
        let mut blocks = Vec::with_capacity(full.len() + other.len() + with.len());
        for (rel, st, nbt) in full.into_iter().chain(other).chain(with) {
            let id = match palette.iter().position(|&x| x == st) {
                Some(i) => i,
                None => {
                    palette.push(st);
                    palette.len() - 1
                }
            };
            let mut f = vec![
                ("pos".to_owned(), Nbt::List(rel.iter().map(|&v| Nbt::Int(v)).collect())),
                ("state".to_owned(), Nbt::Int(id as i32)),
            ];
            if let Some(n) = nbt {
                f.push(("nbt".to_owned(), n));
            }
            blocks.push(Nbt::Compound(f));
        }
        let mut ents = Vec::new();
        if entities {
            let (bmin, bmax) = (
                [lo[0] as f64, lo[1] as f64, lo[2] as f64],
                [hi[0] as f64 + 1.0, hi[1] as f64 + 1.0, hi[2] as f64 + 1.0],
            );
            for e in self.all_ents() {
                if e == E::Player {
                    continue;
                }
                let b = self.e_bbox(e);
                if !(0..3).all(|i| b.min[i] < bmax[i] && b.max[i] > bmin[i]) {
                    continue;
                }
                let at = self.e_pos(e);
                let rel = [at[0] - bmin[0], at[1] - bmin[1], at[2] - bmin[2]];
                let nbt = self.ent_nbt(e);
                let block = match (self.ent_type(e).as_str(), nbt.get("block_pos")) {
                    ("minecraft:painting", Some(Nbt::IntArray(v))) if v.len() == 3 => {
                        [v[0] - lo[0], v[1] - lo[1], v[2] - lo[2]]
                    }
                    _ => [rel[0].floor() as i32, rel[1].floor() as i32, rel[2].floor() as i32],
                };
                ents.push(Nbt::Compound(vec![
                    ("pos".into(), Nbt::List(rel.iter().map(|&v| Nbt::Double(v)).collect())),
                    ("blockPos".into(), Nbt::List(block.iter().map(|&v| Nbt::Int(v)).collect())),
                    ("nbt".into(), nbt),
                ]));
            }
        }
        Nbt::Compound(vec![
            ("blocks".into(), Nbt::List(blocks)),
            ("palette".into(), Nbt::List(palette.iter().map(|&s| self.state_nbt(s)).collect())),
            ("entities".into(), Nbt::List(ents)),
            ("size".into(), Nbt::List(size.iter().map(|&v| Nbt::Int(v)).collect())),
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
        ])
    }

    /// Template written like the game: blocks of the first palette in their order with the
    /// state number of first use; further palettes map the same numbers, index by index.
    pub(crate) fn template_to_nbt(&self, t: &Template) -> Nbt {
        let mut f: Vec<(String, Nbt)> = Vec::new();
        if t.palettes.is_empty() {
            f.push(("blocks".into(), Nbt::List(Vec::new())));
            f.push(("palette".into(), Nbt::List(Vec::new())));
        } else {
            let main = &t.palettes[0];
            let mut ids: Vec<u32> = Vec::new();
            let mut extra: Vec<Vec<Option<u32>>> = vec![Vec::new(); t.palettes.len() - 1];
            let mut blocks = Vec::with_capacity(main.len());
            for (i, b) in main.iter().enumerate() {
                let id = match ids.iter().position(|&x| x == b.state) {
                    Some(k) => k,
                    None => {
                        ids.push(b.state);
                        ids.len() - 1
                    }
                };
                let mut e = vec![
                    ("pos".to_owned(), Nbt::List(b.pos.iter().map(|&v| Nbt::Int(v)).collect())),
                    ("state".to_owned(), Nbt::Int(id as i32)),
                ];
                if let Some(n) = &b.nbt {
                    e.push(("nbt".to_owned(), n.clone()));
                }
                blocks.push(Nbt::Compound(e));
                for (k, pal) in t.palettes[1..].iter().enumerate() {
                    if let Some(o) = pal.get(i) {
                        if extra[k].len() <= id {
                            extra[k].resize(id + 1, None);
                        }
                        extra[k][id] = Some(o.state);
                    }
                }
            }
            f.push(("blocks".into(), Nbt::List(blocks)));
            let list = |states: &mut dyn Iterator<Item = u32>| Nbt::List(states.map(|s| self.state_nbt(s)).collect());
            if t.palettes.len() == 1 {
                f.push(("palette".into(), list(&mut ids.iter().copied())));
            } else {
                let mut all = vec![list(&mut ids.iter().copied())];
                for e in &extra {
                    all.push(list(&mut e.iter().flatten().copied()));
                }
                f.push(("palettes".into(), Nbt::List(all)));
            }
        }
        let ents = t
            .entities
            .iter()
            .map(|e| {
                Nbt::Compound(vec![
                    ("pos".into(), Nbt::List(e.pos.iter().map(|&v| Nbt::Double(v)).collect())),
                    ("blockPos".into(), Nbt::List(e.block_pos.iter().map(|&v| Nbt::Int(v)).collect())),
                    ("nbt".into(), e.nbt.clone()),
                ])
            })
            .collect();
        f.push(("entities".into(), Nbt::List(ents)));
        f.push(("size".into(), Nbt::List(t.size.iter().map(|&v| Nbt::Int(v)).collect())));
        f.push(("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)));
        Nbt::Compound(f)
    }

    /// Placement of a template into the live world, like the game (`place template`, structure
    /// block): palette drawn, integrity on the settings' random, block entities cleared then
    /// loaded (fresh loot seed for the containers), waterlogging and its filling, edge shape
    /// pass, shape and neighbour updates, entities. `false` if nothing can be placed.
    pub(crate) fn place_template_live(&mut self, t: &Template, pos: P, s: &LivePlace, flags: u32) -> bool {
        let Some((wd, _)) = self.place_data() else { return false };
        place_live(self, t, &wd.templates.transforms, &wd.blocks, pos, s, flags)
    }
}

fn place_live(
    srv: &mut Server,
    t: &Template,
    tf: &StateTransforms,
    wb: &Blocks,
    pos: P,
    s: &LivePlace,
    flags: u32,
) -> bool {
    if t.palettes.is_empty() {
        return false;
    }
    // Two generators from the same seed: the settings' (palette, integrity) when the integrity
    // is below 1, and the placement's (loot seeds).
    let mut rng = create_random(s.seed);
    let mut own = (s.integrity < 1.0).then(|| create_random(s.seed));
    let n = t.palettes.len() as i32;
    let pi = match &mut own {
        Some(r) => r.next_int_bound(n),
        None => JRand::new(rc_worldgen::template::pos_seed(pos[0], pos[1], pos[2])).next_int_bound(n),
    } as usize;
    let blocks = &t.palettes[pi];
    if (blocks.is_empty() && (s.ignore_entities || t.entities.is_empty())) || t.size.iter().any(|&v| v < 1) {
        return false;
    }
    let integrity = s.integrity.clamp(0.0, 1.0);
    let barrier = {
        let v = srv.v();
        v.default_state(srv.tb().block_id("minecraft:barrier"))
    };
    let mut placed: Vec<(P, bool)> = Vec::with_capacity(blocks.len());
    let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
    let mut locked: hashbrown::HashSet<P> = hashbrown::HashSet::new();
    let mut to_fill: Vec<P> = Vec::new();
    for b in blocks {
        if let Some(r) = &mut own
            && r.next_float() > integrity
        {
            continue;
        }
        let rp = transform(b.pos, s.mirror, s.rot, [0, 0, 0]);
        let p = [pos[0] + rp[0], pos[1] + rp[1], pos[2] + rp[2]];
        let prev = srv.fluid_at(p);
        let st = tf.apply(b.state, s.mirror, s.rot);
        if b.nbt.is_some() {
            srv.set_block(p, barrier, 820);
        }
        if !srv.set_block(p, st, flags) {
            continue;
        }
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
        placed.push((p, b.nbt.is_some()));
        if let Some(n) = &b.nbt
            && srv.be_get(p).is_some()
        {
            let mut n = n.clone();
            if rc_worldgen::structure_place::is_loot_container(wb.block_name(wb.block(st))) {
                set(&mut n, "LootTableSeed", Nbt::Long(rng.next_long()));
            }
            srv.set_block_data(p, n);
        }
        // The template's own state decides (a source is locked, a waterloggable block takes
        // the water that was there).
        if srv.fluid_of(st).source {
            locked.insert(p);
        } else if srv.v().has(st, "waterlogged") {
            if prev.kind == 1 && prev.source && !srv.v().get_bool(st, "waterlogged") {
                srv.place_water_in(p, st);
            }
            if !prev.source {
                to_fill.push(p);
            }
        }
    }
    // Filling: a waterloggable cell next to a free source (above or beside) takes its water.
    const FILL: [Dir; 5] = [Dir::Up, Dir::North, Dir::East, Dir::South, Dir::West];
    let mut filled = true;
    while filled && !to_fill.is_empty() {
        filled = false;
        let mut i = 0;
        while i < to_fill.len() {
            let p = to_fill[i];
            let mut src = srv.fluid_at(p);
            for d in FILL {
                if src.source {
                    break;
                }
                let q = crate::blocks::rel(p, d);
                let nf = srv.fluid_at(q);
                if nf.source && !locked.contains(&q) {
                    src = nf;
                }
            }
            let cur = srv.get(p);
            if src.source && srv.v().has(cur, "waterlogged") {
                if src.kind == 1 && !srv.v().get_bool(cur, "waterlogged") {
                    srv.place_water_in(p, cur);
                }
                filled = true;
                to_fill.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
    if !placed.is_empty() && !s.known_shape {
        edge_shapes(srv, &placed, lo, hi, flags);
        for &(p, _) in &placed {
            let st = srv.get(p);
            let ns = srv.update_from_neighbour_shapes(st, p);
            if ns != st {
                srv.set_block(p, ns, (flags & !flag::NEIGHBORS) | flag::KNOWN_SHAPE);
            }
            let b = srv.v().block(ns);
            srv.update_neighbors_at(p, b);
        }
    }
    if !s.ignore_entities {
        let src = srv.player_source();
        for e in &t.entities {
            let rp = transform(e.block_pos, s.mirror, s.rot, [0, 0, 0]);
            let bp = [pos[0] + rp[0], pos[1] + rp[1], pos[2] + rp[2]];
            let fp = transform_f(e.pos, s.mirror, s.rot, [0, 0, 0]);
            let wp = [fp[0] + f64::from(pos[0]), fp[1] + f64::from(pos[1]), fp[2] + f64::from(pos[2])];
            let Nbt::Compound(mut f) = e.nbt.clone() else { continue };
            rc_worldgen::template::place_entity_nbt(&mut f, wp, bp, s.mirror, s.rot);
            let Some(ty) = f.iter().find(|(k, _)| k == "id").and_then(|(_, v)| v.as_str()).map(str::to_owned) else {
                continue;
            };
            let _ = srv.summon_entity(&src, &ty, wp, Some(Nbt::Compound(f)));
        }
    }
    true
}

/// Shape pass over the faces of the placed volume (faces between a placed cell and an
/// unplaced one), in the game's order: faces across z, then y, then x; each side's state
/// follows its neighbour's (without neighbour updates).
fn edge_shapes(srv: &mut Server, placed: &[(P, bool)], lo: [i32; 3], hi: [i32; 3], flags: u32) {
    let size = [(hi[0] - lo[0] + 1) as usize, (hi[1] - lo[1] + 1) as usize, (hi[2] - lo[2] + 1) as usize];
    let mut full = vec![false; size[0] * size[1] * size[2]];
    let idx = |x: usize, y: usize, z: usize| (x * size[1] + y) * size[2] + z;
    for &(p, _) in placed {
        full[idx((p[0] - lo[0]) as usize, (p[1] - lo[1]) as usize, (p[2] - lo[2]) as usize)] = true;
    }
    let nf = flags & !flag::NEIGHBORS;
    let face = |srv: &mut Server, d: Dir, c: [usize; 3]| {
        let p = [lo[0] + c[0] as i32, lo[1] + c[1] as i32, lo[2] + c[2] as i32];
        let q = crate::blocks::rel(p, d);
        let st = srv.get(p);
        let ns = srv.get(q);
        let new = srv.update_shape(st, p, d, q, ns);
        if new != st {
            srv.set_block(p, new, nf);
        }
        let nn = srv.update_shape(ns, q, d.opposite(), p, new);
        if nn != ns {
            srv.set_block(q, nn, nf);
        }
    };
    // (a, b, c) axes per pass: faces across z (a = x, b = y), across y (a = z, b = x), across
    // x (a = y, b = z).
    let passes: [([usize; 3], Dir, Dir); 3] =
        [([0, 1, 2], Dir::North, Dir::South), ([2, 0, 1], Dir::Down, Dir::Up), ([1, 2, 0], Dir::West, Dir::East)];
    for (axes, neg, pos) in passes {
        let [ax, bx, cx] = axes;
        for a in 0..size[ax] {
            for b in 0..size[bx] {
                let mut last = false;
                for c in 0..=size[cx] {
                    let mut at = [0usize; 3];
                    at[ax] = a;
                    at[bx] = b;
                    at[cx] = c.min(size[cx].saturating_sub(1));
                    let cur = c != size[cx] && full[idx(at[0], at[1], at[2])];
                    if !last && cur {
                        face(srv, neg, at);
                    }
                    if last && !cur {
                        let mut prev = at;
                        prev[cx] = c - 1;
                        face(srv, pos, prev);
                    }
                    last = cur;
                }
            }
        }
    }
}
