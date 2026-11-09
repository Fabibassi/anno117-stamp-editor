use stamp_editor::filedb::{dump, load};
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

fn assert_pack_roundtrip(relative: &str) {
    let pack_path = testdata().join(relative);
    let pack = load_pack(&pack_path).unwrap_or_else(|e| panic!("load {relative}: {e}"));
    let mut checked = 0;
    for entry in pack.stamps.iter().filter(|s| !s.pipette_data.is_empty()) {
        let stamp = decode_pipette_b64(&entry.pipette_data)
            .unwrap_or_else(|e| panic!("decode {} / {}: {e}", relative, entry.name));
        let raw = {
            use base64::Engine;
            use flate2::read::ZlibDecoder;
            use std::io::Read;
            let compressed = base64::engine::general_purpose::STANDARD
                .decode(&entry.pipette_data)
                .unwrap();
            let mut d = ZlibDecoder::new(std::io::Cursor::new(compressed));
            let mut raw = Vec::new();
            d.read_to_end(&mut raw).unwrap();
            raw
        };
        let rebuilt = dump(&load(&raw).unwrap());
        assert_eq!(rebuilt, raw, "roundtrip failed for {} / {}", relative, entry.name);
        assert!(
            !stamp.buildings.is_empty(),
            "{} / {}",
            relative,
            entry.name
        );
        checked += 1;
    }
    assert!(checked > 0, "no stamps in {relative}");
}

#[test]
fn albion_filedb_roundtrip() {
    assert_pack_roundtrip("Albion/albion.json");
}

#[test]
fn latium_filedb_roundtrip() {
    assert_pack_roundtrip("Latium/latium.json");
}

#[test]
fn unsorted_filedb_roundtrip() {
    assert_pack_roundtrip("unsorted/woodcutter.json");
    assert_pack_roundtrip("unsorted/lumber x4.json");
}
