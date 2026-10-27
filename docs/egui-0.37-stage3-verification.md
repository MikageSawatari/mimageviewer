# egui 0.37 更新 — 段3 検証台帳

## 状態（2026-10-17）

**最新状態（3aa2d8eda後、分類第3版）:** A〜D独立レビューACCEPT後に設計担当が実行した全件113失敗を照合した。PNG103・段4差7に加え、bookmark fixture移行漏れ1、既存HUDのWin32通知前提1、既存metadata testの同期競合候補1。bookmark fixtureを実在rowの採用へ補修し、空一覧採用回帰も追加した。復元後の関連596件（指定3件を含む）は全成功、全23 test executableのコンパイル・fmtも成功。HUDとmetadataは限定成功だけで解消扱いせず、既存テストの設計判断待ち2件を第3版に明示する。末尾の「最終ソース全件との照合」を今回の正本とする。製品コード・PNG・ignoreは変更せず、lib全件も再実行しない。HEAD `3aa2d8eda`、ブランチ`egui-036`のまま未コミット・未ステージで返す。以下は実装担当の内部確認で、設計担当の最終受入とは区別する。

**再確認2件の補修時の報告（履歴、7cb0494f9後）:** 残るCのP3・DのP2はコードで成立を確認し、既存owner内で補修した。新規2回帰は旧処理へのsource変異で各1件の指定assert失敗を確認し、復元後は成功。限定lib42件成功、両integration9件中8件成功・1件は既存PNG2枚の不一致（geometry全12ケース成功）。core check、全test targetコンパイル（23 executable）、fmtは成功。末尾の「再確認の残件への対応（7cb0494f9後）」を今回の正本とする。lib全件は再実行せず、元290失敗の分類第2版（補修180／PNG103／段4差7）を維持する。HEAD `7cb0494f9`、ブランチ`egui-036`のまま未コミット・未ステージで返す。以下は実装担当の内部確認で、設計担当の最終受入とは区別する。

**初回レビュー補修時の報告（履歴・分類第2版）:** 設計担当の段3コミット`1e55235f1`に対する独立レビューA〜Dの9指摘を補修し、変異による検出と指定の限定確認を完了した。末尾の「独立レビュー指摘への対応」を今回の正本とする。旧9件を再分類し、元の全件290失敗は補修成功180・PNG比較103・段4のlayout/挙動差7とした。lib全件は利用者指定により再実行していない。全test targetコンパイル（23 executable）・core/test-script check・fmtは成功。HEADとブランチを維持し、未コミット・未ステージで返す。これは実装担当の内部確認であり、設計担当の最終受入とは区別する。

**独立レビュー前の報告（履歴・分類第1版）:** 第4.6版と双方の構造合意（97ab24ae1 / 5c1584d8a）に基づく段3の実装・指定自動確認を完了。**実装担当の内部確認では、未分類・残る段3契約違反は0件**。設計担当による最終受入とは区別する。最後に1回実行したlib全件は290失敗で、その後の限定検証により179件の移行不備を解消し、PNG比較102件と段4のlayout/挙動差9件へ分類した。全件を最終ソースで再実行したとは扱わない。後述の最終コマンド表と残課題を正本とし、過去の論点・未達記録は履歴として残す。HEAD `5c1584d8a`、ブランチ`egui-036`を維持し、未コミット・未ステージで返す。

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

## 第4.6版の再開記録（実装担当の内部確認）

再開時のHEADは `5c1584d8a`、作業ツリーはclean。2回目のWIP `fe32f2e88`、第4.6版 `97ab24ae1`、独立設計ACCEPTと§11の双方合意 `5c1584d8a` を前提とした。
前掲「設計担当へ返す論点」のClose帰還とdelta配送は、第4.6版で設計上解決した。過去の論点・失敗・未達記録は削除しない。

### Close帰還の所有と接続

主なファイル: `src/app/close_return.rs`、`src/app.rs`、`src/app/viewer_context_registry.rs`、`src/app/viewport_retirement.rs`、`src/app/snapshot_ops.rs`、`src/app/collection_navigation.rs`、`src/remote_ipc/ui.rs`、`src/ui_fullscreen.rs`。

- 既存 `pending_return_to_parent` boolを一つの型付き状態に変更した。要求なし／originだけを保持するViewing／生存viewerのLive／終端viewerから移譲済みのTerminalを区別する。新しいApp field・採番・queueは追加していない。
- originは既存MainListNavigation proof、main context、quick-folder切替identity、BookmarkOpenRequestIdとtargetを保持する。Opening／Detachedと同じcontext bundleで移動し、build・forkで捕捉、park／resumeで保持する。mediaの内部移動でBookmarkViewStateが消えても元ownerを失わない。
- Closeのmodal判断後、teardown前にsummary・帰還先・選択intentを捕捉して存続mainへ移す。summaryはfullscreenの現在mediaを優先する。親resolverを副作用なしの値生成へ抽出し、変換アーカイブの元pathと既存の優先順を共有した。
- bookmark帰還の判断を `bookmark_close_return_effect` に一本化した。logicはread-modelを採用・再構築せず、mainの既存UI consumerがrefresh／Bookmarks／Direct navigationへ一度だけ渡す。現在のmainの選択・scrollを優先し、row keysとopened keyを維持する。
- active at-rest、standalone、parked／passive、通常UI、早期return、native即時consumerを接続する。loading shell／holdoverの真Closeとnative hard-closeも、通常helperを迂回する入口として棚卸しした。
- Remote、新main navigation、同pathへのnavigation、新bookmarkは受付境界で旧要求・旧originを失効する。Close causeはUser／Replaced／RemoteAcquire／AppExitで渡し、reason文字列から推測しない。parkingのsnapshot不可・removed-pathの置換にもReplacedを渡す。
- `src/app/close_return_tests.rs` の11件は、実EpiIntegrationのLogicOnlyとFull、production native poll、実immediate child UIを使う。headless adapterにはnative OS Close loopがないため、LogicOnly wrapperが製品Close callbackを呼ぶ点を実OS受付の証拠とはしない。
- Live帰還のCloseではteardownまでにslideshow timerが進み得る既存経路を確認した。真Close入口から既存の停止処理を呼び、帰還待ちでも対象viewerの自動navigationを終端する。内部folder reopenはこの入口を使わない。
- この停止処理はbundle外のpopupと共有archive変換も触っていたため、投影中contextの停止本体と既存popup操作を分離した。共有変換の取消は既存requestのorigin contextで2段とも対象限定し、同contextの古い世代のcleanupは維持する。兄弟の変換・receiver・cancel tokenとmainのtimer／popupを壊さない回帰を追加した。新しいcontext採番・状態・待機・一括resetはない。

### discard中のdelta配送

主なファイル: `vendor/egui/src/context.rs`、`vendor/egui/src/discard_texture_delivery_tests.rs`、`vendor/eframe/src/native/{wgpu_integration,glow_integration,viewport_lifecycle}.rs`、`vendor/egui-wgpu/src/winit_tests.rs`、各 `PATCHES.md`。

- 第4.6版の候補(a)を実装した。実際のdiscard再pass直前、output hook変更後の蓄積deltaだけをContext束縛TLS RAII scopeへmoveする。最終pass・limit到達・direct begin/end・scopeなしは通常FullOutputに所有を残す。
- scopeは既存backend共有renderer ownerへのWeakを持ち、TLS／Context／texture managerのborrowやlockを保持せずconsumerを呼ぶ。正常終了・早期return・unwindで登録を復元し、Context間・入れ子scopeを分離する。scopeに第二のpending deltaは置かない。
- root／immediate／retirement／discardを既存shared pendingと非相殺の受入へ合流した。古いset／partialを先に処理し、wgpuは描画なしでもupload／submission／freeを完結、glowも既存resource consumerを使う。cleanup遅延・追加present・repaint・UI強制・epaint変更はない。
- egui 4件、eframe wgpu 2件／glow 2件、egui-wgpu 2件を追加した。egui-wgpuは実resource consumerとoffscreen readbackを検査し、GPU前提不成立を成功skipしない。glowはnative adapterとfake GL callsによるresource順序の証拠であり、実GL環境の成功とは扱わない。
- 製品SubmissionPluginのscopeあり／なし比較は `src/ui_fullscreen/page_turn.rs` に別の回帰を追加した。IME・receipt・表示ACKの観測をdelta受入と混同しない。

### 製品Fullと入力の追加接続

主なファイル: `src/app/egui037_connection_tests.rs`、`src/app/update_phases.rs`、`src/app/sidecar_restore.rs`、`src/ui_fullscreen/page_turn.rs`、`src/ui_fullscreen.rs`、`src/wheel_input.rs`、`src/ui_fullscreen/page_turn/barrier.rs`。

Full接続10件は、旧checkpointへのtest専用read-only observerを使い、実App::uiとEpiIntegrationでC2〜C5／D3の結果採用を検査する。新しいpointer両反転、C5のframe予算、D3 cutoff・cause・restart・hydration IDを含む。test observerを製品のpollやgateの代用にはしない。
fullscreen混在wheelはpass-localの順序付き値と既存frame調停・navigation ownerを接続し、bar／HUD／click／key／Closeの既存優先、page-turn barrierの受理・拒否を保つ範囲で実装する。永続queue・新pending fieldを追加しない。実選択結果は後述のコマンド表へ記録する。music timelineのwheel投影は `src/ui_music_timeline.rs` のoptions取得もinput closure外へ移した。

### 検証方針の今回の明示変更

launcherのrelease入力3本不足は既知の前提であるため、利用者の今回の明示指示に従い、`cargo check --workspace --exclude mimageviewer-launcher` とlauncher以外の全target compileをworkspace受入の代替gateとする。前掲の「代用しない」は前回時点の判断の履歴である。代用exeの配置、launcherのbuild規則変更、先取りrelease buildは行わない。
vendor standalone lockは原本ではなく、正式manifest graphを通常のoffline Cargoで解決した記録である（各PATCHESの既存説明どおり）。原本hashはprovenance inventoryに残す。

### 第4.6版の回帰対応表（実装担当の内部確認）

件数は正式コマンドの選択数であり、表の行を合算しない。`close_return` は新規11件と既存bookmark media 1件の計12件、native同dispatchは別filterの1件である。
以下の短い名前は `src/app/close_return_tests.rs` の `close_return_` 後のsuffixを表す。最終実行の結果は後続のコマンド表を用いる。

| Close受入 | 回帰名・接続の証拠 |
| --- | --- |
| 1 active book/mediaの終端と帰還保持 | `actual_logic_only_active_book_and_media_wait_for_main_full`。book/media各条件で実logic、binding/worker終端、main Fullの一回消費 |
| 2 media離脱・book内部移動 | `media_departure_uses_last_real_folder_while_book_navigation_keeps_grid`。最後の実folder/mediaとbookmark一覧を区別 |
| 3 standalone・auto-open・実親path | `standalone_bookmark_navigation_survives_viewer_teardown`、`auto_open_zip_pdf_image_folder_and_converted_source_preserve_parent_selection`。ZIP/PDF/folder/変換元の4条件 |
| 4 main選択・scroll・row復元 | `main_selection_scroll_and_same_list_refresh_survive_until_consumer`。実read-model receiver採用と既存grid復元を経由 |
| 5 parked/passive・置換・兄弟 | `parked_sibling_full_cannot_consume_surviving_main_request`、`same_path_navigation_and_new_bookmark_invalidate_old_origins_at_admission`、`replaced_remote_and_app_exit_causes_never_publish_user_return` |
| 6 Close→Remote→遅いFull | `remote_acquire_and_return_never_resurrects_terminal_request`。取得・ACK・drain・返却は帰還UIを待たない |
| 7 同path新navigation・新bookmark | `same_path_navigation_and_new_bookmark_invalidate_old_origins_at_admission`。既存request IDと受付時失効を検査 |
| 8 LogicOnlyの非表示採用 | `actual_logic_only_active_book_and_media_wait_for_main_full`、parked/sibling回帰。items/read-model/dirty/frame不変と終端・移譲を区別 |
| 9 UI・早期return・native同dispatch | 上記main Full、`loading_and_holdover_shell_escape_and_close_use_actual_child_ui`（loading/holdover × Escape/Closeの4条件）、`native_hard_close_flag_reaches_production_poll_before_teardown`、既存 `native_immediate_close_consumes_bookmark_return_in_same_dispatch` 1件 |

deltaの共通filter `discard_texture_delivery` はegui 4件、eframe wgpu 2件/glow 2件、egui-wgpu 2件を正式manifestで実行し、計10件全て成功（各exit 0）。AppのSubmissionPlugin回帰1件はcontinuationにも含まれる。

| delta受入 | 回帰名・正式選択件数 |
| --- | --- |
| 1 実discard→同ID再open→callback handle drop | egui-wgpu `offscreen_actual_retry_immediate_callback_drop_readback` 1件、eframe各 `native_callback_drop_gc_reopen_ime` 1件 |
| 2 先行whole/partialと後発freeの順 | 同offscreen 1件は実resource consumerを通す。eframe wgpu `production_scope_weak_pending_and_restore`、glow `resource_consumer_pending_unavailable_order` 各1件を併用 |
| 3 実upload/submission/readback/free | egui-wgpu上記1件。glowは実adapterとfake GL callsによるresource順序であり実GL表示の証拠ではない |
| 4 既存pending・利用不能・後発free | egui-wgpu `offscreen_unavailable_pending_later_free_not_cancelled` 1件、eframe上記pending回帰各1件 |
| 5 nested・複数discard・空delta | egui `nested_immediate_discard_uses_same_scope`、`plugins_ime_receipt_and_multiple_retries_once`、`final_limit_direct_and_empty_do_not_call_sink` 各1件 |
| 6 通常最終outputの所有 | egui `final_limit_direct_and_empty_do_not_call_sink` 1件。discardなし/max_passes/direct begin/endを含む |
| 7 plugin変更後・IME/receipt/ACK不変 | egui `plugins_ime_receipt_and_multiple_retries_once` 1件、App `egui037_continuation_submission_plugin_discard_texture_delivery_is_not_acceptance` 1件 |
| 8 Context/scope分離・復元・unwind | egui `context_scope_restore_early_return_and_unwind` 1件、eframe wgpu scope回帰1件 |
| 9 GC/退役/root終了・新寿命/兄弟 | eframe各 `native_callback_drop_gc_reopen_ime` 1件と既存 `viewport_retirement` 各12件、egui `viewport_retirement` 14件。追加delta2件はeframe退役filterにも含まれる |

### 第4.3版受入1〜16の最終接続対応（実装担当の内部確認）

新規Full接続の共通prefixは `egui037_continuation_real_`、所在は `src/app/egui037_connection_tests.rs`。
`egui037_continuation` 全体は旧36件、新規Full接続10件、製品SubmissionPlugin 1件の47件。実選択結果・fixture補修の履歴は後続表へ記録する。
headlessのminimized metadata、fake desktop/native effect、offscreen資源試験を、実OSの最小化・表示・実IMEの成功へ拡大しない。

| 受入 | 回帰名・選択集合 / 接続 |
| --- | --- |
| 1 最小化Full・旧順序 | `minimized_full_preserves_checkpoints_and_owner_adoption` 1件でC2 completion/C3 metadata/C4 inpaint/C5 material/D3 Equalを同一製品Fullで採用。両backend `minimized_full` wgpu9/glow11件を併用 |
| 2 同frame複数LogicOnly | 同新規1件、既存 `remote_service_acquire_ack_and_return_do_not_consume_display`、save/AI/context-state/gamepadのcontinuation。App退役 `remote_logic_waits_for_command_delivery_and_existing_save_ack` |
| 3 対象Fullで一回採用 | `full_discard_and_sibling_keep_result_owner_exactly_once`、旧順序回帰。既存Remote slot、AI live backlog。receiver切断・texture identity・兄弟不変で検査 |
| 4 保存/resume/共有ACK | `sent_save_terminal_unblocks_remote_without_editor_ui`、`remote_resume_response_uses_receiver_without_ui`、`remote_shared_bookmark_router_consumes_each_reply_once`、App退役のsave ACK回帰 |
| 5 返却→再取得→遅いFull | `remote_return_reacquire_late_ui_discards_old_slot`、`remote_old_return_identity_cannot_reload_after_later_return`、第4.6版Close Remote回帰 |
| 6 C3 live保留・取消・寿命終端 | `egui037_continuation_ai_*` 5件、`app_drain_lease_drop_terminals_every_representative_resource_before_one_ack`、App退役/Closeのowner限定回帰 |
| 7 C4新pointer | `full_inpaint_resumes_view_trim_with_new_pointer_level` 1件、old-up/new-downとold-down/new-upの両方向で製品再開経路を検査 |
| 8 C5表示予算/runtime終端 | `minimized_full_retains_material_upload_budget_through_logic`、`full_samples_decision_preview_and_physical_input_only_in_ui`、`full_pdf_promotion_retry_uses_ui_frame_budget`、`full_summary_budget_waits_for_next_ui_frame` 各1件、既存drain/lease終端回帰 |
| 9 D3 cutoff/cause/restart/ID | `full_equal_rescan_acknowledges_cutoff_and_retains_restart_cause`、`full_different_rescan_installs_and_allocates_hydration_only_in_ui` 各1件、既存 `hydration_*` 3件（context mount/cancel-restart/IndexOnly/枯渇） |
| 10 非描画output・capture・ACK | backend `minimized_full` 20件中のoutput/action/immediate入口、App SubmissionPlugin1件、delta追加10件、`texture_output_` 2件 |
| 11 root/child交互・skip・退役 | backend `minimized_full_real_integration_keeps_root_child_input_outputs_and_logic_only_boundary` 各feature、`viewport_input` 各5件、App退役10件、Close sibling回帰 |
| 12 非描画root Closeとbackend判定 | `root_lifecycle_actual_app_logic_reaches_backend_close_decision_without_ui`、`root_lifecycle_actual_app_save_ack_precedes_resumed_backend_close`、ZIP/settings/trayの既存回帰。root filter11件 |
| 13 modal拒否・一回延期/優先 | `root_lifecycle_actual_app_modal_rejection_consumes_only_one_delivery`、既存modal/save/tray/explicit quit回帰。root filter11件 |
| 14 B receipt/ratingを非描画で更新 | `context_state_real_logic_receipt_keeps_sibling_ui_unadopted`、`context_state_real_logic_preserves_unadopted_owner_results`、`context_state_save_receipt_preserves_both_unadopted_views`。context_state filter8件 |
| 15 正当UIでrating/page-edit採用 | `context_state_retains_rating_and_page_edit_until_ui_boundary`、実logic receiptの同UI復帰検査。context_state filter8件 |
| 16 immediate子独自描画・最小化capture | 両backend `minimized_full_immediate_real_entry_preserves_child_output_and_paint_ownership` 各1件（親min/子visible、親子min、子screenshot）。親子paint/UI/callbackを別々に検査 |

### 段3必須回帰と第4.5版の再確認（実装担当の内部確認）

段3「実接続点の必須回帰」の順番に対応する。正式manifestは `vendor/*`、IMEは一本化済みの `crates/egui-ime` を使う。

| 必須回帰 | 正式回帰 / 選択数 |
| --- | --- |
| delta描画/無描画/早期終了/headless | `texture_output_` 2、backend `minimized_full` wgpu9/glow11、追加delta10。全test fixtureの明示破棄は全target compileと監査を併用 |
| root/child交互とskip後の終端 | backend `viewport_input` 各5、`minimized_full_real_integration_keeps_root_child_input_outputs_and_logic_only_boundary`、App退役10 |
| 最小化passive Close | backend `viewport_input`、App managed hook/Close parked-sibling/standalone回帰。非UI callback0を検査 |
| active/parkedの終了委譲 | App `viewport_retirement_mode_change_transfers_active_parked_and_passive_exact_lifetimes`、第4.6版Close12、collection slideshow cancel3 |
| modal拒否の消費/新Close受付 | App `viewport_retirement_managed_hook_consumes_modal_reject_then_accepts_new_close`、eframe `close_request_separation` 各1 |
| OS/programmatic/bar/watcher入口 | App hook/retirementとClose loading/holdover/native hard-closeの実consumer、既存入口棚卸し。Frame OS Close二重読取りは除去済み。実OS操作は段7 |
| root LogicOnly退役/兄弟維持 | egui退役14、eframe退役各12、App退役10、Close12 |
| gesture/media/worker終端・非再作成 | 同退役集合、Close/native/parked、wheel true-close scope回帰、collection slideshow cancel3。deltaは追加10 |
| Disabled producer→実TextEdit | egui-winit lib43中 `disabled_producer_*`、IME共有lib30 |
| 同Context親子の中断分離 | 正式egui `ime_interrupt` 6（parent/child/neither/both） |
| 実run_ui discardのIME矩形/中断 | 同6中 `ime_interrupt_discard_keeps_request_and_latest_text_edit_metadata`、delta plugin回帰 |
| native focus/cursor→intent/control/epoch | `native_ime_interrupt` 3（実TextEdit focus/cursor、visual置換/epoch退役、複数logical pass） |
| 集約中断の対象配送/他viewport分離 | native上記3、egui IME6、backend `minimized_full` output boundaryと`viewport_input` 各5 |
| native visual上書き/複数pass/host退役 | native上記3。新hostへ旧intentが漏れないことを検査 |
| focus前の受信済み離散キー | `keyboard_input::` 17、`synthetic_modifier` 1、egui-winit43中delivery/processedキー回帰 |
| gamepad同frame/短い退避/edge/disconnect/ring/Undo | `gamepad_` 94、うち実logic接続1件。物理とdelivery ownerを区別 |
| gamepad保持軸の復帰後継続 | 同94中 `gamepad_blocked_dispatch_keeps_held_axis_for_admitted_resume`、`rating_undo_grid_blocked_pad_and_ring_moves_keep_held_restore` 等。保持値を消さず既存tickで継続 |
| WH_GETMESSAGE既存drain | `getmessage` 1 |

第4.5版13項目は前掲の対応表を維持し、最終結果に次の補足を適用する。
egui退役14、eframe退役wgpu12/glow12、App退役10は全て正式graphで選択して成功した。
項目9の表示継続P1は上記Close帰還12件＋native同dispatch1件で、項目11のdiscard順P1は上記delta10件＋製品SubmissionPlugin1件で補完する。
項目10ではbackend配送前のFinishAcquire禁止と配送後の既存service/readinessを実App回帰で検査し、Close帰還UI待ちは加えていない。
項目13はroot lifecycle11件とbackend root終了/重複退役回帰を併用し、tray/save/CancelCloseの製品policyをvendorのfake Appに代えない。

| 第4.5版受入 | 正式回帰・選択数（最終対応） |
| --- | --- |
| 1 Full logic→pass外退役 | eframe `viewport_retirement_{wgpu,glow}_full_logic_gc_discard` 各1。退役filterは各12成功 |
| 2 LogicOnlyのみの退役 | eframe `logic_only` 各1、egui `logic_transfer_exact_descendants_and_no_phantom_registration` 1、App退役10、Close実logic回帰 |
| 3 GC前の承認保持 | eframe `context_gc_native_descendants_ime` 各1、egui `gc_discard_keeps_authority_in_root_until_outer_return` 1、App `context_gc_preserves_app_binding_authority` 1 |
| 4 immediate入れ子・最外側配送 | eframe `immediate_nested` 各1、egui `immediate_nested_pass_defers_transfer_and_cleanup` 1、追加delta nested/immediate回帰 |
| 5 discard再pass・二重終端/早いC1禁止 | eframe `full_logic_gc_discard` 各1、egui GC/discard 1、App `remote_full_and_discard_wait_for_outer_delivery` 1 |
| 6 exact子孫・兄弟・部分GC | eframe `context_gc_native_descendants_ime` 各1、egui退役14、App mode-change/parking、Close sibling、collection取消3 |
| 7 旧output/append/同ID再open | eframe `old_output_reopen_duplicate`/`discard_immediate_reopen_replaces_native` 各backend各1、egui `old_output_append_and_reopen_never_mix_lifetimes` 1 |
| 8 modal消費・新Close・非管理 | eframe `close_request_separation`/`hook_captures_lifetime_before_reopen` 各backend各1、`child_close` wgpu3/glow4、App managed hook 1 |
| 9 全終了入口・parking存続 | App退役10中mode-change/managed hook/parking/重複/未登録、Close12＋native同dispatch1。bar/watcher→既存判断のsource棚卸しとlib既存回帰も併用 |
| 10 C1配送完了とreadiness | App `remote_logic_waits_for_command_delivery_and_existing_save_ack`、`remote_standalone_fullscreen_keeps_exact_authority_after_context_gc`、`remote_full_and_discard_wait_for_outer_delivery`、dirty保持、continuationのsave/resume/drain、Close Remote回帰 |
| 11 whole/partial/freeとdrop追加分 | eframe `delta_callback_drop` 各1、egui unlocked callback/drop free、追加delta10＋製品SubmissionPlugin1。実GPU readbackとsubmit→freeを検査 |
| 12 通常GC/重複承認/一回終端 | eframe `overlapping_gc_root_close` 各1、egui overlapping/direct-end、App `repeat_terminal_owner_transfers_once` |
| 13 root同時終了/tray/save/CancelClose | eframe `overlapping_gc_root_close` 各1、root_lifecycle11（製品→実EpiIntegration 3を含む）、backend `minimized_full` wgpu9/glow11中root LogicOnly判定 |

### 追加回帰の初回失敗と限定レビュー

- 新規Full接続の初回はcontinuation47選択/41成功/6失敗。共有headless fixtureにはCreationContextが無く製品begin-pass trackingが未登録だった3件、embedded初期化前のC4 fingerprintとcanvas click混入、初回sizing/discardを通常frame予算と混同したmaterial/summary fixtureを補修した。統合fixtureは無関係なload-folder workerを除き、既存page-index ownerでinstalled baselineを準備する。採用数・予算・pointer両方向の期待と製品poll経路を維持し、製品のgate・max_passesは変更しない。
- wheel初回19選択/18成功/1失敗はHome/End fixtureの誤前提だった。既存jump ownerはnonsequential landでありDisplay sequenceを開始しない。command直後のtarget 0/3、最終wheel ordinal 3、最終配送後Home→1/End→3を検査する。中間wheelが先にDisplayを作ってcommandを拒否する旧不具合はcommand直後のassertで引き続き検出する。
- 既存music overflow回帰1件でContext lock再入を検出した。`src/ui_music_timeline.rs::timeline_manual_scroll_requested` の `ui.input` 内に残った `ui.ctx().options` をclosure外へ移した。10秒debug lock診断であり、runner timeoutによる失敗ではない。関連sourceのinput closure内options取得を再走査し、残件0。追加delay・retryでは回避しない。
- collection slideshow取消の新fixtureに `Option<Box<ArchiveConvertState>>` のBox不足があり、最初のclose/bookmark filterはコンパイルで停止した。Box化後の実選択結果を以下に記録する。コンパイル失敗を0件成功とは扱わない。

Full接続fixture補修後は47選択/45成功/2失敗で、残りはO5の到達経路と予算の誤前提だった。観測付き2件の実行により、`render_update_before_frame` → `poll_mounted_document_open_owners` → `ContextAsyncOwner::ALL` がC4/C5も採用することを確定した。`needs_mounted_service` はpendingのEraseInpaint/LocalAdjustmentを含み、開始HEAD `5c2fd0367` にも同じ経路があり、git blameは `2ff4fe19d1`（2026-09-28）まで遡る。

段1b台帳のO5をC3のみ、C4をF16のみとした到達記録は不完全だった。製品traceは、同じpass内のcounter Nでearly local1件、F8増分後のcounter N+1でmedia local1件を示し、同じcounterで2件採用していない。設計書C4はUI/新pass pointerを要求するがF16専有とはしておらず、G2は既存 `frame_counter` 比較を正本とする。別contextのSol / xhighも独立に履歴・実入口・設計文を照合し、この判断で一致した。製品pollの除去・後置やcounter移動は行わない。

したがって新fixtureの「外側Fullごとに1件」と「C4はmedia入口まで未採用」という未確認前提を訂正する。統合回帰はbefore_frameの未採用→update_beginのearly採用→mediaの再採用なしを固定する。予算回帰はready3件でNの1件→N+1の2件目→同N+1のLogicOnly保持/次Full early抑止→N+2の3件目を検査する。単に上限を2へ変える検査ではない。新しい厳密な1 UI pass/1 Full上限を設けるなら別の仕様変更であり、この移行には混ぜない。初回レビューでこの間接pollを見落としたことも履歴として残す。

別contextのGPT-6.1 Sol / xhighによる限定source reviewは、Close帰還、delta実装・回帰、Full接続fixture、wheel調停・Close scope限定、上記Home/End訂正とmusic hoistをそれぞれACCEPTとした。
途中で指摘されたloading/holdover/native hard-closeのcapture漏れ、Live Close後のslideshow、共有archive取消の兄弟混入は修正し、対象回帰を追加した。
これらはソースの限定レビューであり、正式コマンドの成功や段3全体の設計担当による受入と区別する。内部の実行・分類は**実装担当の内部確認**である。

### 最終コマンド結果（実装担当の内部確認）

この表の後に、最後のlib全件実行と、そこで判明した移行不備の限定修正・再検証を追記する。全件実行は1回だけとし、修正後に全件を再実行したとは扱わない。

`target/egui037-stage3-v46-<label>.json/.log` に引数・exit code・秒数と全出力を保存した。
全Cargo graphはoffline、jobs=1、MSBuild node reuse無効。新しいcrateの取得は不要だった。
前掲の初回・途中失敗を削除せず、以下は最終または影響範囲が不変で再利用できる実行結果を示す。
`--no-run` はコンパイル成立の証拠であり、0件テスト成功とは扱わない。egui-imeのdoc 0件もlib30件と区別する。
`test-focused.ps1` のexit 2はbroad/shared変更による未選択で、成功ではない。要求されたlib全件と個別vendor gateを別々に実行する。

| label | コマンド | 選択・結果 | exit / 秒 |
| --- | --- | --- | --- |
| `core-handoff` | `cargo check -p mimageviewer --bin mimageviewer-core --offline` | check / inspect | 0 / 20.6 |
| `test-script-handoff` | `cargo check -p mimageviewer --bin mimageviewer-core --features test-script --offline` | check / inspect | 0 / 19.92 |
| `workspace-handoff` | `cargo check --workspace --exclude mimageviewer-launcher --offline` | check / inspect | 0 / 29.9 |
| `workspace-all-targets-final-source` | `cargo check --workspace --all-targets --exclude mimageviewer-launcher --offline` | check / inspect | 0 / 30.21 |
| `tests-no-run-handoff` | `cargo test -p mimageviewer --tests --no-run --offline` | 23 executables, no run | 0 / 317.89 |
| `lib-no-run-handoff` | `cargo test -p mimageviewer --lib --no-run --offline` | 1 executables, no run | 0 / 0.69 |
| `snapshot-no-run-handoff` | `cargo test -p mimageviewer --test ui_snapshot --test book_composition_editor_snapshots --no-run --offline` | 2 executables, no run | 0 / 0.74 |
| `ime-interrupt` | `cargo test -p egui --lib ime_interrupt --offline` | 6 passed, 0 failed, 0 ignored | 0 / 6.47 |
| `egui-ime` | `cargo test -p egui-ime --offline` | 30 passed, 0 failed, 0 ignored / 0 passed, 0 failed, 0 ignored | 0 / 7.03 |
| `egui-winit` | `cargo test --manifest-path vendor/egui-winit/Cargo.toml --lib --offline` | 43 passed, 0 failed, 0 ignored | 0 / 13.36 |
| `continuation-handoff` | `cargo test -p mimageviewer --lib egui037_continuation --offline` | 47 passed, 0 failed, 0 ignored | 0 / 10.2 |
| `root_lifecycle` | `cargo test -p mimageviewer --lib root_lifecycle --offline` | 11 passed, 0 failed, 0 ignored | 0 / 5.9 |
| `context_state` | `cargo test -p mimageviewer --lib context_state --offline` | 8 passed, 0 failed, 0 ignored | 0 / 2.62 |
| `minimized_full-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib minimized_full --no-default-features --features default_fonts,wgpu --offline` | 9 passed, 0 failed, 0 ignored | 0 / 0.32 |
| `minimized_full-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib minimized_full --no-default-features --features default_fonts,glow --offline` | 11 passed, 0 failed, 0 ignored | 0 / 0.48 |
| `viewport_input-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_input --no-default-features --features default_fonts,wgpu --offline` | 5 passed, 0 failed, 0 ignored | 0 / 1.05 |
| `viewport_input-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_input --no-default-features --features default_fonts,glow --offline` | 5 passed, 0 failed, 0 ignored | 0 / 0.46 |
| `child-close-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib child_close --no-default-features --features default_fonts,wgpu --offline` | 3 passed, 0 failed, 0 ignored | 0 / 0.78 |
| `child-close-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib child_close --no-default-features --features default_fonts,glow --offline` | 4 passed, 0 failed, 0 ignored | 0 / 0.29 |
| `egui-retirement` | `cargo test --manifest-path vendor/egui/Cargo.toml --lib viewport_retirement --offline` | 14 passed, 0 failed, 0 ignored | 0 / 0.12 |
| `viewport_retirement-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_retirement --no-default-features --features default_fonts,wgpu --offline` | 12 passed, 0 failed, 0 ignored | 0 / 1.35 |
| `viewport_retirement-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_retirement --no-default-features --features default_fonts,glow --offline` | 12 passed, 0 failed, 0 ignored | 0 / 0.5 |
| `viewport_retirement` | `cargo test -p mimageviewer --lib viewport_retirement --offline` | 10 passed, 0 failed, 0 ignored | 0 / 3.17 |
| `close-final` | `cargo test -p mimageviewer --lib close_return --offline` | 12 passed, 0 failed, 0 ignored | 0 / 6.03 |
| `bookmark-final` | `cargo test -p mimageviewer --lib bookmark_return --offline` | 5 passed, 0 failed, 0 ignored | 0 / 1.73 |
| `native_immediate_close_consumes_bookmark_return_in_same_dispatch` | `cargo test -p mimageviewer --lib native_immediate_close_consumes_bookmark_return_in_same_dispatch --offline` | 1 passed, 0 failed, 0 ignored | 0 / 268.56 |
| `delta-egui-final` | `cargo test --manifest-path vendor/egui/Cargo.toml --lib discard_texture_delivery --offline` | 4 passed, 0 failed, 0 ignored | 0 / 10.84 |
| `delta-eframe-wgpu-final` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib discard_texture_delivery --no-default-features --features default_fonts,wgpu --offline` | 2 passed, 0 failed, 0 ignored | 0 / 7.84 |
| `delta-eframe-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib discard_texture_delivery --no-default-features --features default_fonts,glow --offline` | 2 passed, 0 failed, 0 ignored | 0 / 14.61 |
| `delta-offscreen` | `cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --lib discard_texture_delivery --features winit --offline` | 2 passed, 0 failed, 0 ignored | 0 / 56.3 |
| `wheel-final` | `cargo test -p mimageviewer --lib egui037_wheel_frame_ --offline` | 19 passed, 0 failed, 0 ignored | 0 / 7.91 |
| `collection_slideshow_cancel_` | `cargo test -p mimageviewer --lib collection_slideshow_cancel_ --offline` | 3 passed, 0 failed, 0 ignored | 0 / 1.84 |
| `wheel_input--` | `cargo test -p mimageviewer --lib wheel_input:: --offline` | 8 passed, 0 failed, 0 ignored | 0 / 1.06 |
| `page_turn_actual_wheel` | `cargo test -p mimageviewer --lib page_turn_actual_wheel --offline` | 1 passed, 0 failed, 0 ignored | 0 / 1.12 |
| `mixed_key_and_wheel_batch` | `cargo test -p mimageviewer --lib mixed_key_and_wheel_batch --offline` | 1 passed, 0 failed, 0 ignored | 0 / 1.18 |
| `music_ctrl_wheel` | `cargo test -p mimageviewer --lib music_ctrl_wheel --offline` | 3 passed, 0 failed, 0 ignored | 0 / 1.47 |
| `music-overflow-final` | `cargo test -p mimageviewer --lib music_overflow_real_input_wheel --offline` | 1 passed, 0 failed, 0 ignored | 0 / 1.6 |
| `gamepad_` | `cargo test -p mimageviewer --lib gamepad_ --offline` | 94 passed, 0 failed, 0 ignored | 0 / 31.85 |
| `native_ime_interrupt` | `cargo test -p mimageviewer --lib native_ime_interrupt --offline` | 3 passed, 0 failed, 0 ignored | 0 / 0.94 |
| `ime_focus--` | `cargo test -p mimageviewer --lib ime_focus:: --offline` | 12 passed, 0 failed, 0 ignored | 0 / 1.24 |
| `texture_output_` | `cargo test -p mimageviewer --lib texture_output_ --offline` | 2 passed, 0 failed, 0 ignored | 0 / 0.83 |
| `keyboard_input--` | `cargo test -p mimageviewer --lib keyboard_input:: --offline` | 17 passed, 0 failed, 0 ignored | 0 / 0.8 |
| `synthetic_modifier` | `cargo test -p mimageviewer --lib synthetic_modifier --offline` | 1 passed, 0 failed, 0 ignored | 0 / 0.87 |
| `getmessage` | `cargo test -p mimageviewer --lib getmessage --offline` | 1 passed, 0 failed, 0 ignored | 0 / 0.88 |
| `tree-handoff` | `cargo tree -d --offline` | check / inspect | 0 / 0.52 |
| `focused-handoff` | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-focused.ps1` | check / inspect | 2 / 0.61 |
| `fmt-handoff` | `cargo fmt --check` | check / inspect | 0 / 14.51 |
| `diff-handoff` | `git diff --check` | check / inspect | 0 / 0.27 |
| `glyphs-handoff` | `python scripts/check_ui_glyphs.py` | check / inspect | 0 / 2.18 |
| `vendor-fmt` | `rustfmt --edition 2024 --check --config skip_children=true vendor/egui/src/context.rs vendor/egui/src/discard_texture_delivery_tests.rs vendor/eframe/src/native/viewport_lifecycle.rs vendor/eframe/src/native/wgpu_integration.rs vendor/eframe/src/native/glow_integration.rs vendor/egui-wgpu/src/winit_tests.rs` | check / inspect | 0 / 0.21 |

### 最後のlib全件実行と、その後の限定補修（実装担当の内部確認）

`scripts/test-lib-full.ps1 -Force` は最後に1回だけ実行した。exit **100**、495.55秒。
総数 **13,481**、実行 **13,422**、成功 **13,132**、失敗 **290**、既存skip **59**。
run directoryは `target/test-lib-full/runs/20261017T034945177Z-04c522be`、
source hashは `204cffbfed6089ac013bbc28fa0fb997c963e513ca78c52aab452be183d56d61`。
この全件実行は後述の補修前の証拠であり、補修後の全件成功とは扱わない。
全件は再実行せず、失敗の一次原因と変更した契約に絞って検証する。

初期分類はPNG不一致96件、missing glyph88件、未完結delta26件、その他80件だった。
delta26件のうち4件は先行assertの失敗後のdrop panicであり、delta処理だけを直して成功扱いにはしない。
PNG96件を除いた**194件**をexact nextest filterで再実行した最初の限定結果は、
**148成功 / 46失敗、exit 100、396.70秒**。
その実行後に保存した修正（ScrollArea state reader、native panelのsizing、metadataのcold SetTheme等）は、この結果に含めない。

補修分野は次のとおり。元のPNG・操作のassert・欠落glyph検査・既存ignoreは変更しない。

| 分野 | 主なファイル・判断 |
| --- | --- |
| headless deltaの消費 | `src/app/paint_record_test_support.rs`、`sidecar_restore.rs`、`similar_navigation_tests.rs`等。描画しないfixture出力は既存の明示破棄ownerへ接続。font atlasの期待boolは保持 |
| App / root Ui / viewport fixture | `src/app/tests.rs`。kittestが渡す実Ui原点にクリックを相対化。immediate子のmeshは親shapesではなく実child出力で検査。実registrationとbindingの同一lifetimeをroot退役commandまで照合 |
| IME / Modifiers / test-script | `src/video/native_presenter/render_core.rs`、`src/global_search_ui/everything/shortcut.rs`、`src/keymap.rs`、`src/test_script/capture.rs`。modifier通知の順序を含む正確な列、実plugin pass、viewport所有metadataへ移行 |
| テストの日本語フォント | UI fixtureの対象builderに既存`ui_fonts::configure_fonts`を接続。共有GPU builder、製品のfont chain、missing glyph検査は変更しない |
| 一般API / ID / accessibility | Window、UiBuilder、ScrollAreaの実ID生成に合わせてreaderを接続。製品ではmusic backing UIとfile organizeのkeyboard revealが対象。実child/ScrollAreaOutput IDの照合を保持。表示名のないfixture ComboBoxには非視覚のaccessibility名を付与 |
| 旧mouse dragの保持 | `src/ui_dialogs/mod.rs`、`src/external_tool.rs`、`src/video/native_presenter/overlay_draw.rs`。旧default `ScrollSource::ALL / drag: true`と個別無効指定なしを確認した本文だけ`DragScroll::Always`を明示。新しい入力機構は作らない |
| cold passの測定条件 | `src/ui_metadata_panel.rs`とnative batch fixture。固定の初回SetTheme/Area sizingを完了し、測定対象の要求・論理pass・present集計へ混ぜない。実panel layer、100ms非zero要求、terminal無要求の期待を保持 |

製品バイナリのbuild/launch、liveテスト、`test-full.ps1`、PNG baseline更新は依頼の禁止どおり実行していない。

#### Panel寸法予算とfixtureの検出力

下部選択情報バーは単なる表示差ではなく、製品の「グリッドより先に高さを確保する」assertが発火した。
原因は旧58pt（header26＋row28＋frame4）の外寸計算に、0.37 Panelがseparator用に加える外余白1ptが入っていなかったこと。
`selection_info_bar_frame(style)`と`selection_info_bar_height(style)`を同じ既存ownerに置き、実Frameの余白・separator・header・row・solid横バーから予算を計算する。
Panel、detailsの予測、回帰は同helperとroot Uiのstyleを使う。線を消す、余白を削る、assertを緩める変更は行わない。

formal egui rlibによる小さなheadless診断（製品ではない）で、次を確認した。
`target/egui037-stage3-v46-panel-probe.rs/.log`、exit0。正式App回帰とは別の原因確認である。

| overflow / outer予算 | 480pt中の実予約 | Panel response | 結果 |
| --- | --- | --- | --- |
| false / 68 | 68 | y412〜480 | 既存正常分岐 |
| true / 68 | 67 | y413〜481（height68） | 内容が1pt外へ拡張してからclampされ、予約とresponseが不一致 |
| true / 69 | 69 | y411〜480 | 実内容とframe/separatorを収め、予約とresponseが一致 |

`details_header_right_click_tests`の負例も旧固定座標が実header外へ出ていたため補修した。
CentralPanelの実Ui原点と実paintのheader矩形・clipを観測し、その可視中心をraw pointerでクリックする。
可視header内・選択行外の前提assertと行の正例を保持し、製品guardは変更しない。
独立source確認はP2解消（Cargo・mutation実行の証拠とは区別）。

FullNodeIdのreaderは、実`accesskit_consumer 0.39.1`の`Node::id()`と
`u128 = (local_id << 64) | tree_index`を確認し、上位64bitを使う。
途中の下位64bit案は誤りであり、first_setup/settings_restore/whats_newを実producerと一致させた。
GlobalSearchのrepeat期待は、releaseを送っていないfixtureの初回false・以後trueを正確に区別する。
その補修中のindex束縛漏れによる`postfull-recheck`は**compile失敗 exit101 / 109.64秒、テスト実行0**として記録し、古いJUnitを成功結果に流用しない。

#### 322件の再検証とClose帰還の初期Preparing補修

`postfull-recheck2`は**322選択 / 300成功 / 22失敗、exit100 / 340.56秒**。
当初194件に必須契約・変更したfixtureの正例/負例を追加した限定集合であり、lib全件の再実行ではない。
Fullのcontinuation47、wheel19、root lifecycle/context-state、実header内の右クリック2件、Panelの予約/予測はこの集合で成功した。

残った初期media resolver取消の回帰は、fixtureではなく段3の契約漏れだった。
`start_detached_bookmark_loading_context`はchildにpending requestとDetached bookmark targetを導入し、
取消もexact leaseの場合にそのtargetをClose用に保持する。一方、まだ通常contentを提示していないPreparingではitems/selected/fullscreen_idxが空なので、
selected mediaとの一致だけを要求するsummaryはその所有値を落としていた。

teardown前の共通transferで、既存のtyped Preparing session、projected window、window/context bindingが同じownerを指す場合だけ、
この初期Detached targetを捕捉する。一般のContextRef reader、Ready時の実media照合、main origin/requestの失効判断は緩めない。
新しいbool・queue・ID・寿命延長はない。
`close_return_unpresented_media_fallback_requires_exact_preparing_session`を追加し、Readyの空状態と別contextのPreparingでは旧targetを採用しないことも検査する。
前掲のClose12件は途中結果であり、この1件追加後の最終filterは13件として後続表に記録する。
独立source確認はP1/P2なし。正式実行とは区別する。

clipboardの3件はpointer traceで原因を確認した。footerは初期Resize中にy465→502→539→576→613→626へ動き、
従来fixtureが取得したy588.5のクリックは同じwindow layerのまま実ボタン外へ出ていた。最終footerはy651までで700pt viewport内に収まり、
WindowDrag競合や保存workerの失敗ではない。
fixtureだけで通常UI stepによる実ボタン矩形の連続安定（最大16step内、2連続比較）とviewport内包含をassertしてから、
元のpointer入力を送る。入力再送・AccessKitへの置換・追加repaint・待機・製品layout/viewport変更は行わない。
保存phase、同release＋Escape、3種類のClose、実保存内容のassertは保持。一時観測hookは診断後に全除去した。

残るHEX欄の同batch focus移動＋Textは、実Id/rect一致とクリック包含を確認しても、
新fieldがfocus/gained=true・changed=falseとなり、Textが未処理で残ることを実測した。
0.33のinteraction→focus→eventsに対し、0.37はAtom measure内events→allocate/interact→pointer focusの順である。
旧期待を維持したまま、段4の上流挙動差へ分類する。既にfocusが確立したownerのキー/IME commitの段3接続とは区別する。
局所guard、入力延期、追加focus/repaint、広域vendor変更は加えず、一時traceは除去した。

#### 最終の契約検証と全件失敗の分類

`postfull-contract-final`は**128選択 / 127成功 / 1失敗、exit100 / 221.53秒**。
初期Preparing Close、否定2枝を含むClose13件、clipboardの元pointer3件、保存/Remote/Full接続、wheelなどが成功した。
残る1件はmetadataのterminal repaint測定だった。

empty passだけのwarmupでは、初めて描くseparator自身のwidget style初期化要求を除外できなかった。
formal eguiのheadless小診断でlabelsだけなら要求なし、separator追加時だけ要求ありを再現し、
次beginで保存されたcauseを読み、**`vendor/egui/src/theme/mod.rs:37`（初回widget styleの応答未登録）**と確定した。
`repaint_on_widget_change`はfalseであり、widget比較が原因という途中仮説は採用しない。
同じ実terminal drawを固定2pass完了してから、probeと要求記録をresetして同じdrawを測定する。
製品の要求を抑制せず、spinnerなし・理由表示・要求なしの元assertを保持する。
最終の`book_query_limit_terminals_draw_reason_without_spinner_or_repaint`は**1選択 / 1成功 / exit0 / 144.35秒**（compile込み）。

全件の失敗290件について、保存したJUnitと上記限定実行を突き合わせた最終分類は次のとおり。
集計は**失敗test数**であり、PNGファイル数ではない。全件を最終ソースで再実行したという意味でもない。

| 分類 | 件数 | 扱い |
| --- | ---: | --- |
| 段3移行不備 | 179 | 所有/入力/API/fixtureの原因を修正し、該当testの限定再実行で成功を確認 |
| PNG比較由来 | 102 | 初回96件と、機械的前提の補修後にPNG比較まで到達した6件。baselineは更新せず段4b比較・段5更新へ |
| 0.37のlayout/挙動差 | 9 | 次表。元assertを弱めず、ignoreも加えず段4へ |
| 未分類・残る段3契約違反 | 0 | 初期Preparing targetの取りこぼしも補修済み |

PNGまで新たに到達した6件は、local adjust popup 1、preferences seek-strip height 2、folder tree sort header 1、folder-bar organize destinations 2。
全290件の集計根拠は`target/egui037-stage3-v46-final-classification.json`、元JUnitは前記full run、
限定JUnitは`postfull-194`・`postfull-recheck2`・`postfull-contract-final`、最後のmetadata結果は個別Cargo logへ保存した。

| 段4へ残すtest | 観測した差 |
| --- | --- |
| `ui_adjustment_panel::adjust_scope_selector_tests::adjust_scope_action_labels_fit_258px_body` | buttonの自然高25pt、旧期待24pt以下 |
| `ui_fullscreen::tests::fullscreen_overflow_panel_snapshot_root_tall_without_scrollbar` | content384pt / viewport376pt、余分な縦scroll8pt |
| `ui_dialogs::whats_new::tests::version_highlights_initial_width_keeps_previous_reading_area` | scale2の初期外寸537pt / intended546pt。外寸APIの二重margin控除は修正済みで、残りはconstrain/min-size比較 |
| `ui_main::section207_tests::section207_reported_controls_keep_left_input_right_order` | zoom時のaddress input最小幅 |
| `ui_main::section207_tests::section207_snapshot_label_keeps_input_minimum_order_and_panel_bounds` | width360 / zoom1.5のinput幅・bounds |
| `ui_main::section207_tests::section207_default_folder_geometry_matches_the_separate_panels` | 統合Panelと旧別Panelのy差1pt |
| `ui_text::tests::bubble_body_stays_within_panel_width_with_fill_on_or_off` | 268pt枠に対して自然幅268.21875pt |
| `ui_main::toolbar_wrap_tests::a_combo_stays_inside_the_panel_only_when_we_reserve_its_place` | 新WidgetAtomは裸Comboもwrapし、旧negative controlのoverflowが0になった |
| `ui_dialogs::preferences::name_colors::tests::hex_focus_transfer_keeps_the_new_editors_partial_input` | 同batchのpointer focus移動とTextの処理順差。前節の実測どおり |

Panelの製品debug assertはこの9件へ含めず修正・正式回帰成功を確認した。
Id系や残るRust非推奨警告の整理は設計の段5へ残し、IME互換producerを警告削減のために削除しない。

### 第4.6版完了時のコマンド台帳（実装担当の内部確認）

以下は本体最終ソースで再実行したgateと、変更範囲が不変の正式vendor graphで再利用する結果。
本体の全23 test executable、lib、snapshot2 targetのコンパイルを確認した。
`egui037_continuation`47、`root_lifecycle`11、`context_state`8、wheel19、Close13はすべて非0選択・全成功。
段3必須・第4.3版16項目・第4.5版13項目・第4.6版Close/deltaの対応は前掲の各表を参照し、Closeの最終選択数は13へ読み替える。

`cargo check --workspace`はrelease入力3 exeを要求するlauncherを除き、`--all-targets`も同じ除外で実施する。
これは今回の利用者指定の代替であり、製品バイナリを作って前提を埋めない。
`cargo test -p egui-winit --lib`はworkspace外packageのdev-dependencyというCargo制約で未選択（前掲exit101）なので、
正式vendor manifestによる同crate lib43件を用いる。`scripts/test-full.ps1`の検証コマンドもこのmanifestと新featureに合わせてある。
`test-focused.ps1`のexit2はbroad/shared変更による**0件選択**で、成功ではない。
指定されたlib全件と個別vendor gateを実施し、禁止された`test-full.ps1`へは進まない。

| label | コマンド | 選択・結果 | exit / 秒 |
| --- | --- | --- | --- |
| `final-metadata-terminal` | `cargo test -p mimageviewer --lib book_query_limit_terminals_draw_reason_without_spinner_or_repaint --offline` | 1 passed, 0 failed, 0 ignored | 0 / 144.35 |
| `final-core-handoff` | `cargo check -p mimageviewer --bin mimageviewer-core --offline` | check / inspect | 0 / 23.03 |
| `final-test-script-handoff` | `cargo check -p mimageviewer --bin mimageviewer-core --features test-script --offline` | check / inspect | 0 / 21.86 |
| `final-workspace-handoff` | `cargo check --workspace --exclude mimageviewer-launcher --offline` | check / inspect | 0 / 22.17 |
| `final-workspace-all-targets-final-source` | `cargo check --workspace --all-targets --exclude mimageviewer-launcher --offline` | check / inspect | 0 / 115.14 |
| `final-tests-no-run-handoff` | `cargo test -p mimageviewer --tests --no-run --offline` | 23 executables, no run | 0 / 266.46 |
| `final-lib-no-run-handoff` | `cargo test -p mimageviewer --lib --no-run --offline` | 1 executables, no run | 0 / 0.71 |
| `final-snapshot-no-run-handoff` | `cargo test -p mimageviewer --test ui_snapshot --test book_composition_editor_snapshots --no-run --offline` | 2 executables, no run | 0 / 0.69 |
| `final-continuation-handoff` | `cargo test -p mimageviewer --lib egui037_continuation --offline` | 47 passed, 0 failed, 0 ignored | 0 / 12.32 |
| `final-root_lifecycle` | `cargo test -p mimageviewer --lib root_lifecycle --offline` | 11 passed, 0 failed, 0 ignored | 0 / 6.26 |
| `final-context_state` | `cargo test -p mimageviewer --lib context_state --offline` | 8 passed, 0 failed, 0 ignored | 0 / 2.1 |
| `final-wheel-final` | `cargo test -p mimageviewer --lib egui037_wheel_frame_ --offline` | 19 passed, 0 failed, 0 ignored | 0 / 6.79 |
| `final-close-final` | `cargo test -p mimageviewer --lib close_return --offline` | 13 passed, 0 failed, 0 ignored | 0 / 4.99 |
| `final-bookmark-final` | `cargo test -p mimageviewer --lib bookmark_return --offline` | 5 passed, 0 failed, 0 ignored | 0 / 1.46 |
| `final-native_immediate_close_consumes_bookmark_return_in_same_dispatch` | `cargo test -p mimageviewer --lib native_immediate_close_consumes_bookmark_return_in_same_dispatch --offline` | 1 passed, 0 failed, 0 ignored | 0 / 0.89 |
| `final-viewport_retirement` | `cargo test -p mimageviewer --lib viewport_retirement --offline` | 10 passed, 0 failed, 0 ignored | 0 / 2.56 |
| `final-collection_slideshow_cancel_` | `cargo test -p mimageviewer --lib collection_slideshow_cancel_ --offline` | 3 passed, 0 failed, 0 ignored | 0 / 1.28 |
| `final-music_ctrl_wheel` | `cargo test -p mimageviewer --lib music_ctrl_wheel --offline` | 3 passed, 0 failed, 0 ignored | 0 / 1.15 |
| `final-music-overflow-final` | `cargo test -p mimageviewer --lib music_overflow_real_input_wheel --offline` | 1 passed, 0 failed, 0 ignored | 0 / 1.31 |
| `final-tree-handoff` | `cargo tree -d --offline` | check / inspect | 0 / 0.48 |
| `final-focused-handoff` | `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-focused.ps1` | 0 selected (broad/shared), not success | 2 / 0.78 |
| `final-fmt-handoff` | `cargo fmt --check` | check / inspect | 0 / 14.61 |
| `final-diff-handoff` | `git diff --check` | check / inspect | 0 / 0.37 |
| `final-glyphs-handoff` | `python scripts/check_ui_glyphs.py` | check / inspect | 0 / 2.22 |
| `final-vendor-fmt` | `rustfmt --edition 2024 --check --config skip_children=true vendor/egui/src/context.rs vendor/egui/src/discard_texture_delivery_tests.rs vendor/eframe/src/native/viewport_lifecycle.rs vendor/eframe/src/native/wgpu_integration.rs vendor/eframe/src/native/glow_integration.rs vendor/egui-wgpu/src/winit_tests.rs` | check / inspect | 0 / 0.15 |
| `ime-interrupt` | `cargo test -p egui --lib ime_interrupt --offline` | 6 passed, 0 failed, 0 ignored | 0 / 6.47 |
| `egui-ime` | `cargo test -p egui-ime --offline` | 30 passed, 0 failed, 0 ignored / 0 passed, 0 failed, 0 ignored | 0 / 7.03 |
| `egui-winit` | `cargo test --manifest-path vendor/egui-winit/Cargo.toml --lib --offline` | 43 passed, 0 failed, 0 ignored | 0 / 13.36 |
| `egui-retirement` | `cargo test --manifest-path vendor/egui/Cargo.toml --lib viewport_retirement --offline` | 14 passed, 0 failed, 0 ignored | 0 / 0.12 |
| `minimized_full-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib minimized_full --no-default-features --features default_fonts,wgpu --offline` | 9 passed, 0 failed, 0 ignored | 0 / 0.32 |
| `minimized_full-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib minimized_full --no-default-features --features default_fonts,glow --offline` | 11 passed, 0 failed, 0 ignored | 0 / 0.48 |
| `viewport_input-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_input --no-default-features --features default_fonts,wgpu --offline` | 5 passed, 0 failed, 0 ignored | 0 / 1.05 |
| `viewport_input-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_input --no-default-features --features default_fonts,glow --offline` | 5 passed, 0 failed, 0 ignored | 0 / 0.46 |
| `child-close-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib child_close --no-default-features --features default_fonts,wgpu --offline` | 3 passed, 0 failed, 0 ignored | 0 / 0.78 |
| `child-close-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib child_close --no-default-features --features default_fonts,glow --offline` | 4 passed, 0 failed, 0 ignored | 0 / 0.29 |
| `viewport_retirement-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_retirement --no-default-features --features default_fonts,wgpu --offline` | 12 passed, 0 failed, 0 ignored | 0 / 1.35 |
| `viewport_retirement-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib viewport_retirement --no-default-features --features default_fonts,glow --offline` | 12 passed, 0 failed, 0 ignored | 0 / 0.5 |
| `delta-egui-final` | `cargo test --manifest-path vendor/egui/Cargo.toml --lib discard_texture_delivery --offline` | 4 passed, 0 failed, 0 ignored | 0 / 10.84 |
| `delta-eframe-wgpu-final` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib discard_texture_delivery --no-default-features --features default_fonts,wgpu --offline` | 2 passed, 0 failed, 0 ignored | 0 / 7.84 |
| `delta-eframe-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --lib discard_texture_delivery --no-default-features --features default_fonts,glow --offline` | 2 passed, 0 failed, 0 ignored | 0 / 14.61 |
| `delta-offscreen` | `cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --lib discard_texture_delivery --features winit --offline` | 2 passed, 0 failed, 0 ignored | 0 / 56.3 |
| `gamepad_` | `cargo test -p mimageviewer --lib gamepad_ --offline` | 94 passed, 0 failed, 0 ignored | 0 / 31.85 |
| `native_ime_interrupt` | `cargo test -p mimageviewer --lib native_ime_interrupt --offline` | 3 passed, 0 failed, 0 ignored | 0 / 0.94 |
| `ime_focus--` | `cargo test -p mimageviewer --lib ime_focus:: --offline` | 12 passed, 0 failed, 0 ignored | 0 / 1.24 |
| `texture_output_` | `cargo test -p mimageviewer --lib texture_output_ --offline` | 2 passed, 0 failed, 0 ignored | 0 / 0.83 |
| `keyboard_input--` | `cargo test -p mimageviewer --lib keyboard_input:: --offline` | 17 passed, 0 failed, 0 ignored | 0 / 0.8 |
| `synthetic_modifier` | `cargo test -p mimageviewer --lib synthetic_modifier --offline` | 1 passed, 0 failed, 0 ignored | 0 / 0.87 |
| `getmessage` | `cargo test -p mimageviewer --lib getmessage --offline` | 1 passed, 0 failed, 0 ignored | 0 / 0.88 |
| `wheel_input--` | `cargo test -p mimageviewer --lib wheel_input:: --offline` | 8 passed, 0 failed, 0 ignored | 0 / 1.06 |
| `page_turn_actual_wheel` | `cargo test -p mimageviewer --lib page_turn_actual_wheel --offline` | 1 passed, 0 failed, 0 ignored | 0 / 1.12 |
| `mixed_key_and_wheel_batch` | `cargo test -p mimageviewer --lib mixed_key_and_wheel_batch --offline` | 1 passed, 0 failed, 0 ignored | 0 / 1.18 |

最終`cargo tree -d`はegui/eframe/egui-winit/egui-wgpu/egui_kittestが0.37.0、wgpuが30.0.1で、正式vendorを指す。
旧egui/wgpuの混入・egui型分裂・`vendor-next` pathはない。独立したshader compiler用途のnaga27は旧wgpu混入とは扱わない。
`vendor-next/`は存在せず、原本provenance/import script/PATCHESは正式vendorに保持している。
PNG baseline差分0、index差分0、HEAD/branchの変更0を最終確認した。

設計上の未解決相違はない。O5の古い表・段1b台帳のC3限定という説明と実コードの粒度差は前掲の補足に記録し、
baselineから存在するearly ALL pollと実`App.frame_counter`の予算境界を維持した。
段4へは上記9件と外観比較、段5へはPNG baseline更新と残る警告整理、実機/live検証は計画の後段へ渡す。
live ignoreと`test-live-windows.ps1`は維持し、この作業ではlive・製品build/launch・`test-full.ps1`を実行していない。

## 独立レビュー指摘への対応（1e55235f1後、分類第2版）

以下は**実装担当の内部確認**。設計担当がコミットした`1e55235f1`を開始点とし、A〜Dの9指摘をsourceで確認した。誤指摘と判断したものはない。
第4.6版の契約内で補修し、設計変更・新しいpending状態・入力延期・追加repaint・retryを加えていない。
修正版と旧処理へ戻した変異を別ログへ保存し、変異は`finally`で元の修正版のbyte列へ戻した。
本文の第1版の9件・290件の分類と実行記録は履歴として残す。今回lib全件は再実行しない。

### 分野別の判断と修正

- **A / GPU出力・依存宣言:** `vendor/eframe/src/native/viewport_lifecycle.rs`の共通dispatcherがContext deltaを既存共有ownerへ回収し、非空batchだけlazy consumerへ渡す。wgpu / glowの通常イベントで空consumerを作らず、free-onlyは配送する。通常Fullのrenderer owner・submission順は保持。`PATCHES.md`へ追記した。`crates/vst3-host-tester/Cargo.toml`はworkspace MSRV 1.95を継承。`scripts/test-egui-msrv.py`は新しいcompilerでは検出できない宣言不整合を、GUI依存7 packageと正式vendorのMSRV比較で検査する。
- **B / Close帰還・gamepad:** `src/app.rs`のbookmark installは、既存の型付きterminal帰還ownerが持つ「存続するmainのlive grid」という意味を使い、一覧置換前のstable keyとscrollを保持する。`src/app/close_return.rs`のconsumerは採用済みrowsへ復元keysを合わせる。同型producerの削除receiptも、旧rowsを先にretainせず新rowsをinstallerへ渡す。`gamepad_input/physical.rs`では既存handoff内の隣接した同じSessionEndedだけを集約し、間のpress・release・axisを越えてまとめない。
- **C / 子入力・Window操作:** `src/ui_fullscreen.rs`のpass-local proposalへviewport identity、pointer、modifier、Responseのdrag/click事実、音楽入力許可を捕捉する。子のqueueは子のclaimで一度消費し、遅延したOwned処理はroot queueを再消費しない。連結・panoramaのgeometry/dragも捕捉値を使う。aboutの内蔵ScrollAreaは`DragScroll::Always`、Remote接続窓とtray案内窓は`WindowDrag::Anywhere`を明示した。
- **D / test fixture・論理座標:** `src/ui_main.rs`の対象2 fixtureは全viewportのroot Uiを明示し、kittestの外余白8ptを製品Panelの予算へ混ぜない。最小幅は論理定数、Panel / viewport / Node矩形も論理単位へ統一した。両integration targetは`ctx.content_rect()`で境界を検査し、共有`tests/support/logical_bounds.rs`へ画面外の実Nodeを拒否する負例を置いた。fullscreenは製品同様の全viewportを使う共有fixtureへまとめ、既存Fitsと4つのsnapshotを維持したままPNG不要の高さ回帰を追加した。

### 指摘・回帰対応表

選択件数はRust test数。loop内のケース数や未実行のtestを成功件数へ加算しない。manifest検査はRust testとは区別する。

| 指摘 | 判断・検査する失敗 | 回帰名と選択件数 |
| --- | --- | --- |
| A1 P2 | 真。空batchでもGPU consumer / submission、GL context選択を実行 | `retirement_texture_delivery_prepared_wgpu_skips_empty_and_delivers_free_only` 1、`retirement_texture_delivery_glow_skips_empty_context_and_delivers_free_only` 1 |
| A2 P3 | 真。testerのMSRV宣言だけ1.88 | `python scripts/test-egui-msrv.py`、GUI依存7 packageの宣言検査 |
| B1 P1 | 真。Close待ちの変更ありinstallがscrollを0へ戻し、consumerが保存値を上書き | `close_return_changed_bookmark_poll_preserves_live_main_position_before_consumer` 1（book/media × rename/add/removeの6ケース）、`close_return_bookmark_delete_receipt_preserves_live_main_position_before_consumer` 1。Close全filterは15 |
| B2 P2 | 真。無効・無入力の反復logicで終端が無制限増殖 | `gamepad_physical_disabled_inputless_logic_keeps_bounded_terminal` 1、`gamepad_physical_disabled_polls_preserve_reconnect_press_and_terminal_order` 1。physical filterは6 |
| C1 P1 | 真。Context cloneは子viewportを固定せず、遅延処理が親pointer / wheel / 許可を参照 | `egui037_wheel_frame_actual_immediate_child_keeps_zoom_anchor_and_root_wheel` 1、`egui037_wheel_frame_actual_immediate_child_music_gate_uses_child_focus` 1 |
| C2 P2 | 真。Window内蔵ScrollAreaのmouse drag指定漏れ | `egui037_about_builtin_scrollarea_mouse_drag_changes_offset` 1 |
| C3 P2 | 真。2窓の本文mouse drag指定漏れ | `egui037_remote_connection_body_mouse_drag_moves_window` 1、`egui037_tray_notice_body_mouse_drag_moves_window` 1 |
| D1 P2 | 真。Nodeの論理座標と物理単位の比較が混在 | `section207_tests` 17、両integrationの`logical_bounds::high_dpi_node_bounds_reject_a_control_outside_the_logical_viewport` 各1。高DPI境界・操作到達性の限定filterは2 target合計7 |
| D2 P2 | 真。fixture外余白による8ptの不要scrollを製品差と誤分類 | `fullscreen_overflow_panel_tall_viewport_keeps_the_production_body_height` 1（720×520 / 720×720）、既存snapshot 4を含む`fullscreen_overflow_panel` filterは7 |

### 変異による検出力の確認

| 変異 | 選択・観測 | exit |
| --- | --- | ---: |
| A: 共通dispatcherを無条件consumer呼出しへ戻す | wg​​pu 1失敗（空32回でconsumer/submit各32）、glow 1失敗（context entry32） | 各101 |
| A: set非空だけを配送しfree-onlyを落とす | wg​​pu 1失敗（free配送なし）、glow 1失敗 | 各101 |
| A: testerの宣言を1.88へ戻す | MSRV検査がtesterの不整合で停止 | 1 |
| B/C/D: live grid保持を無効化、削除producerを先行retain、終端を無条件push、遅延zoomを親pointer参照、drag指定を削除、viewportをui.max_rect、幅をzoom倍へ戻す | 10選択 / 10 assert失敗。scroll0、選択keyずれ、終端増殖、zoom中心ずれ、2窓の不動、aboutのwindow移動、8pt scroll、物理最小幅をそれぞれ検出 | 100 |
| B/C: consumerのrow_keys更新を削除、Owned区間を再消費、音楽許可を親参照へ戻す | 3選択 / 3 assert失敗。追加行keys不一致、親wheel消失、非focus子でもtimeline変更を検出 | 100 |
| D: 論理境界をpixels_per_point倍の物理矩形へ戻す | 両integration targetで2選択 / 2失敗。論理viewport外・物理矩形内の実ボタンを誤許容 | 100 |

Rust回帰のassert失敗は延べ19件。上記に加えMSRV宣言検査の失敗を確認した。compile失敗・0件一致を変異成功には含めない。
ログは`target/egui037-stage3-review-mut-*`のJSON/log/JUnit。初回free-only変異は未配送deltaのDropが二次panicしたため、
test側で未消費状態を保存してからheadless明示破棄を行い、同じassertを維持して再実行した。二次panicの結果は検出力の証拠に使わない。
実prepared RenderState / Renderer / queueのA回帰は共通dispatcherと実GPU resource処理の入口を検査する。
native WindowEventと実GLを一体で動かした証明ではなく、製品Painterの完結順は既存vendor回帰とsource照合で補う。

### fixture補修・限定独立確認

初回追加回帰のcompile失敗（旧raw_scroll_delta参照、ViewportClassのDebug要求、App型の修飾不足）は修正した。
Bの第二read-model結果は、第一installが始めた既存identity hydration中には保留されるのが正常である。
同passのconsumerとkeysのassertは維持し、既存`finish_page_identity_preparation_for_test`で準備を完了してから次の結果へ進め、両完了後の位置も検査する。
Remote退役の既存fixtureは、App構築で受付済みのresume ReadのACKを前提化してからacquireし、元の正確な2回のlogic検査を維持した。
製品のreadinessを緩めたり、完了までlogicを再送するfixtureへ変えたりしていない。
Trayの自動IdはWindowのAtoms.text()と同じOption値、wheelの期待geometryはtargetの実seek geometryを使う。
実子wheelは通常→Ctrl→通常を配送し、最後の要求のordinalとDeltaを検査する。未提示Display sequenceが次targetを拒否する既存gateも保持する。

別contextのSol / xhighによる読み取り専用の限定source確認はA/C/D2とB/Dに分けて実施し、追加発見の削除producer・無効取消の期待・上記fixture補修の再確認後、いずれもACCEPT。
Cargo・GPU・テストの実行証拠は以下の実装担当の記録であり、このsource確認による実行証拠ではない。

### 全件失敗の追跡分類・第2版

元のlib全件結果は変えない（13,481件、13,132成功 / 290失敗 / 既存skip59）。
以下は保存した失敗290件を、今回の限定実行で追跡した分類であり、全件を再実行した結果ではない。
第1版の「段4差9件／未分類・段3違反0」という完了判断は独立レビュー前の記録として区別する。

| 分類 | 第1版 | 第2版 | 判断 |
| --- | ---: | ---: | --- |
| 段3移行不備を補修し、対象test成功 | 179 | 180 | section207 snapshot-labelを追加。今回の新規回帰数を混ぜない |
| PNG比較由来 | 102 | 103 | fullscreen tallはFits成功後、実際の画像比較不一致まで到達 |
| 0.37のlayout / 挙動差 | 9 | 7 | 旧9件の扱いは次表。reported-controlsは座標補修後の別assertを再分類 |
| 未分類・残る段3契約違反 | 0 | 0 | 今回9指摘は別途全て補修・変異確認。旧0件判断を解消証拠に使わない |

第2版の根拠は`target/egui037-stage3-review-classification-v2.json`と`-evidence.json`。
第1版JSONを保持し、元JUnitと同じ290名に重複・欠落がないことを検査した。
更新元は`review-final-lib`のJUnit。今回の限定libは**256選択 / 247成功 / 9失敗 / exit100**で、
残る9失敗はPNG 7とlayout 2（section207の下記2件）。新規回帰・Close15・wheel21・gamepad105・App退役10は成功している。

| 旧「段4へ渡す9件」の再分類（第2版） | 今回の扱い・残る差 |
| --- | --- |
| `ui_adjustment_panel::adjust_scope_selector_tests::adjust_scope_action_labels_fit_258px_body` | 段4。自然button高25pt / 旧24pt。今回の影響外の証拠を継続使用 |
| `ui_fullscreen::tests::fullscreen_overflow_panel_snapshot_root_tall_without_scrollbar` | **段3 fixture不備を補修。** content384 / viewport384でFits成功、その後のPNG比較不一致へ再分類。新しい高さtestも成功 |
| `ui_dialogs::whats_new::tests::version_highlights_initial_width_keeps_previous_reading_area` | 段4。scale2初期外寸537 / intended546pt。今回の影響外 |
| `ui_main::section207_tests::section207_reported_controls_keep_left_input_right_order` | **段3の座標単位とfixture予算を補修後、別のlayout assertへ到達。** 全viewportの論理991.33pt（physical1487 / zoom1.5）で、left y38.7〜63.3、input y67.2〜92.0となり旧「950pt以上なら同じ行」に不一致。元assertを維持して段4へ |
| `ui_main::section207_tests::section207_snapshot_label_keeps_input_minimum_order_and_panel_bounds` | **段3移行不備を補修し成功。** 論理幅定数・論理bounds・全viewportを検査。旧zoom倍の比較へ戻した変異はwidth360 / zoom1.5で失敗 |
| `ui_main::section207_tests::section207_default_folder_geometry_matches_the_separate_panels` | 段4。今回も統合Panel y49.5 / 旧別Panel y50.5の1pt差で失敗 |
| `ui_text::tests::bubble_body_stays_within_panel_width_with_fill_on_or_off` | 段4。268pt枠 / 自然幅268.21875pt。今回の影響外 |
| `ui_main::toolbar_wrap_tests::a_combo_stays_inside_the_panel_only_when_we_reserve_its_place` | 段4。新WidgetAtomでは裸Comboもwrapするため旧negative controlのoverflow0。今回の影響外 |
| `ui_dialogs::preferences::name_colors::tests::hex_focus_transfer_keeps_the_new_editors_partial_input` | 段4。同batch pointer focus移動とTextの処理順差。今回の影響外 |

段4へ残るlayout / 挙動testは上表の7件。PNG比較103件は段4bで比較し、更新は段5へ残す。
既存assertの弱化・ignore追加・PNG baseline更新は行わない。警告整理も段5へ残す。

### 最終の限定確認コマンド（実装担当の内部確認）

すべてoffline。Cargo実行は直列、`CARGO_NET_OFFLINE=true`、`CARGO_BUILD_JOBS=1`、`MSBUILDDISABLENODEREUSE=1`、
`CARGO_TARGET_DIR`はこのworktreeの`target`、`UPDATE_SNAPSHOTS`は未設定とした。
各labelの完全なcommand / 秒 / exit / logは`target/egui037-stage3-<label>.json`、
nextestの選択・失敗内容は同名`-tests.json`と`-junit.xml`へ保存した。

`review-final-lib`のfilterは次のとおり。他のnextest共通引数は表の完全command JSONを参照する。

```powershell
$filter = 'test(close_return) | test(gamepad) | test(egui037_) | test(section207_tests) | test(fullscreen_overflow_panel) | test(bookmark_return) | test(unchanged_bookmark_refresh) | test(native_immediate_close_consumes_bookmark_return_in_same_dispatch) | test(root_lifecycle) | test(context_state) | test(viewport_retirement) | test(music_ctrl_wheel) | test(music_overflow_real_input_wheel) | test(bookmark_build_) | test(native_bookmark_delete_and_title) | test(bookmark_browser_delete_admits)'
cargo nextest run -p mimageviewer --lib -E $filter --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --offline
```

| filter / 必須契約 | 最終選択・結果 | 既存対応表への補足 |
| --- | --- | --- |
| `egui037_continuation` | 47 / 47成功 | C1〜C5・D3・旧順序 / pointer / 予算 / cutoffの正式Full接続は維持 |
| `root_lifecycle` / `context_state` | 11 / 11成功、8 / 8成功 | 第4.4版の非描画受付と対象限定状態は維持 |
| `close_return` | 15 / 15成功 | 第4.6版帰還に、変更ありread-model・削除receipt・hydration尾を追加 |
| `bookmark_return` / `unchanged_bookmark_refresh` / `bookmark_build_` | 5 / 5成功、1 / 1成功、9 / 9成功 | 従来のRestoringと同内容refresh、source adoptionも保持 |
| `viewport_retirement`（App） | 10 / 10成功 | 第4.5版のRemote配送待ち・既存ACK / readinessを維持 |
| `gamepad`（nextest） / `gamepad_physical_` | 105 / 105成功、うちphysical 6 / 6成功 | 反復logicの終端容量とpress/reconnect順を追加 |
| `egui037_wheel_frame_` | 21 / 21成功 | 実immediate子のpointer / 親queue不変 / 音楽許可2件を追加 |
| `section207_tests` | 17 / 12成功 / 5失敗 | PNG 3、上記layout 2。snapshot-labelの座標回帰は成功 |
| `fullscreen_overflow_panel` | 7 / 3成功 / 4 PNG失敗 | 新しい高さ回帰成功、旧tallのFitsも成功後PNGへ到達 |
| `calculated_overflow_panel_height_matches_all_states_and_scrolls_when_constrained` | 1 / 1成功 | 4状態×3サイズ×2 spacingの既存24ケース |

第4.3版16項目・第4.5版13項目・第4.6版の元対応表は維持し、上の補足を今回の最新件数とする。
vendor全libの実行には既存`minimized_full`（wgpu9 / glow11）、viewport入力・Close・discard delta回帰も含まれ、
新しい空batch回帰は`retirement_texture_delivery`で各backend 1件。liveの既存ignoreは実行していない。

| label | コマンド | 選択・結果 | exit / 秒 |
| --- | --- | --- | --- |
| `review-final-lib` | `cargo nextest run -p mimageviewer --lib -E $filter --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --offline` | 256 selected / 247 passed / 9 failed | 100 / 102.91 |
| `review-final-integration` | `cargo nextest run -p mimageviewer --test ui_snapshot --test book_composition_editor_snapshots -E 'test(high_dpi) \| test(image_folder_zip_small_viewport_controls) \| test(startup_dialogs_refit_after_viewport_resize) \| test(composition_editor_all_phases_keep_cancel_reachable)' --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --offline` | 7 selected / 7 passed / 0 failed | 0 / 14.83 |
| `review-final-overflow-sizes` | `cargo nextest run -p mimageviewer --lib -E 'test(calculated_overflow_panel_height_matches_all_states_and_scrolls_when_constrained)' --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --offline` | 1 selected / 1 passed / 0 failed | 0 / 4.29 |
| `review-final-eframe-wgpu` | `cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib --offline` | test result: ok. 53 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.88s | 0 / 5.17 |
| `review-final-eframe-glow` | `cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,glow --lib --offline` | test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s | 0 / 2.08 |
| `review-final-msrv` | `python scripts/test-egui-msrv.py` | 7 GUI MSRV declarations passed | 0 / 0.05 |
| `review-final-core` | `cargo check -p mimageviewer --bin mimageviewer-core --offline` | check | 0 / 38.78 |
| `review-final-test-script` | `cargo check -p mimageviewer --bin mimageviewer-core --features test-script --offline` | check | 0 / 37.91 |
| `review-final-tester` | `cargo check -p vst3-host-tester --all-targets --offline` | check | 0 / 28.9 |
| `review-final-tests-no-run` | `cargo test -p mimageviewer --tests --no-run --offline` | 23 executables; no run | 0 / 67.06 |
| `review-final-fmt` | `cargo fmt --check` | check | 0 / 25.95 |
| `review-final-vendor-fmt` | `rustfmt --edition 2024 --check --config skip_children=true vendor/eframe/src/native/viewport_lifecycle.rs vendor/eframe/src/native/wgpu_integration.rs vendor/eframe/src/native/glow_integration.rs` | check | 0 / 0.21 |
| `review-final-glyphs` | `python scripts/check_ui_glyphs.py` | check | 0 / 3.56 |
| `review-final-tree` | `cargo tree -d --offline` | check | 0 / 0.82 |
| `review-final-diff` | `git diff --check` | check | 0 / 0.39 |

### 再確認の残件への対応（7cb0494f9後）

設計担当が補修を`7cb0494f9`へコミットした後、同じ独立レビュー担当による再確認で、
A・BはACCEPT、CはP3を1件、DはP2を1件指摘した。以下はその2件への**実装担当の内部確認**。
前節の初回レビュー対応と分類第2版は履歴・有効な検証証拠として保持する。

| 指摘 | コードでの判断・根本原因 | 修正・主なファイル | 回帰 |
| --- | --- | --- | --- |
| C1残件 P3 | 成立。遅延PageSideだけが親へ戻ったContextで操作IDを再検索していた。barrierのsource一致により子クリックの事実を失う | `src/ui_fullscreen.rs`の既存pass-local `FsWheelFrameSemanticInput`にpointer操作IDを捕捉し、PageSide適用へ渡す。wheelと同じ子ownerの受付内で同定を完了 | `egui037_wheel_frame_actual_immediate_child_click_keeps_operation_identity`。実immediate子の通常wheel→Alt+wheel＋左単クリック、最終操作ID・診断のsource/cause、親入力不変を検査。**1選択・1成功**（最終wheel22件の内数） |
| D1残件 P2 | 成立。`Node::rect()`の論理矩形をphysical幅相当の`width * zoom`と比較し、240pt画面で360ptまで許容していた | `tests/ui_snapshot.rs`のstatus labelとinputの境界を`logical_bounds::assert_node_inside`へ接続。両integration targetの同型比較も点検 | 既存`folder_toolbar_snapshot_status_label`の3幅×2zoom×2ラベルを維持。新規`folder_toolbar_snapshot_status_label_rejects_high_dpi_overflow`は同じconsumerで、240pt viewport外・360pt旧境界内の実Nodeを拒否。**追加回帰1選択・1成功**。既存testは1選択、全12 geometry成功後にPNG不一致 |

新しいApp pending状態・非表示入力機構・guard・delay・retry・repaintは追加していない。
既存assert・PNG baseline・ignoreは維持する。子入力の操作ID受渡しは、今回の独立レビューCが
「所有境界を正す構造的修正」と認めた範囲であり、第4.6版の既存pass-local owner内の補修である。
設計書との食い違い・新しい設計判断はない。

限定実行では、従来のstatus-label fixtureが0.37のaccessibility検査で、名前のないTextInputとして停止することも確認した。
そのfixtureのResponseに`accessible_name("Folder address")`を付け、検査の無効化をせずに到達を回復した。
これはAccessKit labelだけの指定で、描画・矩形・disabled状態を変えない。追加1行も読み取り専用の限定独立source確認でACCEPT。
その後は12ケースの配置・最小幅・論理境界検査を全て通過し、`SnapshotResults`集約の終端で
`folder_toolbar_snapshot_label_stacked` / `folder_toolbar_snapshot_label_inline`のPNG不一致2枚に到達した。
このintegration testのPNG差は既存の段4b比較・段5更新方針に従い、baselineは変更しない。
元のlib全件290名の集合にはこのintegration testを含まないため、追跡分類第2版の180成功／103 PNG／7 layout・挙動差は変更しない。
lib全件の再実行は利用者指定により行わない。

#### 再確認2件の変異検証（実装担当の内部確認）

両方とも実sourceを一か所ずつ旧処理へ戻し、既に正常系で到達を確認した新規回帰を単独選択した。
変異後は指定assertでの失敗をJSONで検査し、`finally`で元のbyte列へ復元した。
最終の限定確認は両変異を取り除いた後に実施した。

| label / 変異 | 回帰名 | 選択・結果 | exit / 秒 |
| --- | --- | --- | --- |
| `review2-mut-c`：PageSideを`self.page_turn_pointer_navigation_operation(ctx, "pointer")`の遅延検索へ戻す | `egui037_wheel_frame_actual_immediate_child_click_keeps_operation_identity` | 1選択・1失敗。`PageSide retains the child's click identity`で`None`対`Some(Pointer { source: child, ordinal: 3, .. })` | 100 / 64.53 |
| `review2-mut-d`：status-label consumerを`ctx.content_rect().width() * ctx.zoom_factor()`まで許容する旧物理幅比較へ戻す | `folder_toolbar_snapshot_status_label_rejects_high_dpi_overflow` | 1選択・1失敗。旧上限360pt内の画面外Nodeを誤って許容し、負例の拒否assertで失敗 | 100 / 29.43 |

両変異はコンパイル・起動失敗ではなく、意図したRust assertの失敗まで到達している。
完全なcommand / exit / 秒 / logは`target/egui037-stage3-<label>.json`、
選択件数と失敗本文は同名`-tests.json` / `-junit.xml`に保存した。

#### 再確認2件の最終限定実行（実装担当の内部確認）

今回2件の限定独立source確認（`gpt-6.1-sol` / `xhigh`、読み取り専用）はACCEPT・追加指摘なし。
レビュアーはCargo・製品起動・ファイル変更を行っておらず、下表は実装担当が直列・offlineで実行した結果である。
環境・ログ命名は前節と同じ。`UPDATE_SNAPSHOTS`は未設定、lib全件・live・製品build/launch・`test-full.ps1`は実行しない。

```powershell
$libFilter = 'test(egui037_wheel_frame_) | test(page_turn::input_regression_tests)'
$integrationFilter = 'test(high_dpi) | test(image_folder_zip_small_viewport_controls) | test(startup_dialogs_refit_after_viewport_resize) | test(composition_editor_all_phases_keep_cancel_reachable) | test(folder_toolbar_snapshot_status_label)'
```

nextest両コマンドの共通引数は`--no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --offline`。
全commandは各labelのJSONにも保存した。

| label | コマンド | 選択・結果 | exit / 秒 |
| --- | --- | --- | --- |
| `review2-final-lib` | `cargo nextest run -p mimageviewer --lib -E $libFilter`＋共通引数 | 42選択・42成功。wheel22、既存page-turn input20。新規子クリック1件を含む | 0 / 67.71 |
| `review2-final-integration` | `cargo nextest run -p mimageviewer --test ui_snapshot --test book_composition_editor_snapshots -E $integrationFilter`＋共通引数 | 9選択・8成功・1 PNG失敗。新規status-label負例1件成功。既存status-labelは全12 geometry成功後、stacked / inlineのPNG2枚だけ不一致 | 100 / 15.26 |
| `review2-final-core` | `cargo check -p mimageviewer --bin mimageviewer-core --offline` | check成功 | 0 / 14.07 |
| `review2-final-tests-no-run` | `cargo test -p mimageviewer --tests --no-run --offline` | 23 executableをコンパイル、実行なし | 0 / 51.28 |
| `review2-final-fmt` | `cargo fmt --check` | check成功 | 0 / 13.81 |
| `review2-final-diff` | `git diff --check` | check成功 | 0 / 0.08 |

新規ignore・期待値の弱化・PNG baseline更新はない。今回の2指摘は補修済みで、設計書との食い違いはない。
段4への7 layout / 挙動差、PNGの段4b比較・段5更新、警告の段5整理は従来どおり残る。
A・Bや今回変更しないvendor / GPU / Close / gamepadの証拠は、設計担当の再確認ACCEPTと前節の有効な結果を再利用する。

### 最終ソース全件との照合（3aa2d8eda後、分類第3版）

設計担当が補修を`3aa2d8eda`へコミットし、A〜Dの独立レビュー4本がACCEPTとなった後、
同じ最終ソースで`test-lib-full.ps1`を実行した。runは
`target/test-lib-full/runs/20261017T061555884Z-f72ea765`。
JUnitを再集計すると**総数13,493（skip59を含む）／実行13,434／成功13,321／失敗113**。
これは設計担当が実行した全件であり、今回の実装担当はlib全件を再実行していない。

分類第2版との照合では、PNG103件と段4のlayout / 挙動差7件が全て失敗名と一致した。
PNG分類なのに成功した名前は0件。残る3件のうちbookmarkの1件は旧`resolved`にあり、
HUDとmetadataの2件は旧290失敗の集合外だった。以下の判断・限定実行は**実装担当の内部確認**。

#### 1. Bookmark resolver取消後のrow keys

`detached_media_bookmark_resolve_cancel_preserves_target_until_exact_retirement`は、
旧全件でも失敗していたが、`v46-postfull-contract-final`でPreparing mediaの終端捕捉等を補修した時点では成功していた。
7cbのClose帰還修正は、consumerより前に変更ありread-modelが採用されたときのlive一覧キーを帰還snapshotへ反映する。
その後の`review-final-lib`はこの既存テストを選択していなかったため、第2版が旧成功を無条件に引き継いだのは検証証拠の再利用不備だった。

現在の失敗は**fixtureの段3移行漏れ**。mainの`bookmark_browser_rows`は空なのに、
Opening snapshotには生成不能な`(2, 2)`を保存していた。`stable_key()`はMedia `(0, id)`とBook `(1, id)`のみを生成する。
7cbのconsumerが現在の空keysを採ること自体は、帰還待ち中の全行削除でも必要な契約である。
空のときだけ旧keysへ戻すguardやconsumerの巻戻しは行わない。

`src/app/tests.rs`のfixtureを実在Media行id2の既存installer採用へ直し、既存hydration ownerの完了後にOpening gridをcaptureした。
キャンセル後・consumer前のmain選択解除も明示し、target保持・late resolver拒否・exact owner退役・scroll / grid・一回消費のassertを維持した。
新規`close_return_empty_bookmark_poll_adopts_empty_live_grid`は、実read-modelの全行削除→Terminal consumer→同内容空refreshを通し、
空keys / 選択なし / opened keyとscroll保持 / Restoringの一回消費を検査する。
独立premise確認と補修差分の限定source確認（`gpt-6.1-sol` / `xhigh`、読み取り専用）はACCEPT。
製品consumer・所有構造・終了寿命は変更していない。

#### 2. HUDのexternal raise

`hud_geometry_ignores_nozorder_but_external_zorder_still_requests_raise`の対象ファイル
`src/video/native_window_host/hud_window.rs`は、master `2d75d0c41`とHEAD `3aa2d8eda`で完全同一。
テスト導入の`27fa289ee`（2026-10-15）から該当構造は同じで、1e→3aaにも対象HUD・event route・focus fake等の関連変更はない。
実`CreateWindowExW`→直接`SetWindowPos`→`hud_wnd_proc`→`send_hud_raise`はegui / eframeやnative focus fakeを通らない。
raiseは専用latest slotへ配送して直後にdrainされる。段3によるイベント消失を示す差分は確認できない。

テスト外のWin32状態に依存する前提は、実`SetWindowPos`が`WM_WINDOWPOSCHANGING`を
**`!SWP_NOZORDER`かつ`hwndInsertAfter`が0または-1**で届けること。
テストは隠れたTOPMOST HUDを一つだけ作り、相対z-orderを事前に作らず、TOP / TOPMOSTへの操作にraise1件を期待している。
この前提をテストは確立していない。既存`probe-hidden-hud-zorder.ps1`は対照的に、隠れたeditorをHUD前へ配置して順序を確認してから各操作を行う。

直前全件`20261017T034945177Z-04c522be`では成功、今回全件では失敗し、今回の実装担当の初回限定実行では成功した。
したがって**既存の実Win32状態に依存するテスト前提が原因候補**であり、段3退行の根拠はない。
ただし失敗時の実通知fieldsは記録されておらず、通知なし・NOZORDER化・実HWNDへの正規化のどれかは未確定。
利用者desktop上の特定の別topmost窓が原因、とまでは断定しない。

製品修正やignoreは入れない。設計担当へ、実通知fieldsと相対順の観測、およびhandler/event routeの正負条件と
隠れたsiblingを使う実Win32復旧検査をどう分けるか、テストの扱いを返す。probe・live・製品起動は実行していない。

#### 3. Metadata transferとresume writerの待機

`zip_save_retired_owner_precedes_metadata_preview_import_and_export`の
`src/app/book_composition_editor/metadata_transfer_tests.rs`と`src/book_resume_db.rs`はmasterとHEADで完全同一。
同名テストはmasterにもあり、`check_transfer` / `assert_resume_once` / writerの返信順序も同じである。

**既存のテスト同期競合がsource上成立する。** `assert_resume_once`は`read_position_after`の返信を待つが、
workerは`ReadPositionAfter`の返信send（`book_resume_db.rs:697`）の後でpendingを減算する（同:734）。
warm snapshot経路もRecordの完了watermark公開（同:620）とpending減算の間に即座に返信できる（同:813〜821）。
したがって、返信を受けたテストが先行してImport / Exportのreadinessを問うと、
`rename_migration_writers_busy`→`BookResumeWriter::is_busy()`が正当にbusyを返し、WaitingForWritersが残り得る。
返信の「cutoffまでの保存位置が読めた」は「writer全体がidle」の保証ではない。

同じファイルのGeneric migrationテストは、まさにこの返信→pending減算の隙間をコメントし、
resumeの全assert後に既存DropでFIFOをdrain / joinしてから次の受付を検査している（同:439〜442）。これはmasterにもある。
段3は退役後resumeの配送をglobal persistへ整理しているが、masterも退役ownerで同じ`record_identity`をenqueueする。
この同期競合を段3が新規導入した根拠はなく、製品の保存順序違反も確認していない。

今回の1回のログにはentry / failureの組やbusy内訳がないため、その実測失敗がこの隙間だったとの断定は避ける。
限定初回は成功。製品コード・テストの待機・assert・ignoreは変更せず、既存テスト課題として設計担当へ返す。
補修候補は、保存一回のassert後に既存Genericテストと同じdrain / joinで他writerの完了条件を明示し、
composition ticketの回収境界を独立に検査すること。sleep / retry / busy契約の緩和は不要である。

#### 全件失敗分類の第3版

正本は`target/egui037-stage3-review-classification-v3.json`。
生成・重複欠落検査は`target/egui037-stage3-review3-classify.py`に保存する。
第2版JSONと上の履歴は変更せず、第3版では**全件実行時の113失敗**と**補修後の限定証拠**を分ける。

| 3aa2d8edaで観測した113失敗 | 件数 | 第3版での判断 |
| --- | --- | --- |
| PNG比較 | 103 | 第2版の全103名と一致。baseline更新は段5 |
| 段4のlayout / 挙動差 | 7 | 第2版の全7名と一致。従来の段4表を維持 |
| Bookmark resolver取消のfixture移行漏れ | 1 | 実在rowを採用するfixtureへ補修し、限定再実行で成功を確認 |
| 既存HUDのWin32通知前提 | 1 | masterと同じ経路。失敗時通知未記録のため正確な環境要因は未確定、設計担当のテスト判断待ち |
| 既存metadata testの同期競合候補 | 1 | masterと同じ返信→pending減算の隙間が成立。1回の実測内訳は未記録、設計担当のテスト判断待ち |
| 合計 | **113** | 全件JUnitの失敗名を重複・欠落なく覆う |

第2版の元290名については、他の`resolved`179名が今回の全件で成功していることを再確認し、
残るbookmarkの1名には今回の補修後の限定成功を割り当てる。
これにより元290名の追跡は補修180／PNG103／段4差7のまま、追加の既存テスト課題2名を`pending`として区別する。
**第3版の追跡全体は292名、pending2は既存テストの設計判断待ちであり、解消済みと扱わない。**
今回の限定実行でHUDやmetadataが成功しても、全件で起きた失敗が解消した証明にはしない。
製品の段3契約を変える修正・ignore・assert弱化・PNG更新は行わない。

#### 第3版の限定再実行・変異（実装担当の内部確認）

Cargoは従来と同じoffline・直列、`CARGO_BUILD_JOBS=1`・`MSBUILDDISABLENODEREUSE=1`。
live・製品build/launch・`test-full.ps1`・lib全件は実行していない。今回の恒久Rust変更はテスト2ファイルだけで、製品バイナリは作らない。
各command / exit / 秒は`target/egui037-stage3-<label>.json`、選択件数と本文は同名`-tests.json` / `-junit.xml`を参照する。

| 対象 | 初回3件filter（未補修3aa） | 復元後の関連filter |
| --- | --- | --- |
| `detached_media_bookmark_resolve_cancel_preserves_target_until_exact_retirement` | 1選択・1失敗。空rows対生成不能な保存keyの不一致を再現 | 1選択・1成功。実在行fixtureへ補修 |
| `hud_geometry_ignores_nozorder_but_external_zorder_still_requests_raise` | 1選択・1成功 | 1選択・1成功。未補修の既存テスト判断待ちを維持 |
| `zip_save_retired_owner_precedes_metadata_preview_import_and_export` | 1選択・1成功 | 1選択・1成功。未補修の既存同期競合候補を維持 |
| 新規`close_return_empty_bookmark_poll_adopts_empty_live_grid` | 補修後の2件filterで1選択・1成功 | 1選択・1成功 |

source変異`review3-mut-stale-keys`は、`close_return.rs`のconsumerから現在のrow keysの再捕捉だけを省き、
7cbより前と同じ保存keysの保持へ戻したもの。新規空一覧回帰を1件選択し、
`deleted rows cannot return as saved keys`のassertで1件失敗（exit100 / 57.48秒）した。
コンパイル・起動失敗ではなく、空採用後に古い30行のキーが復活する契約違反を検出している。
`finally`で元のbyte列へ復元してから、以下の最終確認を実行した。

```powershell
$filter = 'test(close_return) | test(bookmark_return) | test(unchanged_bookmark_refresh) | test(still_window_mode_key_tests) | test(video::native_window_host::hud_window::tests) | test(app::book_composition_editor::metadata_transfer_tests) | test(book_resume_db::tests)'
cargo nextest run -p mimageviewer --lib -E $filter --no-tests fail --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --offline
```

最終関連filterの内訳は、Close帰還16、bookmark return5、同内容refresh1、still-window-mode549、
HUD2、metadata-transfer5、resume DB18で、合計596件すべて成功（重複なし・skipなし）。

| label | コマンド | 結果 | exit / 秒 |
| --- | --- | --- | --- |
| `review3-initial-three` | `cargo nextest run -p mimageviewer --lib -E '<指定3件のOR>'`＋従来共通引数 | 3選択・2成功・1失敗（bookmark） | 100 / 2.88 |
| `review3-bookmark-green` | `cargo nextest run -p mimageviewer --lib -E 'test(detached_media_bookmark_resolve_cancel_preserves_target_until_exact_retirement) \| test(close_return_empty_bookmark_poll_adopts_empty_live_grid)'`＋従来共通引数 | 2選択・2成功 | 0 / 280.12 |
| `review3-mut-stale-keys` | `cargo nextest run -p mimageviewer --lib -E 'test(close_return_empty_bookmark_poll_adopts_empty_live_grid)'`＋従来共通引数 | 1選択・指定assertで1失敗（変異検出） | 100 / 57.48 |
| `review3-final-related` | 上記の`cargo nextest run` | 596選択・596成功 | 0 / 76.97 |
| `review3-final-tests-no-run` | `cargo test -p mimageviewer --tests --no-run --offline` | 23 executableコンパイル、実行なし | 0 / 81.85 |
| `review3-final-fmt` | `cargo fmt --check` | 成功 | 0 / 13.72 |
| `review3-final-diff` | `git diff --check` | 成功 | 0 / 0.08 |
| `review3-final-classification` | `python target/egui037-stage3-review3-classify.py` | 113失敗の完全被覆・追跡292名の重複なし・179名の全件成功／1名の補修後成功・master同一性を検証し第3版生成 | 0 / 0.21 |

前節の第2版の段4表・全件分類は過去の記録として保持し、上記を更新後の第3版とする。
HUDは失敗時Win32通知の観測とテストの前提、metadataはactorのidleまで待つ検査の整理を設計担当へ返す。
今回の修正で新しい設計契約や製品の操作変更はない。
