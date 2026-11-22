//! Load Anno UI icons: PNG cache under `_gamedata/ui_png`, filled from `ui.rda` (lazy + bulk).

use crate::i18n::{self, Language};
use crate::library::EditorConfig;
use crate::rda::{
    cache_rel_from_rda_key, candidate_rda_keys, is_bulk_icon_key, open_ui_rda, RdaSet, SharedRda,
};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// PNG icon cache: next to EXE / config / cwd `_gamedata/ui_png` (no machine-specific paths).
fn resolve_ui_png_root() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("_gamedata").join("ui_png"));
            candidates.push(dir.join("ui_png"));
            candidates.push(dir.join("../../..").join("_gamedata").join("ui_png"));
        }
    }
    candidates.push(EditorConfig::config_dir().join("_gamedata").join("ui_png"));
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("_gamedata").join("ui_png"));
        candidates.push(cwd.join("..").join("_gamedata").join("ui_png"));
        candidates.push(cwd.join("../..").join("_gamedata").join("ui_png"));
    }
    candidates
        .into_iter()
        .find(|p| p.is_dir())
        .unwrap_or_else(|| EditorConfig::config_dir().join("_gamedata").join("ui_png"))
}

pub const ICON_CRANE: &str =
    "data/ui/fhd/base/icon_content/infrastructure/icon_3d_construction_crane.png";

pub const ICON_ACCESS_BUILDINGS: &str =
    "data/ui/fhd/base/icon_content/building/icon_3d_construction_main_group_construction.png";

/// Extracted construction-bar chrome (under `ui_png/chrome/`).
pub mod chrome {
    pub const BAR_BG: &str = "bgr_construction_bar.png";
    pub const BTN_NORMAL: &str = "btn_construction_normal.png";
    pub const BTN_SELECTED: &str = "btn_construction_selected.png";
    pub const CHAIN_NORMAL: &str = "btn_constr_chain_normal.png";
    pub const CHAIN_SELECTED: &str = "btn_constr_chain_selected.png";
    pub const CHAIN_FRAME: &str = "bgr_constr_chain_frame.png";
    pub const ARC_BG: &str = "bg_arc_constructionmenu_64x16_9s.png";
}

#[derive(Debug, Clone, Default)]
pub struct IconExtractStatus {
    pub running: bool,
    pub done: u32,
    pub total: u32,
    pub message: String,
}

#[derive(Default)]
pub struct UiIcons {
    textures: HashMap<String, TextureHandle>,
    missing: HashSet<String>,
    root: PathBuf,
    rda: Option<Arc<RdaSet>>,
    status: Arc<Mutex<IconExtractStatus>>,
    bulk_started: Arc<AtomicBool>,
}

impl UiIcons {
    pub fn new() -> Self {
        Self {
            textures: HashMap::new(),
            missing: HashSet::new(),
            root: resolve_ui_png_root(),
            rda: None,
            status: Arc::new(Mutex::new(IconExtractStatus::default())),
            bulk_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cache_root(&self) -> &Path {
        &self.root
    }

    pub fn extract_status(&self) -> IconExtractStatus {
        self.status.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Attach UI RDAs (patches + ui.rda) from a game install and kick off bulk extract.
    pub fn attach_game_install(&mut self, game_install: &Path, lang: Language) {
        match open_ui_rda(game_install) {
            Ok((rda, summary)) => {
                let n = rda.len();
                self.rda = Some(rda.clone());
                // Retry paths that failed when only base ui.rda was linked.
                self.missing.clear();
                if let Ok(mut st) = self.status.lock() {
                    st.message = i18n::tr_fmt(
                        lang,
                        "icon.rda_indexed",
                        &[("n", &n.to_string()), ("summary", &summary)],
                    );
                }
                self.bulk_started.store(false, Ordering::SeqCst);
                self.start_bulk_extract(lang);
            }
            Err(e) => {
                if let Ok(mut st) = self.status.lock() {
                    st.message =
                        i18n::tr_fmt(lang, "icon.rda_error", &[("err", &e.to_string())]);
                }
            }
        }
    }

    pub fn clear_game_install(&mut self) {
        self.rda = None;
        self.bulk_started.store(false, Ordering::SeqCst);
    }

    fn start_bulk_extract(&self, lang: Language) {
        let Some(rda) = self.rda.clone() else {
            return;
        };
        if self
            .bulk_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let root = self.root.clone();
        let status = self.status.clone();
        let started = self.bulk_started.clone();
        thread::Builder::new()
            .name("ui-icon-bulk".into())
            .spawn(move || {
                let mut seen_rel = HashSet::new();
                let mut keys: Vec<(SharedRda, String)> = Vec::new();
                let mut all: Vec<(SharedRda, String)> = Vec::new();
                for arch in rda.archives() {
                    for k in arch.keys() {
                        if !is_bulk_icon_key(k) {
                            continue;
                        }
                        let ks = k.as_str();
                        if !(ks.contains("/4k/")
                            || ks.contains("/construction_bar/")
                            || ks.contains("/ds/icons/"))
                        {
                            continue;
                        }
                        all.push((arch.clone(), k.clone()));
                    }
                }
                // Prefer 4k paths when both map to the same cache file.
                all.sort_by(|a, b| {
                    let score = |k: &str| {
                        if k.contains("/4k/") {
                            0
                        } else if k.contains("/construction_bar/") {
                            1
                        } else {
                            2
                        }
                    };
                    score(&a.1)
                        .cmp(&score(&b.1))
                        .then_with(|| a.1.cmp(&b.1))
                });
                for (arch, k) in all {
                    if let Some(rel) = cache_rel_from_rda_key(&k) {
                        if seen_rel.insert(rel) {
                            keys.push((arch, k));
                        }
                    }
                }
                let total = keys.len() as u32;
                if let Ok(mut st) = status.lock() {
                    st.running = true;
                    st.done = 0;
                    st.total = total;
                    st.message = i18n::tr_fmt(
                        lang,
                        "icon.extract_progress",
                        &[("done", "0"), ("total", &total.to_string())],
                    );
                }
                for (i, (arch, key)) in keys.iter().enumerate() {
                    let _ = extract_rda_key_to_cache(arch, &root, key, None);
                    if let Ok(mut st) = status.lock() {
                        st.done = (i + 1) as u32;
                        if i % 25 == 0 || i + 1 == keys.len() {
                            st.message = i18n::tr_fmt(
                                lang,
                                "icon.extract_progress",
                                &[
                                    ("done", &(i + 1).to_string()),
                                    ("total", &total.to_string()),
                                ],
                            );
                        }
                    }
                }
                if let Ok(mut st) = status.lock() {
                    st.running = false;
                    st.message = i18n::tr_fmt(
                        lang,
                        "icon.extract_done",
                        &[("total", &total.to_string())],
                    );
                }
                let _ = started; // keep flag true so we don't re-bulk every frame
            })
            .ok();
    }

    /// Map a game `IconFilename` (`data/ui/fhd/.../foo.png`) to a local PNG path.
    pub fn local_path(root: &Path, game_path: &str) -> Option<PathBuf> {
        let norm = game_path.replace('\\', "/");
        let name = Path::new(&norm).file_name()?.to_string_lossy().into_owned();
        let alt_name = name
            .strip_suffix("_constructionmenu.png")
            .map(|s| format!("{s}.png"));
        if let Some(idx) = norm.find("/icon_content/") {
            let rest = &norm[idx + "/icon_content/".len()..];
            let p = root.join(rest);
            if p.exists() {
                return Some(p);
            }
            if let Some(alt) = &alt_name {
                let parent = p.parent().unwrap_or(root);
                let p = parent.join(alt);
                if p.exists() {
                    return Some(p);
                }
            }
            let folder = rest.split('/').next().unwrap_or("");
            if !folder.is_empty() {
                let p = root.join(folder).join(&name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        if norm.contains("/ds/icons/") || name.starts_with("icon_2d_numeric_") {
            let p = root.join("numeric").join(&name);
            if p.exists() {
                return Some(p);
            }
        }
        let aliases: &[(&str, &str)] = &[
            (
                "icon_3d_cheese_albion_goods.png",
                "production_goods/icon_3d_cheese_goods.png",
            ),
            (
                "icon_3d_cheese_egypt_goods.png",
                "production_goods/icon_3d_cheese_goods.png",
            ),
            (
                "icon_3d_construction_category_stamp.png",
                "construction_groups/icon_construction_category_tools.png",
            ),
            ("icon_2d_stamp.png", "tools/icon_2d_tools.png"),
            (
                "icon_3d_shrine_sphinx.png",
                "ornaments/shrines/icon_3d_shrine_isis.png",
            ),
            (
                "icon_3d_water_irrigation.png",
                "infrastructure/icon_3d_water_canal.png",
            ),
            (
                "icon_3d_water_basin.png",
                "infrastructure/icon_3d_water_canal.png",
            ),
        ];
        for (from, to) in aliases {
            if name == *from {
                let p = root.join(to);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        for folder in [
            "production_goods",
            "building",
            "infrastructure",
            "portraits",
            "construction_groups",
            "tools",
            "generic",
            "need_categories",
            "workforce",
            "ornaments",
            "ornaments/shrines",
            "colosseum",
            "hippodrome",
            "stamp",
            "granary",
            "numeric",
            "chrome",
        ] {
            let p = root.join(folder).join(&name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    fn ensure_cached(&mut self, game_or_chrome: &str, chrome: bool) -> Option<PathBuf> {
        if chrome {
            let local = self.root.join("chrome").join(game_or_chrome);
            if local.exists() {
                return Some(local);
            }
        } else if let Some(p) = Self::local_path(&self.root, game_or_chrome) {
            return Some(p);
        }

        let rda = self.rda.as_ref()?;
        let candidates = candidate_rda_keys(game_or_chrome);
        let (arch, key) = rda.resolve(&candidates)?;
        let max = if chrome { Some(1024) } else { Some(96) };
        extract_rda_key_to_cache(&arch, &self.root, &key, max).ok()
    }

    pub fn chrome(&mut self, ctx: &egui::Context, filename: &str) -> Option<TextureHandle> {
        let key = format!("chrome/{filename}");
        if let Some(t) = self.textures.get(&key) {
            return Some(t.clone());
        }
        if self.missing.contains(&key) {
            return None;
        }
        let local = match self.ensure_cached(filename, true) {
            Some(p) => p,
            None => {
                self.missing.insert(key);
                return None;
            }
        };
        match load_texture(ctx, &key, &local) {
            Some(tex) => {
                self.textures.insert(key, tex.clone());
                Some(tex)
            }
            None => {
                self.missing.insert(key);
                None
            }
        }
    }

    pub fn get(&mut self, ctx: &egui::Context, game_path: Option<&str>) -> Option<TextureHandle> {
        let path = game_path?.trim();
        if path.is_empty() {
            return None;
        }
        if let Some(t) = self.textures.get(path) {
            return Some(t.clone());
        }
        if self.missing.contains(path) {
            return None;
        }
        let local = match self.ensure_cached(path, false) {
            Some(p) => p,
            None => {
                self.missing.insert(path.to_string());
                return None;
            }
        };
        match load_texture(ctx, path, &local) {
            Some(tex) => {
                self.textures.insert(path.to_string(), tex.clone());
                Some(tex)
            }
            None => {
                self.missing.insert(path.to_string());
                None
            }
        }
    }

    pub fn get_or_placeholder(
        &mut self,
        ctx: &egui::Context,
        game_path: Option<&str>,
        size: [usize; 2],
        tint: [u8; 3],
    ) -> TextureHandle {
        if let Some(t) = self.get(ctx, game_path) {
            return t;
        }
        let key = format!(
            "__ph_{}_{}_{}_{}x{}",
            tint[0], tint[1], tint[2], size[0], size[1]
        );
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let mut rgba = vec![0u8; size[0] * size[1] * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = tint[0];
            px[1] = tint[1];
            px[2] = tint[2];
            px[3] = 220;
        }
        let img = ColorImage::from_rgba_unmultiplied(size, &rgba);
        let tex = ctx.load_texture(key.clone(), img, TextureOptions::LINEAR);
        self.textures.insert(key, tex.clone());
        tex
    }
}

fn dds_to_rgba(bytes: &[u8]) -> Result<RgbaImage, String> {
    let dds = image_dds::ddsfile::Dds::read(&mut std::io::Cursor::new(bytes))
        .map_err(|e| e.to_string())?;
    let img = image_dds::image_from_dds(&dds, 0).map_err(|e| e.to_string())?;
    Ok(img)
}

fn extract_rda_key_to_cache(
    rda: &SharedRda,
    root: &Path,
    key: &str,
    max_edge: Option<u32>,
) -> Result<PathBuf, String> {
    let rel = cache_rel_from_rda_key(key).ok_or_else(|| format!("no cache mapping for {key}"))?;
    let out = root.join(&rel);
    if out.exists() {
        return Ok(out);
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = rda.read_file(key)?;
    let mut img = dds_to_rgba(&bytes)?;
    if let Some(max) = max_edge {
        let w = img.width();
        let h = img.height();
        if w > max || h > max {
            let scale = (max as f32 / w.max(h) as f32).min(1.0);
            let nw = ((w as f32) * scale).round().max(1.0) as u32;
            let nh = ((h as f32) * scale).round().max(1.0) as u32;
            img = DynamicImage::ImageRgba8(img)
                .resize_exact(nw, nh, FilterType::Lanczos3)
                .into_rgba8();
        }
    }
    img.save(&out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Crop near-transparent padding so glyphs sit centered when drawn in a circle.
fn trim_transparent(img: RgbaImage) -> RgbaImage {
    let w = img.width();
    let h = img.height();
    if w == 0 || h == 0 {
        return img;
    }
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > 24 {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !any || (min_x == 0 && min_y == 0 && max_x + 1 == w && max_y + 1 == h) {
        return img;
    }
    // Keep a little padding so soft edges aren't clipped.
    let pad = ((w.max(h) as f32) * 0.04).round() as u32;
    let x0 = min_x.saturating_sub(pad);
    let y0 = min_y.saturating_sub(pad);
    let x1 = (max_x + 1 + pad).min(w);
    let y1 = (max_y + 1 + pad).min(h);
    if x1 <= x0 || y1 <= y0 {
        return img;
    }
    image::imageops::crop_imm(&img, x0, y0, x1 - x0, y1 - y0).to_image()
}

fn load_texture(ctx: &egui::Context, id: &str, path: &Path) -> Option<TextureHandle> {
    let img = image::open(path).ok()?.into_rgba8();
    // Chrome frames keep full canvas; game icons get content-trimmed for circle tiles.
    let img = if path.components().any(|c| c.as_os_str() == "chrome") {
        img
    } else {
        trim_transparent(img)
    };
    let size = [img.width() as usize, img.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(id.to_string(), color, TextureOptions::LINEAR))
}
