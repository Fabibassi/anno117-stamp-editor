//! Data-loss guard: saving must target the open stamp by name, not a stale index.

use stamp_editor::pack::{
    load_pack, open_named_stamp, save_open_stamp, save_pack, StampEntryJson, StampPackJson,
};
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn tmp_pack() -> PathBuf {
    let dir = std::env::temp_dir().join("stamp_editor_save_by_name");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("pack.json")
}

#[test]
fn save_open_stamp_finds_entry_by_name_after_index_shift() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/unsorted/woodcutter.json");
    let wood = open_named_stamp(&src, "Woodcutter").expect("woodcutter");
    let lumber_src =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/unsorted/lumber x4.json");
    let lumber = stamp_editor::pack::open_first_real_stamp(&lumber_src).expect("lumber");

    let path = tmp_pack();
    let pack = StampPackJson {
        version: 1,
        stamps: vec![
            StampEntryJson {
                name: "Alpha".into(),
                folder_name: String::new(),
                region_guid: wood.region_guid,
                pipette_data: {
                    use stamp_editor::stamp::encode_pipette_b64;
                    encode_pipette_b64(&wood.stamp).unwrap()
                },
                icon_guid: wood.icon_guid,
                creation_time: 1,
            },
            StampEntryJson {
                name: "Beta".into(),
                folder_name: String::new(),
                region_guid: lumber.region_guid,
                pipette_data: {
                    use stamp_editor::stamp::encode_pipette_b64;
                    encode_pipette_b64(&lumber.stamp).unwrap()
                },
                icon_guid: lumber.icon_guid,
                creation_time: 2,
            },
            StampEntryJson {
                name: "Gamma".into(),
                folder_name: String::new(),
                region_guid: wood.region_guid,
                pipette_data: {
                    use stamp_editor::stamp::encode_pipette_b64;
                    encode_pipette_b64(&wood.stamp).unwrap()
                },
                icon_guid: wood.icon_guid,
                creation_time: 3,
            },
        ],
    };
    save_pack(&path, &pack).unwrap();

    // Open Beta (index 1), then delete Alpha on disk → Beta becomes index 0, Gamma index 1.
    let mut open = open_named_stamp(&path, "Beta").expect("open Beta");
    assert_eq!(open.entry_index, 1);

    let mut on_disk = load_pack(&path).unwrap();
    on_disk.stamps.retain(|s| s.name != "Alpha");
    save_pack(&path, &on_disk).unwrap();
    // Stale index still points at 1 — which is now Gamma.
    open.entry_index = 1;
    open.entry_name = "Beta".into();
    open.dirty = true;

    // Edit Beta: move first building.
    let (x, y) = (open.stamp.buildings[0].x, open.stamp.buildings[0].y);
    open.stamp
        .set_building_pos(0, x + 5.0, y + 5.0)
        .expect("move");
    save_open_stamp(&open).expect("save");

    let after = load_pack(&path).unwrap();
    let beta = after.stamps.iter().find(|s| s.name == "Beta").expect("Beta");
    let gamma = after.stamps.iter().find(|s| s.name == "Gamma").expect("Gamma");
    let beta_stamp = decode_pipette_b64(&beta.pipette_data).unwrap();
    let gamma_stamp = decode_pipette_b64(&gamma.pipette_data).unwrap();
    assert!(
        (beta_stamp.buildings[0].x - (x + 5.0)).abs() < 0.01,
        "Beta must receive the edit"
    );
    assert!(
        (gamma_stamp.buildings[0].x - wood.stamp.buildings[0].x).abs() < 0.01,
        "Gamma must stay untouched (was index 1 after delete)"
    );
}
