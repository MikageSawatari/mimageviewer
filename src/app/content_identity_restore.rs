use super::App;

impl App {
    /// Worker が DB の最終状態から作った edit bundle を、既存の in-memory sidecar owner へ
    /// 反映する。disk flush は従来どおり `flush_all_sidecars` の既存境界だけが行う。
    pub(crate) fn apply_content_restore_sidecar_mirrors(
        &mut self,
        mirrors: Vec<crate::content_identity::RestoreSidecarMirror>,
    ) {
        for mirror in mirrors {
            let coords = (mirror.folder, mirror.rel_key);
            self.with_sidecar_coords_mut(Some(&coords), move |sidecar, rel_key| {
                sidecar.replace_edit_bundle(rel_key, mirror.entry)
            });
        }
    }

    /// グリッド badge / smart-folder 集計が参照する global page-key presence を増分反映する。
    pub(crate) fn apply_content_restore_presence(
        &mut self,
        presence: crate::content_identity::RestorePresence,
    ) {
        self.adjusted_page_keys.extend(presence.adjusted);
        self.mask_page_keys.extend(presence.masks);
        self.conceal_page_keys.extend(presence.conceals);
        self.local_adjust_page_keys
            .extend(presence.local_adjustments);
        self.comic_page_keys.extend(presence.comics);
    }

    /// 復元完了後の idx-keyed edit state と rating / tag cache の共通 invalidation。
    /// A3b は worker report の sidecar / presence を適用した後にこの関数を呼ぶ。
    pub(crate) fn finish_content_identity_restore(&mut self, errors: Vec<String>) {
        self.finish_book_page_edit_mapping("restore", errors);
    }
}
