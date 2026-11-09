//! Stamp library: scan a root folder for stamp packs.

use crate::pack::{list_stamps, load_pack, StampListEntry};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    "_tools",
    "_gamedata",
    "stamp_editor",
    "target",
    ".git",
    "docs",
    "anno117_stamps",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorConfig {
    pub library_root: PathBuf,
    /// Anno 117 install folder (contains `maindata/ui.rda`). Icons are *not*
    /// shipped with the editor — users link their own game copy.
    #[serde(default)]
    pub game_install: Option<PathBuf>,
}

impl EditorConfig {
    pub fn default_root() -> PathBuf {
        // Outside Ubisoft stamps sync — game may wipe Documents\...\stamps on start.
        PathBuf::from(r"C:\Users\fabi_\Projects\anno117-stamp-editor\library")
    }

    pub fn config_path() -> PathBuf {
        PathBuf::from(r"C:\Users\fabi_\Projects\anno117-stamp-editor\stamp_editor_config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(mut cfg) = serde_json::from_str::<EditorConfig>(&text) {
                if !cfg.library_root.is_dir() {
                    cfg.library_root = Self::default_root();
                }
                if let Some(g) = &cfg.game_install {
                    if !g.is_dir() {
                        cfg.game_install = None;
                    }
                }
                return cfg;
            }
        }
        Self {
            library_root: Self::default_root(),
            game_install: None,
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        fs::write(Self::config_path(), text + "\n")
    }

    /// `…/Anno 117 - Pax Romana/maindata/ui.rda` — UI icons live packed in this archive.
    pub fn ui_rda_path(game_install: &Path) -> PathBuf {
        game_install.join("maindata").join("ui.rda")
    }

    pub fn looks_like_anno_install(game_install: &Path) -> bool {
        Self::ui_rda_path(game_install).is_file()
    }
}

#[derive(Debug, Clone)]
pub struct LibraryPack {
    pub path: PathBuf,
    /// Relative folder from library root (e.g. `Albion`).
    pub folder: String,
    pub file_name: String,
    pub stamps: Vec<StampListEntry>,
    pub region_hint: u32,
}

pub fn scan_library(root: &Path) -> Vec<LibraryPack> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return out;
    }
    scan_dir(root, root, &mut out);
    out.sort_by(|a, b| {
        a.folder
            .to_lowercase()
            .cmp(&b.folder.to_lowercase())
            .then_with(|| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()))
    });
    out
}

/// Cheap fingerprint of pack JSON files under `root` (relative path + mtime + len).
/// Used to auto-refresh the library when files change outside the editor.
pub fn library_fingerprint(root: &Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    if !root.is_dir() {
        return 0;
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_pack_json_paths(root, root, &mut paths);
    paths.sort();
    for path in paths {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        rel.hash(&mut hasher);
        if let Ok(meta) = fs::metadata(&path) {
            meta.len().hash(&mut hasher);
            if let Ok(modified) = meta.modified() {
                modified.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

fn collect_pack_json_paths(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SKIP_DIRS.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                continue;
            }
            collect_pack_json_paths(root, &path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if name == "stamp_editor_config.json" || name == "editor_catalog.json" {
            continue;
        }
        out.push(path);
    }
}

fn scan_dir(root: &Path, dir: &Path, out: &mut Vec<LibraryPack>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SKIP_DIRS.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                continue;
            }
            scan_dir(root, &path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if name == "stamp_editor_config.json" || name == "editor_catalog.json" {
            continue;
        }
        let Ok(pack) = load_pack(&path) else {
            continue;
        };
        if pack.stamps.iter().all(|s| s.pipette_data.is_empty()) {
            continue;
        }
        let stamps = list_stamps(&path).unwrap_or_default();
        if stamps.is_empty() {
            continue;
        }
        let folder = path
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let region_hint = pack
            .stamps
            .iter()
            .find(|s| s.region_guid != 0)
            .map(|s| s.region_guid)
            .unwrap_or(0);
        out.push(LibraryPack {
            path,
            folder,
            file_name: name,
            stamps,
            region_hint,
        });
    }
}

/// Unique path `dir/base.json`, `dir/base 2.json`, …
pub fn unique_json_path(dir: &Path, base: &str) -> PathBuf {
    let candidate = dir.join(format!("{base}.json"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2..10_000 {
        let candidate = dir.join(format!("{base} {n}.json"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!(
        "{base} {}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ))
}

pub fn unique_dir_path(parent: &Path, base: &str) -> PathBuf {
    let candidate = parent.join(base);
    if !candidate.exists() {
        return candidate;
    }
    for n in 2..10_000 {
        let candidate = parent.join(format!("{base} {n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!(
        "{base} {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    ))
}

/// Top-level library folders (excluding tool dirs), plus folders that already contain packs.
pub fn list_library_folders(root: &Path, packs: &[LibraryPack]) -> Vec<String> {
    let mut folders: Vec<String> = packs
        .iter()
        .map(|p| {
            if p.folder.is_empty() {
                "(Root)".into()
            } else {
                p.folder.clone()
            }
        })
        .collect();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP_DIRS.iter().any(|s| name.eq_ignore_ascii_case(s)) {
                continue;
            }
            folders.push(name);
        }
    }
    folders.sort();
    folders.dedup();
    folders
}

pub fn folder_abs_path(root: &Path, folder: &str) -> PathBuf {
    if folder.is_empty() || folder == "(Root)" {
        root.to_path_buf()
    } else {
        root.join(folder)
    }
}

pub fn create_library_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = unique_dir_path(parent, name);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub fn delete_pack_file(path: &Path) -> Result<(), String> {
    fs::remove_file(path).map_err(|e| e.to_string())
}

/// Move a pack JSON into `dest_dir` (renames on name collision).
pub fn move_pack_to_folder(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let name = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| "Ungültiger Pack-Pfad".to_string())?;
    let mut dest = dest_dir.join(&name);
    if dest == src {
        return Ok(src.to_path_buf());
    }
    if dest.exists() {
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Pack".into());
        dest = unique_json_path(dest_dir, &stem);
    }
    fs::rename(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Delete an empty library folder.
pub fn delete_library_folder(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("Kein Ordner".into());
    }
    let mut entries = fs::read_dir(path).map_err(|e| e.to_string())?;
    if entries.next().is_some() {
        return Err("Ordner ist nicht leer".into());
    }
    fs::remove_dir(path).map_err(|e| e.to_string())
}

pub fn copy_pack_file(src: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dest_dir).map_err(|e| e.to_string())?;
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Pack".into());
    let dest = unique_json_path(dest_dir, &format!("{stem} Kopie"));
    fs::copy(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Move packs into region folders under root. Returns number moved.
pub fn sort_packs_by_region(root: &Path, packs: &[LibraryPack]) -> Result<usize, String> {
    let mut moved = 0usize;
    for pack in packs {
        let region_folder = match pack.region_hint {
            6626 => "Albion",
            3225 => "Latium",
            _ => "unsorted",
        };
        let dest_dir = root.join(region_folder);
        fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
        let dest = dest_dir.join(&pack.file_name);
        if dest == pack.path {
            continue;
        }
        if dest.exists() {
            continue;
        }
        fs::rename(&pack.path, &dest).map_err(|e| e.to_string())?;
        moved += 1;
    }
    Ok(moved)
}

