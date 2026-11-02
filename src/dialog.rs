use unreal_asset::exports::ExportBaseTrait;

use super::*;

pub fn open(
    trigger: Trigger<triggers::Open>,
    commands: Commands,
    actors: Query<Entity, With<actor::Actor>>,
    mut notif: EventWriter<Notif>,
    appdata: ResMut<AppData>,
    client: ResMut<Client>,
    map: NonSendMut<Map>,
    registry: ResMut<Registry>,
    meshes: ResMut<Assets<Mesh>>,
    materials: ResMut<Assets<unlit::Unlit>>,
    images: ResMut<Assets<Image>>,
    content: Res<Content>,
    consts: Res<Constants>,
) {
    let Some(path) = trigger.event().0.clone().or_else(|| {
        rfd::FileDialog::new()
            .set_title("open map")
            .add_filter("maps", &["umap"])
            .pick_file()
    }) else {
        return;
    };
    let asset = match asset::open(&path, appdata.version()) {
        Ok(asset) => asset,
        Err(e) => {
            notif.send(Notif {
                message: e.to_string(),
                kind: Error,
            });
            return;
        }
    };
    if !asset::preserves(&path, appdata.version()) {
        notif.send(Notif {
            message: "saving this map would change data stove can't fully read - keep a backup"
                .into(),
            kind: Warning,
        });
    }
    let name: Option<String> = path.to_str().map(|s| s.into());
    open_asset(
        name.as_deref(),
        Some(path),
        asset,
        commands,
        actors,
        notif,
        appdata,
        client,
        map,
        registry,
        meshes,
        materials,
        images,
        content,
        consts,
    );
}

pub fn from_content(
    trigger: Trigger<triggers::FromContent>,
    commands: Commands,
    actors: Query<Entity, With<actor::Actor>>,
    mut notif: EventWriter<Notif>,
    appdata: ResMut<AppData>,
    client: ResMut<Client>,
    map: NonSendMut<Map>,
    registry: ResMut<Registry>,
    meshes: ResMut<Assets<Mesh>>,
    materials: ResMut<Assets<unlit::Unlit>>,
    images: ResMut<Assets<Image>>,
    content: Res<Content>,
    consts: Res<Constants>,
) {
    let name = &trigger.event().0;
    match &trigger.event().1 {
        GamePath::Loose(path) => {
            let asset = match asset::open(path, appdata.version()) {
                Ok(asset) => asset,
                Err(e) => {
                    notif.send(Notif {
                        message: e.to_string(),
                        kind: Error,
                    });
                    return;
                }
            };
            if !asset::preserves(&path, appdata.version()) {
                notif.send(Notif {
                    message: "saving this map would change data stove can't fully read - keep a backup"
                        .into(),
                    kind: Warning,
                });
            }
            open_asset(
                Some(name),
                None,
                asset,
                commands,
                actors,
                notif,
                appdata,
                client,
                map,
                registry,
                meshes,
                materials,
                images,
                content,
                consts,
            );
        }
        GamePath::Packed(path) => {
            let Some(asset) = content.paks.iter().find_map(|(pak_file, pak)| {
                let pak_file = &mut std::io::BufReader::new(std::fs::File::open(pak_file).ok()?);
                match unreal_asset::Asset::new(
                    super::Wrapper::Bytes(std::io::Cursor::new(pak.get(path, pak_file).ok()?)),
                    pak.get(&path.replace(".umap", ".uexp"), pak_file)
                        .ok()
                        .map(std::io::Cursor::new)
                        .map(super::Wrapper::Bytes),
                    appdata.version(),
                    asset::mappings(),
                ) {
                    Ok(asset) => Some(asset),
                    Err(e) => {
                        notif.send(Notif {
                            message: e.to_string(),
                            kind: Error,
                        });
                        None
                    }
                }
            }) else {
                return;
            };
            open_asset(
                Some(name),
                None,
                asset,
                commands,
                actors,
                notif,
                appdata,
                client,
                map,
                registry,
                meshes,
                materials,
                images,
                content,
                consts,
            );
        }
    }
}

fn open_asset(
    name: Option<&str>,
    path: Option<std::path::PathBuf>,
    asset: super::Asset,
    mut commands: Commands,
    actors: Query<Entity, With<actor::Actor>>,
    mut notif: EventWriter<Notif>,
    appdata: ResMut<AppData>,
    mut client: ResMut<Client>,
    mut map: NonSendMut<Map>,
    mut registry: ResMut<Registry>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<unlit::Unlit>>,
    mut images: ResMut<Assets<Image>>,
    content: Res<Content>,
    consts: Res<Constants>,
) {
    use discord_rich_presence::DiscordIpc;
    if let (Some(client), Some(name)) = (client.0.as_mut(), name) {
        let _ = client.set_activity(
            activity()
                .details("currently editing:")
                .state(name.split('\\').last().unwrap_or_default()),
        );
    }
    for actor in actors.iter() {
        commands.entity(actor).despawn_recursive();
    }
    let cache = config()
        .filter(|_| appdata.cache)
        .map(|path| path.join("cache"));
    let version = appdata.version();
    let mut batch = std::collections::BTreeMap::<_, Vec<_>>::new();
    let mut export_names: Vec<_> = asset
        .asset_data
        .exports
        .iter()
        .map(|ex| ex.get_base_export().object_name.get_owned_content())
        .collect();
    let map_actors = actor::get_actors(&asset);
    let actor_count = map_actors.len();
    let mut hidden_actors = 0;
    let mut missing_meshes = 0;
    for i in map_actors {
        let (path, actor) = match actor::Actor::new(&asset, i) {
            Ok(actor) => actor,
            Err(_) => {
                hidden_actors += 1;
                continue;
            }
        };
        export_names[i.index as usize - 1] = actor.name.clone();
        match batch.get_mut(&path) {
            Some(vec) => vec.push(actor),
            None => {
                batch.insert(path, vec![actor]);
            }
        }
    }
    let keys = batch.keys().flatten().cloned().collect::<Vec<_>>();
    std::thread::scope(|s| {
        let threads: Vec<_> = keys
            .into_iter()
            .map(|path| {
                s.spawn(|| {
                    // capture path
                    let path = path;
                    match asset::get(&content, cache.as_deref(), &path, version, |asset, bulk| {
                        Ok(extras::get_mesh_info(asset, bulk)?)
                    }) {
                        Some((positions, indices, uvs, mats, sections)) => {
                            let mut mesh = Mesh::new(
                                bevy::render::render_resource::PrimitiveTopology::TriangleList,
                                default(),
                            )
                            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
                            .with_inserted_attribute(
                                Mesh::ATTRIBUTE_UV_0,
                                uvs.into_iter().map(|uv| uv[0]).collect::<Vec<_>>(),
                            )
                            .with_inserted_indices(bevy::render::mesh::Indices::U32(indices));
                            // only used for shading so they don't need to be the real ones
                            mesh.compute_smooth_normals();
                            Ok((path, mesh, mats, sections))
                        }
                        None => Err(path),
                    }
                })
            })
            .collect();
        // textures read for this map, kept until all of its meshes are done
        let mut decoded = std::collections::HashMap::new();
        for thread in threads {
            match thread.join() {
                Ok(Ok((path, mut mesh, mats, sections))) => {
                    // the materials in the order their sections come in
                    let mut used: Vec<u32> = Vec::new();
                    for section in &sections {
                        if !used.contains(&section.material) {
                            used.push(section.material);
                        }
                    }
                    if used.is_empty() && !mats.is_empty() {
                        used.push(0);
                    }
                    // hard to multithread material loading since the material might not parse
                    let textures: Vec<_> = used
                        .iter()
                        .map(|&material| {
                            let mat = mats.get(material as usize).filter(|_| appdata.textures)?;
                            if let Some(choice) = registry.choices.get(mat) {
                                return choice.clone();
                            }
                            let name = mat.rsplit('/').next().unwrap_or_default();
                            if name.to_ascii_lowercase().contains("glass") {
                                return Some((GLASS.to_string(), [1.0; 2]));
                            }
                            // material instances without textures of their own use their parent's
                            let mut choice = None;
                            let mut current = mat.clone();
                            for _ in 0..4 {
                                let Some((paths, parent)) =
                                    asset::get(&content, cache.as_deref(), &current, version, |mat, _| {
                                        Ok((extras::get_tex_paths(&mat), extras::get_parent_material(&mat)))
                                    })
                                else {
                                    break;
                                };
                                choice = paths.into_iter().find_map(|path| {
                                    let texture = decoded
                                        .entry(path.clone())
                                        .or_insert_with(|| decode(&content, cache.as_deref(), &path, version));
                                    texture.as_ref().map(|texture| (path, texture.3))
                                });
                                match parent {
                                    Some(parent) if choice.is_none() => current = parent,
                                    _ => break,
                                }
                            }
                            registry.choices.insert(mat.clone(), choice.clone());
                            choice
                        })
                        .collect();
                    // textures made of several udim blocks need their uvs scaled
                    // and the layer a vertex's texture is in goes in its second uvs
                    if textures.len() > 1
                        || textures.iter().flatten().any(|(_, scale)| *scale != [1.0; 2])
                    {
                        let mut indices: Vec<u32> = mesh
                            .indices()
                            .map(|indices| indices.iter().map(|i| i as u32).collect())
                            .unwrap_or_default();
                        let count = mesh.count_vertices();
                        let mut layers: Vec<Option<usize>> = vec![None; count];
                        // a vertex used by sections with different textures is copied for each of them
                        // as the layer would otherwise blend across its triangles
                        let mut copies = Vec::new();
                        let mut copy_of = std::collections::HashMap::new();
                        for section in &sections {
                            let Some(layer) = used.iter().position(|&material| material == section.material) else {
                                continue;
                            };
                            let first = (section.first as usize).min(indices.len());
                            let last = (first + section.triangles as usize * 3).min(indices.len());
                            for index in &mut indices[first..last] {
                                let vertex = *index as usize;
                                match layers.get(vertex).copied() {
                                    Some(None) => layers[vertex] = Some(layer),
                                    Some(Some(existing)) if existing != layer => {
                                        *index = *copy_of.entry((vertex, layer)).or_insert_with(|| {
                                            copies.push(vertex);
                                            layers.push(Some(layer));
                                            (layers.len() - 1) as u32
                                        });
                                    }
                                    _ => (),
                                }
                            }
                        }
                        if !copies.is_empty() {
                            use bevy::render::mesh::VertexAttributeValues as Values;
                            for attribute in [Mesh::ATTRIBUTE_POSITION, Mesh::ATTRIBUTE_NORMAL, Mesh::ATTRIBUTE_UV_0] {
                                match mesh.attribute_mut(attribute) {
                                    Some(Values::Float32x3(values)) => {
                                        for &vertex in &copies {
                                            values.push(values[vertex]);
                                        }
                                    }
                                    Some(Values::Float32x2(values)) => {
                                        for &vertex in &copies {
                                            values.push(values[vertex]);
                                        }
                                    }
                                    _ => (),
                                }
                            }
                            mesh.insert_indices(bevy::render::mesh::Indices::U32(indices));
                        }
                        if let Some(bevy::render::mesh::VertexAttributeValues::Float32x2(uvs)) =
                            mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0)
                        {
                            for (uv, layer) in uvs.iter_mut().zip(&layers) {
                                if let Some(Some((_, scale))) = layer.map(|layer| &textures[layer]) {
                                    uv[0] *= scale[0];
                                    uv[1] *= scale[1];
                                }
                            }
                        }
                        mesh.insert_attribute(
                            Mesh::ATTRIBUTE_UV_1,
                            layers
                                .into_iter()
                                .map(|layer| [layer.unwrap_or_default() as f32, 0.0])
                                .collect::<Vec<_>>(),
                        );
                    }
                    // meshes with the same textures share a material
                    let key = textures
                        .iter()
                        .any(Option::is_some)
                        .then(|| {
                            textures
                                .iter()
                                .map(|texture| texture.as_ref().map_or("", |(path, _)| path.as_str()))
                                .collect::<Vec<_>>()
                                .join("|")
                        });
                    if let Some(key) = key.as_ref().filter(|key| !registry.mats.contains_key(*key)) {
                        let layers = textures
                            .iter()
                            .map(|texture| {
                                texture
                                    .as_ref()
                                    .and_then(|(path, _)| {
                                        if path == GLASS {
                                            return Some((1, 1, vec![230, 220, 205, 255]));
                                        }
                                        decoded
                                            .entry(path.clone())
                                            .or_insert_with(|| decode(&content, cache.as_deref(), path, version))
                                            .as_ref()
                                            .map(|(width, height, data, _)| (*width, *height, data.clone()))
                                    })
                                    .unwrap_or_else(unlit::grid)
                            })
                            .collect();
                        registry.mats.insert(
                            key.clone(),
                            materials.add(unlit::Unlit {
                                texture: images.add(unlit::layered(layers)),
                            }),
                        );
                    }
                    registry.meshes.insert(path.clone(), (meshes.add(mesh), key));
                }
                Ok(Err(path)) => {
                    missing_meshes += 1;
                    // these can still be selected and moved as cubes
                    if let Some(mut actors) = batch.remove(&Some(path)) {
                        batch.entry(None).or_default().append(&mut actors);
                    }
                }
                Err(_) => continue,
            }
        }
    });
    // maps can have thousands of these so they're not worth a notification each
    if hidden_actors > 0 {
        notif.send(Notif {
            message: format!(
                "{hidden_actors} of {actor_count} actors can't be shown as they have no transform or couldn't be read"
            ),
            kind: Warning,
        });
    }
    if missing_meshes > 0 {
        notif.send(Notif {
            message: format!("{missing_meshes} meshes couldn't be loaded so their actors are shown as cubes"),
            kind: Warning,
        });
    }
    for (path, actors) in batch {
        match path {
            Some(ref path) => {
                let (mesh, material) = &registry.meshes[path];
                for actor in actors {
                    commands.spawn((
                        MaterialMeshBundle {
                            mesh: mesh.clone_weak(),
                            material: material
                                .as_ref()
                                .map(|mat| registry.mats[mat].clone_weak())
                                .unwrap_or(consts.grid.clone_weak()),
                            transform: actor.transform(&asset),
                            ..default()
                        },
                        bevy_mod_raycast::deferred::RaycastMesh::<()>::default(),
                        actor,
                    ));
                }
            }
            None => {
                for actor in actors {
                    commands
                        .spawn((
                            MaterialMeshBundle {
                                mesh: consts.cube.clone_weak(),
                                material: consts.unselected.clone_weak(),
                                transform: actor.transform(&asset),
                                ..default()
                            },
                            bevy::pbr::wireframe::NoWireframe,
                            actor,
                        ))
                        .with_children(|parent| {
                            parent.spawn((
                                consts.bounds.clone_weak(),
                                SpatialBundle {
                                    visibility: Visibility::Hidden,
                                    ..default()
                                },
                                bevy_mod_raycast::deferred::RaycastMesh::<()>::default(),
                            ));
                        });
                }
                continue;
            }
        }
    }
    let import_names = asset
        .imports
        .iter()
        .map(|ex| ex.object_name.get_owned_content())
        .collect();
    map.0 = Some((asset, path, export_names, import_names));
    notif.send(Notif {
        message: "map opened".into(),
        kind: Success,
    });
}

/// a colour texture's size, pixels and uv scale, normal maps aren't used
fn decode(
    content: &Content,
    cache: Option<&std::path::Path>,
    path: &str,
    version: unreal_asset::engine_version::EngineVersion,
) -> Option<(u32, u32, Vec<u8>, [f32; 2])> {
    match asset::get(content, cache, path, version, |tex, bulk| {
        Ok(extras::get_tex_info(tex, bulk)?)
    }) {
        Some((false, width, height, data, uv_scale)) => Some((width, height, data, uv_scale)),
        _ => None,
    }
}

/// stands in for the texture of glass, whose textures are smudges and droplets meant to be seen through
const GLASS: &str = "glass";

pub fn save_as(
    trigger: Trigger<triggers::SaveAs>,
    mut notif: EventWriter<Notif>,
    appdata: Res<AppData>,
    mut map: NonSendMut<Map>,
) {
    let Some((map, path, ..)) = &mut map.0 else {
        notif.send(Notif {
            message: "no map to save".into(),
            kind: Error,
        });
        return;
    };
    if trigger.event().0 || path.is_none() {
        if let Some(new) = rfd::FileDialog::new()
            .set_title("save map as")
            .add_filter("maps", &["umap"])
            .save_file()
        {
            *path = Some(new);
        }
    }
    let Some(path) = path else { return };
    match asset::save(map, path) {
        Ok(_) => {
            // literally no idea why std::process::Command doesn't work
            #[cfg(target_os = "windows")]
            const PATH: &str = "./script.bat";
            #[cfg(not(target_os = "windows"))]
            const PATH: &str = "./script.sh";
            for line in appdata.script.lines() {
                if let Err(e) = std::fs::write(PATH, line) {
                    notif.send(Notif {
                        message: format!("failed to make save script: {e}"),
                        kind: Error,
                    });
                }
                match std::process::Command::new(PATH)
                    .stdout(std::process::Stdio::piped())
                    .output()
                {
                    Ok(out) => notif.send(Notif {
                        message: String::from_utf8(out.stdout).unwrap_or_default(),
                        kind: Success,
                    }),
                    Err(e) => notif.send(Notif {
                        message: format!("failed to run save script: {e}"),
                        kind: Error,
                    }),
                };
            }
            if !appdata.script.is_empty() {
                if let Err(e) = std::fs::remove_file(PATH) {
                    notif.send(Notif {
                        message: format!("failed to remove save script: {e}"),
                        kind: Error,
                    });
                }
            }
            notif.send(Notif {
                message: "map saved".into(),
                kind: Success,
            });
        }
        Err(e) => {
            notif.send(Notif {
                message: e.to_string(),
                kind: Error,
            });
        }
    }
}

pub fn add_pak(_: Trigger<triggers::AddPak>, mut commands: Commands, mut appdata: ResMut<AppData>) {
    if let Some(path) = rfd::FileDialog::new()
        .set_title("add pak folder")
        .pick_folder()
        .and_then(|path| path.to_str().map(str::to_string))
    {
        appdata.pak = Some(appdata.paks.len());
        appdata.paks.push((path, String::new()));
        commands.trigger(triggers::LoadPaks);
    }
}

pub fn transplant_from(
    _: Trigger<triggers::TransplantFrom>,
    mut notif: EventWriter<Notif>,
    appdata: ResMut<AppData>,
    map: NonSend<Map>,
    mut transplant: NonSendMut<Transplant>,
) {
    if map.0.is_none() {
        notif.send(Notif {
            message: "no map to transplant into".into(),
            kind: Error,
        });
        return;
    };
    let Some(path) = rfd::FileDialog::new()
        .set_title("open map")
        .add_filter("maps", &["umap"])
        .pick_file()
    else {
        return;
    };
    match asset::open(path, appdata.version()) {
        Ok(donor) => {
            // no need for verbose warnings here
            let actors: Vec<_> = actor::get_actors(&donor)
                .into_iter()
                .filter_map(|index| {
                    actor::Actor::new(&donor, index)
                        .ok()
                        .map(|(_, actor)| actor)
                })
                .collect();
            let selected = Vec::with_capacity(actors.len());
            transplant.0 = Some((donor, actors, selected));
        }
        Err(e) => {
            notif.send(Notif {
                message: e.to_string(),
                kind: Error,
            });
        }
    }
}

pub fn transplant_into(
    _: Trigger<triggers::TransplantInto>,
    commands: Commands,
    mut notif: EventWriter<Notif>,
    appdata: ResMut<AppData>,
    mut map: NonSendMut<Map>,
    selected: Query<&actor::Actor, With<actor::Selected>>,
    actors: Query<Entity, With<actor::Actor>>,
    client: ResMut<Client>,
    registry: ResMut<Registry>,
    meshes: ResMut<Assets<Mesh>>,
    materials: ResMut<Assets<unlit::Unlit>>,
    images: ResMut<Assets<Image>>,
    content: Res<Content>,
    consts: Res<Constants>,
) {
    let Some((donor, ..)) = &mut map.0 else {
        notif.send(Notif {
            message: "no map to transplant into".into(),
            kind: Error,
        });
        return;
    };
    if selected.is_empty() {
        notif.send(Notif {
            message: "no actors selected to transplant".into(),
            kind: Error,
        });
        return;
    }
    let Some(path) = rfd::FileDialog::new()
        .set_title("open map")
        .add_filter("maps", &["umap"])
        .pick_file()
    else {
        return;
    };
    let mut recipient = match asset::open(&path, appdata.version()) {
        Ok(recipient) => recipient,
        Err(e) => {
            notif.send(Notif {
                message: e.to_string(),
                kind: Error,
            });
            return;
        }
    };
    for actor in selected.iter() {
        actor.transplant(&mut recipient, donor, &mut vec![], &mut vec![]);
        notif.send(Notif {
            message: format!("transplanted {}", actor.name),
            kind: Success,
        });
    }
    open_asset(
        None,
        Some(path),
        recipient,
        commands,
        actors,
        notif,
        appdata,
        client,
        map,
        registry,
        meshes,
        materials,
        images,
        content,
        consts,
    );
}
