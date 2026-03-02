use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::fs;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use std::process;
use std::{env, panic};

use anyhow::{Context, Result};
use gtk;
use eframe::egui::{self, Align, Color32, CursorIcon, FontId, Layout, Rounding, RichText, Stroke, ViewportCommand};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use reqwest::blocking::Client;
use notify_rust::Notification;
use rfd::FileDialog;
use url::Url;

use crate::{discovery, syncer};
use crate::storage::{self, Feed, FeedEntry, Store};

const TRAY_ID_OPEN: &str = "open";
const TRAY_ID_SYNC: &str = "sync";
const TRAY_ID_QUIT: &str = "quit";

const ACCENT: Color32 = Color32::from_rgb(66, 176, 255);
const TEXT_BRIGHT: Color32 = Color32::from_rgb(226, 243, 255);
const TEXT_MUTED: Color32 = Color32::from_rgb(172, 196, 224);
const TITLE_UNREAD: Color32 = Color32::from_rgb(255, 246, 196);
const TITLE_READ: Color32 = Color32::from_rgb(204, 227, 255);
const BG_PANEL: Color32 = Color32::from_rgb(18, 30, 48);
const BG_PANEL_SOFT: Color32 = Color32::from_rgb(27, 42, 65);
const BG_WINDOW: Color32 = Color32::from_rgb(12, 21, 36);
const BG_EXTREME: Color32 = Color32::from_rgb(8, 14, 24);
const APP_NAME: &str = "Nimbus";
const APP_USER_AGENT: &str = "nimbus-desktop/0.1";

fn strip_html_tags(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => {
                in_tag = true;
            }
            '>' => {
                if in_tag {
                    in_tag = false;
                    out.push(' ');
                } else {
                    out.push('>');
                }
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }

    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#8217;", "'")
        .replace("&#8216;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace("&#8230;", "…")
        .replace("&lt;", "<")
        .replace("&gt;", ">");

    decoded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn canonical_site_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    match Url::parse(trimmed) {
        Ok(parsed) => {
            let host = match parsed.host_str() {
                Some(host) => host.to_string(),
                None => return trimmed.to_string(),
            };
            let mut root = format!("{}://{}", parsed.scheme(), host);
            if let Some(port) = parsed.port() {
                root.push(':');
                root.push_str(&port.to_string());
            }
            root
        }
        Err(_) => trimmed.to_string(),
    }
}

fn feed_fallback_title(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    match Url::parse(trimmed) {
        Ok(parsed) => {
            if let Some(host) = parsed.host_str() {
                host.trim_start_matches("www.").to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => trimmed.to_string(),
    }
}

fn feed_display_title(url: &str, title: Option<&String>) -> String {
    match title {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                feed_fallback_title(url)
            } else {
                value.to_string()
            }
        }
        None => feed_fallback_title(url),
    }
}

fn short_url(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 72 {
        trimmed.to_string()
    } else {
        format!("{}…", &trimmed[..69])
    }
}

#[derive(Debug, Clone)]
enum GuiMessage {
    Synced { new_items: usize },
    Status(String),
    Reload,
    Error(String),
}

pub struct NewsFeedApp {
    db_path: PathBuf,
    sync_interval: Duration,
    rx: Receiver<GuiMessage>,
    tx: Sender<GuiMessage>,

    tray_icon: Option<TrayIcon>,
    tray_enabled: bool,
    tray_init_attempted: bool,

    store: Store,
    feeds: Vec<Feed>,
    entries: Vec<FeedEntry>,
    selected_feed: Option<i64>,
    status: String,
    unread_only: bool,
    search: String,
    add_input: String,
    feed_box_width: f32,
    start_minimized: bool,
    did_apply_window_state: bool,
}

impl NewsFeedApp {
    fn new(db_path: PathBuf, sync_interval_minutes: u64, use_tray: bool, start_minimized: bool) -> Result<Self> {
        let store = Store::open(&db_path)?;
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            db_path,
            sync_interval: Duration::from_secs(sync_interval_minutes.saturating_mul(60)),
            rx,
            tx,
            tray_icon: None,
            tray_enabled: use_tray,
            tray_init_attempted: false,
            store,
            feeds: vec![],
            entries: vec![],
            selected_feed: None,
            status: "ready".to_string(),
            unread_only: true,
            search: String::new(),
            add_input: String::new(),
            feed_box_width: 360.0,
            start_minimized,
            did_apply_window_state: false,
        };

        app.refresh_data()?;
        app.spawn_sync_loop();
        Ok(app)
    }

    fn ensure_tray(&mut self) {
        if !self.tray_enabled || self.tray_icon.is_some() || self.tray_init_attempted {
            return;
        }
        self.tray_init_attempted = true;

        let has_display = env::var_os("DISPLAY").is_some() || env::var_os("WAYLAND_DISPLAY").is_some();
        if !has_display {
            self.status = "tray disabled: no DISPLAY/WAYLAND_DISPLAY".to_string();
            return;
        }

        if !gtk::is_initialized() {
            if let Err(err) = gtk::init() {
                self.status = format!("tray disabled: GTK init failed ({err})");
                self.tray_enabled = false;
                return;
            }
        }

        match panic::catch_unwind(panic::AssertUnwindSafe(|| self.create_tray_icon())) {
            Ok(Ok(tray)) => self.tray_icon = Some(tray),
            Ok(Err(err)) => {
                self.status = format!("tray unavailable: {err}");
            }
            Err(_) => {
                self.status = "tray unavailable: GTK/AppIndicator initialization failed".to_string();
            }
        }
    }

    fn icon_for_tray() -> Result<Icon> {
        const SIZE: u32 = 16;
        const TOTAL_PIXELS: usize = (SIZE * SIZE * 4) as usize;

        let mut bytes = vec![0u8; TOTAL_PIXELS];

        for y in 0..SIZE {
            for x in 0..SIZE {
                let idx = ((y * SIZE + x) * 4) as usize;
                if x == 0 || y == 0 || x == SIZE - 1 || y == SIZE - 1 || x == y || x + y == SIZE - 1 {
                    bytes[idx] = 72;
                    bytes[idx + 1] = 165;
                    bytes[idx + 2] = 246;
                    bytes[idx + 3] = 255;
                } else {
                    bytes[idx] = 12;
                    bytes[idx + 1] = 24;
                    bytes[idx + 2] = 48;
                    bytes[idx + 3] = 220;
                }
            }
        }

        Icon::from_rgba(bytes, SIZE, SIZE).context("creating tray icon")
    }

    fn create_tray_icon(&self) -> Result<TrayIcon> {
        let icon = Self::icon_for_tray()?;
        let open_item = MenuItem::with_id(TRAY_ID_OPEN, &format!("Open {APP_NAME}"), true, None);
        let sync_item = MenuItem::with_id(TRAY_ID_SYNC, "Sync now", true, None);
        let quit_item = MenuItem::with_id(TRAY_ID_QUIT, "Quit", true, None);
        let tray_menu = Menu::new();
        tray_menu.append_items(&[&open_item, &sync_item, &quit_item])?;

        let unread = self.store.unread_count(None).unwrap_or(0);
        let tooltip = format!("{APP_NAME} ({unread} unread)");
        let title = if unread == 0 {
            None
        } else {
            Some(unread.to_string())
        };

        TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip(&tooltip)
            .with_title(APP_NAME)
            .with_menu_on_left_click(false)
            .with_icon(icon)
            .build()
            .context("building tray icon")
            .inspect(|tray| {
                if let Some(tray_title) = title {
                    tray.set_title(Some(tray_title));
                }
            })
    }

    fn refresh_tray_badge(&self) {
        let tray = match &self.tray_icon {
            Some(tray) => tray,
            None => return,
        };

        if let Ok(unread) = self.store.unread_count(None) {
            let tooltip = if unread == 0 {
                APP_NAME.to_string()
            } else {
                format!("{APP_NAME} ({unread} unread)")
            };

            let _ = tray.set_tooltip(Some(tooltip));
            let _ = tray.set_visible(true);
            if unread == 0 {
                tray.set_title::<&str>(None);
            } else {
                tray.set_title(Some(unread.to_string()));
            }
        }
    }

    fn handle_window_visibility_commands(&mut self, ctx: &egui::Context) {
        if self.did_apply_window_state {
            return;
        }

        if self.start_minimized {
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
            self.status = "started minimized to tray".to_string();
        } else {
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }

        self.did_apply_window_state = true;
    }

    fn open_window(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
        self.refresh_tray_badge();
    }

    fn spawn_sync_loop(&self) {
        let db_path = self.db_path.clone();
        let tx = self.tx.clone();
        let interval = self.sync_interval;

        let run_sync = move || {
            let client = match Client::builder()
                .user_agent(APP_USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build()
            {
                Ok(client) => client,
                Err(err) => {
                    let _ = tx.send(GuiMessage::Error(err.to_string()));
                    return;
                }
            };

            match Store::open(&db_path) {
                Ok(store) => {
                    let report = syncer::sync_all_feeds(&store, &client);
                    let _ = tx.send(GuiMessage::Synced {
                        new_items: report.new_entries,
                    });
                    if !report.errors.is_empty() {
                        let _ = tx.send(GuiMessage::Error(report.errors.join("; ")));
                    }
                }
                Err(err) => {
                    let _ = tx.send(GuiMessage::Error(err.to_string()));
                }
            }
        };

        thread::spawn(move || {
            run_sync();
            loop {
                thread::sleep(interval);
                run_sync();
            }
        });
    }

    fn refresh_data(&mut self) -> Result<()> {
        self.feeds = self.store.list_feeds()?;
        self.entries = self.store.list_entries(
            self.selected_feed,
            self.unread_only,
            if self.search.is_empty() { None } else { Some(&self.search) },
            300,
            0,
        )?;
        self.refresh_tray_badge();
        Ok(())
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        if !self.tray_enabled || self.tray_icon.is_none() {
            return;
        }

        let tray_events_ok = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            while let Ok(_event) = TrayIconEvent::receiver().try_recv() {
                self.open_window(ctx);
            }
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                match event.id().as_ref() {
                    TRAY_ID_SYNC => {
                        self.sync_now();
                    }
                    TRAY_ID_OPEN => {
                        self.open_window(ctx);
                    }
                    TRAY_ID_QUIT => {
                        process::exit(0);
                    }
                    _ => {}
                }
            }
        }));

        if tray_events_ok.is_err() {
            self.status = "tray event handling failed; disabling tray".to_string();
            self.tray_icon = None;
            self.tray_enabled = false;
        }
    }

    fn sync_now(&self) {
        let db_path = self.db_path.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let client = Client::builder()
                .user_agent(APP_USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build()
                .expect("client");
            if let Ok(store) = Store::open(&db_path) {
                let report = syncer::sync_all_feeds(&store, &client);
                let _ = tx.send(GuiMessage::Synced {
                    new_items: report.new_entries,
                });
            }
        });
    }

    fn sync_feed_now(&self, feed_id: i64) {
        let db_path = self.db_path.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let client = Client::builder()
                .user_agent(APP_USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build()
                .expect("client");
            if let Ok(store) = Store::open(&db_path) {
                match syncer::sync_single_feed(&store, &client, feed_id) {
                    Ok(new_items) => {
                        let _ = tx.send(GuiMessage::Status(format!("{new_items} new item(s) from feed")));
                        let _ = tx.send(GuiMessage::Synced { new_items });
                    }
                    Err(err) => {
                        let _ = tx.send(GuiMessage::Error(err.to_string()));
                    }
                }
            }
        });
    }

    fn remove_feed(&mut self, feed_id: i64) {
        match self.store.remove_feed(feed_id) {
            Ok(removed) => {
                if removed == 0 {
                    self.status = "feed not found".to_string();
                } else {
                    if self.selected_feed == Some(feed_id) {
                        self.selected_feed = None;
                    }
                    let _ = self.refresh_data();
                    self.status = "feed removed".to_string();
                }
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn discover_and_add_feed(&self, source_urls: Vec<String>) {
        let db_path = self.db_path.clone();
        let tx = self.tx.clone();
        let selected_client = Client::builder()
            .user_agent(APP_USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("client");

        thread::spawn(move || {
            let mut added_count = 0usize;
            let mut attempts = 0usize;

            for source_url in source_urls {
                let url = source_url.trim().to_string();
                if url.is_empty() {
                    continue;
                }
                attempts += 1;

                let candidates = match discovery::discover_feed_urls(&url, &selected_client) {
                    Ok(found) => found.feeds,
                    Err(err) => {
                        let _ = tx.send(GuiMessage::Status(format!(
                            "warn: discovery failed for {url}: {err}"
                        )));
                        vec![discovery::normalize_url(&url).unwrap_or_else(|_| url.clone())]
                    }
                };

                if let Ok(store) = Store::open(&db_path) {
                    for candidate in candidates {
                        if let Ok(feed) = store.add_feed(
                            &candidate,
                            Some(feed_fallback_title(&candidate)),
                            Some(url.clone()),
                        ) {
                            let _ = syncer::sync_single_feed(&store, &selected_client, feed.id);
                            added_count += 1;
                        }
                    }
                }
            }

            if attempts == 0 {
                let _ = tx.send(GuiMessage::Error("enter at least one url".into()));
            } else if added_count == 0 {
                let _ = tx.send(GuiMessage::Status("no new feeds added".into()));
            } else {
                let _ = tx.send(GuiMessage::Status(format!(
                    "{added_count} feed(s) added/updated"
                )));
            }
            let _ = tx.send(GuiMessage::Reload);
        });
    }

    fn export_feeds_to_file(&self, output: PathBuf) {
        let db_path = self.db_path.clone();
        let tx = self.tx.clone();

        thread::spawn(move || {
            let store = match Store::open(&db_path) {
                Ok(store) => store,
                Err(err) => {
                    let _ = tx.send(GuiMessage::Error(format!("export open db failed: {err}")));
                    return;
                }
            };

            let mut seen: HashSet<String> = HashSet::new();
            let mut lines: Vec<String> = Vec::new();

            match store.list_feeds() {
                Ok(feeds) => {
                    for feed in feeds {
                        let source = feed
                            .site_url
                            .unwrap_or_else(|| canonical_site_url(&feed.url));
                        let key = source.to_lowercase();
                        if seen.insert(key) {
                            lines.push(source);
                        }
                    }
                }
                Err(err) => {
                    let _ = tx.send(GuiMessage::Error(err.to_string()));
                    return;
                }
            }

            let payload = if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            };

            match fs::write(&output, payload) {
                Ok(()) => {
                    let _ = tx.send(GuiMessage::Status(format!(
                        "exported {} sites to {}",
                        lines.len(),
                        output.display()
                    )));
                }
                Err(err) => {
                    let _ = tx.send(GuiMessage::Error(format!(
                        "export failed to {}: {err}",
                        output.display()
                    )));
                }
            }
        });
    }

    fn import_feeds_from_file(&self, input: PathBuf) {
        let db_path = self.db_path.clone();
        let tx = self.tx.clone();
        let client = match Client::builder()
            .user_agent(APP_USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                let _ = self.tx.send(GuiMessage::Error(format!("import init failed: {err}")));
                return;
            }
        };

        thread::spawn(move || {
            let raw = match fs::read_to_string(&input) {
                Ok(raw) => raw,
                Err(err) => {
                    let _ = tx.send(GuiMessage::Error(format!(
                        "import read failed for {}: {err}",
                        input.display()
                    )));
                    return;
                }
            };

            let store = match Store::open(&db_path) {
                Ok(store) => store,
                Err(err) => {
                    let _ = tx.send(GuiMessage::Error(format!("import open db failed: {err}")));
                    return;
                }
            };

            let mut attempted = 0usize;
            let mut added = 0usize;
            let mut fallback = 0usize;

            for line in raw.lines() {
                let source = line.trim();
                if source.is_empty() || source.starts_with('#') {
                    continue;
                }

                attempted += 1;
                let discovered = match discovery::discover_feed_urls(source, &client) {
                    Ok(found) => found.feeds,
                    Err(err) => {
                        let _ = tx.send(GuiMessage::Status(format!(
                            "warn: discovery failed for {source}: {err}"
                        )));
                        fallback += 1;
                        match discovery::normalize_url(source) {
                            Ok(normalized) => vec![normalized],
                            Err(_) => Vec::new(),
                        }
                    }
                };

                if discovered.is_empty() {
                    continue;
                }

                for feed_url in discovered {
                    if let Ok(feed) = store.add_feed(&feed_url, None, Some(source.to_string())) {
                        let _ = syncer::sync_single_feed(&store, &client, feed.id);
                        added += 1;
                    }
                }
            }

            if attempted == 0 {
                let _ = tx.send(GuiMessage::Status("no import sites found".to_string()));
            } else {
                let failed = attempted.saturating_sub(added);
                let _ = tx.send(GuiMessage::Status(format!(
                    "import complete: {attempted} site(s) processed, {added} feed URL(s) added/updated, {fallback} fallback URLs, {failed} failed"
                )));
            }
            let _ = tx.send(GuiMessage::Reload);
        });
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.style_mut().spacing.item_spacing = egui::vec2(8.0, 8.0);
        ui.vertical(|ui| {
            let mut selected = self.selected_feed;
            let mut sync_feed_id: Option<i64> = None;
            let mut remove_feed_id: Option<i64> = None;

            ui.separator();
            ui.heading(RichText::new("Feeds").size(22.0).color(TEXT_BRIGHT));
            ui.label(
                RichText::new("Tip: click a feed, then choose sync / remove.")
                    .size(10.5)
                    .color(TEXT_MUTED),
            );
            ui.separator();

            let handle_width = 10.0;
            let available_sidebar_width = ui.available_width();
            let max_width = (available_sidebar_width - handle_width).max(220.0);
            let mut preferred_width = self.feed_box_width.clamp(220.0, max_width);
            let feed_section_reserve = 140.0;
            let available_sidebar_height = ui.available_height();
            let feed_box_height = (available_sidebar_height - feed_section_reserve).clamp(180.0, available_sidebar_height);

            ui.horizontal(|ui| {
                let outer_box = {
                    let frame = egui::Frame::group(ui.style());
                    frame.show(ui, |ui| {
                        ui.set_min_width(preferred_width);
                        ui.set_max_width(preferred_width);
                        ui.set_min_height(feed_box_height);
                        ui.set_max_height(feed_box_height);
                        ui.vertical(|ui| {
                            ui.add_space(6.0);
                            ui.label(RichText::new("Tracked feeds").size(15.0).color(TEXT_BRIGHT));
                            ui.separator();

                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if self.feeds.is_empty() {
                                        ui.label(
                                            RichText::new("No feeds added yet.")
                                                .size(11.5)
                                                .color(TEXT_MUTED),
                                        );
                                    }

                                    for feed in self.feeds.iter() {
                                        let unread = self.store.unread_count(Some(feed.id)).unwrap_or(0);
                                        let title = feed_display_title(&feed.url, feed.title.as_ref());
                                        let label = format!(
                                            "{}{}",
                                            title,
                                            if unread > 0 {
                                                format!(" ({unread})")
                                            } else {
                                                String::new()
                                            }
                                        );
                                        let feed_source = feed
                                            .site_url
                                            .clone()
                                            .unwrap_or_else(|| canonical_site_url(&feed.url));

                                        let row_width = (preferred_width - 20.0).max(200.0);
                                        let frame = egui::Frame::group(ui.style());
                                        frame.show(ui, |ui| {
                                            ui.set_min_width(row_width);
                                            ui.set_max_width(row_width);

                                            ui.horizontal(|ui| {
                                                if ui
                                                    .selectable_label(
                                                        selected == Some(feed.id),
                                                        RichText::new(&label).color(TEXT_BRIGHT),
                                                    )
                                                    .clicked()
                                                {
                                                    selected = Some(feed.id);
                                                }
                                                if ui.small_button(RichText::new("sync").color(TEXT_BRIGHT)).clicked() {
                                                    sync_feed_id = Some(feed.id);
                                                }
                                                if ui.small_button(RichText::new("remove").color(Color32::from_rgb(255, 184, 184))).clicked() {
                                                    remove_feed_id = Some(feed.id);
                                                }
                                            });
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .hyperlink_to(
                                                        RichText::new(format!("feed: {}", short_url(&feed.url)))
                                                            .size(10.5)
                                                            .color(TEXT_BRIGHT)
                                                            .underline(),
                                                        feed.url.as_str(),
                                                    )
                                                    .on_hover_text(&feed.url)
                                                    .clicked()
                                                {
                                                    let _ = webbrowser::open(&feed.url);
                                                }
                                            });
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .hyperlink_to(
                                                        RichText::new(format!("site: {}", short_url(&feed_source)))
                                                            .size(10.5)
                                                            .color(TEXT_BRIGHT)
                                                            .underline(),
                                                        feed_source.as_str(),
                                                    )
                                                    .on_hover_text(&feed_source)
                                                    .clicked()
                                                {
                                                    let _ = webbrowser::open(&feed_source);
                                                }
                                            });
                                        });
                                    }
                                });
                        });
                    })
                };

                let frame_rect = outer_box.response.rect;
                let handle_rect = egui::Rect::from_min_max(
                    egui::pos2(frame_rect.right(), frame_rect.top()),
                    egui::pos2(frame_rect.right() + handle_width, frame_rect.bottom()),
                );
                let handle_response = ui.allocate_rect(handle_rect, egui::Sense::drag());
                let handle_hovered = handle_response.hovered() || handle_response.dragged();
                if handle_hovered {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
                let handle_base = Color32::from_rgb(68, 78, 96);
                let handle_hover = Color32::from_rgb(240, 247, 255);
                let handle_color = if handle_hovered { handle_hover } else { handle_base };
                if handle_response.dragged() {
                    preferred_width += handle_response.drag_delta().x;
                    preferred_width = preferred_width.clamp(220.0, max_width);
                }

                let painter = ui.painter_at(handle_rect);
                let x = handle_rect.center().x;
                painter.line_segment(
                    [egui::pos2(x, handle_rect.top()), egui::pos2(x, handle_rect.bottom())],
                    Stroke::new(1.5, handle_color),
                );
                painter.line_segment(
                    [egui::pos2(x - 2.0, handle_rect.center().y - 4.0), egui::pos2(x - 2.0, handle_rect.center().y + 4.0)],
                    Stroke::new(1.0, handle_color),
                );
                painter.line_segment(
                    [egui::pos2(x + 2.0, handle_rect.center().y - 4.0), egui::pos2(x + 2.0, handle_rect.center().y + 4.0)],
                    Stroke::new(1.0, handle_color),
                );
            });
            self.feed_box_width = preferred_width;

            if let Some(feed_id) = sync_feed_id {
                self.sync_feed_now(feed_id);
            }
            if let Some(feed_id) = remove_feed_id {
                self.remove_feed(feed_id);
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Sync all feeds").color(TEXT_BRIGHT)).clicked() {
                    self.sync_now();
                }
                if ui.button(RichText::new("Show all feeds").color(TEXT_BRIGHT)).clicked() {
                    selected = None;
                }
            });

            ui.separator();
            ui.heading(RichText::new("Add feed").size(18.0).color(TEXT_BRIGHT));
            ui.add(
                egui::TextEdit::multiline(&mut self.add_input)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text("https://blog.example.com\nhttps://site.example.com/feed, https://another.com"),
            );
            if ui.button(RichText::new("Add").color(TEXT_BRIGHT)).clicked() {
                let raw_input = self.add_input.trim().to_string();
                let urls = raw_input
                    .split(|c: char| c == '\n' || c == ',' || c == ';')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>();
                if urls.is_empty() {
                    self.status = "enter at least one url".to_string();
                } else {
                    self.add_input.clear();
                    self.discover_and_add_feed(urls);
                }
            }

            ui.horizontal(|ui| {
                if ui.button(RichText::new("Export sites…").color(TEXT_BRIGHT)).clicked() {
                    if let Some(path) = FileDialog::new().set_file_name("nimbus-feeds.txt").save_file() {
                        self.export_feeds_to_file(path);
                    }
                }
                if ui.button(RichText::new("Import sites…").color(TEXT_BRIGHT)).clicked() {
                    if let Some(path) = FileDialog::new().pick_file() {
                        self.import_feeds_from_file(path);
                    }
                }
            });

            if selected != self.selected_feed {
                self.selected_feed = selected;
                if let Err(err) = self.refresh_data() {
                    self.status = err.to_string();
                }
            }
        });
    }

    fn render_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new(APP_NAME).size(24.0).color(TEXT_BRIGHT));
            ui.separator();
            ui.label(RichText::new(format!("interval: {}m", self.sync_interval.as_secs() / 60)).color(TEXT_MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("Sync now").color(TEXT_BRIGHT)).clicked() {
                    self.sync_now();
                }
            });
            ui.label(RichText::new(format!("unread: {}", self.store.unread_count(None).unwrap_or(0))).color(ACCENT));
        });
        ui.label(RichText::new(&self.status).size(12.0).color(ACCENT));
    }

    fn render_entry_list(&mut self, ui: &mut egui::Ui) {
        let mut mark_all_ids: Vec<i64> = Vec::new();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Articles").size(18.0).color(TEXT_BRIGHT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("Mark visible as read").color(TEXT_BRIGHT)).clicked() {
                    mark_all_ids = self
                        .entries
                        .iter()
                        .filter(|entry| !entry.read)
                        .map(|entry| entry.id)
                        .collect();
                }
            });
        });

        if !mark_all_ids.is_empty() {
            let mut marked = 0usize;
            for id in mark_all_ids.drain(..) {
                if self.store.mark_read(id, true).is_ok() {
                    marked += 1;
                }
            }
            if let Err(err) = self.refresh_data() {
                self.status = err.to_string();
            } else {
                self.status = format!("{marked} entries marked read");
            }
        }

        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.entries.is_empty() {
                ui.label(
                    RichText::new(if self.unread_only {
                        "No unread entries."
                    } else {
                        "No entries found."
                    })
                    .size(16.0)
                    .color(TEXT_MUTED),
                );
                return;
            }

            for entry in self.entries.iter_mut() {
                let is_unread = !entry.read;
                let source = entry
                    .feed_title
                    .clone()
                    .unwrap_or_else(|| feed_fallback_title(entry.feed_url.as_deref().unwrap_or(&entry.link)));
                let source_url = canonical_site_url(entry.feed_url.as_deref().unwrap_or(&entry.link));
                let title_color = if is_unread { TITLE_UNREAD } else { TITLE_READ };
                let summary = if entry.summary.is_empty() {
                    strip_html_tags(&entry.content)
                } else {
                    strip_html_tags(&entry.summary)
                };

                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(if is_unread { "unread" } else { "read" })
                                .size(10.0)
                                .color(if is_unread { TITLE_UNREAD } else { TEXT_MUTED }),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(storage::format_time(entry.published_at.or(Some(entry.inserted_at))))
                                .size(10.0)
                                .color(TEXT_MUTED),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if entry.read { "mark unread" } else { "mark read" };
                            if ui
                                .button(RichText::new(label).color(TEXT_BRIGHT))
                                .clicked()
                            {
                                let _ = self.store.mark_read(entry.id, !entry.read);
                                entry.read = !entry.read;
                            }
                            if ui.button(RichText::new("open").color(ACCENT)).clicked() {
                                let _ = webbrowser::open(&entry.link);
                            }
                        });
                    });

                    ui.label(
                        RichText::new(&entry.title)
                            .size(20.0)
                            .color(title_color)
                            .font(FontId::proportional(20.0)),
                    );

                    ui.horizontal(|ui| {
                        if ui
                            .hyperlink_to(
                                RichText::new(format!(" [{}] ", source))
                                    .size(10.5)
                                    .color(TEXT_BRIGHT)
                                    .underline(),
                                source_url.clone(),
                            )
                            .on_hover_text(source_url.clone())
                            .clicked()
                        {
                            let _ = webbrowser::open(&source_url);
                        }
                        ui.separator();
                        ui.label(
                            RichText::new(storage::format_time(entry.published_at.or(Some(entry.inserted_at))))
                                .size(10.5)
                                .color(TEXT_MUTED),
                        );
                    });
                    ui.hyperlink_to(
                        RichText::new(entry.link.clone())
                            .size(10.0)
                            .color(TEXT_BRIGHT)
                            .underline(),
                        entry.link.clone(),
                    );

                    if !summary.is_empty() {
                        ui.label(RichText::new(summary).size(13.0).color(TEXT_MUTED));
                    }
                });
            }
        });
    }

    fn render_footer(&self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(
            RichText::new(format!(
                "{} entries shown | {}",
                self.entries.len(),
                if self.unread_only { "unread only" } else { "all entries" }
            ))
            .size(11.0)
            .color(TEXT_MUTED),
        );
    }
}

impl eframe::App for NewsFeedApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_tray();
        self.handle_window_visibility_commands(ctx);
        self.handle_tray_events(ctx);

        while let Ok(message) = self.rx.try_recv() {
            match message {
                GuiMessage::Synced { new_items } => {
                    let _ = self.refresh_data();
                    if new_items > 0 {
                        let _ = Notification::new()
                            .summary(APP_NAME)
                            .body(&format!("{} new item(s)", new_items))
                            .show();
                        self.status = format!("last sync: {} new item(s)", new_items);
                    } else {
                        self.status = "last sync: no new items".to_string();
                    }
                }
                GuiMessage::Reload => {
                    let _ = self.refresh_data();
                }
                GuiMessage::Status(status) => {
                    self.status = status;
                }
                GuiMessage::Error(err) => {
                    self.status = err;
                }
            }
        }

        let mut visuals = egui::Visuals::dark();
        visuals.window_rounding = Rounding::same(12.0);
        visuals.widgets.active.bg_fill = BG_PANEL_SOFT;
        visuals.widgets.inactive.bg_fill = BG_PANEL;
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 89, 132);
        visuals.widgets.active.fg_stroke.color = TEXT_BRIGHT;
        visuals.widgets.inactive.fg_stroke.color = TEXT_BRIGHT;
        visuals.widgets.hovered.fg_stroke.color = TEXT_BRIGHT;
        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke.color = ACCENT;
        visuals.window_fill = BG_WINDOW;
        visuals.extreme_bg_color = BG_EXTREME;
        ctx.set_visuals(visuals);

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            self.render_header(ui);
        });

        egui::SidePanel::left("left_panel")
            .default_width(340.0)
            .show(ctx, |ui| {
                self.render_sidebar(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_entry_list(ui);
            self.render_footer(ui);
        });

        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

pub fn run_gui(db_path: &Path, interval: u64, use_tray: bool, start_minimized: bool) -> Result<()> {
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_visible(!start_minimized),
        ..Default::default()
    };
    let path = db_path.to_path_buf();
    let app = NewsFeedApp::new(path, interval, use_tray, start_minimized)?;
    eframe::run_native(
        APP_NAME,
        native,
        Box::new(move |_cc| {
            Box::new(app)
        }),
    )
    .map_err(|err| anyhow::anyhow!("running GUI: {err}"))?;
    Ok(())
}
