use std::{
    cell::RefCell,
    collections::HashSet,
    fs::File,
    io::{BufReader, Cursor},
    path::Path,
    sync::{Arc, RwLock},
};

use unreal_asset::{
    engine_version::EngineVersion,
    error::Error,
    schema::{self, LooseContent},
    unversioned::Usmap,
    Asset,
};

/// the raw usmap, kept as bytes since parsed mappings can't be sent between threads
static MAPPINGS: RwLock<Option<Arc<Vec<u8>>>> = RwLock::new(None);

thread_local! {
    /// mappings parsed on this thread and the bytes they came from
    static PARSED: RefCell<Option<(Arc<Vec<u8>>, Usmap)>> = const { RefCell::new(None) };
}

/// replaces the mappings with the contents of the usmap at this path
pub fn load_mappings(path: &str) -> Result<(), Error> {
    let mappings = match path.is_empty() {
        true => None,
        false => {
            let data = std::fs::read(path)?;
            Usmap::new(Cursor::new(data.clone()))?;
            Some(Arc::new(data))
        }
    };
    *MAPPINGS.write().unwrap() = mappings;
    Ok(())
}

/// a copy of the mappings for an asset to read with
pub fn mappings() -> Option<Usmap> {
    let data = MAPPINGS.read().unwrap().clone()?;
    PARSED.with_borrow_mut(|parsed| {
        if !parsed.as_ref().is_some_and(|(from, _)| Arc::ptr_eq(from, &data)) {
            let usmap = Usmap::new(Cursor::new(data.to_vec())).ok()?;
            *parsed = Some((data, usmap));
        }
        parsed.as_ref().map(|(_, usmap)| usmap.clone())
    })
}

/// reads an asset with these mappings
fn read_file(
    file: &Path,
    version: EngineVersion,
    mappings: Option<Usmap>,
) -> Result<super::Asset, Error> {
    Asset::new(
        super::Wrapper::File(BufReader::new(File::open(file)?)),
        File::open(file.with_extension("uexp"))
            .ok()
            .map(BufReader::new)
            .map(super::Wrapper::File),
        version,
        mappings,
    )
}

/// the folder holding a loose game and the name of the game from a file inside its content
fn game_of(file: &Path) -> Option<(&Path, String)> {
    let game = file.ancestors().find(|dir| dir.join("Content").is_dir())?;
    Some((game.parent()?, game.file_name()?.to_string_lossy().into_owned()))
}

/// adds the blueprints this asset needs to the mappings, returns whether there were any
fn pull_schemas(asset: &super::Asset, file: &Path, version: EngineVersion) -> bool {
    let Some((root, game)) = game_of(file) else {
        return false;
    };
    let Some(data) = MAPPINGS.read().unwrap().clone() else {
        return false;
    };
    // make sure this thread has parsed them
    let Some(mut usmap) = mappings() else {
        return false;
    };
    let content = LooseContent::new(root, &game);
    let visited = &mut HashSet::new();
    let added = schema::pull_schemas(asset, version, &mut usmap, visited, |package| {
        content.load(package)
    });
    PARSED.set(Some((data, usmap)));
    added > 0
}

/// creates an asset from the specified path and version
pub fn open(file: impl AsRef<Path>, version: EngineVersion) -> Result<super::Asset, Error> {
    let file = file.as_ref();
    let asset = read_file(file, version, mappings())?;
    // blueprints are only readable once their schemas are in the mappings
    match pull_schemas(&asset, file, version) {
        true => read_file(file, version, mappings()),
        false => Ok(asset),
    }
}

/// whether saving a map without changing it would give back the same file
///
/// this writes a separate copy since writing can add to an asset's name map
pub fn preserves(file: impl AsRef<Path>, version: EngineVersion) -> bool {
    let file = file.as_ref();
    let Ok(asset) = read_file(file, version, mappings()) else {
        return false;
    };
    let mut data = Cursor::new(Vec::new());
    let mut exports = asset
        .asset_data
        .use_event_driven_loader
        .then(|| Cursor::new(Vec::new()));
    // writers have to be the same type as they're passed together
    if asset.write_data(&mut data, exports.as_mut()).is_err() {
        return false;
    }
    let same = |path: &Path, written: Vec<u8>| std::fs::read(path).is_ok_and(|e| e == written);
    same(file, data.into_inner())
        && exports.map_or(true, |e| same(&file.with_extension("uexp"), e.into_inner()))
}

/// saves an asset's data to the specified path
pub fn save<C: std::io::Read + std::io::Seek>(
    asset: &mut Asset<C>,
    path: impl AsRef<Path>,
) -> Result<(), Error> {
    asset.rebuild_name_map();
    asset.write_data(
        &mut File::create(&path)?,
        asset
            .asset_data
            .use_event_driven_loader
            .then(|| File::create(path.as_ref().with_extension("uexp")))
            .transpose()?
            .as_mut(),
    )
}

#[test]
fn path_with_mount() {
    let game = "Lava";
    let path = "/Theme04_Wasteland/Mesh/SM_TH04_Asphalt_Piece_D";
    let mount = "../../../Lava/Plugins/Objects/Theme_Sets/Theme04_Wasteland/Content/Mesh/";
    let file = "SM_TH04_Asphalt_Piece_D.uasset";
    let path = match path.split('/').nth(1) {
        Some("Game") => path.replace("/Game", &format!("{}/Content", game)),
        Some("Engine") => path.replace("/Engine/", "Engine/Content/"),
        Some(plugin) => path.replace(&format!("/{plugin}/"), &format!("{plugin}/Content/")),
        _ => path.into(),
    };
    assert_eq!(
        path,
        "Theme04_Wasteland/Content/Mesh/SM_TH04_Asphalt_Piece_D"
    );
    assert!((mount.to_owned() + file).ends_with(&(path + ".uasset")));
}

pub fn get<T>(
    content: &super::Content,
    cache: Option<&std::path::Path>,
    path: &str,
    version: unreal_asset::engine_version::EngineVersion,
    func: impl Fn(
        unreal_asset::Asset<super::Wrapper>,
        Option<super::Wrapper>,
    ) -> Result<T, unreal_asset::error::Error>,
) -> Option<T> {
    let path = match path.split('/').nth(1) {
        Some("Game") => path.replace("/Game", &format!("{}/Content", content.game)),
        Some("Engine") => path.replace("/Engine/", "Engine/Content/"),
        Some(plugin) => path.replace(&format!("/{plugin}/"), &format!("{plugin}/Content/")),
        _ => path.into(),
    };
    let loose = content.folder.join(&path);
    let mesh = loose.with_extension("uasset");
    if mesh.exists() {
        if let Ok(asset) = read_file(&mesh, version, mappings()).and_then(|asset| {
            func(
                asset,
                std::fs::File::open(loose.with_extension("ubulk"))
                    .ok()
                    .map_or_else(
                        || std::fs::File::open(loose.with_extension("uptnl")).ok(),
                        Some,
                    )
                    .map(std::io::BufReader::new)
                    .map(super::Wrapper::File),
            )
        }) {
            return Some(asset);
        }
    }
    for (pak_file, pak) in content.paks.iter() {
        if let Ok(asset) = read(pak, pak_file, cache, &path, version, &func) {
            return Some(asset);
        }
    }
    None
}

fn read<T>(
    pak: &repak::PakReader,
    pak_file: &std::path::Path,
    cache: Option<&std::path::Path>,
    path: &str,
    version: unreal_asset::engine_version::EngineVersion,
    func: impl Fn(
        unreal_asset::Asset<super::Wrapper>,
        Option<super::Wrapper>,
    ) -> Result<T, unreal_asset::error::Error>,
) -> Result<T, unreal_asset::error::Error> {
    let mut path: String = path.into();
    let mesh = path.clone() + ".uasset";
    let Some(file) = pak
        .files()
        .into_iter()
        .find(|file| (pak.mount_point().to_string() + file).ends_with(&mesh))
    else {
        return Err(Error::NoData("asset not found in pak".into()));
    };
    path = file.trim_end_matches(".uasset").into();
    let make = |ext: &str| path.clone() + ext;
    let (mesh, exp, bulk, uptnl) = (
        make(".uasset"),
        make(".uexp"),
        make(".ubulk"),
        make(".uptnl"),
    );
    let cached = |path: &str| cache.unwrap().join(path.trim_start_matches('/'));
    let pak_file = &mut std::io::BufReader::new(std::fs::File::open(pak_file)?);
    match cache {
        Some(_)
            if cached(&mesh).exists() ||
            // try to create cache if it doesn't exist
            (
                std::fs::create_dir_all(cached(&path).parent().unwrap()).is_ok() &&
                pak.read_file(&mesh, pak_file, &mut std::fs::File::create(cached(&mesh))?).is_ok() &&
                // we don't care whether these are successful in case they don't exist
                pak.read_file(&exp, pak_file, &mut std::fs::File::create(cached(&exp))?).map_or(true,|_| true) &&
                pak.read_file(&bulk, pak_file, &mut std::fs::File::create(cached(&bulk))?).map_or(true,|_| true) &&
                pak.read_file(&uptnl, pak_file, &mut std::fs::File::create(cached(&uptnl))?).map_or(true,|_| true)
             ) =>
        {
            func(
                unreal_asset::Asset::new(
                    super::Wrapper::File(std::io::BufReader::new(std::fs::File::open(cached(
                        &mesh,
                    ))?)),
                    std::fs::File::open(cached(&exp))
                        .ok()
                        .map(std::io::BufReader::new)
                        .map(super::Wrapper::File),
                    version,
                    mappings(),
                )?,
                std::fs::File::open(cached(&bulk))
                    .ok()
                    .map_or_else(|| std::fs::File::open(cached(&uptnl)).ok(), Some)
                    .map(std::io::BufReader::new)
                    .map(super::Wrapper::File),
            )
        }
        // if the cache cannot be created fall back to storing in memory
        _ => func(
            unreal_asset::Asset::new(
                super::Wrapper::Bytes(std::io::Cursor::new(pak.get(&mesh, pak_file).map_err(
                    |e| {
                        unreal_asset::error::Error::no_data(match e {
                            repak::Error::Oodle => "oodle paks are unsupported atm".to_string(),
                            e => format!("error reading pak: {e}"),
                        })
                    },
                )?)),
                pak.get(&exp, pak_file)
                    .ok()
                    .map(std::io::Cursor::new)
                    .map(super::Wrapper::Bytes),
                version,
                mappings(),
            )?,
            pak.get(&bulk, pak_file)
                .ok()
                .map_or_else(|| pak.get(&uptnl, pak_file).ok(), Some)
                .map(std::io::Cursor::new)
                .map(super::Wrapper::Bytes),
        ),
    }
}
