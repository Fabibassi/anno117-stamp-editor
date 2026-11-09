//! Olive Oil: kitchen has no ComplexOwner — progress belongs on Production Field.

use stamp_editor::catalog::Catalog;
use stamp_editor::modules::{complex_main_index, count_progress_for_stamp_area, needed_modules};
use stamp_editor::pack::open_named_stamp;
use std::path::PathBuf;

fn catalog() -> Catalog {
    Catalog::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("_gamedata/editor_catalog.json"),
    )
    .expect("catalog")
}

fn olive_stamp() -> stamp_editor::pack::OpenStamp {
    open_named_stamp(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json"),
        "Olive Oil x3 v2",
    )
    .expect("Olive Oil x3 v2")
}

#[test]
fn household_olive_oil_has_no_complex_main() {
    let cat = catalog();
    let open = olive_stamp();
    for (i, b) in open.stamp.buildings.iter().enumerate() {
        if b.guid != 4831 {
            continue;
        }
        assert_eq!(
            b.complex_owner_id.filter(|o| *o != 0),
            None,
            "kitchen must not carry farm ComplexOwnerID"
        );
        assert_eq!(
            complex_main_index(&open.stamp.buildings, &cat, i),
            None,
            "kitchen must not open Untergebäude progress UI"
        );
        assert_eq!(needed_modules(cat.building(4831)), 0);
    }
}

#[test]
fn olive_production_field_reports_tile_progress() {
    let cat = catalog();
    let open = olive_stamp();
    let i = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 2699)
        .expect("Production Field Roman Olives");
    let main = complex_main_index(&open.stamp.buildings, &cat, i).expect("main");
    let def = cat.building(open.stamp.buildings[main].guid);
    let need = needed_modules(def);
    assert_eq!(need, 160);
    let have = count_progress_for_stamp_area(
        &open.stamp.buildings,
        &open.stamp.polygon_modules,
        &open.stamp.polygon_cells,
        &open.stamp.polygon_area,
        main,
        def,
    );
    assert_eq!(have, need as usize, "olive SubTileGrid should be full NeededArea");
}
