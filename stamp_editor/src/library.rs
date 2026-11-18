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
    /// Live Ubisoft stamps folder (`Documents\Anno 117 - Pax Romana\stamps`).
    /// Editor library must stay outside this path — the game wipe-syncs it on start.
    #[serde(default = "EditorConfig::default_game_stamps_root")]
    pub game_stamps_root: PathBuf,
    /// After each library save, also copy the pack into `game_stamps_root`
    /// (same relative path) so stamps can appear in-game. Library remains the
    /// durable copy if Ubisoft wipe-sync deletes the Documents folder.
    #[serde(default = "default_true")]
    pub mirror_to_game: bool,
    /// UI language: `"de"` | `"en"`.
    #[serde(default = "default_language")]
    pub language: String,
    /// First-run wizard completed (paths chosen / confirmed).
    /// Missing field in old configs → `true` (no forced re-wizard). Fresh install → `false`.
    #[serde(default = "default_true")]
    pub setup_complete: bool,
    /// Most-recently opened pack paths (newest first, capped + deduped).
    #[serde(default)]
    pub recent_packs: Vec<PathBuf>,
}

/// Max number of entries kept in `EditorConfig::recent_packs`.
pub const RECENT_PACKS_MAX: usize = 8;

fn default_true() -> bool {
    true
}

fn default_language() -> String {
    "en".into()
}

impl EditorConfig {
    /// Writable data folder: next to the EXE when possible, else `%LOCALAPPDATA%\Anno117StampEditor`.
    pub fn config_dir() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let existing = dir.join("stamp_editor_config.json");
                if existing.is_file() {
                    return dir.to_path_buf();
                }
                let probe = dir.join(".stamp_editor_write_test");
                if fs::write(&probe, b"ok").is_ok() {
                    let _ = fs::remove_file(&probe);
                    return dir.to_path_buf();
                }
            }
        }
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("Anno117StampEditor");
        let _ = fs::create_dir_all(&dir);
        dir
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("stamp_editor_config.json")
    }

    /// Safe stamp library (outside Ubisoft wipe-sync).
    pub fn default_root() -> PathBuf {
        Self::config_dir().join("library")
    }

    /// Standard Anno Documents stamps folder — exists for most players after first launch.
    pub fn default_game_stamps_root() -> PathBuf {
        let docs = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
        docs.join(r"Documents\Anno 117 - Pax Romana\stamps")
    }

    pub fn documents_stamps_available() -> bool {
        Self::default_game_stamps_root().is_dir()
    }

    /// Best-effort guess of the Anno 117 install (Steam / common drives).
    pub fn guess_game_install() -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        for drive in ["C", "D", "E", "F", "G", "H"] {
            candidates.push(PathBuf::from(format!(
                r"{drive}:\Program Files (x86)\Steam\steamapps\common\Anno 117 - Pax Romana"
            )));
            candidates.push(PathBuf::from(format!(
                r"{drive}:\SteamLibrary\steamapps\common\Anno 117 - Pax Romana"
            )));
            candidates.push(PathBuf::from(format!(
                r"{drive}:\Program Files\Epic Games\Anno117PaxRomana"
            )));
            candidates.push(PathBuf::from(format!(
                r"{drive}:\Ubisoft Game Launcher\games\Anno 117 - Pax Romana"
            )));
        }
        candidates
            .into_iter()
            .find(|p| Self::looks_like_anno_install(p))
    }

    /// True when the first-run wizard should open.
    pub fn needs_first_run(&self) -> bool {
        if self.setup_complete {
            return false;
        }
        true
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(mut cfg) = serde_json::from_str::<EditorConfig>(&text) {
                if !cfg.library_root.is_dir() {
                    cfg.library_root = Self::default_root();
                }
                if cfg.game_stamps_root.as_os_str().is_empty() || !cfg.game_stamps_root.is_dir() {
                    let fallback = Self::default_game_stamps_root();
                    if fallback.is_dir() {
                        cfg.game_stamps_root = fallback;
                    }
                }
                if let Some(g) = &cfg.game_install {
                    if !g.is_dir() {
                        cfg.game_install = None;
                    }
                }
                // Auto-migrate: never keep the live sync folder as library_root.
                if Self::is_game_stamps_path(&cfg.library_root, &cfg.game_stamps_root) {
                    let safe = Self::default_root();
                    let _ = fs::create_dir_all(&safe);
                    let _ = seed_library_from_game(&safe, &cfg.game_stamps_root);
                    cfg.library_root = safe;
                    let _ = cfg.save();
                }
                return cfg;
            }
        }
        let stamps = Self::default_game_stamps_root();
        Self {
            library_root: Self::default_root(),
            // Don't auto-link install until the user confirms in the wizard.
            game_install: None,
            game_stamps_root: stamps,
            mirror_to_game: true,
            language: default_language(),
            // Only brand-new installs (no config file) open the wizard.
            setup_complete: false,
            recent_packs: Vec::new(),
        }
    }

    /// Insert `path` at the front of the recent list, dedupe, and cap to [`RECENT_PACKS_MAX`].
    pub fn push_recent_pack(&mut self, path: &Path) {
        self.recent_packs.retain(|p| p != path);
        self.recent_packs.insert(0, path.to_path_buf());
        self.recent_packs.truncate(RECENT_PACKS_MAX);
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = Self::config_path().parent() {
            let _ = fs::create_dir_all(parent);
        }
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

    /// True if `path` is the live stamps folder or a subfolder of it.
    pub fn is_game_stamps_path(path: &Path, game_stamps_root: &Path) -> bool {
        let Ok(path) = path.canonicalize() else {
            return path_looks_like_documents_stamps(path);
        };
        let Ok(root) = game_stamps_root.canonicalize() else {
            return path_looks_like_documents_stamps(&path);
        };
        path == root || path.starts_with(&root)
    }
}

fn path_looks_like_documents_stamps(path: &Path) -> bool {
    let s = path.to_string_lossy().to_lowercase();
    s.contains("anno 117") && s.contains("stamps") && s.contains("documents")
}

/// Copy pack JSON files from the live game stamps folder into `library_root`.
/// - Always copies files that are missing in the library.
/// - If `overwrite_newer`, also replaces library files when the game copy is newer (mtime).
pub fn seed_library_from_game(library_root: &Path, game_stamps_root: &Path) -> Result<usize, String> {
    import_packs_from_game(library_root, game_stamps_root, false)
}

pub fn import_packs_from_game(
    library_root: &Path,
    game_stamps_root: &Path,
    overwrite_newer: bool,
) -> Result<usize, String> {
    if !game_stamps_root.is_dir() {
        return Ok(0);
    }
    fs::create_dir_all(library_root).map_err(|e| e.to_string())?;
    let mut copied = 0usize;
    let packs = scan_library(game_stamps_root);
    for pack in packs {
        let rel = pack
            .path
            .strip_prefix(game_stamps_root)
            .unwrap_or(Path::new(&pack.file_name));
        let dest = library_root.join(rel);
        if dest.exists() {
            if !overwrite_newer {
                continue;
            }
            let src_meta = fs::metadata(&pack.path).map_err(|e| e.to_string())?;
            let dst_meta = fs::metadata(&dest).map_err(|e| e.to_string())?;
            let src_mtime = src_meta.modified().ok();
            let dst_mtime = dst_meta.modified().ok();
            match (src_mtime, dst_mtime) {
                (Some(s), Some(d)) if s <= d => continue,
                _ => {}
            }
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(&pack.path, &dest).map_err(|e| e.to_string())?;
        copied += 1;
    }
    Ok(copied)
}

/// Copy one library pack file into the live game stamps folder (same relative path).
pub fn mirror_pack_to_game(
    library_root: &Path,
    game_stamps_root: &Path,
    pack_path: &Path,
) -> Result<PathBuf, String> {
    if !pack_path.is_file() {
        return Err("Pack-Datei fehlt".into());
    }
    let rel = pack_path
        .strip_prefix(library_root)
        .map(|p| p.to_path_buf())
        .map_err(|_| {
            format!(
                "Pack liegt nicht unter der Bibliothek: {}",
                pack_path.display()
            )
        })?;
    let dest = game_stamps_root.join(&rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(pack_path, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Overwrite-copy library packs into the live game stamps folder (same relative paths).
/// Call while the game is closed. Ubisoft sync may still wipe unknown stamps on next start —
/// prefer creating a stamp in-game first, then editing it here.
pub fn export_packs_to_game(
    library_root: &Path,
    game_stamps_root: &Path,
    packs: &[LibraryPack],
) -> Result<usize, String> {
    if packs.is_empty() {
        return Ok(0);
    }
    fs::create_dir_all(game_stamps_root).map_err(|e| e.to_string())?;
    let mut exported = 0usize;
    for pack in packs {
        let rel = pack
            .path
            .strip_prefix(library_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| {
                PathBuf::from(if pack.folder.is_empty() {
                    pack.file_name.clone()
                } else {
                    format!("{}/{}", pack.folder, pack.file_name)
                })
            });
        let dest = game_stamps_root.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(&pack.path, &dest).map_err(|e| e.to_string())?;
        exported += 1;
    }
    Ok(exported)
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

const REGION_LIBRARY_FOLDERS: &[&str] = &[
    "Region Roman",
    "Region Celtic",
    "Region Egyptian",
];

fn is_allowed_region_folder(name: &str) -> bool {
    REGION_LIBRARY_FOLDERS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(name))
}

/// Ensure only Anno region folders exist at library root.
/// Creates the three region dirs; moves stray packs into region folders;
/// removes empty leftovers like `unsorted` (game wipe-syncs those anyway).
pub fn ensure_region_library_layout(library_root: &Path) {
    if !library_root.is_dir() {
        let _ = fs::create_dir_all(library_root);
    }
    for name in REGION_LIBRARY_FOLDERS {
        let _ = fs::create_dir_all(library_root.join(name));
    }
    let packs = scan_library(library_root);
    let _ = sort_packs_by_region(library_root, &packs);
    let Ok(entries) = fs::read_dir(library_root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_allowed_region_folder(&name) {
            continue;
        }
        // Only remove if empty after sort (don't nuke unknown nested work).
        let path = entry.path();
        let empty = fs::read_dir(&path)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if empty {
            let _ = fs::remove_dir(&path);
        }
    }
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

/// Move packs into Anno's region folders (`Region Roman` / `Region Celtic` / …).
/// The game wipe-syncs `stamps/` on startup and drops anything outside these dirs.
pub fn sort_packs_by_region(root: &Path, packs: &[LibraryPack]) -> Result<usize, String> {
    use crate::catalog::RegionFilter;
    let mut moved = 0usize;
    for pack in packs {
        let region_folder = RegionFilter::game_stamp_folder_for_guid(pack.region_hint);
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

