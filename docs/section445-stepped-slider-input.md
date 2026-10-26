# §1.445 step付きスライダーの数値手入力

対象は `gpuslider-1445`、v4.8.2 (`da9a64cd5`) からの差分。
master / 必要ならv4.8.3への移植用。製品exeの起動・commit・他worktreeの編集は行わない。

## 原因と再現

環境設定の `page_gpu_memory` は `u32` の0〜100に `step_by(5.0)` を指定する。
0%は無制限なので、未確定の「2」を丸めて0%として適用してはいけない。

`vendor/egui/src/widgets/slider.rs::value_ui` はローカルの `f64` をDragValueへ渡し、
戻り値を `set_value` → `SliderCore::rounded` でstepへ丸める。
`drag_value.rs::EditState` は入力文字列とそのときのローカル値を保持する。
「2」を入力するとEditStateの値は2、呼出元の設定値は丸め後の0になる。
次frameのDragValueは `edit_state.value == old_value` が不成立なので、
外部変更と判定して入力文字列を捨て、0から編集を再開する。

実 `Context::run_ui` と `Event::Text` のheadless入力で再現した。
修正前は「2」→「5」の確定値が5、最初の「2」だけで設定値が0になった。
初期11回帰の実行は3成功・8失敗、exit 101。記録: `target/gpuslider-before.log`。
失敗は分割入力・途中値・blur・削除と桁修正・Esc・明示入力とclamp・既存27・兄弟範囲。
ドラッグ、矢印刻み、step無しlive更新は元から通る。

ローカルregistryのegui 0.33.3でも同じ分割入力テストを実行し、
25 / 50 / 80 / 100すべて成功した。プローブは
`target/gpuslider-1445/egui033/` に置き、製品やregistryを編集していない。
0.33.3のDragValueは `String` だけを保持し、値の一致による破棄判定がない。
Slider側のstep丸めとローカル値の橋渡しは旧版にもある。
旧0.33.3で途中値非適用テストも実行すると、「2」で0を適用して失敗する（exit 101、
`target/gpuslider-egui033-partial.log`）。旧版は文字列を保持するが、途中の数値丸めはある。
今回は文字列保持の退行を直すだけでなく、利用者指定どおり途中値を確定しない境界にする。
この判定の導入は上流 [PR #8403](https://github.com/emilk/egui/pull/8403)、
merge commit [`d802a982ce959839d92f4f6718fdffa41143110e`](https://github.com/emilk/egui/commit/d802a982ce959839d92f4f6718fdffa41143110e)。
StringをEditStateへ置換し、不一致でtextを破棄する差分を確認した。
[0.36.2の変更一覧](https://github.com/emilk/egui/releases/tag/0.36.2)にも収録され、
ローカルregistryの0.36.2にも同じ判定がある。0.37で初めて入った変更ではない。
mIVでの初発版がv4.8.0かは未確認。今回実行したのはv4.8.2のvendorと
0.33.3のプローブで、過去の製品exeは起動していない。

## 修正の境界

- 共通の `Slider::value_ui` でstepがあるときだけ、既存の
  `update_while_editing(false)` を使う。整数Sliderの暗黙のstep=1も対象。
  追加のpending値・独自入力欄・mIV各所の回避設定を作らない。
- 入力中の文字列はDragValueが保持し、Enter / フォーカス離脱で値が変わったときだけ
  Sliderが丸めて設定へ渡す。数値を変えずに閉じた場合（同値の再入力を含む）は
  既存のoff-grid値も保持する。
  step付きでは `update_while_editing(true)` よりこの確定境界を優先する。
  mIVの明示的な `update_while_editing` 呼出しはない。
- DragValueの編集終了時はEditStateを残さない。従来はEscのframeで残った文字列が
  翌frameのlost-focus経路で適用され得た。外部値変更による古い入力の失効判定は保持する。
- step付きSliderの毎frameの `Always` 処理は範囲clampだけにし、既存値のstep丸めをやめる。
  既存27%を表示・焦点移動・Enter・Escで25%へ変更しない。
  step無しは従来のset_value経路を維持し、fixed_decimals等の既存値丸めを変えない。
  保存処理・DB・設定の適用/取消の所有関係は変更しない。
- スライダーと数値欄のドラッグ・矢印の増減は既存経路のまま即時反映する。
  5%刻み、明示0/100、範囲clampを維持する。

非同期化やmodalの追加は不要。短い文字編集は既存DragValueが所有し、
既存の確定待ちモードで途中値と設定値の組合せ自体をなくせる。
UIへの待機・I/O・スキャンは追加しない。キーは既存widget内の編集操作であり、
新しいアプリ操作・KeyAction・IMEの処理は追加しない。

対象外: RangeSlider（mIVから未使用）、step無しのmax_decimals等による別の丸め経路。
この修正を根拠にそれらの編集方式や全設定の刻みを変更しない。

## 兄弟欄

`rg -n step_by src` は54行。そのうち24行はRust iteratorの `.step_by`、
30行はSliderの呼出しまたはSliderを作るマクロ定義だった。
`post_filter.rs` もpalette生成のiteratorである。
後者はすべて共通修正の対象。マクロの展開数はこの30に含めない。

| 代表欄 / source | 範囲 / step | 修正前のfake-input | 修正後 |
| --- | --- | --- | --- |
| GPU上限 / preferences/pages.rs | u32 0〜100 / 5 | 「2」で0、「25」が5 | 25 / 50 / 80 / 100、途中値非適用 |
| Auto cache時間 / preferences/pages.rs | 10〜100 / 5 | 25の文字は保持。ただし「2」で10を適用 | 25、途中値非適用 |
| Auto cacheサイズ / preferences/pages.rs | 0.5〜10 / 0.5 | 0.5の文字は保持。ただし「0」で0.5を適用 | 0.5、途中値非適用 |
| 文字の縦位置 / preferences/pages.rs | -4〜4 / 0.25 | 「1.2」が次frameで「1.25」に書き戻る | 1.25、入力文字列保持・途中値非適用 |
| 色塗りの検出半径 / ui_adjustment_panel.rs | 0.1〜4 / 0.1 | 0.8の文字は保持。ただし「0」で0.1を適用 | 0.8、途中値非適用 |

兄弟4件は未修正のregistry版0.37.0を使う独立プローブでも調べた。
段階ごとの文字列検査は3成功・1失敗（文字の縦位置）、exit 101。
`target/gpuslider-1445/egui037/` と `target/gpuslider-siblings-before.log` に証跡を残す。
範囲clampでDragValueの記憶値と丸め値が一致する場合は入力文字列が壊れない。
全step欄でGPU上限と全く同じ破壊が起きる、という意味ではない。

代表条件は同じvendor widgetを直接描画するテストで検査する。
全30欄の実アプリ操作を行ったという意味ではない。
ほかはlocal_adjust_effect_ui、ui_adjustment_panel、ui_conceal、ui_erase、
ui_helpers、video/native_presenter/overlay_draw。iteratorは変更対象でない。

## 検証台帳

製品exe・OS入力・通常設定を使わない。UI snapshotの基準画像は更新しない。
本体lib全件は依頼どおりClaudeCodeが後で実行する。

| コマンド | exit / 結果 |
| --- | --- |
| `cargo test --manifest-path vendor/egui/Cargo.toml --lib widgets::slider::tests -- --nocapture`（修正前の初期11件） | 101、3成功・8失敗 |
| `cargo test --offline --manifest-path target/gpuslider-1445/egui033/Cargo.toml --lib stepped_slider_accepts_digits_separately -- --nocapture` | 0、旧0.33.3で分割入力成功 |
| 同プローブの `--lib stepped_slider_does_not_publish_partial_text -- --nocapture` | 101、旧0.33.3も「2」で0を適用。比較用の期待失敗 |
| `cargo test --manifest-path vendor/egui/Cargo.toml --lib` | 0、134成功（追加17件）。`target/gpuslider-egui-final.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0、最終差分。`target/gpuslider-check.log` |
| `cargo test -p mimageviewer --test ui_snapshot` | 最終0、195成功・画像変更なし。`target/gpuslider-ui-snapshot-serial.log`。初回101と前提ビルドの切分けは下記 |
| `.\scripts\test-focused.ps1` | 2、vendor/共通変更を検出して選択実行なし。wrapperの終了値も明示child PowerShellで確認 |
| `cargo fmt --check` | 0 |
| `rustfmt --check --edition 2024 vendor/egui/src/widgets/slider.rs vendor/egui/src/widgets/drag_value.rs vendor/egui/src/widgets/slider_tests.rs` | 0 |
| `python scripts/check_site_cards.py` | 0、49 cards、違反0 |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 0、normal feature setのcore / Remote / EPUB workerを生成。DLL配置・PE依存検査（runtime=4、pe=3）成功。`target/gpuslider-build-dev.log`。製品起動なし |

UI snapshotの初回はテスト開始前にexit 101。FFmpeg bindgenでAV/stack overflow、
TurboJPEGでCMake/MSBuildのexit 1があった。`target/gpuslider-ui-snapshot.log`。
`CARGO_BUILD_JOBS=4` と `MSBUILDDISABLENODEREUSE=1` の再実行ではFFmpegは成功し、
TurboJPEGは失敗した（`target/gpuslider-ui-snapshot-retry.log`）。同じビルド先を直接
CMakeで直列実行すると成功、並列4は失敗。成功後にCargoで並列4を再実行しても失敗した
（`target/gpuslider-ui-snapshot-final.log`、この名前のログは成功結果ではない）。
Windowsではcmake crateがNUM_JOBSを明示的な `--parallel N` へ渡すため、
最終的に `CARGO_BUILD_JOBS=1` / `MSBUILDDISABLENODEREUSE=1` をそのコマンドのprocess内だけへ
設定し、全195件を成功させた。MSBuildの並列経路が失敗する詳細原因は未特定。
製品・依存物・個人の環境設定には変更を加えていない。確認用buildにも同じprocess環境を使う。
build-devは残存MSBuild processの待機をsupported flagで無効化し、自分のnative buildを直列化する。

追加回帰は独立レビューで指摘された外部値変更、implicit整数step、
standalone deferred DragValueのEsc、未確定入力後の数値矢印も含む。
適用ボタンがクリックされたframeで値が確定済みになることも追加検査し、
次frameの描画を待たずUIを閉じるconsumerへ値を渡せることを確認した。
step無しの既存小数桁丸めを保持する回帰も追加した。
独立 `gpt-6.1-sol / xhigh` レビューのP2（step無しにも及んだ未操作丸めの変更）を修正し、
該当初期値1.234→1.2の回帰の失敗→成功と、レビュー時点の133件成功を再利用した再確認で追加指摘なし。
その後の適用ボタン回帰はテストだけの追加で、製品コードはレビュー時点から変更していない。
実アプリでの適用・取消・再読込と実IMEは未確認。

## 確認用バイナリの引き渡し

出力は `target/dev-runtime/mimageviewer-core.exe` と同じディレクトリの
Remote / EPUB worker、必要なDLLとEffeTune bundle。
初回直列buildはcore 35分08秒、Remote 3分08秒、EPUB worker 1分56秒。
既存プロセスを停止しない `-PreserveRuntime` を使い、成果物は起動していない。

利用者がこのworktreeのPowerShellで起動するときは、インストール版・トレイ常駐版を
先に終了する（single-instance mutexを共有する）。このbuildはportableではなく、
通常の `%APPDATA%\mimageviewer` の実設定・データを更新し得る。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

実アプリで残る代表確認は、GPU上限を全選択し「2」「5」を別々に入力して25へ確定、
その後のOK保存・取消・開き直し。入力途中で0へ置き換わらないことを確認する。
