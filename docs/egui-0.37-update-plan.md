> 状態: 設計案 (2026-10-16、実装担当 gpt-6.1-sol / high の読み取り調査)。独立レビュー前。目標は利用者決定により egui 0.37 系 (当初案 0.36 から変更)。正本の backlog は §1.382。

# egui 0.37 更新の影響調査と段階設計

## 0. 調査範囲と結論

調査日: 2026-10-16  
対象: `G:\home\mimageviewer-egui036`、ブランチ `egui-036`  
調査時 HEAD: `05b6a15de26897369a35e5aa00377d174ca3760d`

読み取り専用で、リポジトリ、指定された更新履歴の要約、Cargo registry の 0.33.3／0.37.0 ソース、既存の Rust 1.99 check ログを照合した。ファイル変更、依存解決、ビルド、テスト、製品起動は行っていない。以下の実行時影響はソースからの見立てであり、実機確認結果ではない。

目標は egui 系 0.37.0 とする。実装開始時に 0.37.x の新しいパッチ版を選ぶ場合、その版のソースで本書の該当判断を再照合する。今回、ネットワークによる最新パッチ確認は行っていない。

主な結論は次のとおり。

1. **単純な依存バージョン変更では済まない。** App の非表示時処理、root Ui、入力イベント、texture delta、wgpu surface の API を移行する必要がある。
2. **`vendor/egui-winit` は現時点では削除できない。** 本家 0.37.0 にも、現在の Hidden／Forwarded によるキー配送履歴管理との差が残る。
3. **`vendor/eframe` も削除できない。** README にある非表示窓の修正以外に、再描画所有管理、起動時通知、texture delta 配送、診断 API が存在する。
4. **`ui_fonts.rs` の計測は既に `ttf-parser`。** `ab_glyph` が直接残るのはフォント選択プレビューである。補正式とプレビューの更新を分ける。
5. **wgpu の主対象は HUD overlay と静止画 GPU 処理。** 動画本体は現在 D3D11 native presenter で描画する。D3D11→wgpu の旧共有経路を前提に設計しない。
6. **`raw_scroll_delta` の撤去が追加の重要変更。** 修飾キーのイベント化だけでは、mIV の入力移行は完了しない。
7. **snapshot は現ツリーで445枚。** 「約350枚」を作業量の基準にしない。
8. **Rust 1.99 の f32 警告は更新後に再評価する。** 既存ログの該当警告は445件だが、0.37 の `Stroke::new` は幅の型が `f32` に変わり、多くは呼出側を変更せず解消する見込みである。

### 参照表記

本家ソースの `egui-0.37.0/src/...:line` 等は、次の registry root からの相対パスを示す。

```text
C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\
```

mIV の行番号は調査時 HEAD に対するもの。件数は特記しない限り `src`、`crates`、`tests`、`tools` の Rust ソースを検索した**一致行数**であり、コメントを含む。AST による呼出数ではない。

## 1. 設計上の不変条件

- 非表示になっても、既存のトレイ復帰、非同期完了、Remote、保存終端、メディア制御を停止させない。
- UI が描かれない間、古い入力を再実行しない。
- viewer context ごとの入力、IME、cache、texture、要求世代、worker の所有を維持する。
- 同じ物理押下について、配送済み press の release を失わない。
- popup が所有した wheel は、終了フレームと平滑化残量を含めて背面へ漏らさない。
- 生成した texture delta は、表示 surface の有無にかかわらず、正しい renderer へ配送する。
- 日本語、CJK、記号、絵文字の fallback と、ユーザー指定フォント設定を保持する。
- 動画の D3D11 描画、HUD の wgpu 描画、静止画の GPU 処理を混同しない。
- snapshot の更新によって、入力・到達性・所有権の回帰を承認しない。
- リリース済み設定・ユーザーデータを、更新を理由に初期化しない。

簡素化は、既存 owner と producer への集約を優先する。今回の短い入力変換や描画 API 更新を新しいモーダル処理にする理由はない。既にモーダルである変換・保存・パスワード処理はその境界を維持し、非表示対応のために並行操作を増やさない。

guard、時間窓、retry、追加 repaint、silent fallback による症状対処は追加しない。まれな失敗は既存方針どおりログと通知を基本とし、更新専用の回復状態機械は設けない。

## 2. 前提・更新履歴要約との相違

| 前提・要約 | ソースで確認した内容 | 設計への反映 |
|---|---|---|
| egui-winit は消すだけで済む見込み | 現 vendor は本家の HashSet を Hidden／Forwarded に置換。0.37 本家は HashSet のまま | vendor を維持して差分移植 |
| eframe のローカル変更は非表示窓 scheduler 修正 | 現ソースには、それ以外の製品・診断変更がある | README だけを移植台帳にしない |
| ui_fonts は ab_glyph 外形計測に依存 | 外形・行メトリクス計測は `ttf-parser`。プレビューが `ab_glyph` | 補正式とプレビューを別評価 |
| `CentralPanel::show` が廃止 | **Context を受ける形**がなくなり、`show(&mut Ui, …)` は存在 | 名前の一括撤去ではなく親 Ui の移行 |
| `AsId` が削除 | 0.37.0 では非推奨として定義・再公開される | 削除済みと記載しない |
| Window はタイトルバーだけで移動 | 0.37 は `WindowDrag::OnTouch` が既定。タッチ検出時は全面、それ以外はタイトルバー | 入力機器別に検証 |
| cut/copy/paste は余分な修飾キーなしだけ | X/C/V は `command_only()`。Windows の Shift+Delete／Ctrl+Insert／Shift+Insert は別条件 | 全経路が exact match になったとは扱わない |
| snapshot 約350枚 | 追跡 PNG は445枚 | 更新・目視工数を修正 |

根拠: `egui-winit-0.37.0/src/lib.rs:656`、`:1538`、`egui-0.37.0/src/containers/panel.rs:1377`、`src/id.rs:8`、`src/lib.rs:543`、`src/containers/window.rs:17`。

## 3. 影響件数の概観

| 検索対象 | 一致行／ファイル | 代表箇所 |
|---|---:|---|
| `\bId::new\s*\(` | 571行／84ファイル | `crates/egui-ime/src/lib.rs:159` |
| `ui\.id\(\)` | 99行／5ファイル | `src/ui_music_panels.rs:706` |
| `\bSidePanel::` | 9行／5ファイル | `src/ui_folder_pane.rs:218` |
| `\bTopBottomPanel::` | 23行／9ファイル | `src/ui_main.rs:7400` |
| `\bCentralPanel::` | 339行／60ファイル | `src/ui_main.rs:17580` |
| `show_viewport_(immediate\|deferred)` | 38行／8ファイル | `src/ui_fullscreen.rs:22139` |
| `raw_scroll_delta` | 58行／16ファイル | `src/app.rs:58760` |
| `Event::MouseWheel\s*\{` | 103行／20ファイル | `src/video/native_presenter/render_core.rs:9751` 付近 |
| `impl eframe::App` | 6行／6ファイル | `src/app.rs:97613` |
| `fetch_update\(` | 製品・テスト14行／12ファイル、別途 vendor 1件 | `src/log_writer.rs:105` |

再現用の基本検索:

```powershell
rg -n '\bId::new\s*\(|ui\.id\(\)|\bAsId\b' src crates tests tools -g '*.rs'
rg -n 'SidePanel::|TopBottomPanel::|CentralPanel::|show_viewport_' src crates tests tools -g '*.rs'
rg -n 'RawInput|raw_scroll_delta|Event::MouseWheel|\.modifiers' src crates tests tools -g '*.rs'
```

`.run(` は1274一致行／130ファイルだが、Harness、worker 等を含む。これを `Context::run` の移行件数として扱わない。

## 4. 項目別設計

### 4.1 文字描画、補正、fallback、絵文字

#### 本家の変更

0.37 は skrifa によるフォント解析・hinting、vello_cpu によるラスタライズ、harfrust による shaping を使う。

- outline／色付き glyph の選択: `epaint-0.37.0/src/text/font_face.rs:96`
- 色付き glyph の描画: 同 `:190`
- harfrust のデータ: 同 `:332`
- サイズとメトリクス: 同 `:596`
- フォントサイズからの倍率は units-per-em 基準: 同 `:88`
- hinting／subpixel binning は既定で有効: `epaint-0.37.0/src/text/mod.rs:77`
- 最終的な行内配置: `epaint-0.37.0/src/text/text_layout.rs:1100`

`FontTweak` の `scale`、`y_offset_factor`、`y_offset` は残る。hinting、hinting target、variation coordinates 等が追加されている。単に補正 API がなくなる更新ではない。根拠: `epaint-0.37.0/src/text/font_tweak.rs:6`。

#### mIV の影響箇所

| 箇所 | 内容 |
|---|---|
| `src/ui_fonts.rs:4`、`:7`、`:13` | user／HUD／toolbar family |
| `src/ui_fonts.rs:235` | fallback と family の構築 |
| `src/ui_fonts.rs:360`、`:373` | 主フォントの計測と視覚中央補正 |
| `src/ui_fonts.rs:420` | 主フォント、toolbar、HUD の登録 |
| `src/ui_fonts.rs:546`、`:567` | fallback 補正 |
| `src/ui_fonts.rs:652`、`:687`、`:699` | ttf-parser の bbox／行メトリクス／bitmap bounds |
| `src/ui_font_catalog.rs:6`、`:78` | ab_glyph による選択プレビュー |
| `src/video/native_presenter/bottom_hud.rs` | galley を共有する HUD 文字計測・配置 |
| `build.rs:700`、`src/comic_stamp.rs` | Twemoji SVG スタンプ |
| `tests/ui_snapshot.rs` | フォント比較 snapshot |

#### 対応方針

- `miv-user-text`、`miv-hud-text`、`miv-toolbar-text` の分離を維持する。
- HUD の「固定フォント」は選択 UI フォントから独立したままにする。ただし renderer が変わるため、旧版とのピクセル一致までは保証しない。
- 選択フォント、face index、利用者の縦位置調整値を保持する。保存値を自動リセットしない。
- bbox 計測を理由なく skrifa へ全面交換しない。ttf-parser の計測は引き続き利用可能だが、**新 renderer の実メトリクスとの一致を検査する**。
- 行メトリクス、fallback の中央合わせ、toolbar の固定3.5pt補正を別々に評価する。全部残す／全部削るという判断をしない。
- 色付き glyph は outline と実際の可視範囲が異なり得るため、emoji の outline 優先補正は再検証する。色付き glyph を無視した bbox だけで位置を確定しない。
- hinting による変化と補正の誤りを分けるため、検証用 fixture で hinting 有無を比較する。製品設定を増やすことは前提にしない。
- `ui_font_catalog` のプレビューは、実 UI と異なるラスタライザを使い続けると選択見本がずれる。選択フォントを worker で準備する既存構造を維持しつつ、本体と同じ文字レイアウトを使う方式を優先検討する。worker での preview 描画方式は独立した小設計・レビュー対象とする。
- `comic-core` の `ab_glyph_rasterizer` は注釈焼き込み用の別経路であり、egui 更新だけを理由に変更しない。

#### default_fonts と自動 fallback

`egui` 単体の default feature は空だが、**eframe の default には `default_fonts` が残る**。現 root の指定方法では直ちに全 default font が消えるわけではない。

根拠:

- `egui-0.37.0/Cargo.toml:62`
- `eframe-0.37.0/Cargo.toml:61`
- `epaint-0.37.0/src/text/font_definitions.rs:110`

さらに eframe は system font fallback を既定で有効にする。

- `eframe-0.37.0/src/epi.rs:384`
- `eframe-0.37.0/src/native/epi_integration.rs:260`

初期移行ではフォント feature と `system_font_fallback` を明示し、mIV の既存 fallback 順序を基準にする。新しい自動探索を無条件で重ねない。採用する場合は、mIV が登録した family と native HUD の独立 Context も含め、描画差・読み込み費用を別途評価する。

`monochrome_emoji_fonts` は `default_fonts` と別である。旧組み込み emoji font と同じ集合が default で入るとは限らない。根拠: `epaint-0.37.0/src/text/font_definitions.rs:146`。

#### Twemoji

Twemoji は文字 fallback ではなく、注釈の SVG スタンプ資産である。0.37 の色付き文字対応で代替されない。`vendor/twemoji`、`build.rs` の `emoji_svgs.rs` 生成、保存済みスタンプの識別を維持する。

**見た目の変更:** 字面、太さ、字幅、kerning、合字、折り返し、行高、baseline、emoji の色が変わり得る。利用者相談・目視受入れ対象とする。

### 4.2 App／Context と非表示時処理

#### 本家の契約

`App::logic` は UI の前と、非表示中に repaint 要求がある場合に呼ばれる。非表示中は egui pass を実行せず、`Context::input` の events／time 等は最後に表示した frame の値である。

根拠:

- `eframe-0.37.0/src/epi.rs:152`
- `eframe-0.37.0/src/native/epi_integration.rs:349`
- 同 `:384`
- `egui-0.37.0/src/context.rs:1174`

**logic は常時 tick ではない。** 必要な worker completion／期限による wake は引き続き必要である。

visible な子 viewport がある場合、親も UI 実行対象になる判定が存在する。root 非表示だけで子の処理も止まるとは限らない。根拠: `eframe-0.37.0/src/native/wgpu_integration.rs:705`。

#### mIV の影響箇所

- App 入口と終了: `src/app.rs:97631`、`:97792`
- 現在の巨大な更新処理: `src/app.rs:94680`
- トレイ処理・media wake: `src/app.rs:94969`
- 非表示でも完了すべき処理: `src/app.rs:97661` 以降
- post-render の保存・変換・外部ツール境界: `src/app.rs:97700` 以降
- viewport: `src/ui_fullscreen.rs:22139`、`:22364`、`:24296`、`:24745`
- native overlay の独立 Context: `src/video/native_presenter/render_core.rs:12505`
- theme: `src/os_theme.rs:248`
- 本体以外の App 実装: `tools/*_lab/src/main.rs`、`crates/vst3-host-tester/src/main.rs`

#### 分離方針

| 所有する処理 | 移行先・条件 |
|---|---|
| worker の結果受信、取消・保存終端、Remote の完了、トレイ要求 | `logic`。UI 入力を読まない service 境界にする |
| キー、wheel、pointer、focus、IME pass 診断 | `ui`／対象 viewport の pass |
| menu、dialog、panel、viewport 登録、paint | `ui` |
| UI を表示したことが前提の起動 ACK・外部ツール許可 | 実際の描画側に残す |
| context ごとの結果採用 | 現在の registry／mount owner を維持 |
| native 動画フレーム描画 | 既存 presenter thread のまま |

`update_frame` を丸ごと `logic` に置く案、旧 update をそのまま `ui` と改名する案は採らない。前者は描画契約違反、後者は非表示時の進行停止を起こす。

`logic` に古い events を読ませない。`frame_counter`／pass counter が進むことを、非表示 service の進行条件にしない。反対に、visible 時の複数 pass で完了処理や副作用を重複実行しないよう、既存要求の採用・消費境界を使う。新しい実行済み bool を重ねない。

post-render の `poll_sidecar_restore` 等は、移動によって終端フレームの modal gate を早く外さないことを個別に確認する。

#### API 移行

- 通常描画: `Context::run_ui`。root Ui をそのまま子描画へ渡す。
- 入力・状態だけの headless test: `run_pass` を選択可能。
- Context 専用 API の検査: 必要な場所だけ `root_ui` を使う。
- viewport callback: `&mut Ui` を受け、Context が必要なら `ui.ctx()`。
- `style`／`style_mut`／`set_style`: Context は `global_style` 系へ。Ui 自身の style は一括置換しない。
- `Ui` の Deref により、旧 forwarding method の一部は Context に解決される。削除一覧だけを見て全呼出を変更しない。

根拠: `egui-0.37.0/src/context.rs:950`、`:983`、`:1024`、`:2599`、`:4662`、`src/ui.rs:77`。

eframe の default renderer／feature も変わる。現在の `on_exit(&mut self, Option<&eframe::glow::Context>)` は glow feature に依存するため、採用 feature と同時に直す。終了処理本体は維持する。根拠: `eframe-0.37.0/src/epi.rs:227`、`src/app.rs:97792`。

**挙動変更:** 正常系は従来維持を受入条件とする。非表示時に処理を止める変更は承認しない。

### 4.3 Panel 統一と計算高さ

#### 影響箇所

- menu／toolbar／facet／検索: `src/ui_main.rs:7400`、`:10291`、`:12590`、`:15558`
- 下部情報: `src/ui_main.rs:20434`
- folder pane: `src/ui_folder_pane.rs:218`
- CentralPanel: `src/ui_main.rs:17580`、`:19160`
- 固定 overlay: `src/ui_text.rs:3002`、`:3331`
- 共通 geometry 検査: `tests/support/scroll_layout.rs:6`、`:31`、`:65`

本家 `Panel` は `show(&mut Ui, …)` を使い、幅・高さ別の設定が `default_size`／`min_size`／`max_size`／`exact_size` 等へ統一される。

根拠: `egui-0.37.0/src/containers/panel.rs:252`、`:372`、`:425`。

#### 方針

- top／bottom／side／central の順序と、残り領域の所有を一つの親 Ui に揃える。
- Context 直の panel を置換する際、子 Ui を不必要に追加して margin／id scope を変えない。
- resizable な folder pane は新しい slide、drag-to-close、double-click の既定を明示評価する。単なる型名置換で新操作を導入しない。
- Area＋Frame＋ScrollArea は Panel とは別物として維持する。固定 HUD を新 Panel へ全面交換しない。
- separator 幅修正は Panel の寸法計算に関するもので、mIV 独自の行高・折り返し・sink の不整合を自動修正しない。

新 Panel の collapse／drag は `panel.rs:337`、`:454`、`:741`、separator の扱いは同 `:348` 以降を根拠に検証する。

#### backlog §1.380

| 残件 | 影響箇所 | 0.37 更新での扱い |
|---|---|---|
| クリップボード失敗カードの可変高 | `src/ui_dialogs/clipboard_capture.rs:378`、`:490` | 固定仮想行と折り返しの問題。自動解決とは判断できない |
| 長い保存基点名 | `docs/calculated-panel-height-audit.md` 残件節、fullscreen の見開き調整描画 | 固定本文予算の問題。別 owner／layout 設計が必要 |
| 注釈右詳細・左一覧 | `src/ui_text.rs:2745`、`:2761`、`:3252`、`:3348` | sink、最低高、本文高の不一致は残る |
| 切り取り・表示トリムの枠 | 同監査の残件節 | 実割当矩形で継続確認 |
| 構成 editor の行 | `src/app/book_composition_editor.rs` | §1.242 の既存作業へ残す |

0.37 の ScrollArea も親の available size を上限にする。`max_height` 指定だけで親領域を確保できるようにはなっていない。根拠: `egui-0.37.0/src/containers/scroll_area.rs:842`。

**見た目・操作:** separator、余白、panel の collapse 操作に差が出得る。独自固定パネルの機能削減は行わない。

### 4.4 フォーカス、wheel、drag、Ctrl+Q

#### 影響箇所

- IME helper／focus owner: `crates/egui-ime/src/lib.rs:242`、`:305`、`:329`
- Tab 方針: `crates/egui-ime/src/tab_focus.rs:23`、`:33`
- dialog Enter／Escape: `src/app.rs:22538`、`:22543`
- 一覧 wheel: `src/app.rs:58738`
- menu wheel 消費: `src/ui_main.rs:1464`、`:1471`
- 前面 popup の残量終端: `src/ui_fullscreen.rs:25754`、`:27352`
- touch scroll: `src/ui_main.rs:601`、`:657`
- drag の利用例: `src/ui_music_panels.rs:706`、native HUD の seek／音量部品

#### 方針

- Area／Window／ScrollArea が自動 focus を取らなくなる変更は、container 自体と入力部品を区別する。入力部品の focus 回復を一括削除しない。
- Ctrl+矢印が focus 移動しなくなることは改善だが、Tab traversal 無効という mIV の製品方針は残す。0.37 も Tab を focus 移動として扱う。根拠: `egui-0.37.0/src/memory/mod.rs:590`。
- dialog helper は残し、IME の処理済み押下と通常 Enter／Escape を区別する。
- 本家 ScrollArea の wheel 消費だけに頼らない。raw wheel を使う App の一覧処理、menu、fullscreen 前面 owner の順序を維持する。
- `Sense::drag` は上の widget に隠れた領域から drag を開始しない変更を前提に、sink／overlay／slider の重なりを検査する。click を奪う新しい背景 Sense は追加しない。
- press 後に widget 外へ出た操作は、クリック取消と drag 終端を分けて検査する。release 後の移動、新しい press、disabled 復帰を対象にする。

ScrollArea の drag scroll は `DragScroll::OnTouch` が既定である。根拠: `egui-0.37.0/src/containers/scroll_area.rs:224`、`:286`。

現在 mIV に `.drag_to_scroll` の明示指定は見当たらず、本家既定に依存する画面でマウス drag scroll が減る可能性がある。旧版で使えた画面は、必要箇所に `DragScroll::Always` を明示する方向で調整する。製品全体でマウス操作を削る判断はしない。

#### Ctrl+Q

0.37 は end-pass で `Options::quit_shortcuts` を消費し、viewport Close を送る。

- `egui-0.37.0/src/memory/mod.rs:337`
- `egui-0.37.0/src/context.rs:2935`

mIV は keymap と独自終了・トレイ方針を持つ。各製品 Context の初期化で本家既定 quit shortcut を明示的に無効化し、既存入力 owner に揃えることを推奨する。Ctrl+Q を新しい終了操作として導入するなら、別の利用者相談事項とする。

### 4.5 非推奨 API 削除と IME

#### 本家と mIV の重なり

本家は TextEdit の IME 所有者を `Memory::owns_ime_events` で管理し、IME 表示と候補窓位置を改善している。

- `egui-0.37.0/src/memory/mod.rs:1064`
- `egui-0.37.0/src/widgets/text_edit/builder.rs:617`、`:1049`、`:1382`
- `egui-winit-0.37.0/src/lib.rs:1291`

これは、次の mIV 固有責務をすべて代替するものではない。

- アプリ shortcut／dialog の composition gate
- viewport 別の read-only composition snapshot
- Disabled 等で空 Preedit が来ない取消の補完
- helper-managed field の focus contract
- 製品全体の Tab 方針
- Win32／native presenter の Process press 除去

#### 撤去・保持の判断

| 対象 | 方針 |
|---|---|
| 300ms grace、timestamp、旧 pending-event 有無版 API | 現ツリーでは撤去済み。更新時に復活させない |
| text_mode の確定 Enter 除去 | 撤去済みのまま |
| helper の IME 専用 Tab lock | 撤去済みのまま |
| 全域 Tab traversal 無効 | 保持 |
| `dialog_enter_pressed`／`dialog_escape_pressed` | 保持 |
| viewport 別 composition owner | 保持 |
| 空 Preedit 補完 | 本家 Disabled 処理と実 TextEdit buffer 回帰を照合して判断。自動撤去しない |
| focus recovery／次 pass 限定 claim | 本家の改善と重複する条件を試験で特定してから縮小 |
| Win32 0xE5 press／repeat の生成除外 | 保持 |
| winit Hidden／Forwarded | 本家未解決のため保持 |

本家 TextEdit は `ImeEvent::Enabled | Disabled` を、その場で composition text に置換する対象にはしていない。空 Preedit 補完が不要になったとは読めない。根拠: `egui-0.37.0/src/widgets/text_edit/builder.rs:1417`。

#### egui-winit を削除できない理由

0.37 本家では:

1. `allow_ime=false` なら処理済みキー判定を通過する。
2. Process で physical key を HashSet に入れる。
3. 非 Process release で集合を remove して遮断する。
4. IME 許可解除時に集合を clear する。
5. focus 喪失処理で modifier は clear するが、この配送履歴の型付き管理はない。

根拠: `egui-winit-0.37.0/src/lib.rs:483`、`:656`、`:1258`。

現在の vendor は、通常 press→Process repeat、および Process→通常 repeat→Process repeat を含め、**一度配送した press の事実を release まで保持**する。これを本家へ戻すと、不変条件を失う。

移植対象は時間窓ではなく、同じ State／physical key が所有する Hidden／Forwarded と focus 終端である。trace は既存の既定無効・文字秘匿・非同期 I/O を保持する。

#### Window の操作

0.37 の `WindowDrag::OnTouch` は、マウスではタイトルバー、タッチでは全面移動になる。旧来の本文 drag 移動が必要なダイアログでは `drag_area(WindowDrag::Anywhere)` を明示できる。

マウスで本文から動かす操作を減らすかどうかは利用者相談事項とする。本文選択、slider、touch scroll との衝突も確認する。

### 4.6 Modifiers イベント化と OS 直読み

#### 本家の変更

`RawInput.modifiers` がなくなり、`Event::ModifiersChanged` で InputState の修飾キーを更新する。Key／Pointer／MouseWheel のイベントごとの modifiers は残る。

根拠:

- `egui-0.37.0/src/data/input/event.rs:94`
- `egui-0.37.0/src/input_state/mod.rs:434`
- `egui-winit-0.37.0/src/lib.rs:548`

#### 影響箇所

| 箇所 | 対応 |
|---|---|
| `src/app.rs:59309` | OS と InputState の resync |
| `src/key_input.rs:2459`、`:2466` | 合成 RawInput の注入 |
| `src/video/native_presenter/render_core.rs:2716`、`:9780` 付近、`:12505` | native event queue と独立 Context |
| `src/ui_fullscreen.rs:67268` | テストの RawInput modifiers |
| `src/ui_adjustment_panel.rs:10985` | 修飾キー付きテスト |
| `src/ui_erase.rs:815` | stale modifier 対策 |
| `src/ui_conceal.rs:742`、`:1341` | OS level と drag |
| `src/ui_adjustment_panel.rs:12014`、`:12510` | Space pan、Ctrl による筆切替 |
| `src/keymap.rs:7468` | OS 直読みの chord 判定 |
| `src/keyboard_input.rs:305` | consume 入口 |

#### 方針

- native／合成入力の producer は、modifier の変化を**対象入力より前**にイベント列へ入れる。
- 最終 modifier を frame 末尾に一つ追加するだけの移植はしない。同一 frame の Ctrl down→key→Ctrl up を壊す。
- OS resync は離散 Key イベントの modifiers を書き換えない。level state と過去の edge を分ける。
- FS の OS 直読みを、0.37 更新だけで撤去しない。イベント化は配送されなかった KeyUp を保証する変更ではない。
- focused permit、viewport ごとの物理ラッチ、Enter／NumpadEnter、左右 Ctrl 等の既存境界を維持する。
- 新しい `egui::Key` を理由に keymap の保存名や KeyAction を変更しない。
- modifier key 自体の Key イベントを、通常 shortcut として誤消費しない。
- Windows 固有 Key の追加は対応可能な入力の拡大であり、自動的な製品割当追加にはしない。

`consume_key`／`key_pressed` は残るが、mIV の exact chord 判定は引き続きイベントごとの modifiers を正本にする。

cut/copy/paste は `command_only()` 化した X/C/V と、Windows の Insert／Delete 系を別々に検査する。Process filter の後に新本家 clipboard 処理を通し、旧変換コードを丸ごと上書きしない。

**挙動変更:** 既存 keymap、編集中の Ctrl／Shift／Alt、通常／FS／native 間の結果を維持する。OS 直読み撤去は今回の既定範囲にしない。

### 4.7 Id、scope、永続化

#### 本家ソースでの確認

- `Id::unique` が存在し、`Id::new` はそれを呼ぶ非推奨 API。
- `AsId` は非推奨として残る。
- `AsIdSalt`／`IdSalt` が導入される。
- `Id::with` の内部ハッシュ構成は0.33と異なる。
- `Ui::id` は `scope_id` へ。
- `From<&str> for Id` は0.37.0の当該実装に存在しないため、文字列の `.into()` を移行策にしない。

根拠: `egui-0.37.0/src/id.rs:8`、`:91`、`:102`、`:121`、`:142`、`src/id_salt.rs:43`。

**`Id::new` を残しても child Id の値まで旧版と同じとは限らない。** 非推奨警告を保留することは永続互換対策にならない。

#### 方針

- root identity: `Id::unique`
- 親の中だけで一意な塩: `IdSalt`／`AsIdSalt`
- 既存の widget lookup と生成側を同時に移行する。
- `ui.id()` は機械的に `unique_id()` にせず、既存 scope の意味なら `scope_id()` を使う。
- Accessibility tree の container unique Id を、子入力部品の Response Id と取り違えない。
- `Id::new` 警告は依存切替段の短期間に限って残せるが、後続の Id 移行段で解消する。crate 全体の `allow(deprecated)` は置かない。

代表箇所:

- `crates/egui-ime/src/lib.rs:159`
- `src/ui_music_panels.rs:706`、`:1563`
- `tests/ui_snapshot.rs:443`
- `tests/book_composition_editor_snapshots.rs:405`

#### 永続データ

現 manifests では eframe の `persistence` を明示有効化しておらず、mIV の主要な窓配置・設定は独自保存経路が所有する。`App::persist_egui_memory` の既定が true であることだけから、egui memory がディスク保存されていると判断しない。

本家の保存処理は persistence feature の内側にある。根拠: `eframe-0.37.0/src/native/epi_integration.rs:470`。

実装段で feature graph と独自シリアライズを再確認する。現状の確認範囲では、egui Id の変更を理由とする settings.db 全体の migration／削除は不要であり、行ってはならない。

もしリリース済み経路で egui memory／Id 値を保存していることが追加で見つかれば、旧読込・変換と互換試験を設計してから進める。session 内の折りたたみ・scroll 状態の変化と、ディスク上の利用者データ消失を区別する。

### 4.8 vendor パッチの移植台帳

分類:

- **(a)** 本家に同等の仕組みがあり、対応する旧差分を外せる。
- **(b)** mIV の責務・保証が残り、移植が必要。
- **(c)** API 変更等で旧実装が不要。必要な機能は新 API へ移す。

README／PATCHES だけでなく、registry 0.33.3 と現 vendor のソース差を確認した。参考として、eframe の src 差分は9ファイル、約2270行追加／124行削除、egui-wgpu は9ファイル、約2004行追加／192行削除であり、README の一項目だけでは表せない。

#### vendor/egui

| 項目 | 分類 | 移植方針 |
|---|---|---|
| `released_buttons_with_click_counts` | (b) | `vendor/egui/src/input_state/mod.rs:1485`。本家の新 click 判定から射影する形で移植。raw release 順、非 click、focus、discard 後の非再生を保持 |
| `discard_pending_scroll` | (b) | 同 `:595`。新 WheelState の未配送残量に合わせて実装し直す。旧2フィールドの名前だけ置換しない |
| `miv_requested_repaint_delay` と feature | (b) | `vendor/egui/src/context.rs:1794`、`Cargo.toml:53`。本家 repaint observer は同じ同期 getter の代替ではない |
| 上記の回帰テスト | (b) | 新 event／state 構造へ移植し、条件を弱めない |

本家の wheel state は `egui-0.37.0/src/input_state/wheel_state.rs:54`、InputState の所有箇所は `src/input_state/mod.rs:228`。本家に現在の終了境界と同じ public な残量破棄 API は見当たらない。

`PATCHES.md` に repaint witness が未記載のため、移植時に追加する。

#### vendor/egui-winit

| 項目 | 分類 | 移植方針 |
|---|---|---|
| Process 入力を除く基本機構 | (a) | 本家0.37の実装を土台にする |
| Hidden／Forwarded、release の無条件終端、focus clear | (b) | `vendor/egui-winit/PATCHES.md` 冒頭の3差分を維持 |
| 診断 trace と文字秘匿 | (b) | `src/ime_key_trace.rs` と hook。既定無効を維持 |
| 配送・State・網羅回帰 | (b) | 本家の modifiers／clipboard producer を含む実経路へ接続 |
| 旧300ms補正 | (c) | 既に撤去済み。再導入しない |

#### vendor/eframe

| 項目 | 分類 | 根拠・方針 |
|---|---|---|
| #7905 非表示窓 repaint の基本修正 | (a) | 本家 `eframe-0.37.0/src/native/run.rs:266` 以降を採用。旧 backport 部分は重ねない |
| dirty／painting、surface generation、outer phase の scheduler | (b) | `vendor/eframe/src/native/run.rs:45`、`:118`、`:425`。本家の hidden fix と別責務 |
| 非ゼロ resize のみ inline paint を許す provenance | (b) | 同 `:1282`、`native/wgpu_integration.rs:978`。message dispatch と通常 paint の境界を維持 |
| 最初の visible commit と Frame getter | (b) | `src/epi.rs:736`、`native/epi_integration.rs:305`。mIV の起動 owner が利用 |
| startup window observer | (b) | `src/startup_window_observer.rs:8`。`src/startup_windows_diag.rs:271` が利用 |
| root／child の max_texture_side 統一 | (b) | `native/wgpu_integration.rs:735`、`:1235`。本家にも生成時指定はあるが、旧保証との同等性を回帰で確定するまで外さない |
| 非描画・消滅 viewport の texture delta 配送 | (b) | 同 `:828`、`:1313`。本家 pending delta と統合して移植 |
| immediate child の screenshot | (b) | 旧 Event 配送方式は(c)だが、子窓撮影機能は callback 方式で保持 |
| test-script window witness／capture・focus 診断 | (b) | `src/miv_test_script_window_witness.rs`、backend の callback-local scope |
| manifest、feature、Win32 feature、standalone patch | (b) | 新本家 manifest を基準に必要分だけ再適用 |

本家も pending delta を持つが、mIV の全保証と同等とはいえない。

- root の蓄積: `eframe-0.37.0/src/native/wgpu_integration.rs:795`
- immediate の蓄積: 同 `:1290`
- painter は surface lookup 後に delta を適用: `egui-wgpu-0.37.0/src/winit.rs:552`、`:569`

viewport が先に消える箇所や surface がない場合を含め、生成済み delta を失わない経路を維持する。

#### vendor/egui-wgpu

| 項目 | 分類 | 移植方針 |
|---|---|---|
| mipmap allocation／再生成／sampler | (b) | `src/renderer.rs:712`、`:777`、`:1151`。本家は managed texture を1 levelで作る |
| 公開 MipmapGenerator／texel count | (b) | `src/mipmap.rs`、`src/mipmap.wgsl`、`src/lib.rs:35`。比較・panorama が利用 |
| atlas diagnostics／renderer identity／texture_size | (b) | `src/atlas_diag.rs`、`src/renderer.rs:793`。新 delta map に合わせる |
| 非描画 frame の delta 適用 | (b) | `src/winit.rs:523`、`:586`。本家 pending delta と配送 owner を統合 |
| `set_window(id, None)` が当該 surface のみ破棄 | (b) | 本家0.37も `surfaces.clear()`。兄弟 surface の保証を移植 |
| startup GPU stage／root present observer | (b) | `src/startup_diagnostics.rs`、`src/lib.rs:322`、`src/winit.rs:563` |
| screenshot／readback 診断 | (b) | `src/capture.rs:207`。本家 callback 方式へ適応 |
| shader の空行等、意味のない旧差分 | (c) | 再適用しない |
| ライセンス同梱・テスト接続 | (b) | 新原本を基準に保持 |

surface の本家差: `egui-wgpu-0.37.0/src/winit.rs:203`、`:218`、`:229`、`:247`。

既存 `renderer.rs:667` の atlas overflow 診断には、write を skip する処理もある。これを更新の根治策として設計・拡張しない。診断は保持し、配送不変条件の回帰で overflow が生じないことを受入条件にする。既存 skip 処理の撤去可否は、配送修正とその履歴を含む独立判断として記録する。

#### 取り込み手順

1. 新版原本、checksum、VCS 情報、ライセンスを記録する。
2. **旧 vendor に upstream diff を継ぎ足さず、新版原本へ必要差分を付け直す。**
3. 上の論理項目ごとに差分とテストを分けてレビューする。
4. 新版の manifest、feature、依存版を基準にする。旧 Cargo.toml を上書きコピーしない。
5. root／standalone の egui 系依存が同じ新版 vendor 一実体になるよう確認する。
6. `PATCHES.md`／README を実ソース差と一致させる。
7. `.cargo-ok`、生成 target、不要な registry 管理ファイルを取り込まない。
8. vendor の置換以外の DLL・モデル等を削除しない。reparse point は作らない。

### 4.9 wgpu 27→30、native presenter、シェーダ

#### 現行経路の整理

`windows_058`、`wgpu-hal` の直接依存は root manifest に残るが、`src`／`crates`／`tests` の Rust コードには `windows_058`、hal import、`create_texture_from_hal` の実呼出が見当たらない。旧経路のコメントは残る。

現 native presenter は D3D11 の `OpenSharedResource1` を使う。

- `src/video/native_presenter/render_core.rs:8347`、`:8379`
- 動画 swapchain の Present: 同 `:6708`
- overlay の CompositionVisual surface: `src/video/native_presenter/overlay_gpu.rs:43`

したがって推奨は、**未使用の直接 `wgpu-hal`／`windows_058` を除去し、旧コメントを訂正する**ことである。使わない `windows_062` 別名を追加しない。

将来 hal を直接使う場合、wgpu-hal 30.0.1 が要求する Windows crate は **0.62系**。根拠: `wgpu-hal-30.0.1/Cargo.toml:518`。既存 D3D11 本体の windows 0.61 を巻き添えで全面更新する必要はない。

#### 実影響箇所

| 箇所 | 変更対象 |
|---|---|
| `Cargo.toml:368`、`:369` | wgpu と未使用 hal 直接依存 |
| `src/lib.rs:495`、`:526` | presentation 設定 |
| `src/video/native_presenter/overlay_gpu.rs:31` | Instance 作成、DeviceEpoch |
| `src/video/native_presenter/render_core.rs:12089` | surface configure |
| 同 `:13840` | texture delta 適用 |
| 同 `:13891`、`:13920` 付近 | render pass、submit、present |
| `src/compare_wgpu.rs:274`、`:539` | pipeline と callback |
| `src/panorama_wgpu.rs:332`、`:669`、`:506`、`:833` | pipeline、mipmap、callback |
| `src/gpu_lanczos.rs:680`、`:708` | 静止画縮小 GPU 処理 |
| `build.rs:140`、`:230`、`Cargo.toml:495` | 独立した Naga 27→HLSL→DXBC 経路 |

検索式:

```powershell
rg -n 'wgpu::|egui_wgpu::|wgpu_hal|windows_058|OpenSharedHandle|OpenSharedResource1' src crates tests
```

#### 必須 API 差分

| 変更 | 本家根拠 |
|---|---|
| `Instance::new` が descriptor を値で受ける | `wgpu-30.0.1/src/api/instance.rs:63` |
| pipeline layout の bind group layout が Option 配列 | `wgpu-30.0.1/src/api/pipeline_layout.rs:38` |
| push constant 記述から `immediate_size` へ | 同 `:44` |
| pipeline／render pass の `multiview_mask` | `src/api/render_pipeline.rs:238`、`render_pass.rs:682` |
| surface configuration の color space | `src/api/surface.rs:90` |
| acquire が `CurrentSurfaceTexture` を返す | 同 `:148` |
| present が `Queue::present` へ | `src/api/queue.rs:377` |
| egui-wgpu の surface 設定が `WgpuConfiguration.surface` に集約 | `egui-wgpu-0.37.0/src/lib.rs:325` |

surface acquire の Success／Suboptimal／Timeout／Occluded／Outdated／Lost／Validation を新 API に対応付ける。native overlay に新しい retry 機構は作らない。既存の device-lost latch と terminal error 方針を維持し、surface 状態だけで DeviceEpoch を失効させない。

共有 DeviceEpoch の configure write lock と submit/present read lock は保持する。`Queue::present` へ変わっても gate の外へ出さない。

#### texture delta の追加変更

0.37 の `TexturesDelta.set` は、texture ID ごとの delta 列を持つ map、free は set になっている。

根拠: `epaint-0.37.0/src/textures.rs:284`。

`src/video/native_presenter/render_core.rs:13840` と vendor diagnostics を、ID ごとの delta 順序を保つよう直す。map 全体の反復順序には依存しない。whole→partial、複数 partial、描画後 free を検査する。font atlas は RGBA になり、`ImageData` は Color variant を持つ。根拠: `epaint-0.37.0/src/image.rs:16`。

#### VSync、latency、stencil、surface 寿命

- mIV は現在も AutoVsync／latency 1 を明示している。新本家の低遅延既定だけで、製品方針を変えない。
- 新 `SurfaceConfig` へ環境変数の既存意味を移す。
- native overlay も latency 1 の明示を維持する。
- stencil 対応は、mIV が stencil を要求することを意味しない。callback pipeline と render target の format／sample 数を揃える。
- 本家の surface 修正を採用しても、当該 viewport だけを破棄する mIV の差分は必要。
- Surface を DComp visual／window より先に破棄する既存寿命を維持する。

根拠: `egui-wgpu-0.37.0/src/lib.rs:71`、`:86`、`src/renderer.rs:187`、`src/winit.rs:700`。

D3D11 のアップスケール用 WGSL→HLSL→DXBC は build dependency の Naga 27 を使う独立経路である。wgpu 30 と型を交換しないため、同時更新を必須にしない。wgpu 側 WGSL と native 側生成物を混同せず、それぞれ検証する。

**見た目・性能:** 色空間、alpha、フィルタ、mipmap、present cadence に退行余地がある。動画本体を別方式へ切り替える設計は含めない。

### 4.10 egui_kittest／snapshot

#### 本家の変更

- egui_kittest 0.33.3 は kittest 0.3.0、新版は0.5.0。
- 0.37 は visible な入力部品に accessible name がないと失敗する。
- read-only は除外される。
- label または placeholder が名前として使われる。
- settled 後と snapshot 前に検査される。

根拠: `egui_kittest-0.37.0/src/accessibility.rs:14`、`:26`、`:46`、`src/builder.rs:151`。

#### 影響箇所

- `tests/ui_snapshot.rs:28`
- `tests/support/ui_harness.rs:14`
- `tests/support/scroll_layout.rs:6`
- `tests/book_composition_editor_snapshots.rs:405`
- lib 内の各 snapshot／Harness fixture
- `src/ime_focus.rs:1268` 等の raw TextEdit allowlist 検査

kittest 0.5 の `NodeT`、`accesskit_node()`、関連 node の構築は残る。全 query の全面書換えが必要とまでは確認できない。AccessKit の版と名前付き container によって、label query が複数一致したり、以前の node ID を Response として使えなくなる点を重点確認する。

根拠: `kittest-0.5.0/src/node.rs:9`、`egui_kittest-0.37.0/src/node.rs:21`。

#### 対応方針

- 共通 builder、DX12|VULKAN mask、lazy renderer、CPU adapter 優先、renderer 独立を維持する。
- 入力欄は `labelled_by`／適切な hint、icon-only button は意味のある名前を本体描画側に付ける。
- テストだけに仮の名前を付けて製品の欠落を隠さない。
- 共通 builder で accessibility check を無効化しない。
- test の意味に応じて role／親 container で query を限定する。固定 node ID への依存を増やさない。
- `read_response` で旧 pass の矩形を拾わない。最終描画 pass 内の geometry 採取を優先する。

#### 更新手順

1. **旧 baseline を維持した通常比較を先に実行**し、候補・差分画像を保存する。
2. フォント、空 Frame、固定サイズ部品、ScrollLayout、入力を独立 fixture で確認する。
3. 字面だけの差、文字幅・行高由来の layout 差、余白・panel の差、操作回帰に分類する。
4. 入力／geometry 回帰を解決し、フォント・余白方針を利用者と確定する。
5. snapshot 段でまとめて baseline を更新する。
6. 検証担当が変更445枚を対象に、旧／新／diff を分類一覧で照合する。contact sheet は索引用とし、文字位置・切れ・異常差分は原寸で見る。
7. 独立 reviewer が分類根拠と各画面群の代表、例外差分を確認する。利用者にはフォント・ダイアログ・HUD 等の採択が必要な代表を提示する。
8. 更新モードを外して再比較する。

integration target だけでなく lib 側 PNG も同じ更新台帳へ含める。許容誤差拡大、実入力試験削除、不要な追加 frame によって一致させない。

### 4.11 その他の挙動変化と複数 viewport

| 変更 | mIV の影響・対応 |
|---|---|
| popup 再オープン時の sizing pass | `src/ui_main.rs:1471`、fullscreen popup。sizing pass 中に選択・保存・command を発行しない |
| press で開いた popup が release で即閉じない | 外側 click 終了、右クリック menu、wheel owner の終端回帰を維持 |
| 別 menu が開いている場合の hover open | menu bar の wheel 消費・選択・外側 click を検査 |
| ScrollArea の端フェード | 文字が薄くなる新しい見た目。採用前に利用者確認 |
| Window の margin／最小サイズ | 小画面 dialog の本文と固定操作行を再計測。定数で大きく引かない |
| tooltip の hit-test／container | tooltip が背面入力 owner や widget query に混ざらないことを確認 |
| `request_repaint_after` の余分な repaint 減少 | 予定外 frame に依存する完了処理を直す。repaint を追加して旧頻度へ戻さない |
| eframe の窓復元、root のみ保存 | mIV 独自配置と二重所有しない。現 persistence feature 無効の範囲と区別 |
| raw mouse motion の配送先 | focus を持たない viewport 下の pointer にも届き得る。受信窓から keyboard owner を推定しない |
| Screenshot callback 化 | test-script の撮影要求／結果受信を channel に揃え、既存 batch owner を維持 |
| set_fonts の即時適用 | 初期 frame を一律追加する test を再評価。atlas と galley cache の失効を検証 |

本家根拠:

- popup sizing／終了: `egui-0.37.0/src/containers/popup.rs:629`、`:660`
- menu: `src/containers/menu.rs:440`
- fade: `src/containers/scroll_area.rs:1704`、`src/style.rs:804`
- Window margin／最小サイズ: `src/containers/window.rs:633`、`:696`
- repaint: `src/context.rs:133`
- raw motion: `eframe-0.37.0/src/native/wgpu_integration.rs:511`
- root 保存: 同 `:903`
- screenshot: `egui-0.37.0/src/viewport.rs:1205`、`src/context.rs:4557`
- font 即時適用: `src/context.rs:2505`、`:2561`

撮影の mIV 側は `src/test_script/capture.rs:282`、`:375`。callback 内で PNG encode／ファイル I/O を行わず、既存 worker へ渡す。

#### detached 凍結ルール

今回触れる主な境界は BA-2 の viewport 寿命、BA-6 の配置所有、BA-7 の状態所有である。API 更新を理由に以下を行わない。

- host の rect 捕捉条件追加
- visibility／geometry 由来の recreate
- 新 pending bool／Option
- focus・入力を時間窓で遮断
- root と child の cache／surface 一括 reset

`App::logic/ui` 分離、callback の Ui 化、scheduler／surface／delta 移植は構造変更として、**着手前に ClaudeCode と独立 Codex reviewer が「症状パッチではない」と合意し、範囲と理由を `docs/detached-rework-plan.md` §11へ記録する**。本調査をその合意済み記録とは扱わない。

### 4.12 Rust 1.99 警告と MSRV

指定ログでは f32 fallback 警告445件／36ファイル、mimageviewer lib 全体の警告表示は688件だった。これは既存ログの集計であり、今回の check 結果ではない。

主な箇所:

- `src/ui_main.rs:184`
- `src/ui_text.rs:519`
- `src/ui_adjustment_panel.rs`
- `src/video/native_presenter/overlay_draw.rs`
- `src/ui_fullscreen/draw_icons.rs`
- vendor egui の style 等

#### 処理方針

| 警告 | 方針・段階 |
|---|---|
| `Stroke::new`／`PathStroke::new` の f32 fallback | 依存切替後に再集計。0.37 は幅が `f32` なので先行一括 suffix 追加をしない |
| その他の曖昧 float | 型が必要な API 境界だけ `_f32`／局所型注釈。警告整理段で修正 |
| vendor の旧 `std::f32::EPSILON` | 新原本置換で評価。旧 vendor の警告修正を別移植しない |
| `Atomic::fetch_update` | egui 更新では解消しない。Ordering／戻り値を変えない。MSRV で使える API を確認して処置 |
| dead code | 通常 feature／test／test-script／Windows cfg を分けて棚卸し。機能・診断削除で警告を減らさない |

根拠: `epaint-0.33.3/src/stroke.rs:27` と `epaint-0.37.0/src/stroke.rs:26`、`:140`。

Atomic の代表は `src/log_writer.rs:105`、`:165`、`src/everything_ipc.rs:226`、`src/ime_focus.rs:225`、`src/similar_index.rs:1431`。vendor witness にも1件ある。

**MSRV は少なくとも1.95へ更新が必要**。egui／eframe 0.37 の manifest が要求する。開発 toolchain の1.99固定と、サポート最小版を分ける。

推奨は workspace `rust-version = "1.95"` とし、1.95で検証した上で宣言すること。`try_update` の採用だけを理由に1.99へ引き上げない。1.99用 std docs は今回の環境に見当たらず、その API の最小安定版は本調査では確定していない。

1.95維持に必要なら `fetch_update` を残し、非推奨の扱いを対象箇所へ限定して明記する。MSRV 検証ができない場合、「1.95確認済み」とは記載しない。

### 4.13 Remote／他 crate

| 対象 | 波及 |
|---|---|
| `crates/egui-ime/Cargo.toml:8` | egui 版更新と helper／test の移行が必須 |
| `tools/mask_lab`／`local_adjust_lab`／`comic_lab`／`music_lab` | App trait、Ui、panel、RawInput、入力 feature を更新 |
| `crates/vst3-host-tester/Cargo.toml:22` | egui／App／panel。独自 rust-version も整合確認 |
| `crates/remote-web` | egui 直接依存なし。Web UI／IPC 変更は不要。root 非表示時の応答・完了が影響 |
| `crates/remote-ipc` | protocol 変更なし。既存 UI wake と完了 owner を維持 |
| `crates/launcher` | egui 直接依存なし。workspace MSRV と埋め込み core 更新の波及 |
| `crates/epub-pdf-worker` | egui 移行なし。build-dev の companion 検証対象 |
| `crates/comic-core` | 注釈ラスタライザは別経路。今回変更しない |
| `local-adjust-core`／`music-core` | egui 非依存のまま |
| Susie／LibRaw／ORT／VST bridge | ABI 更新を含めない。既存検証条件を維持 |

ラボを0.33のまま残して本体だけ0.37へ変えると、同じ workspace 内で旧 egui と本体の型が分かれる。manifest の版変更と必要 API 移行を同じ切替段で扱う。

## 5. 利用者に相談する事項

0.37 を目標にする決定は受領済みであり、版の選び直しは求めない。以下は、**具体的な旧／新比較を用意してから**確認する。

| 項目 | 推奨案 | 判断が必要な理由 |
|---|---|---|
| 文字の字面・baseline・hinting | fallback と保存値を保持し、補正を実測で整理 | 全画面の見た目と密度が変わる |
| 色付き emoji | user text の改善として代表比較を提示 | 旧単色表示との視覚差 |
| 本文からの Window 移動 | 当初は必要箇所で従来操作を保持 | タイトルバー限定は操作を減らす |
| マウス drag scroll | 旧版で利用できた画面は保持 | タッチ限定化は操作を減らす |
| Panel の drag collapse／double-click | 当初は既存操作を保持 | 新しい閉じ方・復帰方法が増える |
| ScrollArea の端フェード | 旧外観維持を初期案とする | 端の文字が薄くなる |
| Window margin／最小寸法 | 到達性を保証した比較案を提示 | 同じ画面で本文量が変わり得る |
| system font fallback | 初期移行では既存明示 chain を基準 | 環境差・読み込み費用が増える可能性 |

Ctrl+Q、OS 直読み撤去、動画描画方式変更、既存パネルの内容省略は、今回の自動採用事項にしない。

## 6. 段階設計

### 6.1 共通の進め方

各段の終わりは通常製品ビルドが通り、対象の自動検証を記録してコミットできる状態にする。snapshot の旧 baseline 不一致は、意図した変更候補として記録するが、テスト成功とは扱わない。

依存切替には egui／epaint／egui-wgpu／wgpu の型が一緒に変わる最小原子範囲がある。途中コミットで型を二重化する互換レイヤーは設けない。大きな切替差分は、下記の領域別レビューを先に済ませ、統合してコンパイルが通った時点で一つの切替コミットにする。実際の差分がこの単位でもレビューできない規模なら、先行する挙動不変の抽出を追加し、未コンパイルの段を完了扱いにしない。

規模は見積りであり、上下流の生成物を含む厳密な差分行数ではない。

### 段0: 基準固定と未解決事項の記録

**範囲**

- source／toolchain／feature／native fixture を固定。
- vendor 全差分台帳を本書から起こす。
- 旧版の警告、snapshot 枚数、テストの既知失敗を記録。
- detached §11 の構造合意を得る。

**受入条件**

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt --check
```

必要な旧版焦点テストを実行し、同一条件の有効な既存結果は再利用する。過去の IME 記録には App サイズ制限の失敗があるため、現在の HEAD でも成立するかを区別し、更新後の失敗へ混ぜない。テスト上限を緩めて基準を緑にしない。

**規模:** 文書・検証台帳3～6ファイル、150～350行。製品コード変更なし。

### 段1: 0.33.3 上で非表示処理の責務を分離

**範囲**

- App の service、入力、描画、post-render 境界を抽出する。
- 旧 `update` から同じ順序で呼び、既存挙動を維持する。
- root／mounted／parked context の進行先を明記する。
- 非表示 service が events／pass counter に依存しないようにする。

**受入条件**

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo test -p mimageviewer --lib tray
cargo test -p mimageviewer --lib startup
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib sidecar_restore
.\scripts\test-lib-full.ps1
```

filter は実際の選択件数を確認する。worker 完了、外部ツール ACK、modal 終端、root close、兄弟 context 非変更を検査する。

**規模:** 8～18ファイル、500～1200行。純移動は差分上の増減が大きくなる。

### 段2: 新版 vendor の再構築

root をまだ切り替えない準備用ディレクトリ／manifest で、新原本と最小パッチを検証する。旧・新を製品の同一 graph に混在させない。

| 小段 | 範囲 | 見積り |
|---|---|---|
| 2a | egui の3 API と回帰、egui-winit の配送 owner／trace | 8～14ファイル、ローカル変更400～900行 |
| 2b | egui-wgpu の mipmap、delta、surface、診断、startup | 10～15ファイル、900～1800行 |
| 2c | eframe の scheduler、visible commit、viewport、診断 | 10～16ファイル、1200～2400行 |

原本置換による upstream 行数は別集計し、ローカル差分と混ぜない。

**受入条件**

- 各小段の standalone lib test。
- Process／repeat／release／focus／allow_ime の回帰。
- delta の whole／partial／free、no-surface、当該 surface のみ破棄。
- scheduler の per-window、outer phase、hidden idle、interactive resize。
- feature なしと診断 feature ありを分ける。
- 旧 root の core check が引き続き通る。
- 新原本からの全差分が台帳に載る。

### 段3: 依存と必須 API の原子的切替

**範囲**

- egui 系0.37、wgpu30、ラボ、egui-ime、kittest。
- App trait、root Ui、panel、Context 改名。
- ModifiersChanged、MouseWheel の source／phase、raw scroll 利用の移行。
- native overlay、pipeline／surface／present、TexturesDelta。
- test-script screenshot callback、on_exit feature 整合。
- 未使用 hal／windows_058 の整理。
- 全 test target のコンパイルに必要な API 修正。

レビューは次の4票に分ける。

1. App／viewport／panel
2. 入力／IME／keymap
3. GPU／texture／surface
4. ラボ／Harness／テストの機械移行

**受入条件**

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo test -p egui-ime
cargo test -p egui-winit --lib
cargo test -p mimageviewer --lib --no-run
cargo test -p mimageviewer --test ui_snapshot --no-run
cargo test -p mimageviewer --test book_composition_editor_snapshots --no-run
cargo fmt --check
```

`cargo tree -d` と egui／epaint／egui-wgpu／wgpu の逆依存を確認する。意図しない旧 egui graph、別版による型分裂、古い vendor path がないこと。

**規模:** 本体・テスト・ラボ120～190ファイル、2500～5000行。原本置換・lockfile は別集計。これを主に機械移行へ限定するため、前段で構造判断を済ませる。

### 段4a: 入力・非表示・popup の挙動を揃える

**範囲**

- hidden logic の進行と古い入力の非再実行。
- popup の wheel snapshot／消費／終了時残量。
- modifier の順序、OS level、KeySlot と egui event の同時 claim。
- drag、focus、IME、Tab、dialog。
- Ctrl+Q の製品方針を初期化へ反映。

**受入条件**

```powershell
cargo test -p mimageviewer --lib ime
cargo test -p mimageviewer --lib focus
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib keymap
cargo test -p mimageviewer --lib specified_scale
cargo test -p mimageviewer --lib fullscreen_overflow
cargo test -p mimageviewer --lib navigator
cargo test -p egui-ime
cargo test -p egui-winit --lib
```

さらに close／cancel／focus loss／repeat／後続新規入力、root／FS／native の等価経路を検査する。

**規模:** 12～25ファイル、500～1100行。大きければ hidden service と入力を分け、各小段で core check と焦点試験を通す。

### 段4b: フォント・layout の採択

**範囲**

- baseline／fallback／toolbar／HUD。
- フォントプレビュー方式。
- Window margin／min size、Panel separator。
- 利用者相談事項の比較を用意し、採択を反映。
- §1.380 の既存残件と新しい退行を区別。

**受入条件**

```powershell
cargo test -p mimageviewer --lib ui_fonts
cargo test -p mimageviewer --lib ui_font_catalog
cargo test -p mimageviewer --lib calculated_
cargo test -p mimageviewer --lib startup_dialog
python scripts/check_ui_glyphs.py
```

文字 bbox／行高、Fits／Scrolls、実 wheel による末尾到達、固定操作行の viewport／clip 内到達を確認する。108条件等の既存 geometry 検査を減らさない。

**規模:** 8～18ファイル、400～1000行。プレビュー方式の変更が大きい場合は独立小段にする。

### 段4c: GPU・native HUD の整合

**範囲**

- texture delta、mipmap、callback。
- native overlay の acquire／submit／present と gate。
- DPI／font atlas／galley cache の失効。
- 透明合成、VSync／latency、surface drop。

**受入条件**

```powershell
cargo test -p mimageviewer --lib compare
cargo test -p mimageviewer --lib panorama
cargo test -p mimageviewer --lib gpu_lanczos
cargo test -p mimageviewer --lib native_presenter
cargo test -p mimageviewer --lib bottom_hud
```

必要な診断 feature の headless test を追加する。実 HWND の作成・表示・入力操作を含む test は、この自動段へ無条件に混ぜない。

**規模:** 8～16ファイル、300～800行。D3D11 動画シェーダそのものの変更は既定範囲外。

### 段5: Accessibility と snapshot 更新

**範囲**

- 入力部品の名前を本体側で修正。
- query／Response／scope の整合。
- フォント・layout 方針確定後、期待 PNG を更新。
- Id 非推奨と残る Rust 警告を整理する。

**受入条件**

通常比較で候補差分を収集した後、更新専用の子 PowerShell 等で環境変数を設定する。

```powershell
$env:UPDATE_SNAPSHOTS = '1'
cargo test -p mimageviewer --test ui_snapshot
Remove-Item Env:UPDATE_SNAPSHOTS
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
```

lib snapshot は該当 filter を台帳化して同様に更新・通常比較する。445枚すべてについて、未変更／更新理由／目視担当／例外を記録する。

**規模:** UI・テスト15～40ファイル、300～1000行、PNG 最大445枚。PNG を画面群ごとにレビュー・コミットしても、異なる renderer 方針の baseline を混在させない。

### 段6: 全体 gate と確認用 build

**受入条件**

```powershell
cargo fmt --check
python scripts/check_ui_glyphs.py
.\scripts\test-lib-full.ps1
.\scripts\test-full.ps1
.\scripts\build-dev.ps1 -PreserveRuntime
```

- 通常 feature と `test-script` の必要な検証を記録する。
- 既知失敗が現在も存在する場合、全体 gate 成功とは扱わず、別修正を含めるか設計担当へ返す。
- 本体、Remote、EPUB worker の成果物を揃える。
- feature graph と MSRV を検証する。
- 対象 commit、command、feature/profile、fixture、exit、件数、ログを保存する。
- 同じソース・条件の成功台帳は reviewer も再利用する。
- Cargo／vendor／共通 support 変更では `test-focused.ps1` が広域として exit 2 を返し得る。これを成功と扱わず、明示 filter または full lib gate へ進む。

build／check は最低10分、広域 test は最低15分以上の実行枠を用意する。長い既存テストにはさらに余裕を持たせる。runner timeout 後の BrokenPipe 等を製品失敗として分類しない。

**規模:** 文書・検証補修3～8ファイル、100～300行。新しい機能変更は持ち込まない。

### 段7: 利用者の実機確認と受入れ

エージェントは通常 profile の成果物を起動しない。利用者が段6の成果物で以下を確認する。想定20～35分。必要なサンプルは開発側が少数に絞って用意する。

確認後、対応範囲、未確認機器、採択した外観、保留事項を文書へ記録する。

## 7. 実機確認手順

通常版・トレイ常駐版を先に終了する。single-instance mutex を共有する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

この build は `portable` ではなく、既定で実利用中の `%APPDATA%\mimageviewer` を開き、設定・データを更新し得る。隔離確認を選ぶ場合は、同じ成果物へ `--data-dir .\target\dev-runtime\data` を渡す。

### 7.1 フォントとダイアログ

1. 既定フォントで一覧、詳細、検索、metadata、toolbar を開く。
2. BIZ UDPGothic 9pt、Meiryo 10pt、Meiryo UI 10ptを順に選ぶ。
3. 日本語・Latin・数字・CJK・数学英字・記号・emoji を含む開発側 fixture を表示する。
4. baseline、toolbar の上下位置、折り返し、欠け、豆腐化を比較する。
5. 動画／音楽 HUD の時間、`x1`、`Norm`、dB が選択 UI フォントで崩れないことを確認する。
6. 小さい窓とUI倍率200%で長文 dialog を開き、本文、固定ボタン、タイトル×へ到達できることを確認する。
7. 採択した仕様に従い、マウス／タッチの Window 移動、panel resize、scroll を確認する。

### 7.2 日本語入力

Google 日本語入力と MS-IME で、まず共通の少数シナリオを行う。

1. 注釈本文で変換を Enter 確定する。確定だけで改行しない。
2. 確定直後にもう一度 Enter を押す。改行が1つ入る。
3. 検索欄で確定直後の Enter を押す。検索が実行される。
4. 変換中の Escape／候補選択の Tab／Space がアプリ操作へ漏れない。
5. 確定直後の通常 Escape が、採択済みの通常操作として働く。
6. composition 中に別窓へ focus を移し、戻る。文字の重複・消失・キー残留がない。
7. 長い欄・複数行の末尾で変換し、候補窓がカーソル位置へ追従する。
8. 通常窓、F12別窓、native 動画のタグ入力の代表各1経路で確認する。

### 7.3 ショートカットと wheel

1. main と F12 で Ctrl／Shift／Alt を押して離し、通常操作へ戻る。
2. 編集で Ctrl+矢印、Space pan、Ctrl による筆切替を行う。離した後に修飾状態が残らない。
3. Enter／NumpadEnter と右Ctrlの代表割当を確認する。
4. Tab の既存 Action が働き、意図しない focus traversal が起きない。
5. popup 内で wheel／Ctrl+wheel を操作し、クリックとキーで閉じる。背面一覧・navigator が動かない。
6. 閉じた後の新しい wheel は通常どおり届く。
7. slider／seek を押して外へ移動し、外で離す。後続の新しい押下へ古い drag が残らない。
8. clipboard の Ctrl+C／X／V と修飾キー付きの既存割当を確認する。

### 7.4 トレイ、非表示、複数窓、Remote

1. 完了通知のある開発用処理を開始してトレイへ隠し、完了後に復帰する。
2. 復帰時に古いキー・wheel・dialog 確定が再実行されない。
3. F12を開いたまま root を隠す／復帰し、子窓の表示・入力・終了を確認する。
4. 同じ PDF／画像を使う兄弟 reader の片方を閉じ、もう片方の表示と cache が失われないことを確認する。
5. root 非表示中に Remote の既存操作を1回実行し、応答・完了・表示復帰を確認する。
6. 非表示 idle の CPU と wake は既存 ReleaseChecks の担当手順で確認する。今回の移行用 heartbeat を作らない。

### 7.5 動画 HUD と GPU 表示

1. 通常動画を再生し、HUD の日本語、時間、速度、音量、音声トラックを確認する。
2. 820px程度から狭幅へ縮め、操作の重なり、メニュー、seek／音量 drag を確認する。
3. 一時停止、再開、seek、音声トラック変更、動画→音声→動画を行う。
4. F11／F12を往復し、HUD、親子関係、focus、透明合成を確認する。
5. 静止画の比較 wipe／diff、panorama、縮小表示を確認し、mipmap のちらつき・黒表示・境界の欠けがないことを見る。
6. fullscreen を閉じて一覧へ戻り、黒サムネ・文字atlas破損がないことを確認する。

### 7.6 マルチモニター DPI

1. 100%と150%／200%の異なるDPIの画面間で main、F12、動画窓を移動する。
2. 文字の大きさ・baseline、候補窓、HUD の hit rect、slider／seek の位置を確認する。
3. 最大化・復元・F11往復後も、画面外へ操作が失われない。
4. 子窓を閉じた後も root の描画・screenshot・入力が継続する。

## 8. 完了条件と文書更新

完了には、次のすべてを必要とする。

- 0.37.x と wgpu30 の依存 graph が整理され、vendor 差分が全件説明できる。
- egui-winit を保持する理由と、将来削除できる具体条件が記録される。
- hidden logic、入力 owner、context／surface／texture の回帰が通る。
- snapshot の差を分類し、目視確認と採択が記録される。
- full gate、通常確認用 build、利用者の重点実機確認が完了する。
- リリース済みの機能・設定・操作を黙って削減していない。
- 未実施の機器・IME・性能条件を、確認済みと記載しない。

更新対象は `docs/display-pipeline.md`、`docs/video-architecture.md`、`docs/coding-conventions.md`、`docs/keymap-spec.md`、`docs/ime-process-keys-plan.md`、`docs/ui-snapshot-policy.md`、`docs/detached-rework-plan.md` §11、vendor 各台帳、backlog §1.382。§1.380 の残件は、個別の解決証拠が得られたものだけ更新する。

利用者向け操作・外観が変わる採択を行った場合は、対応するマニュアルと製品ページも更新する。更新番号だけを理由に、未確認の改善や互換性を告知しない。