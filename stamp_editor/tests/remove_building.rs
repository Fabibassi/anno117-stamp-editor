use stamp_editor::library::scan_library;
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::{decode_pipette_b64, encode_pipette_b64};
use std::path::PathBuf;

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

#[test]
fn remove_building_reduces_count_and_reencodes() {
    let pack_path = testdata().join("Albion/albion.json");
    let pack = load_pack(&pack_path).expect("load pack");
    let entry = pack
        .stamps
        .iter()
        .find(|s| !s.pipette_data.is_empty())
        .expect("stamp with pipette");
    let mut stamp = decode_pipette_b64(&entry.pipette_data).expect("decode");
    let before = stamp.buildings.len();
    assert!(before >= 1);
    stamp.remove_building(0).expect("remove");
    assert_eq!(stamp.buildings.len(), before - 1);
    let encoded = encode_pipette_b64(&stamp).expect("encode");
    assert!(!encoded.is_empty());
    let again = decode_pipette_b64(&encoded).expect("re-decode");
    assert_eq!(again.buildings.len(), before - 1);
}

#[test]
fn library_scan_finds_region_folders() {
    let packs = scan_library(&testdata());
    assert!(!packs.is_empty(), "expected packs under testdata");
    let folders: Vec<_> = packs.iter().map(|p| p.folder.as_str()).collect();
    assert!(
        folders.iter().any(|f| f.starts_with("Albion") || *f == "Albion"),
        "expected Albion grouping, got {folders:?}"
    );
}
