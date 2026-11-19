use stamp_editor::buildmenu::{ConstructionMenu, MenuEntry};
use stamp_editor::catalog::{
    blocker_min_for, is_aqueduct_cistern, oriented_blocker, size_for_guid, Catalog, RegionFilter,
};
use stamp_editor::icons::{chrome, UiIcons};
use stamp_editor::coverage::{
    build_road_map_region, building_has_area_effect, compute_coverage_region,
    compute_outgoing_preview_with_roads, compute_roads_only_region, footprint_cells_for_region,
    kind_color, layout_stats_region, Cell, CoverageMap, CoverageMode, RoadKind,
};
use stamp_editor::needs::{
    analyse_city_needs, provided_need_guids, report_for_building, report_for_def,
    ConsumptionRow, ResidenceNeedReport, ServiceCoverageRow,
};
use stamp_editor::area_efficiency::{
    analyze_area_efficiency_with_fields, blocker_map, competition_group,
    cell_is_free_for_group, contested_free_cells, field_resource_map, is_resource_area_building,
    radius_cells, resource_need, resource_radius, AreaEfficiency, BlockReason,
};
use stamp_editor::economy::{analyze_chain_region, ChainReport};
use stamp_editor::history::{History, Snapshot};
use stamp_editor::i18n::{self, Language};
use stamp_editor::library::{self, EditorConfig, LibraryPack};
use stamp_editor::meshgraph::{
    apply_road_path, is_aqueduct_brush_guid, is_aqueduct_maxground_guid, is_road_brush_guid,
    mesh_road_cells, orthogonal_road_path, read_mesh_info, relative_to_tile, road_guid_for_region,
    CisternInlet, ROAD_GUID_DIRT, ROAD_GUID_STONE,
};
use stamp_editor::modules::{
    complex_group_indices, complex_main_index, count_progress_for_stamp_area, dist_tiles,
    is_module_building, module_radius, needed_modules, next_complex_owner_id, suggested_modules,
};
use stamp_editor::pack::{
    append_blank_stamp, copy_stamp_to_pack, delete_stamp_entry, duplicate_stamp, move_stamp_to_pack,
    new_blank_stamp, open_first_real_stamp, open_named_stamp, rename_stamp_entry, save_open_stamp,
    OpenStamp,
};
use stamp_editor::stamp::{
    apply_field_tiles, connected_field_subset, disconnected_field_cells, field_cells_of_owner,
    foreign_field_cells, Building,
};
use eframe::egui::{self, Color32, Frame, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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

fn sanitize_stamp_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cached full city analysis, keyed by a fingerprint of the open stamp.
/// Rebuilt only when the stamp actually changes, so continuous repaints
/// (panning, hovering) don't re-run the expensive BFS/coverage passes.
struct CityAnalysisCache {
    fingerprint: u64,
    areas: Vec<AreaEfficiency>,
    chain: ChainReport,
    city_consumption: Vec<ConsumptionRow>,
    city_services: Vec<ServiceCoverageRow>,
    /// Per-residence incoming coverage used to derive `city_services`; retained
    /// so future UI can drill into it without recomputing.
    #[allow(dead_code)]
    incoming_by_index: HashMap<usize, CoverageMap>,
}

/// Cached area-efficiency pass for the canvas overlay (cheaper subset of the
/// city analysis above), keyed by the same stamp fingerprint.
struct AreaAnalysisCache {
    fingerprint: u64,
    areas: Vec<AreaEfficiency>,
}

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
    language: Language,
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
    /// Pipette `Dir` while a building is in hand (`R` / `Shift+R`).
    place_direction: i32,
    /// Active drag stroke: road line, demolish rect, or place rect.
    drag_stroke: Option<DragStroke>,
    /// After a stroke drag, ignore the trailing `clicked()` so we don't double-apply.
    suppress_stroke_click: bool,
    /// Active road brush GUID (dirt vs Ziegel/stone). `None` = kein Weg-Pinsel.
    road_brush_guid: Option<i32>,
    /// Dedicated road eraser (orthogonal stroke always removes).
    road_erase_mode: bool,
    /// Radial „Kopieren“: next click / select-rect copies into the hand.
    copy_pipette_mode: bool,
    /// PlantFarm field brush: ComplexOwnerID + field module GUID.
    field_brush_owner: Option<u64>,
    field_brush_guid: Option<u32>,
    /// Anno-style RMB radial / quick-select wheel.
    radial: Option<RadialMenu>,
    history: History,
    history_drag_pushed: bool,
    library_root: PathBuf,
    /// Linked Anno 117 install (icons come from `maindata/ui.rda`, not redistributed).
    game_install: Option<PathBuf>,
    /// Live Ubisoft stamps folder (wipe-synced on game start) — export/mirror target.
    game_stamps_root: PathBuf,
    /// Mirror each library save into `game_stamps_root`.
    mirror_to_game: bool,
    /// Packs the user deleted — auto-seed from Documents must not restore them.
    suppressed_game_imports: Vec<String>,
    library_packs: Vec<LibraryPack>,
    library_clipboard: Option<LibraryClipboard>,
    move_group: bool,
    clipboard: Vec<ClipboardBuilding>,
    /// Roads copied with the last Ctrl+C (relative to primary).
    clipboard_roads: Vec<ClipboardRoad>,
    /// Absolute road cells in the current marquee selection.
    selected_roads: HashSet<(i32, i32)>,
    /// PlantFarm SubTileGrid tiles in the current selection (stamp-relative).
    selected_fields: HashSet<(i32, i32)>,
    /// Field-tile drag: origins at drag start.
    drag_field_origins: Vec<(i32, i32)>,
    /// Live delta (tiles) while dragging selected field cells.
    drag_field_delta: (i32, i32),
    /// Field cell under the cursor when the field drag started.
    drag_field_anchor: (i32, i32),
    /// True while LMB-dragging selected field tiles (not a building).
    dragging_fields: bool,
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
    /// Residence upgrade chain shown in the mid build row (place by tier).
    residence_tier_guids: Option<Vec<u32>>,
    /// Keep the service-needs window on this residence while placing its providers.
    residence_needs_pin: Option<usize>,
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
    /// `Some(draft_name)` → modal to create a new stamp in the active region folder.
    new_stamp_dialog: Option<String>,
    /// Request focus once when the new-stamp dialog opens.
    new_stamp_dialog_focus: bool,
    /// Rename stamp: `(pack_path, old_name, draft)`.
    rename_stamp_dialog: Option<(PathBuf, String, String)>,
    rename_stamp_dialog_focus: bool,
    /// First-run / path setup wizard.
    setup_wizard: Option<SetupWizardDraft>,
    setup_complete: bool,
    /// Recently opened pack paths (newest first, capped + deduped). Persisted in config.
    recent_packs: Vec<PathBuf>,
    /// Focus the library filter TextEdit next frame (set by Ctrl+F).
    focus_library_filter: bool,
    /// F1 keyboard-shortcut cheatsheet window.
    show_cheatsheet: bool,
    /// Last canvas rect (for zoom-to-fit, which needs the viewport size).
    last_canvas_rect: Option<Rect>,
    /// Cached full city analysis (analyse panel); rebuilt on stamp change only.
    city_analysis: Option<CityAnalysisCache>,
    /// Cached area-efficiency pass for the canvas overlay.
    area_analysis: Option<AreaAnalysisCache>,
    /// Cached residence incoming coverage for the service-needs window:
    /// `(stamp fingerprint, residence index, coverage)`.
    needs_cov_cache: Option<(u64, usize, CoverageMap)>,
    /// Bumped whenever stamp geometry that analysis depends on changes.
    /// Avoids re-hashing the whole stamp (incl. mesh roads) on every hover frame.
    analysis_gen: u64,
    /// Shared road map for live place-ghost coverage (`analysis_gen`, roads).
    place_cov_roads: RefCell<Option<(u64, Arc<HashMap<Cell, RoadKind>>)>>,
    /// Live Outgoing coverage while a place-ghost is under the cursor:
    /// `(analysis_gen, guid, direction, x bits, y bits, coverage)`.
    /// `RefCell` so the canvas can update the cache while `open` is immutably borrowed.
    place_ghost_cov: RefCell<Option<(u64, u32, i32, u32, u32, CoverageMap)>>,
    /// Last `(building idx, x bits, y bits)` for which drag refreshed coverage.
    drag_cov_key: Option<(usize, u32, u32)>,
}

#[derive(Debug, Clone)]
struct SetupWizardDraft {
    game_install: String,
    game_stamps: String,
    library: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrokeKind {
    /// Orthogonal road paint/toggle-erase along a line.
    Road { paint_on: bool },
    /// PlantFarm SubTileGrid paint/erase along an orthogonal path.
    Field { paint_on: bool },
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

/// Top-left tiles for a PlaceRect grid **anchored at drag start**, stepping toward
/// `cursor`. Only footprints fully inside the drag AABB are included. If the drag
/// is smaller than one footprint, returns `start` alone (click / tiny drag).
fn place_rect_top_lefts(
    start: (i32, i32),
    cursor: (i32, i32),
    step_w: i32,
    step_h: i32,
) -> Vec<(i32, i32)> {
    let step_w = step_w.max(1);
    let step_h = step_h.max(1);
    let (min_x, min_y, max_x, max_y) = rect_bounds(start, cursor);
    // Drag too small for even one footprint → single place at start (click).
    if max_x - min_x + 1 < step_w || max_y - min_y + 1 < step_h {
        return vec![start];
    }
    let dx = if cursor.0 >= start.0 { step_w } else { -step_w };
    let dy = if cursor.1 >= start.1 { step_h } else { -step_h };
    let mut out = Vec::new();
    let mut y = start.1;
    for _ in 0..512 {
        let y1 = y + step_h - 1;
        if y < min_y || y1 > max_y {
            break;
        }
        let mut x = start.0;
        for _ in 0..512 {
            let x1 = x + step_w - 1;
            if x < min_x || x1 > max_x {
                break;
            }
            out.push((x, y));
            let nx = x + dx;
            let nx1 = nx + step_w - 1;
            if nx < min_x || nx1 > max_x {
                break;
            }
            x = nx;
        }
        let ny = y + dy;
        let ny1 = ny + step_h - 1;
        if ny < min_y || ny1 > max_y {
            break;
        }
        y = ny;
    }
    if out.is_empty() {
        out.push(start);
    }
    out
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

/// Pipette `Pos` so the BuildBlocker top-left tile is `(tx, ty)`.
/// Centered buildings: `Pos = TL + size/2`. Decentered (cistern): uses blocker_min.
fn place_center_from_top_left(tx: i32, ty: i32, bw: i32, bh: i32) -> (f32, f32) {
    place_pos_from_top_left(tx, ty, bw as f32, bh as f32, -bw as f32 * 0.5, -bh as f32 * 0.5)
}

fn place_pos_from_top_left(
    tx: i32,
    ty: i32,
    w: f32,
    h: f32,
    mx: f32,
    my: f32,
) -> (f32, f32) {
    let _ = (w, h);
    (tx as f32 - mx, ty as f32 - my)
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

fn footprint_top_left_blocker(
    x: f32,
    y: f32,
    mx: f32,
    my: f32,
    drag: (f32, f32),
) -> (i32, i32) {
    (
        relative_to_tile(x + mx, drag.0),
        relative_to_tile(y + my, drag.1),
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
    /// i18n key (e.g. `radial.road_dirt`).
    label_key: &'static str,
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
                label_key: "radial.road_dirt",
                icon: "data/ui/fhd/base/icon_content/infrastructure/icon_3d_road_dirt.png",
            },
            RadialItem {
                action: RadialAction::RoadStone,
                label_key: "radial.road_stone",
                icon: "data/ui/fhd/base/icon_content/infrastructure/icon_3d_road_stone.png",
            },
            RadialItem {
                action: RadialAction::Demolish,
                label_key: "radial.demolish",
                icon: "data/ui/fhd/base/icon_content/tools/icon_2d_tool_demolish.png",
            },
            RadialItem {
                action: RadialAction::Copy,
                label_key: "radial.copy",
                icon: "data/ui/fhd/base/icon_content/tools/icon_2d_tool_copy.png",
            },
            RadialItem {
                action: RadialAction::Move,
                label_key: "radial.move",
                icon: "data/ui/fhd/base/icon_content/tools/icon_2d_tool_move_building.png",
            },
            RadialItem {
                action: RadialAction::ClearHand,
                label_key: "radial.clear_hand",
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
    RenameStamp { pack: PathBuf, name: String },
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
        let game_stamps_root = cfg.game_stamps_root.clone();
        let mirror_to_game = cfg.mirror_to_game;
        let library_packs = library::scan_library(&library_root);
        let library_fingerprint = library::library_fingerprint(&library_root);
        let language = Language::from_config(&cfg.language);
        let game_note = match &game_install {
            Some(p) if EditorConfig::looks_like_anno_install(p) => {
                i18n::tr_fmt(
                    language,
                    "status.game_linked",
                    &[("path", &p.display().to_string())],
                )
            }
            Some(p) => i18n::tr_fmt(
                language,
                "status.game_invalid",
                &[("path", &p.display().to_string())],
            ),
            None => i18n::tr(language, "status.game_none").to_string(),
        };
        let catalog_note = if catalog_ok {
            String::new()
        } else {
            i18n::tr_fmt(
                language,
                "status.catalog_bad",
                &[("path", &catalog_path.display().to_string())],
            )
        };
        let n_packs = library_packs.len().to_string();
        let lib_path = library_root.display().to_string();
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
            language,
            palette_selected_guid: None,
            status: i18n::tr_fmt(
                language,
                "status.library",
                &[
                    ("path", &lib_path),
                    ("n", &n_packs),
                    ("game", &game_note),
                    ("catalog", &catalog_note),
                ],
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
            place_direction: 0,
            drag_stroke: None,
            suppress_stroke_click: false,
            road_brush_guid: None,
            road_erase_mode: false,
            copy_pipette_mode: false,
            field_brush_owner: None,
            field_brush_guid: None,
            radial: None,
            history: History::new(),
            history_drag_pushed: false,
            library_root,
            game_install,
            game_stamps_root,
            mirror_to_game,
            suppressed_game_imports: cfg.suppressed_game_imports.clone(),
            library_packs,
            library_clipboard: None,
            move_group: false,
            clipboard: Vec::new(),
            clipboard_roads: Vec::new(),
            selected_roads: HashSet::new(),
            selected_fields: HashSet::new(),
            drag_field_origins: Vec::new(),
            drag_field_delta: (0, 0),
            drag_field_anchor: (0, 0),
            dragging_fields: false,
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
            residence_tier_guids: None,
            residence_needs_pin: None,
            ui_icons: UiIcons::new(),
            autosave_at: None,
            library_selection: HashSet::new(),
            library_marquee: None,
            library_fingerprint,
            library_watch_at: Instant::now() + Duration::from_secs(1),
            new_stamp_dialog: None,
            new_stamp_dialog_focus: false,
            rename_stamp_dialog: None,
            rename_stamp_dialog_focus: false,
            setup_complete: cfg.setup_complete,
            setup_wizard: None,
            recent_packs: cfg.recent_packs.clone(),
            focus_library_filter: false,
            show_cheatsheet: false,
            last_canvas_rect: None,
            city_analysis: None,
            area_analysis: None,
            needs_cov_cache: None,
            analysis_gen: 0,
            place_cov_roads: RefCell::new(None),
            place_ghost_cov: RefCell::new(None),
            drag_cov_key: None,
        };
        if cfg.needs_first_run() {
            // Prefill wizard (detect install), but don't load icons/library until confirmed.
            app.setup_wizard = Some(SetupWizardDraft {
                game_install: EditorConfig::guess_game_install()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                game_stamps: if EditorConfig::documents_stamps_available() {
                    EditorConfig::default_game_stamps_root()
                        .display()
                        .to_string()
                } else {
                    String::new()
                },
                library: EditorConfig::default_root().display().to_string(),
                error: None,
            });
            app.library_packs.clear();
            app.game_install = None;
        } else if let Some(g) = app.game_install.clone() {
            app.ui_icons.attach_game_install(&g);
        }
        // Empty canvas is a blank editable stamp (not a dead "open file" screen).
        // Must live under Region Roman/Celtic/Egyptian — Anno wipe-syncs other folders.
        if app.setup_complete {
            library::ensure_region_library_layout(&app.library_root);
            let boot_dir = app.region_stamp_dir();
            let _ = std::fs::create_dir_all(&boot_dir);
            app.start_new_stamp_in(&boot_dir, false);
        }
        if !catalog_ok {
            app.status = format!(
                "Katalog fehlt/kaputt: {} — Footprints/Labels unzuverlässig. {game_note}",
                app.catalog_path.display()
            );
        }
        app
    }

    fn open_setup_wizard(&mut self) {
        self.setup_wizard = Some(SetupWizardDraft {
            game_install: self
                .game_install
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| {
                    EditorConfig::guess_game_install()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                }),
            game_stamps: if self.game_stamps_root.is_dir() {
                self.game_stamps_root.display().to_string()
            } else if EditorConfig::documents_stamps_available() {
                EditorConfig::default_game_stamps_root()
                    .display()
                    .to_string()
            } else {
                String::new()
            },
            library: self.library_root.display().to_string(),
            error: None,
        });
    }

    fn apply_setup_wizard(&mut self) {
        let Some(draft) = self.setup_wizard.clone() else {
            return;
        };
        let game_path = draft.game_install.trim();
        let stamps_path = draft.game_stamps.trim();
        let lib_path = draft.library.trim();
        let err_game = self.t("setup.err_game").to_string();
        let err_stamps = self.t("setup.err_stamps").to_string();
        let err_lib_sync = self.t("setup.err_library_sync").to_string();
        let err_lib_create = self.t("setup.err_library_create").to_string();

        let game_install = if game_path.is_empty() {
            None
        } else {
            let p = PathBuf::from(game_path);
            if !EditorConfig::looks_like_anno_install(&p) {
                if let Some(w) = self.setup_wizard.as_mut() {
                    w.error = Some(err_game);
                }
                return;
            }
            Some(p)
        };

        let game_stamps = if stamps_path.is_empty() {
            let fallback = EditorConfig::default_game_stamps_root();
            if fallback.is_dir() {
                fallback
            } else {
                if let Some(w) = self.setup_wizard.as_mut() {
                    w.error = Some(err_stamps);
                }
                return;
            }
        } else {
            let p = PathBuf::from(stamps_path);
            if !p.is_dir() {
                if let Some(w) = self.setup_wizard.as_mut() {
                    w.error = Some(err_stamps);
                }
                return;
            }
            p
        };

        let library = if lib_path.is_empty() {
            EditorConfig::default_root()
        } else {
            PathBuf::from(lib_path)
        };
        if EditorConfig::is_game_stamps_path(&library, &game_stamps) {
            if let Some(w) = self.setup_wizard.as_mut() {
                w.error = Some(err_lib_sync);
            }
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&library) {
            if let Some(w) = self.setup_wizard.as_mut() {
                w.error = Some(format!("{err_lib_create}: {e}"));
            }
            return;
        }

        self.game_install = game_install.clone();
        self.game_stamps_root = game_stamps.clone();
        self.library_root = library.clone();
        self.setup_complete = true;
        self.setup_wizard = None;
        self.save_config();
        library::ensure_region_library_layout(&self.library_root);
        let seeded = library::seed_library_from_game(&self.library_root, &self.game_stamps_root)
            .unwrap_or(0);
        self.refresh_library();
        if let Some(g) = game_install {
            self.ui_icons.attach_game_install(&g);
        }
        let boot_dir = self.region_stamp_dir();
        let _ = std::fs::create_dir_all(&boot_dir);
        self.start_new_stamp_in(&boot_dir, false);
        self.status = self.tf(
            "setup.done",
            &[
                ("lib", &self.library_root.display().to_string()),
                ("n", &seeded.to_string()),
            ],
        );
    }

    fn show_setup_wizard(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.setup_wizard.clone() else {
            return;
        };
        let mut finish = false;
        let mut cancel = false;
        let mut pick_game = false;
        let mut pick_stamps = false;
        let mut pick_lib = false;
        let mut detect_game = false;
        let stamps_hint = if EditorConfig::documents_stamps_available() {
            self.t("setup.stamps_default_ok")
        } else {
            self.t("setup.stamps_default_missing")
        };

        // Dim the whole UI so the wizard can't be missed.
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("setup_wizard_dim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    Color32::from_rgba_unmultiplied(8, 10, 14, 200),
                );
                ui.allocate_rect(screen, egui::Sense::click());
            });

        egui::Window::new(self.t("setup.title"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_width(520.0);
                ui.label(self.t("setup.intro"));
                ui.add_space(8.0);

                ui.label(RichText::new(self.t("setup.game_label")).strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.game_install)
                            .desired_width(360.0)
                            .hint_text(self.t("setup.game_hint")),
                    );
                    if ui.button(self.t("setup.browse")).clicked() {
                        pick_game = true;
                    }
                    if ui.button(self.t("setup.detect")).clicked() {
                        detect_game = true;
                    }
                });
                ui.label(
                    RichText::new(self.t("setup.game_note"))
                        .small()
                        .weak(),
                );
                ui.add_space(10.0);

                ui.label(RichText::new(self.t("setup.stamps_label")).strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.game_stamps)
                            .desired_width(360.0)
                            .hint_text(self.t("setup.stamps_hint")),
                    );
                    if ui.button(self.t("setup.browse")).clicked() {
                        pick_stamps = true;
                    }
                });
                ui.label(RichText::new(stamps_hint).small().weak());
                ui.add_space(10.0);

                ui.label(RichText::new(self.t("setup.library_label")).strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut draft.library)
                            .desired_width(360.0)
                            .hint_text(self.t("setup.library_hint")),
                    );
                    if ui.button(self.t("setup.browse")).clicked() {
                        pick_lib = true;
                    }
                });
                ui.label(
                    RichText::new(self.t("setup.library_note"))
                        .small()
                        .weak(),
                );

                if let Some(err) = &draft.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(230, 120, 90), err);
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(self.t("setup.finish")).clicked() {
                        finish = true;
                    }
                    if self.setup_complete && ui.button(self.t("setup.cancel")).clicked() {
                        cancel = true;
                    }
                });
                ui.label(
                    RichText::new(self.t("setup.standalone_note"))
                        .small()
                        .weak(),
                );
            });

        if detect_game {
            if let Some(p) = EditorConfig::guess_game_install() {
                draft.game_install = p.display().to_string();
                draft.error = None;
            } else {
                draft.error = Some(self.t("setup.err_detect").into());
            }
        }
        if pick_game {
            let start = PathBuf::from(draft.game_install.trim()).parent().map(|p| p.to_path_buf())
                .or_else(|| EditorConfig::guess_game_install())
                .unwrap_or_else(|| PathBuf::from(r"C:\"));
            if let Some(path) = rfd::FileDialog::new()
                .set_title(self.t("dialog.link_game_title"))
                .set_directory(start)
                .pick_folder()
            {
                draft.game_install = path.display().to_string();
                draft.error = None;
            }
        }
        if pick_stamps {
            let start = if PathBuf::from(draft.game_stamps.trim()).is_dir() {
                PathBuf::from(draft.game_stamps.trim())
            } else {
                EditorConfig::default_game_stamps_root()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from(r"C:\"))
            };
            if let Some(path) = rfd::FileDialog::new()
                .set_title(self.t("setup.stamps_pick"))
                .set_directory(start)
                .pick_folder()
            {
                draft.game_stamps = path.display().to_string();
                draft.error = None;
            }
        }
        if pick_lib {
            let start = if PathBuf::from(draft.library.trim()).is_dir() {
                PathBuf::from(draft.library.trim())
            } else {
                EditorConfig::config_dir()
            };
            if let Some(path) = rfd::FileDialog::new()
                .set_title(self.t("setup.library_pick"))
                .set_directory(start)
                .pick_folder()
            {
                draft.library = path.display().to_string();
                draft.error = None;
            }
        }

        if cancel {
            self.setup_wizard = None;
            return;
        }
        self.setup_wizard = Some(draft);
        if finish {
            self.apply_setup_wizard();
        }
    }

    fn default_stamp_name(&self) -> &'static str {
        self.t("stamp.default_name")
    }

    /// Anno only keeps `Region Roman` / `Region Celtic` / `Region Egyptian` under stamps/.
    fn region_stamp_dir(&self) -> PathBuf {
        self.library_root
            .join(self.region_filter.game_stamp_folder())
    }

    fn unique_new_pack_path_in(&self, dir: &Path, base_name: &str) -> PathBuf {
        let _ = std::fs::create_dir_all(dir);
        let base = base_name.trim();
        let base = if base.is_empty() {
            self.default_stamp_name()
        } else {
            base
        };
        library::unique_json_path(dir, base)
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
    /// Also pulls missing packs from the live game stamps folder (in-game creates).
    fn tick_library_watch(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        // Keep waking egui even when idle so external folder changes are noticed.
        ctx.request_repaint_after(Duration::from_secs(1));
        if now < self.library_watch_at {
            return;
        }
        self.library_watch_at = now + Duration::from_secs(1);
        // Never auto-import game stamps before first-run setup is confirmed.
        // Skip packs the user deleted in the library (tombstones) so Documents
        // leftovers / cloud restore do not undo library deletes every second.
        if self.setup_complete && !self.library_is_game_stamps() {
            if let Ok((n, _)) = library::import_packs_from_game(
                &self.library_root,
                &self.game_stamps_root,
                false,
                &self.suppressed_game_imports,
            ) {
                if n > 0 {
                    self.refresh_library_silent();
                    self.status = self.tf(
                        "status.imported_from_game",
                        &[("n", &n.to_string())],
                    );
                    return;
                }
            }
        }
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

    fn import_from_game(&mut self) {
        if self.library_is_game_stamps() {
            self.status = self.t("status.export_same_folder").into();
            return;
        }
        // Explicit import may restore previously deleted packs — clear their tombstones.
        match library::import_packs_from_game(
            &self.library_root,
            &self.game_stamps_root,
            true,
            &[],
        ) {
            Ok((n, keys)) => {
                for key in keys {
                    self.suppressed_game_imports.retain(|k| k != &key);
                }
                self.refresh_library();
                self.save_config();
                self.status = self.tf(
                    "status.imported_from_game",
                    &[("n", &n.to_string())],
                );
            }
            Err(e) => {
                self.status = self.tf("status.import_failed", &[("err", &e)]);
            }
        }
    }

    /// Write current stamp to disk (creates pack file if still `is_new`).
    /// Only runs when `dirty` — empty boot blanks stay in memory until first edit.
    fn autosave_now(&mut self, refresh_lib: bool) {
        let dirty = self.open.as_ref().is_some_and(|o| o.dirty);
        if !dirty {
            return;
        }
        let save_result = self.open.as_ref().map(save_open_stamp);
        match save_result {
            Some(Ok(())) => {
                let (path, name) = {
                    let open = self.open.as_mut().unwrap();
                    open.dirty = false;
                    open.is_new = false;
                    (open.pack_path.clone(), open.entry_name.clone())
                };
                if refresh_lib {
                    self.refresh_library_silent();
                }
                let mirror_note = self.mirror_pack_after_save(&path);
                self.status = self.tf(
                    "status.saved",
                    &[("name", &name), ("path", &path.display().to_string())],
                ) + &mirror_note;
            }
            Some(Err(err)) => {
                self.status = self.tf("status.save_failed", &[("err", &err.to_string())])
            }
            None => {}
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
        // Persist dirty canvas before replacing `self.open` (e.g. library click).
        // Boot blanks start clean (`dirty: false`) so they are not written here.
        let should = self.open.as_ref().is_some_and(|o| o.dirty);
        if should {
            let refresh = self
                .open
                .as_ref()
                .is_some_and(|o| o.is_new || !o.pack_path.exists());
            self.autosave_now(refresh);
        }
    }

    /// Open the name dialog for a new stamp (current region folder).
    fn start_new_stamp(&mut self) {
        self.new_stamp_dialog = Some(self.default_stamp_name().to_string());
        self.new_stamp_dialog_focus = true;
    }

    fn start_new_stamp_named(&mut self, dir: &Path, name: &str, persist_now: bool) {
        self.flush_autosave_blocking();
        let path = self.unique_new_pack_path_in(dir, name);
        let entry_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.default_stamp_name().to_string());
        let region = self.region_filter.stamp_region_guid();
        match new_blank_stamp(path, entry_name, region) {
            Ok(open) => {
                self.history.clear();
                self.history_drag_pushed = false;
                self.clear_analysis_caches();
                // Don't keep Abriss/Weg/Feld from the previous stamp (e.g. after D).
                self.clear_hand_silent();
                self.clear_canvas_selection();
                self.autosave_at = None;
                self.open = Some(open);
                self.center_view();
                self.refresh_coverage();
                if persist_now {
                    if let Some(o) = self.open.as_mut() {
                        o.dirty = true;
                    }
                    self.autosave_now(true);
                    self.status = self.tf(
                        "status.new_stamp_ok",
                        &[("region", self.region_filter.label_lang(self.language))],
                    );
                } else {
                    self.status = self.tf(
                        "status.new_stamp_empty",
                        &[("region", self.region_filter.label_lang(self.language))],
                    );
                }
            }
            Err(err) => {
                self.status = self.tf("status.new_stamp_failed", &[("err", &err.to_string())])
            }
        }
    }

    fn start_new_stamp_in(&mut self, dir: &Path, persist_now: bool) {
        let name = self.default_stamp_name().to_string();
        self.start_new_stamp_named(dir, &name, persist_now);
    }

    fn show_rename_stamp_dialog(&mut self, ctx: &egui::Context) {
        let Some((_, _, _)) = self.rename_stamp_dialog.as_ref() else {
            return;
        };
        let title = self.t("lib.rename_title");
        let name_lbl = self.t("lib.rename_name");
        let ok_lbl = self.t("lib.rename_ok");
        let cancel_lbl = self.t("lib.rename_cancel");
        let mut confirmed = false;
        let mut cancelled = false;
        let mut enter = false;
        let want_focus = self.rename_stamp_dialog_focus;
        egui::Window::new(title)
            .id(egui::Id::new("rename_stamp_dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(name_lbl);
                let resp = ui.text_edit_singleline(
                    &mut self
                        .rename_stamp_dialog
                        .as_mut()
                        .expect("dialog open")
                        .2,
                );
                if want_focus {
                    resp.request_focus();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    enter = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(ok_lbl).clicked() {
                        confirmed = true;
                    }
                    if ui.button(cancel_lbl).clicked() {
                        cancelled = true;
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancelled = true;
                }
            });
        if want_focus {
            self.rename_stamp_dialog_focus = false;
        }
        if cancelled {
            self.rename_stamp_dialog = None;
            return;
        }
        if confirmed || enter {
            let Some((pack, old_name, draft)) = self.rename_stamp_dialog.take() else {
                return;
            };
            let name = sanitize_stamp_filename(draft.trim());
            if name.is_empty() {
                self.rename_stamp_dialog = Some((pack, old_name.clone(), old_name));
                self.rename_stamp_dialog_focus = true;
                return;
            }
            self.flush_autosave_blocking();
            match rename_stamp_entry(&pack, &old_name, &name) {
                Ok((new_path, new_name)) => {
                    if let Some(open) = self.open.as_mut() {
                        if open.pack_path == pack && open.entry_name == old_name {
                            open.pack_path = new_path.clone();
                            open.entry_name = new_name.clone();
                        }
                    }
                    self.refresh_library();
                    self.status = self.tf(
                        "status.stamp_renamed",
                        &[("old", &old_name), ("new", &new_name)],
                    );
                }
                Err(e) => {
                    self.status =
                        self.tf("status.rename_failed", &[("err", &e.to_string())]);
                }
            }
        }
    }

    fn show_new_stamp_dialog(&mut self, ctx: &egui::Context) {
        let Some(_) = self.new_stamp_dialog.as_ref() else {
            return;
        };
        let title = self.t("stamp.new_dialog_title");
        let name_lbl = self.t("stamp.new_dialog_name");
        let ok_lbl = self.t("stamp.new_dialog_ok");
        let cancel_lbl = self.t("stamp.new_dialog_cancel");
        let mut confirmed = false;
        let mut cancelled = false;
        let mut enter = false;
        let want_focus = self.new_stamp_dialog_focus;
        egui::Window::new(title)
            .id(egui::Id::new("new_stamp_dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(name_lbl);
                let resp = ui.text_edit_singleline(
                    self.new_stamp_dialog.as_mut().expect("dialog open"),
                );
                if want_focus {
                    resp.request_focus();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    enter = true;
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(ok_lbl).clicked() {
                        confirmed = true;
                    }
                    if ui.button(cancel_lbl).clicked() {
                        cancelled = true;
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancelled = true;
                }
            });
        if want_focus {
            self.new_stamp_dialog_focus = false;
        }
        if cancelled {
            self.new_stamp_dialog = None;
            return;
        }
        if confirmed || enter {
            let name = self
                .new_stamp_dialog
                .take()
                .unwrap_or_else(|| self.default_stamp_name().to_string());
            let name = sanitize_stamp_filename(name.trim());
            if name.is_empty() {
                self.new_stamp_dialog = Some(self.default_stamp_name().to_string());
                self.new_stamp_dialog_focus = true;
                return;
            }
            let dir = self.region_stamp_dir();
            let _ = std::fs::create_dir_all(&dir);
            self.start_new_stamp_named(&dir, &name, true);
        }
    }

    fn confirm_delete(&self, label: &str) -> bool {
        matches!(
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title(self.t("dialog.delete_title"))
                .set_description(label)
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show(),
            rfd::MessageDialogResult::Ok
        )
    }

    /// After deleting the open stamp/pack: open another library entry, or a silent blank.
    /// Never opens the New-Stamp name dialog.
    fn open_fallback_after_delete(&mut self) {
        self.autosave_at = None;
        self.open = None;
        self.clear_analysis_caches();
        // Prefer a pack that still matches the region filter.
        let region = self.region_filter;
        let pick = self
            .library_packs
            .iter()
            .find(|p| region.matches_stamp(p.region_hint, &p.folder) && !p.stamps.is_empty())
            .or_else(|| self.library_packs.iter().find(|p| !p.stamps.is_empty()));
        if let Some(pack) = pick {
            if let Some(entry) = pack.stamps.first() {
                let path = pack.path.clone();
                let name = entry.name.clone();
                // open_named flushes; open is already None so that's fine.
                match open_named_stamp(&path, &name) {
                    Ok(open) => {
                        self.set_open(open, &path);
                        return;
                    }
                    Err(_) => {}
                }
            }
        }
        let dir = self.region_stamp_dir();
        let _ = std::fs::create_dir_all(&dir);
        self.start_new_stamp_in(&dir, false);
    }

    /// Delete selected library packs/stamps (with confirm). Returns true if handled.
    fn delete_library_selection(&mut self) -> bool {
        if self.library_selection.is_empty() {
            return false;
        }
        let items: Vec<LibItem> = self.library_selection.iter().cloned().collect();
        let msg = if items.len() == 1 {
            match &items[0] {
                LibItem::Pack(path) => {
                    let name = path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    self.tf("dialog.delete_pack", &[("name", &name)])
                }
                LibItem::Stamp { name, .. } => {
                    self.tf("dialog.delete_stamp", &[("name", name)])
                }
            }
        } else {
            self.tf(
                "dialog.delete_n",
                &[("n", &items.len().to_string())],
            )
        };
        if !self.confirm_delete(&msg) {
            return true;
        }

        let open_pack = self.open.as_ref().map(|o| o.pack_path.clone());
        let open_name = self.open.as_ref().map(|o| o.entry_name.clone());
        let mut deleted_open = false;
        let mut err: Option<String> = None;
        let mut deleted = 0usize;

        for item in &items {
            let hits_open = match item {
                LibItem::Pack(path) => open_pack.as_ref() == Some(path),
                LibItem::Stamp { pack, name } => {
                    open_pack.as_ref() == Some(pack) && open_name.as_deref() == Some(name.as_str())
                }
            };
            if hits_open {
                self.autosave_at = None;
                self.open = None;
                self.clear_analysis_caches();
                deleted_open = true;
            }
            let result = match item {
                LibItem::Pack(path) => {
                    // Tombstone + delete Documents mirror so the 1s auto-seed
                    // cannot resurrect the pack from the game folder.
                    self.suppress_pack_import(path);
                    library::delete_pack_and_mirror(
                        &self.library_root,
                        &self.game_stamps_root,
                        path,
                    )
                    .map(|_| ())
                }
                LibItem::Stamp { pack, name } => {
                    let pack_path = pack.clone();
                    let res = delete_stamp_entry(pack, name).map_err(|e| e.to_string());
                    if res.is_ok() {
                        if pack_path.is_file() {
                            // Pack still has stamps — sync the trimmed file to Documents.
                            self.allow_pack_import(&pack_path);
                            let _ = self.mirror_pack_after_save(&pack_path);
                        } else {
                            // Last stamp removed the pack file — same as pack delete.
                            self.suppress_pack_import(&pack_path);
                            if let Ok(rel) = pack_path.strip_prefix(&self.library_root) {
                                let game_path = self.game_stamps_root.join(rel);
                                if game_path.is_file() {
                                    let _ = std::fs::remove_file(&game_path);
                                }
                            }
                        }
                    }
                    res
                }
            };
            match result {
                Ok(()) => deleted += 1,
                Err(e) => err = Some(e),
            }
        }

        self.library_selection.clear();
        self.refresh_library();
        self.save_config();
        if deleted_open {
            self.open_fallback_after_delete();
        }
        self.status = if let Some(e) = err {
            self.tf(
                "status.delete_partial",
                &[("n", &deleted.to_string()), ("err", &e)],
            )
        } else {
            self.tf("status.deleted_n", &[("n", &deleted.to_string())])
        };
        true
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
                match append_blank_stamp(&pack, self.default_stamp_name(), region) {
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
                let msg = self.tf("dialog.delete_pack", &[("name", &name)]);
                if !self.confirm_delete(&msg) {
                    return;
                }
                let was_open = self.open.as_ref().is_some_and(|o| o.pack_path == path);
                if was_open {
                    // Don't autosave into a file we're about to remove.
                    self.autosave_at = None;
                    self.open = None;
                    self.clear_analysis_caches();
                }
                match library::delete_pack_file(&path) {
                    Ok(()) => {
                        self.refresh_library();
                        if was_open {
                            self.open_fallback_after_delete();
                        }
                        self.status = self.tf("status.pack_deleted", &[("name", &name)]);
                    }
                    Err(e) => {
                        self.status =
                            self.tf("status.delete_failed", &[("err", &e.to_string())])
                    }
                }
            }
            LibraryAction::DeleteStamp { pack, name } => {
                let msg = self.tf("dialog.delete_stamp", &[("name", &name)]);
                if !self.confirm_delete(&msg) {
                    return;
                }
                let was_open = self
                    .open
                    .as_ref()
                    .is_some_and(|o| o.pack_path == pack && o.entry_name == name);
                if was_open {
                    self.autosave_at = None;
                    self.open = None;
                    self.clear_analysis_caches();
                }
                match delete_stamp_entry(&pack, &name) {
                    Ok(()) => {
                        self.refresh_library();
                        if was_open {
                            self.open_fallback_after_delete();
                        }
                        self.status = self.tf("status.stamp_deleted", &[("name", &name)]);
                    }
                    Err(e) => {
                        self.status =
                            self.tf("status.delete_failed", &[("err", &e.to_string())])
                    }
                }
            }
            LibraryAction::DuplicateStamp { pack, name } => {
                match duplicate_stamp(&pack, &name) {
                    Ok(open) => {
                        self.refresh_library();
                        let path = open.pack_path.clone();
                        self.set_open(open, &path);
                        self.status = self.t("status.duplicated").into();
                    }
                    Err(e) => self.status = format!("Duplizieren fehlgeschlagen: {e}"),
                }
            }
            LibraryAction::RenameStamp { pack, name } => {
                self.rename_stamp_dialog = Some((pack, name.clone(), name));
                self.rename_stamp_dialog_focus = true;
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
                    None => self.status = self.t("status.clipboard_empty").into(),
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
                    None => self.status = self.t("status.clipboard_empty").into(),
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
                let folder_msg = self.tf("dialog.delete_folder", &[("name", &name)]);
                if !self.confirm_delete(&folder_msg) {
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
            game_stamps_root: self.game_stamps_root.clone(),
            mirror_to_game: self.mirror_to_game,
            language: self.language.as_config().to_string(),
            setup_complete: self.setup_complete,
            recent_packs: self.recent_packs.clone(),
            suppressed_game_imports: self.suppressed_game_imports.clone(),
        }
        .save();
    }

    fn suppress_pack_import(&mut self, pack_path: &Path) {
        let Some(key) = library::pack_rel_key(&self.library_root, pack_path) else {
            return;
        };
        self.suppressed_game_imports.retain(|k| k != &key);
        self.suppressed_game_imports.insert(0, key);
        self.suppressed_game_imports
            .truncate(library::SUPPRESSED_IMPORTS_MAX);
    }

    fn allow_pack_import(&mut self, pack_path: &Path) {
        if let Some(key) = library::pack_rel_key(&self.library_root, pack_path) {
            self.suppressed_game_imports.retain(|k| k != &key);
        }
    }

    fn library_is_game_stamps(&self) -> bool {
        EditorConfig::is_game_stamps_path(&self.library_root, &self.game_stamps_root)
    }

    /// Mirror a just-saved library pack into Documents stamps (best-effort).
    fn mirror_pack_after_save(&mut self, pack_path: &std::path::Path) -> String {
        // Saving again means the user wants this pack — lift any delete tombstone.
        self.allow_pack_import(pack_path);
        if !self.mirror_to_game || self.library_is_game_stamps() {
            return String::new();
        }
        match library::mirror_pack_to_game(
            &self.library_root,
            &self.game_stamps_root,
            pack_path,
        ) {
            Ok(dest) => format!(" · Spiegel → {}", dest.display()),
            Err(e) => format!(" · Spiegel fehlgeschlagen: {e}"),
        }
    }

    fn export_library_to_game(&mut self) {
        if self.library_is_game_stamps() {
            self.status = self.t("status.export_same_folder").into();
            return;
        }
        match library::export_packs_to_game(
            &self.library_root,
            &self.game_stamps_root,
            &self.library_packs,
        ) {
            Ok(n) => {
                self.status = self.tf(
                    "status.exported_to_game",
                    &[
                        ("n", &n.to_string()),
                        ("path", &self.game_stamps_root.display().to_string()),
                    ],
                );
            }
            Err(e) => {
                self.status = self.tf("status.export_failed", &[("err", &e)]);
            }
        }
    }

    fn use_safe_library(&mut self) {
        let safe = EditorConfig::default_root();
        let _ = std::fs::create_dir_all(&safe);
        match library::seed_library_from_game(&safe, &self.game_stamps_root) {
            Ok(n) => {
                self.library_root = safe;
                self.refresh_library();
                self.save_config();
                self.status = self.tf(
                    "status.safe_library",
                    &[
                        ("n", &n.to_string()),
                        ("path", &self.library_root.display().to_string()),
                    ],
                );
            }
            Err(e) => {
                self.status = self.tf("status.safe_library_failed", &[("err", &e)]);
            }
        }
    }

    fn t(&self, key: &str) -> &'static str {
        i18n::tr(self.language, key)
    }

    fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        i18n::tr_fmt(self.language, key, args)
    }

    /// Language-only helpers — avoid borrowing `self` inside `&self.open` scopes.
    fn t_lang(lang: Language, key: &str) -> &'static str {
        i18n::tr(lang, key)
    }

    fn tf_lang(lang: Language, key: &str, args: &[(&str, &str)]) -> String {
        i18n::tr_fmt(lang, key, args)
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
        self.road_erase_mode = false;
        self.field_brush_owner = None;
        self.field_brush_guid = None;
        self.hand_from_clipboard = false;
        self.drag_stroke = None;
        // Stale suppress from a prior stroke would eat the first place click.
        self.suppress_stroke_click = false;
        // Default: land footprint. Marsh (5×5 warehouse) via place_variation = 1 (Taste V).
        self.place_variation = None;
        self.place_direction = 0;
        self.refresh_place_status(label);
    }

    fn refresh_place_status(&mut self, label: &str) {
        let Some(guid) = self.placing_guid else {
            return;
        };
        let variation = self.place_variation;
        let dir = self.place_direction.rem_euclid(8);
        let mut size_hint = self
            .catalog
            .building(guid)
            .map(|d| {
                let s = d.size_for_variation(variation);
                let (ow, oh) = oriented_size(s[0], s[1], dir);
                let mut t = format!("{}×{}", ow as i32, oh as i32);
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
        size_hint.push_str(&format!(" · Dir {dir} · R drehen"));
        if self
            .catalog
            .building(guid)
            .is_some_and(|d| d.is_aqueduct_cistern())
        {
            let side = match CisternInlet::from_direction(dir) {
                CisternInlet::North => "N",
                CisternInlet::South => "S",
                CisternInlet::West => "W",
                CisternInlet::East => "O",
            };
            size_hint.push_str(&format!(" · Anschluss {side}"));
        }
        self.status = if size_hint.is_empty() {
            self.tf("status.hand_place", &[("label", label)])
        } else {
            self.tf(
                "status.hand_place_size",
                &[("label", label), ("size", &size_hint)],
            )
        };
    }

    /// Inlet cells + outward arrow for cisterns (placed or ghost).
    fn paint_cistern_inlet_hint(
        &self,
        painter: &egui::Painter,
        canvas: Rect,
        min_x: i32,
        max_x: i32,
        min_y: i32,
        max_y: i32,
        direction: i32,
    ) {
        let inlet = CisternInlet::from_direction(direction);
        // Only the center socket cell accepts aqueduct — side cells would cover neighbors.
        let mid_x = (min_x + max_x) / 2;
        let mid_y = (min_y + max_y) / 2;
        // Anno Y-up: north = +Y (max_y+1), south = -Y (min_y-1).
        let (ix, iy) = match inlet {
            CisternInlet::North => (mid_x, max_y + 1),
            CisternInlet::South => (mid_x, min_y - 1),
            CisternInlet::West => (min_x - 1, mid_y),
            CisternInlet::East => (max_x + 1, mid_y),
        };
        let inlet_color = Color32::from_rgb(40, 140, 220);
        let r = self.cell_screen_rect(canvas, ix, iy);
        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(40, 140, 220, 90));
        painter.rect_stroke(
            r,
            0.0,
            Stroke::new(2.0_f32, inlet_color),
            egui::StrokeKind::Inside,
        );
        // Arrow from footprint center toward inlet (clear while building is in hand).
        let foot = self.cells_screen_rect(canvas, min_x, min_y, max_x, max_y);
        let tip = match inlet {
            CisternInlet::North => Pos2::new(foot.center().x, foot.top() - self.cell_px * 0.35),
            CisternInlet::South => Pos2::new(foot.center().x, foot.bottom() + self.cell_px * 0.35),
            CisternInlet::West => Pos2::new(foot.left() - self.cell_px * 0.35, foot.center().y),
            CisternInlet::East => Pos2::new(foot.right() + self.cell_px * 0.35, foot.center().y),
        };
        let base = foot.center();
        let dir_v = (tip - base).normalized();
        let ortho = Vec2::new(-dir_v.y, dir_v.x);
        let len = self.cell_px.max(10.0) * 0.85;
        let tip_p = base + dir_v * len;
        let wing = self.cell_px.max(8.0) * 0.28;
        let p0 = tip_p;
        let p1 = tip_p - dir_v * wing * 1.6 + ortho * wing;
        let p2 = tip_p - dir_v * wing * 1.6 - ortho * wing;
        painter.line_segment([base, tip_p], Stroke::new(2.5_f32, inlet_color));
        painter.add(egui::Shape::convex_polygon(
            vec![p0, p1, p2],
            inlet_color,
            Stroke::NONE,
        ));
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
                self.catalog.label(guid, self.language)
            );
            return;
        }
        self.place_variation = match self.place_variation {
            Some(v) if v >= 1 => None,
            _ => Some(1),
        };
        let label = self.catalog.label(guid, self.language);
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
                self.catalog.label(guid, self.language)
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
                    self.catalog.label(guid, self.language),
                    w as i32,
                    h as i32
                );
            }
            Err(e) => self.status = format!("Variation fehlgeschlagen: {e}"),
        }
    }

    fn is_street_tool(&self, guid: u32) -> bool {
        let g = guid as i32;
        is_road_brush_guid(g)
            || is_aqueduct_brush_guid(g)
            || self.catalog.building(guid).is_some_and(|d| {
                d.is_aqueduct_connector()
                    || d.category == "street"
                    || d.road_kind.is_some()
            })
    }

    fn activate_menu_building(&mut self, guid: u32, label: &str) {
        let g = guid as i32;
        if self.is_street_tool(guid) {
            self.field_brush_owner = None;
            self.field_brush_guid = None;
            self.road_brush_guid = Some(g);
            self.road_erase_mode = false;
            self.placing_guid = None;
            self.place_variation = None;
            self.drag_stroke = None;
            self.suppress_stroke_click = false;
            self.residence_tier_guids = None;
            // Dirt/stone: collapse the infra category. Aqueduct stays open so
            // Basin / Segment / Cistern remain clickable in the mid-row.
            if is_road_brush_guid(g) && !is_aqueduct_brush_guid(g) {
                self.buildmenu_infra_entry = None;
                self.buildmenu_entry = None;
            }
            self.status = self.tf("status.road_brush", &[("label", &label)]);
            return;
        }
        self.road_erase_mode = false;
        self.road_brush_guid = None;
        // Open tier mid-row when placing a residence (or picking a tier from it).
        match self.catalog.building(guid) {
            Some(def) if def.is_residence() => {
                let chain = self.catalog.residence_chain(guid);
                self.residence_tier_guids = if chain.len() > 1 {
                    Some(chain)
                } else {
                    None
                };
            }
            _ => {
                self.residence_tier_guids = None;
            }
        }
        self.pick_place_guid(guid, label);
    }

    fn upgrade_selected_residence(&mut self, new_guid: u32) {
        let Some(sel) = self.selected_building else {
            return;
        };
        self.push_history();
        let label = self.catalog.label(new_guid, self.language);
        if let Some(open) = self.open.as_mut() {
            if open.stamp.set_building_guid(sel, new_guid).is_ok() {
                open.dirty = true;
                self.schedule_autosave();
                self.status = self.tf(
                    "status.residence_tier",
                    &[("label", &label)],
                );
                self.refresh_coverage();
            }
        }
    }

    fn activate_road_eraser(&mut self) {
        self.road_erase_mode = true;
        self.road_brush_guid = None;
        self.field_brush_owner = None;
        self.field_brush_guid = None;
        self.placing_guid = None;
        self.place_variation = None;
        self.module_parent_index = None;
        self.drag_stroke = None;
        self.buildmenu_infra_entry = None;
        self.residence_tier_guids = None;
        self.status =
            self.t("status.demolish").into();
    }

    fn activate_field_brush(&mut self, owner: u64, field_guid: u32, label: &str) {
        self.field_brush_owner = Some(owner);
        self.field_brush_guid = Some(field_guid);
        self.placing_guid = None;
        self.place_variation = None;
        self.module_parent_index = None;
        self.road_brush_guid = None;
        self.road_erase_mode = false;
        self.hand_from_clipboard = false;
        self.drag_stroke = None;
        self.status = self.tf("status.field_paint", &[("label", label)]);
    }

    fn road_tool_active(&self) -> bool {
        self.road_brush_guid.is_some() || self.road_erase_mode
    }

    fn field_tool_active(&self) -> bool {
        self.field_brush_owner.is_some() && self.field_brush_guid.is_some()
    }

    fn hand_busy(&self) -> bool {
        self.placing_guid.is_some() || self.road_tool_active() || self.field_tool_active()
    }

    fn open_radial(&mut self, origin: Pos2) {
        self.radial = Some(RadialMenu {
            origin,
            hover: None,
            ignore_next_click: true,
        });
        self.status =
            self.t("status.radial_open").into();
    }

    /// Close the wheel without picking a slot → drop whatever is in hand.
    fn dismiss_radial(&mut self) {
        self.radial = None;
        if self.hand_busy() {
            self.clear_hand();
            self.status = self.t("status.hand_free").into();
        } else {
            self.status = self.t("status.radial_closed").into();
        }
    }

    fn copy_selection_to_clipboard(&mut self) -> bool {
        let Some(open) = self.open.as_ref() else {
            self.status = self.t("status.nothing_to_copy").into();
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
                let label = self.catalog.label(guid, self.language);
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
        let (ptx, pty) = self.footprint_tl_building(&primary);

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
            let (mx0, my0) = blocker_min_for(
                m.guid,
                w0,
                h0,
                catalog.building(m.guid),
                m.variation,
            );
            let (mx, my, _, _) = oriented_blocker(mx0, my0, w0, h0, m.direction);
            let (tx, ty) = footprint_top_left_blocker(m.x, m.y, mx, my, drag);
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
        let label = self.catalog.label(clip[0].guid, self.language);
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

    /// After pipette click/marquee: clipboard → hand and leave pipette mode.
    fn finish_copy_pipette(&mut self) {
        if self.copy_selection_to_clipboard() {
            self.copy_pipette_mode = false;
            self.arm_clipboard_in_hand();
        } else if !self.clipboard_roads.is_empty() {
            // Roads-only copy still arms a hand if we have a primary building — otherwise keep mode.
            self.copy_pipette_mode = false;
            self.arm_clipboard_in_hand();
        }
    }

    /// Put clipboard primary into the placement hand (ghost follows cursor; click to set).
    fn arm_clipboard_in_hand(&mut self) {
        let Some(p) = self.clipboard.iter().find(|c| c.is_primary).cloned() else {
            self.status = self.t("status.clipboard_empty_copy").into();
            return;
        };
        self.road_erase_mode = false;
        self.road_brush_guid = None;
        self.drag_stroke = None;
        self.module_parent_index = None;
        self.placing_guid = Some(p.guid);
        self.place_variation = p.variation;
        self.place_direction = p.direction;
        self.palette_selected_guid = Some(p.guid);
        self.hand_from_clipboard = true;
        self.residence_tier_guids = self
            .catalog
            .building(p.guid)
            .filter(|d| d.is_residence())
            .map(|d| self.catalog.residence_chain(d.guid))
            .filter(|c| c.len() > 1);
        let label = self.catalog.label(p.guid, self.language);
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
            && self.selected_fields.is_empty()
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

    /// Primary footprint top-left implied by a snapped primary Pos.
    fn clipboard_primary_top_left(&self, primary_cx: f32, primary_cy: f32) -> (i32, i32) {
        let Some(p) = self.clipboard.iter().find(|c| c.is_primary) else {
            return (primary_cx.floor() as i32, primary_cy.floor() as i32);
        };
        let (mx, my, _, _) =
            self.building_oriented_blocker(p.guid, p.variation, p.direction);
        footprint_top_left_blocker(primary_cx, primary_cy, mx, my, self.stamp_drag())
    }

    /// Resolve clipboard buildings to pipette Pos from a snapped primary Pos.
    fn clipboard_building_centers(&self, primary_cx: f32, primary_cy: f32) -> Vec<(ClipboardBuilding, f32, f32)> {
        let (ptx, pty) = self.clipboard_primary_top_left(primary_cx, primary_cy);
        self.clipboard
            .iter()
            .cloned()
            .map(|c| {
                let (cx, cy) = self.place_pos_for_tl(
                    ptx + c.dtx,
                    pty + c.dty,
                    c.guid,
                    c.variation,
                    c.direction,
                );
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
        let mut occupied = self.occupied_footprint_cells(&[]);
        for (c, cx, cy) in &placed {
            if self
                .catalog
                .building(c.guid)
                .is_some_and(|d| d.road_kind.is_some() || d.category == "street")
            {
                continue;
            }
            let cells =
                self.trial_footprint_cells(c.guid, *cx, *cy, c.direction, c.variation);
            if cells.iter().any(|t| occupied.contains(t)) {
                return None;
            }
            for t in cells {
                occupied.insert(t);
            }
        }
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
                self.pan.y / self.cell_px, // matches Y-up pan from frame_on_content
            )
        });
        if let Some(p) = self.clipboard.iter().find(|c| c.is_primary) {
            let (bw, bh) = self.clipboard_oriented_size(p);
            return self.snap_place_pos(
                wx,
                wy,
                p.guid,
                p.variation,
                p.direction,
                bw,
                bh,
            );
        }
        (wx.round(), wy.round())
    }

    /// Instant paste at cursor (Ctrl+Shift+V). Normal Ctrl+V only arms the hand.
    fn paste_clipboard_at_cursor(&mut self) {
        if self.clipboard.is_empty() {
            self.status = self.t("status.clipboard_empty_click").into();
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
                let dirt = road_guid_for_region(
                    stamp_editor::buildmenu::ConstructionMenu::region_key(
                        self.region_filter,
                    ),
                    false,
                );
                self.activate_menu_building(dirt as u32, self.t("radial.road_dirt"));
            }
            RadialAction::RoadStone => {
                let stone = road_guid_for_region(
                    stamp_editor::buildmenu::ConstructionMenu::region_key(
                        self.region_filter,
                    ),
                    true,
                );
                self.activate_menu_building(stone as u32, self.t("radial.road_stone"));
            }
            RadialAction::Demolish => {
                self.activate_road_eraser();
            }
            RadialAction::Copy => {
                self.clear_hand_silent();
                // Already have a selection → copy straight into the hand.
                if !self.selection_indices().is_empty()
                    || !self.selected_roads.is_empty()
                    || self.hover_building.is_some()
                {
                    if self.copy_selection_to_clipboard() {
                        self.arm_clipboard_in_hand();
                    }
                } else {
                    self.copy_pipette_mode = true;
                    self.status = self.t("status.copy_pipette").into();
                }
            }
            RadialAction::Move => {
                self.clear_hand();
                self.status = self.t("status.move_mode").into();
            }
            RadialAction::ClearHand => {
                self.clear_hand();
                self.status = self.t("status.hand_free").into();
            }
        }
    }

    /// Drop active tool / hand state without touching the status line.
    fn clear_hand_silent(&mut self) {
        self.placing_guid = None;
        self.place_variation = None;
        self.module_parent_index = None;
        self.road_brush_guid = None;
        self.road_erase_mode = false;
        self.copy_pipette_mode = false;
        self.field_brush_owner = None;
        self.field_brush_guid = None;
        self.hand_from_clipboard = false;
        self.drag_stroke = None;
        self.suppress_stroke_click = false;
        // Drop infra tile highlight with the tool (was stuck on after RMB).
        self.buildmenu_infra_entry = None;
        self.buildmenu_entry = None;
        self.residence_tier_guids = None;
    }

    fn clear_hand(&mut self) {
        self.clear_hand_silent();
        self.status = self.t("status.hand_cleared").into();
    }

    fn clear_canvas_selection(&mut self) {
        self.selected_building = None;
        self.selected_buildings.clear();
        self.selected_roads.clear();
        self.selected_fields.clear();
        self.dragging_fields = false;
        self.drag_field_origins.clear();
        self.drag_field_delta = (0, 0);
        self.drag_field_anchor = (0, 0);
        // Empty hand + deselect → dismiss pinned service window.
        if !self.hand_busy() {
            self.residence_needs_pin = None;
        }
    }

    fn select_only(&mut self, idx: usize) {
        self.selected_buildings.clear();
        self.selected_buildings.insert(idx);
        self.selected_building = Some(idx);
        self.selected_roads.clear();
        self.selected_fields.clear();
        let is_house = self
            .open
            .as_ref()
            .and_then(|o| o.stamp.buildings.get(idx))
            .and_then(|b| self.catalog.building(b.guid))
            .is_some_and(|d| d.is_residence());
        if is_house {
            self.residence_needs_pin = Some(idx);
        } else if !self.hand_busy() {
            // Empty hand selecting a non-house → close service pin.
            self.residence_needs_pin = None;
        }
    }

    fn set_canvas_selection(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.selected_buildings.clear();
        self.selected_buildings.extend(indices);
        self.selected_building = self.selected_buildings.iter().copied().min();
    }

    fn set_canvas_selection_full(
        &mut self,
        indices: impl IntoIterator<Item = usize>,
        roads: impl IntoIterator<Item = (i32, i32)>,
        fields: impl IntoIterator<Item = (i32, i32)>,
    ) {
        self.set_canvas_selection(indices);
        self.selected_roads.clear();
        self.selected_roads.extend(roads);
        self.selected_fields.clear();
        self.selected_fields.extend(fields);
    }

    fn select_fields_only(&mut self, fields: impl IntoIterator<Item = (i32, i32)>) {
        self.selected_buildings.clear();
        self.selected_building = None;
        self.selected_roads.clear();
        self.selected_fields.clear();
        self.selected_fields.extend(fields);
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
        } else if self.field_tool_active() {
            let owner = self.field_brush_owner;
            let already = self.open.as_ref().is_some_and(|o| {
                o.stamp.polygon_cells.iter().any(|c| {
                    c.complex_owner_id == owner && c.x == tx && c.y == ty
                })
            });
            StrokeKind::Field { paint_on: !already }
        } else if self.road_brush_guid.is_some() {
            // Always paint/extend. Starting on an existing road of the same type must
            // NOT toggle erase — that's what Abriss / road eraser is for. Same cells
            // are skipped inside apply_road_path; other types are overwritten (Dirt→Stone).
            StrokeKind::Road { paint_on: true }
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
            StrokeKind::Field { paint_on } => {
                let Some(owner) = self.field_brush_owner else {
                    return;
                };
                let Some(field_guid) = self.field_brush_guid else {
                    return;
                };
                // In-game style: filled axis-aligned rect; blocked tiles are skipped.
                let (min_x, min_y, max_x, max_y) = rect_bounds(stroke.start, stroke.cursor);
                let area = rect_cells(min_x, min_y, max_x, max_y);
                let seeds = self.farm_main_connect_seeds(owner);
                let mut changed = 0usize;
                if let Some(open_mut) = self.open.as_mut() {
                    let mut blocked: HashSet<(i32, i32)> =
                        stamp_editor::meshgraph::mesh_road_cells(&open_mut.stamp.document)
                            .into_iter()
                            .map(|(x, y, _)| (x, y))
                            .collect();
                    let region = open_mut.region_guid;
                    let drag = read_mesh_info(&open_mut.stamp.document)
                        .map(|i| i.drag_center)
                        .unwrap_or((0.0, 0.0));
                    for b in &open_mut.stamp.buildings {
                        let def = self.catalog.building(b.guid);
                        if def.is_some_and(|d| d.category == "street") {
                            continue;
                        }
                        for c in footprint_cells_for_region(b, def, region, drag) {
                            blocked.insert((c.x, c.y));
                        }
                    }
                    changed = apply_field_tiles(
                        &mut open_mut.stamp,
                        owner,
                        field_guid,
                        &area,
                        paint_on,
                        &blocked,
                        Some(&seeds),
                    );
                    if changed > 0 {
                        open_mut.stamp.refresh_buildings();
                        open_mut.dirty = true;
                        self.schedule_autosave();
                    }
                }
                self.refresh_coverage();
                let verb = if paint_on { "gesetzt" } else { "entfernt" };
                let w = max_x - min_x + 1;
                let h = max_y - min_y + 1;
                self.status = format!(
                    "Feld {verb}: {changed}/{n} frei in {w}×{h}",
                    n = area.len()
                );
            }
            StrokeKind::SelectRect => {
                let (min_x, min_y, max_x, max_y) = rect_bounds(stroke.start, stroke.cursor);
                let mut hits = Vec::new();
                let mut road_hits = Vec::new();
                let mut field_hits = Vec::new();
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
                    // Individual PlantFarm tiles — not the whole farm.
                    for c in &open_ref.stamp.polygon_cells {
                        if c.x >= min_x && c.x <= max_x && c.y >= min_y && c.y <= max_y {
                            field_hits.push((c.x, c.y));
                        }
                    }
                    field_hits.sort_unstable();
                    field_hits.dedup();
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
                let n_fields = field_hits.len();
                self.set_canvas_selection_full(hits, road_hits, field_hits);
                self.refresh_coverage();
                if self.copy_pipette_mode {
                    if n > 0 || n_roads > 0 {
                        self.finish_copy_pipette();
                    } else {
                        self.status = self.t("status.copy_pipette_miss").into();
                    }
                } else {
                    self.status = match (n == 0, n_roads == 0, n_fields == 0) {
                        (true, true, true) => self.t("status.sel_empty").into(),
                        (false, true, true) => {
                            self.tf("status.sel_buildings", &[("n", &n.to_string())])
                        }
                        (true, false, true) => {
                            self.tf("status.sel_roads", &[("n", &n_roads.to_string())])
                        }
                        (true, true, false) => {
                            self.tf("status.sel_fields", &[("n", &n_fields.to_string())])
                        }
                        _ => self.tf(
                            "status.sel_mixed",
                            &[
                                ("b", &n.to_string()),
                                ("r", &n_roads.to_string()),
                                ("f", &n_fields.to_string()),
                            ],
                        ),
                    };
                }
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
                let place_dir = self.place_direction;
                let (bw, bh) = if from_clipboard {
                    self.clipboard
                        .iter()
                        .find(|c| c.is_primary)
                        .map(|p| self.clipboard_oriented_size(p))
                        .unwrap_or_else(|| {
                            let (w0, h0) = self.building_size(guid, variation);
                            oriented_size(w0, h0, place_dir)
                        })
                } else {
                    let (w0, h0) = self.building_size(guid, variation);
                    oriented_size(w0, h0, place_dir)
                };
                let bw_i = bw.round().max(1.0) as i32;
                let bh_i = bh.round().max(1.0) as i32;
                // Clipboard groups step by the whole group AABB so a marquee the size of
                // the selection places one copy — not a broken primary-sized grid.
                let (step_w, step_h) = if from_clipboard {
                    self.clipboard_group_span()
                } else {
                    (bw_i, bh_i)
                };
                let origins =
                    place_rect_top_lefts(stroke.start, stroke.cursor, step_w, step_h);
                let mut centers = Vec::with_capacity(origins.len());
                for (gx, gy) in origins {
                    if from_clipboard {
                        if let Some(p) = self.clipboard.iter().find(|c| c.is_primary) {
                            centers.push(self.place_pos_for_tl(
                                gx,
                                gy,
                                p.guid,
                                p.variation,
                                p.direction,
                            ));
                        } else {
                            centers.push(place_center_from_top_left(gx, gy, bw_i, bh_i));
                        }
                    } else {
                        centers.push(self.place_pos_for_tl(
                            gx, gy, guid, variation, place_dir,
                        ));
                    }
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
                    let label = self.catalog.label(guid, self.language);
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

                let parent_idx = self.module_parent_index;
                let mut occupied = self.occupied_footprint_cells(&[]);
                let place_drag = self.stamp_drag();
                let place_region = self.open.as_ref().map(|o| o.region_guid).unwrap_or(0);
                let place_def = self.catalog.building(guid);
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
                        let trial = Building {
                            guid,
                            x: sx,
                            y: sy,
                            direction: place_dir,
                            complex_owner_id: None,
                            variation,
                        };
                        let cells: Vec<(i32, i32)> = footprint_cells_for_region(
                            &trial,
                            place_def,
                            place_region,
                            place_drag,
                        )
                        .into_iter()
                        .map(|c| (c.x, c.y))
                        .collect();
                        if cells.iter().any(|c| occupied.contains(c)) {
                            blocked += 1;
                            continue;
                        }
                        match open_mut.stamp.add_building_from_template(
                            guid, sx, sy, place_dir, variation,
                        ) {
                            Ok(()) => {
                                let new_idx = open_mut.stamp.buildings.len() - 1;
                                if let Some(pi) = parent_idx {
                                    let oid = open_mut.stamp.buildings[pi]
                                        .complex_owner_id
                                        .filter(|o| *o != 0)
                                        .unwrap_or_else(|| {
                                            next_complex_owner_id(
                                                &open_mut.stamp.buildings,
                                                &open_mut.stamp.polygon_cells,
                                            )
                                        });
                                    let _ = open_mut.stamp.set_building_owner(pi, oid);
                                    let _ = open_mut.stamp.set_building_owner(new_idx, oid);
                                } else if needed_modules(place_def) > 0 {
                                    // Plant/Animal farms need a ComplexOwnerID so field
                                    // painting / Untergebäude overlay can attach modules.
                                    let oid = next_complex_owner_id(
                                        &open_mut.stamp.buildings,
                                        &open_mut.stamp.polygon_cells,
                                    );
                                    let _ = open_mut.stamp.set_building_owner(new_idx, oid);
                                }
                                for c in cells {
                                    occupied.insert(c);
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
                let label = self.catalog.label(guid, self.language);
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

    fn bonus_attr_short(&self, attr: &str) -> String {
        let key = format!("buff.{attr}");
        let translated = self.t(&key);
        if translated != "???" {
            translated.to_string()
        } else {
            attr.to_string()
        }
    }

    fn format_bonus_chip(&self, attr: &str, value: f32) -> String {
        if attr == "ProvidedNeed" {
            return self.t("buff.need_ok").to_string();
        }
        let name = self.bonus_attr_short(attr);
        if value.fract().abs() < 0.05 {
            format!("{name} {value:+.0}")
        } else {
            format!("{name} {value:+.1}")
        }
    }

    /// Compact incoming-buff summary: aggregated attrs + source count (no per-source spam).
    fn format_incoming_summary(&self, hits: &[stamp_editor::coverage::EffectHit]) -> Vec<String> {
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
            .map(|(a, v)| self.format_bonus_chip(a, *v))
            .collect();
        if need_count > 0 {
            lines.push(self.tf("buff.needs_ok", &[("n", &need_count.to_string())]));
        }
        if !hits.is_empty() {
            lines.push(self.tf("buff.sources", &[("n", &hits.len().to_string())]));
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
    /// Residences use the population portrait + tier digit (shared house icon is useless).
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
        let def = self.catalog.building(guid);
        let icon_path = def
            .and_then(|d| {
                if d.is_residence() {
                    d.residence_display_icon().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .or_else(|| {
                self.buildmenu
                    .building_icon(guid)
                    .map(|s| s.to_string())
            })
            .or_else(|| def.and_then(|d| d.icon.clone()));
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
        if let Some(tag) = self.catalog.residence_tier_tag(guid) {
            // Show from fairly zoomed-out; "T1" needs a pill, not a tiny circle.
            if min_side >= 12.0 {
                let font_sz = (min_side * 0.22).clamp(9.0, 13.0);
                let galley = painter.layout_no_wrap(
                    tag,
                    egui::FontId::proportional(font_sz),
                    Color32::from_rgba_unmultiplied(250, 245, 230, a),
                );
                let pad = Vec2::new(4.0, 2.0);
                let bw = galley.size().x + pad.x * 2.0;
                let bh = galley.size().y + pad.y * 2.0;
                let bg = Rect::from_min_size(
                    Pos2::new(outline.right() - bw - 2.0, outline.bottom() - bh - 2.0),
                    Vec2::new(bw, bh),
                );
                painter.rect_filled(
                    bg,
                    3.0,
                    Color32::from_rgba_unmultiplied(20, 16, 10, a),
                );
                painter.rect_stroke(
                    bg,
                    3.0,
                    Stroke::new(
                        1.2_f32,
                        Color32::from_rgba_unmultiplied(220, 185, 110, a),
                    ),
                    egui::StrokeKind::Outside,
                );
                painter.galley(
                    Pos2::new(bg.min.x + pad.x, bg.min.y + pad.y),
                    galley,
                    Color32::from_rgba_unmultiplied(250, 245, 230, a),
                );
            }
        }
        true
    }

    /// Footprint fill so T1…T4 residences stay distinct when zoomed out.
    fn residence_tier_fill(tier: u8, selected: bool, in_cov: bool) -> Color32 {
        if selected {
            return Color32::from_rgb(90, 150, 220);
        }
        if in_cov {
            return Color32::from_rgb(60, 110, 150);
        }
        match tier {
            1 => Color32::from_rgb(72, 118, 78),   // T1 Liberti / Wanderer
            2 => Color32::from_rgb(88, 108, 150),  // T2 Plebejer / …
            3 => Color32::from_rgb(130, 95, 145),  // T3 Equites / …
            _ => Color32::from_rgb(160, 120, 70),  // T4+ Patrizier
        }
    }

    /// Footprint tiles that block placing/moving a building.
    /// Includes other buildings plus MeshGraph roads/aqueducts (not street BuildingInfo —
    /// those are already in the mesh).
    fn occupied_footprint_cells(&self, exclude: &[usize]) -> HashSet<(i32, i32)> {
        let mut out = HashSet::new();
        let Some(open) = self.open.as_ref() else {
            return out;
        };
        let drag = self.stamp_drag();
        let region = open.region_guid;
        let skip: HashSet<usize> = exclude.iter().copied().collect();
        for (i, b) in open.stamp.buildings.iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            if self.catalog.building(b.guid).is_some_and(|d| {
                d.road_kind.is_some() || d.category == "street"
            }) {
                continue;
            }
            for c in footprint_cells_for_region(
                b,
                self.catalog.building(b.guid),
                region,
                drag,
            ) {
                out.insert((c.x, c.y));
            }
        }
        for (x, y, _) in mesh_road_cells(&open.stamp.document) {
            out.insert((x, y));
        }
        out
    }

    fn trial_footprint_cells(
        &self,
        guid: u32,
        x: f32,
        y: f32,
        direction: i32,
        variation: Option<u32>,
    ) -> Vec<(i32, i32)> {
        let b = Building {
            guid,
            x,
            y,
            direction,
            complex_owner_id: None,
            variation,
        };
        let region = self.open.as_ref().map(|o| o.region_guid).unwrap_or(0);
        footprint_cells_for_region(
            &b,
            self.catalog.building(guid),
            region,
            self.stamp_drag(),
        )
        .into_iter()
        .map(|c| (c.x, c.y))
        .collect()
    }

    fn footprint_overlaps_occupied(
        &self,
        guid: u32,
        x: f32,
        y: f32,
        direction: i32,
        variation: Option<u32>,
        occupied: &HashSet<(i32, i32)>,
    ) -> bool {
        self.trial_footprint_cells(guid, x, y, direction, variation)
            .iter()
            .any(|c| occupied.contains(c))
    }

    /// Icon path for a menu entry (entry icon, else building_icons map).
    fn resolve_menu_icon(&self, entry: &MenuEntry) -> Option<String> {
        if let Some(i) = entry.icon().filter(|s| !s.is_empty()) {
            return Some(i.to_string());
        }
        match entry {
            MenuEntry::Building { guid, .. } => self
                .buildmenu
                .building_icon(*guid)
                .map(|s| s.to_string()),
            MenuEntry::Chain { buildings, .. } => buildings
                .first()
                .and_then(|g| self.buildmenu.building_icon(*g))
                .map(|s| s.to_string()),
            MenuEntry::Category { entries, .. } => {
                entries.iter().find_map(|e| self.resolve_menu_icon(e))
            }
        }
    }

    /// Circular goods / chain slot — drawn geometrically (chrome PNGs are off-center).
    /// Optional `overlay` (e.g. tier "1"…"5") is painted centered on the icon.
    fn goods_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        game_icon: Option<&str>,
        selected: bool,
        tip: &str,
        overlay: Option<&str>,
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
            // Keep the caret inside the tile — drawing above the rect bled into the
            // row above / canvas and looked like a shifted second strip.
            let tip_p = Pos2::new(c.x, rect.top() + 6.0);
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
        if let Some(text) = overlay {
            // Bottom-right badge so population portraits stay readable.
            let badge_c = Pos2::new(rect.right() - 11.0, rect.bottom() - 11.0);
            ui.painter().circle_filled(
                badge_c,
                10.0,
                Color32::from_rgb(28, 22, 14),
            );
            ui.painter().circle_stroke(
                badge_c,
                10.0,
                Stroke::new(1.25_f32, Color32::from_rgb(220, 185, 110)),
            );
            ui.painter().text(
                badge_c,
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(14.0),
                Color32::from_rgb(250, 245, 230),
            );
        }
        resp.on_hover_text(tip)
    }

    /// T1 residence root for the active culture filter (empty `base_guids`).
    fn default_residence_root(&self, region: RegionFilter) -> Option<u32> {
        if matches!(region, RegionFilter::All) {
            return None;
        }
        let mut roots: Vec<&stamp_editor::catalog::BuildingDef> = self
            .catalog
            .buildings_iter()
            .filter(|b| {
                b.is_residence()
                    && b.base_guids.is_empty()
                    && region.matches_building(b)
            })
            .collect();
        // Prefer culture-pure roots (single region tag) over hybrids.
        roots.sort_by_key(|b| (b.regions.len(), b.guid));
        roots.first().map(|b| b.guid)
    }

    /// When the region filter changes while a residence tool is active, swap the
    /// mid-row chain (+ hand) to that culture's house tiers.
    fn refresh_residence_tiers_for_region(&mut self) {
        let placing_residence = self
            .placing_guid
            .and_then(|g| self.catalog.building(g))
            .is_some_and(|d| d.is_residence());
        if self.residence_tier_guids.is_none() && !placing_residence {
            return;
        }
        let old_idx = self
            .placing_guid
            .and_then(|g| {
                self.residence_tier_guids
                    .as_ref()
                    .and_then(|c| c.iter().position(|&x| x == g))
            })
            .unwrap_or(0);
        let Some(root) = self.default_residence_root(self.region_filter) else {
            self.residence_tier_guids = None;
            if placing_residence {
                self.placing_guid = None;
                self.place_variation = None;
                self.palette_selected_guid = None;
            }
            return;
        };
        let chain = self.catalog.residence_chain(root);
        let new_guid = chain
            .get(old_idx)
            .copied()
            .or_else(|| chain.first().copied())
            .unwrap_or(root);
        self.residence_tier_guids = if chain.len() > 1 {
            Some(chain)
        } else {
            None
        };
        let label = self.catalog.label(new_guid, self.language);
        self.pick_place_guid(new_guid, &label);
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

    /// Snap mouse → pipette Pos (cursor stays in the middle of the footprint).
    fn snap_place_center(&self, wx: f32, wy: f32, bw: f32, bh: f32) -> (f32, f32) {
        // Legacy path (unknown GUID): assume centered blocker.
        self.snap_place_pos(wx, wy, 0, None, 0, bw, bh)
    }

    /// `wx,wy` = geometric footprint center under the cursor (place / ghost).
    fn snap_place_pos(
        &self,
        wx: f32,
        wy: f32,
        guid: u32,
        variation: Option<u32>,
        direction: i32,
        bw: f32,
        bh: f32,
    ) -> (f32, f32) {
        if self.allow_45_deg {
            return self.snap_world(wx, wy);
        }
        let (tx, ty) = self.place_top_left_from_mouse(wx, wy, bw, bh);
        if guid != 0 {
            self.place_pos_for_tl(tx, ty, guid, variation, direction)
        } else {
            let bw_i = bw.round().max(1.0) as i32;
            let bh_i = bh.round().max(1.0) as i32;
            place_center_from_top_left(tx, ty, bw_i, bh_i)
        }
    }

    /// Snap an existing pipette `Pos` onto the tile grid without shifting decentered pivots.
    fn snap_existing_pos(
        &self,
        px: f32,
        py: f32,
        guid: u32,
        variation: Option<u32>,
        direction: i32,
    ) -> (f32, f32) {
        if self.allow_45_deg {
            return self.snap_world(px, py);
        }
        let (mx, my, w, h) = self.building_oriented_blocker(guid, variation, direction);
        let (tx, ty) = footprint_top_left_blocker(px, py, mx, my, self.stamp_drag());
        place_pos_from_top_left(tx, ty, w, h, mx, my)
    }

    /// Tile under the cursor for the current tool (place = footprint anchor, else cell).
    fn stroke_anchor_tile(&self, wx: f32, wy: f32) -> (i32, i32) {
        if let Some(guid) = self.placing_guid {
            if !self.road_erase_mode && self.road_brush_guid.is_none() {
                let (w0, h0) = self.building_size(guid, self.place_variation);
                let (bw, bh) = oriented_size(w0, h0, self.place_direction);
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
        let cov_key = (_primary_idx, sx.to_bits(), sy.to_bits());
        if self.drag_cov_key != Some(cov_key) {
            self.drag_cov_key = Some(cov_key);
            self.analysis_gen = self.analysis_gen.wrapping_add(1);
            *self.place_cov_roads.borrow_mut() = None;
            *self.place_ghost_cov.borrow_mut() = None;
            self.city_analysis = None;
            self.area_analysis = None;
            self.needs_cov_cache = None;
            self.refresh_coverage();
        }
        let n = self.drag_group_origins.len();
        let nr = self.drag_group_roads.len();
        if n > 1 || nr > 0 {
            self.status = format!("Auswahl verschoben ({n} Gebäude, {nr} Wege)");
        }
    }

    fn field_cell_meta(&self, tx: i32, ty: i32) -> Option<(u64, u32)> {
        let open = self.open.as_ref()?;
        open.stamp
            .polygon_cells
            .iter()
            .find(|c| c.x == tx && c.y == ty)
            .and_then(|c| {
                let owner = c.complex_owner_id.filter(|o| *o != 0)?;
                Some((owner, c.guid))
            })
    }

    /// Farm main footprint tiles — fields must stay 4-connected to these.
    fn farm_main_connect_seeds(&self, owner: u64) -> HashSet<(i32, i32)> {
        let mut seeds = HashSet::new();
        let Some(open) = self.open.as_ref() else {
            return seeds;
        };
        let region = open.region_guid;
        let drag = read_mesh_info(&open.stamp.document)
            .map(|i| i.drag_center)
            .unwrap_or((0.0, 0.0));
        for (i, b) in open.stamp.buildings.iter().enumerate() {
            if b.complex_owner_id != Some(owner) {
                continue;
            }
            if complex_main_index(&open.stamp.buildings, &self.catalog, i) != Some(i) {
                continue;
            }
            let def = self.catalog.building(b.guid);
            for c in footprint_cells_for_region(b, def, region, drag) {
                seeds.insert((c.x, c.y));
            }
        }
        seeds
    }

    fn erase_field_cells(&mut self, cells: &[(i32, i32)]) -> usize {
        if cells.is_empty() {
            return 0;
        }
        // Group by (owner, guid) using current polygon_cells metadata.
        let mut groups: HashMap<(u64, u32), Vec<(i32, i32)>> = HashMap::new();
        let metas: Vec<_> = cells
            .iter()
            .filter_map(|&(x, y)| self.field_cell_meta(x, y).map(|m| (x, y, m.0, m.1)))
            .collect();
        for (x, y, owner, guid) in metas {
            groups.entry((owner, guid)).or_default().push((x, y));
        }
        let empty = HashSet::new();
        let mut n = 0usize;
        for ((owner, guid), list) in &groups {
            let seeds = self.farm_main_connect_seeds(*owner);
            let Some(open) = self.open.as_mut() else {
                break;
            };
            n += apply_field_tiles(
                &mut open.stamp,
                *owner,
                *guid,
                list,
                false,
                &empty,
                Some(&seeds),
            );
        }
        if n > 0 {
            if let Some(open) = self.open.as_mut() {
                open.stamp.refresh_buildings();
            }
        }
        n
    }

    fn paint_field_cells(&mut self, cells: &[(i32, i32, u64, u32)]) -> usize {
        if cells.is_empty() {
            return 0;
        }
        let mut groups: HashMap<(u64, u32), Vec<(i32, i32)>> = HashMap::new();
        for &(x, y, owner, guid) in cells {
            groups.entry((owner, guid)).or_default().push((x, y));
        }
        let mut n = 0usize;
        for ((owner, guid), list) in &groups {
            let seeds = self.farm_main_connect_seeds(*owner);
            let Some(open) = self.open.as_mut() else {
                break;
            };
            let mut blocked: HashSet<(i32, i32)> =
                stamp_editor::meshgraph::mesh_road_cells(&open.stamp.document)
                    .into_iter()
                    .map(|(x, y, _)| (x, y))
                    .collect();
            let region = open.region_guid;
            let drag = read_mesh_info(&open.stamp.document)
                .map(|i| i.drag_center)
                .unwrap_or((0.0, 0.0));
            for b in &open.stamp.buildings {
                let def = self.catalog.building(b.guid);
                if def.is_some_and(|d| d.category == "street") {
                    continue;
                }
                for c in footprint_cells_for_region(b, def, region, drag) {
                    blocked.insert((c.x, c.y));
                }
            }
            n += apply_field_tiles(
                &mut open.stamp,
                *owner,
                *guid,
                list,
                true,
                &blocked,
                Some(&seeds),
            );
        }
        if n > 0 {
            if let Some(open) = self.open.as_mut() {
                open.stamp.refresh_buildings();
            }
        }
        n
    }

    fn begin_field_drag(&mut self, anchor: (i32, i32)) {
        self.drag_field_origins = self.selected_fields.iter().copied().collect();
        self.drag_field_origins.sort_unstable();
        self.drag_field_delta = (0, 0);
        self.drag_field_anchor = anchor;
        self.dragging_fields = true;
        self.dragging_building = None;
        self.drag_stroke = None;
        self.clear_selection_drag();
        self.history_drag_pushed = false;
    }

    fn commit_field_drag(&mut self) {
        if !self.dragging_fields {
            return;
        }
        let (dx, dy) = self.drag_field_delta;
        let origins = std::mem::take(&mut self.drag_field_origins);
        self.dragging_fields = false;
        self.drag_field_delta = (0, 0);
        if dx == 0 && dy == 0 {
            return;
        }
        // Capture owner/guid before erase.
        let painted: Vec<(i32, i32, u64, u32)> = origins
            .iter()
            .filter_map(|&(x, y)| {
                self.field_cell_meta(x, y)
                    .map(|(o, g)| (x + dx, y + dy, o, g))
            })
            .collect();
        if !self.history_drag_pushed {
            self.push_history();
        }
        let erased = self.erase_field_cells(&origins);
        let added = self.paint_field_cells(&painted);
        if erased > 0 || added > 0 {
            if let Some(open) = self.open.as_mut() {
                open.dirty = true;
                self.schedule_autosave();
            }
            self.selected_fields = painted.iter().map(|&(x, y, _, _)| (x, y)).collect();
            self.status = self.tf(
                "status.fields_moved",
                &[
                    ("n", &self.selected_fields.len().to_string()),
                    ("dx", &dx.to_string()),
                    ("dy", &dy.to_string()),
                ],
            );
            self.refresh_coverage();
        }
    }

    fn delete_selection(&mut self, group_ops: bool) {
        let mut indices = self.selection_indices();
        let roads_to_clear: Vec<(i32, i32)> = self.selected_roads.iter().copied().collect();
        let fields_to_clear: Vec<(i32, i32)> = self.selected_fields.iter().copied().collect();
        if indices.is_empty() && roads_to_clear.is_empty() && fields_to_clear.is_empty() {
            return;
        }
        self.push_history();
        // Erase fields first (need polygon_cells metadata + buildings still present).
        let fields_n = if !fields_to_clear.is_empty() {
            self.erase_field_cells(&fields_to_clear)
        } else {
            0
        };
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
        if ok > 0 || roads_n > 0 || fields_n > 0 {
            open.dirty = true;
            self.schedule_autosave();
            self.clear_canvas_selection();
            self.module_parent_index = None;
            self.status = match (ok, roads_n, fields_n) {
                (0, 0, f) => self.tf("status.deleted_fields", &[("n", &f.to_string())]),
                (0, n, 0) => self.tf("status.deleted_roads", &[("n", &n.to_string())]),
                (b, 0, 0) if b > 1 => {
                    self.tf("status.deleted_buildings", &[("n", &b.to_string())])
                }
                (1, 0, 0) => self.t("status.deleted_building").into(),
                (b, n, f) => self.tf(
                    "status.deleted_mixed",
                    &[
                        ("b", &b.to_string()),
                        ("r", &n.to_string()),
                        ("f", &f.to_string()),
                    ],
                ),
            };
            self.refresh_coverage();
        } else if let Some(e) = err {
            self.status = self.tf("status.delete_failed", &[("err", &e)]);
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
        self.flush_autosave_blocking();
        match open_named_stamp(&path, name) {
            Ok(open) => {
                self.set_open(open, &path);
                self.push_recent_pack(&path);
            }
            Err(err) => self.status = format!("Öffnen fehlgeschlagen: {err}"),
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        self.flush_autosave_blocking();
        match open_first_real_stamp(&path) {
            Ok(open) => {
                self.set_open(open, &path);
                self.push_recent_pack(&path);
            }
            Err(err) => self.status = format!("Öffnen fehlgeschlagen: {err}"),
        }
    }

    /// Remember a just-opened pack in the recent list and persist it.
    fn push_recent_pack(&mut self, path: &Path) {
        let path = path.to_path_buf();
        self.recent_packs.retain(|p| p != &path);
        self.recent_packs.insert(0, path);
        self.recent_packs.truncate(library::RECENT_PACKS_MAX);
        self.save_config();
    }

    /// Hash of everything the analysis passes depend on. When this is unchanged
    /// the cached results stay valid, so repaints (pan/hover) are cheap.
    fn stamp_analysis_fingerprint(open: &OpenStamp, lang: Language) -> u64 {
        let mut h = DefaultHasher::new();
        (lang as u8).hash(&mut h);
        open.region_guid.hash(&mut h);
        for b in &open.stamp.buildings {
            b.guid.hash(&mut h);
            b.x.to_bits().hash(&mut h);
            b.y.to_bits().hash(&mut h);
            b.direction.hash(&mut h);
            b.variation.hash(&mut h);
            b.complex_owner_id.hash(&mut h);
        }
        for c in &open.stamp.polygon_cells {
            c.guid.hash(&mut h);
            c.x.hash(&mut h);
            c.y.hash(&mut h);
            c.complex_owner_id.hash(&mut h);
        }
        // Road edits must invalidate too; sort so order changes don't matter.
        let mut roads = mesh_road_cells(&open.stamp.document);
        roads.sort_unstable();
        roads.hash(&mut h);
        h.finish()
    }

    /// Drop all cached analysis (on stamp open/close/switch).
    fn clear_analysis_caches(&mut self) {
        self.city_analysis = None;
        self.area_analysis = None;
        self.needs_cov_cache = None;
        self.analysis_gen = self.analysis_gen.wrapping_add(1);
        *self.place_cov_roads.borrow_mut() = None;
        *self.place_ghost_cov.borrow_mut() = None;
        self.drag_cov_key = None;
    }

    fn place_cov_road_map(&self, open: &OpenStamp) -> Arc<HashMap<Cell, RoadKind>> {
        let gen = self.analysis_gen;
        if let Some((g, roads)) = self.place_cov_roads.borrow().as_ref() {
            if *g == gen {
                return Arc::clone(roads);
            }
        }
        let roads = Arc::new(build_road_map_region(
            &open.stamp.buildings,
            &self.catalog,
            Some(&open.stamp.document),
            open.region_guid,
        ));
        *self.place_cov_roads.borrow_mut() = Some((gen, Arc::clone(&roads)));
        roads
    }

    /// Outgoing coverage for a building held in hand at a snapped ghost pose.
    fn place_ghost_coverage(&self, open: &OpenStamp, source: &Building) -> CoverageMap {
        let empty = CoverageMap::default();
        let Some(def) = self.catalog.building(source.guid) else {
            return empty;
        };
        if !building_has_area_effect(def) {
            return empty;
        }
        let gen = self.analysis_gen;
        let xb = source.x.to_bits();
        let yb = source.y.to_bits();
        {
            let cache = self.place_ghost_cov.borrow();
            if let Some((cgen, g, d, cx, cy, map)) = cache.as_ref() {
                if *cgen == gen
                    && *g == source.guid
                    && *d == source.direction
                    && *cx == xb
                    && *cy == yb
                {
                    return map.clone();
                }
            }
        }
        let roads = self.place_cov_road_map(open);
        let map = compute_outgoing_preview_with_roads(
            &open.stamp.buildings,
            &self.catalog,
            source,
            roads.as_ref(),
            self.stamp_drag(),
            open.region_guid,
            false,
        );
        *self.place_ghost_cov.borrow_mut() = Some((
            gen,
            source.guid,
            source.direction,
            xb,
            yb,
            map.clone(),
        ));
        map
    }

    fn paint_outgoing_coverage_preview(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        open: &OpenStamp,
        cov: &CoverageMap,
    ) {
        if !matches!(cov.mode, CoverageMode::Outgoing) {
            return;
        }
        // Skip full road-flood tint on the hot path — chips/dots are enough to see who
        // gets the bonus, and painting hundreds of road cells every move caused hitching.
        let Some(hit) = cov.hits.first() else {
            return;
        };
        let (r, g, bl) = kind_color(hit.service_kind.as_deref());
        let color = Color32::from_rgb(r, g, bl);
        for &ai in &cov.affected {
            let Some(tb) = open.stamp.buildings.get(ai) else {
                continue;
            };
            if self.cell_px >= 20.0 {
                let anchor = self.world_to_screen(rect, tb.x, tb.y - 0.9);
                let short: Vec<String> = hit
                    .bonuses
                    .iter()
                    .take(2)
                    .map(|b| self.format_bonus_chip(&b.attr, b.value))
                    .collect();
                self.paint_bonus_label(painter, anchor, &short, color);
            } else if self.cell_px >= 10.0 {
                let c = self.world_to_screen(rect, tb.x, tb.y);
                let rad = (self.cell_px * 0.22).clamp(2.5, 5.0);
                painter.circle_filled(
                    c,
                    rad,
                    Color32::from_rgba_unmultiplied(r, g, bl, 220),
                );
                painter.circle_stroke(
                    c,
                    rad,
                    Stroke::new(1.0_f32, Color32::from_rgb(240, 245, 255)),
                );
            }
        }
    }

    /// Ensure the canvas area-efficiency cache matches the current stamp.
    fn ensure_area_analysis(&mut self) {
        let fp = match &self.open {
            Some(open) => Self::stamp_analysis_fingerprint(open, self.language),
            None => {
                self.area_analysis = None;
                return;
            }
        };
        if self.area_analysis.as_ref().map(|c| c.fingerprint) == Some(fp) {
            return;
        }
        let open = self.open.as_ref().expect("open checked above");
        let areas = analyze_area_efficiency_with_fields(
            &open.stamp.buildings,
            &self.catalog,
            Some(&open.stamp.document),
            open.region_guid,
            &open.stamp.polygon_cells,
            self.language,
        );
        self.area_analysis = Some(AreaAnalysisCache {
            fingerprint: fp,
            areas,
        });
    }

    /// Ensure the full city-analysis cache matches the current stamp. Only the
    /// analyse panel needs this, so it is called just when that panel is shown.
    fn ensure_city_analysis(&mut self) {
        let fp = match &self.open {
            Some(open) => Self::stamp_analysis_fingerprint(open, self.language),
            None => {
                self.city_analysis = None;
                return;
            }
        };
        if self.city_analysis.as_ref().map(|c| c.fingerprint) == Some(fp) {
            return;
        }
        let open = self.open.as_ref().expect("open checked above");
        let lang = self.language;
        let region_guid = open.region_guid;
        let areas = analyze_area_efficiency_with_fields(
            &open.stamp.buildings,
            &self.catalog,
            Some(&open.stamp.document),
            region_guid,
            &open.stamp.polygon_cells,
            lang,
        );
        let chain = analyze_chain_region(
            &open.stamp.buildings,
            &self.catalog,
            Some(&open.stamp.document),
            region_guid,
            &open.stamp.polygon_cells,
            lang,
        );
        let mut incoming_by_index: HashMap<usize, CoverageMap> = HashMap::new();
        for (idx, b) in open.stamp.buildings.iter().enumerate() {
            if self
                .catalog
                .building(b.guid)
                .is_some_and(|d| d.is_residence())
            {
                incoming_by_index.insert(
                    idx,
                    compute_coverage_region(
                        &open.stamp.buildings,
                        &self.catalog,
                        idx,
                        Some(&open.stamp.document),
                        region_guid,
                    ),
                );
            }
        }
        let (city_consumption, city_services) = analyse_city_needs(
            &self.catalog,
            &open.stamp.buildings,
            &incoming_by_index,
            lang,
        );
        self.city_analysis = Some(CityAnalysisCache {
            fingerprint: fp,
            areas,
            chain,
            city_consumption,
            city_services,
            incoming_by_index,
        });
    }

    fn set_open(&mut self, open: OpenStamp, path: &Path) {
        self.clear_analysis_caches();
        self.history.clear();
        self.history_drag_pushed = false;
        self.clear_hand_silent();
        self.module_parent_index = None;
        self.residence_needs_pin = None;
        // Only auto-pick region when opening a pack (global filter stays user-controlled after).
        self.region_filter = RegionFilter::from_region_guid(open.region_guid);
        let cistern_note = if open.stamp.cistern_fixes_on_load > 0 {
            format!(
                " · {} Zisternen-Pos korrigiert (Speichern schreibt Fix)",
                open.stamp.cistern_fixes_on_load
            )
        } else {
            String::new()
        };
        self.status = format!(
            "Geöffnet: {} ({}) — {} Gebäude · Filter {}{}",
            open.entry_name,
            path.display(),
            open.stamp.buildings.len(),
            self.region_filter.label_lang(self.language),
            cistern_note
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
                    self.region_filter.label_lang(self.language)
                );
                self.clear_hand_silent();
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
                .unwrap_or_else(|| {
                    library_root.join(RegionFilter::game_stamp_folder_for_guid(region_guid))
                });
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
                let mirror_note = self.mirror_pack_after_save(&path);
                self.status = self
                    .tf("status.saved_path", &[("path", &path.display().to_string())])
                    + &mirror_note;
            }
            Err(err) => {
                self.status = self.tf("status.save_failed", &[("err", &err.to_string())])
            }
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
        // Anno Y-up: pan.y = +avg_y so world_to_screen centers with the Y flip below.
        self.pan = Vec2::new(-avg_x * self.cell_px, avg_y * self.cell_px);
    }

    /// Zoom + pan so the whole stamp bounding box fits in the canvas, then center it.
    fn fit_view(&mut self) {
        let Some(open) = &self.open else { return };
        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        let mut n = 0usize;
        for b in open.stamp.all_draw_buildings() {
            min_x = min_x.min(b.x);
            max_x = max_x.max(b.x);
            min_y = min_y.min(b.y);
            max_y = max_y.max(b.y);
            n += 1;
        }
        for c in &open.stamp.polygon_cells {
            let cx = c.x as f32 + 0.5;
            let cy = c.y as f32 + 0.5;
            min_x = min_x.min(cx);
            max_x = max_x.max(cx);
            min_y = min_y.min(cy);
            max_y = max_y.max(cy);
            n += 1;
        }
        if n == 0 {
            self.center_view();
            return;
        }
        // Pad a few tiles so footprints near the edge aren't clipped.
        let span_x = (max_x - min_x).max(1.0) + 6.0;
        let span_y = (max_y - min_y).max(1.0) + 6.0;
        // Fall back to a sensible default viewport before the canvas has been drawn once.
        let (vw, vh) = self
            .last_canvas_rect
            .map(|r| (r.width().max(80.0), r.height().max(80.0)))
            .unwrap_or((900.0, 600.0));
        let fit = (vw / span_x).min(vh / span_y);
        self.cell_px = fit.clamp(8.0, 48.0);
        self.center_view();
    }

    /// 90°/45° direction step honoring `allow_45_deg`. `reverse` = counter-clockwise.
    fn rotate_dir(&self, cur: i32, reverse: bool) -> i32 {
        let d = cur.rem_euclid(8);
        if reverse {
            if self.allow_45_deg || d % 2 == 1 {
                (d + 7).rem_euclid(8)
            } else {
                (d + 6).rem_euclid(8)
            }
        } else {
            self.next_direction(cur)
        }
    }

    /// `R` / Shift+R entry point: rotates the hand, a multi-selection, or a single building.
    fn rotate_selection(&mut self, reverse: bool) {
        if self.placing_guid.is_some() {
            if self.hand_from_clipboard {
                self.rotate_clipboard_hand(reverse);
            } else {
                let cur = self.place_direction;
                self.place_direction = self.rotate_dir(cur, reverse);
                let label = self
                    .placing_guid
                    .map(|g| self.catalog.label(g, self.language))
                    .unwrap_or_default();
                self.refresh_place_status(&label);
            }
            return;
        }
        let indices = self.selection_indices();
        if indices.len() > 1 {
            self.rotate_group(&indices, reverse);
        } else if let Some(idx) = self.selected_building {
            self.rotate_single(idx, reverse);
        }
    }

    /// Rotate one placed building in-place, keeping its footprint top-left stable.
    fn rotate_single(&mut self, idx: usize, reverse: bool) {
        let cur = self
            .open
            .as_ref()
            .and_then(|o| o.stamp.buildings.get(idx))
            .map(|b| b.direction);
        let Some(cur) = cur else { return };
        let next = self.rotate_dir(cur, reverse);
        let allow_45 = self.allow_45_deg;
        self.push_history();
        // Keep top-left tile stable so 4×5 → 5×4 doesn't jump by a full cell.
        let (guid, variation, ox, oy) = self
            .open
            .as_ref()
            .and_then(|o| o.stamp.buildings.get(idx))
            .map(|b| (b.guid, b.variation, b.x, b.y))
            .unwrap_or((0, None, 0.0, 0.0));
        let (old_w0, old_h0) = self.building_size(guid, variation);
        let (tx, ty) = {
            let (mx, my, _, _) = self.building_oriented_blocker(guid, variation, cur);
            footprint_top_left_blocker(ox, oy, mx, my, self.stamp_drag())
        };
        let (nw, nh) = oriented_size(old_w0, old_h0, next);
        let (nx, ny) = self.place_pos_for_tl(tx, ty, guid, variation, next);
        if let Some(open) = self.open.as_mut() {
            match open.stamp.set_building_direction(idx, next) {
                Ok(()) => {
                    let _ = open.stamp.set_building_pos(idx, nx, ny);
                    open.dirty = true;
                    self.schedule_autosave();
                    self.status = self.tf(
                        "status.rotated",
                        &[
                            ("dir", &next.to_string()),
                            ("w", &format!("{nw:.0}")),
                            ("h", &format!("{nh:.0}")),
                            ("mode", if allow_45 { "45°" } else { "90°" }),
                        ],
                    );
                    self.refresh_coverage();
                }
                Err(e) => {
                    self.status = self.tf("status.rotate_failed", &[("err", &e.to_string())]);
                }
            }
        }
    }

    /// Rotate a multi-selection around its bounding-box center, preserving the layout.
    /// Position rotation only runs on the 90° grid (not in 45° mode).
    fn rotate_group(&mut self, indices: &[usize], reverse: bool) {
        let rotate_pos = !self.allow_45_deg;
        // Snapshot positions/dirs and compute the bounds center.
        let mut items: Vec<(usize, f32, f32, i32)> = Vec::new();
        {
            let Some(open) = self.open.as_ref() else { return };
            for &i in indices {
                if let Some(b) = open.stamp.buildings.get(i) {
                    items.push((i, b.x, b.y, b.direction));
                }
            }
        }
        if items.len() < 2 {
            return;
        }
        let (mut min_x, mut max_x, mut min_y, mut max_y) =
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(_, x, y, _) in &items {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        self.push_history();
        let mut ok = 0usize;
        for &(i, x, y, dir) in &items {
            let next = self.rotate_dir(dir, reverse);
            let (nx, ny) = if rotate_pos {
                let (dx, dy) = (x - cx, y - cy);
                // World Y-up: CW = (dx,dy)->(dy,-dx); CCW = (dx,dy)->(-dy,dx).
                let (rx, ry) = if reverse { (-dy, dx) } else { (dy, -dx) };
                ((cx + rx).round(), (cy + ry).round())
            } else {
                (x, y)
            };
            if let Some(open) = self.open.as_mut() {
                if open.stamp.set_building_direction(i, next).is_ok() {
                    let _ = open.stamp.set_building_pos(i, nx, ny);
                    ok += 1;
                }
            }
        }
        if ok > 0 {
            if let Some(open) = self.open.as_mut() {
                open.dirty = true;
            }
            self.schedule_autosave();
            self.status = self.tf("status.rotated_group", &[("n", &ok.to_string())]);
            self.refresh_coverage();
        }
    }

    /// Rotate the clipboard group currently held in hand (directions + relative offsets).
    /// Approximate for non-square footprints, but keeps uniform groups aligned.
    fn rotate_clipboard_hand(&mut self, reverse: bool) {
        // Primary ghost direction.
        self.place_direction = self.rotate_dir(self.place_direction, reverse);
        // Centroid of the footprint top-left deltas.
        let (mut sx, mut sy) = (0i64, 0i64);
        for c in &self.clipboard {
            sx += c.dtx as i64;
            sy += c.dty as i64;
        }
        let n = self.clipboard.len().max(1) as i64;
        let cx = (sx as f32) / (n as f32);
        let cy = (sy as f32) / (n as f32);
        let allow_45 = self.allow_45_deg;
        let rot = |d: i32| -> i32 {
            let d = d.rem_euclid(8);
            if reverse {
                if allow_45 || d % 2 == 1 {
                    (d + 7).rem_euclid(8)
                } else {
                    (d + 6).rem_euclid(8)
                }
            } else if allow_45 || d % 2 == 1 {
                (d + 1).rem_euclid(8)
            } else {
                (d + 2).rem_euclid(8)
            }
        };
        for c in &mut self.clipboard {
            c.direction = rot(c.direction);
            let (dx, dy) = (c.dtx as f32 - cx, c.dty as f32 - cy);
            // Screen-space TL deltas are Y-down, so CW = (dx,dy)->(-dy,dx).
            let (rx, ry) = if reverse { (dy, -dx) } else { (-dy, dx) };
            c.dtx = (cx + rx).round() as i32;
            c.dty = (cy + ry).round() as i32;
        }
        for r in &mut self.clipboard_roads {
            let (dx, dy) = (r.dx as f32 - cx, r.dy as f32 - cy);
            let (rx, ry) = if reverse { (dy, -dx) } else { (-dy, dx) };
            r.dx = (cx + rx).round() as i32;
            r.dy = (cy + ry).round() as i32;
        }
        let label = self
            .placing_guid
            .map(|g| self.catalog.label(g, self.language))
            .unwrap_or_default();
        self.refresh_place_status(&label);
    }

    /// True when something movable (buildings / roads / fields) is selected.
    fn has_movable_selection(&self) -> bool {
        self.selected_building.is_some()
            || !self.selected_buildings.is_empty()
            || !self.selected_roads.is_empty()
            || !self.selected_fields.is_empty()
    }

    /// Move the current selection by `(dx, dy)` whole tiles (arrow-key nudge).
    fn nudge_selection(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let indices = self.selection_indices();
        let has_fields = !self.selected_fields.is_empty();
        // Roads aren't nudged (they require per-cell GUID rewrites); buildings + fields only.
        if indices.is_empty() && !has_fields {
            return;
        }
        self.push_history();
        let fx = dx as f32;
        let fy = dy as f32;
        let mut changed = false;
        // Buildings (whole-group link when „Gruppe verschieben“ is on).
        let link = self.move_group;
        let origins: Vec<(usize, f32, f32)> = {
            let Some(open) = self.open.as_ref() else {
                return;
            };
            indices
                .iter()
                .filter_map(|&i| open.stamp.buildings.get(i).map(|b| (i, b.x, b.y)))
                .collect()
        };
        for (i, bx, by) in origins {
            if let Some(open) = self.open.as_mut() {
                if open.stamp.set_building_pos_ex(i, bx + fx, by + fy, link).is_ok() {
                    changed = true;
                }
            }
        }
        // Fields: erase + repaint shifted cells.
        if has_fields {
            let cells: Vec<(i32, i32)> = self.selected_fields.iter().copied().collect();
            let painted: Vec<(i32, i32, u64, u32)> = cells
                .iter()
                .filter_map(|&(x, y)| self.field_cell_meta(x, y).map(|(o, g)| (x + dx, y + dy, o, g)))
                .collect();
            let erased = self.erase_field_cells(&cells);
            let added = self.paint_field_cells(&painted);
            if erased > 0 || added > 0 {
                self.selected_fields = painted.iter().map(|&(x, y, _, _)| (x, y)).collect();
                changed = true;
            }
        }
        if changed {
            if let Some(open) = self.open.as_mut() {
                open.dirty = true;
            }
            self.schedule_autosave();
            self.status = self.tf(
                "status.nudged",
                &[("dx", &dx.to_string()), ("dy", &dy.to_string())],
            );
            self.refresh_coverage();
        }
    }

    fn world_to_screen(&self, rect: Rect, x: f32, y: f32) -> Pos2 {
        // Anno stamp space is Y-up (north = +Y). Screen Y grows downward, so flip.
        Pos2::new(
            rect.center().x + self.pan.x + x * self.cell_px,
            rect.center().y + self.pan.y - y * self.cell_px,
        )
    }

    fn screen_to_world(&self, rect: Rect, pos: Pos2) -> (f32, f32) {
        let x = (pos.x - rect.center().x - self.pan.x) / self.cell_px;
        let y = -(pos.y - rect.center().y - self.pan.y) / self.cell_px;
        (x, y)
    }

    /// Integer tile (x,y) → screen rect covering that full cell (voxel/blueprint style).
    ///
    /// With Y-up world → screen flip, the world corner `(x,y)` is below `(x+1,y+1)` on
    /// screen, so corners must be sorted — raw `from_min_max` would invert the rect and
    /// paint nothing (footprints/roads vanish; only center labels remain).
    fn cell_screen_rect(&self, rect: Rect, x: i32, y: i32) -> Rect {
        let a = self.world_to_screen(rect, x as f32, y as f32);
        let b = self.world_to_screen(rect, x as f32 + 1.0, y as f32 + 1.0);
        Rect::from_min_max(
            Pos2::new(a.x.min(b.x), a.y.min(b.y)),
            Pos2::new(a.x.max(b.x), a.y.max(b.y)),
        )
    }

    /// Inclusive tile AABB → screen rect (union of corner cells; Y-flip safe).
    fn cells_screen_rect(&self, rect: Rect, min_x: i32, min_y: i32, max_x: i32, max_y: i32) -> Rect {
        let a = self.cell_screen_rect(rect, min_x, min_y);
        let b = self.cell_screen_rect(rect, max_x, max_y);
        Rect::from_min_max(
            Pos2::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y)),
            Pos2::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y)),
        )
    }

    fn building_size(&self, guid: u32, variation: Option<u32>) -> (f32, f32) {
        size_for_guid(
            guid,
            2.0,
            2.0,
            self.catalog.building(guid),
            variation,
        )
    }

    /// Oriented BuildBlocker `(min_x, min_y, w, h)` relative to pipette Pos.
    fn building_oriented_blocker(
        &self,
        guid: u32,
        variation: Option<u32>,
        direction: i32,
    ) -> (f32, f32, f32, f32) {
        let (w0, h0) = self.building_size(guid, variation);
        let (mx0, my0) = blocker_min_for(
            guid,
            w0,
            h0,
            self.catalog.building(guid),
            variation,
        );
        oriented_blocker(mx0, my0, w0, h0, direction)
    }

    fn footprint_tl_building(&self, b: &Building) -> (i32, i32) {
        let (mx, my, _, _) =
            self.building_oriented_blocker(b.guid, b.variation, b.direction);
        footprint_top_left_blocker(b.x, b.y, mx, my, self.stamp_drag())
    }

    fn place_pos_for_tl(
        &self,
        tx: i32,
        ty: i32,
        guid: u32,
        variation: Option<u32>,
        direction: i32,
    ) -> (f32, f32) {
        let (mx, my, w, h) = self.building_oriented_blocker(guid, variation, direction);
        place_pos_from_top_left(tx, ty, w, h, mx, my)
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
        let (mx, my, w, h) = self.building_oriented_blocker(guid, variation, dir);
        let (x0, y0) = footprint_top_left_blocker(x, y, mx, my, self.stamp_drag());
        let bw = w.round().max(1.0) as i32;
        let bh = h.round().max(1.0) as i32;
        self.cells_screen_rect(rect, x0, y0, x0 + bw - 1, y0 + bh - 1)
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

    /// PlantFarm field tile → ComplexOwner main building index (for click/marquee/drag).
    fn farm_main_at_field_tile(&self, tx: i32, ty: i32) -> Option<usize> {
        let open = self.open.as_ref()?;
        let owner = open
            .stamp
            .polygon_cells
            .iter()
            .find(|c| c.x == tx && c.y == ty)
            .and_then(|c| c.complex_owner_id.filter(|o| *o != 0))?;
        let seed = open
            .stamp
            .buildings
            .iter()
            .position(|b| b.complex_owner_id == Some(owner))?;
        complex_main_index(&open.stamp.buildings, &self.catalog, seed)
    }

    /// Building under pointer, else farm main via field tile (hover / farm UI only).
    fn selectable_at_pointer(
        &self,
        rect: Rect,
        buildings: &[Building],
        pointer: Pos2,
    ) -> Option<usize> {
        if let Some(i) = self.building_at_pointer(rect, buildings, pointer) {
            return Some(i);
        }
        let (wx, wy) = self.screen_to_world(rect, pointer);
        let (tx, ty) = (wx.floor() as i32, wy.floor() as i32);
        self.farm_main_at_field_tile(tx, ty)
    }

    fn field_tile_at_pointer(&self, rect: Rect, pointer: Pos2) -> Option<(i32, i32)> {
        let (wx, wy) = self.screen_to_world(rect, pointer);
        let (tx, ty) = (wx.floor() as i32, wy.floor() as i32);
        let open = self.open.as_ref()?;
        open.stamp
            .polygon_cells
            .iter()
            .any(|c| c.x == tx && c.y == ty)
            .then_some((tx, ty))
    }

    /// Floating controls: module count + place-field buttons for the focused farm.
    fn residence_index_if_house(&self, idx: usize) -> Option<usize> {
        let open = self.open.as_ref()?;
        let b = open.stamp.buildings.get(idx)?;
        let def = self.catalog.building(b.guid)?;
        def.is_residence().then_some(idx)
    }

    /// Residence the service window should stay on (selection, pin while placing, or hover).
    fn residence_needs_focus(&self) -> Option<usize> {
        if let Some(s) = self.selected_building {
            if self.residence_index_if_house(s).is_some() {
                return Some(s);
            }
        }
        if let Some(p) = self.residence_needs_pin {
            if self.residence_index_if_house(p).is_some() {
                return Some(p);
            }
        }
        self.hover_building
            .and_then(|h| self.residence_index_if_house(h))
    }

    /// Top-right quick-place panel for a selected/hovered residence's service needs
    /// (same pattern as farm Untergebäude).
    fn show_residence_needs_ui(&mut self, ctx: &egui::Context) {
        if self.open.is_none() {
            return;
        }
        // Drop a stale pin (deleted / upgraded away from residence).
        if let Some(p) = self.residence_needs_pin {
            if self.residence_index_if_house(p).is_none() {
                self.residence_needs_pin = None;
            }
        }
        let Some(sel) = self.residence_needs_focus() else {
            return;
        };
        let open = self.open.as_ref().expect("open checked");
        let Some(b) = open.stamp.buildings.get(sel) else {
            return;
        };
        let Some(def) = self.catalog.building(b.guid) else {
            return;
        };
        if !def.is_residence() {
            return;
        }
        let lang = self.language;
        let region = RegionFilter::from_region_guid(open.region_guid);
        let house_label = self.catalog.label(b.guid, lang);
        let report = if self.selected_building == Some(sel)
            && matches!(self.coverage.mode, CoverageMode::Incoming)
        {
            // Selected residence already has live Incoming coverage — use it.
            report_for_def(def, &provided_need_guids(&self.coverage.hits))
        } else {
            // Otherwise reuse a cached coverage for this (stamp, residence);
            // only recompute the BFS when the stamp or focus actually changes.
            let fp = Self::stamp_analysis_fingerprint(open, lang);
            let stale = self
                .needs_cov_cache
                .as_ref()
                .map(|(cfp, cidx, _)| *cfp != fp || *cidx != sel)
                .unwrap_or(true);
            if stale {
                let cov = compute_coverage_region(
                    &open.stamp.buildings,
                    &self.catalog,
                    sel,
                    Some(&open.stamp.document),
                    open.region_guid,
                );
                self.needs_cov_cache = Some((fp, sel, cov));
            }
            let cov = &self
                .needs_cov_cache
                .as_ref()
                .expect("needs coverage just cached")
                .2;
            report_for_def(def, &provided_need_guids(&cov.hits))
        };
        if report.services.is_empty() {
            return;
        }
        let rows: Vec<(bool, String, Option<String>, Vec<(u32, String)>)> = report
            .services
            .iter()
            .map(|s| {
                let need_lbl = s.need.label(lang);
                let icon = s.need.icon.clone();
                let providers = self
                    .catalog
                    .buildings_providing_need(s.need.need_guid, region);
                let place_btns: Vec<(u32, String)> =
                    if providers.first().is_some_and(|p| p.is_aqueduct_cistern()) {
                        // Basin + MinGround + MaxGround (hoch) + Zisterne
                        self.catalog
                            .aqueduct_kit(region)
                            .into_iter()
                            .map(|p| {
                                let mut label = self.catalog.label(p.guid, lang);
                                if p.is_aqueduct_maxground() {
                                    label = format!(
                                        "{} ({})",
                                        label,
                                        i18n::tr(lang, "aqueduct.maxground_short")
                                    );
                                } else if p.is_aqueduct_minground() {
                                    label = format!(
                                        "{} ({})",
                                        label,
                                        i18n::tr(lang, "aqueduct.minground_short")
                                    );
                                }
                                (p.guid, label)
                            })
                            .collect()
                    } else {
                        providers
                            .into_iter()
                            .next()
                            .map(|p| vec![(p.guid, self.catalog.label(p.guid, lang))])
                            .unwrap_or_default()
                    };
                (s.met, need_lbl, icon, place_btns)
            })
            .collect();
        let met = report.met_count();
        let total = report.service_count();
        let mut pending_place: Option<(u32, String)> = None;

        let screen = ctx.screen_rect();
        let default_h = ((rows.len() as f32) * 26.0 + 88.0).clamp(140.0, screen.height() * 0.55);
        let title = self.t("analyse.residence_services").to_string();

        egui::Window::new(title)
            .id(egui::Id::new("residence_needs_ui"))
            .collapsible(false)
            .movable(true)
            .resizable([false, true])
            .default_size([260.0, default_h])
            .min_height(120.0)
            .max_height(screen.height() * 0.85)
            .default_pos(egui::pos2(screen.right() - 276.0, screen.top() + 72.0))
            .constrain(true)
            .show(ctx, |ui| {
                ui.label(RichText::new(&house_label).strong());
                let color = if met >= total && total > 0 {
                    Color32::from_rgb(90, 180, 90)
                } else if met > 0 {
                    Color32::from_rgb(220, 160, 60)
                } else {
                    Color32::from_rgb(220, 100, 80)
                };
                ui.colored_label(
                    color,
                    self.tf(
                        "analyse.services_badge",
                        &[
                            ("met", &met.to_string()),
                            ("total", &total.to_string()),
                        ],
                    ),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (is_met, need_lbl, icon, providers) in &rows {
                            let mark = if *is_met { "[x]" } else { "[ ]" };
                            let mcolor = if *is_met {
                                Color32::from_rgb(90, 180, 90)
                            } else {
                                Color32::from_rgb(220, 100, 80)
                            };
                            ui.horizontal_wrapped(|ui| {
                                if let Some(path) = icon.as_deref() {
                                    let tex = self.ui_icons.get_or_placeholder(
                                        ui.ctx(),
                                        Some(path),
                                        [24, 24],
                                        [40, 50, 70],
                                    );
                                    let (rect, _) = ui.allocate_exact_size(
                                        Vec2::splat(16.0),
                                        Sense::hover(),
                                    );
                                    Self::paint_tex(ui, &tex, rect, Color32::WHITE);
                                }
                                ui.colored_label(mcolor, mark);
                                ui.label(need_lbl);
                                if providers.is_empty() {
                                    ui.label(
                                        RichText::new(self.t("analyse.service_no_provider"))
                                            .small()
                                            .weak(),
                                    );
                                } else {
                                    for (pg, pl) in providers {
                                        let btn = self.tf(
                                            "analyse.place_service",
                                            &[("name", pl)],
                                        );
                                        if ui.small_button(btn).clicked() {
                                            pending_place = Some((*pg, pl.clone()));
                                        }
                                    }
                                }
                            });
                        }
                    });
            });

        if let Some((guid, label)) = pending_place {
            self.residence_needs_pin = Some(sel);
            self.activate_menu_building(guid, &label);
            self.module_parent_index = None;
            self.status = self.tf(
                "status.hand_place",
                &[("label", &label)],
            );
        }
    }

    /// Assign a ComplexOwnerID to a farm main that still lacks one (needed for field paint).
    fn ensure_farm_complex_owner(&mut self, main_idx: usize) -> Option<u64> {
        let open = self.open.as_mut()?;
        let b = open.stamp.buildings.get(main_idx)?;
        if let Some(oid) = b.complex_owner_id.filter(|o| *o != 0) {
            return Some(oid);
        }
        let def = self.catalog.building(b.guid)?;
        if needed_modules(Some(def)) == 0 {
            return None;
        }
        let oid = next_complex_owner_id(&open.stamp.buildings, &open.stamp.polygon_cells);
        open.stamp.set_building_owner(main_idx, oid).ok()?;
        open.dirty = true;
        self.schedule_autosave();
        Some(oid)
    }

    fn show_complex_module_ui(&mut self, ctx: &egui::Context) {
        let Some(sel) = self.selected_building.or(self.hover_building) else {
            return;
        };
        let (main_idx, main_guid, need) = {
            let Some(open) = self.open.as_ref() else {
                return;
            };
            // Farms needing modules/fields — also when ComplexOwnerID is still missing
            // (fresh place); kitchens without a quota stay quiet.
            let Some(main_idx) = complex_main_index(&open.stamp.buildings, &self.catalog, sel)
            else {
                return;
            };
            let Some(main_b) = open.stamp.buildings.get(main_idx) else {
                return;
            };
            let main_guid = main_b.guid;
            let need = needed_modules(self.catalog.building(main_guid));
            if need == 0 {
                return;
            }
            (main_idx, main_guid, need)
        };
        let farm_owner = self.ensure_farm_complex_owner(main_idx);
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let main_def = self.catalog.building(main_guid);
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
        let main_label = self.catalog.label(main_guid, self.language);
        let ready = need > 0 && have as u32 >= need;
        let area_farm = stamp_editor::modules::uses_area_progress(main_def);

        // Area (not Window): cannot be dismissed/persisted-closed; always visible while focused.
        egui::Area::new(egui::Id::new("complex_module_ui"))
            .anchor(egui::Align2::RIGHT_TOP, [-16.0, 72.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_min_width(200.0);
                        ui.label(RichText::new(self.t("analyse.subbuildings")).strong());
                        ui.label(RichText::new(&main_label).strong());
                        let color = if ready {
                            Color32::from_rgb(90, 180, 90)
                        } else if need > 0 {
                            Color32::from_rgb(220, 160, 60)
                        } else {
                            Color32::GRAY
                        };
                        let unit = if area_farm {
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
                        if area_farm {
                            if let Some((fg, fl)) = mod_btns.first().cloned() {
                                let active = self.field_brush_owner == farm_owner
                                    && self.field_brush_guid == Some(fg);
                                let btn = ui.add(
                                    egui::Button::new(format!("Feld malen: {fl}")).selected(active),
                                );
                                if btn.clicked() {
                                    if active {
                                        self.clear_hand();
                                    } else if let Some(oid) = farm_owner {
                                        self.activate_field_brush(oid, fg, &fl);
                                    } else {
                                        self.status =
                                            "Farm braucht ComplexOwnerID für Feld-Malen".into();
                                    }
                                }
                            }
                        } else {
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
                    self.t(item.label_key)
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_string(),
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
                self.t(item.label_key),
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
            A,
            F,
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
                            egui::Key::A if !modifiers.alt && !modifiers.shift => {
                                out.push(Chord::A)
                            }
                            egui::Key::F if !modifiers.alt && !modifiers.shift => {
                                out.push(Chord::F)
                            }
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
                            self.status = self.t("status.undo").into();
                        }
                    }
                }
                Chord::Y => {
                    if let Some(cur) = self.take_snapshot() {
                        if let Some(next) = self.history.redo(cur) {
                            self.apply_snapshot(next);
                            self.status = self.t("status.redo").into();
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
                Chord::A => self.select_all(),
                Chord::F => self.focus_library_filter = true,
            }
        }
    }

    /// Ctrl+A: select every building, road and field in the open stamp.
    fn select_all(&mut self) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let indices: Vec<usize> = (0..open.stamp.buildings.len()).collect();
        let roads: Vec<(i32, i32)> =
            self.coverage.roads.iter().map(|(c, _)| (c.x, c.y)).collect();
        let fields: Vec<(i32, i32)> = open
            .stamp
            .polygon_cells
            .iter()
            .map(|c| (c.x, c.y))
            .collect();
        let (nb, nr, nf) = (indices.len(), roads.len(), fields.len());
        if nb == 0 && nr == 0 && nf == 0 {
            return;
        }
        self.set_canvas_selection_full(indices, roads, fields);
        self.status = self.tf(
            "status.selected_all",
            &[
                ("b", &nb.to_string()),
                ("r", &nr.to_string()),
                ("f", &nf.to_string()),
            ],
        );
        self.refresh_coverage();
    }

    /// F1 keyboard-shortcut cheatsheet window.
    fn show_cheatsheet_window(&mut self, ctx: &egui::Context) {
        if !self.show_cheatsheet {
            return;
        }
        let mut open = true;
        egui::Window::new(self.t("help.title"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(self.t("help.body")).monospace());
                ui.separator();
                if ui.button(self.t("help.close")).clicked() {
                    self.show_cheatsheet = false;
                }
            });
        if !open {
            self.show_cheatsheet = false;
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
        if self.setup_complete && self.open.is_none() {
            let dir = self.region_stamp_dir();
            let _ = std::fs::create_dir_all(&dir);
            self.start_new_stamp_in(&dir, false);
        }
        self.show_setup_wizard(ctx);
        self.show_new_stamp_dialog(ctx);
        self.show_rename_stamp_dialog(ctx);
        self.show_cheatsheet_window(ctx);

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
            // Fixed rows — never put Region/tools inside menu::bar (it wraps and
            // the first line "jumps" onto a second when Hand/clipboard labels appear).
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
                style.spacing.button_padding = egui::vec2(10.0, 6.0);
                style.spacing.item_spacing = egui::vec2(10.0, 6.0);
                style.spacing.interact_size.y = 26.0;
            }

            // Row 1: menus (no wrap) + stamp title beside them — never inside
            // menu::bar alone (egui wraps the title onto a second menu line).
            ui.horizontal(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                egui::menu::bar(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.menu_button(self.t("menu.file"), |ui| {
                    if ui.button(self.t("file.setup_wizard")).clicked() {
                        self.open_setup_wizard();
                        ui.close_menu();
                    }
                    if ui.button(self.t("file.link_game")).clicked() {
                        let start = self
                            .game_install
                            .clone()
                            .or_else(EditorConfig::guess_game_install)
                            .unwrap_or_else(|| PathBuf::from(r"C:\"));
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title(self.t("dialog.link_game_title"))
                            .set_directory(start)
                            .pick_folder()
                        {
                            self.link_game_install(path);
                        }
                        ui.close_menu();
                    }
                    if self.game_install.is_some() {
                        if ui.button(self.t("file.unlink_game")).clicked() {
                            self.game_install = None;
                            self.ui_icons.clear_game_install();
                            self.save_config();
                            self.status = self.t("status.game_unlinked").into();
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button(self.t("file.library_folder")).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_directory(&self.library_root)
                            .pick_folder()
                        {
                            if EditorConfig::is_game_stamps_path(&path, &self.game_stamps_root) {
                                self.status = self.t("status.library_is_sync").into();
                            } else {
                                self.library_root = path;
                                self.refresh_library();
                                self.save_config();
                            }
                        }
                        ui.close_menu();
                    }
                    if ui.button(self.t("file.use_safe_library")).clicked() {
                        self.use_safe_library();
                        ui.close_menu();
                    }
                    if ui.button(self.t("file.export_to_game")).clicked() {
                        match rfd::MessageDialog::new()
                            .set_level(rfd::MessageLevel::Warning)
                            .set_title(self.t("dialog.export_title"))
                            .set_description(self.t("dialog.export_body"))
                            .set_buttons(rfd::MessageButtons::OkCancel)
                            .show()
                        {
                            rfd::MessageDialogResult::Ok => self.export_library_to_game(),
                            _ => {}
                        }
                        ui.close_menu();
                    }
                    if ui.button(self.t("file.import_from_game")).clicked() {
                        self.import_from_game();
                        ui.close_menu();
                    }
                    {
                        let mirror_label = self.t("file.mirror_to_game");
                        if ui
                            .checkbox(&mut self.mirror_to_game, mirror_label)
                            .changed()
                        {
                            self.save_config();
                        }
                    }
                    if ui.button(self.t("file.rescan_library")).clicked() {
                        self.refresh_library();
                        ui.close_menu();
                    }
                    if ui.button(self.t("file.sort_by_region")).clicked() {
                        match rfd::MessageDialog::new()
                            .set_level(rfd::MessageLevel::Warning)
                            .set_title(self.t("dialog.sort_title"))
                            .set_description(self.t("dialog.sort_body"))
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
                                        self.status = self.tf(
                                            "status.sorted",
                                            &[("n", &n.to_string())],
                                        );
                                    }
                                    Err(e) => {
                                        self.status = self.tf(
                                            "status.sort_failed",
                                            &[("err", &e.to_string())],
                                        )
                                    }
                                }
                            }
                            _ => {}
                        }
                        ui.close_menu();
                    }
                    if ui.button(self.t("file.open_pack")).clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Stamp Pack", &["json"])
                            .set_directory(&self.library_root)
                            .pick_file()
                        {
                            self.open_path(path);
                        }
                        ui.close_menu();
                    }
                    {
                        let recent = self.recent_packs.clone();
                        ui.menu_button(self.t("file.recent"), |ui| {
                            if recent.is_empty() {
                                ui.add_enabled(
                                    false,
                                    egui::Button::new(self.t("file.recent_empty")),
                                );
                            }
                            for path in recent {
                                let label = path
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| path.display().to_string());
                                let exists = path.is_file();
                                if ui
                                    .add_enabled(exists, egui::Button::new(label))
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    self.open_path(path);
                                    ui.close_menu();
                                }
                            }
                        });
                    }
                    if ui.button(self.t("file.save")).clicked() {
                        self.save_current();
                        ui.close_menu();
                    }
                    if ui.button(self.t("file.reload_catalog")).clicked() {
                        match Catalog::load(&self.catalog_path) {
                            Ok(c) => {
                                self.catalog = c;
                                self.status = self.t("status.catalog_loaded").into();
                                self.refresh_coverage();
                            }
                            Err(e) => {
                                self.status = self.tf(
                                    "status.catalog_failed",
                                    &[("err", &e.to_string())],
                                )
                            }
                        }
                        ui.close_menu();
                    }
                });
                ui.menu_button(self.t("menu.edit"), |ui| {
                    if ui
                        .add_enabled(
                            self.history.can_undo(),
                            egui::Button::new(self.t("edit.undo")),
                        )
                        .clicked()
                    {
                        if let Some(cur) = self.take_snapshot() {
                            if let Some(prev) = self.history.undo(cur) {
                                self.apply_snapshot(prev);
                                self.status = self.t("status.undo").into();
                            }
                        }
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.history.can_redo(),
                            egui::Button::new(self.t("edit.redo")),
                        )
                        .clicked()
                    {
                        if let Some(cur) = self.take_snapshot() {
                            if let Some(next) = self.history.redo(cur) {
                                self.apply_snapshot(next);
                                self.status = self.t("status.redo").into();
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
                        .add_enabled(can_copy, egui::Button::new(self.t("edit.cut")))
                        .clicked()
                    {
                        self.cut_selection();
                        self.sync_os_clipboard_marker(ctx);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(can_copy, egui::Button::new(self.t("edit.copy")))
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
                            egui::Button::new(self.t("edit.paste_hand")),
                        )
                        .clicked()
                    {
                        self.arm_clipboard_in_hand();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            !self.clipboard.is_empty(),
                            egui::Button::new(self.t("edit.paste_cursor")),
                        )
                        .clicked()
                    {
                        self.paste_clipboard_at_cursor();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.selected_building.is_some() || !self.clipboard.is_empty(),
                            egui::Button::new(self.t("edit.duplicate")),
                        )
                        .clicked()
                    {
                        self.duplicate_selection();
                        ui.close_menu();
                    }
                    ui.separator();
                    let can_rotate = self.placing_guid.is_some()
                        || self.selected_building.is_some()
                        || !self.selected_buildings.is_empty();
                    if ui
                        .add_enabled(can_rotate, egui::Button::new(self.t("edit.rotate")))
                        .clicked()
                    {
                        self.rotate_selection(false);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(can_rotate, egui::Button::new(self.t("edit.rotate_ccw")))
                        .clicked()
                    {
                        self.rotate_selection(true);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.open.is_some(),
                            egui::Button::new(self.t("edit.select_all")),
                        )
                        .clicked()
                    {
                        self.select_all();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.selected_building.is_some()
                                || !self.selected_buildings.is_empty()
                                || !self.selected_roads.is_empty()
                                || !self.selected_fields.is_empty(),
                            egui::Button::new(self.t("edit.delete")),
                        )
                        .clicked()
                    {
                        self.request_delete = true;
                        ui.close_menu();
                    }
                    let move_group_lbl = self.t("edit.move_group");
                    ui.checkbox(&mut self.move_group, move_group_lbl);
                });
                if ui.button(self.t("menu.new")).clicked() {
                    self.start_new_stamp();
                }
                ui.menu_button(self.t("menu.view"), |ui| {
                    if ui.button(self.t("view.center")).clicked() {
                        self.center_view();
                        self.status = self.t("status.view_centered").into();
                        ui.close_menu();
                    }
                    if ui.button(self.t("view.fit")).clicked() {
                        self.fit_view();
                        self.status = self.t("status.view_fit").into();
                        ui.close_menu();
                    }
                    if ui.button(self.t("view.shortcuts")).clicked() {
                        self.show_cheatsheet = true;
                        ui.close_menu();
                    }
                    let palette_lbl = self.t("view.palette");
                    let analyse_lbl = self.t("view.analyse");
                    let zoom_lbl = self.t("view.zoom");
                    ui.checkbox(&mut self.show_palette, palette_lbl);
                    ui.checkbox(&mut self.show_analyse, analyse_lbl);
                    ui.add(egui::Slider::new(&mut self.cell_px, 8.0..=48.0).text(zoom_lbl));
                });
                ui.menu_button(self.t("menu.language"), |ui| {
                    for lang in Language::ALL {
                        let selected = self.language == lang;
                        if ui
                            .selectable_label(selected, lang.label())
                            .clicked()
                        {
                            self.language = lang;
                            self.save_config();
                            self.status = self.tf(
                                "status.lang_changed",
                                &[("lang", lang.label())],
                            );
                            ui.close_menu();
                        }
                    }
                });
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
            });

            // Row 2: tools + region — always one line.
            ui.horizontal(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                if ui
                    .button(RichText::new(self.t("toolbar.copy")).size(15.0))
                    .on_hover_text(self.t("toolbar.copy_tip"))
                    .clicked()
                {
                    if self.copy_selection_to_clipboard() {
                        self.sync_os_clipboard_marker(ctx);
                    }
                }
                if ui
                    .add_enabled(
                        !self.clipboard.is_empty(),
                        egui::Button::new(RichText::new(self.t("lib.paste")).size(15.0)),
                    )
                    .on_hover_text(self.t("edit.paste_hand"))
                    .clicked()
                {
                    self.arm_clipboard_in_hand();
                }
                ui.separator();
                // 45°-Modus: UI disabled for now (unreliable; low priority).
                self.allow_45_deg = false;
                let mut forty_five_off = false;
                ui.add_enabled(
                    false,
                    egui::Checkbox::new(
                        &mut forty_five_off,
                        RichText::new(self.t("toolbar.45")).size(15.0),
                    ),
                )
                .on_hover_text(self.t("toolbar.45_tip"));
                ui.separator();
                ui.label(RichText::new(self.t("region")).size(15.0).strong());
                for region in RegionFilter::ALL {
                    let selected = self.region_filter == region;
                    if ui
                        .selectable_label(
                            selected,
                            RichText::new(region.label_lang(self.language)).size(15.0),
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
                        self.refresh_residence_tiers_for_region();
                    }
                }
            });

            // Row 3: status + controls hint.
            ui.horizontal(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.label(RichText::new(&self.status).size(14.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(self.t("canvas.controls"))
                            .size(11.0)
                            .weak(),
                    );
                });
            });
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
        // Must match draw-time `content_entries` / `show_mid`. Setting infra_entry on a
        // leaf Building (road) used to reserve mid height with nothing drawn → blank
        // bottom row and the strip visually "jumped" up one row.
        let mid_preview_entries: Vec<&MenuEntry> =
            if let Some(infra_ei) = self.buildmenu_infra_entry {
                tabs_preview
                    .iter()
                    .find(|t| t.is_infrastructure())
                    .and_then(|t| t.entries.get(infra_ei))
                    .and_then(|e| match e {
                        MenuEntry::Category { entries, .. } => Some(entries.as_slice()),
                        _ => None,
                    })
                    .unwrap_or(&[])
                    .iter()
                    .collect()
            } else if need_mode_preview {
                tabs_preview
                    .get(tab_idx_preview)
                    .map(|t| t.entries.iter().collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        let show_mid_preview = !mid_preview_entries.is_empty()
            || self
                .residence_tier_guids
                .as_ref()
                .is_some_and(|c| c.len() > 1);
        // Deep (3rd) row: selected mid entry is a chain/category with children.
        // Residence tier mid is leaf buildings — never opens a deep row.
        let show_deep_preview = show_mid_preview
            && self.residence_tier_guids.is_none()
            && self.buildmenu_entry.is_some_and(|ei| {
                matches!(
                    mid_preview_entries.get(ei),
                    Some(MenuEntry::Chain { buildings, .. }) if !buildings.is_empty()
                ) || matches!(
                    mid_preview_entries.get(ei),
                    Some(MenuEntry::Category { entries, .. }) if !entries.is_empty()
                )
            });
        // Bottom strip + optional mid + optional deep row (+ gaps).
        const BUILD_ROW_H: f32 = 60.0;
        const BUILD_ROW_GAP: f32 = 4.0;
        // Extra slack: Frame margins + item_spacing must not trigger a scrollbar
        // (scrollbar insets rows horizontally → "shift" when deep opens).
        let build_bar_h: f32 = BUILD_STRIP_H
            + if show_mid_preview {
                BUILD_ROW_H + BUILD_ROW_GAP
            } else {
                0.0
            }
            + if show_deep_preview {
                BUILD_ROW_H + BUILD_ROW_GAP
            } else {
                0.0
            }
            + 8.0;
        // Library tree + stamps of the open pack.
        egui::SidePanel::left("stamps")
            .resizable(true)
            .default_width(240.0)
            .show(ctx, |ui| {
                ui.heading(self.t("lib.title"));
                ui.label(
                    RichText::new(self.library_root.display().to_string())
                        .small()
                        .weak(),
                );
                if self.library_is_game_stamps() {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 120, 40),
                        self.t("lib.sync_warning"),
                    );
                    if ui.button(self.t("file.use_safe_library")).clicked() {
                        self.use_safe_library();
                    }
                }
                ui.label(RichText::new(self.t("lib.select_hint")).small().weak());
                let filter_hint = self.t("lib.filter_ctrlf");
                let filter_resp = ui.add(
                    egui::TextEdit::singleline(&mut self.stamp_list_filter)
                        .hint_text(filter_hint),
                );
                if self.focus_library_filter {
                    self.focus_library_filter = false;
                    filter_resp.request_focus();
                }
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
                        // Hide folders that only contain packs for other regions
                        // (or that don't match the text filter). Truly empty folders stay visible.
                        if packs.is_empty() {
                            let has_any_packs = self.library_packs.iter().any(|p| {
                                let f = if p.folder.is_empty() {
                                    "(Root)".to_string()
                                } else {
                                    p.folder.clone()
                                };
                                f == folder
                            });
                            if has_any_packs || !text_filter.is_empty() {
                                continue;
                            }
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
                                            if ui.button(self.t("lib.open")).clicked() {
                                                lib_action = Some(LibraryAction::OpenStamp {
                                                    pack: pack_path.clone(),
                                                    name: stamp_name.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button(self.t("lib.rename")).clicked() {
                                                lib_action = Some(LibraryAction::RenameStamp {
                                                    pack: pack_path.clone(),
                                                    name: stamp_name.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button(self.t("lib.duplicate")).clicked() {
                                                lib_action = Some(LibraryAction::DuplicateStamp {
                                                    pack: pack_path.clone(),
                                                    name: stamp_name.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button(self.t("lib.copy")).clicked() {
                                                lib_action = Some(LibraryAction::CopyStamp {
                                                    pack: pack_path.clone(),
                                                    name: stamp_name.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button(self.t("lib.copy_pack")).clicked() {
                                                lib_action = Some(LibraryAction::CopyPack {
                                                    path: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button(self.t("lib.delete")).clicked() {
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
                                                    if ui.button(self.t("lib.open")).clicked() {
                                                        lib_action =
                                                            Some(LibraryAction::OpenStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            });
                                                        ui.close_menu();
                                                    }
                                                    if ui.button(self.t("lib.rename")).clicked() {
                                                        lib_action =
                                                            Some(LibraryAction::RenameStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            });
                                                        ui.close_menu();
                                                    }
                                                    if ui.button(self.t("lib.duplicate")).clicked() {
                                                        lib_action = Some(
                                                            LibraryAction::DuplicateStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            },
                                                        );
                                                        ui.close_menu();
                                                    }
                                                    if ui.button(self.t("lib.copy")).clicked() {
                                                        lib_action =
                                                            Some(LibraryAction::CopyStamp {
                                                                pack: stamp_pack.clone(),
                                                                name: stamp_name.clone(),
                                                            });
                                                        ui.close_menu();
                                                    }
                                                    ui.separator();
                                                    if ui.button(self.t("lib.delete")).clicked() {
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
                                            if ui.button(self.t("lib.new_stamp_in_pack")).clicked() {
                                                lib_action = Some(LibraryAction::NewStampInPack {
                                                    pack: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui
                                                .add_enabled(
                                                    can_paste,
                                                    egui::Button::new(self.t("lib.paste")),
                                                )
                                                .clicked()
                                            {
                                                lib_action = Some(LibraryAction::PasteIntoPack {
                                                    pack: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            if ui.button(self.t("lib.copy_pack")).clicked() {
                                                lib_action = Some(LibraryAction::CopyPack {
                                                    path: pack_path.clone(),
                                                });
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            if ui.button(self.t("lib.delete_pack")).clicked() {
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
                            if ui.button(self.t("lib.new_stamp")).clicked() {
                                lib_action = Some(LibraryAction::NewStampPack {
                                    folder: folder_for_menu.clone(),
                                });
                                ui.close_menu();
                            }
                            if ui.button(self.t("lib.new_folder")).clicked() {
                                lib_action = Some(LibraryAction::NewFolder {
                                    parent: folder_for_menu.clone(),
                                });
                                ui.close_menu();
                            }
                            if ui
                                .add_enabled(can_paste, egui::Button::new(self.t("lib.paste")))
                                .clicked()
                            {
                                lib_action = Some(LibraryAction::Paste {
                                    target_folder: folder_for_menu.clone(),
                                });
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button(self.t("lib.delete_folder")).clicked() {
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
                        if ui.button(self.t("lib.new_stamp")).clicked() {
                            lib_action = Some(LibraryAction::NewStampPack {
                                folder: root_folder.clone(),
                            });
                            ui.close_menu();
                        }
                        if ui.button(self.t("lib.new_folder")).clicked() {
                            lib_action = Some(LibraryAction::NewFolder {
                                parent: root_folder.clone(),
                            });
                            ui.close_menu();
                        }
                        if ui
                            .add_enabled(can_paste, egui::Button::new(self.t("lib.paste")))
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
                ui.heading(self.t("search.title"));
                ui.label(
                    RichText::new(self.t("search.hint"))
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
                    self.activate_menu_building(guid, &label);
                }
            });
        }

        if self.show_analyse {
        let mut pending_residence_tier: Option<u32> = None;
        egui::SidePanel::right("stats").resizable(true).default_width(280.0).show(ctx, |ui| {
            let lang = self.language;
            ui.horizontal(|ui| {
                ui.heading(Self::t_lang(lang, "analyse.title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        self.show_analyse = false;
                    }
                });
            });
            // Rebuild the (expensive) city analysis only when the stamp changed;
            // otherwise reuse the cache so panning/hovering stays smooth.
            self.ensure_city_analysis();
            if let Some(open) = &self.open {
                let placed: Vec<_> = open.stamp.all_draw_buildings().cloned().collect();
                let stats = layout_stats_region(
                    &placed,
                    &self.catalog,
                    Some(&open.stamp.document),
                    open.region_guid,
                );
                ui.group(|ui| {
                    ui.label(RichText::new(Self::t_lang(lang, "analyse.layout")).strong());
                    ui.label(Self::tf_lang(
                        lang,
                        "analyse.buildings_fields",
                        &[
                            ("b", &open.stamp.buildings.len().to_string()),
                            ("mods", &open.stamp.polygon_modules.len().to_string()),
                            ("cells", &open.stamp.polygon_cells.len().to_string()),
                        ],
                    ));
                    ui.label(Self::tf_lang(
                        lang,
                        "analyse.unique_guids",
                        &[("n", &stats.unique_guids.to_string())],
                    ));
                    ui.label(Self::tf_lang(
                        lang,
                        "analyse.occupied",
                        &[("n", &stats.occupied_cells.to_string())],
                    ));
                    ui.label(Self::tf_lang(
                        lang,
                        "analyse.bbox",
                        &[("n", &stats.bbox_cells.to_string())],
                    ));
                    ui.label(Self::tf_lang(
                        lang,
                        "analyse.empty_bbox",
                        &[("n", &stats.empty_cells_in_bbox.to_string())],
                    ));
                    ui.label(Self::tf_lang(
                        lang,
                        "analyse.fill_ratio",
                        &[("pct", &format!("{:.1}", stats.fill_ratio * 100.0))],
                    ));
                    ui.label(Self::tf_lang(
                        lang,
                        "analyse.roads",
                        &[
                            ("n", &stats.road_cells.to_string()),
                            ("dirt", &stats.dirt_cells.to_string()),
                            ("stone", &stats.stone_cells.to_string()),
                        ],
                    ));
                });

                // Read precomputed results from the cache built by
                // `ensure_city_analysis()` above (cheap clone vs. recompute).
                let analysis = self
                    .city_analysis
                    .as_ref()
                    .expect("city analysis ensured above");
                let areas = analysis.areas.clone();
                let chain = analysis.chain.clone();
                let city_consumption = analysis.city_consumption.clone();
                let city_services = analysis.city_services.clone();
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label(
                        RichText::new(Self::t_lang(lang, "analyse.city_consumption")).strong(),
                    );
                    if city_consumption.is_empty() {
                        ui.label(
                            RichText::new(Self::t_lang(lang, "analyse.no_residences")).weak(),
                        );
                    } else {
                        egui::Grid::new("city_consumption_grid")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(Self::t_lang(lang, "analyse.product"));
                                ui.label(Self::t_lang(lang, "analyse.consumption_houses"));
                                ui.label(Self::t_lang(lang, "analyse.consumption_rate"));
                                ui.end_row();
                                for row in &city_consumption {
                                    ui.label(&row.label);
                                    ui.label(row.houses.to_string());
                                    ui.label(format!("{:.2}", row.rate_sum));
                                    ui.end_row();
                                }
                            });
                    }
                });
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label(
                        RichText::new(Self::t_lang(lang, "analyse.service_coverage")).strong(),
                    );
                    if city_services.is_empty() {
                        ui.label(
                            RichText::new(Self::t_lang(lang, "analyse.no_residences")).weak(),
                        );
                    } else {
                        for row in &city_services {
                            let unmet = row.houses_needing.saturating_sub(row.houses_met);
                            let color = if unmet == 0 {
                                Color32::from_rgb(90, 180, 90)
                            } else {
                                Color32::from_rgb(220, 140, 70)
                            };
                            ui.horizontal(|ui| {
                                if let Some(icon) = row.icon.as_deref() {
                                    let tex = self.ui_icons.get_or_placeholder(
                                        ui.ctx(),
                                        Some(icon),
                                        [24, 24],
                                        [40, 50, 70],
                                    );
                                    let side = 16.0_f32;
                                    let (rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                                    Self::paint_tex(ui, &tex, rect, Color32::WHITE);
                                }
                                ui.label(&row.label);
                                ui.colored_label(
                                    color,
                                    Self::tf_lang(
                                        lang,
                                        "analyse.service_houses",
                                        &[
                                            ("met", &row.houses_met.to_string()),
                                            ("need", &row.houses_needing.to_string()),
                                        ],
                                    ),
                                );
                            });
                        }
                    }
                });
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label(RichText::new(Self::t_lang(lang, "analyse.chain")).strong());
                    ui.label(Self::tf_lang(lang, 
                        "analyse.producers",
                        &[("n", &chain.producers.to_string())],
                    ));
                    if chain.all_balanced {
                        ui.colored_label(
                            Color32::from_rgb(80, 180, 80),
                            Self::t_lang(lang, "analyse.chain_ok"),
                        );
                    } else {
                        ui.colored_label(
                            Color32::from_rgb(220, 120, 60),
                            Self::tf_lang(
                                lang,
                                "analyse.imbalance",
                                &[("detail", Self::t_lang(lang, "analyse.imbalance_detail"))],
                            ),
                        );
                    }
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        egui::Grid::new("chain_grid").striped(true).show(ui, |ui| {
                            ui.label(Self::t_lang(lang, "analyse.product"));
                            ui.label(Self::t_lang(lang, "analyse.in_per_min"));
                            ui.label(Self::t_lang(lang, "analyse.out_per_min"));
                            ui.label(Self::t_lang(lang, "analyse.net"));
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
                        ui.label(RichText::new(Self::t_lang(lang, "analyse.area_eff")).strong());
                        ui.label(
                            RichText::new(Self::t_lang(lang, "analyse.area_hint"))
                                .small()
                                .weak(),
                        );
                        egui::Grid::new("area_eff_grid")
                            .num_columns(4)
                            .spacing([8.0, 2.0])
                            .show(ui, |ui| {
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.building")).strong());
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.free_need")).strong());
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.blocker")).strong());
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.eff")).strong());
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
                                            Self::tf_lang(
                                                lang,
                                                "analyse.contested",
                                                &[
                                                    ("b", &format!("{:.0}", a.blocked_buildings)),
                                                    ("r", &format!("{:.0}", a.blocked_roads)),
                                                    ("f", &format!("{:.0}", a.blocked_fields)),
                                                    ("c", &format!("{:.0}", a.contested_tiles)),
                                                ],
                                            )
                                        } else {
                                            Self::tf_lang(
                                                lang,
                                                "analyse.blockers_only",
                                                &[
                                                    ("b", &format!("{:.0}", a.blocked_buildings)),
                                                    ("r", &format!("{:.0}", a.blocked_roads)),
                                                    ("f", &format!("{:.0}", a.blocked_fields)),
                                                ],
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
                        let label = self.catalog.label(guid, lang);
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
                        let residence_report: Option<ResidenceNeedReport> =
                            def.filter(|d| d.is_residence()).map(|d| {
                                let provided = if matches!(cov_mode, CoverageMode::Incoming) {
                                    provided_need_guids(&cov_hits)
                                } else {
                                    HashSet::new()
                                };
                                report_for_def(d, &provided)
                            });
                        let residence_tiers: Vec<u32> = def
                            .filter(|d| d.is_residence())
                            .map(|_| self.catalog.residence_chain(guid))
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
                            ui.label(RichText::new(Self::t_lang(lang, "analyse.selection")).strong());
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
                                .map(|mb| self.catalog.label(mb.guid, lang))
                                .unwrap_or_else(|| "?".into());
                                ui.label(Self::tf_lang(
                                    lang,
                                    "analyse.complex",
                                    &[
                                        ("oid", &oid.to_string()),
                                        ("n", &group_n.to_string()),
                                        ("main", &main_lbl),
                                    ],
                                ));
                                ui.label(
                                    RichText::new(Self::t_lang(lang, "analyse.complex_hint"))
                                        .small()
                                        .weak(),
                                );
                            } else {
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.no_complex")).weak());
                            }
                            match cov_mode {
                                CoverageMode::Outgoing => {
                                    ui.label(Self::tf_lang(
                                        lang,
                                        "analyse.coverage_out",
                                        &[
                                            ("n", &cov_n.to_string()),
                                            ("street", &format!("{cov_s:.0}")),
                                            ("radius", &format!("{cov_r:.0}")),
                                        ],
                                    ));
                                }
                                CoverageMode::Incoming => {
                                    ui.label(Self::tf_lang(
                                        lang,
                                        "analyse.reached",
                                        &[("n", &cov_n.to_string())],
                                    ));
                                }
                                CoverageMode::None => {
                                    ui.label(RichText::new(Self::t_lang(lang, "analyse.no_service")).weak());
                                }
                            }
                            if let Some(scope) = &effect_scope {
                                ui.label(format!("EffectScope: {scope}"));
                            }
                            if let Some(k) = &service_kind {
                                ui.label(Self::tf_lang(lang, "analyse.kind", &[("k", k)]));
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
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.resource_area")).strong());
                                let color = if a.efficiency >= 0.999 {
                                    Color32::from_rgb(90, 180, 90)
                                } else if a.efficiency >= 0.7 {
                                    Color32::from_rgb(220, 160, 60)
                                } else {
                                    Color32::from_rgb(220, 100, 80)
                                };
                                ui.colored_label(
                                    color,
                                    Self::tf_lang(
                                        lang,
                                        "analyse.eff_need",
                                        &[
                                            ("have", &format!("{:.0}", a.have)),
                                            ("need", &format!("{:.0}", a.need)),
                                            ("pct", &format!("{:.0}", a.efficiency * 100.0)),
                                        ],
                                    ),
                                );
                                ui.label(Self::tf_lang(
                                    lang,
                                    "analyse.class_split",
                                    &[
                                        ("group", &a.competition_group),
                                        ("excl", &format!("{:.0}", a.have_exclusive)),
                                        ("cont", &format!("{:.0}", a.contested_tiles)),
                                        ("won", &format!("{:.0}", a.have_contested_won)),
                                    ],
                                ));
                                ui.label(Self::tf_lang(
                                    lang,
                                    "analyse.radius_block",
                                    &[
                                        ("radius", &format!("{:.0}", a.radius)),
                                        ("raw", &format!("{:.0}", a.raw_in_radius)),
                                        ("b", &format!("{:.0}", a.blocked_buildings)),
                                        ("r", &format!("{:.0}", a.blocked_roads)),
                                        ("f", &format!("{:.0}", a.blocked_fields)),
                                    ],
                                ));
                            }
                            if !effect_bonuses.is_empty() {
                                ui.separator();
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.bonuses")).strong());
                                for b in &effect_bonuses {
                                    ui.label(format!("  {} {:+}", b.attr, b.value));
                                }
                            }
                            if matches!(cov_mode, CoverageMode::Incoming) && !cov_hits.is_empty()
                            {
                                ui.separator();
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.sources")).strong());
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
                            if residence_tiers.len() > 1 {
                                ui.separator();
                                ui.label(
                                    RichText::new(Self::t_lang(lang, "analyse.residence_tier"))
                                        .strong(),
                                );
                                ui.horizontal_wrapped(|ui| {
                                    for &tg in &residence_tiers {
                                        let lbl = self.catalog.label(tg, lang);
                                        let sel = tg == guid;
                                        if ui.selectable_label(sel, &lbl).clicked() && tg != guid
                                        {
                                            pending_residence_tier = Some(tg);
                                        }
                                    }
                                });
                            }
                            if let Some(report) = &residence_report {
                                ui.separator();
                                ui.label(
                                    RichText::new(Self::t_lang(lang, "analyse.service_needs"))
                                        .strong(),
                                );
                                ui.label(Self::tf_lang(
                                    lang,
                                    "analyse.services_badge",
                                    &[
                                        ("met", &report.met_count().to_string()),
                                        ("total", &report.service_count().to_string()),
                                    ],
                                ));
                                for s in &report.services {
                                    let mark = if s.met { "[x]" } else { "[ ]" };
                                    let color = if s.met {
                                        Color32::from_rgb(90, 180, 90)
                                    } else {
                                        Color32::from_rgb(220, 100, 80)
                                    };
                                    ui.horizontal(|ui| {
                                        if let Some(icon) = s.need.icon.as_deref() {
                                            let tex = self.ui_icons.get_or_placeholder(
                                                ui.ctx(),
                                                Some(icon),
                                                [24, 24],
                                                [40, 50, 70],
                                            );
                                            let side = 16.0_f32;
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::splat(side),
                                                Sense::hover(),
                                            );
                                            Self::paint_tex(ui, &tex, rect, Color32::WHITE);
                                        }
                                        ui.colored_label(color, mark);
                                        ui.colored_label(color, s.need.label(lang));
                                    });
                                }
                            }
                            if need > 0 {
                                ui.separator();
                                ui.label(RichText::new(Self::t_lang(lang, "analyse.modules")).strong());
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
                ui.label(self.t("analyse.no_stamp"));
            }
        });
        if let Some(tg) = pending_residence_tier {
            self.upgrade_selected_residence(tg);
        }
        } // show_analyse

        // F1 toggles the keyboard-shortcut cheatsheet.
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.show_cheatsheet = !self.show_cheatsheet;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // Staged Escape: cheatsheet → radial → hand → deselect.
            // Only one stage per press so a hand just cleared keeps the selection.
            if self.show_cheatsheet {
                self.show_cheatsheet = false;
            } else if self.radial.is_some() {
                self.dismiss_radial();
            } else if self.hand_busy() {
                self.clear_hand();
            } else {
                self.clear_canvas_selection();
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
                // Canvas selection always wins over leftover library selection.
                // Library Entf only when nothing on the canvas is selected
                // (and not via Bearbeiten → Löschen, which is canvas-only).
                let canvas_sel = self.selected_building.is_some()
                    || !self.selected_buildings.is_empty()
                    || !self.selected_roads.is_empty()
                    || !self.selected_fields.is_empty();
                if menu_delete || canvas_sel {
                    self.delete_selection(group_ops);
                } else if !self.library_selection.is_empty() {
                    let _ = self.delete_library_selection();
                }
            }
            // Land/Marsh footprint (V): while placing, or on selected building with size_marsh.
            if !mods.ctrl && !mods.command && ctx.input(|i| i.key_pressed(egui::Key::V)) {
                if self.placing_guid.is_some() {
                    self.toggle_place_variation();
                } else if let Some(idx) = self.selected_building {
                    self.toggle_selected_variation(idx);
                }
            }
            // Rotate: hand (building/clipboard group), multi-select, or single building.
            // Shift+R = reverse.
            if !mods.ctrl
                && !mods.command
                && ctx.input(|i| i.key_pressed(egui::Key::R))
            {
                self.rotate_selection(mods.shift);
            }
            // Arrow nudge: move selection by 1 tile (Shift = 5).
            if !mods.ctrl && !mods.command {
                let step = if mods.shift { 5 } else { 1 };
                let mut dx = 0i32;
                let mut dy = 0i32;
                // World is Y-up (north = +Y): Up = +Y, Down = −Y.
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                    dx -= step;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                    dx += step;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    dy += step;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    dy -= step;
                }
                if (dx != 0 || dy != 0) && self.has_movable_selection() {
                    self.nudge_selection(dx, dy);
                }
            }
            // Tool quick-keys (no Ctrl): 1 dirt road, 2 stone road, 3 move, D demolish.
            if !mods.ctrl && !mods.command {
                if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
                    self.apply_radial_action(RadialAction::RoadDirt);
                } else if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
                    self.apply_radial_action(RadialAction::RoadStone);
                } else if ctx.input(|i| i.key_pressed(egui::Key::Num3)) {
                    self.apply_radial_action(RadialAction::Move);
                } else if ctx.input(|i| i.key_pressed(egui::Key::D)) {
                    self.apply_radial_action(RadialAction::Demolish);
                }
            }
            // Panel toggles: B = building search, L = analysis.
            if !mods.ctrl && !mods.command {
                if ctx.input(|i| i.key_pressed(egui::Key::B)) {
                    self.show_palette = !self.show_palette;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::L)) {
                    self.show_analyse = !self.show_analyse;
                }
            }
            // Focus / center view (Shift+F = fit to stamp).
            if !mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::F)) {
                if mods.shift {
                    self.fit_view();
                    self.status = self.t("status.view_fit").into();
                } else {
                    self.center_view();
                    self.status = self.t("status.view_centered").into();
                }
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
            // Remember the viewport for zoom-to-fit (Shift+F) next frame.
            self.last_canvas_rect = Some(rect);
            // Build-bar band (same math as the overlay below) — canvas must not
            // place/drag through mid/deep rows.
            let build_band_top = (rect.max.y - build_bar_h).max(rect.min.y + 40.0);
            let over_build_bar = response
                .hover_pos()
                .or_else(|| response.interact_pointer_pos())
                .is_some_and(|p| p.y >= build_band_top);
            if let Some(pos) = response.hover_pos() {
                if !over_build_bar {
                    self.last_pointer_world = Some(self.screen_to_world(rect, pos));
                }
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
                    // Zoom toward the cursor: keep the world point under the pointer fixed.
                    let anchor = response.hover_pos();
                    let world_at = anchor.map(|p| self.screen_to_world(rect, p));
                    self.cell_px = (self.cell_px + scroll * 0.05).clamp(8.0, 48.0);
                    if let (Some(p), Some((wx, wy))) = (anchor, world_at) {
                        // Solve world_to_screen(rect, wx, wy) == p for pan (Y-up flip).
                        self.pan.x = p.x - rect.center().x - wx * self.cell_px;
                        self.pan.y = p.y - rect.center().y + wy * self.cell_px;
                    }
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

            // Aqueducts (MeshGraph layer) — MinGround (Boden) vs MaxGround (hoch über Straße).
            if let Some(doc) = self.open.as_ref().map(|o| &o.stamp.document) {
                for (x, y, guid) in mesh_road_cells(doc) {
                    if !is_aqueduct_brush_guid(guid) {
                        continue;
                    }
                    let r = self.cell_screen_rect(rect, x, y);
                    let (fill, stroke) = if is_aqueduct_maxground_guid(guid) {
                        // Same blue family as MinGround; slightly brighter = raised over road.
                        (
                            Color32::from_rgba_unmultiplied(110, 195, 240, 185),
                            Color32::from_rgb(160, 225, 255),
                        )
                    } else {
                        (
                            Color32::from_rgba_unmultiplied(70, 160, 210, 160),
                            Color32::from_rgb(120, 200, 230),
                        )
                    };
                    painter.rect_filled(r, 0.0, fill);
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(1.0_f32, stroke),
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
                        let brush_g = self.road_brush_guid.unwrap_or(0);
                        let brush_aqueduct = is_aqueduct_brush_guid(brush_g);
                        let brush_aq_max = is_aqueduct_maxground_guid(brush_g);
                        let brush_stone = self.catalog.building(brush_g as u32).is_some_and(|d| {
                            d.road_kind.as_deref() == Some("stone")
                                || d.logistic_speed_factor.unwrap_or(0.0) >= 1.4
                        });
                        let preview = if paint_on {
                            if brush_aq_max {
                                Color32::from_rgba_unmultiplied(110, 195, 240, 180)
                            } else if brush_aqueduct {
                                Color32::from_rgba_unmultiplied(70, 160, 210, 170)
                            } else if brush_stone {
                                // Match placed Stone tint (gray), not Dirt ochre.
                                Color32::from_rgba_unmultiplied(150, 150, 165, 160)
                            } else {
                                Color32::from_rgba_unmultiplied(200, 170, 90, 150)
                            }
                        } else {
                            Color32::from_rgba_unmultiplied(220, 80, 80, 140)
                        };
                        for (tx, ty) in path {
                            let r = self.cell_screen_rect(rect, tx, ty);
                            painter.rect_filled(r, 0.0, preview);
                        }
                    }
                    StrokeKind::Field { paint_on } => {
                        let (min_x, min_y, max_x, max_y) =
                            rect_bounds(stroke.start, stroke.cursor);
                        let area = rect_cells(min_x, min_y, max_x, max_y);
                        let owner = self.field_brush_owner;
                        let mut ok: HashSet<(i32, i32)> = HashSet::new();
                        if paint_on {
                            if let (Some(oid), Some(open)) = (owner, self.open.as_ref()) {
                                let mut blocked: HashSet<(i32, i32)> = self
                                    .coverage
                                    .roads
                                    .keys()
                                    .map(|c| (c.x, c.y))
                                    .collect();
                                blocked.extend(foreign_field_cells(&open.stamp, oid));
                                let region = open.region_guid;
                                let drag = read_mesh_info(&open.stamp.document)
                                    .map(|i| i.drag_center)
                                    .unwrap_or((0.0, 0.0));
                                for b in &open.stamp.buildings {
                                    let def = self.catalog.building(b.guid);
                                    if def.is_some_and(|d| d.category == "street") {
                                        continue;
                                    }
                                    for c in footprint_cells_for_region(b, def, region, drag) {
                                        blocked.insert((c.x, c.y));
                                    }
                                }
                                let mut seeds = self.farm_main_connect_seeds(oid);
                                seeds.extend(field_cells_of_owner(&open.stamp, oid));
                                let cand: HashSet<(i32, i32)> = area
                                    .iter()
                                    .copied()
                                    .filter(|c| !blocked.contains(c))
                                    .collect();
                                ok = connected_field_subset(&seeds, &cand);
                            }
                        } else {
                            ok = area.iter().copied().collect();
                        }
                        for (tx, ty) in &area {
                            let r = self.cell_screen_rect(rect, *tx, *ty);
                            let fill = if !paint_on {
                                Color32::from_rgba_unmultiplied(220, 100, 70, 120)
                            } else if ok.contains(&(*tx, *ty)) {
                                Color32::from_rgba_unmultiplied(110, 180, 90, 130)
                            } else {
                                // Blocked / foreign / not connected to farm.
                                Color32::from_rgba_unmultiplied(180, 80, 80, 70)
                            };
                            painter.rect_filled(r, 0.0, fill);
                        }
                        let outline = self.cells_screen_rect(rect, min_x, min_y, max_x, max_y);
                        painter.rect_stroke(
                            outline,
                            0.0,
                            Stroke::new(
                                1.75_f32,
                                if paint_on {
                                    Color32::from_rgb(140, 220, 120)
                                } else {
                                    Color32::from_rgb(240, 120, 80)
                                },
                            ),
                            egui::StrokeKind::Outside,
                        );
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
                        let outline = self.cells_screen_rect(rect, min_x, min_y, max_x, max_y);
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
                        let outline = self.cells_screen_rect(rect, min_x, min_y, max_x, max_y);
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
                            let from_clipboard = self.hand_from_clipboard;
                            let place_dir = self.place_direction;
                            let (bw, bh) = if from_clipboard {
                                self.clipboard
                                    .iter()
                                    .find(|c| c.is_primary)
                                    .map(|p| self.clipboard_oriented_size(p))
                                    .unwrap_or_else(|| {
                                        let (w0, h0) = self.building_size(guid, variation);
                                        oriented_size(w0, h0, place_dir)
                                    })
                            } else {
                                let (w0, h0) = self.building_size(guid, variation);
                                oriented_size(w0, h0, place_dir)
                            };
                            let bw_i = bw.round().max(1.0) as i32;
                            let bh_i = bh.round().max(1.0) as i32;
                            let (step_w, step_h) = if from_clipboard {
                                self.clipboard_group_span()
                            } else {
                                (bw_i, bh_i)
                            };
                            let origins = place_rect_top_lefts(
                                stroke.start,
                                stroke.cursor,
                                step_w,
                                step_h,
                            );
                            let mut centers = Vec::with_capacity(origins.len());
                            for (gx, gy) in &origins {
                                if from_clipboard {
                                    if let Some(p) =
                                        self.clipboard.iter().find(|c| c.is_primary)
                                    {
                                        centers.push(self.place_pos_for_tl(
                                            *gx, *gy, p.guid, p.variation, p.direction,
                                        ));
                                    } else {
                                        centers.push(place_center_from_top_left(
                                            *gx, *gy, bw_i, bh_i,
                                        ));
                                    }
                                } else {
                                    centers.push(self.place_pos_for_tl(
                                        *gx, *gy, guid, variation, place_dir,
                                    ));
                                }
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
                            // Faint raw drag AABB (what the mouse covers).
                            let (dmin_x, dmin_y, dmax_x, dmax_y) =
                                rect_bounds(stroke.start, stroke.cursor);
                            let raw_outline = self.cells_screen_rect(
                                rect, dmin_x, dmin_y, dmax_x, dmax_y,
                            );
                            painter.rect_stroke(
                                raw_outline,
                                0.0,
                                Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgba_unmultiplied(140, 210, 255, 90),
                                ),
                                egui::StrokeKind::Outside,
                            );
                            let mut union_min_x = i32::MAX;
                            let mut union_max_x = i32::MIN;
                            let mut union_min_y = i32::MAX;
                            let mut union_max_y = i32::MIN;
                            for (sx, sy) in &centers {
                                let parts: Vec<(u32, f32, f32, i32, Option<u32>)> =
                                    if from_clipboard {
                                        self.clipboard_building_centers(*sx, *sy)
                                            .into_iter()
                                            .map(|(c, cx, cy)| {
                                                (c.guid, cx, cy, c.direction, c.variation)
                                            })
                                            .collect()
                                    } else {
                                        vec![(guid, *sx, *sy, place_dir, variation)]
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
                                    let cells =
                                        footprint_cells_for_region(&ghost, def, region, drag);
                                    let mut min_x = i32::MAX;
                                    let mut max_x = i32::MIN;
                                    let mut min_y = i32::MAX;
                                    let mut max_y = i32::MIN;
                                    for c in &cells {
                                        min_x = min_x.min(c.x);
                                        max_x = max_x.max(c.x);
                                        min_y = min_y.min(c.y);
                                        max_y = max_y.max(c.y);
                                        union_min_x = union_min_x.min(c.x);
                                        union_max_x = union_max_x.max(c.x);
                                        union_min_y = union_min_y.min(c.y);
                                        union_max_y = union_max_y.max(c.y);
                                        let r = self.cell_screen_rect(rect, c.x, c.y);
                                        painter.rect_filled(r, 0.0, fill);
                                    }
                                    if def.is_some_and(|d| d.is_aqueduct_cistern())
                                        && min_x <= max_x
                                    {
                                        self.paint_cistern_inlet_hint(
                                            &painter, rect, min_x, max_x, min_y, max_y, g_dir,
                                        );
                                    }
                                }
                                if from_clipboard {
                                    for (rx, ry, _) in self.clipboard_road_cells(*sx, *sy) {
                                        union_min_x = union_min_x.min(rx);
                                        union_max_x = union_max_x.max(rx);
                                        union_min_y = union_min_y.min(ry);
                                        union_max_y = union_max_y.max(ry);
                                        painter.rect_filled(
                                            self.cell_screen_rect(rect, rx, ry),
                                            0.0,
                                            road_fill,
                                        );
                                    }
                                }
                            }
                            // Strong outline around what will actually be placed.
                            if union_min_x <= union_max_x {
                                let placed_outline = self.cells_screen_rect(
                                    rect,
                                    union_min_x,
                                    union_min_y,
                                    union_max_x,
                                    union_max_y,
                                );
                                painter.rect_stroke(
                                    placed_outline,
                                    0.0,
                                    Stroke::new(
                                        2.0_f32,
                                        Color32::from_rgba_unmultiplied(160, 220, 255, 230),
                                    ),
                                    egui::StrokeKind::Outside,
                                );
                                painter.text(
                                    placed_outline.min + Vec2::new(6.0, 4.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("{center_count}×"),
                                    egui::FontId::proportional(13.0),
                                    Color32::from_rgb(200, 230, 255),
                                );
                            }
                        }
                    }
                }
            }

            if let Some(open) = self.open.clone() {
                let affected_set: std::collections::HashSet<usize> =
                    self.coverage.affected.iter().copied().collect();
                let hover_idx = response.hover_pos().and_then(|p| {
                    self.selectable_at_pointer(rect, &open.stamp.buildings, p)
                });
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
                let (fdx, fdy) = if self.dragging_fields {
                    self.drag_field_delta
                } else {
                    (0, 0)
                };
                // Tiles with no 4-path to their farm main → red warning (not auto-deleted).
                let mut disconnected: HashSet<(i32, i32)> = HashSet::new();
                {
                    let owners: HashSet<u64> = open
                        .stamp
                        .polygon_cells
                        .iter()
                        .filter_map(|c| c.complex_owner_id.filter(|o| *o != 0))
                        .collect();
                    for oid in owners {
                        let seeds = self.farm_main_connect_seeds(oid);
                        disconnected
                            .extend(disconnected_field_cells(&open.stamp, oid, &seeds));
                    }
                }
                for cell in &open.stamp.polygon_cells {
                    let selected = self.selected_fields.contains(&(cell.x, cell.y));
                    // While dragging, hide selected tiles at origin (ghost drawn at target).
                    if self.dragging_fields && selected {
                        continue;
                    }
                    let r = self.cell_screen_rect(rect, cell.x, cell.y);
                    let orphan = disconnected.contains(&(cell.x, cell.y));
                    let linked =
                        focus_owner.is_some() && cell.complex_owner_id == focus_owner;
                    let fill = if orphan {
                        Color32::from_rgba_unmultiplied(220, 60, 50, 130)
                    } else if selected {
                        Color32::from_rgba_unmultiplied(80, 160, 255, 110)
                    } else if linked {
                        Color32::from_rgba_unmultiplied(110, 150, 70, 90)
                    } else {
                        Color32::from_rgba_unmultiplied(75, 110, 60, 70)
                    };
                    painter.rect_filled(r, 0.0, fill);
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(
                            if orphan || selected || linked {
                                1.35_f32
                            } else {
                                0.75_f32
                            },
                            if orphan {
                                Color32::from_rgb(255, 100, 90)
                            } else if selected {
                                Color32::from_rgb(140, 200, 255)
                            } else if linked {
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
                // Ghost of selected field tiles while dragging.
                if self.dragging_fields {
                    for &(x, y) in &self.drag_field_origins {
                        let r = self.cell_screen_rect(rect, x + fdx, y + fdy);
                        painter.rect_filled(
                            r,
                            0.0,
                            Color32::from_rgba_unmultiplied(90, 180, 255, 120),
                        );
                        painter.rect_stroke(
                            r,
                            0.0,
                            Stroke::new(1.5_f32, Color32::from_rgb(160, 220, 255)),
                            egui::StrokeKind::Outside,
                        );
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
                    let is_cistern = def.is_some_and(|d| d.is_aqueduct_cistern());
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
                    let fill = if let Some(tier) = self.catalog.residence_tier_number(b.guid) {
                        if in_cov && !selected {
                            match self.coverage.mode {
                                CoverageMode::Outgoing => Color32::from_rgb(60, 110, 150),
                                CoverageMode::Incoming => Color32::from_rgb(120, 90, 150),
                                CoverageMode::None => Self::residence_tier_fill(tier, false, false),
                            }
                        } else {
                            Self::residence_tier_fill(tier, selected, false)
                        }
                    } else if selected {
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
                        Some(self.cells_screen_rect(rect, min_x, min_y, max_x, max_y))
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

                    // Cistern water inlet (Dir → side); aqueduct must touch this edge.
                    if is_cistern && !cells.is_empty() {
                        let min_x = cells.iter().map(|c| c.x).min().unwrap();
                        let max_x = cells.iter().map(|c| c.x).max().unwrap();
                        let min_y = cells.iter().map(|c| c.y).min().unwrap();
                        let max_y = cells.iter().map(|c| c.y).max().unwrap();
                        self.paint_cistern_inlet_hint(
                            &painter,
                            rect,
                            min_x,
                            max_x,
                            min_y,
                            max_y,
                            b.direction,
                        );
                    }

                    // Text only when no icon (or extremely zoomed-out fallback).
                    if !drew_icon && self.cell_px >= 12.0 {
                        let label = self.catalog.label(b.guid, self.language);
                        let (ew, eh) = oriented_size(w, h, b.direction);
                        let tier = self
                            .catalog
                            .residence_tier_tag(b.guid)
                            .map(|t| format!("{t} · "))
                            .unwrap_or_default();
                        let size_tag = if b.direction.rem_euclid(8) == 0 {
                            format!("{tier}{label} {}×{}", ew as i32, eh as i32)
                        } else {
                            format!(
                                "{tier}{label} {}×{} · Dir {}",
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
                if self.cell_px >= 14.0 {
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
                        let field_res =
                            field_resource_map(&open.stamp.polygon_cells, &self.catalog);
                        // Reuse the cached area-efficiency pass instead of
                        // recomputing it on every repaint (pan/select/hover).
                        self.ensure_area_analysis();
                        let area_hits: Vec<AreaEfficiency> = self
                            .area_analysis
                            .as_ref()
                            .map(|c| c.areas.clone())
                            .unwrap_or_default();
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
                                    let free = cell_is_free_for_group(
                                        reason, cell, group, &field_res,
                                    );
                                    let fill = if free {
                                        if contested.is_some_and(|m| m.contains_key(&cell)) {
                                            Color32::from_rgba_unmultiplied(
                                                230, 170, 50, if multi { 50 } else { 95 },
                                            )
                                        } else if matches!(reason, Some(BlockReason::Field)) {
                                            // Flower fields that count for this class.
                                            Color32::from_rgba_unmultiplied(
                                                90, 200, 120, if multi { 35 } else { 70 },
                                            )
                                        } else {
                                            Color32::from_rgba_unmultiplied(
                                                70, 180, 90, if multi { 28 } else { 55 },
                                            )
                                        }
                                    } else {
                                        match reason {
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
                                            None => Color32::from_rgba_unmultiplied(
                                                70, 180, 90, if multi { 28 } else { 55 },
                                            ),
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
                // Zoomed out: text chips bury the layout — use dots / hide until readable.
                match self.coverage.mode {
                    CoverageMode::Outgoing => {
                        if let Some(hit) = self.coverage.hits.first() {
                            let (r, g, bl) = kind_color(hit.service_kind.as_deref());
                            let color = Color32::from_rgb(r, g, bl);
                            for &ai in &self.coverage.affected {
                                let Some(tb) = open.stamp.buildings.get(ai) else {
                                    continue;
                                };
                                if self.cell_px >= 20.0 {
                                    let anchor =
                                        self.world_to_screen(rect, tb.x, tb.y - 0.9);
                                    let short: Vec<String> = hit
                                        .bonuses
                                        .iter()
                                        .take(2)
                                        .map(|b| self.format_bonus_chip(&b.attr, b.value))
                                        .collect();
                                    self.paint_bonus_label(&painter, anchor, &short, color);
                                } else if self.cell_px >= 10.0 {
                                    // Compact marker so coverage stays visible without clutter.
                                    let c = self.world_to_screen(rect, tb.x, tb.y);
                                    let rad = (self.cell_px * 0.22).clamp(2.5, 5.0);
                                    painter.circle_filled(
                                        c,
                                        rad,
                                        Color32::from_rgba_unmultiplied(r, g, bl, 220),
                                    );
                                    painter.circle_stroke(
                                        c,
                                        rad,
                                        Stroke::new(1.0_f32, Color32::from_rgb(240, 245, 255)),
                                    );
                                }
                            }
                        }
                    }
                    CoverageMode::Incoming => {
                        // One summary card — fine when zoomed out a bit; skip at min zoom.
                        if self.cell_px >= 14.0 {
                            if let Some(sel_i) = self.selected_building {
                                if let Some(sel_b) = open.stamp.buildings.get(sel_i) {
                                    let mut lines =
                                        self.format_incoming_summary(&self.coverage.hits);
                                    if let Some(report) = report_for_building(
                                        &self.catalog,
                                        &open.stamp.buildings,
                                        sel_i,
                                        &self.coverage,
                                    ) {
                                        if report.service_count() > 0 {
                                            lines.insert(
                                                0,
                                                self.tf(
                                                    "analyse.services_badge",
                                                    &[
                                                        ("met", &report.met_count().to_string()),
                                                        (
                                                            "total",
                                                            &report.service_count().to_string(),
                                                        ),
                                                    ],
                                                ),
                                            );
                                        }
                                    }
                                    if !lines.is_empty() {
                                        let anchor = self
                                            .world_to_screen(rect, sel_b.x, sel_b.y - 1.4);
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
                    }
                    CoverageMode::None => {}
                }

                // Hover tooltip: name + live buffs (compute coverage for hovered building).
                // Skip while placing/dragging — those paths already run their own coverage.
                if response.hovered()
                    && self.placing_guid.is_none()
                    && self.dragging_building.is_none()
                {
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
                                tip = Some((
                                    idx,
                                    self.catalog.label(b.guid, self.language),
                                ));
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
                            let tip_dim = Color32::from_rgb(210, 220, 235);
                            let mut extra: Vec<(String, Color32)> = Vec::new();
                            match cov.mode {
                                CoverageMode::Outgoing => {
                                    if let Some(hit) = cov.hits.first() {
                                        if idx == hit.source_index
                                            || cov.affected.contains(&idx)
                                        {
                                            for bns in hit.bonuses.iter().take(6) {
                                                extra.push((
                                                    self.format_bonus_chip(
                                                        &bns.attr, bns.value,
                                                    ),
                                                    tip_dim,
                                                ));
                                            }
                                            extra.push((
                                                self.tf(
                                                    "buff.affects",
                                                    &[(
                                                        "n",
                                                        &cov.affected.len().to_string(),
                                                    )],
                                                ),
                                                tip_dim,
                                            ));
                                        }
                                    }
                                }
                                CoverageMode::Incoming => {
                                    if let Some(report) = report_for_building(
                                        &self.catalog,
                                        &open.stamp.buildings,
                                        idx,
                                        cov,
                                    ) {
                                        if report.service_count() > 0 {
                                            extra.push((
                                                self.tf(
                                                    "analyse.services_badge",
                                                    &[
                                                        (
                                                            "met",
                                                            &report.met_count().to_string(),
                                                        ),
                                                        (
                                                            "total",
                                                            &report
                                                                .service_count()
                                                                .to_string(),
                                                        ),
                                                    ],
                                                ),
                                                tip_dim,
                                            ));
                                            for s in &report.services {
                                                let mark =
                                                    if s.met { "[x]" } else { "[ ]" };
                                                let color = if s.met {
                                                    Color32::from_rgb(120, 200, 120)
                                                } else {
                                                    Color32::from_rgb(220, 120, 110)
                                                };
                                                extra.push((
                                                    format!(
                                                        "{mark} {}",
                                                        s.need.label(self.language)
                                                    ),
                                                    color,
                                                ));
                                            }
                                        }
                                    }
                                    if cov.hits.is_empty() {
                                        extra.push((
                                            self.t("buff.no_sources").to_string(),
                                            tip_dim,
                                        ));
                                    } else {
                                        for line in
                                            self.format_incoming_summary(&cov.hits)
                                        {
                                            extra.push((line, tip_dim));
                                        }
                                    }
                                }
                                CoverageMode::None => {
                                    let (bw, bh) = self.building_size(
                                        open.stamp.buildings[idx].guid,
                                        open.stamp.buildings[idx].variation,
                                    );
                                    extra.push((format!("{bw:.0}×{bh:.0}"), tip_dim));
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
                                    ui.set_max_width(280.0);
                                    ui.label(
                                        RichText::new(&name)
                                            .size(13.0)
                                            .strong(),
                                    );
                                    for (line, color) in &extra {
                                        ui.label(
                                            RichText::new(line).size(12.0).color(*color),
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
                                    let (w0, h0) =
                                        self.building_size(guid, self.place_variation);
                                    oriented_size(w0, h0, self.place_direction)
                                })
                        } else {
                            let (w0, h0) = self.building_size(guid, self.place_variation);
                            oriented_size(w0, h0, self.place_direction)
                        };
                        let (sx, sy) = if from_clipboard {
                            if let Some(p) = self.clipboard.iter().find(|c| c.is_primary) {
                                self.snap_place_pos(
                                    wx, wy, p.guid, p.variation, p.direction, bw, bh,
                                )
                            } else {
                                self.snap_place_center(wx, wy, bw, bh)
                            }
                        } else {
                            self.snap_place_pos(
                                wx,
                                wy,
                                guid,
                                self.place_variation,
                                self.place_direction,
                                bw,
                                bh,
                            )
                        };
                        let mut blocked = false;
                        if let Some(pi) = self.module_parent_index {
                            if let Some(parent) = open.stamp.buildings.get(pi) {
                                let r = module_radius(self.catalog.building(parent.guid));
                                if r > 0.0 && dist_tiles(parent.x, parent.y, sx, sy) > r {
                                    blocked = true;
                                }
                            }
                        }
                        if !blocked {
                            let occupied = self.occupied_footprint_cells(&[]);
                            let ghost_check: Vec<(u32, f32, f32, i32, Option<u32>)> =
                                if from_clipboard {
                                    self.clipboard_building_centers(sx, sy)
                                        .into_iter()
                                        .map(|(c, cx, cy)| {
                                            (c.guid, cx, cy, c.direction, c.variation)
                                        })
                                        .collect()
                                } else {
                                    vec![(
                                        guid,
                                        sx,
                                        sy,
                                        self.place_direction,
                                        self.place_variation,
                                    )]
                                };
                            blocked = ghost_check.iter().any(|(g, gx, gy, gd, gv)| {
                                self.footprint_overlaps_occupied(
                                    *g, *gx, *gy, *gd, *gv, &occupied,
                                )
                            });
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
                                vec![(
                                    guid,
                                    sx,
                                    sy,
                                    self.place_direction,
                                    self.place_variation,
                                )]
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
                            let is_cistern = def.is_some_and(|d| d.is_aqueduct_cistern());
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
                                let outline =
                                    self.cells_screen_rect(rect, min_x, min_y, max_x, max_y);
                                painter.rect_stroke(
                                    outline,
                                    0.0,
                                    Stroke::new(2.0_f32, stroke_c),
                                    egui::StrokeKind::Outside,
                                );
                                self.paint_building_icon(
                                    ctx, &painter, outline, *g_guid, 0.85,
                                );
                                if is_cistern {
                                    self.paint_cistern_inlet_hint(
                                        &painter, rect, min_x, max_x, min_y, max_y, *g_dir,
                                    );
                                }
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

                        // Live Outgoing bonus/malus range while the building is in hand.
                        for (g_guid, gx, gy, g_dir, g_var) in &ghost_parts {
                            let source = Building {
                                guid: *g_guid,
                                x: *gx,
                                y: *gy,
                                direction: *g_dir,
                                complex_owner_id: None,
                                variation: *g_var,
                            };
                            if !self
                                .catalog
                                .building(*g_guid)
                                .is_some_and(building_has_area_effect)
                            {
                                continue;
                            }
                            let cov = self.place_ghost_coverage(&open, &source);
                            self.paint_outgoing_coverage_preview(&painter, rect, &open, &cov);
                        }
                    }
                    }
                }

                if let Some(pointer) = response.interact_pointer_pos() {
                    if over_build_bar {
                        // Clicks belong to the construction strip (mid/deep/bottom).
                    } else if self.radial.is_none()
                        && response.clicked_by(egui::PointerButton::Primary)
                    {
                        let field_hit = self.field_tile_at_pointer(rect, pointer);
                        let bld_hit =
                            self.building_at_pointer(rect, &open.stamp.buildings, pointer);
                        if self.suppress_stroke_click {
                            self.suppress_stroke_click = false;
                        } else if self.copy_pipette_mode {
                            if let Some(i) = bld_hit {
                                self.select_only(i);
                                self.refresh_coverage();
                                self.finish_copy_pipette();
                            } else if let Some(cell) = field_hit {
                                // Field-only pipette: select tile then copy if a farm is implied — skip.
                                let _ = cell;
                                self.status = self.t("status.copy_pipette_miss").into();
                            } else {
                                self.status = self.t("status.copy_pipette_miss").into();
                            }
                        } else if self.placing_guid.is_some()
                            || self.road_tool_active()
                            || self.field_tool_active()
                        {
                            // Place/paint even if the click lands on an existing building.
                            if self.drag_stroke.is_none() {
                                let (wx, wy) = self.screen_to_world(rect, pointer);
                                let (tx, ty) = self.stroke_anchor_tile(wx, wy);
                                self.begin_drag_stroke(tx, ty);
                                self.commit_drag_stroke();
                            }
                        } else if let Some(cell) = field_hit {
                            // Prefer individual field tiles over the farm building.
                            let ctrl =
                                ui.input(|inp| inp.modifiers.ctrl || inp.modifiers.command);
                            if ctrl {
                                if self.selected_fields.contains(&cell) {
                                    self.selected_fields.remove(&cell);
                                } else {
                                    self.selected_fields.insert(cell);
                                }
                            } else {
                                self.select_fields_only([cell]);
                            }
                            self.drag_stroke = None;
                            self.status = self.tf(
                                "status.sel_fields",
                                &[("n", &self.selected_fields.len().to_string())],
                            );
                            self.refresh_coverage();
                        } else if bld_hit.is_none() {
                            self.clear_canvas_selection();
                            self.refresh_coverage();
                        } else if self.road_erase_mode {
                            if let Some(i) = bld_hit {
                                self.select_only(i);
                            }
                            self.drag_stroke = None;
                            self.delete_selection(false);
                        } else if let Some(i) = bld_hit {
                            let ctrl = ui.input(|inp| inp.modifiers.ctrl || inp.modifiers.command);
                            if ctrl {
                                self.toggle_canvas_selection(i);
                            } else {
                                self.select_only(i);
                            }
                            self.drag_stroke = None;
                            if let Some(b) = open.stamp.buildings.get(i) {
                                let name = self.catalog.label(b.guid, self.language);
                                self.status =
                                    self.tf("status.selected", &[("name", &name)]);
                            }
                            self.refresh_coverage();
                        }
                    }

                    if !over_build_bar
                        && response.clicked_by(egui::PointerButton::Secondary)
                    {
                        // Wheel already open → RMB anywhere = cancel, never reopen here.
                        if self.radial.is_some() {
                            self.dismiss_radial();
                        } else if self.hand_busy() {
                            // Tool in hand → RMB drops it (Abriss/Wege/Gebäude/Feld-Pinsel).
                            self.clear_hand();
                        } else if self.selected_building.is_some()
                            || !self.selected_buildings.is_empty()
                            || !self.selected_roads.is_empty()
                            || !self.selected_fields.is_empty()
                        {
                            // Selection first: RMB clears it. Tool wheel only with empty hand.
                            self.clear_canvas_selection();
                            self.refresh_coverage();
                            self.status = "Auswahl aufgehoben".into();
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
                                // Keep footprint top-left stable when swapping to/from a
                                // decentered aqueduct cistern (otherwise Pos stays centered
                                // and the game shows the classic +1 tile gap).
                                let (old_guid, old_var, old_dir, ox, oy) = self
                                    .open
                                    .as_ref()
                                    .and_then(|o| o.stamp.buildings.get(idx))
                                    .map(|b| (b.guid, b.variation, b.direction, b.x, b.y))
                                    .unwrap_or((0, None, 0, 0.0, 0.0));
                                let (tx, ty) = {
                                    let (mx, my, _, _) =
                                        self.building_oriented_blocker(old_guid, old_var, old_dir);
                                    footprint_top_left_blocker(ox, oy, mx, my, self.stamp_drag())
                                };
                                let resnap = is_aqueduct_cistern(old_guid)
                                    || is_aqueduct_cistern(new_guid);
                                let (nx, ny) = if resnap {
                                    self.place_pos_for_tl(tx, ty, new_guid, old_var, old_dir)
                                } else {
                                    (ox, oy)
                                };
                                if let Some(open_mut) = self.open.as_mut() {
                                    if let Ok(()) = open_mut.stamp.set_building_guid(idx, new_guid)
                                    {
                                        if resnap {
                                            let _ = open_mut.stamp.set_building_pos(idx, nx, ny);
                                        }
                                        open_mut.dirty = true;
                                        self.schedule_autosave();
                                        self.status = format!(
                                            "GUID getauscht → {} ({})",
                                            self.catalog.label(new_guid, self.language),
                                            new_guid
                                        );
                                        self.selected_building = Some(idx);
                                        self.refresh_coverage();
                                    }
                                }
                            } else {
                                self.palette_selected_guid = None;
                                self.status = "Palette abgewählt".into();
                            }
                        } else {
                            // Truly empty hand → Anno-style quick-select wheel.
                            self.open_radial(pointer);
                        }
                    }
                }

                if self.radial.is_none()
                    && !over_build_bar
                    && response.drag_started_by(egui::PointerButton::Primary)
                {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let mut on_target = false;
                        let tool_drag = self.placing_guid.is_some()
                            || self.road_tool_active()
                            || self.field_tool_active();
                        if !tool_drag {
                            if let Some(cell) = self.field_tile_at_pointer(rect, pointer) {
                                // Already selected → move; otherwise fall through to marquee select.
                                if self.selected_fields.contains(&cell) {
                                    self.begin_field_drag(cell);
                                    on_target = true;
                                }
                            } else if let Some(idx) =
                                self.building_at_pointer(rect, &open.stamp.buildings, pointer)
                            {
                                if let Some(b) = open.stamp.buildings.get(idx) {
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
                                    if self.selected_buildings.len() > 1
                                        || !self.selected_roads.is_empty()
                                    {
                                        self.begin_selection_drag(idx);
                                    } else {
                                        self.clear_selection_drag();
                                    }
                                    on_target = true;
                                }
                            }
                        }
                        // Tool strokes OR empty-hand marquee selection.
                        if !on_target && (tool_drag || !self.hand_busy()) {
                            let (wx, wy) = self.screen_to_world(rect, pointer);
                            let (tx, ty) = self.stroke_anchor_tile(wx, wy);
                            self.begin_drag_stroke(tx, ty);
                            self.suppress_stroke_click = true;
                        }
                    }
                }
                if response.dragged_by(egui::PointerButton::Primary) {
                    if self.dragging_fields {
                        if let Some(pointer) = response.interact_pointer_pos() {
                            if !self.history_drag_pushed {
                                self.push_history();
                                self.history_drag_pushed = true;
                            }
                            let (wx, wy) = self.screen_to_world(rect, pointer);
                            let (tx, ty) = (wx.floor() as i32, wy.floor() as i32);
                            let (ax, ay) = self.drag_field_anchor;
                            self.drag_field_delta = (tx - ax, ty - ay);
                        }
                    } else if let (Some(idx), Some(pointer)) =
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
                        let (guid, variation, direction, bw, bh) = self
                            .open
                            .as_ref()
                            .and_then(|o| o.stamp.buildings.get(idx))
                            .map(|b| {
                                let (w0, h0) = self.building_size(b.guid, b.variation);
                                let (ow, oh) = oriented_size(w0, h0, b.direction);
                                (b.guid, b.variation, b.direction, ow, oh)
                            })
                            .unwrap_or((0, None, 0, 1.0, 1.0));
                        let (mut sx, mut sy) = if guid != 0 {
                            self.snap_existing_pos(
                                world.0, world.1, guid, variation, direction,
                            )
                        } else {
                            self.snap_place_center(world.0, world.1, bw, bh)
                        };

                        if self.selection_drag_active() {
                            let exclude: Vec<usize> =
                                self.selected_buildings.iter().copied().collect();
                            let occupied = self.occupied_footprint_cells(&exclude);
                            let collides = self.open.as_ref().is_some_and(|o| {
                                let (ox, oy) = self.drag_anchor_start;
                                let dx = sx - ox;
                                let dy = sy - oy;
                                self.drag_group_origins.iter().any(|(&i, &(bx, by))| {
                                    let Some(b) = o.stamp.buildings.get(i) else {
                                        return false;
                                    };
                                    self.footprint_overlaps_occupied(
                                        b.guid,
                                        bx + dx,
                                        by + dy,
                                        b.direction,
                                        b.variation,
                                        &occupied,
                                    )
                                })
                            });
                            if collides {
                                self.status = self.t("status.place_blocked").into();
                            } else {
                                self.apply_selection_drag(idx, sx, sy);
                            }
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
                                                let snapped = if guid != 0 {
                                                    self.snap_existing_pos(
                                                        cx, cy, guid, variation, direction,
                                                    )
                                                } else {
                                                    self.snap_place_center(cx, cy, bw, bh)
                                                };
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
                        // Reject moves that land on another building's footprint.
                        let mut move_exclude: Vec<usize> = if move_linked {
                            self.open
                                .as_ref()
                                .map(|o| {
                                    complex_group_indices(&o.stamp.buildings, idx)
                                })
                                .unwrap_or_else(|| vec![idx])
                        } else {
                            vec![idx]
                        };
                        if self.selection_drag_active() {
                            move_exclude.extend(self.selected_buildings.iter().copied());
                        }
                        move_exclude.sort_unstable();
                        move_exclude.dedup();
                        let occupied = self.occupied_footprint_cells(&move_exclude);
                        let (g_guid, g_dir, g_var) = self
                            .open
                            .as_ref()
                            .and_then(|o| o.stamp.buildings.get(idx))
                            .map(|b| (b.guid, b.direction, b.variation))
                            .unwrap_or((0, 0, None));
                        let collides = self.footprint_overlaps_occupied(
                            g_guid, sx, sy, g_dir, g_var, &occupied,
                        );
                        if collides {
                            self.status = self.t("status.place_blocked").into();
                        } else if let Some(open_mut) = self.open.as_mut() {
                            let prev = open_mut
                                .stamp
                                .buildings
                                .get(idx)
                                .map(|b| (b.x.to_bits(), b.y.to_bits()));
                            if open_mut
                                .stamp
                                .set_building_pos_ex(idx, sx, sy, move_linked)
                                .is_ok()
                            {
                                let now = open_mut
                                    .stamp
                                    .buildings
                                    .get(idx)
                                    .map(|b| (b.x.to_bits(), b.y.to_bits()));
                                // Only rewrite coverage when the snap tile actually changed —
                                // drag events fire every frame and were hitching large stamps.
                                if prev != now {
                                    open_mut.dirty = true;
                                    self.schedule_autosave();
                                    self.analysis_gen = self.analysis_gen.wrapping_add(1);
                                    *self.place_cov_roads.borrow_mut() = None;
                                    *self.place_ghost_cov.borrow_mut() = None;
                                    self.city_analysis = None;
                                    self.area_analysis = None;
                                    self.needs_cov_cache = None;
                                    self.drag_cov_key = Some((idx, sx.to_bits(), sy.to_bits()));
                                    self.refresh_coverage();
                                }
                            }
                        }
                        if !collides {
                            if let Some(msg) = radius_clamp_msg {
                                self.status = msg;
                            }
                        }
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
                    if self.dragging_fields {
                        self.commit_field_drag();
                    } else if self.drag_stroke.is_some() && self.dragging_building.is_none() {
                        self.commit_drag_stroke();
                    }
                    // Always clear — otherwise a stroke without a following `clicked()`
                    // leaves suppress set and the next place/road click does nothing.
                    self.suppress_stroke_click = false;
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
                    self.t("empty.new_hint"),
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
            // Full click-sense: canvas allocate_painter is under us and would otherwise
            // place buildings through empty gaps in the mid/deep rows.
            // No on_hover_text here — that tooltip covered the whole 2nd row.
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(band), |ui| {
                let panel_rect = ui.max_rect();
                ui.painter()
                    .rect_filled(panel_rect, 0.0, Color32::from_rgb(24, 28, 36));
                ui.interact(
                    panel_rect,
                    ui.id().with("build_bar_block"),
                    Sense::click_and_drag(),
                );
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
                let residence_tiers = self.residence_tier_guids.clone();
                let show_mid = !content_entries.is_empty()
                    || residence_tiers.as_ref().is_some_and(|c| c.len() > 1);
                let deep_entry = self
                    .buildmenu_entry
                    .and_then(|ei| content_entries.get(ei));
                let show_deep = residence_tiers.is_none()
                    && (matches!(
                        deep_entry,
                        Some(MenuEntry::Chain { buildings, .. }) if !buildings.is_empty()
                    ) || matches!(
                        deep_entry,
                        Some(MenuEntry::Category { entries, .. }) if !entries.is_empty()
                    ));
                let vpad_strip = ((strip_h - tile_row_h) * 0.5).max(0.0);
                let row_h = BUILD_ROW_H;
                let row_vpad = ((row_h - tile_row_h) * 0.5).max(0.0);
                // Fixed label column so mid/deep icon columns share the same X
                // (and frames stay full-width — no stair-step when deep opens).
                const LABEL_COL_W: f32 = 122.0;

                ui.spacing_mut().item_spacing.y = BUILD_ROW_GAP;
                ui.vertical(|ui| {
                    let full_w = ui.available_width();

                    // Row 3 (top): leaf items of selected chain/group.
                    if show_deep {
                        if let Some(entry) = deep_entry {
                            egui::Frame::new()
                                .fill(Color32::from_rgb(36, 40, 52))
                                .corner_radius(4.0)
                                .inner_margin(egui::Margin {
                                    left: 4,
                                    right: 4,
                                    top: row_vpad as i8,
                                    bottom: row_vpad as i8,
                                })
                                .show(ui, |ui| {
                                    ui.set_min_width(full_w);
                                    ui.set_width(full_w);
                                    ui.set_min_height(tile_row_h);
                                    ui.set_max_height(tile_row_h);
                                    ui.with_layout(
                                        egui::Layout::left_to_right(egui::Align::Center)
                                            .with_main_wrap(false),
                                        |ui| {
                                            ui.style_mut().wrap_mode =
                                                Some(egui::TextWrapMode::Extend);
                                            let (lbl_text, color) = match entry {
                                                MenuEntry::Chain {
                                                    short_name, name, ..
                                                } => {
                                                    let chain_lbl = short_name
                                                        .clone()
                                                        .unwrap_or_else(|| {
                                                            name.replace("&amp;", "&")
                                                        });
                                                    (
                                                        self.tf(
                                                            "buildmenu.chain",
                                                            &[("name", &chain_lbl)],
                                                        ),
                                                        Color32::from_rgb(230, 210, 160),
                                                    )
                                                }
                                                MenuEntry::Category {
                                                    short_name, name, ..
                                                } => {
                                                    let cat_lbl = short_name
                                                        .clone()
                                                        .unwrap_or_else(|| {
                                                            name.replace("&amp;", "&")
                                                        });
                                                    (
                                                        self.tf(
                                                            "buildmenu.group",
                                                            &[("name", &cat_lbl)],
                                                        ),
                                                        Color32::from_rgb(230, 210, 160),
                                                    )
                                                }
                                                MenuEntry::Building { .. } => {
                                                    (String::new(), Color32::GRAY)
                                                }
                                            };
                                            ui.allocate_ui_with_layout(
                                                Vec2::new(LABEL_COL_W, tile_row_h),
                                                egui::Layout::left_to_right(
                                                    egui::Align::Center,
                                                ),
                                                |ui| {
                                                    ui.set_min_width(LABEL_COL_W);
                                                    ui.label(
                                                        RichText::new(lbl_text)
                                                            .size(13.0)
                                                            .color(color),
                                                    );
                                                },
                                            );
                                            match entry {
                                                MenuEntry::Chain { buildings, .. } => {
                                                    for &bg in buildings {
                                                        let lbl = self
                                                            .catalog
                                                            .label(bg, self.language);
                                                        let icon = self
                                                            .buildmenu
                                                            .building_icon(bg)
                                                            .map(|s| s.to_string());
                                                        let sel = self.placing_guid
                                                            == Some(bg)
                                                            || self.road_brush_guid
                                                                == Some(bg as i32);
                                                        if self
                                                            .goods_tile(
                                                                ui,
                                                                ctx,
                                                                icon.as_deref(),
                                                                sel,
                                                                &lbl,
                                                                None,
                                                            )
                                                            .clicked()
                                                        {
                                                            self.activate_menu_building(
                                                                bg, &lbl,
                                                            );
                                                        }
                                                    }
                                                }
                                                MenuEntry::Category {
                                                    entries: nested, ..
                                                } => {
                                                    for nest in nested {
                                                        let tip = nest.label();
                                                        let icon =
                                                            self.resolve_menu_icon(nest);
                                                        let sel = matches!(
                                                            nest,
                                                            MenuEntry::Building {
                                                                guid,
                                                                ..
                                                            } if self.placing_guid
                                                                == Some(*guid)
                                                                || self.road_brush_guid
                                                                    == Some(*guid as i32)
                                                        );
                                                        if self
                                                            .goods_tile(
                                                                ui,
                                                                ctx,
                                                                icon.as_deref(),
                                                                sel,
                                                                &tip,
                                                                None,
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
                                                                        .label(
                                                                            *guid,
                                                                            self.language,
                                                                        );
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
                                                                            .label(
                                                                                bg,
                                                                                self.language,
                                                                            );
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
                                                }
                                                MenuEntry::Building { .. } => {}
                                            }
                                        },
                                    );
                                });
                        }
                    }

                    // Row 2 (mid): categories / chains / needs / residence tiers.
                    if show_mid {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(32, 36, 46))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin {
                                left: 4,
                                right: 4,
                                top: row_vpad as i8,
                                bottom: row_vpad as i8,
                            })
                            .show(ui, |ui| {
                                ui.set_min_width(full_w);
                                ui.set_width(full_w);
                                ui.set_min_height(tile_row_h);
                                ui.set_max_height(tile_row_h);
                                ui.with_layout(
                                    egui::Layout::left_to_right(egui::Align::Center)
                                        .with_main_wrap(false),
                                    |ui| {
                                        ui.style_mut().wrap_mode =
                                            Some(egui::TextWrapMode::Extend);
                                        let mid_lbl = if residence_tiers.is_some() {
                                            self.t("buildmenu.residence_tiers")
                                        } else if need_mode {
                                            self.t("buildmenu.needs")
                                        } else {
                                            self.t("buildmenu.buildings")
                                        };
                                        ui.allocate_ui_with_layout(
                                            Vec2::new(LABEL_COL_W, tile_row_h),
                                            egui::Layout::left_to_right(
                                                egui::Align::Center,
                                            ),
                                            |ui| {
                                                ui.set_min_width(LABEL_COL_W);
                                                ui.label(
                                                    RichText::new(mid_lbl)
                                                        .size(12.0)
                                                        .weak(),
                                                );
                                            },
                                        );
                                        if let Some(tiers) = &residence_tiers {
                                            for (ti, &tg) in tiers.iter().enumerate() {
                                                let def = self.catalog.building(tg);
                                                let lbl =
                                                    self.catalog.label(tg, self.language);
                                                let icon = def
                                                    .and_then(|d| {
                                                        d.residence_display_icon()
                                                            .map(|s| s.to_string())
                                                    })
                                                    .or_else(|| {
                                                        self.buildmenu
                                                            .building_icon(tg)
                                                            .map(|s| s.to_string())
                                                    });
                                                let sel = self.placing_guid == Some(tg);
                                                let tier_n = self
                                                    .catalog
                                                    .residence_tier_tag(tg)
                                                    .unwrap_or_else(|| format!("T{}", ti + 1));
                                                if self
                                                    .goods_tile(
                                                        ui,
                                                        ctx,
                                                        icon.as_deref(),
                                                        sel,
                                                        &lbl,
                                                        Some(&tier_n),
                                                    )
                                                    .clicked()
                                                {
                                                    self.activate_menu_building(tg, &lbl);
                                                }
                                            }
                                        } else {
                                            for (ei, entry) in
                                                content_entries.iter().enumerate()
                                            {
                                                let tip = entry.label();
                                                let icon = self.resolve_menu_icon(entry);
                                                let sel = match entry {
                                                    MenuEntry::Building { guid, .. } => {
                                                        self.road_brush_guid
                                                            == Some(*guid as i32)
                                                            || self.placing_guid
                                                                == Some(*guid)
                                                            || self.buildmenu_entry
                                                                == Some(ei)
                                                    }
                                                    _ => {
                                                        self.buildmenu_entry == Some(ei)
                                                    }
                                                };
                                                if self
                                                    .goods_tile(
                                                        ui,
                                                        ctx,
                                                        icon.as_deref(),
                                                        sel,
                                                        &tip,
                                                        None,
                                                    )
                                                    .clicked()
                                                {
                                                    self.buildmenu_entry = Some(ei);
                                                    match entry {
                                                        MenuEntry::Building {
                                                            guid, ..
                                                        } => {
                                                            let lbl = self
                                                                .catalog
                                                                .label(
                                                                    *guid,
                                                                    self.language,
                                                                );
                                                            self.activate_menu_building(
                                                                *guid, &lbl,
                                                            );
                                                        }
                                                        MenuEntry::Chain { .. } => {
                                                            self.placing_guid = None;
                                                            self.road_brush_guid = None;
                                                            self.road_erase_mode = false;
                                                            self.residence_tier_guids =
                                                                None;
                                                            self.status = self.tf(
                                                                "buildmenu.pick_chain",
                                                                &[("name", &tip)],
                                                            );
                                                        }
                                                        MenuEntry::Category { .. } => {
                                                            self.placing_guid = None;
                                                            self.road_brush_guid = None;
                                                            self.road_erase_mode = false;
                                                            self.residence_tier_guids =
                                                                None;
                                                            self.status = self.tf(
                                                                "buildmenu.pick_group",
                                                                &[("name", &tip)],
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    },
                                );
                            });
                    }

                    // Row 1 (bottom): infrastructure + residents — single line only.
                    // Nested right_to_left + Hand label made egui wrap Residents onto
                    // a second strip row when a building was in hand.
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
                            ui.set_min_width(full_w);
                            ui.set_width(full_w);
                            ui.set_min_height(tile_row_h);
                            ui.set_max_height(tile_row_h);
                            ui.allocate_ui_with_layout(
                                Vec2::new(full_w - 8.0, tile_row_h),
                                egui::Layout::left_to_right(egui::Align::Center)
                                    .with_main_wrap(false),
                                |ui| {
                                    ui.style_mut().wrap_mode =
                                        Some(egui::TextWrapMode::Extend);
                                    if let Some(infra) = infra_tab.as_ref() {
                                        for (ei, entry) in infra.entries.iter().enumerate()
                                        {
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
                                            let icon = self.resolve_menu_icon(entry);
                                            if self
                                                .goods_tile(
                                                    ui,
                                                    ctx,
                                                    icon.as_deref(),
                                                    sel,
                                                    &tip,
                                                    None,
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
                                                    // Leaf tool (road etc.): no mid row —
                                                    // clear infra_entry so we don't reserve
                                                    // empty mid height / dual-highlight.
                                                    MenuEntry::Building { guid, .. } => {
                                                        self.buildmenu_infra_entry = None;
                                                        self.buildmenu_entry = None;
                                                        let lbl = self
                                                            .catalog
                                                            .label(*guid, self.language);
                                                        self.activate_menu_building(
                                                            *guid, &lbl,
                                                        );
                                                    }
                                                    MenuEntry::Category { .. } => {
                                                        self.buildmenu_infra_entry =
                                                            Some(ei);
                                                        self.buildmenu_entry = None;
                                                        self.placing_guid = None;
                                                        self.road_brush_guid = None;
                                                        self.road_erase_mode = false;
                                                        self.residence_tier_guids = None;
                                                        self.status =
                                                            format!("Kategorie: {tip}");
                                                    }
                                                    MenuEntry::Chain {
                                                        buildings, ..
                                                    } => {
                                                        self.buildmenu_infra_entry =
                                                            Some(ei);
                                                        self.buildmenu_entry = Some(0);
                                                        self.road_brush_guid = None;
                                                        self.road_erase_mode = false;
                                                        if let Some(&bg) =
                                                            buildings.first()
                                                        {
                                                            let lbl = self
                                                                .catalog
                                                                .label(bg, self.language);
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
                                    ui.label(
                                        RichText::new(self.t("buildmenu.residents"))
                                            .size(12.0)
                                            .weak(),
                                    );
                                    ui.add_space(4.0);

                                    for &i in &need_indices {
                                        let t = &tabs[i];
                                        let selected = need_mode
                                            && self.buildmenu_tab == Some(i);
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
                                            self.residence_tier_guids = None;
                                            self.status = format!(
                                                "Tier: {}",
                                                t.short_tier_label()
                                            );
                                        }
                                    }

                                    // Region only (Hand already in top status — kept
                                    // out of this strip so it cannot force a wrap).
                                    ui.with_layout(
                                        egui::Layout::right_to_left(
                                            egui::Align::Center,
                                        )
                                        .with_main_wrap(false),
                                        |ui| {
                                            ui.add_space(8.0);
                                            ui.label(
                                                RichText::new(
                                                    self.region_filter
                                                        .label_lang(self.language),
                                                )
                                                .size(12.0)
                                                .weak(),
                                            );
                                        },
                                    );
                                },
                            );
                        });
                });
            });

            self.paint_radial_menu(ui, ctx, &painter, rect);

        });

        self.show_complex_module_ui(ctx);
        self.show_residence_needs_ui(ctx);
    }
}

