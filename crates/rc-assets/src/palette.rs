//! Textures recoloured by palette (armour trims).
//!
//! A palette is an image `textures/palettes/<id>.png` read pixel by pixel (row order).
//! A mapping associates each opaque colour of a base palette with the colour of the same
//! rank in a target palette; a recoloured image replaces each visible pixel whose colour
//! (without alpha) appears in the base, and multiplies its alpha by that of the target colour.
//!
//! Two uses, like the game: the permutations declared by the item atlas
//! (`atlases/items.json`, source `paletted_permutations`: `<texture><separator><suffix>`), and
//! the entity textures recoloured on demand (base declared by the `.png.mcmeta` of the
//! texture: `{"palette": {"base_palette": "<id>"}}`).

use hashbrown::HashMap;

use crate::source::Assets;
use crate::texture::Image;

/// Colours of a palette, in RGBA.
pub type Palette = Vec<[u8; 4]>;

fn split(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

/// Palette `id` (`minecraft:trim/iron` → `assets/minecraft/textures/palettes/trim/iron.png`).
pub fn load_palette(assets: &Assets, id: &str) -> Option<Palette> {
    let (ns, path) = split(id);
    let img = assets.image(&format!("assets/{ns}/textures/palettes/{path}.png"))?;
    Some(img.rgba.as_chunks::<4>().0.to_vec())
}

fn rgb(c: [u8; 4]) -> u32 {
    u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2])
}

/// Mapping base colour (without alpha) → target colour; empty if the sizes differ.
pub fn mapping(base: &[[u8; 4]], target: &[[u8; 4]]) -> HashMap<u32, [u8; 4]> {
    if base.len() != target.len() {
        return HashMap::new();
    }
    // Like a table filled in order: the last occurrence of a colour wins.
    base.iter().zip(target).filter(|(b, _)| b[3] != 0).map(|(&b, &t)| (rgb(b), t)).collect()
}

/// Recoloured copy of `img`.
pub fn apply(img: &Image, map: &HashMap<u32, [u8; 4]>) -> Image {
    let mut out = img.clone();
    for px in out.rgba.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a == 0 {
            continue;
        }
        let key = rgb([px[0], px[1], px[2], 255]);
        let t = map.get(&key).copied().unwrap_or([px[0], px[1], px[2], 255]);
        px[..3].copy_from_slice(&t[..3]);
        px[3] = (u32::from(a) * u32::from(t[3]) / 255) as u8;
    }
    out
}

/// Base palette declared by the `.png.mcmeta` of a texture (`assets/…/x.png`).
pub fn base_palette(assets: &Assets, png_path: &str) -> Option<String> {
    let b = assets.read(&format!("{png_path}.mcmeta"))?;
    let v: serde_json::Value = serde_json::from_slice(&b).ok()?;
    let id = v["palette"]["base_palette"].as_str()?;
    Some(if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") })
}

/// An atlas `paletted_permutations` source.
#[derive(Debug, Clone, Default)]
pub struct Permutations {
    textures: Vec<String>,
    key: String,
    /// (suffix, target palette), in file order.
    perms: Vec<(String, String)>,
    separator: String,
}

fn norm(id: &str) -> String {
    let (ns, path) = split(id);
    format!("{ns}:{path}")
}

impl Permutations {
    /// Sources declared by `assets/minecraft/atlases/<atlas>.json`.
    pub fn of_atlas(assets: &Assets, atlas: &str) -> Vec<Self> {
        let Some(b) = assets.read(&format!("assets/minecraft/atlases/{atlas}.json")) else { return Vec::new() };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) else { return Vec::new() };
        v["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| {
                s["type"].as_str().map(|t| t.strip_prefix("minecraft:").unwrap_or(t)) == Some("paletted_permutations")
            })
            .filter_map(|s| {
                Some(Permutations {
                    textures: s["textures"].as_array()?.iter().filter_map(|t| t.as_str().map(norm)).collect(),
                    key: norm(s["palette_key"].as_str()?),
                    perms: s["permutations"]
                        .as_object()?
                        .iter()
                        .filter_map(|(k, p)| Some((k.clone(), norm(p.as_str()?))))
                        .collect(),
                    separator: s["separator"].as_str().unwrap_or("_").to_owned(),
                })
            })
            .collect()
    }

    /// Image of the permutation `name` (`minecraft:trims/items/helmet_trim_iron`), if this source
    /// declares it.
    pub fn resolve(&self, assets: &Assets, name: &str) -> Option<Image> {
        let name = norm(name);
        for t in &self.textures {
            let Some(rest) = name.strip_prefix(t.as_str()).and_then(|r| r.strip_prefix(self.separator.as_str())) else {
                continue;
            };
            let Some((_, pal)) = self.perms.iter().find(|(s, _)| s == rest) else { continue };
            let (ns, path) = split(t);
            let base = assets.image(&format!("assets/{ns}/textures/{path}.png"))?.first_frame();
            let map = mapping(&load_palette(assets, &self.key)?, &load_palette(assets, pal)?);
            return Some(apply(&base, &map));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recolors_known_colors_and_keeps_others() {
        let base = vec![[10, 20, 30, 255], [1, 2, 3, 0], [40, 50, 60, 255]];
        let target = vec![[100, 110, 120, 128], [9, 9, 9, 255], [7, 8, 9, 255]];
        let map = mapping(&base, &target);
        // The transparent colour of the base maps to nothing.
        assert!(!map.contains_key(&rgb([1, 2, 3, 0])));
        let img =
            Image { width: 4, height: 1, rgba: vec![10, 20, 30, 255, 40, 50, 60, 200, 5, 5, 5, 255, 10, 20, 30, 0] };
        let out = apply(&img, &map);
        // Alpha multiplied by that of the target (255 × 128 / 255).
        assert_eq!(&out.rgba[0..4], &[100, 110, 120, 128]);
        assert_eq!(&out.rgba[4..8], &[7, 8, 9, 200]);
        // Unknown colour: unchanged; invisible pixel: intact.
        assert_eq!(&out.rgba[8..12], &[5, 5, 5, 255]);
        assert_eq!(&out.rgba[12..16], &[10, 20, 30, 0]);
    }

    #[test]
    fn different_sizes_give_no_mapping() {
        assert!(mapping(&[[1, 1, 1, 255]], &[]).is_empty());
    }
}
