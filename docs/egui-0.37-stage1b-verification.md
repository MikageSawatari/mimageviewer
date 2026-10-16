# egui 0.37 更新・段1b 検証台帳

## 段1b-1: App の責務抽出（2026-10-16）

開始: `egui-036` / `a5f8887b5`。実装・検証 owner は今回の Codex 実装担当。
[更新計画第4.2版](egui-0.37-update-plan.md) §1・§4.2・§4.11・段1bに従う。
Rust 1.99.0 / egui 0.33.3 / wgpu 27.0.1を維持し、manifest / lock / vendorを変更しない。
段1b-2・1b-3、非表示logic接続、新Close hookとbackend退役は実装しない。
製品起動・live試験・製品build・`test-full.ps1`・PNG更新・コミット・branch操作・pushは行わない。
開始時に変更表示のあった `src/video/dsp/mod.rs` は対象外で、SHA-256
`2f099cbba3855cbce0f5ae1a5034b9ccea668b0ff6cfae98e8b5bca2a09a8ae0` のバイト列を保護する。

### 実装境界と不変条件

`src/app/update_phases.rs` に連続ブロックを関数として抽出した。
`App::update` と `update_frame` は旧順序・条件・早期returnを保持する呼出driverである。
serviceを先頭へ集めず、UIが作ったmodalやnavigationとの前後関係を維持する。
`MainFrameNavigation` は既存のframe-local変数を渡す引数束だけで、Appのフィールド・pending・queueを増やさない。
実装担当の内部確認で、移動body内に呼出元へ抜けるreturn / break / continueがないことも確認した。

`service_*` はUI入力を読まない進行の断片、`input_*` はpass入力・診断、
`render_*` は描画とUIに依存する準備/worker採用継続、`post_render_*` は描画後の処理である。
**`render_*` にworker pollがあることは意図的**。worker pollという名前だけではlogicへ移せない。
transitiveな表示継続の依存が確認できた入口は、非表示serviceと誤認しないようrender側へ分類した。
レビュー追補でF13にもUI継続が判明した。`service_initial_worker_results`という既存名も
logic-readyの保証にはならず、今回の修正では製品コード/名前を変更せず台帳で明示する。
下記は段3の接続予定であり、今回logic-only動作を実装・検証したことは意味しない。

簡素化:既存modal、registry/mount、終了ownerをそのまま使う。
非表示用の第二poll owner、保存終端の前倒し、callbackの偽造、追加repaint、guard / retry / delayは設けない。
今回のために変換/保存の新しい並行組合せを作らない。

### 分類と実行順

表は上から旧実行順。Rはrootで現在投影されるviewer（通常main）、Mは既存ownerが一時mountするcontext、
PはParkedLive media、AはApp-global。rootのContextとviewerのcontext identityは別概念である。
`logic候補` も、採用先のregistry/mountと既存gateを保って段3で接続する。呼出を一括前倒ししない。
`ui継続あり` は下記の未解消依存を整理するまでlogicへ直接接続できない。

| 順 | 入口 / 残したdriver処理 | 分類・内容 | 進行先 | 0.37 接続予定 |
| --- | --- | --- | --- | --- |
| O1 | diagnostics export poll → begin_grid_click_input_frame | service → input | A / R | logic / ui |
| O2 | startup visibility / present watch / owner / notice / geometry / deferred size | serviceと実表示ACK・frame操作の既存順序 | A / root HWND | owner pollはlogic候補、表示ACK・geometry frameはui |
| O3 | service_update_registered_writers | page-edit/rating/tag repaint登録とreconciliation | A / R、既存registry | logic候補 |
| O4 | clipboard / captureの既存poll | process clipboard snapshotと選択 | A | 段1b-3で整理、今回順序不変 |
| O5 | render_update_before_frame | pin refresh prune、collection UI、mounted document owner | A / R | ui継続あり（C3） |
| O6 | defer_*_root_close 3本 | settings/composition/ZIP modal中のroot Close消費 | root | ui。子Close hookとは別契約 |
| O7 | non-Windows preview poll、poll_image_folder_zip | 元のcontextでcancel/worker/変換進行 | R / operation起点のM | 非表示候補。ただし採用継続を含め段3で再監査 |
| F1 | render_update_begin | tray wake ACK、heartbeat、Remote進行、modal診断 | A / R、既存Remote close owner | ui継続あり（C1） |
| F2 | input_update_begin → input_test_script_actions | sidecar中入力消費、keyboard pass owner、診断featureのaction | R | ui（test-script cfg維持） |
| F3 | service_creative_luts → detect_scroll_input_intent | LUT完成採用 → 同passのscroll intent | A / R | logic候補 → ui |
| F4 | heartbeat gap → service_edit_completions | 診断、brush deferred、local-adjust保存採用/idle編集終端 | A / R | 診断はui、進行はlogic候補 |
| F5 | font適用 → service_capture_main_window → key_input begin_frame | font output、main HWND初回捕捉/既存hook設定 → Win32 pass入口 | root HWND / R | outputはui、HWND設定はlogic候補、入力はui |
| F6 | service_tray_requests → maybe_intercept_close → sync_tray_resident_media_wake | tray設定/要求、root Close、media wake投影 | A / root、対象窓は既存tray owner | logic候補 / ui / logic候補。tray_active gate維持 |
| F7 | input_main_focus_resume → service_folder_observation | focus復帰判定、watch/rescan/prewarm/sidecar周期flush | R | ui → logic候補 |
| F8 | input_frame_diagnostics → service_startup_notifications | frame counter/性能診断、indexer/housekeeping/VST fullscreen状態反映 | A / R | ui → logic候補（minimized metadataのみ）。counter増分をUIへ残したままG1〜G4のserviceをlogicへ移すと進行が止まり得るため、counter/予算ownerの分離を接続前に確認 |
| F9 | startup overlay / preparing panel | 描画とstartup入力破棄、従来早期return | root | ui。outer終端は続行 |
| F10 | render_startup_workers | VST/EffeTune/play-test/update check | A / R | ui継続あり（C2） |
| F11 | input_pointer_and_ime → service_page_edit_completion → touch診断 → input_drop_and_activity | D&D pointer終端、IME pass、spread/sidecar終端、外部drop・activity入力 | R | ui / logic候補 / ui |
| F12 | render_apply_theme → render_initial_open → track_window_rect / selected_cell_rect | theme、初回open/activation採用、表示geometry | A / R | ui継続あり（C3）、geometryはui |
| F13 | service_initial_worker_results | preview、content identity、pinned調整、Smart metadata結果 | R / cache Clearedは既存registryが選ぶM | UI 継続あり（C3・C5）。現在の関数名にかかわらずlogicへ直接接続しない |
| F14 | poll_thumbnails → reconcile preview → fs nav lock → thumbnail bookkeeping / VRAM / tag prewarm | 現在表示に応じた回収と要求/会計 | R | 表示依存prepareはui、結果採用と分離が必要 |
| F15 | FullscreenDecode.service / non-Windows prefetch | 既存context async owner | R | 既存ownerを維持し、C5の表示決定とG1のPDF昇格再試行、G2/G3の予算gateを確認・分離してlogicへ。F8のcounter増分との関係を保持 |
| F16 | render_media_and_edit_results | media / AI / erase / local-adjust結果 | R（media owner選別を保持） | ui継続あり（C4・C5） |
| F17 | render_search_and_metadata_results | 検索、facet、評価、bookmark、metadata | R | ui継続あり（C3・C5） |
| F18 | render_other_worker_results | stack、subfolder、Smart、pano、file操作/変換/export/cache等 | R / A、元の要求owner | ui継続あり（C3・C5） |
| F19 | render_prepare_fullscreen_materials → render_main_title → modifiers | 表示決定、AI/prefetch/調整準備、title、修飾キー同期 | R | ui。modifiers内部は1b-3まで不変更 |
| F20 | input_root_keyboard → sample/dispatch_gamepad_batch | rootに配送されたfullscreen key / focus / clipboard / keyboard、pad | R、padは下記Mへ保留可 | ui。gamepad内部は1b-2まで不変更 |
| F21 | liveness / activation watcher / commit → update_active_viewer_context | 寿命・activation、active viewerの既存mount更新 | M → Rへ復帰 | poll/input/renderを同owner内で分離してlogic/uiへ。今回は呼出不変更 |
| F22 | gamepad fallback / finish → poll_parked_live_detached_windows | mounted配送不成立時の既存返却、pad終端、parked media回収 | R / P | padは1b-2、Pのpollは既存mount owner（C3・C5監査要） |
| F23 | keep_fullscreen_viewport_alive → embedded transition polls → fullscreen render | viewport維持、描画前folder/PDF/ZIP/nav-lock、登録/paint | R / Mで描画済みならskip | 登録はui、採用継続はui依存を分離してlogicへ |
| F24 | render_detached_image_windows 内 service_deferred_detached_window_events → callback登録/paint → active backstop | 既存イベントdrain/終了判断と描画を分離 | 対象window ID → 既存寿命/gesture/context owner | drainはlogic候補（L1留保）、登録/実callback ACKはui |
| F25 | fullscreen close-nav / native front・VST・HUD → backdrop / holdover / embedded branches | 表示後navigationとnative出力、既存早期return | R / native既存owner | ui / post-render。inputを含むnative入口は段1b-3対象 |
| F26 | render_menubar → render_main_overlays → render_menu_worker_results | menu、overlay、tag/rating/video-upscale結果 | R / A | ui → ui継続あり（C3・C5） |
| F27 | render_main_dialogs_before_open → open-folder → render_main_file_dialogs → spread-shift poll → render_main_conversion_dialogs | 元のmodal生成順とopen候補 | R / A | 描画はui、pollは既存ownerのlogic候補 |
| F28 | service_thumb_quality_results → render_main_settings_dialogs → service_pdf_worker_notices | thumbnail quality結果、設定/VST UI、PDF失敗通知 | R / A | logic候補 / ui / logic候補 |
| F29 | PDF/Susie/TRT notice poll/show → addon/stats/context menu → render_main_dialogs_after_context → PDF/ZIP enumerate | noticeと後半dialog/変換/保存UI、列挙採用 | R / A | showはui、pollはlogic候補だが列挙のC3・C5を分離 |
| F30 | stack reconcile → toolbar → refresh消費 → input_main_grid_shortcuts | 同frameのmenu結果/refreshとkey順序を保持 | R | ui / UI継続。main_viewer_blockedの同じsnapshotを引数で渡す |
| F31 | render_main_bars → popup/folder pane wheel guard → process_scroll | 検索/facet/info/pane/選択overlay、wheel消費 | R | ui。wheel内部は1b-3まで不変更 |
| F32 | root startup tag → render_grid → details/調整budget → selection info → delete key → reload | grid paint、表示後採用と入力 | R | ui / post-render |
| F33 | post_render_main_navigation | 既存nav候補の優先裁定、folder/pane結果、open | R | uiの後処理（入力を再取得しないがopen先のUI継続あり） |
| F34 | late-generation repaint → detached selection sync → tail repaint / perf | 同frameのmutation、要求/診断 | R / 対象detached | 表示同期はui、非表示進行は既存owner（C5分離要） |
| F35 | post_render_native_drag → post_render_scroll_bookkeeping | native D&D境界、settle/auto-aspect deadline | R / root HWND | ui後処理。既存modal/時計を維持 |
| F36 | key overlay / Remote dialogs → external-tool authorize → perf finish | 実際のUI Cancel/Esc通過後のspawn境界ACK | R / A | ui後処理。前倒し禁止 |
| O8 | render_update_after_frame | composition operation/save/read採用 | A / R / 既存registryが選ぶM | ui継続あり（C3・C5）。update_frameの全return後も実行 |
| O9 | render_update_owner_dialogs | composition/ZIP/capture/EPUB/password UI | R、専用viewportは既存callbackのM | ui。parkedへの新しいdialog serviceはしない |
| O10 | post_render_sidecar_restore → test-script finish → post_render_favorite_writes | 復旧取消/保存終端、favorite保存差分検出 | 復旧要求のcontextをM、favoriteはR | 非表示候補でもterminal frameのmodal gateを前倒ししない |
| O11 | render_update_external_tool_checkpoint → maximize/normal-present/runtime cleanup ACK → outer perf/trace finish | 早期return時UI補完、実表示後の終端 | root / A | ui後処理。cumulative_frame_nr > 0は実表示前cleanup抑止として維持 |

### service の入力依存監査と設計担当への返却

直接のevents消費、key level、focusによる操作、pass/frame条件をservice断片へ混ぜない。
sidecar入力消費とkeyboard所有は`input_update_begin`、scroll intentは旧位置のinput、
root Closeは`maybe_intercept_close`、pointer/IMEは`input_pointer_and_ime`、
activity event判定は`input_drop_and_activity`、frame計装は`input_frame_diagnostics`へ区分した。
startup geometry / runtime cleanup / external-tool UI ACK のframe条件は表示側へ残した。
`service_startup_notifications` が読む `viewport().minimized` はwindow metadataだけであり、
events / pass counter / frame counterを操作の条件にしない。

さらに呼出先を静的に追うと、単純なpollのlogic移動では断てない依存があった。
下記は旧0.33.3のコード事実であり、hidden runtimeの観測結果や今回の不具合診断ではない。
frame/passの引数化だけで古い値への依存が解消したとは扱わない。

| ID | 到達経路・依存 | 今回の扱い / 段3より前に必要な整理 |
| --- | --- | --- |
| C1 | poll_remote_session → poll_remote_fullscreen_restore → open_fullscreen → book hydration / target採用 → fs_page_turn_decision_for_frame_with_perf | Remoteサービスと表示復帰継続が同じ入口。`render_update_begin`に区分。Remote進行そのものをUI待ちにする修正はしない。受信/取消/ACKと表示継続の所有境界を設計担当へ返す |
| C2 | poll_play_test → open_fullscreen → 同じ表示継続 | `render_startup_workers`。VST/更新pollも同順のまま。表示open完了の切り分けが必要 |
| C3 | startup/activation open、mounted document/collection navigation、bookmark open、tag/rating採用のrebuild、composition operation/service → book prepare / permission → resume_qualified_book_target / complete_book_navigation_target → fullscreen open | `render_initial_open` / `render_update_before_frame` / `render_search_and_metadata_results` / `render_menu_worker_results` / `render_update_after_frame`等に区分。F13にも `poll_edit_preview_cache(Cleared)` → `clear_all_edit_preview_materializations` → `with_viewer_context` → mount境界の評価反映 → `adopt_rating_membership` → fullscreen再openの経路がある。現在の完了採用は同passで表示継続も行うため、それだけを後回しにすると挙動が変わる。要求ownerやmodalを変えず分離する小設計が必要 |
| C4 | poll_erase_inpaint → resume_original_material_entry → resume_view_trim_after_original → ctx.input(pointer.primary_down) | `render_media_and_edit_results`。表示トリムのドラッグ終端を古いpointer levelで決める非表示serviceにはしない。原画完成と入力に依存するeditor再開の切り分けが必要 |
| C5 | prefetch/final-effects、pano high-res→resolve_pano_source→final composite、上記open継続 → fs_page_turn_decision_for_frame / original_preview_frame_observation / fs_page_turn_physical_input_for_frame | frame/passをキーとしたdecision cache、focus/物理入力sampleを参照。`render_prepare_fullscreen_materials`等のUI準備に区分。既存registry async ownerも内部までlogic-readyとは断言しない。F13のcache Cleared→mount境界の評価反映→fullscreen再openも同じ表示決定へ到達する。紙めくり/原画previewとの境界整理に加え、下記G1〜G3のframe進行gateも分離前提として確認する |
| D1 | main/parkedのpollにあるframe_counter % 60 / % 600 | ログ間引きだけ。worker採用や取消/終了をこのcounterでgateしていない。新しいservice counterに置換しない |
| D2 | poll_indexer_notifications / startup notice / log writer noticeのminimized | current viewport metadata。古いeventsを操作として使う依存ではない。従来の通知抑止条件を保持 |
| D3 | service_folder_observation → poll_external_rescan → prepared listing install / hydrate → begin_sidecar_restore | request_idをframe_counter + input_seq + 1から生成する。操作/進行gateではなく既存要求identityであり、今段は不変更。段3の同frame内の複数logic実行に対する一意性を要確認。Some(scan)なのでPDFのpre_scan.is_none()枝への静的到達は成立しない |

#### UI frame counterを進行gateに使う依存（独立レビュー追補）

以下はD1のログ間引きやD3の要求identityとは異なり、処理の実行/回収を抑止する。
`input_frame_diagnostics`（F8）が更新するAppの`frame_counter`は、logic-onlyではUIと一緒に
止まる想定になる。複数logic呼出しが同じcounterを見る場合の進行と、UIの重複実行/予算を守る
条件を分ける必要がある。今回counter・再試行・予算・配送の挙動は変更しない。

| ID | 所在・条件 | 進行への効果 / logic接続時の確認事項 |
| --- | --- | --- |
| G1 | `src/app.rs:84154` `refresh_fullscreen_pdf_promotion_with` の `last_attempt_frame == self.frame_counter` | 同じdisplay/pending集合でnot-found再試行を抑止。prefetch/fullscreen openから到達する。F8/F15をlogicへ分ける前に、UI counterが止まっても必要な後続昇格を進めるownerと同一試行重複防止の境界を整理する |
| G2 | `src/app.rs:81920` `poll_local_adjust_render`、`:82179` `poll_local_adjust_layer_bypass_preview`、`:82248` `poll_local_adjust_prefix_preview`、`:83009`/`:83029` `ensure_edit_result_pixels` の `last_edit_materialize_upload_frame` 比較 | 前者は完成結果の適用/receiver drainを延期し、後者はtexture uploadを省略する共通1-frame予算。F8/F15/F16/F19の接続ではworker完了進行とUI/GPU upload予算を区別する。同counterのままでは後続の完成処理が留まり得る |
| G3 | `src/app.rs:91618` `reconcile_colorize_mono_summaries` の `colorize_mono_summary_last_reconcile_frame == self.frame_counter` | prefetchから呼ぶ近モノクロ要約の1-frame計算予算を抑止。F8/F15のlogic接続では、UI counter更新なしで未処理要約が進める境界と既存予算を確認する |
| G4 | `src/external_tool.rs:3101` `advance_external_preparation` の `preparing.last_frame == Some(self.frame_counter)` | file-list等のフレーム分割準備を同frameに一度だけ進める。F18/F36/O11のサービス/描画分離では、完了待ちをUI counterに結び付けない進行単位とUIのCancel/ACK順序を分けて確認する |
| G5 | `src/app.rs:3452` `deferred_detached_activation_watch_targets` の `frame_counter >= activation_ready_frame`（生成は`:65325`/`:65371`で`frame_counter + 1`） | watcher activationの受付資格を抑止する既存入力gate。F21/F24を非描画へ接続する際、UI frame前提の資格をserviceの進行条件と混同しない。今回の凍結範囲では変更しない |

描画の重複防止/実表示ACKである `active_detached_viewport_rendered_this_frame`
（`src/app.rs:60735`）、`show_external_tool_modals` とouter checkpoint
（`src/external_tool.rs:3528` / `src/app/update_phases.rs:1668`）もframeを比較するが、
G1〜G4のworker/準備進行と同じものではない。F24/F36/O11の実描画側に保持する。
`frame_counter == 1` のstartup性能ログと、`passthrough_last_call`の経過frameは診断のみ。
Contextのframe/passを使うgamepad/keyboard/原画previewの既存UI batchは別のC5/段1b-2対象として残す。

監査補助: `target/egui037-stage1b-1/service-input-audit.json` と `service-audit.txt`。
self/appメソッド呼出をたどる検索補助であり、Rustの全call graphを証明するものではない。
名前が同じメソッド、callback/closure、native出力は別途sourceを確認する。
このため「service依存0件」をApp全体の完了条件として主張しない。
非表示serviceへの接続を今回先行せず、現0.33.3の呼出条件のまま返す。

### 子Close / lifecycleの入口（L1）

`service_deferred_detached_window_events` は、既存のshared viewイベントdrain →
`queue_deferred_detached_image_window_event` → `apply_detached_image_window_event_batch`を抽出しただけである。
`render_detached_image_windows` の空チェック/診断後、登録前という従来位置を保持する。
既存test入口も同じproduction serviceを呼ぶようにし、drainを二重に持たない。
FrameのClose/独自bar、右drag Cancel/input、placement/focus、FirstCallbackは既存のfactをそのまま渡す。
modal判断、close_idsの重複除去、寿命/gesture/media cleanupは既存ownerが行う。
新しいContext input読取やpass条件、表示ACKの偽造は加えていない。

**L1留保:** この入口にはClose以外のcallback factも含まれる。現在のOS Close検出は子callback内であり、
0.33.3上で非表示子のOS Closeを新たに検出・消費する実装ではない。
将来のClose-only hookは対象ViewportIdから既存window/contextを解決し、同じmodal/終了ownerへ合流させる。
active/parkedはpassive画像cleanupで代用せず、既存のactive終了/取消とParkedLive mount後終端へ委譲する。
承認/拒否の一回消費、backend/Contextの非描画退役、兄弟資源不変の回帰は段2・段3で接続する。
FirstCallback / 初期配置 / 実表示ACK / focus由来右drag Cancelは通常callbackの通知として保持する。

### 検証

初回焦点nextestは1,065件中1,062 passed / 3 failed、exit 100（全体268.367秒、実行42.528秒）。
失敗は `external_tool::tests::the_external_tool_modals_are_drawn_from_one_place_only`、
`the_launch_boundary_is_acked_even_when_the_frame_body_returns_early`、
`ui_adjustment_panel::local_adjust_fold_audit::the_key_run_settle_runs_before_the_embedded_fullscreen_early_return`。
いずれも旧app.rsへの直書きを前提とするsource監査で、移動したbodyを追えていなかった。
初回結果は `checks-focused-results-initial.json` / `focused-nextest-initial.log` / `focused-junit-initial.xml` に保存する。

監査を新境界へ追従させ、制約を保持した。
`src/external_tool.rs` はappとphaseの合算で単一描画owner数を維持し、outer tailのcheckpoint呼出しと
helper内のdraw→ACK順を検査する。`src/ui_adjustment_panel.rs` はdriverのservice呼出しがembedded gateより前で、
helper内poll→settle順であることを検査し、既存parserもpub(super) fnを関数境界として扱う。
両監査へ変異したsourceを渡すnegativeテストを2件追加し、呼出し削除/前倒し/順序逆転/ACK削除、
service後置/worker処理削除が検査で拒否されることを確認する。
`src/app/tests.rs` のroot Close producer監査もapp+phaseへ検索を広げ、1件/ROOT限定の期待値を維持する。
製品の動作・期待値・待機時間・ignoreは変更しない。

実装担当の内部確認（別contextのGPT-6.1 Sol / xhigh）では、製品抽出と上記監査追従に問題なしと判断した。
これは正式な独立レビューではない。正式な独立レビューは`01ad17ca9`に対しACCEPT WITH CHANGES
（P2二件・P3一件）で、指摘への修正は下の追補に記録する。
`review-reconstruction.json` に44連続ブロックの本文保存、外側return/break/continue不在、
App呼出しを元bodyへ展開したHEADとの一致、fullscreen drainの同等再構成を記録した。
追加のsidecar wrapperを含めApp helperは45関数、passive drain helperは1関数。
内部確認担当はテストを重複実行していない。

最終検証は以下のとおり。環境は通常feature / Rust 1.99.0、
`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`、snapshot更新変数なし。
証跡は `target/egui037-stage1b-1/`。詳細command・exit・時間・件数を保存する。

| 検査 | 最終結果 | exit code |
| --- | --- | --- |
| `cargo fmt` / `cargo fmt --check` | 成功。監査追従後にも実施、check 15.391秒 | 0 / 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、17.309秒 | 0 |
| 同 `--features test-script` | 成功、17.238秒 | 0 |
| `git diff --check` | 成功。台帳追記後にも再確認 | 0 |
| 焦点nextest | 1,068 passed / 0 failed、実行38.443秒、compile込み210.484秒 | 0 |
| integration snapshot 2本 | 181 + 11 = 192 passed、compile込み175.094 / 11.690秒 | 各0 |
| lib snapshot 50 filter | 103 passed、全filterが1件以上を選択、計93.038秒 | 各0 |
| 基準PNG SHA-256 | 445枚すべて一致、更新0枚 | 0 |
| `scripts/test-lib-full.ps1` | 13,277 passed / 0 failed / 59 skipped、実行350.562秒、全体356.698秒 | 0 |

焦点は `tray` 47 / `startup` 269 / `keyboard` 50 / `sidecar_restore` 62 / `gamepad` 86件を含む
（substring集計、重複あり）。追加したnegative監査2件とroot Close監査も成功した。
その他の対象はpassive/deferred detached、book composition、external tool、Remote session、local adjust、
original material、view trim、update perf。全commandは `checks-focused-results.json`、
件数は `focused-summary.json`、実行ログ/JUnitは `focused-nextest.log` / `focused-junit.xml`。
通常/診断check・fmt/diffの個別ログも同directoryへ保存した。

snapshotは段0b/段1aの同じ50 lib filterと2 integrationを更新なしで比較した。
`snapshots-results.json` / `snapshot-summary.json` / `lib-snapshot-filters.json` と52個別ログに結果を保存した。
全件後にも445 PNGと対象外DSPのバイト列を再照合して一致した。

lib全件は新規実行（MISS）。runは
`target/test-lib-full/runs/20261016T084732071Z-ffbbc96a/`、source IDは
`5a4a8b29789d9dd78f18358c4d3044059e961e6d1c6bba760c7738b386a163cb`。
成功receiptは `target/test-lib-full/c09e30e9c91f7eefe7a02cd5500eba66a195be9191a22ff1c8b54578f40836bd.json`。
同runのJUnitを再集計し、13,277成功 / 失敗0 / skip59と一致した。
`lib-full.log` / `full-results.json` / `full-summary.json` に結果を保存した。
過去段の間欠失敗候補を成功扱いで省略せず、今回の全件で実行して成功した。

製品3ソースは実装担当の内部確認時のSHA-256と一致。監査追従3テストファイルも内部確認では問題なしと判断した
（`review-tests.json`。正式な独立レビューの判定とは別）。全件後に `source-hashes.json` のRust6ファイルとmanifest/lockの一致を確認した。
以後の追記は本台帳の検証結果のみで、実行ソース・依存・fixture・条件は変えていない。
今回の残課題は下記とC1〜C5 / D3 / L1であり、全logic-readyの受入れを意味しない。

### 未実施・設計との差異

gamepadのowner/batch、foreground/wheel/clipboard/native IME/output内部は未変更。
抽出はApp本体とpassiveイベントdrainに限定し、各ラボは今回対象外。
上記C1〜C5は設計書の「UI入力を読まないlogic service」へ移す前の追加整理点である。
旧updateの分類だけで全worker採用がlogic-readyになる、という前提は成立しない。
挙動不変の範囲を越えて表示継続を延期/取消する変更をせず、設計担当へ返す。
段1b全体や段3の受入完了とはしない。


## 段1b-1 独立レビュー後の修正（基点01ad17ca9）

ACCEPT WITH CHANGESのP2二件/P3一件へ対応する。F13のUI継続とC3/C5、
F8/F15の進行gateおよびG1〜G5を上記へ追記した。製品の実行コード、counter、関数名、
依存/PNGを変更しない。更新設計書は設計担当の並行read-only比較対象なので変更しない。

### App source監査の棚卸し

| 対象テスト | 扱い |
| --- | --- |
| `borderless_runtime_state_writes_stay_on_the_observed_path` | app.rs + update_phases.rsの合算で既存の各1書込を検査 |
| `app_has_no_viewport_lifecycle_font_resync_state_machine` | App禁止識別子の対象を両ファイルへ拡張 |
| `product_consumers_do_not_bypass_the_lifecycle_owner`（TRT） | product_sourcesへapp/update_phases.rsを追加 |
| `epub_pending_and_visible_owner_inventory_is_explicit` | RAII外のsync_epub_ui_read_leases禁止を両ファイルへ拡張。型宣言の存在inventoryは元の宣言ファイルを維持 |
| `promotion_only_asks_one_predicate_whether_a_page_is_displayed` | 共有述語を通らない昇格判定の走査へupdate_phases.rsを追加 |
| root Close producer / external-tool単一描画owner | 01ad17ca9で両ファイルを対象化済み。今回変更なし |
| update/early-tail順序、local-adjust settle順序 | 01ad17ca9でdriverとphase本文の両方を検査済み。今回変更なし |
| media resumeのsave_all/poll_video、poll_prefetchのループ、close-and-retire seam、settings restoreのupdate/on_exit順序 | 特定関数の本文/順序の検査でありApp全体の禁止監査ではない。対象関数は移動しておらず、旧ファイルを維持 |

`include_str!` / `read_to_string` / `src/app.rs`のファイル列挙を`src` / `tests`で検索して上記を確認した。
UI/製品の期待値や監査の禁止条件・許容件数は変更しない。
証跡は`target/egui037-stage1b-1/review-changes/`。

### 修正後の検証

通常feature / Rust 1.99.0、`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`。
変更した上記5監査をnextestの明示filterで選択した。command/exit/時間は`results.json`、
各実行のlog / JUnit / summaryも同directoryに保存した。

| 検査 | 結果 | exit code |
| --- | --- | --- |
| `cargo fmt` / `cargo fmt --check` | 成功、後者13.978秒 | 0 / 0 |
| core check | 成功、13.122秒 | 0 |
| 変異ありの焦点nextest | 対象5件すべて期待した禁止監査で失敗、compile込み142.517秒 | 100（期待値） |
| 復元後の焦点nextest | 5 passed / 0 failed、実行0.156秒、compile込み62.758秒 | 0 |
| `git diff --check` | 成功。結果追記後にも確認 | 0 |

変異は移動先`service_creative_luts`内に、`#[cfg(any())]`で実行/型検査から除外した
font-resync識別子、borderless二種の直接代入、TRT直接start、EPUB同期、共有述語を通さない昇格条件を
一時的に挿入した。5件がコンパイルエラーでなく各監査のassertで失敗したことを確認した。
終了時には元の`update_phases.rs`をバイト単位で復元し、復元したソースから再compileして5件を実行した。
一時変異は最終diffに含まれない。

製品source（app.rs / update_phases.rs / ui_fullscreen.rs / external_tool.rs）、更新設計書、
対象外DSPのSHA-256は開始時と一致。今回のRust差分は3テストファイル内の監査対象一覧だけである。
lib全件・snapshot・製品build・起動・live試験は依頼に従って再実行しない。
前節のlib全件結果は段1b-1基点の履歴として保持し、今回のテスト変更後の全件成功とは主張しない。
未コミットで引き渡す。

## 段1b-2: gamepadの物理取込み・UI batch境界（2026-10-16）

開始: `egui-036` / `9453bd3c9`。Rust 1.99.0 / egui 0.33.3のまま、旧呼出順の抽出だけを行う。
更新計画§4.2のゲームパッド項目1〜11、detached構造合意の範囲に従う。
境界案は実装担当の内部確認を経て実装した。利用者決定の最小化案Bは段3以降の接続条件であり、
今回の最小化/repaint/foreground挙動を変更しない。更新設計書・manifest/lock/vendorは編集しない。
対象外DSPは段1b-1冒頭と同じSHA-256を保護する。製品build/起動・live試験・PNG更新・コミットは行わない。

### 分けた境界と旧順序

| 順 | 境界 | 所有・処理 | 0.37接続予定 |
| --- | --- | --- | --- |
| B2-1 | `sample_gamepad_input`のtracking / pass / frame cache判定 | UIの`SampledPadTransaction`。追跡ONの再passはdrain前に同一batchをReplayとして返す。追跡OFFは従来どおりdiscard後のpassでもlive poll | ui。logicへframe/pass gateを移さない |
| B2-2 | `drain_gamepad_physical_input` → `GamepadPhysicalInput::drain/ingest` | App-global単一owner。既存runtimeを1回drainし、押下/軸/repeat予約の保持更新、無効化/disconnectで保持解除、未配送edge/終端を追記。Contextはruntimeの既存wake登録にだけ渡す | logicの物理取込み入口。今回もB2-1の後からだけ呼ぶ |
| B2-3 | `take_pending` → `gamepad_batch_from_physical_input` | 同ownerから一度移譲。終端のoverlay後始末、旧`PadAction`/operation ID生成、既存UI repeat生成、batch作成。設定OFFは操作・activity・analogを作らず終端だけ返す | ui。段3でB2-2を別駆動するときはここでtakeし、UI側からの二重drainを除く |
| B2-4 | ingress trace / `SampledPadTransaction`固定 | UI frameのimmutable batch。OFFの早期returnは従来どおりtrace/cacheを作らない | ui |
| B2-5 | 既存root / mounted配送・fallback / finish | `dispatch_gamepad_batch_with_admission`のmodal/OS foreground判定、continuous timer抑止、`suppress_pending_actions`、ring終端/Undoを既存context内で実行 | ui。今回呼出元・配送順・判定を変更しない |

`src/app.rs`のruntimeと保持stateの2フィールドを`GamepadPhysicalInput`へ集約した。
物理ownerはroot / mounted / parkedの切替対象ではなく、viewer contextを進めない。
配送先は既存active viewerのmountが決め、成立しなければ既存root fallbackへbatchを返す。
parked viewerへ追加配送しない。`update_phases.rs`/viewport経路も変更しない。

`src/app/gamepad_input/physical.rs`の`PadPhysicalHandoff`は、順序付き
`Vec<PadPhysicalEvent>`（`ButtonPressed` / `ButtonReleased` / `SessionEnded`）と
受信activity・analog観測の集約値を持つ。最終保持レベルや最新batchで過去edge/終端を置き換えない。
`take_pending`は`mem::take`で一度だけ所有権を移し、次のtakeは空になる。
`SampledPadTransaction`は未配送ownerに流用しない。新App pending bool・時間窓・第二drainはない。

意味終端の**保持**と**実行**を分けた。無効化/disconnectの保持解除は物理owner内のイベント位置で行う。
UIではoverlayだけを後始末し、同drain内のdisconnect後に届いたpressを再clearしない。
ringは引き続きFresh batchの既存`session_ended`判断で、mounted配送先のadmission前に確定する。
sidecar restoreのcommitしない終端、contextの保存先・Undo構築も変更しない。
現UI batchは従来どおり終端をboolへ集約する。複数logic drainで保持した終端を将来どの操作順で実行するかは、
段3でこの順序付きhandoffから接続する。今回、その複数drainの製品配送は有効化しない。

非前面/modal時の既存`reset_gamepad_continuous_steps`と`suppress_pending_actions`は移動・変更しない
（保持stateへの参照パスだけ変更）。通常の保持軸は保持し、復帰時の中立通過を追加しない。
West ring操作の既存neutral要求はそのまま残す。gilrs producerの受付/foreground判定も変更しない。

### 追加回帰と段3への引継ぎ

物理owner4件、UI境界4件の回帰を追加した。

- 実runtimeのテストqueueから2回drainしたpress→releaseが順序通り残り、take後は空になる。
- 複数disconnectの終端がedge間に残り、disconnect後の新pressの保持を消さない。
- ON press/軸→OFFで保持を解除しても、未配送edgeと終端を上書きしない。
- 軸active→別drainでneutralになってもanalog観測事実を保持する。
- UIは追加pollなしのtakeから両edgeと異なるordinalを作る。
- overlayの終端後始末が同drain内の後続pressを消さない。
- OFFのUIは既に未配送edgeがあっても操作を作らず、tracking cacheも作らない。
- admission不成立時の抑止後、追加axisイベント/中立通過なしで保持軸の操作を再開する（headless admission注入）。

段3ではB2-2だけをlogicへ接続し、B2-1/B2-3/B2-4/B2-5をUIへ残す。
producer側の受付事実と離散edge抑止（項目4/5/7）、複数drainの意味終端配送（項目10/11）、
sidecar restore終端と未配送ownerの接続は今回未実装であり、logic-ready全体の受入れとはしない。
保持値更新とrepeat予約は物理側、repeatの期限確認/操作化は今回もUI側に残すため、
段3の接続時にもcadenceとOFF/再passの既存契約を確認する。
最小化案Bの決定とこの構造抽出の間に矛盾はない。

### 検証

実行記録は`target/egui037-stage1b-2/`へ保存する。通常feature / Rust 1.99.0、
`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`。結果は完了後に追記する。

| 検査 | 結果 | exit code |
| --- | --- | --- |
| `cargo fmt` / `cargo fmt --check` | 成功（16.090秒 / 15.252秒） | 0 / 0 |
| core check 通常 / test-script | 成功（50.153秒 / 50.538秒） | 0 / 0 |
| 焦点nextest | 223 passed / 0 failed、実行14.919秒（compile込み335.241秒）。追加8件も全てpassed | 0 |
| 旧snapshotの通常比較 | 295 passed / 0 failed（統合181 + 11、lib明示filter 50本で103件）。PNG 445枚のSHA-256不変、更新なし | 全52コマンド 0 |
| `scripts/test-lib-full.ps1` | 13,285 passed / 0 failed / 59 skipped、実行357.814秒（wrapper込み362.620秒）。成功台帳の再利用でなく今回のsourceで実行 | 0 |
| `git diff --check` | 成功。結果追記後にも確認 | 0 |

焦点filterは`test(gamepad) | test(pad_operation) | test(app::gamepad_input::) | test(sidecar_restore) | test(tray) | test(still_window_pad) | test(ring_picker) | test(rating_undo)`。
追跡ONのreplayと追跡OFFのdiscard後poll、設定OFFの保持/overlay終端、sidecar終端、
ring/context/Undoの既存回帰を含む。fullの件数は段1b-1の13,277件から追加8件分増え、skip数は59のまま。
既存の5件のslow表示は打ち切りでなく、すべて完了した。

実装担当の内部確認（別contextの`gpt-6.1-sol` / `xhigh`）では、境界案・実装に問題なしと判断した。
これは正式な独立レビューではない。内部確認の軽微な旧入口コメントの指摘は、現行のsidecar入口・
物理owner/overlay分離の説明へ修正した。内部確認担当はテストを重複実行せず、上記の結果を再利用した。

正式な独立レビューは`4ed025839`に対し**ACCEPT WITH CHANGES（P3二件）**。
台帳の内部確認/正式レビューの区別と、UI batch化の余分なdrainを検出する回帰の補強を指摘された。
今回の修正・検証は下の追補に記録し、先行の内部確認と分けて扱う。
command/exit/時間は`checks-focused-results.json` / `snapshots-results.json` / `full-results.json`、
件数は`focused-summary.json` / `snapshot-summary.json` / `full-summary.json`、実行sourceは`source-hashes.json`。
fullの元runは`target/test-lib-full/runs/20261016T112422610Z-90304c4d/`、
成功receiptは`target/test-lib-full/0e949feeb8a049c41c6a20f2c1a6829045923f6d41571e753f229f246434afc4.json`。

実行中に設計担当の計画第4.3版commit `b14571d8d` が入りHEADが進んだ。
`9453bd3c9..b14571d8d`は`docs/egui-0.37-update-plan.md`だけの変更であり、実装担当は同ファイルを編集していない。
Rust実行source/テストsource、manifest/lock/toolchain、対象外DSPは保存したSHA-256と全件後も一致した。
以後はこの台帳の結果追記だけで、実行コード・依存・fixtureを変更しない。
今回の段1b-2変更は未コミット。段1b-3・段3・最小化案Bの接続は未着手で、上記留保以外に設計との食い違いはない。

## 段1b-2 正式独立レビュー後の修正（対象4ed025839）

正式判定はACCEPT WITH CHANGES（P3二件）。製品コードの挙動不変との判定を受け、
以下の文書・テストだけを修正する。作業開始HEADは`a50dcc37f`（`4ed025839`の後は更新計画だけのcommit）。

- 先行の別contextでの確認は「実装担当の内部確認」と訂正し、正式な独立レビューと区別した。
  段1b-1の同種表記も訂正し、`01ad17ca9`に対する正式判定を別記した。
- `gamepad_ui_take_preserves_split_drain_edges_without_another_poll`を補強した。
  press→releaseの2回drainからtakeした直後に、別ボタンEastのpressをruntime queueへ追加する。
  UI batch化後もEastの保持が変わらず未配送handoffも空であること、後続の明示drainで初めて
  Eastの保持とedgeが更新され、次のUI batchへ1件だけ届くことを検査する。

証跡は`target/egui037-stage1b-2/review-changes/`。
通常feature / Rust 1.99.0、`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`で検証する。
余分なdrainを`gamepad_batch_from_physical_input`先頭へ一時挿入する変異を検査し、復元後に
core check / fmt check / gamepad焦点nextestを実行する。結果は完了後に追記する。
全件lib・snapshot・製品build/起動・live試験は今回の依頼に従い再実行しない。

| 検査 | 結果 | exit code |
| --- | --- | --- |
| `cargo fmt` / `cargo fmt --check` | 成功、23.836秒 / 14.783秒 | 0 / 0 |
| core check | 成功、14.260秒 | 0 |
| 余分なdrainありの単独nextest | 0 passed / 1 failed。補強した保持stateのassertで期待どおり失敗、compile込み150.141秒 | 100（期待値） |
| 復元後のgamepad焦点nextest | 94 passed / 0 failed、実行6.219秒、compile込み83.941秒 | 0 |
| `git diff --check` | 成功。結果追記後にも再確認 | 0 |

変異はUI batch化の先頭に`self.drain_gamepad_physical_input(ctx, now)`を一時追加した。
対象テストが`UI batch construction must leave queued input untouched`で失敗したことを確認した。
コンパイルエラーや0件一致による失敗ではない。変異後は保存したfmt済みソースをバイト単位で復元し、
復元後のソースから再compileして焦点テストを実行した。
焦点filterは`test(gamepad) | test(pad_operation) | test(app::gamepad_input::)`。
command/exit/時間は`results.json`、JUnitと件数は`*-nextest-junit.xml` / `summary.json`、
変異の失敗理由と復元SHA-256は`mutation-restored.json`に保存した。

`4ed025839`の元ソースとの比較で、Rust差分が上記の`#[cfg(test)]`内の1関数だけであることを確認した。
物理owner・App・既存テスト・runtime・manifest/lock/toolchain・対象外DSPのSHA-256も開始時と一致した。
検証中に設計担当の文書commit `b86cce154` / `6ee226289`が入りHEADが進んだ。
`a50dcc37f..6ee226289`の差分は更新計画とdetached設計の文書だけで、実行/テストsourceは変わっていない。
両設計文書は現在のHEADと一致し、実装担当の編集はない。製品実行コード・一時変異は最終diffに含まれない。
今回の修正は未コミットで返す。この追補は修正と検証の記録であり、正式独立レビューの再判定ではない。

## 段1b-3a: C1 Remoteと共有完了の境界（2026-10-16）

開始: `egui-036` / `9dfc6e91e`、Rust 1.99.0 / egui 0.33.3を維持する。
更新計画第4.4版の改訂履歴・§1追補・§4.2条件表/責務表/C1〜C5/Closeと、段1b-3a〜3c/共通受入、
Remote設計§2.1/§2.2、detached設計§2/§11の追加合意を読み直した。
最小化案B、root lifecycle、非描画mount、immediate子のbackend接続は今回実装しない。
製品/liveは起動せず、更新設計書・manifest/lock/vendor・対象外DSPを編集しない。コミットしない。

### 境界と旧driver順序

| 順 | 入口 | 所有・内容 | 今回と段3接続の扱い |
| --- | --- | --- | --- |
| R1 | `service_remote_session_begin` | App-global `remote_session_ui`。session snapshot、sequence採用、旧stream取消、AI lease返却、Local表示slot登録 | 通常driverの旧位置。表示reloadは呼ばない |
| R2 | `continue_remote_local_display` | 同ownerのslotを一度takeし、Local phaseと取得/返却sequenceを照合して既存一覧reload/既定起動先へ | 旧control_return採用直後の表示位置。rootのUI継続 |
| R3 | `service_remote_session_requests` | 既存全context Close/editor取消、取得/AI/save/resume/shift barrier、headless stream、要求受信、resume read、DrainingRemote完了 | 旧R2後。既存終了owner/副作用は維持。下記の推移依存留保あり |
| R4 | `continue_remote_local_display` | R3でApp drainがLocal返却した場合の同じ表示slot消費 | 旧complete_app_drain直後の表示位置 |
| R5 | `service_remote_session_finish` | reading write receipt/ACK、既存16ms/50ms/1sのwake条件 | 旧R4後。表示slotを新しいwake条件にしない |

`poll_remote_session`はR1→R2→R3→R4→R5の通常0.33.3 driver。
`render_update_begin`内のtray wake ACK→heartbeat→Remote→modal診断の位置も維持する。
`service_remote_session`はR1→R3→R5だけを組み合わせ、PC表示継続を消費しない入口である。
0.33.3の製品driverはこのservice-only入口へ切り替えず、最小化時の表示延期を導入しない。

旧`pending_fullscreen_restore`を同ownerの`pending_local_display: Option<PendingLocalDisplay>`へ置換した。
slotは`acquisition_sequence` / `control_return_sequence` / `ReloadedView`を持つ。
返却sequenceの採用と同じ境界で登録し、再取得で失効する。UI消費時にもsessionのidentityを照合し、
serviceを挟まず再取得→遅いUIとなった場合も旧Local表示へ渡さない。coalesced reacquireのAI lease保持は従来どおり。
旧fullscreen復元には製品の生成元がなく、直接生成していたApp終了drain fixtureだけを新slotへ合わせた。
`item_key` / `smart_request_id` / `wait_frames`と旧復元pollを撤去し、閉じた閲覧窓を再openしない。
第二queue、新pending bool、時間窓、追加repaint、強制UIによる新しい継続機構は設けない。
App終了時は既存`RemoteAppDrainLease`へslotを移して破棄し、UI消費をACK/drain条件にしない。

### 共有完了とcontext owner

| 対象 | 受信・採用・取消・ACK | 旧位置/表示との境界 |
| --- | --- | --- |
| Remote内read/write | resume/readingの既存receiverとclaimed requestが一度だけ応答。headless動画も既存owner | R3/R5を維持 |
| bookmark | `poll_book_bookmark_service_responses`が唯一の共有receiver/router。Remote応答を先に判定して残りを既存local ownerへ | `poll_book_bookmarks`はrouter→`poll_book_bookmark_context_projections`。旧metadata位置で順序不変。local rows採用はrouter内に残る |
| shift | `poll_spread_shift_service`の既存単一routerを共通完了入口として利用可能にした。Remote ShiftClearを先に判定、残りは現owner/AtRestへ | 通常driverのfile dialog後の旧位置を変更しない。第二receiverを作らない |
| composition | `poll_composition_editor_save_receipts`が送信済ticketだけを収穫。既存global ticket ownerを維持 | 旧wrapperはRemote時close→receipt→Preparing/Reloading/編集中result。旧post-frame位置と、その後のpublication順序を維持 |

Remote session/AI lease/slot/claimed request/ticket/routerはApp-globalで、viewer context切替とともに交換しない。
ローカルのClose・editor状態・bookmark invalidationは既存owner identity/registryをそのまま使う。
受渡し先をforegroundから推定したり、rootの結果をparked/兄弟へ表示したりしない。
shared routerやticket収穫をR1の早い位置へ追加せず、通常のmodal/terminal frameの順序を維持する。

### 推移依存の留保・設計との照合

今回の入口名やservice-only回帰は、C1全体が非描画logicへ接続可能になったことを意味しない。
コードで確認した次の前提は、予定済み1b-3b/段2・3の整理を必要とする。

- composition close/receiptのresume記録・editor状態更新、Remote bookmark invalidation、pause/AI barrierは
  既存`with_viewer_context` / active wrapperへ到達する。往路/復路のrating publication・page-edit reconciliationが残る。
  `Some(ResumeCommit)`のowner付き直接記録、editor状態アクセス、未反映ratingを持つ別context回帰は1b-3bへ残す。
- `close_all_detached_viewers_for_mode_change`にはVisible(false)を成立させる既存`show_viewport_immediate`とfocus処理がある。
  既存関数を呼ぶだけで「UIを強制実行せず全contextを終了できる」とは扱えない。
  挙動不変の段ではこれを削除せず、合意済みの非描画退役/終了接続を段2・3へ残す。
- AI pending/orphanの静止終端には既存表示poll依存、bookmark router内にはlocal結果採用/refreshが残る。
  取消・破棄済workerとlive表示結果の分離は1b-3b。publicationのchanged scopesを先にdrainして捨てない。

したがって「3a単独で全取得条件を任意の非描画contextで完結できる」という前提は成立しない。
今回は既存副作用を変えず境界と単一ownerを整理し、段3のlogic接続前に必要な依存を明記する。
同じservice入口を旧mountのままlogicへ接続する、表示副作用をflag/guardで抑える、といった代用はしない。
これは既存コードとの照合で明らかになった接続上の留保であり、まれな失敗用の回復状態は追加しない。

### 内部確認と検証範囲

実装担当の内部確認（別contextのコード棚卸しを含む）を行った。正式な独立レビューとは区別する。
内部確認の指摘で、slot消失だけでなくReadingHistoryの実reloadによるitems generationの変化と、
通常driverの2つの返却位置での一度だけのreloadを回帰へ加えた。source順序検査はその補助証拠である。
追加の境界回帰は共通filter `egui037_continuation`で選択する。
取得/AI lease、fake保存worker ACK/Draining、resume read receiver、共有bookmark router、返却slot/再取得/coalescing、
送信済saveの成功(None)/失敗終端を、PC表示継続/UI passを呼ばず同じframe番号で検査する。
別contextのresume/meter保存、live editor状態更新、全contextの非描画退役を網羅したとは記録しない。

通常feature / Rust 1.99.0、`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`。
証跡は`target/egui037-stage1b-3a/`。結果は以下に記録する。

### 検証結果

| 検査 | 結果 | exit code |
| --- | --- | --- |
| core check 通常 / test-script | 成功、20.244秒 / 21.796秒 | 0 / 0 |
| `cargo fmt` / `cargo fmt --check` | 成功。最後の境界回帰追加後にも実行、14.430秒 / 14.163秒 | 0 / 0 |
| Remote等の焦点nextest | 936 passed / 0 failed、実行34.801秒（compile込み265.039秒） | 0 |
| 共通filter `egui037_continuation` | 最終の10件が全てpassed、実行0.594秒（compile込み139.784秒） | 0 |
| 旧snapshot通常比較 | 統合181 + 11、lib 103の計295 passed。52コマンド全て成功。PNG 445枚SHA-256不変、更新なし | 全て0 |
| `git diff --check` | 成功。台帳の結果追記後にも再確認 | 0 |
| `scripts/test-lib-full.ps1` 初回 | 13,292 passed / 2 failed / 59 skipped、実行393.247秒（wrapper396.952秒） | 100 |
| 初回失敗2件の焦点nextest | 2 passed / 0 failed、実行0.491秒 | 0 |
| `scripts/test-lib-full.ps1` 同条件の再検証 | 13,293 passed / 1 failed / 59 skipped、実行419.706秒（wrapper423.501秒） | 100 |
| 再検証の失敗1件の焦点nextest | 1 passed / 0 failed、実行1.554秒 | 0 |

**全件ゲートは未達。** 失敗履歴を成功扱いで省略しない。

- 初回: `favorite_search_worker_installs_sources_before_loading_and_requeries_on_refresh`が
  `audio_favsearch_sidecars.rs:110`のsidecar削除でWindows共有違反(code 32)。
  `recovery_modal_escape_keeps_failure_and_requests_quarantine_cancel`が
  `tests.rs:41670`でquarantine終端の3秒以内待ちを満たさなかった。
- 同じソースで2件を絞ると成功したため、一度だけ元の全件条件で再検証した。再検証では初回2件は成功。
  別の`clipboard_capture::fetch::tests::total_deadline_covers_slow_headers_drip_bodies_and_redirect_remaining_time`が
  `fetch_tests.rs:566`でdrip処理731.1077msとなり、650ms未満の判定を満たさなかった。
  この1件もコードを変えずに絞ると成功した。
- Favorite search試験は直接のfavsearch/sidecar poll、rename試験は専用modal helper、
  clipboard試験はfetch単体で、今回のRemote coordinator/共有完了入口を通らない。
  今回差分との因果関係は確認できていないが、間欠失敗の根本原因を解消したとは主張しない。
  対象外の製品コード・テスト・期待値・期限・並列数を変更せず、3回目の全件実行は行わない。

Remote焦点filterは`test(egui037_continuation) | test(remote) | test(session) | test(reading_write) | test(composition_editor) | test(book_bookmark) | test(spread_shift)`。
共通filterは新規9件と既存App終了drain試験の改名1件で計10件。936件成功後にtest-onlyのstale fixture補強と
返却identity回帰1件を追加したため、最終common 10件を別実行した。実行コードはその間も不変で、
最終ソースの全件実行にも全10件が含まれ成功した。source grepによるdriver順序検査は、実reload副作用の回帰に対する補助証拠。
初回焦点compileは追加テストの`ReadingPosition`比較型とfixture借用の2エラーでexit 101となり、
テストだけを修正して再compileした。core check後の変更は`cfg(test)`内だけである。

`checks-results.json` / `focused-summary.json` / `focused-remote-junit.xml` / `boundary-junit.xml`に焦点結果、
`snapshots-results.json` / `snapshot-summary.json`に通常比較、`full-summary.json`に全件と失敗再実行の件数を保存した。
全件のraw結果は`lib-full-first.log` / `full-first-results.json` / `full-first-junit.xml`と
`lib-full-second.log` / `full-second-results.json` / `full-second-junit.xml`。
個別失敗の再実行は`full-failures-focused*` / `full-second-failure-focused*`、失敗詳細は`full-failures.json`に残す。
両fullのsource IDは`99e6b89068a6509c70edcf3d2639160251ffc5770c9fce4a1a1ddcbc98b7ca38`で同一。
元runは`target/test-lib-full/runs/20261016T120806050Z-86ddf4b5/`と
`target/test-lib-full/runs/20261016T121615834Z-9db40088/`。成功receiptは生成されていない。

`body-reconstruction.json`ではbookmark router/後段projectionとcomposition receiptの本文保存、
Remote断片をslot受渡し・旧dead restoreの明示差分だけで再構成できることを確認した（空白差は正規化）。
`source-hashes.json`の最終Rust source/テスト/依存と、開始時の更新計画・対象外DSP・PNG 445枚は全件後も一致した。
以後は台帳結果の追記だけで、実行source・依存・fixtureは変更していない。

今回の7ファイルの差分は未コミットで返す。1b-3b/3c、段2・3の正式接続は未着手。
上記のmount/Close等の接続前提と、全件ゲートの失敗3件の原因確認を残課題とする。

#### 段1b-3aの受入れ（2026-10-16、設計担当）

HEAD `c02e62349` で、他のcargo/nextestプロセスが無いことを確かめてから設計担当が`scripts/test-lib-full.ps1`を実行した。
結果は **13,294 passed / 0 failed / 59 skipped、374.818秒、exit 0**（run dir `target/test-lib-full/runs/20261016T122757583Z-61166b76`）。
実装担当の全件で落ちた3件はすべて成功した。原因の断定はしない。
独立レビュー（GPT-6.1 Sol / xhigh）は`c02e62349`をACCEPT（指摘なし）。以上で段1b-3aを受け入れる。
台帳の留保（mount前後の表示副作用、bookmarkのlocal採用、AI取消終端の表示poll依存、Closeの`show_viewport_immediate`依存）は1b-3bと段2・3で扱う。

## 段1b-3b: 保存・取消終端、D3、root lifecycle、状態アクセス（2026-10-16）

開始HEAD `1aee8863c`。1b-3aは設計担当による正式独立レビューACCEPT・全件13,294 passed/0 failedで受入済み。
本節はその後の未コミット差分の記録であり、前節の失敗履歴は書き換えない。
Rust 1.99.0 / egui 0.33.3を維持。第4.4版の段1b-3b、root Close、C3 mount、D3、共通受入と
detached §2/§11の追加合意を再読した。更新計画書を変更せず、logic接続・製品/live起動・commitを行わない。

### 境界・owner・通常driverの接続

| 境界 | 非描画側の責務 | UI側と通常0.33.3の順序 |
| --- | --- | --- |
| composition receipt | App-global ticketを一度収穫。receiptのpath/page identity/meterを既存共有writer/meter/dedup mirrorへ直接記録。editorはowner identityを照合しMounted/AtRestの当該stateだけ更新 | Preparing/Reloading・publication/permissionは旧post-frame位置。receipt→publication順を維持 |
| context access | `with_viewer_context_state`は既存table plan/payload交換/finishとfavorite投影・元ownerへの復元だけ。callback unwindも同じ復元契約 | `with_viewer_context`にmount_outgoing→mount_incoming→callback→return_outgoing→return_incomingのrating境界とincoming後のpage-edit reconciliationを明示して残す |
| owner付きeditor終了 | 狭い`with_composition_editor_owner_state`で既存stateを更新し、mount不要。古いticketは新sessionを変更しない | 表示準備のpollは既存UI mount wrapperを使う |
| Remote/共有bookmark | pause/AI barrier・bookmark invalidation/disconnectをstate accessへ。唯一の共有receiver/routerを維持 | local `ContainerListed`は既存requestの`RowsReceived` phaseへ渡す。projection worker開始と採用は旧UI projection checkpoint。共通完成queueは追加しない |
| discarded sidecar | `service_discarded_sidecar_restore`はDiscardedの予約返却・global effects・reader再開だけ。preview clear ACKは収穫し、共有Clearedイベントを表示pollへ残す | live hydration・retained shift readoption・F13のcache採用/rating再採用は旧UI位置。reader再開の一時mountはstate access |
| AI取消終端 | `service_cancelled_ai_terminals`は取消済receiver/exact context/key/jobを終端。retained orphanは既存CPU cache処理、live結果は既存backlogに保持 | 共通collector/orphan処理をUIと共有。通常UIの取消Ready採用等の従来policy・旧poll位置は変更せず、新serviceのlogic接続は段3 |
| watcher/周期保存 | watcher collectと既存周期保存の入口 | `poll_external_rescan`の走査結果受信・Equal/Different採用・hydration/cutoff ACKをその間の旧UI位置へ残す |
| D3 | App-global hydration要求ownerの`sidecar_restore_request_seq`をchecked採番。一要求一回、通常/IndexOnly共通 | Commonとbook restoreに同じID。取消/完了で消えるper-request stateやcontext bundleへsequenceを入れない。frame/input計装は進めない |

root lifecycleは通常入力と独立し、最新ROOT ViewportInfoを明示して読む。
`RootLifecycleSnapshot`は一呼出し内だけの値で、Appのpending stateではない。
通常driverでは旧outer-update位置のsettings→composition→ZIP受付と、旧tray要求poll後のhide判断を2 checkpointに保つ。
modal ownerは終端poll前にsnapshot化し、通常×・明示Quit・保持TrayHideを区別する。
`service_root_lifecycle`は同じ受付/判断とreceipt・settings-family・ZIP exit・discarded sidecar終端をUIなしで呼べる。
既存EditorExitRequest/DeferredRootClose/ZIP Exitだけが終了意図を保持し、各終端から共通のbarrier判断へ戻す。
rootのCancelCloseはlifecycleで返し、sidecarのキー/pointer/IME sanitizeはroot Closeを判断しない。子の入力sanitizeは従来どおり。

ZIPの終了経路ではresource/trim返却・source cache更新をstate accessで行い、表示membership再構築を分けた。
既存per-context `metadata_import_refresh_index` ownerを`Collecting`/`DisplayReconciliation`のtyped phaseへ整理し、
同generationの最終chunkで表示採用を保持する。次の当該UI boundaryで一度だけ再構築し、state accessでは消費しない。
タグ更新もあるため、page-edit stampだけではこの表示再構築を代用できなかった。新bool・第二queue/registry/bundleは追加しない。
ParentHydration中のCloseは、そのownerのImageFolderZip continuationだけを既存Discardedへ渡し、
sender破棄後もglobal終端が済むまで既存ZIP phaseを保持する。通常取消/非exitの表示更新と親一覧採用は旧UI経路を維持する。

### 回帰・接続前提

追加境界回帰は`egui037_continuation`、rootは`root_lifecycle`、状態アクセスは`context_state`、D3は`hydration`でも選択する。
実owner/receiver/ACKと、未反映rating・受信済み未採用page-edit snapshot、fake save receipt、実共有resume writerを使う。
source grepだけを主証拠にせず、receipt・one-shot終了・UI側の後続採用を検査する。
実装担当の内部確認を行い、正式な独立レビューとは区別する。

0.37 backendのlogic直後のClose確定/RawInputの配送消費は段2c・段3へ残す。
0.33.3では後続file-organize admissionもROOT Closeを参照するため、ここでrawのCloseを消去しない。
backendが届けた一回を旧driverの一回のcheckpointで受付け、frame counterや新しい重複受付flagは作らない。
tray hideの既存root自身の親一覧復元はauto_fullscreen=falseの既存navigationを維持する。egui UI/viewport callbackの強制実行ではない。
前節の`close_all_detached_viewers_for_mode_change`の既存immediate viewport退役は段2・3のvendor/App接続の留保であり、
今回新たに強制UI・repaint・delay・retryを加えて代用しない。

### 検証

証跡は`target/egui037-stage1b-3b/`。通常feature、`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`。
検証対象は最終Rust source 715ファイルのSHA-256で記録する（`sources-final.json`）。

| 検査 | 結果 | exit code |
| --- | --- | --- |
| core check 通常 / test-script | 成功、20.922秒 / 48.458秒。以後のsource変更は既存test-only 4ファイルの比較・監査修正だけ | 0 / 0 |
| cargo fmt / fmt --check | 成功。最後のtest-only比較修正後にも実行、15.855秒 / 16.347秒 | 0 / 0 |
| 共通境界初回compile | 既存テストの旧struct `.next_item`参照でcompile失敗。typed phaseを明示して同じ上限を検査する形へ修正 | 101 |
| 共通境界32件 | 全て成功、2.529秒（compile込み345.253秒） | 0 |
| 関連焦点nextest（2回） | 各2,008 passed / 1 failed。失敗は同じretired receiptテストで、下記の期待値所有前提とpath比較。81.241秒 / 81.453秒 | 100 / 100 |
| 修正対象＋共通境界の最終nextest | 33 passed / 0 failed、2.598秒（compile込み144.618秒）。他の2,008件は同じ製品sourceでの成功結果を再利用、unique 2,009件をカバー | 0 |
| 個別filter成立件数 | 最終33件内で共通32、root_lifecycle 8、context_state 6、hydration 4（D3 3件とZIP hydration 1件） | 上記0の内数 |
| 旧snapshot通常比較 | 統合181 + 11、lib 103の計295 passed。成功52コマンド、PNG 445枚SHA-256不変。初回の異常終了は下記へ別記 | 最終全て0 |
| 全件lib初回 | 13,312 passed / 1 failed / 59 skipped、374.759秒（wrapper378.447秒）。下記のsource監査文字列不一致 | 100 |
| 外部ツール監査＋共通境界 | 34 passed / 0 failed、監査anchor修正後（compile込み135.715秒） | 0 |
| App分割先の禁止コード監査を含む最終焦点 | 40 passed / 0 failed（compile込み171.948秒）。他の既存成功と重複を除く件数はevidenceへ記録 | 0 |
| 最後のfmt / fmt --check | 監査対象追加後も成功、15.315秒 / 16.826秒 | 0 / 0 |

焦点filterは`test(egui037_continuation) | test(tray) | test(startup) | test(sidecar_restore) | test(composition) | test(rating) | test(hydration) | test(remote) | test(book_bookmark) | test(spread_shift) | test(metadata_import) | test(ai_cancel) | test(final_ai) | test(edit_preview) | test(image_folder_zip)`。
最終は`test(egui037_continuation) | test(retired_owner_save_receipt_records_once_without_reviving_owner_or_touching_main)`。
`evidence-summary.json`は成功名のunionを取り、重複を足し合わせない。

- 既存`retired_owner_save_receipt_records_once_without_reviving_owner_or_touching_main`は、
  退役後のreceiptが`last_book_resume`を更新しないことを要求していた。しかしmirrorはbundleに含まれないApp-global stateであり、
  第4.4版C3はwriter/meter/dedup mirrorの共通利用を指定する。この前提を訂正し、正しいpath/page identity/meterと、
  共通入口から再記録してもwriter受付件数が増えないことを検査する。mainのitems/generation不変・失敗時に記録しない検査は維持した。
  追加比較のpath表記（raw Windows pathと正規化済みscope）も既存`path_key::normalize`で照合した。製品のpath解釈は変えない。
- snapshotの`book_composition_editor_snapshots`初回は7件成功後に`STATUS_ACCESS_VIOLATION`で異常終了
  （exit 3221225477 / 0xc0000005、7.584秒）。Appを生成しないpure editor fixtureで、今回差分との因果関係は未確認。
  コード・backend・並列数・期待PNGを変えず同じコマンドで単独再実行し、11 passed / 0 failed、8.378秒、exit 0。
  UI統合181件は初回成功（187.600秒）を再利用し、残りの通常lib比較50コマンドも全て成功した。異常終了の原因解消は主張しない。

`boundary*-result.json` / `*-junit.xml`、`focused*-result.json` / `*-junit.xml`に焦点結果、
`snapshots-first-results.json`と`book_composition_editor_snapshots-first.log`に異常終了、
`snapshots-remaining-results.json` / `snapshot-remaining-summary.json`に再実行と残りの通常比較を保存した。
更新計画・Cargo manifest/lock・toolchain・対象外DSPの5ファイルとPNG 445枚は比較前後で不変。

全件初回の失敗は`external_tool::tests::the_launch_boundary_is_acked_even_when_the_frame_body_returns_early`。
今回の`update_frame(ctx, frame, root_lifecycle)`への引数受渡しに対し、test-only監査が旧2引数文字列を探していた。
製品のdraw/launch ACK入口と順序は変えず、監査anchorと「frame前へ移した場合」の変異fixtureを新呼出しへ合わせた。
変異fixtureが実際に文字列を書き換えたことも検査する。外部ツールの製品コードを1b-3cより先に変更したものではない。

分割先の追加に合わせ、1b-1で整えたApp禁止コード監査の5種（borderless状態書込み、子Close生成、昇格述語、
font-resync、TRT lifecycle迂回）へ`root_lifecycle.rs` / `ai_terminal_service.rs`を加えた。
子Close生成数の監査は、新moduleのinline testのROOT Close assertionを除いた製品本文を対象とし、生成数の上限は維持する。
これらと外部ツールの変更はtest-onlyで、core/snapshotの製品source・fixture・条件は不変。
初回失敗は今回の呼出し抽出に関連する監査互換の問題として修正し、焦点成功後に同条件で全件を再検証する。
再検証の`scripts/test-lib-full.ps1`は **13,313 passed / 0 failed / 59 skipped、375.601秒
（wrapper381.036秒）、exit 0**。runは`target/test-lib-full/runs/20261016T133838852Z-73d3bdb9/`、
source IDは`0859396af08fcc24553038e5b065ece10a97756eee37d0551b37923c29e5f2c6`。
成功receiptは`target/test-lib-full/3b0b7300c88416fa68ca2f051d35f7c7c0a928d53fe40a0bfda6841d4687f6a9.json`。
初回run `20261016T132120965Z-3f2a2a01`（source `a55f6845af11a23b85177fcac862758f6c5410cd36135d1fb02291bbb914b683`）の
13,312成功/監査1失敗も、`lib-full.log` / `lib-full-result.json` / `lib-full-first-junit.xml`に保持する。
最終runは`lib-full-final.log` / `lib-full-final-result.json` / `lib-full-final-junit.xml`。

最終焦点40件は2.641秒、前の成功結果とのunique unionは2,016件。
common 32 / root_lifecycle 8 / context_state 6の内数を二重加算しない。
core以後の差分4ファイルは`app/tests.rs`、`app/book_composition_editor/tests.rs`、`external_tool.rs`のtests、
`ai/trt_worker_lifecycle.rs`のtestsだけ。実行製品source・依存・snapshot fixtureは不変として成功を再利用した。
全件終了後も最終Rust 715ファイル、保護対象5ファイル、PNG 445枚のハッシュが一致した（`evidence-summary.json`）。
最後は台帳だけを追記し、`git diff --check`もexit 0で確認した。

変更は新規`app/root_lifecycle.rs` / `app/ai_terminal_service.rs` / `app/context_state_tests.rs`と、
App driver/registry、composition editor/receipt/meter、sidecar/D3、ZIP/tray/settings-family、対応監査・台帳の18ファイル。
対象外DSPの開始時差分は保持した。HEAD `1aee8863c`のまま未コミットで返し、branch/push操作は行わない。

残る接続は1b-3cと段2・3。特にbackendの最新root Close消費/logic直後のCancelClose確定、
既存immediate viewport退役、App::logic/uiの正式接続を今回の成功と混同しない。
設計との照合では、per-requestのhydration Optionへsequenceを置けないことと、ZIPのtag membership再構築がpage-edit stampだけでは
延期できないことを確認し、前述の既存App-global owner/既存refresh ownerのtyped phaseへ整理した。
旧テストのdedup mirrorのmain所有前提は訂正済み。それ以外の重大な設計不一致は確認していない。

## 段1b-3c: 入力投影・clipboard・native出力

2026-10-16、開始HEAD `9481bc344`。前段1b-3bは利用者から独立レビューACCEPT（指摘なし）の通知を受け、
設計担当がコミット済み。本節は第4.4版の1b-3cの実装記録であり、以下のソース照合は**実装担当の内部確認**である。
正式な独立レビューの結果は先取りしない。Rust 1.99.0 / egui 0.33.3 / wgpu 27を維持し、logic接続・新版API移行は行わない。

### 抽出した境界と旧呼出順

| 境界 | 入口・配置 | 保持する順序・所有 | 段2・3の接続先／留保 |
| --- | --- | --- | --- |
| foreground物理取込み | `native_window::foreground_process_hwnd`、既存`foreground_app_hwnd`から委譲 | OS foreground HWNDを一回採取し、null / PID 0 / 別processを拒否。同じHWNDを物理gamepad ownerへ返す。non-Windowsとheadless fakeは旧入口のまま | 既存の取込みownerが同じ判定を使用。新しいforeground入力除外は加えない |
| main mouse-nav pending | `keyboard_input::main_keyboard_drains_mouse_nav` | 先行するsession blockとactive detached foreground除外の後だけ判定。静止画detached＋main非focusは子へ残し、native動画はglobal hookの重複をdrain。modal / IME等によるearly returnより前という位置も不変 | UIのviewport ownerへ残す。現在focusの有無と、配送済み離散edgeの資格を混同しない |
| unsmoothed wheel取得 | `wheel_input::remaining_wheel_input` | root一覧はmenu / toolbar / facet後・grid直前。fullscreenは従来の前面owner／navigator内。0.33の残存`raw_scroll_delta`とpass modifiersをその場で読み、threshold 0.5・一回の集約・Ctrl列数操作を維持 | UI側。0.37でのみ消費位置の`InputState.events`から単位換算・event別modifiersを投影する。raw.events保存やsmooth値で代用しない |
| wheel消費 | `wheel_input::consume_wheel_input` | App一覧・popup、main menu、fullscreen navigator / HUD popup、筆半径の旧消費位置でraw / smooth / MouseWheel eventsをまとめて消費。筆のShift専用notch計算、popup終了時の既存内部残量終端は変更しない | UI側。内部smoothing tailを新たにresetしない |
| text clipboard条件 | `egui-ime/clipboard`の`ClipboardCommand`、通常／nativeの二つの純粋入口 | 通常はcommand+C/X/V・専用キー・Windows Insert/Deleteを維持。nativeはCtrlかつAltなしのC/X/V。両方でShiftを許可。Process / press-release / text focus / IME gateは各integrationの旧位置、I/O・正規化・空paste抑止も各ownerへ残す | `egui-winit`が既存共有crateへpath依存する。版変更なし。Ctrl+AltやInsert/Deleteの従来のintegration差を共通化の名目で拡大・縮小しない |
| grid Shell clipboard | `App::grid_clipboard_requests`、semantic / transition bitの純粋判定は`keyboard_input` | main focus・text owner・grid admission → semantic events → Ctrl/VのGetAsyncKeyState → effective keymap / pressを旧順序で採取。解除済み割当のsemantic復活なし。Cut優先、copy/cut実行後にpaste実行。call-localな値だけでApp状態を追加しない | root UI入力。Shell貼付けのtransition bit契約を通常text clipboardへ置換しない |
| modifier resync / routed edge | 既存`resync_egui_modifiers_from_os`、`keyboard_input_permits`、`key_input::begin_frame_for_context_with_tracking` | title後のresyncとfocused guard、event時点modifiers、最初のpassだけWin32 queueをdrainする条件を変更しない。focus喪失後も同じviewport宛routed edgeを離散操作として採用 | 0.37のModifiersChanged化は段3。過去edgeの修飾値の書換え・二重drainを加えない |
| native platform出力変換 | `append_native_platform_window_intents` / `native_ime_cursor_area_intent` | logical batchで既存focus intent → IME cursor area → cursor policy。DPI倍率・round・最小矩形とnative CopyTextの旧処理位置を維持。別viewport出力の合成を追加しない | 0.37のIME中断生成／discard再pass集約／control命令は段2a・3。今回のIMEOutputには中断flagがない |
| native IME配送 | `native_window_host::native_ime_window_forms` / `update_ime_cursor_area` | pumpの既存WindowEpoch / visual資格を経て同じhostへ配送。self.hwndのHIMC → composition → candidate → release、nullなら従来どおり終了。foregroundから配送先を選び直さない | 新IMM32取消操作・新intentは段3。既存pump・control / visual配送条件は変更しない |
| texture適用・完結 | `egui_output::apply_texture_sets` / `finish_texture_output` | nativeの既存atlas診断／GPU gate内でset順を適用 → acquire / draw / submit / present → 既存commit処理 → free。set/free数の診断も維持。完結入口はdeltaを所有権で受ける | 新版set列のdrain、無surface／acquire失敗時upload submit、eframe共有rendererへの引継ぎは段2・3。旧早期error経路は今回変更しない |
| headless破棄 | `egui_output::discard_headless_output`（test限定） | nativeの実logical batchを検査するfake target、wheel回帰、IMEの実TextEdit回帰から明示的に破棄。製品Contextでの資源配送省略には使えない | 0.37ではこの境界を`drop_without_applying_deltas`へ接続。今回の0.33では所有したFullOutputをdropするだけ |

Appのphase driverと`input_root_keyboard`の呼出順・early returnは変更していない。
Win32 native key queue、WH_GETMESSAGEのpending counter、focus喪失時のrouted edgeのproducer / drain本体も変更していない。
新しいApp pending bool、queue、時間窓、repaint、強制UI、IME中断や入力除外は追加していない。
detached側は§11で合意済みの入力／出力owner境界の抽出だけで、viewport寿命や再生成には触れていない。

### 回帰と実装担当の内部確認

- wheelの実0.33 `InputState::begin_pass`でPoint / Line / Page、Shift / Altの軸変換、event modifiersとpass modifiersの区別、
  同pass集約を確認する。消費後もimmutable raw.eventsが残るfixtureで、配送値とeventsが復活しないことを確認する（新規4件）。
- clipboardのmodifier 32組合せで通常／nativeの受付差を固定する（共有crateの新規1件）。
  通常bridgeの既存delivery回帰はCtrl / Ctrl+Shift / Ctrl+Alt / Ctrl+Shift+Alt、physical fallback、repeat / release、空pasteを維持する。
  nativeの実`push_native_event`でCtrl+Shift+C/Xの一回配送・raw Key/Text抑止・KeyUp重複なし・Ctrl+Alt通常経路、
  IME中とpending Commit直後のgateを検査する（新規2件）。Shellのhigh bit＋transition bitとsemantic抽出も純粋回帰に分ける（新規1件）。
- native IMEの矩形変換／intent順、IMM32形式のcursor除外と最小矩形を検査する（新規4件）。
  textureは同ID whole→partialの順序とsubmit後free、free-only出力をfake consumerで検査する（新規2件）。
  既存native batch driverのmulti-pass集約／一回present、key入力replay／focus喪失／mouse-nav／gamepad回帰も焦点対象とする。
- HEADとのソース照合ではclipboard採取／dispatch順、foregroundのstrict判定、set / submit / present / free、
  nativeの早期errorとWindowEpoch配送条件を確認した。**実装担当の内部確認**であり、独立レビューの代替とはしない。

### 検証

証跡は`target/egui037-stage1b-3c/`。`MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`で実行する。
製品・live・`test-full.ps1`・snapshot更新・commit / branch / pushは実行しない。
対象外DSPの開始時変更表示はそのまま保持し、設計書・root Cargo.toml・toolchainとともにバイト列を照合する。
Cargo.lockの差分はegui-winitから既存egui-imeへの一つの依存edgeのみで、依存versionは変わらない。

| 検査 | 結果 | exit code |
| --- | --- | --- |
| core check 通常 / test-script | 成功、62.836秒 / 63.584秒 | 0 / 0 |
| cargo fmt / fmt --check | 成功、最終15.768秒 / 15.688秒 | 0 / 0 |
| vendor egui-winitのrustfmt --check | 成功、0.082秒（workspace fmt対象外も明示確認） | 0 |
| git diff --check | 成功、0.183秒。台帳追記後も最後に確認する | 0 |
| check_ui_glyphs.py | 危険glyph 0件、2.404秒 | 0 |
| 本体入力／native／output焦点nextest | 1,345 passed / 0 failed、77.893秒（compile込み488.082秒） | 0 |
| 共通clipboard純粋判定nextest | egui-ime 1 passed / 0 failed、0.008秒（compile込み15.826秒） | 0 |
| 通常bridgeのclipboard／Process／release焦点nextest | egui-winit 17 passed / 0 failed、0.069秒（compile込み70.735秒） | 0 |
| 旧snapshot通常比較 | 統合181 + 11、lib 103、計295 passed / 0 failed。52コマンド全て成功、PNG 445枚SHA-256不変 | 全て0 |
| 全件lib初回 | 13,325 passed / 1 failed / 59 skipped、425.957秒（wrapper431.230秒）。下記の既存videoテスト | 100 |
| 上記失敗の単独診断 | 1 passed / 0 failed、0.203秒（compile込み2.882秒）。同じmiv-tool config / feature / source | 0 |
| 全件lib再確認 | 13,325 passed / 1 failed / 59 skipped、673.185秒（wrapper699.263秒）、8 slow / 1 leaky。初回のvideoは成功、今回は下記PDF時間条件 | 100 |
| PDF失敗の単独診断 | 1 passed / 0 failed、0.161秒（compile込み2.103秒）。runtime設定は同じ、JUnit出力先だけ分離 | 0 |

焦点は計1,363件（crateが異なるため重複なし）。本体filterは
`test(keyboard) | test(key_input) | test(keymap) | test(ime) | test(focus) | test(wheel) | test(clipboard) | test(native_presenter) | test(egui_output) | test(mouse_nav) | test(gamepad)`。
通常bridgeは`test(clipboard) | test(delivery_) | test(process) | test(hidden)`で、Process／repeatの非配送、
Forwarded release、IME許可の切替、空pasteの既存回帰も維持する。共通crateは`test(clipboard)`。
全コマンドで既存ignoredを除外し、retry 0 / no-fail-fast / 0件を失敗とする。

最初のbridgeコマンドはworkspace memberのegui-imeとvendor依存egui-winitを一括`-p`指定し、
`package ID specification egui-winit did not match any packages`、exit 101（1.011秒）で選択に失敗した。
テスト未実行で、製品／テストの失敗件数には数えない。packageごとに分けた上記2コマンドは成功した。
失敗の`clipboard-bridges.log` / `clipboard-bridges-result.json`は保持し、この失敗時の古いJUnitは
`clipboard-bridges-stale-junit.xml`として成功証拠に使わない。ソース・feature条件の修正は行っていない。

残課題は段2・3の正式接続。案Bのscheduler条件変更、0.37のwheel／ModifiersChanged、IME中断要求のviewport所有・
discard再pass集約・control配送、texture deltaの新しいdrain／早期error／無surface経路は今回未実装である。
0.33における現在の集約値と通常／native clipboard受付差を記録し、設計書の「新版delta／新中断は後段」へ合わせた。
重大な設計不一致は確認していない。

全件初回の失敗は`video::tests::real_demux_seek_failure_retires_selection_base`、
`src/video/mod.rs:14065`の`take_seek_request().unwrap()`がNoneだったもの（テスト本文0.07秒、timeoutではない）。
このfixtureは実demuxをspawnし、`decoder.rs:3200`とtestが同じclockのtakeを呼ぶため競合の可能性がある。
これはソースからの見立てであり、原因を確定／解消したとは扱わない。
該当test / decoder / clockは今回変更しておらず、native overlayを使わないfixtureである。
同じconfig・profileのexact単独再実行は1 passed / exit 0。
新コード・既存テスト・上限・待機・並列数を修正して通したものではない。

初回全件のsourceは`83195f868f8b7e3df8132c6affda5e932b899208657fc8bb36ca8a9c1fa89ec4`、
runは`target/test-lib-full/runs/20261016T142316656Z-5a7ea903/`。
`lib-full.log` / `lib-full-result.json`とrun内stdout / stderrに失敗・件数を保持した。
単独診断で同じconfigを使ったため、このrun内のJUnitは診断1件の結果に上書きされた。
これを初回全件のJUnitとしては扱わず、`lib-full-single-junit.xml`として保存する。
初回全件の件数の根拠は元のstdout / stderrで、診断結果は`lib-full-single.log` / `lib-full-single-result.json`。
全件の失敗を単独成功へ読み替えず、履歴を保持したまま、未変更の製品／テストsourceと同条件で全件を再確認する。

再全件の失敗は`pdf_loader::tests::abort_on_cancel_returns_interrupted_immediately`。
`src/pdf_loader.rs:10313`の既存`elapsed_ms < 200`に対し208msだった。
実装はtest専用`mock_recv_loop`（30ms後に取消、10ms recv timeout）で、Appや今回の入力／native出力を通らない。
このファイルは変更していない。再全件中には別nextestの同時CPU使用も観測したが、そのコマンド／worktreeや失敗との因果は未確認。
既存テスト・上限・待機・並列数を変えずexact単独診断し、1 passed / exit 0。
診断configはJUnit出力先だけを`lib-full-pdf-single-junit.xml`へ分け、再全件のJUnitが保持されたことをハッシュで確認した。
初回videoの失敗は再全件で再発していない。いずれも原因解消・全件PASSとは扱わず、三回目の全件再実行で成功を探さない。

再全件runは`target/test-lib-full/runs/20261016T143249825Z-97cf90cd/`、
source IDは`fc1ab36e915e10153009812506d3ac0b733194a612e0ca87cfe4c86388988f35`。
`lib-full-final.log` / `lib-full-final-result.json` / `lib-full-final-junit.xml`とrun内stdout / stderr / JUnitを保持する。
PDF単独は`lib-full-pdf-single.log` / `lib-full-pdf-single-result.json` / `lib-full-pdf-single-junit.xml`。
二つの全件source IDの差は台帳への初回結果追記によるもので、検証開始以後のsrc・共有crate・関係vendor 734ファイルは不変。
`evidence-summary.json`は初回全件の元ログ由来の件数と、再全件／診断のJUnit由来の件数を区別する。
**全件libゲートは未達（両回exit 100）**として返す。単独成功・別runの成功件数を合算して全件成功にはしない。

変更は19ファイル。新規`wheel_input.rs` / `egui_output.rs` / `egui-ime/clipboard.rs`、
App / keyboard_input / gamepad foreground委譲、main / fullscreen / brush wheel、
native presenter / native window / host、IME headless fixture、module宣言、vendor依存edgeとCargo.lock、台帳。
core通常／test-script、fmt、glyph、焦点1,363件、snapshot295件の成功は同じ製品／テストsourceのまま有効。
設計書・root Cargo.toml・toolchain・対象外DSPの4ファイルとPNG 445枚も検証前後でバイト列不変。
最後は台帳だけを追記して`git diff --check`を実施し、HEAD `9481bc344`のまま未コミットで引き渡す。
