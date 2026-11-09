use stamp_editor::meshgraph::read_mesh_info;
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::path::PathBuf;

fn main() {
    let pack = load_pack(&PathBuf::from(
        r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Region Roman\latium.json",
    ))
    .unwrap();
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Loungers x4".into());
    let entry = pack.stamps.iter().find(|s| s.name == name).unwrap();
    let stamp = decode_pipette_b64(&entry.pipette_data).unwrap();
    let info = read_mesh_info(&stamp.document).unwrap();
    println!(
        "stamp={name} drag={:?} aabb={:?} tile_count={} edges={}",
        info.drag_center,
        info.aabb,
        info.tile_count,
        info.edges.len()
    );
    let (cx, cy) = info.drag_center;
    for (i, e) in info.edges.iter().enumerate() {
        let x0 = e.x0 as f32 * 0.5 - cx;
        let y0 = e.y0 as f32 * 0.5 - cy;
        let x1 = e.x1 as f32 * 0.5 - cx;
        let y1 = e.y1 as f32 * 0.5 - cy;
        println!(
            "e{i:02} guid={} raw=({},{})->({},{}) tile=({:.1},{:.1})->({:.1},{:.1})",
            e.guid, e.x0, e.y0, e.x1, e.y1, x0, y0, x1, y1
        );
    }
    let minx = stamp
        .buildings
        .iter()
        .map(|b| b.x)
        .fold(f32::INFINITY, f32::min);
    let maxx = stamp
        .buildings
        .iter()
        .map(|b| b.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let miny = stamp
        .buildings
        .iter()
        .map(|b| b.y)
        .fold(f32::INFINITY, f32::min);
    let maxy = stamp
        .buildings
        .iter()
        .map(|b| b.y)
        .fold(f32::NEG_INFINITY, f32::max);
    println!("building centers x={minx}..{maxx} y={miny}..{maxy}");
}
