# v3.5.0 第6回 再レビュー指摘

対象: `6cd99d915` (実動作の修正は `af6fe006d`)。
**残件2件: P1 1件 / P2 1件**。画像設定の初回確定先は修正されている。
owner を要求に保持して既存の mount 境界へ戻す方向は構造的に妥当。ただし、Undo の再適用と
native 動画の表示終了まで同じ所有者で完結する状態にはなっていない。

## T01 [P1 / S01残件] 確定後のUndoが、現在の窓の同じindexに別画像の補正を書き込む

- 主な場所: `src/app/gamepad_input.rs:4723–4727` (ownerへの確定と呼び出し元への復帰)、
  `src/undo_ops.rs:552–565` (Undo構築)、`402–407` (Undo / Redoの適用)。
- 条件: メイン一覧 A または画像窓 C が前面で、画像窓 B のピッカーを切断 / OFF により確定。
  A/C と B にそれぞれ別画像の個別設定があり、同じ `idx=0` を使う。確定後、操作対象の
  A/C で Ctrl+Z を押す。
- 今回の修正で初回の補正・AIモデル保存は B に届く。しかし `capture_adjust_full_inner` は
  `AdjustUndoScope::Page(0)` だけを App-global の `meta_undo` に積む。picker の `owner`
  や page key は Undo に残らない。mount から戻った後の `apply_meta_undo` は現在の App に対して
  `apply_adjustment_change_to_app` を呼ぶため、**A/C の画像を B の変更前パラメータで上書きし、
  B は変更後のまま**になる。Redo はさらに B の変更後値を A/C に書く。
- 単に「一覧と別窓の履歴が同じスタックに並ぶ」ことではない。共有スタックでも、評価のように
  保存先が固定されていれば正しく戻せる。問題は **復元対象が index しか持たず、別ページの正本を
  書き換えること**。既存の `PageKey` 復元経路はあるが、この確定では使われていない。
- 証拠: `undo_probe.py` / `undo_probe.log`。現 production の owner wrapper / image apply /
  capture / `apply_adjustment_change_to_app` を変更せず抽出し、context swap / DB / 描画を
  代用。PostFilter / UpscaleModel × B→B / B→A / B→C の6ケースで、初回確定はすべて正しい。
  BでUndoするcontrolは成功し、A/CでUndoする4ケースは上記の誤書き込みになる。
  `apply_meta_undo` (`181–229`) / Redo (`235–279`) / キー入口 (`288–320`) に owner の再選択が
  ないことも照合した。実 HWND / SQLite / キー操作を通した再現ではない。
- 新テストの盲点: `a_ring_display_effect_lands_on_the_page_the_ring_was_opened_on` は
  rootをBとして組み、Cをmountした閉包で確定した後、**Bへ戻ってから** Undoする。
  実際の切断入口を使う `losing_the_pad_commits_the_ring_into_the_window_that_opened_it`
  (`src/app/tests.rs:66158`) は Bへの確定までを検証し、その後のメイン側Undoを行わない。
- 修正境界: owner上でUndoを作る時点で復元先のpage identity、必要ならcontextも保持し、
  Undo / Redoで現在のindexに解釈し直さない。前面窓でのUndo / Redo、所有窓の消失、一覧再構成を
  同じ記録で検証する。スタックを窓ごとに分けること自体を必須とはしていない。
- `docs/detached-rework-plan.md:1489–1491` は共有Undoを延期済みR11と「同じ層」としているが、
  R11 / backlog §1.173 は **同一ページの編集後のcache失効通知**。この別ページへの書き込みを
  延期する判断は記録されていないので、その延期範囲へ自動では含めていない。

## T02 [P2 / S01の終了処理に残件] 動画ピッカーの表示解除はownerへのmountより前に送られる

- 主な場所: `src/app/gamepad_input.rs:4563–4564`、`4214–4218`、
  `src/app/native_video.rs:11640–11648`。
- 条件: active detached の動画窓 B で X ピッカーを開く。Bを開いたままメイン一覧 A を前面にし、
  モーダル等で通常dispatchが抑止されている間、または焦点移動と同じフレームで切断 / OFF。
  Bは閉じたりParkedLiveへ移したりせず、activeのままにする。
- `commit_ring_picker` はまず App-global な picker をtakeし、現在のAで
  `clear_native_video_picker_overlay` を呼ぶ。Aの `fullscreen_idx=None` なら native setterは
  何もしない。その後でBをmountするが、その閉包に入っているのは **設定のapplyだけ**で、
  Bのplayerへ表示解除は送られない。結果、App上のpickerは終了済みなのに、Bのnative HUDに
  ピッカーが残る。パッドを無効にしていると、そのピッカーを操作して閉じることもできない。
- 証拠: `overlay_probe.py` / `overlay_probe.log`。現 production の dispatch / commit /
  owner wrapper / video apply / clear / native setterを変更せず抽出し、native playerの
  保持状態をbooleanで代用。Bで終了するcontrolは `closed=true / native_visible=false`。
  Aで終了すると `closed=true / native_visible=true`。実D3D11/HWND描画は行っていない。
- native presenterの `set_ring_picker_overlay` は保持値を更新するsetterで、自動消去の期限を
  持たない (`src/video/native_presenter/render_core.rs:8572–8579`)。
  `poll_parked_live_detached_windows` に別の表示同期はあるが、対象はParkedLiveのみ
  (`src/app.rs:39235–39245`, `39716–39733`)。上記のactive Bには当たらず、
  `update_active_viewer_context` の毎フレーム処理もpicker終了のNoneを同期しない。
- 新8テストにはnative動画pickerの表示保持を観測するケースがない。
- 修正境界: native表示の終了を、開いたcontext / presenterが所有するライフサイクルへ戻す。
  設定値の適用だけをownerに戻しても表示リソースは片付かない。B→A / B→C、通常確定 / 切断 /
  OFF、active / ParkedLive、所有窓消失を確認する。無関係な前面playerへの一括clearで置き換えない。

前回S01の「別画像へ初回確定される」現象自体は解消している。上記はその後のUndo / 表示終了の
残件として分けて記録し、このcommitだけで新たに作った退行とは断定しない。
