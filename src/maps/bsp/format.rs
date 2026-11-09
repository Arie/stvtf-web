//! The BSP file format: lumps, record layouts and checked readers.

use anyhow::{Result, anyhow, bail};

use super::Vec3;

pub(super) const LUMP_ENTITIES: usize = 0;
pub(super) const LUMP_PLANES: usize = 1;
pub(super) const LUMP_TEXDATA: usize = 2;
pub(super) const LUMP_VERTEXES: usize = 3;
pub(super) const LUMP_NODES: usize = 5;
pub(super) const LUMP_TEXINFO: usize = 6;
pub(super) const LUMP_FACES: usize = 7;
pub(super) const LUMP_LEAFS: usize = 10;
pub(super) const LUMP_EDGES: usize = 12;
pub(super) const LUMP_SURFEDGES: usize = 13;
pub(super) const LUMP_MODELS: usize = 14;
pub(super) const LUMP_LEAFBRUSHES: usize = 17;
pub(super) const LUMP_BRUSHES: usize = 18;
pub(super) const LUMP_BRUSHSIDES: usize = 19;
pub(super) const LUMP_DISPINFO: usize = 26;
pub(super) const LUMP_DISP_VERTS: usize = 33;
pub(super) const LUMP_GAME_LUMP: usize = 35;
pub(super) const LUMP_TEXDATA_STRING_DATA: usize = 43;
pub(super) const LUMP_TEXDATA_STRING_TABLE: usize = 44;

/// `SURF_*` texture flags of faces that are not drawn as part of the world.
pub(super) const SURF_SKY2D: i32 = 0x2;
pub(super) const SURF_SKY: i32 = 0x4;
pub(super) const SURF_TRIGGER: i32 = 0x40;
pub(super) const SURF_NODRAW: i32 = 0x80;
pub(super) const SURF_HINT: i32 = 0x100;
pub(super) const SURF_SKIP: i32 = 0x200;
pub(super) const SKIP_FLAGS: i32 = SURF_SKY2D | SURF_SKY | SURF_TRIGGER | SURF_NODRAW | SURF_HINT | SURF_SKIP;
// The lumps read here are under 2 MB on real maps; a crafted one could otherwise claim 512 MB
// unpacked for each, from a few KB of LZMA.
pub(super) const MAX_LUMP: usize = 16 << 20;
/// The lump directory follows the `VBSP` id and version: offset, length, version and id per lump.
pub(super) const LUMP_DIRECTORY: usize = 8;
pub(super) const LUMP_ENTRY: usize = 16;
pub(super) const LUMP_OFFSET: usize = 0;
pub(super) const LUMP_LENGTH: usize = 4;
/// Valve's LZMA lump header: `LZMA`, actual size (u32), packed size (u32), 5 property bytes.
pub(super) const LZMA_MAGIC: &[u8] = b"LZMA";
pub(super) const LZMA_ACTUAL_SIZE: usize = 4;
pub(super) const LZMA_PROPERTIES: usize = 12;
pub(super) const LZMA_HEADER: usize = 17;
/// The sizes of the fields read.
pub(super) const F32_SIZE: usize = 4;
pub(super) const U16_SIZE: usize = 2;
/// Record sizes and field offsets of the lumps read (`dplane_t`, `dface_t`, …).
pub(super) const PLANE_SIZE: usize = 20;
pub(super) const VERTEX_SIZE: usize = 12;
pub(super) const EDGE_SIZE: usize = 4;
pub(super) const SURFEDGE_SIZE: usize = 4;
pub(super) const TEXINFO_SIZE: usize = 72;
pub(super) const TEXINFO_FLAGS: usize = 64;
pub(super) const TEXINFO_TEXDATA: usize = 68;
pub(super) const TEXDATA_SIZE: usize = 32;
pub(super) const TEXDATA_NAME: usize = 12;
pub(super) const STRING_TABLE_ENTRY: usize = 4;
pub(super) const FACE_SIZE: usize = 56;
pub(super) const FACE_SIDE: usize = 2;
pub(super) const FACE_FIRST_EDGE: usize = 4;
pub(super) const FACE_NUM_EDGES: usize = 8;
pub(super) const FACE_TEXINFO: usize = 10;
pub(super) const FACE_DISPINFO: usize = 12;
pub(super) const PLANE_DIST: usize = 12;
pub(super) const MODEL_SIZE: usize = 48;
pub(super) const MODEL_MINS: usize = 0;
pub(super) const MODEL_MAXS: usize = 12;
pub(super) const MODEL_FIRST_FACE: usize = 40;
pub(super) const MODEL_NUM_FACES: usize = 44;

// Maps come from fastdl and can be broken, so every read is checked: a panic would take down
// the request and poison the lock around map loading.
pub(super) fn bytes<const N: usize>(b: &[u8], o: usize) -> Result<[u8; N]> {
    o.checked_add(N).and_then(|end| b.get(o..end)).and_then(|s| s.try_into().ok()).ok_or_else(|| anyhow!("BSP truncated at {o}"))
}
pub(super) fn u16_at(b: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(b, o)?))
}
pub(super) fn i16_at(b: &[u8], o: usize) -> Result<i16> {
    Ok(i16::from_le_bytes(bytes(b, o)?))
}
pub(super) fn i32_at(b: &[u8], o: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(bytes(b, o)?))
}
pub(super) fn f32_at(b: &[u8], o: usize) -> Result<f32> {
    Ok(f32::from_le_bytes(bytes(b, o)?))
}
/// Three floats in a row: a point or a direction.
pub(super) fn vec3_at(b: &[u8], o: usize) -> Result<Vec3> {
    Ok([f32_at(b, o)?, f32_at(b, o + F32_SIZE)?, f32_at(b, o + 2 * F32_SIZE)?])
}
/// An offset, count or index stored as an i32; negative ones are an error.
pub(super) fn index_at(b: &[u8], o: usize) -> Result<usize> {
    let v = i32_at(b, o)?;
    usize::try_from(v).map_err(|_| anyhow!("negative index {v} at {o}"))
}

/// Where lump `index`'s entry is in the lump directory.
pub(super) fn lump_entry(index: usize) -> usize {
    LUMP_DIRECTORY + LUMP_ENTRY * index
}

pub(super) fn lump(data: &[u8], index: usize) -> Result<Vec<u8>> {
    let h = lump_entry(index);
    let (ofs, len) = (index_at(data, h + LUMP_OFFSET)?, index_at(data, h + LUMP_LENGTH)?);
    let Some(raw) = data.get(ofs..ofs + len) else { bail!("lump {index} out of range") };
    if len > MAX_LUMP {
        bail!("lump {index}: {len} bytes");
    }
    unpack(raw).map_err(|e| anyhow!("lump {index}: {e}"))
}

/// A lump's bytes as stored, unpacked if packed (Valve's LZMA header).
pub(super) fn unpack(raw: &[u8]) -> Result<Vec<u8>> {
    if !raw.starts_with(LZMA_MAGIC) {
        return Ok(raw.to_vec());
    }
    let actual = u32::from_le_bytes(bytes(raw, LZMA_ACTUAL_SIZE)?) as usize;
    if actual > MAX_LUMP {
        bail!("{actual} bytes unpacked");
    }
    let props: [u8; 5] = bytes(raw, LZMA_PROPERTIES)?;
    let mut alone = props.to_vec();
    alone.extend_from_slice(&(actual as u64).to_le_bytes());
    alone.extend_from_slice(&raw[LZMA_HEADER..]);
    let mut out = Vec::with_capacity(actual);
    lzma_rs::lzma_decompress(&mut &alone[..], &mut out).map_err(|e| anyhow!("{e:?}"))?;
    out.truncate(actual);
    Ok(out)
}
