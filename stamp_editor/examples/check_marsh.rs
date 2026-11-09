use stamp_editor::catalog::Catalog;
use stamp_editor::coverage::footprint_cells;
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn main() {
    let cat = Catalog::load(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\_gamedata\editor_catalog.json",
    ))
    .unwrap();
    let d = cat.building(7055).unwrap();
    println!(
        "default {:?} marsh {:?} var0 {:?} var1 {:?}",
        d.size,
        d.size_marsh,
        d.size_for_variation(None),
        d.size_for_variation(Some(1))
    );
    let pack = load_pack(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Albion\Albion Stamp Pack.json",
    ))
    .unwrap();
    for name in ["Reed Shoes x4", "Drinking Horns x4"] {
        let entry = pack.stamps.iter().find(|s| s.name == name).unwrap();
        let stamp = decode_pipette_b64(&entry.pipette_data).unwrap();
        let wh = stamp.buildings.iter().find(|b| b.guid == 7055).unwrap();
        let cells = footprint_cells(wh, Some(d));
        println!(
            "{name}: variation={:?} cells={} ({}x approx)",
            wh.variation,
            cells.len(),
            (cells.len() as f32).sqrt()
        );
    }
}
