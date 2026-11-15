//! Road-aware service / adjacency coverage (Anno 117 EffectSource).

use crate::catalog::{BuildingDef, Catalog, EffectBonus};
use crate::filedb::DocumentOrdered;
use crate::meshgraph::{
    is_aqueduct_brush_guid, mesh_road_cells, read_mesh_info, relative_to_tile,
};
use crate::stamp::Building;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoadKind {
    Dirt,
    Stone,
}

/// Dirt costs more street-distance than stone (CityInstitutions modifiers ≈ 1.5).
const DIRT_COST: f32 = 1.5;
const STONE_COST: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageMode {
    None,
    /// Selected building is a service/production source → highlight targets.
    Outgoing,
    /// Selected building is a residence/target → list sources that reach it.
    Incoming,
}

#[derive(Debug, Clone)]
pub struct EffectHit {
    pub source_index: usize,
    pub source_guid: u32,
    pub source_name: String,
    pub service_kind: Option<String>,
    pub via_street: bool,
    pub via_radius: bool,
    pub street_dist: Option<f32>,
    pub radius_dist: Option<f32>,
    pub bonuses: Vec<EffectBonus>,
}

#[derive(Debug, Clone, Default)]
pub struct CoverageMap {
    /// Road cells colored by kind (always filled when stamp open).
    pub roads: HashMap<Cell, RoadKind>,
    /// Road cells reachable via street-distance from selection (subtle highlight).
    pub reachable_roads: HashSet<Cell>,
    /// Building indices affected by outgoing coverage / sources for incoming.
    pub affected: Vec<usize>,
    pub hits: Vec<EffectHit>,
    pub mode: CoverageMode,
    pub radius: f32,
    pub street_budget: f32,
    pub selected: Option<usize>,
}

impl Default for CoverageMode {
    fn default() -> Self {
        CoverageMode::None
    }
}

fn drag_of(document: Option<&DocumentOrdered>) -> (f32, f32) {
    document
        .and_then(read_mesh_info)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0))
}

/// Footprint with drag `(0,0)` — only correct when DragRectCenter is integer.
pub fn footprint_cells(b: &Building, def: Option<&BuildingDef>) -> Vec<Cell> {
    footprint_cells_for_region(b, def, 0, (0.0, 0.0))
}

pub fn footprint_cells_for_region(
    b: &Building,
    def: Option<&BuildingDef>,
    _region_guid: u32,
    drag: (f32, f32),
) -> Vec<Cell> {
    let (w0, h0) = crate::catalog::size_for_guid(b.guid, 1.0, 1.0, def, b.variation);
    let (mx0, my0) = crate::catalog::blocker_min_for(b.guid, w0, h0, def, b.variation);
    let (mx, my, w, h) = crate::catalog::oriented_blocker(mx0, my0, w0, h0, b.direction);
    let ww = w.round().max(1.0) as i32;
    let hh = h.round().max(1.0) as i32;
    // Snap BuildBlocker top-left through absolute tile space (see relative_to_tile).
    // Must use blocker_min — not center−size/2 — for decentered pivots (cistern).
    let x0 = relative_to_tile(b.x + mx, drag.0);
    let y0 = relative_to_tile(b.y + my, drag.1);
    let mut cells = Vec::with_capacity((ww * hh) as usize);
    for dy in 0..hh {
        for dx in 0..ww {
            cells.push(Cell {
                x: x0 + dx,
                y: y0 + dy,
            });
        }
    }
    cells
}

fn is_road(def: Option<&BuildingDef>) -> Option<RoadKind> {
    let def = def?;
    if let Some(kind) = def.road_kind.as_deref() {
        if kind == "aqueduct" {
            return None;
        }
        return Some(match kind {
            "stone" => RoadKind::Stone,
            _ => RoadKind::Dirt,
        });
    }
    if def.category == "street" {
        if def.logistic_speed_factor.unwrap_or(0.0) >= 1.4 {
            return Some(RoadKind::Stone);
        }
        if let Some(folder) = def.tileset_folder.as_deref() {
            let f = folder.to_lowercase();
            if f.contains("street_02")
                || f.contains("street_03")
                || f.contains("quay_street_02")
                || f.contains("quay_street_03")
            {
                return Some(RoadKind::Stone);
            }
        }
        return Some(RoadKind::Dirt);
    }
    None
}

pub fn build_road_map(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
) -> HashMap<Cell, RoadKind> {
    build_road_map_region(buildings, catalog, document, 0)
}

pub fn build_road_map_region(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
) -> HashMap<Cell, RoadKind> {
    let drag = drag_of(document);
    let mut roads = HashMap::new();
    for b in buildings {
        if let Some(kind) = is_road(catalog.building(b.guid)) {
            for cell in footprint_cells_for_region(b, catalog.building(b.guid), region_guid, drag) {
                roads.insert(cell, kind);
            }
        }
    }
    if let Some(doc) = document {
        for (x, y, guid) in mesh_road_cells(doc) {
            // Aqueducts share MeshGraph with roads but are not street network.
            if is_aqueduct_brush_guid(guid) {
                continue;
            }
            let kind = catalog
                .building(guid as u32)
                .and_then(|d| is_road(Some(d)))
                .unwrap_or(RoadKind::Dirt);
            roads.insert(Cell { x, y }, kind);
        }
    }
    roads
}

fn road_cost(kind: RoadKind) -> f32 {
    match kind {
        RoadKind::Dirt => DIRT_COST,
        RoadKind::Stone => STONE_COST,
    }
}

fn has_area_effect(def: &BuildingDef) -> bool {
    def.street_distance.is_some()
        || def.radius_distance.is_some()
        || def.effect_scope.is_some()
        || !def.effect_bonuses.is_empty()
        || matches!(
            def.category.as_str(),
            "institution" | "public" | "production"
        ) && (def.street_distance.is_some() || def.radius_distance.is_some())
}

fn display_name(def: Option<&BuildingDef>, guid: u32) -> String {
    def.and_then(|d| d.short_name.clone().filter(|s| !s.is_empty()))
        .or_else(|| def.map(|d| d.name.clone()))
        .unwrap_or_else(|| format!("GUID {guid}"))
}

/// Seeds: road cells adjacent to (or under) the building footprint.
fn road_seeds(own: &[Cell], roads: &HashMap<Cell, RoadKind>) -> Vec<Cell> {
    let mut seeds = Vec::new();
    let mut seen = HashSet::new();
    for c in own {
        if roads.contains_key(c) && seen.insert(*c) {
            seeds.push(*c);
        }
        for n in [
            Cell { x: c.x + 1, y: c.y },
            Cell { x: c.x - 1, y: c.y },
            Cell { x: c.x, y: c.y + 1 },
            Cell { x: c.x, y: c.y - 1 },
        ] {
            if roads.contains_key(&n) && seen.insert(n) {
                seeds.push(n);
            }
        }
    }
    seeds
}

/// Dijkstra along roads with dirt/stone costs. Returns cost to each reached road cell.
fn street_reach(
    seeds: &[Cell],
    roads: &HashMap<Cell, RoadKind>,
    budget: f32,
) -> HashMap<Cell, f32> {
    let mut best: HashMap<Cell, f32> = HashMap::new();
    let mut q = VecDeque::new();
    for s in seeds {
        best.insert(*s, 0.0);
        q.push_back(*s);
    }
    while let Some(c) = q.pop_front() {
        let d = best[&c];
        if d > budget {
            continue;
        }
        for n in [
            Cell { x: c.x + 1, y: c.y },
            Cell { x: c.x - 1, y: c.y },
            Cell { x: c.x, y: c.y + 1 },
            Cell { x: c.x, y: c.y - 1 },
        ] {
            let Some(kind) = roads.get(&n).copied() else {
                continue;
            };
            let nd = d + road_cost(kind);
            if nd > budget {
                continue;
            }
            let replace = match best.get(&n) {
                Some(&old) => nd + 1e-4 < old,
                None => true,
            };
            if replace {
                best.insert(n, nd);
                q.push_back(n);
            }
        }
    }
    best
}

fn building_center(b: &Building) -> (f32, f32) {
    (b.x, b.y)
}

fn within_radius(a: &Building, b: &Building, radius: f32) -> bool {
    let (ax, ay) = building_center(a);
    let (bx, by) = building_center(b);
    let dx = ax - bx;
    let dy = ay - by;
    dx * dx + dy * dy <= radius * radius
}

fn touches_reachable_road(own: &[Cell], reach: &HashMap<Cell, f32>) -> bool {
    for c in own {
        if reach.contains_key(c) {
            return true;
        }
        for n in [
            Cell { x: c.x + 1, y: c.y },
            Cell { x: c.x - 1, y: c.y },
            Cell { x: c.x, y: c.y + 1 },
            Cell { x: c.x, y: c.y - 1 },
        ] {
            if reach.contains_key(&n) {
                return true;
            }
        }
    }
    false
}

fn source_ranges(def: &BuildingDef) -> (Option<f32>, Option<f32>, bool, bool) {
    let scope = def.effect_scope.as_deref().unwrap_or("");
    let mut use_street = scope == "StreetDistance" || def.street_distance.is_some();
    let mut use_radius = scope == "Radius" || (def.radius_distance.is_some() && scope != "StreetDistance");
    // Institutions / public: prefer street when both present.
    if matches!(def.category.as_str(), "institution" | "public") {
        if def.street_distance.is_some() {
            use_street = true;
        }
        // RadiusDistance on services is often the circular attribute aura;
        // show street coverage as primary for institutions.
        if scope == "StreetDistance" {
            use_radius = false;
        }
    }
    // Production adjacency with Radius scope: radius only.
    if scope == "Radius" {
        use_street = false;
        use_radius = true;
    }
    if scope == "StreetDistance" {
        use_street = true;
        use_radius = false;
    }
    (def.street_distance, def.radius_distance, use_street, use_radius)
}

/// Anno Effect.Targets for adjacency Money/etc. is "Villas and Guesthouses" —
/// production/public attribute auras do not hit warehouses or pastures.
fn effect_applies_to(source: &BuildingDef, target: Option<&BuildingDef>) -> bool {
    let Some(target) = target else {
        return false;
    };
    if target.category == "street" || target.road_kind.is_some() {
        return false;
    }
    let adjacency_attr = source.category == "production"
        || source.service_kind.as_deref() == Some("production")
        || source.category == "public"
        || source.service_kind.as_deref() == Some("public");
    if adjacency_attr {
        return target.category == "residence";
    }
    // Institutions (fire / riot / illness): keep broader footprint coverage.
    true
}

fn covers_target(
    source: &Building,
    target: &Building,
    target_own: &[Cell],
    reach: &HashMap<Cell, f32>,
    use_street: bool,
    use_radius: bool,
    radius: Option<f32>,
) -> (bool, bool) {
    let via_street = use_street && !reach.is_empty() && touches_reachable_road(target_own, reach);
    let via_radius = if use_radius {
        radius
            .map(|r| within_radius(source, target, r))
            .unwrap_or(false)
    } else {
        false
    };
    (via_street, via_radius)
}

pub fn compute_roads_only(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
) -> CoverageMap {
    compute_roads_only_region(buildings, catalog, document, 0)
}

pub fn compute_roads_only_region(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
) -> CoverageMap {
    CoverageMap {
        roads: build_road_map_region(buildings, catalog, document, region_guid),
        mode: CoverageMode::None,
        ..CoverageMap::default()
    }
}

pub fn compute_coverage(
    buildings: &[Building],
    catalog: &Catalog,
    selected: usize,
    document: Option<&DocumentOrdered>,
) -> CoverageMap {
    compute_coverage_region(buildings, catalog, selected, document, 0)
}

pub fn compute_coverage_region(
    buildings: &[Building],
    catalog: &Catalog,
    selected: usize,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
) -> CoverageMap {
    let drag = drag_of(document);
    let roads = build_road_map_region(buildings, catalog, document, region_guid);
    let mut map = CoverageMap {
        roads: roads.clone(),
        selected: Some(selected),
        mode: CoverageMode::None,
        ..CoverageMap::default()
    };
    let Some(sel) = buildings.get(selected) else {
        return map;
    };
    let sel_def = catalog.building(sel.guid);
    let sel_own = footprint_cells_for_region(sel, sel_def, region_guid, drag);

    // Outgoing: selected building has area effects.
    if let Some(def) = sel_def {
        if has_area_effect(def)
            && (def.street_distance.is_some()
                || def.radius_distance.is_some()
                || !def.effect_bonuses.is_empty()
                || def.effect_scope.is_some())
        {
            let (street_budget, radius, use_street, use_radius) = source_ranges(def);
            map.mode = CoverageMode::Outgoing;
            map.street_budget = street_budget.unwrap_or(0.0);
            map.radius = radius.unwrap_or(0.0);

            let seeds = road_seeds(&sel_own, &roads);
            let reach = if use_street {
                if let Some(budget) = street_budget {
                    street_reach(&seeds, &roads, budget)
                } else {
                    HashMap::new()
                }
            } else {
                HashMap::new()
            };
            map.reachable_roads = reach.keys().copied().collect();

            let mut affected = Vec::new();
            for (i, other) in buildings.iter().enumerate() {
                if i == selected {
                    continue;
                }
                let other_def = catalog.building(other.guid);
                if is_road(other_def).is_some() {
                    continue;
                }
                if !effect_applies_to(def, other_def) {
                    continue;
                }
                let other_own = footprint_cells_for_region(other, other_def, region_guid, drag);
                let (via_s, via_r) = covers_target(
                    sel,
                    other,
                    &other_own,
                    &reach,
                    use_street,
                    use_radius,
                    radius,
                );
                if via_s || via_r {
                    affected.push(i);
                }
            }
            map.affected = affected;
            map.hits.push(EffectHit {
                source_index: selected,
                source_guid: sel.guid,
                source_name: display_name(sel_def, sel.guid),
                service_kind: def.service_kind.clone(),
                via_street: use_street,
                via_radius: use_radius,
                street_dist: street_budget,
                radius_dist: radius,
                bonuses: def.effect_bonuses.clone(),
            });
            return map;
        }
    }

    // Incoming: selected is a non-road building → find sources that cover it.
    if is_road(sel_def).is_none() {
        map.mode = CoverageMode::Incoming;
        let mut affected_sources = Vec::new();
        let mut hits = Vec::new();
        let mut road_union: HashSet<Cell> = HashSet::new();

        for (i, src) in buildings.iter().enumerate() {
            if i == selected {
                continue;
            }
            let Some(sdef) = catalog.building(src.guid) else {
                continue;
            };
            if !has_area_effect(sdef) {
                continue;
            }
            if sdef.street_distance.is_none()
                && sdef.radius_distance.is_none()
                && sdef.effect_scope.is_none()
            {
                continue;
            }
            if !effect_applies_to(sdef, sel_def) {
                continue;
            }
            let (street_budget, radius, use_street, use_radius) = source_ranges(sdef);
            let src_own = footprint_cells_for_region(src, Some(sdef), region_guid, drag);
            let seeds = road_seeds(&src_own, &roads);
            let reach = if use_street {
                if let Some(budget) = street_budget {
                    street_reach(&seeds, &roads, budget)
                } else {
                    HashMap::new()
                }
            } else {
                HashMap::new()
            };
            let (via_s, via_r) = covers_target(
                src,
                sel,
                &sel_own,
                &reach,
                use_street,
                use_radius,
                radius,
            );
            if via_s || via_r {
                affected_sources.push(i);
                if via_s {
                    road_union.extend(reach.keys().copied());
                }
                hits.push(EffectHit {
                    source_index: i,
                    source_guid: src.guid,
                    source_name: display_name(Some(sdef), src.guid),
                    service_kind: sdef.service_kind.clone(),
                    via_street: via_s,
                    via_radius: via_r,
                    street_dist: street_budget,
                    radius_dist: radius,
                    bonuses: sdef.effect_bonuses.clone(),
                });
            }
        }
        map.affected = affected_sources;
        map.hits = hits;
        map.reachable_roads = road_union;
    }

    map
}

#[derive(Debug, Clone)]
pub struct LayoutStats {
    pub building_count: usize,
    pub unique_guids: usize,
    pub road_cells: usize,
    pub dirt_cells: usize,
    pub stone_cells: usize,
    pub occupied_cells: usize,
    pub bbox_cells: usize,
    pub empty_cells_in_bbox: usize,
    pub fill_ratio: f32,
}

pub fn layout_stats(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
) -> LayoutStats {
    layout_stats_region(buildings, catalog, document, 0)
}

pub fn layout_stats_region(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
) -> LayoutStats {
    let drag = drag_of(document);
    let mut occupied: HashSet<Cell> = HashSet::new();
    let roads = build_road_map_region(buildings, catalog, document, region_guid);
    for b in buildings {
        for c in footprint_cells_for_region(b, catalog.building(b.guid), region_guid, drag) {
            occupied.insert(c);
        }
    }
    for c in roads.keys() {
        occupied.insert(*c);
    }
    let mut unique = HashSet::new();
    for b in buildings {
        unique.insert(b.guid);
    }
    if occupied.is_empty() {
        return LayoutStats {
            building_count: buildings.len(),
            unique_guids: unique.len(),
            road_cells: roads.len(),
            dirt_cells: roads.values().filter(|k| **k == RoadKind::Dirt).count(),
            stone_cells: roads.values().filter(|k| **k == RoadKind::Stone).count(),
            occupied_cells: 0,
            bbox_cells: 0,
            empty_cells_in_bbox: 0,
            fill_ratio: 0.0,
        };
    }
    let min_x = occupied.iter().map(|c| c.x).min().unwrap();
    let max_x = occupied.iter().map(|c| c.x).max().unwrap();
    let min_y = occupied.iter().map(|c| c.y).min().unwrap();
    let max_y = occupied.iter().map(|c| c.y).max().unwrap();
    let bbox = ((max_x - min_x + 1) * (max_y - min_y + 1)) as usize;
    let empty = bbox.saturating_sub(occupied.len());
    LayoutStats {
        building_count: buildings.len(),
        unique_guids: unique.len(),
        road_cells: roads.len(),
        dirt_cells: roads.values().filter(|k| **k == RoadKind::Dirt).count(),
        stone_cells: roads.values().filter(|k| **k == RoadKind::Stone).count(),
        occupied_cells: occupied.len(),
        bbox_cells: bbox,
        empty_cells_in_bbox: empty,
        fill_ratio: if bbox == 0 {
            0.0
        } else {
            occupied.len() as f32 / bbox as f32
        },
    }
}

/// Outline color for a service kind.
pub fn kind_color(kind: Option<&str>) -> (u8, u8, u8) {
    match kind {
        Some("fire") => (220, 80, 60),
        Some("riot") => (200, 160, 40),
        Some("illness") => (80, 180, 120),
        Some("public") => (90, 140, 220),
        Some("production") => (180, 100, 200),
        Some("institution") => (220, 100, 80),
        _ => (100, 180, 220),
    }
}

#[cfg(test)]
mod regression_footprint {
    use super::*;
    use crate::catalog::BuildingDef;

    fn def(w: f32, h: f32) -> BuildingDef {
        BuildingDef {
            guid: 1,
            name: "t".into(),
            short_name: None,
            name_en: None,
            name_de: None,
            template: String::new(),
            category: "public".into(),
            size: [w, h],
            size_marsh: None,
            blocker_min: None,
            outputs: vec![],
            inputs: vec![],
            cycle_ms: None,
            influence_radius: None,
            needed_area: None,
            module_limit: None,
            farm_type: None,
            radius_distance: None,
            street_distance: None,
            effect_scope: None,
            effect_bonuses: vec![],
            service_kind: None,
            road_kind: None,
            needs_fuel: false,
            logistic_speed_factor: None,
            tileset_folder: None,
            constructable: None,
            regions: vec![],
        }
    }

    fn building(x: f32, y: f32) -> Building {
        Building {
            guid: 1,
            x,
            y,
            direction: 0,
            complex_owner_id: None,
            variation: None,
        }
    }

    /// place_center_from_top_left(tx,ty,bw,bh) = (tx+bw/2, ty+bh/2)
    #[test]
    fn odd_size_from_top_left_covers_exact_tiles() {
        // Market 5×6 placed at top-left (0,0) → center (2.5, 3.0)
        let cells = footprint_cells(&building(2.5, 3.0), Some(&def(5.0, 6.0)));
        assert_eq!(cells.len(), 30);
        let xs: Vec<_> = cells.iter().map(|c| c.x).collect();
        let ys: Vec<_> = cells.iter().map(|c| c.y).collect();
        assert_eq!(*xs.iter().min().unwrap(), 0);
        assert_eq!(*xs.iter().max().unwrap(), 4);
        assert_eq!(*ys.iter().min().unwrap(), 0);
        assert_eq!(*ys.iter().max().unwrap(), 5);
    }

    #[test]
    fn even_size_from_top_left_covers_exact_tiles() {
        // Tavern 4×6 at top-left (1,2) → center (3.0, 5.0)
        let cells = footprint_cells(&building(3.0, 5.0), Some(&def(4.0, 6.0)));
        assert_eq!(cells.len(), 24);
        assert_eq!(cells.iter().map(|c| c.x).min(), Some(1));
        assert_eq!(cells.iter().map(|c| c.x).max(), Some(4));
        assert_eq!(cells.iter().map(|c| c.y).min(), Some(2));
        assert_eq!(cells.iter().map(|c| c.y).max(), Some(7));
    }

    #[test]
    fn integer_center_on_odd_width_still_integer_aabb() {
        // Regression: wrong snap used integer centers for 5-wide buildings.
        // Footprint must still land on integer cells (not half-tiles).
        let cells = footprint_cells(&building(2.0, 3.0), Some(&def(5.0, 6.0)));
        for c in &cells {
            // cells are i32 by type; ensure contiguous block
            let _ = c;
        }
        let min_x = cells.iter().map(|c| c.x).min().unwrap();
        let max_x = cells.iter().map(|c| c.x).max().unwrap();
        assert_eq!(max_x - min_x + 1, 5);
    }

    #[test]
    fn aqueduct_cistern_decentered_blocker_flush_to_road() {
        // Game-correct Pos for a 3×3 cistern with TL at x=1 (road at x=0): Pos.x = 1.5.
        // Old centered math used Pos.x = TL+1.5 = 2.5 → +1 tile gap in-game.
        let mut cistern = building(1.5, 1.5);
        cistern.guid = 19753;
        let mut d = def(3.0, 3.0);
        d.guid = 19753;
        let cells = footprint_cells(&cistern, Some(&d));
        assert_eq!(cells.iter().map(|c| c.x).min(), Some(1));
        assert_eq!(cells.iter().map(|c| c.x).max(), Some(3));
        assert_eq!(cells.iter().map(|c| c.y).min(), Some(0));
        assert_eq!(cells.iter().map(|c| c.y).max(), Some(2));

        // Centered write would have been Pos=(2.5, 1.5) for the same editor TL —
        // that footprint must start at x=2 (one tile off the road).
        let mut wrong = building(2.5, 1.5);
        wrong.guid = 19753;
        let wrong_cells = footprint_cells(&wrong, Some(&d));
        assert_eq!(wrong_cells.iter().map(|c| c.x).min(), Some(2));
    }
}

