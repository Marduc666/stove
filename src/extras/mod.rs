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

/// a mip in its data resource layout, the smallest one that's close to [MAX_MIP_SIZE] is used
fn read_mip<R: io::Read + io::Seek>(
    data: &mut io::Cursor<&[u8]>,
    mut bulk: Option<R>,
    resources: &[unreal_asset::asset::DataResource],
) -> io::Result<(Vec<u8>, usize, usize)> {
    use io::Read;
    for _ in 0..data.read_i32::<LE>()? {
        let resource = usize::try_from(data.read_i32::<LE>()?)
            .ok()
            .and_then(|resource| resources.get(resource));
        let flags = resource.map_or(BulkDataFlags::empty(), |resource| {
            BulkDataFlags::from_bits_truncate(resource.legacy_bulk_data_flags)
        });
        let size = resource.map_or(0, |resource| resource.serial_size.max(0) as u64);
        // a payload that isn't anywhere else comes right after its header
        let mut inline = Vec::new();
        if !flags.intersects(
            BulkDataFlags::Unused
                | BulkDataFlags::OptionalPayload
                | BulkDataFlags::PayloadInSeperateFile
                | BulkDataFlags::PayloadAtEndOfFile,
        ) {
            data.by_ref().take(size).read_to_end(&mut inline)?;
        }
        let x = data.read_i32::<LE>()?.max(0) as usize;
        let y = data.read_i32::<LE>()?.max(0) as usize;
        // z
        data.read_i32::<LE>()?;
        if x > MAX_MIP_SIZE || y > MAX_MIP_SIZE || x == 0 || y == 0 {
            continue;
        }
        if flags.intersects(BulkDataFlags::CompressedZlib) {
            continue;
        }
        if inline.len() as u64 == size && size > 0 {
            return Ok((inline, x, y));
        }
        if let (Some(bulk), Some(resource), true) = (
            bulk.as_mut(),
            resource,
            flags.intersects(BulkDataFlags::PayloadInSeperateFile)
                && !flags.intersects(BulkDataFlags::OptionalPayload),
        ) {
            bulk.seek(io::SeekFrom::Start(resource.serial_offset as u64))?;
            let mut mip = Vec::new();
            bulk.take(size).read_to_end(&mut mip)?;
            if mip.len() as u64 == size {
                return Ok((mip, x, y));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "no mip of a usable size is available",
    ))
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
