from pathlib import Path

path = Path(r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\stamp_editor\src\app.rs")
text = path.read_text(encoding="utf-8")

# 1) Replace tile helpers (from paint_tex through end of portrait_tile)
h_start = text.index("    fn paint_tex(ui: &egui::Ui")
# portrait_tile ends just before snap_world or delete_selection - find next fn after portrait
h_end = text.index("\n    fn snap_world(")
helpers = r'''    fn paint_tex(ui: &egui::Ui, tex: &egui::TextureHandle, rect: Rect, tint: Color32) {
        ui.painter().image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    }

    /// Simple icon button: dark tile + game icon (no chrome overlay).
    fn icon_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        game_icon: Option<&str>,
        selected: bool,
        size: Vec2,
        tip: &str,
    ) -> egui::Response {
        let fill = if selected {
            Color32::from_rgb(70, 48, 88)
        } else {
            Color32::from_rgb(36, 44, 58)
        };
        let stroke = if selected {
            Stroke::new(2.0_f32, Color32::from_rgb(120, 175, 235))
        } else {
            Stroke::new(1.0_f32, Color32::from_rgb(70, 85, 110))
        };
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        ui.painter()
            .rect(rect, 6.0, fill, stroke, egui::StrokeKind::Outside);
        let pad = 6.0;
        let img_rect = rect.shrink(pad);
        let tex = self.ui_icons.get_or_placeholder(
            ctx,
            game_icon,
            [64, 64],
            if game_icon.is_some() {
                [40, 50, 70]
            } else {
                [60, 50, 40]
            },
        );
        Self::paint_tex(ui, &tex, img_rect, Color32::WHITE);
        resp.on_hover_text(tip)
    }

    /// Goods / chain icon with optional caption under it.
    fn goods_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        game_icon: Option<&str>,
        selected: bool,
        tip: &str,
        show_caption: bool,
    ) -> egui::Response {
        let icon = 64.0_f32;
        let cap_h = if show_caption { 16.0 } else { 0.0 };
        let total = Vec2::new(icon + 8.0, icon + cap_h + 4.0);
        let (rect, resp) = ui.allocate_exact_size(total, Sense::click());
        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.top() + icon * 0.5 + 2.0),
            Vec2::splat(icon),
        );
        let fill = if selected {
            Color32::from_rgb(78, 52, 92)
        } else {
            Color32::from_rgb(32, 40, 54)
        };
        let stroke = if selected {
            Stroke::new(2.0_f32, Color32::from_rgb(230, 200, 120))
        } else {
            Stroke::new(1.0_f32, Color32::from_rgb(60, 75, 100))
        };
        ui.painter()
            .rect(icon_rect, 8.0, fill, stroke, egui::StrokeKind::Outside);
        let tex = self
            .ui_icons
            .get_or_placeholder(ctx, game_icon, [64, 64], [40, 50, 70]);
        Self::paint_tex(ui, &tex, icon_rect.shrink(6.0), Color32::WHITE);
        if show_caption {
            let short = if tip.chars().count() > 10 {
                format!("{}…", tip.chars().take(9).collect::<String>())
            } else {
                tip.to_string()
            };
            ui.painter().text(
                Pos2::new(rect.center().x, rect.bottom() - 2.0),
                egui::Align2::CENTER_BOTTOM,
                short,
                egui::FontId::proportional(11.0),
                if selected {
                    Color32::from_rgb(255, 230, 180)
                } else {
                    Color32::from_rgb(190, 200, 215)
                },
            );
        }
        resp.on_hover_text(tip)
    }

    fn portrait_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        portrait: Option<&str>,
        numeral: Option<&str>,
        selected: bool,
        tip: &str,
    ) -> egui::Response {
        let size = Vec2::new(64.0, 88.0);
        let fill = if selected {
            Color32::from_rgb(92, 42, 72)
        } else {
            Color32::from_rgb(34, 42, 56)
        };
        let stroke = if selected {
            Stroke::new(2.0_f32, Color32::from_rgb(110, 165, 230))
        } else {
            Stroke::new(1.0_f32, Color32::from_rgb(55, 70, 95))
        };
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        ui.painter()
            .rect(rect, 6.0, fill, stroke, egui::StrokeKind::Outside);
        let img_rect = Rect::from_min_size(
            Pos2::new(rect.left() + 5.0, rect.top() + 4.0),
            Vec2::new(rect.width() - 10.0, rect.height() - 22.0),
        );
        let tex = self
            .ui_icons
            .get_or_placeholder(ctx, portrait, [72, 96], [45, 40, 55]);
        Self::paint_tex(ui, &tex, img_rect, Color32::WHITE);
        let badge = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.bottom() - 11.0),
            Vec2::new(24.0, 16.0),
        );
        ui.painter()
            .rect_filled(badge, 3.0, Color32::from_rgb(16, 20, 28));
        if let Some(num) = numeral {
            if let Some(tex) = self.ui_icons.get(ctx, Some(num)) {
                Self::paint_tex(ui, &tex, badge.shrink(2.0), Color32::WHITE);
            } else {
                ui.painter().text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    tip.chars().next().unwrap_or('·'),
                    egui::FontId::proportional(12.0),
                    Color32::from_rgb(230, 220, 190),
                );
            }
        }
        resp.on_hover_text(tip)
    }

'''

text = text[:h_start] + helpers + text[h_end:]

# 2) Replace buildmenu panel
p_start = text.index("        // Anno 117 construction bar")
p_end = text.index("        egui::CentralPanel::default().show(ctx, |ui| {")

panel = r'''        // Construction bar — UX: Tier → Kette/Goods → Gebäude der Kette.
        let chain_open = self.buildmenu_entry.is_some();
        let bar_h = if chain_open { 268.0 } else { 196.0 };
        egui::TopBottomPanel::bottom("buildmenu")
            .resizable(true)
            .default_height(bar_h)
            .min_height(180.0)
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, Color32::from_rgb(24, 28, 36));
                {
                    let style = ui.style_mut();
                    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
                    style.spacing.button_padding = egui::vec2(6.0, 4.0);
                }

                let tabs = self.buildmenu.tabs_for(self.region_filter).to_vec();
                if let Some(t) = self.buildmenu_tab {
                    if t >= tabs.len() {
                        self.buildmenu_tab = if tabs.is_empty() { None } else { Some(0) };
                        self.buildmenu_entry = None;
                    }
                } else if !tabs.is_empty() {
                    self.buildmenu_tab = tabs.iter().position(|t| t.is_need()).or(Some(0));
                }
                if tabs.is_empty() {
                    ui.label("Kein Construction-Menu geladen.");
                    return;
                }

                let need_indices: Vec<usize> = tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.is_need())
                    .map(|(i, _)| i)
                    .collect();
                let infra_tab = tabs.iter().find(|t| t.is_infrastructure()).cloned();
                let tab_idx = self.buildmenu_tab.unwrap_or(0);
                let need_mode = self.buildmenu_infra_entry.is_none()
                    && tabs.get(tab_idx).map(|t| t.is_need()).unwrap_or(false);

                let content_entries: Vec<MenuEntry> = if let Some(ei) = self.buildmenu_infra_entry {
                    if let Some(infra) = infra_tab.as_ref() {
                        match infra.entries.get(ei) {
                            Some(MenuEntry::Category { entries, .. }) => entries.clone(),
                            _ => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    }
                } else if need_mode {
                    tabs.get(tab_idx).map(|t| t.entries.clone()).unwrap_or_default()
                } else {
                    Vec::new()
                };

                // —— 1) Expanded chain: buildings you can place ——
                if let Some(ei) = self.buildmenu_entry {
                    if let Some(entry) = content_entries.get(ei) {
                        match entry {
                            MenuEntry::Chain {
                                buildings,
                                short_name,
                                name,
                                ..
                            } => {
                                let chain_lbl = short_name
                                    .clone()
                                    .unwrap_or_else(|| name.replace("&amp;", "&"));
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("Kette: {chain_lbl}"))
                                            .size(14.0)
                                            .color(Color32::from_rgb(230, 210, 160)),
                                    );
                                    ui.label(
                                        RichText::new("— Gebäude wählen zum Platzieren")
                                            .size(12.0)
                                            .weak(),
                                    );
                                });
                                egui::ScrollArea::horizontal()
                                    .id_salt("buildmenu_chain_buildings")
                                    .max_height(92.0)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.add_space(8.0);
                                            for &bg in buildings {
                                                let lbl = self.catalog.label(bg);
                                                let icon = self
                                                    .buildmenu
                                                    .building_icon(bg)
                                                    .map(|s| s.to_string());
                                                let sel = self.placing_guid == Some(bg);
                                                if self
                                                    .goods_tile(
                                                        ui,
                                                        ctx,
                                                        icon.as_deref(),
                                                        sel,
                                                        &lbl,
                                                        true,
                                                    )
                                                    .clicked()
                                                {
                                                    self.activate_menu_building(bg, &lbl);
                                                }
                                            }
                                        });
                                    });
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);
                            }
                            MenuEntry::Category {
                                entries: nested,
                                short_name,
                                name,
                                ..
                            } => {
                                let cat_lbl = short_name
                                    .clone()
                                    .unwrap_or_else(|| name.replace("&amp;", "&"));
                                ui.label(
                                    RichText::new(format!("Gruppe: {cat_lbl}"))
                                        .size(14.0)
                                        .color(Color32::from_rgb(230, 210, 160)),
                                );
                                egui::ScrollArea::horizontal()
                                    .id_salt("buildmenu_nested")
                                    .max_height(92.0)
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            for nest in nested {
                                                let tip = nest.label();
                                                let sel = matches!(
                                                    nest,
                                                    MenuEntry::Building { guid, .. }
                                                        if self.placing_guid == Some(*guid)
                                                );
                                                if self
                                                    .goods_tile(
                                                        ui, ctx, nest.icon(), sel, &tip, true,
                                                    )
                                                    .clicked()
                                                {
                                                    match nest {
                                                        MenuEntry::Building { guid, .. } => {
                                                            let lbl = self.catalog.label(*guid);
                                                            self.activate_menu_building(
                                                                *guid, &lbl,
                                                            );
                                                        }
                                                        MenuEntry::Chain { buildings, .. } => {
                                                            if let Some(&bg) = buildings.first() {
                                                                let lbl = self.catalog.label(bg);
                                                                self.activate_menu_building(
                                                                    bg, &lbl,
                                                                );
                                                            }
                                                        }
                                                        MenuEntry::Category { .. } => {}
                                                    }
                                                }
                                            }
                                        });
                                    });
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);
                            }
                            MenuEntry::Building { .. } => {}
                        }
                    }
                }

                // —— 2) Goods / chains for active tier (or infra category) ——
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if need_mode {
                            "Needs / Ketten"
                        } else {
                            "Gebäude / Gruppen"
                        })
                        .size(12.0)
                        .weak(),
                    );
                });
                egui::ScrollArea::horizontal()
                    .id_salt("buildmenu_goods_row")
                    .max_height(88.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            if content_entries.is_empty() && !need_mode {
                                if let Some(infra) = infra_tab.as_ref() {
                                    for (ei, entry) in infra.entries.iter().enumerate() {
                                        let sel = self.buildmenu_infra_entry == Some(ei);
                                        let tip = entry.label();
                                        if self
                                            .goods_tile(
                                                ui, ctx, entry.icon(), sel, &tip, true,
                                            )
                                            .clicked()
                                        {
                                            match entry {
                                                MenuEntry::Building { guid, .. } => {
                                                    self.buildmenu_infra_entry = Some(ei);
                                                    self.buildmenu_entry = None;
                                                    let lbl = self.catalog.label(*guid);
                                                    self.activate_menu_building(*guid, &lbl);
                                                }
                                                MenuEntry::Category { .. } => {
                                                    self.buildmenu_infra_entry = Some(ei);
                                                    self.buildmenu_entry = None;
                                                    self.placing_guid = None;
                                                    self.status = format!("Kategorie: {tip}");
                                                }
                                                MenuEntry::Chain { buildings, .. } => {
                                                    self.buildmenu_infra_entry = Some(ei);
                                                    self.buildmenu_entry = Some(0);
                                                    if let Some(&bg) = buildings.first() {
                                                        let lbl = self.catalog.label(bg);
                                                        self.activate_menu_building(bg, &lbl);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                for (ei, entry) in content_entries.iter().enumerate() {
                                    let sel = self.buildmenu_entry == Some(ei);
                                    let tip = entry.label();
                                    if self
                                        .goods_tile(ui, ctx, entry.icon(), sel, &tip, true)
                                        .clicked()
                                    {
                                        self.buildmenu_entry = Some(ei);
                                        match entry {
                                            MenuEntry::Building { guid, .. } => {
                                                let lbl = self.catalog.label(*guid);
                                                self.activate_menu_building(*guid, &lbl);
                                            }
                                            MenuEntry::Chain { .. } => {
                                                // Expand chain row — don't place yet.
                                                self.placing_guid = None;
                                                self.road_brush_guid = None;
                                                self.status = format!(
                                                    "Kette „{tip}“ — Gebäude oben wählen"
                                                );
                                            }
                                            MenuEntry::Category { .. } => {
                                                self.placing_guid = None;
                                                self.status = format!("Gruppe „{tip}“");
                                            }
                                        }
                                    }
                                }
                            }
                        });
                    });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                // —— 3) Bottom: Infra toggle + tier portraits ——
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    let infra_on = !need_mode;
                    if self
                        .icon_tile(
                            ui,
                            ctx,
                            Some(ICON_CRANE),
                            infra_on,
                            Vec2::new(56.0, 72.0),
                            "Infrastruktur (Wege, Häuser, …)",
                        )
                        .clicked()
                    {
                        if let Some(i) = tabs.iter().position(|t| t.is_infrastructure()) {
                            self.buildmenu_tab = Some(i);
                        }
                        self.buildmenu_infra_entry = None;
                        self.buildmenu_entry = None;
                        self.placing_guid = None;
                        self.status = "Infrastruktur".into();
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.label(
                        RichText::new("Einwohner")
                            .size(12.0)
                            .weak(),
                    );
                    ui.add_space(4.0);

                    for &i in &need_indices {
                        let t = &tabs[i];
                        let selected = need_mode && self.buildmenu_tab == Some(i);
                        if self
                            .portrait_tile(
                                ui,
                                ctx,
                                t.category_icon.as_deref(),
                                t.secondary_icon.as_deref(),
                                selected,
                                &t.short_tier_label(),
                            )
                            .clicked()
                        {
                            self.buildmenu_tab = Some(i);
                            self.buildmenu_entry = None;
                            self.buildmenu_infra_entry = None;
                            self.road_brush_guid = None;
                            self.status = format!("Tier: {}", t.short_tier_label());
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if let Some(g) = self.placing_guid {
                            ui.label(
                                RichText::new(format!("Hand: {}", self.catalog.label(g)))
                                    .size(13.0)
                                    .color(Color32::from_rgb(170, 205, 255)),
                            );
                        }
                        ui.label(
                            RichText::new(self.region_filter.label())
                                .size(12.0)
                                .weak(),
                        );
                    });
                });
            });

'''

text = text[:p_start] + panel + text[p_end:]
path.write_text(text, encoding="utf-8")
print("ok")

