use crate::stamp::{
    decode_pipette_b64, encode_pipette_b64, CisternMigrateMode, Stamp, StampError,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PackError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Stamp(#[from] StampError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StampEntryJson {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "FolderName", default)]
    pub folder_name: String,
    #[serde(rename = "RegionGUID", default)]
    pub region_guid: u32,
    #[serde(rename = "PipetteData", default)]
    pub pipette_data: String,
    #[serde(rename = "IconGUID", default)]
    pub icon_guid: u32,
    #[serde(rename = "CreationTime", default)]
    pub creation_time: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StampPackJson {
    #[serde(rename = "Version")]
    pub version: u32,
    #[serde(rename = "Stamps")]
    pub stamps: Vec<StampEntryJson>,
}

#[derive(Debug, Clone)]
pub struct OpenStamp {
    pub pack_path: std::path::PathBuf,
    pub entry_index: usize,
    pub entry_name: String,
    pub region_guid: u32,
    pub icon_guid: u32,
    pub stamp: Stamp,
    pub dirty: bool,
    /// Pack file does not exist yet (created via „Neues Stamp“).
    pub is_new: bool,
}

pub fn load_pack(path: &Path) -> Result<StampPackJson, PackError> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save_pack(path: &Path, pack: &StampPackJson) -> Result<(), PackError> {
    let text = serde_json::to_string_pretty(pack)?;
    fs::write(path, text + "\n")?;
    Ok(())
}

pub fn open_first_real_stamp(path: &Path) -> Result<OpenStamp, PackError> {
    let pack = load_pack(path)?;
    for (idx, entry) in pack.stamps.iter().enumerate() {
        if entry.pipette_data.is_empty() {
            continue;
        }
        let stamp = decode_pipette_b64(&entry.pipette_data)?;
        let dirty = stamp.cistern_fixes_on_load > 0;
        return Ok(OpenStamp {
            pack_path: path.to_path_buf(),
            entry_index: idx,
            entry_name: entry.name.clone(),
            region_guid: entry.region_guid,
            icon_guid: entry.icon_guid,
            stamp,
            dirty,
            is_new: false,
        });
    }
    Err(PackError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no stamp with PipetteData in pack",
    )))
}

pub fn open_named_stamp(path: &Path, name: &str) -> Result<OpenStamp, PackError> {
    let pack = load_pack(path)?;
    for (idx, entry) in pack.stamps.iter().enumerate() {
        if entry.name == name && !entry.pipette_data.is_empty() {
            let stamp = decode_pipette_b64(&entry.pipette_data)?;
            let dirty = stamp.cistern_fixes_on_load > 0;
            return Ok(OpenStamp {
                pack_path: path.to_path_buf(),
                entry_index: idx,
                entry_name: entry.name.clone(),
                region_guid: entry.region_guid,
                icon_guid: entry.icon_guid,
                stamp,
                dirty,
                is_new: false,
            });
        }
    }
    Err(PackError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("stamp not found: {name}"),
    )))
}

#[derive(Debug, Clone)]
pub struct StampListEntry {
    pub name: String,
    pub region_guid: u32,
    pub folder_name: String,
}

pub fn list_stamps(path: &Path) -> Result<Vec<StampListEntry>, PackError> {
    let pack = load_pack(path)?;
    Ok(pack
        .stamps
        .iter()
        .filter(|s| !s.pipette_data.is_empty())
        .map(|s| StampListEntry {
            name: s.name.clone(),
            region_guid: s.region_guid,
            folder_name: s.folder_name.clone(),
        })
        .collect())
}

pub fn list_stamp_names(path: &Path) -> Result<Vec<String>, PackError> {
    Ok(list_stamps(path)?.into_iter().map(|s| s.name).collect())
}

pub fn save_open_stamp(open: &OpenStamp) -> Result<(), PackError> {
    let encoded = encode_pipette_b64(&open.stamp)?;
    if open.is_new || !open.pack_path.exists() {
        if let Some(parent) = open.pack_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let pack = StampPackJson {
            version: 1,
            stamps: vec![StampEntryJson {
                name: open.entry_name.clone(),
                folder_name: String::new(),
                region_guid: open.region_guid,
                pipette_data: encoded,
                icon_guid: open.icon_guid,
                creation_time: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            }],
        };
        return save_pack(&open.pack_path, &pack);
    }
    let mut pack = load_pack(&open.pack_path)?;
    // Resolve by name — entry_index goes stale when other stamps are deleted
    // or reordered while this stamp stays open.
    let Some(entry) = pack
        .stamps
        .iter_mut()
        .find(|s| s.name == open.entry_name)
    else {
        return Err(PackError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("stamp {:?} missing in pack", open.entry_name),
        )));
    };
    entry.pipette_data = encoded;
    entry.region_guid = open.region_guid;
    save_pack(&open.pack_path, &pack)
}

/// In-memory blank stamp; file is created on first Speichern.
pub fn new_blank_stamp(
    pack_path: std::path::PathBuf,
    name: impl Into<String>,
    region_guid: u32,
) -> Result<OpenStamp, PackError> {
    let stamp = Stamp::blank()?;
    Ok(OpenStamp {
        pack_path,
        entry_index: 0,
        entry_name: name.into(),
        region_guid,
        icon_guid: 0,
        stamp,
        dirty: true,
        is_new: true,
    })
}

fn unique_stamp_name(pack: &StampPackJson, base: &str) -> String {
    if !pack.stamps.iter().any(|s| s.name == base) {
        return base.to_string();
    }
    for n in 2..10_000 {
        let candidate = format!("{base} {n}");
        if !pack.stamps.iter().any(|s| s.name == candidate) {
            return candidate;
        }
    }
    format!(
        "{base} {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    )
}

/// Append a blank stamp entry to an existing pack and return it open.
pub fn append_blank_stamp(
    path: &Path,
    name: &str,
    region_guid: u32,
) -> Result<OpenStamp, PackError> {
    let mut pack = load_pack(path)?;
    let entry_name = unique_stamp_name(&pack, name);
    let stamp = Stamp::blank()?;
    let encoded = encode_pipette_b64(&stamp)?;
    pack.stamps.push(StampEntryJson {
        name: entry_name.clone(),
        folder_name: String::new(),
        region_guid,
        pipette_data: encoded,
        icon_guid: 0,
        creation_time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    let entry_index = pack.stamps.len() - 1;
    save_pack(path, &pack)?;
    Ok(OpenStamp {
        pack_path: path.to_path_buf(),
        entry_index,
        entry_name,
        region_guid,
        icon_guid: 0,
        stamp,
        dirty: false,
        is_new: false,
    })
}

/// Duplicate a stamp entry inside the same pack.
pub fn duplicate_stamp(path: &Path, name: &str) -> Result<OpenStamp, PackError> {
    let mut pack = load_pack(path)?;
    let Some(src) = pack.stamps.iter().find(|s| s.name == name).cloned() else {
        return Err(PackError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("stamp not found: {name}"),
        )));
    };
    let mut copy = src;
    copy.name = unique_stamp_name(&pack, &format!("{} Kopie", name));
    copy.creation_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry_name = copy.name.clone();
    let region_guid = copy.region_guid;
    let icon_guid = copy.icon_guid;
    let stamp = decode_pipette_b64(&copy.pipette_data)?;
    pack.stamps.push(copy);
    let entry_index = pack.stamps.len() - 1;
    save_pack(path, &pack)?;
    Ok(OpenStamp {
        pack_path: path.to_path_buf(),
        entry_index,
        entry_name,
        region_guid,
        icon_guid,
        stamp,
        dirty: false,
        is_new: false,
    })
}

/// Remove a stamp entry from a pack (deletes pack file if it becomes empty).
pub fn delete_stamp_entry(path: &Path, name: &str) -> Result<(), PackError> {
    let mut pack = load_pack(path)?;
    let before = pack.stamps.len();
    pack.stamps.retain(|s| s.name != name);
    if pack.stamps.len() == before {
        return Err(PackError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("stamp not found: {name}"),
        )));
    }
    if pack.stamps.is_empty() {
        fs::remove_file(path)?;
    } else {
        save_pack(path, &pack)?;
    }
    Ok(())
}

/// Move a stamp entry into another pack (copy + delete source). Same pack = no-op.
pub fn move_stamp_to_pack(
    src_pack: &Path,
    src_name: &str,
    dest_pack: &Path,
) -> Result<String, PackError> {
    if src_pack == dest_pack {
        return Ok(src_name.to_string());
    }
    let new_name = copy_stamp_to_pack(src_pack, src_name, dest_pack)?;
    delete_stamp_entry(src_pack, src_name)?;
    Ok(new_name)
}

/// Copy a stamp entry into another pack (or create the target pack).
pub fn copy_stamp_to_pack(
    src_pack: &Path,
    src_name: &str,
    dest_pack: &Path,
) -> Result<String, PackError> {
    let src = load_pack(src_pack)?;
    let Some(entry) = src.stamps.iter().find(|s| s.name == src_name).cloned() else {
        return Err(PackError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("stamp not found: {src_name}"),
        )));
    };
    let mut dest = if dest_pack.exists() {
        load_pack(dest_pack)?
    } else {
        if let Some(parent) = dest_pack.parent() {
            fs::create_dir_all(parent)?;
        }
        StampPackJson {
            version: 1,
            stamps: Vec::new(),
        }
    };
    let mut copy = entry;
    copy.name = unique_stamp_name(&dest, &copy.name);
    let out_name = copy.name.clone();
    dest.stamps.push(copy);
    save_pack(dest_pack, &dest)?;
    Ok(out_name)
}

/// Result of a batch cistern-pivot repair over stamp packs.
#[derive(Debug, Clone, Default)]
pub struct CisternFixReport {
    pub packs_scanned: usize,
    pub stamps_scanned: usize,
    pub stamps_rewritten: usize,
    pub buildings_fixed: usize,
    pub errors: Vec<String>,
}

/// Scan `root` recursively for `*.json` stamp packs, Force-migrate unmarked
/// aqueduct cisterns, and rewrite packs that changed.
pub fn fix_cistern_pivots_in_tree(root: &Path) -> CisternFixReport {
    let mut report = CisternFixReport::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(err) => {
                report
                    .errors
                    .push(format!("{}: {err}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match fix_cistern_pivots_in_pack(&path) {
                Ok((stamps, rewritten, fixed)) => {
                    report.packs_scanned += 1;
                    report.stamps_scanned += stamps;
                    report.stamps_rewritten += rewritten;
                    report.buildings_fixed += fixed;
                }
                Err(err) => {
                    // Not every JSON under the library is a stamp pack.
                    let msg = err.to_string();
                    if msg.contains("missing field")
                        || msg.contains("Stamps")
                        || msg.contains("expected")
                    {
                        continue;
                    }
                    report
                        .errors
                        .push(format!("{}: {msg}", path.display()));
                }
            }
        }
    }
    report
}

fn fix_cistern_pivots_in_pack(path: &Path) -> Result<(usize, usize, usize), PackError> {
    let mut pack = load_pack(path)?;
    let mut stamps = 0usize;
    let mut rewritten = 0usize;
    let mut fixed = 0usize;
    let mut changed = false;
    for entry in &mut pack.stamps {
        if entry.pipette_data.is_empty() {
            continue;
        }
        stamps += 1;
        let original = entry.pipette_data.clone();
        let mut stamp = decode_pipette_b64(&original)?;
        let auto_fixed = stamp.cistern_fixes_on_load;
        // Catch unmarked cisterns Auto left alone (e.g. roads touch both TLs).
        let force_fixed = stamp.migrate_cistern_pivots(CisternMigrateMode::Force);
        let n = auto_fixed + force_fixed;
        // Only rewrite when a cistern Pos actually moved — avoid zlib/pretty
        // re-encoding of unrelated stamps.
        if n > 0 {
            entry.pipette_data = encode_pipette_b64(&stamp)?;
            rewritten += 1;
            fixed += n;
            changed = true;
        }
    }
    if changed {
        save_pack(path, &pack)?;
    }
    Ok((stamps, rewritten, fixed))
}

/// Convenience for CLI: fix one pack path (file) or a directory tree.
pub fn fix_cistern_pivots_path(path: &Path) -> CisternFixReport {
    if path.is_file() {
        let mut report = CisternFixReport::default();
        match fix_cistern_pivots_in_pack(path) {
            Ok((stamps, rewritten, fixed)) => {
                report.packs_scanned = 1;
                report.stamps_scanned = stamps;
                report.stamps_rewritten = rewritten;
                report.buildings_fixed = fixed;
            }
            Err(err) => {
                report.errors.push(format!("{}: {err}", path.display()));
            }
        }
        return report;
    }
    fix_cistern_pivots_in_tree(path)
}

