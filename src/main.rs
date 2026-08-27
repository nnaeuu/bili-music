#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // release 下隐藏控制台窗口

mod db;
mod import;
mod media;
mod player;
mod store;
mod watch;

use eframe::egui;
use player::{PlayMode, Player};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() -> eframe::Result<()> {
    // 命令行模式：--prepare 导入并提取音频后退出（用于打包前准备）
    if std::env::args().any(|a| a == "--prepare") {
        prepare_cli();
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([800.0, 520.0])
            .with_title("Bili Music"),
        ..Default::default()
    };

    eframe::run_native(
        "Bili Music",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// 加载系统中文字体。
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let candidates = [
        ("msyh", "C:\\Windows\\Fonts\\msyh.ttc"),
        ("simhei", "C:\\Windows\\Fonts\\simhei.ttf"),
        ("simsun", "C:\\Windows\\Fonts\\simsun.ttc"),
    ];

    for (name, path) in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                name.to_owned(),
                Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .push(name.to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push(name.to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

struct App {
    conn: Connection,

    // 导入
    cache_root: String,
    status: String,

    // 数据
    tracks: Vec<store::Track>,
    uploaders: Vec<String>,
    tags: Vec<store::Tag>,
    playlists: Vec<store::Playlist>,

    // 筛选
    filter_uploader: Option<String>,
    filter_tag: Option<i64>,

    // 选中
    selected: Option<i64>,

    // 编辑
    editing: Option<store::Track>,
    edit_title: String,
    edit_uploader: String,

    // 输入
    new_tag: String,
    new_playlist: String,

    // 播放器
    player: Player,
    current_id: Option<i64>,

    // 监听
    watch: watch::WatchHandle,
    last_change: Option<Instant>,
    pending_import: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);

        let db_path = db::default_db_path();
        let conn = db::init_db(&db_path).expect("初始化数据库失败");

        let cache_root = db::get_setting(&conn, "cache_root").unwrap_or_else(default_cache_root);
        let watch = watch::WatchHandle::start(PathBuf::from(&cache_root));

        // 启动时校验失效文件
        let cleaned = store::clean_missing(&conn).unwrap_or(0);
        let status = if cleaned > 0 {
            format!("启动时清理了 {cleaned} 条失效记录")
        } else {
            "就绪".to_string()
        };

        let mut app = Self {
            conn,
            cache_root,
            status,
            tracks: vec![],
            uploaders: vec![],
            tags: vec![],
            playlists: vec![],
            filter_uploader: None,
            filter_tag: None,
            selected: None,
            editing: None,
            edit_title: String::new(),
            edit_uploader: String::new(),
            new_tag: String::new(),
            new_playlist: String::new(),
            player: Player::new().expect("音频初始化失败"),
            current_id: None,
            watch,
            last_change: None,
            pending_import: false,
        };

        app.reload();
        app
    }

    /// 重新从数据库加载列表、UP主、标签、歌单。
    fn reload(&mut self) {
        self.tracks = store::filter_tracks(
            &self.conn,
            self.filter_uploader.as_deref(),
            self.filter_tag,
        )
        .unwrap_or_default();
        self.uploaders = store::list_uploaders(&self.conn).unwrap_or_default();
        self.tags = store::list_tags(&self.conn).unwrap_or_default();
        self.playlists = store::list_playlists(&self.conn).unwrap_or_default();
    }

    /// 扫描缓存目录导入，并提取音频。
    fn do_import(&mut self) {
        let root = Path::new(self.cache_root.trim());
        self.watch.set_path(root.to_path_buf());
        let _ = db::set_setting(&self.conn, "cache_root", self.cache_root.trim());
        let import_msg = match import::import_from_dir(&self.conn, root) {
            Ok(r) => format!(
                "导入：扫描 {}，新增 {}，跳过 {}，失败 {}",
                r.scanned, r.imported, r.skipped, r.failed
            ),
            Err(e) => format!("导入失败：{e}"),
        };
        let media_msg = match media::process_all(&self.conn, &db::audio_dir()) {
            Ok(r) => format!(
                "提取：成功 {}，DRM {}，失败 {}",
                r.processed, r.skipped_drm, r.failed
            ),
            Err(e) => format!("提取失败：{e}"),
        };
        self.status = format!("{import_msg}；{media_msg}");
        self.reload();
    }

    fn open_edit(&mut self, track: &store::Track) {
        self.edit_title = track.title.clone();
        self.edit_uploader = track.uploader.clone();
        self.editing = Some(track.clone());
    }

    fn save_edit(&mut self) {
        if let Some(t) = &self.editing {
            match store::update_track(
                &self.conn,
                t.id,
                self.edit_title.trim(),
                self.edit_uploader.trim(),
            ) {
                Ok(_) => self.status = "已保存".to_string(),
                Err(e) => self.status = format!("保存失败：{e}"),
            }
            self.editing = None;
            self.reload();
        }
    }

    // ---- 播放器 ----

    fn current_track(&self) -> Option<&store::Track> {
        self.current_id
            .and_then(|id| self.tracks.iter().find(|x| x.id == id))
    }

    fn play_track(&mut self, id: i64) {
        let Some(track) = self.tracks.iter().find(|x| x.id == id).cloned() else {
            return;
        };
        match self.player.play_file(&db::resolve_path(&track.file_path)) {
            Ok(_) => {
                self.current_id = Some(id);
                self.selected = Some(id);
                self.status = format!("正在播放：{}", track.title);
            }
            Err(e) => {
                self.current_id = None;
                self.status = format!("播放失败：{e}");
            }
        }
    }

    /// 播放按钮：有曲目则切换暂停，否则播放选中项。
    fn play_or_toggle(&mut self) {
        if self.current_id.is_some() && self.player.has_loaded() && !self.player.is_finished() {
            self.player.toggle_pause();
        } else if let Some(id) = self.selected {
            self.play_track(id);
        }
    }

    fn next_track(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        let len = self.tracks.len();
        let idx = self
            .current_id
            .and_then(|id| self.tracks.iter().position(|x| x.id == id));
        let next = match idx {
            Some(i) => (i + 1) % len,
            None => 0,
        };
        let id = self.tracks[next].id;
        self.play_track(id);
    }

    fn prev_track(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        let len = self.tracks.len();
        let idx = self
            .current_id
            .and_then(|id| self.tracks.iter().position(|x| x.id == id));
        let prev = match idx {
            Some(i) => (i + len - 1) % len,
            None => 0,
        };
        let id = self.tracks[prev].id;
        self.play_track(id);
    }

    fn random_track_id(&self) -> Option<i64> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0);
        self.tracks.get(nanos % len).map(|t| t.id)
    }

    /// 播放结束后按模式自动切换。
    fn auto_advance(&mut self) {
        if !self.player.is_finished() || self.current_id.is_none() {
            return;
        }
        let idx = self
            .current_id
            .and_then(|id| self.tracks.iter().position(|x| x.id == id));
        match self.player.mode() {
            PlayMode::Sequential => match idx {
                Some(i) if i + 1 < self.tracks.len() => {
                    let id = self.tracks[i + 1].id;
                    self.play_track(id);
                }
                _ => self.current_id = None,
            },
            PlayMode::Loop => {
                let next = idx.map(|i| (i + 1) % self.tracks.len()).unwrap_or(0);
                let id = self.tracks[next].id;
                self.play_track(id);
            }
            PlayMode::Random => {
                if let Some(id) = self.random_track_id() {
                    self.play_track(id);
                }
            }
        }
    }

    /// 处理监听事件：防抖后自动增量导入。
    fn handle_watch(&mut self) {
        if self.watch.has_change() {
            self.last_change = Some(Instant::now());
            self.pending_import = true;
        }
        if self.pending_import {
            if let Some(t) = self.last_change {
                if t.elapsed() >= Duration::from_secs(3) {
                    self.pending_import = false;
                    self.last_change = None;
                    self.do_import();
                }
            }
        }
    }

    // ---- UI ----

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("缓存目录：");
                ui.add(egui::TextEdit::singleline(&mut self.cache_root).desired_width(420.0));
                if ui.button("导入").clicked() {
                    self.do_import();
                }
                if ui.button("刷新").clicked() {
                    self.reload();
                }
                if ui.button("清理失效").clicked() {
                    match store::clean_missing(&self.conn) {
                        Ok(n) => {
                            self.status = if n > 0 {
                                format!("清理了 {n} 条失效记录")
                            } else {
                                "无失效记录".to_string()
                            };
                            self.reload();
                        }
                        Err(e) => self.status = format!("清理失败：{e}"),
                    }
                }
            });
            ui.add_space(4.0);
            ui.label(&self.status);
            ui.add_space(6.0);
        });
    }

    fn side_bar(&mut self, ctx: &egui::Context) {
        let uploaders = self.uploaders.clone();
        let tags = self.tags.clone();
        let playlists = self.playlists.clone();

        let mut clicked_all = false;
        let mut clicked_up: Option<String> = None;
        let mut clicked_tag: Option<i64> = None;
        let mut create_pl = false;

        egui::SidePanel::left("side")
            .resizable(true)
            .default_width(180.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.strong("筛选");
                let show_all = self.filter_uploader.is_none() && self.filter_tag.is_none();
                if ui.selectable_label(show_all, "全部").clicked() {
                    clicked_all = true;
                }

                ui.separator();
                ui.strong("UP主");
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for up in &uploaders {
                            let selected = self.filter_uploader.as_deref() == Some(up.as_str());
                            if ui.selectable_label(selected, up).clicked() {
                                clicked_up = Some(up.clone());
                            }
                        }
                    });

                ui.separator();
                ui.strong("标签");
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for tag in &tags {
                            let selected = self.filter_tag == Some(tag.id);
                            if ui.selectable_label(selected, &tag.name).clicked() {
                                clicked_tag = Some(tag.id);
                            }
                        }
                    });

                ui.separator();
                ui.strong("歌单");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for pl in &playlists {
                        ui.label(&pl.name);
                    }
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_playlist)
                                .desired_width(110.0)
                                .hint_text("新歌单"),
                        );
                        if ui.button("+").clicked() && !self.new_playlist.trim().is_empty() {
                            create_pl = true;
                        }
                    });
                });
            });

        if clicked_all {
            self.filter_uploader = None;
            self.filter_tag = None;
            self.reload();
        } else if let Some(up) = clicked_up {
            self.filter_uploader = Some(up);
            self.filter_tag = None;
            self.reload();
        } else if let Some(tag) = clicked_tag {
            self.filter_tag = Some(tag);
            self.filter_uploader = None;
            self.reload();
        }
        if create_pl {
            let _ = store::create_playlist(&self.conn, &self.new_playlist);
            self.new_playlist.clear();
            self.reload();
        }
    }

    fn track_list(&mut self, ctx: &egui::Context) {
        let tracks = self.tracks.clone();
        let playlists = self.playlists.clone();
        let selected_id = self.selected;

        let mut select_clicked: Option<i64> = None;
        let mut play_clicked: Option<i64> = None;
        let mut want_edit = false;
        let mut add_tag_clicked = false;
        let mut add_to_pl: Option<i64> = None;

        egui::CentralPanel::default().show(ctx, |ui| {
            if tracks.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("暂无曲目，请在上方选择缓存目录并点击「导入」");
                });
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("tracks")
                        .striped(true)
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            ui.strong("标题");
                            ui.strong("UP主");
                            ui.strong("BV号");
                            ui.strong("时长");
                            ui.end_row();

                            for t in &tracks {
                                let sel = selected_id == Some(t.id);
                                let resp = ui.selectable_label(sel, &t.title);
                                if resp.double_clicked() {
                                    play_clicked = Some(t.id);
                                } else if resp.clicked() {
                                    select_clicked = Some(t.id);
                                }
                                ui.label(&t.uploader);
                                ui.label(&t.bvid);
                                ui.label(format_duration(t.duration));
                                ui.end_row();
                            }
                        });
                });

            if let Some(id) = selected_id {
                if let Some(t) = tracks.iter().find(|x| x.id == id) {
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("编辑元数据").clicked() {
                            want_edit = true;
                        }
                        ui.label("标签：");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_tag)
                                .desired_width(100.0)
                                .hint_text("新标签"),
                        );
                        if ui.button("打标签").clicked() && !self.new_tag.trim().is_empty() {
                            add_tag_clicked = true;
                        }
                        if let Ok(tags) = store::track_tags(&self.conn, t.id) {
                            for tg in &tags {
                                ui.label(format!("#{}", tg.name));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("加入歌单：");
                        for pl in &playlists {
                            if ui.button(&pl.name).clicked() {
                                add_to_pl = Some(pl.id);
                            }
                        }
                    });
                }
            }
        });

        // 闭包外统一处理
        if let Some(id) = play_clicked {
            self.play_track(id);
        } else if let Some(id) = select_clicked {
            self.selected = Some(id);
        }
        if want_edit {
            if let Some(id) = selected_id {
                if let Some(t) = self.tracks.iter().find(|x| x.id == id).cloned() {
                    self.open_edit(&t);
                }
            }
        }
        if add_tag_clicked {
            if let Some(id) = selected_id {
                let _ = store::add_tag(&self.conn, id, &self.new_tag);
                self.new_tag.clear();
                self.reload();
            }
        }
        if let Some(pl_id) = add_to_pl {
            if let Some(id) = selected_id {
                if let Some(pl) = self.playlists.iter().find(|p| p.id == pl_id) {
                    let _ = store::add_to_playlist(&self.conn, pl_id, id);
                    self.status = format!("已加入歌单「{}」", pl.name);
                }
            }
        }
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("⏮").clicked() {
                    self.prev_track();
                }
                let playing = self.current_id.is_some()
                    && self.player.has_loaded()
                    && !self.player.is_finished()
                    && !self.player.is_paused();
                let label = if playing { "⏸" } else { "▶" };
                if ui.button(label).clicked() {
                    self.play_or_toggle();
                }
                if ui.button("⏭").clicked() {
                    self.next_track();
                }

                ui.separator();

                // 进度
                let total = self
                    .current_track()
                    .and_then(|t| t.duration)
                    .unwrap_or(0.0);
                let pos = self.player.position().as_secs_f64().min(total);
                ui.label(format!(
                    "{} / {}",
                    format_duration(Some(pos)),
                    format_duration(Some(total))
                ));
                if total > 0.0 {
                    let mut p = pos;
                    if ui
                        .add(egui::Slider::new(&mut p, 0.0..=total).show_value(false))
                        .changed()
                    {
                        self.player.seek(Duration::from_secs_f64(p));
                    }
                }

                ui.separator();

                // 音量
                ui.label("🔊");
                let mut vol = self.player.volume();
                if ui
                    .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    self.player.set_volume(vol);
                }

                ui.separator();

                // 播放模式
                let modes = [
                    (PlayMode::Sequential, "顺序"),
                    (PlayMode::Loop, "循环"),
                    (PlayMode::Random, "随机"),
                ];
                for (m, label) in modes {
                    if ui.selectable_label(self.player.mode() == m, label).clicked() {
                        self.player.set_mode(m);
                    }
                }
            });
            ui.add_space(6.0);
        });
    }

    fn edit_window(&mut self, ctx: &egui::Context) {
        let Some(track) = self.editing.clone() else { return; };
        let mut open = true;
        egui::Window::new("编辑元数据")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("BV号：{}", track.bvid));
                ui.label("标题");
                ui.add(egui::TextEdit::singleline(&mut self.edit_title).desired_width(360.0));
                ui.label("UP主");
                ui.add(egui::TextEdit::singleline(&mut self.edit_uploader).desired_width(360.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        self.save_edit();
                    }
                    if ui.button("取消").clicked() {
                        self.editing = None;
                    }
                });
            });
        if !open {
            self.editing = None;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.auto_advance();
        self.handle_watch();
        self.top_bar(ctx);
        self.side_bar(ctx);
        self.track_list(ctx);
        self.bottom_bar(ctx);
        self.edit_window(ctx);
    }
}

/// 默认 B 站缓存路径：C:\Users\<当前用户名>\Videos\bilibili。
/// 动态获取当前用户名，避免硬编码暴露隐私。
fn default_cache_root() -> String {
    std::env::var("USERPROFILE")
        .map(|p| format!("{}\\Videos\\bilibili", p))
        .unwrap_or_else(|_| "C:\\Videos\\bilibili".to_string())
}

/// 打包前准备：导入缓存并提取音频，然后退出。
fn prepare_cli() {
    let db_path = db::default_db_path();
    let conn = db::init_db(&db_path).expect("初始化数据库失败");
    let root = default_cache_root();
    println!("缓存目录：{root}");
    match import::import_from_dir(&conn, Path::new(&root)) {
        Ok(r) => println!(
            "导入：扫描 {}，新增 {}，跳过 {}，失败 {}",
            r.scanned, r.imported, r.skipped, r.failed
        ),
        Err(e) => println!("导入失败：{e}"),
    }
    match media::process_all(&conn, &db::audio_dir()) {
        Ok(r) => println!(
            "提取：成功 {}，DRM {}，失败 {}",
            r.processed, r.skipped_drm, r.failed
        ),
        Err(e) => println!("提取失败：{e}"),
    }
}

/// 把时长（秒）格式化为 `分:秒`。
fn format_duration(d: Option<f64>) -> String {
    match d {
        Some(s) => {
            let s = s.round() as i64;
            format!("{}:{:02}", s / 60, s % 60)
        }
        None => "--:--".to_string(),
    }
}
