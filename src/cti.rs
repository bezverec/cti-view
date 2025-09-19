use anyhow::{anyhow, bail, ensure, Result};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

// --- veřejné typy ---

#[derive(Debug, Clone, Copy)]
pub struct CTIHeader {
    pub magic: [u8; 4],
    pub version: u16,
    pub flags: u16,
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub color_type: u8,
    pub compression: u8,
    pub quality: u8,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum CompressionId {
    None = 0,
    Rle = 1,
    Lz77 = 2,
    Delta = 3,
    Predictive = 4,
    Zstd = 10,
    Lz4 = 11,
    Unknown(u8),
}
impl From<u8> for CompressionId {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Rle,
            2 => Self::Lz77,
            3 => Self::Delta,
            4 => Self::Predictive,
            10 => Self::Zstd,
            11 => Self::Lz4,
            x => Self::Unknown(x),
        }
    }
}
impl CompressionId {
    pub fn as_str(self) -> &'static str {
        match self {
            CompressionId::None => "None",
            CompressionId::Rle => "RLE",
            CompressionId::Lz77 => "LZ77",
            CompressionId::Delta => "Delta",
            CompressionId::Predictive => "Predictive",
            CompressionId::Zstd => "Zstd",
            CompressionId::Lz4 => "LZ4",
            CompressionId::Unknown(_) => "Unknown",
        }
    }

    /// Human-readable description; includes numeric value for Unknown(_).
    pub fn describe(self) -> String {
        match self {
            CompressionId::Unknown(v) => format!("Unknown({v})"),
            other => other.as_str().to_string(),
        }
    }
}

pub struct CTIDecoder;

impl CTIDecoder {
    /// Načte pouze hlavičku (rychlá kontrola metadat).
    pub fn info<P: AsRef<Path>>(path: P) -> Result<CTIHeader> {
        let mut br = BufReader::new(File::open(path)?);
        read_header(&mut br)
    }

    /// Dekóduje celý obrázek do RAW bufferu (interleaved) a vrátí (header, data).
    pub fn decode_file<P: AsRef<Path>>(path: P) -> Result<(CTIHeader, Vec<u8>)> {
        let p = path.as_ref();
        let mut f = BufReader::new(File::open(p)?);

        let hdr = read_header(&mut f)?;
        ensure!(&hdr.magic == b"CTI1", "Bad magic");

        // Index dlaždic
        let total_tiles = (hdr.tiles_x * hdr.tiles_y) as usize;
        let indices = read_indices(&mut f, total_tiles)?;

        // bpp z color_type
        let bpp = match hdr.color_type {
            1 => 1u32, // L8
            2 => 2u32, // L16
            3 => 3u32, // RGB8
            4 => 4u32, // RGBA8
            5 => 6u32, // RGB16
            _ => bail!("Unsupported color type id {}", hdr.color_type),
        };

        let mut out = vec![0u8; (hdr.width * hdr.height * bpp) as usize];
        let use_rct = (hdr.flags & 1) != 0 && matches!(hdr.color_type, 3 | 5);

        // Přímé čtení komprimovaných dlaždic
        let mut file = f.into_inner();
        for (i, t) in indices.iter().enumerate() {
            file.seek(SeekFrom::Start(t.offset))?;
            let mut comp = vec![0u8; t.compressed_size as usize];
            file.read_exact(&mut comp)?;

            let mut tile = decompress_tile_with_size(hdr.compression, &comp, t.original_size as usize)?;
            ensure!(crc32(&tile) == t.crc32, "CRC mismatch at tile {}", i);

            if use_rct {
                match hdr.color_type {
                    3 => rct_inverse_rgb8(&mut tile),
                    5 => rct_inverse_rgb16(&mut tile),
                    _ => {}
                }
            }

            let tx = (i as u32) % hdr.tiles_x;
            let ty = (i as u32) / hdr.tiles_x;
            blit_tile(
                &mut out,
                &tile,
                hdr.width,
                hdr.height,
                hdr.tile_size,
                bpp as u32,
                tx,
                ty,
            )?;
        }

        Ok((hdr, out))
    }
}

// --- interní formát / IO ---

#[derive(Debug, Clone, Copy)]
struct TileIndex {
    offset: u64,
    compressed_size: u32,
    original_size: u32,
    crc32: u32,
}

fn read_header<R: Read>(r: &mut R) -> Result<CTIHeader> {
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    let version = read_u16_le(r)?;
    let flags = read_u16_le(r)?;
    let width = read_u32_le(r)?;
    let height = read_u32_le(r)?;
    let tile_size = read_u32_le(r)?;
    let tiles_x = read_u32_le(r)?;
    let tiles_y = read_u32_le(r)?;
    let color_type = read_u8(r)?;
    let compression = read_u8(r)?;
    let quality = read_u8(r)?;
    // reserved 33B
    let mut _reserved = [0u8; 33];
    r.read_exact(&mut _reserved)?;
    Ok(CTIHeader {
        magic,
        version,
        flags,
        width,
        height,
        tile_size,
        tiles_x,
        tiles_y,
        color_type,
        compression,
        quality,
    })
}

fn read_indices<R: Read>(r: &mut R, n: usize) -> Result<Vec<TileIndex>> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = read_u64_le(r)?;
        let compressed_size = read_u32_le(r)?;
        let original_size = read_u32_le(r)?;
        let crc32 = read_u32_le(r)?;
        v.push(TileIndex {
            offset,
            compressed_size,
            original_size,
            crc32,
        });
    }
    Ok(v)
}

// --- malé IO utily ---
fn read_u8<R: Read>(r: &mut R) -> Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}
fn read_u16_le<R: Read>(r: &mut R) -> Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn read_u32_le<R: Read>(r: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64_le<R: Read>(r: &mut R) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

// --- skládání dlaždic ---
fn blit_tile(
    out: &mut [u8],
    tile: &[u8],
    w: u32,
    h: u32,
    ts: u32,
    bpp: u32,
    tx: u32,
    ty: u32,
) -> Result<()> {
    let start_x = tx * ts;
    let start_y = ty * ts;
    let end_x = (start_x + ts).min(w);
    let end_y = (start_y + ts).min(h);
    let tile_w = end_x - start_x;
    let tile_h = end_y - start_y;

    for row in 0..tile_h {
        let dst_off = (((start_y + row) * w + start_x) * bpp) as usize;
        let src_off = (row * tile_w * bpp) as usize;
        let len = (tile_w * bpp) as usize;
        out[dst_off..dst_off + len].copy_from_slice(&tile[src_off..src_off + len]);
    }
    Ok(())
}

// --- dekomprese + jednoduché RCT inverse ---
fn decompress_tile_with_size(kind: u8, comp: &[u8], original_size: usize) -> Result<Vec<u8>> {
    match CompressionId::from(kind) {
        CompressionId::None => Ok(comp.to_vec()),
        CompressionId::Zstd => zstd::bulk::decompress(comp, original_size)
            .map_err(|e| anyhow!("zstd decompress failed: {e}")),
        CompressionId::Lz4 => lz4_flex::block::decompress_size_prepended(comp).map_err(|e| anyhow!(e)),
        // viewer je minimalistický – ostatní módy nepodporujeme
        other => bail!("Unsupported compression in viewer: {}", other.as_str()),
    }
}

fn rct_inverse_rgb8(buf: &mut [u8]) {
    for p in buf.chunks_exact_mut(3) {
        let y = p[0] as i32;
        let cb = (p[1] as i8) as i32;
        let cr = (p[2] as i8) as i32;
        let g = y - ((cb + cr) >> 2);
        let r = cr + g;
        let b = cb + g;
        p[0] = r.clamp(0, 255) as u8;
        p[1] = g.clamp(0, 255) as u8;
        p[2] = b.clamp(0, 255) as u8;
    }
}
fn rct_inverse_rgb16(buf: &mut [u8]) {
    for p in buf.chunks_exact_mut(6) {
        let y = u16::from_le_bytes([p[0], p[1]]) as i32;
        let cb = (u16::from_le_bytes([p[2], p[3]]) as i16) as i32;
        let cr = (u16::from_le_bytes([p[4], p[5]]) as i16) as i32;
        let g = y - ((cb + cr) >> 2);
        let r = cr + g;
        let b = cb + g;
        p[0..2].copy_from_slice(&(r.clamp(0, 65535) as u16).to_le_bytes());
        p[2..4].copy_from_slice(&(g.clamp(0, 65535) as u16).to_le_bytes());
        p[4..6].copy_from_slice(&(b.clamp(0, 65535) as u16).to_le_bytes());
    }
}

// --- CRC32 ---
fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc32_table();
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        let idx = ((crc ^ (b as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ TABLE[idx];
    }
    !crc
}
const fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            c = if (c & 1) != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}
