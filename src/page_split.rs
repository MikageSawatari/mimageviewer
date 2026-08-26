//! 横長ページ 1 枚を左右の表示ステップへ分けて読む (§1.119)。
//!
//! **元の item index が正本のまま**で、フルスクリーンの表示位置だけが左右へ分かれる。
//! ★ / タグ / しおり / 読書位置 / 補正 / 注釈 / 切り取りはすべて分割前のページへ記録し、
//! サムネイルも分割前の画像を使う。永続的な論理ページを作らないのが前提である
//! (作ると DB・検索・シークバー・サムネイルまで一斉に論理ページ化する必要が出る)。
//!
//! このモジュールは**分割の順序だけ**を持つ。持たないもの:
//!
//! - **分割対象かどうかの判定**。回転を反映した縦横比と「静止画か」は一覧側の
//!   `is_landscape` / `is_spread_pairable_item` が既に持っているので、述語で受け取る。
//!   ここで縦横比を読み直すと、同じ判定が 2 か所に増える。
//! - 描画、テクスチャ、永続化。`PageSlice::uv_rect` が返すのは範囲だけで、
//!   誰がどう描くかは呼び出し側の責務。
//!
//! 縦連結でも同じステップ列を使う。連結時は「同じ texture 由来の 2 領域を縦に並べる」
//! ことになり、並べる順序はページ送りの順序と同じものである。

use eframe::egui;

/// 分割したページの、どちら側を見ているか。
///
/// `Full` は「分割していない」であって「左右の中間」ではない。縦長ページ、動画、
/// 分割 OFF はすべて `Full` になる。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageSlice {
    #[default]
    Full,
    Left,
    Right,
}

impl PageSlice {
    /// テクスチャのどの範囲を描くか (左上原点の正規化座標)。
    ///
    /// 分割位置は 50% 固定。手動調整は MVP に含めない。
    pub fn uv_rect(self) -> egui::Rect {
        match self {
            Self::Full => egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Self::Left => egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(0.5, 1.0)),
            Self::Right => egui::Rect::from_min_max(egui::pos2(0.5, 0.0), egui::pos2(1.0, 1.0)),
        }
    }

    /// 半分だけを見ているか。自動表示トリムを無効にする条件でもある。
    pub fn is_half(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// **元画像空間**の部分矩形。`content_bbox` に渡すのはこちら。
    ///
    /// [`Self::uv_rect`] は「画面で見て左半分 / 右半分」なので**表示空間**である。
    /// 保存回転があると両者は一致しない (90 度回転したページの画面左半分は、元画像では
    /// 上半分)。写像は screen ↔ source と同じ `inverse_uv` を使う。
    pub fn source_bbox(self, rotation: crate::rotation_db::Rotation) -> egui::Rect {
        let display = self.uv_rect();
        let (ax, ay) =
            crate::displayed_image_transform::inverse_uv(rotation, display.min.x, display.min.y);
        let (bx, by) =
            crate::displayed_image_transform::inverse_uv(rotation, display.max.x, display.max.y);
        egui::Rect::from_min_max(
            egui::pos2(ax.min(bx), ay.min(by)),
            egui::pos2(ax.max(bx), ay.max(by)),
        )
    }
}

/// 分割したページをどちら側から読むか。
///
/// 表示モードとして排他的に選ぶ。「1ページ表示 / 通常の見開き」と組み合わせる独立の
/// bool にはしない (組み合わせ状態が増え、どの経路が有効かを各所で判定し直すことになる)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    /// 左半分から読む。
    LeftFirst,
    /// 右半分から読む。
    RightFirst,
}

impl SplitDirection {
    /// 表示モードから分割の向きを取る。分割モードでなければ `None`。
    ///
    /// `SpreadMode::is_split` との食い違いは
    /// `every_split_mode_names_a_direction` が固定する。
    pub fn from_spread_mode(mode: crate::settings::SpreadMode) -> Option<Self> {
        match mode {
            crate::settings::SpreadMode::SplitLtr => Some(Self::LeftFirst),
            crate::settings::SpreadMode::SplitRtl => Some(Self::RightFirst),
            _ => None,
        }
    }

    /// このページで最初に見る側。しおり等から開いたときの着地先でもある。
    pub fn first(self) -> PageSlice {
        match self {
            Self::LeftFirst => PageSlice::Left,
            Self::RightFirst => PageSlice::Right,
        }
    }

    /// 2 つ目に見る側。
    pub fn second(self) -> PageSlice {
        match self {
            Self::LeftFirst => PageSlice::Right,
            Self::RightFirst => PageSlice::Left,
        }
    }
}

/// フルスクリーンの表示位置。
///
/// `source_idx` が正本で、`slice` は表示だけの一時状態。**どちらを見ていたかは
/// 永続化しない** ので、この型は保存経路へ渡さない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationStep {
    pub source_idx: usize,
    pub slice: PageSlice,
}

impl PresentationStep {
    /// 分割しないページの表示位置。
    pub fn whole(source_idx: usize) -> Self {
        Self {
            source_idx,
            slice: PageSlice::Full,
        }
    }
}

/// 表示を 1 つ動かした結果。
///
/// 元ページが変わったかどうかで、テクスチャの読み直し・表示確定・履歴記録の扱いが
/// 変わる。呼び出し側が `before.source_idx != after.source_idx` を各所で組み立てると
/// 判定が散るので、ここで型にして返す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepMove {
    /// 同じ元ページの中で左右が変わった。
    WithinPage { to: PresentationStep },
    /// 別の元ページへ移った。
    ToAnotherPage { to: PresentationStep },
    /// 端にいて動かない。
    AtEnd,
}

impl StepMove {
    /// 移動先。端なら `None`。
    pub fn destination(self) -> Option<PresentationStep> {
        match self {
            Self::WithinPage { to } | Self::ToAnotherPage { to } => Some(to),
            Self::AtEnd => None,
        }
    }
}

/// nav 順の item を、分割を織り込んだ表示ステップ列へ広げる。
///
/// `is_split_idx` が真の item だけが 2 ステップになる。まだ寸法が分からない item は
/// 偽を返してもらい 1 ステップになる — 寸法が届いた後にステップ列は組み直される。
/// これは既存の見開きユニット生成が `is_landscape` に対して持つ性質と同じで、
/// 分割のためだけに読み込みを待たせない。
///
/// 述語は既存の見開きユニット生成と同じ `(nav 内の位置, item index)` を受け取る。
/// 保存回転が nav と同じ並びで返るので、位置が要る。
pub fn presentation_steps(
    nav: &[usize],
    direction: SplitDirection,
    mut is_split_idx: impl FnMut(usize, usize) -> bool,
) -> Vec<PresentationStep> {
    let mut steps = Vec::with_capacity(nav.len());
    for (nav_pos, &idx) in nav.iter().enumerate() {
        if is_split_idx(nav_pos, idx) {
            steps.push(PresentationStep {
                source_idx: idx,
                slice: direction.first(),
            });
            steps.push(PresentationStep {
                source_idx: idx,
                slice: direction.second(),
            });
        } else {
            steps.push(PresentationStep::whole(idx));
        }
    }
    steps
}

/// この item を開いたときに着地するステップ位置。
///
/// **分割方向の最初の半分**へ着地する。しおり・履歴・検索・シークバーから開き直した
/// ときに「前回どちらを見ていたか」を覚えていないのは仕様で、覚えると保存対象が
/// 増え、元ページ単位という前提が崩れる。
pub fn landing_step(steps: &[PresentationStep], source_idx: usize) -> Option<usize> {
    steps.iter().position(|s| s.source_idx == source_idx)
}

/// 表示を 1 つ進める。
pub fn step_forward(steps: &[PresentationStep], at: usize) -> StepMove {
    step_to(steps, at, at.checked_add(1))
}

/// 表示を 1 つ戻す。
pub fn step_backward(steps: &[PresentationStep], at: usize) -> StepMove {
    step_to(steps, at, at.checked_sub(1))
}

fn step_to(steps: &[PresentationStep], at: usize, target: Option<usize>) -> StepMove {
    let (Some(current), Some(to)) = (steps.get(at), target.and_then(|t| steps.get(t))) else {
        return StepMove::AtEnd;
    };
    if current.source_idx == to.source_idx {
        StepMove::WithinPage { to: *to }
    } else {
        StepMove::ToAnotherPage { to: *to }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 分割対象がなければ、ステップ列は nav とそのまま 1 対 1 になる。
    #[test]
    fn pages_that_do_not_split_stay_one_step_each() {
        let steps = presentation_steps(&[4, 7, 9], SplitDirection::LeftFirst, |_, _| false);
        assert_eq!(
            steps,
            vec![
                PresentationStep::whole(4),
                PresentationStep::whole(7),
                PresentationStep::whole(9),
            ]
        );
    }

    #[test]
    fn a_split_page_is_read_from_the_side_the_direction_names() {
        let ltr = presentation_steps(&[0], SplitDirection::LeftFirst, |_, _| true);
        assert_eq!(
            ltr.iter().map(|s| s.slice).collect::<Vec<_>>(),
            vec![PageSlice::Left, PageSlice::Right]
        );

        let rtl = presentation_steps(&[0], SplitDirection::RightFirst, |_, _| true);
        assert_eq!(
            rtl.iter().map(|s| s.slice).collect::<Vec<_>>(),
            vec![PageSlice::Right, PageSlice::Left]
        );

        // どちらの向きでも、元ページは 1 つのまま。
        assert!(ltr.iter().all(|s| s.source_idx == 0));
        assert!(rtl.iter().all(|s| s.source_idx == 0));
    }

    /// 横長と縦長が混ざったときに、分割したページだけが 2 ステップになる。
    #[test]
    fn only_the_landscape_pages_are_split() {
        let steps = presentation_steps(&[0, 1, 2], SplitDirection::RightFirst, |_, idx| idx == 1);
        assert_eq!(
            steps,
            vec![
                PresentationStep::whole(0),
                PresentationStep {
                    source_idx: 1,
                    slice: PageSlice::Right
                },
                PresentationStep {
                    source_idx: 1,
                    slice: PageSlice::Left
                },
                PresentationStep::whole(2),
            ]
        );
    }

    #[test]
    fn an_empty_navigation_has_no_steps() {
        assert!(presentation_steps(&[], SplitDirection::LeftFirst, |_, _| true).is_empty());
    }

    /// 開き直しは分割方向の最初の半分へ着地する。後ろの半分には着地しない。
    #[test]
    fn reopening_a_split_page_lands_on_its_first_half() {
        let steps = presentation_steps(&[5, 6], SplitDirection::RightFirst, |_, _| true);
        let at = landing_step(&steps, 6).expect("6 がステップ列に無い");
        assert_eq!(
            steps[at],
            PresentationStep {
                source_idx: 6,
                slice: PageSlice::Right
            }
        );
        assert_eq!(landing_step(&steps, 99), None);
    }

    /// 同じ元ページ内の左右移動と、次の元ページへの移動を区別する。
    #[test]
    fn moving_within_a_page_is_distinguished_from_moving_to_the_next_one() {
        let steps = presentation_steps(&[0, 1], SplitDirection::LeftFirst, |_, idx| idx == 0);
        // [0:Left, 0:Right, 1:Full]
        assert_eq!(
            step_forward(&steps, 0),
            StepMove::WithinPage {
                to: PresentationStep {
                    source_idx: 0,
                    slice: PageSlice::Right
                }
            }
        );
        assert_eq!(
            step_forward(&steps, 1),
            StepMove::ToAnotherPage {
                to: PresentationStep::whole(1)
            }
        );
        assert_eq!(
            step_backward(&steps, 2),
            StepMove::ToAnotherPage {
                to: PresentationStep {
                    source_idx: 0,
                    slice: PageSlice::Right
                }
            }
        );
        assert_eq!(
            step_backward(&steps, 1),
            StepMove::WithinPage {
                to: PresentationStep {
                    source_idx: 0,
                    slice: PageSlice::Left
                }
            }
        );
    }

    /// 端では動かない。分割の途中で端に当たる形にはしない。
    #[test]
    fn both_ends_stop_instead_of_wrapping() {
        let steps = presentation_steps(&[0], SplitDirection::LeftFirst, |_, _| true);
        assert_eq!(step_backward(&steps, 0), StepMove::AtEnd);
        assert_eq!(step_forward(&steps, 1), StepMove::AtEnd);
        assert_eq!(step_forward(&steps, 99), StepMove::AtEnd);
        assert_eq!(StepMove::AtEnd.destination(), None);
    }

    /// 左右で元画像をちょうど覆い、重ならない。
    #[test]
    fn the_two_halves_tile_the_whole_image() {
        let left = PageSlice::Left.uv_rect();
        let right = PageSlice::Right.uv_rect();
        assert_eq!(left.max.x, right.min.x);
        assert_eq!(left.min.x, 0.0);
        assert_eq!(right.max.x, 1.0);
        assert_eq!(left.width(), right.width());
        // 縦は切らない。
        for rect in [left, right, PageSlice::Full.uv_rect()] {
            assert_eq!(rect.min.y, 0.0);
            assert_eq!(rect.max.y, 1.0);
        }
    }

    #[test]
    fn only_the_halves_count_as_split_for_display_rules() {
        assert!(!PageSlice::Full.is_half());
        assert!(PageSlice::Left.is_half());
        assert!(PageSlice::Right.is_half());
    }

    /// 分割モードだと名乗るモードは、必ず向きを答えられる。
    ///
    /// `SpreadMode::is_split` と `SplitDirection::from_spread_mode` が別々に書かれて
    /// いるので、モードを増やしたときに片方だけ直すと**分割 ON なのに何も起きない**
    /// 状態になる。そこを固定する。
    #[test]
    fn every_split_mode_names_a_direction() {
        use crate::settings::SpreadMode;
        // `all()` はプルダウンに出す並びなので、網羅の根拠には使わない。
        let every_mode = [
            SpreadMode::Single,
            SpreadMode::Ltr,
            SpreadMode::LtrCover,
            SpreadMode::Rtl,
            SpreadMode::RtlCover,
            SpreadMode::Vertical,
            SpreadMode::SplitLtr,
            SpreadMode::SplitRtl,
        ];
        for mode in every_mode {
            assert_eq!(
                mode.is_split(),
                SplitDirection::from_spread_mode(mode).is_some(),
                "{mode:?} の is_split と from_spread_mode が食い違っている"
            );
        }
        assert_eq!(
            SplitDirection::from_spread_mode(SpreadMode::SplitLtr),
            Some(SplitDirection::LeftFirst)
        );
        assert_eq!(
            SplitDirection::from_spread_mode(SpreadMode::SplitRtl),
            Some(SplitDirection::RightFirst)
        );
    }

    /// 画面で見た左右が、回転に応じて元画像のどこになるか。
    ///
    /// ここを表示空間のまま渡すと、90 度回転したページで**左右ではなく上下**が切れる。
    #[test]
    fn the_halves_map_back_through_the_rotation() {
        use crate::rotation_db::Rotation;
        let left = PageSlice::Left;
        // 無回転: 画面左 = 元画像の左。
        assert_eq!(left.source_bbox(Rotation::None), left.uv_rect());
        // 時計回り 90 度で表示している = 元画像の下半分が画面左に来る。
        let mapped = left.source_bbox(Rotation::Cw90);
        assert!((mapped.min.x - 0.0).abs() < 1e-5, "{mapped:?}");
        assert!((mapped.max.x - 1.0).abs() < 1e-5, "{mapped:?}");
        assert!((mapped.min.y - 0.5).abs() < 1e-5, "{mapped:?}");
        assert!((mapped.max.y - 1.0).abs() < 1e-5, "{mapped:?}");
        // 180 度なら左右が入れ替わる。
        assert_eq!(
            left.source_bbox(Rotation::Cw180),
            PageSlice::Right.uv_rect()
        );
    }

    /// どの回転でも、左右あわせて元画像をちょうど覆う。
    #[test]
    fn the_two_halves_still_tile_the_source_under_every_rotation() {
        use crate::rotation_db::Rotation;
        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            let a = PageSlice::Left.source_bbox(rotation);
            let b = PageSlice::Right.source_bbox(rotation);
            assert!((a.area() - 0.5).abs() < 1e-5, "{rotation:?} {a:?}");
            assert!((b.area() - 0.5).abs() < 1e-5, "{rotation:?} {b:?}");
            assert!(
                !a.intersects(b) || a.intersect(b).area() < 1e-5,
                "{rotation:?}"
            );
        }
    }

    /// 分割しないときは元画像全体。回転しても全体のまま。
    #[test]
    fn a_whole_page_stays_whole_under_rotation() {
        use crate::rotation_db::Rotation;
        for rotation in [Rotation::None, Rotation::Cw90, Rotation::Cw270] {
            let full = PageSlice::Full.source_bbox(rotation);
            assert!(
                (full.min.x).abs() < 1e-5 && (full.min.y).abs() < 1e-5,
                "{full:?}"
            );
            assert!((full.max.x - 1.0).abs() < 1e-5 && (full.max.y - 1.0).abs() < 1e-5);
        }
    }

    /// 分割は見開きではない。見開き用の分岐へ紛れ込ませない。
    #[test]
    fn splitting_is_not_a_spread() {
        use crate::settings::SpreadMode;
        for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
            assert!(!mode.is_spread(), "{mode:?}");
            assert!(!mode.is_rtl(), "{mode:?}");
            assert!(!mode.has_cover(), "{mode:?}");
        }
    }

    /// 整数との往復で向きが失われない (DB へは整数で入る)。
    #[test]
    fn the_split_modes_survive_the_integer_round_trip() {
        use crate::settings::SpreadMode;
        for mode in [SpreadMode::SplitLtr, SpreadMode::SplitRtl] {
            assert_eq!(SpreadMode::from_int(mode.to_int()), mode);
        }
        // 旧版が書いた値と衝突しない。
        assert_eq!(SpreadMode::from_int(5), SpreadMode::Vertical);
        // 知らない値は既定へ倒す (新版が書いた値を旧版が読んだときの経路)。
        assert_eq!(SpreadMode::from_int(99), SpreadMode::Single);
    }
}
