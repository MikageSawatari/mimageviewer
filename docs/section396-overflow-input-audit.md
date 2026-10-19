# §1.396 overflow と静止画 raw pointer 入力の監査

対象は `render_fullscreen_viewport` の静止画描画と、その前後で実行する入力処理。
`pointer.*`（`primary_clicked` / `any_click` 等を含む）、`PointerButton`、
`Response::interact_pointer_pos` と呼出先を検索し、テスト用入力生成と描画だけの参照を分けた。
§1.403 で左・右・中・touch の前面所有へ一般化した。Extra ボタン・wheel は従来の契約を保つ。

`capture_fs_front_pointer_input` が root の早期描画とキー処理・navigator 予約より前に一度だけ所有を確定する。
現在のパネル矩形を egui の layer hit-test に通し、押下位置の最前面が overflow の Area のときだけ
左・右・中・touch の押下を解放までパネルが所有する。提示中の F / S / R / P / Q / fullscreen context menu と
対象 viewport の egui memory popup は、位置によらず新規押下を所有する。パネルを閉じるフレームや、キーで
別モードへ入るフレームでも同じ `FsFrontPointerFrameInput` を各 consumer へ渡す。
矩形・layer を各 consumer で判定し直さない。外側開始の入力は従来どおり扱う。
overflow だけなら touch 除外も同じ pass の最前面領域を使う。メニュー提示中 / touch 所有中は全 canvas を
touch recognizer の除外へ渡す（メニュー項目の Response は維持）。
初回 / sizing の未登録・非 interactive な Area は現在の矩形で補うが、既存の Foreground 以上の
hit を上書きしない。Area の登録順序は変更しない。閉じた直後は現在の矩形がないため、
egui に前 pass の Area が残っていても新規所有しない。
シーク設定 popup は毎 pass 描画し、選択は従来の Response に任せる。
物理列は App 全体の前面一時状態 `FsFrontPointerOwnership` が保持し、context bundle には移さない。
旧 `OverflowStream` / `FullscreenSecondaryPress::Overflow` は廃止した。同 viewer のページ移動では終わらず、
物理 release、最後の touch End / Cancel、所有 viewport の focus loss、既存の前面 handoff / close だけで解放する。
他 viewport の入力・focus loss はその所有を変更しない。同一 pass の旧 release → 新 press は新列として判定する。
double-click は前面所有境界以降の背面受理 click と egui の click count の両方で成立させる。

以下の consumer 表の所有結果は一般化後の `FsFrontPointerFrameInput` を指す。追加接続は
中ボタン短押し / drag zoom と範囲 capture。隠蔽のパイプライン前 hold は同じ owner を viewport 一致で参照する。

| 背面処理 | overflow との到達（§1.396 の棚卸し） | 一般化後の所有結果・変更 / 除外根拠（関数が参照点） |
| --- | --- | --- |
| ナビゲータの予約・直接処理・予約後処理、右範囲選択ズーム | 可 | `ui_fullscreen.rs::handle_fs_wheel_and_click_frame` から `without_front_buttons` 済み raw snapshot を全入口へ渡す。ボタン edge / down だけを除き位置・focus・wheel・非所有ボタンは保つ（既存修正）。 |
| 比較ワイプ境界 | 可 | `handle_compare_wipe_drag` の呼出前に確定済み primary 所有を参照。 |
| 通常パン・Ctrl 回転・連結読みスクロール・panorama drag・double click / ページクリック | 可 | `apply_fs_wheel_frame_proposal` の `primary_suppression` 分岐が全 primary 操作より前。確定済み primary 所有は touch 終端でも上書きしない（既存修正）。 |
| 右クリック・リング・ジェスチャ | 可 | `recognize_fs_secondary_input` が確定済み secondary 所有を参照。既存右操作を primary press で中断する入口も、前面所有の primary は配送しない。 |
| 中ボタン短押し / drag zoom | 可 | `update_mouse_middle_short_click` の開始と `handle_middle_drag_zoom` を確定済み middle 所有で除く。ページ移動で保持を失わず、終端後の新規列を受理する。 |
| 分析のパン・ガイド作成・右クリックの filter / 色固定 | 可 | `draw_analysis_panel` へ同じ結果を渡し、primary 各 edge / down と secondary press をボタン別に除く。分析は `fullscreen_top_bar_chrome_allowed` の除外対象ではない。 |
| 補正パネルのブックマーク行 | 可 | `draw_adjustment_panel` → `draw_bookmark_panel_body` へ同じ結果を渡して raw `primary_clicked` を除く。現在位置の `contains_pointer` だけでは press 所有にならず、狭い窓の重なりで行へ漏れ得た。 |
| 消しゴム | モード中は不可、キー入場の同一 pass は可 | `handle_erase_paint` へ同じ結果を渡し primary / secondary を個別に除く。従来の `fs_primary_suppression.is_active` だけでは終端 pass と secondary 所有を扱えない。 |
| 隠蔽 canvas | 同上 | `handle_conceal_paint` へ同じ結果を渡し primary / secondary を個別に除く。 |
| テキスト注釈 | 同上 | `handle_text_canvas_input` へ同じ結果を渡し primary 操作を除く。 |
| 補正レイヤー canvas | 同上 | `handle_local_adjust_canvas_input` へ同じ結果を渡す。準備中の bucket click を含め primary / secondary を個別に除く。 |
| 切り取り | overflow とは通常同時到達なし。編集パネルの memory popup とは共存 | `draw_export_crop_overlay` → `handle_export_crop_pointer_with_holds_and_front` へ同じ結果を渡し、所有した primary press / down / release を除く。通常表示で残った crop 枠は描画だけ。非所有の既存 drag の継続条件は維持する。 |
| SNS 分割 | モード中は不可、キー入場の同一 pass は可 | `draw_sns_split_overlay` へ同じ結果を渡し primary 所有中は raw 操作を実行しない。 |
| 隠蔽の先行 preview hold | 前面 press を保持して隠蔽へ入った後に可 | `sync_conceal_preview_before_pipeline` は子 viewport の per-pass capture より前に必要なため、前 pass で確定した同じ `FsFrontPointerOwnership` を参照して raw down を除く。`enter_conceal_mode` は eye rect を消し、隠蔽中は overflow を開けないので、新規 press の再判定は不要。 |
| シークバー・サムネイル列・固定 / 表示部品 | 可 | `draw_fullscreen_seek_overlay_with_geometry` へ同じ結果を渡し raw down・gesture 更新・クリック確定を除く。もともと操作開始は Response 所有だが、確定結果へも接続する。 |
| touch ページ送り・chrome・pan / pinch コマンド | 可 | recognizer は左右ボタンと共有する最前面の overflow 領域で終端まで駆動（`handle_fs_wheel_and_click_frame` の通常 / 前面復帰経路）。コマンド配送も確定済み primary 所有を参照し、早期 close 後に再利用しない。外側開始の連続操作は維持。 |
| 範囲キャプチャ | overflow 上からの通常開始はなし。Q と共存可能 | `draw_capture_region_selection_overlay` へ同じ結果を渡し、所有中は primary の event / move を配送しない。focus / キーの取消は維持する。開始時の `wait_for_release` と通常の新規選択は維持する。 |
| metadata の類似画像 / 本の対応帯 hold preview | 可 | `ui_metadata_panel.rs::draw_page_strip` / `draw_similar_hit_buttons` は `is_pointer_button_down_on` と Response の位置を要求する。それに加え `draw_metadata_panel` → `draw_metadata_panel_inner` の共通適用境界へ同じ結果を渡し、primary 所有中は両 producer の `peek_press` を開始しない。raw 位置による fallback は診断用 observation だけで、hover・診断・既存 preview の終端監視は維持する。 |
| ルーペ・ZipPla 照準・分析 hover 色・各筆カーソル・ナビゲータのカーソル | 可、押下による背面操作ではない | hover 位置 / 表示用 down を読むだけ（例 `src/ui_fullscreen.rs:47721` のルーペ）。画像・選択・編集の入力を発生させないため、既存の描画を維持する。 |
| 表示トリム | 可、raw 単独の操作開始なし | `src/ui_view_trim.rs:1312` / `1810` の raw down は保存バッチ終了時期の判定のみ。値操作は Response が所有する。 |
| filename stack | 独立 consumer なし | `filename_stack_ui.rs` に fullscreen 用 raw pointer handler はない。キーの `stack_jump`と、通常の fullscreen ナビゲーション / マウス割当 / リング経路へ合流する。 |
| HUD popup、カーソル activity、フォーカス復帰 | 可、画像操作とは別 | popup の raw click は自分の outside-dismiss（例 `src/ui_fullscreen.rs:49854`）、activity はカーソル復帰（`26436`）、focus は前面 widget を使うための復帰（`37767`）。後段の画像操作には上の所有結果を適用する。 |

編集6モードは `ui_conceal.rs::is_overlay_edit_mode_active` に含まれるため、通常は
`fullscreen_top_bar_chrome_allowed` が false となり overflow を開けない。ただし
`handle_fs_key_input_with_operation` はメニューを閉じたキーを通常処理へ渡すため、モード排他だけでは
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

## 最前面 layer 判定の追補

1093×614 の実描画でスライドショーパネルと下部フィルムボタンのシーク設定 popup を同時に開く。
描画された項目の位置と実 layer を検査し、次の回帰を追加した。

| テスト | 検査対象 |
| --- | --- |
| `overflow_top_layer_seek_popup_accepts_real_mouse_and_touch_row_selection` | 重なったシーク設定の高さ項目をマウス / touch で選択、設定更新と popup 終了 |
| `overflow_top_layer_secondary_press_does_not_claim_the_higher_seek_popup` | 同じ重なり位置の右押下を overflow が所有しない |
| `overflow_nonoverlapping_front_region_still_shields_the_real_wipe_with_seek_popup_open` | popup と重ならないパネル部分では実比較ワイプを遮断、touch 除外にも含む |
| `overflow_first_open_and_sizing_pass_claim_each_button_before_the_area_can_interact` | 初回・1 / 2 pass、左右 / touch の所有と背面パンの遮断 |
| `overflow_closed_state_does_not_reclaim_buttons_or_touch_from_a_stale_area` | 閉鎖直後に egui の Area が残っても新規左右 / touch を所有しない |
| `overflow_surface_matches_top_area_at_popup_edges_and_adjacent_floats` | popup の四辺・角・隣接 f32、線・点・外側からの接触を実 layer hit-test と比較 |

`Rect::contains` は両端を含むため、領域分割の外側片は `next_down` / `next_up` で共有辺を持たない。
popup の辺上を含め、ボタンと touch で同じ最前面判定を保つ。
閉鎖直後に前 pass の widget が最初の押下を保持する egui の既存の Response 寿命は変更しない。
閉鎖テストは新規 overflow 所有なし / touch 除外領域が空であることを直接検査し、
その押下の解放後に新しい canvas 操作が有効であることを対照にする。

### §1.396 時点で範囲外だった別 popup と背面の関係（履歴）

以下はソース比較による到達経路で、実アプリでの再現確認は行っていない。
overflow を閉じた状態でも存在し、`fb14a73a1` の同じ呼出条件 / raw 入力処理を確認した。
§1.396 では別 popup 全般の入力所有を変更せず、設計担当へ引き継いだ。下表の3経路は
§1.403 で前面所有へ統合し、実描画テストの ignore を解除した。共通 modal Window / VST manager / gamepad picker /
上部バー・360品質ボタンの余白 / AI status・toast は引き続き対象外。

| 操作手順 | 根拠 |
| --- | --- |
| 静止画を拡大し右上ナビゲータを表示 → 下部フィルムボタンからシーク設定を開く → popup とナビゲータが重なる位置で押下 / ドラッグ | `handle_fs_wheel_and_click_frame` の navigator 予約 / 直接処理は modal のみを除外。`fs_navigator_allowed_for_display_unit` に別 popup gate はない。基点の `ui_fullscreen.rs:37424` / `37439` も同じ。 |
| 横長画像で比較ワイプを開始し、境界を popup が重なる位置へ移す → シーク設定等を開く → 境界上で左ドラッグ | `handle_compare_wipe_drag` の raw primary とその呼出条件。基点の `ui_fullscreen.rs:35148` / `38249` でも `cursor_in_panel` のみで、別 popup gate はない。 |
| 分析表示で回転 / フィット等の popup を画像領域へ重ねる → 画像と重なる部分で左ドラッグ / 修飾右押下 | `draw_analysis_panel` の raw primary / secondary 入力。基点の `ui_analysis_panel.rs:852` 以降にも別 popup gate はない。分析中はシークを表示しないため、上 HUD の popup を使う。 |

## raw 入力横断監査時（`5e9e7a5c3` 後）の変更ファイル

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

最前面 layer 判定の追補は `ui_fullscreen.rs`、同ディレクトリの `overflow_input_tests.rs` /
`wheel_frame_tests.rs`、本監査表、display-pipeline、backlog に限定する。
