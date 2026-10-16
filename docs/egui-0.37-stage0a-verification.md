# egui 0.37 更新・段0a 検証台帳

2026-10-16、`egui-036` / `9acf4122612a5aa69e34f11ac25f456e7c65e33f` 上の未コミット差分。
Rust 1.99.0、Windows x86_64 MSVC、cargo-nextest 0.9.148、PowerShell 5.1.26100.9711、通常feature / test profile。
実装担当が非対話検証を所有し、設計担当がレビュー・コミットを行う。
live試験・製品バイナリの起動、ブランチ操作、コミットは行わない。

## 隔離対象

既定実行されていたeframeの1件だけに、理由付き `#[ignore]` を追加した。
実行ロジック・期待値・期限は維持する。自己再実行のため、子test executableの
exact名呼出しに `--ignored` を追加する。これがないと親だけを明示選択しても子側がskipされる。

| 所在（module） | テスト名 | liveの根拠 |
| --- | --- | --- |
| `vendor/eframe/src/native/run.rs` / `native::run::tests::windows_process_test` | `synchronous_damage_returns_before_outer_paint_gate` | 子で`run_native`、320×200窓を作りWin32 message送信、paint順序検査 |

この1件だけが `scripts/test-live-windows.ps1` のexact allowlistに対応する。
runnerは承認済みswitchを要求するが
対話promptを持たない。利用者が隔離desktop/sessionで実行する専用入口であり、
エージェントからは起動しない。未実施のままPASSにしない。

## 横断監査

`vendor/eframe`、`vendor/egui-wgpu`、`vendor/egui-winit`、`vendor/egui`、`crates/`、
`src/`、`tests/` のRust test属性と、window作成・`run_native`・foreground・実入力API、
子process、production owner/helperの呼出元を照合した。追加の未隔離live試験は見つからなかった。
実装者の監査に加え、別contextのread-only監査でも残りの疑わしい経路を確認した。

設計レビュー後の修正：初回は実HWNDの作成までliveに含め、本体8件を広くignoreしすぎた。
AGENTS.mdの承認対象はアプリ起動・前面切替・稼働中UIへの実入力送出であり、
headless unit / snapshot / fake-input検査は自動継続する。本体8件のignoreと子側へ加えた
`--ignored`を撤回し、該当する`src/`の3ファイルはHEADと同じ内容へ戻した。

| 自動gateを維持する本体headless試験 | 対象外とした理由・コード根拠 |
| --- | --- |
| Everything IPCの`fake_hwnd_*` 5件（`src/everything_ipc/native.rs`） | `HWND_MESSAGE`のfake endpoint。アプリ起動・前面切替・実入力送出なし |
| hidden HUD 2件（`src/video/native_window_host/hud_window.rs`） | `hidden_hud`は非表示HWNDを作り、非表示のままgeometry/z-orderを検査 |
| HUD focus 1件（`src/video/native_window_pump.rs`） | 子popupは(-2000,-2000)、`WS_EX_NOACTIVATE`＋`SW_SHOWNOACTIVATE`。入力はsinkへの合成イベント。`install_focus_claim_probe_for_test`により`native_window_host.rs`のClaimTextInputFocus分岐はカウンタ更新後continueし、実`claim_foreground`へ進まない。HUDも画面外・空regionで利用者に見える表示なし |

既存ignoreのlive試験は変更せず、今回のrunnerの対象にも追加しない：
`real_everything_diagnostic_probe`（実Everythingとinput desktop）、
`stationary_cursor_routes_separate_top_level_window_show_and_hide`（実窓とcursor）、
`production_parent_destroy_remains_bounded_during_render_stall`（DComp実窓）、
`cross_thread_dcomp_present_remains_pump_independent_when_render_stalls`（hardware/driver gate）。

| 対象外とした候補 | 到達しない根拠 |
| --- | --- |
| eframe pure scheduler 14件 | `RenderScheduler<u8,u8>`の状態遷移のみ。窓・event loopを作らない |
| eframe initial visibility 2件 / screenshot 1件 | visibilityはshow closure、screenshotはqueued action抽出のみ。実Windowを作らない |
| `miv_test_script_window_witness` | `WindowWitnessFixture`は`Arc<()>`とregistry/TLSによるfake |
| egui-winit State/IME tests | `headless_state`は`NoDisplay`。Windows clipboard初期化もHWNDを作らない |
| egui-wgpu mipmap / no-surface tests | offscreen GPU、`RenderState::create(..., None, ...)`。surface/window作成なし |
| egui、egui-ime、UI snapshot / headless Harness | egui Context/合成input/texture readback。native event loopなし |
| `native_window::presenter_wndproc_handles_setcursor_without_defaulting_to_class_arrow` | NULL HWNDで対象branchを直接検査。ログと`LRESULT(1)`だけでcursor変更なし |
| `window_host_contract` / `native_ui_smoke` receipt tests | 連番opaque handle、固定fake HWNDとbroker検証。SendInputを呼ばない |
| tray / gamepad | `cfg(test)` controllerはatomic/channelだけ、gilrs readerを起動しない |
| clipboard capture / cut clipboard | 注入reader・0 HWND mailbox・Inert/Disabled observer。production窓ownerを起動しない |
| native context menu / native name dialog | IDataObject/HGLOBAL、pure判定、template検査のみ。popup/dialog表示ownerを呼ばない |
| Effetune / VST editor配送 | `FakeHost` / `Bridge::fake_host_for_test`でprocess/editor境界を置換 |
| EPUB worker | URL/options/process fixtureのみ。WebView用`Host::new`を呼ばない |
| vst3-host-tester | testはprotocol JSON。`run_native`/GUI hostはmain経路のみ |
| launcher/watchdog subprocess tests | test executable内の終了処理やheadless Contextを呼び、アプリmainを呼ばない |

## 初回検証（修正前の履歴）

以下は、本体8件を一時的にignoreしていた修正前の結果。現行の再検証は次節に記録する。

ログは `target/egui037-stage0a/`。runner fake検査の証跡は
`target/live-runner-fake-46d2e7f2f91d48dca8e5166217600b94/`。

| コマンド | 結果 | exit code |
| --- | --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功（既存警告あり）、`core-check.log` | 0 |
| `cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib --no-run` | compile成功、`eframe-compile.log` | 0 |
| 同graphの`cargo nextest list ... --run-ignored default --message-format json` | 18件中live 1件が`ignored` mismatch、非live 17件match、`eframe-list.json` | 0 |
| 同graphの`cargo nextest run ... --run-ignored default --retries 0 --no-fail-fast --no-tests fail` | 17 passed、1 skipped（pure scheduler 14件を含む）、`eframe-run.log` | 0 |
| `powershell -NoProfile -File scripts/test-live-windows-runner.ps1` | PS 5.1 parse/ASCII、承認なし拒否、All/各suite、失敗・0件のexit伝播：fake 6ケース成功 | 0 |
| `cargo fmt`（引数なし） / `cargo fmt --check` | workspace全体成功 | 0 / 0 |
| `cargo nextest run -p mimageviewer --lib --no-run` | turbojpeg-sysのCMake/MSBuild段でcompile失敗（test実行0件）、`main-compile.log` | 101 |
| 同上（`MSBUILDDISABLENODEREUSE=1`） | 同じcompile失敗、`main-compile-node-reuse-disabled.log` | 101 |
| `cmake --build target/debug/build/turbojpeg-sys-fac779e9c83ac1ee/out/build --target install --config Debug --parallel 1 -- /verbosity:diagnostic`（同環境変数） | native build成功、`turbojpeg-msbuild-diagnostic.log` | 0 |
| `cargo nextest run -p mimageviewer --lib --no-run`（`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`） | compile成功、`main-compile-serial.log` | 0 |
| 同環境で`cargo nextest list -p mimageviewer --lib --config-file .config/nextest.toml --profile default --ignore-default-filter --run-ignored default --message-format json` | 13,290件中13,223件match、67件ignore。新規live 8/8件が`ignored` mismatch、`main-list.json` / `main-list-summary.txt` | 0 |
| 同環境で`powershell -NoProfile -File scripts/test-lib-full.ps1` | 13,223件実行、13,185 passed、38 failed、67 skipped。362.261秒、全件成功ではない、`main-run.log` | 100 |
| 同環境で`cargo nextest run -p mimageviewer --lib -E 'test(=ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions)' --config-file .config/nextest.toml --profile default --ignore-default-filter --run-ignored default --retries 0 --no-tests fail` | 全件時の非RAW失敗を単独再確認：1 passed、13,289 skipped、`page-turn-rerun.log` | 0 |

本体lib全件の詳細は `target/test-lib-full/runs/20261016T021746482Z-a20bcc5d/`
（`stderr.log`、`junit.xml`）。対象source IDは
`8c83b7da8c3c373679934fbf382316f2e3a7e3dd52543bce5c8f1df82e3369c6`。
初回全件終了直後は本台帳への結果追記のみを行った。その後の設計レビュー修正は上記のとおり。
失敗名一覧は `target/egui037-stage0a/main-failures.txt`。

- 37件は`vendor/raw-samples/`欠落。36件がsetup scriptを要求してpanic、残る
  `remote_ipc::container::tests::cached_zip_raw_representative_can_prefetch_without_extracting_nested_archives`
  は同じ`885.dng`を読む`src/remote_ipc/container.rs:12128`でNotFound。
- 残る1件は`ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions`。
  全件時には`src/ui_fullscreen/page_turn/draw_submit_tests.rs:456`のold9可視性assertが
  progress=0.75で失敗した。単独再実行は成功。原因未確定であり、既知baseline失敗とは断定しない。
  この段では描画実装・期待値・fixture・ignoreを変更しない。
- 当時一時的にignoreした本体8/8件は全件JUnitでも`skipped`。非liveはEverything IPC 14件と
  native window pump 3件が成功、同領域の失敗0件。

本体test executableのWindows loader用に、未配置だった`vendor/ffmpeg/bin/*.dll`の6本を
`target/debug/deps/`へコピーした（通常profileや製品出力先は変更していない）。

上記MSBuild失敗のログにはMSB3191やアクセス拒否の詳細は出なかった。
並列1のnative buildは成功したため、`MSBUILDDISABLENODEREUSE=1`と`CARGO_BUILD_JOBS=1`で
本体lib compileが成功した。恒久的なbuild設定・依存ソースは変更していない。

## 設計レビュー修正後の検証

本体8件を既定実行へ戻し、live runnerをeframe 1件に限定した。
RAW fixtureは設計担当が`vendor/raw-samples/`へコピーし、`885.dng`の存在を確認した。
同一worktreeで以下を再検証した。

| コマンド・検査 | 結果 | exit code |
| --- | --- | --- |
| `git diff --exit-code -- src/everything_ipc/native.rs src/video/native_window_host/hud_window.rs src/video/native_window_pump.rs` | 本体3ファイルがHEADと一致。8件のignoreと子側`--ignored`を撤回 | 0 |
| `powershell -NoProfile -File scripts/test-live-windows-runner.ps1` | PS 5.1、fake 4ケース成功（承認なし拒否、eframe 1件のみ選択、失敗・0件exit伝播）。証跡`target/live-runner-fake-9378fd38afab43158603de22d79e934c/` | 0 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | workspace全体の整形・差分検査成功 | 0 / 0 / 0 |
| `powershell -NoProfile -File scripts/test-lib-full.ps1`（`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`） | 13,231件実行、13,231 passed、0 failed、59 skipped。386.420秒。nextest表示は5 slow / 6 leaky、成功台帳発行 | 0 |
| 同runのJUnit照合 | 復帰した本体headless 8/8件が実行・成功。初回失敗38/38件も実行・成功 | 0 |

再検証ログ：`target/egui037-stage0a/main-run-review-fix.log`、
`target/test-lib-full/runs/20261016T023209103Z-93afae00/`（stderr / JUnit）。
照合結果は`target/egui037-stage0a/review-fix-summary.txt`。
成功台帳は`target/test-lib-full/c0d801cb5e990b7261aa0fd993f542f44708627e5901f2faf9ec92f49b2457ef.json`、
source IDは`64f86f8ec4a6a3af23d4bab7a788aa7828dffe5fabaf6f9681839c2160f7ed8a`。
成功後は本台帳への結果追記のみで、Rust・runnerを変更していない。

RAW fixture欠落の37失敗は解消した。ページめくり1件も今回は再発せず成功したため、
追加の単独再実行は行わない。初回全件失敗→初回単独成功の履歴は上節に保持し、
描画の原因調査・修正はこの段の範囲外として実施していない。

## 未実施

- 全live試験：**未実施**。段0aでは実行禁止。専用入口もfake Cargo以外では起動していない。
- `scripts/test-full.ps1`：**未実施**。依頼どおり設計担当の判断へ残す。
- `test-script`追加featureや既存ignored hardware/probe：**未実施**。今回の変更は属性と実行入口のみ。
- ページめくり描画の修正・原因調査：この段の範囲外。今回の全件再検証では再発なし。
- 確認用製品build：**対象外**（テスト・script・文書のみの変更）。

## 設計書との差異

- 第4.2版の`test-full.ps1:85`は旧行番号。現行nextest runnerの
  `--run-ignored default`を維持し、属性で分離した。gateのコマンド本体は変更していない。
- 本体libのHWNDを使うheadless 8件は設計レビューに従い自動gateへ戻し、追加分離はeframe 1件だけとした。
- eframeの親子で同じテストを呼ぶため、ignore追加に伴い子側へ`--ignored`が必要。
  期待値・試験ロジック・timeoutは変更していない。
