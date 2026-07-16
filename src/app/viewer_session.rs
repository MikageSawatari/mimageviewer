use super::{ViewerPresentation, ViewerSyncStamp};

/// ビューア 1 セッションの表示先と detached 連携状態。
///
/// 現段階では、表示中のセッションは `App` の既存フィールドへマウントされ、退避中の
/// セッションだけがこの型を直接所有する。`swap_with_mounted` を唯一の交換境界にすることで、
/// 表示先・同期 stamp・detached window ID などの交換漏れを防ぐ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ViewerSession {
    pub(super) presentation: ViewerPresentation,
    pub(super) last_sync_stamp: Option<ViewerSyncStamp>,
    pub(super) independent_active: bool,
    pub(super) open_next_still_detached_once: bool,
    pub(super) detached_window_id: Option<u64>,
}

impl Default for ViewerSession {
    fn default() -> Self {
        Self {
            presentation: ViewerPresentation::Fullscreen,
            last_sync_stamp: None,
            independent_active: false,
            open_next_still_detached_once: false,
            detached_window_id: None,
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
        detached_window_id: &mut Option<u64>,
    ) {
        std::mem::swap(&mut self.presentation, presentation);
        std::mem::swap(&mut self.last_sync_stamp, last_sync_stamp);
        std::mem::swap(&mut self.independent_active, independent_active);
        std::mem::swap(
            &mut self.open_next_still_detached_once,
            open_next_still_detached_once,
        );
        std::mem::swap(&mut self.detached_window_id, detached_window_id);
    }

    pub(super) fn activate_independent_detached(&mut self, window_id: u64) {
        self.presentation = ViewerPresentation::DetachedWindow;
        self.independent_active = true;
        self.open_next_still_detached_once = false;
        self.detached_window_id = Some(window_id);
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
        assert_eq!(session.detached_window_id, None);
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
            detached_window_id: Some(91),
        };
        let mut stored = detached.clone();
        let mut presentation = ViewerPresentation::MainWindow;
        let mut last_sync_stamp = Some(main_stamp.clone());
        let mut independent_active = false;
        let mut open_next_still_detached_once = false;
        let mut detached_window_id = None;

        stored.swap_with_mounted(
            &mut presentation,
            &mut last_sync_stamp,
            &mut independent_active,
            &mut open_next_still_detached_once,
            &mut detached_window_id,
        );

        assert_eq!(presentation, detached.presentation);
        assert_eq!(last_sync_stamp, Some(detached_stamp));
        assert!(independent_active);
        assert!(open_next_still_detached_once);
        assert_eq!(detached_window_id, Some(91));
        assert_eq!(stored.presentation, ViewerPresentation::MainWindow);
        assert_eq!(stored.last_sync_stamp, Some(main_stamp));
        assert!(!stored.independent_active);
        assert!(!stored.open_next_still_detached_once);
        assert_eq!(stored.detached_window_id, None);

        stored.swap_with_mounted(
            &mut presentation,
            &mut last_sync_stamp,
            &mut independent_active,
            &mut open_next_still_detached_once,
            &mut detached_window_id,
        );

        assert_eq!(stored, detached);
        assert_eq!(presentation, ViewerPresentation::MainWindow);
        assert_eq!(last_sync_stamp, Some(stamp(2, "main", 20)));
        assert!(!independent_active);
        assert!(!open_next_still_detached_once);
        assert_eq!(detached_window_id, None);
    }

    #[test]
    fn activating_independent_detached_sets_the_complete_identity_tuple() {
        let mut session = ViewerSession::default();
        session.last_sync_stamp = Some(stamp(1, "keep", 10));

        session.activate_independent_detached(37);

        assert_eq!(session.presentation, ViewerPresentation::DetachedWindow);
        assert!(session.independent_active);
        assert!(!session.open_next_still_detached_once);
        assert_eq!(session.detached_window_id, Some(37));
        assert_eq!(session.last_sync_stamp, Some(stamp(1, "keep", 10)));
    }
}
