fn main() {
    use stamp_editor::catalog::Catalog;
    use stamp_editor::pack::open_named_stamp;
    use std::path::{Path, PathBuf};

    let pack = PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Albion\Albion Stamp Pack.json",
    );
    let catalog = Catalog::load(Path::new(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\_gamedata\editor_catalog.json",
    ))
    .unwrap_or_default();

    // Summarize warehouse presence for all stamps in Albion pack
    let text = std::fs::read_to_string(&pack).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let stamps = v["Stamps"].as_array().cloned().unwrap_or_default();
    println!("Albion pack stamps: {}", stamps.len());
    let mut with = 0usize;
    let mut without = Vec::new();
    for s in &stamps {
        let name = s["Name"].as_str().unwrap_or("?");
        if s["PipetteData"].as_str().unwrap_or("").is_empty() {
            continue;
        }
        match open_named_stamp(&pack, name) {
            Ok(open) => {
                let whs: Vec<_> = open
                    .stamp
                    .buildings
                    .iter()
                    .filter(|b| matches!(b.guid, 7055 | 7056 | 7057 | 3310 | 3311 | 3312))
                    .collect();
                if whs.is_empty() {
                    without.push(name.to_string());
                } else {
                    with += 1;
                    let b = whs[0];
                    let sz = catalog
                        .building(b.guid)
                        .map(|d| d.size_for_variation(b.variation))
                        .unwrap_or([0.0, 0.0]);
                    println!(
                        "OK  {name}: WH@({:.1},{:.1}) {}×{} var={:?}",
                        b.x, b.y, sz[0] as i32, sz[1] as i32, b.variation
                    );
                }
            }
            Err(e) => println!("ERR {name}: {e}"),
        }
    }
    println!("\nwith warehouse: {with}");
    println!("without warehouse ({}):", without.len());
    for n in without {
        println!("  - {n}");
    }
}

