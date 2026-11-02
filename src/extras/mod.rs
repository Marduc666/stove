mod mesh;
mod texture;

use std::io;

use byteorder::{ReadBytesExt, LE};
pub use mesh::*;
pub use texture::*;

/// the full path of a package as names can have a number stored separately
pub fn package_path(name: &unreal_asset::types::FName) -> String {
    match name.get_number() {
        number if number > 0 => format!("{}_{}", name.get_owned_content(), number - 1),
        _ => name.get_owned_content(),
    }
}

/// the largest mip to decode as doing this for every 4k texture of a map takes forever
const MAX_MIP_SIZE: usize = 1024;

/// a bulk data payload in its data resource layout, a payload that isn't anywhere else comes right after its header
fn read_payload<R: io::Read + io::Seek>(
    data: &mut io::Cursor<&[u8]>,
    bulk: Option<&mut R>,
    resource: Option<&unreal_asset::asset::DataResource>,
) -> io::Result<Option<Vec<u8>>> {
    use io::Read;
    let flags = resource.map_or(BulkDataFlags::empty(), |resource| {
        BulkDataFlags::from_bits_truncate(resource.legacy_bulk_data_flags)
    });
    let size = resource.map_or(0, |resource| resource.serial_size.max(0) as u64);
    if !flags.intersects(
        BulkDataFlags::Unused
            | BulkDataFlags::OptionalPayload
            | BulkDataFlags::PayloadInSeperateFile
            | BulkDataFlags::PayloadAtEndOfFile,
    ) {
        let mut inline = Vec::new();
        data.by_ref().take(size).read_to_end(&mut inline)?;
        return Ok((inline.len() as u64 == size && size > 0).then_some(inline));
    }
    if flags.intersects(BulkDataFlags::CompressedZlib | BulkDataFlags::OptionalPayload)
        || !flags.intersects(BulkDataFlags::PayloadInSeperateFile)
    {
        return Ok(None);
    }
    let (Some(bulk), Some(resource)) = (bulk, resource) else {
        return Ok(None);
    };
    bulk.seek(io::SeekFrom::Start(resource.serial_offset as u64))?;
    let mut payload = Vec::new();
    bulk.take(size).read_to_end(&mut payload)?;
    Ok((payload.len() as u64 == size).then_some(payload))
}

/// a mip in its data resource layout, the smallest one that's close to [MAX_MIP_SIZE] is used
///
/// none being usable leaves the reader after the mips
fn read_mip<R: io::Read + io::Seek>(
    data: &mut io::Cursor<&[u8]>,
    mut bulk: Option<&mut R>,
    resources: &[unreal_asset::asset::DataResource],
) -> io::Result<Option<(Vec<u8>, usize, usize)>> {
    for _ in 0..data.read_i32::<LE>()? {
        let resource = usize::try_from(data.read_i32::<LE>()?)
            .ok()
            .and_then(|resource| resources.get(resource));
        let fits = {
            // the size comes after an inline payload so look ahead for it
            let start = data.position();
            let size = resource.map_or(0, |resource| resource.serial_size.max(0) as u64);
            let inline = !BulkDataFlags::from_bits_truncate(
                resource.map_or(0, |resource| resource.legacy_bulk_data_flags),
            )
            .intersects(
                BulkDataFlags::Unused
                    | BulkDataFlags::OptionalPayload
                    | BulkDataFlags::PayloadInSeperateFile
                    | BulkDataFlags::PayloadAtEndOfFile,
            );
            data.set_position(start + if inline { size } else { 0 });
            let x = data.read_i32::<LE>()?.max(0) as usize;
            let y = data.read_i32::<LE>()?.max(0) as usize;
            data.set_position(start);
            (x, y)
        };
        let mip = match fits {
            (1..=MAX_MIP_SIZE, 1..=MAX_MIP_SIZE) => {
                read_payload(data, bulk.as_deref_mut(), resource)?
            }
            _ => {
                read_payload(data, None::<&mut io::Cursor<&[u8]>>, resource)?;
                None
            }
        };
        let x = data.read_i32::<LE>()?.max(0) as usize;
        let y = data.read_i32::<LE>()?.max(0) as usize;
        // z
        data.read_i32::<LE>()?;
        if let Some(mip) = mip {
            return Ok(Some((mip, x, y)));
        }
    }
    Ok(None)
}

/// an array of u32s, the count can't be trusted to reserve memory with
fn read_u32s(data: &mut io::Cursor<&[u8]>) -> io::Result<Vec<u32>> {
    let mut array = Vec::new();
    for _ in 0..data.read_i32::<LE>()? {
        array.push(data.read_u32::<LE>()?);
    }
    Ok(array)
}

fn read_fstring(data: &mut io::Cursor<&[u8]>) -> io::Result<String> {
    use io::Read;
    let mut string = match data.read_i32::<LE>()? {
        // no name read with this is anywhere close to this long
        len if !(-64..=64).contains(&len) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid string",
            ));
        }
        len if len.is_negative() => {
            let mut buf = Vec::new();
            for _ in 0..-len {
                buf.push(data.read_u16::<LE>()?);
            }
            String::from_utf16(&buf).unwrap_or_default()
        }
        len => {
            let mut buf = vec![0; len as usize];
            data.read_exact(&mut buf)?;
            String::from_utf8(buf).unwrap_or_default()
        }
    };
    // remove the null byte
    string.pop();
    Ok(string)
}

/// the size in pixels and bytes of the blocks of a format
fn block_size(format: &str) -> Option<(usize, usize)> {
    match format {
        "PF_DXT1" | "PF_BC4" => Some((4, 8)),
        "PF_DXT5" | "PF_BC5" | "PF_BC7" => Some((4, 16)),
        "PF_B8G8R8A8" => Some((1, 4)),
        "PF_G8" => Some((1, 1)),
        _ => None,
    }
}

/// interleaves the bits of a tile's position to get its address
fn morton(x: usize, y: usize) -> usize {
    let spread = |mut v: usize| {
        let mut out = 0;
        let mut bit = 0;
        while v > 0 {
            out |= (v & 1) << (2 * bit);
            v >>= 1;
            bit += 1;
        }
        out
    };
    spread(x) | spread(y) << 1
}

// reference implementation:
// CUE4Parse virtual textures: https://github.com/FabianFG/CUE4Parse/blob/master/CUE4Parse/UE4/Assets/Exports/Texture/FVirtualTextureBuiltData.cs
/// the first layer of a virtual texture's level close to [MAX_MIP_SIZE] put back together from its tiles
///
/// the tile borders are cut off whole blocks at a time so the result is still in the layer's format
fn read_virtual<R: io::Read + io::Seek>(
    data: &mut io::Cursor<&[u8]>,
    mut bulk: Option<&mut R>,
    resources: &[unreal_asset::asset::DataResource],
) -> io::Result<VirtualLevel> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidInput, message.to_string());
    if data.read_u32::<LE>()? == 0 {
        return Err(invalid("no mip of a usable size is available"));
    }
    // cooked
    data.read_u32::<LE>()?;
    let layers = data.read_u32::<LE>()?;
    if !(1..=8).contains(&layers) {
        return Err(invalid("invalid virtual texture layer count"));
    }
    // udim blocks put next to each other, their uvs go past 1
    let blocks_x = data.read_u32::<LE>()?.max(1);
    let blocks_y = data.read_u32::<LE>()?.max(1);
    let tile_size = data.read_u32::<LE>()? as usize;
    let border = data.read_u32::<LE>()? as usize;
    let layer_offsets = read_u32s(data)?;
    let mips = data.read_u32::<LE>()? as usize;
    let width = data.read_u32::<LE>()? as usize;
    let height = data.read_u32::<LE>()? as usize;
    let chunk_per_mip = read_u32s(data)?;
    let base_per_mip = read_u32s(data)?;
    let mut tile_offsets = Vec::new();
    for _ in 0..data.read_i32::<LE>()? {
        let tiles_x = data.read_u32::<LE>()? as usize;
        let tiles_y = data.read_u32::<LE>()? as usize;
        // max address
        data.read_u32::<LE>()?;
        tile_offsets.push((tiles_x, tiles_y, read_u32s(data)?, read_u32s(data)?));
    }
    // tile index per chunk and per mip
    read_u32s(data)?;
    read_u32s(data)?;
    if !read_u32s(data)?.is_empty() {
        return Err(invalid("virtual textures from before UE 5 aren't supported"));
    }
    let mut formats = Vec::new();
    for _ in 0..layers {
        formats.push(read_fstring(data)?);
    }
    // fallback colours
    data.set_position(data.position() + 16 * layers as u64);
    let mut chunks = Vec::new();
    for _ in 0..data.read_i32::<LE>()? {
        // hash
        data.set_position(data.position() + 20);
        // size
        data.read_u32::<LE>()?;
        // codec payload size
        data.read_u32::<LE>()?;
        let mut codec = 0;
        for layer in 0..layers {
            if layer == 0 {
                codec = data.read_u8()?;
            } else {
                data.read_u8()?;
            }
            // codec payload offset
            data.read_u32::<LE>()?;
        }
        let resource = usize::try_from(data.read_i32::<LE>()?)
            .ok()
            .and_then(|resource| resources.get(resource));
        chunks.push((codec, resource, read_payload(data, bulk.as_deref_mut(), resource)?));
    }

    let format = formats.swap_remove(0);
    let Some((block, bytes)) = block_size(&format) else {
        return Err(invalid(&format!("{format}: currently unsupported soz :p")));
    };
    if tile_size == 0 || tile_size % block != 0 || border % block != 0 {
        return Err(invalid("virtual texture tiles don't line up with blocks"));
    }
    let Some(level) = (0..mips.min(tile_offsets.len()))
        .find(|level| width >> level <= MAX_MIP_SIZE && height >> level <= MAX_MIP_SIZE)
    else {
        return Err(invalid("no mip of a usable size is available"));
    };
    let (Some(&chunk), Some(&base), Some(&tile_bytes), Some(&layer_bytes)) = (
        chunk_per_mip.get(level),
        base_per_mip.get(level),
        layer_offsets.last(),
        layer_offsets.first(),
    ) else {
        return Err(invalid("invalid virtual texture tables"));
    };
    // only uncompressed tiles have data to read
    const RAW_GPU: u8 = 4;
    let Some((RAW_GPU, _, Some(chunk))) = chunks.get(chunk as usize) else {
        return Err(invalid("virtual texture tiles aren't stored raw"));
    };
    let (tiles_x, tiles_y, addresses, offsets) = &tile_offsets[level];
    // sizes in blocks
    let x = (width >> level).max(1).div_ceil(block);
    let y = (height >> level).max(1).div_ceil(block);
    let tile = tile_size / block;
    let border = border / block;
    let physical = tile + 2 * border;
    if (layer_bytes as usize) < physical * physical * bytes {
        return Err(invalid("virtual texture tiles are smaller than expected"));
    }
    let mut out = vec![0; x * y * bytes];
    for tile_y in 0..*tiles_y {
        for tile_x in 0..*tiles_x {
            let address = morton(tile_x, tile_y);
            // the tiles are stored in runs that start at these addresses
            let run = addresses.partition_point(|&start| start as usize <= address);
            let Some(run) = run.checked_sub(1) else {
                continue;
            };
            let (start, offset) = (addresses[run] as usize, offsets[run]);
            // no data for these tiles
            if offset == u32::MAX {
                continue;
            }
            let at = base as usize + (offset as usize + address - start) * tile_bytes as usize;
            let Some(source) = chunk.get(at..at + physical * physical * bytes) else {
                return Err(invalid("virtual texture tile is out of bounds"));
            };
            for row in 0..tile {
                let out_y = tile_y * tile + row;
                let out_x = tile_x * tile;
                if out_y >= y || out_x >= x {
                    break;
                }
                let count = tile.min(x - out_x) * bytes;
                let from = ((border + row) * physical + border) * bytes;
                let to = (out_y * x + out_x) * bytes;
                out[to..to + count].copy_from_slice(&source[from..from + count]);
            }
        }
    }
    Ok(VirtualLevel {
        data: out,
        x: x * block,
        y: y * block,
        format,
        uv_scale: [1.0 / blocks_x as f32, 1.0 / blocks_y as f32],
    })
}

struct VirtualLevel {
    data: Vec<u8>,
    x: usize,
    y: usize,
    format: String,
    uv_scale: [f32; 2],
}

#[derive(Default)]
struct StripDataFlags {
    pub global: u8,
    pub class: u8,
}

impl StripDataFlags {
    fn read(data: &mut io::Cursor<&[u8]>) -> Result<Self, io::Error> {
        Ok(Self {
            global: data.read_u8()?,
            class: data.read_u8()?,
        })
    }

    fn editor_data_stripped(&self) -> bool {
        (self.global & 1) != 0
    }

    fn data_stripped_for_server(&self) -> bool {
        (self.global & 2) != 0
    }

    fn class_data_stripped(&self, flag: u8) -> bool {
        (self.class & flag) != 0
    }
}

struct BulkData {
    data: Vec<u8>,
}

impl BulkData {
    fn new<R: io::Read + io::Seek>(
        data: &mut io::Cursor<&[u8]>,
        mut bulk: Option<R>,
        data_offset: i64,
    ) -> io::Result<Self> {
        use io::Read;
        // bulk data flags
        let mut flags = BulkDataFlags::from_bits_truncate(data.read_u32::<LE>()?);
        let len = match flags.intersects(BulkDataFlags::Size64Bit) {
            true => data.read_i64::<LE>()? as usize,
            false => data.read_i32::<LE>()? as usize,
        };
        // size on disk
        match flags.intersects(BulkDataFlags::Size64Bit) {
            true => data.read_u64::<LE>()?,
            false => data.read_u32::<LE>()? as u64,
        };
        let mut file_offset = data.read_u64::<LE>()?;
        if !flags.intersects(BulkDataFlags::NoOffsetFixUp) {
            file_offset = (file_offset as i64 + data_offset) as u64
        }
        if flags.intersects(BulkDataFlags::BadDataVersion) {
            // idk
            data.read_i16::<LE>()?;
            flags &= !BulkDataFlags::BadDataVersion;
        }
        if len > i32::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid bulk data size",
            ));
        }
        let mut buf = vec![0; len];
        match flags {
            flags if len == 0 || flags.intersects(BulkDataFlags::Unused) => (),
            flags if flags.intersects(BulkDataFlags::ForceInlinePayload) => {
                data.read_exact(&mut buf)?;
            }
            flags
                if flags.intersects(
                    BulkDataFlags::OptionalPayload | BulkDataFlags::PayloadInSeperateFile,
                ) =>
            {
                let Some(bulk) = bulk.as_mut() else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "texture is raw",
                    ));
                };
                bulk.seek(io::SeekFrom::Start(file_offset))?;
                bulk.read_exact(&mut buf)?;
            }
            flags if flags.intersects(BulkDataFlags::PayloadAtEndOfFile) => {
                let cur = data.position();
                data.set_position(file_offset);
                data.read_exact(&mut buf)?;
                data.set_position(cur);
            }
            _ => (),
        }
        Ok(Self { data: buf })
    }
}

bitflags::bitflags! {
    struct BulkDataFlags: u32 {
        const PayloadAtEndOfFile = 0x0001;
        const CompressedZlib = 0x0002;
        const Unused = 0x0020;
        const ForceInlinePayload = 0x0040;
        const PayloadInSeperateFile = 0x0100;
        const SerializeCompressedBitWindow = 0x0200;
        const OptionalPayload = 0x0800;
        const Size64Bit = 0x2000;
        const BadDataVersion = 0x8000;
        const NoOffsetFixUp = 0x10000;
    }
}
