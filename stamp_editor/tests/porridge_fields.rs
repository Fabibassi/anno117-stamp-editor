//! Porridge / PlantFarm field loading guards.

use stamp_editor::catalog::Catalog;
use stamp_editor::modules::{
    complex_main_index, count_progress_for_stamp_area, is_module_building, needed_modules,
    uses_area_progress,
};
use stamp_editor::pack::open_named_stamp;
use std::collections::HashSet;
use std::path::PathBuf;

fn latium() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json")
}

fn catalog() -> Catalog {
    Catalog::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("_gamedata/editor_catalog.json"),
    )
    .expect("catalog")
}

#[test]
fn porridge_x4_has_subtile_field_cells_not_fat_modules_only() {
    let open = open_named_stamp(&latium(), "Porridge x4").expect("Porridge x4");
    let cat = catalog();

    assert!(
        open.stamp.polygon_cells.len() >= 80,
        "expected SubTileGrid field tiles, got {}",
        open.stamp.polygon_cells.len()
    );
    assert!(
        open.stamp.polygon_cells.iter().all(|c| c.guid == 2220),
        "field cells should be Module Field Roman Oats"
    );

    // Every polygon_module owner should already have cells (canvas skips fat 3×3 markers).
    let owners_with_cells: HashSet<u64> = open
        .stamp
        .polygon_cells
        .iter()
        .filter_map(|c| c.complex_owner_id.filter(|o| *o != 0))
        .collect();
    for m in &open.stamp.polygon_modules {
        let o = m.complex_owner_id.expect("module owner");
        assert!(
            owners_with_cells.contains(&o),
            "polygon_module owner {o} missing cells"
        );
        assert!(!is_module_building(cat.building(m.guid).unwrap()) || m.guid == 2220);
    }

    let oats: Vec<_> = open
        .stamp
        .buildings
        .iter()
        .enumerate()
        .filter(|(_, b)| b.guid == 2200)
        .collect();
    assert_eq!(oats.len(), 4);
    for (i, b) in oats {
        let def = cat.building(b.guid);
        assert!(uses_area_progress(def));
        assert_eq!(needed_modules(def), 80);
        let have = count_progress_for_stamp_area(
            &open.stamp.buildings,
            &open.stamp.polygon_modules,
            &open.stamp.polygon_cells,
            &open.stamp.polygon_area,
            i,
            def,
        );
        assert_eq!(have, 80, "PlantFarm SubTileGrid should decode to full NeededArea, got {have}");
        assert_eq!(
            complex_main_index(&open.stamp.buildings, &cat, i),
            Some(i)
        );
    }
}
