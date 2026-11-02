use super::*;

/// what's kept between runs
///
/// this has its own file as values egui persists are tied to the build of stove that wrote them
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    /// the name of the version so it survives versions being added or removed
    version: String,
    usmap: String,
    paks: Vec<(String, String)>,
    pak: Option<usize>,
    cache: bool,
    textures: bool,
    wireframe: bool,
    script: String,
    cap: bool,
    rate: f64,
    fullscreen: bool,
    updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: VERSIONS[0].1.to_string(),
            usmap: String::new(),
            paks: Vec::new(),
            pak: None,
            cache: false,
            textures: true,
            wireframe: false,
            script: String::new(),
            cap: false,
            rate: 60.0,
            fullscreen: false,
            updates: true,
        }
    }
}

pub fn load(
    mut commands: Commands,
    mut ctx: bevy_egui::EguiContexts,
    mut fps: ResMut<bevy_framepace::FramepaceSettings>,
    mut wire: ResMut<bevy::pbr::wireframe::WireframeConfig>,
    mut windows: Query<&mut Window>,
) {
    let saved = config()
        .map(|config| config.join("settings.ron"))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|str| ron::from_str::<Settings>(&str).ok());
    let mut settings = Settings::default();
    // the ui's own state like where windows are
    ctx.ctx_mut().memory_mut(|storage| {
        if let Some(config) = config()
            .map(|config| config.join("config.ron"))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|str| ron::from_str::<egui::util::IdTypeMap>(&str).ok())
        {
            storage.data = config
        }
        if saved.is_some() {
            return;
        }
        // older builds kept settings in egui which builds made the same way can still read
        let data = &mut storage.data;
        fn retrieve<T: egui::util::id_type_map::SerializableAny>(
            val: &mut T,
            key: &str,
            data: &mut egui::util::IdTypeMap,
        ) {
            if let Some(inner) = data.get_persisted(egui::Id::new(key)) {
                *val = inner
            }
        }
        let mut version = 0usize;
        retrieve(&mut version, "version", data);
        if let Some((_, name)) = VERSIONS.get(version) {
            settings.version = name.to_string();
        }
        retrieve(&mut settings.usmap, "usmap", data);
        retrieve(&mut settings.paks, "paks", data);
        retrieve(&mut settings.pak, "pak", data);
        retrieve(&mut settings.cache, "cache", data);
        retrieve(&mut settings.textures, "textures", data);
        retrieve(&mut settings.wireframe, "wireframe", data);
        retrieve(&mut settings.script, "script", data);
        retrieve(&mut settings.cap, "cap", data);
        retrieve(&mut settings.rate, "rate", data);
        retrieve(&mut settings.fullscreen, "fullscreen", data);
    });
    let settings = saved.unwrap_or(settings);
    let mut appdata = AppData {
        version: VERSIONS
            .iter()
            .position(|(_, name)| *name == settings.version)
            .unwrap_or_default(),
        usmap: settings.usmap,
        pak: settings.pak.filter(|&pak| pak < settings.paks.len()),
        paks: settings.paks,
        cache: settings.cache,
        textures: settings.textures,
        wireframe: settings.wireframe,
        script: settings.script,
        cap: settings.cap,
        rate: settings.rate,
        updates: settings.updates,
        ..default()
    };
    fps.limiter = match appdata.cap {
        true => bevy_framepace::Limiter::from_framerate(appdata.rate),
        false => bevy_framepace::Limiter::Off,
    };
    if settings.fullscreen {
        let mut window = windows.single_mut();
        window.mode = bevy::window::WindowMode::BorderlessFullscreen
    }
    wire.global = appdata.wireframe;
    // a mapping file that has since moved shouldn't stop stove from starting
    if crate::asset::load_mappings(&appdata.usmap).is_err() {
        appdata.usmap.clear();
    }
    commands.insert_resource(appdata);
    commands.trigger(triggers::LoadPaks);
}

pub fn write(mut ctx: bevy_egui::EguiContexts, appdata: Res<AppData>, windows: Query<&Window>) {
    if !appdata.is_changed() {
        return;
    }
    let Some(config) = config() else {
        return;
    };
    let _ = std::fs::create_dir_all(&config);
    ctx.ctx_mut().memory_mut(|storage| {
        if let Ok(data) = ron::to_string(&storage.data) {
            let _ = std::fs::write(config.join("config.ron"), data);
        }
    });
    let settings = Settings {
        version: VERSIONS[appdata.version].1.to_string(),
        usmap: appdata.usmap.clone(),
        paks: appdata.paks.clone(),
        pak: appdata.pak,
        cache: appdata.cache,
        textures: appdata.textures,
        wireframe: appdata.wireframe,
        script: appdata.script.clone(),
        cap: appdata.cap,
        rate: appdata.rate,
        updates: appdata.updates,
        fullscreen: windows
            .get_single()
            .map(|window| window.mode != bevy::window::WindowMode::Windowed)
            .unwrap_or_default(),
    };
    if let Ok(data) = ron::ser::to_string_pretty(&settings, ron::ser::PrettyConfig::default()) {
        let _ = std::fs::write(config.join("settings.ron"), data);
    }
}
