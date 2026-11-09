//! Some Latium stamps omit `Pos` on the center warehouse (GUID 3310).
//! Original packs do too — not editor data loss. Missing Pos ⇒ (0,0).

use stamp_editor::pack::open_named_stamp;
use std::path::PathBuf;

fn latium() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/Latium/latium.json")
}

#[test]
fn timber_porridge_leather_keep_center_warehouse() {
    for name in [
        "Timber x4",
        "Timber x8",
        "Timber x12",
        "Porridge x8",
        "Leather x8 (Lard)",
    ] {
        let open = open_named_stamp(&latium(), name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let wh: Vec<_> = open
            .stamp
            .buildings
            .iter()
            .filter(|b| b.guid == 3310)
            .collect();
        assert_eq!(wh.len(), 1, "{name}: expected one Warehouse Roman 01");
        assert!(
            wh[0].x.abs() < 0.01 && wh[0].y.abs() < 0.01,
            "{name}: omitted Pos must default to (0,0), got ({}, {})",
            wh[0].x,
            wh[0].y
        );
    }
}

#[test]
fn moving_posless_warehouse_creates_pos_attrib() {
    let mut open = open_named_stamp(&latium(), "Timber x4").expect("Timber x4");
    let idx = open
        .stamp
        .buildings
        .iter()
        .position(|b| b.guid == 3310)
        .expect("warehouse");
    open.stamp
        .set_building_pos(idx, 1.0, 2.0)
        .expect("set pos on formerly pos-less warehouse");
    assert_eq!(open.stamp.buildings[idx].x, 1.0);
    assert_eq!(open.stamp.buildings[idx].y, 2.0);
}
