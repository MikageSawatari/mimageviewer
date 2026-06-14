use super::*;

// scroll_settle 計装の helper 群は inherent impl (trait method ではない)
impl App {
    /// `App::update` 冒頭で 1 回呼ぶ。同フレーム内の wheel / arrow keys / Page / Home / End
    /// 等のスクロール入力意図を ctx.input から拾って `last_prefetch_scroll_at` を即時更新する。
    ///
    /// なぜ早期検出か: `update_keep_range_and_requests` の gate 判定 (line 18204 付近) は
    /// 同フレーム内の `process_scroll` / `handle_keyboard` の `scroll_offset_y` mutate より前に
    /// 走るので、offset 変化ベースの検出 (= `update_scroll_settle_state`) では 1 フレ遅れる。
    /// → input intent ベースなら gate 判定時に既に立っている。
    ///
    /// scrollbar drag / touch などキー以外の経路は `update_scroll_settle_state` の offset 変化
    /// fallback で 1 フレ遅れて拾う。実害は 1 frame の prefetch 漏れ程度で、次フレ q.retain で
    /// prune される。
    pub(super) fn detect_scroll_input_intent(&mut self, ctx: &egui::Context) {
        let scrolling = ctx.input(|i| {
            i.raw_scroll_delta.length() > 0.1
                || i.key_pressed(egui::Key::ArrowDown)
                || i.key_pressed(egui::Key::ArrowUp)
                || i.key_pressed(egui::Key::PageDown)
                || i.key_pressed(egui::Key::PageUp)
                || i.key_pressed(egui::Key::Home)
                || i.key_pressed(egui::Key::End)
        });
        if scrolling {
            self.last_prefetch_scroll_at = Some(std::time::Instant::now());
        }
    }

    /// `update_scroll_settle_state`: scroll_offset_y の変化検出 + 300ms 経過後の
    /// settle イベント発火 + first/all ready 計測の更新。
    /// `App::update` の終盤 (= render_grid 反映後) で呼ぶ。
    pub(super) fn update_scroll_settle_state(&mut self) {
        let now = std::time::Instant::now();
        let cur_offset = self.scroll_offset_y;
        if (cur_offset - self.prev_scroll_offset_y).abs() > 0.5 {
            self.last_scroll_event_at = Some(now);
            // prefetch gate にも fallback で書く (= scrollbar drag / touch 経路の保険)。
            // settle で clear されないので backstop の計時起点に使える。
            self.last_prefetch_scroll_at = Some(now);
            self.prev_scroll_offset_y = cur_offset;
            // 新しいスクロールが始まったので前 settle 状態を破棄
            self.scroll_settle_state = None;
        }

        // settle 判定: 最後の scroll から 300ms 経過 + scroll_settle_state 未発火
        const SETTLE_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(300);
        let should_emit_settle = self
            .last_scroll_event_at
            .is_some_and(|t| now.saturating_duration_since(t) >= SETTLE_THRESHOLD)
            && self.scroll_settle_state.is_none();

        if should_emit_settle {
            self.emit_scroll_settle_event();
        }

        // settle 後の first/all ready チェック (poll_thumbnails で更新済みの thumbnails 状態を見る)
        self.check_visible_thumb_ready();
    }

    /// scroll_settle イベントを 1 回 emit + scroll_settle_state を初期化。
    fn emit_scroll_settle_event(&mut self) {
        // 現フレームの visible 範囲を計算 (render_grid 反映後なのでこの値で正)
        let cell_h = self.last_cell_h.max(32.0);
        let viewport_h = self.last_viewport_h.max(cell_h);
        let cols = self.settings.grid_cols.max(1);
        let rows_per_page = (viewport_h / cell_h).ceil() as usize;
        let items_per_page = (rows_per_page * cols).max(1);
        let vis_count = self.visible_indices.len();
        if vis_count == 0 {
            return;
        }
        let vis_first_raw = (self.scroll_offset_y / cell_h) as usize * cols;
        let vis_first = vis_first_raw.min(vis_count.saturating_sub(1));
        let vis_end = vis_first.saturating_add(items_per_page).min(vis_count);

        // 厳密 visible の raw idx 集合 + 既 Loaded 集合 (= 二重カウント防止、Codex P2-3)
        let visible_target: HashSet<usize> = self.visible_indices[vis_first..vis_end]
            .iter()
            .copied()
            .collect();
        let loaded_at_settle: HashSet<usize> = visible_target
            .iter()
            .copied()
            .filter(|i| {
                matches!(
                    self.thumbnails.get(*i),
                    Some(crate::grid_item::ThumbnailState::Loaded { .. })
                )
            })
            .collect();
        let already_loaded = loaded_at_settle.len();

        self.scroll_settle_seq = self.scroll_settle_seq.wrapping_add(1);
        let seq = self.scroll_settle_seq;
        let settled_at = std::time::Instant::now();
        let target_count = visible_target.len();

        if crate::perf::is_enabled() {
            // **注 (Codex follow-up)**: perf::event の seq 引数 (= 4 番目) は input
            // correlation 用で `seq` field に書かれる。scroll_settle 自体は入力イベント
            // ではないので `0` で送る。settle 識別は `settle_seq` field を使う
            // (first_ready / all_ready 側の `settle_seq` と join 可能)。
            let mut fields: Vec<(&str, serde_json::Value)> = vec![
                ("settle_seq", serde_json::Value::from(seq)),
                (
                    "visible_target_count",
                    serde_json::Value::from(target_count),
                ),
                ("visible_first_idx", serde_json::Value::from(vis_first)),
                ("already_loaded", serde_json::Value::from(already_loaded)),
                (
                    "vis_first_raw_idx",
                    serde_json::Value::from(
                        self.visible_indices.get(vis_first).copied().unwrap_or(0),
                    ),
                ),
            ];
            if let Some(snap) = crate::pdf_loader::pool_queue_snapshot() {
                fields.push(("pool_critical", serde_json::Value::from(snap.critical)));
                fields.push((
                    "pool_high_normal",
                    serde_json::Value::from(snap.high_normal),
                ));
                fields.push(("pool_normal", serde_json::Value::from(snap.normal)));
                fields.push(("pool_in_flight", serde_json::Value::from(snap.in_flight)));
            }
            crate::perf::event("ui", "scroll_settle", None, 0, &fields);
        }

        // already-loaded で全部済んでいれば first_ready/all_ready は emit 不要 (latency=0 扱い)
        if already_loaded >= target_count {
            // この window は emit せず終了 (latency=0)
            self.scroll_settle_state = None;
            self.last_scroll_event_at = None;
            return;
        }

        self.scroll_settle_state = Some(ScrollSettleState {
            seq,
            settled_at,
            visible_target,
            loaded_at_settle,
            newly_loaded: HashSet::new(),
            first_ready_emitted: false,
            all_ready_emitted: false,
        });
        // 次の scroll を待つ (settle 済みなので last_scroll_event_at は clear)
        self.last_scroll_event_at = None;
    }

    /// settle window 内で visible 範囲の thumbnail が Loaded 化したか確認、
    /// first_ready / all_ready を emit。
    fn check_visible_thumb_ready(&mut self) {
        let Some(state) = self.scroll_settle_state.as_mut() else {
            return;
        };
        if state.all_ready_emitted {
            return;
        }

        // 新規 Loaded 化を集める。
        // **Codex P2-3 対応**: settle 時点で既に Loaded だった idx (= loaded_at_settle に
        // 入ってる) は newly_loaded に含めない。さもないと all_ready の二重カウントで
        // 早期発火 (未ロードが残ってるのに all_ready) する。
        let mut newly: Vec<usize> = Vec::new();
        for &idx in state.visible_target.iter() {
            if state.loaded_at_settle.contains(&idx) {
                continue; // settle 時点で既に Loaded → 二重カウント防止
            }
            if state.newly_loaded.contains(&idx) {
                continue;
            }
            if matches!(
                self.thumbnails.get(idx),
                Some(crate::grid_item::ThumbnailState::Loaded { .. })
            ) {
                newly.push(idx);
            }
        }
        if newly.is_empty() {
            // all_ready 判定: settle 時点で全部 loaded だったケース (= target ⊆ loaded_at_settle)
            // は emit_scroll_settle_event 側で state を None にして早期 return しているので、
            // ここに来るのは「未 loaded が残ってる」ケース。
            // ただし「target が空」のエッジケースだけは保護する。
            if state.visible_target.is_empty() && !state.all_ready_emitted {
                state.all_ready_emitted = true;
            }
            return;
        }
        for idx in &newly {
            state.newly_loaded.insert(*idx);
        }
        // first_ready
        if !state.first_ready_emitted {
            state.first_ready_emitted = true;
            if crate::perf::is_enabled() {
                let latency_ms = state.settled_at.elapsed().as_secs_f64() * 1000.0;
                crate::perf::event(
                    "ui",
                    "visible_thumb_first_ready",
                    None,
                    0, // input correlation 不要、settle_seq で join
                    &[
                        ("settle_seq", serde_json::Value::from(state.seq)),
                        ("latency_ms", serde_json::Value::from(latency_ms)),
                        ("idx", serde_json::Value::from(*newly.first().unwrap_or(&0))),
                        (
                            "already_loaded_at_settle",
                            serde_json::Value::from(state.loaded_at_settle.len()),
                        ),
                        (
                            "target_count",
                            serde_json::Value::from(state.visible_target.len()),
                        ),
                    ],
                );
            }
        }
        // all_ready: loaded_at_settle ∪ newly_loaded が target を覆ったか
        let total_covered = state.loaded_at_settle.len() + state.newly_loaded.len();
        if total_covered >= state.visible_target.len() && !state.all_ready_emitted {
            state.all_ready_emitted = true;
            if crate::perf::is_enabled() {
                let latency_ms = state.settled_at.elapsed().as_secs_f64() * 1000.0;
                crate::perf::event(
                    "ui",
                    "visible_thumb_all_ready",
                    None,
                    0,
                    &[
                        ("settle_seq", serde_json::Value::from(state.seq)),
                        ("latency_ms", serde_json::Value::from(latency_ms)),
                        (
                            "target_count",
                            serde_json::Value::from(state.visible_target.len()),
                        ),
                    ],
                );
            }
        }
    }

    /// `pdf_loader::pool_queue_snapshot` を 1 秒 tick で emit (frame-driven、pool 未初期化なら skip)。
    pub(super) fn maybe_emit_pool_queue_snapshot(&mut self) {
        if !crate::perf::is_enabled() {
            return;
        }
        let now = std::time::Instant::now();
        let elapsed = self
            .last_pdf_pool_snapshot_at
            .map(|t| now.saturating_duration_since(t))
            .unwrap_or(std::time::Duration::MAX);
        if elapsed < std::time::Duration::from_secs(1) {
            return;
        }
        let Some(snap) = crate::pdf_loader::pool_queue_snapshot() else {
            // pool 未初期化 → emit せず last_at も更新しない (次フレームで再チェック)
            return;
        };
        self.last_pdf_pool_snapshot_at = Some(now);
        crate::perf::event(
            "pdf",
            "pool_queue_snapshot",
            None,
            0,
            &[
                ("critical", serde_json::Value::from(snap.critical)),
                ("high_normal", serde_json::Value::from(snap.high_normal)),
                ("normal", serde_json::Value::from(snap.normal)),
                ("in_flight", serde_json::Value::from(snap.in_flight)),
                (
                    "in_flight_age_max_ms",
                    serde_json::Value::from(snap.in_flight_age_ms_max),
                ),
                (
                    "in_flight_age_p95_ms",
                    serde_json::Value::from(snap.in_flight_age_ms_p95),
                ),
                (
                    "in_flight_age_p50_ms",
                    serde_json::Value::from(snap.in_flight_age_ms_p50),
                ),
            ],
        );
    }

    /// 旧 `eframe::App::on_exit` の中身。trait impl は 1 つしか書けないので、
    /// scroll_settle helper 群を inherent impl に逃がす都合上、本体を inherent
    /// メソッドに移して trait impl 側から委譲する。
    pub(super) fn on_exit_inner(&mut self) {
        // VST3 プラグイン内部状態 (= EQ カーブ / chunk) と GUI ウィンドウ位置 / サイズを
        // bridge から snapshot して settings.json に永続化する。終了前に取らないと再起動時に
        // 全部 default に戻る。bridge teardown は eframe の Drop で走るので、ここで先に
        // query して結果を settings に書き込んでから持続化する順序が必要。
        #[cfg(windows)]
        {
            self.sync_native_video_main_cloak(false);
            // T22: 終了経路は早く抜けたい (ユーザーが close ボタンを押した文脈) ので 2 秒
            // で打ち切る。timeout した slot は前回保存の state を保持する
            let states = self.snapshot_vst3_states_into_settings(std::time::Duration::from_secs(2));
            let positions = self.snapshot_vst3_window_positions_into_settings();
            if states > 0 || positions > 0 {
                self.settings.save();
            }
        }
        self.stop_video_upscale_queue_for_exit();
        // legacy タグ worker は detach thread なので、終了時はキャンセルを立てて
        // ファイル境界で止める (特に ImportAndRemove はユーザーファイルを書き換える)。
        if let Some(pending) = self.tag_legacy_seed_pending.as_ref() {
            pending.cancel();
        }
        if let Some(pending) = self.tag_legacy_xmp_pending.as_ref() {
            pending.cancel();
        }
        self.persist_window_state_and_flush();
    }
}
