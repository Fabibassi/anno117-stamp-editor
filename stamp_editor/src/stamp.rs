use crate::catalog::{
    blocker_min_for, is_aqueduct_cistern, oriented_blocker, AQUEDUCT_CISTERN_SIZE,
};
use crate::filedb::{dump, load, Attrib, DocumentOrdered, Node, Tag, NONE_ATTRIB_ID};
use crate::meshgraph::{
    clear_roads, clone_polygon_for_new_building, read_mesh_info, recompute_bounds,
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

pub fn decode_pipette_b64(data: &str) -> Result<Stamp, StampError> {
    let compressed = B64.decode(data).map_err(|e| StampError::Zlib(e.to_string()))?;
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

pub fn encode_pipette_b64(stamp: &Stamp) -> Result<String, StampError> {
    let raw = dump(&stamp.document);
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&raw)
        .map_err(|e| StampError::Zlib(e.to_string()))?;
    let compressed = enc.finish().map_err(|e| StampError::Zlib(e.to_string()))?;
    Ok(B64.encode(compressed))
}

/// Minimal valid pipette (from a tiny in-game stamp) used as document skeleton /
/// building-entry template when creating a blank stamp.
const BLANK_SEED_PIPETTE_B64: &str = "eNrFVU1OwlAQHqGlpcVEjmDiATyBKVBCWBCNojsXjTxLk9KStiju3tYlJ4CdS70ASW/h0ht4A4Pz2mKbBkL5k0km7TdvvvdN30zzAABy6Bz6EUWH2PLRk8VElkcZmvgAvg8RhxmfyAtiNF5bZmw9j3kXcojZ/hxdnJvSV7LoF/amz2y1fi6jfrm0rn6ovUof9qYf9F/57/7vqt/fpY3mTTnwvCkHnDcly7xxO9Sf2/xMhSifT52zkKqjgOv3p/H+AuLXE4D3cohFxKMIp793GX+U4o834E9T/Oka/HFK/23L+kdb6i+qP92PXeH53BRR9yzCKA8SYhUL+kRvo3+Jcc1JKybe2ZrM5s3n6vBxXE/mSRnz5nty+LcJeDvyOI0iRiRQDQeujD7xPKJqngb1jk5cqA4Ms2NYetN6tOFJMwcEWsTtNhyt3w3SVGISj1yTBw+aumU7JORd2eaLblsu05ODezlH87RAOcpTgYq0SCUq0xtP6/XviOMatoUUFxq3TRX0gdGBmt3rm2R4+WwRB2N/ojV7YHkst2UElJY2hLZhkjBeqVSroDqazgqqEcsjTvjNrJBzbM5ECM/nZzab/QKoeplWeNoDAAAAAAE4CAAA";

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
        // Seed pipette still carries MeshGraph streets — wipe them for a true blank.
        let _ = clear_roads(&mut stamp.document, &stamp.buildings);
        recompute_bounds(&mut stamp.document, &stamp.buildings);
        Ok(stamp)
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
            // Marsh / land CFG selector (Warehouse Celtic: 0=6×6, 1=5×5).
            if let Some(v) = variation.filter(|v| *v > 0) {
                if let Some(a) = find_attrib_mut(&mut entry.children, "Variation") {
                    let mut bytes = vec![0u8; a.content.len().max(4)];
                    bytes[0..4].copy_from_slice(&v.to_le_bytes());
                    a.content = bytes;
                } else {
                    entry.children.push(Node::Attrib(Attrib {
                        id: 0,
                        name: "Variation".into(),
                        content: v.to_le_bytes().to_vec(),
                    }));
                }
            } else {
                entry
                    .children
                    .retain(|n| !matches!(n, Node::Attrib(a) if a.name == "Variation"));
            }
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

    /// Set pipette `Variation` (e.g. warehouse Land=`None` / Marsh=`Some(1)`).
    pub fn set_building_variation(
        &mut self,
        index: usize,
        variation: Option<u32>,
    ) -> Result<(), StampError> {
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
                    if let Some(v) = variation.filter(|v| *v > 0) {
                        if let Some(a) = find_attrib_mut(&mut entry.children, "Variation") {
                            let mut bytes = vec![0u8; a.content.len().max(4)];
                            bytes[0..4].copy_from_slice(&v.to_le_bytes());
                            a.content = bytes;
                        } else {
                            entry.children.push(Node::Attrib(Attrib {
                                id: 0,
                                name: "Variation".into(),
                                content: v.to_le_bytes().to_vec(),
                            }));
                        }
                    } else {
                        entry
                            .children
                            .retain(|n| !matches!(n, Node::Attrib(a) if a.name == "Variation"));
                    }
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
}


