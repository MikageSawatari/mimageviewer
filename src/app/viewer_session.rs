use super::{ViewerPresentation, ViewerSyncStamp};

/// ビューア 1 セッションの表示先と detached 連携状態。
///
/// 現段階では、表示中のセッションは `App` の既存フィールドへマウントされ、退避中の
/// セッションだけがこの型を直接所有する。`swap_with_mounted` を唯一の交換境界にすることで、
/// 表示先・同期 stamp などの交換漏れを防ぐ。窓 ID は session に保存せず、
/// ViewerContextRegistry の予約 / binding から導出する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ViewerSession {
    pub(super) presentation: ViewerPresentation,
    pub(super) last_sync_stamp: Option<ViewerSyncStamp>,
    pub(super) independent_active: bool,
    pub(super) open_next_still_detached_once: bool,
}

impl Default for ViewerSession {
    fn default() -> Self {
        Self {
            presentation: ViewerPresentation::Fullscreen,
            last_sync_stamp: None,
            independent_active: false,
            open_next_still_detached_once: false,
        }
    }
}

impl ViewerSession {
    /// 退避中の session と、現在 `App` にマウントされている session 状態を一括交換する。
    pub(super) fn swap_with_mounted(
        &mut self,
        presentation: &mut ViewerPresentation,
        last_sync_stamp: &mut Option<ViewerSyncStamp>,
        independent_active: &mut bool,
        open_next_still_detached_once: &mut bool,
    ) {
        std::mem::swap(&mut self.presentation, presentation);
        std::mem::swap(&mut self.last_sync_stamp, last_sync_stamp);
        std::mem::swap(&mut self.independent_active, independent_active);
        std::mem::swap(
            &mut self.open_next_still_detached_once,
            open_next_still_detached_once,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(idx: usize, item_key: &str, items_generation: u64) -> ViewerSyncStamp {
        ViewerSyncStamp {
            idx,
            item_key: item_key.to_owned(),
            items_generation,
        }
    }

    #[test]
    fn default_session_is_fullscreen_and_unlinked_from_a_detached_window() {
        let session = ViewerSession::default();

        assert_eq!(session.presentation, ViewerPresentation::Fullscreen);
        assert_eq!(session.last_sync_stamp, None);
        assert!(!session.independent_active);
        assert!(!session.open_next_still_detached_once);
    }

    #[test]
    fn mounted_state_swap_round_trips_every_session_field() {
        let detached_stamp = stamp(4, "detached", 40);
        let main_stamp = stamp(2, "main", 20);
        let detached = ViewerSession {
            presentation: ViewerPresentation::DetachedWindow,
            last_sync_stamp: Some(detached_stamp.clone()),
            independent_active: true,
            open_next_still_detached_once: true,
        };
        let mut stored = detached.clone();
        let mut presentation = ViewerPresentation::MainWindow;
        let mut last_sync_stamp = Some(main_stamp.clone());
        let mut independent_active = false;
        let mut open_next_still_detached_once = false;

        stored.swap_with_mounted(
            &mut presentation,
            &mut last_sync_stamp,
            &mut independent_active,
            &mut open_next_still_detached_once,
        );

        assert_eq!(presentation, detached.presentation);
        assert_eq!(last_sync_stamp, Some(detached_stamp));
        assert!(independent_active);
        assert!(open_next_still_detached_once);
        assert_eq!(stored.presentation, ViewerPresentation::MainWindow);
        assert_eq!(stored.last_sync_stamp, Some(main_stamp));
        assert!(!stored.independent_active);
        assert!(!stored.open_next_still_detached_once);

        stored.swap_with_mounted(
            &mut presentation,
            &mut last_sync_stamp,
            &mut independent_active,
            &mut open_next_still_detached_once,
        );

        assert_eq!(stored, detached);
        assert_eq!(presentation, ViewerPresentation::MainWindow);
        assert_eq!(last_sync_stamp, Some(stamp(2, "main", 20)));
        assert!(!independent_active);
        assert!(!open_next_still_detached_once);
    }

    #[test]
    fn independent_detached_identity_tuple_round_trips_through_the_mounted_projection() {
        let mut session = ViewerSession {
            presentation: ViewerPresentation::DetachedWindow,
            last_sync_stamp: Some(stamp(1, "keep", 10)),
            independent_active: true,
            open_next_still_detached_once: false,
        };
        let mut presentation = ViewerPresentation::Fullscreen;
        let mut last_sync_stamp = None;
        let mut independent_active = false;
        let mut open_next_still_detached_once = true;

        session.swap_with_mounted(
            &mut presentation,
            &mut last_sync_stamp,
            &mut independent_active,
            &mut open_next_still_detached_once,
        );

        assert_eq!(presentation, ViewerPresentation::DetachedWindow);
        assert!(independent_active);
        assert!(!open_next_still_detached_once);
        assert_eq!(last_sync_stamp, Some(stamp(1, "keep", 10)));
    }
}
