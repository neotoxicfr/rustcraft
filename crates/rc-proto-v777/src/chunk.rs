//! "Chunk with light" packet and light update. Format: `docs/specs/chunks.md`.

use rc_proto::varint::read_varint;
use rc_proto::{Decode, Error, Reader, Result, nbt, read_count};
use rc_world::{Container, LIGHT_BYTES, LightLayer, SECTION_BIOMES, SECTION_BLOCKS, Section, longs_for};

/// What decoding needs to know about the current world.
#[derive(Debug, Clone, Copy)]
pub struct ChunkContext {
    /// Number of sections of the dimension (`height / 16`).
    pub sections: usize,
    /// Number of block states and width of the local global palette.
    pub block_states: u32,
    pub block_bits: u8,
    /// Number of biomes received at configuration and the matching global width.
    pub biomes: u32,
    pub biome_bits: u8,
}

impl ChunkContext {
    pub fn new(sections: usize, block_states: u32, biomes: u32) -> Self {
        let bits = |n: u32| (u32::BITS - n.max(2).saturating_sub(1).leading_zeros()) as u8;
        ChunkContext { sections, block_states, block_bits: bits(block_states), biomes, biome_bits: bits(biomes) }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntity {
    /// Local coordinates in the column: x and z on 4 bits, absolute y.
    pub x: u8,
    pub z: u8,
    pub y: i16,
    pub kind: i32,
    pub data: Option<nbt::Nbt>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LightData {
    /// `sections + 2` positions; `Some` if the data is present in the packet.
    pub sky: Vec<LightLayer>,
    pub block: Vec<LightLayer>,
    /// Positions explicitly empty (zero light) according to the server.
    pub sky_empty: Vec<u64>,
    pub block_empty: Vec<u64>,
    /// Raw masks of the positions present (for a partial update).
    pub sky_mask: Vec<u64>,
    pub block_mask: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChunkPacket {
    pub x: i32,
    pub z: i32,
    pub heightmaps: Vec<(i32, Box<[i64]>)>,
    pub sections: Vec<Section>,
    pub block_entities: Vec<BlockEntity>,
    pub light: LightData,
}

#[derive(Clone, Copy)]
enum Kind {
    Blocks,
    Biomes,
}

fn read_i64s(r: &mut Reader<'_>, n: usize) -> Result<Box<[i64]>> {
    let bytes = r.take(n.checked_mul(8).ok_or(Error::LengthOutOfRange { len: n as i64, max: usize::MAX / 8 })?)?;
    Ok(bytes.as_chunks::<8>().0.iter().map(|c| i64::from_be_bytes(*c)).collect())
}

fn read_id(r: &mut Reader<'_>, limit: u32) -> Result<u32> {
    let id = read_varint(r)?;
    if id < 0 || id as u32 >= limit {
        return Err(Error::InvalidValue { what: "palette identifier", value: id.into() });
    }
    Ok(id as u32)
}

/// Paletted container. The read width follows the local configuration (see the spec).
fn read_container(r: &mut Reader<'_>, kind: Kind, ctx: &ChunkContext) -> Result<Container> {
    let (entries, limit, global_bits) = match kind {
        Kind::Blocks => (SECTION_BLOCKS, ctx.block_states, ctx.block_bits),
        Kind::Biomes => (SECTION_BIOMES, ctx.biomes, ctx.biome_bits),
    };
    let sent = r.u8()?;
    let (bits, has_palette) = match (kind, sent) {
        (_, 0) => {
            let v = read_id(r, limit)?;
            return Ok(Container::Single(v));
        }
        (Kind::Blocks, 1..=4) => (4, true),
        (Kind::Blocks, 5..=8) => (sent, true),
        (Kind::Biomes, 1..=3) => (sent, true),
        _ => (global_bits, false),
    };
    let palette = if has_palette {
        let n = read_count(r, 1)?;
        if n > 1 << bits {
            return Err(Error::LengthOutOfRange { len: n as i64, max: 1 << bits });
        }
        Some((0..n).map(|_| read_id(r, limit)).collect::<Result<Box<[u32]>>>()?)
    } else {
        None
    };
    let n = longs_for(entries, bits);
    let raw = r.take(n * 8)?;
    let data: Box<[u64]> = raw.as_chunks::<8>().0.iter().map(|c| u64::from_be_bytes(*c)).collect();
    let direct = palette.is_none();
    let c = Container::packed(entries, bits, palette, data)
        .ok_or_else(|| Error::InvalidValue { what: "container", value: bits.into() })?;
    // Without a palette, identifiers are raw: an unknown state is refused as in the game.
    if direct {
        let mut values = [0u32; SECTION_BLOCKS];
        let values = &mut values[..entries];
        c.unpack_into(values);
        if let Some(&v) = values.iter().find(|&&v| v >= limit) {
            return Err(Error::InvalidValue { what: "direct identifier", value: v.into() });
        }
    }
    Ok(c)
}

/// Bit set: prefixed byte array (VarInt), bit 0 = least significant bit of the
/// first byte. Stored as 64-bit words for position tests.
fn read_bitset(r: &mut Reader<'_>) -> Result<Vec<u64>> {
    let n = read_count(r, 1)?;
    let bytes = r.take(n)?;
    Ok(bytes
        .chunks(8)
        .map(|c| {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            u64::from_le_bytes(w)
        })
        .collect())
}

fn bit(set: &[u64], i: usize) -> bool {
    set.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)
}

fn read_layers(r: &mut Reader<'_>, mask: &[u64], positions: usize) -> Result<Vec<LightLayer>> {
    let n = read_count(r, LIGHT_BYTES)?;
    let mut out: Vec<LightLayer> = vec![None; positions];
    let mut slots = (0..positions).filter(|&i| bit(mask, i));
    for _ in 0..n {
        let len = read_count(r, 1)?;
        if len != LIGHT_BYTES {
            return Err(Error::LengthOutOfRange { len: len as i64, max: LIGHT_BYTES });
        }
        let bytes: [u8; LIGHT_BYTES] = r.take_array()?;
        let slot =
            slots.next().ok_or(Error::InvalidValue { what: "light sheet without position", value: n as i64 })?;
        out[slot] = Some(std::sync::Arc::new(bytes));
    }
    Ok(out)
}

/// Light (shared by the chunk packet and the light update).
pub fn read_light(r: &mut Reader<'_>, sections: usize) -> Result<LightData> {
    let positions = sections + 2;
    let sky_mask = read_bitset(r)?;
    let block_mask = read_bitset(r)?;
    let sky_empty = read_bitset(r)?;
    let block_empty = read_bitset(r)?;
    let mut sky = read_layers(r, &sky_mask, positions)?;
    let mut block = read_layers(r, &block_mask, positions)?;
    // Positions declared empty (and not present): shared zero sheet.
    for (layers, empty) in [(&mut sky, &sky_empty), (&mut block, &block_empty)] {
        for (i, l) in layers.iter_mut().enumerate() {
            if l.is_none() && bit(empty, i) {
                *l = Some(rc_world::zero_layer());
            }
        }
    }
    Ok(LightData { sky, block, sky_empty, block_empty, sky_mask, block_mask })
}

/// Body of the chunk-with-light packet (after the identifier).
/// `chunks_biomes` (`protocol-supplements.md` § 13): for each column, its position
/// (packed like `forget_level_chunk`: x in the low 32 bits, z in the high) and the biome
/// containers of all its sections, bottom to top.
pub fn read_chunks_biomes(r: &mut Reader<'_>, ctx: &ChunkContext) -> Result<Vec<(i32, i32, Vec<Container>)>> {
    let n = read_count(r, 9)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let packed = i64::decode(r)?;
        let (x, z) = (packed as i32, (packed >> 32) as i32);
        let len = read_count(r, 1)?;
        if len > 2 << 20 {
            return Err(Error::LengthOutOfRange { len: len as i64, max: 2 << 20 });
        }
        let mut sr = Reader::new(r.take(len)?);
        let biomes = (0..ctx.sections).map(|_| read_container(&mut sr, Kind::Biomes, ctx)).collect::<Result<_>>()?;
        out.push((x, z, biomes));
    }
    Ok(out)
}

pub fn read_chunk(r: &mut Reader<'_>, ctx: &ChunkContext) -> Result<ChunkPacket> {
    let x = i32::decode(r)?;
    let z = i32::decode(r)?;

    let n = read_count(r, 2)?;
    let mut heightmaps = Vec::with_capacity(n);
    for _ in 0..n {
        let kind = read_varint(r)?;
        let len = read_count(r, 8)?;
        heightmaps.push((kind, read_i64s(r, len)?));
    }

    let len = read_count(r, 1)?;
    if len > 2 << 20 {
        return Err(Error::LengthOutOfRange { len: len as i64, max: 2 << 20 });
    }
    let mut sr = Reader::new(r.take(len)?);
    let mut sections = Vec::with_capacity(ctx.sections);
    for _ in 0..ctx.sections {
        let non_empty = i16::decode(&mut sr)?;
        // Number of fluids: used for the server's random draw, not by the client.
        let _fluids = i16::decode(&mut sr)?;
        let blocks = read_container(&mut sr, Kind::Blocks, ctx)?;
        let biomes = read_container(&mut sr, Kind::Biomes, ctx)?;
        sections.push(Section { non_empty, blocks, biomes });
    }
    sr.finish()?;

    let n = read_count(r, 4)?;
    let mut block_entities = Vec::with_capacity(n);
    for _ in 0..n {
        let xz = r.u8()?;
        let y = i16::decode(r)?;
        let kind = read_varint(r)?;
        let data = nbt::read(r)?;
        block_entities.push(BlockEntity { x: xz >> 4, z: xz & 15, y, kind, data });
    }

    let light = read_light(r, ctx.sections)?;
    Ok(ChunkPacket { x, z, heightmaps, sections, block_entities, light })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_proto::varint::write_varint;

    fn ctx() -> ChunkContext {
        ChunkContext::new(1, 35723, 65)
    }

    #[test]
    fn context_bits() {
        let c = ctx();
        assert_eq!((c.block_bits, c.biome_bits), (16, 7));
    }

    #[test]
    fn small_palette_is_read_with_four_bits() {
        // Received byte 2 for blocks: 4-bit read (256 words).
        let mut w = vec![2u8];
        write_varint(&mut w, 2);
        write_varint(&mut w, 0);
        write_varint(&mut w, 9);
        let mut words = vec![0u64; 256];
        words[0] = 0x10; // entry 1 = index 1
        for x in &words {
            w.extend_from_slice(&x.to_be_bytes());
        }
        let c = read_container(&mut Reader::new(&w), Kind::Blocks, &ctx()).unwrap();
        assert_eq!((c.get(0), c.get(1), c.get(2)), (0, 9, 0));
    }

    #[test]
    fn direct_uses_local_global_width() {
        // Received byte 15: 16-bit read (local width), 1024 words.
        let mut w = vec![15u8];
        let mut words = vec![0u64; 1024];
        words[0] = 0x8b8a_0000_0000_1234;
        for x in &words {
            w.extend_from_slice(&x.to_be_bytes());
        }
        let c = read_container(&mut Reader::new(&w), Kind::Blocks, &ctx()).unwrap();
        assert_eq!((c.get(0), c.get(3)), (0x1234, 35722));
        // 35723 exceeds the state table: refused.
        w[1..9].copy_from_slice(&0x8b8b_0000_0000_1234u64.to_be_bytes());
        assert!(read_container(&mut Reader::new(&w), Kind::Blocks, &ctx()).is_err());
    }

    #[test]
    fn rejects_out_of_range_palette_ids() {
        let mut w = vec![0u8];
        write_varint(&mut w, 40000);
        assert!(read_container(&mut Reader::new(&w), Kind::Blocks, &ctx()).is_err());
    }
}
