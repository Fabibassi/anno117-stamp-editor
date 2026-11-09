from pathlib import Path

path = Path(__file__).with_name("src") / "app.rs"
text = path.read_text(encoding="utf-8")
start = text.index("            // Coverage overlay (selection only)")
end = text.index("                if let Some(pointer) = response.interact_pointer_pos()")
new = r'''            // Coverage overlay (selection only) — full tile cells.
            for cell in &self.coverage.covered {
                let r = self.cell_screen_rect(rect, cell.x, cell.y);
                painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(70, 160, 255, 40));
            }

            // Roads — always on; full 1×1 tiles (blueprint / voxel style).
            for (cell, kind) in &self.coverage.roads {
                let r = self.cell_screen_rect(rect, cell.x, cell.y);
                let color = match kind {
                    RoadKind::Dirt => Color32::from_rgb(150, 118, 72),
                    RoadKind::Stone => Color32::from_rgb(130, 130, 140),
                };
                painter.rect_filled(r, 0.0, color);
            }

            if let Some(open) = self.open.clone() {
                for (idx, b) in open.stamp.buildings.iter().enumerate() {
                    let def = self.catalog.building(b.guid);
                    let selected = self.selected_building == Some(idx);
                    let fill = if selected {
                        Color32::from_rgb(90, 150, 220)
                    } else {
                        match def.map(|d| d.category.as_str()) {
                            Some("production") => Color32::from_rgb(70, 120, 80),
                            Some("warehouse") => Color32::from_rgb(90, 90, 140),
                            Some("module") => Color32::from_rgb(100, 140, 70),
                            Some("institution") => Color32::from_rgb(150, 90, 90),
                            Some("street") => Color32::from_rgb(110, 100, 80),
                            Some("residence") => Color32::from_rgb(110, 160, 200),
                            _ => Color32::from_rgb(80, 80, 90),
                        }
                    };

                    let cells = footprint_cells(b, def);
                    let mut min_x = i32::MAX;
                    let mut max_x = i32::MIN;
                    let mut min_y = i32::MAX;
                    let mut max_y = i32::MIN;
                    for c in &cells {
                        min_x = min_x.min(c.x);
                        max_x = max_x.max(c.x);
                        min_y = min_y.min(c.y);
                        max_y = max_y.max(c.y);
                        let r = self.cell_screen_rect(rect, c.x, c.y);
                        painter.rect_filled(r, 0.0, fill);
                    }
                    if min_x <= max_x {
                        let outline = Rect::from_min_max(
                            self.cell_screen_rect(rect, min_x, min_y).min,
                            self.cell_screen_rect(rect, max_x, max_y).max,
                        );
                        painter.rect_stroke(
                            outline,
                            0.0,
                            Stroke::new(
                                if selected { 2.0_f32 } else { 1.0_f32 },
                                Color32::WHITE,
                            ),
                            egui::StrokeKind::Outside,
                        );
                    }

                    if self.cell_px >= 12.0 && !cells.is_empty() {
                        let label = self.catalog.label(b.guid);
                        let font_size = if self.cell_px >= 22.0 { 11.0 } else { 9.0 };
                        let bw = (max_x - min_x + 1) as f32;
                        let max_w = (bw * self.cell_px * 0.92).max(20.0);
                        let galley = painter.layout(
                            label,
                            egui::FontId::proportional(font_size),
                            Color32::WHITE,
                            max_w,
                        );
                        let center = self.world_to_screen(rect, b.x, b.y);
                        let text_pos = Pos2::new(
                            center.x - galley.size().x * 0.5,
                            center.y - galley.size().y * 0.5,
                        );
                        painter.galley(text_pos, galley, Color32::WHITE);
                    }
                }

'''
path.write_text(text[:start] + new + text[end:], encoding="utf-8")
print("patched", start, end)

