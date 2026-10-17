# egui 0.37 更新 — 段3 検証台帳

## 状態（2026-10-17）

**再開後の最新状態:** 第4.5版の型付き退役と正式接続を実装中。初回の設計論点は解決済みだが、独立実装レビューでCloseの表示継続・dirty消費とdiscard/immediate間delta配送のP1を確認した。dirty消費は既存owner内で補修し、回帰を追加した。Close表示継続とdelta配送は追加の構造契約が未解決。末尾「再開後の設計担当へ返す論点」とその補修追記を参照。段3完了・独立ACCEPTではない。

以下の初回停止記録は履歴として保持する。

開始HEAD `5c2fd03673a1587b205885ed74c82b136b6e7427`、ブランチ`egui-036`、開始時差分なし。
Rust 1.99.0固定。以下は**実装担当の内部確認**であり、独立レビュー・ACCEPTではない。
**段3は未完了。設計書と実コードの接続前提の不足を確認し、設計判断待ちで停止した。**
受入条件の未達を段4へ移したり、機能制限・期待値弱化・ignoreで隠したりしない。

## 設計担当へ返す論点

設計書§4.2「受付・判断・消費」「承認後の対象限定退役」と、段3のC1接続は、
Appの既存終了ownerから非描画でbackendの対象限定退役まで到達することを要求する。
しかし、現在の接続には次の不足がある。

1. `src/app.rs::close_all_detached_viewers_for_mode_change`は空の
   `show_viewport_immediate`を使って旧fullscreen viewportを退役している。
   `service_remote_session`から同関数へ到達するため、そのままlogicへ接続すると非UI契約に違反する。
2. `finish_active_detached_session_close`→`retire_terminal_detached_viewport_identity`は
   App側のruntime・window/context bindingを先に終了する。
   その後の`ViewportCommand::Close`は承認済み退役ではなく、新たなClose要求になる。
3. 段2の`vendor/eframe/src/native/viewport_lifecycle.rs::dispatch_child_close_requests`は
   ViewportIdだけをApp hookへ渡し、Appが`Close`を返した対象を退役する。
   既にApp側ownerを終了した子では、その同定元を失っている。
4. `vendor/egui/src/context.rs::retire_viewport`はactive UI pass中を禁止する。
   `EpiIntegration::update`はroot `run_ui`の中で`App::logic`を呼ぶため、
   Fullのlogicから同APIを直接呼ぶことでも解決しない。

実装担当二名が別々にコードを照合し、既存APIだけで直接成立する経路は見つからなかった。
これは実装担当の内部確認であり、正式な独立設計レビューではない。
未知の子を一律承認する処理、第二queue、pending bool、次FullのGC待ち、強制UIは追加していない。

別contextのGPT-6.1 Sol / xhighにも、このClose接続前提だけを読み取りレビュー依頼した。
同担当の追加指摘では、`ContextImpl::end_pass`は未使用childをGCした後にviewport outputを組むため、
`Closing`を残して通常Close commandを送るだけではcommand自体が消え得る。
通常backend GCが先にnative ownerを除けばClose hookからの完了通知も来ない。
いずれの案でも、GC前に終端出力へ移譲し、Full/LogicOnly双方からpass外へ確実に配送し、
通常GCと完了通知を一つの寿命境界へ揃える設計が必要である。
この限定レビューは段3全体の独立ACCEPTではない。
最終結論は「実在する設計受渡しの不足、直接経路なし（確信度高）」だった。
同担当は、Appの終了ownerを延長する範囲を抑えやすいtyped承認済み退役案を推奨したが、
利用者の選択や設計合意に代えて採用していない。
確定すべき契約は、exact対象/子孫への一度の権限移譲、GC前の保持とFull/LogicOnly配送、
古い出力による非再作成、共有delta移譲後の両backend/Context/IME cleanup順、
通常OS Closeのmodal拒否・新規Close・兄弟資源・root終了との分離である。

設計判断の候補は、(A)既存runtimeの`Closing`とID対応をbackend受理まで存続させる寿命境界変更、
(B)既存viewport command出力へ承認済み退役のtyped commandを追加してpass外cleanupへ配送する方式。
影響する終了入口と非管理viewport、通常GCとの関係を定義し、独立レビューと§11の追記を済ませてから再開する。
判断前にいずれかを実装しない。Remoteは暫定的にlogicへ結線せず、既存UI checkpointに残している。
これは受入済みの最終動作ではなく、未完了の移行差分の安全な引継ぎである。

## 分野ごとの差分

| 分野 | 主なファイル | 実装・判断と残作業 |
| --- | --- | --- |
| App / Ui / Panel / 一般API・ラボ | `src/app.rs`, `src/app/update_phases.rs`, `src/ui_*.rs`, `src/ui_fullscreen/**`, `src/global_search_ui*`, `tools/*_lab`, `crates/vst3-host-tester/src/main.rs` | App::logic/ui、root Ui伝播、callbackのUi化、PanelとContext APIを移行中。Window本文drag・mouse drag scrollを明示。一部ラボ・test fixtureの移行とruntime確認は未完 |
| 入力（Modifiers・wheel・keymap・IME） | `src/wheel_input.rs`, `src/key_input.rs`, `src/keymap.rs`, `src/keyboard_input.rs`, `src/modifier_probe.rs`, `src/ime_focus.rs`, `crates/egui-ime/src/**` | ModifiersChangedと消費時点のwheel eventsへ接続。混在modifiersは隣接する同条件区間で処理。Ctrl+Shift+C/X/Vの共有条件を保持。正式graphの回帰実行は未実施 |
| GPU / native presenter / delta | `src/compare_wgpu.rs`, `src/panorama_wgpu.rs`, `src/gpu_lanczos.rs`, `src/gpu_anime4k.rs`, `src/video/**`, `src/egui_output.rs`, `src/lib.rs` | wgpu30、mutable map delta、upload submit/free、IME中断intent/control/WindowEpochを実装。`TextureUpdateSubmission`を正式vendorで公開。D3D11本体・Naga27・windows0.61を維持。最新compile・回帰未達 |
| Close / lifecycle / Remote / 保存 / gamepad | `src/app/logic_service.rs`, `src/app/root_lifecycle.rs`, `src/app/gamepad_input*`, `src/gamepad.rs`, `src/ui_fullscreen.rs` | root lifecycle・保存ACK・取消終端等のlogic入口を追加中。passive Closeの既存modal判断を共有化。子hook・App起点退役は上記設計待ち。gamepadは物理ownerと受付事実・順序付きbatchへ移行中 |
| 機械的test移行 | `tests/**`, `src/app/tests*`, 各lib内fixture, `src/test_script*` | run_ui/run_pass、root Ui、IME/MouseWheel/Modifiersイベント、screenshot callback、headless delta明示破棄を移行中。PNGは更新しない。全test compileは未達 |
| vendor正式置換とegui-ime一本化 | `vendor/{egui,egui-winit,egui-wgpu,eframe}`, `vendor/upstream-provenance.json`, `vendor/import-upstream.py`, `crates/egui-ime` | 段2の4 crateを正式vendorへコピー。policyの段2差分・GC・明示退役APIを共有crateへ統合。原本provenance/PATCHESを保持。standalone manifestへ正式patchを追加。準備物の撤去とPATCHESの正式graph追補は未完 |
| 依存とlock | root・共有IME・ラボ・testerの`Cargo.toml`, `Cargo.lock` | egui系0.37.0 / wgpu30、workspace MSRV1.95。cargoの通常offline解決で更新。旧版で間接有効だったWindows Variantを明示。既存on_exitとworkspaceのglow feature統合を揃えるためrootでglowを明示 |

main/native製品ContextでCtrl+Qを無効化する差分、main NativeOptionsで自動system font fallbackを無効化する差分を含む。
文字baseline・色付き絵文字・margin/min sizeの採択は行っていない。
`scripts/test-full.ps1`の新版feature更新、live runnerとの照合、全consumerの最終棚卸しは未完。

退避した旧vendor4 crateは`target/egui037-stage3-old-vendor/`に実体で保持した。
置換前に絶対パスがworkspace内であることとreparse point不在を確認した。
`vendor-next/`は準備コピー照合用として現時点では残る。root patchは正式vendorを参照する。

## 実施済みコマンド

ログとcommand/exit/秒数は`target/egui037-stage3-<項目>.log/.json`。
Cargo実行は親担当へ一本化し、`CARGO_NET_OFFLINE=true`、
`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`を使用した。

| コマンド | 結果 | 件数 / exit |
| --- | --- | --- |
| 隔離preflight `cargo metadata --offline`（既存lock複製） | 成功。既存lockなしの最初の試行はyanked zip 2.4.2で解決不可。既存lockを使う通常更新へ訂正 | テストなし / 0（最初は101） |
| root `cargo metadata --offline --format-version 1` | 正式vendorへの更新解決成功。必要crate欠落なし | テストなし / 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core --offline` 初回 | 移行途中のAPI等182エラー。修正を進めたが完成証拠にしない | テストなし / 101 |
| 同2回目 | 移行途中のAPI等33エラー | テストなし / 101 |
| 同3回目 | standalone manifest追加時のresolver二重指定で失敗。workspace側の重複を除去した | テストなし / 101 |
| `cargo tree -d --offline` | 成功。出力にegui/eframe 0.33・wgpu27・vendor-next参照なし。Naga27は既存D3D11 build経路として意図的に維持 | テストなし / 0 |
| 最新root `cargo metadata --offline --format-version 1` | egui/eframe/egui-winit/egui-wgpu/egui_kittestは各0.37.0のみ、wgpuは30.0.1のみ。前4 crateは正式vendorを参照 | テストなし / 0 |
| 停止時core診断 `check-stop` | 機械的移行漏れ5件（ログformat引数2、Vec2 sum、Context改名2）。この5件だけ補修した | テストなし / 101 |
| 最新core `cargo check -p mimageviewer --bin mimageviewer-core --offline` (`check-checkpoint`) | 成功、45.34秒。既存/移行中警告651件。Close契約・全test targetの成立証拠にはしない | テストなし / 0 |
| 全体`cargo fmt` / 停止時`cargo fmt --check` / 5件補修後`cargo fmt --check` | 全て成功。最終check 13.62秒 | テストなし / 全て0 |
| `git diff --check` / index差分確認 / snapshot差分確認 | whitespace違反なし、index差分なし、`tests/snapshots`差分なし。vendor eframe iconは新版原本への置換で変わる | テストなし / 0 |

## 未実施の受入条件

以下は全て未達であり、0件成功とは扱わない。

| コマンド / gate | 状態 |
| --- | --- |
| test-script feature core check / `cargo check --workspace` | 未実施（通常coreだけ成功） |
| `cargo test -p egui --lib ime_interrupt` | 未実施、選択件数未取得 |
| `cargo test -p egui-ime` / `cargo test -p egui-winit --lib` | 未実施、選択件数未取得 |
| `cargo test -p mimageviewer --lib --no-run` | 未実施 |
| ui_snapshot / book_composition_editor_snapshots `--no-run` | 未実施 |
| `cargo test -p mimageviewer --lib egui037_continuation` | 未実施、選択件数未取得 |
| `root_lifecycle` / `context_state` / 両backend `minimized_full` filters | 未実施、選択件数未取得 |
| `scripts/test-focused.ps1` | 未実施 |
| `scripts/test-lib-full.ps1` 最後の1回 | 未実施。失敗分類はまだない |
| 最終`cargo fmt --check` | 上表のとおり成功。機能の受入証拠とは区別 |

## 必須回帰・受入1〜16の対応（まだ受入証拠ではない）

段1b/段2の既存回帰は正式graphで再実行して初めて段3の証拠になる。以下の選択件数は全て未取得。

| 受入 | 対応するowner / 回帰候補 | 段3の状態 |
| --- | --- | --- |
| 1 最小化Full C2〜C5/D3旧順序 | App UI checkpoint + `minimized_full` | 接続回帰未実装/未実行 |
| 2 同frame複数LogicOnly | logic service + `egui037_continuation` | C1は設計待ち、未実行 |
| 3 対象Fullへの一回採用 | 既存receiver / discard回帰 | 未実行 |
| 4 Remote取得中save/resume/shift ACK | 保存ticket・共有service router | C1設計待ち、未実行 |
| 5 返却→再取得→遅いFull | `pending_local_display` owner | 未実行 |
| 6 live表示保留中Close/Remote | 取消・retirement owner | Close設計待ち |
| 7 C4新しいpointer状態 | UI内inpaint採用/editor再開 | 未実行 |
| 8 C5 decision/preview/PDF予算 | UI既存owner、取消はlogic | 未実行 |
| 9 D3 cutoff/cause/要求ID | 段1b hydration回帰 | 未実行 |
| 10 非描画出力/delta/clipboard/screenshot/ACK | 段2 `minimized_full`、`texture_output_` | 新版接続差分あり、未実行 |
| 11 root/child交互、skip、Close退役 | 段2 viewport入力/Close回帰 | vendor未実行、製品Close接続は設計待ち |
| 12 root Close UIなし・backend終了確定 | `root_lifecycle` + 実EpiIntegration | 製品とbackendの一体回帰未実装 |
| 13 modal拒否/延期/Quit優先 | 段1b `root_lifecycle` | 未実行 |
| 14 別context receipt/rating・非表示採用 | 段1b `context_state` | 未実行 |
| 15 次UI mountで正当な表示再採用 | 段1b `context_state` | 未実行 |
| 16 immediate親子描画資格/Screenshot | 段2両backend `minimized_full` | 正式vendor未実行 |

その他の段3必須回帰：Disabled producer→実TextEdit、親子IME中断・discard集約、
focus移動前discrete、WH_GETMESSAGE、gamepad保持軸・ring/Undo、
native IME control/epoch・delta各出口も未実行。
追加したnative回帰は`native_ime_interrupt`3件、delta helperは`texture_output_`2件だが、
compile・実選択件数を確認していないため成功件数に数えない。

## 再開時の注意

設計判断・独立レビュー後、今回の分野別差分を再利用して未完のAPI/test移行を補修する。
必須契約は段3内で完成させる。段4/5へ残すのは承認済み外観・挙動比較とPNG更新/警告整理だけ。
lib全件はまだ一度も実行していないため、最後の1回と失敗分類が丸ごと残る。
コミット・git add・branch操作・push・ネットワーク取得・live・製品build/起動・test-fullは実施していない。
確認用バイナリは未生成。段3未完・設計待ちであり、段6のbuild前提にも達していない。

停止時の分野別handoffは`target/egui037-stage3-{gpu,ui,input}-handoff.md`。
UIのnamed headless出力破棄、Context-only fixtureの入れ子root Ui、ラボの残API、
fullscreen混在wheelの複数navigation採用、gamepadの追加回帰/既存fixture整合が未完。
`src/app/tests.rs`/子/`*_tests.rs`の一括機械移行も未着手であり、
通常core成功を全test compile成功へ読み替えない。

## 第4.5版による再開（実装担当の内部確認）

設計担当がWIPを `df0037eb7` として保存し、第4.5版 `8944ef6a5` と独立設計レビューACCEPT・§11合意 `f5d632d17` を適用した状態から再開した。
前掲「設計担当へ返す論点」は第4.5版で解決した。採択は案B（承認済み退役をrootの既存viewport command出力へ型付き移譲）であり、AppのClosing/bindingをbackend待ちで延命する案Aは採らない。前掲の未実施表と停止時記録は当時の証拠として残し、以下に再開後の結果を追記する。

登録時に割り当てるopaqueな `ViewportRegistration` とcommand sinkのContextを既存detached runtimeのbindingへ一体で保持し、終了ownerがbindingをtakeして承認を移譲する。standalone fullscreenも既存surface ownerに同じ型のbindingを持つ。新しいpending flag/queueは置かず、初回登録前には架空の登録を作らない。Context GC後も同じ旧寿命を移譲できる。Remoteはlogicへ接続し、root command列に未配送承認がある間は既存close readinessをInProgressとする。

再開初回workspace checkは編集中のheadless fixtureのtail expression構文3件でexit 101（123.29秒）。2回目はroot lib通過後、launcherのrelease入力3実行ファイル不足でexit 101（54.68秒）。これは段1aにも記録した前提不足で、workspace全体成功とは扱わない。代用exeの配置やbuild規則の変更は行わない。

### 再開後の分野別差分

| 分野 | 主なファイル | 実装・判断と限界 |
| --- | --- | --- |
| App / Ui / Panel / 一般APIとラボ | `src/ui_*.rs`, `src/app/grid_paint.rs`, `tools/*_lab/src/main.rs`, `crates/vst3-host-tester/src/main.rs` | Ui callback伝播、kittest 0.5 builder/ID/CharIndex、DroppedFile、FontData等の残APIを移行。Context-only fixtureからの正当なroot Ui入口を整理 |
| 入力 | `src/wheel_input.rs`, `src/key_input.rs`, `src/keymap.rs`, `src/ime_focus.rs`, `src/touch_correlation.rs` | 新イベント形式・残wheelの区間投影と消費、IME Preedit構造体、TextEdit応答を移行。fullscreen混在wheelの複数navigation配送は既存page-turn受理規則の設計確認待ち |
| GPU / native / delta | `src/gpu_lanczos.rs`, `src/video/native_presenter/{render_core,overlay_draw}.rs`, `src/ui_fullscreen/page_turn/roi_tests.rs` | test RenderState/Instance、renderer option、fake IME出力、ROIの実delta drain/submit/freeを移行。headless観測fixtureはtest専用helperで明示破棄し、texture契約fixtureには適用しない |
| Close / lifecycle / Remote / 保存 / gamepad | `src/app/{viewport_retirement,detached_window_manager,root_lifecycle,logic_service,viewer_context_registry,gamepad_input}.rs`, `src/gamepad.rs`, `src/app.rs`, `src/remote_ipc/ui.rs` | lifetime+sink bindingからroot承認へtake移譲、非管理はDelegate、watcher/bar/OSのowner合流、C1 logicと未配送承認readiness。standalone fullscreenのC1退役漏れも補修。gamepadは同一owner内のphysical/delivered段階と順序axis配送へ分離。Close表示継続とdirty消費は下記P1が残る |
| 機械的test移行 | `src/app/tests*`, 各inline test, `tests/support/ui_harness.rs`, `tests/{ui_snapshot,book_composition_editor_snapshots}.rs` | root Ui・run_pass/run_ui・明示delta終端・新入力形式を移行。PNG・ignore・期待値弱化なし |
| vendorと共有IME | `vendor/{egui,eframe,egui-winit}/**`, `vendor/egui-wgpu/PATCHES.md`, `scripts/test-full.ps1` | root承認列、opaque寿命、旧新output分離、pass外共通cleanup、同ID新寿命へのnative retag防止、Context/IMEの対象限定終了、実headless EpiIntegration driver。standalone vendor gateはdefault_fonts+wgpuとglowへ更新。live ignore/runnerは維持。discard/immediateのdelta順は下記P1が残る |
| 依存とlock / 準備物 | `vendor/upstream-provenance.json`, `vendor/import-upstream.py`, 既存root/vendor manifest/lock | 第4.5版再開時の0.37/wgpu30 graphを使用。`vendor-next`はworkspace内の絶対パスとreparse point不在15,721項目を確認後、`target/egui037-stage3-preparation-archive`へ実体移動。準備workspace/egui-imeコピーの旧pathは撤去、原本記録と各PATCHESは正式vendorに保持 |

### 再開後の設計担当へ返す論点

別contextのSol / xhighによる**独立実装レビュー**は当初P1 3件で受入保留。その後2のdirty採用を既存owner内で補修し、source上の解消を独立reviewerが確認した。1のClose表示継続と3のdelta配送のP1は残る。第4.5版の独立設計ACCEPTを変更するものではない。ここまでの差分・検証台帳の筆者による確認は**実装担当の内部確認**と区別する。

1. **Closeの表示継続の所有。** `src/app/viewport_retirement.rs`のactive at-rest branchは`ClosedBookmarkSummary`をboolへ捨て、従来`reconcile_closed_bookmark_detached_context`が担うbookmark一覧更新・最後の実フォルダ復帰へ戻れない。standaloneも直接teardownでauto-open/bookmarkの親一覧復帰を飛ばす。先に従来Close handlerを呼ぶだけでは、teardownがbookmark return ownerを消すため不十分。終端前に宛先を既存navigation/bookmark ownerへ型付きで移す必要がある。既存`BookmarkViewState`の終端継続variant案はあるが未実装。Remoteによる失効、mainの新しいnavigation・新bookmark開始の上書きも同じownerで定義する。
2. **C1非描画終了からdirty表示採用への到達。** `close_fullscreen_now_with_metadata_boundary`はvideo thumbnail override/folder thumbnail pinのdirty consumerを呼ぶ。dirty時は`reload_current_view_in_place`やZIP階層再構築へ到達するため、C1をlogicへ結線するだけでは非描画契約を満たさない。既存UI側consumerを残し、service terminalと表示継続の境界を明示する必要がある。guard・新pending bool・強制UIは追加していない。

   **補修追記:** dirty setsはApp-globalでcontext退役時に失われず、一覧UIには既にgate付きの両consumerが存在することを実装担当と独立reviewerがsourceで確認した。
   非描画Close内の2呼出しを除き、既存dirty集合と既存grid UI consumerへ一本化した。video consumer内の一時NowPlaying artwork更新はUI側に維持し、pin永続保存・Remote ACK・Local player終端ownerは変更しない。
   新owner・新状態は追加せず、既存第4.5版の非描画service契約内の修正とした。
   実EpiIntegrationからApp::logicを2回実行し、Remote取得とfullscreen終端が完了してもdirty集合・表示items・frame/input番号を消費しない回帰 `egui037_continuation_viewport_retirement_remote_close_retains_pin_dirty_for_grid_ui` を追加した。実行結果は末尾へ記録する。
3. **discard/immediate間のdelta順序。** eguiの第1pass deltaは`Context::run_dyn`のローカルFullOutputへ保持される。第2passで同ID immediateを再openすると、旧native callbackのTextureHandle破棄によるfreeが実immediate consumerから先にsubmissionできる。その後に最外側の旧set/partialを配送するとfree→旧setとなる。現回帰はfake adapterにdeltaを保持しており、このsubmission順を証明しない。discard retry直前に生成済みdeltaを既存backend共有ownerへ先行移譲するintegration callback案を設計照会した。新しい所有受渡しの設置/解除・Context一致・テスト隔離・二重配送防止が未定義のため、**新callbackは未実装**。第二queue・遅延cleanup・retryで埋めない。

   追加のread-only検討では、callbackを増やさず既存の共有TextureManagerへ未配送deltaを戻す案も確認した。
   ただし現epaint 0.37はprivate deltaと`take_delta`しか持たず、metadataを変えないprepend APIはない。
   `set/free`再演は既に消えたmetadata・retain count・partial寸法を壊すため使えない。
   実現するならepaintの追加patchと「旧local→GC/drop後の新manager delta」の年代順合流契約が必要になる。
   既存`TexturesDelta::append`は新freeが旧setを除去するため、whole/partial→submit→freeを要求する回帰への適合も明文化する。
   実retry時だけlocalをtakeして返し、上限到達・最終pass・direct begin/endには適用しないこと、output pluginの観測が重なることも検討対象。
   実装担当と独立reviewerがそれぞれsourceで前提を確認したが、これは追加構造の合意や実装ではない。

レビューで指摘されたstandalone C1退役漏れと、immediate prepareで旧native資源を新寿命へretagする問題はsource上補修済み。実行結果を確認するまでは回帰成功に数えない。

### 再開後の追加検証（実装担当の内部確認）

全CargoはRust 1.99.0 / offline / `MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=1`。
ログとcommand/exit/秒数は `target/egui037-stage3-resume-<label>.log/.json` に保持する。
standalone vendorも実体のroot `target`を指定し、reparse pointや共有リンクは使わない。

| コマンド / label | 結果・選択件数 | exit / 秒 |
| --- | --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core --offline` / `core-checkpoint` | 成功。後続のfade設定・testのみ補修とは区別 | 0 / 40.84 |
| 同 `--features test-script` / `core-test-script` | 成功 | 0 / 55.53 |
| `cargo check --workspace --offline` / `workspace-initial`, `workspace-second` | 初回は編集中構文3件、2回目はlauncherのrelease入力3実行ファイル不足。全体成功ではない | 101 / 123.29, 54.68 |
| 参考 `cargo check --workspace --exclude mimageviewer-launcher --offline` / `workspace-excluding-launcher`, `workspace-reference-final` | 初回labの旧surface設定3件を補修後、全workspace member（launcher除外）のcheck成功。全体gateの代用にはしない | 101 / 41.02 → 0 / 15.83 |
| `cargo test -p mimageviewer --lib --no-run --offline` / `lib-compile-initial`, `lib-compile-second`, `lib-compile-third` | API/fixture 199件→12件を補修し、3回目成功。3回目開始後に追加したgamepad等の最終source検証とは区別 | 101 / 284.78、101 / 114.17、0 / 325.87 |
| 参考 `cargo check -p mimageviewer --tests --offline` / `all-test-typecheck`, `all-test-typecheck-final` | snapshot fixture 4件、続いてkey fixtureの誤ったunit値へのdelta破棄1件を検出し補修。未成功の結果を全test compile成立とは扱わない | 101 / 74.69、101 / 119.80 |
| `cargo test -p egui --lib viewport_retirement --offline` / `egui-retirement`, `egui-retirement-final`, `egui-retirement-fixture` | 初回compile 2件補修。次は新fixtureのchild RawInput不足5件（14選択、9成功/5失敗）を補修し、最終14選択/14成功/0失敗/0ignore | 101 / 21.72、101 / 7.13、0 / 3.04 |
| `cargo test -p eframe ... --features default_fonts,wgpu` / `eframe-retirement-wgpu` | workspace外packageにfeature指定不可というCargo制約で未選択。正式standalone manifestを使用する | 101 / 0.05 |
| `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_retirement --no-default-features --features default_fonts,wgpu --offline` / `eframe-retirement-wgpu-manifest`, `eframe-retirement-wgpu-final` | map deltaのfixture `.set`を`.push`へ補修後、10選択/10成功/0失敗/0ignore | 101 / 125.75 → 0 / 9.52 |
| 同 `--features default_fonts,glow` / `eframe-retirement-glow` | 10選択/10成功/0失敗/0ignore | 0 / 78.27 |
| `cargo test -p egui --lib ime_interrupt --offline` / `ime-interrupt` | 6選択/6成功/0失敗/0ignore | 0 / 0.49 |
| `cargo test -p egui-ime --offline` / `egui-ime` | lib 30選択/30成功、docは0件。libの0件成功ではない | 0 / 33.34 |
| `cargo test -p egui-winit --lib --offline` / `egui-winit` | workspace外packageのdev-dependencyが必要というCargo制約で未選択 | 101 / 0.46 |
| `cargo test --manifest-path vendor/egui-winit/Cargo.toml --lib --offline` / `egui-winit-manifest` | 43選択/43成功/0失敗/0ignore。`scripts/test-full.ps1`も同manifestへ修正（script自体は実行しない） | 0 / 66.32 |
| eframe正式manifest `minimized_full` / `minimized-wgpu`, `minimized-glow` | wgpu 9選択/9成功、glow 11選択/11成功、失敗/ignoreなし | 各0 / 0.33, 0.26 |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-focused.ps1` / `focused` | broad/shared変更のため選択実行せず、全lib gateを要求。選択0を成功にはしない | 2 / 0.93 |
| `cargo tree -d --offline` / `tree` | 旧egui/eframe 0.33/0.36、wgpu27、vendor-next参照なし。Naga27等の既存別経路は維持 | 0 / 0.47 |
| `cargo fmt --all`, 同最終 / `format`, `format-final` | 成功 | 各0 / 14.35, 14.23 |
| `cargo fmt --check` / `fmt-check` | 成功（この後の機械的fixture補修は末尾の最終結果を使用） | 0 / 13.53 |
| `git diff --check` / `diff-check` | 成功 | 0 / 0.43 |
| `python scripts/check_ui_glyphs.py` | dangerous glyph 0件 | 0 / 2.49 |

新gamepadのphysical/delivered境界の独立レビューで、退役済みownerのfallbackが新DPad repeatを消すP2を確認した。
`terminate_rating_picker_in_owner`へfallbackを合流し、通常manual/source-adoption wrapperだけに物理抑止を維持する修正を実施。
別contextのSol / xhighがfocused再確認し、P2解消・追加findingなしとした。これはruntime回帰成功やClose/deltaのP1解消を意味しない。

§5の推奨に従い `src/os_theme.rs` の既存Light/Dark scroll style ownerで `scroll.fade.strength = 0.0` を明示した。
文字baseline/hinting、色付き絵文字、margin/min sizeの旧版合わせは行わず、段4bへ残す。

### 第4.5版追加13項目の対応（部分検証）

eframeの下表suffixは `native::{wgpu,glow}_integration::viewport_retirement_tests::viewport_retirement_{wgpu,glow}_<suffix>`。
feature別各10選択の結果に基づき、該当する同名scenarioは各backend 1件ずつ成功した。
eguiの新6件は `context::stage3_viewport_retirement_tests`、旧8件と合わせてfilter全体14選択。
native fake adapterの成功を実GPU submission・OS/HWND・製品UIの成功へ拡大しない。

| 項目 | 回帰対応 / 正式選択結果 | 未証明・残り |
| --- | --- | --- |
| 1 実Full logicから退役 | eframe `full_logic_gc_discard` 各1成功 | 実GPU/live外 |
| 2 LogicOnly・UIなし | eframe `logic_only` 各1成功、egui `logic_transfer_exact_descendants_and_no_phantom_registration` 1成功 | App退役filterも10成功。ただし表示継続は別P1 |
| 3 end_pass GC後のroot承認 | eframe `context_gc_native_descendants_ime` 各1成功、egui `gc_discard_keeps_authority_in_root_until_outer_return` 1成功 | App旧binding回帰追加済み |
| 4 nested immediate最外側 | eframe `immediate_nested` 各1成功、egui `immediate_nested_pass_defers_transfer_and_cleanup` 1成功 | 未配送delta順は項目11のP1 |
| 5 実discard保持・C1先行禁止 | eframe `full_logic_gc_discard` 各1成功、eguiの上記GC/discard 1成功、App `remote_full_and_discard_wait_for_outer_delivery` 1成功 | deltaのsubmission順P1は別途残る |
| 6 exact子孫・兄弟・部分GC | eframe `context_gc_native_descendants_ime` 各1成功、egui旧subtree回帰成功 | workerのApp ownerは別回帰 |
| 7 old output/append/reopen | eframe `old_output_reopen_duplicate` 各1成功、egui `old_output_append_and_reopen_never_mix_lifetimes` 1成功 | liveは段7 |
| 8 拒否消費・新規Close・非管理 | eframe `close_request_separation`, `hook_captures_lifetime_before_reopen` 各backend各1成功 | Appの表示継続P1が残る |
| 9 App全終了入口・parking存続 | `src/app/viewport_retirement_tests.rs` にmode change/GC/初回登録前/重複/parking/modal hookを追加、最終filter10成功 | bookmark/親一覧の既存意味を修正する必要あり。全入口の表示継続まで受入完了ではない |
| 10 C1 Full/LogicOnly/discardとreadiness | Appにsave ACK待ち、standalone GC有無、Full/discard各回帰を追加。dirty保持を含むApp退役filterは最終10成功 | Close内dirty採用は補修済み。Close表示継続の所有は項目9のP1 |
| 11 delta set/partial/free・root終了 | eframe `delta_callback_drop` 各1成功、egui unlocked callback/drop free 1成功 | **discard→immediate実consumerのsubmission順は未証明・P1・追加設計待ち** |
| 12 GC/approval重なり・一回終了 | eframe `overlapping_gc_root_close` 各1成功、egui overlapping/direct end_pass 1成功 | Appのowner合流は別filter |
| 13 root Close同時・tray/save/CancelClose | eframe `overlapping_gc_root_close` 各1成功。実App→EpiIntegration root lifecycle 3件も成功、root全filterは11成功 | 全組合せの製品受入完了ではない |

段3の必須回帰・第4.3版1〜16について、前掲の歴史表は削除しない。
今回追加した実App接続は `root_lifecycle_actual_*` 3件、実logic context-state 2件、実logic gamepad 1件、
App退役の当初9件とdirty保持1件であり、最終選択件数は次の実行結果欄で記録する。
既存`egui037_continuation`32件・実seam3件・dirty保持1件を合わせた36件は実行して成功した。
製品IME/discrete/WH_GETMESSAGE/native delta helperと全test target no-runも以下に結果を記録した。
C2/C4/C5/D3の最小化root Full旧順序などの接続証拠の不足、lib全件最後の1回と失敗分類は未達のままであり、
選択filter成功を段3全体のPASSへ拡大しない。

### 正式App graphの実行結果（実装担当の内部確認・追記）

以下は上の定義件数と区別した実選択結果。後続修正がある項目は、最終再実行を別行へ記録する。

| コマンド / `resume-` label | 選択・結果 | exit / 秒 |
| --- | --- | --- |
| `cargo test -p mimageviewer --lib --no-run --offline` / `lib-compile-current` | 全lib testのコンパイル成功。テスト実行0件を回帰成功件数にはしない | 0 / 290.77 |
| 同 `viewport_retirement` / `app-retirement` | 9選択 / 9成功 / 0失敗 / 0ignore | 0 / 2.89 |
| 同 `egui037_continuation` / `continuation` | 35選択 / 31成功 / 4失敗 / 0ignore。ZIP headless出力未処理2件、旧update文字列検査2件を検出して補修 | 101 / 6.88 |
| 同 `root_lifecycle` / `root-lifecycle` | 11選択 / 8成功 / 3失敗 / 0ignore。上記ZIP2件と旧update検査1件。実App→EpiIntegrationの追加3件は成功 | 101 / 2.95 |
| 同 `context_state` / `context-state` | 8選択 / 8成功 / 0失敗 / 0ignore | 0 / 2.02 |
| 同 `gamepad_` / `gamepad` | 94選択 / 90成功 / 4失敗 / 0ignore。Westのdiscrete回帰とcontinuous tickのfixture干渉1件、Context lock再入3件 | 101 / 50.11 |
| 上記実test executableの1件exact再現 / `gamepad-lock-trace` | 1選択 / 1失敗。backtraceで`record_current_pass`の`ctx.input`内`ctx.options`再取得を特定 | 101 / 11.39 |
| `cargo check -p mimageviewer --tests --offline` / `all-test-typecheck-current` | 全test targetの型検査成功（後続のlock hoist後の最終結果は別行） | 0 / 119.16 |
| eframe正式manifest `viewport_input` / `viewport-input-wgpu`, `viewport-input-glow` | wgpu 5選択/5成功、glow 5選択/5成功、失敗/ignoreなし | 各0 / 0.61, 0.24 |
| `cargo fmt --check` / `fmt-check-latest` | lock修正後の改行整形差を検出。`cargo fmt --all`で整形し再確認する | 1 / 34.15 |
| `cargo test -p mimageviewer --test ui_snapshot --no-run --offline` / `ui-snapshot-compile` | コンパイル成功。PNG実行/更新なし | 0 / 168.78 |
| `cargo test -p mimageviewer --test ui_snapshot --test book_composition_editor_snapshots --no-run --offline` / `snapshot-compile-final` | 両test executableのコンパイル成功。後続dirty consumer 2呼出し除去より前の結果 | 0 / 55.49 |
| `cargo test -p mimageviewer --lib egui037_continuation --offline` / `continuation-final` | fixture補修・lock hoist後、35選択/35成功/0失敗/0ignore。後続dirty保持回帰1件の追加前 | 0 / 200.32 |
| `cargo fmt --all` / `format-lock-fixes`, `format-dirty-fix` | 整形成功 | 各0 / 14.42, 14.19 |
| `cargo fmt --check` / `fmt-check-final` | lock hoist整形後成功。後続dirty補修後の最終checkは別行へ記録 | 0 / 14.08 |
| `cargo test -p mimageviewer --lib egui037_continuation --offline` / `continuation-dirty-final` | dirty保持回帰を含め36選択/36成功/0失敗/0ignore | 0 / 164.57 |
| 同 `root_lifecycle` / `root-lifecycle-final` | 11選択/11成功/0失敗/0ignore | 0 / 4.42 |
| 同 `context_state` / `context-state-final` | 8選択/8成功/0失敗/0ignore | 0 / 2.02 |
| 同 `gamepad_` / `gamepad-final` | 94選択/94成功/0失敗/0ignore。West、退役picker terminal、lock再入の3 sibling経路を含む | 0 / 21.11 |
| 同 `native_ime_interrupt` / `native-ime-final` | 3選択/2成功/1失敗。新fixtureが既存`batch_prepare`を呼ばずstrip/panel rect未初期化だったため補修 | 101 / 0.78 |
| 同 `texture_output_` / `texture-output-final` | 2選択/2成功/0失敗/0ignore | 0 / 0.69 |
| 同 `wheel_input::` / `wheel-input-final` | 6選択/6成功/0失敗/0ignore | 0 / 0.72 |
| 同 `keyboard_input::` / `keyboard-input-final` | 17選択/17成功/0失敗/0ignore | 0 / 0.71 |
| 同 `synthetic_modifier` / `synthetic-modifier-final` | 1選択/1成功/0失敗/0ignore | 0 / 0.73 |
| 同 `ime_focus::` / `ime-focus-final` | 12選択/10成功/2失敗。child登録寿命のfixture、実egui出力fixtureのraw TextEdit在庫登録を調査・補修 | 101 / 0.98 |
| 同 `getmessage` / `getmessage-final` | 1選択/1成功/0失敗/0ignore | 0 / 0.74 |
| 同 `--no-run` / `lib-no-run-final` | dirty保持回帰を含む全lib testコンパイル成功。後続IME fixture補修の最終結果は別行 | 0 / 0.65 |
| `cargo check --workspace --all-targets --exclude mimageviewer-launcher --offline` / `workspace-all-targets` | 参考gate。comic_lab testの9件API漏れを検出し、Context/root Ui入口とPreeditを補修 | 101 / 212.76 |
| `cargo fmt --check` / `fmt-check-dirty-final` | dirty補修・comic fixture補修後成功 | 0 / 13.77 |
| `cargo check --workspace --all-targets --exclude mimageviewer-launcher --offline` / `workspace-all-targets-final` | launcher以外の全workspace member/test targetの型検査成功。全workspace gateの代用にはしない | 0 / 11.88 |
| `cargo check -p mimageviewer --bin mimageviewer-core --offline` / `core-final` | dirty採用分離後のcore成功 | 0 / 19.71 |
| 同 `--features test-script` / `core-test-script-final` | 成功 | 0 / 19.55 |
| `cargo test -p mimageviewer --lib native_ime_interrupt --offline` / `native-ime-fixture-final` | 補修後3選択/3成功/0失敗/0ignore | 0 / 134.19 |
| 同 `ime_focus::` / `ime-focus-fixture-final` | 補修後12選択/12成功/0失敗/0ignore | 0 / 1.00 |
| 同 `viewport_retirement` / `app-retirement-final` | dirty保持を含め10選択/10成功/0失敗/0ignore | 0 / 2.44 |
| `cargo fmt --check` / `fmt-check-handoff` | IME fixture補修後成功 | 0 / 14.08 |
| `git diff --check` / `diff-check-handoff` | 成功 | 0 / 0.45 |
| `cargo test -p mimageviewer --tests --no-run --offline` / `all-tests-no-run` | 現default graphの全23 test executable（lib 1 / core bin test 1 / integration 21）コンパイル成功。2種類のsnapshotも含む。実行/PNG更新なし | 0 / 106.17 |
| `cargo check --workspace --offline` / `workspace-handoff` | 最終再確認もlauncherのrelease入力3実行ファイル不足だけで停止。`target/release/mimageviewer-{core,remote,epub-pdf}.exe`が存在しないことを確認。代用ファイル/規則回避/先取りrelease buildなし | 101 / 18.70 |
| `cargo test -p mimageviewer --lib --no-run --offline` / `lib-no-run-handoff` | IME fixture補修を含む現sourceで成功 | 0 / 0.61 |
| `cargo test -p mimageviewer --test ui_snapshot --test book_composition_editor_snapshots --no-run --offline` / `snapshot-no-run-handoff` | 現sourceの両targetで成功。PNG実行/更新なし | 0 / 0.64 |
| `cargo tree -d --offline` / `tree-handoff` | 旧egui/eframe 0.33/0.36、wgpu27、vendor-next参照なし | 0 / 0.48 |
| `scripts/test-focused.ps1` / `focused-handoff` | 最終diffでもbroad/shared判定のため未選択。0件成功とは扱わない | 2 / 0.87 |
| `python scripts/check_ui_glyphs.py` / `glyphs-handoff` | dangerous glyph 0件 | 0 / 2.20 |

gamepad P2の退役済みpicker terminal回帰は上記94件に含まれて成功した。
lock再入はタイムアウトしたcommand runnerによる失敗ではなく、eguiの10秒debug lock診断である。
read closure外でInputOptionsを取得する修正を、fullscreen・grid・dialog・関連fixtureの同種経路へ適用する。
Westは保持軸0.9を消さず、既存timerでdiscrete境界と次continuous tickを分け、両方の既存動作を検査する。
これらを追加delay・retry・期待値弱化で回避しない。
別contextのSol / xhighはWest検査の期待値弱化なしと、製品wheel consumerの局所hoistの妥当性をsourceで確認し、追加findingなしとした。
入力参照中の直接options再入について独立に189呼出しを走査し、同型残件なし。App側関連fixtureの4箇所も同様に確認した。
これはClose/delta P1の受入や、修正後runtimeの代証ではない。

IME facadeのchild回帰は、RawInputのmetadataだけではroot end-pass GCを止めない0.37の登録契約にfixtureが未対応だった。
親の各passで正式なdeferred childを宣言し、従来の診断・events・focus contract不変の検査を維持した。
最後に宣言を止めてstale metadataだけを残すとIME policyが退役する検査も追加した。
nativeの実egui出力生成fixture 8箇所は既存監査のpath・限定anchor・正確な件数（4+2+1+1）・理由の在庫へ登録した。
全て`#[cfg(test)]`内であり、製品自由入力欄を除外せず、監査の照合規則も緩めていない。
native複数pass回帰のstrip/panel rectは既存`batch_prepare`によって初期化し、一回配送の検査は維持した。
WIPの同fixtureに残っていた`??? sample`は`日本語 sample`へ戻した。
別contextのSol / xhighは、この3点を限定source reviewし、期待値弱化・監査の穴・追加findingなしとした。
正式な実行結果は以下の最終結果欄を用いる。

### 第4.3版1〜16の現在の対応と不足（実装担当の内部確認）

件数は個々のfilter内の実選択に基づく。同じ回帰を複数項目で参照するため、行の件数を合算しない。
この表は受入完了表ではなく、正式graphで得た証拠と残る接続検証を分けた表である。

| 受入 | 正式graphの回帰名・件数 | 残る接続証拠 / 契約 |
| --- | --- | --- |
| 1 最小化Fullの旧順序 | backend `minimized_full` wgpu 9 / glow 11成功。`remote_normal_driver_retains_both_display_checkpoints` は新seam検査へ補修 | C2〜C5/D3を一緒に通す製品root Fullの実採用・旧順序の証拠は未完 |
| 2 同frame複数LogicOnly | `viewport_retirement_remote_logic_waits_for_command_delivery_and_existing_save_ack`、`remote_service_acquire_ack_and_return_do_not_consume_display`、実logic context-state 2件とgamepad 1件が成功 | C1 dirty保持の追加回帰も成功。Close表示継続・deltaのP1は別に残る |
| 3 対象Fullへの一回採用 | `remote_display_slot_is_consumed_once_after_return`、`ai_live_result_waits_in_existing_backlog_for_ui` 各1成功 | 全C2〜C5の実Full/discard一回採用を網羅した証拠ではない |
| 4 保存/resume/共有ACK | `sent_save_terminal_unblocks_remote_without_editor_ui`、`remote_resume_response_uses_receiver_without_ui`、`remote_shared_bookmark_router_consumes_each_reply_once`、上記App退役save ACK 各1成功 | dirtyのlogic表示採用は補修・回帰成功 |
| 5 返却→再取得→遅いFull | `remote_return_reacquire_late_ui_discards_old_slot`、`remote_old_return_identity_cannot_reload_after_later_return` 各1成功 | 旧bookmark/親一覧Close継続のP1は別途必要 |
| 6 live結果の表示保留と取消 | `egui037_continuation_ai_*` 5件成功（取消済みexact job、upscale、孤児終端、live backlog、disconnect） | 全C3の製品Close/Remote経路を網羅した証拠ではない |
| 7 新FullのpointerによるC4再開 | 現時点では正式接続の選択結果なし | 古いprimary_downを流用しない実Full回帰が必要 |
| 8 C5表示予算とruntime終端 | `egui037_continuation_app_drain_lease_drop_terminals_every_representative_resource_before_one_ack` 1成功 | page-turn/preview/PDF予算の実UI継続の証拠は未完 |
| 9 D3 cutoff/cause/ID | `egui037_continuation_hydration_*` 3件成功（context mount、cancel/restart/IndexOnly、採番枯渇） | cutoff ACK/mtime/signatureの正式採用境界の網羅は未完 |
| 10 非描画出力 | backend `minimized_full` 計20成功。App `texture_output_` / `native_ime_interrupt` は別欄の実行結果を参照 | discard→immediateの実submission順P1が残り、fake delta保持を代証にしない |
| 11 viewport入力・skip・退役 | eframe `viewport_input` wgpu 5 / glow 5成功。egui退役14、eframe退役各10、App退役9成功 | Appの表示継続とdeltaのP1が残る |
| 12 root Close UIなし | `root_lifecycle_actual_app_*` 3件成功、backend `minimized_full_root_logic_only_close_{wgpu,glow}` 各1成功（上記filterに含む） | ZIP/settings-family補修後のroot filter全11件成功 |
| 13 modal拒否・Quit優先 | `root_lifecycle_modal_rejection_is_not_deferred`、`root_lifecycle_tray_once_and_explicit_quit`、実App modal rejection 各1成功 | root全filterの最終結果は別欄へ記録 |
| 14 別contextのreceiptのみ | `context_state` 8選択/8成功。実logic receipt・未採用owner結果の2件を含む | 追加dirty保持も成功。Close表示継続のP1とは別の証拠 |
| 15 正当なUI mountで表示再採用 | `context_state_retains_rating_and_page_edit_until_ui_boundary`、`context_state_real_logic_preserves_unadopted_owner_results` 各1成功（上記8件に含む） | 通常のUI採用を止める変更なし。全viewer経路の受入完了ではない |
| 16 immediate子描画資格とScreenshot | backend `minimized_full` wgpu 9 / glow 11成功、各backendのimmediate描画/非描画/Screenshotを含む | OS/windowの実機確認は計画どおり後段。delta順P1は段3内で解消が必要 |

最終lib全件は、残る段3の契約違反を補修した後の最後の1回として留保している。
現時点では未実行であり、snapshot/見た目・段4対象の挙動変化・段3契約違反という全件失敗分類は未取得。
既知の段3P1を段4へ移さず、未実行を0失敗とは記録しない。

### 最終引継ぎ状態（実装担当の内部確認）

第4.5版の退役移譲、C1のlogic接続、root lifecycle、context-state、gamepad、IME・入力、
App/ラボ/test APIの移行を進め、上記の正式graphの選択件数とdefault全test target compileを得た。
ただし**段3は未完了**であり、独立レビューの全体ACCEPTはない。

- 設計判断が必要なのは前掲1のClose表示継続と3のdiscard/immediate delta配送。
  前者は破棄前の宛先・grid/summaryを既存return/navigation ownerへ型付きで渡し、Remote取得・新navigation・新bookmarkで失効する契約が要る。
  後者は既存backend共有ownerへの先行移譲、またはepaintのmetadata非変更prepend APIと共有managerへの再預託のいずれかについて、合流順・一度配送・plugin観測を追加定義する必要がある。
  新たなcallbackやepaint vendor追加は未実装で、§11の構造合意を先取りしない。
- 既存owner内で解消できたdirty P1は補修済みで、実logic回帰も成功した。
  2件のP1を段4へ残す判断や、表示継続の削除・遅延cleanup・repaint・第二queueでの回避はしていない。
- launcherを含むworkspace gateはrelease入力3ファイル不足で未達。
  launcher以外の全workspace target型検査と本体default全23 test executableコンパイルは成功したが、全workspace成功の代用にしない。
- 第4.3版1〜16の接続証拠の不足（特に製品FullでのC2〜C5/D3旧順序、新pointerによるC4再開、C5予算とD3 cutoff）と、fullscreen混在wheelの複数navigation配送は段3内の残り。
  最終lib全件は、契約補修後の最後の1回として未実行。全件失敗分類も未取得。
- 段4bへは文字baseline/hinting・色付き絵文字・margin/min sizeの比較を、段5へはPNG更新とId移行を渡す。今回snapshot PNG・期待値・test ignoreは変更していない。

`vendor-next/`は撤去済みで、準備物はignoredな`target/egui037-stage3-preparation-archive`に実体を保持した。
正式vendorに原本記録とPATCHESを残し、旧pathへのactive参照・旧eguiの型分裂・必要crate欠落は検出していない。
新規のAppファイルは`src/app/viewport_retirement.rs`と`src/app/viewport_retirement_tests.rs`で、どちらも未追跡のまま引き渡す。
indexは空、コミット・git add・branch操作・push・ネットワーク取得なし。
live・製品起動・`test-full.ps1`・確認用build/release buildは行っていない。
通常のbuild-dev引継ぎは今回の段3未完・計画上のbuild前提未達のため行わない。
