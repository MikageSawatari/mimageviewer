use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use eframe::egui;
use rayon::ThreadPool;

const THUMB_SIZE: u32 = 256;
const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp"];

/// サムネイルロードに使うスレッド数の上限（CPU負荷を抑える）
const THUMB_THREADS: usize = 4;

// -----------------------------------------------------------------------
// データモデル
// -----------------------------------------------------------------------

#[derive(Clone)]
pub enum GridItem {
    Folder(PathBuf),
    Image(PathBuf),
}

impl GridItem {
    fn path(&self) -> &Path {
        match self {
            GridItem::Folder(p) | GridItem::Image(p) => p,
        }
    }

    fn name(&self) -> &str {
        self.path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    }
}

pub enum ThumbnailState {
    Pending,
    Loaded(egui::TextureHandle),
    Failed,
}

// -----------------------------------------------------------------------
// App
// -----------------------------------------------------------------------

pub struct App {
    address: String,
    current_folder: Option<PathBuf>,
    items: Vec<GridItem>,
    thumbnails: Vec<ThumbnailState>,
    selected: Option<usize>,
    grid_cols: usize,
    tx: mpsc::Sender<(usize, egui::ColorImage)>,
    rx: mpsc::Receiver<(usize, egui::ColorImage)>,
    /// サムネイルロード専用スレッドプール（負荷上限付き）
    thumb_pool: Arc<ThreadPool>,
    /// 前フレームのセルサイズ（行単位スクロール計算に使用）
    last_cell_size: f32,
    /// true のとき次フレームで選択セルにスクロールする
    scroll_to_selected: bool,
}

impl Default for App {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let thumb_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(THUMB_THREADS)
                .build()
                .expect("スレッドプール作成失敗"),
        );
        Self {
            address: String::new(),
            current_folder: None,
            items: Vec::new(),
            thumbnails: Vec::new(),
            selected: None,
            grid_cols: 4,
            tx,
            rx,
            thumb_pool,
            last_cell_size: 200.0,
            scroll_to_selected: false,
        }
    }
}

impl App {
    /// フォルダを開いてアイテム一覧を構築し、並列サムネイルロードを開始する
    pub fn load_folder(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.tx = tx.clone();
        self.rx = rx;

        self.current_folder = Some(path.clone());
        self.address = path.to_string_lossy().to_string();
        self.selected = None;
        self.scroll_to_selected = false;

        let mut folders: Vec<GridItem> = Vec::new();
        let mut images: Vec<GridItem> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    folders.push(GridItem::Folder(p));
                } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if SUPPORTED_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
                        images.push(GridItem::Image(p));
                    }
                }
            }
        }

        folders.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        images.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
        folders.extend(images);

        self.items = folders;
        self.thumbnails = (0..self.items.len())
            .map(|_| ThumbnailState::Pending)
            .collect();

        let image_paths: Vec<(usize, PathBuf)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                GridItem::Image(p) => Some((i, p.clone())),
                GridItem::Folder(_) => None,
            })
            .collect();

        // 専用スレッドプールで並列ロード（THUMB_THREADS 本まで）
        let pool = Arc::clone(&self.thumb_pool);
        std::thread::spawn(move || {
            pool.install(|| {
                use rayon::prelude::*;
                image_paths.par_iter().for_each(|(i, path)| {
                    if let Ok(img) = image::open(path) {
                        let thumb = img.thumbnail(THUMB_SIZE, THUMB_SIZE);
                        let rgba = thumb.to_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let color_image =
                            egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                        let _ = tx.send((*i, color_image));
                    }
                });
            });
        });
    }

    /// 完成サムネイルをテクスチャに登録する
    fn poll_thumbnails(&mut self, ctx: &egui::Context) {
        let mut any_new = false;
        while let Ok((i, color_image)) = self.rx.try_recv() {
            if i < self.thumbnails.len() {
                let handle = ctx.load_texture(
                    format!("thumb_{i}"),
                    color_image,
                    egui::TextureOptions::LINEAR,
                );
                self.thumbnails[i] = ThumbnailState::Loaded(handle);
                any_new = true;
            }
        }
        if any_new {
            ctx.request_repaint();
        }
    }

    /// キーボード操作を処理し、フォルダ移動が必要なら Some(path) を返す
    fn handle_keyboard(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let cols = self.grid_cols.max(1);
        let n = self.items.len();

        let (right, left, down, up, enter, ctrl_up) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp),
            )
        });

        if n > 0 {
            let sel = self.selected.unwrap_or(0);
            let new_sel = if right {
                Some((sel + 1).min(n - 1))
            } else if left {
                Some(sel.saturating_sub(1))
            } else if down {
                Some((sel + cols).min(n - 1))
            } else if up {
                Some(sel.saturating_sub(cols))
            } else {
                None
            };

            if let Some(s) = new_sel {
                self.selected = Some(s);
                self.scroll_to_selected = true; // 選択変更時にスクロール
            }

            if enter {
                if let Some(idx) = self.selected {
                    if let Some(GridItem::Folder(p)) = self.items.get(idx) {
                        return Some(p.clone());
                    }
                }
            }
        }

        if ctrl_up {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(parent) = cur.parent() {
                    return Some(parent.to_path_buf());
                }
            }
        }

        None
    }

    /// マウスホイールを行単位スクロールに変換する
    /// （egui のデフォルトピクセルスクロールを上書きする）
    fn intercept_scroll(&self, ctx: &egui::Context) {
        let cell_size = self.last_cell_size.max(1.0);
        let raw_y = ctx.input(|i| i.raw_scroll_delta.y);
        if raw_y.abs() > 0.5 {
            // 符号だけ取り出し、1行分の高さに置き換える
            let row_delta = raw_y.signum() * cell_size;
            ctx.input_mut(|i| {
                i.raw_scroll_delta.y = row_delta;
                i.smooth_scroll_delta.y = row_delta;
            });
        }
    }
}

// -----------------------------------------------------------------------
// eframe::App 実装
// -----------------------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_thumbnails(ctx);
        self.intercept_scroll(ctx);

        let keyboard_nav = self.handle_keyboard(ctx);

        // ── メニューバー ─────────────────────────────────────────────
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("ファイル", |ui| {
                    if ui.button("終了").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("お気に入り", |ui| {
                    ui.label("（Phase 3 で実装）");
                });
                ui.menu_button("設定", |ui| {
                    ui.horizontal(|ui| {
                        ui.label("サムネイル列数:");
                        ui.add(egui::DragValue::new(&mut self.grid_cols).range(1..=12));
                    });
                });
            });
        });

        // ── アドレスバー ─────────────────────────────────────────────
        let address_nav = egui::TopBottomPanel::top("address_bar")
            .show(ctx, |ui| -> Option<PathBuf> {
                ui.add_space(3.0);
                let mut result = None;
                ui.horizontal(|ui| {
                    ui.label("フォルダ:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.address)
                            .desired_width(f32::INFINITY),
                    );
                    if resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let p = PathBuf::from(&self.address);
                        if p.is_dir() {
                            result = Some(p);
                        }
                    }
                });
                ui.add_space(3.0);
                result
            })
            .inner;

        // ── 左サイドバー ─────────────────────────────────────────────
        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(160.0)
            .show(ctx, |ui| {
                ui.heading("お気に入り");
                ui.separator();
                ui.label("（Phase 3 で実装）");
            });

        // ── サムネイルグリッド ────────────────────────────────────────
        let scroll_to = self.scroll_to_selected;
        self.scroll_to_selected = false;

        let grid_nav = egui::CentralPanel::default()
            .show(ctx, |ui| -> Option<PathBuf> {
                if self.items.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label("フォルダを入力して Enter キーを押してください");
                    });
                    return None;
                }

                let cols = self.grid_cols.max(1);
                let avail_w = ui.available_width();
                let cell_size = (avail_w / cols as f32).floor();
                self.last_cell_size = cell_size;

                let total_rows = self.items.len().div_ceil(cols);
                let total_h = total_rows as f32 * cell_size;

                let mut nav: Option<PathBuf> = None;

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show_viewport(ui, |ui, viewport| {
                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(avail_w, total_h),
                            egui::Sense::hover(),
                        );

                        // 選択セルが見えるようにスクロール
                        if scroll_to {
                            if let Some(sel) = self.selected {
                                let row = sel / cols;
                                let col = sel % cols;
                                let sel_rect = egui::Rect::from_min_size(
                                    content_rect.min
                                        + egui::vec2(
                                            col as f32 * cell_size,
                                            row as f32 * cell_size,
                                        ),
                                    egui::vec2(cell_size, cell_size),
                                );
                                ui.scroll_to_rect(sel_rect, Some(egui::Align::Center));
                            }
                        }

                        let first_row = (viewport.min.y / cell_size) as usize;
                        let last_row =
                            ((viewport.max.y / cell_size) as usize + 2).min(total_rows);

                        for row in first_row..last_row {
                            for col in 0..cols {
                                let idx = row * cols + col;
                                if idx >= self.items.len() {
                                    break;
                                }

                                let cell_rect = egui::Rect::from_min_size(
                                    content_rect.min
                                        + egui::vec2(
                                            col as f32 * cell_size,
                                            row as f32 * cell_size,
                                        ),
                                    egui::vec2(cell_size, cell_size),
                                );

                                let response = ui.interact(
                                    cell_rect,
                                    ui.id().with(idx),
                                    egui::Sense::click(),
                                );
                                if response.clicked() {
                                    self.selected = Some(idx);
                                }
                                if response.double_clicked() {
                                    if let Some(GridItem::Folder(p)) = self.items.get(idx) {
                                        nav = Some(p.clone());
                                    }
                                }

                                draw_cell(
                                    ui,
                                    cell_rect,
                                    self.selected == Some(idx),
                                    &self.items[idx],
                                    &self.thumbnails[idx],
                                );
                            }
                        }
                    });

                nav
            })
            .inner;

        let navigate = keyboard_nav.or(address_nav).or(grid_nav);
        if let Some(p) = navigate {
            self.load_folder(p);
        }
    }
}

// -----------------------------------------------------------------------
// セル描画
// -----------------------------------------------------------------------

fn draw_cell(
    ui: &egui::Ui,
    rect: egui::Rect,
    is_selected: bool,
    item: &GridItem,
    thumb: &ThumbnailState,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }

    let painter = ui.painter();
    let padding = 4.0;
    let inner = rect.shrink(padding);

    // 背景: 白ベース、選択時は青みがかった色
    let bg = if is_selected {
        egui::Color32::from_rgb(180, 210, 255)
    } else {
        egui::Color32::WHITE
    };
    painter.rect_filled(rect, 2.0, bg);

    match item {
        GridItem::Folder(path) => {
            painter.text(
                inner.center() - egui::vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                "📁",
                egui::FontId::proportional(42.0),
                egui::Color32::from_rgb(220, 170, 30),
            );
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            painter.text(
                egui::pos2(inner.center().x, inner.max.y - 4.0),
                egui::Align2::CENTER_BOTTOM,
                truncate_name(name, 18),
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(30),
            );
        }
        GridItem::Image(_) => {
            match thumb {
                ThumbnailState::Loaded(tex) => {
                    let tex_size = tex.size_vec2();
                    let scale =
                        (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
                    let img_size = tex_size * scale;
                    let img_rect =
                        egui::Rect::from_center_size(inner.center(), img_size);
                    painter.image(
                        tex.id(),
                        img_rect,
                        egui::Rect::from_min_max(
                            egui::pos2(0.0, 0.0),
                            egui::pos2(1.0, 1.0),
                        ),
                        egui::Color32::WHITE,
                    );
                    // ファイル名は非表示（設定で切り替え予定）
                }
                ThumbnailState::Pending => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_gray(220));
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "読込中",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(140),
                    );
                }
                ThumbnailState::Failed => {
                    painter.rect_filled(inner, 2.0, egui::Color32::from_rgb(255, 220, 220));
                    painter.text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "読込失敗",
                        egui::FontId::proportional(12.0),
                        egui::Color32::DARK_RED,
                    );
                }
            }
        }
    }

    // ボーダー
    let border = if is_selected {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(60, 120, 220))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_gray(200))
    };
    painter.rect_stroke(rect, 2.0, border, egui::StrokeKind::Middle);
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        name.to_owned()
    } else {
        chars[..max_chars - 1].iter().collect::<String>() + "…"
    }
}
