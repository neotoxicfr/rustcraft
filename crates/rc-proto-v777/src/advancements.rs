//! Advancements and statistics (`docs/specs/advancements-stats.md` § 1, § 2): `update_advancements`,
//! `select_advancements_tab`, `award_stats` (to the client), `seen_advancements` (to the
//! server).

use rc_proto::nbt::Nbt;
use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{Decode, Reader, Result, read_count, read_str, write_str};

use crate::item::{ItemCodec, ItemStack};

const IDENT_MAX: usize = 32767;

fn ident(r: &mut Reader<'_>) -> Result<String> {
    read_str(r, IDENT_MAX).map(str::to_owned)
}

/// "Display" block of an advancement (§ 1.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub title: Option<Nbt>,
    pub description: Option<Nbt>,
    pub icon: ItemStack,
    /// 0 task, 1 challenge, 2 goal (out of range → 0).
    pub frame: u8,
    /// Background identifier (bit 0 of the flags).
    pub background: Option<String>,
    /// Bit 1: notification on obtaining.
    pub toast: bool,
    /// Bit 2: hidden until obtained.
    pub hidden: bool,
}

/// Advancement added, with its position in the tree (in cells).
#[derive(Debug, Clone, PartialEq)]
pub struct Added {
    pub id: String,
    pub parent: Option<String>,
    pub display: Option<Display>,
    /// Requirements: AND of ORs (groups of criterion names).
    pub requirements: Vec<Vec<String>>,
    pub x: f32,
    pub y: f32,
}

/// Criteria of an advancement: name → date obtained (Unix ms) if obtained.
pub type Criteria = Vec<(String, Option<i64>)>;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAdvancements {
    pub reset: bool,
    pub added: Vec<Added>,
    pub removed: Vec<String>,
    pub progress: Vec<(String, Criteria)>,
    /// Notifications allowed.
    pub show: bool,
}

fn read_display(r: &mut Reader<'_>, items: &ItemCodec) -> Result<Display> {
    let title = rc_proto::nbt::read(r)?;
    let description = rc_proto::nbt::read(r)?;
    let icon = items.read_template(r)?;
    let frame = match read_varint(r)? {
        f @ 0..=2 => f as u8,
        _ => 0,
    };
    let flags = i32::decode(r)?;
    let background = if flags & 1 != 0 { Some(ident(r)?) } else { None };
    Ok(Display { title, description, icon, frame, background, toast: flags & 2 != 0, hidden: flags & 4 != 0 })
}

pub fn read_update_advancements(r: &mut Reader<'_>, items: &ItemCodec) -> Result<UpdateAdvancements> {
    let reset = bool::decode(r)?;
    let n = read_count(r, 1)?;
    let mut added = Vec::with_capacity(n);
    for _ in 0..n {
        let id = ident(r)?;
        let parent = if bool::decode(r)? { Some(ident(r)?) } else { None };
        let display = if bool::decode(r)? { Some(read_display(r, items)?) } else { None };
        let groups = read_count(r, 1)?;
        let mut requirements = Vec::with_capacity(groups);
        for _ in 0..groups {
            let m = read_count(r, 1)?;
            requirements.push((0..m).map(|_| ident(r)).collect::<Result<Vec<_>>>()?);
        }
        let _telemetry = bool::decode(r)?;
        let x = f32::decode(r)?;
        let y = f32::decode(r)?;
        added.push(Added { id, parent, display, requirements, x, y });
    }
    let n = read_count(r, 1)?;
    let removed = (0..n).map(|_| ident(r)).collect::<Result<Vec<_>>>()?;
    let n = read_count(r, 2)?;
    let mut progress = Vec::with_capacity(n);
    for _ in 0..n {
        let id = ident(r)?;
        let m = read_count(r, 2)?;
        let mut criteria = Vec::with_capacity(m);
        for _ in 0..m {
            let name = ident(r)?;
            let at = if bool::decode(r)? { Some(i64::decode(r)?) } else { None };
            criteria.push((name, at));
        }
        progress.push((id, criteria));
    }
    let show = bool::decode(r)?;
    Ok(UpdateAdvancements { reset, added, removed, progress, show })
}

/// `select_advancements_tab`: root of the chosen tab.
pub fn read_select_tab(r: &mut Reader<'_>) -> Result<Option<String>> {
    if bool::decode(r)? { ident(r).map(Some) } else { Ok(None) }
}

/// `award_stats`: (type, value in the type's registry) → value.
pub fn read_award_stats(r: &mut Reader<'_>) -> Result<Vec<(i32, i32, i32)>> {
    let n = read_count(r, 3)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((read_varint(r)?, read_varint(r)?, read_varint(r)?));
    }
    Ok(out)
}

/// `seen_advancements`: `Some(root)` tab opened, `None` screen closed.
pub fn write_seen_advancements(w: &mut Vec<u8>, tab: Option<&str>) {
    match tab {
        Some(id) => {
            write_varint(w, 0);
            write_str(w, id);
        }
        None => write_varint(w, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn award_stats_and_seen() {
        let mut w = Vec::new();
        write_varint(&mut w, 2);
        for v in [0, 1, 5, 8, 6, 300] {
            write_varint(&mut w, v);
        }
        let got = read_award_stats(&mut Reader::new(&w)).unwrap();
        assert_eq!(got, vec![(0, 1, 5), (8, 6, 300)]);
        let mut s = Vec::new();
        write_seen_advancements(&mut s, Some("minecraft:story/root"));
        assert_eq!(s[0], 0);
        let mut s = Vec::new();
        write_seen_advancements(&mut s, None);
        assert_eq!(s, vec![1]);
    }

    #[test]
    fn update_without_display() {
        let items = ItemCodec::new(&[], &[]);
        let mut w = vec![1u8];
        write_varint(&mut w, 1);
        write_str(&mut w, "a:b");
        w.push(1);
        write_str(&mut w, "a:root");
        w.push(0);
        write_varint(&mut w, 1);
        write_varint(&mut w, 2);
        write_str(&mut w, "c1");
        write_str(&mut w, "c2");
        w.push(0);
        w.extend_from_slice(&1.5f32.to_be_bytes());
        w.extend_from_slice(&(-2.0f32).to_be_bytes());
        write_varint(&mut w, 0);
        write_varint(&mut w, 1);
        write_str(&mut w, "a:b");
        write_varint(&mut w, 1);
        write_str(&mut w, "c1");
        w.push(1);
        w.extend_from_slice(&42i64.to_be_bytes());
        w.push(1);
        let mut r = Reader::new(&w);
        let u = read_update_advancements(&mut r, &items).unwrap();
        r.finish().unwrap();
        assert!(u.reset && u.show);
        assert_eq!(u.added[0].parent.as_deref(), Some("a:root"));
        assert_eq!(u.added[0].requirements, vec![vec!["c1".to_owned(), "c2".to_owned()]]);
        assert_eq!((u.added[0].x, u.added[0].y), (1.5, -2.0));
        assert_eq!(u.progress[0].1, vec![("c1".to_owned(), Some(42))]);
    }
}
