use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

use eframe::egui;

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
    /// フォルダ移動時に true にセットすると旧ロードタスクが中断する
    cancel_token: Arc<AtomicBool>,

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
        Self {
            address: String::new(),
            current_folder: None,
            items: Vec::new(),
            thumbnails: Vec::new(),
            selected: None,
            grid_cols: 4,
            tx,
            rx,
            cancel_token: Arc::new(AtomicBool::new(false)),
            scroll_offset_y: 0.0,
            last_cell_size: 200.0,
            last_viewport_h: 600.0,
            scroll_to_selected: false,
        }
    }
}

impl App {
    pub fn load_folder(&mut self, path: PathBuf) {
        crate::logger::log(format!(
            "=== load_folder: {} ===",
            path.display()
        ));

        // ── 旧タスクをキャンセル ──────────────────────────────────────
        self.cancel_token.store(true, Ordering::Relaxed);
        crate::logger::log("  cancel_token -> true (old tasks will stop)");
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_token = Arc::clone(&cancel);

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

        let thumb_px = (self.last_cell_size as u32).max(512).min(1200);

        // ── 可視範囲を計算して優先順に並べる ──────────────────────────
        let cols = self.grid_cols.max(1);
        let cell_size = self.last_cell_size.max(1.0);
        let first_vis_item = (self.scroll_offset_y / cell_size) as usize * cols;
        let vis_rows = (self.last_viewport_h / cell_size).ceil() as usize + 1;
        let last_vis_item = first_vis_item + vis_rows * cols;

        let image_paths: Vec<(usize, PathBuf)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                GridItem::Image(p) => Some((i, p.clone())),
                GridItem::Folder(_) => None,
            })
            .collect();

        let (visible, rest): (Vec<_>, Vec<_>) = image_paths
            .into_iter()
            .partition(|(i, _)| *i >= first_vis_item && *i < last_vis_item);

        crate::logger::log(format!(
            "  thumb_px={thumb_px}  total_images={}  visible={} (items {first_vis_item}..{last_vis_item})  rest={}",
            visible.len() + rest.len(), visible.len(), rest.len()
        ));

        // フォルダごとに新規プールを作成する。
        // こうすることで旧フォルダのタスクが旧プールのスレッドを占有していても、
        // 新フォルダのタスクは即座に新プールの専用スレッドで開始できる。
        // 旧プールは旧OSスレッドの Arc が解放されたタイミングで自動的に破棄される。
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(THUMB_THREADS)
            .build()
            .expect("スレッドプール作成失敗");
        crate::logger::log(format!("  new thread pool created ({THUMB_THREADS} threads)"));

        std::thread::spawn(move || {
            pool.install(|| {
                use rayon::prelude::*;

                // フェーズ1: 可視範囲を最優先
                let t1 = std::time::Instant::now();
                crate::logger::log(format!("  [Phase1] START {} items", visible.len()));
                visible.par_iter().for_each(|(i, path)| {
                    if cancel.load(Ordering::Relaxed) {
                        crate::logger::log(format!("  [Phase1] CANCELLED before idx={i}"));
                        return;
                    }
                    load_one(path, thumb_px, *i, &tx);
                });
                crate::logger::log(format!(
                    "  [Phase1] END  {:.0}ms",
                    t1.elapsed().as_secs_f64() * 1000.0
                ));

                // フェーズ2: 残り
                let t2 = std::time::Instant::now();
                crate::logger::log(format!("  [Phase2] START {} items", rest.len()));
                rest.par_iter().for_each(|(i, path)| {
                    if cancel.load(Ordering::Relaxed) {
                        crate::logger::log(format!("  [Phase2] CANCELLED before idx={i}"));
                        return;
                    }
                    load_one(path, thumb_px, *i, &tx);
                });
                crate::logger::log(format!(
                    "  [Phase2] END  {:.0}ms",
                    t2.elapsed().as_secs_f64() * 1000.0
                ));
            });
        });
    }

    fn poll_thumbnails(&mut self, ctx: &egui::Context) {
        let mut count = 0u32;
        while let Ok((i, color_image)) = self.rx.try_recv() {
            if i < self.thumbnails.len() {
                let handle = ctx.load_texture(
                    format!("thumb_{i}"),
                    color_image,
                    egui::TextureOptions::LINEAR,
                );
                self.thumbnails[i] = ThumbnailState::Loaded(handle);
                count += 1;
            }
        }
        if count > 0 {
            // 最初の1枚受信時はメインスレッド側のタイムスタンプを記録
            crate::logger::log(format!("  [main] poll_thumbnails: received {count} thumbnail(s)"));
            ctx.request_repaint();
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let cols = self.grid_cols.max(1);
        let n = self.items.len();

        let (right, left, down, up, enter, backspace, ctrl_up, ctrl_down) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Backspace),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowUp),
                i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowDown),
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

        // BS: 親フォルダへ
        if backspace {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(parent) = cur.parent() {
                    return Some(parent.to_path_buf());
                }
            }
        }

        // Ctrl+↓: 深さ優先で次のフォルダへ
        if ctrl_down {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(next) = next_folder_dfs(cur) {
                    return Some(next);
                }
            }
        }

        // Ctrl+↑: 深さ優先で前のフォルダへ
        if ctrl_up {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(prev) = prev_folder_dfs(cur) {
                    return Some(prev);
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
                // 最大オフセットは「最終行のトップが見える最小の行境界スナップ値」
                // → ceil((total_h - viewport_h) / cell_size) * cell_size
                // こうすることで最大スクロール時も先頭行がウィンドウ上部に揃い、
                // 最終行の下に余白が生じる（ピクセル端数を切り上げて行境界に合わせる）
                let max_offset = if total_h <= self.last_viewport_h {
                    0.0
                } else {
                    (((total_h - self.last_viewport_h) / cell_size).ceil() * cell_size)
                        .min(total_h)
                };
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

/// 1枚の画像をロードしてサムネイル化し、チャネルへ送信する
fn load_one(
    path: &Path,
    thumb_px: u32,
    idx: usize,
    tx: &mpsc::Sender<(usize, egui::ColorImage)>,
) {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    let t = std::time::Instant::now();

    match image::open(path) {
        Ok(img) => {
            let decode_ms = t.elapsed().as_secs_f64() * 1000.0;
            let t2 = std::time::Instant::now();
            let thumb = img.thumbnail(thumb_px, thumb_px);
            let resize_ms = t2.elapsed().as_secs_f64() * 1000.0;
            let rgba = thumb.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            let _ = tx.send((idx, color_image));
            crate::logger::log(format!(
                "    idx={idx:>4} decode={decode_ms:>6.1}ms resize={resize_ms:>5.1}ms  {name}"
            ));
        }
        Err(e) => {
            crate::logger::log(format!("    idx={idx:>4} FAIL {e}  {name}"));
        }
    }
}

fn truncate_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        name.to_owned()
    } else {
        chars[..max_chars - 1].iter().collect::<String>() + "…"
    }
}

// -----------------------------------------------------------------------
// フォルダツリー走査（深さ優先・前順）
// -----------------------------------------------------------------------

/// 深さ優先前順で次のフォルダを返す
/// 子があれば最初の子、なければ次の兄弟、なければ祖先の次の兄弟
fn next_folder_dfs(current: &std::path::Path) -> Option<PathBuf> {
    // 1. 子フォルダがあれば最初の子へ
    if let Some(first_child) = sorted_subdirs(current).into_iter().next() {
        return Some(first_child);
    }
    // 2. 子がなければ、次の兄弟または祖先の次の兄弟を探す
    next_sibling_or_ancestor_sibling(current)
}

/// 深さ優先前順で前のフォルダを返す
/// 前の兄弟がいればその最後の子孫、最初の子であれば親
fn prev_folder_dfs(current: &std::path::Path) -> Option<PathBuf> {
    let parent = current.parent()?;
    let siblings = sorted_subdirs(parent);
    let pos = siblings.iter().position(|s| path_eq(s, current))?;

    if pos == 0 {
        // 最初の子 → 親へ
        Some(parent.to_path_buf())
    } else {
        // 前の兄弟の最後の子孫へ
        Some(last_descendant_dir(&siblings[pos - 1]))
    }
}

/// path の次の兄弟を返す。兄弟がなければ親で再帰する
fn next_sibling_or_ancestor_sibling(path: &std::path::Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let siblings = sorted_subdirs(parent);
    let pos = siblings.iter().position(|s| path_eq(s, path))?;

    if pos + 1 < siblings.len() {
        Some(siblings[pos + 1].clone())
    } else {
        next_sibling_or_ancestor_sibling(parent)
    }
}

/// path の最も深い最後の子孫フォルダを返す（子がなければ path 自身）
fn last_descendant_dir(path: &std::path::Path) -> PathBuf {
    let children = sorted_subdirs(path);
    match children.last() {
        Some(last) => last_descendant_dir(last),
        None => path.to_path_buf(),
    }
}

/// パス直下のサブフォルダを名前順で返す（隠しフォルダは含む）
fn sorted_subdirs(path: &std::path::Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    dirs.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    dirs
}

/// Windows のファイルシステムは大文字小文字を区別しないため小文字化して比較
fn path_eq(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}
