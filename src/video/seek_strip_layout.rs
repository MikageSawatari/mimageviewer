//! 動画シークストリップの**表示モードと寸法**を 1 か所で解く純粋ロジック。
//!
//! ストリップの高さは描画・入力・映像の予約領域・波形ラスタの要求という 4 つの経路が
//! 同時に見る値で、以前は `SEEK_STRIP_HEIGHT` 定数を各所で直接読んでいた。高さを可変に
//! するにあたり、**選択済みの高さと表示範囲から解決した 1 つの値** ([`SeekStripLayout`])
//! を全経路が共有する形に変える。定数を局所置換すると、映像の予約だけ旧高さで残る、
//! hit-test だけずれる、といった破れ方をする。

use eframe::egui;

use crate::settings::VideoSeekStripMode;

/// ストリップが動画のどこを写すか。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeekStripSpan {
    /// 再生位置を中央に固定し、その周辺を流す (従来)。
    #[default]
    Window,
    /// 動画全体を帯の横幅へ収める。中身は動かず、現在位置の赤線が左から右へ動く。
    Whole,
}

impl SeekStripSpan {
    pub fn label(self) -> &'static str {
        match self {
            Self::Window => "周辺",
            Self::Whole => "全体",
        }
    }

    /// 表示範囲の設定 (画像間隔 / 表示範囲) を持つか。
    ///
    /// 全体表示は尺と帯の幅だけで決まるので、範囲の段階値を持たない。ホイールの
    /// 段階変更もこの述語で止める。
    pub const fn has_range_setting(self) -> bool {
        matches!(self, Self::Window)
    }
}

/// ストリップの高さプリセット。
///
/// egui の UI は DPI に応じた論理座標なので、設定名には px 数を出さず「大 / 中 / 小」で
/// 選ばせる。`Large` は従来の固定値そのままで、既存の見た目を変えない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeekStripHeight {
    #[default]
    Large,
    Medium,
    Small,
}

/// 帯の高さ (points)。`Large` は 2 段化リデザイン以来の値。
const SEEK_STRIP_HEIGHT_LARGE: f32 = 104.0;
const SEEK_STRIP_HEIGHT_MEDIUM: f32 = 72.0;
const SEEK_STRIP_HEIGHT_SMALL: f32 = 48.0;

/// 周辺表示のセル幅 (points)。
///
/// `Large` の 152 は出荷済みの値で、変えるとドラッグ 1 ポイントあたりの進み方まで
/// 変わるので据え置く。中 / 小は同じセル内寸比 (148:94) を保った値へ丸めた。
const SEEK_STRIP_CELL_WIDTH_LARGE: f32 = 152.0;
const SEEK_STRIP_CELL_WIDTH_MEDIUM: f32 = 102.0;
const SEEK_STRIP_CELL_WIDTH_SMALL: f32 = 64.0;

/// 帯の高さとセルの高さの差 (上下の余白の合計)。
pub(crate) const SEEK_STRIP_CELL_VERTICAL_INSET: f32 = 10.0;

impl SeekStripHeight {
    pub const fn points(self) -> f32 {
        match self {
            Self::Large => SEEK_STRIP_HEIGHT_LARGE,
            Self::Medium => SEEK_STRIP_HEIGHT_MEDIUM,
            Self::Small => SEEK_STRIP_HEIGHT_SMALL,
        }
    }

    pub const fn window_cell_width_points(self) -> f32 {
        match self {
            Self::Large => SEEK_STRIP_CELL_WIDTH_LARGE,
            Self::Medium => SEEK_STRIP_CELL_WIDTH_MEDIUM,
            Self::Small => SEEK_STRIP_CELL_WIDTH_SMALL,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Large => "大",
            Self::Medium => "中",
            Self::Small => "小",
        }
    }

    pub const ALL: [Self; 3] = [Self::Large, Self::Medium, Self::Small];
}

/// 表示内容と表示範囲の組。非表示は [`SeekStripView`] 側が持つ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeekStripShowing {
    pub mode: VideoSeekStripMode,
    pub span: SeekStripSpan,
}

impl SeekStripShowing {
    pub const fn new(mode: VideoSeekStripMode, span: SeekStripSpan) -> Self {
        Self { mode, span }
    }

    pub fn label(self) -> &'static str {
        match (self.mode, self.span) {
            (VideoSeekStripMode::Thumbnails, SeekStripSpan::Window) => "場面 (周辺)",
            (VideoSeekStripMode::Thumbnails, SeekStripSpan::Whole) => "場面 (全体)",
            (VideoSeekStripMode::Waveform, SeekStripSpan::Window) => "波形 (周辺)",
            (VideoSeekStripMode::Waveform, SeekStripSpan::Whole) => "波形 (全体)",
        }
    }
}

/// `Shift+S` の巡回順。右下メニューの並びもこれを使う。
pub const SEEK_STRIP_SHOWING_ORDER: [SeekStripShowing; 4] = [
    SeekStripShowing::new(VideoSeekStripMode::Thumbnails, SeekStripSpan::Window),
    SeekStripShowing::new(VideoSeekStripMode::Thumbnails, SeekStripSpan::Whole),
    SeekStripShowing::new(VideoSeekStripMode::Waveform, SeekStripSpan::Window),
    SeekStripShowing::new(VideoSeekStripMode::Waveform, SeekStripSpan::Whole),
];

/// ストリップの表示状態。**非表示を含めた 5 値ひとつ**で持つ。
///
/// 開閉 bool + 内容 + 範囲の 3 本立てにすると、「非表示なのに全体表示」のような
/// 意味のない組が表現できてしまう。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SeekStripView {
    #[default]
    Hidden,
    Showing(SeekStripShowing),
}

impl SeekStripView {
    pub const fn showing(mode: VideoSeekStripMode, span: SeekStripSpan) -> Self {
        Self::Showing(SeekStripShowing::new(mode, span))
    }

    pub const fn mode(self) -> Option<VideoSeekStripMode> {
        match self {
            Self::Hidden => None,
            Self::Showing(showing) => Some(showing.mode),
        }
    }

    /// 表示範囲。非表示のときは既定の周辺表示を返す (寸法の解決に使う)。
    pub const fn span(self) -> SeekStripSpan {
        match self {
            Self::Hidden => SeekStripSpan::Window,
            Self::Showing(showing) => showing.span,
        }
    }

    pub const fn is_visible(self) -> bool {
        matches!(self, Self::Showing(_))
    }
}

/// `Shift+S` の巡回に含めるモードの選択。
///
/// 巡回から外したモードも右下メニューからは直接選べる。ここは**到達可能性ではなく
/// 巡回の長さ**を決める設定であり、機能自体の非表示にはしない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeekStripCycleSet {
    pub thumbnails_window: bool,
    pub thumbnails_whole: bool,
    pub waveform_window: bool,
    pub waveform_whole: bool,
}

impl Default for SeekStripCycleSet {
    fn default() -> Self {
        Self {
            thumbnails_window: true,
            thumbnails_whole: true,
            waveform_window: true,
            waveform_whole: true,
        }
    }
}

impl SeekStripCycleSet {
    pub const fn contains(self, showing: SeekStripShowing) -> bool {
        match (showing.mode, showing.span) {
            (VideoSeekStripMode::Thumbnails, SeekStripSpan::Window) => self.thumbnails_window,
            (VideoSeekStripMode::Thumbnails, SeekStripSpan::Whole) => self.thumbnails_whole,
            (VideoSeekStripMode::Waveform, SeekStripSpan::Window) => self.waveform_window,
            (VideoSeekStripMode::Waveform, SeekStripSpan::Whole) => self.waveform_whole,
        }
    }

    pub fn set(&mut self, showing: SeekStripShowing, enabled: bool) {
        let slot = match (showing.mode, showing.span) {
            (VideoSeekStripMode::Thumbnails, SeekStripSpan::Window) => &mut self.thumbnails_window,
            (VideoSeekStripMode::Thumbnails, SeekStripSpan::Whole) => &mut self.thumbnails_whole,
            (VideoSeekStripMode::Waveform, SeekStripSpan::Window) => &mut self.waveform_window,
            (VideoSeekStripMode::Waveform, SeekStripSpan::Whole) => &mut self.waveform_whole,
        };
        *slot = enabled;
    }

    /// 全解除は許可しない。空で読み込んだ設定はサムネイル周辺だけを有効へ正規化する。
    ///
    /// 空を許すと `Shift+S` が「非表示 → 非表示」になり、キーが無反応になる。
    #[must_use]
    pub fn normalized(self) -> Self {
        if SEEK_STRIP_SHOWING_ORDER
            .iter()
            .any(|showing| self.contains(*showing))
        {
            return self;
        }
        Self {
            thumbnails_window: true,
            thumbnails_whole: false,
            waveform_window: false,
            waveform_whole: false,
        }
    }

    /// `Shift+S` の次の状態。
    ///
    /// 巡回から外したモードを表示中でも、並び順の次にある有効なモードへ進む
    /// (メニューから選んだモードで行き止まりにしない)。
    #[must_use]
    pub fn next(self, current: SeekStripView) -> SeekStripView {
        let enabled = self.normalized();
        let start = match current {
            SeekStripView::Hidden => 0,
            SeekStripView::Showing(showing) => SEEK_STRIP_SHOWING_ORDER
                .iter()
                .position(|candidate| *candidate == showing)
                .map_or(0, |index| index + 1),
        };
        SEEK_STRIP_SHOWING_ORDER
            .iter()
            .skip(start)
            .find(|showing| enabled.contains(**showing))
            .map_or(SeekStripView::Hidden, |showing| {
                SeekStripView::Showing(*showing)
            })
    }
}

/// 全体表示のセル数の上限。
///
/// 幅と高さから決まるので通常は数十だが、極端な縦横比や将来の高解像度で軸のセル数が
/// 際限なく増えないよう天井を置く。
const SEEK_STRIP_WHOLE_MAX_CELLS: usize = 512;

/// 全体表示でセルのアスペクト比が取れないときに使う比。
const SEEK_STRIP_FALLBACK_ASPECT: f32 = 16.0 / 9.0;

/// 選択済みの高さと表示範囲から解決した、ストリップの寸法一式。
///
/// **描画・入力・映像の予約領域・波形ラスタの要求はすべてこの 1 つの値を見る。**
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SeekStripLayout {
    /// 帯そのもの。overlay 座標 (points)。
    pub(crate) rect: egui::Rect,
    pub(crate) span: SeekStripSpan,
    /// セル 1 つの幅 (points)。全体表示では帯幅をセル数で割った実寸。
    pub(crate) cell_width: f32,
    /// セル 1 つの高さ (points)。
    pub(crate) cell_height: f32,
    /// 全体表示のセル数。周辺表示は帯の外まで続くので `None`。
    pub(crate) whole_cell_count: Option<usize>,
}

impl SeekStripLayout {
    /// 帯の矩形と寸法を解決する。
    ///
    /// `overlay_size` と `bottom_bar_height` は points。`aspect` は表示上の動画の縦横比で、
    /// 分かっていない間は 16:9 を使う (セル数が 1 度決まればあとは実測値で作り直す)。
    pub(crate) fn resolve(
        overlay_size: egui::Vec2,
        bottom_bar_height: f32,
        height: SeekStripHeight,
        span: SeekStripSpan,
        aspect: Option<f32>,
    ) -> Self {
        let overlay_w = overlay_size.x.max(0.0);
        let overlay_h = overlay_size.y.max(0.0);
        let strip_height = height.points().min(overlay_h);
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                0.0,
                (overlay_h - bottom_bar_height - height.points()).max(0.0),
            ),
            egui::vec2(overlay_w, strip_height),
        );
        let cell_height = (strip_height - SEEK_STRIP_CELL_VERTICAL_INSET).max(1.0);
        match span {
            SeekStripSpan::Window => Self {
                rect,
                span,
                cell_width: height.window_cell_width_points(),
                cell_height,
                whole_cell_count: None,
            },
            SeekStripSpan::Whole => {
                let count = whole_cell_count(rect.width(), cell_height, aspect);
                Self {
                    rect,
                    span,
                    cell_width: rect.width() / count as f32,
                    cell_height,
                    whole_cell_count: Some(count),
                }
            }
        }
    }

    /// 波形ラスタを要求する物理ピクセル高さ。
    pub(crate) fn wave_pixel_height(&self, pixels_per_point: f32) -> usize {
        (self.cell_height * pixels_per_point).round().max(1.0) as usize
    }

    /// 波形ラスタを要求する物理ピクセル幅。
    pub(crate) fn wave_pixel_width(&self, pixels_per_point: f32) -> usize {
        (self.rect.width() * pixels_per_point).round().max(1.0) as usize
    }
}

/// 全体表示のセル数。
///
/// 高さと動画のアスペクト比から理想のセル幅を出し、帯幅に**最も近い枚数**を採る。
/// `floor` にすると右端に最大 1 セル分の空きが残り、赤線の左右端との対応が崩れる。
/// 実セル幅は `帯幅 / 枚数` なので理想幅とのずれは数 % に収まり、横スクロールも
/// セル間の空白も生まれない。
pub(crate) fn whole_cell_count(
    available_width: f32,
    cell_height: f32,
    aspect: Option<f32>,
) -> usize {
    let aspect = aspect
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(SEEK_STRIP_FALLBACK_ASPECT);
    let ideal_width = (cell_height * aspect).max(1.0);
    if !available_width.is_finite() || available_width <= 0.0 {
        return 1;
    }
    let count = (available_width / ideal_width).round();
    if !count.is_finite() || count < 1.0 {
        return 1;
    }
    (count as usize).min(SEEK_STRIP_WHOLE_MAX_CELLS)
}

/// 全体表示で、時刻を帯の x 座標へ写像する割合 (0.0 = 左端、1.0 = 右端)。
///
/// 長さが無い / 0 の動画では `None`。呼び出し側は全体表示を成立させず周辺表示へ
/// フォールバックする。
pub(crate) fn whole_position_fraction(time_secs: f64, duration_secs: f64) -> Option<f32> {
    if !time_secs.is_finite() || !duration_secs.is_finite() || duration_secs <= 0.0 {
        return None;
    }
    Some((time_secs / duration_secs).clamp(0.0, 1.0) as f32)
}

/// 全体表示で、帯上の x 座標を時刻へ写像する。
pub(crate) fn whole_time_at_x(pointer_x: f32, rect: egui::Rect, duration_secs: f64) -> Option<f64> {
    if !pointer_x.is_finite() || !duration_secs.is_finite() || duration_secs <= 0.0 {
        return None;
    }
    let width = rect.width();
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    let fraction = f64::from((pointer_x - rect.min.x) / width).clamp(0.0, 1.0);
    Some(fraction * duration_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUD_BOTTOM: f32 = 64.0;

    #[test]
    fn every_height_preset_keeps_the_band_above_the_bottom_bar() {
        for height in SeekStripHeight::ALL {
            let layout = SeekStripLayout::resolve(
                egui::vec2(1920.0, 1080.0),
                HUD_BOTTOM,
                height,
                SeekStripSpan::Window,
                None,
            );
            assert_eq!(layout.rect.height(), height.points());
            assert_eq!(layout.rect.max.y, 1080.0 - HUD_BOTTOM);
            assert_eq!(layout.cell_width, height.window_cell_width_points());
            assert_eq!(
                layout.cell_height,
                height.points() - SEEK_STRIP_CELL_VERTICAL_INSET
            );
        }
    }

    #[test]
    fn the_large_preset_reproduces_the_shipped_geometry() {
        let layout = SeekStripLayout::resolve(
            egui::vec2(1280.0, 720.0),
            HUD_BOTTOM,
            SeekStripHeight::Large,
            SeekStripSpan::Window,
            None,
        );
        assert_eq!(layout.rect.height(), 104.0);
        assert_eq!(layout.cell_width, 152.0);
        assert_eq!(layout.cell_height, 94.0);
        assert_eq!(layout.wave_pixel_height(1.0), 94);
    }

    #[test]
    fn whole_cells_fill_the_band_without_a_leftover_gap() {
        let layout = SeekStripLayout::resolve(
            egui::vec2(1920.0, 1080.0),
            HUD_BOTTOM,
            SeekStripHeight::Small,
            SeekStripSpan::Whole,
            Some(16.0 / 9.0),
        );
        let count = layout
            .whole_cell_count
            .expect("whole mode has a cell count");
        assert!(count > 1);
        assert!((layout.cell_width * count as f32 - layout.rect.width()).abs() < 0.01);
        // 理想幅 (高さ x アスペクト) から数 % 以内に収まる。
        let ideal = layout.cell_height * (16.0 / 9.0);
        assert!((layout.cell_width - ideal).abs() / ideal < 0.1);
    }

    #[test]
    fn taller_whole_cells_are_wider_and_therefore_fewer() {
        let small = SeekStripLayout::resolve(
            egui::vec2(1920.0, 1080.0),
            HUD_BOTTOM,
            SeekStripHeight::Small,
            SeekStripSpan::Whole,
            Some(16.0 / 9.0),
        );
        let large = SeekStripLayout::resolve(
            egui::vec2(1920.0, 1080.0),
            HUD_BOTTOM,
            SeekStripHeight::Large,
            SeekStripSpan::Whole,
            Some(16.0 / 9.0),
        );
        assert!(large.cell_width > small.cell_width);
        assert!(large.whole_cell_count < small.whole_cell_count);
    }

    #[test]
    fn a_portrait_video_gets_narrow_whole_cells() {
        let portrait = whole_cell_count(1920.0, 94.0, Some(9.0 / 16.0));
        let landscape = whole_cell_count(1920.0, 94.0, Some(16.0 / 9.0));
        assert!(portrait > landscape);
    }

    #[test]
    fn an_unusable_aspect_falls_back_instead_of_producing_zero_cells() {
        for aspect in [None, Some(0.0), Some(f32::NAN), Some(-2.0)] {
            assert_eq!(
                whole_cell_count(1920.0, 94.0, aspect),
                whole_cell_count(1920.0, 94.0, Some(16.0 / 9.0))
            );
        }
        assert_eq!(whole_cell_count(0.0, 94.0, None), 1);
    }

    #[test]
    fn the_default_cycle_visits_all_four_modes_then_hides() {
        let set = SeekStripCycleSet::default();
        let mut view = SeekStripView::Hidden;
        let mut seen = Vec::new();
        for _ in 0..4 {
            view = set.next(view);
            let SeekStripView::Showing(showing) = view else {
                panic!("cycle ended early at {view:?}");
            };
            seen.push(showing);
        }
        assert_eq!(seen, SEEK_STRIP_SHOWING_ORDER.to_vec());
        assert_eq!(set.next(view), SeekStripView::Hidden);
    }

    #[test]
    fn the_cycle_skips_modes_that_were_switched_off() {
        let set = SeekStripCycleSet {
            thumbnails_window: true,
            thumbnails_whole: false,
            waveform_window: false,
            waveform_whole: true,
        };
        let first = set.next(SeekStripView::Hidden);
        assert_eq!(
            first,
            SeekStripView::showing(VideoSeekStripMode::Thumbnails, SeekStripSpan::Window)
        );
        let second = set.next(first);
        assert_eq!(
            second,
            SeekStripView::showing(VideoSeekStripMode::Waveform, SeekStripSpan::Whole)
        );
        assert_eq!(set.next(second), SeekStripView::Hidden);
    }

    #[test]
    fn a_mode_chosen_from_the_menu_still_advances_even_when_it_is_off_the_cycle() {
        let set = SeekStripCycleSet {
            thumbnails_window: true,
            thumbnails_whole: false,
            waveform_window: false,
            waveform_whole: false,
        };
        // メニューから直接選べるモード。巡回対象でなくても行き止まりにしない。
        let picked = SeekStripView::showing(VideoSeekStripMode::Thumbnails, SeekStripSpan::Whole);
        assert_eq!(set.next(picked), SeekStripView::Hidden);
        let picked_first =
            SeekStripView::showing(VideoSeekStripMode::Waveform, SeekStripSpan::Whole);
        assert_eq!(set.next(picked_first), SeekStripView::Hidden);
    }

    #[test]
    fn an_all_off_cycle_set_normalizes_to_thumbnails_window() {
        let empty = SeekStripCycleSet {
            thumbnails_window: false,
            thumbnails_whole: false,
            waveform_window: false,
            waveform_whole: false,
        };
        let normalized = empty.normalized();
        assert!(normalized.thumbnails_window);
        assert!(!normalized.thumbnails_whole);
        assert!(!normalized.waveform_window);
        assert!(!normalized.waveform_whole);
        assert_eq!(
            empty.next(SeekStripView::Hidden),
            SeekStripView::showing(VideoSeekStripMode::Thumbnails, SeekStripSpan::Window)
        );
    }

    #[test]
    fn the_red_line_maps_the_ends_and_the_middle_of_the_video() {
        assert_eq!(whole_position_fraction(0.0, 120.0), Some(0.0));
        assert_eq!(whole_position_fraction(60.0, 120.0), Some(0.5));
        assert_eq!(whole_position_fraction(120.0, 120.0), Some(1.0));
        // 尺を越える報告値でも帯の外へは出さない。
        assert_eq!(whole_position_fraction(999.0, 120.0), Some(1.0));
    }

    #[test]
    fn an_unknown_or_zero_duration_has_no_whole_mapping() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 48.0));
        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(whole_position_fraction(1.0, duration), None);
            assert_eq!(whole_time_at_x(10.0, rect, duration), None);
        }
    }

    #[test]
    fn pointer_positions_round_trip_through_the_whole_mapping() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 900.0), egui::vec2(1000.0, 48.0));
        for (x, expected) in [(0.0, 0.0), (500.0, 60.0), (1000.0, 120.0)] {
            let time = whole_time_at_x(x, rect, 120.0).expect("mapping exists");
            assert!((time - expected).abs() < 1e-9, "{x} -> {time}");
            let fraction = whole_position_fraction(time, 120.0).expect("mapping exists");
            assert!((rect.min.x + rect.width() * fraction - x).abs() < 1e-3);
        }
    }

    #[test]
    fn only_the_window_span_owns_a_range_setting() {
        assert!(SeekStripSpan::Window.has_range_setting());
        assert!(!SeekStripSpan::Whole.has_range_setting());
    }
}
