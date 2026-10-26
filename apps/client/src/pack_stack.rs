//! Resource pack stack (`docs/specs/resource-packs.md` § 1 to 3): known packs
//! (`vanilla`, built-in packs of the asset index, `resourcepacks` folder), player's choice
//! (`options.txt`) and active server packs; paths in priority order for `resources.rs`.

use std::path::{Path, PathBuf};

use rc_assets::packs::Compat;
use rc_text::Component;

use crate::options::{Options, Val};

pub(crate) const VANILLA: &str = "vanilla";

/// Optional built-in packs, read from the asset index if it has them.
const BUILT_IN: [&str; 2] = ["programmer_art", "high_contrast"];

/// A known pack.
#[derive(Debug, Clone)]
pub(crate) struct PackEntry {
    pub id: String,
    pub title: Component,
    pub description: Component,
    pub compat: Compat,
    /// File or folder of the pack; `vanilla`: version jar (icon only).
    pub path: PathBuf,
    /// Required (never removed from the selected ones): `vanilla`.
    pub required: bool,
}

/// Known packs, player's choice, server packs.
#[derive(Default)]
pub(crate) struct PackStack {
    /// Known packs, sorted by id.
    pub known: Vec<PackEntry>,
    /// Player's choice, from bottom to top (order of `resourcePacks`).
    pub selected: Vec<String>,
    /// Active server packs, in order of arrival (the last has priority).
    pub server: Vec<PathBuf>,
    /// `server` carries the singleplayer world pack (`resources.zip`): "World" source on screen.
    pub world: bool,
}

/// `resourcepacks` folder of the game folder.
pub(crate) fn pack_dir() -> PathBuf {
    crate::servers::game_dir().join("resourcepacks")
}

fn description(v: &serde_json::Value) -> Component {
    rc_client::status::json_component(v).unwrap_or_else(|| Component::text(""))
}

/// Path of a built-in pack in the launcher's objects (`minecraft/resourcepacks/<name>.zip`).
fn built_in_paths() -> Vec<(String, PathBuf)> {
    let Ok(install) = rc_data::locate("26.3") else { return Vec::new() };
    let Some((index, objects)) = install.asset_index() else { return Vec::new() };
    let Some(v) = std::fs::read(&index).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
        return Vec::new();
    };
    BUILT_IN
        .iter()
        .filter_map(|n| {
            let h = v["objects"][format!("minecraft/resourcepacks/{n}.zip")]["hash"].as_str()?;
            let p = objects.join(h.get(..2)?).join(h);
            p.is_file().then(|| ((*n).to_owned(), p))
        })
        .collect()
}

/// All the known packs (§ 1, 2), sorted by id.
pub(crate) fn scan() -> Vec<PackEntry> {
    let mut out = Vec::new();
    if let Ok(install) = rc_data::locate("26.3") {
        out.push(PackEntry {
            id: VANILLA.into(),
            title: Component::translate("resourcePack.vanilla.name", vec![]),
            description: Component::translate("resourcePack.vanilla.description", vec![]),
            compat: Compat::Compatible,
            path: install.client_jar,
            required: true,
        });
    }
    for (name, path) in built_in_paths() {
        let Some(meta) =
            rc_assets::packs::read_root(&path, "pack.mcmeta").and_then(|b| rc_assets::packs::parse_meta(&b))
        else {
            continue;
        };
        out.push(PackEntry {
            title: Component::translate(format!("resourcePack.{name}.name"), vec![]),
            id: name,
            description: description(&meta.description),
            compat: meta.compat,
            path,
            required: false,
        });
    }
    let dir = pack_dir();
    let _ = std::fs::create_dir_all(&dir);
    for p in rc_assets::packs::discover(&dir, crate::servers::allowed_symlinks()) {
        out.push(PackEntry {
            id: p.id,
            title: Component::text(p.name),
            description: description(&p.meta.description),
            compat: p.meta.compat,
            path: p.path,
            required: false,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

impl PackStack {
    pub fn get(&self, id: &str) -> Option<&PackEntry> {
        self.known.iter().find(|p| p.id == id)
    }

    /// Startup (§ 3): known packs, choices of `options.txt` validated (absent and now
    /// incompatible ones removed, `vanilla` put back at the bottom if missing), settings rewritten if changed.
    pub fn startup(opts: &mut Options, options_path: &Path) -> PackStack {
        let mut s = PackStack { known: scan(), ..Default::default() };
        let mut incompatible = opts.list("incompatibleResourcePacks");
        let mut selected: Vec<String> = Vec::new();
        for id in opts.list("resourcePacks") {
            let found =
                s.get(&id).or_else(|| (!id.starts_with("file/")).then(|| s.get(&format!("file/{id}"))).flatten());
            let Some(p) = found else {
                eprintln!("resource pack removed from the settings (absent): {id}");
                continue;
            };
            if !p.compat.ok() && !incompatible.contains(&id) {
                eprintln!("resource pack removed from the settings (now incompatible): {id}");
                continue;
            }
            if p.compat.ok() {
                incompatible.retain(|x| *x != id);
            }
            if !selected.contains(&p.id) {
                selected.push(p.id.clone());
            }
        }
        if s.get(VANILLA).is_some() && !selected.iter().any(|x| x == VANILLA) {
            selected.insert(0, VANILLA.into());
        }
        s.selected = selected;
        let changed = s.write_options(opts);
        let changed_incompat = opts.set("incompatibleResourcePacks", Val::L(s.incompatible_selected()));
        if changed || changed_incompat {
            let _ = opts.save(options_path);
        }
        s
    }

    fn incompatible_selected(&self) -> Vec<String> {
        self.selected.iter().filter(|id| self.get(id).is_some_and(|p| !p.compat.ok())).cloned().collect()
    }

    /// Writes `resourcePacks` and `incompatibleResourcePacks`; true if `resourcePacks` changed.
    pub fn write_options(&self, opts: &mut Options) -> bool {
        let changed = opts.set("resourcePacks", Val::L(self.selected.clone()));
        opts.set("incompatibleResourcePacks", Val::L(self.incompatible_selected()));
        changed
    }

    /// Paths of the packs above the jar, decreasing priority: server packs (last
    /// arrived first), then the player's choice from top to bottom. Packs chosen below `vanilla`
    /// are not stacked (the jar remains the last source).
    pub fn paths(&self) -> Vec<PathBuf> {
        let (mut out, player) = self.split_paths();
        out.extend(player);
        out
    }

    /// [`PackStack::paths`] in two: server packs, then the player's choice.
    pub fn split_paths(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let server: Vec<PathBuf> = self.server.iter().rev().cloned().collect();
        let mut player = Vec::new();
        for id in self.selected.iter().rev() {
            if id == VANILLA {
                break;
            }
            if let Some(p) = self.get(id) {
                player.push(p.path.clone());
            }
        }
        (server, player)
    }

    /// Loading failed (default font): player's choices removed like the game (only
    /// `vanilla`, imposed, stays), server packs too if `all`; both settings lists
    /// are emptied, like the game.
    pub fn drop_after_failure(&mut self, all: bool, opts: &mut Options) {
        self.selected.retain(|id| id == VANILLA);
        if all {
            self.server.clear();
            self.world = false;
        }
        opts.set("resourcePacks", Val::L(Vec::new()));
        opts.set("incompatibleResourcePacks", Val::L(Vec::new()));
    }

    /// Rereads the folder (screen open, folder modified): choices kept if they still exist.
    pub fn rescan(&mut self) {
        self.known = scan();
        let known = std::mem::take(&mut self.known);
        self.selected.retain(|id| known.iter().any(|p| p.id == *id));
        self.known = known;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> PackEntry {
        PackEntry {
            id: id.into(),
            title: Component::text(id),
            description: Component::text(""),
            compat: Compat::Compatible,
            path: PathBuf::from(id),
            required: id == VANILLA,
        }
    }

    #[test]
    fn priority_order() {
        let s = PackStack {
            known: vec![entry("file/a"), entry("file/b"), entry("file/c"), entry(VANILLA)],
            selected: vec!["file/c".into(), VANILLA.into(), "file/a".into(), "file/b".into()],
            server: vec![PathBuf::from("s1"), PathBuf::from("s2")],
            world: false,
        };
        let p: Vec<String> = s.paths().iter().map(|p| p.display().to_string()).collect();
        assert_eq!(p, ["s2", "s1", "file/b", "file/a"]);
    }

    #[test]
    fn failed_load_drops_the_choices_like_the_game() {
        let mut s = PackStack {
            known: vec![entry("file/a"), entry(VANILLA)],
            selected: vec![VANILLA.into(), "file/a".into()],
            server: vec![PathBuf::from("s1")],
            world: false,
        };
        let mut o = Options::default();
        s.drop_after_failure(false, &mut o);
        assert_eq!(s.selected, [VANILLA]);
        assert!(o.list("resourcePacks").is_empty());
        assert_eq!(s.server.len(), 1);
        s.drop_after_failure(true, &mut o);
        assert!(s.paths().is_empty());
    }
}
