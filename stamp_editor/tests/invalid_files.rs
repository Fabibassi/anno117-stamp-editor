//! Error paths: corrupt / empty packs must fail as Result, never panic.

use stamp_editor::pack::{load_pack, open_named_stamp};
use stamp_editor::stamp::decode_pipette_b64;

#[test]
fn empty_json_is_err() {
    let dir = std::env::temp_dir().join("stamp_editor_invalid_files");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("empty.json");
    std::fs::write(&path, b"").unwrap();
    assert!(load_pack(&path).is_err());
}

#[test]
fn garbage_pipette_is_err() {
    assert!(decode_pipette_b64("not-valid-base64!!!").is_err());
}

#[test]
fn pack_with_bad_pipette_open_is_err() {
    let dir = std::env::temp_dir().join("stamp_editor_invalid_files");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("bad_pipette.json");
    let json = r#"{
  "Version": 1,
  "Stamps": [
    {
      "Name": "Broken",
      "FolderName": "",
      "RegionGUID": 0,
      "PipetteData": "not-valid-base64!!!",
      "IconGUID": 0,
      "CreationTime": 0
    }
  ]
}
"#;
    std::fs::write(&path, json).unwrap();
    let err = open_named_stamp(&path, "Broken").expect_err("corrupt pipette must fail");
    let msg = err.to_string();
    assert!(!msg.is_empty());
}
