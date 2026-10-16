# egui 0.37 更新・段1a 検証台帳

2026-10-16、`egui-036` / 開始HEAD `f65b606c1f0d48d64c091042e1d14e93b520d5a2`。
[更新計画第4.2版](egui-0.37-update-plan.md)の段1aを実装した。
Rust 1.99.0、egui 0.33.3、wgpu 27.0.1を維持する。
製品起動・live試験・`test-full.ps1`・snapshot更新・コミット・ブランチ操作・pushは行わない。
開始時に`src/video/dsp/mod.rs`だけ変更表示があり、Gitの内容差分はなかった。
対象外としてバイト列を保護し、整形後もSHA-256
`2f099cbba3855cbce0f5ae1a5034b9ccea668b0ff6cfae98e8b5bca2a09a8ae0`の一致を確認した。

## 依存・コメント

- `src` / `crates` / `tools` / `tests` / `build.rs`の`wgpu_hal`、`windows_058`、
  `create_texture_from_hal`、`as_hal`を検索し、削除前の直接利用がないことを確認した。
- ルート`Cargo.toml`の`wgpu-hal`と`windows_058`別名を除去した。
  `wgpu = "27"`、本体の`windows = "0.61"`は変更しない。
- 同manifestの旧D3D11→wgpu import説明と`src/app.rs`の
  `wgpu_render_state`説明を現行用途へ訂正した。動画本体はD3D11 native presenter内で
  shared resourceを開き、wgpuは静止画縮小・比較・パノラマ・HUD overlayに使う。
- `src/video/decoder.rs`、`src/video/gpu_renderer/{mod,d3d11_device}.rs`も、
  旧D3D12共有・毎フレーム新規出力・keyed mutex未使用という説明を現行へ訂正した。
  VPP intermediate → CopyResource → BGRA8 pool、`OpenSharedResource1`、
  `OpenSharedFence`、Context4のSignal/Waitとwriter/reader keyの所有を説明する。
  3ファイルはHEAD比で`//`コメント行を除いた全行の完全一致を機械確認した。
  明示された過去のwgpu OOM記録は維持した。
- `cargo tree --offline ... -i wgpu-hal`と`cargo metadata --locked --offline`で確認した。
  `Cargo.lock`の差はmimageviewerの依存辺`wgpu-hal`と`windows 0.58.0`の2行削除だけ。
  895 packageの集合・版・checksumは不変。間接`wgpu-hal 27.0.4`と`windows 0.58.0`は残る。
  `windows_062`などの別名追加はない。

証跡は`target/egui037-stage1a/wgpu-hal-tree.txt`、`metadata-full.json`。

## accessibilityの対象と方法

registry rootは
`C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\`。
`egui_kittest-0.37.0/src/accessibility.rs`と`egui-0.37.0/src/accessibility.rs`を読んだ。
検査対象はvisible、非read-onlyのinput role。Button、CheckBox、ColorWell、ComboBox、
TextInput系、Slider、SpinButton、RadioButton、Link、menu/list option、Tab、TreeItem、
Splitter等を含み、非空labelまたはplaceholderが必要になる。Unknownの独自canvasや
read-only表示、native Win32入力はこの検査と分ける。

本体の入力生成、IME helper、空文字checkbox、画像・ベクターボタン、custom interact、
既存tooltip/hoverを静的に確認した。テスト専用の仮名や検査無効化は追加しない。

- `Response::labelled_by`で既存のlabel・見出し・行/列header・選択肢と関連付けた。
  このAPIは0.33.3と0.37.0の両方に残る。可視widget・ID・描画順を増やさない。
- `ui_helpers::set_accessibility_label`は`Context::accesskit_node_builder`で
  nodeのlabelだけを設定する。0.37の`Response::accessible_name`と同じ方式であり、
  role、numeric/text value、checked、disabled、range、actionは再構築しない。
  `widget_info`全体の上書きによる既存metadataの欠落を避けた。
- 標準`on_hover_text`は0.37で既存tooltipから名前を補うので、そのためだけの個別変更はしない。
  独自`HoverTipExt`の通常・暗色・disabledとtest-script観測入口の計4経路には、
  hoverの有無にかかわらず既存tooltipを空名へ補う共通処理を追加した。
  明示名・`labelled_by`を優先し、Unknown/Labelは命名しない。tooltipの位置・描画は維持する。
- 新しい可視文字、tooltip、placeholderは追加していない。読み上げ専用名は実際の操作の意味を表し、
  ComboBoxの現在選択値を部品名で代用しない。IME・focus・key処理は変更しない。

### 対応箇所

数はループの生成数ではなく静的な命名対象箇所。共通slider macroは定義を1箇所として数え、
local-effect ComboBoxは20の製品呼出箇所、resume matrixは6入力として数える。
同じ入力への行/列2本の関係は1箇所。合計**220箇所**。
加えて上記4つの共通hover入口を対応した。共通描画の呼出・macro展開・動的loopにより
実行時の部品数はこれより増え、既存名を持つ部品にはhover fallbackの名前を重ねない。

| 変更ファイル | 箇所数 | 対応 |
| --- | ---: | --- |
| `src/ui_main.rs` | 34 | toolbar/facet等のComboBox 20、DragValue 11、本の名前等のTextEdit 3 |
| `src/ui_folder_pane.rs` | 1 | drive選択 |
| `src/global_search_ui.rs` | 3 | お気に入り・種類・検索対象 |
| `src/local_adjust_effect_ui.rs` | 20 | local-effect ComboBoxの意味名を共通helperへ渡す |
| `src/ui_adjustment_panel.rs` | 13 | slider macro 2、個別slider 8、enable checkbox、適用量DragValue、LUT選択 |
| `src/ui_text.rs` | 19 | font sample、数値15、ComboBox 2、stamp-size slider |
| `src/ui_crop.rs` | 5 | 縦横比とX/Y/W/H |
| `src/ui_sns_split.rs` | 1 | 継ぎ目の割合 |
| `src/ui_raw.rs` | 1 | 同時現像数 |
| `src/ui_dialogs/preferences/pages.rs` | 87 | 表示・動画・入力・AI・再開等の既存項目ラベルと意味名 |
| `src/ui_dialogs/preferences/name_colors.rs` | 3 | カテゴリ・HEX・色well |
| `src/ui_dialogs/preferences.rs` | 1 | 場所 |
| `src/ui_dialogs/archive_cache_manager.rs` | 1 | 元ファイル行のcheckbox |
| `src/ui_dialogs/cache_manager.rs` | 1 | 最終更新からの日数 |
| `src/ui_dialogs/collections.rs` | 3 | 作成・改名 |
| `src/ui_dialogs/export_batch.rs` | 1 | 長辺指定 |
| `src/ui_dialogs/fav_add.rs` | 1 | 表示名 |
| `src/ui_dialogs/favorites_editor.rs` | 1 | 表示名header |
| `src/ui_dialogs/smart_folder_editor.rs` | 4 | 名前・条件・並び方 |
| `src/ui_dialogs/subfolder_expansion.rs` | 7 | サイズ・日付・日数・深さ |
| `src/ui_dialogs/tag_editor.rs` | 1 | タグ名header |
| `src/ui_dialogs/thumb_quality.rs` | 2 | サイズ・品質 |
| `src/ui_fullscreen.rs` | 5 | slideshow描画ラベル4、export長辺指定1。viewport経路は変更しない |
| `src/video/native_presenter/overlay_draw.rs` | 5 | filter・Anime4K budget・LUT・適用量・共通grade slider |

`src/ui_helpers.rs`には共通命名処理と非対話回帰1件も追加した。
hoverなしでの命名、既存名/label関係の優先、disabled、Slider/DragValueのrole・数値・範囲、
checkboxのchecked保持を検査する。
共通grade sliderは8呼出、調整slider macroは5呼出、address入力は通常/disabledの2分岐を持つ。
上の220を実行時の全input node数とは扱わない。

### 変更しない箇所・残件

- **可視文字追加を必要とする製品部品／利用者相談が要る既知箇所: 0。**
  今回見つけた製品入力は既存label・既存tooltip・読み上げ専用の意味名で対応できた。
- その他の標準button/checkbox/radio等の非空可視名、`.text`付きslider、`from_label`のComboBox、
  既存hint付き入力、既存標準tooltip、上流color pickerの名前は維持した。
  `ui_erase` / `ui_conceal` / `ui_view_trim`、music/metadata/composition editor、
  Everything検索、Remote PIN欄等はこのため変更不要。dialogは全55ファイルを確認した。
- `crates/egui-ime`は呼出元が意味名を所有する描画helper。共通の仮名は付けない。
  `egui_focus_policy` / `keymap`等のテスト用raw入力は製品描画とは分ける。
- 製品描画と非共通のfixtureは段3で正式graphと一緒に確認する。
  `ui_main`の`ten-sort-rows` / `collection-sort-rows` / `probe_combo`、
  `ui_folder_pane`の`folder_pane_drive_combo_test`のHarness直書き4箇所、
  local-effectの`repair_quality_theme_regression` 1箇所が既知。
  後者は共通helperの新引数へ空文字を渡し、テストだけの名は付けない。
  さらに`ui_main`の`sort-popup-test` / `probe` / `from_id_salt(text)`3箇所の
  raw Context geometry probe計5箇所は現在kittestの検査外。追加相談は不要。
- 0.33の`labelled_by`は関係追加、0.37は自身のlabelをclearして関係を優先する。
  `.text`なしSliderの内部数値nodeへの連結も0.37の`slider.rs:644-652`が補う。
  旧版ですべてのinput nodeが命名済みと実証したとはしない。
- 静的監査には動的状態や全画面の取りこぼしがあり得る。
  **段3の0.37切替時にkittestの実検査が出す残りを補完する。**
  Unknown role等を含むスクリーンリーダー操作全体の完全性を保証する監査ではない。

ファイル別の変更不要理由・命名箇所・fixture位置は
`target/egui037-stage1a/{main,adjust,dialog}-audit.md`。
独立レビューは別contextの`gpt-6.1-sol` / `xhigh`が実施し、P1/P2指摘なし。
実行検証は実装担当が所有し、レビュー担当は再実行しない。

## 検証

ログは`target/egui037-stage1a/`。通常feature・追加featureなし。
`MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`を使用し、
`UPDATE_SNAPSHOTS`は未設定の別processで比較する。
[基準台帳](egui-0.37-baseline.md)の両integration targetと50 lib filterを個別実行する。
結果は下表へ追記する。

| 検査 | 結果 | exit code |
| --- | --- | --- |
| `cargo fmt` / `cargo fmt --check` | 成功。15.678 / 16.276秒 | 0 / 0 |
| `python scripts/check_ui_glyphs.py` | dangerous glyph 0件 | 0 |
| `git diff --check` | 成功 | 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功、60.264秒、既存警告あり | 0 |
| `cargo check --workspace` | ランチャーの前提実行ファイル3本の不足でbuild script停止。236.763秒 | 101 |
| 変更moduleの焦点nextest | 2,115 passed、0 failed、11,178 skipped。実行86.816秒、compile込み594.101秒 | 0 |
| `cargo test -p mimageviewer --test ui_snapshot` | 181 passed、0 failed/ignored。実行25.53秒、compile込み514.890秒 | 0 |
| `cargo test -p mimageviewer --test book_composition_editor_snapshots` | 11 passed、0 failed/ignored。実行8.70秒、compile込み15.646秒 | 0 |
| 基準台帳のlib snapshot全50 filterを個別に通常比較 | 全103 passed、各filter 0件選択なし、全command成功 | 各0 |
| PNG SHA-256照合 | 追跡445枚すべて開始時と一致。更新0枚 | 0 |
| `scripts/test-lib-full.ps1` 初回 | 13,234実行、13,233 passed、1 failed、59 skipped。実行489.383秒、compile込み686.950秒 | 100 |
| 初回失敗1件の単独nextest | 1 passed、13,292 skipped。実行0.307秒 | 0 |
| 最終`cargo check -p mimageviewer --bin mimageviewer-core` | 成功、17.872秒 | 0 |
| 参考`cargo check --workspace --exclude mimageviewer-launcher` | 成功、22.537秒。workspace全体成功の代替とはしない | 0 |
| 最終`cargo fmt --check` / glyph / diff検査 | 成功。glyph 0件、16.561 / 2.346 / 0.247秒 | 各0 |
| `scripts/test-lib-full.ps1` 最終再確認 | 13,234実行、13,233 passed、同じ1 failed、59 skipped。実行426.565秒、compile込み515.774秒 | 100 |
| 確認用`build-dev.ps1` | 未実施。全件gateが未通過で、検証用buildの前提を満たさない | — |

workspace checkの不足物は`target/release/`の`mimageviewer-core.exe`、
`mimageviewer-remote.exe`、`mimageviewer-epub-pdf.exe`。
`crates/launcher/build.rs`がこれらを`include_bytes!`の実入力として要求する。
製品のコンパイルエラーやMSBuild失敗ではない。代用exeの配置やbuild規則の変更は行わない。
この結果をworkspace全体成功とは扱わない。

焦点filterは`ui_helpers` / `ui_dialogs` / `local_adjust_effect_ui` /
`ui_adjustment_panel` / `ui_text` / `ui_crop` / `ui_raw` / `ui_sns_split` /
`ui_main` / `ui_folder_pane` / `global_search_ui` / `ui_fullscreen` /
`video::native_presenter::overlay_draw`のmodule部分文字列の和集合。
完全なcommand・exit・時間は`focused-results.json`、出力は`focused-nextest.log`、
JUnitは`focused-junit.xml`。追加した`ui_helpers::accessibility_names_tests`の1件も
JUnit上で実行・成功を確認した。slow表示1件は既存still-seek geometry回帰で、86.746秒で成功した。

snapshotのcommand・時間・exitは`snapshots-results.json`、件数は`snapshot-summary.json`、
全50 filterは`lib-snapshot-filters.json`。PNGの開始時ハッシュは`snapshots-before.json`。
integration 192件＋lib 103件＝295テストが成功した。
snapshot/focused後のvideo 3ファイルの追補はコメントのみであり、非コメント行の完全一致を確認したため、
同じ描画・実行コードに対する成功結果を再利用する。最終check/buildと全件再確認は追補後のソースで行う。

### lib全件初回の失敗

`app::tests::still_window_mode_key_tests::detached_native_video_navigation_syncs_main_selection`が
`src/app/tests.rs:62195`の`media navigation worker did not finish`で失敗した。
helperは10,000回のpoll/yieldで完了を待つ。テストrunnerのtimeoutではなく、当該helperのassertである。
全件runでは1.055秒、同一条件の単独nextestでは0.307秒で成功した。
並列負荷とworker schedulingの関係は未検証であり、原因や既知baseline失敗とは断定しない。
テスト上限・fixture・ignore・製品動作・detached経路は変更しない。

初回runは`target/test-lib-full/runs/20261016T043618443Z-12065cab/`。
source IDは`9c7b446b66b8e3dcb86f6cd9b6ddf8a1e058034956c4b12c088b0ec5d81ee72d`。
`target/egui037-stage1a/full-initial-results.json`と同runの`stderr.log` / `junit.xml`が初回失敗の証跡。
単独のcommand・結果は`full-failure-isolated.log` / `full-failure-isolated.exit.txt`。
初回全件を成功扱いせず、コメント整理後の全件を一度再確認する。
コメントの残件は独立レビューで再確認し、`gpu_renderer/mod.rs`の経路図も含めて解決した。
非コメント行は3ファイルともHEADと完全一致。再検索で残るwgpu記述は非依存説明と明示された過去のOOM記録だけ。

### 最終再確認・残る検証上の障害

同じ既定条件の全件再確認でも、上記1件だけが同じassertで失敗した（1.125秒）。
他の13,233件は成功し、59件は既存ignore。単独成功をもって全件成功へ置き換えない。
原因は未確定であり、全件負荷との因果や段0b時点でも起きた失敗とは断定しない。
対象テストは直接のUI描画を行う前のnavigation worker完了待ちで失敗している。
`src/app/tests.rs`とdetached/viewportの処理経路は変更していない。
依頼の境界に従って、そのfixture・待機方式・製品経路をこの段で変更して通すことはせず、
設計担当へ調査・扱いを返す。3回目の全件再実行や上限緩和は行わない。

最終runは`target/test-lib-full/runs/20261016T045208590Z-fb6eb63c/`。
source IDは`ff427ad2057ae8f9cf8f70d34b680fef463f9a6928bcaa9057f0e98eabf964fb`。
`lib-full.log` / `full-results.json`と同runの`stderr.log` / `junit.xml`を保存した。
両runのJUnitを`full-summary.json`で照合し、新規命名回帰1件と、基準台帳にある
ページめくりの過去間欠候補1件も両runで成功したことを確認した。
失敗した全件の成功台帳は発行されていない。

最終core / launcher除外check・fmt・glyph・diffのcommand/exit/時間は
`remaining-checks-results.json`。全件再確認後、445 PNGのSHA-256と対象外DSPのバイト列を再照合して一致した。
その後は本台帳の結果追記のみでRust・manifest・lock・fixture・scriptは変更していない。

確認用buildは、リポジトリの検証用build前提（自動検証green）が満たされないため未作成。
成果物を起動したり、存在しない新buildの起動を利用者に依頼したりしない。
**実装差分はレビュー可能だが、workspace全体checkとlib全件gateは未通過であり、段1aの受入れ完了とはしない。**

## 設計書との差異・未実施

機能・構造の設計変更はない。`widget_info`全体の再設定に代えて、同じ公開AccessKit node APIで
名前だけを設定しmetadataを保った。意味名の静的対応は見積りより広がったが、
見た目・製品操作・detached述語・viewport配送/生成/終了・永続データは変更していない。
変更は計33ファイル（31 source/manifest/lock＋計画のリンク・本台帳）で、12〜30ファイルの規模見積りを3ファイル上回る。
初回の未通過workspace checkと、全件gateの再現した失敗・確認用build未実施は上記のとおり残す。
live / ignored hardware、実0.37 graphでの検査、製品起動、`test-full.ps1`、
snapshot更新は未実施。依頼に従い未コミットで設計担当へ渡す。
