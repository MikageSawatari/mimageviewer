# v3.5.0 第5回 再レビュー指摘

対象: `7127fd14f7046024b34bea76b324922f78e36b91`。
Q01 のコンテナ評価の保存先・Undo payload は解消、Q02 も解消。
ただし、Q01 と同じ終了要求の所有境界に **1 件の残件 (P1)** がある。
延期承認済みの R07 / R08 / R09 / R10 / R11 は再指摘していない。

## S01 [P1 / Q01 と同根の残件] 切断時の画像設定は今も現在の別窓へ確定され、エフェクトのUndoも元値を失う

- 主な場所: `src/app/gamepad_input.rs:4568–4570`、`4711–4716`、`4828–4842`。
- 今回の固定 target が保護するのは評価行。`commit_ring_picker` は評価とその Undo の後、
  そのまま `apply_ring_picker_state` を呼ぶ。画像の設定はここで **現在 mount 中の
  `fullscreen_idx`** を引き直して適用し、開いた viewer / page / adjustment scope を使わない。
- 再現条件: B / C で異なる画像の個別補正を持つ。画像ウィンドウ B で X ピッカーを開き、表示エフェクトまたは AI
  アップスケールを選ぶ。ピッカーを閉じる前に、別の画像ウィンドウ C が配送先になった状態で
  パッドを切断する、またはゲームパッドを無効化する。前回 Q01 と同じく、モーダル等で通常の
  dispatch が抑止された間、または活性変更と切断が同じフレームに入った場合、stale 検査より
  先に確定が走る (`2392–2393`)。グローバルな picker は context bundle の swap 対象ではない。
- 表示エフェクトでは C に B の選択値を保存するだけでなく、確定前に C の値を
  **B の `original.post_filter` / `original.colorize` に戻してから**
  `capture_adjust_full` に渡す (`4828–4839`)。そのため Undo に入る変更前値も C の本来の値では
  なくなる。例: B は None → Sepia、C は別のエフェクトの場合、C が Sepia になり、Undo しても
  None に戻るだけで C の元のエフェクトを復元できない。
- AI モデルの選択も C の現在の scope へ保存される (`4841–4842` → `4970–4987`)。
  この行はプレビューを行わず確定時だけ適用するため (`3825` 付近)、B の選択は B に残らない。
  配送先がメイン一覧 A (`fullscreen_idx=None`) なら画像設定の確定全体がスキップされる。
  エフェクトは B のプレビューだけ、AI モデルは未適用のまま picker が消える。
- 証拠: `flow_probe.py` / `flow_probe.log`。現 production の dispatch / commit / finalize /
  image apply / post-filter preview・保存 / AI 保存 / `capture_adjust_full_inner` の関数本体を
  **変更せず抽出**し、HWND、描画、DB 書き込みの境界を代用品にして検査した。
  B→B は B のみ保存され正しい Undo。B→C は C のエフェクトまたは AI モデルが変わり、
  エフェクトの Undo.before は B の元値。B→A は画像設定の保存・Undo がない。
  同じ 6 ケースで、今回直したコンテナ評価の保存先・Undo payload はすべて B のままである。
  実 HWND / ゲームパッド / SQLite による通しの操作再現ではない。
- 到達性は `gamepad_batch_goes_to_active_context` (`631`)、root / active への配送
  (`src/app.rs:68134–68174` 付近)、`ring_picker_is_stale` の呼び出し箇所
  (`3077`, `3135`) と終了時確定の順序を照合した。通常の入力を抑止しても終了時確定は走る。
- 新テスト `a_ring_container_rating_lands_on_the_folder_it_was_opened_in`
  (`src/app/tests.rs:65570`) は Grid picker の評価 finalize / Undo を直接呼ぶので、
  `commit_ring_picker` から続く画像設定の適用を通らない。
- 修正境界: picker を開いた所有 context と変更対象・変更前値を確定まで保ち、
  **既存の編集を終える要求**を現在の前面への新規入力と区別する。評価だけでなく全 dirty row
  の確定、元のプレビューの後始末、Undo を同じ所有者上で完結させる。
  B→B / B→A / B→C、通常確定 / OFF / 切断、PostFilter / UpscaleModel、所有窓を閉じた場合を
  一緒に検証する必要がある。stale として単に捨てる修正では R12 の Undo 欠落へ戻る。

これは延期済み R11 (同一ページの編集結果を他の viewer の cache に通知する問題) とは異なる。
**操作していない別ページの正本を書き換える問題**であり、今回その延期範囲へは追加していない。
