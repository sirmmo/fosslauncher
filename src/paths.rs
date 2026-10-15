use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn dirs() -> &'static directories::ProjectDirs {
    static DIRS: OnceLock<directories::ProjectDirs> = OnceLock::new();
    DIRS.get_or_init(|| {
        directories::ProjectDirs::from("org", "fosslauncher", "fosslauncher")
            .expect("no home directory")
    })
}

/// Apps installed by the launcher itself ("download" installers) and `installed.json`.
pub fn data_dir() -> &'static Path {
    dirs().data_dir()
}

pub fn cache_dir() -> &'static Path {
    dirs().cache_dir()
}

pub fn app_dir(id: &str) -> PathBuf {
    data_dir().join("apps").join(id)
}

/// Locate an executable on PATH.
pub fn which(cmd: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or(".EXE;.CMD;.BAT".into())
            .split(';')
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        exts.iter()
            .map(|ext| dir.join(format!("{cmd}{ext}")))
            .find(|p| p.is_file())
    })
}
