//! Module / sub-building helpers (farms, pens, pastures).

use crate::catalog::{BuildingDef, Catalog};
use crate::stamp::{Building, PolygonCell};

/// Indices of all buildings sharing `ComplexOwnerID` with `index` (incl. itself).
/// Empty if the building has no owner / owner 0.
pub fn complex_group_indices(buildings: &[Building], index: usize) -> Vec<usize> {
    let Some(owner) = buildings.get(index).and_then(|b| b.complex_owner_id) else {
        return Vec::new();
    };
    if owner == 0 {
        return Vec::new();
    }
    buildings
        .iter()
        .enumerate()
        .filter(|(_, b)| b.complex_owner_id == Some(owner))
        .map(|(i, _)| i)
        .collect()
}

/// Count module buildings linked to the main at `main_index` via ComplexOwnerID.
pub fn count_modules_for(buildings: &[Building], main_index: usize) -> usize {
    let group = complex_group_indices(buildings, main_index);
    group.iter().filter(|i| **i != main_index).count()
}

/// BuildingInfo modules + Polygons field markers sharing the main's ComplexOwnerID.
pub fn count_modules_for_stamp(
    buildings: &[Building],
    polygon_modules: &[Building],
    main_index: usize,
) -> usize {
    let Some(owner) = buildings.get(main_index).and_then(|b| b.complex_owner_id) else {
        return 0;
    };
    if owner == 0 {
        return 0;
    }
    let from_bi = buildings
        .iter()
        .enumerate()
        .filter(|(i, b)| *i != main_index && b.complex_owner_id == Some(owner))
        .count();
    let from_poly = polygon_modules
        .iter()
        .filter(|b| b.complex_owner_id == Some(owner))
        .count();
    from_bi + from_poly
}

/// True when progress is measured in SubTileGrid tiles (PlantFarm / large limits).
/// Resource-area buildings (Waldlager / Marsh / Embankment) use `area_efficiency` instead.
pub fn uses_area_progress(def: Option<&BuildingDef>) -> bool {
    let Some(d) = def else {
        return false;
    };
    if crate::area_efficiency::is_resource_area_building(d) {
        return false;
    }
    if d.farm_type.as_deref() == Some("PlantFarm") {
        return true;
    }
    if d.module_limit.unwrap_or(0) >= 10 {
        return true;
    }
    d.needed_area.unwrap_or(0.0) >= 10.0
}

/// Farm progress toward ModuleLimits / NeededArea.
/// Plant farms: SubTileGrid area (`set_bits/4` via `polygon_area`). Animal farms: modules.
pub fn count_progress_for_stamp(
    buildings: &[Building],
    polygon_modules: &[Building],
    polygon_cells: &[PolygonCell],
    main_index: usize,
    main_def: Option<&BuildingDef>,
) -> usize {
    count_progress_for_stamp_area(
        buildings,
        polygon_modules,
        polygon_cells,
        &std::collections::HashMap::new(),
        main_index,
        main_def,
    )
}

/// Like `count_progress_for_stamp`, but prefers `polygon_area` (exact `set_bits/4`) for PlantFarms.
pub fn count_progress_for_stamp_area(
    buildings: &[Building],
    polygon_modules: &[Building],
    polygon_cells: &[PolygonCell],
    polygon_area: &std::collections::HashMap<u64, u32>,
    main_index: usize,
    main_def: Option<&BuildingDef>,
) -> usize {
    let Some(owner) = buildings.get(main_index).and_then(|b| b.complex_owner_id) else {
        return 0;
    };
    if owner == 0 {
        return 0;
    }
    if uses_area_progress(main_def) {
        if let Some(&area) = polygon_area.get(&owner) {
            return area as usize;
        }
        return polygon_cells
            .iter()
            .filter(|c| c.complex_owner_id == Some(owner))
            .count();
    }
    count_modules_for_stamp(buildings, polygon_modules, main_index)
}

/// Best-effort main building inside a ComplexOwnerID group (farm, not pasture/field).
pub fn complex_main_index(
    buildings: &[Building],
    catalog: &Catalog,
    index: usize,
) -> Option<usize> {
    let group = complex_group_indices(buildings, index);
    if group.is_empty() {
        return None;
    }
    let is_sub = |i: usize| -> bool {
        catalog
            .building(buildings[i].guid)
            .map(|d| is_module_building(d) || d.category == "module")
            .unwrap_or(false)
    };
    let mains: Vec<usize> = group.iter().copied().filter(|&i| !is_sub(i)).collect();
    if mains.is_empty() {
        return Some(group[0]);
    }
    mains
        .into_iter()
        .max_by_key(|&i| {
            let d = catalog.building(buildings[i].guid);
            let need = d
                .map(|x| {
                    x.module_limit.unwrap_or_else(|| {
                        x.needed_area.unwrap_or(0.0).round().max(0.0) as u32
                    }) as i32
                })
                .unwrap_or(0);
            let area = d
                .map(|x| (x.size[0] * x.size[1]) as i32)
                .unwrap_or(0);
            (need, area)
        })
}

/// Needed progress from catalog: `module_limit` (preferred), else `needed_area`.
/// Animal pastures without either still default to 3 module slots.
/// Resource-area buildings are excluded — their NeededArea is forest/marsh tiles.
pub fn needed_modules(def: Option<&BuildingDef>) -> u32 {
    let Some(d) = def else {
        return 0;
    };
    // Never treat field modules themselves as needing a quota overlay.
    if is_module_building(d) || d.category == "module" {
        return 0;
    }
    if crate::area_efficiency::is_resource_area_building(d) {
        return 0;
    }
    if let Some(lim) = d.module_limit {
        return lim;
    }
    if let Some(a) = d.needed_area {
        return a.round().max(0.0) as u32;
    }
    let n = d.name.to_lowercase();
    let animal = d.farm_type.as_deref() == Some("AnimalFarm")
        || n.contains("pasture")
        || n.contains("meadow");
    if animal {
        return 3;
    }
    0
}

/// Placement radius in tiles for modules around a main building.
pub fn module_radius(def: Option<&BuildingDef>) -> f32 {
    if let Some(d) = def {
        if let Some(r) = d.influence_radius {
            if r > 0.0 {
                return r;
            }
        }
        let need = needed_modules(Some(d)) as f32;
        if need >= 10.0 {
            return (need.sqrt() * 3.0 + d.size[0].max(d.size[1])).max(6.0);
        }
        if let Some(a) = d.needed_area {
            return (a.sqrt() * 3.0 + d.size[0].max(d.size[1])).max(6.0);
        }
        return d.size[0].max(d.size[1]) + 8.0;
    }
    10.0
}

fn is_farm_module_template(template: &str) -> bool {
    matches!(
        template,
        "ModulePolygonField" | "ModuleField" | "ModuleMarshField" | "ProductionModuleSilo"
    )
}

/// True if this asset is itself a placeable field/pasture module (not a farm main).
pub fn is_module_building(def: &BuildingDef) -> bool {
    def.category == "module" && is_farm_module_template(&def.template)
}

fn is_placeable_farm_module(d: &BuildingDef) -> bool {
    if d.constructable == Some(false) || d.category != "module" {
        return false;
    }
    if !is_farm_module_template(&d.template) {
        return false;
    }
    let n = d.name.to_lowercase();
    !n.contains("ship")
        && !n.contains("notification")
        && !n.contains("infotip")
        && !n.contains("tooltip")
        && !n.contains("techreward")
        && !n.starts_with("image ")
        && !n.starts_with("icon ")
        && !n.starts_with("tech ")
}

/// Crop / animal token used to pair farm mains with their field modules.
fn product_token(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    const TOKENS: &[&str] = &[
        "wheat", "barley", "oats", "flax", "hemp", "grape", "herb", "lavender", "olive", "dye",
        "reed", "auroch", "pig", "sheep", "wool", "horse", "cattle", "cow", "sau", "camel",
        "lotus", "pony", "ponies", "honey", "charcoal", "resin", "timber", "wood",
    ];
    TOKENS.iter().copied().find(|t| n.contains(t))
}

/// Tokens that should match each other when pairing main ↔ module (sheep farm ↔ wool pasture).
fn product_aliases(token: &str) -> &'static [&'static str] {
    match token {
        "sheep" | "wool" => &["sheep", "wool"],
        "pig" | "sau" => &["pig", "sau"],
        "horse" | "pony" | "ponies" => &["horse", "pony", "ponies"],
        "cattle" | "cow" | "auroch" | "sanga" => &["cattle", "cow", "auroch", "sanga"],
        "wood" | "timber" => &["wood", "timber"],
        other => {
            // single-element slices for unknown tokens
            match other {
                "wheat" => &["wheat"],
                "barley" => &["barley"],
                "oats" => &["oats"],
                "flax" => &["flax"],
                "hemp" => &["hemp"],
                "grape" => &["grape"],
                "herb" => &["herb"],
                "lavender" => &["lavender"],
                "olive" => &["olive"],
                "dye" => &["dye"],
                "reed" => &["reed"],
                "camel" => &["camel"],
                "lotus" => &["lotus"],
                "honey" => &["honey"],
                "charcoal" => &["charcoal"],
                "resin" => &["resin"],
                _ => &[],
            }
        }
    }
}

fn region_ok(d: &BuildingDef, region_guid: u32) -> bool {
    if region_guid == 0 || d.regions.is_empty() {
        return true;
    }
    match region_guid {
        6626 => d.regions.iter().any(|r| r == "Celtic"),
        3225 => d.regions.iter().any(|r| r == "Roman"),
        _ => true,
    }
}

fn is_farm_main(main: &BuildingDef) -> bool {
    if is_module_building(main) || crate::area_efficiency::is_resource_area_building(main) {
        return false;
    }
    let n = main.name.to_lowercase();
    let t = main.template.to_lowercase();
    main.needed_area.is_some()
        || main.template == "Production Field"
        || n.contains("farm")
        || t.contains("farm")
        || product_token(&main.name).is_some() && main.category == "production"
}

/// Suggest module GUIDs from catalog for a selected main (heuristic).
pub fn suggested_modules<'a>(
    catalog: &'a Catalog,
    main: &BuildingDef,
    region_guid: u32,
) -> Vec<&'a BuildingDef> {
    if !is_farm_main(main) {
        return Vec::new();
    }

    let mut out: Vec<&'a BuildingDef> = catalog
        .buildings_iter()
        .filter(|d| is_placeable_farm_module(d))
        .filter(|d| region_ok(d, region_guid))
        .collect();

    if let Some(token) = product_token(&main.name) {
        let aliases = product_aliases(token);
        let matched: Vec<_> = out
            .iter()
            .copied()
            .filter(|d| {
                let n = d.name.to_lowercase();
                aliases.iter().any(|a| n.contains(a))
            })
            .collect();
        if !matched.is_empty() {
            out = matched;
        }
    }

    out.into_iter().take(8).collect()
}

pub fn dist_tiles(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = ax - bx;
    let dy = ay - by;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stamp::Building;

    #[test]
    fn complex_group_bidirectional() {
        let buildings = vec![
            Building {
                guid: 1,
                x: 0.0,
                y: 0.0,
                direction: 0,
                complex_owner_id: Some(42),
                variation: None,
            },
            Building {
                guid: 2,
                x: 1.0,
                y: 0.0,
                direction: 0,
                complex_owner_id: Some(42),
                variation: None,
            },
            Building {
                guid: 3,
                x: 2.0,
                y: 0.0,
                direction: 0,
                complex_owner_id: Some(99),
                variation: None,
            },
        ];
        assert_eq!(complex_group_indices(&buildings, 0), vec![0, 1]);
        assert_eq!(complex_group_indices(&buildings, 1), vec![0, 1]);
        assert_eq!(complex_group_indices(&buildings, 2), vec![2]);
    }

    fn def(name: &str, template: &str, category: &str, constructable: bool) -> BuildingDef {
        BuildingDef {
            guid: 1,
            name: name.into(),
            short_name: None,
            name_en: None,
            name_de: None,
            template: template.into(),
            category: category.into(),
            size: [3.0, 3.0],
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
            constructable: Some(constructable),
            regions: vec!["Celtic".into()],
        }
    }

    #[test]
    fn rejects_ship_and_notification_noise() {
        assert!(is_placeable_farm_module(&def(
            "Module Field Roman Celtic Wheat",
            "ModulePolygonField",
            "module",
            true
        )));
        assert!(!is_placeable_farm_module(&def(
            "Celtic ShipModule Hull",
            "ShipModule",
            "module",
            false
        )));
        assert!(!is_placeable_farm_module(&def(
            "Notification BB Villa Module intro",
            "SequenceCharNotif",
            "module",
            false
        )));
        assert!(!is_module_building(&def(
            "Production Field Roman Celtic Wheat",
            "Production Field",
            "production",
            true
        )));
    }

    #[test]
    fn pasture_defaults_to_three_modules_and_sheep_pairs_wool() {
        let mut sheep = def(
            "Production Pasture Roman Sheep",
            "Production Field",
            "production",
            true,
        );
        sheep.farm_type = Some("AnimalFarm".into());
        sheep.module_limit = Some(3);
        assert_eq!(needed_modules(Some(&sheep)), 3);
        assert!(!uses_area_progress(Some(&sheep)));
        assert!(product_aliases("sheep").contains(&"wool"));

        let mut oats = def(
            "Production Field Roman Oats",
            "Production Field",
            "production",
            true,
        );
        oats.farm_type = Some("PlantFarm".into());
        oats.module_limit = Some(80);
        assert_eq!(needed_modules(Some(&oats)), 80);
        assert!(uses_area_progress(Some(&oats)));

        let buildings = vec![
            Building {
                guid: 2786,
                x: 0.0,
                y: 0.0,
                direction: 0,
                complex_owner_id: Some(7),
                variation: None,
            },
            Building {
                guid: 2787,
                x: 4.0,
                y: 0.0,
                direction: 0,
                complex_owner_id: Some(7),
                variation: None,
            },
            Building {
                guid: 2787,
                x: 8.0,
                y: 0.0,
                direction: 0,
                complex_owner_id: Some(7),
                variation: None,
            },
            Building {
                guid: 2787,
                x: 4.0,
                y: 4.0,
                direction: 0,
                complex_owner_id: Some(7),
                variation: None,
            },
        ];
        let poly = vec![];
        assert_eq!(count_modules_for_stamp(&buildings, &poly, 0), 3);
        assert_eq!(complex_group_indices(&buildings, 0).len(), 4);
    }
}

