use super::Asset;
use unreal_asset::{
    cast,
    error::Error,
    exports::{Export, ExportBaseTrait, ExportNormalTrait},
    properties::{Property, PropertyDataTrait},
    reader::archive_trait::ArchiveTrait,
    types::{fname::FName, PackageIndex, PackageIndexTrait},
};

mod delete;
mod duplicate;
mod transform;
mod transplant;
mod ui;

pub const LOCATION: &str = "RelativeLocation";
pub const ROTATION: &str = "RelativeRotation";
pub const SCALE: &str = "RelativeScale3D";

#[derive(bevy::prelude::Bundle)]
pub struct SelectedBundle {
    selected: Selected,
    outline: bevy_mod_outline::OutlineBundle,
}

impl Default for SelectedBundle {
    fn default() -> Self {
        Self {
            selected: Selected,
            outline: bevy_mod_outline::OutlineBundle {
                outline: bevy_mod_outline::OutlineVolume {
                    visible: true,
                    colour: bevy::prelude::Color::srgb(1.0, 1.0, 0.5),
                    width: 15.0,
                },
                ..Default::default()
            },
        }
    }
}

#[derive(bevy::prelude::Component)]
pub struct Selected;

#[derive(bevy::prelude::Component)]
pub struct Matched;

#[derive(bevy::prelude::Component)]
pub struct Actor {
    pub export: usize,
    transform: usize,
    pub name: String,
    pub display: String,
    pub class: String,
}

impl Actor {
    fn index(&self) -> PackageIndex {
        PackageIndex::new(self.export as i32 + 1)
    }

    pub fn new(asset: &Asset, package: PackageIndex) -> Result<(Option<String>, Self), Error> {
        if package.index == 0 {
            return Err(Error::invalid_package_index(
                "actor was null reference".to_string(),
            ));
        }
        let export = package.index as usize - 1;
        let Some(ex) = asset.get_export(package) else {
            return Err(Error::invalid_package_index(format!(
                "failed to find actor at index {}",
                package.index
            )));
        };
        let Some(norm) = ex.get_normal_export() else {
            return Err(Error::no_data(format!(
                "actor at index {} failed to parse",
                package.index
            )));
        };
        let name = match asset.get_engine_version()
            >= unreal_asset::engine_version::EngineVersion::VER_UE5_1
        {
            true => match norm
                .extras
                .get(8..12)
                .and_then(|len| len.try_into().ok())
                .map(|len| i32::from_le_bytes(len) as usize)
                .and_then(|len| norm.extras.get(12..12 + len))
                .and_then(|name| String::from_utf8(name.to_vec()).ok())
            {
                Some(name) if !name.chars().all(char::is_whitespace) => name,
                _ => norm.base_export.object_name.get_owned_content(),
            },
            false => norm.base_export.object_name.get_owned_content(),
        };
        let display = format!("{name} ({})", export + 1);
        let class = asset
            .get_import(norm.base_export.class_index)
            .map(|import| import.object_name.get_owned_content())
            .unwrap_or_default();
        let path = norm
            .properties
            .iter()
            .find_map(|comp| {
                cast!(Property, ObjectProperty, comp)
                    .filter(|_| comp.get_name() == "StaticMeshComponent")
            })
            .and_then(|i| asset.get_export(i.value))
            .and_then(Export::get_normal_export)
            .and_then(|mesh| {
                mesh.properties.iter().find_map(|mesh| {
                    cast!(Property, ObjectProperty, mesh)
                        .filter(|_| mesh.get_name() == "StaticMesh")
                })
            })
            .and_then(|i| asset.get_import(i.value))
            .and_then(|i| asset.get_import(i.outer_index))
            .map(|path| crate::extras::package_path(&path.object_name));
        // normally these are further back so reversed should be a bit faster
        for prop in norm.properties.iter().rev() {
            match prop.get_name().get_owned_content().as_str() {
                // of course this wouldn't be able to be detected if all transforms were left default
                LOCATION | ROTATION | SCALE => {
                    return Ok((
                        path,
                        Self {
                            export,
                            transform: export,
                            name,
                            display,
                            class,
                        },
                    ))
                }
                "RootComponent" => {
                    if let Property::ObjectProperty(obj) = prop {
                        if obj.value.is_export() {
                            return Ok((
                                path,
                                Self {
                                    export,
                                    transform: obj.value.index as usize - 1,
                                    name,
                                    display,
                                    class,
                                },
                            ));
                        }
                    }
                }
                _ => continue,
            }
        }
        norm.base_export.object_name.get_content(|name| {
            Err(Error::no_data(format!(
                "couldn't find transform component for {name}",
            )))
        })
    }

    /// gets all exports related to the given actor
    fn get_actor_exports(&self, asset: &super::Asset, offset: usize) -> Vec<super::Export> {
        let level = asset
            .asset_data
            .exports
            .iter()
            .find_map(|ex| unreal_asset::cast!(Export, LevelExport, ex))
            .unwrap();
        // get references to all the actor's children
        let mut child_indexes: Vec<PackageIndex> = asset.asset_data.exports[self.export]
            .get_base_export()
            .create_before_serialization_dependencies
            .iter()
            .filter(|dep| dep.is_export())
            // dw PackageIndex is just a wrapper around i32 which is cloned by default anyway
            .cloned()
            .collect();
        let actors: Vec<_> = child_indexes
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(i, child)| level.actors.contains(child).then_some(i))
            .collect();
        for i in actors {
            child_indexes.remove(i);
        }
        // add the top-level actor reference
        child_indexes.insert(0, self.index());

        // get all the exports from those indexes
        let mut children: Vec<_> = child_indexes
            .iter()
            .filter_map(|index| asset.get_export(*index))
            // i'm pretty sure i have to clone here so i can modify then insert data
            .cloned()
            .collect();

        let package_offset = (offset + 1) as i32;
        // update export references to what they will be once added
        for (i, child_index) in child_indexes.into_iter().enumerate() {
            for child in children.iter_mut() {
                on_export_refs(child, |index| {
                    if index == &child_index {
                        index.index = package_offset + i as i32;
                    }
                });
            }
        }
        children
    }
}

/// gets all actor exports within a map (all exports direct children of PersistentLevel)
pub fn get_actors(asset: &Asset) -> Vec<PackageIndex> {
    match asset
        .asset_data
        .exports
        .iter()
        .find_map(|ex| cast!(Export, LevelExport, ex))
    {
        Some(level) => level
            .actors
            .iter()
            .filter(|index| index.is_export())
            .copied()
            .collect(),
        None => Vec::new(),
    }
}

/// creates and assigns a unique name
fn give_unique_name(orig: &mut FName, asset: &mut Asset) {
    // for the cases where the number is unnecessary
    let mut name = orig.get_owned_content();
    if asset.search_name_reference(&name).is_none() {
        *orig = asset.add_fname(&name);
        return;
    }
    let mut id: u16 = match name.rfind(|ch: char| ch.to_digit(10).is_none()) {
        Some(index) if index != name.len() - 1 => {
            name.drain(index + 1..).collect::<String>().parse().unwrap()
        }
        _ => 1,
    };
    while asset
        .search_name_reference(&format!("{}{}", &name, id))
        .is_some()
    {
        id += 1;
    }
    *orig = asset.add_fname(&(name + &id.to_string()))
}

/// on all possible export references
fn on_export_refs(export: &mut super::Export, mut func: impl FnMut(&mut PackageIndex)) {
    if let Some(norm) = export.get_normal_export_mut() {
        for prop in norm.properties.iter_mut() {
            on_prop_refs(prop, &mut func);
        }
    }
    let export = export.get_base_export_mut();
    export
        .create_before_create_dependencies
        .iter_mut()
        .for_each(&mut func);
    export
        .create_before_serialization_dependencies
        .iter_mut()
        .for_each(&mut func);
    export
        .serialization_before_create_dependencies
        .iter_mut()
        .for_each(&mut func);
    func(&mut export.outer_index);
}

fn on_props(prop: &mut Property, func: &mut impl FnMut(&mut Property)) {
    match prop {
        Property::ArrayProperty(arr) => {
            for entry in arr.value.iter_mut() {
                on_props(entry, func);
            }
        }
        Property::MapProperty(map) => {
            for val in map.value.values_mut() {
                on_props(val, func);
            }
        }
        Property::SetProperty(set) => {
            for entry in set.value.value.iter_mut() {
                on_props(entry, func);
            }
            for entry in set.removed_items.value.iter_mut() {
                on_props(entry, func);
            }
        }
        Property::StructProperty(struc) => {
            for entry in struc.value.iter_mut() {
                on_props(entry, func);
            }
        }
        prop => func(prop),
    }
}

/// on any possible references stashed away in properties
fn on_prop_refs(prop: &mut Property, func: &mut impl FnMut(&mut PackageIndex)) {
    on_props(prop, &mut |prop| match prop {
        Property::ObjectProperty(obj) => {
            func(&mut obj.value);
        }
        Property::DelegateProperty(del) => func(&mut del.value.object),
        Property::MulticastDelegateProperty(del) => {
            for delegate in del.value.iter_mut() {
                func(&mut delegate.object)
            }
        }
        Property::MulticastSparseDelegateProperty(del) => {
            for delegate in del.value.iter_mut() {
                func(&mut delegate.object)
            }
        }
        Property::MulticastInlineDelegateProperty(del) => {
            for delegate in del.value.iter_mut() {
                func(&mut delegate.object)
            }
        }
        _ => (),
    })
}

/// opens a map like stove would, set `STOVE_TEST_MAP` and `STOVE_TEST_USMAP` to run this
#[test]
fn opens_map() {
    let (Ok(map), Ok(usmap)) = (std::env::var("STOVE_TEST_MAP"), std::env::var("STOVE_TEST_USMAP"))
    else {
        return;
    };
    super::asset::load_mappings(&usmap).unwrap();
    let asset = super::asset::open(
        &map,
        unreal_asset::engine_version::EngineVersion::VER_UE5_8,
    )
    .unwrap();
    let actors = get_actors(&asset);
    let mut errors = std::collections::BTreeMap::new();
    let mut meshes = 0;
    let mut opened = 0;
    for &actor in &actors {
        match Actor::new(&asset, actor) {
            Ok((mesh, _)) => {
                opened += 1;
                meshes += mesh.is_some() as usize;
            }
            Err(e) => *errors.entry(e.to_string()).or_insert(0) += 1,
        }
    }
    println!(
        "{map}: {opened}/{} actors, {meshes} with meshes, saves identically: {}",
        actors.len(),
        super::asset::preserves(&map, unreal_asset::engine_version::EngineVersion::VER_UE5_8),
    );
    if let Ok(folder) = std::env::var("STOVE_TEST_CONTENT") {
        let content = crate::Content {
            game: std::env::var("STOVE_TEST_GAME").unwrap(),
            folder: folder.into(),
            ..Default::default()
        };
        let paths: std::collections::BTreeSet<_> = actors
            .iter()
            .filter_map(|&actor| Actor::new(&asset, actor).ok()?.0)
            .collect();
        let (mut loaded, mut triangles, mut materials, mut textures) = (0, 0, 0, 0);
        let version = unreal_asset::engine_version::EngineVersion::VER_UE5_8;
        for path in &paths {
            match super::asset::get(&content, None, path, version, |asset, bulk| {
                Ok(crate::extras::get_mesh_info(asset, bulk)?)
            }) {
                Some((positions, indices, _, mats, _)) => {
                    loaded += 1;
                    for mat in mats {
                        let Some(paths) = super::asset::get(&content, None, &mat, version, |mat, _| {
                            Ok(crate::extras::get_tex_paths(mat))
                        }) else {
                            continue;
                        };
                        for path in paths {
                            materials += 1;
                            textures += super::asset::get(&content, None, &path, version, |tex, bulk| {
                                Ok(crate::extras::get_tex_info(tex, bulk)?)
                            })
                            .is_some() as usize;
                        }
                    }
                    triangles += indices.len() / 3;
                    assert!(indices.iter().all(|i| (*i as usize) < positions.len()));
                    assert!(positions.iter().all(|p| p.is_finite() && p.abs().max_element() < 1e5));
                    let (min, max) = positions.iter().fold(
                        (bevy::math::Vec3::MAX, bevy::math::Vec3::MIN),
                        |(min, max), p| (min.min(*p), max.max(*p)),
                    );
                    if std::env::var_os("STOVE_TEST_SIZES").is_some() {
                        println!(
                            "  {:>7} verts {:>7} tris {:>6.1} x {:>6.1} x {:>6.1} m  {}",
                            positions.len(),
                            indices.len() / 3,
                            (max - min).x,
                            (max - min).y,
                            (max - min).z,
                            path.rsplit('/').next().unwrap_or_default()
                        );
                    }
                }
                None => println!("  no mesh {path}"),
            }
        }
        println!("meshes: {loaded}/{} loaded, {triangles} triangles, {textures}/{materials} textures", paths.len());
    }
    let mut errors: Vec<_> = errors.into_iter().collect();
    errors.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (error, count) in errors.iter().take(8) {
        println!("  {count}x {}", error.chars().take(120).collect::<String>());
    }
    assert!(opened > 0);
}

/// reads every mesh and texture below `STOVE_TEST_SWEEP` so a bad file can't take stove down
#[test]
fn reads_all_content() {
    let (Ok(root), Ok(usmap)) = (std::env::var("STOVE_TEST_SWEEP"), std::env::var("STOVE_TEST_USMAP"))
    else {
        return;
    };
    super::asset::load_mappings(&usmap).unwrap();
    let version = unreal_asset::engine_version::EngineVersion::VER_UE5_8;
    let open = |path: &std::path::Path| -> Option<_> {
        std::fs::File::open(path.with_extension("ubulk"))
            .ok()
            .map(std::io::BufReader::new)
            .map(super::Wrapper::File)
    };
    let mut errors = std::collections::BTreeMap::<String, usize>::new();
    let (mut meshes, mut meshes_read, mut textures, mut textures_read) = (0, 0, 0, 0);
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "uasset") {
            continue;
        }
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let is_mesh = name.starts_with("SM_");
        if !is_mesh && !name.starts_with("T_") {
            continue;
        }
        let Ok(asset) = super::asset::read_file(path, version, super::asset::mappings()) else {
            continue;
        };
        let error = match is_mesh {
            true => {
                meshes += 1;
                crate::extras::get_mesh_info(asset, open(path)).map(|_| meshes_read += 1).err()
            }
            false => {
                textures += 1;
                crate::extras::get_tex_info(asset, open(path)).map(|_| textures_read += 1).err()
            }
        };
        if let Some(e) = error {
            *errors.entry(e.to_string()).or_default() += 1;
        }
    }
    println!("sweep: {meshes_read}/{meshes} meshes, {textures_read}/{textures} textures");
    let mut errors: Vec<_> = errors.into_iter().collect();
    errors.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (error, count) in errors.iter().take(12) {
        println!("  {count}x {}", error.chars().take(100).collect::<String>());
    }
}
