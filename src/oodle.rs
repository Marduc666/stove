//! oodle can't be redistributed but games that use it ship the library alongside their paks

use std::{path::Path, sync::OnceLock};

type Decompress = unsafe extern "C" fn(
    comp_buf: *const u8,
    comp_buf_size: usize,
    raw_buf: *mut u8,
    raw_len: usize,
    fuzz_safe: u32,
    check_crc: u32,
    verbosity: u32,
    dec_buf_base: u64,
    dec_buf_size: usize,
    fp_callback: u64,
    callback_user_data: u64,
    decoder_memory: *mut u8,
    decoder_memory_size: usize,
    thread_phase: u32,
) -> i32;

static OODLE: OnceLock<Decompress> = OnceLock::new();

/// loads the first oodle library found in a game's folder if there isn't one loaded already
pub fn load(folder: impl AsRef<Path>) {
    if OODLE.get().is_some() {
        return;
    }
    let libraries = walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|path| {
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with("oo2core") && name.ends_with(".dll"))
        });
    for path in libraries {
        // the library stays loaded for as long as stove is open as the function pointer needs it
        let Some(library) = (unsafe { libloading::Library::new(path) })
            .ok()
            .map(|library| &*Box::leak(Box::new(library)))
        else {
            continue;
        };
        if let Ok(decompress) = unsafe { library.get::<Decompress>(b"OodleLZ_Decompress\0") } {
            let _ = OODLE.set(*decompress);
            return;
        }
    }
}

/// the decompression function for repak if there's a library to use
pub fn get() -> Result<repak_oodle::Decompress, Box<dyn std::error::Error>> {
    match OODLE.get() {
        Some(_) => Ok(decompress),
        None => Err("no oodle library was found in the game's folder".into()),
    }
}

fn decompress(comp_buf: &[u8], raw_buf: &mut [u8]) -> i32 {
    let Some(oodle) = OODLE.get() else { return 0 };
    unsafe {
        oodle(
            comp_buf.as_ptr(),
            comp_buf.len(),
            raw_buf.as_mut_ptr(),
            raw_buf.len(),
            1,
            1,
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            0,
            3,
        )
    }
}

/// repak keeps its signature private so spell it out here
mod repak_oodle {
    pub type Decompress = fn(comp_buf: &[u8], raw_buf: &mut [u8]) -> i32;
}
