use crate::catalog::{
    blocker_min_for, is_aqueduct_cistern, oriented_blocker, AQUEDUCT_CISTERN_SIZE,
};
use crate::filedb::{
    dump, load, Attrib, DocumentOrdered, Node, Tag, NONE_ATTRIB_ID, NONE_TAG_ID,
};
use crate::meshgraph::{
    clear_all_mesh_paths, clone_polygon_for_new_building, read_mesh_info, recompute_bounds,
    relative_to_tile, sync_after_building_move,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::collections::HashSet;
use std::io::{Read, Write};
use thiserror::Error;

/// BuildingInfo marker: cistern Pos already uses the decentered pivot (not TL+size/2).
const CISTERN_PIVOT_ATTRIB: &str = "RusticusCisternPivot";

#[derive(Debug, Error)]
pub enum StampError {
    #[error(transparent)]
    FileDb(#[from] crate::filedb::FileDbError),
    #[error("zlib error: {0}")]
    Zlib(String),
    #[error("missing building field")]
    MissingField,
}

#[derive(Debug, Clone)]
pub struct Building {
    pub guid: u32,
    pub x: f32,
    pub y: f32,
    pub direction: i32,
    pub complex_owner_id: Option<u64>,
    /// Pipette `Variation` index (e.g. Warehouse 7055: 1 = marsh CFG 5×5).
    pub variation: Option<u32>,
}

/// One occupied tile from a `Polygons` / SubTileGrid farm field.
#[derive(Debug, Clone, Copy)]
pub struct PolygonCell {
    pub guid: u32,
    pub x: i32,
    pub y: i32,
    pub complex_owner_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Stamp {
    pub document: DocumentOrdered,
    pub buildings: Vec<Building>,
    /// Field/pasture modules stored under `Polygons` (GUID + RelativePos), not BuildingInfo.
    /// Fallback when a polygon has no SubTileGrid bits.
    pub polygon_modules: Vec<Building>,
    /// Occupied tiles decoded from SubTileGrid bitmasks (fills stamp road wings).
    pub polygon_cells: Vec<PolygonCell>,
    /// PlantFarm area toward ModuleLimits: ComplexOwnerID → `set_bits / 4`
    /// (4 horizontal quarter-bits = 1 tile). Prefer over `polygon_cells.len()` for progress.
    pub polygon_area: std::collections::HashMap<u64, u32>,
    pub stamp_version: i32,
    /// How many aqueduct cisterns were pivot-corrected during the last migrate/decode.
    pub cistern_fixes_on_load: usize,
}

/// How to rewrite legacy centered cistern Pos → decentered pivot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CisternMigrateMode {
    /// Road-flush heuristic + `RusticusCisternPivot` marker (safe default).
    Auto,
    /// Always re-anchor unmarked cisterns (centered TL → decentered Pos).
    Force,
}

fn u32_le(data: &[u8]) -> u32 {
    if data.len() < 4 {
        return 0;
    }
    u32::from_le_bytes(data[0..4].try_into().unwrap())
}

fn i32_le(data: &[u8]) -> i32 {
    if data.len() < 4 {
        return 0;
    }
    i32::from_le_bytes(data[0..4].try_into().unwrap())
}

fn f32x2(data: &[u8]) -> Option<(f32, f32)> {
    if data.len() < 8 {
        return None;
    }
    Some((
        f32::from_le_bytes(data[0..4].try_into().unwrap()),
        f32::from_le_bytes(data[4..8].try_into().unwrap()),
    ))
}

fn find_tag<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Tag> {
    nodes.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == name => Some(t),
        _ => None,
    })
}

fn find_attrib<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Attrib> {
    nodes.iter().find_map(|n| match n {
        Node::Attrib(a) if a.name == name => Some(a),
        _ => None,
    })
}

fn find_tag_mut<'a>(nodes: &'a mut [Node], name: &str) -> Option<&'a mut Tag> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == name => Some(t),
        _ => None,
    })
}

fn find_attrib_mut<'a>(nodes: &'a mut [Node], name: &str) -> Option<&'a mut Attrib> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Attrib(a) if a.name == name => Some(a),
        _ => None,
    })
}

fn read_direction(building: &Tag) -> i32 {
    let Some(dir_tag) = find_tag(&building.children, "Dir") else {
        return 0;
    };
    let Some(value) = find_tag(&dir_tag.children, "value") else {
        return 0;
    };
    let Some(none1) = find_tag(&value.children, "None") else {
        return 0;
    };
    let Some(attrib) = find_attrib(&none1.children, "None") else {
        return 0;
    };
    i32_le(&attrib.content)
}

/// Aqueduct cisterns (Distributor) — in-game stamps omit `Variation`; adding
/// `Variation=0` makes the game refuse to place them in a blueprint.
fn is_cistern_guid(guid: u32) -> bool {
    matches!(guid, 19753 | 29526 | 149794) // Roman / Celtic / Egyptian Distributor
}

fn building_entry_guid(entry: &Tag) -> Option<u32> {
    find_attrib(&entry.children, "GUID").map(|a| u32_le(&a.content))
}

fn remove_variation_attrib(entry: &mut Tag) {
    entry
        .children
        .retain(|n| !matches!(n, Node::Attrib(a) if a.name == "Variation"));
}

/// Keep a `Variation` attrib for normal buildings (game rejects modern stamps without it).
fn apply_variation_attrib(entry: &mut Tag, id: u16, value: u32) {
    if building_entry_guid(entry).is_some_and(is_cistern_guid) {
        remove_variation_attrib(entry);
        return;
    }
    let bytes = value.to_le_bytes().to_vec();
    if let Some(a) = find_attrib_mut(&mut entry.children, "Variation") {
        a.id = id;
        a.content = bytes;
        return;
    }
    // Insert after GUID when possible (matches in-game field order).
    let insert_at = entry
        .children
        .iter()
        .position(|n| matches!(n, Node::Attrib(a) if a.name == "GUID"))
        .map(|i| i + 1)
        .unwrap_or(entry.children.len());
    entry.children.insert(
        insert_at,
        Node::Attrib(Attrib {
            id,
            name: "Variation".into(),
            content: bytes,
        }),
    );
}

fn write_direction(entry: &mut Tag, direction: i32) -> bool {
    let Some(dir_tag) = entry.children.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == "Dir" => Some(t),
        _ => None,
    }) else {
        return false;
    };
    let Some(value) = dir_tag.children.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == "value" => Some(t),
        _ => None,
    }) else {
        return false;
    };
    let Some(none1) = value.children.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == "None" => Some(t),
        _ => None,
    }) else {
        return false;
    };
    let Some(attrib) = find_attrib_mut(&mut none1.children, "None") else {
        return false;
    };
    attrib.content = direction.to_le_bytes().to_vec();
    true
}

fn owner_id_from_attrib(data: &[u8]) -> Option<u64> {
    if data.len() >= 8 {
        Some(u64::from_le_bytes(data[0..8].try_into().unwrap()))
    } else if data.len() >= 4 {
        Some(u32_le(data) as u64)
    } else {
        None
    }
}

fn extract_buildings(doc: &DocumentOrdered) -> Vec<Building> {
    let mut out = Vec::new();
    for root in &doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
            continue;
        };
        for child in &info.children {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let Some(guid_a) = find_attrib(&entry.children, "GUID") else {
                continue;
            };
            // Some authored stamps omit Pos on the center warehouse (GUID 3310);
            // the game treats that as DragRectCenter-relative (0,0). Skipping them
            // hid the WH and desynced BuildingInfo indices vs `buildings[]`.
            let pos = find_attrib(&entry.children, "Pos")
                .and_then(|a| f32x2(&a.content))
                .unwrap_or((0.0, 0.0));
            let owner = find_attrib(&entry.children, "ComplexOwnerID")
                .and_then(|a| owner_id_from_attrib(&a.content));
            let variation = find_attrib(&entry.children, "Variation").and_then(|a| {
                if a.content.len() >= 4 {
                    Some(u32_le(&a.content))
                } else {
                    None
                }
            });
            out.push(Building {
                guid: u32_le(&guid_a.content),
                x: pos.0,
                y: pos.1,
                direction: read_direction(entry),
                complex_owner_id: owner,
                variation,
            });
        }
    }
    out
}

fn drag_center(doc: &DocumentOrdered) -> (f32, f32) {
    for root in &doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        if let Some(mesh) = find_tag(&pipette.children, "MeshGraphData") {
            if let Some(p) = find_attrib(&mesh.children, "DragRectCenter").and_then(|a| f32x2(&a.content))
            {
                return p;
            }
        }
        if let Some(p) =
            find_attrib(&pipette.children, "DragRectCenter").and_then(|a| f32x2(&a.content))
        {
            return p;
        }
    }
    (0.0, 0.0)
}

fn bit_set(bits: &[u8], index: usize) -> bool {
    let byte = index / 8;
    let bit = index % 8;
    bits.get(byte).map(|b| (b >> bit) & 1 == 1).unwrap_or(false)
}

fn bit_set_mut(bits: &mut Vec<u8>, index: usize, on: bool) {
    let byte = index / 8;
    let bit = index % 8;
    if bits.len() <= byte {
        bits.resize(byte + 1, 0);
    }
    if on {
        bits[byte] |= 1 << bit;
    } else {
        bits[byte] &= !(1 << bit);
    }
}

fn subtile_stride(gx: usize) -> usize {
    ((gx + 31) / 32) * 32
}

/// Copy bits into a possibly larger grid (origin may move left/up).
fn remap_subtile_bits(
    old_bits: &[u8],
    old_ox: i32,
    old_oy: i32,
    old_gx: usize,
    old_gy: usize,
    new_ox: i32,
    new_oy: i32,
    new_gx: usize,
    new_gy: usize,
) -> Vec<u8> {
    let old_stride = subtile_stride(old_gx);
    let new_stride = subtile_stride(new_gx);
    let mut out = vec![0u8; (new_stride * new_gy + 7) / 8];
    let shift_x = (old_ox - new_ox) * 4;
    let shift_y = old_oy - new_oy;
    for sy in 0..old_gy {
        for sx in 0..old_gx {
            if !bit_set(old_bits, sy * old_stride + sx) {
                continue;
            }
            let nsx = sx as i32 + shift_x;
            let nsy = sy as i32 + shift_y;
            if nsx < 0 || nsy < 0 {
                continue;
            }
            let nsx = nsx as usize;
            let nsy = nsy as usize;
            if nsx >= new_gx || nsy >= new_gy {
                continue;
            }
            bit_set_mut(&mut out, nsy * new_stride + nsx, true);
        }
    }
    out
}

/// Decode SubTileGrid → unique tile cells + set-bit count.
/// `set_bits / 4` is the PlantFarm area unit (2×2 half-tile bits per tile).
fn subtile_grid_cells(
    entry: &Tag,
    drag: (f32, f32),
    guid: u32,
    complex_owner_id: Option<u64>,
) -> (Vec<PolygonCell>, u32) {
    let Some(stg) = find_tag(&entry.children, "SubTileGrid") else {
        return (Vec::new(), 0);
    };
    let Some(origin_a) = find_attrib(&stg.children, "GridOriginWS") else {
        return (Vec::new(), 0);
    };
    if origin_a.content.len() < 8 {
        return (Vec::new(), 0);
    }
    let ox_abs = i32_le(&origin_a.content[0..4]);
    let oy_abs = i32_le(&origin_a.content[4..8]);
    let Some(grid_wrap) = find_tag(&stg.children, "Grid") else {
        return (Vec::new(), 0);
    };
    let Some(grid) = find_tag(&grid_wrap.children, "grid") else {
        return (Vec::new(), 0);
    };
    let gx = find_attrib(&grid.children, "x")
        .map(|a| i32_le(&a.content))
        .unwrap_or(0)
        .max(0) as usize;
    let gy = find_attrib(&grid.children, "y")
        .map(|a| i32_le(&a.content))
        .unwrap_or(0)
        .max(0) as usize;
    let Some(bits_a) = find_attrib(&grid.children, "bits") else {
        return (Vec::new(), 0);
    };
    if gx == 0 || gy == 0 {
        return (Vec::new(), 0);
    }
    // SubTileGrid bit layout (Anno 117 PlantFarm):
    //   - each ROW = one world tile in Y
    //   - every 4 COLUMNS = one world tile in X (horizontal quarter-bits)
    // Isotropic 0.25 collapsed height (~3–4 tiles); isotropic 0.5 stretched X past the
    // road frame. Area unit remains set_bits/4. Rows pad to 32-bit stride.
    let stride = ((gx + 31) / 32) * 32;
    let mut cells = std::collections::BTreeSet::new();
    let mut set_bits = 0u32;
    for sy in 0..gy {
        for sx in 0..gx {
            let idx = sy * stride + sx;
            if !bit_set(&bits_a.content, idx) {
                continue;
            }
            set_bits += 1;
            let ax = ox_abs + (sx as i32) / 4;
            let ay = oy_abs + sy as i32;
            let tx = ax - drag.0.floor() as i32;
            let ty = ay - drag.1.floor() as i32;
            cells.insert((tx, ty));
        }
    }
    let cells = cells
        .into_iter()
        .map(|(x, y)| PolygonCell {
            guid,
            x,
            y,
            complex_owner_id,
        })
        .collect();
    (cells, set_bits)
}

/// Farm modules that live in `Polygons` (RelativePos + GUID), common in production stamps.
fn extract_polygon_modules(
    doc: &DocumentOrdered,
) -> (
    Vec<Building>,
    Vec<PolygonCell>,
    std::collections::HashMap<u64, u32>,
) {
    let drag = drag_center(doc);
    let mut modules = Vec::new();
    let mut cells = Vec::new();
    let mut area: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    for root in &doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(polys) = find_tag(&pipette.children, "Polygons") else {
            continue;
        };
        for child in &polys.children {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let Some(guid_a) = find_attrib(&entry.children, "GUID") else {
                continue;
            };
            let guid = u32_le(&guid_a.content);
            if guid == 0 {
                continue;
            }
            let owner = find_attrib(&entry.children, "ComplexOwnerID")
                .and_then(|a| owner_id_from_attrib(&a.content));
            let (grid_cells, set_bits) = subtile_grid_cells(entry, drag, guid, owner);
            if let Some(own) = owner.filter(|o| *o != 0) {
                *area.entry(own).or_default() += set_bits / 4;
            }
            // Module marker (icon + ComplexOwner link) even when SubTileGrid fills cells.
            let pos = find_attrib(&entry.children, "RelativePos")
                .and_then(|a| f32x2(&a.content))
                .or_else(|| {
                    if grid_cells.is_empty() {
                        None
                    } else {
                        let n = grid_cells.len() as f32;
                        let cx = grid_cells.iter().map(|c| c.x as f32).sum::<f32>() / n + 0.5;
                        let cy = grid_cells.iter().map(|c| c.y as f32).sum::<f32>() / n + 0.5;
                        Some((cx, cy))
                    }
                });
            if !grid_cells.is_empty() {
                cells.extend(grid_cells);
            }
            let Some(pos) = pos else {
                continue;
            };
            modules.push(Building {
                guid,
                x: pos.0,
                y: pos.1,
                direction: 0,
                complex_owner_id: owner,
                variation: None,
            });
        }
    }
    (modules, cells, area)
}

/// Anno appends a 12-byte trailer after the zlib stream:
/// `78 da 03 00 | 00 00 00 01 | uncompressed_size_u32_le`.
/// Without it the game loads the stamp briefly, then drops it from the bar.
const PIPETTE_ZLIB_FOOTER_PREFIX: [u8; 8] = [0x78, 0xda, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];

fn append_pipette_zlib_footer(compressed: &mut Vec<u8>, uncompressed_len: u32) {
    compressed.extend_from_slice(&PIPETTE_ZLIB_FOOTER_PREFIX);
    compressed.extend_from_slice(&uncompressed_len.to_le_bytes());
}

pub fn decode_pipette_b64(data: &str) -> Result<Stamp, StampError> {
    let compressed = B64.decode(data).map_err(|e| StampError::Zlib(e.to_string()))?;
    // Trailing Anno footer is ignored by ZlibDecoder (unused_data).
    let mut dec = ZlibDecoder::new(std::io::Cursor::new(compressed));
    let mut raw = Vec::new();
    dec.read_to_end(&mut raw)
        .map_err(|e| StampError::Zlib(e.to_string()))?;
    decode_pipette_bytes(&raw)
}

pub fn decode_pipette_bytes(raw: &[u8]) -> Result<Stamp, StampError> {
    let document = load(raw)?;
    let stamp_version = document
        .roots
        .iter()
        .find_map(|n| match n {
            Node::Tag(t) if t.name == "PipetteData" => find_attrib(&t.children, "StampVersion"),
            _ => None,
        })
        .map(|a| i32_le(&a.content))
        .unwrap_or(1);
    let buildings = extract_buildings(&document);
    let (polygon_modules, polygon_cells, polygon_area) = extract_polygon_modules(&document);
    let polygon_cells = filter_field_cells_off_roads(&document, polygon_cells);
    let mut stamp = Stamp {
        document,
        buildings,
        polygon_modules,
        polygon_cells,
        polygon_area,
        stamp_version,
        cistern_fixes_on_load: 0,
    };
    // Legacy editor wrote cistern Pos as geometric center (TL+1.5) → +1 tile in-game.
    stamp.migrate_cistern_pivots(CisternMigrateMode::Auto);
    Ok(stamp)
}

fn write_f32x2_attrib(attrib: &mut Attrib, x: f32, y: f32) {
    let mut bytes = vec![0u8; 8];
    bytes[0..4].copy_from_slice(&x.to_le_bytes());
    bytes[4..8].copy_from_slice(&y.to_le_bytes());
    attrib.content = bytes;
}

fn entry_has_cistern_pivot_mark(entry: &Tag) -> bool {
    find_attrib(&entry.children, CISTERN_PIVOT_ATTRIB).is_some()
}

/// Register (or reuse) a FileDB attrib id for `name`. Ids must have bit 0x8000 set;
/// id 0 is the end-of-tag sentinel and must never be written as an attrib.
fn ensure_attrib_id(doc: &mut DocumentOrdered, name: &str) -> u16 {
    if let Some((id, _)) = doc.attribs.items.iter().find(|(_, n)| n == name) {
        return *id;
    }
    let mut used: HashSet<u16> = doc.attribs.items.iter().map(|(id, _)| *id).collect();
    used.insert(NONE_ATTRIB_ID);
    let mut id = 0x8001u16;
    loop {
        if !used.contains(&id) {
            doc.attribs.items.push((id, name.to_string()));
            return id;
        }
        id = id.wrapping_add(1);
        if id < 0x8000 {
            id = 0x8001;
        }
    }
}

fn ensure_cistern_pivot_mark(entry: &mut Tag, attrib_id: u16) {
    if entry_has_cistern_pivot_mark(entry) {
        return;
    }
    entry.children.push(Node::Attrib(Attrib {
        id: attrib_id,
        name: CISTERN_PIVOT_ATTRIB.into(),
        content: 1u32.to_le_bytes().to_vec(),
    }));
}

fn strip_cistern_pivot_mark(entry: &mut Tag) {
    entry
        .children
        .retain(|n| !matches!(n, Node::Attrib(a) if a.name == CISTERN_PIVOT_ATTRIB));
}

fn footprint_tl_from_pos(x: f32, y: f32, mx: f32, my: f32, drag: (f32, f32)) -> (i32, i32) {
    (
        relative_to_tile(x + mx, drag.0),
        relative_to_tile(y + my, drag.1),
    )
}

fn pos_from_tl(tx: i32, ty: i32, mx: f32, my: f32) -> (f32, f32) {
    (tx as f32 - mx, ty as f32 - my)
}

fn footprint_touches_road(tl: (i32, i32), w: i32, h: i32, roads: &HashSet<(i32, i32)>) -> bool {
    for dy in 0..h {
        for dx in 0..w {
            let x = tl.0 + dx;
            let y = tl.1 + dy;
            for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
                if roads.contains(&(nx, ny)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Half-tile bits can touch the road frame; never paint fields on streets.
fn filter_field_cells_off_roads(
    document: &DocumentOrdered,
    mut cells: Vec<PolygonCell>,
) -> Vec<PolygonCell> {
    let roads: std::collections::HashSet<(i32, i32)> =
        crate::meshgraph::mesh_road_cells(document)
            .into_iter()
            .map(|(x, y, _)| (x, y))
            .collect();
    if !roads.is_empty() {
        cells.retain(|c| !roads.contains(&(c.x, c.y)));
    }
    cells
}

fn write_i32_le(a: &mut Attrib, v: i32) {
    let mut bytes = vec![0u8; a.content.len().max(4)];
    bytes[0..4].copy_from_slice(&v.to_le_bytes());
    a.content = bytes;
}

fn write_i32_pair_attrib(a: &mut Attrib, x: i32, y: i32) {
    let mut bytes = vec![0u8; a.content.len().max(8)];
    bytes[0..4].copy_from_slice(&x.to_le_bytes());
    bytes[4..8].copy_from_slice(&y.to_le_bytes());
    a.content = bytes;
}

fn clear_all_subtile_bits(entry: &mut Tag) {
    let Some(stg) = find_tag_mut(&mut entry.children, "SubTileGrid") else {
        return;
    };
    let Some(grid_wrap) = find_tag_mut(&mut stg.children, "Grid") else {
        return;
    };
    let Some(grid) = find_tag_mut(&mut grid_wrap.children, "grid") else {
        return;
    };
    if let Some(bits) = find_attrib_mut(&mut grid.children, "bits") {
        for b in bits.content.iter_mut() {
            *b = 0;
        }
    }
}

fn ensure_owner_attrib(entry: &mut Tag, owner: u64) {
    if let Some(a) = find_attrib_mut(&mut entry.children, "ComplexOwnerID") {
        let mut bytes = vec![0u8; a.content.len().max(8)];
        bytes[0..8].copy_from_slice(&owner.to_le_bytes());
        a.content = bytes;
    } else {
        entry.children.push(Node::Attrib(Attrib {
            id: 0,
            name: "ComplexOwnerID".into(),
            content: owner.to_le_bytes().to_vec(),
        }));
    }
}

fn ensure_tag_id(doc: &mut DocumentOrdered, name: &str) -> u16 {
    if name == "None" {
        return NONE_TAG_ID;
    }
    if let Some((id, _)) = doc.tags.items.iter().find(|(_, n)| n == name) {
        return *id;
    }
    let mut used: HashSet<u16> = doc.tags.items.iter().map(|(id, _)| *id).collect();
    used.insert(NONE_TAG_ID);
    used.insert(0);
    let mut id = 2u16;
    loop {
        if !used.contains(&id) && (id & 0x8000) == 0 {
            doc.tags.items.push((id, name.to_string()));
            return id;
        }
        id = id.wrapping_add(1);
        if id == 0 || id == NONE_TAG_ID {
            id = 2;
        }
    }
}

fn write_guid_attrib(entry: &mut Tag, field_guid: u32) {
    if let Some(g) = find_attrib_mut(&mut entry.children, "GUID") {
        let mut bytes = g.content.clone();
        if bytes.len() < 4 {
            bytes.resize(4, 0);
        }
        bytes[0..4].copy_from_slice(&(field_guid as i32).to_le_bytes());
        g.content = bytes;
    }
}

fn bind_field_polygon_seed(doc: &DocumentOrdered, entry: &mut Tag, seed_abs: (i32, i32)) {
    if let Some(stg) = find_tag_mut(&mut entry.children, "SubTileGrid") {
        if let Some(origin) = find_attrib_mut(&mut stg.children, "GridOriginWS") {
            write_i32_pair_attrib(origin, seed_abs.0, seed_abs.1);
        }
    }
    let drag = drag_center(doc);
    let rx = seed_abs.0 as f32 - drag.0.floor();
    let ry = seed_abs.1 as f32 - drag.1.floor();
    if let Some(rel) = find_attrib_mut(&mut entry.children, "RelativePos") {
        write_f32x2_attrib(rel, rx, ry);
    }
    if let Some(cr) = find_tag_mut(&mut entry.children, "CreateRect") {
        if let Some(rect) = find_tag_mut(&mut cr.children, "Rect") {
            if let Some(value) = find_attrib_mut(&mut rect.children, "value") {
                let mut bytes = vec![0u8; 16];
                bytes[0..4].copy_from_slice(&seed_abs.0.to_le_bytes());
                bytes[4..8].copy_from_slice(&seed_abs.1.to_le_bytes());
                bytes[8..12].copy_from_slice(&seed_abs.0.to_le_bytes());
                bytes[12..16].copy_from_slice(&seed_abs.1.to_le_bytes());
                value.content = bytes;
            }
        }
    }
}

/// Minimal empty SubTileGrid field polygon (1 tile extent, all bits clear).
fn make_empty_field_polygon(
    doc: &mut DocumentOrdered,
    owner: u64,
    field_guid: u32,
    seed_abs: (i32, i32),
) -> Tag {
    let none_id = NONE_TAG_ID;
    let stg_id = ensure_tag_id(doc, "SubTileGrid");
    let grid_wrap_id = ensure_tag_id(doc, "Grid");
    let grid_id = ensure_tag_id(doc, "grid");
    let create_id = ensure_tag_id(doc, "CreateRect");
    let rect_id = ensure_tag_id(doc, "Rect");

    let guid_id = ensure_attrib_id(doc, "GUID");
    let origin_id = ensure_attrib_id(doc, "GridOriginWS");
    let x_id = ensure_attrib_id(doc, "x");
    let y_id = ensure_attrib_id(doc, "y");
    let bits_id = ensure_attrib_id(doc, "bits");
    let rel_id = ensure_attrib_id(doc, "RelativePos");
    let owner_id = ensure_attrib_id(doc, "ComplexOwnerID");
    let index_id = ensure_attrib_id(doc, "index");
    let value_id = ensure_attrib_id(doc, "value");

    let gx = 4i32; // one tile = 4 quarter-bits in X
    let gy = 1i32;
    let stride = subtile_stride(gx as usize);
    let bits = vec![0u8; (stride * gy as usize + 7) / 8];
    let drag = drag_center(doc);
    let rx = seed_abs.0 as f32 - drag.0.floor();
    let ry = seed_abs.1 as f32 - drag.1.floor();

    Tag {
        id: none_id,
        name: "None".into(),
        children: vec![
            Node::Attrib(Attrib {
                id: guid_id,
                name: "GUID".into(),
                content: (field_guid as i32).to_le_bytes().to_vec(),
            }),
            Node::Tag(Tag {
                id: stg_id,
                name: "SubTileGrid".into(),
                children: vec![
                    Node::Attrib(Attrib {
                        id: origin_id,
                        name: "GridOriginWS".into(),
                        content: {
                            let mut b = vec![0u8; 8];
                            b[0..4].copy_from_slice(&seed_abs.0.to_le_bytes());
                            b[4..8].copy_from_slice(&seed_abs.1.to_le_bytes());
                            b
                        },
                    }),
                    Node::Tag(Tag {
                        id: grid_wrap_id,
                        name: "Grid".into(),
                        children: vec![Node::Tag(Tag {
                            id: grid_id,
                            name: "grid".into(),
                            children: vec![
                                Node::Attrib(Attrib {
                                    id: x_id,
                                    name: "x".into(),
                                    content: gx.to_le_bytes().to_vec(),
                                }),
                                Node::Attrib(Attrib {
                                    id: y_id,
                                    name: "y".into(),
                                    content: gy.to_le_bytes().to_vec(),
                                }),
                                Node::Attrib(Attrib {
                                    id: bits_id,
                                    name: "bits".into(),
                                    content: bits,
                                }),
                            ],
                        })],
                    }),
                ],
            }),
            Node::Attrib(Attrib {
                id: rel_id,
                name: "RelativePos".into(),
                content: {
                    let mut b = vec![0u8; 8];
                    b[0..4].copy_from_slice(&rx.to_le_bytes());
                    b[4..8].copy_from_slice(&ry.to_le_bytes());
                    b
                },
            }),
            Node::Attrib(Attrib {
                id: owner_id,
                name: "ComplexOwnerID".into(),
                content: owner.to_le_bytes().to_vec(),
            }),
            Node::Tag(Tag {
                id: create_id,
                name: "CreateRect".into(),
                children: vec![Node::Tag(Tag {
                    id: rect_id,
                    name: "Rect".into(),
                    children: vec![
                        Node::Attrib(Attrib {
                            id: index_id,
                            name: "index".into(),
                            content: 0i64.to_le_bytes().to_vec(),
                        }),
                        Node::Attrib(Attrib {
                            id: value_id,
                            name: "value".into(),
                            content: {
                                let mut b = vec![0u8; 16];
                                b[0..4].copy_from_slice(&seed_abs.0.to_le_bytes());
                                b[4..8].copy_from_slice(&seed_abs.1.to_le_bytes());
                                b[8..12].copy_from_slice(&seed_abs.0.to_le_bytes());
                                b[12..16].copy_from_slice(&seed_abs.1.to_le_bytes());
                                b
                            },
                        }),
                    ],
                })],
            }),
        ],
    }
}

/// Ensure an owned SubTileGrid polygon exists (clone template or synthesize).
fn ensure_field_polygon(
    doc: &mut DocumentOrdered,
    owner: u64,
    field_guid: u32,
    seed_abs: (i32, i32),
) -> bool {
    // Already have an owned SubTileGrid polygon?
    for root in &doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(polys) = find_tag(&pipette.children, "Polygons") else {
            continue;
        };
        for child in &polys.children {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let oid = find_attrib(&entry.children, "ComplexOwnerID")
                .and_then(|a| owner_id_from_attrib(&a.content));
            if oid != Some(owner) {
                continue;
            }
            if find_tag(&entry.children, "SubTileGrid").is_some() {
                return true;
            }
        }
    }

    // Prefer same field GUID; otherwise any SubTileGrid (rewrite GUID).
    let template = {
        let mut same_guid = None;
        let mut any_grid = None;
        for root in &doc.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(polys) = find_tag(&pipette.children, "Polygons") else {
                continue;
            };
            for child in &polys.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if find_tag(&entry.children, "SubTileGrid").is_none() {
                    continue;
                }
                let g = find_attrib(&entry.children, "GUID").map(|a| i32_le(&a.content) as u32);
                if g == Some(field_guid) {
                    same_guid = Some(entry.clone());
                    break;
                }
                if any_grid.is_none() {
                    any_grid = Some(entry.clone());
                }
            }
            if same_guid.is_some() {
                break;
            }
        }
        same_guid.or(any_grid)
    };

    let entry = if let Some(mut entry) = template {
        write_guid_attrib(&mut entry, field_guid);
        ensure_owner_attrib(&mut entry, owner);
        clear_all_subtile_bits(&mut entry);
        // Shrink to a 1×1 empty grid so paint_tile_on_grid can grow from seed.
        if let Some(stg) = find_tag_mut(&mut entry.children, "SubTileGrid") {
            if let Some(grid_wrap) = find_tag_mut(&mut stg.children, "Grid") {
                if let Some(grid) = find_tag_mut(&mut grid_wrap.children, "grid") {
                    let gx = 4i32;
                    let gy = 1i32;
                    let stride = subtile_stride(gx as usize);
                    if let Some(a) = find_attrib_mut(&mut grid.children, "x") {
                        write_i32_le(a, gx);
                    }
                    if let Some(a) = find_attrib_mut(&mut grid.children, "y") {
                        write_i32_le(a, gy);
                    }
                    if let Some(a) = find_attrib_mut(&mut grid.children, "bits") {
                        a.content = vec![0u8; (stride * gy as usize + 7) / 8];
                    }
                }
            }
        }
        bind_field_polygon_seed(doc, &mut entry, seed_abs);
        entry
    } else {
        make_empty_field_polygon(doc, owner, field_guid, seed_abs)
    };

    for root in &mut doc.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(polys) = find_tag_mut(&mut pipette.children, "Polygons") else {
            continue;
        };
        polys.children.push(Node::Tag(entry));
        return true;
    }
    false
}

fn paint_tile_on_grid(
    entry: &mut Tag,
    drag: (f32, f32),
    tx: i32,
    ty: i32,
    paint_on: bool,
) -> bool {
    let (ox0, oy0, gx0, gy0, bits0) = {
        let Some(stg) = find_tag(&entry.children, "SubTileGrid") else {
            return false;
        };
        let Some(origin_a) = find_attrib(&stg.children, "GridOriginWS") else {
            return false;
        };
        if origin_a.content.len() < 8 {
            return false;
        }
        let ox = i32_le(&origin_a.content[0..4]);
        let oy = i32_le(&origin_a.content[4..8]);
        let Some(grid_wrap) = find_tag(&stg.children, "Grid") else {
            return false;
        };
        let Some(grid) = find_tag(&grid_wrap.children, "grid") else {
            return false;
        };
        let gx = find_attrib(&grid.children, "x")
            .map(|a| i32_le(&a.content).max(0) as usize)
            .unwrap_or(0);
        let gy = find_attrib(&grid.children, "y")
            .map(|a| i32_le(&a.content).max(0) as usize)
            .unwrap_or(0);
        let Some(bits_a) = find_attrib(&grid.children, "bits") else {
            return false;
        };
        if gx == 0 || gy == 0 {
            return false;
        }
        (ox, oy, gx, gy, bits_a.content.clone())
    };

    let ax = tx + drag.0.floor() as i32;
    let ay = ty + drag.1.floor() as i32;
    let tile_cols = (gx0 as i32) / 4;
    let cur_max_x = ox0 + tile_cols - 1;
    let cur_max_y = oy0 + gy0 as i32 - 1;
    let new_ox = ox0.min(ax);
    let new_oy = oy0.min(ay);
    let new_max_x = cur_max_x.max(ax);
    let new_max_y = cur_max_y.max(ay);
    let new_gx = ((new_max_x - new_ox + 1) * 4).max(4) as usize;
    let new_gy = (new_max_y - new_oy + 1).max(1) as usize;

    let mut bits = if new_ox != ox0 || new_oy != oy0 || new_gx != gx0 || new_gy != gy0 {
        remap_subtile_bits(&bits0, ox0, oy0, gx0, gy0, new_ox, new_oy, new_gx, new_gy)
    } else {
        bits0
    };
    let ox = new_ox;
    let oy = new_oy;
    let gx = new_gx;
    let gy = new_gy;

    let sx0 = (ax - ox) * 4;
    let sy = ay - oy;
    if sx0 < 0 || sy < 0 {
        return false;
    }
    let sx0 = sx0 as usize;
    let sy = sy as usize;
    if sx0 + 4 > gx || sy >= gy {
        return false;
    }
    let stride = subtile_stride(gx);
    let mut changed = false;
    for dx in 0..4 {
        let idx = sy * stride + sx0 + dx;
        let was = bit_set(&bits, idx);
        if was != paint_on {
            bit_set_mut(&mut bits, idx, paint_on);
            changed = true;
        }
    }
    if !changed && ox == ox0 && oy == oy0 && gx == gx0 && gy == gy0 {
        return false;
    }

    let Some(stg) = find_tag_mut(&mut entry.children, "SubTileGrid") else {
        return false;
    };
    if let Some(origin_a) = find_attrib_mut(&mut stg.children, "GridOriginWS") {
        write_i32_pair_attrib(origin_a, ox, oy);
    }
    let Some(grid_wrap) = find_tag_mut(&mut stg.children, "Grid") else {
        return false;
    };
    let Some(grid) = find_tag_mut(&mut grid_wrap.children, "grid") else {
        return false;
    };
    if let Some(a) = find_attrib_mut(&mut grid.children, "x") {
        write_i32_le(a, gx as i32);
    }
    if let Some(a) = find_attrib_mut(&mut grid.children, "y") {
        write_i32_le(a, gy as i32);
    }
    if let Some(a) = find_attrib_mut(&mut grid.children, "bits") {
        a.content = bits;
    }
    true
}

fn ortho4(x: i32, y: i32) -> [(i32, i32); 4] {
    [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
}

/// Field tiles claimed by `owner`.
pub fn field_cells_of_owner(stamp: &Stamp, owner: u64) -> std::collections::HashSet<(i32, i32)> {
    stamp
        .polygon_cells
        .iter()
        .filter(|c| c.complex_owner_id == Some(owner))
        .map(|c| (c.x, c.y))
        .collect()
}

/// Field tiles claimed by any *other* farm (must not overlap).
pub fn foreign_field_cells(stamp: &Stamp, owner: u64) -> std::collections::HashSet<(i32, i32)> {
    stamp
        .polygon_cells
        .iter()
        .filter(|c| {
            c.complex_owner_id
                .filter(|o| *o != 0 && *o != owner)
                .is_some()
        })
        .map(|c| (c.x, c.y))
        .collect()
}

/// Among `candidates`, keep those 4-connected to `seeds` through `seeds ∪ candidates`.
pub fn connected_field_subset(
    seeds: &std::collections::HashSet<(i32, i32)>,
    candidates: &std::collections::HashSet<(i32, i32)>,
) -> std::collections::HashSet<(i32, i32)> {
    use std::collections::VecDeque;
    let mut connected = seeds.clone();
    let mut q: VecDeque<(i32, i32)> = connected.iter().copied().collect();
    while let Some((x, y)) = q.pop_front() {
        for n in ortho4(x, y) {
            if candidates.contains(&n) && connected.insert(n) {
                q.push_back(n);
            }
        }
    }
    connected
        .into_iter()
        .filter(|c| candidates.contains(c))
        .collect()
}

fn paint_cells_on_owner_polygon(
    stamp: &mut Stamp,
    owner: u64,
    field_guid: u32,
    cells: &[(i32, i32)],
    paint_on: bool,
    blocked: &std::collections::HashSet<(i32, i32)>,
) -> usize {
    if cells.is_empty() {
        return 0;
    }
    let drag = drag_center(&stamp.document);
    if paint_on {
        let Some(seed) = cells.iter().copied().find(|c| !blocked.contains(c)) else {
            return 0;
        };
        let seed_abs = (
            seed.0 + drag.0.floor() as i32,
            seed.1 + drag.1.floor() as i32,
        );
        if !ensure_field_polygon(&mut stamp.document, owner, field_guid, seed_abs) {
            return 0;
        }
    }

    fn apply_on_entry(
        entry: &mut Tag,
        drag: (f32, f32),
        cells: &[(i32, i32)],
        paint_on: bool,
        blocked: &std::collections::HashSet<(i32, i32)>,
    ) -> usize {
        let mut local = 0usize;
        for &(tx, ty) in cells {
            if paint_on && blocked.contains(&(tx, ty)) {
                continue;
            }
            if paint_tile_on_grid(entry, drag, tx, ty, paint_on) {
                local += 1;
            }
        }
        local
    }

    for root in &mut stamp.document.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(polys) = find_tag_mut(&mut pipette.children, "Polygons") else {
            continue;
        };
        for child in &mut polys.children {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let oid = find_attrib(&entry.children, "ComplexOwnerID")
                .and_then(|a| owner_id_from_attrib(&a.content));
            if oid != Some(owner) {
                continue;
            }
            if find_tag(&entry.children, "SubTileGrid").is_none() {
                continue;
            }
            let g = find_attrib(&entry.children, "GUID").map(|a| i32_le(&a.content) as u32);
            if g.is_some() && g != Some(field_guid) {
                continue;
            }
            return apply_on_entry(entry, drag, cells, paint_on, blocked);
        }
        for child in &mut polys.children {
            let Node::Tag(entry) = child else { continue };
            if entry.name != "None" {
                continue;
            }
            let oid = find_attrib(&entry.children, "ComplexOwnerID")
                .and_then(|a| owner_id_from_attrib(&a.content));
            if oid != Some(owner) {
                continue;
            }
            if find_tag(&entry.children, "SubTileGrid").is_none() {
                continue;
            }
            return apply_on_entry(entry, drag, cells, paint_on, blocked);
        }
    }
    0
}

/// Owner field tiles that are *not* 4-connected to `main_seeds` (farm footprint).
pub fn disconnected_field_cells(
    stamp: &Stamp,
    owner: u64,
    main_seeds: &std::collections::HashSet<(i32, i32)>,
) -> std::collections::HashSet<(i32, i32)> {
    let remaining = field_cells_of_owner(stamp, owner);
    let keep = connected_field_subset(main_seeds, &remaining);
    remaining.into_iter().filter(|c| !keep.contains(c)).collect()
}

/// Paint or erase PlantFarm field tiles into the owner's SubTileGrid.
///
/// When painting:
/// - skips `blocked` (roads/buildings) and tiles already owned by another farm
/// - if `connect_seeds` is set (farm footprint ± existing fields), only paints tiles
///   that stay 4-connected to those seeds (Anno: fields must touch the main)
///
/// Disconnected leftovers after erase are kept and shown red in the UI
/// (`disconnected_field_cells`) — they are not auto-deleted.
pub fn apply_field_tiles(
    stamp: &mut Stamp,
    owner: u64,
    field_guid: u32,
    cells: &[(i32, i32)],
    paint_on: bool,
    blocked: &std::collections::HashSet<(i32, i32)>,
    connect_seeds: Option<&std::collections::HashSet<(i32, i32)>>,
) -> usize {
    if owner == 0 || cells.is_empty() {
        return 0;
    }

    let mut effective_blocked = blocked.clone();
    if paint_on {
        effective_blocked.extend(foreign_field_cells(stamp, owner));
    }

    let cells: Vec<(i32, i32)> = if paint_on {
        if let Some(seeds) = connect_seeds {
            let mut seed_set = seeds.clone();
            seed_set.extend(field_cells_of_owner(stamp, owner));
            let cand: std::collections::HashSet<(i32, i32)> = cells
                .iter()
                .copied()
                .filter(|c| !effective_blocked.contains(c))
                .collect();
            let ok = connected_field_subset(&seed_set, &cand);
            cells
                .iter()
                .copied()
                .filter(|c| ok.contains(c))
                .collect()
        } else {
            cells.to_vec()
        }
    } else {
        cells.to_vec()
    };

    paint_cells_on_owner_polygon(
        stamp,
        owner,
        field_guid,
        &cells,
        paint_on,
        &effective_blocked,
    )
}

pub fn encode_pipette_b64(stamp: &Stamp) -> Result<String, StampError> {
    let raw = dump(&stamp.document);
    // Anno 1800/117 stamps use zlib level 8 plus a 12-byte size trailer.
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(8));
    enc.write_all(&raw)
        .map_err(|e| StampError::Zlib(e.to_string()))?;
    let mut compressed = enc.finish().map_err(|e| StampError::Zlib(e.to_string()))?;
    append_pipette_zlib_footer(&mut compressed, raw.len() as u32);
    Ok(B64.encode(compressed))
}

/// Minimal valid pipette used as document skeleton / building-entry template.
/// Must be a **modern** in-game stamp that includes `Variation` on buildings —
/// older seeds (e.g. Woodcutter) omit it and the game rejects the stamp after load.
const BLANK_SEED_PIPETTE_B64: &str = "eNq9VE1PwkAQHaGlUEjUn+G/4KOGYNJIVLiZsJG1bFJa0i6KF7M/xTsH4t1E/UmGGA7G4GxbbC1EizFOMoE3+97M5HVbAIAcpoK5IzAhjnz0K2tFyRMBfALoVSHSyFATvKCGvAJ8H5KXR95uJcaKiHvJearYrF3fBx5/2kfJuM+BufX8ahY/cv/nRzWLH4U/9GMVq55axC+k5mipPTQ8fziP+xcRv+oAUz3EpQg/65vnbepXNr/2e0HtLNFP4nt93b+s+nmEs+oXKf3blvp5Sr/YoE/78Vu8eu46ztmP8B5mGfEEL1EH8xaTqPGOySgl/suzirwv7ZkBvamR5OkZeaueCr49Gn6NVLxNRazoYDAP2mxEOacG4QQO+xb1oT5mdp85Vsu5dOGK2GMKJvUHTY+MBgHNoDbl9IRecGhZjuvRUNd27RvLdXw5rxJ8B3MiLzShCFUURFGUhC7KoiJOORmOutTzmeugyIdmp2WANWZ96BKPES7rDXc4sunk+NqhHp5+LtBwxw6XKpMFYpNM4IzZNKzXavU6GB6x5HIN6nDqxR4cYd7hg9lTQ7/el8vlB4JShyp42gMAAAAAAbgFAAA=";

fn seed_building_template_tag() -> Result<Tag, StampError> {
    let seed = decode_pipette_b64(BLANK_SEED_PIPETTE_B64)?;
    for root in &seed.document.roots {
        let Node::Tag(pipette) = root else { continue };
        if pipette.name != "PipetteData" {
            continue;
        }
        let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
            continue;
        };
        for child in &info.children {
            if let Node::Tag(entry) = child {
                if entry.name == "None" {
                    return Ok(entry.clone());
                }
            }
        }
    }
    Err(StampError::MissingField)
}

impl Stamp {
    /// Empty editable stamp (valid pipette shell, zero buildings).
    pub fn blank() -> Result<Self, StampError> {
        let mut stamp = decode_pipette_b64(BLANK_SEED_PIPETTE_B64)?;
        for root in &mut stamp.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            if let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") {
                info.children.retain(|n| !matches!(n, Node::Tag(t) if t.name == "None"));
            }
            if let Some(polygons) = find_tag_mut(&mut pipette.children, "Polygons") {
                polygons.children.retain(|n| !matches!(n, Node::Tag(t) if t.name == "None"));
            }
        }
        stamp.buildings.clear();
        stamp.polygon_modules.clear();
        stamp.polygon_cells.clear();
        stamp.polygon_area.clear();
        stamp.cistern_fixes_on_load = 0;
        // Seed pipette still carries MeshGraph streets + aqueduct edges — wipe both.
        // `clear_roads` alone leaves the aqueduct list (GUID 19723 …) → cyan L on boot.
        let _ = clear_all_mesh_paths(&mut stamp.document, &stamp.buildings);
        recompute_bounds(&mut stamp.document, &stamp.buildings);
        // Keep Variation in the attrib dictionary even with zero buildings.
        let _ = stamp.document.ensure_attrib_id("Variation");
        Ok(stamp)
    }

    /// Ensure every BuildingInfo entry has a `Variation` attrib (game requirement).
    pub fn ensure_building_variations(&mut self) {
        let var_id = self.document.ensure_attrib_id("Variation");
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                let current = find_attrib(&entry.children, "Variation")
                    .map(|a| u32_le(&a.content))
                    .unwrap_or(0);
                apply_variation_attrib(entry, var_id, current);
            }
        }
    }

    pub fn refresh_buildings(&mut self) {
        self.buildings = extract_buildings(&self.document);
        let (mods, cells, area) = extract_polygon_modules(&self.document);
        self.polygon_modules = mods;
        self.polygon_cells = filter_field_cells_off_roads(&self.document, cells);
        self.polygon_area = area;
    }

    /// Rewrite legacy centered aqueduct-cistern `Pos` to the decentered pivot.
    ///
    /// Old editor: `Pos = TL + 1.5` → game footprint at TL+1 (one grass gap).
    /// New: `Pos = TL + 0.5`. Returns how many buildings were moved.
    ///
    /// `Auto` uses roads when present (flush under centered math but not under
    /// decentered → migrate). Already-marked entries are left alone (idempotent
    /// after save). `Force` re-anchors every unmarked cistern.
    pub fn migrate_cistern_pivots(&mut self, mode: CisternMigrateMode) -> usize {
        let drag = read_mesh_info(&self.document)
            .map(|i| i.drag_center)
            .unwrap_or((0.0, 0.0));
        let roads: HashSet<(i32, i32)> = crate::meshgraph::mesh_road_cells(&self.document)
            .into_iter()
            .map(|(x, y, _)| (x, y))
            .collect();
        let has_roads = !roads.is_empty();
        let (w0, h0) = (AQUEDUCT_CISTERN_SIZE[0], AQUEDUCT_CISTERN_SIZE[1]);

        // Only register the FileDB attrib when unmarked cisterns exist — otherwise
        // every stamp dump would grow an unused dictionary entry on batch fix.
        let has_unmarked_cistern = {
            let mut found = false;
            'scan: for root in &self.document.roots {
                let Node::Tag(pipette) = root else { continue };
                if pipette.name != "PipetteData" {
                    continue;
                }
                let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
                    continue;
                };
                for child in &info.children {
                    let Node::Tag(entry) = child else { continue };
                    let guid = find_attrib(&entry.children, "GUID")
                        .map(|a| u32_le(&a.content))
                        .unwrap_or(0);
                    if is_aqueduct_cistern(guid) && !entry_has_cistern_pivot_mark(entry) {
                        found = true;
                        break 'scan;
                    }
                }
            }
            found
        };
        let mark_id = if has_unmarked_cistern {
            Some(ensure_attrib_id(&mut self.document, CISTERN_PIVOT_ATTRIB))
        } else {
            None
        };

        let mut fixed = 0usize;
        let mut idx = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                let guid = find_attrib(&entry.children, "GUID")
                    .map(|a| u32_le(&a.content))
                    .unwrap_or(0);
                if !is_aqueduct_cistern(guid) {
                    idx += 1;
                    continue;
                }
                if entry_has_cistern_pivot_mark(entry) {
                    idx += 1;
                    continue;
                }

                let direction = read_direction(entry);
                let pos = find_attrib(&entry.children, "Pos")
                    .and_then(|a| f32x2(&a.content))
                    .unwrap_or((0.0, 0.0));

                let (mx_c0, my_c0) = (-w0 * 0.5, -h0 * 0.5);
                let (mx_d0, my_d0) = blocker_min_for(guid, w0, h0, None, None);
                let (mx_c, my_c, _, _) = oriented_blocker(mx_c0, my_c0, w0, h0, direction);
                let (mx_d, my_d, ow, oh) = oriented_blocker(mx_d0, my_d0, w0, h0, direction);
                let fw = ow.round().max(1.0) as i32;
                let fh = oh.round().max(1.0) as i32;
                let tl_centered = footprint_tl_from_pos(pos.0, pos.1, mx_c, my_c, drag);
                let tl_decentered = footprint_tl_from_pos(pos.0, pos.1, mx_d, my_d, drag);
                let new_pos = pos_from_tl(tl_centered.0, tl_centered.1, mx_d, my_d);

                let flush_c = has_roads && footprint_touches_road(tl_centered, fw, fh, &roads);
                let flush_d = has_roads && footprint_touches_road(tl_decentered, fw, fh, &roads);

                // Unmarked = pre-pivot-fix file (new placements always get the mark).
                // With roads: only move when centered math is flush and decentered is not.
                // Without roads: assume centered Pos (same as Force).
                let should_move = match mode {
                    CisternMigrateMode::Force => true,
                    CisternMigrateMode::Auto => {
                        if !has_roads {
                            true
                        } else {
                            flush_c && !flush_d
                        }
                    }
                };

                if should_move {
                    let dx = (new_pos.0 - pos.0).abs();
                    let dy = (new_pos.1 - pos.1).abs();
                    if dx > 1e-4 || dy > 1e-4 {
                        if let Some(p) = find_attrib_mut(&mut entry.children, "Pos") {
                            write_f32x2_attrib(p, new_pos.0, new_pos.1);
                        }
                        if let Some(b) = self.buildings.get_mut(idx) {
                            b.x = new_pos.0;
                            b.y = new_pos.1;
                        }
                        fixed += 1;
                    }
                    if let Some(id) = mark_id {
                        ensure_cistern_pivot_mark(entry, id);
                    }
                } else if flush_d {
                    // Already correct under decentered math — mark so we never shift it.
                    if let Some(id) = mark_id {
                        ensure_cistern_pivot_mark(entry, id);
                    }
                }
                idx += 1;
            }
        }
        self.cistern_fixes_on_load = fixed;
        if fixed > 0 {
            recompute_bounds(&mut self.document, &self.buildings);
        }
        fixed
    }

    /// BuildingInfo entries plus polygon-stored field modules (for paint / occupancy).
    pub fn all_draw_buildings(&self) -> impl Iterator<Item = &Building> {
        self.buildings.iter().chain(self.polygon_modules.iter())
    }

    pub fn set_building_pos(&mut self, index: usize, x: f32, y: f32) -> Result<(), StampError> {
        self.set_building_pos_ex(index, x, y, false)
    }

    pub fn set_building_direction(&mut self, index: usize, direction: i32) -> Result<(), StampError> {
        let direction = direction.rem_euclid(8);
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    if !write_direction(entry, direction) {
                        return Err(StampError::MissingField);
                    }
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.direction = direction;
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    /// Move building. When `move_linked` is true, also shift modules/polygons with same ComplexOwnerID.
    pub fn set_building_pos_ex(
        &mut self,
        index: usize,
        x: f32,
        y: f32,
        move_linked: bool,
    ) -> Result<(), StampError> {
        let old = self
            .buildings
            .get(index)
            .map(|b| (b.x, b.y))
            .ok_or(StampError::MissingField)?;
        // Drag fires every frame — skip document rewrite when the snap didn't move.
        if (old.0 - x).abs() < 1e-5 && (old.1 - y).abs() < 1e-5 {
            return Ok(());
        }
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    // Create Pos when absent (center-warehouse stamps omit it).
                    let pos = if let Some(p) = find_attrib_mut(&mut entry.children, "Pos") {
                        p
                    } else {
                        entry.children.push(Node::Attrib(Attrib {
                            id: 0,
                            name: "Pos".into(),
                            content: vec![0u8; 8],
                        }));
                        find_attrib_mut(&mut entry.children, "Pos")
                            .ok_or(StampError::MissingField)?
                    };
                    let mut bytes = vec![0u8; 8];
                    bytes[0..4].copy_from_slice(&x.to_le_bytes());
                    bytes[4..8].copy_from_slice(&y.to_le_bytes());
                    pos.content = bytes;
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.x = x;
                        b.y = y;
                    }
                    if move_linked {
                        sync_after_building_move(
                            &mut self.document,
                            &mut self.buildings,
                            index,
                            old.0,
                            old.1,
                            x,
                            y,
                        );
                    } else {
                        recompute_bounds(&mut self.document, &self.buildings);
                    }
                    // Rebuild polygon_modules / SubTileGrid cells from the document.
                    self.refresh_buildings();
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    pub fn remove_building(&mut self, index: usize) -> Result<(), StampError> {
        let owner = self
            .buildings
            .get(index)
            .and_then(|b| b.complex_owner_id)
            .filter(|o| *o != 0);
        let guid = self
            .buildings
            .get(index)
            .map(|b| b.guid)
            .ok_or(StampError::MissingField)?;
        // If this was the last BuildingInfo with that ComplexOwnerID, drop owned field polygons.
        let drop_owned_polygons = owner.is_some_and(|own| {
            self.buildings
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .filter(|(_, b)| b.complex_owner_id == Some(own))
                .count()
                == 0
        });
        let mut count = 0usize;
        let mut removed = false;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            if let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") {
                let mut remove_at = None;
                for (i, child) in info.children.iter().enumerate() {
                    let Node::Tag(entry) = child else { continue };
                    if entry.name != "None" {
                        continue;
                    }
                    if count == index {
                        remove_at = Some(i);
                        break;
                    }
                    count += 1;
                }
                if let Some(i) = remove_at {
                    info.children.remove(i);
                    removed = true;
                }
            }
            // Drop polygons for this GUID, and all owned fields when the complex main is gone.
            if let Some(polygons) = find_tag_mut(&mut pipette.children, "Polygons") {
                polygons.children.retain(|n| {
                    let Node::Tag(entry) = n else {
                        return true;
                    };
                    if entry.name != "None" {
                        return true;
                    }
                    let g = find_attrib(&entry.children, "GUID").map(|a| u32_le(&a.content));
                    let oid = find_attrib(&entry.children, "ComplexOwnerID").and_then(|a| {
                        if a.content.len() >= 8 {
                            Some(u64::from_le_bytes(a.content[0..8].try_into().ok()?))
                        } else if a.content.len() >= 4 {
                            Some(u32_le(&a.content) as u64)
                        } else {
                            None
                        }
                    });
                    if g == Some(guid) {
                        return false;
                    }
                    if drop_owned_polygons {
                        if let (Some(o), Some(own)) = (oid, owner) {
                            if o == own {
                                return false;
                            }
                        }
                    }
                    true
                });
            }
        }
        if !removed {
            return Err(StampError::MissingField);
        }
        self.buildings.remove(index);
        recompute_bounds(&mut self.document, &self.buildings);
        self.refresh_buildings();
        Ok(())
    }

    pub fn set_building_owner(&mut self, index: usize, owner: u64) -> Result<(), StampError> {
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    if let Some(a) = find_attrib_mut(&mut entry.children, "ComplexOwnerID") {
                        let mut bytes = vec![0u8; a.content.len().max(8)];
                        bytes[0..8].copy_from_slice(&owner.to_le_bytes());
                        a.content = bytes;
                    } else {
                        entry.children.push(Node::Attrib(Attrib {
                            id: 0,
                            name: "ComplexOwnerID".into(),
                            content: owner.to_le_bytes().to_vec(),
                        }));
                    }
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.complex_owner_id = Some(owner);
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    /// Clone the first building entry as template and place `guid` at (x,y).
    /// If the stamp is empty, uses an embedded seed building entry.
    /// `variation`: pipette Variation (e.g. warehouse marsh CFG = `Some(1)`).
    pub fn add_building_from_template(
        &mut self,
        guid: u32,
        x: f32,
        y: f32,
        direction: i32,
        variation: Option<u32>,
    ) -> Result<(), StampError> {
        let cistern_mark_id = is_aqueduct_cistern(guid)
            .then(|| ensure_attrib_id(&mut self.document, CISTERN_PIVOT_ATTRIB));
        let var_val = variation.unwrap_or(0);
        let var_id = self.document.ensure_attrib_id("Variation");
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            let template = info.children.iter().find_map(|n| match n {
                Node::Tag(t) if t.name == "None" => Some(t.clone()),
                _ => None,
            });
            let mut entry = match template {
                Some(t) => t,
                None => seed_building_template_tag()?,
            };
            let template_guid = find_attrib(&entry.children, "GUID")
                .map(|a| u32_le(&a.content))
                .unwrap_or(guid);
            // Always write Pos — warehouse templates omit it (game treats as 0,0),
            // but aqueduct cisterns and every other placed building need an explicit Pos.
            if let Some(pos) = find_attrib_mut(&mut entry.children, "Pos") {
                let mut bytes = vec![0u8; 8];
                bytes[0..4].copy_from_slice(&x.to_le_bytes());
                bytes[4..8].copy_from_slice(&y.to_le_bytes());
                pos.content = bytes;
            } else {
                let mut bytes = vec![0u8; 8];
                bytes[0..4].copy_from_slice(&x.to_le_bytes());
                bytes[4..8].copy_from_slice(&y.to_le_bytes());
                entry.children.push(Node::Attrib(Attrib {
                    id: 0,
                    name: "Pos".into(),
                    content: bytes,
                }));
            }
            if let Some(g) = find_attrib_mut(&mut entry.children, "GUID") {
                let mut content = g.content.clone();
                if content.len() < 4 {
                    content.resize(4, 0);
                }
                content[0..4].copy_from_slice(&guid.to_le_bytes());
                g.content = content;
            }
            let _ = write_direction(&mut entry, direction);
            // Cisterns must omit Variation; other modern buildings need it.
            // Marsh / land CFG: Variation>0 kept via apply_variation_attrib.
            apply_variation_attrib(&mut entry, var_id, var_val);
            if let Some(mark_id) = cistern_mark_id {
                ensure_cistern_pivot_mark(&mut entry, mark_id);
            } else {
                strip_cistern_pivot_mark(&mut entry);
            }
            info.children.push(Node::Tag(entry));
            self.buildings.push(Building {
                guid,
                x,
                y,
                direction,
                complex_owner_id: None,
                variation,
            });
            clone_polygon_for_new_building(&mut self.document, template_guid, guid, x, y);
            recompute_bounds(&mut self.document, &self.buildings);
            self.refresh_buildings();
            return Ok(());
        }
        Err(StampError::MissingField)
    }

    pub fn set_building_guid(&mut self, index: usize, guid: u32) -> Result<(), StampError> {
        let mark_id = if is_aqueduct_cistern(guid) {
            Some(ensure_attrib_id(&mut self.document, CISTERN_PIVOT_ATTRIB))
        } else {
            None
        };
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    let Some(g) = find_attrib_mut(&mut entry.children, "GUID") else {
                        return Err(StampError::MissingField);
                    };
                    let mut content = g.content.clone();
                    if content.len() < 4 {
                        content.resize(4, 0);
                    }
                    content[0..4].copy_from_slice(&guid.to_le_bytes());
                    g.content = content;
                    if let Some(id) = mark_id {
                        ensure_cistern_pivot_mark(entry, id);
                    } else {
                        strip_cistern_pivot_mark(entry);
                    }
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.guid = guid;
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }

    /// Set pipette `Variation` (e.g. warehouse Land=`None`/`Some(0)` / Marsh=`Some(1)`).
    /// Never removes the attrib — the game expects it on every building.
    pub fn set_building_variation(
        &mut self,
        index: usize,
        variation: Option<u32>,
    ) -> Result<(), StampError> {
        let var_val = variation.unwrap_or(0);
        let var_id = self.document.ensure_attrib_id("Variation");
        let mut count = 0usize;
        for root in &mut self.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                if count == index {
                    apply_variation_attrib(entry, var_id, var_val);
                    if let Some(b) = self.buildings.get_mut(index) {
                        b.variation = variation.filter(|v| *v > 0);
                    }
                    return Ok(());
                }
                count += 1;
            }
        }
        Err(StampError::MissingField)
    }
}

#[cfg(test)]
mod cistern_roundtrip {
    use super::*;
    use crate::catalog::{blocker_min_for, size_for_guid};
    use crate::coverage::footprint_cells;

    /// Editor TL flush to road at x=0 ⇒ written Pos must be the decentered pivot,
    /// not the geometric center. Neighbor buildings stay centered.
    ///
    /// Numbers (road column x=0, cistern TL x=1, size 3×3):
    /// - OLD centered write: Pos=(2.5, 1.5) → game TL=2 → 1 grass gap
    /// - NEW decentered write: Pos=(1.5, 1.5) → game TL=1 → flush
    #[test]
    fn cistern_flush_to_road_encodes_decentered_pos_not_center() {
        let road_x = 0i32;
        let tl = (road_x + 1, 0i32); // flush
        let (w, h) = size_for_guid(19753, 3.0, 3.0, None, None);
        assert!((w - 3.0).abs() < 1e-5 && (h - 3.0).abs() < 1e-5);

        let (mx, my) = blocker_min_for(19753, w, h, None, None);
        let cistern_pos = (tl.0 as f32 - mx, tl.1 as f32 - my);
        let neighbor_pos = (tl.0 as f32 + 1.5, tl.1 as f32 + 4.5); // centered 3×3 below

        assert!(
            (cistern_pos.0 - 1.5).abs() < 1e-5,
            "cistern Pos.x must be 1.5 (pivot), not centered 2.5; got {}",
            cistern_pos.0
        );
        assert!(
            (neighbor_pos.0 - 2.5).abs() < 1e-5,
            "neighbor stays centered at 2.5"
        );

        let mut stamp = Stamp::blank().expect("blank");
        stamp
            .add_building_from_template(19753, cistern_pos.0, cistern_pos.1, 0, None)
            .expect("cistern");
        stamp
            .add_building_from_template(2878, neighbor_pos.0, neighbor_pos.1, 0, None)
            .expect("neighbor");

        let encoded = encode_pipette_b64(&stamp).expect("encode");
        let back = decode_pipette_b64(&encoded).expect("decode");

        let mut raw_pos: Vec<(u32, f32, f32)> = Vec::new();
        for root in &back.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                let guid = find_attrib(&entry.children, "GUID")
                    .map(|a| u32_le(&a.content))
                    .unwrap_or(0);
                let pos = find_attrib(&entry.children, "Pos")
                    .and_then(|a| f32x2(&a.content))
                    .unwrap_or((0.0, 0.0));
                raw_pos.push((guid, pos.0, pos.1));
            }
        }
        assert_eq!(raw_pos.len(), 2, "raw BuildingInfo entries: {raw_pos:?}");
        let cistern = raw_pos.iter().find(|p| p.0 == 19753).expect("cistern raw");
        let neighbor = raw_pos.iter().find(|p| p.0 == 2878).expect("neighbor raw");
        assert!(
            (cistern.1 - 1.5).abs() < 1e-5 && (cistern.2 - 1.5).abs() < 1e-5,
            "encoded cistern Pos must be (1.5,1.5), got ({},{})",
            cistern.1,
            cistern.2
        );
        assert!(
            (neighbor.1 - 2.5).abs() < 1e-5,
            "encoded neighbor Pos.x must stay 2.5, got {}",
            neighbor.1
        );

        let b = back
            .buildings
            .iter()
            .find(|b| b.guid == 19753)
            .expect("cistern building");
        let min_x = footprint_cells(b, None).iter().map(|c| c.x).min().unwrap();
        assert_eq!(min_x, 1, "editor footprint TL.x must be 1 (flush to road@0)");
    }

    #[test]
    fn cistern_pos_survives_encode_decode() {
        let mut stamp = Stamp::blank().expect("blank");
        stamp
            .add_building_from_template(19753, 1.5, 1.5, 0, None)
            .expect("add");
        let encoded = encode_pipette_b64(&stamp).expect("encode");
        let reloaded = decode_pipette_b64(&encoded).expect("decode");
        let b = reloaded
            .buildings
            .iter()
            .find(|b| b.guid == 19753)
            .expect("cistern");
        assert!((b.x - 1.5).abs() < 1e-5 && (b.y - 1.5).abs() < 1e-5);
        assert_eq!(footprint_cells(b, None).iter().map(|c| c.x).min(), Some(1));
    }

    fn strip_all_cistern_marks(stamp: &mut Stamp) {
        for root in &mut stamp.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag_mut(&mut pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &mut info.children {
                let Node::Tag(entry) = child else { continue };
                strip_cistern_pivot_mark(entry);
            }
        }
    }

    fn raw_cistern_pos(stamp: &Stamp, guid: u32) -> (f32, f32) {
        for root in &stamp.document.roots {
            let Node::Tag(pipette) = root else { continue };
            if pipette.name != "PipetteData" {
                continue;
            }
            let Some(info) = find_tag(&pipette.children, "BuildingInfo") else {
                continue;
            };
            for child in &info.children {
                let Node::Tag(entry) = child else { continue };
                if entry.name != "None" {
                    continue;
                }
                let g = find_attrib(&entry.children, "GUID")
                    .map(|a| u32_le(&a.content))
                    .unwrap_or(0);
                if g != guid {
                    continue;
                }
                return find_attrib(&entry.children, "Pos")
                    .and_then(|a| f32x2(&a.content))
                    .unwrap_or((0.0, 0.0));
            }
        }
        panic!("cistern {guid} missing");
    }

    /// Legacy editor wrote Pos=(2.5,1.5) for TL≈(1,0). Load must re-anchor to
    /// Pos=(1.5,1.5); next encode must persist the corrected bytes.
    #[test]
    fn legacy_centered_cistern_pos_migrates_on_load_and_save() {
        for guid in [19753u32, 29526u32] {
            let mut stamp = Stamp::blank().expect("blank");
            stamp
                .add_building_from_template(guid, 2.5, 1.5, 0, None)
                .expect("add legacy-centered pos");
            // Simulate a pre-fix file: centered Pos, no pivot mark.
            strip_all_cistern_marks(&mut stamp);
            // Road column at x=0 → flush under centered TL=1, gap under decentered TL=2.
            for y in 0..3 {
                assert!(crate::meshgraph::set_road_cell(
                    &mut stamp.document,
                    &stamp.buildings,
                    0,
                    y,
                    true,
                    crate::meshgraph::ROAD_GUID_DIRT,
                ));
            }

            let encoded = encode_pipette_b64(&stamp).expect("encode legacy");
            let loaded = decode_pipette_b64(&encoded).expect("decode migrates");
            assert_eq!(
                loaded.cistern_fixes_on_load, 1,
                "GUID {guid}: expected one pivot fix on load"
            );
            let b = loaded
                .buildings
                .iter()
                .find(|b| b.guid == guid)
                .expect("cistern building");
            assert!(
                (b.x - 1.5).abs() < 1e-5 && (b.y - 1.5).abs() < 1e-5,
                "GUID {guid}: Pos after load must be (1.5,1.5), got ({},{})",
                b.x,
                b.y
            );
            assert_eq!(
                footprint_cells(b, None).iter().map(|c| c.x).min(),
                Some(1),
                "GUID {guid}: footprint TL.x must be 1"
            );

            let saved = encode_pipette_b64(&loaded).expect("save corrected");
            let reloaded = decode_pipette_b64(&saved).expect("reload after save");
            assert_eq!(
                reloaded.cistern_fixes_on_load, 0,
                "GUID {guid}: migrate must be idempotent after save"
            );
            let (px, py) = raw_cistern_pos(&reloaded, guid);
            assert!(
                (px - 1.5).abs() < 1e-5 && (py - 1.5).abs() < 1e-5,
                "GUID {guid}: saved Pos must be (1.5,1.5), got ({px},{py})"
            );
        }
    }

    #[test]
    fn force_migrate_rewrites_unmarked_centered_pos_without_roads() {
        let mut stamp = Stamp::blank().expect("blank");
        stamp
            .add_building_from_template(19753, 2.5, 1.5, 0, None)
            .expect("add");
        strip_all_cistern_marks(&mut stamp);
        let n = stamp.migrate_cistern_pivots(CisternMigrateMode::Force);
        assert_eq!(n, 1);
        let (px, py) = raw_cistern_pos(&stamp, 19753);
        assert!((px - 1.5).abs() < 1e-5 && (py - 1.5).abs() < 1e-5);
    }

    #[test]
    fn blank_stamp_has_no_mesh_roads_or_aqueducts() {
        let stamp = Stamp::blank().expect("blank");
        assert!(stamp.buildings.is_empty());
        let roads = crate::meshgraph::mesh_road_cells(&stamp.document);
        assert!(
            roads.is_empty(),
            "blank seed must wipe aqueduct MeshGraph too, got {roads:?}"
        );
    }
}


