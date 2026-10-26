//! Mount inventories (`docs/specs/special-screens.md` § 2): opened by
//! `mount_screen_open`, slots active depending on the entity, background, frames, chest and model of
//! the entity following the mouse.

use crate::inventory::Container;
use crate::menu::Kind;
use crate::menu_special::{Mount, MountKind, Sp};
use crate::special::{Ctx, ModelReq};

/// Recognised mount: kind, saddle and body active, from the entity kind and its data.
fn mount_of(e: &crate::entities::Entity, cols: i32) -> Option<Mount> {
    let k = e.kind.strip_prefix("minecraft:").unwrap_or(&e.kind);
    let adult = !e.baby();
    // Flags (index 18, after the 2 values of the "growing mob" layer): 2 = tamed equine;
    // tameable animal: 4.
    let flags = e.meta_byte(18) as u8;
    let horse_tame = flags & 2 != 0;
    let cols = cols.clamp(0, 5) as u8;
    let (kind, saddle, body) = match k {
        // Armor slot: tag `can_wear_horse_armor` (horse, zombie horse); saddle: tamed adult,
        // like the other equines.
        "horse" | "zombie_horse" => (MountKind::Horse, adult && horse_tame, true),
        "skeleton_horse" => (MountKind::Horse, adult && horse_tame, false),
        "donkey" | "mule" => (MountKind::Horse, adult && horse_tame, false),
        "llama" | "trader_llama" => (MountKind::Llama, false, true),
        "camel" | "camel_husk" => (MountKind::Horse, adult && horse_tame, false),
        "nautilus" | "zombie_nautilus" => {
            let tame = adult && flags & 4 != 0;
            (MountKind::Nautilus, tame, tame)
        }
        _ => return None,
    };
    Some(Mount { kind, saddle, body, cols })
}

/// Displayed name of an entity: custom name, otherwise the type name.
pub fn display_name(e: &crate::entities::Entity) -> rc_text::Component {
    if let Some(rc_proto_v777::entity::Meta::Text(Some(n))) = e.meta.get(&2)
        && let Some(c) = rc_text::from_nbt(n)
    {
        return c;
    }
    let k = e.kind.strip_prefix("minecraft:").unwrap_or(&e.kind);
    rc_text::Component::translate(format!("entity.minecraft.{k}"), vec![])
}

pub fn draw_back(cx: &mut Ctx<'_, '_>, c: &Container, m: Mount) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let bg = if m.kind == MountKind::Nautilus { "container_bg/nautilus" } else { "container_bg/horse" };
    let g = &mut *cx.gui;
    g.sprite_region(bg, 256.0, 256.0, 0.0, 0.0, f(gx), f(gy), 176.0, 166.0);
    if m.cols > 0 && m.kind != MountKind::Nautilus {
        let w = 18 * i32::from(m.cols);
        g.sprite_region("container/horse/chest_slots", 90.0, 54.0, 0.0, 0.0, f(gx + 79), f(gy + 17), f(w), 54.0);
    }
    if m.saddle {
        g.sprite("container/slot", f(gx + 7), f(gy + 17), 18.0, 18.0, 0xffff_ffff);
    }
    if m.body {
        g.sprite("container/slot", f(gx + 7), f(gy + 35), 18.0, 18.0, 0xffff_ffff);
    }
    crate::special::titles(cx, c, Some((8, 6)), Some((8, 72)));
    // Entity model (follows the mouse), between the background and the slots.
    if let Some(id) = c.extra.work.mount_entity {
        let rect = [f(gx + 26), f(gy + 18), f(gx + 78), f(gy + 70)];
        let mouse = cx.mouse.unwrap_or(((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0));
        crate::special::request_model(cx.gui, ModelReq::Entity { id, rect, size: 17.0, offset: 0.25, mouse });
    }
}

impl crate::App {
    /// `mount_screen_open` (§ 2.1): mount menu if the entity is known and is a mount.
    pub(crate) fn open_mount(&mut self, window: i32, columns: i32, entity: i32) {
        let Some(e) = self.entities.map.get(&entity) else { return };
        let Some(m) = mount_of(e, columns) else { return };
        let title = display_name(e);
        self.inventory.open(window, Kind::Special(Sp::Mount(m)), Some(title));
        if let Some(c) = &mut self.inventory.container {
            c.extra.work.mount_entity = Some(entity);
        }
        self.screens.show();
        self.keys.clear();
        self.grab(false);
    }

    /// Inventory key while riding a vehicle with an inventory (§ 2.1): "open the
    /// inventory" command to the server; true if it was sent.
    pub(crate) fn ride_inventory(&mut self) -> bool {
        let Some(v) = self.vehicle() else { return false };
        let Some(e) = self.entities.map.get(&v) else { return false };
        let k = e.kind.strip_prefix("minecraft:").unwrap_or(&e.kind);
        let has = mount_of(e, 0).is_some()
            || k.ends_with("chest_minecart")
            || k.ends_with("chest_boat")
            || k.ends_with("chest_raft");
        if has {
            let _ = self.session.link.send_player_command(OPEN_INVENTORY);
        }
        has
    }
}

/// "Open the inventory" action of the player command.
const OPEN_INVENTORY: i32 = 5;
