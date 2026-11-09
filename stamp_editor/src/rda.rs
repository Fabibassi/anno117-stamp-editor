//! Minimal Anno 117 RDA (Resource File V2.2) reader for UI icon extraction.

use flate2::read::ZlibDecoder;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ENTRY: usize = 560;

#[derive(Debug, Clone, Copy)]
struct RdaEntry {
    offset: u64,
    compressed: u64,
    uncompressed: u64,
}

/// Index over one `.rda` file (file re-opened for each read).
pub struct RdaArchive {
    path: PathBuf,
    /// Lowercase forward-slash path → entry.
    index: HashMap<String, RdaEntry>,
}

impl RdaArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();
        if file_len < 800 {
            return Err("RDA too small".into());
        }
        let mut hdr = [0u8; 8];
        file.seek(SeekFrom::Start(784))
            .map_err(|e| e.to_string())?;
        file.read_exact(&mut hdr).map_err(|e| e.to_string())?;
        let mut block = u64::from_le_bytes(hdr);
        let mut index = HashMap::new();
        let mut seen = std::collections::HashSet::new();

        while block != 0 && seen.insert(block) && block + 32 <= file_len {
            file.seek(SeekFrom::Start(block))
                .map_err(|e| e.to_string())?;
            let mut block_hdr = [0u8; 32];
            file.read_exact(&mut block_hdr).map_err(|e| e.to_string())?;
            let count = u32::from_le_bytes(block_hdr[4..8].try_into().unwrap()) as usize;
            let csize = u64::from_le_bytes(block_hdr[8..16].try_into().unwrap());
            let usize_ = u64::from_le_bytes(block_hdr[16..24].try_into().unwrap());
            let nxt = u64::from_le_bytes(block_hdr[24..32].try_into().unwrap());

            if count > 0 && csize > 0 && block >= csize {
                let mut raw = vec![0u8; csize as usize];
                file.seek(SeekFrom::Start(block - csize))
                    .map_err(|e| e.to_string())?;
                file.read_exact(&mut raw).map_err(|e| e.to_string())?;
                if csize != usize_ {
                    let mut dec = ZlibDecoder::new(&raw[..]);
                    let mut out = Vec::new();
                    if dec.read_to_end(&mut out).is_ok() {
                        raw = out;
                    }
                }
                let n = count.min(raw.len() / ENTRY);
                for i in 0..n {
                    let e = &raw[i * ENTRY..(i + 1) * ENTRY];
                    let path = decode_utf16le_path(&e[0..520]);
                    if path.is_empty() {
                        continue;
                    }
                    let ptr = u64::from_le_bytes(e[520..528].try_into().unwrap());
                    let cs = u64::from_le_bytes(e[528..536].try_into().unwrap());
                    let us = u64::from_le_bytes(e[536..544].try_into().unwrap());
                    if ptr == 0 || cs == 0 {
                        continue;
                    }
                    index.insert(
                        normalize_key(&path),
                        RdaEntry {
                            offset: ptr,
                            compressed: cs,
                            uncompressed: us,
                        },
                    );
                }
            }
            if nxt == 0 || nxt >= file_len {
                break;
            }
            block = nxt;
        }

        Ok(Self { path, index })
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.index.keys()
    }

    pub fn read_file(&self, key: &str) -> Result<Vec<u8>, String> {
        let entry = self
            .index
            .get(&normalize_key(key))
            .copied()
            .ok_or_else(|| format!("not in RDA: {key}"))?;
        let mut file = File::open(&self.path).map_err(|e| e.to_string())?;
        let mut blob = vec![0u8; entry.compressed as usize];
        file.seek(SeekFrom::Start(entry.offset))
            .map_err(|e| e.to_string())?;
        file.read_exact(&mut blob).map_err(|e| e.to_string())?;
        if entry.compressed != entry.uncompressed
            || (blob.len() >= 2 && blob[0] == 0x78 && matches!(blob[1], 0x01 | 0x9c | 0xda))
        {
            let mut dec = ZlibDecoder::new(&blob[..]);
            let mut out = Vec::with_capacity(entry.uncompressed as usize);
            match dec.read_to_end(&mut out) {
                Ok(_) => Ok(out),
                Err(_) => Ok(blob),
            }
        } else {
            Ok(blob)
        }
    }

    /// Resolve first matching candidate key present in the archive.
    pub fn resolve(&self, candidates: &[String]) -> Option<String> {
        candidates
            .iter()
            .find(|k| self.index.contains_key(*k))
            .cloned()
    }
}

fn decode_utf16le_path(bytes: &[u8]) -> String {
    let mut u16s = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let v = u16::from_le_bytes([chunk[0], chunk[1]]);
        if v == 0 {
            break;
        }
        u16s.push(v);
    }
    String::from_utf16_lossy(&u16s).replace('\\', "/")
}

pub fn normalize_key(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_lowercase()
}

pub type SharedRda = Arc<RdaArchive>;

pub fn open_ui_rda(game_install: &Path) -> Result<SharedRda, String> {
    let rda = game_install.join("maindata").join("ui.rda");
    Ok(Arc::new(RdaArchive::open(rda)?))
}

fn push_unique(out: &mut Vec<String>, s: impl AsRef<str>) {
    let k = normalize_key(s.as_ref());
    if !out.iter().any(|x| x == &k) {
        out.push(k);
    }
}

fn with_dds_variants(out: &mut Vec<String>, path_no_ext: &str) {
    push_unique(out, format!("{path_no_ext}_0.dds"));
    push_unique(out, format!("{path_no_ext}.dds"));
}

/// Candidate RDA keys for a game `IconFilename` or bare chrome PNG name.
pub fn candidate_rda_keys(game_or_chrome: &str) -> Vec<String> {
    let raw = game_or_chrome.replace('\\', "/");
    let name = Path::new(&raw)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| raw.clone());
    let stem = name
        .trim_end_matches(".png")
        .trim_end_matches(".PNG")
        .trim_end_matches(".dds")
        .trim_end_matches(".DDS")
        .trim_end_matches("_0")
        .to_string();

    let mut out = Vec::new();

    if raw.contains('/') {
        let base = raw.trim_start_matches('/');
        let no_ext = base
            .strip_suffix(".png")
            .or_else(|| base.strip_suffix(".PNG"))
            .or_else(|| base.strip_suffix(".dds"))
            .unwrap_or(base);
        let no_ext = no_ext.strip_suffix("_0").unwrap_or(no_ext);

        let variants = [
            no_ext.to_string(),
            no_ext.replace("/fhd/", "/4k/"),
            no_ext.replace("/4k/", "/fhd/"),
            no_ext.replace("data/ui/fhd/", "data/ui/2kimages/fhd/"),
            no_ext.replace("data/ui/4k/", "data/ui/2kimages/4k/"),
            no_ext.replace("data/ui/fhd/", "data/ui/2kimages/4k/"),
            no_ext.replace("data/ui/fhd/", "data/ui/4k/"),
        ];
        for v in variants {
            with_dds_variants(&mut out, &v);
        }

        if let Some(idx) = base.to_lowercase().find("/icon_content/") {
            let rest = &base[idx + "/icon_content/".len()..];
            let rest = rest
                .strip_suffix(".png")
                .or_else(|| rest.strip_suffix(".dds"))
                .unwrap_or(rest);
            let rest = rest.strip_suffix("_0").unwrap_or(rest);
            for prefix in [
                "data/ui/4k/base/icon_content/",
                "data/ui/fhd/base/icon_content/",
                "data/ui/2kimages/4k/base/icon_content/",
                "data/ui/2kimages/fhd/base/icon_content/",
            ] {
                with_dds_variants(&mut out, &format!("{prefix}{rest}"));
            }
        }
    }

    for prefix in [
        "data/ui/2kimages/main/assets/construction_bar/",
        "data/ui/4k/base/features/construction_menu/btn/",
    ] {
        with_dds_variants(&mut out, &format!("{prefix}{stem}"));
    }

    if stem.starts_with("icon_2d_numeric_") {
        for prefix in [
            "data/ui/4k/base/ds/icons/",
            "data/ui/fhd/base/ds/icons/",
            "data/ui/2kimages/4k/base/ds/icons/",
        ] {
            with_dds_variants(&mut out, &format!("{prefix}{stem}"));
        }
    }

    out
}

/// Map an RDA key to a cache-relative PNG path under `ui_png/`.
pub fn cache_rel_from_rda_key(key: &str) -> Option<PathBuf> {
    let k = normalize_key(key);
    let file = Path::new(&k).file_name()?.to_string_lossy();
    let stem = file
        .trim_end_matches(".dds")
        .trim_end_matches("_0")
        .to_string();

    if let Some(idx) = k.find("/icon_content/") {
        let rest = &k[idx + "/icon_content/".len()..];
        let parent = Path::new(rest).parent().unwrap_or(Path::new(""));
        return Some(parent.join(format!("{stem}.png")));
    }
    if k.contains("/construction_bar/") || (k.contains("construction_menu") && k.contains("/btn/"))
    {
        return Some(PathBuf::from("chrome").join(format!("{stem}.png")));
    }
    if k.contains("/ds/icons/") || stem.starts_with("icon_2d_numeric_") {
        return Some(PathBuf::from("numeric").join(format!("{stem}.png")));
    }
    None
}

pub fn is_bulk_icon_key(key: &str) -> bool {
    let k = key.to_lowercase();
    if !k.ends_with(".dds") || k.contains("/console/") {
        return false;
    }
    // Prefer 4k / main; skip duplicate 2kimages/fhd when 4k exists (bulk still ok to extract both → same cache).
    k.contains("/icon_content/")
        || k.contains("/construction_bar/")
        || k.contains("/ds/icons/icon_2d_numeric_")
}
