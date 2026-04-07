use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use eframe::egui;
use rayon::ThreadPool;

const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp"];
const THUMB_THREADS: usize = 8;

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
        self.path().file_name().and_then(|n| n.to_str()).unwrap_or("")
    }
}

pub enum ThumbnailState {
    Pending,
    Loaded(egui::TextureHandle),
    #[allow(dead_code)]
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
    thumb_pool: Arc<ThreadPool>,

    /// スクロールオフセット（行境界にスナップ済み）。自前管理する
    scroll_offset_y: f32,
    /// 前フレームのセルサイズ（スクロール計算に使用）
    last_cell_size: f32,
    /// 前フレームのビューポート高さ（カーソルキースクロールに使用）
    last_viewport_h: f32,
    /// true のとき選択セルが見えるようにオフセットを調整する
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
            scroll_offset_y: 0.0,
            last_cell_size: 200.0,
            last_viewport_h: 600.0,
            scroll_to_selected: false,
        }
    }
}

impl App {
    pub fn load_folder(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.tx = tx.clone();
        self.rx = rx;

        self.current_folder = Some(path.clone());
        self.address = path.to_string_lossy().to_string();
        self.selected = None;
        self.scroll_offset_y = 0.0;
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

        // セルサイズに合わせたサムネイルサイズ（前フレームの値を使用）
        // 4K表示で十分な解像度を確保、最低 512px
        let thumb_px = (self.last_cell_size as u32).max(512).min(1200);

        let image_paths: Vec<(usize, PathBuf)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                GridItem::Image(p) => Some((i, p.clone())),
                GridItem::Folder(_) => None,
            })
            .collect();

        let pool = Arc::clone(&self.thumb_pool);
        std::thread::spawn(move || {
            pool.install(|| {
                use rayon::prelude::*;
                image_paths.par_iter().for_each(|(i, path)| {
                    if let Ok(img) = image::open(path) {
                        let thumb = img.thumbnail(thumb_px, thumb_px);
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
                self.scroll_to_selected = true;
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

    /// マウスホイールイベントを消費し、行単位でスナップしたオフセットに変換する
    fn process_scroll(&mut self, ctx: &egui::Context) {
        let cell_size = self.last_cell_size.max(1.0);

        // マウスホイールイベントだけを取り出し、egui には渡さない
        let scroll_delta_y = ctx.input(|i| i.raw_scroll_delta.y);
        if scroll_delta_y.abs() > 0.5 {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                // MouseWheel イベントも消費
                i.events
                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });
            // 上スクロール(delta>0) → オフセット減、下スクロール(delta<0) → オフセット増
            let direction = -scroll_delta_y.signum();
            self.scroll_offset_y =
                (self.scroll_offset_y + direction * cell_size).max(0.0);
            // 行境界にスナップ
            self.scroll_offset_y =
                (self.scroll_offset_y / cell_size).round() * cell_size;
        }
    }

    /// カーソルキー移動後、選択行がビューポートに収まるようオフセットを調整する
    fn apply_scroll_to_selected(&mut self, cols: usize, cell_size: f32) {
        let sel = match self.selected {
            Some(s) => s,
            None => return,
        };
        let row = sel / cols;
        let row_top = row as f32 * cell_size;
        let row_bottom = row_top + cell_size;
        let vp_top = self.scroll_offset_y;
        let vp_bottom = self.scroll_offset_y + self.last_viewport_h;

        if row_top < vp_top {
            // 選択行が上に隠れている → 選択行が最上行になるようスクロール
            self.scroll_offset_y = row_top;
        } else if row_bottom > vp_bottom {
            // 選択行が下に隠れている → 選択行が最下行になるようスクロール
            self.scroll_offset_y =
                (row_bottom - self.last_viewport_h).max(0.0);
            // 行境界にスナップ
            self.scroll_offset_y =
                (self.scroll_offset_y / cell_size).ceil() * cell_size;
        }
    }
}

// -----------------------------------------------------------------------
// eframe::App 実装
// -----------------------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_thumbnails(ctx);

        // スクロールは egui に触れる前に処理（イベントを消費）
        self.process_scroll(ctx);

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

                // ウィンドウリサイズでセルサイズが変わった場合はスナップし直す
                if (cell_size - self.last_cell_size).abs() > 0.5 {
                    self.scroll_offset_y =
                        (self.scroll_offset_y / cell_size).round() * cell_size;
                    self.last_cell_size = cell_size;
                }

                if scroll_to {
                    self.apply_scroll_to_selected(cols, cell_size);
                }

                let total_rows = self.items.len().div_ceil(cols);
                let total_h = total_rows as f32 * cell_size;

                // スクロール上限
                let max_offset = (total_h - self.last_viewport_h).max(0.0);
                self.scroll_offset_y = self.scroll_offset_y.min(max_offset);

                let mut nav: Option<PathBuf> = None;

                // egui にスクロールを管理させず、自前の offset を毎フレーム注入する
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        // ビューポート高さを記録（次フレームのスクロール計算に使う）
                        self.last_viewport_h = viewport.height();
                        // egui が内部で更新したオフセットは無視し、自前値を維持する
                        // （process_scroll で消費済みなので delta=0 のはず）

                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(avail_w, total_h),
                            egui::Sense::hover(),
                        );

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
        GridItem::Image(_) => match thumb {
            ThumbnailState::Loaded(tex) => {
                let tex_size = tex.size_vec2();
                let scale =
                    (inner.width() / tex_size.x).min(inner.height() / tex_size.y);
                let img_size = tex_size * scale;
                let img_rect = egui::Rect::from_center_size(inner.center(), img_size);
                painter.image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(
                        egui::pos2(0.0, 0.0),
                        egui::pos2(1.0, 1.0),
                    ),
                    egui::Color32::WHITE,
                );
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
        },
    }

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
