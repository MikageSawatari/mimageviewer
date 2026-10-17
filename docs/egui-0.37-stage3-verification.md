# egui 0.37 更新 — 段3 検証台帳

## 状態（2026-10-17）

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
