# IME 処理済みキーの根本修正（I1 共有化・I2）

2026-10-14。音楽ビデオより先に本体へ入れる独立した移植。
範囲と完了条件は [移植ブリーフ](brief-ime-process-keys-port.md)。元の設計・観測・レビュー記録は
`musicvideo` ブランチの `docs/music-video-export-plan.md` I1 / I2 節であり、以下の実機結果は
設計 lead の先行確認の記録。今回の worktree ではアプリを起動しない。

## 原因と利用者の決定

画像注釈の本文で日本語を確定してすぐ Enter を押すと、改行が入らなかった。
Windows の `VK_PROCESSKEY` が winit の `NamedKey::Process` となり、egui-winit 0.33.3 が
物理キーへ fallback して、IME の処理済み Enter / Escape / Tab / Space / 文字キーを通常キーとして
再配送していた。確定 Enter の誤配送を避ける本体の 300ms Enter 除去は、利用者が新たに押した
Enter まで捨てていた。除去のない multiline 欄では逆に確定 Enter が改行になった。

利用者は 2026-10-14 に A（狭い修正）/ B（全面修正）/ C（見送り）の **B** を選択した。
確定直後だけ Enter / Escape / shortcut の結果が異なる旧挙動を、間を空けた通常時と同じに揃える。
候補の確定とアプリ操作を同じ押下で二重実行しないこと、通常入力を時間で捨てないことが不変条件。

## I1：egui-ime への共有化

`crates/egui-ime` は egui のみに依存し、Context 内の単一 plugin が viewport 別 composition、
取消の空 preedit 補完、single-line helper、focus contract / retry、read-only snapshot / pending event
投影を所有する。本体 `src/ime_focus.rs` は既存 API の facade とし、診断・文字秘匿・logger・
keyboard owner の記録を保持する。全域 Tab traversal 無効の製品方針も共有 helper に移し、
登録はホストが行う。音楽ビデオの UI・ラボ・core は移植しない。

I1 段 2a（`367f9c9d3`）と段 2b の本体部分（`fde3e819e`）は先行独立レビュー ACCEPT。
抽出時は挙動を維持し、I2 で不要な補正を撤去する。

## I2：段階と現在の設計

| 段階・移植元 | 設計・決定 |
| --- | --- |
| 段 1：`282f173c4` / `1cbe1888f` | egui-winit 0.33.3 を vendor に取り込み、既定無効の `MIV_IME_KEY_TRACE` で winit 入力と実際の egui 出力を観測。文字と Space は秘匿し、ファイル I/O は worker に置く。入力挙動を変更しない |
| 段 2a：`62bc9f47c` / `29367ba4d` | Process press / repeat を Key・Text・clipboard producer の前で遮断。本体 text_mode / comic_lab の Enter 除去、共有 plugin の composing Esc 除去、helper の IME 専用 Tab lock を撤去。実配送関数 `append_keyboard_input` で回帰検証する |
| 段 2b：`291dbf8e5` / `efce16eb0` | 300ms grace と timestamp、pending-event 有無版 API を撤去。今 pass の Ime 列を反映した composing と、queue 最終状態の投影へ統一。Win32 queue / native 生キー公開は 0xE5 press / repeat を生成境界で捨て、release は保持する |
| 本家への整合（今回追加） | Windows の `allow_ime` が true のとき、Process を遮断して `pressed_processed_physical_keys: HashSet<PhysicalKey>` に記録し、対応する release も遮断する。allow_ime=false なら状態を変えず通す。通常 press の物理キーの release は通す |

段 2a の「release を通し、集合を持たない」判断は履歴であり、現在は本家との整合が優先する。
本家 [PR #7967](https://github.com/emilk/egui/pull/7967) の egui-winit 0.34.0 と同じ判定・状態。
[本家実装](https://raw.githubusercontent.com/emilk/egui/0.34.0/crates/egui-winit/src/lib.rs) に揃え、
egui 更新時は `vendor/egui-winit` と path patch をこの差分ごと削除する。
処理済みキーの EventResponse も本家同様、repaint=false / consumed=wants_keyboard_input とする。
追加の focus 喪失時の集合リセットや時間窓は設けない。trace は既定無効で保持する。

focus のキー所有と復帰は別責務。直前 pass の helper 欄に対する recovery claim は維持する。
復帰は pointer 無し・他 widget focus 無し・composing または通常 Enter / Escape press 無しで決める。
retry は helper が viewport / widget / pass 番号で所有し、次 pass の同条件でだけ消費する。
呼び出し側の初回 focus latch には書き込まない。欄の消失や正当な focus 移動を跨いで復帰しない。

composition は `Inactive` / `Composing` / `UnfocusedComposition` の単一 enum。
viewport 消滅時はその状態を破棄し、focus 喪失時は shortcut claim を終了するが、取消の終端待ちは保持する。
後続の Disabled / 空 Commit に空 preedit を補い、非空 Commit は確定として残す。
生存する兄弟 viewport の読み取りは状態を書き換えない。

簡素化：短い入力変換なのでモーダル化や窓の再生成は不要。既存 plugin と focus contract を使い、
時刻・重複 gate を減らす。focus 喪失直後の一律取消は、後続の正当な確定まで消すため採用しない。
egui 本家の `remove_ime_incompatible_events`、`return_key(None)`、矢印 lock、全域 Tab 方針、
空 preedit 補完は維持する。detached predicate / registry / 生成 / placement は変更しない。
稀な trace I/O 失敗は記録を停止し、再試行・復旧・終了待ちを増やさない。

## 先行実機確認・独立レビューの記録

設計 lead が 2026-10-14、隔離データの本体注釈本文・検索欄と scratchpad のラボで確認した。

| 確認 | Google 日本語入力 | MS-IME |
| --- | --- | --- |
| 処理済みキーの入力 | Process press として到着。段 2a 後は全 18 press の egui 出力なし | Process または press 自体の配送なし。候補一覧 Enter・部分確定 Ctrl+↓も通常 press を漏らさない |
| 注釈本文：確定だけ / 確定→すぐ Enter / Esc 取消 / 候補 Tab | 確定で改行なし、直後 Enter で改行 1 つ、取消・focus 保持 | 同じ期待結果を確認 |
| 検索：確定→すぐ Enter | 検索実行 | 42ms 後の Enter で検索実行 |
| 検索：確定→すぐ Esc | 個別の記録なし | 35ms 後の Esc で欄を閉じる |
| 変換中に別窓へ focus 移動（efce16eb0 後） | 確定文字が 1 つだけ残る | 同左 |

focus 移動で Disabled だけ届く取消列は実機では未観測で、実 TextEdit buffer の単体テストで固定した。
detached、他の PC / IME、今回の本家整合後の実機挙動は未確認。
段 1 の Space 秘匿 P2、段 2a の実配送テスト P3、段 2b の focus 喪失取消 P2 と retry / 文書 P3 は
修正後に先行正式独立レビュー ACCEPT。段 2b の設計は第 5 版で ACCEPT。
これらは移植元の評価であり、今回の移植と本家整合の独立レビュー・検収は設計 lead が別途依頼する。

## 今回の移植・検証記録

移植元 8 コミットをパス限定で順に適用。衝突なし。`fde3e819e` の Cargo.lock 差分は
音楽ビデオ UI の依存追加だけなので除外。本体用の既存変更を保持し、音楽ビデオ専用ファイルは追加しない。
本家整合の配送・状態遷移テストは release 遮断、allow_ime=false、非対応 release、repeat、複数キー、状態分離を検証する。

検証ログと command / exit code / 件数は `target/ime-port/verification/*.log` / `*.json`。
通常 feature、test / dev profile、`RUST_MIN_STACK=67108864`。アプリ起動は行わない。

| コマンド | 結果 |
| --- | --- |
| `cargo fmt --check` / vendor `rustfmt --edition 2024 --check` / `python scripts/check_ui_glyphs.py` | exit 0 / exit 0 / 危険 glyph 0 |
| `cargo test -p egui-winit --lib` / `cargo test -p egui-ime` / `cargo test -p comic_lab` | 23 / 22 / 9 passed、失敗なし。egui-ime doc-test は 0 件 |
| `cargo test -p mimageviewer --lib ime` | 448 passed / 既存 ignored 2 / 0 failed。子プロセスによる 2 件の再掲は加算しない |
| 同 `focus` / `keyboard` / `text_mode` / `ime_focus` / `egui_focus_policy` | 118 / 42 / 3 / 12 / 5 passed、失敗なし |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0 |
| `cargo tree -i egui-winit` | vendor 0.33.3 の 1 系統 |
| `scripts/test-release-build-safety.ps1` | PASS |
| release 前提：core / remote（embedded-web-assets）/ EPUB worker | exit 0。881.19 / 46.54 / 25.96 秒。起動なし |
| `scripts/test-full.ps1 -SuppressCrashDialogs` | exit 101（1,718.22 秒）。本体 lib 12,648 passed / 57 ignored / 既知 App サイズ制限 1 failed。他 workspace・統合・doc target は 1,270 passed / 7 ignored / 0 failed |
| vendor egui / egui-wgpu（winit）/ eframe（wgpu）の無 filter lib test | 25 / 11 / 18 passed、失敗なし。原 full gate が未到達の範囲を別実行で補完。egui-winit 23 件の成功も再利用 |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0（413.12 秒）。MSBUILDDISABLENODEREUSE=1、通常 feature / dev-runtime の core・remote・EPUB worker、FFmpeg / VCRT / EffeTune を staging。成果物の起動なし |

初回の本体テスト / check は turbojpeg-sys の MSBuild 並列ビルドで終了し、テスト失敗とは区別した。
同じ CMake 構成の単一並列診断は警告・エラー 0、exit 0（166.58 秒）。
`MSBUILDDISABLENODEREUSE=1` と単一並列で依存を準備し、feature / profile / 最適化設定は変更しない。
初回 `ime` 実行の 6 失敗は RAW サンプル未配置によるもの。既存 worktree から実ファイルをコピーし、
`setup-raw-samples.ps1` で全 27 ファイルの SHA-256 / サイズ一致を確認して再実行すると上表どおり成功した。
FFmpeg DLL は lib テスト前に `target/debug/deps` へ配置した。
移植されたソース 21 ファイルの照合では、master 側の Everything UI snapshot 追加だけが移植元との差として残り、
その既存テストを維持した。既存の全体ゲート失敗を修正・抑止するコード変更は加えない。

全体ゲートの初回は CARGO_BUILD_JOBS=24 の並列 compile 中、mmap が Windows 1455
（ページングファイル不足）で失敗し、テストは未実行。結果をログに保持したまま、
CARGO_BUILD_JOBS=1 / RUST_TEST_THREADS=4 で再実行すると上表の全件結果になった。
App は 110,264 bytes（移植元の master 単独記録と同じ、上限は 110,000 未満）。
失敗の `history_transition_storage_keeps_app_stack_footprint_bounded` は変更せず、全体 gate 成功とは扱わない。
本体 test exe の `--list` 12,706 登録を完了ログと照合し、未確認 0 を確認した
（`full-lib-coverage.json` / `full-summary.json`）。子プロセス summary は重複加算せず、
追加 vendor / egui-winit を含む検証範囲は計 13,995 passed / 64 ignored / 既知 1 failed。
stack overflow は起こらず、crash-dialog 抑止の error mode は元の 0x00008001 へ復元された。

確認 build の CMake 依存は同 profile の `turbojpeg-sys` / `libwebp-sys` を単一並列で先に準備した
（281.69 秒）。本体の build-dev は CARGO_BUILD_JOBS=24、feature / 最適化設定を維持して成功した。
今回の port / 本家整合に対する実機確認と独立レビューは未実施。Google / MS-IME で、
注釈本文の確定だけでは改行なし・確定→すぐ Enter で改行 1 つ、検索欄の確定→すぐ Enter / Escape、
Alt 復帰後の Enter / Escape、別窓への focus 移動を設計 lead が隔離データで確認する。
成果物は `target/dev-runtime/mimageviewer-core.exe`。通常起動は
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
既定では実利用中の `%APPDATA%\mimageviewer` を更新し得るため、インストール版 / tray 常駐版を先に終了する。
今回の隔離確認では同じ core へ `--data-dir .\target\dev-runtime\data` を渡す。

Git 管理領域が作業ディレクトリ外のため、この実行環境では元 worktree の index / branch への書き込みが拒否された。
コミット履歴は `target/ime-port/git` に保存し、元ブランチへの反映は保留する。
