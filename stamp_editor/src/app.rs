use stamp_editor::buildmenu::{ConstructionMenu, MenuEntry};
use stamp_editor::catalog::{Catalog, RegionFilter};
use stamp_editor::icons::{chrome, UiIcons};
use stamp_editor::coverage::{
    build_road_map_region, compute_coverage_region, compute_roads_only_region,
    footprint_cells_for_region, kind_color, layout_stats_region, CoverageMap, CoverageMode,
    RoadKind,
};
use stamp_editor::area_efficiency::{
    analyze_area_efficiency_with_fields, blocker_map, competition_group,
    contested_free_cells, is_resource_area_building, radius_cells, resource_need, resource_radius,
    BlockReason,
};
use stamp_editor::economy::analyze_chain_region;
use stamp_editor::history::{History, Snapshot};
use stamp_editor::library::{self, EditorConfig, LibraryPack};
use stamp_editor::meshgraph::{
    apply_road_path, is_road_brush_guid, orthogonal_road_path, read_mesh_info, relative_to_tile,
    ROAD_GUID_DIRT, ROAD_GUID_STONE,
};
use stamp_editor::modules::{
    complex_group_indices, complex_main_index, count_progress_for_stamp_area, dist_tiles,
    is_module_building, module_radius, needed_modules, suggested_modules,
};
use stamp_editor::pack::{
    append_blank_stamp, copy_stamp_to_pack, delete_stamp_entry, duplicate_stamp, move_stamp_to_pack,
    new_blank_stamp, open_first_real_stamp, open_named_stamp, save_open_stamp, OpenStamp,
};
use stamp_editor::stamp::Building;
use eframe::egui::{self, Color32, Frame, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct ClipboardBuilding {
    guid: u32,
    direction: i32,
    variation: Option<u32>,
    /// Footprint top-left delta from the primary's footprint top-left (tiles).
    /// Using TL deltas (not center deltas) avoids half-tile drift on odd sizes.
    dtx: i32,
    dty: i32,
    is_primary: bool,
}

#[derive(Debug, Clone, Copy)]
struct ClipboardRoad {
    /// Tile offset from the primary building's footprint top-left.
    dx: i32,
    dy: i32,
    guid: i32,
}

/// Marker written to the OS clipboard so Windows/eframe emits `Event::Paste` on Ctrl+V.
const OS_CLIPBOARD_MARKER: &str = "rusticus-stamp-clipboard-v1";

pub struct StampEditorApp {
    catalog: Catalog,
    catalog_path: PathBuf,
    open: Option<OpenStamp>,
    stamp_list_filter: String,
    /// Primary selection (coverage / analyse focus).
    selected_building: Option<usize>,
    /// Multi-select set (marquee / Ctrl+Klick). Always includes `selected_building` when set.
    selected_buildings: HashSet<usize>,
    palette_filter: String,
    /// Building search sidebar (fallback); toggled via Ansicht.
    show_palette: bool,
    /// Right-hand analysis panel; off by default so it doesn't cover the baulette.
    show_analyse: bool,
    region_filter: RegionFilter,
    palette_selected_guid: Option<u32>,
    status: String,
    cell_px: f32,
    pan: Vec2,
    coverage: CoverageMap,
    dragging_building: Option<usize>,
    drag_offset: Vec2,
    /// Multi-select drag: building positions at drag start.
    drag_group_origins: HashMap<usize, (f32, f32)>,
    /// Selected roads at drag start `(x, y, guid)`.
    drag_group_roads: Vec<(i32, i32, i32)>,
    /// Road cells currently painted for this multi-drag (erase before next frame).
    drag_group_roads_live: Vec<(i32, i32)>,
    /// World position of the dragged building at drag start.
    drag_anchor_start: (f32, f32),
    placing_guid: Option<u32>,
    /// Pipette Variation while placing (warehouse marsh = `Some(1)` → 5×5).
    place_variation: Option<u32>,
    /// Active drag stroke: road line, demolish rect, or place rect.
    drag_stroke: Option<DragStroke>,
    /// After a stroke drag, ignore the trailing `clicked()` so we don't double-apply.
    suppress_stroke_click: bool,
    /// Active road brush GUID (dirt vs Ziegel/stone). `None` = kein Weg-Pinsel.
    road_brush_guid: Option<i32>,
    /// Dedicated road eraser (orthogonal stroke always removes).
    road_erase_mode: bool,
    /// Anno-style RMB radial / quick-select wheel.
    radial: Option<RadialMenu>,
    history: History,
    history_drag_pushed: bool,
    library_root: PathBuf,
    /// Linked Anno 117 install (icons come from `maindata/ui.rda`, not redistributed).
    game_install: Option<PathBuf>,
    library_packs: Vec<LibraryPack>,
    library_clipboard: Option<LibraryClipboard>,
    move_group: bool,
    clipboard: Vec<ClipboardBuilding>,
    /// Roads copied with the last Ctrl+C (relative to primary).
    clipboard_roads: Vec<ClipboardRoad>,
    /// Absolute road cells in the current marquee selection.
    selected_roads: HashSet<(i32, i32)>,
    /// True while the hand was armed from Ablage (place whole group + roads).
    hand_from_clipboard: bool,
    /// Last canvas pointer in world tiles (for Ctrl+V paste under cursor).
    last_pointer_world: Option<(f32, f32)>,
    /// When placing a module, assign this parent building index's ComplexOwnerID.
    module_parent_index: Option<usize>,
    /// Building under pointer on the canvas (for complex outline + Untergebäude).
    hover_building: Option<usize>,
    /// Menu „Löschen“ sets this; consumed next frame with Entf/Backspace path.
    request_delete: bool,
    /// When false (default): snap to whole tiles, R rotates 90°.
    /// When true: half-tile snap, R rotates 45°.
    allow_45_deg: bool,
    buildmenu: ConstructionMenu,
    buildmenu_tab: Option<usize>,
    buildmenu_entry: Option<usize>,
    /// Selected entry inside the infrastructure left strip (category/building).
    buildmenu_infra_entry: Option<usize>,
    ui_icons: UiIcons,
    /// Debounced autosave deadline.
    autosave_at: Option<Instant>,
    /// Multi-select in the left library tree.
    library_selection: HashSet<LibItem>,
    /// Marquee selection: start → current pointer (library scroll coords).
    library_marquee: Option<(Pos2, Pos2)>,
    /// Last seen library folder fingerprint (path+mtime+len of pack JSONs).
    library_fingerprint: u64,
    /// Next time to re-check the library folder for external changes.
    library_watch_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrokeKind {
    /// Orthogonal road paint/toggle-erase along a line.
    Road { paint_on: bool },
    /// Axis-aligned demolish box (roads + buildings).
    DemolishRect,
    /// Axis-aligned place grid for `placing_guid`.
    PlaceRect,
    /// Empty-hand marquee multi-select (Windows-desktop style).
    SelectRect,
}

/// One LMB drag gesture: start → cursor, committed on release.
#[derive(Debug, Clone, Copy)]
struct DragStroke {
    start: (i32, i32),
    cursor: (i32, i32),
    kind: StrokeKind,
}

fn rect_bounds(a: (i32, i32), b: (i32, i32)) -> (i32, i32, i32, i32) {
    (
        a.0.min(b.0),
        a.1.min(b.1),
        a.0.max(b.0),
        a.1.max(b.1),
    )
}

fn rect_cells(min_x: i32, min_y: i32, max_x: i32, max_y: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::with_capacity(((max_x - min_x + 1) * (max_y - min_y + 1)) as usize);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            out.push((x, y));
        }
    }
    out
}

/// Building center so the footprint's top-left tile is `(tx, ty)`.
fn place_center_from_top_left(tx: i32, ty: i32, bw: i32, bh: i32) -> (f32, f32) {
    let bw = bw.max(1) as f32;
    let bh = bh.max(1) as f32;
    (tx as f32 + bw * 0.5, ty as f32 + bh * 0.5)
}

/// Catalog size after 90° direction (2/6 swap axes). Diagonals keep catalog order for AABB.
fn oriented_size(w: f32, h: f32, direction: i32) -> (f32, f32) {
    let d = direction.rem_euclid(8);
    if d == 2 || d == 6 {
        (h, w)
    } else {
        (w, h)
    }
}

fn footprint_top_left(x: f32, y: f32, w: f32, h: f32, drag: (f32, f32)) -> (i32, i32) {
    (
        relative_to_tile(x - w * 0.5, drag.0),
        relative_to_tile(y - h * 0.5, drag.1),
    )
}

/// Tile centers for a footprint grid that fits inside the inclusive cell rect.
/// `(min_x, min_y)` is the top-left tile of the first building.
#[allow(dead_code)]
fn place_grid_centers(
    min_x: i32,
    min_y: i32,
    max_x: i32,
    max_y: i32,
    bw: i32,
    bh: i32,
) -> Vec<(f32, f32)> {
    let bw = bw.max(1);
    let bh = bh.max(1);
    let mut out = Vec::new();
    let mut y = min_y;
    while y + bh - 1 <= max_y {
        let mut x = min_x;
        while x + bw - 1 <= max_x {
            out.push(place_center_from_top_left(x, y, bw, bh));
            x += bw;
        }
        y += bh;
    }
    out
}

/// Anno-like quick-select tools (default construction tools + roads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadialAction {
    RoadDirt,
    RoadStone,
    Demolish,
    Copy,
    Move,
    ClearHand,
}

#[derive(Debug, Clone)]
struct RadialItem {
    action: RadialAction,
    label: &'static str,
    /// Game IconFilename under icon_content/…
    icon: &'static str,
}

#[derive(Debug, Clone)]
struct RadialMenu {
    origin: Pos2,
    hover: Option<usize>,
    /// Skip the RMB that opened the wheel so it doesn't instantly close.
    ignore_next_click: bool,
}

impl RadialMenu {
    const INNER_R: f32 = 36.0;
    const OUTER_R: f32 = 118.0;
    const ICON_RING: f32 = 78.0;
    const SLOT_R: f32 = 26.0;

    fn items() -> &'static [RadialItem] {
        &[
            RadialItem {
                action: RadialAction::RoadDirt,
                label: "Weg Erde",
                icon: "data/ui/fhd/base/icon_content/infrastructure/icon_3d_road_dirt.png",
            },
            RadialItem {
                action: RadialAction::RoadStone,
                label: "Weg Stein",
                icon: "data/ui/fhd/base/icon_content/infrastructure/icon_3d_road_stone.png",
            },
            RadialItem {
                action: RadialAction::Demolish,
                label: "Abriss",
                icon: "data/ui/fhd/base/icon_content/tools/icon_2d_tool_demolish.png",
            },
            RadialItem {
                action: RadialAction::Copy,
                label: "Kopieren",
                icon: "data/ui/fhd/base/icon_content/tools/icon_2d_tool_copy.png",
            },
            RadialItem {
                action: RadialAction::Move,
                label: "Verschieben",
                icon: "data/ui/fhd/base/icon_content/tools/icon_2d_tool_move_building.png",
            },
            RadialItem {
                action: RadialAction::ClearHand,
                label: "Hand frei",
                icon: "data/ui/fhd/base/icon_content/construction_tools/icon_2d_tools.png",
            },
        ]
    }

    fn sector_at(origin: Pos2, pointer: Pos2, n: usize) -> Option<usize> {
        if n == 0 {
            return None;
        }
        let d = pointer - origin;
        let dist = d.length();
        if dist < Self::INNER_R * 0.55 || dist > Self::OUTER_R + 12.0 {
            return None;
        }
        let mut ang = d.y.atan2(d.x);
        ang += std::f32::consts::FRAC_PI_2;
        if ang < 0.0 {
            ang += std::f32::consts::TAU;
        }
        let idx = ((ang / std::f32::consts::TAU) * n as f32).floor() as usize;
        Some(idx.min(n - 1))
    }
}

#[derive(Debug, Clone)]
enum LibraryClipboard {
    Pack(PathBuf),
    Stamp { pack: PathBuf, name: String },
}

/// Selectable / draggable library row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LibItem {
    Pack(PathBuf),
    Stamp { pack: PathBuf, name: String },
}

/// Payload for egui library drag-and-drop.
#[derive(Debug, Clone)]
struct LibraryDragPayload {
    items: Vec<LibItem>,
}

#[derive(Debug, Clone)]
enum LibraryDropTarget {
    Folder(PathBuf),
    Pack(PathBuf),
}

#[derive(Debug, Clone)]
enum LibraryAction {
    NewFolder { parent: PathBuf },
    NewStampPack { folder: PathBuf },
    NewStampInPack { pack: PathBuf },
    DeletePack { path: PathBuf },
    DeleteStamp { pack: PathBuf, name: String },
    DeleteFolder { path: PathBuf },
    DuplicateStamp { pack: PathBuf, name: String },
    CopyPack { path: PathBuf },
    CopyStamp { pack: PathBuf, name: String },
    Paste { target_folder: PathBuf },
    PasteIntoPack { pack: PathBuf },
    OpenStamp { pack: PathBuf, name: String },
    DropItems {
        items: Vec<LibItem>,
        target: LibraryDropTarget,
    },
}

impl StampEditorApp {
    pub fn new(catalog_path: PathBuf) -> Self {
        let catalog_load = Catalog::load(&catalog_path);
        let catalog_ok = catalog_load.is_ok();
        let catalog = catalog_load.unwrap_or_default();
        let cfg = EditorConfig::load();
        let library_root = cfg.library_root.clone();
        let game_install = cfg.game_install.clone();
        let library_packs = library::scan_library(&library_root);
        let library_fingerprint = library::library_fingerprint(&library_root);
        let game_note = match &game_install {
            Some(p) if EditorConfig::looks_like_anno_install(p) => {
                format!("Spiel verknüpft: {}", p.display())
            }
            Some(p) => format!("Spielordner ungültig (keine ui.rda): {}", p.display()),
            None => "Kein Spielordner — Datei → Anno-Spielordner verknüpfen…".into(),
        };
        let catalog_note = if catalog_ok {
            String::new()
        } else {
            format!(
                " · Katalog fehlt/kaputt: {} — Gebäudegrößen unzuverlässig!",
                catalog_path.display()
            )
        };
        let mut app = Self {
            catalog,
            catalog_path,
            open: None,
            stamp_list_filter: String::new(),
            selected_building: None,
            selected_buildings: HashSet::new(),
            palette_filter: String::new(),
            show_palette: false,
            show_analyse: false,
            // Concrete region so the construction bar (incl. Wege) is visible.
            region_filter: RegionFilter::Latium,
            palette_selected_guid: None,
            status: format!(
                "Bibliothek: {} ({} Packs). {game_note}{catalog_note}",
                library_root.display(),
                library_packs.len()
            ),
            cell_px: 18.0,
            pan: Vec2::ZERO,
            coverage: CoverageMap::default(),
            dragging_building: None,
            drag_offset: Vec2::ZERO,
            drag_group_origins: HashMap::new(),
            drag_group_roads: Vec::new(),
            drag_group_roads_live: Vec::new(),
            drag_anchor_start: (0.0, 0.0),
            placing_guid: None,
            place_variation: None,
            drag_stroke: None,
            suppress_stroke_click: false,
            road_brush_guid: None,
            road_erase_mode: false,
            radial: None,
            history: History::new(),
            history_drag_pushed: false,
            library_root,
            game_install,
            library_packs,
            library_clipboard: None,
            move_group: false,
            clipboard: Vec::new(),
            clipboard_roads: Vec::new(),
            selected_roads: HashSet::new(),
            hand_from_clipboard: false,
            last_pointer_world: None,
            module_parent_index: None,
            hover_building: None,
            request_delete: false,
            allow_45_deg: false,
            buildmenu: ConstructionMenu::load_default(),
            buildmenu_tab: Some(0),
            buildmenu_entry: None,
            buildmenu_infra_entry: None,
            ui_icons: UiIcons::new(),
            autosave_at: None,
            library_selection: HashSet::new(),
            library_marquee: None,
            library_fingerprint,
            library_watch_at: Instant::now() + Duration::from_secs(1),
        };
        if let Some(g) = app.game_install.clone() {
            app.ui_icons.attach_game_install(&g);
        }
        // Empty canvas is a blank editable stamp (not a dead "open file" screen).
        app.start_new_stamp();
        if !catalog_ok {
            app.status = format!(
                "Katalog fehlt/kaputt: {} — Footprints/Labels unzuverlässig. {game_note}",
                app.catalog_path.display()
            );
        }
        app
    }

    fn unique_new_pack_path_in(&self, dir: &Path) -> PathBuf {
        let _ = std::fs::create_dir_all(dir);
        library::unique_json_path(dir, "Neues Stamp")
    }

    fn schedule_autosave(&mut self) {
        self.autosave_at = Some(Instant::now() + Duration::from_millis(350));
    }

    fn refresh_library_silent(&mut self) {
        self.library_packs = library::scan_library(&self.library_root);
        self.library_fingerprint = library::library_fingerprint(&self.library_root);
        self.save_config();
    }

    /// Poll library folder for external add/remove/edit; rescan when fingerprint changes.
    fn tick_library_watch(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        // Keep waking egui even when idle so external folder changes are noticed.
        ctx.request_repaint_after(Duration::from_secs(1));
        if now < self.library_watch_at {
            return;
        }
        self.library_watch_at = now + Duration::from_secs(1);
        let fp = library::library_fingerprint(&self.library_root);
        if fp == self.library_fingerprint {
            return;
        }
        let before = self.library_packs.len();
        self.library_packs = library::scan_library(&self.library_root);
        self.library_fingerprint = fp;
        // Drop selection entries that no longer exist.
        self.library_selection.retain(|item| match item {
            LibItem::Pack(path) => self.library_packs.iter().any(|p| p.path == *path),
            LibItem::Stamp { pack, name } => self
                .library_packs
                .iter()
                .any(|p| p.path == *pack && p.stamps.iter().any(|s| s.name == *name)),
        });
        let after = self.library_packs.len();
        if before != after {
            self.status = format!(
                "Bibliothek aktualisiert (extern): {after} Packs"
            );
        } else {
            self.status = format!(
                "Bibliothek aktualisiert (extern): {} — {after} Packs",
                self.library_root.display()
            );
        }
    }

    /// Write current stamp to disk (creates pack file if still `is_new`).
    fn autosave_now(&mut self, refresh_lib: bool) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if !open.dirty && !open.is_new {
            return;
        }
        match save_open_stamp(open) {
            Ok(()) => {
                open.dirty = false;
                open.is_new = false;
                let path = open.pack_path.clone();
                let name = open.entry_name.clone();
                if refresh_lib {
                    self.refresh_library_silent();
                }
                self.status = format!("Gespeichert: {name} ({})", path.display());
            }
            Err(err) => self.status = format!("Speichern fehlgeschlagen: {err}"),
        }
    }

    fn tick_autosave(&mut self, ctx: &egui::Context) {
        let Some(due) = self.autosave_at else {
            return;
        };
        let now = Instant::now();
        if now < due {
            ctx.request_repaint_after(due.saturating_duration_since(now));
            return;
        }
        if self.dragging_building.is_some() || self.drag_stroke.is_some() {
            self.autosave_at = Some(now + Duration::from_millis(350));
            ctx.request_repaint_after(Duration::from_millis(350));
            return;
        }
        self.autosave_at = None;
        let refresh = self
            .open
            .as_ref()
            .is_some_and(|o| o.is_new || !o.pack_path.exists());
        self.autosave_now(refresh);
    }

    fn flush_autosave_blocking(&mut self) {
        self.autosave_at = None;
        if self.open.as_ref().is_some_and(|o| o.dirty || o.is_new) {
            let should = self.open.as_ref().is_some_and(|o| {
                o.dirty || (!o.is_new && o.pack_path.exists())
            });
            if should {
                let refresh = self.open.as_ref().is_some_and(|o| o.is_new);
                self.autosave_now(refresh);
            }
        }
    }

    /// Replace the canvas with a new empty stamp.
    fn start_new_stamp(&mut self) {
        let dir = self.library_root.join("unsorted");
        self.start_new_stamp_in(&dir, false);
    }

    fn start_new_stamp_in(&mut self, dir: &Path, persist_now: bool) {
        self.flush_autosave_blocking();
        let path = self.unique_new_pack_path_in(dir);
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Neues Stamp".into());
        let region = self.region_filter.stamp_region_guid();
        match new_blank_stamp(path, name, region) {
            Ok(open) => {
                self.history.clear();
                self.history_drag_pushed = false;
                self.module_parent_index = None;
                self.clear_canvas_selection();
                self.placing_guid = None;
                self.place_variation = None;
                self.autosave_at = None;
                self.open = Some(open);
                self.center_view();
                self.refresh_coverage();
                if persist_now {
                    if let Some(o) = self.open.as_mut() {
                        o.dirty = true;
                    }
                    self.autosave_now(true);
                    self.status = format!(
                        "Neues Stamp angelegt · Region {} · speichert automatisch",
                        self.region_filter.label()
                    );
                } else {
                    self.status = format!(
                        "Neues Stamp — leer · Region {} · speichert bei erster Änderung",
                        self.region_filter.label()
                    );
                }
            }
            Err(err) => self.status = format!("Neues Stamp fehlgeschlagen: {err}"),
        }
    }

    fn confirm_delete(label: &str) -> bool {
        matches!(
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Löschen?")
                .set_description(label)
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show(),
            rfd::MessageDialogResult::Ok
        )
    }

    fn apply_library_action(&mut self, action: LibraryAction) {
        match action {
            LibraryAction::NewFolder { parent } => {
                match library::create_library_folder(&parent, "Neuer Ordner") {
                    Ok(dir) => {
                        self.refresh_library();
                        self.status = format!("Ordner angelegt: {}", dir.display());
                    }
                    Err(e) => self.status = format!("Ordner fehlgeschlagen: {e}"),
                }
            }
            LibraryAction::NewStampPack { folder } => {
                self.start_new_stamp_in(&folder, true);
            }
            LibraryAction::NewStampInPack { pack } => {
                let region = self.region_filter.stamp_region_guid();
                match append_blank_stamp(&pack, "Neues Stamp", region) {
                    Ok(open) => {
                        self.refresh_library();
                        self.set_open(open, &pack);
                        self.status = "Stamp im Pack angelegt.".into();
                    }
                    Err(e) => self.status = format!("Stamp anlegen fehlgeschlagen: {e}"),
                }
            }
            LibraryAction::DeletePack { path } => {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                if !Self::confirm_delete(&format!("Pack „{name}“ wirklich löschen?")) {
                    return;
                }
                let was_open = self.open.as_ref().is_some_and(|o| o.pack_path == path);
                match library::delete_pack_file(&path) {
                    Ok(()) => {
                        if was_open {
                            self.start_new_stamp();
                        }
                        self.refresh_library();
                        self.status = format!("Pack gelöscht: {name}");
                    }
                    Err(e) => self.status = format!("Löschen fehlgeschlagen: {e}"),
                }
            }
            LibraryAction::DeleteStamp { pack, name } => {
                if !Self::confirm_delete(&format!("Stamp „{name}“ wirklich löschen?")) {
                    return;
                }
                let was_open = self
                    .open
                    .as_ref()
                    .is_some_and(|o| o.pack_path == pack && o.entry_name == name);
                match delete_stamp_entry(&pack, &name) {
                    Ok(()) => {
                        if was_open {
                            self.start_new_stamp();
                        }
                        self.refresh_library();
                        self.status = format!("Stamp gelöscht: {name}");
                    }
                    Err(e) => self.status = format!("Löschen fehlgeschlagen: {e}"),
                }
            }
            LibraryAction::DuplicateStamp { pack, name } => {
                match duplicate_stamp(&pack, &name) {
                    Ok(open) => {
                        self.refresh_library();
                        let path = open.pack_path.clone();
                        self.set_open(open, &path);
                        self.status = "Stamp dupliziert.".into();
                    }
                    Err(e) => self.status = format!("Duplizieren fehlgeschlagen: {e}"),
                }
            }
            LibraryAction::CopyPack { path } => {
                self.library_clipboard = Some(LibraryClipboard::Pack(path.clone()));
                self.status = format!(
                    "Pack kopiert: {}",
                    path.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                );
            }
            LibraryAction::CopyStamp { pack, name } => {
                self.library_clipboard = Some(LibraryClipboard::Stamp { pack, name: name.clone() });
                self.status = format!("Stamp kopiert: {name}");
            }
            LibraryAction::Paste { target_folder } => {
                match self.library_clipboard.clone() {
                    Some(LibraryClipboard::Pack(src)) => {
                        match library::copy_pack_file(&src, &target_folder) {
                            Ok(dest) => {
                                self.refresh_library();
                                self.status = format!("Pack eingefügt: {}", dest.display());
                            }
                            Err(e) => self.status = format!("Einfügen fehlgeschlagen: {e}"),
                        }
                    }
                    Some(LibraryClipboard::Stamp { pack, name }) => {
                        let dest = library::unique_json_path(&target_folder, &name);
                        match copy_stamp_to_pack(&pack, &name, &dest) {
                            Ok(new_name) => {
                                self.refresh_library();
                                self.status = format!(
                                    "Stamp „{new_name}“ als Pack eingefügt: {}",
                                    dest.display()
                                );
                            }
                            Err(e) => self.status = format!("Einfügen fehlgeschlagen: {e}"),
                        }
                    }
                    None => self.status = "Zwischenablage leer.".into(),
                }
            }
            LibraryAction::PasteIntoPack { pack: dest } => {
                match self.library_clipboard.clone() {
                    Some(LibraryClipboard::Stamp { pack, name }) => {
                        match copy_stamp_to_pack(&pack, &name, &dest) {
                            Ok(new_name) => {
                                self.refresh_library();
                                self.status = format!("Stamp „{new_name}“ eingefügt.");
                            }
                            Err(e) => self.status = format!("Einfügen fehlgeschlagen: {e}"),
                        }
                    }
                    Some(LibraryClipboard::Pack(_)) => {
                        self.status = "Pack-Datei bitte in einen Ordner einfügen.".into();
                    }
                    None => self.status = "Zwischenablage leer.".into(),
                }
            }
            LibraryAction::OpenStamp { pack, name } => {
                self.open_named(pack, &name);
            }
            LibraryAction::DeleteFolder { path } => {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                if !Self::confirm_delete(&format!("Leeren Ordner „{name}“ löschen?")) {
                    return;
                }
                match library::delete_library_folder(&path) {
                    Ok(()) => {
                        self.refresh_library();
                        self.status = format!("Ordner gelöscht: {name}");
                    }
                    Err(e) => self.status = format!("Ordner löschen: {e}"),
                }
            }
            LibraryAction::DropItems { items, target } => {
                self.apply_library_drop(items, target);
            }
        }
    }

    fn apply_library_drop(&mut self, items: Vec<LibItem>, target: LibraryDropTarget) {
        self.flush_autosave_blocking();
        let mut moved = 0usize;
        let mut err: Option<String> = None;
        let open_pack = self.open.as_ref().map(|o| o.pack_path.clone());
        let open_stamp = self.open.as_ref().map(|o| o.entry_name.clone());
        let mut remapped_open: Option<(PathBuf, String)> = None;

        match &target {
            LibraryDropTarget::Folder(dest_dir) => {
                for item in &items {
                    match item {
                        LibItem::Pack(src) => {
                            if src.parent() == Some(dest_dir.as_path()) {
                                continue;
                            }
                            match library::move_pack_to_folder(src, dest_dir) {
                                Ok(dest) => {
                                    moved += 1;
                                    if open_pack.as_ref() == Some(src) {
                                        let name = open_stamp.clone().unwrap_or_default();
                                        remapped_open = Some((dest, name));
                                    }
                                }
                                Err(e) => {
                                    err = Some(e);
                                    break;
                                }
                            }
                        }
                        LibItem::Stamp { pack, name } => {
                            let dest = library::unique_json_path(dest_dir, name);
                            match move_stamp_to_pack(pack, name, &dest) {
                                Ok(new_name) => {
                                    moved += 1;
                                    if open_pack.as_ref() == Some(pack)
                                        && open_stamp.as_deref() == Some(name.as_str())
                                    {
                                        remapped_open = Some((dest, new_name));
                                    }
                                }
                                Err(e) => {
                                    err = Some(e.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            LibraryDropTarget::Pack(dest_pack) => {
                for item in &items {
                    match item {
                        LibItem::Stamp { pack, name } => {
                            if pack == dest_pack {
                                continue;
                            }
                            match move_stamp_to_pack(pack, name, dest_pack) {
                                Ok(new_name) => {
                                    moved += 1;
                                    if open_pack.as_ref() == Some(pack)
                                        && open_stamp.as_deref() == Some(name.as_str())
                                    {
                                        remapped_open = Some((dest_pack.clone(), new_name));
                                    }
                                }
                                Err(e) => {
                                    err = Some(e.to_string());
                                    break;
                                }
                            }
                        }
                        LibItem::Pack(src) => {
                            if src == dest_pack {
                                continue;
                            }
                            let list = stamp_editor::pack::list_stamps(src)
                                .unwrap_or_default()
                                .into_iter()
                                .map(|e| e.name)
                                .collect::<Vec<_>>();
                            for name in list {
                                match move_stamp_to_pack(src, &name, dest_pack) {
                                    Ok(_) => moved += 1,
                                    Err(e) => {
                                        err = Some(e.to_string());
                                        break;
                                    }
                                }
                            }
                            if err.is_some() {
                                break;
                            }
                            if open_pack.as_ref() == Some(src) {
                                remapped_open = Some((
                                    dest_pack.clone(),
                                    open_stamp.clone().unwrap_or_default(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        self.library_selection.clear();
        self.refresh_library();
        if let Some((path, name)) = remapped_open {
            if !name.is_empty() {
                self.open_named(path, &name);
            } else {
                self.open_path(path);
            }
        }
        self.status = if let Some(e) = err {
            format!("Verschieben teilweise ({moved}): {e}")
        } else {
            format!("{moved} Element(e) verschoben")
        };
    }

    fn library_drag_armed(ui: &egui::Ui) -> bool {
        ui.input(|i| {
            i.pointer
                .press_origin()
                .zip(i.pointer.interact_pos())
                .is_some_and(|(a, b)| a.distance(b) >= 12.0)
        })
    }

    fn save_config(&self) {
        let _ = EditorConfig {
            library_root: self.library_root.clone(),
            game_install: self.game_install.clone(),
        }
        .save();
    }

    fn link_game_install(&mut self, path: PathBuf) {
        if EditorConfig::looks_like_anno_install(&path) {
            self.game_install = Some(path.clone());
            self.save_config();
            self.ui_icons.attach_game_install(&path);
            self.status = format!(
                "Spiel verknüpft: {} — Icons werden aus ui.rda geladen/gecacht…",
                path.display()
            );
        } else {
            self.status = format!(
                "Kein gültiger Anno-117-Ordner (erwartet maindata\\ui.rda): {}",
                path.display()
            );
        }
    }

    fn pick_place_guid(&mut self, guid: u32, label: &str) {
        self.palette_selected_guid = Some(guid);
        self.placing_guid = Some(guid);
        self.module_parent_index = None;
        self.road_brush_guid = None;
        self.hand_from_clipboard = false;
        // Default: land footprint. Marsh (5×5 warehouse) via place_variation = 1 (Taste V).
        self.place_variation = None;
        self.refresh_place_status(label);
    }

    fn refresh_place_status(&mut self, label: &str) {
        let Some(guid) = self.placing_guid else {
            return;
        };
        let variation = self.place_variation;
        let size_hint = self
            .catalog
            .building(guid)
            .map(|d| {
                let s = d.size_for_variation(variation);
                let mut t = format!("{}×{}", s[0] as i32, s[1] as i32);
                if d.size_marsh.is_some() {
                    let kind = if matches!(variation, Some(v) if v >= 1) {
                        "Marsh"
                    } else {
                        "Land"
                    };
                    t.push_str(&format!(" · {kind} · V = Land/Marsh"));
                }
                t
            })
            .unwrap_or_default();
        self.status = if size_hint.is_empty() {
            format!("Hand: {label} — ziehen = Rechteck · Klick = 1 · RMB = fallen lassen")
        } else {
            format!(
                "Hand: {label} ({size_hint}) — ziehen = Rechteck · Klick = 1 · RMB = fallen lassen"
            )
        };
    }

    /// Toggle Land ↔ Marsh footprint while placing (buildings with `size_marsh`).
    fn toggle_place_variation(&mut self) {
        let Some(guid) = self.placing_guid else {
            return;
        };
        let Some(def) = self.catalog.building(guid) else {
            return;
        };
        if def.size_marsh.is_none() {
            self.status = format!(
                "{} hat keine Marsh-Variante",
                self.catalog.label(guid)
            );
            return;
        }
        self.place_variation = match self.place_variation {
            Some(v) if v >= 1 => None,
            _ => Some(1),
        };
        let label = self.catalog.label(guid);
        self.refresh_place_status(&label);
    }

    /// Toggle Land ↔ Marsh on an already placed building (writes pipette Variation).
    fn toggle_selected_variation(&mut self, idx: usize) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let Some(b) = open.stamp.buildings.get(idx) else {
            return;
        };
        let guid = b.guid;
        let cur = b.variation;
        let Some(def) = self.catalog.building(guid) else {
            return;
        };
        if def.size_marsh.is_none() {
            self.status = format!(
                "{} hat keine Marsh-Variante",
                self.catalog.label(guid)
            );
            return;
        }
        let next = match cur {
            Some(v) if v >= 1 => None,
            _ => Some(1),
        };
        self.push_history();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match open.stamp.set_building_variation(idx, next) {
            Ok(()) => {
                open.dirty = true;
                self.schedule_autosave();
                let kind = if matches!(next, Some(v) if v >= 1) {
                    "Marsh"
                } else {
                    "Land"
                };
                let (w, h) = self.building_size(guid, next);
                self.status = format!(
                    "{} → {kind} {}×{}",
                    self.catalog.label(guid),
                    w as i32,
                    h as i32
                );
            }
            Err(e) => self.status = format!("Variation fehlgeschlagen: {e}"),
        }
    }

    fn activate_menu_building(&mut self, guid: u32, label: &str) {
        let g = guid as i32;
        if is_road_brush_guid(g) {
            self.road_brush_guid = Some(g);
            self.road_erase_mode = false;
            self.placing_guid = None;
            self.place_variation = None;
            self.drag_stroke = None;
            self.status = format!("Weg-Pinsel: {label} — ziehen = Linie/Knick · Start auf Weg = radieren");
            return;
        }
        self.road_erase_mode = false;
        self.pick_place_guid(guid, label);
    }

    fn activate_road_eraser(&mut self) {
        self.road_erase_mode = true;
        self.road_brush_guid = None;
        self.placing_guid = None;
        self.place_variation = None;
        self.module_parent_index = None;
        self.drag_stroke = None;
        self.buildmenu_infra_entry = None;
        self.status =
            "Abriss — Rechteck ziehen (Wege + Gebäude) · RMB = fallen lassen".into();
    }

    fn road_tool_active(&self) -> bool {
        self.road_brush_guid.is_some() || self.road_erase_mode
    }

    fn hand_busy(&self) -> bool {
        self.placing_guid.is_some() || self.road_tool_active()
    }

    fn open_radial(&mut self, origin: Pos2) {
        self.radial = Some(RadialMenu {
            origin,
            hover: None,
            ignore_next_click: true,
        });
        self.status =
            "Schnellwahl-Rad — Tool wählen · Mitte/außen/Esc = Hand frei".into();
    }

    /// Close the wheel without picking a slot → drop whatever is in hand.
    fn dismiss_radial(&mut self) {
        self.radial = None;
        if self.hand_busy() {
            self.clear_hand();
            self.status = "Hand frei".into();
        } else {
            self.status = "Schnellwahl geschlossen".into();
        }
    }

    fn copy_selection_to_clipboard(&mut self) -> bool {
        let Some(open) = self.open.as_ref() else {
            self.status = "Nichts zu kopieren — kein Stamp geladen".into();
            return false;
        };
        let mut indices = self.selection_indices();
        if indices.is_empty() {
            if let Some(h) = self.hover_building {
                indices.push(h);
            }
        }
                // No canvas building: copy whatever is currently in the placement hand / palette.
        if indices.is_empty() {
            if let Some(guid) = self.placing_guid.or(self.palette_selected_guid) {
                let label = self.catalog.label(guid);
                self.clipboard = vec![ClipboardBuilding {
                    guid,
                    direction: 0,
                    variation: self.place_variation,
                    dtx: 0,
                    dty: 0,
                    is_primary: true,
                }];
                self.clipboard_roads.clear();
                self.status = format!("Kopiert: {label} — Ctrl+V → in die Hand");
                return true;
            }
            let n = open.stamp.buildings.len();
            self.status = if n == 0 {
                "Leeres Stamp — Gebäude auf der Karte anklicken, oder Stamp „Woodcutter“ in der Bibliothek öffnen".into()
            } else {
                "Nichts markiert — Gebäude anklicken oder mit leerer Hand ein Rechteck aufziehen".into()
            };
            return false;
        }
        let primary_idx = self
            .selected_building
            .filter(|i| indices.contains(i))
            .unwrap_or(indices[0]);
        let Some(primary) = open.stamp.buildings.get(primary_idx).cloned() else {
            return false;
        };
        let drag = self.stamp_drag();
        let (ppw, pph) = {
            let (w0, h0) = self.building_size(primary.guid, primary.variation);
            oriented_size(w0, h0, primary.direction)
        };
        let (ptx, pty) = footprint_top_left(primary.x, primary.y, ppw, pph, drag);

        let mut clip = vec![ClipboardBuilding {
            guid: primary.guid,
            direction: primary.direction,
            variation: primary.variation,
            dtx: 0,
            dty: 0,
            is_primary: true,
        }];
        let mut seen = HashSet::from([primary_idx]);
        let push_rel = |clip: &mut Vec<ClipboardBuilding>,
                        m: &Building,
                        catalog: &Catalog,
                        drag: (f32, f32)| {
            let (w0, h0) = catalog
                .building(m.guid)
                .map(|d| {
                    let s = d.size_for_variation(m.variation);
                    (s[0].max(1.0), s[1].max(1.0))
                })
                .unwrap_or((2.0, 2.0));
            let (bw, bh) = oriented_size(w0, h0, m.direction);
            let (tx, ty) = footprint_top_left(m.x, m.y, bw, bh, drag);
            clip.push(ClipboardBuilding {
                guid: m.guid,
                direction: m.direction,
                variation: m.variation,
                dtx: tx - ptx,
                dty: ty - pty,
                is_primary: false,
            });
        };
        for &i in &indices {
            if !seen.insert(i) {
                continue;
            }
            let Some(m) = open.stamp.buildings.get(i) else {
                continue;
            };
            push_rel(&mut clip, m, &self.catalog, drag);
        }
        // Single selection: also attach field/pasture modules of that farm.
        if indices.len() == 1 {
            if let Some(owner) = primary.complex_owner_id.filter(|o| *o != 0) {
                for (i, m) in open.stamp.buildings.iter().enumerate() {
                    if !seen.insert(i) || m.complex_owner_id != Some(owner) {
                        continue;
                    }
                    let is_mod = self
                        .catalog
                        .building(m.guid)
                        .is_some_and(|d| is_module_building(d) || d.category == "module");
                    if !is_mod {
                        continue;
                    }
                    push_rel(&mut clip, m, &self.catalog, drag);
                }
            }
        }
        // Roads from marquee (absolute → relative to primary footprint top-left).
        let road_map = build_road_map_region(
            &open.stamp.buildings,
            &self.catalog,
            Some(&open.stamp.document),
            open.region_guid,
        );
        let mut roads = Vec::new();
        for &(rx, ry) in &self.selected_roads {
            let guid = road_map
                .get(&stamp_editor::coverage::Cell { x: rx, y: ry })
                .map(|k| match k {
                    RoadKind::Stone => ROAD_GUID_STONE,
                    RoadKind::Dirt => ROAD_GUID_DIRT,
                })
                .unwrap_or(ROAD_GUID_DIRT);
            roads.push(ClipboardRoad {
                dx: rx - ptx,
                dy: ry - pty,
                guid,
            });
        }

        self.select_only(primary_idx);
        for &i in &indices {
            self.selected_buildings.insert(i);
        }
        self.selected_building = Some(primary_idx);
        // Keep road selection visible until next marquee/clear.
        let n = clip.len();
        let n_roads = roads.len();
        let label = self.catalog.label(clip[0].guid);
        self.clipboard = clip;
        self.clipboard_roads = roads;
        // Copy only fills the clipboard — Ctrl+V puts it into the hand.
        self.status = match (n > 1, n_roads > 0) {
            (true, true) => format!(
                "Kopiert: {label} (+{}) + {n_roads} Wege — Ctrl+V → Hand",
                n - 1
            ),
            (true, false) => format!("Kopiert: {label} (+{}) — Ctrl+V → Hand", n - 1),
            (false, true) => format!("Kopiert: {label} + {n_roads} Wege — Ctrl+V → Hand"),
            (false, false) => format!("Kopiert: {label} — Ctrl+V → Hand"),
        };
        true
    }

    /// Put clipboard primary into the placement hand (ghost follows cursor; click to set).
    fn arm_clipboard_in_hand(&mut self) {
        let Some(p) = self.clipboard.iter().find(|c| c.is_primary).cloned() else {
            self.status = "Zwischenablage leer — zuerst Ctrl+C".into();
            return;
        };
        self.road_erase_mode = false;
        self.road_brush_guid = None;
        self.drag_stroke = None;
        self.module_parent_index = None;
        self.placing_guid = Some(p.guid);
        self.place_variation = p.variation;
        self.palette_selected_guid = Some(p.guid);
        self.hand_from_clipboard = true;
        let label = self.catalog.label(p.guid);
        let extra = self.clipboard.len().saturating_sub(1);
        let roads = self.clipboard_roads.len();
        self.status = match (extra > 0, roads > 0) {
            (true, true) => {
                format!("Hand: {label} (+{extra}, {roads} Wege) — Klick setzt alles")
            }
            (true, false) => format!("Hand: {label} (+{extra}) — Klick setzt die ganze Gruppe"),
            (false, true) => format!("Hand: {label} (+{roads} Wege) — Klick setzt"),
            (false, false) => {
                format!("Hand: {label} — Klick zum Platzieren · Esc = ablegen")
            }
        };
    }

    /// Cut = copy then delete the current selection (buildings + roads).
    fn cut_selection(&mut self) {
        if self.selection_indices().is_empty()
            && self.selected_roads.is_empty()
            && self.hover_building.is_none()
        {
            self.status = "Nichts ausgewählt zum Ausschneiden".into();
            return;
        }
        if !self.copy_selection_to_clipboard() && self.clipboard_roads.is_empty() {
            return;
        }
        let n = self.clipboard.len();
        let n_roads = self.clipboard_roads.len();
        // Multi-select already lists everything; don't expand to farm groups again.
        // delete_selection also clears selected roads.
        self.delete_selection(self.selected_buildings.len() <= 1);
        if !self.clipboard.is_empty() || n_roads > 0 {
            self.status = format!("Ausgeschnitten ({n} Gebäude, {n_roads} Wege) — Ctrl+V");
        }
    }

    /// Oriented catalog size for a clipboard entry.
    fn clipboard_oriented_size(&self, c: &ClipboardBuilding) -> (f32, f32) {
        let (w0, h0) = self.building_size(c.guid, c.variation);
        oriented_size(w0, h0, c.direction)
    }

    /// Primary footprint top-left implied by a snapped primary center.
    fn clipboard_primary_top_left(&self, primary_cx: f32, primary_cy: f32) -> (i32, i32) {
        let Some(p) = self.clipboard.iter().find(|c| c.is_primary) else {
            return (primary_cx.floor() as i32, primary_cy.floor() as i32);
        };
        let (pw, ph) = self.clipboard_oriented_size(p);
        footprint_top_left(primary_cx, primary_cy, pw, ph, self.stamp_drag())
    }

    /// Resolve clipboard buildings to world centers from a snapped primary center.
    fn clipboard_building_centers(&self, primary_cx: f32, primary_cy: f32) -> Vec<(ClipboardBuilding, f32, f32)> {
        let (ptx, pty) = self.clipboard_primary_top_left(primary_cx, primary_cy);
        self.clipboard
            .iter()
            .cloned()
            .map(|c| {
                let (bw, bh) = self.clipboard_oriented_size(&c);
                let bw_i = bw.round().max(1.0) as i32;
                let bh_i = bh.round().max(1.0) as i32;
                let (cx, cy) =
                    place_center_from_top_left(ptx + c.dtx, pty + c.dty, bw_i, bh_i);
                (c, cx, cy)
            })
            .collect()
    }

    /// Resolve clipboard roads to absolute tiles from a snapped primary center.
    fn clipboard_road_cells(&self, primary_cx: f32, primary_cy: f32) -> Vec<(i32, i32, i32)> {
        let (ptx, pty) = self.clipboard_primary_top_left(primary_cx, primary_cy);
        self.clipboard_roads
            .iter()
            .map(|r| (ptx + r.dx, pty + r.dy, r.guid))
            .collect()
    }

    /// Inclusive tile span of the clipboard group (for PlaceRect grid step).
    fn clipboard_group_span(&self) -> (i32, i32) {
        let Some(p) = self.clipboard.iter().find(|c| c.is_primary) else {
            return (1, 1);
        };
        let (pw, ph) = self.clipboard_oriented_size(p);
        let mut min_x = 0i32;
        let mut min_y = 0i32;
        let mut max_x = pw.round().max(1.0) as i32 - 1;
        let mut max_y = ph.round().max(1.0) as i32 - 1;
        for c in &self.clipboard {
            let (bw, bh) = self.clipboard_oriented_size(c);
            let bw_i = bw.round().max(1.0) as i32;
            let bh_i = bh.round().max(1.0) as i32;
            min_x = min_x.min(c.dtx);
            min_y = min_y.min(c.dty);
            max_x = max_x.max(c.dtx + bw_i - 1);
            max_y = max_y.max(c.dty + bh_i - 1);
        }
        for r in &self.clipboard_roads {
            min_x = min_x.min(r.dx);
            min_y = min_y.min(r.dy);
            max_x = max_x.max(r.dx);
            max_y = max_y.max(r.dy);
        }
        ((max_x - min_x + 1).max(1), (max_y - min_y + 1).max(1))
    }

    /// Place full clipboard group (buildings + roads) at primary center. Returns primary index.
    fn place_clipboard_group_at(&mut self, base_x: f32, base_y: f32) -> Option<usize> {
        let placed = self.clipboard_building_centers(base_x, base_y);
        let roads = self.clipboard_road_cells(base_x, base_y);
        let Some(open) = self.open.as_mut() else {
            return None;
        };
        let mut main_idx = None;
        for (c, cx, cy) in &placed {
            if open
                .stamp
                .add_building_from_template(c.guid, *cx, *cy, c.direction, c.variation)
                .is_err()
            {
                if c.is_primary {
                    return None;
                }
                continue;
            }
            let idx = open.stamp.buildings.len() - 1;
            if c.is_primary {
                main_idx = Some(idx);
            } else if let Some(mi) = main_idx {
                let is_mod = self
                    .catalog
                    .building(c.guid)
                    .is_some_and(|d| is_module_building(d) || d.category == "module");
                if is_mod {
                    let oid = open.stamp.buildings[mi]
                        .complex_owner_id
                        .unwrap_or_else(|| (mi as u64).wrapping_mul(1009).wrapping_add(7));
                    let _ = open.stamp.set_building_owner(mi, oid);
                    let _ = open.stamp.set_building_owner(idx, oid);
                }
            }
        }
        if !roads.is_empty() {
            let mut by_guid: std::collections::HashMap<i32, Vec<(i32, i32)>> =
                std::collections::HashMap::new();
            for (x, y, guid) in roads {
                by_guid.entry(guid).or_default().push((x, y));
            }
            for (guid, cells) in by_guid {
                let _ = apply_road_path(
                    &mut open.stamp.document,
                    &open.stamp.buildings,
                    &cells,
                    true,
                    guid,
                );
            }
        }
        open.dirty = true;
        main_idx
    }

    fn paste_anchor(&self) -> (f32, f32) {
        let (wx, wy) = self.last_pointer_world.unwrap_or_else(|| {
            (
                -self.pan.x / self.cell_px,
                -self.pan.y / self.cell_px,
            )
        });
        if let Some(p) = self.clipboard.iter().find(|c| c.is_primary) {
            let (bw, bh) = self.clipboard_oriented_size(p);
            return self.snap_place_center(wx, wy, bw, bh);
        }
        (wx.round(), wy.round())
    }

    /// Instant paste at cursor (Ctrl+Shift+V). Normal Ctrl+V only arms the hand.
    fn paste_clipboard_at_cursor(&mut self) {
        if self.clipboard.is_empty() {
            self.status = "Zwischenablage leer — zuerst Gebäude anklicken, dann Ctrl+C".into();
            return;
        }
        let (base_x, base_y) = self.paste_anchor();
        self.push_history();
        match self.place_clipboard_group_at(base_x, base_y) {
            Some(main_idx) => {
                self.schedule_autosave();
                self.select_only(main_idx);
                // Keep the same group in hand for another click-place.
                self.arm_clipboard_in_hand();
                let extra = self.clipboard.len().saturating_sub(1);
                let roads = self.clipboard_roads.len();
                self.status = format!(
                    "Eingefügt bei ({base_x:.0}, {base_y:.0}) — +{extra} Gebäude, {roads} Wege · noch in der Hand"
                );
                self.refresh_coverage();
            }
            None => self.status = "Einfügen fehlgeschlagen".into(),
        }
    }

    /// Push a marker into the OS clipboard so Ctrl+V produces `Event::Paste` on Windows.
    fn sync_os_clipboard_marker(&self, ctx: &egui::Context) {
        if !self.clipboard.is_empty() || !self.clipboard_roads.is_empty() {
            ctx.copy_text(OS_CLIPBOARD_MARKER.to_string());
        }
    }

    fn duplicate_selection(&mut self) {
        let Some(idx) = self.selected_building else {
            if self.clipboard.is_empty() {
                self.status = "Nichts zum Duplizieren".into();
                return;
            }
            // Nudge paste anchor +2 tiles from last pointer / view center.
            let anchor_was = self.last_pointer_world;
            let (ax, ay) = self.paste_anchor();
            self.last_pointer_world = Some((ax + 2.0, ay));
            self.paste_clipboard_at_cursor();
            self.last_pointer_world = anchor_was;
            if self.status.starts_with("Eingefügt") {
                self.status = "Dupliziert — noch in der Hand".into();
            }
            return;
        };
        if !self.copy_selection_to_clipboard() {
            return;
        }
        // Place next to the original (not under the cursor).
        let (ox, oy) = self
            .open
            .as_ref()
            .and_then(|o| o.stamp.buildings.get(idx))
            .map(|b| (b.x + 2.0, b.y))
            .unwrap_or_else(|| self.paste_anchor());
        let anchor_was = self.last_pointer_world;
        self.last_pointer_world = Some((ox, oy));
        self.paste_clipboard_at_cursor();
        self.last_pointer_world = anchor_was;
        if self.status.starts_with("Eingefügt") {
            self.status = "Dupliziert — noch in der Hand".into();
        }
    }

    fn apply_radial_action(&mut self, action: RadialAction) {
        self.radial = None;
        match action {
            RadialAction::RoadDirt => {
                self.activate_menu_building(ROAD_GUID_DIRT as u32, "Weg Erde");
            }
            RadialAction::RoadStone => {
                self.activate_menu_building(ROAD_GUID_STONE as u32, "Weg Stein");
            }
            RadialAction::Demolish => {
                self.activate_road_eraser();
            }
            RadialAction::Copy => {
                self.clear_hand();
                self.copy_selection_to_clipboard();
            }
            RadialAction::Move => {
                self.clear_hand();
                self.status = "Verschieben — Gebäude mit LMB ziehen · RMB = Rad öffnen".into();
            }
            RadialAction::ClearHand => {
                self.clear_hand();
                self.status = "Hand frei".into();
            }
        }
    }

    fn clear_hand(&mut self) {
        self.placing_guid = None;
        self.place_variation = None;
        self.module_parent_index = None;
        self.road_brush_guid = None;
        self.road_erase_mode = false;
        self.hand_from_clipboard = false;
        self.drag_stroke = None;
        // Drop infra tile highlight with the tool (was stuck on after RMB).
        self.buildmenu_infra_entry = None;
        self.buildmenu_entry = None;
        self.status = "Hand geleert".into();
    }

    fn clear_canvas_selection(&mut self) {
        self.selected_building = None;
        self.selected_buildings.clear();
        self.selected_roads.clear();
    }

    fn select_only(&mut self, idx: usize) {
        self.selected_buildings.clear();
        self.selected_buildings.insert(idx);
        self.selected_building = Some(idx);
        self.selected_roads.clear();
    }

    fn set_canvas_selection(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.selected_buildings.clear();
        self.selected_buildings.extend(indices);
        self.selected_building = self.selected_buildings.iter().copied().min();
    }

    fn set_canvas_selection_with_roads(
        &mut self,
        indices: impl IntoIterator<Item = usize>,
        roads: impl IntoIterator<Item = (i32, i32)>,
    ) {
        self.set_canvas_selection(indices);
        self.selected_roads.clear();
        self.selected_roads.extend(roads);
    }

    fn toggle_canvas_selection(&mut self, idx: usize) {
        if self.selected_buildings.contains(&idx) {
            self.selected_buildings.remove(&idx);
            if self.selected_building == Some(idx) {
                self.selected_building = self.selected_buildings.iter().copied().min();
            }
        } else {
            self.selected_buildings.insert(idx);
            self.selected_building = Some(idx);
        }
    }

    fn selection_indices(&self) -> Vec<usize> {
        if !self.selected_buildings.is_empty() {
            let mut v: Vec<_> = self.selected_buildings.iter().copied().collect();
            v.sort_unstable();
            v
        } else if let Some(i) = self.selected_building {
            vec![i]
        } else {
            Vec::new()
        }
    }

    fn begin_drag_stroke(&mut self, tx: i32, ty: i32) {
        let kind = if self.road_erase_mode {
            StrokeKind::DemolishRect
        } else if self.placing_guid.is_some() {
            StrokeKind::PlaceRect
        } else if self.road_brush_guid.is_some() {
            let already = self
                .coverage
                .roads
                .contains_key(&stamp_editor::coverage::Cell { x: tx, y: ty });
            StrokeKind::Road { paint_on: !already }
        } else if !self.hand_busy() {
            StrokeKind::SelectRect
        } else {
            return;
        };
        if !matches!(kind, StrokeKind::SelectRect) {
            self.push_history();
        }
        self.drag_stroke = Some(DragStroke {
            start: (tx, ty),
            cursor: (tx, ty),
            kind,
        });
    }

    fn update_drag_stroke_cursor(&mut self, tx: i32, ty: i32) {
        if let Some(stroke) = self.drag_stroke.as_mut() {
            stroke.cursor = (tx, ty);
        }
    }

    fn commit_drag_stroke(&mut self) {
        let Some(stroke) = self.drag_stroke.take() else {
            return;
        };
        match stroke.kind {
            StrokeKind::Road { paint_on } => {
                let Some(brush) = self.road_brush_guid else {
                    return;
                };
                let path = orthogonal_road_path(
                    stroke.start.0,
                    stroke.start.1,
                    stroke.cursor.0,
                    stroke.cursor.1,
                );
                let mut changed = 0usize;
                if let Some(open_mut) = self.open.as_mut() {
                    changed = apply_road_path(
                        &mut open_mut.stamp.document,
                        &open_mut.stamp.buildings,
                        &path,
                        paint_on,
                        brush,
                    );
                    if changed > 0 {
                        open_mut.dirty = true;
                        self.schedule_autosave();
                    }
                }
                self.clear_canvas_selection();
                self.refresh_coverage();
                let verb = if paint_on { "gesetzt" } else { "entfernt" };
                self.status = format!(
                    "Weg {verb}: {} Zelle(n) · Linie/Knick ({},{})→({},{})",
                    path.len().max(changed),
                    stroke.start.0,
                    stroke.start.1,
                    stroke.cursor.0,
                    stroke.cursor.1
                );
            }
            StrokeKind::SelectRect => {
                let (min_x, min_y, max_x, max_y) = rect_bounds(stroke.start, stroke.cursor);
                let mut hits = Vec::new();
                let mut road_hits = Vec::new();
                if let Some(open_ref) = self.open.as_ref() {
                    let region = open_ref.region_guid;
                    let drag = read_mesh_info(&open_ref.stamp.document)
                        .map(|i| i.drag_center)
                        .unwrap_or((0.0, 0.0));
                    for (i, b) in open_ref.stamp.buildings.iter().enumerate() {
                        let def = self.catalog.building(b.guid);
                        if def.is_some_and(|d| d.category == "street") {
                            continue;
                        }
                        let footprint = footprint_cells_for_region(b, def, region, drag);
                        let hit = footprint.iter().any(|c| {
                            c.x >= min_x && c.x <= max_x && c.y >= min_y && c.y <= max_y
                        });
                        if hit {
                            hits.push(i);
                        }
                    }
                    for cell in rect_cells(min_x, min_y, max_x, max_y) {
                        if self
                            .coverage
                            .roads
                            .contains_key(&stamp_editor::coverage::Cell {
                                x: cell.0,
                                y: cell.1,
                            })
                        {
                            road_hits.push(cell);
                        }
                    }
                }
                let n = hits.len();
                let n_roads = road_hits.len();
                self.set_canvas_selection_with_roads(hits, road_hits);
                self.refresh_coverage();
                self.status = match (n == 0, n_roads == 0) {
                    (true, true) => "Auswahl leer".into(),
                    (false, true) => {
                        format!("{n} Gebäude markiert — Ctrl+C / Ctrl+X / Entf")
                    }
                    (true, false) => {
                        format!("{n_roads} Weg-Zellen markiert — Ctrl+C / Ctrl+X / Entf")
                    }
                    (false, false) => format!(
                        "{n} Gebäude + {n_roads} Wege markiert — Ctrl+C / Ctrl+X / Entf"
                    ),
                };
            }
            StrokeKind::DemolishRect => {
                let (min_x, min_y, max_x, max_y) = rect_bounds(stroke.start, stroke.cursor);
                let cells = rect_cells(min_x, min_y, max_x, max_y);
                let mut roads_n = 0usize;
                let mut bld_n = 0usize;
                if let Some(open_mut) = self.open.as_mut() {
                    roads_n = apply_road_path(
                        &mut open_mut.stamp.document,
                        &open_mut.stamp.buildings,
                        &cells,
                        false,
                        0,
                    );
                    let region = open_mut.region_guid;
                    let drag = read_mesh_info(&open_mut.stamp.document)
                        .map(|i| i.drag_center)
                        .unwrap_or((0.0, 0.0));
                    let mut remove_idx: Vec<usize> = Vec::new();
                    for (i, b) in open_mut.stamp.buildings.iter().enumerate() {
                        let def = self.catalog.building(b.guid);
                        if def.is_some_and(|d| d.category == "street") {
                            continue;
                        }
                        let footprint = footprint_cells_for_region(b, def, region, drag);
                        let hit = footprint.iter().any(|c| {
                            c.x >= min_x && c.x <= max_x && c.y >= min_y && c.y <= max_y
                        });
                        if hit {
                            remove_idx.push(i);
                        }
                    }
                    remove_idx.sort_unstable_by(|a, b| b.cmp(a));
                    for i in remove_idx {
                        if open_mut.stamp.remove_building(i).is_ok() {
                            bld_n += 1;
                        }
                    }
                    if roads_n > 0 || bld_n > 0 {
                        open_mut.dirty = true;
                        self.schedule_autosave();
                    }
                }
                self.clear_canvas_selection();
                self.module_parent_index = None;
                self.refresh_coverage();
                self.status = format!(
                    "Abriss-Rechteck: {roads_n} Weg-Zellen, {bld_n} Gebäude · ({min_x},{min_y})–({max_x},{max_y})"
                );
            }
            StrokeKind::PlaceRect => {
                let Some(guid) = self.placing_guid else {
                    return;
                };
                let variation = self.place_variation;
                let from_clipboard = self.hand_from_clipboard;
                let (bw, bh) = if from_clipboard {
                    self.clipboard
                        .iter()
                        .find(|c| c.is_primary)
                        .map(|p| self.clipboard_oriented_size(p))
                        .unwrap_or_else(|| self.building_size(guid, variation))
                } else {
                    self.building_size(guid, variation)
                };
                let bw_i = bw.round().max(1.0) as i32;
                let bh_i = bh.round().max(1.0) as i32;
                let (min_x, min_y, max_x, max_y) = rect_bounds(stroke.start, stroke.cursor);
                // Clipboard groups step by the whole group AABB so a marquee the size of
                // the selection places one copy — not a broken primary-sized grid.
                let (step_w, step_h) = if from_clipboard {
                    self.clipboard_group_span()
                } else {
                    (bw_i, bh_i)
                };
                let mut centers = Vec::new();
                let mut gy = min_y;
                while gy + step_h - 1 <= max_y {
                    let mut gx = min_x;
                    while gx + step_w - 1 <= max_x {
                        centers.push(place_center_from_top_left(gx, gy, bw_i, bh_i));
                        gx += step_w;
                    }
                    gy += step_h;
                }
                if centers.is_empty() {
                    // start/cursor are footprint top-left tiles — never treat them as centers.
                    centers.push(place_center_from_top_left(
                        stroke.start.0,
                        stroke.start.1,
                        bw_i,
                        bh_i,
                    ));
                }
                let mut placed = 0usize;
                let mut blocked = 0usize;
                let mut last_main = None;
                let extra_n = self.clipboard.len().saturating_sub(1);
                let road_n = self.clipboard_roads.len();

                if from_clipboard {
                    // Full group (all buildings + roads) at each grid center.
                    let parent_idx = self.module_parent_index;
                    let parent_pos_r = parent_idx.and_then(|pi| {
                        self.open.as_ref().and_then(|o| {
                            o.stamp.buildings.get(pi).map(|p| {
                                (p.x, p.y, module_radius(self.catalog.building(p.guid)))
                            })
                        })
                    });
                    for (sx, sy) in centers {
                        if let Some((px, py, r)) = parent_pos_r {
                            if r > 0.0 && dist_tiles(px, py, sx, sy) > r {
                                blocked += 1;
                                continue;
                            }
                        }
                        match self.place_clipboard_group_at(sx, sy) {
                            Some(idx) => {
                                last_main = Some(idx);
                                placed += 1;
                            }
                            None => blocked += 1,
                        }
                    }
                    if placed > 0 {
                        if let Some(i) = last_main {
                            self.select_only(i);
                        }
                        self.schedule_autosave();
                    }
                    self.refresh_coverage();
                    let label = self.catalog.label(guid);
                    self.status = if blocked > 0 {
                        format!("Platziert {placed}× Gruppe (übersprungen {blocked})")
                    } else if extra_n > 0 || road_n > 0 {
                        format!(
                            "Platziert {placed}× Gruppe ({label} +{extra_n}, {road_n} Wege)"
                        )
                    } else {
                        format!("Platziert {placed}× {label}")
                    };
                    return;
                }

                let place_dir = 0;
                let parent_idx = self.module_parent_index;
                if let Some(open_mut) = self.open.as_mut() {
                    let parent_pos_r = parent_idx.and_then(|pi| {
                        open_mut.stamp.buildings.get(pi).map(|p| {
                            (
                                p.x,
                                p.y,
                                module_radius(self.catalog.building(p.guid)),
                            )
                        })
                    });
                    for (sx, sy) in centers {
                        if let Some((px, py, r)) = parent_pos_r {
                            if r > 0.0 && dist_tiles(px, py, sx, sy) > r {
                                blocked += 1;
                                continue;
                            }
                        }
                        match open_mut.stamp.add_building_from_template(
                            guid, sx, sy, place_dir, variation,
                        ) {
                            Ok(()) => {
                                let new_idx = open_mut.stamp.buildings.len() - 1;
                                if let Some(pi) = parent_idx {
                                    let oid = open_mut.stamp.buildings[pi]
                                        .complex_owner_id
                                        .unwrap_or_else(|| {
                                            (pi as u64).wrapping_mul(1009).wrapping_add(7)
                                        });
                                    let _ = open_mut.stamp.set_building_owner(pi, oid);
                                    let _ = open_mut.stamp.set_building_owner(new_idx, oid);
                                }
                                last_main = Some(new_idx);
                                placed += 1;
                            }
                            Err(_) => blocked += 1,
                        }
                    }
                    if placed > 0 {
                        open_mut.dirty = true;
                        if let Some(i) = last_main {
                            self.select_only(i);
                        }
                    }
                }
                if placed > 0 {
                    self.schedule_autosave();
                }
                self.refresh_coverage();
                let label = self.catalog.label(guid);
                self.status = if blocked > 0 {
                    format!("Platziert {placed}× {label} (übersprungen {blocked})")
                } else {
                    format!("Platziert {placed}× {label} im Rechteck")
                };
            }
        }
    }

    fn paint_tex(ui: &egui::Ui, tex: &egui::TextureHandle, rect: Rect, tint: Color32) {
        ui.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    }

    fn bonus_attr_short(attr: &str) -> &str {
        match attr {
            "FireSafety" => "Brandschutz",
            "Health" => "Gesundheit",
            "Happiness" => "Zufriedenheit",
            "Money" | "Income" => "Geld",
            "Workforce" => "Arbeitskraft",
            "Faith" => "Glaube",
            "Knowledge" => "Wissen",
            "Prestige" => "Prestige",
            "Attractiveness" => "Attraktivität",
            "ResidenceProsperity" => "Wohlstand",
            "Population" => "Bevölkerung",
            "ProvidedNeed" => "Need",
            other => other,
        }
    }

    fn format_bonus_chip(attr: &str, value: f32) -> String {
        if attr == "ProvidedNeed" {
            return "Need ✓".into();
        }
        let name = Self::bonus_attr_short(attr);
        if value.fract().abs() < 0.05 {
            format!("{name} {value:+.0}")
        } else {
            format!("{name} {value:+.1}")
        }
    }

    /// Compact incoming-buff summary: aggregated attrs + source count (no per-source spam).
    fn format_incoming_summary(hits: &[stamp_editor::coverage::EffectHit]) -> Vec<String> {
        let mut agg: std::collections::BTreeMap<String, f32> =
            std::collections::BTreeMap::new();
        let mut need_count = 0usize;
        for h in hits {
            for b in &h.bonuses {
                if b.attr == "ProvidedNeed" {
                    need_count += 1;
                    continue;
                }
                *agg.entry(b.attr.clone()).or_default() += b.value;
            }
        }
        let mut lines: Vec<String> = agg
            .iter()
            .take(6)
            .map(|(a, v)| Self::format_bonus_chip(a, *v))
            .collect();
        if need_count > 0 {
            lines.push(format!("Needs ✓ {need_count}"));
        }
        if !hits.is_empty() {
            lines.push(format!("{} Quellen", hits.len()));
        }
        lines
    }

    fn paint_bonus_label(&self, painter: &egui::Painter, anchor: Pos2, lines: &[String], color: Color32) {
        if lines.is_empty() {
            return;
        }
        let font = egui::FontId::proportional(11.0);
        let mut width = 0.0_f32;
        let mut galley_lines = Vec::with_capacity(lines.len());
        for line in lines {
            let g = painter.layout_no_wrap(line.clone(), font.clone(), Color32::WHITE);
            width = width.max(g.size().x);
            galley_lines.push(g);
        }
        let line_h = 13.0_f32;
        let pad_x = 5.0;
        let pad_y = 3.0;
        let h = line_h * lines.len() as f32 + pad_y * 2.0;
        let w = width + pad_x * 2.0;
        let top_left = Pos2::new(anchor.x - w * 0.5, anchor.y - h - 4.0);
        let bg = Rect::from_min_size(top_left, Vec2::new(w, h));
        painter.rect_filled(bg, 3.0, Color32::from_rgba_unmultiplied(20, 22, 28, 210));
        painter.rect_stroke(
            bg,
            3.0,
            Stroke::new(1.0_f32, color),
            egui::StrokeKind::Outside,
        );
        for (i, g) in galley_lines.into_iter().enumerate() {
            let pos = Pos2::new(bg.min.x + pad_x, bg.min.y + pad_y + i as f32 * line_h);
            painter.galley(pos, g, color);
        }
    }

    /// Draw a game icon centered in a building footprint rectangle.
    /// Returns true when an icon was painted (caller can skip text fallback).
    fn paint_building_icon(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        outline: Rect,
        guid: u32,
        alpha: f32,
    ) -> bool {
        let min_side = outline.width().min(outline.height());
        if min_side < 8.0 {
            return false;
        }
        let icon_path = self
            .buildmenu
            .building_icon(guid)
            .map(|s| s.to_string());
        let Some(path) = icon_path else {
            return false;
        };
        let Some(tex) = self.ui_icons.get(ctx, Some(&path)) else {
            return false;
        };
        let pad = (min_side * 0.08).clamp(1.0, 6.0);
        // Scale with footprint; allow large warehouses to show a readable icon.
        let side = (min_side - pad * 2.0).clamp(6.0, 128.0);
        let icon_rect = Rect::from_center_size(outline.center(), Vec2::splat(side));
        let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
        painter.image(
            tex.id(),
            icon_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_rgba_unmultiplied(255, 255, 255, a),
        );
        true
    }

    /// Circular goods / chain slot — drawn geometrically (chrome PNGs are off-center).
    fn goods_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        game_icon: Option<&str>,
        selected: bool,
        tip: &str,
    ) -> egui::Response {
        let size = Vec2::splat(48.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let c = rect.center();
        let radius = size.x * 0.46;
        ui.painter().circle_filled(
            c,
            radius,
            if selected {
                Color32::from_rgb(48, 42, 58)
            } else {
                Color32::from_rgb(30, 36, 48)
            },
        );
        ui.painter().circle_stroke(
            c,
            radius,
            Stroke::new(
                if selected { 2.0_f32 } else { 1.25_f32 },
                if selected {
                    Color32::from_rgb(220, 190, 120)
                } else {
                    Color32::from_rgb(70, 85, 110)
                },
            ),
        );
        if selected {
            let tip_p = Pos2::new(c.x, rect.top() + 1.0);
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    tip_p + Vec2::new(0.0, -5.0),
                    tip_p + Vec2::new(4.5, 2.0),
                    tip_p + Vec2::new(-4.5, 2.0),
                ],
                Color32::from_rgb(240, 240, 245),
                Stroke::new(0.0_f32, Color32::TRANSPARENT),
            ));
        }
        let icon_side = size.x * 0.58;
        let img_rect = Rect::from_center_size(c, Vec2::splat(icon_side));
        let tex = self
            .ui_icons
            .get_or_placeholder(ctx, game_icon, [64, 64], [40, 50, 70]);
        Self::paint_tex(ui, &tex, img_rect, Color32::WHITE);
        resp.on_hover_text(tip)
    }

    fn portrait_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        portrait: Option<&str>,
        // Roman mark as text ("I"…"IV") — game numeric PNGs crush at badge size.
        roman: &str,
        selected: bool,
        tip: &str,
    ) -> egui::Response {
        // Keep under BUILD_STRIP_H so the strip can vertically center circles + portraits.
        let size = Vec2::new(40.0, 48.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let frame_name = if selected {
            chrome::BTN_SELECTED
        } else {
            chrome::BTN_NORMAL
        };
        if let Some(frame) = self.ui_icons.chrome(ctx, frame_name) {
            Self::paint_tex(ui, &frame, rect, Color32::WHITE);
        } else {
            let fill = if selected {
                Color32::from_rgb(98, 38, 72)
            } else {
                Color32::from_rgb(26, 34, 48)
            };
            ui.painter().rect(
                rect,
                4.0,
                fill,
                Stroke::new(1.0_f32, Color32::from_rgb(50, 62, 85)),
                egui::StrokeKind::Outside,
            );
        }
        if selected {
            // Active tier: purple wash + blue rim (ingame).
            ui.painter().rect_filled(
                rect.shrink(2.0),
                3.0,
                Color32::from_rgba_unmultiplied(110, 40, 80, 90),
            );
            ui.painter().rect_stroke(
                rect,
                4.0,
                Stroke::new(2.0_f32, Color32::from_rgb(100, 160, 230)),
                egui::StrokeKind::Outside,
            );
        }
        let badge_h = 13.0_f32;
        let img_h = (rect.height() - badge_h - 5.0).max(16.0);
        let img_rect = Rect::from_min_size(
            Pos2::new(rect.left() + 4.0, rect.top() + 3.0),
            Vec2::new(rect.width() - 8.0, img_h),
        );
        let tex = self
            .ui_icons
            .get_or_placeholder(ctx, portrait, [72, 96], [45, 40, 55]);
        Self::paint_tex(ui, &tex, img_rect, Color32::WHITE);
        let badge_w = match roman.chars().count() {
            0 | 1 => 14.0,
            2 => 18.0,
            _ => 22.0,
        };
        let badge = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.bottom() - badge_h * 0.5 - 1.0),
            Vec2::new(badge_w, badge_h),
        );
        ui.painter()
            .rect_filled(badge, 2.0, Color32::from_rgb(12, 16, 24));
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            roman,
            egui::FontId::proportional(11.0),
            Color32::from_rgb(235, 225, 195),
        );
        resp.on_hover_text(tip)
    }

    fn snap_world(&self, x: f32, y: f32) -> (f32, f32) {
        if self.allow_45_deg {
            ((x * 2.0).round() / 2.0, (y * 2.0).round() / 2.0)
        } else {
            (x.round(), y.round())
        }
    }

    /// Footprint top-left tile so the building stays centered under the cursor
    /// while still snapping to the grid.
    fn stamp_drag(&self) -> (f32, f32) {
        self.open
            .as_ref()
            .and_then(|o| read_mesh_info(&o.stamp.document))
            .map(|i| i.drag_center)
            .unwrap_or((0.0, 0.0))
    }

    fn place_top_left_from_mouse(&self, wx: f32, wy: f32, bw: f32, bh: f32) -> (i32, i32) {
        let bw_i = bw.round().max(1.0) as i32;
        let bh_i = bh.round().max(1.0) as i32;
        let drag = self.stamp_drag();
        (
            relative_to_tile(wx - bw_i as f32 * 0.5, drag.0),
            relative_to_tile(wy - bh_i as f32 * 0.5, drag.1),
        )
    }

    /// Snap mouse → building center (cursor stays in the middle of the footprint).
    fn snap_place_center(&self, wx: f32, wy: f32, bw: f32, bh: f32) -> (f32, f32) {
        let bw_i = bw.round().max(1.0) as i32;
        let bh_i = bh.round().max(1.0) as i32;
        if self.allow_45_deg {
            return self.snap_world(wx, wy);
        }
        let (tx, ty) = self.place_top_left_from_mouse(wx, wy, bw, bh);
        place_center_from_top_left(tx, ty, bw_i, bh_i)
    }

    /// Tile under the cursor for the current tool (place = footprint anchor, else cell).
    fn stroke_anchor_tile(&self, wx: f32, wy: f32) -> (i32, i32) {
        if let Some(guid) = self.placing_guid {
            if !self.road_erase_mode && self.road_brush_guid.is_none() {
                let (bw, bh) = self.building_size(guid, self.place_variation);
                return self.place_top_left_from_mouse(wx, wy, bw, bh);
            }
        }
        (wx.floor() as i32, wy.floor() as i32)
    }

    fn next_direction(&self, dir: i32) -> i32 {
        let d = dir.rem_euclid(8);
        if self.allow_45_deg {
            (d + 1).rem_euclid(8)
        } else if d % 2 == 1 {
            // Diagonal → next cardinal, then stamps stay on 90° grid.
            (d + 1).rem_euclid(8)
        } else {
            (d + 2).rem_euclid(8)
        }
    }

    fn selection_drag_active(&self) -> bool {
        self.drag_group_origins.len() > 1 || !self.drag_group_roads.is_empty()
    }

    fn begin_selection_drag(&mut self, primary_idx: usize) {
        self.drag_group_origins.clear();
        self.drag_group_roads.clear();
        self.drag_group_roads_live.clear();
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let mut indices = self.selection_indices();
        if !indices.contains(&primary_idx) {
            indices.push(primary_idx);
        }
        for i in indices {
            if let Some(b) = open.stamp.buildings.get(i) {
                self.drag_group_origins.insert(i, (b.x, b.y));
            }
        }
        if let Some(b) = open.stamp.buildings.get(primary_idx) {
            self.drag_anchor_start = (b.x, b.y);
        }
        for &(rx, ry) in &self.selected_roads {
            let guid = self
                .coverage
                .roads
                .get(&stamp_editor::coverage::Cell { x: rx, y: ry })
                .map(|k| match k {
                    RoadKind::Stone => ROAD_GUID_STONE,
                    RoadKind::Dirt => ROAD_GUID_DIRT,
                })
                .unwrap_or(ROAD_GUID_DIRT);
            self.drag_group_roads.push((rx, ry, guid));
        }
    }

    fn clear_selection_drag(&mut self) {
        self.drag_group_origins.clear();
        self.drag_group_roads.clear();
        self.drag_group_roads_live.clear();
    }

    /// Move every selected building (+ roads) by the delta of the dragged primary.
    fn apply_selection_drag(&mut self, _primary_idx: usize, sx: f32, sy: f32) {
        let (ox, oy) = self.drag_anchor_start;
        let dx = sx - ox;
        let dy = sy - oy;
        let tdx = dx.round() as i32;
        let tdy = dy.round() as i32;

        // Farm mains in the selection move their ComplexOwner group via move_linked.
        let mut covered_by_link: HashSet<usize> = HashSet::new();
        let mains_with_link: Vec<usize> = {
            let Some(open) = self.open.as_ref() else {
                return;
            };
            self.drag_group_origins
                .keys()
                .copied()
                .filter(|&i| {
                    let b = match open.stamp.buildings.get(i) {
                        Some(b) => b,
                        None => return false,
                    };
                    let Some(own) = b.complex_owner_id.filter(|id| *id != 0) else {
                        return false;
                    };
                    let is_main = complex_main_index(
                        &open.stamp.buildings,
                        &self.catalog,
                        i,
                    ) == Some(i);
                    let need = needed_modules(self.catalog.building(b.guid));
                    let has_cells = open
                        .stamp
                        .polygon_cells
                        .iter()
                        .any(|c| c.complex_owner_id == Some(own));
                    is_main && (need > 0 || has_cells || self.move_group)
                })
                .collect()
        };
        if let Some(open) = self.open.as_ref() {
            for &mi in &mains_with_link {
                for gi in complex_group_indices(&open.stamp.buildings, mi) {
                    if gi != mi {
                        covered_by_link.insert(gi);
                    }
                }
            }
        }

        let origins: Vec<(usize, f32, f32)> = self
            .drag_group_origins
            .iter()
            .map(|(&i, &(x, y))| (i, x, y))
            .collect();
        // Move higher indices first? set_building_pos doesn't change indices.
        for &(i, bx, by) in &origins {
            if covered_by_link.contains(&i) {
                continue;
            }
            let link = mains_with_link.contains(&i);
            if let Some(open_mut) = self.open.as_mut() {
                let _ = open_mut
                    .stamp
                    .set_building_pos_ex(i, bx + dx, by + dy, link);
                open_mut.dirty = true;
            }
        }

        // Roads: erase previous live (or originals on first frame), paint at origin+delta.
        if !self.drag_group_roads.is_empty() {
            let erase_cells: Vec<(i32, i32)> = if self.drag_group_roads_live.is_empty() {
                self.drag_group_roads
                    .iter()
                    .map(|&(x, y, _)| (x, y))
                    .collect()
            } else {
                self.drag_group_roads_live.clone()
            };
            let mut paint_by_guid: HashMap<i32, Vec<(i32, i32)>> = HashMap::new();
            let mut live = Vec::new();
            for &(x, y, guid) in &self.drag_group_roads {
                let nx = x + tdx;
                let ny = y + tdy;
                paint_by_guid.entry(guid).or_default().push((nx, ny));
                live.push((nx, ny));
            }
            if let Some(open_mut) = self.open.as_mut() {
                let _ = apply_road_path(
                    &mut open_mut.stamp.document,
                    &open_mut.stamp.buildings,
                    &erase_cells,
                    false,
                    0,
                );
                for (guid, cells) in paint_by_guid {
                    let _ = apply_road_path(
                        &mut open_mut.stamp.document,
                        &open_mut.stamp.buildings,
                        &cells,
                        true,
                        guid,
                    );
                }
                open_mut.dirty = true;
            }
            self.drag_group_roads_live = live;
            // Keep selected_roads in sync with live positions.
            self.selected_roads.clear();
            self.selected_roads.extend(self.drag_group_roads_live.iter().copied());
        }
        self.schedule_autosave();
        self.refresh_coverage();
        let n = self.drag_group_origins.len();
        let nr = self.drag_group_roads.len();
        if n > 1 || nr > 0 {
            self.status = format!("Auswahl verschoben ({n} Gebäude, {nr} Wege)");
        }
    }

    fn delete_selection(&mut self, group_ops: bool) {
        let mut indices = self.selection_indices();
        let roads_to_clear: Vec<(i32, i32)> = self.selected_roads.iter().copied().collect();
        if indices.is_empty() && roads_to_clear.is_empty() {
            return;
        }
        self.push_history();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if group_ops && indices.len() == 1 {
            let idx = indices[0];
            if let Some(owner) = open
                .stamp
                .buildings
                .get(idx)
                .and_then(|b| b.complex_owner_id)
                .filter(|o| *o != 0)
            {
                indices = open
                    .stamp
                    .buildings
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.complex_owner_id == Some(owner))
                    .map(|(i, _)| i)
                    .collect();
            }
        }
        indices.sort_unstable_by(|a, b| b.cmp(a));
        let mut ok = 0usize;
        let mut err: Option<String> = None;
        for i in indices {
            match open.stamp.remove_building(i) {
                Ok(()) => ok += 1,
                Err(e) => {
                    err = Some(e.to_string());
                    break;
                }
            }
        }
        let mut roads_n = 0usize;
        if !roads_to_clear.is_empty() {
            roads_n = apply_road_path(
                &mut open.stamp.document,
                &open.stamp.buildings,
                &roads_to_clear,
                false,
                0,
            );
        }
        if ok > 0 || roads_n > 0 {
            open.dirty = true;
            self.schedule_autosave();
            self.clear_canvas_selection();
            self.module_parent_index = None;
            self.status = match (ok, roads_n) {
                (0, n) => format!("{n} Weg-Zellen gelöscht"),
                (b, 0) if b > 1 => format!("{b} Gebäude gelöscht"),
                (1, 0) => "Gebäude gelöscht".into(),
                (b, n) => format!("{b} Gebäude + {n} Weg-Zellen gelöscht"),
            };
            self.refresh_coverage();
        } else if let Some(e) = err {
            self.status = format!("Löschen fehlgeschlagen: {e}");
        }
    }

    fn refresh_library(&mut self) {
        self.library_packs = library::scan_library(&self.library_root);
        self.library_fingerprint = library::library_fingerprint(&self.library_root);
        self.library_watch_at = Instant::now() + Duration::from_secs(1);
        self.save_config();
        self.status = format!(
            "Bibliothek: {} — {} Packs",
            self.library_root.display(),
            self.library_packs.len()
        );
    }

    fn take_snapshot(&self) -> Option<Snapshot> {
        let open = self.open.as_ref()?;
        Some(Snapshot {
            stamp: open.stamp.clone(),
            selected_building: self.selected_building,
        })
    }

    fn push_history(&mut self) {
        if let Some(snap) = self.take_snapshot() {
            self.history.push(snap);
        }
    }

    fn apply_snapshot(&mut self, snap: Snapshot) {
        if let Some(open) = self.open.as_mut() {
            open.stamp = snap.stamp;
            open.dirty = true;
            self.schedule_autosave();
            self.selected_building = snap.selected_building;
            self.selected_buildings.clear();
            if let Some(i) = snap.selected_building {
                self.selected_buildings.insert(i);
            }
            self.refresh_coverage();
        }
    }

    

    fn refresh_coverage(&mut self) {
        let Some(open) = self.open.as_ref() else {
            self.coverage = CoverageMap::default();
            return;
        };
        // Roads always visible; influence overlay only with selection.
        let region = open.region_guid;
        if let Some(sel) = self.selected_building {
            self.coverage = compute_coverage_region(
                &open.stamp.buildings,
                &self.catalog,
                sel,
                Some(&open.stamp.document),
                region,
            );
        } else {
            self.coverage = compute_roads_only_region(
                &open.stamp.buildings,
                &self.catalog,
                Some(&open.stamp.document),
                region,
            );
        }
    }

    pub fn open_path_public(&mut self, path: PathBuf) {
        self.open_path(path);
    }

    pub fn open_named(&mut self, path: PathBuf, name: &str) {
        match open_named_stamp(&path, name) {
            Ok(open) => self.set_open(open, &path),
            Err(err) => self.status = format!("Öffnen fehlgeschlagen: {err}"),
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        match open_first_real_stamp(&path) {
            Ok(open) => self.set_open(open, &path),
            Err(err) => self.status = format!("Öffnen fehlgeschlagen: {err}"),
        }
    }

    fn set_open(&mut self, open: OpenStamp, path: &Path) {
        self.history.clear();
        self.history_drag_pushed = false;
        self.module_parent_index = None;
        // Only auto-pick region when opening a pack (global filter stays user-controlled after).
        self.region_filter = RegionFilter::from_region_guid(open.region_guid);
        self.status = format!(
            "Geöffnet: {} ({}) — {} Gebäude · Filter {}",
            open.entry_name,
            path.display(),
            open.stamp.buildings.len(),
            self.region_filter.label()
        );
        // Prefer selecting a farm main that already has a ComplexOwner group.
        self.selected_building = (0..open.stamp.buildings.len()).find(|&i| {
            let Some(owner) = open.stamp.buildings[i].complex_owner_id.filter(|o| *o != 0) else {
                return false;
            };
            if complex_main_index(&open.stamp.buildings, &self.catalog, i) != Some(i) {
                return false;
            }
            open.stamp
                .buildings
                .iter()
                .enumerate()
                .any(|(j, b)| j != i && b.complex_owner_id == Some(owner))
        });
        self.open = Some(open);
        self.center_view();
        self.refresh_coverage();
    }

    fn switch_stamp(&mut self, name: &str) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if open.entry_name == name {
            return;
        }
        let path = open.pack_path.clone();
        // Persist pending edits before replacing `self.open` (autosave debounce alone
        // would see the new stamp as clean and drop the previous dirty buffer).
        self.flush_autosave_blocking();
        match open_named_stamp(&path, name) {
            Ok(open) => {
                // Keep global region filter; do not override from stamp.
                self.status = format!(
                    "Stamp: {} — {} Gebäude · Filter {}",
                    open.entry_name,
                    open.stamp.buildings.len(),
                    self.region_filter.label()
                );
                self.clear_canvas_selection();
                self.module_parent_index = None;
                self.open = Some(open);
                self.center_view();
                self.refresh_coverage();
            }
            Err(err) => self.status = format!("Wechsel fehlgeschlagen: {err}"),
        }
    }

    fn save_current(&mut self) {
        if self.open.is_none() {
            self.status = "Nichts zu speichern.".into();
            return;
        }
        let region_guid = self.region_filter.stamp_region_guid();
        let library_root = self.library_root.clone();
        let (is_new, start_dir, suggested) = {
            let open = self.open.as_ref().unwrap();
            let start_dir = open
                .pack_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| library_root.join("unsorted"));
            let suggested = open
                .pack_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{}.json", open.entry_name));
            (open.is_new, start_dir, suggested)
        };
        if is_new {
            let Some(path) = rfd::FileDialog::new()
                .add_filter("Stamp Pack", &["json"])
                .set_directory(start_dir)
                .set_file_name(&suggested)
                .save_file()
            else {
                return;
            };
            let open = self.open.as_mut().unwrap();
            open.region_guid = region_guid;
            open.pack_path = path;
            if let Some(stem) = open.pack_path.file_stem() {
                open.entry_name = stem.to_string_lossy().into_owned();
            }
        }
        let save_result = {
            let open = self.open.as_ref().unwrap();
            save_open_stamp(open)
        };
        match save_result {
            Ok(()) => {
                let path = {
                    let open = self.open.as_mut().unwrap();
                    open.dirty = false;
                    open.is_new = false;
                    open.pack_path.clone()
                };
                self.refresh_library();
                self.status = format!("Gespeichert: {}", path.display());
            }
            Err(err) => self.status = format!("Speichern fehlgeschlagen: {err}"),
        }
    }

    fn center_view(&mut self) {
        let Some(open) = &self.open else { return };
        let mut sx = 0.0f32;
        let mut sy = 0.0f32;
        let mut n = 0usize;
        for b in open.stamp.all_draw_buildings() {
            sx += b.x;
            sy += b.y;
            n += 1;
        }
        for c in &open.stamp.polygon_cells {
            sx += c.x as f32 + 0.5;
            sy += c.y as f32 + 0.5;
            n += 1;
        }
        if n == 0 {
            self.pan = Vec2::ZERO;
            return;
        }
        let avg_x = sx / n as f32;
        let avg_y = sy / n as f32;
        self.pan = Vec2::new(-avg_x * self.cell_px, -avg_y * self.cell_px);
    }

    fn world_to_screen(&self, rect: Rect, x: f32, y: f32) -> Pos2 {
        Pos2::new(
            rect.center().x + self.pan.x + x * self.cell_px,
            rect.center().y + self.pan.y + y * self.cell_px,
        )
    }

    fn screen_to_world(&self, rect: Rect, pos: Pos2) -> (f32, f32) {
        let x = (pos.x - rect.center().x - self.pan.x) / self.cell_px;
        let y = (pos.y - rect.center().y - self.pan.y) / self.cell_px;
        (x, y)
    }

    /// Integer tile (x,y) → screen rect covering that full cell (voxel/blueprint style).
    fn cell_screen_rect(&self, rect: Rect, x: i32, y: i32) -> Rect {
        let min = self.world_to_screen(rect, x as f32, y as f32);
        let max = self.world_to_screen(rect, x as f32 + 1.0, y as f32 + 1.0);
        Rect::from_min_max(min, max)
    }

    fn building_size(&self, guid: u32, variation: Option<u32>) -> (f32, f32) {
        self.catalog
            .building(guid)
            .map(|d| {
                let s = d.size_for_variation(variation);
                (s[0].max(1.0), s[1].max(1.0))
            })
            .unwrap_or((2.0, 2.0))
    }

    fn building_hit_rect(
        &self,
        rect: Rect,
        x: f32,
        y: f32,
        guid: u32,
        dir: i32,
        variation: Option<u32>,
    ) -> Rect {
        let (w0, h0) = self.building_size(guid, variation);
        let d = dir.rem_euclid(8);
        if d % 2 == 1 {
            // 45° diamond AABB
            let (w, h) = (w0, h0);
            let center = self.world_to_screen(rect, x, y);
            let s = (w + h) * 0.5 * std::f32::consts::SQRT_2;
            return Rect::from_center_size(center, Vec2::splat(s * self.cell_px));
        }
        let (w, h) = oriented_size(w0, h0, dir);
        let (x0, y0) = footprint_top_left(x, y, w, h, self.stamp_drag());
        let bw = w.round().max(1.0) as i32;
        let bh = h.round().max(1.0) as i32;
        Rect::from_min_max(
            self.cell_screen_rect(rect, x0, y0).min,
            self.cell_screen_rect(rect, x0 + bw - 1, y0 + bh - 1).max,
        )
    }

    fn building_at_pointer(
        &self,
        rect: Rect,
        buildings: &[Building],
        pointer: Pos2,
    ) -> Option<usize> {
        buildings.iter().enumerate().find_map(|(idx, b)| {
            let r = self.building_hit_rect(
                rect,
                b.x,
                b.y,
                b.guid,
                b.direction,
                b.variation,
            );
            r.contains(pointer).then_some(idx)
        })
    }

    /// Floating controls: module count + place-field buttons for the focused farm.
    fn show_complex_module_ui(&mut self, ctx: &egui::Context) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let Some(sel) = self.selected_building.or(self.hover_building) else {
            return;
        };
        // Only farms/pastures with a ComplexOwnerID — kitchens without owner stay quiet.
        let Some(main_idx) = complex_main_index(&open.stamp.buildings, &self.catalog, sel) else {
            return;
        };
        let Some(main_b) = open.stamp.buildings.get(main_idx) else {
            return;
        };
        let main_guid = main_b.guid;
        let main_def = self.catalog.building(main_guid);
        let need = needed_modules(main_def);
        if need == 0 {
            return;
        }
        let region = open.region_guid;
        let mod_btns: Vec<(u32, String)> = main_def
            .map(|d| {
                suggested_modules(&self.catalog, d, region)
                    .into_iter()
                    .map(|m| {
                        let l = m
                            .short_name
                            .clone()
                            .unwrap_or_else(|| stamp_editor::catalog::short_label(&m.name));
                        (m.guid, l)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let have = count_progress_for_stamp_area(
                            &open.stamp.buildings,
                            &open.stamp.polygon_modules,
                            &open.stamp.polygon_cells,
                            &open.stamp.polygon_area,
            main_idx,
            main_def,
        );
        let main_label = self.catalog.label(main_guid);
        let ready = need > 0 && have as u32 >= need;

        // Area (not Window): cannot be dismissed/persisted-closed; always visible while focused.
        egui::Area::new(egui::Id::new("complex_module_ui"))
            .anchor(egui::Align2::RIGHT_TOP, [-16.0, 72.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_min_width(200.0);
                        ui.label(RichText::new("Untergebäude").strong());
                        ui.label(RichText::new(&main_label).strong());
                        let color = if ready {
                            Color32::from_rgb(90, 180, 90)
                        } else if need > 0 {
                            Color32::from_rgb(220, 160, 60)
                        } else {
                            Color32::GRAY
                        };
                        let unit = if stamp_editor::modules::uses_area_progress(main_def) {
                            "Fläche (Kacheln)"
                        } else {
                            "Felder / Module"
                        };
                        ui.colored_label(color, format!("{unit}: {have}/{need}"));
                        ui.label(
                            RichText::new(
                                "Zusammengehörige Gebäude sind auf dem Canvas umrandet.",
                            )
                            .small()
                            .weak(),
                        );
                        for (mg, ml) in &mod_btns {
                            if ui.button(format!("+ {ml} platzieren")).clicked() {
                                self.placing_guid = Some(*mg);
                                self.module_parent_index = Some(main_idx);
                                self.palette_selected_guid = Some(*mg);
                                self.hand_from_clipboard = false;
                                self.status = format!(
                                    "Modul „{ml}“ — im Radius der Farm platzieren (Owner-Link)"
                                );
                            }
                        }
                    });
            });
    }

    fn paint_radial_menu(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        painter: &egui::Painter,
        canvas: Rect,
    ) {
        let Some(mut radial) = self.radial.clone() else {
            return;
        };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dismiss_radial();
            return;
        }

        let items = RadialMenu::items();
        let n = items.len();
        let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(radial.origin);
        radial.hover = RadialMenu::sector_at(radial.origin, pointer, n);

        painter.rect_filled(
            canvas,
            0.0,
            Color32::from_rgba_unmultiplied(8, 10, 14, 110),
        );

        let origin = radial.origin;
        let inner = RadialMenu::INNER_R;
        let outer = RadialMenu::OUTER_R;
        let ring = RadialMenu::ICON_RING;
        let slot_r = RadialMenu::SLOT_R;
        let wedge = std::f32::consts::TAU / n as f32;
        let accent = Color32::from_rgb(212, 186, 130);
        let accent_hot = Color32::from_rgb(255, 230, 170);

        // Outer plate
        painter.circle_filled(
            origin,
            outer,
            Color32::from_rgba_unmultiplied(22, 24, 30, 235),
        );
        painter.circle_stroke(
            origin,
            outer,
            Stroke::new(2.0_f32, Color32::from_rgba_unmultiplied(212, 186, 130, 180)),
        );
        painter.circle_stroke(
            origin,
            outer - 4.0,
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(80, 70, 55, 160)),
        );

        for (i, item) in items.iter().enumerate() {
            let a0 = -std::f32::consts::FRAC_PI_2 + i as f32 * wedge;
            let a1 = a0 + wedge;
            let amid = (a0 + a1) * 0.5;
            let hovered = radial.hover == Some(i);

            // Soft wedge highlight
            if hovered {
                let mut points = Vec::with_capacity(16);
                points.push(origin);
                let steps = 8;
                for s in 0..=steps {
                    let t = s as f32 / steps as f32;
                    let a = a0 + (a1 - a0) * t;
                    points.push(origin + Vec2::angled(a) * (outer - 2.0));
                }
                painter.add(egui::Shape::convex_polygon(
                    points,
                    Color32::from_rgba_unmultiplied(212, 186, 130, 40),
                    Stroke::NONE,
                ));
            }

            // Divider ticks
            let tick0 = origin + Vec2::angled(a0) * (inner + 4.0);
            let tick1 = origin + Vec2::angled(a0) * (outer - 6.0);
            painter.line_segment(
                [tick0, tick1],
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(120, 110, 90, 90)),
            );

            let slot_c = origin + Vec2::angled(amid) * ring;
            let slot_fill = if hovered {
                match item.action {
                    RadialAction::Demolish => Color32::from_rgb(130, 48, 42),
                    _ => Color32::from_rgb(48, 56, 68),
                }
            } else {
                Color32::from_rgb(32, 36, 44)
            };
            painter.circle_filled(slot_c, slot_r, slot_fill);
            painter.circle_stroke(
                slot_c,
                slot_r,
                Stroke::new(
                    if hovered { 2.0_f32 } else { 1.2_f32 },
                    if hovered { accent_hot } else { accent },
                ),
            );

            if let Some(tex) = self.ui_icons.get(ctx, Some(item.icon)) {
                let side = if hovered { 34.0 } else { 30.0 };
                let icon_rect = Rect::from_center_size(slot_c, Vec2::splat(side));
                painter.image(
                    tex.id(),
                    icon_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.text(
                    slot_c,
                    egui::Align2::CENTER_CENTER,
                    item.label.chars().next().unwrap_or('?').to_string(),
                    egui::FontId::proportional(16.0),
                    Color32::WHITE,
                );
            }
        }

        // Center hub — preview hovered tool
        painter.circle_filled(origin, inner, Color32::from_rgb(18, 20, 26));
        painter.circle_stroke(
            origin,
            inner,
            Stroke::new(2.0_f32, Color32::from_rgba_unmultiplied(212, 186, 130, 200)),
        );

        let hub_item = radial.hover.map(|i| &items[i]);
        if let Some(item) = hub_item {
            if let Some(tex) = self.ui_icons.get(ctx, Some(item.icon)) {
                let icon_rect = Rect::from_center_size(origin - Vec2::Y * 6.0, Vec2::splat(36.0));
                painter.image(
                    tex.id(),
                    icon_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            painter.text(
                origin + Vec2::Y * 18.0,
                egui::Align2::CENTER_CENTER,
                item.label,
                egui::FontId::proportional(13.0),
                accent_hot,
            );
        } else {
            painter.text(
                origin,
                egui::Align2::CENTER_CENTER,
                "Tools",
                egui::FontId::proportional(14.0),
                Color32::from_gray(170),
            );
        }

        let lmb = ui.input(|i| i.pointer.primary_clicked());
        let rmb = ui.input(|i| i.pointer.secondary_clicked());
        if radial.ignore_next_click {
            radial.ignore_next_click = false;
            self.radial = Some(radial);
            return;
        }
        if lmb {
            if let Some(i) = radial.hover {
                let action = items[i].action;
                self.apply_radial_action(action);
            } else {
                let dist = (pointer - origin).length();
                if dist <= inner * 0.7 || dist > outer + 10.0 {
                    // Center / outside = cancel tool (Anno-like).
                    self.dismiss_radial();
                } else {
                    self.radial = Some(radial);
                }
            }
            return;
        }
        if rmb {
            self.dismiss_radial();
            return;
        }

        self.radial = Some(radial);
    }
}


impl StampEditorApp {
    /// Handle edit shortcuts before panels.
    ///
    /// Important: on Windows, eframe often emits `Event::Copy` / `Cut` / `Paste(String)`
    /// for Ctrl+C/X/V instead of (or in addition to) `Event::Key`. Matching only Key::C
    /// misses the real shortcut.
    fn handle_edit_shortcuts(&mut self, ctx: &egui::Context) {
        #[derive(Clone, Copy)]
        enum Chord {
            C,
            X,
            V,
            ShiftV,
            D,
            S,
            Z,
            Y,
        }
        let chords: Vec<Chord> = ctx.input(|i| {
            let mut out = Vec::new();
            for ev in &i.events {
                match ev {
                    // OS / eframe clipboard chords — this is what Ctrl+C/V usually are on Windows.
                    egui::Event::Copy => out.push(Chord::C),
                    egui::Event::Cut => out.push(Chord::X),
                    egui::Event::Paste(_) => {
                        if i.modifiers.shift {
                            out.push(Chord::ShiftV);
                        } else {
                            out.push(Chord::V);
                        }
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let cmd = modifiers.command || modifiers.ctrl;
                        if !cmd {
                            continue;
                        }
                        match key {
                            egui::Key::C if !modifiers.alt => out.push(Chord::C),
                            egui::Key::X if !modifiers.alt => out.push(Chord::X),
                            egui::Key::V if modifiers.shift => out.push(Chord::ShiftV),
                            egui::Key::V if !modifiers.alt => out.push(Chord::V),
                            egui::Key::D if !modifiers.alt => out.push(Chord::D),
                            egui::Key::S if !modifiers.shift && !modifiers.alt => {
                                out.push(Chord::S)
                            }
                            egui::Key::Y if !modifiers.alt => out.push(Chord::Y),
                            egui::Key::Z if modifiers.shift => out.push(Chord::Y),
                            egui::Key::Z if !modifiers.alt => out.push(Chord::Z),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            out
        });

        // Drop OS clipboard events so TextEdit doesn't also paste into search fields.
        ctx.input_mut(|i| {
            i.events.retain(|ev| {
                !matches!(
                    ev,
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                )
            });
        });

        for ch in chords {
            match ch {
                Chord::Z => {
                    if let Some(cur) = self.take_snapshot() {
                        if let Some(prev) = self.history.undo(cur) {
                            self.apply_snapshot(prev);
                            self.status = "Rückgängig".into();
                        }
                    }
                }
                Chord::Y => {
                    if let Some(cur) = self.take_snapshot() {
                        if let Some(next) = self.history.redo(cur) {
                            self.apply_snapshot(next);
                            self.status = "Wiederhergestellt".into();
                        }
                    }
                }
                Chord::S => self.save_current(),
                Chord::X => {
                    self.cut_selection();
                    self.sync_os_clipboard_marker(ctx);
                }
                Chord::C => {
                    if self.copy_selection_to_clipboard() {
                        self.sync_os_clipboard_marker(ctx);
                    }
                }
                Chord::ShiftV => self.paste_clipboard_at_cursor(),
                Chord::V => {
                    // Ctrl+V = into hand (click to place). Ctrl+Shift+V = instant at cursor.
                    self.arm_clipboard_in_hand();
                }
                Chord::D => self.duplicate_selection(),
            }
        }
    }
}

impl eframe::App for StampEditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick_autosave(ctx);
        self.tick_library_watch(ctx);
        {
            let st = self.ui_icons.extract_status();
            if st.running {
                self.status = st.message.clone();
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        // BEFORE panels: otherwise library/palette TextEdit steals Ctrl+C/V.
        self.handle_edit_shortcuts(ctx);
        if self.open.is_none() {
            self.start_new_stamp();
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for path in dropped {
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                self.open_path(path);
            }
        }

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            // Larger top-bar controls (menus, region, road brush, 45°).
            {
                let style = ui.style_mut();
                style.text_styles.insert(
                    egui::TextStyle::Body,
                    egui::FontId::proportional(16.0),
                );
                style.text_styles.insert(
                    egui::TextStyle::Button,
                    egui::FontId::proportional(16.0),
                );
                style.text_styles.insert(
                    egui::TextStyle::Heading,
                    egui::FontId::proportional(18.0),
                );
                style.spacing.button_padding = egui::vec2(12.0, 8.0);
                style.spacing.item_spacing = egui::vec2(12.0, 8.0);
                style.spacing.interact_size.y = 28.0;
            }
            ui.set_min_height(44.0);

            egui::menu::bar(ui, |ui| {
                ui.menu_button("Datei", |ui| {
                    if ui.button("Anno-Spielordner verknüpfen…").clicked() {
                        let start = self
                            .game_install
                            .clone()
                            .or_else(|| {
                                Some(PathBuf::from(
                                    r"G:\Program Files (x86)\Steam\steamapps\common",
                                ))
                            })
                            .unwrap_or_else(|| PathBuf::from(r"C:\"));
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Anno 117 - Pax Romana Installationsordner")
                            .set_directory(start)
                            .pick_folder()
                        {
                            self.link_game_install(path);
                        }
                        ui.close_menu();
                    }
                    if self.game_install.is_some() {
                        if ui.button("Spielordner-Verknüpfung lösen").clicked() {
                            self.game_install = None;
                            self.ui_icons.clear_game_install();
                            self.save_config();
                            self.status =
                                "Spielordner gelöst — Icons nur aus lokalem Cache, falls vorhanden."
                                    .into();
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("Bibliothek-Ordner…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_directory(&self.library_root)
                            .pick_folder()
                        {
                            self.library_root = path;
                            self.refresh_library();
                        }
                        ui.close_menu();
                    }
                    if ui.button("Bibliothek neu scannen").clicked() {
                        self.refresh_library();
                        ui.close_menu();
                    }
                    if ui.button("Nach Region sortieren…").clicked() {
                        match rfd::MessageDialog::new()
                            .set_level(rfd::MessageLevel::Warning)
                            .set_title("Packs verschieben?")
                            .set_description(
                                "Packs werden anhand RegionGUID nach Albion/Latium/unsorted verschoben.",
                            )
                            .set_buttons(rfd::MessageButtons::OkCancel)
                            .show()
                        {
                            rfd::MessageDialogResult::Ok => {
                                match library::sort_packs_by_region(
                                    &self.library_root,
                                    &self.library_packs,
                                ) {
                                    Ok(n) => {
                                        self.refresh_library();
                                        self.status = format!("{n} Pack(s) sortiert.");
                                    }
                                    Err(e) => self.status = format!("Sortieren fehlgeschlagen: {e}"),
                                }
                            }
                            _ => {}
                        }
                        ui.close_menu();
                    }
                    if ui.button("Neues Stamp").clicked() {
                        self.start_new_stamp();
                        ui.close_menu();
                    }
                    if ui.button("Pack-Datei öffnen…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Stamp Pack", &["json"])
                            .set_directory(&self.library_root)
                            .pick_file()
                        {
                            self.open_path(path);
                        }
                        ui.close_menu();
                    }
                    if ui.button("Speichern\tCtrl+S").clicked() {
                        self.save_current();
                        ui.close_menu();
                    }
                    if ui.button("Katalog neu laden").clicked() {
                        match Catalog::load(&self.catalog_path) {
                            Ok(c) => {
                                self.catalog = c;
                                self.status = "Katalog geladen.".into();
                                self.refresh_coverage();
                            }
                            Err(e) => self.status = format!("Katalog-Fehler: {e}"),
                        }
                        ui.close_menu();
                    }
                });
                ui.menu_button("Bearbeiten", |ui| {
                    if ui
                        .add_enabled(self.history.can_undo(), egui::Button::new("Rückgängig\tCtrl+Z"))
                        .clicked()
                    {
                        if let Some(cur) = self.take_snapshot() {
                            if let Some(prev) = self.history.undo(cur) {
                                self.apply_snapshot(prev);
                                self.status = "Rückgängig".into();
                            }
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.history.can_redo(),
                            egui::Button::new("Wiederherstellen\tCtrl+Y"),
                        )
                        .clicked()
                    {
                        if let Some(cur) = self.take_snapshot() {
                            if let Some(next) = self.history.redo(cur) {
                                self.apply_snapshot(next);
                                self.status = "Wiederhergestellt".into();
                            }
                        }
                        ui.close_menu();
                    }
                    ui.separator();
                    let can_copy = self.selected_building.is_some()
                        || self.hover_building.is_some()
                        || !self.selected_buildings.is_empty()
                        || self.placing_guid.is_some()
                        || self.palette_selected_guid.is_some()
                        || self
                            .open
                            .as_ref()
                            .is_some_and(|o| !o.stamp.buildings.is_empty());
                    if ui
                        .add_enabled(can_copy, egui::Button::new("Ausschneiden\tCtrl+X"))
                        .clicked()
                    {
                        self.cut_selection();
                        self.sync_os_clipboard_marker(ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(can_copy, egui::Button::new("Kopieren\tCtrl+C"))
                        .clicked()
                    {
                        if self.copy_selection_to_clipboard() {
                            self.sync_os_clipboard_marker(ctx);
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.clipboard.is_empty(),
                            egui::Button::new("Einfügen (in die Hand)\tCtrl+V"),
                        )
                        .clicked()
                    {
                        self.arm_clipboard_in_hand();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.clipboard.is_empty(),
                            egui::Button::new("Sofort unter Cursor\tCtrl+Shift+V"),
                        )
                        .clicked()
                    {
                        self.paste_clipboard_at_cursor();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.selected_building.is_some() || !self.clipboard.is_empty(),
                            egui::Button::new("Duplizieren\tCtrl+D"),
                        )
                        .clicked()
                    {
                        self.duplicate_selection();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.selected_building.is_some()
                                || !self.selected_buildings.is_empty()
                                || !self.selected_roads.is_empty(),
                            egui::Button::new("Löschen\tEntf"),
                        )
                        .clicked()
                    {
                        self.request_delete = true;
                        ui.close_menu();
                    }
                    ui.checkbox(
                        &mut self.move_group,
                        "Gruppe verschieben (Main+Module, oder Alt halten)",
                    );
                });
                ui.menu_button("Ansicht", |ui| {
                    if ui.button("Zentrieren\tF").clicked() {
                        self.center_view();
                        ui.close_menu();
                    }
                    ui.checkbox(&mut self.show_palette, "Gebäudesuche (Seitenleiste)");
                    ui.checkbox(&mut self.show_analyse, "Analyse (rechte Seite)");
                    ui.add(egui::Slider::new(&mut self.cell_px, 8.0..=48.0).text("Zoom"));
                });
                if let Some(open) = &self.open {
                    ui.separator();
                    let label = if open.dirty {
                        format!("{} *", open.entry_name)
                    } else {
                        open.entry_name.clone()
                    };
                    ui.label(RichText::new(label).strong());
                }
                ui.separator();
                if ui
                    .button(RichText::new("Kopieren").size(15.0))
                    .on_hover_text("Ctrl+C — nur in die Zwischenablage")
                    .clicked()
                {
                    if self.copy_selection_to_clipboard() {
                        self.sync_os_clipboard_marker(ctx);
                    }
                }
                if ui
                    .add_enabled(
                        !self.clipboard.is_empty(),
                        egui::Button::new(RichText::new("Einfügen").size(15.0)),
                    )
                    .on_hover_text("Ctrl+V — Ablage in die Hand · Klick setzt die ganze Gruppe")
                    .clicked()
                {
                    self.arm_clipboard_in_hand();
                }
                if let Some(p) = self.clipboard.iter().find(|c| c.is_primary) {
                    let extra = self.clipboard.len().saturating_sub(1);
                    let roads = self.clipboard_roads.len();
                    let abl = if roads > 0 {
                        format!(
                            "Ablage: {} (+{}, {} Wege)",
                            self.catalog.label(p.guid),
                            extra,
                            roads
                        )
                    } else {
                        format!("Ablage: {} (+{})", self.catalog.label(p.guid), extra)
                    };
                    ui.colored_label(
                        Color32::from_rgb(140, 210, 160),
                        RichText::new(abl).size(13.0),
                    );
                }
                ui.separator();
                // 45°-Modus: UI disabled for now (unreliable; low priority).
                self.allow_45_deg = false;
                let mut forty_five_off = false;
                ui.add_enabled(
                    false,
                    egui::Checkbox::new(
                        &mut forty_five_off,
                        RichText::new("45°-Modus").size(16.0),
                    ),
                );
                ui.label(
                    RichText::new("45° vorerst aus · R = 90° · Snap 1")
                        .size(13.0)
                        .weak(),
                );
                ui.separator();
                ui.label(RichText::new("Region").size(16.0).strong());
                for region in RegionFilter::ALL {
                    let selected = self.region_filter == region;
                    if ui
                        .selectable_label(
                            selected,
                            RichText::new(region.label()).size(16.0),
                        )
                        .clicked()
                    {
                        self.region_filter = region;
                        self.buildmenu_tab = None;
                        self.buildmenu_entry = None;
                        self.buildmenu_infra_entry = None;
                        if let Some(open) = self.open.as_mut() {
                            if open.is_new {
                                open.region_guid = region.stamp_region_guid();
                            }
                        }
                    }
                }
                });
            ui.label(RichText::new(&self.status).size(14.0));
        });

        // Height follows content: strip alone vs strip + needs/kette row.
        // Fixed 148/210 left a dead empty band when the upper row was hidden.
        const BUILD_STRIP_H: f32 = 64.0;
        let tabs_preview = self.buildmenu.tabs_for(self.region_filter).to_vec();
        if let Some(t) = self.buildmenu_tab {
            if t >= tabs_preview.len() {
                self.buildmenu_tab = if tabs_preview.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.buildmenu_entry = None;
            }
        } else if !tabs_preview.is_empty() {
            self.buildmenu_tab = tabs_preview
                .iter()
                .position(|t| t.is_need())
                .or(Some(0));
        }
        let tab_idx_preview = self.buildmenu_tab.unwrap_or(0);
        let need_mode_preview = self.buildmenu_infra_entry.is_none()
            && tabs_preview
                .get(tab_idx_preview)
                .map(|t| t.is_need())
                .unwrap_or(false);
        let show_upper_preview = need_mode_preview || self.buildmenu_infra_entry.is_some();
        // Strip (~64) + optional needs row (~60) + small gaps — no dead filler band.
        let build_bar_h: f32 = if show_upper_preview { 128.0 } else { 68.0 };
        // Library tree + stamps of the open pack.
        egui::SidePanel::left("stamps")
            .resizable(true)
            .default_width(240.0)
            .show(ctx, |ui| {
                ui.heading("Bibliothek");
                ui.label(
                    RichText::new(self.library_root.display().to_string())
                        .small()
                        .weak(),
                );
                                 ui.label(
                    RichText::new("Ctrl+Klick · Rechteck · Drag auf Ordner/Pack")
                        .small()
                        .weak(),
                );
                ui.text_edit_singleline(&mut self.stamp_list_filter);
                let text_filter = self.stamp_list_filter.to_lowercase();
                let region = self.region_filter;
                let current_pack = self.open.as_ref().map(|o| o.pack_path.clone());
                let current_stamp = self.open.as_ref().map(|o| o.entry_name.clone());
                let can_paste = self.library_clipboard.is_some();
                let mut open_pack: Option<PathBuf> = None;
                let mut switch_to: Option<String> = None;
                let mut lib_action: Option<LibraryAction> = None;
                let mut lib_sel = std::mem::take(&mut self.library_selection);
                let mut marquee = self.library_marquee.take();
                let mut item_rects: Vec<(LibItem, Rect)> = Vec::new();
                let mods = ui.input(|i| i.modifiers);
                let root_folder = self.library_root.clone();

                let scroll_out = egui::ScrollArea::vertical()
                    .id_salt("library_scroll")
                    .show(ui, |ui| {
                    let folders =
                        library::list_library_folders(&self.library_root, &self.library_packs);

                    for folder in folders {
                        let folder_path =
                            library::folder_abs_path(&self.library_root, &folder);
                        let packs: Vec<LibraryPack> = self
                            .library_packs
                            .iter()
                            .filter(|p| {
                                let f = if p.folder.is_empty() {
                                    "(Root)".to_string()
                                } else {
                                    p.folder.clone()
                                };
                                f == folder
                                    && region.matches_stamp(p.region_hint, &p.folder)
                                    && (text_filter.is_empty()
                                        || p.file_name.to_lowercase().contains(&text_filter)
                                        || p.stamps.iter().any(|s| {
                                            s.name.to_lowercase().contains(&text_filter)
                                        }))
                            })
                            .cloned()
                            .collect();
                        if packs.is_empty() && !text_filter.is_empty() {
                            continue;
                        }

                        let folder_header = egui::CollapsingHeader::new(&folder)
                            .default_open(true)
                            .show(ui, |ui| {
                                for pack in &packs {
                                    let pack_open =
                                        current_pack.as_ref().is_some_and(|p| p == &pack.path);
                                    let pack_path = pack.path.clone();
                                    let single = pack.stamps.len() == 1;

                                    if single {
                                        let entry = &pack.stamps[0];
                                        let item = LibItem::Pack(pack_path.clone());
                                        let selected = lib_sel.contains(&item)
                                            || (pack_open
                                                && current_stamp.as_deref()
                                                    == Some(entry.name.as_str()));
                                        let payload = if lib_sel.contains(&item) {
                                            LibraryDragPayload {
                                                items: lib_sel.iter().cloned().collect(),
                                            }
                                        } else {
                                            LibraryDragPayload {
                                                items: vec![item.clone()],
                                            }
                                        };
                                        // Click opens; DnD only after 12px move (dnd_drag_source steals clicks).
                                        let resp = ui.selectable_label(selected, &entry.name);
                                        item_rects.push((item.clone(), resp.rect));
                                        if resp.is_pointer_button_down_on()
                                            && Self::library_drag_armed(ui)
                                        {
                                            egui::DragAndDrop::set_payload(ui.ctx(), payload);
                                        }
                                        if resp.clicked() {
                                            if mods.ctrl || mods.command {
                                                if !lib_sel.insert(item.clone()) {
                                                    lib_sel.remove(&item);
                                                }
                                            } else {
                                                lib_sel.clear();
                                                lib_sel.insert(item);
                                                open_pack = Some(pack_path.clone());
                                                switch_to = Some(entry.name.clone());
                                            }
                                        }
                                        let stamp_name = entry.name.clone();
                                        resp.context_menu(|ui| {
                                            if ui.button("Öffnen").clicked() {
                                                lib_action = Some(LibraryAction::OpenStamp {
                                                    pack: pack_path.clone(),
                                                    name: stamp_name.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button("Duplizieren").clicked() {
                                                lib_action = Some(LibraryAction::DuplicateStamp {
                                                    pack: pack_path.clone(),
                                                    name: stamp_name.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button("Kopieren").clicked() {
                                                lib_action = Some(LibraryAction::CopyStamp {
                                                    pack: pack_path.clone(),
                                                    name: stamp_name.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button("Pack kopieren").clicked() {
                                                lib_action = Some(LibraryAction::CopyPack {
                                                    path: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Löschen…").clicked() {
                                                lib_action = Some(LibraryAction::DeletePack {
                                                    path: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                        });
                                    } else {
                                        let pack_item = LibItem::Pack(pack_path.clone());
                                        let title = pack
                                            .file_name
                                            .trim_end_matches(".json")
                                            .to_string();
                                        let pack_selected = lib_sel.contains(&pack_item);
                                        let pack_payload = if lib_sel.contains(&pack_item) {
                                            LibraryDragPayload {
                                                items: lib_sel.iter().cloned().collect(),
                                            }
                                        } else {
                                            LibraryDragPayload {
                                                items: vec![pack_item.clone()],
                                            }
                                        };
                                        let pack_header = egui::CollapsingHeader::new(
                                            if pack_selected {
                                                format!("● {title}")
                                            } else {
                                                title.clone()
                                            },
                                        )
                                        .id_salt(pack.path.to_string_lossy().as_ref())
                                        .default_open(pack_open)
                                        .show(ui, |ui| {
                                            for entry in &pack.stamps {
                                                if !text_filter.is_empty()
                                                    && !entry
                                                        .name
                                                        .to_lowercase()
                                                        .contains(&text_filter)
                                                {
                                                    continue;
                                                }
                                                let item = LibItem::Stamp {
                                                    pack: pack_path.clone(),
                                                    name: entry.name.clone(),
                                                };
                                                let selected = lib_sel.contains(&item)
                                                    || (pack_open
                                                        && current_stamp.as_deref()
                                                            == Some(entry.name.as_str()));
                                                let payload = if lib_sel.contains(&item) {
                                                    LibraryDragPayload {
                                                        items: lib_sel.iter().cloned().collect(),
                                                    }
                                                } else {
                                                    LibraryDragPayload {
                                                        items: vec![item.clone()],
                                                    }
                                                };
                                                let resp =
                                                    ui.selectable_label(selected, &entry.name);
                                                item_rects.push((item.clone(), resp.rect));
                                                if resp.is_pointer_button_down_on()
                                                    && Self::library_drag_armed(ui)
                                                {
                                                    egui::DragAndDrop::set_payload(
                                                        ui.ctx(),
                                                        payload,
                                                    );
                                                }
                                                if resp.clicked() {
                                                    if mods.ctrl || mods.command {
                                                        if !lib_sel.insert(item.clone()) {
                                                            lib_sel.remove(&item);
                                                        }
                                                    } else {
                                                        lib_sel.clear();
                                                        lib_sel.insert(item);
                                                        if pack_open {
                                                            switch_to = Some(entry.name.clone());
                                                        } else {
                                                            open_pack = Some(pack_path.clone());
                                                            switch_to = Some(entry.name.clone());
                                                        }
                                                    }
                                                }
                                                let stamp_name = entry.name.clone();
                                                let stamp_pack = pack_path.clone();
                                                resp.context_menu(|ui| {
                                                    if ui.button("Öffnen").clicked() {
                                                        lib_action =
                                                            Some(LibraryAction::OpenStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            });
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("Duplizieren").clicked() {
                                                        lib_action = Some(
                                                            LibraryAction::DuplicateStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            },
                                                        );
                                                        ui.close_menu();
                                                    }
                                                    if ui.button("Kopieren").clicked() {
                                                        lib_action =
                                                            Some(LibraryAction::CopyStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            });
                                                        ui.close_menu();
                                                    }
                                                    ui.separator();
                                                    if ui.button("Löschen…").clicked() {
                                                        lib_action =
                                                            Some(LibraryAction::DeleteStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            });
                                                        ui.close_menu();
                                                    }
                                                });
                                            }
                                        });
                                        item_rects
                                            .push((pack_item.clone(), pack_header.header_response.rect));
                                        if pack_header.header_response.is_pointer_button_down_on()
                                            && Self::library_drag_armed(ui)
                                        {
                                            egui::DragAndDrop::set_payload(
                                                ui.ctx(),
                                                pack_payload,
                                            );
                                        }
                                        if pack_header.header_response.clicked()
                                            && (mods.ctrl || mods.command)
                                        {
                                            if !lib_sel.insert(pack_item.clone()) {
                                                lib_sel.remove(&pack_item);
                                            }
                                        }
                                        if let Some(payload) = pack_header
                                            .header_response
                                            .dnd_release_payload::<LibraryDragPayload>()
                                        {
                                            lib_action = Some(LibraryAction::DropItems {
                                                items: payload.items.clone(),
                                                target: LibraryDropTarget::Pack(pack_path.clone()),
                                            });
                                        } else if pack_header
                                            .header_response
                                            .dnd_hover_payload::<LibraryDragPayload>()
                                            .is_some()
                                        {
                                            ui.painter().rect_stroke(
                                                pack_header.header_response.rect,
                                                2.0,
                                                Stroke::new(
                                                    1.5_f32,
                                                    Color32::from_rgb(90, 180, 255),
                                                ),
                                                egui::StrokeKind::Outside,
                                            );
                                        }
                                        pack_header.header_response.context_menu(|ui| {
                                            if ui.button("Neues Stamp in Pack").clicked() {
                                                lib_action = Some(LibraryAction::NewStampInPack {
                                                    pack: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui
                                                .add_enabled(
                                                    can_paste,
                                                    egui::Button::new("Einfügen"),
                                                )
                                                .clicked()
                                            {
                                                lib_action = Some(LibraryAction::PasteIntoPack {
                                                    pack: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button("Pack kopieren").clicked() {
                                                lib_action = Some(LibraryAction::CopyPack {
                                                    path: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button("Pack löschen…").clicked() {
                                                lib_action = Some(LibraryAction::DeletePack {
                                                    path: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                        });
                                    }
                                }
                            });

                        // Folder is a drop target for packs/stamps.
                        if let Some(payload) = folder_header
                            .header_response
                            .dnd_release_payload::<LibraryDragPayload>()
                        {
                            lib_action = Some(LibraryAction::DropItems {
                                items: payload.items.clone(),
                                target: LibraryDropTarget::Folder(folder_path.clone()),
                            });
                        } else if folder_header
                            .header_response
                            .dnd_hover_payload::<LibraryDragPayload>()
                            .is_some()
                        {
                            ui.painter().rect_stroke(
                                folder_header.header_response.rect,
                                2.0,
                                Stroke::new(2.0_f32, Color32::from_rgb(120, 200, 120)),
                                egui::StrokeKind::Outside,
                            );
                        }

                        let folder_for_menu = folder_path.clone();
                        folder_header.header_response.context_menu(|ui| {
                            if ui.button("Neues Stamp").clicked() {
                                lib_action = Some(LibraryAction::NewStampPack {
                                    folder: folder_for_menu.clone(),
                                });
                                ui.close_menu();
                            }
                            if ui.button("Neuer Ordner").clicked() {
                                lib_action = Some(LibraryAction::NewFolder {
                                    parent: folder_for_menu.clone(),
                                });
                                ui.close_menu();
                            }
                            if ui
                                .add_enabled(can_paste, egui::Button::new("Einfügen"))
                                .clicked()
                            {
                                lib_action = Some(LibraryAction::Paste {
                                    target_folder: folder_for_menu.clone(),
                                });
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Leeren Ordner löschen…").clicked() {
                                lib_action = Some(LibraryAction::DeleteFolder {
                                    path: folder_for_menu.clone(),
                                });
                                ui.close_menu();
                            }
                        });
                    }

                    // Fill + marquee / empty context menu.
                    let fill = ui.available_size();
                    let fill_rect = if fill.y > 8.0 {
                        let (_id, r) = ui.allocate_space(fill);
                        r
                    } else {
                        Rect::NOTHING
                    };
                    let fill_resp = ui.interact(
                        fill_rect,
                        ui.id().with("library_empty"),
                        Sense::click_and_drag(),
                    );
                    if fill_resp.drag_started() && ui.input(|i| i.pointer.primary_down()) {
                        if let Some(pos) = fill_resp.interact_pointer_pos() {
                            marquee = Some((pos, pos));
                            if !(mods.ctrl || mods.command) {
                                lib_sel.clear();
                            }
                        }
                    }
                    if fill_resp.dragged() {
                        if let (Some((start, _)), Some(pos)) =
                            (marquee.as_ref(), fill_resp.interact_pointer_pos())
                        {
                            marquee = Some((*start, pos));
                        }
                    }
                    fill_resp.context_menu(|ui| {
                        if ui.button("Neues Stamp").clicked() {
                            lib_action = Some(LibraryAction::NewStampPack {
                                folder: root_folder.clone(),
                            });
                            ui.close_menu();
                        }
                        if ui.button("Neuer Ordner").clicked() {
                            lib_action = Some(LibraryAction::NewFolder {
                                parent: root_folder.clone(),
                            });
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(can_paste, egui::Button::new("Einfügen"))
                            .clicked()
                        {
                            lib_action = Some(LibraryAction::Paste {
                                target_folder: root_folder.clone(),
                            });
                            ui.close_menu();
                        }
                    });
                });

                // Marquee overlay + selection commit.
                if let Some((a, b)) = marquee {
                    let rect = Rect::from_two_pos(a, b);
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        Color32::from_rgba_unmultiplied(80, 140, 220, 40),
                    );
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        Stroke::new(1.0_f32, Color32::from_rgb(100, 160, 240)),
                        egui::StrokeKind::Outside,
                    );
                    if ui.input(|i| i.pointer.any_released()) {
                        for (item, r) in &item_rects {
                            if r.intersects(rect) {
                                lib_sel.insert(item.clone());
                            }
                        }
                        marquee = None;
                    }
                }

                let _ = scroll_out;
                let _ = Frame::default(); // keep Frame import used if drop zones unused
                self.library_selection = lib_sel;
                self.library_marquee = marquee;

                if let Some(path) = open_pack {
                    if let Some(name) = switch_to.clone() {
                        self.open_named(path, &name);
                    } else {
                        self.open_path(path);
                    }
                } else if let Some(name) = switch_to {
                    self.switch_stamp(&name);
                }
                if let Some(action) = lib_action {
                    self.apply_library_action(action);
                }
            });



        if self.show_palette {
            egui::SidePanel::left("palette").resizable(true).default_width(220.0).show(ctx, |ui| {
                ui.heading("Suche");
                ui.label(
                    RichText::new("Fallback · Aus: Ansicht → Gebäudesuche")
                        .small()
                        .weak(),
                );
                ui.text_edit_singleline(&mut self.palette_filter);
                let rows: Vec<(u32, String, String)> = self
                    .catalog
                    .palette(&self.palette_filter, self.region_filter)
                    .into_iter()
                    .take(400)
                    .map(|def| {
                        let label_name = def
                            .short_name
                            .clone()
                            .unwrap_or_else(|| stamp_editor::catalog::short_label(&def.name));
                        let sz = def.size_for_variation(None);
                        let row = format!(
                            "{}  [{}×{}]",
                            label_name, sz[0] as i32, sz[1] as i32
                        );
                        (def.guid, label_name, row)
                    })
                    .collect();
                ui.label(
                    RichText::new(format!(
                        "{} Treffer · auch RMB = GUID tauschen",
                        rows.len()
                    ))
                    .small(),
                );
                let mut pick: Option<(u32, String)> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (guid, label_name, row) in &rows {
                        let selected = self.palette_selected_guid == Some(*guid);
                        if ui.selectable_label(selected, row).clicked() {
                            pick = Some((*guid, label_name.clone()));
                        }
                    }
                });
                if let Some((guid, label)) = pick {
                    self.pick_place_guid(guid, &label);
                }
            });
        }

        if self.show_analyse {
        egui::SidePanel::right("stats").resizable(true).default_width(280.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Analyse");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        self.show_analyse = false;
                    }
                });
            });
            if let Some(open) = &self.open {
                let placed: Vec<_> = open.stamp.all_draw_buildings().cloned().collect();
                let stats = layout_stats_region(
                    &placed,
                    &self.catalog,
                    Some(&open.stamp.document),
                    open.region_guid,
                );
                ui.group(|ui| {
                    ui.label(RichText::new("Layout-Effizienz").strong());
                    ui.label(format!(
                        "Gebäude: {} (+{} Feld-Polygone / {} Zellen)",
                        open.stamp.buildings.len(),
                        open.stamp.polygon_modules.len(),
                        open.stamp.polygon_cells.len()
                    ));
                    ui.label(format!("Unique GUIDs: {}", stats.unique_guids));
                    ui.label(format!("Belegte Zellen: {}", stats.occupied_cells));
                    ui.label(format!("BBox-Zellen: {}", stats.bbox_cells));
                    ui.label(format!("Leere Zellen in BBox: {}", stats.empty_cells_in_bbox));
                    ui.label(format!("Fill-Ratio: {:.1}%", stats.fill_ratio * 100.0));
                    ui.label(format!(
                        "Wege: {} (Dreck {}, Stein {})",
                        stats.road_cells, stats.dirt_cells, stats.stone_cells
                    ));
                });

                let areas = analyze_area_efficiency_with_fields(
                    &open.stamp.buildings,
                    &self.catalog,
                    Some(&open.stamp.document),
                    open.region_guid,
                    &open.stamp.polygon_cells,
                );
                let chain = analyze_chain_region(
                    &open.stamp.buildings,
                    &self.catalog,
                    Some(&open.stamp.document),
                    open.region_guid,
                    &open.stamp.polygon_cells,
                );
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label(RichText::new("Produktionskette").strong());
                    ui.label(format!("Produzenten mit I/O: {}", chain.producers));
                    if chain.all_balanced {
                        ui.colored_label(
                            Color32::from_rgb(80, 180, 80),
                            "Kette ausgeglichen (kein Input-Defizit)",
                        );
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(220, 120, 60),
                            "Ungleichgewicht: Inputs fehlen",
                        );
                    }
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        egui::Grid::new("chain_grid").striped(true).show(ui, |ui| {
                            ui.label("Produkt");
                            ui.label("Rein/min");
                            ui.label("Raus/min");
                            ui.label("Netto");
                            ui.end_row();
                            for f in &chain.flows {
                                let color = if f.consumed_per_min > 0.0 && f.net() < -0.001 {
                                    Color32::from_rgb(220, 90, 90)
                                } else if f.net() > 0.001 {
                                    Color32::from_rgb(90, 180, 90)
                                } else {
                                    Color32::GRAY
                                };
                                ui.colored_label(color, &f.name);
                                ui.label(format!("{:.2}", f.produced_per_min));
                                ui.label(format!("{:.2}", f.consumed_per_min));
                                ui.colored_label(color, format!("{:.2}", f.net()));
                                ui.end_row();
                            }
                        });
                    });
                });

                if !areas.is_empty() {
                    ui.add_space(8.0);
                    ui.group(|ui| {
                        ui.label(RichText::new("Flächeneffizienz (Wald/Sumpf/Ufer)").strong());
                        ui.label(
                            RichText::new(
                                "Frei = Radius − Blocker; Überlappung: exklusiv + Bedarfs-Priorität (wie Anno 1800)",
                            )
                            .small()
                            .weak(),
                        );
                        egui::Grid::new("area_eff_grid")
                            .num_columns(4)
                            .spacing([8.0, 2.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new("Gebäude").strong());
                                ui.label(RichText::new("Frei/Need").strong());
                                ui.label(RichText::new("Blocker").strong());
                                ui.label(RichText::new("Eff.").strong());
                                ui.end_row();
                                for a in &areas {
                                    let color = if a.efficiency >= 0.999 {
                                        Color32::from_rgb(90, 180, 90)
                                    } else if a.efficiency >= 0.7 {
                                        Color32::from_rgb(220, 160, 60)
                                    } else {
                                        Color32::from_rgb(220, 100, 80)
                                    };
                                    ui.label(&a.label);
                                    ui.label(format!("{:.0}/{:.0}", a.have, a.need));
                                    ui.label(
                                        RichText::new(if a.contested_tiles > 0.0 {
                                            format!(
                                                "G{:.0} W{:.0} F{:.0} · Konk.{:.0}",
                                                a.blocked_buildings,
                                                a.blocked_roads,
                                                a.blocked_fields,
                                                a.contested_tiles
                                            )
                                        } else {
                                            format!(
                                                "G{:.0} W{:.0} F{:.0}",
                                                a.blocked_buildings,
                                                a.blocked_roads,
                                                a.blocked_fields
                                            )
                                        })
                                        .small(),
                                    );
                                    ui.colored_label(
                                        color,
                                        format!("{:.0}%", a.efficiency * 100.0),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
                }

                if let Some(sel) = self.selected_building {
                    if let Some(b) = open.stamp.buildings.get(sel) {
                        let guid = b.guid;
                        let bx = b.x;
                        let by = b.y;
                        let dir = b.direction;
                        let variation = b.variation;
                        let owner_id = b.complex_owner_id;
                        let group_n =
                            complex_group_indices(&open.stamp.buildings, sel).len();
                        let label = self.catalog.label(guid);
                        let name = self.catalog.name(guid).to_string();
                        let (w, h) = self.building_size(guid, variation);
                        let cov_r = self.coverage.radius;
                        let cov_s = self.coverage.street_budget;
                        let cov_n = self.coverage.affected.len();
                        let cov_mode = self.coverage.mode;
                        let cov_hits = self.coverage.hits.clone();
                        let def = self.catalog.building(guid);
                        let influence = def.and_then(|d| d.influence_radius);
                        let needed_area = def.and_then(|d| d.needed_area);
                        let sel_area = areas.iter().find(|a| a.building_index == sel).cloned();
                        let street_d = def.and_then(|d| d.street_distance);
                        let radius_d = def.and_then(|d| d.radius_distance);
                        let effect_scope = def.and_then(|d| d.effect_scope.clone());
                        let service_kind = def.and_then(|d| d.service_kind.clone());
                        let effect_bonuses = def
                            .map(|d| d.effect_bonuses.clone())
                            .unwrap_or_default();
                        // Radius / productivity always relative to complex main.
                        let main_idx = complex_main_index(
                            &open.stamp.buildings,
                            &self.catalog,
                            sel,
                        )
                        .unwrap_or(sel);
                        let main_guid = open
                            .stamp
                            .buildings
                            .get(main_idx)
                            .map(|mb| mb.guid)
                            .unwrap_or(guid);
                        let main_def = self.catalog.building(main_guid);
                        let need = needed_modules(main_def);
                        let have = count_progress_for_stamp_area(
                            &open.stamp.buildings,
                            &open.stamp.polygon_modules,
                            &open.stamp.polygon_cells,
                            &open.stamp.polygon_area,
                            main_idx,
                            main_def,
                        );
                        let place_r = module_radius(main_def);
                        let region = open.region_guid;
                        let mod_btns: Vec<(u32, String)> = main_def
                            .map(|d| {
                                suggested_modules(&self.catalog, d, region)
                                    .into_iter()
                                    .map(|m| {
                                        let l = m.short_name.clone().unwrap_or_else(|| {
                                            stamp_editor::catalog::short_label(&m.name)
                                        });
                                        (m.guid, l)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        ui.add_space(8.0);
                        ui.group(|ui| {
                            ui.label(RichText::new("Auswahl / Coverage").strong());
                            ui.label(label);
                            ui.label(RichText::new(name).small().weak());
                            ui.label(format!("GUID {guid}  Pos ({bx:.1}, {by:.1})  Dir {dir}"));
                            ui.label(format!("Footprint {w:.0}×{h:.0}"));
                            if let Some(oid) = owner_id.filter(|o| *o != 0) {
                                let main_lbl = complex_main_index(
                                    &open.stamp.buildings,
                                    &self.catalog,
                                    sel,
                                )
                                .and_then(|mi| open.stamp.buildings.get(mi))
                                .map(|mb| self.catalog.label(mb.guid))
                                .unwrap_or_else(|| "?".into());
                                ui.label(format!(
                                    "Komplex Owner {oid} · {group_n} Gebäude (Haupt: {main_lbl})"
                                ));
                                ui.label(
                                    RichText::new("Rot = Hauptgebäude · Gelb = Untergebäude")
                                        .small()
                                        .weak(),
                                );
                            } else {
                                ui.label(RichText::new("Kein ComplexOwnerID — Einzelgebäude").weak());
                            }
                            match cov_mode {
                                CoverageMode::Outgoing => {
                                    ui.label(format!(
                                        "Abdeckung → {cov_n} Gebäude · Straße {cov_s:.0} · Radius {cov_r:.0}"
                                    ));
                                }
                                CoverageMode::Incoming => {
                                    ui.label(format!("Erreicht von {cov_n} Quelle(n)"));
                                }
                                CoverageMode::None => {
                                    ui.label(RichText::new("Keine Service-/Bonus-Reichweite").weak());
                                }
                            }
                            if let Some(scope) = &effect_scope {
                                ui.label(format!("EffectScope: {scope}"));
                            }
                            if let Some(k) = &service_kind {
                                ui.label(format!("Art: {k}"));
                            }
                            if let Some(s) = street_d {
                                ui.label(format!("StreetDistance: {s:.0}"));
                            }
                            if let Some(r) = radius_d {
                                ui.label(format!("RadiusDistance: {r:.0}"));
                            }
                            if let Some(r) = influence {
                                ui.label(format!("InfluenceRadius: {r}"));
                            }
                            if let Some(a) = needed_area {
                                ui.label(format!("NeededArea: {a}"));
                            }
                            if let Some(a) = &sel_area {
                                ui.separator();
                                ui.label(RichText::new("Ressourcenfläche").strong());
                                let color = if a.efficiency >= 0.999 {
                                    Color32::from_rgb(90, 180, 90)
                                } else if a.efficiency >= 0.7 {
                                    Color32::from_rgb(220, 160, 60)
                                } else {
                                    Color32::from_rgb(220, 100, 80)
                                };
                                ui.colored_label(
                                    color,
                                    format!(
                                        "Effektiv {:.0} / Need {:.0} ({:.0}%)",
                                        a.have,
                                        a.need,
                                        a.efficiency * 100.0
                                    ),
                                );
                                ui.label(format!(
                                    "Klasse „{}“ · exklusiv {:.0} · umkämpft {:.0} (gewonnen {:.0})",
                                    a.competition_group,
                                    a.have_exclusive,
                                    a.contested_tiles,
                                    a.have_contested_won
                                ));
                                ui.label(format!(
                                    "Radius {:.0}: {:.0} Kacheln · −Gebäude {:.0} · −Wege {:.0} · −Felder {:.0}",
                                    a.radius,
                                    a.raw_in_radius,
                                    a.blocked_buildings,
                                    a.blocked_roads,
                                    a.blocked_fields
                                ));
                            }
                            if !effect_bonuses.is_empty() {
                                ui.separator();
                                ui.label(RichText::new("Boni (Asset)").strong());
                                for b in &effect_bonuses {
                                    ui.label(format!("  {} {:+}", b.attr, b.value));
                                }
                            }
                            if matches!(cov_mode, CoverageMode::Incoming) && !cov_hits.is_empty()
                            {
                                ui.separator();
                                ui.label(RichText::new("Wirkende Quellen").strong());
                                for h in &cov_hits {
                                    let via = match (h.via_street, h.via_radius) {
                                        (true, true) => "Straße+Radius",
                                        (true, false) => "Straße",
                                        (false, true) => "Radius",
                                        _ => "?",
                                    };
                                    ui.label(format!("• {} ({via})", h.source_name));
                                    for b in &h.bonuses {
                                        ui.label(
                                            RichText::new(format!("    {} {:+}", b.attr, b.value))
                                                .small(),
                                        );
                                    }
                                }
                            }
                            if need > 0 {
                                ui.separator();
                                ui.label(RichText::new("Module / Untergebäude").strong());
                                let color = if have as u32 >= need {
                                    Color32::from_rgb(90, 180, 90)
                                } else {
                                    Color32::from_rgb(220, 160, 60)
                                };
                                ui.colored_label(color, format!("Produktivität {have}/{need}"));
                                ui.label(format!(
                                    "Platzierungsradius (Haupt) ≈ {place_r:.0} Tiles"
                                ));
                                for (mg, ml) in &mod_btns {
                                    if ui.button(format!("+ {ml}")).clicked() {
                                        self.placing_guid = Some(*mg);
                                        self.module_parent_index = Some(main_idx);
                                        self.palette_selected_guid = Some(*mg);
                                        self.hand_from_clipboard = false;
                                        self.status = format!(
                                            "Modul „{ml}“ — im Radius des Mains platzieren"
                                        );
                                    }
                                }
                            }
                        });
                    }
                }
            } else {
                ui.label("Kein Stamp geladen.");
            }
        });
        } // show_analyse

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.radial.is_some() {
                self.dismiss_radial();
            } else {
                self.clear_hand();
            }
        }

        // Bare letter keys stay out of text fields (Ctrl-chords already handled at frame start).
        let typing = ctx.wants_keyboard_input();
        let mods = ctx.input(|i| i.modifiers);

        let menu_delete = self.request_delete;
        self.request_delete = false;
        if !typing {
            // Delete (optional: whole ComplexOwnerID group when „Gruppe verschieben“ / Alt)
            let alt_held = ctx.input(|i| i.modifiers.alt);
            let group_ops = self.move_group || alt_held;
            let key_delete = ctx.input(|i| {
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
            });
            if key_delete || menu_delete {
                self.delete_selection(group_ops);
            }
            // Land/Marsh footprint (V): while placing, or on selected building with size_marsh.
            if !mods.ctrl && !mods.command && ctx.input(|i| i.key_pressed(egui::Key::V)) {
                if self.placing_guid.is_some() {
                    self.toggle_place_variation();
                } else if let Some(idx) = self.selected_building {
                    self.toggle_selected_variation(idx);
                }
            }
            // Rotate selected building (R). Shift+R = reverse.
            if !mods.ctrl
                && !mods.command
                && ctx.input(|i| i.key_pressed(egui::Key::R))
                && self.selected_building.is_some()
            {
                if let Some(idx) = self.selected_building {
                    let cur = self
                        .open
                        .as_ref()
                        .and_then(|o| o.stamp.buildings.get(idx))
                        .map(|b| b.direction);
                    if let Some(cur) = cur {
                        let next = if mods.shift {
                            let d = cur.rem_euclid(8);
                            if self.allow_45_deg {
                                (d + 7).rem_euclid(8)
                            } else if d % 2 == 1 {
                                (d + 7).rem_euclid(8)
                            } else {
                                (d + 6).rem_euclid(8)
                            }
                        } else {
                            self.next_direction(cur)
                        };
                        let allow_45 = self.allow_45_deg;
                        self.push_history();
                        // Keep top-left tile stable so 4×5 → 5×4 doesn't jump by a full cell.
                        let (old_w0, old_h0) = self
                            .open
                            .as_ref()
                            .and_then(|o| o.stamp.buildings.get(idx))
                            .map(|b| self.building_size(b.guid, b.variation))
                            .unwrap_or((1.0, 1.0));
                        let (ox, oy) = self
                            .open
                            .as_ref()
                            .and_then(|o| o.stamp.buildings.get(idx))
                            .map(|b| (b.x, b.y))
                            .unwrap_or((0.0, 0.0));
                        let (ow, oh) = oriented_size(old_w0, old_h0, cur);
                        let (tx, ty) = footprint_top_left(ox, oy, ow, oh, self.stamp_drag());
                        let (nw, nh) = oriented_size(old_w0, old_h0, next);
                        let (nx, ny) = place_center_from_top_left(
                            tx,
                            ty,
                            nw.round().max(1.0) as i32,
                            nh.round().max(1.0) as i32,
                        );
                        if let Some(open) = self.open.as_mut() {
                            match open.stamp.set_building_direction(idx, next) {
                                Ok(()) => {
                                    let _ = open.stamp.set_building_pos(idx, nx, ny);
                                    open.dirty = true;
                                    self.schedule_autosave();
                                    self.status = format!(
                                        "Gedreht → Dir {next} · {nw:.0}×{nh:.0} ({})",
                                        if allow_45 { "45°" } else { "90°" }
                                    );
                                    self.refresh_coverage();
                                }
                                Err(e) => self.status = format!("Drehen fehlgeschlagen: {e}"),
                            }
                        }
                    }
                }
            }
            // Focus / center view
            if !mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::F)) {
                self.center_view();
                self.status = "Ansicht zentriert".into();
            }
        } else if menu_delete {
            let alt_held = ctx.input(|i| i.modifiers.alt);
            let group_ops = self.move_group || alt_held;
            self.delete_selection(group_ops);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let (response, painter) =
                ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
            let rect = response.rect;
            if let Some(pos) = response.hover_pos() {
                self.last_pointer_world = Some(self.screen_to_world(rect, pos));
            }
            painter.rect_filled(rect, 0.0, Color32::from_rgb(28, 30, 34));

            // Background tile grid (every 1×1 cell; stronger every 5).
            {
                let (x0, y0) = self.screen_to_world(rect, rect.min);
                let (x1, y1) = self.screen_to_world(rect, rect.max);
                let min_x = x0.min(x1).floor() as i32 - 1;
                let max_x = x0.max(x1).ceil() as i32 + 1;
                let min_y = y0.min(y1).floor() as i32 - 1;
                let max_y = y0.max(y1).ceil() as i32 + 1;
                let grid = Color32::from_rgb(40, 43, 48);
                let major = Color32::from_rgb(52, 56, 64);
                for x in min_x..=max_x {
                    let p0 = self.world_to_screen(rect, x as f32, min_y as f32);
                    let p1 = self.world_to_screen(rect, x as f32, max_y as f32);
                    let stroke = if x.rem_euclid(5) == 0 {
                        Stroke::new(1.0_f32, major)
                    } else {
                        Stroke::new(1.0_f32, grid)
                    };
                    painter.line_segment([p0, p1], stroke);
                }
                for y in min_y..=max_y {
                    let p0 = self.world_to_screen(rect, min_x as f32, y as f32);
                    let p1 = self.world_to_screen(rect, max_x as f32, y as f32);
                    let stroke = if y.rem_euclid(5) == 0 {
                        Stroke::new(1.0_f32, major)
                    } else {
                        Stroke::new(1.0_f32, grid)
                    };
                    painter.line_segment([p0, p1], stroke);
                }
            }

            let origin = self.world_to_screen(rect, 0.0, 0.0);
            painter.line_segment(
                [Pos2::new(rect.left(), origin.y), Pos2::new(rect.right(), origin.y)],
                Stroke::new(1.0_f32, Color32::from_gray(70)),
            );
            painter.line_segment(
                [Pos2::new(origin.x, rect.top()), Pos2::new(origin.x, rect.bottom())],
                Stroke::new(1.0_f32, Color32::from_gray(70)),
            );

            if response.dragged_by(egui::PointerButton::Middle)
                || (response.dragged_by(egui::PointerButton::Secondary)
                    && self.dragging_building.is_none())
            {
                self.pan += response.drag_delta();
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    self.cell_px = (self.cell_px + scroll * 0.05).clamp(8.0, 48.0);
                }
            }

            // Roads — full cell fill so adjacent tiles look continuous (no 0.78 gaps).
            for (cell, kind) in &self.coverage.roads {
                let r = self.cell_screen_rect(rect, cell.x, cell.y);
                let color = match kind {
                    RoadKind::Dirt => Color32::from_rgb(150, 118, 72),
                    RoadKind::Stone => Color32::from_rgb(130, 130, 140),
                };
                painter.rect_filled(r, 0.0, color);
                if self.selected_roads.contains(&(cell.x, cell.y)) {
                    painter.rect_filled(
                        r,
                        0.0,
                        Color32::from_rgba_unmultiplied(80, 160, 255, 110),
                    );
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(1.25_f32, Color32::from_rgb(140, 200, 255)),
                        egui::StrokeKind::Outside,
                    );
                }
            }

            // Street-reach tint on top of roads (not a building-tile flood).
            for cell in &self.coverage.reachable_roads {
                let r = self.cell_screen_rect(rect, cell.x, cell.y);
                painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(70, 160, 255, 70));
            }

            // Live preview for active drag stroke.
            if let Some(stroke) = self.drag_stroke {
                match stroke.kind {
                    StrokeKind::Road { paint_on } => {
                        let path = orthogonal_road_path(
                            stroke.start.0,
                            stroke.start.1,
                            stroke.cursor.0,
                            stroke.cursor.1,
                        );
                        let preview = if paint_on {
                            Color32::from_rgba_unmultiplied(200, 170, 90, 150)
                        } else {
                            Color32::from_rgba_unmultiplied(220, 80, 80, 140)
                        };
                        for (tx, ty) in path {
                            let r = self.cell_screen_rect(rect, tx, ty);
                            painter.rect_filled(r, 0.0, preview);
                        }
                    }
                    StrokeKind::DemolishRect => {
                        let (min_x, min_y, max_x, max_y) =
                            rect_bounds(stroke.start, stroke.cursor);
                        for (tx, ty) in rect_cells(min_x, min_y, max_x, max_y) {
                            let r = self.cell_screen_rect(rect, tx, ty);
                            painter.rect_filled(
                                r,
                                0.0,
                                Color32::from_rgba_unmultiplied(220, 60, 50, 90),
                            );
                        }
                        let outline = Rect::from_min_max(
                            self.cell_screen_rect(rect, min_x, min_y).min,
                            self.cell_screen_rect(rect, max_x, max_y).max,
                        );
                        painter.rect_stroke(
                            outline,
                            0.0,
                            Stroke::new(2.0_f32, Color32::from_rgb(240, 90, 70)),
                            egui::StrokeKind::Outside,
                        );
                    }
                    StrokeKind::SelectRect => {
                        let (min_x, min_y, max_x, max_y) =
                            rect_bounds(stroke.start, stroke.cursor);
                        for (tx, ty) in rect_cells(min_x, min_y, max_x, max_y) {
                            let r = self.cell_screen_rect(rect, tx, ty);
                            painter.rect_filled(
                                r,
                                0.0,
                                Color32::from_rgba_unmultiplied(70, 140, 255, 55),
                            );
                        }
                        let outline = Rect::from_min_max(
                            self.cell_screen_rect(rect, min_x, min_y).min,
                            self.cell_screen_rect(rect, max_x, max_y).max,
                        );
                        painter.rect_stroke(
                            outline,
                            0.0,
                            Stroke::new(1.75_f32, Color32::from_rgb(120, 180, 255)),
                            egui::StrokeKind::Outside,
                        );
                    }
                    StrokeKind::PlaceRect => {
                        if let Some(guid) = self.placing_guid {
                            let variation = self.place_variation;
                            let from_clip = self.hand_from_clipboard;
                            let (bw, bh) = if from_clip {
                                self.clipboard
                                    .iter()
                                    .find(|c| c.is_primary)
                                    .map(|p| self.clipboard_oriented_size(p))
                                    .unwrap_or_else(|| self.building_size(guid, variation))
                            } else {
                                self.building_size(guid, variation)
                            };
                            let bw_i = bw.round().max(1.0) as i32;
                            let bh_i = bh.round().max(1.0) as i32;
                            let (min_x, min_y, max_x, max_y) =
                                rect_bounds(stroke.start, stroke.cursor);
                            let outline = Rect::from_min_max(
                                self.cell_screen_rect(rect, min_x, min_y).min,
                                self.cell_screen_rect(rect, max_x, max_y).max,
                            );
                            painter.rect_stroke(
                                outline,
                                0.0,
                                Stroke::new(
                                    1.5_f32,
                                    Color32::from_rgba_unmultiplied(140, 210, 255, 200),
                                ),
                                egui::StrokeKind::Outside,
                            );
                            let from_clipboard = self.hand_from_clipboard;
                            let (step_w, step_h) = if from_clipboard {
                                self.clipboard_group_span()
                            } else {
                                (bw_i, bh_i)
                            };
                            let mut centers = Vec::new();
                            let mut gy = min_y;
                            while gy + step_h - 1 <= max_y {
                                let mut gx = min_x;
                                while gx + step_w - 1 <= max_x {
                                    centers.push(place_center_from_top_left(
                                        gx, gy, bw_i, bh_i,
                                    ));
                                    gx += step_w;
                                }
                                gy += step_h;
                            }
                            if centers.is_empty() {
                                centers.push(place_center_from_top_left(
                                    stroke.start.0,
                                    stroke.start.1,
                                    bw_i,
                                    bh_i,
                                ));
                            }
                            let center_count = centers.len();
                            let region = self
                                .open
                                .as_ref()
                                .map(|o| o.region_guid)
                                .unwrap_or(0);
                            let drag = self.stamp_drag();
                            let fill =
                                Color32::from_rgba_unmultiplied(90, 180, 255, 80);
                            let road_fill =
                                Color32::from_rgba_unmultiplied(200, 170, 90, 110);
                            for (sx, sy) in centers {
                                let parts: Vec<(u32, f32, f32, i32, Option<u32>)> =
                                    if from_clipboard {
                                        self.clipboard_building_centers(sx, sy)
                                            .into_iter()
                                            .map(|(c, cx, cy)| {
                                                (c.guid, cx, cy, c.direction, c.variation)
                                            })
                                            .collect()
                                    } else {
                                        vec![(guid, sx, sy, 0, variation)]
                                    };
                                for (g_guid, gx, gy, g_dir, g_var) in parts {
                                    let ghost = Building {
                                        guid: g_guid,
                                        x: gx,
                                        y: gy,
                                        direction: g_dir,
                                        complex_owner_id: None,
                                        variation: g_var,
                                    };
                                    let def = self.catalog.building(g_guid);
                                    for c in
                                        footprint_cells_for_region(&ghost, def, region, drag)
                                    {
                                        let r = self.cell_screen_rect(rect, c.x, c.y);
                                        painter.rect_filled(r, 0.0, fill);
                                    }
                                }
                                if from_clipboard {
                                    for (rx, ry, _) in self.clipboard_road_cells(sx, sy) {
                                        painter.rect_filled(
                                            self.cell_screen_rect(rect, rx, ry),
                                            0.0,
                                            road_fill,
                                        );
                                    }
                                }
                            }
                            painter.text(
                                outline.min + Vec2::new(6.0, 4.0),
                                egui::Align2::LEFT_TOP,
                                format!("{center_count}×"),
                                egui::FontId::proportional(13.0),
                                Color32::from_rgb(200, 230, 255),
                            );
                        }
                    }
                }
            }

            if let Some(open) = self.open.clone() {
                let affected_set: std::collections::HashSet<usize> =
                    self.coverage.affected.iter().copied().collect();
                let hover_idx = response
                    .hover_pos()
                    .and_then(|p| self.building_at_pointer(rect, &open.stamp.buildings, p));
                self.hover_building = hover_idx;
                let focus_idx = self.selected_building.or(hover_idx);
                let focus_group: HashSet<usize> = focus_idx
                    .map(|i| complex_group_indices(&open.stamp.buildings, i))
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let focus_main = focus_idx.and_then(|i| {
                    complex_main_index(&open.stamp.buildings, &self.catalog, i)
                });
                let focus_owner = focus_idx
                    .and_then(|i| open.stamp.buildings.get(i))
                    .and_then(|b| b.complex_owner_id)
                    .filter(|o| *o != 0);
                // Owners that already have SubTileGrid 1×1 cells — skip fat module footprints.
                let owners_with_cells: HashSet<u64> = open
                    .stamp
                    .polygon_cells
                    .iter()
                    .filter_map(|c| c.complex_owner_id.filter(|o| *o != 0))
                    .collect();
                let guids_with_cells: HashSet<u32> = open
                    .stamp
                    .polygon_cells
                    .iter()
                    .map(|c| c.guid)
                    .collect();

                // Owner → farm main GUID (for crop icons; Module Field uses generic farm_fields).
                let mut owner_main_guid: std::collections::HashMap<u64, u32> =
                    std::collections::HashMap::new();
                for (i, b) in open.stamp.buildings.iter().enumerate() {
                    let Some(owner) = b.complex_owner_id.filter(|o| *o != 0) else {
                        continue;
                    };
                    if complex_main_index(&open.stamp.buildings, &self.catalog, i) != Some(i) {
                        continue;
                    }
                    owner_main_guid.entry(owner).or_insert(b.guid);
                }

                // PlantFarm fields: each SubTileGrid bit → 1×1 tile with crop icon.
                for cell in &open.stamp.polygon_cells {
                    let r = self.cell_screen_rect(rect, cell.x, cell.y);
                    let linked =
                        focus_owner.is_some() && cell.complex_owner_id == focus_owner;
                    let fill = if linked {
                        Color32::from_rgba_unmultiplied(110, 150, 70, 90)
                    } else {
                        Color32::from_rgba_unmultiplied(75, 110, 60, 70)
                    };
                    painter.rect_filled(r, 0.0, fill);
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(
                            if linked { 1.25_f32 } else { 0.75_f32 },
                            if linked {
                                Color32::from_rgb(230, 200, 90)
                            } else {
                                Color32::from_rgba_unmultiplied(180, 200, 140, 120)
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    if self.cell_px >= 8.0 {
                        // Module Field GUIDs map to generic farm_fields.png — use the
                        // ComplexOwner farm main instead (olives→olives goods, oats→oats, …).
                        let icon_guid = cell
                            .complex_owner_id
                            .and_then(|o| owner_main_guid.get(&o).copied())
                            .unwrap_or(cell.guid);
                        let _ = self.paint_building_icon(ctx, &painter, r, icon_guid, 0.95);
                    }
                }

                // BuildingInfo first (selectable indices), then Polygons field module markers
                // (skipped when SubTileGrid cells already represent that field).
                let draw_buildings: Vec<(Option<usize>, &stamp_editor::stamp::Building)> = open
                    .stamp
                    .buildings
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (Some(i), b))
                    .chain(
                        open.stamp
                            .polygon_modules
                            .iter()
                            .map(|b| (None, b)),
                    )
                    .collect();
                for (idx, b) in draw_buildings {
                    let def = self.catalog.building(b.guid);
                    let is_poly_module = def.is_some_and(|d| {
                        is_module_building(d)
                            || d.category == "module"
                            || d.template == "ModulePolygonField"
                            || d.template == "ModuleField"
                            || d.template == "ModuleMarshField"
                    });
                    // Avoid 3×3 giant module icons when 1×1 cells already paint the field.
                    if is_poly_module {
                        let owner_covered = b
                            .complex_owner_id
                            .filter(|o| *o != 0)
                            .is_some_and(|o| owners_with_cells.contains(&o));
                        let guid_covered = guids_with_cells.contains(&b.guid);
                        if owner_covered || (idx.is_none() && guid_covered) {
                            continue;
                        }
                    }
                    let (w, h) = self.building_size(b.guid, b.variation);
                    let center = self.world_to_screen(rect, b.x, b.y);
                    let selected = idx.is_some_and(|i| {
                        self.selected_buildings.contains(&i) || self.selected_building == Some(i)
                    });
                    let in_group = idx.map(|i| focus_group.contains(&i)).unwrap_or_else(|| {
                        focus_owner.is_some() && b.complex_owner_id == focus_owner
                    });
                    let is_group_main = idx.is_some() && idx == focus_main;
                    let in_cov = idx.map(|i| affected_set.contains(&i)).unwrap_or(false);
                    let fill = if selected {
                        Color32::from_rgb(90, 150, 220)
                    } else if in_cov {
                        match self.coverage.mode {
                            CoverageMode::Outgoing => Color32::from_rgb(60, 110, 150),
                            CoverageMode::Incoming => Color32::from_rgb(120, 90, 150),
                            CoverageMode::None => Color32::from_rgb(80, 80, 90),
                        }
                    } else {
                        match def.map(|d| d.category.as_str()) {
                            Some("production") => Color32::from_rgb(70, 120, 80),
                            Some("warehouse") => Color32::from_rgb(90, 90, 140),
                            Some("module") => Color32::from_rgb(100, 140, 70),
                            Some("institution") => Color32::from_rgb(150, 90, 90),
                            Some("street") => Color32::from_rgb(110, 100, 80),
                            _ => Color32::from_rgb(80, 80, 90),
                        }
                    };
                    let (stroke_w, stroke_c) = if selected {
                        // Loud selection so copy/paste feedback is obvious.
                        (3.5_f32, Color32::from_rgb(80, 220, 255))
                    } else if is_group_main {
                        (3.0_f32, Color32::from_rgb(230, 90, 70))
                    } else if in_group {
                        (2.5_f32, Color32::from_rgb(240, 200, 70))
                    } else {
                        (1.0_f32, Color32::WHITE)
                    };

                    let diagonal = b.direction.rem_euclid(2) == 1;
                    // Axis-aligned: draw exact footprint cell AABB (same math as coverage).
                    let drag = self.stamp_drag();
                    let cells = footprint_cells_for_region(b, def, open.region_guid, drag);
                    let footprint_rect = if !diagonal && !cells.is_empty() {
                        let min_x = cells.iter().map(|c| c.x).min().unwrap();
                        let max_x = cells.iter().map(|c| c.x).max().unwrap();
                        let min_y = cells.iter().map(|c| c.y).min().unwrap();
                        let max_y = cells.iter().map(|c| c.y).max().unwrap();
                        Some(Rect::from_min_max(
                            self.cell_screen_rect(rect, min_x, min_y).min,
                            self.cell_screen_rect(rect, max_x, max_y).max,
                        ))
                    } else {
                        None
                    };
                    let mut drew_icon = false;
                    if diagonal {
                        // 45° footprint: draw as diamond (rotated rect approx).
                        let hw = w * self.cell_px * 0.5;
                        let hh = h * self.cell_px * 0.5;
                        let cos = std::f32::consts::FRAC_1_SQRT_2;
                        let sin = std::f32::consts::FRAC_1_SQRT_2;
                        let corners = [
                            (-hw, -hh),
                            (hw, -hh),
                            (hw, hh),
                            (-hw, hh),
                        ]
                        .map(|(lx, ly)| {
                            Pos2::new(
                                center.x + lx * cos - ly * sin,
                                center.y + lx * sin + ly * cos,
                            )
                        });
                        painter.add(egui::Shape::convex_polygon(
                            corners.to_vec(),
                            fill,
                            Stroke::new(stroke_w, stroke_c),
                        ));
                        let icon_box = Rect::from_center_size(
                            center,
                            Vec2::splat((w.min(h) * self.cell_px).max(8.0)),
                        );
                        drew_icon =
                            self.paint_building_icon(ctx, &painter, icon_box, b.guid, 1.0);
                    } else if let Some(r) = footprint_rect {
                        // Sharp corners; Inside stroke keeps the border on-grid
                        // (Outside bled into neighboring cells and looked like gaps).
                        painter.rect_filled(r, 0.0, fill);
                        painter.rect_stroke(
                            r,
                            0.0,
                            Stroke::new(stroke_w, stroke_c),
                            egui::StrokeKind::Inside,
                        );
                        drew_icon = self.paint_building_icon(ctx, &painter, r, b.guid, 1.0);
                    }

                    // Text only when no icon (or extremely zoomed-out fallback).
                    if !drew_icon && self.cell_px >= 12.0 {
                        let label = self.catalog.label(b.guid);
                        let (ew, eh) = oriented_size(w, h, b.direction);
                        let size_tag = if b.direction.rem_euclid(8) == 0 {
                            format!("{label} {}×{}", ew as i32, eh as i32)
                        } else {
                            format!(
                                "{label} {}×{} · Dir {}",
                                ew as i32,
                                eh as i32,
                                b.direction.rem_euclid(8)
                            )
                        };
                        let font_size = if self.cell_px >= 22.0 { 11.0 } else { 9.0 };
                        let max_w = (ew * self.cell_px * 0.92).max(20.0);
                        let galley = painter.layout(
                            size_tag,
                            egui::FontId::proportional(font_size),
                            Color32::WHITE,
                            max_w,
                        );
                        let text_pos = Pos2::new(
                            center.x - galley.size().x * 0.5,
                            center.y - galley.size().y * 0.5,
                        );
                        painter.galley(text_pos, galley, Color32::WHITE);
                    }
                }

                // Have/need only on the focused complex main (hover or selection).
                if self.cell_px >= 10.0 {
                    if let Some(main_i) = focus_main {
                        let b = &open.stamp.buildings[main_i];
                        let def = self.catalog.building(b.guid);
                        let need = needed_modules(def);
                        let show = need > 0
                            && !def.is_some_and(|d| {
                                is_module_building(d) || d.category == "module"
                            });
                        if show {
                            let have = count_progress_for_stamp_area(
                            &open.stamp.buildings,
                            &open.stamp.polygon_modules,
                            &open.stamp.polygon_cells,
                            &open.stamp.polygon_area,
                                main_i,
                                def,
                            );
                            let (_, bh) = self.building_size(b.guid, b.variation);
                            let anchor = self.world_to_screen(
                                rect,
                                b.x,
                                b.y - bh * 0.5 - 0.35,
                            );
                            let ok = have as u32 >= need;
                            let color = if ok {
                                Color32::from_rgb(120, 220, 130)
                            } else {
                                Color32::from_rgb(240, 190, 90)
                            };
                            let tag = format!("{have}/{need}");
                            let galley = painter.layout_no_wrap(
                                tag,
                                egui::FontId::proportional(13.0),
                                color,
                            );
                            let pad = Vec2::new(5.0, 2.0);
                            let bg = Rect::from_min_size(
                                Pos2::new(
                                    anchor.x - galley.size().x * 0.5 - pad.x,
                                    anchor.y - galley.size().y * 0.5 - pad.y,
                                ),
                                galley.size() + pad * 2.0,
                            );
                            painter.rect_filled(
                                bg,
                                3.0,
                                Color32::from_rgba_unmultiplied(20, 24, 30, 200),
                            );
                            painter.galley(
                                Pos2::new(
                                    anchor.x - galley.size().x * 0.5,
                                    anchor.y - galley.size().y * 0.5,
                                ),
                                galley,
                                color,
                            );
                        }
                    }

                    // Resource-area overlays: selection + same-class neighbors they influence.
                    let mut area_targets: Vec<usize> = self
                        .selected_buildings
                        .iter()
                        .copied()
                        .filter(|&i| open.stamp.buildings.get(i).is_some())
                        .collect();
                    if area_targets.is_empty() {
                        if let Some(fi) = focus_idx {
                            area_targets.push(fi);
                        }
                    } else if let Some(fi) = focus_idx {
                        if !area_targets.contains(&fi) {
                            area_targets.push(fi);
                        }
                    }
                    let selected_seed: HashSet<usize> = area_targets.iter().copied().collect();
                    let expand_influenced = !self.selected_buildings.is_empty()
                        || self.dragging_building.is_some()
                        || self.selected_building.is_some();
                    if !area_targets.is_empty() {
                        let blocked = blocker_map(
                            &open.stamp.buildings,
                            &self.catalog,
                            Some(&open.stamp.document),
                            open.region_guid,
                            &open.stamp.polygon_cells,
                        );
                        let area_hits = analyze_area_efficiency_with_fields(
                            &open.stamp.buildings,
                            &self.catalog,
                            Some(&open.stamp.document),
                            open.region_guid,
                            &open.stamp.polygon_cells,
                        );
                        // Seeds: selected/focused resource buildings with group + radius.
                        let mut seeds: Vec<(usize, f32, f32, f32, &'static str)> = Vec::new();
                        for &fi in &area_targets {
                            let b = &open.stamp.buildings[fi];
                            let Some(def) = self.catalog.building(b.guid) else {
                                continue;
                            };
                            if !is_resource_area_building(def) {
                                continue;
                            }
                            let Some(radius) = resource_radius(def) else {
                                continue;
                            };
                            seeds.push((fi, b.x, b.y, radius, competition_group(def)));
                        }
                        // Neighbors in the same class whose radius overlaps a seed.
                        if expand_influenced && !seeds.is_empty() {
                            for a in &area_hits {
                                if selected_seed.contains(&a.building_index) {
                                    continue;
                                }
                                let b = &open.stamp.buildings[a.building_index];
                                let Some(def) = self.catalog.building(b.guid) else {
                                    continue;
                                };
                                let Some(radius) = resource_radius(def) else {
                                    continue;
                                };
                                let group = competition_group(def);
                                let overlaps = seeds.iter().any(|&(_, sx, sy, sr, sg)| {
                                    sg == group
                                        && dist_tiles(sx, sy, b.x, b.y) <= sr + radius + 0.05
                                });
                                if overlaps {
                                    area_targets.push(a.building_index);
                                }
                            }
                        }
                        let mut contested_by_group: HashMap<
                            &str,
                            HashMap<stamp_editor::coverage::Cell, usize>,
                        > = HashMap::new();
                        let multi = area_targets.len() > 1;
                        for &fi in &area_targets {
                            let b = &open.stamp.buildings[fi];
                            let Some(def) = self.catalog.building(b.guid) else {
                                continue;
                            };
                            if !is_resource_area_building(def) {
                                continue;
                            }
                            let Some(need) = resource_need(def) else {
                                continue;
                            };
                            let Some(radius) = resource_radius(def) else {
                                continue;
                            };
                            let is_seed = selected_seed.contains(&fi);
                            let group = competition_group(def);
                            if is_seed {
                                if !contested_by_group.contains_key(group) {
                                    contested_by_group.insert(
                                        group,
                                        contested_free_cells(
                                            &open.stamp.buildings,
                                            &self.catalog,
                                            Some(&open.stamp.document),
                                            open.region_guid,
                                            &open.stamp.polygon_cells,
                                            group,
                                        ),
                                    );
                                }
                                let contested = contested_by_group.get(group);
                                // Full tile tint only for selection/focus seeds.
                                for (cell, reason) in radius_cells(b.x, b.y, radius, &blocked)
                                {
                                    let r = self.cell_screen_rect(rect, cell.x, cell.y);
                                    let fill = match reason {
                                        None if contested
                                            .is_some_and(|m| m.contains_key(&cell)) =>
                                        {
                                            Color32::from_rgba_unmultiplied(
                                                230, 170, 50, if multi { 50 } else { 95 },
                                            )
                                        }
                                        None => Color32::from_rgba_unmultiplied(
                                            70, 180, 90, if multi { 28 } else { 55 },
                                        ),
                                        Some(BlockReason::Road) => {
                                            Color32::from_rgba_unmultiplied(
                                                180, 120, 50, if multi { 40 } else { 90 },
                                            )
                                        }
                                        Some(BlockReason::Building) => {
                                            Color32::from_rgba_unmultiplied(
                                                200, 70, 70, if multi { 40 } else { 90 },
                                            )
                                        }
                                        Some(BlockReason::Field) => {
                                            Color32::from_rgba_unmultiplied(
                                                140, 160, 60, if multi { 35 } else { 80 },
                                            )
                                        }
                                    };
                                    painter.rect_filled(r, 0.0, fill);
                                }
                            }
                            let center = self.world_to_screen(rect, b.x, b.y);
                            let screen_r = radius * self.cell_px;
                            let (circle_w, circle_c) = if is_seed {
                                (
                                    if focus_idx == Some(fi) {
                                        1.75_f32
                                    } else {
                                        1.25_f32
                                    },
                                    Color32::from_rgba_unmultiplied(
                                        120,
                                        200,
                                        140,
                                        if focus_idx == Some(fi) { 200 } else { 150 },
                                    ),
                                )
                            } else {
                                // Influenced neighbor — amber ring, no tile wash.
                                (
                                    1.35_f32,
                                    Color32::from_rgba_unmultiplied(240, 180, 70, 170),
                                )
                            };
                            painter.circle_stroke(
                                center,
                                screen_r,
                                Stroke::new(circle_w, circle_c),
                            );
                            if let Some(a) =
                                area_hits.iter().find(|a| a.building_index == fi)
                            {
                                let (_, bh) = self.building_size(b.guid, b.variation);
                                let anchor = self.world_to_screen(
                                    rect,
                                    b.x,
                                    b.y - bh * 0.5 - 0.35,
                                );
                                // Badge color = fill ratio only (≥100% green), for seeds and peers.
                                let ok = a.have + 0.001 >= need;
                                let color = if ok {
                                    Color32::from_rgb(120, 220, 130)
                                } else {
                                    Color32::from_rgb(240, 190, 90)
                                };
                                let tag = format!("{:.0}/{:.0}", a.have, a.need);
                                let galley = painter.layout_no_wrap(
                                    tag,
                                    egui::FontId::proportional(if multi { 11.0 } else { 13.0 }),
                                    color,
                                );
                                let pad = Vec2::new(5.0, 2.0);
                                let bg = Rect::from_min_size(
                                    Pos2::new(
                                        anchor.x - galley.size().x * 0.5 - pad.x,
                                        anchor.y - galley.size().y * 0.5 - pad.y,
                                    ),
                                    galley.size() + pad * 2.0,
                                );
                                painter.rect_filled(
                                    bg,
                                    3.0,
                                    Color32::from_rgba_unmultiplied(20, 24, 30, 200),
                                );
                                painter.galley(
                                    Pos2::new(
                                        anchor.x - galley.size().x * 0.5,
                                        anchor.y - galley.size().y * 0.5,
                                    ),
                                    galley,
                                    color,
                                );
                            }
                        }
                    }
                }

                // Bonus chips on canvas (selection coverage).
                match self.coverage.mode {
                    CoverageMode::Outgoing => {
                        if let Some(hit) = self.coverage.hits.first() {
                            for &ai in &self.coverage.affected {
                                if let Some(tb) = open.stamp.buildings.get(ai) {
                                    let anchor =
                                        self.world_to_screen(rect, tb.x, tb.y - 0.9);
                                    let (r, g, bl) = kind_color(hit.service_kind.as_deref());
                                    let short: Vec<String> = hit
                                        .bonuses
                                        .iter()
                                        .take(2)
                                        .map(|b| Self::format_bonus_chip(&b.attr, b.value))
                                        .collect();
                                    self.paint_bonus_label(
                                        &painter,
                                        anchor,
                                        &short,
                                        Color32::from_rgb(r, g, bl),
                                    );
                                }
                            }
                        }
                    }
                    CoverageMode::Incoming => {
                        if let Some(sel_i) = self.selected_building {
                            if let Some(sel_b) = open.stamp.buildings.get(sel_i) {
                                let lines =
                                    Self::format_incoming_summary(&self.coverage.hits);
                                if !lines.is_empty() {
                                    let anchor =
                                        self.world_to_screen(rect, sel_b.x, sel_b.y - 1.4);
                                    self.paint_bonus_label(
                                        &painter,
                                        anchor,
                                        &lines,
                                        Color32::from_rgb(220, 230, 255),
                                    );
                                }
                            }
                        }
                    }
                    CoverageMode::None => {}
                }

                // Hover tooltip: name + live buffs (compute coverage for hovered building).
                if response.hovered() {
                    if let Some(pointer) = response.hover_pos() {
                        let mut tip: Option<(usize, String)> = None;
                        for (idx, b) in open.stamp.buildings.iter().enumerate() {
                            let r = self.building_hit_rect(
                                rect,
                                b.x,
                                b.y,
                                b.guid,
                                b.direction,
                                b.variation,
                            );
                            if r.contains(pointer) {
                                tip = Some((idx, self.catalog.name(b.guid).to_string()));
                                break;
                            }
                        }
                        if let Some((idx, name)) = tip {
                            let live = if self.selected_building == Some(idx) {
                                None
                            } else {
                                Some(compute_coverage_region(
                                    &open.stamp.buildings,
                                    &self.catalog,
                                    idx,
                                    Some(&open.stamp.document),
                                    open.region_guid,
                                ))
                            };
                            let cov = live.as_ref().unwrap_or(&self.coverage);
                            let mut extra: Vec<String> = Vec::new();
                            match cov.mode {
                                CoverageMode::Outgoing => {
                                    if let Some(hit) = cov.hits.first() {
                                        if idx == hit.source_index
                                            || cov.affected.contains(&idx)
                                        {
                                            for bns in hit.bonuses.iter().take(6) {
                                                extra.push(Self::format_bonus_chip(
                                                    &bns.attr, bns.value,
                                                ));
                                            }
                                            extra.push(format!(
                                                "→ {} Gebäude",
                                                cov.affected.len()
                                            ));
                                        }
                                    }
                                }
                                CoverageMode::Incoming => {
                                    if cov.hits.is_empty() {
                                        extra.push("Keine Buff-Quellen in Reichweite".into());
                                    } else {
                                        extra.extend(Self::format_incoming_summary(&cov.hits));
                                    }
                                }
                                CoverageMode::None => {
                                    let (bw, bh) = self.building_size(
                                        open.stamp.buildings[idx].guid,
                                        open.stamp.buildings[idx].variation,
                                    );
                                    extra.push(format!("{bw:.0}×{bh:.0}"));
                                }
                            }
                            egui::show_tooltip_at_pointer(
                                ctx,
                                egui::LayerId::new(
                                    egui::Order::Tooltip,
                                    egui::Id::new("stamp_building_tip"),
                                ),
                                egui::Id::new("stamp_building_tip"),
                                |ui| {
                                    ui.set_max_width(220.0);
                                    ui.label(
                                        RichText::new(&name)
                                            .size(13.0)
                                            .strong(),
                                    );
                                    for line in &extra {
                                        ui.label(
                                            RichText::new(line)
                                                .size(12.0)
                                                .color(Color32::from_rgb(210, 220, 235)),
                                        );
                                    }
                                },
                            );
                        }
                    }
                }

                // Ghost preview for building/group in hand — hide while rect-dragging.
                if let Some(guid) = self.placing_guid {
                    if self.drag_stroke.is_none() {
                    if let Some(pointer) = response.hover_pos() {
                        let (wx, wy) = self.screen_to_world(rect, pointer);
                        let from_clipboard = self.hand_from_clipboard;
                        let (bw, bh) = if from_clipboard {
                            self.clipboard
                                .iter()
                                .find(|c| c.is_primary)
                                .map(|p| self.clipboard_oriented_size(p))
                                .unwrap_or_else(|| {
                                    self.building_size(guid, self.place_variation)
                                })
                        } else {
                            self.building_size(guid, self.place_variation)
                        };
                        let (sx, sy) = self.snap_place_center(wx, wy, bw, bh);
                        let mut blocked = false;
                        if let Some(pi) = self.module_parent_index {
                            if let Some(parent) = open.stamp.buildings.get(pi) {
                                let r = module_radius(self.catalog.building(parent.guid));
                                if r > 0.0 && dist_tiles(parent.x, parent.y, sx, sy) > r {
                                    blocked = true;
                                }
                            }
                        }
                        let fill = if blocked {
                            Color32::from_rgba_unmultiplied(220, 60, 50, 90)
                        } else {
                            Color32::from_rgba_unmultiplied(90, 180, 255, 90)
                        };
                        let stroke_c = if blocked {
                            Color32::from_rgb(240, 80, 70)
                        } else {
                            Color32::from_rgb(140, 210, 255)
                        };
                        let road_fill = if blocked {
                            Color32::from_rgba_unmultiplied(220, 80, 70, 100)
                        } else {
                            Color32::from_rgba_unmultiplied(200, 170, 90, 120)
                        };

                        let ghost_parts: Vec<(u32, f32, f32, i32, Option<u32>)> =
                            if from_clipboard {
                                self.clipboard_building_centers(sx, sy)
                                    .into_iter()
                                    .map(|(c, cx, cy)| {
                                        (c.guid, cx, cy, c.direction, c.variation)
                                    })
                                    .collect()
                            } else {
                                vec![(guid, sx, sy, 0, self.place_variation)]
                            };

                        for (g_guid, gx, gy, g_dir, g_var) in &ghost_parts {
                            let ghost = Building {
                                guid: *g_guid,
                                x: *gx,
                                y: *gy,
                                direction: *g_dir,
                                complex_owner_id: None,
                                variation: *g_var,
                            };
                            let def = self.catalog.building(*g_guid);
                            let cells = footprint_cells_for_region(
                                &ghost,
                                def,
                                open.region_guid,
                                self.stamp_drag(),
                            );
                            let mut min_x = i32::MAX;
                            let mut max_x = i32::MIN;
                            let mut min_y = i32::MAX;
                            let mut max_y = i32::MIN;
                            for c in &cells {
                                min_x = min_x.min(c.x);
                                max_x = max_x.max(c.x);
                                min_y = min_y.min(c.y);
                                max_y = max_y.max(c.y);
                                painter.rect_filled(
                                    self.cell_screen_rect(rect, c.x, c.y),
                                    0.0,
                                    fill,
                                );
                            }
                            if min_x <= max_x {
                                let outline = Rect::from_min_max(
                                    self.cell_screen_rect(rect, min_x, min_y).min,
                                    self.cell_screen_rect(rect, max_x, max_y).max,
                                );
                                painter.rect_stroke(
                                    outline,
                                    0.0,
                                    Stroke::new(2.0_f32, stroke_c),
                                    egui::StrokeKind::Outside,
                                );
                                self.paint_building_icon(
                                    ctx, &painter, outline, *g_guid, 0.85,
                                );
                            }
                        }
                        if from_clipboard {
                            for (rx, ry, _) in self.clipboard_road_cells(sx, sy) {
                                painter.rect_filled(
                                    self.cell_screen_rect(rect, rx, ry),
                                    0.0,
                                    road_fill,
                                );
                            }
                        }
                    }
                    }
                }

                if let Some(pointer) = response.interact_pointer_pos() {
                    if self.radial.is_none()
                        && response.clicked_by(egui::PointerButton::Primary)
                    {
                        let mut hit = None;
                        for (idx, b) in open.stamp.buildings.iter().enumerate() {
                            let r = self.building_hit_rect(
                                rect,
                                b.x,
                                b.y,
                                b.guid,
                                b.direction,
                                b.variation,
                            );
                            if r.contains(pointer) {
                                hit = Some(idx);
                                break;
                            }
                        }
                        if self.suppress_stroke_click {
                            self.suppress_stroke_click = false;
                        } else if self.placing_guid.is_some() || self.road_tool_active() {
                            // Place/paint even if the click lands on an existing building.
                            if self.drag_stroke.is_none() {
                                let (wx, wy) = self.screen_to_world(rect, pointer);
                                let (tx, ty) = self.stroke_anchor_tile(wx, wy);
                                self.begin_drag_stroke(tx, ty);
                                self.commit_drag_stroke();
                            }
                        } else if hit.is_none() {
                            self.clear_canvas_selection();
                            self.refresh_coverage();
                        } else if self.road_erase_mode {
                            if let Some(i) = hit {
                                self.select_only(i);
                            }
                            self.drag_stroke = None;
                            self.delete_selection(false);
                        } else if let Some(i) = hit {
                            let ctrl = ui.input(|inp| inp.modifiers.ctrl || inp.modifiers.command);
                            if ctrl {
                                self.toggle_canvas_selection(i);
                            } else {
                                self.select_only(i);
                            }
                            self.drag_stroke = None;
                            if let Some(b) = open.stamp.buildings.get(i) {
                                self.status = format!(
                                    "Ausgewählt: {} — Ctrl+C / Button „Kopieren“",
                                    self.catalog.label(b.guid)
                                );
                            }
                            self.refresh_coverage();
                        }
                    }

                    if response.clicked_by(egui::PointerButton::Secondary) {
                        // Wheel already open → RMB anywhere = cancel, never reopen here.
                        if self.radial.is_some() {
                            self.dismiss_radial();
                        } else if self.hand_busy() {
                            // Tool in hand → RMB drops it (Abriss/Wege/Gebäude).
                            self.clear_hand();
                        } else if let Some(new_guid) = self.palette_selected_guid {
                            // Palette-GUID swap when a palette pick is active.
                            let mut hit = None;
                            for (idx, b) in open.stamp.buildings.iter().enumerate() {
                                let r = self.building_hit_rect(
                                    rect,
                                    b.x,
                                    b.y,
                                    b.guid,
                                    b.direction,
                                    b.variation,
                                );
                                if r.contains(pointer) {
                                    hit = Some(idx);
                                    break;
                                }
                            }
                            if let Some(idx) = hit {
                                self.push_history();
                                if let Some(open_mut) = self.open.as_mut() {
                                    if let Ok(()) = open_mut.stamp.set_building_guid(idx, new_guid)
                                    {
                                        open_mut.dirty = true;
                                        self.schedule_autosave();
                                        self.status = format!(
                                            "GUID getauscht → {} ({})",
                                            self.catalog.label(new_guid),
                                            new_guid
                                        );
                                        self.selected_building = Some(idx);
                                        self.refresh_coverage();
                                    }
                                }
                            } else {
                                self.open_radial(pointer);
                            }
                        } else {
                            // Empty hand → Anno-style quick-select wheel.
                            self.open_radial(pointer);
                        }
                    }
                }

                if self.radial.is_none()
                    && response.drag_started_by(egui::PointerButton::Primary)
                {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let mut on_building = false;
                        let tool_drag = self.placing_guid.is_some() || self.road_tool_active();
                        if !tool_drag {
                            for (idx, b) in open.stamp.buildings.iter().enumerate() {
                                let r = self.building_hit_rect(
                                    rect,
                                    b.x,
                                    b.y,
                                    b.guid,
                                    b.direction,
                                    b.variation,
                                );
                                if r.contains(pointer) {
                                    let center = self.world_to_screen(rect, b.x, b.y);
                                    if !self.selected_buildings.contains(&idx) {
                                        self.select_only(idx);
                                        self.clear_selection_drag();
                                    } else {
                                        self.selected_building = Some(idx);
                                    }
                                    self.dragging_building = Some(idx);
                                    self.drag_offset =
                                        Vec2::new(pointer.x - center.x, pointer.y - center.y);
                                    self.drag_stroke = None;
                                    self.history_drag_pushed = false;
                                    // Multi-select (buildings and/or roads) → move whole group.
                                    if self.selected_buildings.len() > 1
                                        || !self.selected_roads.is_empty()
                                    {
                                        self.begin_selection_drag(idx);
                                    } else {
                                        self.clear_selection_drag();
                                    }
                                    on_building = true;
                                    break;
                                }
                            }
                        }
                        // Tool strokes OR empty-hand marquee selection.
                        if !on_building && (tool_drag || !self.hand_busy()) {
                            let (wx, wy) = self.screen_to_world(rect, pointer);
                            let (tx, ty) = self.stroke_anchor_tile(wx, wy);
                            self.begin_drag_stroke(tx, ty);
                            self.suppress_stroke_click = true;
                        }
                    }
                }
                if response.dragged_by(egui::PointerButton::Primary) {
                    if let (Some(idx), Some(pointer)) =
                        (self.dragging_building, response.interact_pointer_pos())
                    {
                        if !self.history_drag_pushed {
                            self.push_history();
                            self.history_drag_pushed = true;
                        }
                        let world = self.screen_to_world(
                            rect,
                            Pos2::new(pointer.x - self.drag_offset.x, pointer.y - self.drag_offset.y),
                        );
                        let (bw, bh) = self
                            .open
                            .as_ref()
                            .and_then(|o| o.stamp.buildings.get(idx))
                            .map(|b| {
                                let (w0, h0) = self.building_size(b.guid, b.variation);
                                oriented_size(w0, h0, b.direction)
                            })
                            .unwrap_or((1.0, 1.0));
                        let (mut sx, mut sy) =
                            self.snap_place_center(world.0, world.1, bw, bh);

                        if self.selection_drag_active() {
                            self.apply_selection_drag(idx, sx, sy);
                        } else {
                        // Farms with SubTileGrid fields: always move the complex together,
                        // otherwise GridOriginWS stays put and fields desync from the main.
                        let farm_drags_fields = self.open.as_ref().is_some_and(|o| {
                            let b = match o.stamp.buildings.get(idx) {
                                Some(b) => b,
                                None => return false,
                            };
                            let Some(own) = b.complex_owner_id.filter(|id| *id != 0) else {
                                return false;
                            };
                            let is_main = complex_main_index(
                                &o.stamp.buildings,
                                &self.catalog,
                                idx,
                            ) == Some(idx);
                            let need = needed_modules(self.catalog.building(b.guid));
                            let has_cells = o
                                .stamp
                                .polygon_cells
                                .iter()
                                .any(|c| c.complex_owner_id == Some(own));
                            is_main && (need > 0 || has_cells)
                        });
                        let move_linked = self.move_group
                            || ui.input(|i| i.modifiers.alt)
                            || farm_drags_fields;
                        let mut radius_clamp_msg: Option<String> = None;
                        if !move_linked {
                            if let Some(open_ref) = self.open.as_ref() {
                                if let Some(main_i) = complex_main_index(
                                    &open_ref.stamp.buildings,
                                    &self.catalog,
                                    idx,
                                ) {
                                    if main_i != idx {
                                        if let Some(main) =
                                            open_ref.stamp.buildings.get(main_i)
                                        {
                                            let r = module_radius(
                                                self.catalog.building(main.guid),
                                            );
                                            let dist =
                                                dist_tiles(main.x, main.y, sx, sy);
                                            if r > 0.0 && dist > r {
                                                let scale = r / dist;
                                                let cx = main.x + (sx - main.x) * scale;
                                                let cy = main.y + (sy - main.y) * scale;
                                                let snapped =
                                                    self.snap_place_center(cx, cy, bw, bh);
                                                sx = snapped.0;
                                                sy = snapped.1;
                                                radius_clamp_msg = Some(format!(
                                                    "Zu weit vom Hauptgebäude — begrenzt auf Radius {r:.0}"
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(open_mut) = self.open.as_mut() {
                            if open_mut
                                .stamp
                                .set_building_pos_ex(idx, sx, sy, move_linked)
                                .is_ok()
                            {
                                open_mut.dirty = true;
                                self.schedule_autosave();
                            }
                        }
                        if let Some(msg) = radius_clamp_msg {
                            self.status = msg;
                        }
                        self.refresh_coverage();
                        }
                    } else if self.drag_stroke.is_some() && self.dragging_building.is_none() {
                        if let Some(pointer) = response.interact_pointer_pos() {
                            let (wx, wy) = self.screen_to_world(rect, pointer);
                            let (tx, ty) = self.stroke_anchor_tile(wx, wy);
                            self.update_drag_stroke_cursor(tx, ty);
                        }
                    }
                }
                if response.drag_stopped() {
                    if self.drag_stroke.is_some() && self.dragging_building.is_none() {
                        self.commit_drag_stroke();
                    }
                    self.dragging_building = None;
                    self.history_drag_pushed = false;
                    self.clear_selection_drag();
                }
            }

            let stamp_empty = self.open.as_ref().is_some_and(|o| {
                o.is_new
                    && o.stamp.buildings.is_empty()
                    && self.coverage.roads.is_empty()
            });
            if stamp_empty {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Neues Stamp — Wege/Gebäude aus der Baulette platzieren",
                    egui::FontId::proportional(18.0),
                    Color32::from_gray(160),
                );
            }

            // Flush to window bottom (canvas rect bottom == client bottom in CentralPanel).
            let band_bottom = rect.max.y;
            let band = Rect::from_min_max(
                Pos2::new(rect.min.x, (band_bottom - build_bar_h).max(rect.min.y + 40.0)),
                Pos2::new(rect.max.x, band_bottom),
            );
            painter.rect_filled(band, 0.0, Color32::from_rgb(24, 28, 36));
            painter.text(
                Pos2::new(rect.left() + 10.0, band.min.y - 8.0),
                egui::Align2::LEFT_BOTTOM,
                "LMB: wählen/ziehen · RMB: Tool fallen / (leer) Schnellwahl-Rad · Esc: Hand · MMB: pan · Scroll: zoom",
                egui::FontId::proportional(12.0),
                Color32::from_gray(180),
            );
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(band), |ui| {

                let panel_rect = ui.max_rect();
                ui.painter()
                    .rect_filled(panel_rect, 0.0, Color32::from_rgb(24, 28, 36));
                {
                    let style = ui.style_mut();
                    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
                    style.spacing.button_padding = egui::vec2(4.0, 2.0);
                }

                let icon_st = self.ui_icons.extract_status();
                if icon_st.running {
                    ui.label(
                        RichText::new(format!("🖼 {}", icon_st.message))
                            .size(12.0)
                            .color(Color32::from_rgb(180, 200, 140)),
                    );
                }

                let tabs = self.buildmenu.tabs_for(self.region_filter).to_vec();
                if tabs.is_empty() {
                    ui.label("Kein Construction-Menu geladen (_gamedata/construction_menu.json).");
                    return;
                }

                let need_indices: Vec<usize> = tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.is_need())
                    .map(|(i, _)| i)
                    .collect();
                let infra_tab = tabs.iter().find(|t| t.is_infrastructure()).cloned();
                let tab_idx = self.buildmenu_tab.unwrap_or(0);
                let need_mode = self.buildmenu_infra_entry.is_none()
                    && tabs.get(tab_idx).map(|t| t.is_need()).unwrap_or(false);

                let content_entries: Vec<MenuEntry> = if let Some(ei) = self.buildmenu_infra_entry {
                    if let Some(infra) = infra_tab.as_ref() {
                        match infra.entries.get(ei) {
                            Some(MenuEntry::Category { entries, .. }) => entries.clone(),
                            _ => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    }
                } else if need_mode {
                    tabs.get(tab_idx)
                        .map(|t| t.entries.clone())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };

                // Frame with equal top/bottom margin => circles mittig in each row.
                let strip_h = BUILD_STRIP_H;
                let tile_row_h = 48.0_f32;
                let show_upper = need_mode || self.buildmenu_infra_entry.is_some();
                let vpad_strip = ((strip_h - tile_row_h) * 0.5).max(0.0);

                ui.vertical(|ui| {
                    if show_upper && !content_entries.is_empty() {
                        let upper_h = 60.0_f32;
                        let vpad = ((upper_h - tile_row_h) * 0.5).max(0.0);
                        egui::Frame::new()
                            .fill(Color32::from_rgb(32, 36, 46))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin {
                                left: 4,
                                right: 4,
                                top: vpad as i8,
                                bottom: vpad as i8,
                            })
                            .show(ui, |ui| {
                                ui.set_min_height(tile_row_h);
                                ui.set_max_height(tile_row_h);
                                ui.horizontal(|ui| {
                                    if let Some(ei) = self.buildmenu_entry {
                                        if let Some(entry) = content_entries.get(ei) {
                                            match entry {
                                                MenuEntry::Chain {
                                                    buildings,
                                                    short_name,
                                                    name,
                                                    ..
                                                } => {
                                                    let chain_lbl = short_name
                                                        .clone()
                                                        .unwrap_or_else(|| {
                                                            name.replace("&amp;", "&")
                                                        });
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "Kette: {chain_lbl}"
                                                        ))
                                                        .size(13.0)
                                                        .color(Color32::from_rgb(
                                                            230, 210, 160,
                                                        )),
                                                    );
                                                    ui.add_space(6.0);
                                                    for &bg in buildings {
                                                        let lbl = self.catalog.label(bg);
                                                        let icon = self
                                                            .buildmenu
                                                            .building_icon(bg)
                                                            .map(|s| s.to_string());
                                                        let sel =
                                                            self.placing_guid == Some(bg);
                                                        if self
                                                            .goods_tile(
                                                                ui,
                                                                ctx,
                                                                icon.as_deref(),
                                                                sel,
                                                                &lbl,
                                                            )
                                                            .clicked()
                                                        {
                                                            self.activate_menu_building(
                                                                bg, &lbl,
                                                            );
                                                        }
                                                    }
                                                    ui.separator();
                                                }
                                                MenuEntry::Category {
                                                    entries: nested,
                                                    short_name,
                                                    name,
                                                    ..
                                                } => {
                                                    let cat_lbl = short_name
                                                        .clone()
                                                        .unwrap_or_else(|| {
                                                            name.replace("&amp;", "&")
                                                        });
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "Gruppe: {cat_lbl}"
                                                        ))
                                                        .size(13.0)
                                                        .color(Color32::from_rgb(
                                                            230, 210, 160,
                                                        )),
                                                    );
                                                    ui.add_space(6.0);
                                                    for nest in nested {
                                                        let tip = nest.label();
                                                        let sel = matches!(
                                                            nest,
                                                            MenuEntry::Building {
                                                                guid,
                                                                ..
                                                            } if self.placing_guid
                                                                == Some(*guid)
                                                        );
                                                        if self
                                                            .goods_tile(
                                                                ui,
                                                                ctx,
                                                                nest.icon(),
                                                                sel,
                                                                &tip,
                                                            )
                                                            .clicked()
                                                        {
                                                            match nest {
                                                                MenuEntry::Building {
                                                                    guid,
                                                                    ..
                                                                } => {
                                                                    let lbl = self
                                                                        .catalog
                                                                        .label(*guid);
                                                                    self.activate_menu_building(
                                                                        *guid, &lbl,
                                                                    );
                                                                }
                                                                MenuEntry::Chain {
                                                                    buildings,
                                                                    ..
                                                                } => {
                                                                    if let Some(&bg) =
                                                                        buildings.first()
                                                                    {
                                                                        let lbl = self
                                                                            .catalog
                                                                            .label(bg);
                                                                        self.activate_menu_building(
                                                                            bg, &lbl,
                                                                        );
                                                                    }
                                                                }
                                                                MenuEntry::Category {
                                                                    ..
                                                                } => {}
                                                            }
                                                        }
                                                    }
                                                    ui.separator();
                                                }
                                                MenuEntry::Building { .. } => {}
                                            }
                                        }
                                    }

                                    ui.label(
                                        RichText::new(if need_mode {
                                            "Needs / Ketten"
                                        } else {
                                            "Gebäude / Gruppen"
                                        })
                                        .size(12.0)
                                        .weak(),
                                    );
                                    ui.add_space(6.0);
                                    for (ei, entry) in content_entries.iter().enumerate() {
                                        let sel = self.buildmenu_entry == Some(ei);
                                        let tip = entry.label();
                                        if self
                                            .goods_tile(
                                                ui, ctx, entry.icon(), sel, &tip,
                                            )
                                            .clicked()
                                        {
                                            self.buildmenu_entry = Some(ei);
                                            match entry {
                                                MenuEntry::Building { guid, .. } => {
                                                    let lbl = self.catalog.label(*guid);
                                                    self.activate_menu_building(
                                                        *guid, &lbl,
                                                    );
                                                }
                                                MenuEntry::Chain { .. } => {
                                                    self.placing_guid = None;
                                                    self.road_brush_guid = None;
                                                    self.status = format!(
                                                        "Kette „{tip}“ — Gebäude oben wählen"
                                                    );
                                                }
                                                MenuEntry::Category { .. } => {
                                                    self.placing_guid = None;
                                                    self.status =
                                                        format!("Gruppe „{tip}“");
                                                }
                                            }
                                        }
                                    }
                                });
                            });
                        ui.add_space(2.0);
                        ui.separator();
                        ui.add_space(2.0);
                    }

                    egui::Frame::new()
                        .fill(Color32::from_rgb(32, 36, 46))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin {
                            left: 4,
                            right: 4,
                            top: vpad_strip as i8,
                            bottom: vpad_strip as i8,
                        })
                        .show(ui, |ui| {
                            ui.set_min_height(tile_row_h);
                            ui.set_max_height(tile_row_h);
                            ui.horizontal(|ui| {
                                if let Some(infra) = infra_tab.as_ref() {
                                    for (ei, entry) in infra.entries.iter().enumerate() {
                                        let sel = match entry {
                                            MenuEntry::Building { guid, .. } => {
                                                self.road_brush_guid
                                                    == Some(*guid as i32)
                                                    || self.placing_guid == Some(*guid)
                                            }
                                            MenuEntry::Category { .. } => {
                                                self.buildmenu_infra_entry == Some(ei)
                                            }
                                            MenuEntry::Chain { buildings, .. } => {
                                                self.buildmenu_infra_entry == Some(ei)
                                                    || buildings.iter().any(|g| {
                                                        self.placing_guid == Some(*g)
                                                    })
                                            }
                                        };
                                        let tip = entry.label();
                                        if self
                                            .goods_tile(
                                                ui, ctx, entry.icon(), sel, &tip,
                                            )
                                            .clicked()
                                        {
                                            if let Some(i) = tabs
                                                .iter()
                                                .position(|t| t.is_infrastructure())
                                            {
                                                self.buildmenu_tab = Some(i);
                                            }
                                            match entry {
                                                MenuEntry::Building { guid, .. } => {
                                                    self.buildmenu_infra_entry = Some(ei);
                                                    self.buildmenu_entry = None;
                                                    let lbl = self.catalog.label(*guid);
                                                    self.activate_menu_building(
                                                        *guid, &lbl,
                                                    );
                                                }
                                                MenuEntry::Category { .. } => {
                                                    self.buildmenu_infra_entry = Some(ei);
                                                    self.buildmenu_entry = None;
                                                    self.placing_guid = None;
                                                    self.status =
                                                        format!("Kategorie: {tip}");
                                                }
                                                MenuEntry::Chain { buildings, .. } => {
                                                    self.buildmenu_infra_entry = Some(ei);
                                                    self.buildmenu_entry = Some(0);
                                                    if let Some(&bg) = buildings.first() {
                                                        let lbl = self.catalog.label(bg);
                                                        self.activate_menu_building(
                                                            bg, &lbl,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                ui.add_space(10.0);
                                ui.separator();
                                ui.add_space(8.0);
                                ui.label(RichText::new("Einwohner").size(12.0).weak());
                                ui.add_space(4.0);

                                for &i in &need_indices {
                                    let t = &tabs[i];
                                    let selected =
                                        need_mode && self.buildmenu_tab == Some(i);
                                    if self
                                        .portrait_tile(
                                            ui,
                                            ctx,
                                            t.category_icon.as_deref(),
                                            t.tier_mark(),
                                            selected,
                                            &t.short_tier_label(),
                                        )
                                        .clicked()
                                    {
                                        self.buildmenu_tab = Some(i);
                                        self.buildmenu_entry = None;
                                        self.buildmenu_infra_entry = None;
                                        self.road_brush_guid = None;
                                        self.status =
                                            format!("Tier: {}", t.short_tier_label());
                                    }
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(8.0);
                                        if let Some(g) = self.placing_guid {
                                            ui.label(
                                                RichText::new(format!(
                                                    "Hand: {}",
                                                    self.catalog.label(g)
                                                ))
                                                .size(13.0)
                                                .color(Color32::from_rgb(170, 205, 255)),
                                            );
                                        }
                                        ui.label(
                                            RichText::new(self.region_filter.label())
                                                .size(12.0)
                                                .weak(),
                                        );
                                    },
                                );
                            });
                        });
                });
            });

            self.paint_radial_menu(ui, ctx, &painter, rect);

        });

        self.show_complex_module_ui(ctx);
    }
}

