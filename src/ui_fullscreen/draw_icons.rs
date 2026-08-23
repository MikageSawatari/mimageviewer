use eframe::egui;

use crate::grid_item::GridItem;
use crate::pdf_loader::PdfPageContentType;
use crate::settings::SpreadMode;
use crate::ui_helpers::format_bytes_small;

use super::{BAR_BUTTON_SIZE, CHECKMARK_MARGIN, CHECKMARK_RADIUS};

#[derive(Clone, Copy, Debug, PartialEq)]
struct BarButtonTouchTarget {
    id: egui::Id,
    rect: egui::Rect,
}

/// Previous/current-pass button geometry for one fullscreen viewport.
///
/// The registry lives in `Context::data_temp`; it deliberately does not add
/// another lifetime or input flag to `App`. `previous` is the last consecutive
/// app frame in which the same bar rectangle was drawn. `current` is populated
/// only through `draw_bar_button`, keeping that helper as the single geometry
/// seam for every button in the bar.
#[derive(Clone, Debug, Default)]
struct BarButtonTouchState {
    frame: Option<u64>,
    prepared_pass: Option<u64>,
    bar_rect: Option<egui::Rect>,
    previous: Vec<BarButtonTouchTarget>,
    current: Vec<BarButtonTouchTarget>,
    resolved_ids: Vec<egui::Id>,
}

fn bar_button_touch_state_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::new((module_path!(), ctx.viewport_id(), line!()))
}

fn begin_bar_button_touch_frame(
    state: &mut BarButtonTouchState,
    frame: u64,
    pass: u64,
    bar_rect: egui::Rect,
) {
    if state.frame != Some(frame) {
        let consecutive = state
            .frame
            .is_some_and(|previous| previous.checked_add(1) == Some(frame));
        if consecutive && state.bar_rect == Some(bar_rect) {
            state.previous = std::mem::take(&mut state.current);
        } else {
            state.previous.clear();
            state.current.clear();
        }
        state.resolved_ids.clear();
        state.frame = Some(frame);
    } else if state.bar_rect != Some(bar_rect) {
        // A resize/UI-scale transition invalidates the old Voronoi cells. Wait
        // for one freshly recorded frame rather than resolving against stale
        // geometry.
        state.previous.clear();
        state.current.clear();
        state.resolved_ids.clear();
    }

    state.prepared_pass = Some(pass);
    state.bar_rect = Some(bar_rect);
}

/// Resolve a tap to the horizontal Voronoi cell of the nearest button.
///
/// Vertically every cell spans the full bar. Horizontally, cells start at the
/// left edge of the leftmost button, meet at adjacent center midpoints, and the
/// rightmost cell reaches the bar's right edge. Midpoints belong to the button
/// on their right, so adjacent cells never overlap. The unused area to the left
/// of the leftmost button intentionally remains inert.
fn resolve_bar_touch_target(
    targets: &[BarButtonTouchTarget],
    bar_rect: egui::Rect,
    pos: egui::Pos2,
) -> Option<egui::Id> {
    if targets.is_empty() || !bar_rect.contains(pos) {
        return None;
    }

    let mut ordered: Vec<&BarButtonTouchTarget> = targets.iter().collect();
    ordered.sort_by(|a, b| a.rect.center().x.total_cmp(&b.rect.center().x));
    if pos.x < ordered[0].rect.left() {
        return None;
    }

    for (index, target) in ordered.iter().enumerate() {
        let left = if index == 0 {
            target.rect.left()
        } else {
            (ordered[index - 1].rect.center().x + target.rect.center().x) * 0.5
        };
        let right = if index + 1 == ordered.len() {
            bar_rect.right()
        } else {
            (target.rect.center().x + ordered[index + 1].rect.center().x) * 0.5
        };
        let contains_x = pos.x >= left
            && if index + 1 == ordered.len() {
                pos.x <= right
            } else {
                pos.x < right
            };
        if contains_x {
            return Some(target.id);
        }
    }

    None
}

/// Prepare the current frame's touch-only bar resolution before the fullscreen
/// background click handler runs. Returns true for any correlated primary tap
/// inside the bar, including inert space, so that it cannot leak through to the
/// fullscreen page-navigation response.
pub(super) fn prepare_bar_button_touch_targets(
    ctx: &egui::Context,
    frame: u64,
    bar_rect: egui::Rect,
    touch_frame: &crate::touch_correlation::TouchFrame,
) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let primary_clicked =
        ctx.input(|input| input.pointer.button_clicked(egui::PointerButton::Primary));
    let release_positions = if primary_clicked {
        touch_frame
            .correlated_primary_release_positions()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let tap_in_bar = release_positions.iter().any(|pos| bar_rect.contains(*pos));
    let id = bar_button_touch_state_id(ctx);

    ctx.data_mut(|data| {
        let mut state = data.get_temp::<BarButtonTouchState>(id).unwrap_or_default();
        begin_bar_button_touch_frame(&mut state, frame, pass, bar_rect);
        state.resolved_ids = release_positions
            .iter()
            .filter_map(|pos| resolve_bar_touch_target(&state.previous, bar_rect, *pos))
            .collect();
        data.insert_temp(id, state);
    });

    tap_in_bar
}

/// Ensure that a newly summoned bar records its first geometry pass even when
/// it became visible after the input handler computed its visibility snapshot.
pub(super) fn record_bar_button_touch_frame(ctx: &egui::Context, frame: u64, bar_rect: egui::Rect) {
    let pass = ctx.cumulative_pass_nr();
    let id = bar_button_touch_state_id(ctx);
    ctx.data_mut(|data| {
        let mut state = data.get_temp::<BarButtonTouchState>(id).unwrap_or_default();
        begin_bar_button_touch_frame(&mut state, frame, pass, bar_rect);
        data.insert_temp(id, state);
    });
}

fn record_bar_button_and_resolve_touch(
    ctx: &egui::Context,
    id: egui::Id,
    rect: egui::Rect,
) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let state_id = bar_button_touch_state_id(ctx);
    ctx.data_mut(|data| {
        let Some(mut state) = data.get_temp::<BarButtonTouchState>(state_id) else {
            return false;
        };
        let belongs_to_prepared_bar = state.prepared_pass == Some(pass)
            && state
                .bar_rect
                .is_some_and(|bar_rect| bar_rect.contains(rect.min) && bar_rect.contains(rect.max));
        if !belongs_to_prepared_bar {
            return false;
        }

        if let Some(target) = state.current.iter_mut().find(|target| target.id == id) {
            target.rect = rect;
        } else {
            state.current.push(BarButtonTouchTarget { id, rect });
        }
        let touch_clicked = state.resolved_ids.contains(&id);
        data.insert_temp(state_id, state);
        touch_clicked
    })
}

/// `egui::Response` plus a touch-only logical click resolved from the fixed
/// visual widget's previous-frame bar geometry.
///
/// Dereferencing retains the exact original response rectangle, hover state,
/// and mouse interaction. Only `clicked()` is widened, and only after positive
/// raw-touch correlation.
pub(super) struct BarButtonResponse {
    response: egui::Response,
    touch_clicked: bool,
}

impl BarButtonResponse {
    pub(super) fn clicked(&self) -> bool {
        self.touch_clicked || self.response.clicked()
    }

    pub(super) fn hover_tip_dark(self, text: impl Into<egui::WidgetText>) -> Self {
        Self {
            response: crate::ui_helpers::HoverTipExt::hover_tip_dark(self.response, text),
            touch_clicked: self.touch_clicked,
        }
    }
}

impl std::ops::Deref for BarButtonResponse {
    type Target = egui::Response;

    fn deref(&self) -> &Self::Target {
        &self.response
    }
}

// ── ホバーバーのアイコン描画関数 ────────────────────────────────────────

/// バーボタンの標準背景色を返す。
pub(super) fn bar_button_bg(hovered: bool, active: bool) -> egui::Color32 {
    if active {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
    } else if hovered {
        egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
    } else {
        egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
    }
}

/// 「その他の機能」ボタン。font glyph に頼らず、3 点を vector で描く。
pub(super) fn draw_more_icon(painter: &egui::Painter, center: egui::Pos2, _r: f32) {
    for offset in [-7.0, 0.0, 7.0] {
        painter.circle_filled(center + egui::vec2(offset, 0.0), 2.0, egui::Color32::WHITE);
    }
}

/// バーボタンの共通描画。位置とアイコン描画関数を受け取る。
pub(super) fn draw_bar_button(
    ui: &mut egui::Ui,
    x: f32,
    y: f32,
    id: &str,
    bg_fn: impl FnOnce(bool) -> egui::Color32,
    _active: bool,
    icon_fn: impl FnOnce(&egui::Painter, egui::Pos2, f32),
) -> BarButtonResponse {
    let rect = egui::Rect::from_min_size(
        egui::pos2(x, y),
        egui::vec2(BAR_BUTTON_SIZE, BAR_BUTTON_SIZE),
    );
    let id = egui::Id::new(id);
    let resp = ui.interact(rect, id, egui::Sense::click());
    let bg = bg_fn(resp.hovered());
    ui.painter().rect_filled(rect, 4.0, bg);
    let r = BAR_BUTTON_SIZE * 0.28;
    icon_fn(ui.painter(), rect.center(), r);
    let touch_clicked = record_bar_button_and_resolve_touch(ui.ctx(), id, rect);
    BarButtonResponse {
        response: resp,
        touch_clicked,
    }
}

#[cfg(test)]
mod bar_button_touch_tests {
    use super::*;

    fn target(id: u64, left: f32) -> BarButtonTouchTarget {
        BarButtonTouchTarget {
            id: egui::Id::new(id),
            rect: egui::Rect::from_min_size(
                egui::pos2(left, 6.0),
                egui::vec2(BAR_BUTTON_SIZE, BAR_BUTTON_SIZE),
            ),
        }
    }

    #[test]
    fn adjacent_button_midpoints_are_disjoint_and_choose_the_right_cell() {
        let bar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 44.0));
        let targets = [target(1, 100.0), target(2, 136.0)];
        let midpoint = (targets[0].rect.center().x + targets[1].rect.center().x) * 0.5;

        assert_eq!(
            resolve_bar_touch_target(&targets, bar, egui::pos2(midpoint - 0.001, 22.0)),
            Some(targets[0].id)
        );
        assert_eq!(
            resolve_bar_touch_target(&targets, bar, egui::pos2(midpoint, 22.0)),
            Some(targets[1].id)
        );
    }

    #[test]
    fn rightmost_cell_reaches_top_and_right_screen_edges() {
        let bar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 44.0));
        let targets = [target(1, 100.0), target(2, 136.0)];

        assert_eq!(
            resolve_bar_touch_target(&targets, bar, egui::pos2(300.0, 0.0)),
            Some(targets[1].id)
        );
        assert_eq!(
            resolve_bar_touch_target(&targets, bar, egui::pos2(299.0, 44.0)),
            Some(targets[1].id)
        );
    }

    #[test]
    fn inert_bar_space_and_points_outside_the_bar_resolve_to_nothing() {
        let bar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 44.0));
        let targets = [target(1, 100.0), target(2, 136.0)];

        assert_eq!(
            resolve_bar_touch_target(&targets, bar, egui::pos2(99.999, 22.0)),
            None
        );
        assert_eq!(
            resolve_bar_touch_target(&targets, bar, egui::pos2(150.0, 44.001)),
            None
        );
    }

    #[test]
    fn zero_and_one_button_bars_are_well_defined() {
        let bar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 44.0));
        assert_eq!(
            resolve_bar_touch_target(&[], bar, egui::pos2(300.0, 0.0)),
            None
        );

        let only = target(1, 136.0);
        assert_eq!(
            resolve_bar_touch_target(&[only], bar, egui::pos2(300.0, 0.0)),
            Some(only.id)
        );
        assert_eq!(
            resolve_bar_touch_target(&[only], bar, egui::pos2(135.999, 22.0)),
            None
        );
    }

    #[test]
    fn uncorrelated_mouse_click_never_arms_the_wide_target() {
        let ctx = egui::Context::default();
        let bar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 44.0));
        let close = target(1, 136.0);
        let corner = egui::pos2(300.0, 0.0);
        assert_eq!(
            resolve_bar_touch_target(&[close], bar, corner),
            Some(close.id)
        );

        let raw = egui::RawInput {
            screen_rect: Some(bar),
            events: vec![
                egui::Event::PointerMoved(corner),
                egui::Event::PointerButton {
                    pos: corner,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: corner,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            let state_id = bar_button_touch_state_id(ctx);
            ctx.data_mut(|data| {
                data.insert_temp(
                    state_id,
                    BarButtonTouchState {
                        frame: Some(0),
                        bar_rect: Some(bar),
                        current: vec![close],
                        ..Default::default()
                    },
                );
            });

            assert!(
                ctx.input(|input| { input.pointer.button_clicked(egui::PointerButton::Primary) })
            );
            assert!(!prepare_bar_button_touch_targets(
                ctx,
                1,
                bar,
                &crate::touch_correlation::TouchFrame::default(),
            ));
            let state = ctx
                .data(|data| data.get_temp::<BarButtonTouchState>(state_id))
                .unwrap();
            assert!(state.resolved_ids.is_empty());
        });
    }
}

#[cfg(test)]
pub(super) fn test_current_bar_button_targets(ctx: &egui::Context) -> Vec<(egui::Id, egui::Rect)> {
    let id = bar_button_touch_state_id(ctx);
    let mut targets = ctx
        .data(|data| data.get_temp::<BarButtonTouchState>(id))
        .map(|state| {
            state
                .current
                .into_iter()
                .map(|target| (target.id, target.rect))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    targets.sort_by(|left, right| left.1.min.x.total_cmp(&right.1.min.x));
    targets
}

/// "VST" ラベルを線分で自前描画する (= ホバーバーの VST3 管理ボタン用アイコン)。
/// proportional font だと S/T や V/S が重なって読みづらいため、line_segment で
/// 等幅 + 文字間 gap を確保する。3 文字を 32px のボタンに余裕を持って収める。
pub(super) fn draw_vst_text_label(painter: &egui::Painter, c: egui::Pos2) {
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    let char_w = 5.5_f32;
    let gap = 2.0_f32;
    let char_h = 10.0_f32;
    let group_w = 3.0 * char_w + 2.0 * gap;
    let top = c.y - char_h * 0.5;
    let bot = c.y + char_h * 0.5;
    let mid = c.y;
    let left = c.x - group_w * 0.5;

    // V
    let v_x0 = left;
    let v_x1 = v_x0 + char_w;
    let v_xc = (v_x0 + v_x1) * 0.5;
    painter.line_segment([egui::pos2(v_x0, top), egui::pos2(v_xc, bot)], stroke);
    painter.line_segment([egui::pos2(v_x1, top), egui::pos2(v_xc, bot)], stroke);

    // S (zigzag polyline)
    let s_x0 = v_x1 + gap;
    let s_x1 = s_x0 + char_w;
    painter.line_segment([egui::pos2(s_x1, top), egui::pos2(s_x0, top)], stroke);
    painter.line_segment([egui::pos2(s_x0, top), egui::pos2(s_x0, mid)], stroke);
    painter.line_segment([egui::pos2(s_x0, mid), egui::pos2(s_x1, mid)], stroke);
    painter.line_segment([egui::pos2(s_x1, mid), egui::pos2(s_x1, bot)], stroke);
    painter.line_segment([egui::pos2(s_x1, bot), egui::pos2(s_x0, bot)], stroke);

    // T
    let t_x0 = s_x1 + gap;
    let t_x1 = t_x0 + char_w;
    let t_xc = (t_x0 + t_x1) * 0.5;
    painter.line_segment([egui::pos2(t_x0, top), egui::pos2(t_x1, top)], stroke);
    painter.line_segment([egui::pos2(t_xc, top), egui::pos2(t_xc, bot)], stroke);
}

/// × アイコンを描画する。
///
/// 隠蔽加工 / 消しゴムパネルのタイトル行右端の閉じるボタンからも参照されるため
/// `pub(crate)` に昇格 (Phase 4)。`r` 引数は呼び出し側互換のため受け取るが、
/// 実装上はサイズを `BAR_BUTTON_SIZE` 派生で固定している。
pub(crate) fn draw_close_icon(painter: &egui::Painter, c: egui::Pos2, _r: f32) {
    let r = BAR_BUTTON_SIZE * 0.25;
    let stroke = egui::Stroke::new(2.5, egui::Color32::WHITE);
    painter.line_segment(
        [egui::pos2(c.x - r, c.y - r), egui::pos2(c.x + r, c.y + r)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(c.x + r, c.y - r), egui::pos2(c.x - r, c.y + r)],
        stroke,
    );
}

/// 下部ページシークバーの固定表示トグル用ロックアイコン。
pub(crate) fn draw_seek_lock_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, locked: bool) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.7, white);
    let body = egui::Rect::from_center_size(
        egui::pos2(c.x, c.y + r * 0.22),
        egui::vec2(r * 1.45, r * 0.95),
    );
    painter.rect_stroke(body, 2.0, stroke, egui::StrokeKind::Outside);

    painter.add(egui::Shape::line(
        seek_lock_shackle_points(c, r, body.top(), locked),
        stroke,
    ));

    painter.circle_filled(egui::pos2(c.x, c.y + r * 0.18), r * 0.14, white);
    painter.line_segment(
        [
            egui::pos2(c.x, c.y + r * 0.28),
            egui::pos2(c.x, c.y + r * 0.50),
        ],
        egui::Stroke::new(1.4, white),
    );
}

/// 潰れた角形ではなく、上端に十分な高さを持つ半円状の錠前シャックルを作る。
/// unlocked は半円全体を右へずらし、右端を本体外へ出して開状態を表す。
fn seek_lock_shackle_points(c: egui::Pos2, r: f32, body_top: f32, locked: bool) -> Vec<egui::Pos2> {
    const SEGMENTS: usize = 12;
    let half_w = r * 0.48;
    let arch_h = r * 0.72;
    let center_x = c.x + if locked { 0.0 } else { r * 0.50 };
    (0..=SEGMENTS)
        .map(|i| {
            let t = i as f32 / SEGMENTS as f32;
            let angle = std::f32::consts::PI + std::f32::consts::PI * t;
            egui::pos2(
                center_x + half_w * angle.cos(),
                body_top + arch_h * angle.sin(),
            )
        })
        .collect()
}

#[cfg(test)]
mod seek_lock_icon_tests {
    use super::*;

    #[test]
    fn shackle_is_a_visible_semicircle_and_unlock_has_an_open_end() {
        let c = egui::pos2(20.0, 20.0);
        let r = 9.0;
        let body_top = 18.0;
        let locked = seek_lock_shackle_points(c, r, body_top, true);
        assert!((locked.first().unwrap().y - body_top).abs() < 0.01);
        assert!((locked.last().unwrap().y - body_top).abs() < 0.01);
        assert!(body_top - locked[locked.len() / 2].y > r * 0.65);
        assert!((locked.first().unwrap().x + locked.last().unwrap().x - 2.0 * c.x).abs() < 0.01);

        let unlocked = seek_lock_shackle_points(c, r, body_top, false);
        let body_right = c.x + r * 1.45 * 0.5;
        assert!(unlocked.last().unwrap().x > body_right);
    }
}

/// 目アイコン (= プレビューボタン)。横長楕円 + 中央の瞳孔。
/// 消しゴム / 隠蔽加工パネルの「押している間だけ最終結果プレビュー」ボタンに使う。
pub(crate) fn draw_eye_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    // 外形 (アーモンド型) を線分で近似: 上半円と下半円の組み合わせ。
    // 簡単のため、横長の楕円輪郭 + 中央に瞳孔を描く。
    let rx = r * 1.05;
    let ry = r * 0.6;
    // 楕円輪郭 (36 段の polyline)
    const N: usize = 32;
    let mut pts = Vec::with_capacity(N + 1);
    for i in 0..=N {
        let theta = i as f32 * std::f32::consts::TAU / N as f32;
        pts.push(egui::pos2(c.x + rx * theta.cos(), c.y + ry * theta.sin()));
    }
    painter.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.5, white)));
    // 瞳孔 (= 塗りつぶし円)
    painter.circle_filled(c, r * 0.32, white);
}

/// 動画 / 音楽のブックマーク追加ボタンと同じ、切り欠き付きのブックマーク形状。
///
/// 実体は `video::native_presenter::overlay_draw::draw_overlay_bookmark_icon` と同じ形だが、
/// あちらは native presenter (= `#[cfg(windows)]`) 配下にあり非 Windows ビルドから参照できない
/// ため、静止画パネル用にここへ複製している。**形を変えるときは両方揃えること**。
pub(crate) fn draw_bookmark_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    fill: egui::Color32,
) {
    let rect = egui::Rect::from_center_size(c, egui::vec2(r * 1.10, r * 1.55));
    let notch = egui::pos2(rect.center().x, rect.max.y - r * 0.35);
    painter.add(egui::Shape::convex_polygon(
        vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            notch,
            rect.left_bottom(),
        ],
        fill,
        egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 245, 190)),
    ));
}

/// 消しゴムアイコン (= Codex Phase 4 redesign)。コンパクトな斜めブロックの消しゴム +
/// 先端の摩擦帯 + 消しカスの小さな粒で「消す動作」を示唆する。
///
/// `r ≈ BAR_BUTTON_SIZE * 0.28` ≈ 9 を想定。-14° の浅い傾きで「斜めに置かれた消しゴム」
/// を表現しつつ、シルエットを読みやすく保つ。ボディは pale gray (`#EEEEEF`)、
/// 先端カットは worn gray (`#B2B2BC`)、下に消しカスの粒 3 個。
pub(crate) fn draw_eraser_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let theta: f32 = (-14.0_f32).to_radians();
    let (sin, cos) = (theta.sin(), theta.cos());

    let rotate = |p: egui::Vec2| egui::vec2(p.x * cos - p.y * sin, p.x * sin + p.y * cos);
    let to_pos = |p: egui::Vec2| egui::pos2(c.x + p.x, c.y + p.y);

    let hw = r * 0.92;
    let hh = r * 0.44;
    let cut = r * 0.34;

    // ボディ (左上の角を斜めカット)
    let body = [
        rotate(egui::vec2(-hw + cut, -hh)),
        rotate(egui::vec2(hw, -hh)),
        rotate(egui::vec2(hw, hh)),
        rotate(egui::vec2(-hw, hh)),
        rotate(egui::vec2(-hw, -hh + cut)),
    ];

    // 先端 (摩擦帯) — 左端の小さい台形
    let tip = [
        rotate(egui::vec2(-hw + cut, -hh)),
        rotate(egui::vec2(-hw, -hh + cut)),
        rotate(egui::vec2(-hw, hh)),
        rotate(egui::vec2(-hw + cut * 0.95, hh)),
    ];

    painter.add(egui::Shape::convex_polygon(
        body.iter().map(|&p| to_pos(p)).collect(),
        egui::Color32::from_rgb(238, 238, 242),
        egui::Stroke::NONE,
    ));

    painter.add(egui::Shape::convex_polygon(
        tip.iter().map(|&p| to_pos(p)).collect(),
        egui::Color32::from_rgb(178, 178, 188),
        egui::Stroke::NONE,
    ));

    // ボディ全体の白アウトライン
    painter.add(egui::Shape::closed_line(
        body.iter().map(|&p| to_pos(p)).collect(),
        egui::Stroke::new(1.25, egui::Color32::WHITE),
    ));

    // ボディ / 先端の境界線 (薄灰)
    let seam_a = to_pos(rotate(egui::vec2(-hw + cut * 0.95, -hh * 0.88)));
    let seam_b = to_pos(rotate(egui::vec2(-hw + cut * 0.95, hh * 0.88)));
    painter.line_segment(
        [seam_a, seam_b],
        egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
    );

    // 消しカス (= erase 動作の示唆)
    let dust = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 150);
    painter.circle_filled(egui::pos2(c.x - r * 0.72, c.y + r * 0.55), r * 0.10, dust);
    painter.circle_filled(egui::pos2(c.x - r * 0.38, c.y + r * 0.72), r * 0.08, dust);
    painter.line_segment(
        [
            egui::pos2(c.x - r * 0.96, c.y + r * 0.34),
            egui::pos2(c.x - r * 0.64, c.y + r * 0.28),
        ],
        egui::Stroke::new(1.0, dust),
    );
}

/// モザイクアイコン (= Codex Phase 4 redesign)。3x3 グリッド + セル色の微変化で
/// 「タイルの集合 = モザイク」感を出す。隠蔽加工 (補正パネルの conceal ボタン) 専用で、
/// 動画タイルモードの `draw_tile_grid_icon` (= 2x2) とは用途を分けている。
///
/// セルサイズ `r * 0.54` + ギャップ `r * 0.13` で、タイルが「点」ではなく
/// 「大きめの正方ピース」として読めるように調整。3x3 = 9 セルそれぞれに
/// 白〜淡灰の 9 色を割り当て、平面でなく "色の集まり" としての mosaic を表現。
pub(crate) fn draw_mosaic_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let cell = r * 0.54;
    let gap = r * 0.13;
    let step = cell + gap;
    let rounding = 1.0;

    let colors = [
        egui::Color32::from_rgb(255, 255, 255),
        egui::Color32::from_rgb(218, 224, 235),
        egui::Color32::from_rgb(246, 246, 248),
        egui::Color32::from_rgb(205, 214, 228),
        egui::Color32::from_rgb(255, 255, 255),
        egui::Color32::from_rgb(226, 232, 240),
        egui::Color32::from_rgb(238, 240, 244),
        egui::Color32::from_rgb(210, 218, 232),
        egui::Color32::from_rgb(250, 250, 252),
    ];

    let mut i = 0;
    for y in -1..=1 {
        for x in -1..=1 {
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(c.x + x as f32 * step, c.y + y as f32 * step),
                    egui::vec2(cell, cell),
                ),
                rounding,
                colors[i],
            );
            i += 1;
        }
    }
}

/// エクスポートアイコン (= ファイル保存。トレイへ下向き矢印の "書き出し" メタファ)。
///
/// 補正パネルヘッダーの消しゴム / 隠蔽アイコンと並ぶ 3 つ目の起動ボタン用 (Ctrl+E)。
/// 下向き矢印 (= 画像をファイルへ書き出す) + 下部の受け皿 (トレイ) で「ディスクに
/// 保存」を表す。ホバーバーのカメラ (= クリップボードコピー) とは別シンボルにして
/// 役割を区別する。`r ≈ HEADER_BTN_SIZE * 0.28` を想定。
pub(crate) fn draw_export_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.8, white);

    // 下向き矢印 (上から下へ書き出すイメージ)
    let stem_top = egui::pos2(c.x, c.y - r * 0.95);
    let stem_bot = egui::pos2(c.x, c.y + r * 0.2);
    painter.line_segment([stem_top, stem_bot], stroke);
    // 矢じり
    painter.line_segment(
        [stem_bot, egui::pos2(c.x - r * 0.45, c.y - r * 0.25)],
        stroke,
    );
    painter.line_segment(
        [stem_bot, egui::pos2(c.x + r * 0.45, c.y - r * 0.25)],
        stroke,
    );

    // 受け皿 (トレイ) — 上辺を開けた U 字。下向き矢印を受け止める器。
    let tray_l = c.x - r * 0.85;
    let tray_r = c.x + r * 0.85;
    let tray_top = c.y + r * 0.45;
    let tray_bot = c.y + r * 0.9;
    painter.line_segment(
        [egui::pos2(tray_l, tray_top), egui::pos2(tray_l, tray_bot)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(tray_l, tray_bot), egui::pos2(tray_r, tray_bot)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(tray_r, tray_top), egui::pos2(tray_r, tray_bot)],
        stroke,
    );
}

/// 切り取り (crop) アイコン。重なった 2 本の L 字マークで写真のトリミング枠を表す。
pub(crate) fn draw_crop_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.8, white);
    let a = r * 0.45; // 内側フレームの半幅
    let ext = r * 0.95; // アームの外側端
    // 左アーム (縦) — 上に飛び出す
    painter.line_segment(
        [egui::pos2(c.x - a, c.y - ext), egui::pos2(c.x - a, c.y + a)],
        stroke,
    );
    // 上アーム (横) — 左に飛び出す
    painter.line_segment(
        [egui::pos2(c.x - ext, c.y - a), egui::pos2(c.x + a, c.y - a)],
        stroke,
    );
    // 右アーム (縦) — 下に飛び出す
    painter.line_segment(
        [egui::pos2(c.x + a, c.y - a), egui::pos2(c.x + a, c.y + ext)],
        stroke,
    );
    // 下アーム (横) — 右に飛び出す
    painter.line_segment(
        [egui::pos2(c.x - a, c.y + a), egui::pos2(c.x + ext, c.y + a)],
        stroke,
    );
}

/// テキスト注釈 (comic) アイコン。吹き出し + 本文線 + しっぽで注釈ツールを表す。
pub(crate) fn draw_text_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.6, white);
    let bw = r * 0.92; // 吹き出し本体の半幅
    let bh = r * 0.62; // 吹き出し本体の半高
    let body = egui::Rect::from_min_max(
        egui::pos2(c.x - bw, c.y - bh - r * 0.14),
        egui::pos2(c.x + bw, c.y + bh - r * 0.14),
    );
    painter.rect_stroke(body, r * 0.30, stroke, egui::StrokeKind::Middle);
    // しっぽ (左下)
    painter.add(egui::Shape::line(
        vec![
            egui::pos2(c.x - bw * 0.42, body.max.y - 0.6),
            egui::pos2(c.x - bw * 0.58, c.y + bh + r * 0.34),
            egui::pos2(c.x - bw * 0.04, body.max.y - 0.6),
        ],
        stroke,
    ));
    // 本文線 2 本
    let line_y0 = body.center().y - r * 0.20;
    let line_y1 = body.center().y + r * 0.16;
    painter.line_segment(
        [
            egui::pos2(c.x - bw * 0.55, line_y0),
            egui::pos2(c.x + bw * 0.55, line_y0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x - bw * 0.55, line_y1),
            egui::pos2(c.x + bw * 0.28, line_y1),
        ],
        stroke,
    );
}

/// 補正レイヤーアイコン。重なった薄いシートで「非破壊レイヤー」を表す。
pub(crate) fn draw_local_adjust_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.45, white);
    let layer = |dy: f32| {
        [
            egui::pos2(c.x, c.y + dy - r * 0.58),
            egui::pos2(c.x + r * 0.95, c.y + dy - r * 0.12),
            egui::pos2(c.x, c.y + dy + r * 0.46),
            egui::pos2(c.x - r * 0.95, c.y + dy - r * 0.12),
        ]
    };

    for (dy, alpha) in [(r * 0.52, 95), (r * 0.18, 120), (-r * 0.18, 155)] {
        let points = layer(dy);
        painter.add(egui::Shape::convex_polygon(
            points.to_vec(),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha),
            egui::Stroke::NONE,
        ));
        painter.add(egui::Shape::closed_line(points.to_vec(), stroke));
    }

    let accent = egui::Stroke::new(
        1.15,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 190),
    );
    painter.line_segment(
        [
            egui::pos2(c.x - r * 0.36, c.y - r * 0.18),
            egui::pos2(c.x + r * 0.36, c.y - r * 0.18),
        ],
        accent,
    );
    painter.line_segment(
        [
            egui::pos2(c.x, c.y - r * 0.54),
            egui::pos2(c.x, c.y + r * 0.18),
        ],
        accent,
    );
}

/// ウィンドウ / 全画面 切り替えアイコン (タイトルバー付きウィンドウ枠)。
/// native 動画 HUD の `draw_overlay_window_toggle_icon` と見た目を揃える。
pub(super) fn draw_window_toggle_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
    let win = egui::Rect::from_center_size(c, egui::vec2(r * 1.85, r * 1.5));
    painter.rect_stroke(win, 1.0, stroke, egui::StrokeKind::Inside);
    let title_y = win.top() + r * 0.46;
    painter.line_segment(
        [
            egui::pos2(win.left(), title_y),
            egui::pos2(win.right(), title_y),
        ],
        stroke,
    );
}

/// 一時停止アイコン (2本の縦線) を描画する。
pub(super) fn draw_pause_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let bar_w = r * 0.3;
    let gap = r * 0.35;
    let stroke = egui::Stroke::new(bar_w, egui::Color32::WHITE);
    painter.line_segment(
        [
            egui::pos2(c.x - gap, c.y - r),
            egui::pos2(c.x - gap, c.y + r),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + gap, c.y - r),
            egui::pos2(c.x + gap, c.y + r),
        ],
        stroke,
    );
}

/// 再生アイコン (右向き三角形) を描画する。
pub(super) fn draw_play_triangle(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let cx = c.x + r * 0.12;
    let points = vec![
        egui::pos2(cx - r * 0.5, c.y - r * 0.75),
        egui::pos2(cx - r * 0.5, c.y + r * 0.75),
        egui::pos2(cx + r * 0.7, c.y),
    ];
    painter.add(egui::Shape::convex_polygon(
        points,
        egui::Color32::WHITE,
        egui::Stroke::NONE,
    ));
}

/// 🔬 分析アイコン（虫眼鏡＋十字線）を描画する。
/// タイルモード アイコン: 2x2 の塗りつぶし正方形 (= ■ ■ / ■ ■)。
/// 旧 ▦ 文字は font glyph によっては tofu (□) に化けるため自前描画に切替。
///
/// 隠蔽加工 (モザイク) のアイコンとしても流用するため `pub(crate)` に昇格 (Phase 4)。
pub(crate) fn draw_tile_grid_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    // セル 1 個の半幅、セル間ギャップ
    let cell = r * 0.36;
    let gap = r * 0.10;
    let off = cell + gap * 0.5;
    for &(dx, dy) in &[(-1.0_f32, -1.0_f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let cx = c.x + dx * off;
        let cy = c.y + dy * off;
        painter.rect_filled(
            egui::Rect::from_center_size(egui::pos2(cx, cy), egui::vec2(cell, cell)),
            1.0,
            white,
        );
    }
}

pub(super) fn draw_camera_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.8, white);
    let body =
        egui::Rect::from_center_size(c + egui::vec2(0.0, r * 0.18), egui::vec2(r * 1.75, r * 1.2));
    painter.rect_stroke(body, 2.0, stroke, egui::StrokeKind::Inside);
    let hump = egui::Rect::from_min_size(
        egui::pos2(body.min.x + r * 0.25, body.min.y - r * 0.32),
        egui::vec2(r * 0.72, r * 0.34),
    );
    painter.rect_filled(hump, 1.2, white);
    painter.circle_stroke(body.center(), r * 0.38, stroke);
    painter.circle_filled(body.center(), r * 0.15, white);
}

/// 360 度パノラマアイコン (球体 + 経度線) を描画する。
/// docs/panorama-360-view-plan.md §5.3 のホバーバーボタン用。
/// 検出強度 (Auto / Hint) によらず単一の見た目に統一 (Codex P3 反映)。
/// 非対応画像のときは bar_button_bg 側で disable 色にすることで区別する。
pub(super) fn draw_panorama_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let white = egui::Color32::WHITE;
    let main_stroke = egui::Stroke::new(1.8, white);
    let line_stroke = egui::Stroke::new(1.2, white);
    // 外側の球 (円)
    let sphere_r = r * 1.05;
    painter.circle_stroke(c, sphere_r, main_stroke);
    // 赤道 (水平線)
    painter.line_segment(
        [
            egui::pos2(c.x - sphere_r, c.y),
            egui::pos2(c.x + sphere_r, c.y),
        ],
        line_stroke,
    );
    // 中央経度 (垂直線)
    painter.line_segment(
        [
            egui::pos2(c.x, c.y - sphere_r),
            egui::pos2(c.x, c.y + sphere_r),
        ],
        line_stroke,
    );
    // 左右の経度線 (球体感を出す縦線、±0.55r)
    let inner_a = sphere_r * 0.55;
    let inner_h = (sphere_r * sphere_r - inner_a * inner_a).sqrt();
    painter.line_segment(
        [
            egui::pos2(c.x - inner_a, c.y - inner_h),
            egui::pos2(c.x - inner_a, c.y + inner_h),
        ],
        line_stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x + inner_a, c.y - inner_h),
            egui::pos2(c.x + inner_a, c.y + inner_h),
        ],
        line_stroke,
    );
}

/// 投影方式アイコン。360 モード中の上バーに出し、クリックで方式を順送りする。
///
/// **絵で方式そのものを表す**: 球のアイコンに引く経度線の位置を、その方式の
/// 半径写像 `r = g(θ)` で決める (θ = 30° / 60° を θ_max = 80° で正規化)。
/// 透視は線が外周へ寄り、等距離は等間隔、等立体角は中心寄り、立体射影はその中間に
/// なる。方式名はツールチップと切り替え時のトーストで示す
/// (UI 文字列に環境依存グリフを使わない方針のため、記号は描かない)。
pub(super) fn draw_panorama_projection_icon(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    projection: crate::panorama::PanoProjection,
) {
    use crate::panorama::PanoProjection as P;
    let white = egui::Color32::WHITE;
    let main_stroke = egui::Stroke::new(1.8, white);
    let line_stroke = egui::Stroke::new(1.2, white);
    let sphere_r = r * 1.05;
    painter.circle_stroke(c, sphere_r, main_stroke);
    painter.line_segment(
        [
            egui::pos2(c.x - sphere_r, c.y),
            egui::pos2(c.x + sphere_r, c.y),
        ],
        line_stroke,
    );
    // θ = 30° / 60° を θ_max = 80° で正規化した相対半径。
    // panorama.rs の `PanoProjection` の写像表と同じ式。
    let deg = std::f32::consts::PI / 180.0;
    let (p1, p2) = match projection.normalized() {
        P::Perspective => (
            (30.0 * deg).tan() / (80.0 * deg).tan(),
            (60.0 * deg).tan() / (80.0 * deg).tan(),
        ),
        P::Stereographic => (
            (15.0 * deg).tan() / (40.0 * deg).tan(),
            (30.0 * deg).tan() / (40.0 * deg).tan(),
        ),
        P::Equidistant => (30.0 / 80.0, 60.0 / 80.0),
        P::EquisolidAngle => (
            (15.0 * deg).sin() / (40.0 * deg).sin(),
            (30.0 * deg).sin() / (40.0 * deg).sin(),
        ),
        P::Unknown => unreachable!(),
    };
    // 中央経度 + 左右対称の経度線 2 組。円内に収まる弦として引く。
    for offset in [0.0, p1, -p1, p2, -p2] {
        let a = sphere_r * offset;
        let half_chord = (sphere_r * sphere_r - a * a).max(0.0).sqrt();
        painter.line_segment(
            [
                egui::pos2(c.x + a, c.y - half_chord),
                egui::pos2(c.x + a, c.y + half_chord),
            ],
            line_stroke,
        );
    }
}

/// 360 度パノラマアイコン (disabled 版)。非対応画像のときに dim 色で描画。
/// シルエットは同じだが、配色だけ落として「ボタン自体は存在するが押せない」感を出す。
pub(super) fn draw_panorama_icon_disabled(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let dim = egui::Color32::from_rgba_unmultiplied(180, 180, 180, 140);
    let main_stroke = egui::Stroke::new(1.5, dim);
    let line_stroke = egui::Stroke::new(1.0, dim);
    let sphere_r = r * 1.05;
    painter.circle_stroke(c, sphere_r, main_stroke);
    painter.line_segment(
        [
            egui::pos2(c.x - sphere_r, c.y),
            egui::pos2(c.x + sphere_r, c.y),
        ],
        line_stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x, c.y - sphere_r),
            egui::pos2(c.x, c.y + sphere_r),
        ],
        line_stroke,
    );
}

/// ℹ アイコンを描画する。
pub(super) fn draw_info_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    draw_info_icon_colored(painter, c, r, egui::Color32::WHITE);
}

fn draw_info_icon_colored(painter: &egui::Painter, c: egui::Pos2, r: f32, color: egui::Color32) {
    painter.circle_stroke(c, r, egui::Stroke::new(1.5, color));
    let bar_w = r * 0.22;
    painter.line_segment(
        [
            egui::pos2(c.x, c.y - r * 0.05),
            egui::pos2(c.x, c.y + r * 0.55),
        ],
        egui::Stroke::new(bar_w, color),
    );
    painter.circle_filled(egui::pos2(c.x, c.y - r * 0.45), bar_w * 0.7, color);
}

/// ClickToShow の ℹ アイコン。青い情報アイコンへ小さなマウスカーソルを重ねる。
pub(super) fn draw_info_click_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    let blue = egui::Color32::from_rgb(95, 175, 255);
    draw_info_icon_colored(painter, c, r, blue);

    let tip = egui::pos2(c.x + r * 0.18, c.y + r * 0.1);
    let size = r * 0.75;
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip,
            tip + egui::vec2(size * 0.12, size),
            tip + egui::vec2(size * 0.42, size * 0.7),
        ],
        egui::Color32::WHITE,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(30, 80, 130)),
    ));
    painter.line_segment(
        [
            tip + egui::vec2(size * 0.35, size * 0.64),
            tip + egui::vec2(size * 0.68, size * 0.95),
        ],
        egui::Stroke::new(size * 0.18, blue),
    );
}

/// 左右パネル呼び出しバーの内向き三角。フォント glyph は使わない。
pub(super) fn draw_panel_callout_arrow(
    painter: &egui::Painter,
    center: egui::Pos2,
    points_right: bool,
    color: egui::Color32,
) {
    let dx = 5.5;
    let dy = 8.0;
    let sign = if points_right { 1.0 } else { -1.0 };
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(center.x + sign * dx, center.y),
            egui::pos2(center.x - sign * dx, center.y - dy),
            egui::pos2(center.x - sign * dx, center.y + dy),
        ],
        color,
        egui::Stroke::NONE,
    ));
}

/// チェックマーク（右上）を描画する。
pub(super) fn draw_fs_checkmark(ui: &mut egui::Ui, full_rect: egui::Rect) {
    let check_center = egui::pos2(
        full_rect.max.x - CHECKMARK_RADIUS - CHECKMARK_MARGIN,
        full_rect.min.y + CHECKMARK_RADIUS + CHECKMARK_MARGIN,
    );
    ui.painter().circle_filled(
        check_center,
        CHECKMARK_RADIUS,
        egui::Color32::from_rgb(40, 160, 40),
    );
    let s = CHECKMARK_RADIUS * 0.55;
    let stroke = egui::Stroke::new(3.0, egui::Color32::WHITE);
    ui.painter().line_segment(
        [
            egui::pos2(check_center.x - s * 0.6, check_center.y),
            egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            egui::pos2(check_center.x - s * 0.1, check_center.y + s * 0.5),
            egui::pos2(check_center.x + s * 0.7, check_center.y - s * 0.5),
        ],
        stroke,
    );
}

/// 上部ホバーバー左側の表示文字列を組み立てる。
///
/// - `ZipImage`: `<archive-path> > <entry_name>`
/// - `PdfPage`:  `<pdf-path> > Page N`
/// - それ以外: `<folder>\<filename>` (Windows パス区切り)
///
/// `base_folder` は `effective_folder()` の表示文字列を想定 (変換済みアーカイブ
/// 閲覧中は元 RAR/7z/LZH のパスが渡ってくる)。空文字列なら基底パス部分を省略する。
pub(super) fn compute_location_display(
    item: Option<&GridItem>,
    base_folder: &str,
    filename: &str,
) -> String {
    match item {
        Some(GridItem::ZipImage { entry_name, .. }) => {
            if base_folder.is_empty() {
                entry_name.clone()
            } else {
                format!("{base_folder} > {entry_name}")
            }
        }
        Some(GridItem::PdfPage { page_num, .. }) => {
            if base_folder.is_empty() {
                format!("Page {}", page_num + 1)
            } else {
                format!("{base_folder} > Page {}", page_num + 1)
            }
        }
        _ => {
            // 通常画像・動画等: folder + basename を連結。
            if base_folder.is_empty() {
                filename.to_string()
            } else if filename.is_empty() {
                base_folder.to_string()
            } else {
                let ends_with_sep =
                    base_folder.ends_with(std::path::MAIN_SEPARATOR) || base_folder.ends_with('/');
                if ends_with_sep {
                    format!("{base_folder}{filename}")
                } else {
                    format!("{base_folder}{}{filename}", std::path::MAIN_SEPARATOR)
                }
            }
        }
    }
}

/// 画面フィットによる縮小とは無関係で、GPU テクスチャの互換上限 (8192px) を超えた元画像を
/// 表示用に縮小していることを示す。文言とテストが離れないよう定義元はここだけにする。
pub(super) const DOWNSCALE_MARKER: &str = " ⚠ GPU上限により解像度制限中";

/// ファイル情報テキスト (PDF 種別・寸法・AI・ファイルサイズ) を描画する。
/// ファイル名は左側 `location_display` に統合済みなのでここでは扱わない。
pub(super) fn draw_fs_bar_info_text(
    ui: &mut egui::Ui,
    bar_rect: egui::Rect,
    right_anchor: egui::Pos2,
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    image_downscaled: bool,
    ai_upscale_info: Option<(&str, u32, u32)>,
    pdf_content_type: Option<PdfPageContentType>,
) {
    let text = build_info_text(
        image_dims,
        image_file_size,
        image_downscaled,
        ai_upscale_info,
        pdf_content_type,
    );
    if !text.is_empty() {
        // ダウンスケール警告だけ黄色で強調したいのでマーカー部分を切り分けて描画する。
        let (main_text, has_marker) = if image_downscaled && text.ends_with(DOWNSCALE_MARKER) {
            (&text[..text.len() - DOWNSCALE_MARKER.len()], true)
        } else {
            (text.as_str(), false)
        };
        let marker = DOWNSCALE_MARKER;
        let font = egui::FontId::proportional(15.0);
        if has_marker {
            // 右詰で書くので、まず marker を右端に、次に main_text をその左に置く。
            let marker_galley = ui.painter().layout_no_wrap(
                marker.to_string(),
                font.clone(),
                egui::Color32::from_rgb(255, 210, 80),
            );
            ui.painter().galley(
                egui::pos2(
                    right_anchor.x - marker_galley.size().x,
                    right_anchor.y - marker_galley.size().y * 0.5,
                ),
                marker_galley.clone(),
                egui::Color32::from_rgb(255, 210, 80),
            );
            let main_anchor = egui::pos2(right_anchor.x - marker_galley.size().x, right_anchor.y);
            ui.painter().text(
                main_anchor,
                egui::Align2::RIGHT_CENTER,
                main_text,
                font,
                ui.visuals().text_color(),
            );
        } else {
            ui.painter().text(
                right_anchor,
                egui::Align2::RIGHT_CENTER,
                text,
                font,
                ui.visuals().text_color(),
            );
        }
    }
    let _ = bar_rect;
}

/// 上部バー右側に表示する画像情報テキスト (PDF 種別 / 寸法 / AI / サイズ) を組み立てる。
/// `image_downscaled` が true の場合、dims の直後 (AI 情報がある場合はその後) に
/// [`DOWNSCALE_MARKER`] を挿入する。
pub(super) fn build_info_text(
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    image_downscaled: bool,
    ai_upscale_info: Option<(&str, u32, u32)>,
    pdf_content_type: Option<PdfPageContentType>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(ct) = pdf_content_type {
        match ct {
            PdfPageContentType::Raster { w, h } => {
                parts.push(format!("PDF Raster {w}×{h}"));
            }
            PdfPageContentType::Vector => {
                parts.push("PDF Vector".to_string());
            }
        }
    }
    if let Some((w, h)) = image_dims {
        let mut dims_part = if let Some((model_name, ai_w, ai_h)) = ai_upscale_info {
            // AI アップスケール情報: "11 × 22 (漫画 44×88)"
            format!("{w} × {h} ({model_name} {ai_w}×{ai_h})")
        } else {
            format!("{w} × {h}")
        };
        if image_downscaled {
            dims_part.push_str(DOWNSCALE_MARKER);
        }
        parts.push(dims_part);
    }
    if let Some(bytes) = image_file_size {
        parts.push(format_bytes_small(bytes));
    }
    parts.join("    ")
}

/// 動画モード時の上ホバーバー右側情報を組み立てる (Phase 6)。
/// `(W × H、長さ mm:ss、平均 X.X Mbps、ファイルサイズ)` をスペース区切りで連結。
/// 各項目は値が無い (0 / None) ならスキップ。
pub(super) fn build_info_text_video(
    dims: Option<(u32, u32)>,
    file_size: Option<u64>,
    video_meta: Option<(f64, i64)>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some((w, h)) = dims {
        parts.push(format!("{w} × {h}"));
    }
    if let Some((dur, bitrate)) = video_meta {
        if dur > 0.0 {
            parts.push(crate::ui_helpers::format_hms(dur));
        }
        if bitrate > 0 {
            parts.push(crate::ui_helpers::format_bitrate_bps(bitrate));
        }
    }
    if let Some(bytes) = file_size {
        parts.push(format_bytes_small(bytes));
    }
    parts.join("    ")
}

/// 回転アイコンを自前描画する。
pub(super) fn draw_rotate_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    clockwise: bool,
) {
    let stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
    let n = 24;

    let start_rad = 315.0_f32.to_radians();
    let end_rad = (315.0 + 270.0_f32).to_radians();
    let arc_span = end_rad - start_rad;

    let mut points = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let angle = start_rad + arc_span * t;
        points.push(egui::pos2(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }

    for i in 0..n {
        painter.line_segment([points[i], points[i + 1]], stroke);
    }

    let (arrow_pt, tangent_x, tangent_y) = if clockwise {
        let angle = end_rad;
        (points[n], angle.sin(), -angle.cos())
    } else {
        let angle = start_rad;
        (points[0], -angle.sin(), angle.cos())
    };

    let nx = -tangent_y;
    let ny = tangent_x;
    let a = radius * 0.55;
    let p1 = egui::pos2(
        arrow_pt.x + tangent_x * a + nx * a * 0.45,
        arrow_pt.y + tangent_y * a + ny * a * 0.45,
    );
    let p2 = egui::pos2(
        arrow_pt.x + tangent_x * a - nx * a * 0.45,
        arrow_pt.y + tangent_y * a - ny * a * 0.45,
    );
    painter.line_segment([arrow_pt, p1], stroke);
    painter.line_segment([arrow_pt, p2], stroke);
}

/// 表示モードアイコンを描画する。
/// 余白カットフィットのアイコン: 外枠 (余白込みの画像) + 内枠 (中身) + 4 辺から内枠へ
/// 向かう矢印 (= 余白を詰めて中身を拡大するイメージ)。
pub(super) fn draw_margin_fit_icon(painter: &egui::Painter, c: egui::Pos2, r: f32) {
    // 外枠 (薄いグレー = 余白込みの画像全体)
    let outer = egui::Rect::from_center_size(c, egui::vec2(r * 1.7, r * 2.0));
    painter.rect_stroke(
        outer,
        1.0,
        egui::Stroke::new(1.2, egui::Color32::from_gray(150)),
        egui::StrokeKind::Middle,
    );
    // 内枠 (白 = 中身)
    let inner = egui::Rect::from_center_size(c, egui::vec2(r * 0.9, r * 1.15));
    painter.rect_stroke(
        inner,
        1.0,
        egui::Stroke::new(1.8, egui::Color32::WHITE),
        egui::StrokeKind::Middle,
    );
    let stroke = egui::Stroke::new(1.4, egui::Color32::WHITE);
    let head = |tip: egui::Pos2, dir: egui::Vec2| {
        let n = egui::vec2(-dir.y, dir.x) * 2.0;
        let back = tip - dir * 3.0;
        painter.line_segment([tip, back + n], stroke);
        painter.line_segment([tip, back - n], stroke);
    };
    let t_tip = egui::pos2(c.x, inner.min.y - 1.5);
    painter.line_segment([egui::pos2(c.x, outer.min.y + 2.0), t_tip], stroke);
    head(t_tip, egui::vec2(0.0, 1.0));
    let b_tip = egui::pos2(c.x, inner.max.y + 1.5);
    painter.line_segment([egui::pos2(c.x, outer.max.y - 2.0), b_tip], stroke);
    head(b_tip, egui::vec2(0.0, -1.0));
    let l_tip = egui::pos2(inner.min.x - 1.5, c.y);
    painter.line_segment([egui::pos2(outer.min.x + 2.0, c.y), l_tip], stroke);
    head(l_tip, egui::vec2(1.0, 0.0));
    let rt_tip = egui::pos2(inner.max.x + 1.5, c.y);
    painter.line_segment([egui::pos2(outer.max.x - 2.0, c.y), rt_tip], stroke);
    head(rt_tip, egui::vec2(-1.0, 0.0));
}

pub(super) fn draw_spread_icon(painter: &egui::Painter, c: egui::Pos2, r: f32, mode: SpreadMode) {
    let white = egui::Color32::WHITE;
    let stroke = egui::Stroke::new(1.5, white);
    let page_w = r * 0.7;
    let page_h = r * 1.1;

    match mode {
        SpreadMode::Single => {
            // 単独ページ: 1枚の矩形
            let rect = egui::Rect::from_center_size(c, egui::vec2(page_w, page_h));
            painter.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Outside);
        }
        SpreadMode::Ltr | SpreadMode::Rtl => {
            // 見開き（表紙なし）: 2枚の矩形
            let gap = r * 0.15;
            let left_rect = egui::Rect::from_center_size(
                egui::pos2(c.x - page_w * 0.5 - gap * 0.5, c.y),
                egui::vec2(page_w, page_h),
            );
            let right_rect = egui::Rect::from_center_size(
                egui::pos2(c.x + page_w * 0.5 + gap * 0.5, c.y),
                egui::vec2(page_w, page_h),
            );
            painter.rect_stroke(left_rect, 1.0, stroke, egui::StrokeKind::Outside);
            painter.rect_stroke(right_rect, 1.0, stroke, egui::StrokeKind::Outside);
            // 方向矢印
            draw_spread_direction_arrow(painter, c, r, mode.is_rtl());
        }
        SpreadMode::LtrCover | SpreadMode::RtlCover => {
            // 表紙あり: 小さい表紙 + 見開き2枚
            let small_w = page_w * 0.55;
            let gap = r * 0.12;
            let total = small_w + gap + page_w * 2.0 + gap;
            let start_x = c.x - total * 0.5;

            // 表紙（小さい矩形）
            let cover_rect = egui::Rect::from_center_size(
                egui::pos2(start_x + small_w * 0.5, c.y),
                egui::vec2(small_w, page_h),
            );
            painter.rect_stroke(cover_rect, 1.0, stroke, egui::StrokeKind::Outside);

            // 見開き2枚
            let spread_x = start_x + small_w + gap;
            let left_rect = egui::Rect::from_center_size(
                egui::pos2(spread_x + page_w * 0.5, c.y),
                egui::vec2(page_w, page_h),
            );
            let right_rect = egui::Rect::from_center_size(
                egui::pos2(spread_x + page_w * 1.5 + gap, c.y),
                egui::vec2(page_w, page_h),
            );
            painter.rect_stroke(left_rect, 1.0, stroke, egui::StrokeKind::Outside);
            painter.rect_stroke(right_rect, 1.0, stroke, egui::StrokeKind::Outside);
            // 方向矢印
            draw_spread_direction_arrow(painter, c, r, mode.is_rtl());
        }
        SpreadMode::Vertical => {
            // 縦読み: 縦に連なるページ + 下向き矢印
            let stack_w = page_w * 0.9;
            let stack_h = page_h * 0.48;
            let gap = r * 0.10;
            for offset in [-1.0, 0.0, 1.0] {
                let rect = egui::Rect::from_center_size(
                    egui::pos2(c.x, c.y + offset * (stack_h + gap)),
                    egui::vec2(stack_w, stack_h),
                );
                painter.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Outside);
            }
            let tip = egui::pos2(c.x + stack_w * 0.55, c.y + r * 0.72);
            let top = egui::pos2(c.x + stack_w * 0.55, c.y - r * 0.62);
            painter.line_segment([top, tip], stroke);
            painter.line_segment([tip, tip + egui::vec2(-r * 0.22, -r * 0.22)], stroke);
            painter.line_segment([tip, tip + egui::vec2(r * 0.22, -r * 0.22)], stroke);
        }
    }
}

/// 見開きモードの方向矢印（→ or ←）を描画する。
pub(super) fn draw_spread_direction_arrow(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    rtl: bool,
) {
    let white = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180);
    let arrow_stroke = egui::Stroke::new(1.2, white);
    let ay = c.y + r * 1.4; // 矩形の下
    let ax = c.x;
    let alen = r * 0.6;
    let ahead = r * 0.3;

    if rtl {
        // ←
        painter.line_segment(
            [egui::pos2(ax + alen, ay), egui::pos2(ax - alen, ay)],
            arrow_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(ax - alen, ay),
                egui::pos2(ax - alen + ahead, ay - ahead),
            ],
            arrow_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(ax - alen, ay),
                egui::pos2(ax - alen + ahead, ay + ahead),
            ],
            arrow_stroke,
        );
    } else {
        // →
        painter.line_segment(
            [egui::pos2(ax - alen, ay), egui::pos2(ax + alen, ay)],
            arrow_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(ax + alen, ay),
                egui::pos2(ax + alen - ahead, ay - ahead),
            ],
            arrow_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(ax + alen, ay),
                egui::pos2(ax + alen - ahead, ay + ahead),
            ],
            arrow_stroke,
        );
    }
}

// ── パネル用 egui ウィジェット helper ─────────────────────────────
// 隠蔽パネル (egui) と将来統一する消しゴムパネル (現 raw painter) の双方で
// 同じ「ボタンらしい見た目」を出すための共通ヘルパー。
//
// 既定の `selectable_label` / `selectable_value` は active 時に枠が少し変わる
// 程度でほぼフラットに見えるため、ユーザーから「ボタンに見えない」とフィード
// バックを受けた経緯がある (2026-05-27)。raw painter の eraser パネルは active
// 時に色付き矩形 + inactive 時に gray50 を塗っており、その look を egui::Button
// + fill で再現する。

/// パネル トグルボタンの色定義。
pub(crate) struct PanelToggleColors {
    /// active (selected=true) 時の塗りつぶし。
    pub selected_fill: egui::Color32,
    /// inactive (selected=false) 時の塗りつぶし。
    pub inactive_fill: egui::Color32,
    /// hover (inactive 時のみ) の塗りつぶし。
    pub hover_fill: egui::Color32,
}

impl PanelToggleColors {
    /// 既定 (青系 active、暗灰 inactive)。
    pub(crate) const fn blue() -> Self {
        Self {
            selected_fill: egui::Color32::from_rgb(60, 120, 200),
            inactive_fill: egui::Color32::from_rgb(50, 50, 50),
            hover_fill: egui::Color32::from_rgb(70, 70, 70),
        }
    }
    /// 描画モード active (赤系)。
    pub(crate) const fn paint_red() -> Self {
        Self {
            selected_fill: egui::Color32::from_rgb(180, 60, 60),
            inactive_fill: egui::Color32::from_rgb(50, 50, 50),
            hover_fill: egui::Color32::from_rgb(70, 70, 70),
        }
    }
    /// 消去モード active (青系、paint_red と対になる)。
    pub(crate) const fn erase_blue() -> Self {
        Self {
            selected_fill: egui::Color32::from_rgb(60, 120, 180),
            inactive_fill: egui::Color32::from_rgb(50, 50, 50),
            hover_fill: egui::Color32::from_rgb(70, 70, 70),
        }
    }
}

/// パネル トグルボタン (塗りつぶし矩形 + 中央テキスト)。
///
/// 既定の `selectable_label` だと active/inactive で塗りに差がなくフラットに
/// 見えるので、明確な fill 差で「押せる」と分かるようにする。
///
/// `min_size` を渡すと幅/高さの下限を強制する (= 隣接ボタンを揃えるときに使う)。
/// `colors` を省略すると青系の既定セットを使う。
///
/// 戻り値は `egui::Response`。クリック判定は呼び出し側で
/// `if resp.clicked() { ... }` のように使う。
pub(crate) fn panel_toggle_button(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    selected: bool,
    min_size: Option<egui::Vec2>,
    colors: Option<PanelToggleColors>,
) -> egui::Response {
    let c = colors.unwrap_or_else(PanelToggleColors::blue);
    // 一旦 Button で probe して hover 状態を取り、hover 中の inactive は
    // fill を hover_fill に差し替える (= 2 段階) と raw painter の look に揃う。
    // egui::Button 自体には「hover 時の fill 差し替え」の直接 API がないので、
    // allocate_exact_size + 自前描画でシンプルに書く。
    let widget_text: egui::WidgetText = text.into();
    let galley = widget_text.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let mut size = galley.size() + egui::vec2(12.0, 6.0);
    if let Some(min) = min_size {
        size.x = size.x.max(min.x);
        size.y = size.y.max(min.y);
    }
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let bg = if selected {
        c.selected_fill
    } else if resp.hovered() {
        c.hover_fill
    } else {
        c.inactive_fill
    };
    ui.painter().rect_filled(rect, 3.0, bg);
    let text_pos = rect.center() - galley.size() * 0.5;
    ui.painter().galley(text_pos, galley, egui::Color32::WHITE);
    resp
}
