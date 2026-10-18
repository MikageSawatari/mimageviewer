# §1.396 overflow と静止画 raw pointer 入力の監査

対象は `render_fullscreen_viewport` の静止画描画と、その前後で実行する入力処理。
`pointer.*`（`primary_clicked` / `any_click` 等を含む）、`PointerButton`、
`Response::interact_pointer_pos` と呼出先を検索し、テスト用入力生成と描画だけの参照を分けた。
中ボタン・Extra ボタン・wheel の既存操作は今回の左右ボタン所有へ巻き込まない。

`capture_fs_overflow_pointer_input` (`src/ui_fullscreen.rs:39455`) が root の早期描画より前に一度だけ所有を確定する。
パネル内で開始した左右の押下は解放までパネルが所有する。パネルを閉じるフレームや、キーで
別モードへ入るフレームでも同じ `FsOverflowPointerFrameInput` を各 consumer へ渡す。
矩形を各 consumer で判定し直さない。外側開始の入力は従来どおり扱う。

| 背面処理 | overflow 所有と同時到達 | 所有結果・変更 / 除外根拠（関数が参照点） |
| --- | --- | --- |
| ナビゲータの予約・直接処理・予約後処理、右範囲選択ズーム | 可 | `ui_fullscreen.rs::handle_fs_wheel_and_click_frame` (`src/ui_fullscreen.rs:37600`) から `without_overflow_buttons` (`src/ui_fullscreen.rs:2623`) 済み raw snapshot を全入口へ渡す。ボタン edge / down だけを除き位置・focus・wheel・非所有ボタンは保つ（既存修正）。 |
| 比較ワイプ境界 | 可 | `handle_compare_wipe_drag` (`src/ui_fullscreen.rs:35396`) の呼出前に確定済み primary 所有を参照。 |
| 通常パン・Ctrl 回転・連結読みスクロール・panorama drag・double click / ページクリック | 可 | `apply_fs_wheel_frame_proposal` (`src/ui_fullscreen.rs:38918`) の `primary_suppression` 分岐が全 primary 操作より前。確定済み primary 所有は touch 終端でも上書きしない（既存修正）。 |
| 右クリック・リング・ジェスチャ | 可 | `recognize_fs_secondary_input` (`src/ui_fullscreen.rs:39517`) が確定済み secondary 所有を参照。既存右操作を primary press で中断する入口も、前面所有の primary は配送しない。 |
| 分析のパン・ガイド作成・右クリックの filter / 色固定 | 可 | `draw_analysis_panel` (`src/ui_analysis_panel.rs:839`) へ同じ結果を渡し、primary 各 edge / down と secondary press をボタン別に除く。分析は `fullscreen_top_bar_chrome_allowed` (`src/ui_fullscreen.rs:23505`) の除外対象ではない。 |
| 補正パネルのブックマーク行 | 可 | `draw_adjustment_panel` (`src/ui_adjustment_panel.rs:16322`) → `draw_bookmark_panel_body` (`src/ui_adjustment_panel.rs:15864`) へ同じ結果を渡して raw `primary_clicked` を除く。現在位置の `contains_pointer` だけでは press 所有にならず、狭い窓の重なりで行へ漏れ得た。 |
| 消しゴム | モード中は不可、キー入場の同一 pass は可 | `handle_erase_paint` (`src/ui_erase.rs:1616`) へ同じ結果を渡し primary / secondary を個別に除く。従来の `fs_primary_suppression.is_active` だけでは終端 pass と secondary 所有を扱えない。 |
| 隠蔽 canvas | 同上 | `handle_conceal_paint` (`src/ui_conceal.rs:1320`) へ同じ結果を渡し primary / secondary を個別に除く。 |
| テキスト注釈 | 同上 | `handle_text_canvas_input` (`src/ui_text.rs:2423`) へ同じ結果を渡し primary 操作を除く。 |
| 補正レイヤー canvas | 同上 | `handle_local_adjust_canvas_input` (`src/ui_adjustment_panel.rs:12666`) へ同じ結果を渡す。準備中の bucket click を含め primary / secondary を個別に除く。 |
| 切り取り | パネル内 primary 開始との通常同時到達なし | 製品入口は補正パネルの「切り取り」Button の Response click のみ（`src/ui_adjustment_panel.rs:16698`、キー / ring 入口なし）。モード中は topbar / overflow を描かない。通常表示で残った crop 枠は `draw_export_crop_overlay` (`src/ui_crop.rs:522`) の `!export_crop_mode` で描画だけ。ボタン入場は crop 描画より後で、次 pass では press edge が終了済み、drag も入場時に空となる。既存 drag の継続条件を変えない。 |
| SNS 分割 | モード中は不可、キー入場の同一 pass は可 | `draw_sns_split_overlay` (`src/ui_sns_split.rs:1115`) へ同じ結果を渡し primary 所有中は raw 操作を実行しない。 |
| 隠蔽の先行 preview hold | 前面 press を保持して隠蔽へ入った後に可 | `sync_conceal_preview_before_pipeline` (`src/ui_conceal.rs:1961`) は子 viewport の per-pass capture より前に必要なため、前 pass で確定した同じ `OverflowStream` を参照して raw down を除く。`enter_conceal_mode` は eye rect を消し、隠蔽中は overflow を開けないので、新規 press の再判定は不要。 |
| シークバー・サムネイル列・固定 / 表示部品 | 可 | `draw_fullscreen_seek_overlay_with_geometry` (`src/ui_fullscreen.rs:24132`) へ同じ結果を渡し raw down・gesture 更新・クリック確定を除く。もともと操作開始は Response 所有だが、確定結果へも接続する。 |
| touch ページ送り・chrome・pan / pinch コマンド | 可 | recognizer は既存の overflow 除外矩形で終端まで駆動（`src/ui_fullscreen.rs:38385` 付近の適用境界）。通常 / 前面復帰のコマンド配送も確定済み primary 所有を参照し、早期 close 後に再利用しない。外側開始の連続操作は維持。 |
| 範囲キャプチャ | パネル内 primary 開始との通常同時到達なし | 製品入口は Ctrl + 上部カメラの Response click だけ（`src/ui_fullscreen.rs:49520` → `28021`）。カメラと overflow は上部バー下の 12pt margin で離れ、capture 中は `fullscreen_top_bar_chrome_allowed` (`src/ui_fullscreen.rs:23505`) が false。開始時の押下は `src/ui_fullscreen.rs:52488` の `wait_for_release` で消費する。外側 primary による開始まで遮断しない。 |
| metadata の類似画像 / 本の対応帯 hold preview | 可 | `ui_metadata_panel.rs::draw_page_strip` (`src/ui_metadata_panel.rs:4650`) / `draw_similar_hit_buttons` (`src/ui_metadata_panel.rs:4920`) は `is_pointer_button_down_on` と Response の位置を要求する。それに加え `draw_metadata_panel` (`src/ui_metadata_panel.rs:2046`) → `draw_metadata_panel_inner` (`src/ui_metadata_panel.rs:2293`) の共通適用境界へ同じ結果を渡し、primary 所有中は両 producer の `peek_press` を開始しない。raw 位置による fallback は診断用 observation だけで、hover・診断・既存 preview の終端監視は維持する。 |
| ルーペ・ZipPla 照準・分析 hover 色・各筆カーソル・ナビゲータのカーソル | 可、押下による背面操作ではない | hover 位置 / 表示用 down を読むだけ（例 `src/ui_fullscreen.rs:47721` のルーペ）。画像・選択・編集の入力を発生させないため、既存の描画を維持する。 |
| 表示トリム | 可、raw 単独の操作開始なし | `src/ui_view_trim.rs:1312` / `1810` の raw down は保存バッチ終了時期の判定のみ。値操作は Response が所有する。 |
| filename stack | 独立 consumer なし | `filename_stack_ui.rs` に fullscreen 用 raw pointer handler はない。キーの `stack_jump`（`src/ui_fullscreen.rs:35243`）と、通常の fullscreen ナビゲーション / マウス割当 / リング経路へ合流する。 |
| HUD popup、カーソル activity、フォーカス復帰 | 可、画像操作とは別 | popup の raw click は自分の outside-dismiss（例 `src/ui_fullscreen.rs:49854`）、activity はカーソル復帰（`26436`）、focus は前面 widget を使うための復帰（`37767`）。後段の画像操作には上の所有結果を適用する。 |

編集6モードは `ui_conceal.rs::is_overlay_edit_mode_active` (`src/ui_conceal.rs:73`) に含まれるため、通常は
`fullscreen_top_bar_chrome_allowed` (`src/ui_fullscreen.rs:23505`) が false となり overflow を開けない。ただし
`handle_fs_key_input_with_operation` (`src/ui_fullscreen.rs:32985`) はメニューを閉じたキーを通常処理へ渡すため、モード排他だけでは
同一 pass の保護にならない。キーから入れる上表の編集入口にも確定済み所有を渡す理由はこの遷移である。

検証は headless の実 `render_fullscreen_viewport` を用いる。実アプリは起動しない。
結果・ログは backlog §1.396 へ記録する。

## 追加した実描画回帰

`src/ui_fullscreen/overflow_input_tests.rs` に以下を追加した。前面で所有した入力による背面状態の不変と、
新規の外側開始入力による従来操作の両方を検査する。

| テスト名 | 検査対象 |
| --- | --- |
| `overflow_real_wipe_drag_respects_panel_press_origin_in_both_directions` | 横長画像、ナビゲータ OFF、実比較ワイプ境界、左右両方向の内外ドラッグ |
| `overflow_real_analysis_primary_pan_and_guide_preserve_outside_operations` | 分析の左パン / Shift ガイド作成 |
| `overflow_real_analysis_secondary_filter_is_blocked_only_for_the_owned_button` | 分析の右クリック filter / 色固定 |
| `overflow_same_pass_text_entry_does_not_reuse_the_old_panel_press` | メニューを閉じて注釈へ入る同一 pass |
| `overflow_real_bookmark_row_keeps_press_ownership_after_release_moves_outside` | 640pt 幅でパネルから 4pt 外へ出るブックマーク行クリック |
| `overflow_same_pass_local_adjust_entry_preserves_owned_press_and_outside_brush` | 補正レイヤーへ入る同一 pass と外側 brush |
| `overflow_held_press_cannot_become_conceal_pipeline_eye_preview_after_mode_entry` | 隠蔽 canvas の mask とパイプライン前の eye hold |
| `overflow_same_pass_erase_entry_preserves_owned_press_and_outside_brush` | 消しゴムへ入る同一 pass と外側 brush |
| `overflow_same_pass_sns_entry_preserves_owned_press_and_outside_drag` | SNS 分割へ入る同一 pass と外側 drag |
| `overflow_real_seek_ignores_panel_press_but_accepts_a_fresh_outside_click` | シークの押下所有と新規外側クリック |
| `overflow_real_touch_menu_key_dismissal_keeps_the_physical_contact_owned` | キーでメニューを閉じた元 touch の遮断と次 contact の pan |
| `overflow_primary_press_does_not_cancel_an_outside_secondary_ring_or_gesture` | 外側開始の右操作と前面左押下の独立性 |
| `overflow_real_metadata_hold_preview_requires_an_unowned_new_press` | 実 Similar タブと前面パネルの重なり、key dismiss 中の hold と解放後の新規プレビュー開始 |

## 今回の変更ファイル

- `src/ui_fullscreen.rs`
- `src/ui_analysis_panel.rs`
- `src/ui_adjustment_panel.rs`
- `src/ui_metadata_panel.rs`
- `src/ui_erase.rs`
- `src/ui_conceal.rs`
- `src/ui_text.rs`
- `src/ui_sns_split.rs`
- `src/ui_fullscreen/overflow_input_tests.rs`
- `src/app/page_edit_release_tests.rs`
- `src/app/tests.rs`
- `docs/display-pipeline.md`
- `docs/next-release-backlog.md`
- `docs/section396-overflow-input-audit.md`（新規）

作業中に追加された `docs/detached-rework-plan.md` の既存差分はこの変更に含めず、そのまま保持した。
