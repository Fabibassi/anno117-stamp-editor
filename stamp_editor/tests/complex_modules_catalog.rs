//! Catalog-backed checks for farm ↔ module pairing (complex modules design).

use stamp_editor::catalog::Catalog;
use stamp_editor::modules::{
    complex_group_indices, count_modules_for_stamp, needed_modules, suggested_modules,
    uses_area_progress,
};
use stamp_editor::pack::{save_pack, StampEntryJson, StampPackJson};
use stamp_editor::stamp::{encode_pipette_b64, Stamp};
use std::path::PathBuf;

fn catalog() -> Catalog {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("_gamedata")
        .join("editor_catalog.json");
    Catalog::load(&path).expect("editor_catalog.json")
}

#[test]
fn roman_sheep_pasture_needs_three_and_suggests_wool() {
    let cat = catalog();
    let sheep = cat.building(2786).expect("Production Pasture Roman Sheep");
    assert_eq!(sheep.name, "Production Pasture Roman Sheep");
    assert_eq!(sheep.module_limit, Some(3));
    assert_eq!(sheep.farm_type.as_deref(), Some("AnimalFarm"));
    assert_eq!(needed_modules(Some(sheep)), 3);
    assert!(!uses_area_progress(Some(sheep)));

    let mods = suggested_modules(&cat, sheep, 3225); // Latium / Roman
    let guids: Vec<u32> = mods.iter().map(|d| d.guid).collect();
    assert!(
        guids.contains(&2787),
        "expected Module Pasture Roman Wool (2787) in {guids:?}; names={:?}",
        mods.iter().map(|d| &d.name).collect::<Vec<_>>()
    );
}

#[test]
fn roman_oats_uses_tile_limit_eighty() {
    let cat = catalog();
    let oats = cat.building(2200).expect("Production Field Roman Oats");
    assert_eq!(oats.module_limit, Some(80));
    assert_eq!(oats.farm_type.as_deref(), Some("PlantFarm"));
    assert_eq!(needed_modules(Some(oats)), 80);
    assert!(uses_area_progress(Some(oats)));
    // Field modules themselves never show a quota.
    let field = cat.building(2220).expect("Module Field Roman Oats");
    assert_eq!(needed_modules(Some(field)), 0);
}

#[test]
fn sheep_stamp_with_three_wool_modules_is_ready() {
    let mut stamp = Stamp::blank().expect("blank");
    stamp
        .add_building_from_template(2786, 0.0, 0.0, 0, None)
        .expect("sheep main");
    stamp
        .add_building_from_template(2787, 4.0, 0.0, 0, None)
        .expect("wool 1");
    stamp
        .add_building_from_template(2787, 8.0, 0.0, 0, None)
        .expect("wool 2");
    stamp
        .add_building_from_template(2787, 4.0, 4.0, 0, None)
        .expect("wool 3");
    let owner = 42u64;
    for i in 0..4 {
        stamp.set_building_owner(i, owner).expect("owner");
    }

    assert_eq!(complex_group_indices(&stamp.buildings, 0).len(), 4);
    let have = count_modules_for_stamp(&stamp.buildings, &stamp.polygon_modules, 0);
    assert_eq!(have, 3);
    let cat = catalog();
    let need = needed_modules(cat.building(2786));
    assert_eq!(need, 3);
    assert!(have as u32 >= need);

    // Persist into the user library so the editor can open it for visual check.
    let encoded = encode_pipette_b64(&stamp).expect("encode");
    let pack = StampPackJson {
        version: 1,
        stamps: vec![StampEntryJson {
            name: "Sheep Pasture 3-3".into(),
            folder_name: String::new(),
            region_guid: 3225,
            pipette_data: encoded,
            icon_guid: 2786,
            creation_time: 0,
        }],
    };
    let out = PathBuf::from(r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Region Roman\sheep pasture 3-3.json");
    save_pack(&out, &pack).expect("save sheep pack");
    assert!(out.is_file());
}
