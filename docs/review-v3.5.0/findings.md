# 指摘と根拠

対象 HEAD / 範囲は README.md を参照。アプリコードの修正は行っていない。
最終確認 `512b49d4dbb1fb3d64801458c629d769331bf881`。未解消 15 件 (P1: 1 / P2: 13 / P3: 1)。
「静的」はコード上の不変条件違反を確認した意味で、実機再現や所要時間測定を行った意味ではない。
各項目の根本修正は提案であり、実装・検証済みの解決策ではない。

## F01 [P1・解消済み] マージ後の export 統合テストがコンパイルできない

- `tests/export_integration.rs:613` の `BakedEditSnapshot` 初期化に `ai` / `creative_lut` / `stage` が欠ける。
- `scripts/test-full.ps1` で E0063 を確認。単体テストだけでは見つからず、リリースゲートを阻害する。
- 根本修正: 共通 snapshot の変更を全 producer に反映し、この integration test の意図に合う焼き込み段を明示する。
- 証拠: `test-full.log`。runner timeout ではなく Rust の型検査エラー。
- レビュー中の別作業で `512b49d4d` が入り、欠けた 3 field が追加された。更新後の `scripts/test-full.ps1` は PASS (`test-full-current.log`)。現在の未解消指摘には含めない。

## F02 [P2] 関連付けアプリの Batch で一部失敗を全件成功として返す（確定・静的）

- `src/open_with.rs` の `shell_execute_with_progid` は各 path の `ShellExecuteExW` 成功を `launched_any` へ OR し、1 件でも成功すれば `Some(true)`。
- 呼び出し元 `invoke_association_handler_inner` はそれを集合全体の `Ok` とし、`external_tool.rs::run_materialize_launch_operation` は全 path を成功数へ計上し、全 temp を process 所有へ移す。
- 再現条件: Association + Batch、複数ファイルのうち 1 件が Shell 起動エラー。成功通知だけで失敗対象が分からない。
- 根本修正: path ごとの結果を保持する。既に成功した path は再起動せず、失敗 path だけの通知 / fallback / temp 所有権を確定する。単なる `all()` への置換は成功済みの二重起動を起こす。
- 検証: fake Shell executor で success / failure の混在と全失敗、成功済み非再実行、成功数、temp ownership を固定する。

## F03 [P2] 焼き込み段階の UI が未実装の動作を選択できる（確定・静的、既知計画あり）

- `preferences/pages.rs::page_bake_stage_body` は製本 / 単枚 / 一括 / 外部ツールの 3 段を全て操作可能として表示する。
- `ui_fullscreen.rs::book_baked_edit_snapshot` と `materializer.rs::load_page_edits` は `ai: None` 固定。AI / DisplayAdjust を選んでも AI 拡大・デノイズは実行されない。
- 単枚 export の `bake_stage_export` は保存/UI以外で利用されず、表示画素が常に入力となる。浅い段を選んでも AI / カラー化等を除外できない。Merged external tool も同じ表示画素経路で段を無視するが、こちらの例外は現在の設定画面に明記されている。
- 計画 `bake-stage-unification-plan.md` §5 は段取り 4–6 未完、既存 `review-status-bake-stage.md` にも Merged の問題を記載。レビュー時点でも未接続がユーザー UI に露出。
- 根本修正: source + 編集 snapshot + shared AI model policy を全 output に通し、stage ごとの実出力を検証する。仕様制限での対処は利用者判断が必要。

## F04 [P2] 一括 export の準備が全件分の同期 DB / mask 展開を UI で行う（確定・静的）

- `ui_dialogs/export_batch.rs::open_export_batch_dialog` → `grid_batch_export_items` → 全対象の `book_baked_edit_snapshot` が同期実行。
- snapshot は mask/conceal の `dimensions` / `get_full` (圧縮 payload 展開)、local-adjust 同期 load、comic font 準備等を行う。worker 起動は「書き出す」ボタンを押した後で、ダイアログが表示される前の全件準備を保護しない。
- 再現条件: 多数の画像へ隠蔽/消しゴム/部分補正を付けて一覧から Ctrl+E。UI が全件分の DB・展開完了まで応答せず、キャンセルもできない。
- 根本修正: UI の未保存編集 override と軽量 identity だけを固定し、DB snapshot / mask / font 準備を worker に移す。単枚の既存関数を N 回呼ぶ構造を解消する。

## F05 [P2] 見開き外部ツールの合成と全画素 SHA が UI を占有（確定・静的、既知）

- `external_tool.rs::merged_spread_target` が UI 入力 handler 内で `render_export_pixels` と全画素 SHA-256 を完了してから materializer を起動する。
- 高解像度見開きでは合成/回転/crop/コピー/hash が画素数に比例し、進捗 modal と cancel が届かない。
- 既知指摘 #8 の未解消を確認。F03 と共通の source snapshot → worker composite 境界で解決する。

## F06 [P2] ゲームパッド OFF が保持入力を解除せず、アナログ操作と repaint が続く（確定・静的）

- `app/gamepad_input.rs:2286` は無効時に空のイベント列を受け取るだけで、`gamepad_state` を clear しない。続く `due_button_repeat` と `dispatch_gamepad_analog` は以前の pressed / axis 状態を利用する。`gamepad_dispatch_allowed` にも enabled 判定はない。
- 再現条件: スティックがずれている状態で設定を OFF にする。設定を閉じると、最後の軸入力による移動・ズーム等と repaint が続き、停止済みスレッドから中立イベントは届かない。
- modal 中の `suppress_pending_actions` は button の repeat 予約を消すが、軸と button 保持状態自体は意図的に残す。したがって「全てのボタンが必ず repeat を続ける」という指摘ではない。OFF の遷移が consumer state を終了していない点が原因。
- 根本修正: デバイス有効状態の遷移で runtime と consumer の保持状態・操作 UI を一緒に終了し、OFF 中は repeat / analog / repaint を生成しない。単なるイベント読み捨てにしない。
- 現テストは空の保持状態で OFF → ON するだけ。axis 非ゼロ・repeat 待機・リング保持の状態から OFF、OFF 中の再描画、ON 後の新規入力を回帰テストにする。
- 関連: `gamepad.rs::stop` は UI thread から `join` する。通常でも polling sleep の最大 16ms、初期化中ならデバイス初期化の終了を待つ。停止要求と thread 回収は UI を待たせない所有者へ分離する。

## F07 [P2] 非表示からキーでストリップを戻すと「全体」の選択が失われる（確定・静的）

- `app/native_video.rs::requested_video_seek_strip_view` は `current.span()` を復元値として使うが、`SeekStripView::Hidden.span()` は常に Window。`VideoSeekStripToggle` / 直接指定 Thumbnails / Waveform は保存済みの `settings.video_seek_strip_span` を受け取らない。
- 再現: メニューで「波形 (全体)」→ Toggle キーで閉じる → 同じキーで開くと「波形 (周辺)」へ変わる。上ドラッグの `open_video_seek_strip` は保存済み span を使うため、入力経路で結果が異なる。
- 根本修正: 非表示の表示状態と「最後に選んだ内容・範囲」を区別し、復元操作の共通 resolver に保存済み showing を渡す。非表示の layout 用既定値を利用者の選択に流用しない。

## F08 [P1] 一括編集の結果を別 viewer の runtime へ反映できる（確定・静的）

- `edit_bundle_bulk.rs::BulkPageEditPending` / target は App-global で viewer context identity を持たない。`show_bulk_page_edit_dialog` は root (`app.rs:68227`) と fullscreen (`ui_fullscreen.rs:16675`) の両方から poll する。関数冒頭 (`edit_bundle_bulk.rs:1223`) に所有 context の判定はない。
- `apply_bulk_worker_success` はその時に mount 中の items で index を引き直し、`commit_page_edit_bundle_to_runtime` へ渡す。`adjustment_page_params` / `mask_pages` / local-adjust / crop などは context bundle のフィールドで、他 viewer へ更新を配送しない。
- 再現条件: 独立 F12 viewer を残し、一覧側で一括貼付/解除。別窓側の描画が結果を先に drain すると、一覧側のキャッシュ・保持設定が更新されない。別窓に対象が無ければ idx=None のまま DB だけ変わり、回転込み reset は bundle 適用後に対象不明で失敗し得る。
- 元機能の単一貼付にも同型の所有境界がある。新しい一括 owner へ同じ前提を拡張しており、単一路の成功テストでは保証できない。
- 根本修正: request に origin context を固定してそこで完了処理を行い、同一 page key を保持する sibling には mutation として明示的な失効を配送する。入力 index だけによる undo 破棄も対象 context/key に結び付ける。detached BA-7 (状態所有) の問題であり、viewport ごとの条件追加で塞がない。

## F09 [P2] 新しい出力 worker が Remote のローカル AI 停止確認から漏れる（確定・静的）

- `app.rs::local_ai_remote_barrier_snapshot` は既存の final AI / erase / local-adjust / 製本 / `LocalAiActivityLease` 等だけを数え、batch export と external materializer は含まない。両新 worker は lease も取得しない。
- しかし両経路は `BookEraseRunner` → `ui_erase::erase_from_saved_mask` を実行する。materializer は既存 AiRuntime の clone または独自 DirectML runtime を保持し、Remote acquire 後も実行を継続できる。
- `remote_ipc/ui.rs:813` はこの snapshot が静止なら RemoteActive へ進むため、消しゴム処理つきの一括出力/外部起動中に接続するとローカル AI が残ったまま操作権を移す。Remote 側の AI と GPU/モデル資源利用が競合する。実機での発生頻度・性能影響は未測定。
- 根本修正: 出力 worker の AI resource lifetime を既存の activity lease / acquire barrier に登録し、cancel・終了・孤児 worker の回収まで ownership を維持する。単にダイアログを閉じて空きと見なさない。

## F10 [P2] スタック展開時にページ固有の Creative LUT が解決されない（確定・静的）

- `external_tool.rs:1982` は一覧 index のない stack member に favorite/global params を仮置きし、同 1989 の LUT もその値から解決する。
- materializer は `load_page_params_from_db` により worker でページ固有 params を読み直すが、`materializer.rs:785` では仮置き params 由来の `context.creative_lut` をそのまま合成へ渡す。
- 再現: スタック内ページに個別 LUT A、全体に LUT 無しまたは B を設定し、「補正済み一時ファイル」+ 表示補正まで焼き込みで起動。単ページ閲覧時と異なる色で出力される。
- 根本修正: effective params と LUT 実体を同じ snapshot の所有者が解決する。UI で LUT registry の軽量 snapshot を渡し、worker が確定した ID に対応する LUT を選ぶ等。ページ固有/親継承/未解決 LUT のテストが必要。
- 同じ helper の親設定解決にも不一致: `stack_member_default_params` は `find_nearest_favorite` の 1 件しか見ない。内側のお気に入りが設定 OFF、外側が ON なら、表示の `active_favorite_default_id_for_idx` は外側 ON を使うが、外部出力は global へ落ちる。共通 effective-params resolver へ統合する際にこの入れ子条件も固定する。

## F11 [P2] 狭い音楽ビューの固定パネルがウィンドウ外へはみ出す（確定・数値検証）

- `ui_fullscreen.rs:37141` は予約後の `rect.width()` から元の幅を `min(430, (width + 430)/2)` で復元する。元幅が 860pt 未満では、この式は実際に予約した幅の逆算にならない。
- 幅640pt (本体の最小幅) は予約320pt・残り320pt。描画側は予約375ptと誤認して右端695ptへ配置する。幅800ptでも右へ15ptはみ出し、右端の鍵ボタン等が欠ける。
- 根本修正: full rect と予約済み panel rect を同じ layout snapshot から渡す。clamp 後の content width から元の値を推測しない。
- 証拠: `geometry_probe.py` / `geometry-probe.log`。UI実機は未実施。

## F12 [P2] 負座標を跨ぐ連結読みで 0.5px の丸めが共有境界を分離する（確定・計算関数実行）

- `continuous_reading_page_rects` (`ui_fullscreen.rs:4344`) はページごとに原点を `round()`。`vertical_reading_offsets` は辺長を揃えるが、Rust の round は負の .5 を負側、正の .5 を正側へ丸める。コメントの「整数平行移動と丸めは可換」はこの tie で成立しない。
- 例: 501×1001 の縦画像を等倍で縦連結し、スクロールして先頭 unit の中心を0へ置く。gap0なら次の中心は1001。ページ原点 -500.5 / +500.5 が -501 / +501 となり、先頭下端500・次の上端501で1pxの隙間。gap1は2px、gap20は21px。100/125/150/200%でも該当例を確認。
- 高解像度→表示用テクスチャ差替えの共通寸法化は適切だが、配置する原点の丸めまで共同所有されていない。unit間の共有辺を一度丸め、そこから整数の可視長・gapを積む設計、または符号を跨いでも整数平行移動と可換な格子丸めが必要。
- 証拠: `geometry_probe.py` は実ソースの extent / snap / offsets 関数をそのまま抽出してコンパイル。単ページ・trim無しの配置算術を検証したもので、アプリ全体のUI実行ではない。
- 既存の描画回帰テストは横長の短いページを開始位置で検査する。縦長・奇数高さ・負原点・0.5境界・スクロール中を production draw test に追加する。

## F13 [P2] 右クリックのたびに関連付け一覧を UI thread で列挙する（確定・静的）

- `ui_dialogs/context_menu.rs:1243,1287` のメニュー組み立ては `SHAssocEnumHandlers` / `Next` / `GetUIName` / `GetName` を同期実行する。`cached_handlers` は native menu を閉じるたびに同 851 で消されるので、同じ拡張子でも次の右クリックでは再列挙する。
- v3.4.0 の既定 native menu に無かった mIV 側の関連付け列挙が統一メニューの前処理へ追加されている。Shell 拡張・関連付け先・ネットワーク状態の遅延中はメニュー自体がまだ表示されず、UI が待つ。`QueryContextMenu` を遅延してもこの前処理は残る。実機での所要時間は未測定。
- 根本修正: 環境設定の `start_external_tool_handler_enumeration` と同様に worker で拡張子ごとの候補 snapshot を作り、メニューは準備済み snapshot を使う。明示更新/関連付け変更時に失効させる。項目削除で軽くする対処はしない。

## F14 [P2] 外部変更の修正がファイル更新ごとの全フォルダ走査を UI へ広げる（確定・静的）

- `app.rs:17710,17888` の `ExternalChangeCheck::Notified` は directory mtime gate を迂回し、同 17907 付近の `scan_directory_with_settings` → `app/folder_scan.rs::scan_directory_entries` を UI thread で実行する。
- 同名ファイルの上書きを directory mtime では検出できない、という原因の特定は正しい。ただし画像の上書き/連続保存の通知ごとに、全項目の列挙・名前変換・分類・signature 計算を同期実行するようになる。大きなフォルダや遅い共有先では表示と入力を止める。既存の debounce は発火回数をまとめるだけで、発火後の UI 待ちを解消しない。
- 根本修正: 監視通知から context / folder / generation 付きの再走査要求を worker へ渡し、完了を所有者へ適用する。mtime gate を元へ戻すと上書き検出の不具合が再発するため、それは修正にならない。
- `DirEntry` の cached metadata を使う点は適切。問題は追加 syscall の数ではなく、全件処理とネットワーク待ちの実行 thread。

## F15 [P2] 旧外部アプリの移行失敗後、通常保存で「移行済み」が確定する（確定・障害注入で再現）

- `settings_db.rs:599-611` は移行の I/O / SQL 失敗を記録して load を続行し、空の `external_tools` を持つ Settings を返す。「marker を残さず次回 load で再試行する」という契約。
- しかし `save_full` は同 848 でその空リストを書き、同 873-876 で無条件に移行 marker を書く。移行時だけ一時的に書き込みが失敗し、その後テーマ変更等の通常保存が成功すると、再起動時には移行が済んだ扱いになり、旧登録アプリが戻らない。旧 table は残るが利用者の UI から登録が失われる。
- 根本修正: 新規 DB bootstrap と、旧 DB の未完了 migration を保存境界で区別する。未完了なら保存前に移行を完遂するか、未移行の外部ツール領域と marker を通常保存で確定しない。
- 回帰条件: 旧 table に登録あり → migration の INSERT/marker を一度失敗させる → 障害解除 → 別の設定を保存 → 再 load。登録が保持されること。現在の成功/一度だけ/未知 enum テストにこの lifecycle が無い。
- `settings_migration_probe.rs` で公開 `SettingsDb` API を使用し、一時 DB の migration INSERT のみを trigger で失敗させた。失敗後は legacy=1 / external=0、障害解除後の通常保存・再 load も legacy=1 / external=0 / marker=1。`settings-migration-probe.log`。通常の `%APPDATA%` は使用していない。

## F16 [P3] 統一メニューのキー表示が操作カスタマイズに追随しない（確定・静的）

- `context_menu_model.rs:288-289,342,477-478` は `左に回転 (L)` / `右に回転 (R)` / `選択解除 (Ctrl+D)` を固定文字列で生成する。
- 該当 KeyAction を別キーへ変更/解除しても、native / fallback 双方のメニューに以前のキーが出る。共通モデルに移したことで、キー表示を実割り当てから作る既存方針との不一致が双方へ固定されている。実行コマンド自体の誤配線ではない。
- 根本修正: メニュー入力 snapshot に該当 action の解決済みラベルを持たせる。モデル内へ既定キーを再記述しない。

## 確定指摘にしなかった事項 / 残る確認

- 情報パネル固定を見ない旧 `metadata_panel_click_shown || hover_active` 述語が mouse click / cursor / touch handle に残る。一方、wheel と touch classifier は実効 visible、パネル背景は click を消費するので、これだけで click-through の発生を断定しない。固定を hover から行い、カーソルを左へ出した後の cursor hide / touch handle を実機で確認する。
- 一括編集の Remote 接続取得との競合（AI barrier 以外の DB mutation / Remote cache の境界）。
- 一括 export の cancel token は item 間だけ参照される。マニュアルが「まだ始まっていない分は作成されません」と規定するため、実行中 item の完了待ちは今回の不具合指摘から除外。ただし大きな AI item では待ちが長いことは実機確認事項。
- `seek_strip_wheel_is_consumed_and_becomes_one_range_step` に `#[test]` がなく、重複 `#[test]` も警告される。v3.4.0 にも同じ登録漏れがあるため、新規退行としては数えない。今回の動画 wheel の検証に使えていない既存 coverage の穴として記録する。
