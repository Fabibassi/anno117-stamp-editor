//! Resource-area productivity (Förster / Waldlager / Marsh / Embankment / Meadow).
//!
//! Wiki / community rule: count usable resource tiles inside `InfluenceRadius`;
//! efficiency = min(100%, have / NeededArea).
//!
//! Same-class overlap (Anno 1800 model): a free tile is **exclusive** — it
//! counts fully for exactly one building. Assignment:
//! 1. Unique tiles (only one claimant) go to that building.
//! 2. Contested tiles go to the neediest covering building (highest
//!    `need − have`), preferring those still below NeededArea; ties → nearest
//!    building, then lower index. Different classes (Forest vs Meadow) do not
//!    compete.
//!
//! Stamps have no terrain — we approximate dense resource cover on every tile
//! that is not occupied by a building footprint, a road, or a farm field polygon.

use crate::catalog::{BuildingDef, Catalog};
use crate::coverage::{build_road_map_region, footprint_cells_for_region, Cell};
use crate::filedb::DocumentOrdered;
use crate::meshgraph::read_mesh_info;
use crate::stamp::{Building, PolygonCell};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    Building,
    Road,
    Field,
}

#[derive(Debug, Clone)]
pub struct AreaEfficiency {
    pub building_index: usize,
    pub guid: u32,
    pub label: String,
    /// Competition class (`forest`, `meadow`, `marsh`, …).
    pub competition_group: String,
    /// Effective free resource tiles after blockers + exclusive assignment.
    pub have: f32,
    /// Free tiles only this building covers (no competitor).
    pub have_exclusive: f32,
    /// Contested free tiles this building won in the priority pass.
    pub have_contested_won: f32,
    pub need: f32,
    pub radius: f32,
    /// 0.0 ..= 1.0
    pub efficiency: f32,
    /// All tiles that fall in the influence circle (before blockers).
    pub raw_in_radius: f32,
    pub blocked_buildings: f32,
    pub blocked_roads: f32,
    pub blocked_fields: f32,
    /// Free tiles also covered by ≥1 other same-group competitor.
    pub contested_tiles: f32,
}

/// Production Area / Marsh Area / Embankment — resource circle, not farm fields.
pub fn is_resource_area_building(def: &BuildingDef) -> bool {
    matches!(
        def.template.as_str(),
        "Production Area" | "Production Marsh Area" | "ProductionEmbankmentArea"
    )
}

pub fn resource_need(def: &BuildingDef) -> Option<f32> {
    if !is_resource_area_building(def) {
        return None;
    }
    let need = def.needed_area.filter(|a| *a > 0.0)?;
    Some(need)
}

/// Catalog `InfluenceRadius`, or a soft fallback when assets omit it (Resin / Dates).
pub fn resource_radius(def: &BuildingDef) -> Option<f32> {
    if !is_resource_area_building(def) {
        return None;
    }
    if let Some(r) = def.influence_radius {
        if r > 0.0 {
            return Some(r);
        }
    }
    // Wood 80→8, Charcoal 80→9, Marsh birds 180→10 — roughly r ≈ √(need/π)·1.6 + 2
    let need = def.needed_area.filter(|a| *a > 0.0)?;
    Some((need / std::f32::consts::PI).sqrt() * 1.6 + 2.0)
}

/// Buildings that harvest the same terrain class compete for free tiles.
///
/// Derived from catalog short names (`Forest …`, `Meadow …`, `Riverbank …`)
/// and templates (`Production Marsh Area`, `ProductionEmbankmentArea`).
pub fn competition_group(def: &BuildingDef) -> &'static str {
    match def.template.as_str() {
        "Production Marsh Area" => "marsh",
        "ProductionEmbankmentArea" => "embankment",
        _ => {
            let name = def.short_name.as_deref().unwrap_or(def.name.as_str());
            let lower = name.to_ascii_lowercase();
            if lower.contains("forest") {
                "forest"
            } else if lower.contains("meadow") {
                "meadow"
            } else if lower.contains("riverbank") || lower.contains("embankment") {
                "embankment"
            } else if lower.contains("marsh") {
                "marsh"
            } else {
                // Same template without a terrain tag → still compete with peers.
                "production"
            }
        }
    }
}

fn drag_of(document: Option<&DocumentOrdered>) -> (f32, f32) {
    document
        .and_then(read_mesh_info)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0))
}

fn is_street_def(def: Option<&BuildingDef>) -> bool {
    def.is_some_and(|d| d.category == "street" || d.road_kind.is_some())
}

/// Build the blocker map: cell → why it cannot grow resources.
pub fn blocker_map(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
    polygon_cells: &[PolygonCell],
) -> HashMap<Cell, BlockReason> {
    let drag = drag_of(document);
    let roads = build_road_map_region(buildings, catalog, document, region_guid);
    let mut blocked: HashMap<Cell, BlockReason> = HashMap::new();

    for b in buildings {
        let def = catalog.building(b.guid);
        // Street BuildingInfo footprints are covered by the road map (mesh + road GUIDs).
        if is_street_def(def) {
            continue;
        }
        for c in footprint_cells_for_region(b, def, region_guid, drag) {
            blocked.entry(c).or_insert(BlockReason::Building);
        }
    }
    for &c in roads.keys() {
        blocked.insert(c, BlockReason::Road);
    }
    for pc in polygon_cells {
        blocked
            .entry(Cell { x: pc.x, y: pc.y })
            .or_insert(BlockReason::Field);
    }
    blocked
}

struct Claimant {
    index: usize,
    guid: u32,
    label: String,
    group: &'static str,
    x: f32,
    y: f32,
    radius: f32,
    need: f32,
}

/// Dense-resource approximation: free tiles in radius / NeededArea, capped at 100%.
pub fn analyze_area_efficiency(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
) -> Vec<AreaEfficiency> {
    analyze_area_efficiency_with_fields(buildings, catalog, document, region_guid, &[])
}

pub fn analyze_area_efficiency_with_fields(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
    polygon_cells: &[PolygonCell],
) -> Vec<AreaEfficiency> {
    let blocked = blocker_map(buildings, catalog, document, region_guid, polygon_cells);

    let mut claimants: Vec<Claimant> = Vec::new();
    for (i, b) in buildings.iter().enumerate() {
        let Some(def) = catalog.building(b.guid) else {
            continue;
        };
        let Some(need) = resource_need(def) else {
            continue;
        };
        let Some(radius) = resource_radius(def) else {
            continue;
        };
        claimants.push(Claimant {
            index: i,
            guid: b.guid,
            label: catalog.label(b.guid),
            group: competition_group(def),
            x: b.x,
            y: b.y,
            radius,
            need,
        });
    }

    // Per-claimant blocker stats + free cells in radius.
    let mut raw: HashMap<usize, f32> = HashMap::new();
    let mut blocked_b: HashMap<usize, f32> = HashMap::new();
    let mut blocked_r: HashMap<usize, f32> = HashMap::new();
    let mut blocked_f: HashMap<usize, f32> = HashMap::new();
    let mut free_cells: HashMap<usize, Vec<Cell>> = HashMap::new();
    for c in &claimants {
        let mut cells = Vec::new();
        for (cell, reason) in radius_cells(c.x, c.y, c.radius, &blocked) {
            *raw.entry(c.index).or_default() += 1.0;
            match reason {
                Some(BlockReason::Building) => {
                    *blocked_b.entry(c.index).or_default() += 1.0;
                }
                Some(BlockReason::Road) => {
                    *blocked_r.entry(c.index).or_default() += 1.0;
                }
                Some(BlockReason::Field) => {
                    *blocked_f.entry(c.index).or_default() += 1.0;
                }
                None => cells.push(cell),
            }
        }
        free_cells.insert(c.index, cells);
    }

    // group → cell → building indices covering that free tile.
    let mut cover: HashMap<&'static str, HashMap<Cell, Vec<usize>>> = HashMap::new();
    for c in &claimants {
        let map = cover.entry(c.group).or_default();
        for &cell in free_cells.get(&c.index).into_iter().flatten() {
            map.entry(cell).or_default().push(c.index);
        }
    }

    // Claimant lookup by building index.
    let by_index: HashMap<usize, &Claimant> =
        claimants.iter().map(|c| (c.index, c)).collect();

    // have accumulates exclusive + won contested.
    let mut have: HashMap<usize, f32> = HashMap::new();
    let mut have_excl: HashMap<usize, f32> = HashMap::new();
    let mut have_won: HashMap<usize, f32> = HashMap::new();
    let mut contested_in_radius: HashMap<usize, f32> = HashMap::new();

    // Phase 1: unique tiles.
    for c in &claimants {
        let group_map = cover.get(c.group);
        let mut excl = 0.0_f32;
        let mut contested = 0.0_f32;
        for &cell in free_cells.get(&c.index).into_iter().flatten() {
            let n = group_map
                .and_then(|m| m.get(&cell))
                .map(|v| v.len())
                .unwrap_or(1);
            if n <= 1 {
                excl += 1.0;
            } else {
                contested += 1.0;
            }
        }
        have_excl.insert(c.index, excl);
        contested_in_radius.insert(c.index, contested);
        have.insert(c.index, excl);
        have_won.insert(c.index, 0.0);
    }

    // Phase 2: contested tiles — exclusive assignment by need priority.
    let mut contested_cells: Vec<(&str, Cell, Vec<usize>)> = Vec::new();
    for (group, map) in &cover {
        for (cell, owners) in map {
            if owners.len() >= 2 {
                contested_cells.push((group, *cell, owners.clone()));
            }
        }
    }
    contested_cells.sort_by(|a, b| (a.1.y, a.1.x).cmp(&(b.1.y, b.1.x)));

    for (_group, cell, owners) in contested_cells {
        let cx = cell.x as f32 + 0.5;
        let cy = cell.y as f32 + 0.5;
        let mut best: Option<(usize, f32, f32)> = None; // idx, deficit, -dist2
        for &idx in &owners {
            let Some(cl) = by_index.get(&idx) else {
                continue;
            };
            let h = *have.get(&idx).unwrap_or(&0.0);
            let deficit = (cl.need - h).max(0.0);
            let dx = cx - cl.x;
            let dy = cy - cl.y;
            let dist2 = dx * dx + dy * dy;
            // Prefer still-needy buildings; among them max deficit, then nearest.
            let score_deficit = if deficit > 0.0 {
                deficit
            } else {
                // Already satisfied — only take surplus after all needy are done.
                // Use tiny negative so needy always win.
                -1.0
            };
            let cand = (idx, score_deficit, -dist2);
            let take = match best {
                None => true,
                Some((bi, bd, bdist)) => {
                    if (cand.1 - bd).abs() > 1e-4 {
                        cand.1 > bd
                    } else if (cand.2 - bdist).abs() > 1e-6 {
                        cand.2 > bdist // less negative dist2 = nearer
                    } else {
                        cand.0 < bi
                    }
                }
            };
            if take {
                best = Some(cand);
            }
        }
        // Second pass: if everyone is satisfied (all score_deficit < 0), pick nearest.
        let winner = if best.is_some_and(|(_, d, _)| d < 0.0) {
            owners
                .iter()
                .copied()
                .min_by(|a, b| {
                    let ca = by_index[a];
                    let cb = by_index[b];
                    let da = (cx - ca.x).powi(2) + (cy - ca.y).powi(2);
                    let db = (cx - cb.x).powi(2) + (cy - cb.y).powi(2);
                    da.partial_cmp(&db)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.cmp(b))
                })
        } else {
            best.map(|(i, _, _)| i)
        };
        if let Some(w) = winner {
            *have.entry(w).or_default() += 1.0;
            *have_won.entry(w).or_default() += 1.0;
        }
    }

    let mut out = Vec::with_capacity(claimants.len());
    for c in &claimants {
        let h = *have.get(&c.index).unwrap_or(&0.0);
        let efficiency = if c.need > 0.0 {
            (h / c.need).clamp(0.0, 1.0)
        } else {
            1.0
        };
        out.push(AreaEfficiency {
            building_index: c.index,
            guid: c.guid,
            label: c.label.clone(),
            competition_group: c.group.to_string(),
            have: h,
            have_exclusive: *have_excl.get(&c.index).unwrap_or(&0.0),
            have_contested_won: *have_won.get(&c.index).unwrap_or(&0.0),
            need: c.need,
            radius: c.radius,
            efficiency,
            raw_in_radius: *raw.get(&c.index).unwrap_or(&0.0),
            blocked_buildings: *blocked_b.get(&c.index).unwrap_or(&0.0),
            blocked_roads: *blocked_r.get(&c.index).unwrap_or(&0.0),
            blocked_fields: *blocked_f.get(&c.index).unwrap_or(&0.0),
            contested_tiles: *contested_in_radius.get(&c.index).unwrap_or(&0.0),
        });
    }
    out
}

/// Cells inside the influence radius of one building, with free/blocked classification.
pub fn radius_cells(
    cx: f32,
    cy: f32,
    radius: f32,
    blocked: &HashMap<Cell, BlockReason>,
) -> Vec<(Cell, Option<BlockReason>)> {
    let r = radius.max(0.0);
    let mut out = Vec::new();
    if r <= 0.0 {
        return out;
    }
    let r2 = r * r;
    let min_x = (cx - r).floor() as i32;
    let max_x = (cx + r).ceil() as i32;
    let min_y = (cy - r).floor() as i32;
    let max_y = (cy + r).ceil() as i32;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy > r2 {
                continue;
            }
            let cell = Cell { x, y };
            out.push((cell, blocked.get(&cell).copied()));
        }
    }
    out
}

/// Free cells covered by ≥2 buildings of `group` (for canvas contested tint).
pub fn contested_free_cells(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
    polygon_cells: &[PolygonCell],
    group: &str,
) -> HashMap<Cell, usize> {
    let blocked = blocker_map(buildings, catalog, document, region_guid, polygon_cells);
    let mut cover: HashMap<Cell, usize> = HashMap::new();
    for b in buildings {
        let Some(def) = catalog.building(b.guid) else {
            continue;
        };
        if resource_need(def).is_none() {
            continue;
        }
        if competition_group(def) != group {
            continue;
        }
        let Some(radius) = resource_radius(def) else {
            continue;
        };
        for (cell, reason) in radius_cells(b.x, b.y, radius, &blocked) {
            if reason.is_none() {
                *cover.entry(cell).or_insert(0) += 1;
            }
        }
    }
    cover.retain(|_, n| *n >= 2);
    cover
}

/// Index → efficiency (missing → 1.0 = no penalty).
pub fn efficiency_by_index(areas: &[AreaEfficiency]) -> HashMap<usize, f32> {
    areas
        .iter()
        .map(|a| (a.building_index, a.efficiency))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::BuildingDef;

    fn area_def(
        guid: u32,
        template: &str,
        short: &str,
        size: [f32; 2],
        radius: f32,
        need: f32,
    ) -> BuildingDef {
        BuildingDef {
            guid,
            name: format!("Test {guid}"),
            short_name: Some(short.into()),
            template: template.into(),
            category: "production".into(),
            size,
            size_marsh: None,
            outputs: vec![],
            inputs: vec![],
            cycle_ms: Some(60.0),
            influence_radius: Some(radius),
            needed_area: Some(need),
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
            constructable: Some(true),
            regions: vec![],
        }
    }

    fn road_def(guid: u32) -> BuildingDef {
        let mut d = area_def(guid, "Street", "Street", [1.0, 1.0], 0.0, 0.0);
        d.template = "Street".into();
        d.category = "street".into();
        d.road_kind = Some("dirt".into());
        d.influence_radius = None;
        d.needed_area = None;
        d.short_name = None;
        d
    }

    #[test]
    fn detects_all_area_templates() {
        for t in [
            "Production Area",
            "Production Marsh Area",
            "ProductionEmbankmentArea",
        ] {
            let d = area_def(1, t, "Forest Test", [2.0, 2.0], 8.0, 80.0);
            assert!(is_resource_area_building(&d));
            assert_eq!(resource_need(&d), Some(80.0));
            assert_eq!(resource_radius(&d), Some(8.0));
        }
        let farm = area_def(2, "PlantFarm", "Farm", [3.0, 3.0], 8.0, 80.0);
        assert!(!is_resource_area_building(&farm));
    }

    #[test]
    fn competition_group_from_name_and_template() {
        let wood = area_def(1, "Production Area", "Forest Roman Wood", [2.0, 2.0], 8.0, 80.0);
        let honey = area_def(2, "Production Area", "Meadow Roman Honeycombs", [2.0, 2.0], 8.0, 80.0);
        let birds = area_def(3, "Production Marsh Area", "Roman Celtic Birds", [2.0, 2.0], 10.0, 180.0);
        let hippo = area_def(
            4,
            "ProductionEmbankmentArea",
            "Riverbank Greco Roman Hippos",
            [2.0, 2.0],
            8.0,
            100.0,
        );
        assert_eq!(competition_group(&wood), "forest");
        assert_eq!(competition_group(&honey), "meadow");
        assert_eq!(competition_group(&birds), "marsh");
        assert_eq!(competition_group(&hippo), "embankment");
    }

    #[test]
    fn lone_woodcutter_reaches_full_efficiency() {
        let wood = area_def(2878, "Production Area", "Forest Roman Wood", [2.0, 2.0], 8.0, 80.0);
        let mut map = HashMap::new();
        map.insert(2878, wood);
        let catalog = Catalog {
            buildings: map,
            products: HashMap::new(),
        };
        let buildings = vec![Building {
            guid: 2878,
            x: 20.0,
            y: 20.0,
            direction: 0,
            variation: None,
            complex_owner_id: None,
        }];
        let areas = analyze_area_efficiency(&buildings, &catalog, None, 0);
        assert_eq!(areas.len(), 1);
        assert!(areas[0].have >= 80.0, "have={}", areas[0].have);
        assert!((areas[0].efficiency - 1.0).abs() < 1e-3);
        assert!(areas[0].blocked_buildings > 0.0); // own footprint
        assert_eq!(areas[0].contested_tiles, 0.0);
    }

    #[test]
    fn two_overlapping_woodcutters_compete_exclusively() {
        let wood = area_def(2878, "Production Area", "Forest Roman Wood", [2.0, 2.0], 8.0, 80.0);
        let mut map = HashMap::new();
        map.insert(2878, wood);
        let catalog = Catalog {
            buildings: map,
            products: HashMap::new(),
        };
        let alone = vec![Building {
            guid: 2878,
            x: 20.0,
            y: 20.0,
            direction: 0,
            variation: None,
            complex_owner_id: None,
        }];
        let alone_have = analyze_area_efficiency(&alone, &catalog, None, 0)[0].have;

        let pair = vec![
            Building {
                guid: 2878,
                x: 20.0,
                y: 20.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
            Building {
                guid: 2878,
                x: 22.0,
                y: 20.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
        ];
        let areas = analyze_area_efficiency(&pair, &catalog, None, 0);
        assert_eq!(areas.len(), 2);
        assert!(areas[0].contested_tiles > 0.0);
        assert!(areas[1].contested_tiles > 0.0);
        // Each tile goes to exactly one claimant: unique + contested_won sums match free pool.
        let sum_have = areas[0].have + areas[1].have;
        let sum_unique = areas[0].have_exclusive + areas[1].have_exclusive;
        let contested_union = areas[0].have_contested_won + areas[1].have_contested_won;
        assert!(
            (sum_have - (sum_unique + contested_union)).abs() < 1e-3,
            "have={} unique+won={}",
            sum_have,
            sum_unique + contested_union
        );
        // No double-counting contested: won tiles ≤ each side's contested count.
        assert!(areas[0].have_contested_won <= areas[0].contested_tiles + 1e-3);
        assert!(areas[1].have_contested_won <= areas[1].contested_tiles + 1e-3);
        // Overlap must cost someone vs alone.
        assert!(
            areas[0].have < alone_have || areas[1].have < alone_have,
            "at least one woodcutter should lose tiles vs alone"
        );
        // have is exclusive (integer-ish) assignment, not fractional 1/N.
        for a in &areas {
            assert!(
                (a.have - a.have.round()).abs() < 1e-3,
                "exclusive assignment should yield whole tiles, got {}",
                a.have
            );
            assert!(a.have + 1e-3 >= a.have_exclusive);
        }
    }

    #[test]
    fn wood_and_charcoal_compete_on_forest() {
        let wood = area_def(2878, "Production Area", "Forest Roman Wood", [2.0, 2.0], 8.0, 80.0);
        let coal = area_def(2880, "Production Area", "Forest Roman Charcoal", [2.0, 2.0], 9.0, 80.0);
        let mut map = HashMap::new();
        map.insert(2878, wood);
        map.insert(2880, coal);
        let catalog = Catalog {
            buildings: map,
            products: HashMap::new(),
        };
        let pair = vec![
            Building {
                guid: 2878,
                x: 20.0,
                y: 20.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
            Building {
                guid: 2880,
                x: 22.0,
                y: 20.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
        ];
        let areas = analyze_area_efficiency(&pair, &catalog, None, 0);
        assert!(areas.iter().any(|a| a.contested_tiles > 0.0));
        let won: f32 = areas.iter().map(|a| a.have_contested_won).sum();
        assert!(won > 0.0, "contested tiles must be assigned to someone");
    }

    #[test]
    fn satisfied_building_yields_contested_to_needy_neighbor() {
        // Large need on B, tiny need on A — A fills from exclusive, contested go to B.
        let a_def = area_def(1, "Production Area", "Forest A", [1.0, 1.0], 6.0, 5.0);
        let b_def = area_def(2, "Production Area", "Forest B", [1.0, 1.0], 6.0, 80.0);
        let mut map = HashMap::new();
        map.insert(1, a_def);
        map.insert(2, b_def);
        let catalog = Catalog {
            buildings: map,
            products: HashMap::new(),
        };
        let buildings = vec![
            Building {
                guid: 1,
                x: 10.0,
                y: 10.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
            Building {
                guid: 2,
                x: 12.0,
                y: 10.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
        ];
        let areas = analyze_area_efficiency(&buildings, &catalog, None, 0);
        let a = areas.iter().find(|x| x.guid == 1).unwrap();
        let b = areas.iter().find(|x| x.guid == 2).unwrap();
        assert!(a.efficiency >= 0.999, "tiny-need A should hit 100% from exclusives");
        assert!(
            b.have_contested_won + 1e-3 >= a.have_contested_won,
            "needy B should win most contested (B won {}, A won {})",
            b.have_contested_won,
            a.have_contested_won
        );
    }

    #[test]
    fn wood_and_honey_do_not_compete() {
        let wood = area_def(2878, "Production Area", "Forest Roman Wood", [2.0, 2.0], 8.0, 80.0);
        let honey = area_def(
            2795,
            "Production Area",
            "Meadow Roman Honeycombs",
            [2.0, 2.0],
            8.0,
            80.0,
        );
        let mut map = HashMap::new();
        map.insert(2878, wood);
        map.insert(2795, honey);
        let catalog = Catalog {
            buildings: map,
            products: HashMap::new(),
        };
        let pair = vec![
            Building {
                guid: 2878,
                x: 20.0,
                y: 20.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
            Building {
                guid: 2795,
                x: 20.0,
                y: 20.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            },
        ];
        let areas = analyze_area_efficiency(&pair, &catalog, None, 0);
        assert_eq!(areas.len(), 2);
        for a in &areas {
            assert_eq!(a.contested_tiles, 0.0, "{} should not contest across groups", a.label);
            assert!((a.have - a.have_exclusive).abs() < 1e-3);
        }
    }

    #[test]
    fn roads_and_other_buildings_reduce_free_tiles() {
        let wood = area_def(2878, "Production Area", "Forest Roman Wood", [2.0, 2.0], 8.0, 80.0);
        let road = road_def(100);
        let hut = area_def(200, "Residence", "Hut", [3.0, 3.0], 0.0, 0.0);
        let mut hut = hut;
        hut.template = "Residence".into();
        hut.influence_radius = None;
        hut.needed_area = None;

        let mut map = HashMap::new();
        map.insert(2878, wood);
        map.insert(100, road);
        map.insert(200, hut);
        let catalog = Catalog {
            buildings: map,
            products: HashMap::new(),
        };

        let alone = vec![Building {
            guid: 2878,
            x: 20.0,
            y: 20.0,
            direction: 0,
            variation: None,
            complex_owner_id: None,
        }];
        let alone_have = analyze_area_efficiency(&alone, &catalog, None, 0)[0].have;

        let mut packed = alone.clone();
        // Ring of roads + a hut inside the woodcutter radius.
        for i in -6..=6 {
            packed.push(Building {
                guid: 100,
                x: 20.0 + i as f32,
                y: 20.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            });
        }
        packed.push(Building {
            guid: 200,
            x: 24.0,
            y: 24.0,
            direction: 0,
            variation: None,
            complex_owner_id: None,
        });
        let with = analyze_area_efficiency(&packed, &catalog, None, 0)[0].clone();
        assert!(
            with.have < alone_have,
            "with roads/buildings have={} alone={}",
            with.have,
            alone_have
        );
        assert!(with.blocked_roads > 0.0, "roads should block");
        assert!(
            with.blocked_buildings > alone.iter().len() as f32,
            "hut footprint should add blockers"
        );
    }

    #[test]
    fn field_polygons_block_resource_tiles() {
        let wood = area_def(2878, "Production Area", "Forest Roman Wood", [2.0, 2.0], 8.0, 80.0);
        let mut map = HashMap::new();
        map.insert(2878, wood);
        let catalog = Catalog {
            buildings: map,
            products: HashMap::new(),
        };
        let buildings = vec![Building {
            guid: 2878,
            x: 10.0,
            y: 10.0,
            direction: 0,
            variation: None,
            complex_owner_id: None,
        }];
        let fields: Vec<PolygonCell> = (0..5)
            .flat_map(|dx| {
                (0..5).map(move |dy| PolygonCell {
                    guid: 1,
                    x: 10 + dx,
                    y: 10 + dy,
                    complex_owner_id: None,
                })
            })
            .collect();
        let without = analyze_area_efficiency_with_fields(
            &buildings, &catalog, None, 0, &[],
        )[0]
        .have;
        let with = analyze_area_efficiency_with_fields(
            &buildings, &catalog, None, 0, &fields,
        )[0]
        .clone();
        assert!(with.have < without);
        assert!(with.blocked_fields >= 20.0);
    }
}
