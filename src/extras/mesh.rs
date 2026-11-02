use super::*;
use unreal_asset::{
    cast,
    engine_version::EngineVersion,
    exports::{ExportBaseTrait, ExportNormalTrait},
    object_version::ObjectVersion,
    properties::{Property, PropertyDataTrait},
    reader::archive_trait::ArchiveTrait,
};

// currently doesn't read bulk data but we'll get there
#[test]
fn parse_mesh() -> Result<(), unreal_asset::error::Error> {
    let parse = |asset, bulk, name: &str, version| {
        use obj_exporter::*;
        let (verts, indices, ..) = get_mesh_info(unreal_asset::Asset::new(
            io::Cursor::new(asset),
            Some(io::Cursor::new(bulk)),
            version,
            None,
        )?, None::<io::Cursor<&[u8]>>)?;
        export_to_file(
            &ObjSet {
                material_library: None,
                objects: vec![Object {
                    name: name.to_string(),
                    vertices: verts
                        .into_iter()
                        .map(|bevy::math::Vec3 { x, y, z }| Vertex {
                            x: x as f64,
                            y: y as f64,
                            z: z as f64,
                        })
                        .collect(),
                    tex_vertices: vec![],
                    normals: vec![],
                    geometry: vec![Geometry {
                        material_name: None,
                        shapes: indices
                            .chunks(3)
                            .map(|face| Shape {
                                primitive: Primitive::Triangle(
                                    (face[0] as usize, None, None),
                                    (face[1] as usize, None, None),
                                    (face[2] as usize, None, None),
                                ),
                                groups: vec![],
                                smoothing_groups: vec![],
                            })
                            .collect(),
                    }],
                }],
            },
            format!("{name}.obj"),
        )?;
        Ok::<_, unreal_asset::error::Error>(())
    };
    parse(
        include_bytes!("tests/A02_Outside_Castle.uasset").as_slice(),
        include_bytes!("tests/A02_Outside_Castle.uexp").as_slice(),
        "A02_Outside_Castle",
        EngineVersion::VER_UE4_25,
    )?;
    parse(
        include_bytes!("tests/AirKickPreRoom.uasset").as_slice(),
        include_bytes!("tests/AirKickPreRoom.uexp").as_slice(),
        "AirKickPreRoom",
        EngineVersion::VER_UE5_1,
    )?;
    parse(
        include_bytes!("tests/SM_Cybercity_Hook_End.uasset").as_slice(),
        include_bytes!("tests/SM_Cybercity_Hook_End.uexp").as_slice(),
        "SM_Cybercity_Hook_End",
        EngineVersion::VER_UE4_27,
    )?;
    Ok(())
}

// reference implementations:
// umodel: https://github.com/gildor2/UEViewer/blob/master/Unreal/UnrealMesh/UnMesh4.cpp#L2633
// cue4parse: https://github.com/FabianFG/CUE4Parse/blob/master/CUE4Parse/UE4/Assets/Exports/StaticMesh/UStaticMesh.cs#L13
// CAS UAssetAPI: https://github.com/LongerWarrior/UEAssetToolkitGenerator/blob/master/UAssetApi/ExportTypes/StaticMeshExport.cs#L6
/// parses the extra data of the static mesh export to get render data
pub fn get_mesh_info<C: io::Read + io::Seek, B: io::Read + io::Seek>(
    asset: unreal_asset::Asset<C>,
    bulk: Option<B>,
) -> io::Result<(
    Vec<bevy::math::Vec3>,
    Vec<u32>,
    Vec<Vec<bevy::math::Vec2>>,
    Vec<String>,
    Vec<Section>,
)> {
    // get the static mesh
    let Some(mesh) = asset.asset_data.exports.iter().find(|ex| {
        asset
            .get_import(ex.get_base_export().class_index)
            .map(|import| import.object_name == "StaticMesh")
            .unwrap_or(false)
    }) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "failed to find mesh export",
        ));
    };
    // get the normal export
    let Some(mesh) = mesh.get_normal_export() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "failed to cast mesh data",
        ));
    };
    let mats = mesh
        .properties
        .iter()
        .find(|prop| prop.get_name() == "StaticMaterials")
        .and_then(|prop| cast!(Property, ArrayProperty, prop))
        .map(|arr| {
            arr.value
                .iter()
                // sections refer to these by index so ones that can't be found still need a place
                .map(|prop| {
                    cast!(Property, StructProperty, prop)
                        .and_then(|struc| {
                            struc
                                .value
                                .iter()
                                .find(|prop| prop.get_name() == "MaterialInterface")
                        })
                        .and_then(|inter| cast!(Property, ObjectProperty, inter))
                        .and_then(|obj| asset.get_import(obj.value))
                        .and_then(|imp| asset.get_import(imp.outer_index))
                        .map(|imp| package_path(&imp.object_name))
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let engine = asset.get_engine_version();
    let object = asset.get_object_version();
    let mut data = io::Cursor::new(mesh.extras.as_slice());
    // if this isn't read it breaks
    data.read_i32::<LE>()?;
    if !StripDataFlags::read(&mut data)?.editor_data_stripped()
        // data isn't cooked
        || data.read_u32::<LE>()? == 0
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "mesh is raw"));
    }
    // bodysetup reference
    data.read_u32::<LE>()?;
    // nav collision reference
    if object >= ObjectVersion::VER_UE4_STATIC_MESH_STORE_NAV_COLLISION {
        data.read_u32::<LE>()?;
    }
    // lighting guid
    for _ in 0..16 {
        data.read_u8()?;
    }
    // array of socket references
    for _ in 0..data.read_u32::<LE>()? {
        data.read_u32::<LE>()?;
    }
    // KeepMobileMinLODSettingOnDesktop is not here by default
    /*
    if engine >= EngineVersion::VER_UE4_27 {
        data.read_i32::<LE>()?;
    }
    */
    // array of lod resources
    let resources = asset.data_resources();
    let mut bulk = bulk;
    let mut mat_data = Vec::new();
    for _ in 0..data.read_u32::<LE>()? {
        let flags = StripDataFlags::read(&mut data)?;
        if flags.data_stripped_for_server() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mesh data is stripped for server",
            ));
        }
        mat_data.clear();
        // array of sections
        for _ in 0..data.read_u32::<LE>()? {
            mat_data.push(Section {
                material: data.read_u32::<LE>()?,
                first: data.read_u32::<LE>()?,
                triangles: data.read_u32::<LE>()?,
            });
            // min vertex index
            data.read_u32::<LE>()?;
            // max vertex index
            data.read_u32::<LE>()?;
            // collides
            data.read_u32::<LE>()?;
            // casts shadow
            data.read_u32::<LE>()?;
            // force opaque
            if engine >= EngineVersion::VER_UE4_25 {
                data.read_u32::<LE>()?;
            }
            // visible in ray tracing
            if engine >= EngineVersion::VER_UE4_26 {
                data.read_u32::<LE>()?;
            }
            // affect distance field lighting
            if engine >= EngineVersion::VER_UE5_1 {
                data.read_u32::<LE>()?;
            }
        }
        // source mesh bounds
        if engine >= EngineVersion::VER_UE5_6 {
            data.set_position(data.position() + 7 * 8);
        }
        // max deviation
        data.read_f32::<LE>()?;
        if engine < EngineVersion::VER_UE4_23 {
            if flags.class_data_stripped(2) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "lod data is cooked out",
                ));
            }
            let (positions, indices, uvs) = read_buffers(&mut data, engine, object)?;
            return Ok((positions, indices, uvs, mats, mat_data));
        }
        let cooked_out = data.read_u32::<LE>()? != 0;
        let inlined = data.read_u32::<LE>()? != 0;
        if cooked_out {
            continue;
        }
        // has ray tracing geometry
        if engine >= EngineVersion::VER_UE5_5 {
            data.read_u32::<LE>()?;
        }
        if inlined {
            StripDataFlags::read(&mut data)?;
            let (positions, indices, uvs) = read_buffers(&mut data, engine, object)?;
            return Ok((positions, indices, uvs, mats, mat_data));
        }
        // bulk data with the location of it in the header isn't supported
        if engine < EngineVersion::VER_UE5_2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mesh uses bulk data",
            ));
        }
        let resource = data.read_i32::<LE>()?;
        // the highest detail lod that's available is the best one so try this one first
        if let (Some(bulk), Some(resource)) = (
            bulk.as_mut(),
            usize::try_from(resource)
                .ok()
                .and_then(|resource| resources.get(resource)),
        ) {
            if let Ok(lod) = read_bulk_lod(bulk, resource, engine, object) {
                return Ok((lod.0, lod.1, lod.2, mats, mat_data));
            }
        }
        // the buffers' metadata
        let metadata = 8 + 16 + 8 + 8 + 5 * 2 * 4;
        // raw data header
        let header = match engine >= EngineVersion::VER_UE5_6 {
            true => 6 * 4,
            false => 0,
        };
        // the buffer sizes
        data.set_position(data.position() + metadata + header + 12);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "no lod has any data available",
    ))
}

/// reads the buffers of a lod that was stored in a bulk data file
fn read_bulk_lod<C: io::Read + io::Seek>(
    bulk: &mut C,
    resource: &unreal_asset::asset::DataResource,
    engine: EngineVersion,
    object: ObjectVersion,
) -> io::Result<Buffers> {
    const PAYLOAD_IN_SEPARATE_FILE: u32 = 0x100;
    const COMPRESSED: u32 = 0x2 | 0x10 | 0x80 | 0x1000 | 0x4000;
    if resource.legacy_bulk_data_flags & PAYLOAD_IN_SEPARATE_FILE == 0
        || resource.legacy_bulk_data_flags & COMPRESSED != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bulk data isn't in the bulk data file",
        ));
    }
    bulk.seek(io::SeekFrom::Start(resource.serial_offset as u64))?;
    let mut lod = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(bulk, resource.serial_size as u64), &mut lod)?;
    let mut data = io::Cursor::new(lod.as_slice());
    StripDataFlags::read(&mut data)?;
    read_buffers(&mut data, engine, object)
}

/// a part of a mesh drawn with one material
#[derive(Clone, Copy)]
pub struct Section {
    /// index into the mesh's materials
    pub material: u32,
    /// where its triangles start in the index buffer
    pub first: u32,
    pub triangles: u32,
}

type Buffers = (Vec<bevy::math::Vec3>, Vec<u32>, Vec<Vec<bevy::math::Vec2>>);

/// reads the vertices, uvs and indices of a lod
fn read_buffers(
    mut data: &mut io::Cursor<&[u8]>,
    engine: EngineVersion,
    object: ObjectVersion,
) -> io::Result<Buffers> {
    // position buffer
    // stride
    data.read_u32::<LE>()?;
    // vertex count
    data.read_u32::<LE>()?;
    // size
    data.read_u32::<LE>()?;
    let mut positions = Vec::new();
    for _ in 0..(data.read_u32::<LE>()? as usize) {
        let (x, y, z) = (
            data.read_f32::<LE>()?,
            data.read_f32::<LE>()?,
            data.read_f32::<LE>()?,
        );
        positions.push(bevy::math::vec3(x, z, y) * 0.01);
    }

    // vertex buffer
    if match object >= ObjectVersion::VER_UE4_STATIC_SKELETAL_MESH_SERIALIZATION_FIX {
        true => StripDataFlags::read(&mut data)?,
        false => StripDataFlags::default(),
    }
    .data_stripped_for_server()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "vertex buffer is stripped",
        ));
    }
    let num_tex_coords = data.read_u32::<LE>()?;
    // stride
    if engine < EngineVersion::VER_UE4_19 {
        data.read_u32::<LE>()?;
    }
    // num verts
    let num_verts = data.read_u32::<LE>()?;
    let precise_uvs = data.read_u32::<LE>()? == 1;
    let precise_tangents = engine >= EngineVersion::VER_UE4_12 && data.read_u32::<LE>()? == 1;
    fn read_tangents(
        data: &mut io::Cursor<&[u8]>,
        precise_tangents: bool,
    ) -> Result<(), io::Error> {
        match precise_tangents {
            true => {
                for _ in 0..2 {
                    for _ in 0..4 {
                        data.read_u16::<LE>()?;
                    }
                }
            }
            false => {
                for _ in 0..2 {
                    data.read_u32::<LE>()?;
                }
            }
        }
        Ok(())
    }
    fn half_to_single(half: u16) -> f32 {
        use std::mem::transmute;
        const SHIFTED: u32 = 0x7C00 << 13;
        const MAGIC: f32 = unsafe { transmute::<u32, f32>(113 << 23) };
        let mut single = (half as u32 & 0x7FFF) << 13;
        let exp = SHIFTED & single;
        single += (127 - 15) << 23;
        match exp {
            exp if exp == SHIFTED => single += (128 - 16) << 23,
            0 => {
                single += 1 << 23;
                single = (f32::from_bits(single) - MAGIC).to_bits();
            }
            _ => (),
        }
        single |= (half as u32 & 0x8000) << 16;
        f32::from_bits(single)
    }
    fn read_tex_coords(
        data: &mut io::Cursor<&[u8]>,
        num_tex_coords: u32,
        precise_uvs: bool,
    ) -> Result<Vec<bevy::math::Vec2>, io::Error> {
        let mut uvs = Vec::new();
        for _ in 0..num_tex_coords {
            uvs.push(
                match precise_uvs {
                    true => (data.read_f32::<LE>()?, data.read_f32::<LE>()?),
                    false => (
                        half_to_single(data.read_u16::<LE>()?),
                        half_to_single(data.read_u16::<LE>()?),
                    ),
                }
                .into(),
            )
        }
        Ok(uvs)
    }

    let mut uvs = Vec::new();
    match engine >= EngineVersion::VER_UE4_20 {
        true => {
            // item size
            data.read_u32::<LE>()?;
            // item count
            data.read_u32::<LE>()?;
            // packed normals
            for _ in 0..num_verts {
                read_tangents(&mut data, precise_tangents)?;
            }
            // item size
            data.read_u32::<LE>()?;
            // item count
            data.read_u32::<LE>()?;
            // mesh uv
            for _ in 0..num_verts {
                uvs.push(read_tex_coords(&mut data, num_tex_coords, precise_uvs)?);
            }
        }
        false => {
            // size
            data.read_u32::<LE>()?;
            // length
            data.read_u32::<LE>()?;
            for _ in 0..num_verts {
                read_tangents(&mut data, precise_tangents)?;
                uvs.push(read_tex_coords(&mut data, num_tex_coords, precise_uvs)?);
            }
        }
    }

    // color vertex buffer
    if match object >= ObjectVersion::VER_UE4_STATIC_SKELETAL_MESH_SERIALIZATION_FIX {
        true => StripDataFlags::read(&mut data)?,
        false => StripDataFlags::default(),
    }
    .data_stripped_for_server()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "colour data is stripped",
        ));
    }
    // stride
    data.read_u32::<LE>()?;
    // when num verts is 0 array isn't serialised
    if data.read_u32::<LE>()? > 0 {
        // size
        data.read_u32::<LE>()?;
        // vertex colours
        for _ in 0..data.read_u32::<LE>()? {
            data.read_i32::<LE>()?;
        }
    }

    let indices = match object >= ObjectVersion::VER_UE4_SUPPORT_32BIT_STATIC_MESH_INDICES {
        true => {
            let x32 = data.read_u32::<LE>()? == 1;
            // size
            data.read_u32::<LE>()?;
            match x32 {
                true => {
                    let mut indices = Vec::new();
                    for _ in 0..(data.read_u32::<LE>()? as usize / 4) {
                        indices.push(data.read_u32::<LE>()?);
                    }
                    indices
                }
                false => {
                    let mut indices = Vec::new();
                    for _ in 0..(data.read_u32::<LE>()? as usize / 2) {
                        indices.push(data.read_u16::<LE>()? as u32);
                    }
                    indices
                }
            }
        }
        false => {
            // size
            data.read_u32::<LE>()?;
            let mut indices = Vec::new();
            for _ in 0..(data.read_u32::<LE>()? as usize) {
                indices.push(data.read_u16::<LE>()? as u32);
            }
            indices
        }
    };
    Ok((positions, indices, uvs))
}
