//! Core workflow: open → edit → save → reload (no data loss).

use stamp_editor::pack::{load_pack, open_named_stamp, save_open_stamp, OpenStamp};
use stamp_editor::stamp::{decode_pipette_b64, encode_pipette_b64};
use std::path::PathBuf;

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

#[test]
fn woodcutter_move_save_reload_keeps_position() {
    let src = testdata().join("unsorted/woodcutter.json");
    let open = open_named_stamp(&src, "Woodcutter").expect("open");
    assert!(!open.stamp.buildings.is_empty());

    let i = 0usize;
    let (ox, oy) = (open.stamp.buildings[i].x, open.stamp.buildings[i].y);
    let nx = ox + 2.0;
    let ny = oy + 1.0;

    let dir = std::env::temp_dir().join("stamp_editor_edit_save_reload");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmpdir");
    let out = dir.join("woodcutter_edited.json");

    let mut to_save = OpenStamp {
        pack_path: out.clone(),
        entry_index: 0,
        entry_name: open.entry_name.clone(),
        region_guid: open.region_guid,
        icon_guid: open.icon_guid,
        stamp: open.stamp.clone(),
        dirty: true,
        is_new: true,
    };
    to_save
        .stamp
        .set_building_pos(i, nx, ny)
        .expect("move building");
    save_open_stamp(&to_save).expect("save");

    let reloaded = open_named_stamp(&out, "Woodcutter").expect("reload");
    assert_eq!(reloaded.stamp.buildings[i].x, nx);
    assert_eq!(reloaded.stamp.buildings[i].y, ny);
    assert_eq!(
        reloaded.stamp.buildings.len(),
        open.stamp.buildings.len(),
        "building count must survive roundtrip"
    );
}

#[test]
fn latium_olive_encode_decode_preserves_field_cells() {
    let pack = load_pack(&testdata().join("Latium/latium.json")).expect("latium");
    let entry = pack
        .stamps
        .iter()
        .find(|s| s.name == "Olive Oil x3 v2")
        .expect("Olive Oil x3 v2");
    let stamp = decode_pipette_b64(&entry.pipette_data).expect("decode");
    let cells_before = stamp.polygon_cells.len();
    assert!(cells_before > 100, "expected field cells");

    let encoded = encode_pipette_b64(&stamp).expect("encode");
    let again = decode_pipette_b64(&encoded).expect("re-decode");
    assert_eq!(
        again.polygon_cells.len(),
        cells_before,
        "field cell count must survive encode→decode"
    );
    assert_eq!(again.buildings.len(), stamp.buildings.len());
}
