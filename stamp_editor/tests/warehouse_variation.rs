use stamp_editor::catalog::Catalog;
use stamp_editor::coverage::footprint_cells;
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

#[test]
fn drinking_horns_warehouse_is_land_6x6_not_marsh() {
    let cat = Catalog::load(&root().join("_gamedata/editor_catalog.json")).expect("catalog");
    let pack = load_pack(&testdata().join("Albion/albion.json")).expect("pack");
    let entry = pack
        .stamps
        .iter()
        .find(|s| s.name == "Drinking Horns x4")
        .expect("Drinking Horns x4");
    let stamp = decode_pipette_b64(&entry.pipette_data).expect("decode");
    let wh = stamp
        .buildings
        .iter()
        .find(|b| b.guid == 7055)
        .expect("warehouse 7055");
    assert_eq!(wh.variation, None, "land stamp must not carry marsh Variation");
    let def = cat.building(7055).expect("def");
    assert_eq!(def.size, [6.0, 6.0]);
    assert_eq!(def.size_marsh, Some([5.0, 5.0]));
    let cells = footprint_cells(wh, Some(def));
    assert_eq!(cells.len(), 36, "Drinking Horns warehouse must render 6×6");
}

#[test]
fn warehouse_size_for_variation_switches_land_and_marsh() {
    let cat = Catalog::load(&root().join("_gamedata/editor_catalog.json")).expect("catalog");
    let def = cat.building(7055).expect("warehouse 7055");
    assert_eq!(def.size_for_variation(None), [6.0, 6.0]);
    assert_eq!(def.size_for_variation(Some(1)), [5.0, 5.0]);
    assert!(def.size_marsh.is_some());
}

#[test]
fn set_building_variation_roundtrips_on_reed_shoes_warehouse() {
    let pack = load_pack(&testdata().join("Albion/albion.json")).expect("pack");
    let entry = pack
        .stamps
        .iter()
        .find(|s| s.name == "Reed Shoes x4")
        .expect("Reed Shoes x4");
    let mut stamp = decode_pipette_b64(&entry.pipette_data).expect("decode");
    let idx = stamp
        .buildings
        .iter()
        .position(|b| b.guid == 7055)
        .expect("warehouse");
    assert_eq!(stamp.buildings[idx].variation, Some(1));
    stamp
        .set_building_variation(idx, None)
        .expect("to land");
    assert_eq!(stamp.buildings[idx].variation, None);
    let encoded = stamp_editor::stamp::encode_pipette_b64(&stamp).expect("encode");
    let again = decode_pipette_b64(&encoded).expect("re-decode");
    assert_eq!(again.buildings[idx].variation, None);
    stamp
        .set_building_variation(idx, Some(1))
        .expect("back to marsh");
    assert_eq!(stamp.buildings[idx].variation, Some(1));
}

#[test]
fn reed_shoes_warehouse_uses_marsh_variation_5x5() {
    let cat = Catalog::load(&root().join("_gamedata/editor_catalog.json")).expect("catalog");
    let pack = load_pack(&testdata().join("Albion/albion.json")).expect("pack");
    let entry = pack
        .stamps
        .iter()
        .find(|s| s.name == "Reed Shoes x4")
        .expect("Reed Shoes x4");
    let stamp = decode_pipette_b64(&entry.pipette_data).expect("decode");
    let wh = stamp
        .buildings
        .iter()
        .find(|b| b.guid == 7055)
        .expect("warehouse 7055");
    assert_eq!(wh.variation, Some(1));
    let def = cat.building(7055).expect("def");
    let cells = footprint_cells(wh, Some(def));
    assert_eq!(cells.len(), 25, "Reed Shoes marsh warehouse must render 5×5");
}
