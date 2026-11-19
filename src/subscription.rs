//! 订阅页：管理多个 Clash 订阅链接/脚本，展示各自的流量信息。

use crate::app::{App, SUBSCRIPTION_PAGE};
use crate::extras::{setting_row, settings_card, settings_label};
use crate::paint::{Card, H};
use crate::theme::{self, W};
use base64::Engine;
use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Item {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub source: String,
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    pub expire: u64,
    pub updated: u64,
    pub error: String,
}

impl Item {
    fn used(&self) -> u64 { self.upload.saturating_add(self.download) }
    fn percent(&self) -> Option<f32> { (self.total > 0).then(|| self.used() as f32 * 100.0 / self.total as f32) }
    fn remaining(&self) -> Option<u64> { (self.total > 0).then(|| self.total.saturating_sub(self.used())) }
    fn is_verge(&self) -> bool { self.kind == "verge" }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    items: Vec<Item>,
    names: HashMap<String, String>,
    hidden: HashSet<String>,
    selected: String,
}

pub(crate) struct State {
    items: Vec<Item>,
    selected: String,
    names: HashMap<String, String>,
    hidden: HashSet<String>,
    expanded: HashSet<String>,
    pub link: String,
    pub script: String,
    pub note: String,
    pending: HashSet<String>,
    tx: Sender<(String, Result<Fetched, String>)>,
    rx: Receiver<(String, Result<Fetched, String>)>,
    path: Option<PathBuf>,
    loading: bool,
}

struct Fetched {
    name: Option<String>,
    upload: u64,
    download: u64,
    total: u64,
    expire: u64,
}

impl State {
    pub(crate) fn load() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        if cfg!(test) {
            return Self { items: Vec::new(), selected: String::new(), names: HashMap::new(), hidden: HashSet::new(), expanded: HashSet::new(), link: String::new(), script: String::new(), note: String::new(), pending: HashSet::new(), tx, rx, path: None, loading: false };
        }
        let path = crate::model::config_dir().join("subscriptions.json");
        let saved: Saved = std::fs::read(&path).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default();
        let mut state = Self {
            items: saved.items,
            selected: saved.selected,
            names: saved.names,
            hidden: saved.hidden,
            expanded: HashSet::new(),
            link: String::new(),
            script: String::new(),
            note: String::new(),
            pending: HashSet::new(),
            tx,
            rx,
            path: Some(path),
            loading: false,
        };
        state.merge_verge();
        for item in &mut state.items {
            if let Some(name) = state.names.get(&item.id) { item.name = name.clone(); }
        }
        state.ensure_selected();
        state
    }

    fn ensure_selected(&mut self) {
        if !self.items.iter().any(|item| item.id == self.selected) {
            self.selected = self.items.first().map(|item| item.id.clone()).unwrap_or_default();
        }
    }

    fn merge_verge(&mut self) {
        for mut item in verge_items() {
            if self.hidden.contains(&item.id) { continue; }
            if let Some(name) = self.names.get(&item.id) { item.name = name.clone(); }
            if let Some(existing) = self.items.iter_mut().find(|existing| existing.id == item.id) {
                existing.upload = item.upload;
                existing.download = item.download;
                existing.total = item.total;
                existing.expire = item.expire;
                existing.updated = item.updated;
                existing.name = item.name;
            } else {
                self.items.push(item);
            }
        }
    }

    fn save(&mut self) {
        let Some(path) = &self.path else { return; };
        let saved = Saved {
            items: self.items.iter().filter(|item| !item.is_verge()).cloned().collect(),
            names: self.names.clone(),
            hidden: self.hidden.clone(),
            selected: self.selected.clone(),
        };
        let Ok(raw) = serde_json::to_vec(&saved) else { self.note = "订阅记录保存失败".into(); return; };
        if crate::model::atomic_write(path, &raw).is_err() { self.note = "订阅记录保存失败，请检查数据目录权限".into(); }
    }

    pub(crate) fn poll(&mut self) {
        while let Ok((id, result)) = self.rx.try_recv() {
            self.pending.remove(&id);
            if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                match result {
                    Ok(fetched) => {
                        if let Some(name) = fetched.name.filter(|name| !name.trim().is_empty()) {
                            if !self.names.contains_key(&id) { item.name = name; }
                        }
                        item.upload = fetched.upload;
                        item.download = fetched.download;
                        item.total = fetched.total;
                        item.expire = fetched.expire;
                        item.updated = now();
                        item.error.clear();
                    }
                    Err(error) => item.error = error,
                }
            }
        }
        self.loading = !self.pending.is_empty();
    }

    fn import_link(&mut self) {
        let value = self.link.trim().to_owned();
        if !(value.starts_with("https://") || value.starts_with("http://")) {
            self.note = "请输入以 http:// 或 https:// 开头的订阅链接".into();
            return;
        }
        if self.items.iter().any(|item| item.url == value) {
            self.note = "该链接已经导入过".into();
            return;
        }
        let id = format!("link:{}", digest(&value));
        let source = host_of(&value).unwrap_or_else(|| "订阅链接".into());
        self.items.push(Item { id: id.clone(), name: source.clone(), kind: "link".into(), url: value.clone(), source, upload: 0, download: 0, total: 0, expire: 0, updated: 0, error: "正在获取订阅信息…".into() });
        self.selected = id.clone();
        self.link.clear();
        self.note = "已添加订阅，正在获取流量信息".into();
        self.save();
        self.fetch(id, value);
    }

    fn import_script(&mut self) {
        let value = self.script.trim().to_owned();
        let path = PathBuf::from(&value);
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase();
        if !path.is_file() || !matches!(extension.as_str(), "js" | "yaml" | "yml") {
            self.note = "请填写本机已有的 .js / .yaml / .yml 脚本路径".into();
            return;
        }
        if self.items.iter().any(|item| item.url == value) {
            self.note = "该脚本已经导入过".into();
            return;
        }
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("脚本").to_owned();
        let id = format!("script:{}", digest(&value));
        self.items.push(Item { id: id.clone(), name, kind: "script".into(), url: value.clone(), source: "本地脚本".into(), upload: 0, download: 0, total: 0, expire: 0, updated: now(), error: String::new() });
        self.selected = id;
        self.script.clear();
        self.note = "脚本已添加".into();
        self.save();
    }

    fn remove(&mut self, id: &str) {
        if let Some(index) = self.items.iter().position(|item| item.id == id) {
            let item = self.items.remove(index);
            if item.is_verge() { self.hidden.insert(item.id.clone()); }
            self.names.remove(id);
            self.pending.remove(id);
            self.ensure_selected();
            self.save();
            self.note = "已移除订阅".into();
        }
    }

    fn fetch(&mut self, id: String, url: String) {
        if self.pending.contains(&id) { return; }
        self.pending.insert(id.clone());
        self.loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || { let result = fetch(&url); let _ = tx.send((id, result)); });
    }

    fn refresh_all(&mut self) {
        self.merge_verge();
        let targets: Vec<(String, String)> = self.items.iter().filter(|item| item.kind == "link").map(|item| (item.id.clone(), item.url.clone())).collect();
        for (id, url) in targets { self.fetch(id, url); }
        self.note = "正在刷新订阅信息".into();
    }
}


impl App {
    pub(crate) fn subscription_bar_count(&self) -> usize {
        if !self.settings.visible_pages[SUBSCRIPTION_PAGE] || self.settings.minimal || self.subscriptions.items.is_empty() { 0 } else { 1 }
    }

    pub(crate) fn subscription_bar_extra(&self, ctx: &egui::Context) -> f32 {
        let text = self.subscriptions.items.iter().find(|item| item.id == self.subscriptions.selected)
            .and_then(|item| item.percent()).map(percent_text).unwrap_or_else(|| "--".into());
        ctx.fonts(|fonts| fonts.layout_no_wrap(text, theme::font(theme::size::PILL_CLOCK, W::Bold), theme::WHITE).size().x).ceil() + 6.0
    }

    pub(crate) fn draw_subscription_bar(&mut self, c: &Card, cx: f32, layout: &crate::bar_layout::Layout, position: usize) {
        let Some(item) = self.subscriptions.items.iter().find(|item| item.id == self.subscriptions.selected).cloned() else { return; };
        let slot = crate::bar_layout::quota_slot(position);
        let Some(offset) = layout.x.get(slot).copied().flatten() else { return; };
        let alpha = layout.pet_clearance(slot);
        let faded = c.faded(alpha);
        let c = if alpha < 0.8 { &faded.ghost() } else { &faded };
        let x = cx + offset;
        let extra = (layout.widths[slot] - 27.0).max(0.0);
        let icon_x = x - extra / 2.0;
        subscription_icon_card(c, icon_x - 12.0, 3.0, 24.0);
        let text = item.percent().map(percent_text).unwrap_or_else(|| "--".into());
        c.text(icon_x + 15.0, 21.0, &text, theme::size::PILL_CLOCK, W::Bold, theme::WHITE, H::Left);
        let summary = if item.total > 0 { format!("{} · {}\n{} / {}", item.name, text, bytes(item.used()), bytes(item.total)) } else { format!("{}\n尚未获取流量信息", item.name) };
        if c.hit(x - (27.0 + extra) / 2.0, 1.0, 27.0 + extra, 28.0, "subscription-pin").on_hover_text(summary).clicked() {
            self.show_page(SUBSCRIPTION_PAGE);
        }
    }
}

impl App {
    pub(crate) fn subscription_settings(&mut self, ui: &mut egui::Ui) {
        self.subscriptions.poll();
        settings_card(ui, |ui| {
            let mut visible = self.settings.visible_pages[SUBSCRIPTION_PAGE];
            setting_row(ui, "常驻岛内", "关闭后订阅页不在岛上显示，也不读取订阅信息", &mut visible);
            if visible != self.settings.visible_pages[SUBSCRIPTION_PAGE] {
                self.set_page_visible(SUBSCRIPTION_PAGE, visible);
            }
        });
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.add_sized([ui.available_width() - 76.0, 30.0], egui::TextEdit::singleline(&mut self.subscriptions.link).hint_text("订阅文件链接"));
            if ui.add(egui::Button::new(egui::RichText::new("新建").color(Color32::WHITE)).fill(Color32::from_rgb(0, 122, 255))).clicked() {
                self.subscriptions.import_link();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_sized([ui.available_width() - 90.0, 28.0], egui::TextEdit::singleline(&mut self.subscriptions.script).hint_text("脚本文件路径（.js / .yaml）"));
            if ui.button("导入脚本").clicked() { self.subscriptions.import_script(); }
        });
        if !self.subscriptions.note.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(&self.subscriptions.note).size(11.0).color(ui.visuals().weak_text_color()));
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("订阅列表").size(12.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!self.subscriptions.loading, egui::Button::new("刷新")).clicked() { self.subscriptions.refresh_all(); }
            });
        });
        ui.add_space(6.0);

        let items = self.subscriptions.items.clone();
        if items.is_empty() {
            settings_card(ui, |ui| { ui.label(egui::RichText::new("还没有订阅。在上方粘贴订阅链接后点“新建”。").size(12.0).color(ui.visuals().weak_text_color())); });
            return;
        }
        let mut remove = None;
        let mut select = None;
        for item in items {
            let fill = crate::settings_theme::color(ui.ctx(), Color32::from_rgb(35, 35, 38), Color32::WHITE);
            egui::Frame::new().fill(fill).corner_radius(10).inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                ui.set_width((ui.available_width() - 1.0).max(0.0));
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.push_id(item.id.clone(), |ui| {
                    ui.horizontal(|ui| {
                        subscription_icon(ui, 26.0);
                        let mut name = item.name.clone();
                        if ui.add_sized([ui.available_width() - 140.0, 26.0], egui::TextEdit::singleline(&mut name).hint_text("订阅名称")).changed() {
                            self.subscriptions.names.insert(item.id.clone(), name.clone());
                            if let Some(target) = self.subscriptions.items.iter_mut().find(|target| target.id == item.id) { target.name = name; }
                            self.subscriptions.save();
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("移除").clicked() { remove = Some(item.id.clone()); }
                            if ui.selectable_label(self.subscriptions.selected == item.id, "当前").clicked() { select = Some(item.id.clone()); }
                        });
                    });
                    let source = if item.source.is_empty() { "Clash 订阅".to_owned() } else { item.source.clone() };
                    ui.label(egui::RichText::new(format!("来自 {source}")).size(10.5).color(ui.visuals().weak_text_color()));
                    if item.kind != "script" {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("已使用 / 总量").size(11.0).color(ui.visuals().weak_text_color()));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let total = if item.total > 0 { bytes(item.total) } else { if item.error.is_empty() { "未知".into() } else { "未获取".into() } };
                                ui.label(egui::RichText::new(format!("{} / {}", bytes(item.used()), total)).size(12.0).strong());
                            });
                        });
                        if let Some(percent) = item.percent() {
                            ui.add(egui::ProgressBar::new((percent / 100.0).clamp(0.0, 1.0)).desired_height(5.0).fill(Color32::from_rgb(0, 122, 255)));
                        }
                        if !item.error.is_empty() {
                            ui.label(egui::RichText::new(&item.error).size(10.5).color(Color32::from_rgb(214, 108, 60)));
                        }
                        let expanded = self.subscriptions.expanded.contains(&item.id);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("更新 {} · {}", date_text(item.updated), expire_text(item.expire))).size(10.0).color(ui.visuals().weak_text_color()));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(if expanded { "收起" } else { "详情" }).clicked() {
                                    if expanded { self.subscriptions.expanded.remove(&item.id); } else { self.subscriptions.expanded.insert(item.id.clone()); }
                                }
                            });
                        });
                        if expanded {
                            ui.label(egui::RichText::new(format!("上传 {} · 下载 {} · 剩余 {}", bytes(item.upload), bytes(item.download), item.remaining().map(bytes).unwrap_or_else(|| "未知".into()))).size(10.0).color(ui.visuals().weak_text_color()));
                        }
                    } else {
                        ui.label(egui::RichText::new("本地脚本，无流量信息").size(10.5).color(ui.visuals().weak_text_color()));
                    }
                });
            });
            ui.add_space(8.0);
        }
        if let Some(id) = select { self.subscriptions.selected = id; self.subscriptions.save(); }
        if let Some(id) = remove { self.subscriptions.remove(&id); }
        ui.label(egui::RichText::new("链接与脚本只记录在岛仔本地，不会修改 Clash Verge 的配置或订阅。").size(11.0).color(ui.visuals().weak_text_color()));
    }

    pub(crate) fn page_subscription(&mut self, c: &Card) {
        self.subscriptions.poll();
        let items = self.subscriptions.items.clone();
        if items.is_empty() {
            c.text(15.0, 66.0, "还没有订阅", 11.5, W::Semibold, theme::WHITE, H::Left);
            c.text(15.0, 85.0, "在设置 → 订阅中导入链接或脚本", 9.5, W::Regular, theme::DIM, H::Left);
            return;
        }
        // 订阅切换：最多显示三个名称，超过时用右侧箭头循环。
        let selected = items.iter().position(|item| item.id == self.subscriptions.selected).unwrap_or(0);
        let start = selected.saturating_sub(1).min(items.len().saturating_sub(3));
        for (slot, item) in items.iter().skip(start).take(3).enumerate() {
            let x = 15.0 + slot as f32 * 128.0;
            let on = item.id == self.subscriptions.selected;
            if on { c.fill(x, 30.0, 122.0, 21.0, 6.0, theme::BTN); }
            c.text(x + 61.0, 45.0, &c.fit(&item.name, 9.5, W::Regular, 110.0), 9.5, W::Regular, if on { theme::WHITE } else { theme::GRAY }, H::Center);
            if c.hit(x, 30.0, 122.0, 21.0, ("sub-tab", item.id.clone())).clicked() {
                self.subscriptions.selected = item.id.clone();
                self.subscriptions.save();
            }
        }
        if items.len() > 3 {
            if c.hit(363.0, 30.0, 26.0, 21.0, "sub-next").clicked() {
                let next = (selected + 1) % items.len();
                self.subscriptions.selected = items[next].id.clone();
                self.subscriptions.save();
            }
            c.text(376.0, 44.0, "›", 15.0, W::Regular, theme::GRAY, H::Center);
        }
        let Some(item) = self.subscriptions.items.iter().find(|item| item.id == self.subscriptions.selected).cloned() else { return; };
        let expanded = self.subscriptions.expanded.contains(&item.id);
        let (x, y, w, h) = (15.0, 60.0, 386.0, 128.0);
        c.fill(x, y, w, h, 10.0, theme::BTN);
        if c.hit(x, y, w, h, "sub-card").on_hover_text(if expanded { "点击收起详情" } else { "点击查看完整订阅详情" }).clicked() {
            if expanded { self.subscriptions.expanded.remove(&item.id); } else { self.subscriptions.expanded.insert(item.id.clone()); }
        }
        subscription_icon_card(c, 27.0, 72.0, 28.0);
        c.text(66.0, 85.0, &c.fit(&item.name, 12.0, W::Semibold, 250.0), 12.0, W::Semibold, theme::WHITE, H::Left);
        if let Some(percent) = item.percent() {
            c.text(389.0, 85.0, &percent_text(percent), 13.0, W::Semibold, theme::BLUE, H::Right);
        }
        c.text(66.0, 101.0, &c.fit(&format!("来自 {}", if item.source.is_empty() { "Clash 订阅" } else { &item.source }), 9.0, W::Regular, 260.0), 9.0, W::Regular, theme::GRAY, H::Left);
        c.fill(27.0, 111.0, 362.0, 1.0, 0.5, Color32::from_rgb(58, 58, 62));
        c.text(27.0, 126.0, "已使用 / 总量", 9.0, W::Regular, theme::GRAY, H::Left);
        let total = if item.total > 0 { bytes(item.total) } else { "未知".into() };
        c.text(389.0, 126.0, &format!("{} / {}", bytes(item.used()), total), 10.0, W::Semibold, theme::WHITE, H::Right);
        if let Some(percent) = item.percent() {
            let bar_w = 362.0;
            c.fill(27.0, 133.0, bar_w, 4.0, 2.0, Color32::from_rgb(58, 58, 62));
            let filled = (bar_w * (percent / 100.0)).clamp(2.0, bar_w);
            c.fill(27.0, 133.0, filled, 4.0, 2.0, theme::BLUE);
        }
        c.text(27.0, 152.0, &c.fit(&format!("更新 {} · {}", date_text(item.updated), expire_text(item.expire)), 8.5, W::Regular, 330.0), 8.5, W::Regular, theme::DIM, H::Left);
        if expanded {
            c.text(27.0, 172.0, &format!("上传 {} · 下载 {}", bytes(item.upload), bytes(item.download)), 9.0, W::Regular, theme::GRAY, H::Left);
            c.text(389.0, 172.0, &format!("剩余 {}", item.remaining().map(bytes).unwrap_or_else(|| "未知".into())), 9.0, W::Regular, theme::DIM, H::Right);
        }
        if !item.error.is_empty() {
            c.text(27.0, 190.0, &c.fit(&item.error, 9.0, W::Regular, 360.0), 9.0, W::Regular, Color32::from_rgb(214, 108, 60), H::Left);
        }
    }
}

fn subscription_icon(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, 6.0, Color32::from_rgb(228, 238, 252));
    let color = Color32::from_rgb(0, 122, 255);
    let center = rect.center();
    ui.painter().line_segment([center + egui::vec2(0.0, -size * 0.28), center + egui::vec2(0.0, size * 0.06)], egui::Stroke::new(2.0, color));
    ui.painter().line_segment([center + egui::vec2(-size * 0.18, size * 0.02), center + egui::vec2(0.0, size * 0.2)], egui::Stroke::new(2.0, color));
    ui.painter().line_segment([center + egui::vec2(size * 0.18, size * 0.02), center + egui::vec2(0.0, size * 0.2)], egui::Stroke::new(2.0, color));
    ui.painter().line_segment([center + egui::vec2(-size * 0.22, size * 0.3), center + egui::vec2(size * 0.22, size * 0.3)], egui::Stroke::new(2.0, color));
}

fn subscription_icon_card(c: &Card, x: f32, y: f32, size: f32) {
    c.fill(x, y, size, size, 7.0, Color32::from_rgb(24, 58, 92));
    let cx = x + size / 2.0;
    let cy = y + size / 2.0;
    c.fill(cx - 1.0, cy - 8.0, 2.0, 10.0, 1.0, theme::BLUE);
    c.fill(cx - 5.0, cy + 0.0, 10.0, 2.0, 1.0, theme::BLUE);
    c.fill(cx - 3.0, cy + 3.0, 6.0, 2.0, 1.0, theme::BLUE);
    c.fill(cx - 6.0, cy + 7.0, 12.0, 2.0, 1.0, theme::BLUE);
}

fn fetch(url: &str) -> Result<Fetched, String> {
    let proxy = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "HTTP_PROXY", "http_proxy"]
        .iter().find_map(|key| std::env::var(key).ok().filter(|value| !value.trim().is_empty()))
        .or_else(crate::quota::system_proxy);
    let mut builder = ureq::AgentBuilder::new().try_proxy_from_env(false).timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(15)).timeout_write(Duration::from_secs(15));
    if let Some(proxy) = proxy { builder = builder.proxy(ureq::Proxy::new(&proxy).map_err(|_| "代理地址无效".to_owned())?); }
    let agent = builder.build();
    let response = agent.get(url).set("User-Agent", "clash-verge/v2.5.10").set("Accept", "*/*").call().map_err(|error| match error {
        ureq::Error::Status(code, _) => format!("订阅返回 HTTP {code}"),
        _ => "无法连接订阅地址，请检查网络或代理".to_owned(),
    })?;
    let name = response.header("profile-title").and_then(decode_title)
        .or_else(|| response.header("content-disposition").and_then(filename));
    let (upload, download, total, expire) = parse_userinfo(response.header("subscription-userinfo").unwrap_or_default());
    if upload == 0 && download == 0 && total == 0 {
        return Err("订阅未返回流量信息".into());
    }
    Ok(Fetched { name, upload, download, total, expire })
}

fn decode_title(value: &str) -> Option<String> {
    let raw = value.trim();
    let decoded = raw.strip_prefix("base64:").map(|text| {
        base64::engine::general_purpose::STANDARD.decode(text.trim()).ok().and_then(|bytes| String::from_utf8(bytes).ok())
    }).unwrap_or_else(|| Some(raw.to_owned()))?;
    (!decoded.trim().is_empty()).then(|| decoded.trim().to_owned())
}

fn filename(value: &str) -> Option<String> {
    let name = value.split("filename*=UTF-8''").nth(1).or_else(|| value.split("filename=").nth(1))?;
    Some(name.trim_matches('"').trim().to_owned())
}

fn parse_userinfo(value: &str) -> (u64, u64, u64, u64) {
    let mut upload = 0;
    let mut download = 0;
    let mut total = 0;
    let mut expire = 0;
    for part in value.split(';') {
        let Some((key, value)) = part.split_once('=') else { continue; };
        let number = value.trim().parse().unwrap_or(0);
        match key.trim() {
            "upload" => upload = number,
            "download" => download = number,
            "total" => total = number,
            "expire" => expire = number,
            _ => {}
        }
    }
    (upload, download, total, expire)
}

fn verge_items() -> Vec<Item> {
    let Some(base) = std::env::var_os("APPDATA").map(PathBuf::from) else { return Vec::new(); };
    let path = base.join("io.github.clash-verge-rev.clash-verge-rev").join("profiles.yaml");
    let Ok(raw) = std::fs::read_to_string(path) else { return Vec::new(); };
    let mut result = Vec::new();
    let mut current: Option<(String, String, String, u64, u64, u64, u64, u64)> = None;
    for line in raw.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("- uid:") && indent == 0 {
            if let Some(item) = current.take().and_then(verge_item) { result.push(item); }
            current = Some((trimmed.trim_start_matches("- uid:").trim().to_owned(), String::new(), String::new(), 0, 0, 0, 0, 0));
            continue;
        }
        let Some((_, kind, name, updated, upload, download, total, expire)) = current.as_mut() else { continue; };
        if indent == 2 {
            if let Some(value) = trimmed.strip_prefix("type:") { *kind = value.trim().to_owned(); }
            else if let Some(value) = trimmed.strip_prefix("name:") { *name = value.trim().trim_matches('"').to_owned(); }
            else if let Some(value) = trimmed.strip_prefix("updated:") { *updated = value.trim().parse().unwrap_or(0); }
        } else if indent == 4 {
            if let Some(value) = trimmed.strip_prefix("upload:") { *upload = value.trim().parse().unwrap_or(0); }
            else if let Some(value) = trimmed.strip_prefix("download:") { *download = value.trim().parse().unwrap_or(0); }
            else if let Some(value) = trimmed.strip_prefix("total:") { *total = value.trim().parse().unwrap_or(0); }
            else if let Some(value) = trimmed.strip_prefix("expire:") { *expire = value.trim().parse().unwrap_or(0); }
        }
    }
    if let Some(item) = current.take().and_then(verge_item) { result.push(item); }
    result
}

fn verge_item((uid, kind, name, updated, upload, download, total, expire): (String, String, String, u64, u64, u64, u64, u64)) -> Option<Item> {
    if kind != "remote" { return None; }
    Some(Item {
        id: format!("verge:{uid}"),
        name: if name.is_empty() || name == "null" { "Clash 订阅".into() } else { name },
        kind: "verge".into(),
        url: String::new(),
        source: "Clash Verge".into(),
        upload, download, total, expire, updated,
        error: String::new(),
    })
}

fn digest(value: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, value.as_bytes()).as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() }

fn host_of(value: &str) -> Option<String> {
    let rest = value.split("://").nth(1).unwrap_or(value);
    let host = rest.split('/').next()?.trim();
    (!host.is_empty()).then(|| host.to_owned())
}

fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut number = value as f64;
    let mut unit = 0;
    while number >= 1024.0 && unit < UNITS.len() - 1 { number /= 1024.0; unit += 1; }
    if unit == 0 { format!("{value} B") } else if number >= 100.0 { format!("{number:.0} {}", UNITS[unit]) } else { format!("{number:.1} {}", UNITS[unit]) }
}

fn percent_text(percent: f32) -> String {
    if percent >= 10.0 { format!("{percent:.0}%") } else if percent >= 1.0 { format!("{percent:.1}%") } else { format!("{percent:.2}%") }
}

fn date_text(epoch: u64) -> String {
    if epoch == 0 { return "未知".into(); }
    chrono::DateTime::from_timestamp(epoch as i64, 0).map(|time| time.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_else(|| "未知".into())
}

fn expire_text(expire: u64) -> String {
    if expire == 0 { "长期有效".into() } else { date_text(expire) }
}
