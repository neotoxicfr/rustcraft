//! Objects drawn in text (`object` components, `text.md` § 5.2): sprite from any known
//! atlas of the game, or player head (8 × 8 face, hat on top as chosen).
//!
//! Sprites of the interface atlas are taken as they are; the others are read on first
//! request (atlas sources) and copied into a reserved slot `dyn/text_obj/<i>`, heads
//! into `dyn/text_head/<i>`; the least recently drawn gives up its place. Sprite not found:
//! missing texture; unknown atlas: the missing glyph (advance of that glyph), like the game.

use std::sync::Arc;

use hashbrown::{HashMap, HashSet};
use rc_assets::gui::{KNOWN_ATLASES, MISSING_SPRITE, Sprite, TEXT_HEAD_SLOTS, TEXT_OBJECT_SIZE, TEXT_OBJECT_SLOTS};
use rc_assets::texture::Image;
use rc_text::ObjectFont;

use super::Gui;

/// Content of a head slot: skin identity, hat, skin generation.
type HeadKey = (u128, bool, u32);

pub struct TextObjects {
    /// Reserved slots of the interface atlas (objects, heads), in order.
    obj_sprites: Vec<Sprite>,
    head_sprites: Vec<Sprite>,
    /// Resources (reading of sprites and skins on demand); set with the renderer.
    pub assets: Option<Arc<rc_assets::Assets>>,
    /// Atlas object font → slot of the interface atlas already resolved.
    resolved: HashMap<String, Resolved>,
    /// Slots `dyn/text_obj/<i>`: object font copied, last frame drawn.
    obj_slots: Vec<(Option<String>, u64)>,
    head_slots: Vec<(Option<HeadKey>, u64)>,
    /// Skins received for the requested heads: face, hat, generation.
    skins: HashMap<u128, (Vec<u8>, Vec<u8>, u32)>,
    asked: HashSet<u128>,
    /// Skins to request (identity, profile), emptied by the main loop.
    pub wanted: Vec<(u128, String)>,
    pub(super) frame: u64,
    warned: bool,
}

#[derive(Clone, Copy)]
enum Resolved {
    Gui(Sprite),
    Slot(usize),
    Missing,
}

impl TextObjects {
    pub fn new(atlas: &rc_assets::gui::GuiAtlas) -> Self {
        let slots = |name: &str, n: usize| (0..n).filter_map(|i| atlas.sprites.get(&format!("{name}/{i}")).copied()).collect();
        TextObjects {
            obj_sprites: slots("dyn/text_obj", TEXT_OBJECT_SLOTS),
            head_sprites: slots("dyn/text_head", TEXT_HEAD_SLOTS),
            assets: None,
            resolved: HashMap::new(),
            obj_slots: Vec::new(),
            head_slots: Vec::new(),
            skins: HashMap::new(),
            asked: HashSet::new(),
            wanted: Vec::new(),
            frame: 0,
            warned: false,
        }
    }

    /// Skin arrived (Tab list, placed or item heads, text): face and hat kept even if
    /// the text has not requested it yet, because a key already requested elsewhere is not delivered
    /// a second time. Beyond `MAX_SKINS`, only the skins requested by the text get in.
    pub fn skin(&mut self, key: u128, img: &Image) {
        if !self.asked.contains(&key) && !self.skins.contains_key(&key) && self.skins.len() >= MAX_SKINS {
            return;
        }
        let Some((face, hat)) = crate::tablist::face_parts(img) else { return };
        let generation = self.skins.get(&key).map_or(0, |s| s.2) + 1;
        self.skins.insert(key, (face, hat, generation));
    }

    /// Content of a head slot; requests the profile's skin the first time.
    fn head_key(&mut self, hat: bool, uuid: u128, request: Option<(u128, &str)>, body: &str) -> HeadKey {
        if !body.is_empty() {
            (rc_text::head_hash(body), hat, u32::MAX)
        } else if let Some((key, spec)) = request {
            if self.asked.insert(key) {
                self.wanted.push((key, spec.to_owned()));
            }
            (key, hat, self.skins.get(&key).map_or(0, |s| s.2))
        } else {
            (uuid, hat, 0)
        }
    }

    /// More distinct objects in a frame than slots: reported once.
    fn saturated(&mut self) {
        if !self.warned {
            self.warned = true;
            eprintln!("text: more distinct objects in a frame than reserved slots");
        }
    }
}

/// Skins kept without a text request (face + hat: 512 bytes each).
const MAX_SKINS: usize = 1024;

/// Free slot, otherwise the least recently drawn; never a slot already drawn in
/// frame `frame` (a quad emitted earlier would show the new content): `None`, everything is taken.
fn victim<K>(slots: &mut Vec<(Option<K>, u64)>, n: usize, frame: u64) -> Option<usize> {
    if slots.len() < n {
        slots.push((None, 0));
        return Some(slots.len() - 1);
    }
    (0..slots.len())
        .filter(|&i| slots[i].0.is_none() || slots[i].1 != frame)
        .min_by_key(|&i| (slots[i].0.is_some(), slots[i].1))
}

impl Gui {
    /// Sprite of the interface atlas that draws this object; `None`: atlas unknown to the game
    /// (the character is then drawn with the missing glyph).
    pub(super) fn object_sprite(&mut self, queue: &wgpu::Queue, font: &str, o: ObjectFont<'_>) -> Option<Sprite> {
        match o {
            ObjectFont::Atlas { atlas, sprite } => self.atlas_object(queue, font, atlas, sprite),
            ObjectFont::Player { hat, uuid, request, body } => self.head_object(queue, hat, uuid, request, body),
        }
    }

    fn atlas_object(&mut self, queue: &wgpu::Queue, font: &str, atlas: &str, sprite: &str) -> Option<Sprite> {
        if !KNOWN_ATLASES.contains(&atlas) {
            return None;
        }
        let frame = self.objects.frame;
        let known = match self.objects.resolved.get(font) {
            Some(Resolved::Slot(i)) if self.objects.obj_slots[*i].0.as_deref() == Some(font) => Some(Resolved::Slot(*i)),
            Some(Resolved::Slot(_)) | None => None,
            Some(r) => Some(*r),
        };
        let r = match known {
            Some(r) => r,
            None => {
                // All slots taken in this frame: missing texture (advance 8 like the
                // measure), nothing kept; retry on the next frame.
                let Some(r) = self.load_atlas_object(queue, font, atlas, sprite) else {
                    self.objects.saturated();
                    return self.atlas.sprites.get(MISSING_SPRITE).copied();
                };
                self.objects.resolved.insert(font.to_owned(), r);
                r
            }
        };
        match r {
            Resolved::Gui(s) => Some(s),
            Resolved::Slot(i) => {
                self.objects.obj_slots[i].1 = frame;
                self.objects.obj_sprites.get(i).copied()
            }
            Resolved::Missing => self.atlas.sprites.get(MISSING_SPRITE).copied(),
        }
    }

    /// `None`: no slot can be freed in this frame (nothing is read).
    fn load_atlas_object(&mut self, queue: &wgpu::Queue, font: &str, atlas: &str, sprite: &str) -> Option<Resolved> {
        if atlas == "minecraft:gui"
            && let Some(&s) = self.atlas.sprites.get(sprite.strip_prefix("minecraft:").unwrap_or(sprite))
        {
            return Some(Resolved::Gui(s));
        }
        let o = &mut self.objects;
        let i = victim(&mut o.obj_slots, o.obj_sprites.len(), o.frame)?;
        let Some(img) = o.assets.as_deref().and_then(|a| rc_assets::gui::atlas_sprite(a, atlas, sprite)) else {
            return Some(Resolved::Missing);
        };
        let n = TEXT_OBJECT_SIZE;
        if !self.write_sprite(queue, &format!("dyn/text_obj/{i}"), (n, n), &img.resized(n).rgba) {
            return Some(Resolved::Missing);
        }
        self.objects.obj_slots[i] = (Some(font.to_owned()), self.objects.frame);
        Some(Resolved::Slot(i))
    }

    fn head_object(
        &mut self,
        queue: &wgpu::Queue,
        hat: bool,
        uuid: Result<u128, &str>,
        request: Option<(u128, &str)>,
        body: &str,
    ) -> Option<Sprite> {
        let uuid = uuid.unwrap_or_else(rc_client::offline_uuid);
        let key = self.objects.head_key(hat, uuid, request, body);
        let o = &mut self.objects;
        let frame = o.frame;
        let slot = match o.head_slots.iter().position(|s| s.0 == Some(key)) {
            Some(i) => i,
            None => {
                let n = o.head_sprites.len();
                let Some(i) = victim(&mut o.head_slots, n, frame) else {
                    o.saturated();
                    return self.atlas.sprites.get(MISSING_SPRITE).copied();
                };
                // Unreadable skin (resources absent): missing texture, advance 8 like the
                // measure (the game always has a default skin).
                let Some(rgba) = self.head_pixels(key, uuid, body) else {
                    return self.atlas.sprites.get(MISSING_SPRITE).copied();
                };
                if !self.write_sprite(queue, &format!("dyn/text_head/{i}"), (8, 8), &rgba) {
                    return self.atlas.sprites.get(MISSING_SPRITE).copied();
                }
                self.objects.head_slots[i].0 = Some(key);
                i
            }
        };
        self.objects.head_slots[slot].1 = frame;
        self.objects.head_sprites.get(slot).copied()
    }

    /// 8 × 8 pixels of a head: skin received, otherwise forced skin or default skin.
    fn head_pixels(&self, key: HeadKey, uuid: u128, body: &str) -> Option<Vec<u8>> {
        let o = &self.objects;
        let (face, hat_layer) = match o.skins.get(&key.0).filter(|_| key.2 != 0 && key.2 != u32::MAX) {
            Some(s) => (s.0.clone(), s.1.clone()),
            None => {
                let assets = o.assets.as_deref()?;
                let img = if body.is_empty() {
                    crate::tablist::default_skin(assets, uuid)
                } else {
                    let (ns, path) = body.split_once(':').unwrap_or(("minecraft", body));
                    assets.image(&format!("assets/{ns}/textures/{path}.png"))
                };
                crate::tablist::face_parts(&img?)?
            }
        };
        Some(crate::tablist::compose(&face, &hat_layer, key.1, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objects() -> TextObjects {
        let image = Image { width: 1, height: 1, rgba: vec![0; 4] };
        TextObjects::new(&rc_assets::gui::GuiAtlas { image, sprites: HashMap::new() })
    }

    #[test]
    fn skin_delivered_before_the_text_asks_is_kept() {
        // Skin already requested by the Tab list (same key, same property) and delivered before the
        // message: the skin queue will not send it again, the text's head must take it over.
        let mut o = objects();
        let key = 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10u128;
        o.skin(key, &Image { width: 64, height: 64, rgba: vec![7; 64 * 64 * 4] });
        assert_eq!(o.head_key(true, key, Some((key, "e30=")), ""), (key, true, 1));
        // Requested anyway once (another property possible: the queue decides).
        assert_eq!(o.wanted, vec![(key, "e30=".to_owned())]);
    }

    #[test]
    fn slot_drawn_in_this_frame_is_never_evicted() {
        let mut slots: Vec<(Option<u8>, u64)> = vec![(Some(1), 5), (Some(2), 4)];
        assert_eq!(victim(&mut slots, 2, 6), Some(1));
        assert_eq!(victim(&mut slots, 2, 4), Some(0));
        slots[0].1 = 4;
        assert_eq!(victim(&mut slots, 2, 4), None);
        assert_eq!(victim(&mut slots, 3, 4), Some(2));
    }
}
