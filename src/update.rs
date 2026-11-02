use super::*;
use std::sync::{mpsc, Mutex};

/// where releases are checked for
const REPO: &str = "Marduc666/stove";
/// the release this was built for, which only builds made by the release workflow have
const TAG: Option<&str> = option_env!("STOVE_TAG");
/// the name each platform's build is uploaded as
const ASSET: &str = match () {
    _ if cfg!(windows) => "stove.exe",
    _ if cfg!(target_os = "macos") => "stove-macos",
    _ => "stove-linux",
};

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    prerelease: bool,
    draft: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(serde::Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

enum Message {
    Found(Found),
    Installed,
    Failed(String),
}

pub struct Found {
    tag: String,
    page: String,
    download: Option<String>,
}

#[derive(Default, Resource)]
pub struct Update {
    found: Option<Found>,
    open: bool,
    busy: bool,
    installed: bool,
    // receivers aren't sync
    worker: Option<Mutex<mpsc::Receiver<Message>>>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .user_agent("stove")
        .build()
        .into()
}

/// the newest release a build of `tag` should move to, test builds follow test releases
fn newest(releases: Vec<Release>, tag: &str) -> Option<Release> {
    // releases come newest first
    let current = releases.iter().position(|release| release.tag_name == tag);
    let testing = current.is_none_or(|current| releases[current].prerelease);
    releases
        .into_iter()
        .take(current.unwrap_or(usize::MAX))
        .find(|release| !release.draft && (testing || !release.prerelease))
}

fn check() -> Result<Option<Found>, String> {
    let Some(tag) = TAG else {
        return Ok(None);
    };
    let releases: Vec<Release> = agent()
        .get(format!("https://api.github.com/repos/{REPO}/releases?per_page=30"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    Ok(newest(releases, tag).map(|release| Found {
        download: release
            .assets
            .into_iter()
            .find(|asset| asset.name == ASSET)
            .map(|asset| asset.browser_download_url),
        tag: release.tag_name,
        page: release.html_url,
    }))
}

/// downloads the new build next to the running one and swaps them
fn install(url: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let download = exe.with_extension("download");
    let result = (|| {
        let mut file = std::fs::File::create(&download).map_err(|e| e.to_string())?;
        let response = agent().get(url).call().map_err(|e| e.to_string())?;
        std::io::copy(&mut response.into_body().into_reader(), &mut file)
            .map_err(|e| e.to_string())?;
        drop(file);
        // a running program can't be overwritten but it can be moved out of the way
        self_replace::self_replace(&download).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&download);
    result
}

pub fn start(mut update: ResMut<Update>, appdata: Res<AppData>) {
    if !appdata.updates || TAG.is_none() {
        return;
    }
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(match check() {
            Ok(Some(found)) => Message::Found(found),
            // being offline isn't worth bothering anyone about
            Ok(None) | Err(_) => return,
        });
    });
    update.worker = Some(Mutex::new(receive));
}

pub fn window(
    mut ctx: bevy_egui::EguiContexts,
    mut update: ResMut<Update>,
    mut notif: EventWriter<Notif>,
    mut exit: EventWriter<AppExit>,
) {
    let message = update
        .worker
        .as_ref()
        .and_then(|worker| worker.lock().ok()?.try_recv().ok());
    match message {
        Some(Message::Found(found)) => {
            update.found = Some(found);
            update.open = true;
        }
        Some(Message::Installed) => {
            update.busy = false;
            update.installed = true;
        }
        Some(Message::Failed(e)) => {
            update.busy = false;
            notif.send(Notif {
                message: format!("couldn't update: {e}"),
                kind: Error,
            });
        }
        None => (),
    }
    if !update.open {
        return;
    }
    let Some(found) = update.found.as_ref() else {
        return;
    };
    let mut open = true;
    let mut install_from = None;
    let mut restart = false;
    egui::Window::new("update")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx.ctx_mut(), |ui| {
            ui.label(format!("stove {} is out, you have {}", found.tag, TAG.unwrap_or_default()));
            ui.hyperlink_to("see what's new", &found.page);
            if update.installed {
                ui.label("the new version is in place and will be used from the next start");
                ui.label("save your map before restarting");
                restart = ui.button("restart now").clicked();
            } else if update.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("downloading");
                });
            } else {
                match &found.download {
                    Some(url) => {
                        if ui.button("update").clicked() {
                            install_from = Some(url.clone());
                        }
                    }
                    None => {
                        ui.label("this release has no build for your system yet");
                    }
                }
            }
        });
    if let Some(url) = install_from {
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(match install(&url) {
                Ok(()) => Message::Installed,
                Err(e) => Message::Failed(e),
            });
        });
        update.worker = Some(Mutex::new(receive));
        update.busy = true;
    }
    if restart {
        match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).spawn()) {
            Ok(_) => {
                exit.send(AppExit::Success);
            }
            Err(e) => {
                notif.send(Notif {
                    message: format!("couldn't restart: {e}"),
                    kind: Error,
                });
            }
        }
    }
    // closing it while downloading would hide whether it worked
    update.open = open || update.busy;
}

#[test]
fn picks_newest() {
    let release = |tag: &str, prerelease| Release {
        tag_name: tag.into(),
        html_url: String::new(),
        prerelease,
        draft: false,
        assets: Vec::new(),
    };
    let releases = || {
        vec![
            release("1.1.0-test2", true),
            release("1.1.0-test1", true),
            release("1.0.0", false),
            release("1.0.0-test1", true),
        ]
    };
    let tag = |found: Option<Release>| found.map(|release| release.tag_name);
    // test builds move to newer test builds
    assert_eq!(tag(newest(releases(), "1.1.0-test1")).as_deref(), Some("1.1.0-test2"));
    assert_eq!(tag(newest(releases(), "1.0.0-test1")).as_deref(), Some("1.1.0-test2"));
    // stable builds only move to stable ones
    assert_eq!(tag(newest(releases(), "1.0.0")), None);
    // the newest has nothing to move to
    assert_eq!(tag(newest(releases(), "1.1.0-test2")), None);
}
