> 状態: 設計案 第2版 (2026-10-16、独立レビュー REVISE への対応、再レビュー前)。目標は利用者決定により egui 0.37 系。正本の backlog は §1.382。

# 改訂履歴

2026-10-16、`226985787` に対する独立レビュー REVISE への対応案。9件ともソース上の根拠を確認し反映した。改訂案の再レビュー、実装、検証は未実施。

- **P1-1 — 反映:** TexturesDelta の適用・drain・引継ぎ・意図的破棄を経路別に定義し、段3の必須移行へ追加。
- **P1-2 — 反映:** backend の保留 RawInput と Win32 edge queue の各所有者に非表示入力の受付・終端境界を定義。`raw_input_hook` 単独では解決できない理由を明記。
- **P2-3 — 反映:** 無描画配送にも queue submission と適用後 free の完結条件を追加。
- **P2-4 — 反映:** unsmoothed wheel の取得元、単位換算、軸変換、イベントごとの修飾キー、snapshot と消費順を定義。
- **P2-5 — 反映:** Ctrl+Shift+C/X/V は従来操作の保持を推奨し、通常窓・native 入力欄の共通契約と利用者相談事項へ追加。
- **P2-6 — 反映:** 実ウィンドウを起動する既存テストを自動 gate から分離する準備段を追加。承認と隔離成果物の条件を明記。
- **P2-7 — 反映:** accessibility 修正を候補画像収集より前へ移し、両 integration target と列挙した lib snapshot の更新・通常比較を必須化。
- **P3-8 — 反映:** 通常版 immediate viewport の段階別時間・delta bytes・遅延ログを独立した vendor 差分として保持対象に追加。
- **P3-9 — 反映:** 0.33.3 上の準備段を分割・明記して段3を再見積り。固定の「4票」と PNG ごとの担当記録を撤去し、画面群の共通理由と例外を一つの台帳で管理。

# egui 0.37 更新の影響調査と段階設計

## 0. 調査範囲と結論

初回調査日・改訂日: 2026-10-16  
対象: `G:\home\mimageviewer-egui036`、ブランチ `egui-036`  
初回調査時 HEAD: `05b6a15de26897369a35e5aa00377d174ca3760d`  
改訂時 HEAD: `226985787`。初回調査後の変更は設計書の保存。

リポジトリ、指定された更新履歴要約、Cargo registry の0.33.3／0.37.0、Rust 1.99の既存 check ログ、独立レビュー全文を読み取り専用で照合した。ファイル変更、依存解決、ビルド、テスト、ネットワーク利用、製品起動は行っていない。

以下の実行時影響はコードからの見立てである。既存文書に記録された他担当の結果と、今回のソース確認を区別する。

目標は egui 系0.37.0。実装開始時に0.37.xの新しいパッチ版を選ぶ場合は、その版で該当判断を再照合する。

主な結論:

1. App の非表示処理、root Ui、入力、texture delta、wgpu surface の移行が必要。
2. `vendor/egui-winit` は削除できない。本家に Hidden／Forwarded と同等の配送保証がない。
3. `vendor/eframe` も削除できない。非表示 scheduler 以外の製品・診断差分がある。
4. `ui_fonts.rs` の計測は既に ttf-parser。ab_glyph が直接残るフォントプレビューとは分けて扱う。
5. 動画本体は D3D11 native presenter。wgpu 更新の主対象は HUD overlay、比較、panorama、静止画縮小。
6. `raw_scroll_delta` はイベント列からの消費可能な unsmoothed 投影へ移す。
7. **TexturesDelta は非空のまま drop できない。** 描画・無描画・終了・headless の各消費責任を移行する。
8. **hidden logic の入力は次回へ保留される。** App の処理分離だけでは、復帰時の操作再生を防げない。
9. snapshot は現ツリーで445枚。accessibility 修正後に、両 integration target と lib を更新する。
10. f32 fallback 警告は既存ログで445件。多くは0.37の明確な引数型で解消する見込みなので、先行一括修正しない。

### 参照表記

本家の `egui-0.37.0/src/...:line` 等は、次の registry root からの相対パス。

```text
C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\
```

mIV の行番号は改訂時の製品ソースに対するもの。件数は特記しない限り `src`、`crates`、`tests`、`tools` の Rust ソースの一致行数で、コメントを含む。AST による呼出数ではない。

## 1. 不変条件と簡素化方針

- 非表示でも、トレイ復帰、非同期完了、Remote、保存終端、メディア制御を進める。
- 非表示期間の通常操作開始を、復帰後に遅延採用しない。
- release、focus、IME 終端は、既存操作を正しく終えるために保持する。操作開始として再解釈しない。
- 復帰後の新しい入力と、表示中の兄弟 viewport の入力を失わない。
- viewport ごとの入力、IME、cache、texture、要求世代、worker の所有を維持する。
- 配送済み press の release を失わない。
- popup が所有した wheel は、終了フレームと平滑化残量を含めて背面へ漏らさない。
- texture delta は所有者を失わず、適用・drain、引継ぎ、対象 renderer の明示終了のいずれかへ到達させる。
- queue に書いた upload は submission まで完結させ、使用中 texture は submit 前に free しない。
- フォント fallback、選択設定、リリース済み機能・データを保持する。
- snapshot 更新で入力・到達性・所有権の回帰を承認しない。

最初に検討する簡素化は次のとおり。

| 対象 | 採用する簡素化 |
|---|---|
| 非表示入力 | hidden の通常操作を保留して後から選別する組合せを減らし、受付側で操作開始を除く。終端だけを既存 owner へ届ける |
| RawInput と Win32 queue | App の二重 gate を増やさず、各 queue の所有者で同じ受付契約を実装 |
| texture delta | 描画経路と無描画経路で別々に借用適用せず、共通の配送・完結処理へ揃える |
| wheel | 旧値を別 cache へ復元せず、その時点で未消費のイベントから投影 |
| snapshot | PNGごとの重複記録を作らず、画面群・共通変更理由・例外の一台帳 |
| 変換・保存・パスワード | 既存モーダル境界を維持。更新のために並行操作を増やさない |

短い入力変換を新たにモーダル化する必要はない。非表示中も描画 pass を回し続ける案は、入力保留を消すために UI 状態と消費電力を変えるので採らない。窓の再生成で入力・texture を捨てる案も採らない。

新しい pending bool、時間窓、retry、追加 repaint、silent fallback は設けない。状態が必要な場合は既存 owner の型付き状態・配送履歴へ集約する。まれな失敗はログ・通知と既存の単純な終端処理を基本とし、更新専用の回復機構を作らない。

## 2. 前提・更新履歴要約との相違

| 前提・要約 | ソース確認 | 反映 |
|---|---|---|
| egui-winit は削除できる見込み | 本家0.37も HashSet。現 vendor の配送済み press 保持との差が残る | vendor 維持 |
| eframe の変更は hidden repaint 修正 | scheduler、visible commit、delta 配送、通常版計測、診断もある | 全差分を台帳化 |
| ui_fonts は ab_glyph 計測 | 外形・行計測は ttf-parser。プレビューが ab_glyph | 別々に移行 |
| `CentralPanel::show` 廃止 | Context を受ける形がなくなり、Ui を受ける `show` は残る | 親 Ui を移行 |
| `AsId` 削除 | 0.37.0では非推奨として残る | 削除済みと書かない |
| Window はタイトルバーのみ移動 | 既定 `WindowDrag::OnTouch`。タッチでは全面移動 | 入力機器別に確認 |
| clipboard は余分な修飾キーなし | X/C/V は `command_only()`。Windows Insert/Delete 系は別条件 | 従来 Ctrl+Shift 操作を保持 |
| texture delta は map 化 | 非空 drop assertion と明示破棄 API もある | 消費契約を必須移行 |
| hidden では古い入力を読まなければよい | RawInput を保留して次回へ連結する | 受付・保留・終端を設計 |
| snapshot 約350枚 | 追跡 PNG445枚 | 工数を修正 |

根拠: `egui-winit-0.37.0/src/lib.rs:656`、`:1538`、`egui-0.37.0/src/containers/panel.rs:1377`、`src/id.rs:8`、`src/containers/window.rs:17`、`epaint-0.37.0/src/textures.rs:335`、`eframe-0.37.0/src/native/epi_integration.rs:398`。

## 3. 影響件数

| 検索対象 | 一致行／ファイル | 代表箇所 |
|---|---:|---|
| `\bId::new\s*\(` | 571／84 | `crates/egui-ime/src/lib.rs:159` |
| `ui\.id\(\)` | 99／5 | `src/ui_music_panels.rs:706` |
| `\bSidePanel::` | 9／5 | `src/ui_folder_pane.rs:218` |
| `\bTopBottomPanel::` | 23／9 | `src/ui_main.rs:7400` |
| `\bCentralPanel::` | 339／60 | `src/ui_main.rs:17580` |
| `show_viewport_(immediate\|deferred)` | 38／8 | `src/ui_fullscreen.rs:22139` |
| `raw_scroll_delta` | 58／16 | `src/app.rs:58760` |
| `Event::MouseWheel\s*\{` | 103／20 | `src/video/native_presenter/render_core.rs:9751`付近 |
| `impl eframe::App` | 6／6 | `src/app.rs:97613` |
| `fetch_update\(` | 製品・テスト14／12、別途 vendor 1件 | `src/log_writer.rs:105` |

```powershell
rg -n '\bId::new\s*\(|ui\.id\(\)|\bAsId\b' src crates tests tools -g '*.rs'
rg -n 'SidePanel::|TopBottomPanel::|CentralPanel::|show_viewport_' src crates tests tools -g '*.rs'
rg -n 'RawInput|raw_scroll_delta|Event::MouseWheel|\.modifiers' src crates tests tools -g '*.rs'
rg -n 'textures_delta|drop_without_applying_deltas|\.run\(|run_ui|run_pass' src crates tests tools vendor -g '*.rs'
```

`.run(` の1274一致行／130ファイルには Harness や worker も含む。Context の移行件数には使わない。FullOutput の破棄候補も、単なる変数名検索で完了とせず、戻り値の所有を追う。

## 4. 項目別設計

### 4.1 文字描画、補正、fallback、絵文字

#### 本家の変更

0.37は skrifa、vello_cpu、harfrust を使う。

- outline／color glyph 選択: `epaint-0.37.0/src/text/font_face.rs:96`
- color glyph 描画: 同 `:190`
- shaping data: 同 `:332`
- units-per-em 基準の倍率: 同 `:88`
- メトリクス・offset: 同 `:596`
- hinting／subpixel binning 既定有効: `epaint-0.37.0/src/text/mod.rs:77`
- 行内配置: `epaint-0.37.0/src/text/text_layout.rs:1100`

`FontTweak` の scale／offset は残り、hinting target、variation coordinates 等が加わる。根拠: `epaint-0.37.0/src/text/font_tweak.rs:6`。

#### 影響箇所と方針

| 箇所 | 内容・方針 |
|---|---|
| `src/ui_fonts.rs:4`、`:7`、`:13` | user／HUD／toolbar family を維持 |
| 同 `:235`、`:420` | fallback 順と主フォント登録を保持 |
| 同 `:360`、`:373` | 主フォントの視覚中央補正を再検証 |
| 同 `:546`、`:567` | fallback 補正を新 renderer のメトリクスと比較 |
| 同 `:652`、`:687`、`:699` | ttf-parser の bbox／行計測。API更新だけを理由に全面交換しない |
| `src/ui_font_catalog.rs:6`、`:78` | ab_glyph プレビューを別設計で本体表示へ近づける |
| `src/video/native_presenter/bottom_hud.rs` | 実 galley の計測・描画共有と失効を確認 |
| `build.rs:700`、`src/comic_stamp.rs` | Twemoji SVG スタンプは保持 |

選択フォント、face index、利用者の縦位置調整値を自動リセットしない。toolbar の固定3.5pt、主フォント補正、CJK／数学／emoji の補正は別々に判断する。

color glyph は outline bbox と可視範囲が異なり得る。emoji の outline 優先計測は、色付き描画の実際の ink と比較して採否を決める。補正を全部残す／全部捨てる判断はしない。

hinting 由来の差と補正式の誤りを分ける検証 fixture を用意する。製品の設定項目を増やすことは前提にしない。

フォントプレビューは既存の worker 準備を維持し、本体と異なるラスタライザによる見本のずれを解消する方式を小設計としてレビューする。`comic-core` の注釈焼き込み用 `ab_glyph_rasterizer` は別経路なので変更しない。

#### default_fonts と system fallback

egui 単体の default feature は空だが、eframe default には `default_fonts` が残る。現 root の指定方法で全 default font が直ちに消えるわけではない。

根拠: `egui-0.37.0/Cargo.toml:62`、`eframe-0.37.0/Cargo.toml:61`、`epaint-0.37.0/src/text/font_definitions.rs:110`。

一方、eframe の system font fallback は既定有効。初期移行では feature と `NativeOptions::system_font_fallback` を明示し、mIV の既存 chain を基準にする。新しい自動探索を無条件に重ねない。

根拠: `eframe-0.37.0/src/epi.rs:384`、`src/native/epi_integration.rs:260`。

`monochrome_emoji_fonts` は別 feature であり、旧組み込み emoji font と同じ集合が既定登録されるとは限らない。Twemoji は文字 fallback ではなく保存済み注釈の SVG 資産なので、色付き文字対応で代替しない。

**見た目:** 字面、太さ、字幅、合字、折り返し、行高、baseline、emoji の色が変わり得る。具体的な比較を用意して受け入れる。

### 4.2 App／Context、非表示処理と入力所有

#### 本家の契約

`App::logic` は UI の前と、非表示中に repaint 要求がある場合に呼ばれる。常時 tick ではない。非表示中の `Context::input` は、viewport 情報の一部を除き最後の表示 pass の値を持つ。

根拠: `eframe-0.37.0/src/epi.rs:152`、`egui-0.37.0/src/context.rs:1174`。

さらに、非表示中の新 RawInput は消費済みにはならない。

1. `prepare_raw_input` が前回保留分と今回分を連結。
2. `App::raw_input_hook` を呼ぶ。
3. logic-only 実行後、その RawInput を `pending_raw_input` に保存。
4. 次回に再び連結。

根拠: `eframe-0.37.0/src/native/epi_integration.rs:384`、`:398`、`:418`、`egui-0.37.0/src/data/input/raw_input.rs:168`。

visible child がある場合は、非表示 root でも子を登録するため親 UI を実行する。**UI が実行されることと、root が通常操作を受け付けることは別の判定**とする。

#### 影響箇所

- `src/app.rs:94680`、`:97631`: 更新と App trait
- `src/app.rs:94969`: tray／media wake
- `src/app.rs:97661`以降: 非同期完了
- `src/app.rs:97700`以降: post-render の modal／許可境界
- `src/tray_integration.rs:279`以降: hide／restore
- `src/key_input.rs:2023`、`:2099`: queue owner／enqueue
- 同 `:3381`、`:3418`: pass ごとの drain
- 同 `:3960`以降: Win32 subclass の edge／focus 終端
- `src/ui_fullscreen.rs:22139`、`:22364`、`:24745`: viewport callback
- `src/video/native_presenter/render_core.rs:12505`: 独立 Context
- `tools/*_lab/src/main.rs`、`crates/vst3-host-tester/src/main.rs`: App 実装

#### App の責務分離

| 処理 | 移行先 |
|---|---|
| worker 結果、取消・保存終端、Remote 完了、トレイ要求 | `logic`。UI入力を読まない service |
| key／wheel／pointer／focus／IME の pass 処理 | `ui`／対象 viewport |
| panel／dialog／viewport 登録・paint | `ui` |
| 表示済みであることが前提の外部ツール ACK 等 | 実際の描画側 |
| context ごとの結果採用 | 既存 registry／mount owner |
| native 動画フレーム | 既存 presenter thread |

旧 update を丸ごと logic／ui のどちらかへ改名しない。logic は古い events を読まず、UI pass counter の進行を service の条件にしない。副作用は既存要求の採用・消費で一度だけ実行し、新しい実行済みフラグを重ねない。

post-render の終端処理を前へ移す際は、その frame の modal gate を早く外さない。

#### 非表示入力の設計

**推奨は、非表示期間の通常操作を保留しない構造である。** 復帰時に大量の入力を識別する仕組みや、新しい時間窓は作らない。

受付判断は対象 HWND／ViewportId ごとに行う。既存の明示 hide・minimize・restore・focus・host 終端を正本とし、geometry や App の stale な `window_visible` だけから推定しない。`ViewportInfo::visible()` は minimized／occluded の合成であり、明示 hide の全情報を表すものではない。根拠: `egui-0.37.0/src/data/input/viewport_info.rs:98`。

受付時に使う区分は「通常入力」と「状態終端のみ」。新しい App pending bool は置かず、既存 window／input owner の遷移から導く。

| 入力 | 非表示期間の扱い |
|---|---|
| key press／repeat、pointer press、wheel、gesture による操作開始 | 通常操作列へ入れない |
| Text、Cut／Copy／Paste、通常の AccessKit action、drop 等 | 復帰時の通常操作として保留しない |
| key／pointer release、touch End／Cancel | 既存 hold／gesture を終えるため保持・処理。新しい click／確定操作を生成しない |
| focus、ModifiersChanged、PointerGone | 対象 viewport の状態遷移として保持 |
| IME の Commit／空 Preedit／Disabled 等 | 既存 composition の完了・取消を保持。通常キーへ変換しない |
| トレイ復帰、IPC、完了通知、明示 close | 通常入力列とは別の既存制御経路で保持 |
| 復帰後の新しい入力 | 通常受付。古い保留入力と一括廃棄しない |

IME は非空 Commit を含め、非表示化前に開始した composition の正当な終端を落とさない。既存 IME owner へ一度だけ届ける。非表示中に新しい入力先を選び、通常 Text として配送し直さない。

release を普通の pass へ渡すだけでは不十分である。例えば PointerState は release から click を生成する。根拠: `egui-0.37.0/src/input_state/mod.rs:1160`付近。**状態終端用の配送と、widget／App の操作を生成する配送を区別する。**

実装は各状態所有者の終端処理へ接続する。egui 側に必要な限定 API がなければ、InputState／PointerState／interaction owner の範囲内に「対象 viewport の既存入力を終えるが click・shortcut を作らない」処理を追加し、vendor 差分としてレビューする。隠れた UI pass や全 memory reset で代替しない。

#### backend の保留 RawInput

eframe／egui-winit の境界で次を行う。

1. 入力が対象 window の queue へ入る時点で受付を判定する。
2. 明示的な非表示化の境界では、当該 window に既に溜まった未採用の操作開始も通常配送から外す。
3. `update_logic_only` へ入る前に、保留 RawInput を状態終端と通常操作に整理する。非表示の通常操作を `pending_raw_input` へ保存しない。
4. release／focus／IME 終端は上表の owner に消費させるか、既存終端の配送先を保ったまま保持する。未処理終端を消費済み扱いしない。
5. 復帰時は、旧終端を先に完結させてから新しい通常入力を採用する。
6. hidden→restore の間に logic が一度も呼ばれなくても、受付・可視性遷移の境界で成立させる。
7. visible window の scheduling 上の logic-only と、本当に操作を受け付けない期間を混同しない。

`pending_raw_input` を残す場合も、その既存 owner の責務として行う。App に第二の pending queue や「復帰後一回捨てる」フラグを作らない。

#### `App::raw_input_hook` の採否

本家 hook は連結**後**に呼ばれ、保留分と今回分を区別する情報を渡さない。そのため、復帰時に hook だけで通常操作を一括除去すると新規入力も失う。hook 単独の解決案は不採用とする。

hook は、backend が受付・終端を整理した後の mIV 固有の正規化、合成入力の整合確認には利用できる。対象は `raw_input.viewport_id` と新しい入力情報から決め、Context の旧 events を正本にしない。hook で backend の取りこぼしを埋める二重 gate は作らない。

#### Win32 edge queue

現 `begin_frame_with_tracking` は全 pending edge を frame へ移すため、UI pass が止まるだけでは滞留が残る。

- `KeyInputState`／HWND registry が、受付時点の対象 window と物理押下を所有する。
- 非表示化時は当該 HWND の未採用操作を `pending`／`frame`／`operation_frame` の採用対象から外す。兄弟の記録は変更しない。
- release は ReturnKeyState 等の終端へ通す。focus loss、unregister／destroy の既存終端を共用する。
- 非表示期間に始まった押下の repeat を、復帰後の新規 press と見なさない。WndProc の initial／repeat と既存物理押下履歴で判定する。
- 過去の edge を `GetAsyncKeyState` の現在値で fresh／stale に分類しない。
- egui event と Win32 edge の同一押下は、両経路で整合した採否にする。
- 初回登録、Enter／NumpadEnter、再生成 HWND、合成入力でも同じ owner 境界を検査する。

root が非表示でも visible child の入力を止めない。逆に、child 登録のため root UI が回っていても、非表示 root の入力受付を再開しない。native presenter の HWND はその実表示先の既存 owner に従い、root の非表示だけで一括停止させない。

この境界は**段3で回帰込みで必須**。段4へ先送りしない。

#### API 移行

- 描画は `run_ui` と受け取った root Ui。
- 入力・状態のみの headless は `run_pass`、必要箇所だけ `root_ui`。
- viewport callback は `&mut Ui`、Context は `ui.ctx()`。
- Context の `style` 系は `global_style` 系へ。Ui の局所 style は一括置換しない。
- Ui の Deref により解決される旧 forwarding method を無意味に書換えない。

根拠: `egui-0.37.0/src/context.rs:950`、`:983`、`:1024`、`:2599`、`:4662`、`src/ui.rs:77`。

`on_exit` の署名は glow feature と揃える。終了処理本体は保持する。根拠: `eframe-0.37.0/src/epi.rs:227`、`src/app.rs:97792`。

### 4.3 Panel 統一と計算高さ

影響箇所:

- `src/ui_main.rs:7400`、`:10291`、`:12590`、`:15558`: menu／toolbar／検索
- 同 `:20434`: 下部情報
- `src/ui_folder_pane.rs:218`: side panel
- `src/ui_main.rs:17580`、`:19160`: central
- `src/ui_text.rs:3002`、`:3331`: Area 固定パネル
- `tests/support/scroll_layout.rs:6`、`:31`、`:65`: geometry 検査

`Panel::show(&mut Ui, …)` と `default_size`／`min_size` 等へ移す。根拠: `egui-0.37.0/src/containers/panel.rs:252`、`:372`、`:425`。

panel の順序・残り領域を一つの親 Ui に揃え、不要な子 Ui による margin／scope 変更を避ける。slide、drag-to-close、double-click collapse は既存操作と比較して明示設定する。

Area＋Frame＋ScrollArea の固定 HUD を新 Panel へ全面交換しない。separator 幅修正は mIV 独自の計算高さを自動修正しない。新版 ScrollArea も親の available size を上限にする。根拠: `egui-0.37.0/src/containers/scroll_area.rs:842`。

| backlog §1.380 | 影響・扱い |
|---|---|
| clipboard 失敗カード | `src/ui_dialogs/clipboard_capture.rs:378`、`:490`。可変高と固定仮想行の問題は残る |
| 長い保存基点名 | 監査文書の残件節。固定本文予算の問題を別途扱う |
| 注釈右詳細・左一覧 | `src/ui_text.rs:2745`、`:2761`、`:3252`、`:3348`。sink・最低高・本文高を同じ pass で検査 |
| 切り取り・表示トリム | 内外枠と固定操作の実矩形で確認 |
| 構成 editor の行 | §1.242の既存作業へ残す |

Fits／Scrolls、1物理pixel許容、実 wheel による末尾到達を維持する。snapshot の一致だけで高さを受け入れない。

### 4.4 フォーカス、wheel、drag、Ctrl+Q

#### フォーカス・drag

影響箇所: `crates/egui-ime/src/lib.rs:242`、`:305`、`:329`、`src/tab_focus.rs:23`、`src/app.rs:22538`、`:22543`、`src/ui_music_panels.rs:706`、native HUD の seek／音量。

Area／Window／ScrollArea の非 focus 化と、入力部品の focus 回復を区別する。Ctrl+矢印の改善だけで helper を撤去しない。全域 Tab traversal 無効という製品方針は保持する。本家も Tab を focus 移動として扱う。根拠: `egui-0.37.0/src/memory/mod.rs:590`。

drag は、上位 widget との重なり、press 後の widget 外移動、外側 release、disabled 復帰、次の新規押下を検査する。背景 Sense を増やして click を奪わない。

ScrollArea は `DragScroll::OnTouch` が既定。旧版でマウス drag scroll が使えた画面は必要箇所に `Always` を明示する方針。削減する場合は利用者相談を先に行う。

根拠: `egui-0.37.0/src/containers/scroll_area.rs:224`、`:286`。

#### raw_scroll_delta の代替契約

影響箇所:

- `src/app.rs:58738`、`:58760`: 一覧・列数変更
- `src/ui_main.rs:1462`: menu wheel 消費
- `src/ui_helpers.rs:414`、`:475`: 筆半径 wheel
- `src/ui_fullscreen.rs:25754`、`:27352`: 前面 owner と残量終端
- `src/app/runtime_ops.rs`、dialog、ラボ、関連テスト

旧 `raw_scroll_delta` は単純な生 delta 合計ではない。単位換算と修飾キーによる軸変換を含み、その後に scroll／zoom の平滑化へ分かれる。根拠: `egui-0.33.3/src/input_state/mod.rs:440`。

**取得元は、その消費位置での `InputState.events` に残る MouseWheel。** `raw.events`、WheelState の smooth 値、OS現在の修飾キーは使わない。

共通の純粋な投影関数で、各イベントを順に次のように解釈する。

| 処理 | 契約 |
|---|---|
| Point | delta を logical point として使用 |
| Line | `InputOptions::line_scroll_speed * delta` |
| Page | 当該 pass の viewport 高さ `* delta`。content 高さではない |
| 横修飾だけ一致 | `(x+y, 0)` |
| 縦修飾だけ一致 | `(0, x+y)` |
| 両方一致／どちらも不一致 | 軸を保持 |
| 修飾キー | 各 MouseWheel の `modifiers` |
| phase／source | 保存し、Start／End／Cancel は gesture 境界として扱う。通常 wheel は Move。平滑化量を離散量へ混ぜない |

0.37 WheelState の gesture 中の modifier latch と、mIV の離散操作の解釈は分ける。根拠: `egui-0.37.0/src/input_state/wheel_state.rs:104`。

同一 frame に Ctrl あり／なし等が混ざる場合は、受理先と modifiers を保持した連続区間として処理する。同一操作の区間内では従来どおり集約し、通常 wheel で複数 notch だけ行数を増やすような変更をしない。Ctrl+wheel も同一条件の区間で従来の列数変更単位を維持する。異なる修飾条件を frame 最終値でまとめない。

筆半径等、既に Line／Point／Page から独自の notch を作る操作は、その既存換算を保持する。全 consumer を同じ「point量」に押し込まない。

**順序:**

1. 可視性受付後の入力列を対象 viewport の既存 owner が受け取る。
2. fullscreen の前面 owner が、キー・navigator より前に frame の配送先を確定する。
3. 必要な原イベント snapshot は、その既存 owner の範囲だけで保持する。
4. menu／popup／前面領域が wheel を受け、対応する `InputState.events` と公開 smooth 値を消費する。
5. App の一覧処理は従来どおり menu／toolbar 等の後、grid の直前で、**残った events から** unsmoothed 値を取得する。
6. 操作を受理した consumer は同じ入力所有境界でイベントと対応する smooth 値を消費する。
7. 前面 popup の終了時だけ、既存 `discard_pending_scroll` の新版相当でその owner の平滑化残量を終端する。

共通投影関数に未消費入力を cache させない。consume 後に raw snapshot から復活させない。診断用原 snapshot と consumer 用配送投影は混同しない。

本家 ScrollArea の消費だけに依存せず、mIV の raw-event consumer との順序を維持する。

#### Ctrl+Q

本家は end-pass で `quit_shortcuts` を消費して Close を送る。根拠: `egui-0.37.0/src/memory/mod.rs:337`、`src/context.rs:2935`。

各製品 Context で本家既定を明示無効化し、既存 keymap／終了・tray owner へ揃えることを推奨する。新しい終了操作として採用するなら別途相談する。

### 4.5 非推奨 API 削除と IME

本家は TextEdit の IME owner、下線／cursor、候補窓位置を改善する。

根拠: `egui-0.37.0/src/memory/mod.rs:1064`、`src/widgets/text_edit/builder.rs:617`、`:1049`、`:1382`、`egui-winit-0.37.0/src/lib.rs:1291`。

これらは、mIV の shortcut gate、viewport 別 snapshot、取消補完、focus contract、Tab 方針、Win32 Process filter を一括代替しない。

| 対象 | 方針 |
|---|---|
| 300ms grace／timestamp／旧 pending-event API | 撤去済み。復活させない |
| 確定 Enter 除去／IME 専用 Tab lock | 撤去済みのまま |
| 全域 Tab 方針 | 保持 |
| dialog Enter／Escape helper | 保持 |
| viewport 別 composition owner | 保持 |
| 空 Preedit 補完 | 実 TextEdit buffer の取消回帰を照合して判断 |
| focus recovery／次 pass 限定 claim | 本家と重複する条件を特定してから縮小 |
| Win32 0xE5 press／repeat 除外 | 保持 |
| Hidden／Forwarded | 保持 |
| 非表示化に伴う composition 終端 | §4.2の受付契約と一体で保持 |

本家 TextEdit の Enabled／Disabled 自体は composition text の置換対象ではない。空 Preedit 補完が不要になったとは判断しない。根拠: `egui-0.37.0/src/widgets/text_edit/builder.rs:1417`。

#### egui-winit を削除できない理由

本家0.37は `allow_ime=false` で判定を通過し、Process の physical key を HashSet へ入れ、対応 release を遮断する。許可解除時は集合を clear する。

根拠: `egui-winit-0.37.0/src/lib.rs:483`、`:656`、`:1258`。

現 vendor は、一度配送した press を後続 Process repeat で Hidden に戻さず、release を allow_ime／logical に依存せず終端する。通常 press→Process repeat、および Process→通常 repeat→Process repeat の配送保証が本家と異なる。

trace は既定無効、文字秘匿、worker I/O を保持する。非表示入力受付を加える際も IME と別の矛盾した押下状態を作らず、同じ physical key owner の配送履歴に揃える。

Window の移動は既定 `OnTouch` を評価する。本文 drag を減らす場合は利用者相談とする。

### 4.6 Modifiers イベント化、keymap、clipboard

`RawInput.modifiers` はなくなり、`Event::ModifiersChanged` が level state を更新する。イベントごとの modifiers は残る。

根拠: `egui-0.37.0/src/data/input/event.rs:94`、`src/input_state/mod.rs:434`、`egui-winit-0.37.0/src/lib.rs:548`。

| 影響箇所 | 内容 |
|---|---|
| `src/app.rs:59309` | OS resync |
| `src/key_input.rs:2459`、`:2466` | 合成入力 |
| `src/video/native_presenter/render_core.rs:2716`、`:9780`付近、`:12505` | native queue／独立 Context |
| `src/ui_fullscreen.rs:67268`、`src/ui_adjustment_panel.rs:10985` | テスト入力 |
| `src/ui_erase.rs:815`、`src/ui_conceal.rs:742`、`:1341` | stale 対策 |
| `src/ui_adjustment_panel.rs:12014`、`:12510` | Space／Ctrl 編集 |
| `src/keymap.rs:7468`、`src/keyboard_input.rs:305` | exact chord／consume |

producer は modifier 変化を対象入力より前に並べる。frame 末尾に最終値を一つ追加するだけにしない。OS resync は過去の離散イベントの修飾キーを書き換えない。

FS の OS 直読み、focused permit、物理ラッチ、左右Ctrl、Enter／NumpadEnter を更新だけで撤去しない。ModifiersChanged は失われた KeyUp を保証しない。新しい Key を理由に keymap の保存名・既定割当を変更しない。

#### Ctrl+Shift+C/X/V

現 vendor の判定は `modifiers.command` を使うため Shift を許す。本家0.37は X/C/V に `command_only()` を使い、Shift／Alt を拒否する。

- 旧判定: `vendor/egui-winit/src/lib.rs:2314`
- 新判定: `egui-winit-0.37.0/src/lib.rs:1538`
- native 入力欄: `src/video/native_presenter/render_core.rs:9863`。Ctrlかつ非AltでShiftを許す。

**推奨は、Ctrl+Shift+C/X/V を通常窓と native 入力欄で保持すること。** 通常窓だけ本家の狭い条件へ変更しない。

Windows の text clipboard 判定を、Ctrl、Alt、Shift、対象キーから決まる共通の純粋な契約へ揃える。実 clipboard I/O は各 integration に残す。Process filter、IME owner、キーの press／release、空 clipboard 時の二重 Text 防止を保持する。

Ctrl+Alt 系や Insert／Delete 系は、既存コードの別条件を確認して扱う。今回の必須互換範囲から外れる条件を、ついでに拡大・縮小しない。

本家の image paste 等の新機能を取り込む際も、text clipboard の従来操作を壊さない。挙動縮小を採るなら、通常窓・native の両方と利用者向け説明を含む明示決定を先に得る。

### 4.7 Id、scope、永続化

- `Id::new` は `Id::unique` を呼ぶ非推奨 API。
- `AsId` は非推奨として残る。
- `AsIdSalt`／`IdSalt` が導入され、`Id::with` の内部ハッシュ構成が旧版と異なる。
- `Ui::id` は `scope_id`。
- `From<&str> for Id` を使う移行にはしない。

根拠: `egui-0.37.0/src/id.rs:8`、`:91`、`:102`、`:121`、`:142`、`src/id_salt.rs:43`。

`Id::new` を残しても child Id まで旧値と同じとは限らない。非推奨警告保留は永続互換対策にならない。

root identity は `Id::unique`、局所塩は `IdSalt`。`ui.id()` を機械的に `unique_id()` へ変えず、scope の意味なら `scope_id()` を使う。生成側と lookup 側を同時に移す。

代表: `crates/egui-ime/src/lib.rs:159`、`src/ui_music_panels.rs:706`、`:1563`、`tests/ui_snapshot.rs:443`、`tests/book_composition_editor_snapshots.rs:405`。

Id 非推奨は切替直後の短期間だけ許容し、後段で整理する。crate 全体の `allow(deprecated)` は置かない。

現 manifests・確認範囲では eframe persistence は有効化されておらず、独自の egui memory 保存も見当たらない。本家保存処理は feature の内側である。根拠: `eframe-0.37.0/src/native/epi_integration.rs:470`。

settings.db を初期化・全面 migration しない。実装時の graph 確認でリリース済み Id 永続経路が追加発見された場合だけ、旧読込・変換・互換試験を設計する。独自窓配置保存と session 内 memory を区別する。

### 4.8 vendor パッチ台帳

分類:

- **(a)** 本家に同等の仕組みがあり旧差分を外せる。
- **(b)** mIV の保証・責務が残り移植する。
- **(c)** 旧実装が不要。機能は新 API へ移す。

元ソースとの差は、eframe src が9ファイル・約2270行追加／124行削除、egui-wgpu src が9ファイル・約2004行追加／192行削除。README の項目だけを移植台帳にしない。

#### egui

| 項目 | 分類・方針 |
|---|---|
| release と click count の射影 | (b) `vendor/egui/src/input_state/mod.rs:1485`。順序・非click・focus・discard 回帰を保持 |
| `discard_pending_scroll` | (b) 同 `:595`。新 WheelState の所有に合わせる |
| repaint delay witness | (b) `src/context.rs:1794`、feature。observer だけでは同じ getter を代替しない |
| 非表示入力の状態終端 | (b・今回追加設計) §4.2の限定処理。通常操作を生成しない回帰を追加 |
| 上記テスト | (b) 新状態・イベント構造へ移植 |

新 wheel 所有箇所は `egui-0.37.0/src/input_state/mod.rs:228`、`wheel_state.rs:54`。PATCHES に未記載の repaint witness 等も追記する。

#### egui-winit

| 項目 | 分類・方針 |
|---|---|
| Process 除外の基本機構 | (a) 新本家を土台 |
| Hidden／Forwarded、release／focus 終端 | (b) 保持 |
| trace・秘匿 | (b) 保持 |
| 配送／State 網羅回帰 | (b) 新 modifier／clipboard 経路へ接続 |
| Windows clipboard 互換 | (b) Ctrl+Shift を含む §4.6の契約 |
| 非表示受付 | (b・今回追加設計) window 単位の生成・保留境界 |
| 旧300ms補正 | (c) 撤去済み。戻さない |

#### eframe

| 項目 | 分類・方針 |
|---|---|
| #7905 hidden repaint 基本修正 | (a) 新本家採用、旧 backport を重ねない |
| dirty／painting、surface generation、outer phase | (b) `native/run.rs:45`、`:118`、`:425` |
| 非ゼロ resize の inline paint provenance | (b) 同 `:1282`、`native/wgpu_integration.rs:978` |
| 初回 visible commit／Frame getter | (b) `src/epi.rs:736`、`native/epi_integration.rs:305` |
| startup window observer | (b) `src/startup_window_observer.rs:8` |
| max_texture_side 統一 | (b) `native/wgpu_integration.rs:735`、`:1235`。同等性を回帰で確認してから外す |
| 非描画・消滅 viewport の delta 配送 | (b) 同 `:828`、`:1313`。§4.9の共通 owner へ接続 |
| immediate child screenshot | (b) 旧 Event 方式は(c)、callback で機能を維持 |
| 通常版 immediate viewport 計測 | **(b)** 同 `:1206`、`:1396`。段階別時間、delta bytes、遅い delta 付き描画ログを保持 |
| test-script window witness／capture・focus 診断 | (b) callback-local identity を保持 |
| pending RawInput の受付・終端 | (b・今回追加設計) §4.2。復帰 frame の一括消去にしない |
| 実ウィンドウ試験の隔離 | (b) §6.2。試験を削除しない |
| manifest／Win32 feature／standalone patch | (b) 新原本を基準 |

通常版 immediate 計測は startup／test-script 診断と別責務である。現実装は、遅い描画かつ delta bytes がある場合を記録する。移植で常時ログ・常時 I/O を追加せず、renderer の新 delta 構造に合わせる。

本家 pending delta は `eframe-0.37.0/src/native/wgpu_integration.rs:795`、`:1290`。保持だけで消費完了とは判断しない。

#### egui-wgpu

| 項目 | 分類・方針 |
|---|---|
| managed mipmap／再生成／sampler | (b) `src/renderer.rs:712`、`:777`、`:1151` |
| 公開 MipmapGenerator／texel count | (b) `src/mipmap.rs`、`src/mipmap.wgsl`、`src/lib.rs:35` |
| atlas 診断／renderer identity／texture_size | (b) 新 delta 列へ適応 |
| 無描画 delta 適用 | (b) `src/winit.rs:523`。drain／submit／free まで完結 |
| `set_window(id,None)` の対象 surface のみ破棄 | (b) 本家も全 surface clear のため保持 |
| startup GPU stage／root present observer | (b) `src/startup_diagnostics.rs`、`src/lib.rs:322` |
| screenshot／readback 診断 | (b) callback 方式へ |
| shader 空行等 | (c) 再適用しない |
| license／テスト接続 | (b) 新原本を基準 |

本家 surface 差: `egui-wgpu-0.37.0/src/winit.rs:203`、`:218`、`:229`、`:247`。

既存 atlas overflow の skip-write は新しい根治策として拡張しない。診断を保ち、配送不変条件の回帰で overflow が生じないことを受け入れる。既存 skip 処理の撤去可否は履歴を含め別判断する。

#### 取り込み手順

1. 新版原本、checksum、VCS、license を記録。
2. 新原本へ論理項目ごとに必要差分を付け直す。
3. 旧 manifest をコピーせず新 manifest を基準にする。
4. root／standalone が同じ新版実体を使うことを確認。
5. PATCHES／README を実差分と一致させる。
6. `.cargo-ok`、target 等を取り込まない。
7. 他の DLL・モデル・SDK を巻き込まない。reparse point を作らない。

### 4.9 wgpu、texture delta、native presenter

#### 現行経路と Windows 依存

実コードに直接 hal／windows_058 の利用は見当たらず、旧共有経路のコメントが残る。現 native presenter は D3D11 `OpenSharedResource1` を使う。

- `src/video/native_presenter/render_core.rs:8347`、`:8379`
- 動画 Present: 同 `:6708`
- overlay surface: `src/video/native_presenter/overlay_gpu.rs:43`

未使用の直接 `wgpu-hal`／`windows_058` 除去とコメント訂正は0.33.3上へ前倒しする。未使用 `windows_062` 別名を追加しない。hal30を将来直接使う場合の Windows crate は0.62系。根拠: `wgpu-hal-30.0.1/Cargo.toml:518`。

D3D11 本体の windows0.61は今回全面更新しない。

#### 影響箇所と API

| mIV 箇所 | 内容 |
|---|---|
| `Cargo.toml:368` | wgpu30 |
| `src/lib.rs:495`、`:526` | surface 設定 |
| `overlay_gpu.rs:31` | Instance／DeviceEpoch |
| `render_core.rs:12089` | configure |
| 同 `:13840`、`:13957` | delta の適用・free・消費 |
| 同 `:13891`以降 | render pass／submit／present |
| `src/compare_wgpu.rs:274`、`:539` | pipeline／callback |
| `src/panorama_wgpu.rs:332`、`:669`、`:506`、`:833` | pipeline／mipmap／callback |
| `src/gpu_lanczos.rs:680`、`:708` | 縮小 |
| `build.rs:140`、`:230`、`Cargo.toml:495` | 独立 Naga27→HLSL→DXBC |

必須差分:

- Instance descriptor を値渡し: `wgpu-30.0.1/src/api/instance.rs:63`
- bind group layout の Option 配列／`immediate_size`: `src/api/pipeline_layout.rs:38`、`:44`
- `multiview_mask`: `render_pipeline.rs:238`、`render_pass.rs:682`
- color space: `surface.rs:90`
- `CurrentSurfaceTexture`: 同 `:148`
- `Queue::present`: `queue.rs:377`
- `WgpuConfiguration.surface`: `egui-wgpu-0.37.0/src/lib.rs:325`

Surface／DeviceEpoch の寿命と configure write lock、submit/present read lock を維持する。新 present を gate 外へ出さない。surface 状態だけで device-lost latch を立てず、新しい retry を作らない。

#### TexturesDelta の所有・消費契約

新版は set が ID ごとの delta 列、free が set。非空 drop は debug assertion になる。

根拠:

- `epaint-0.37.0/src/textures.rs:284`、`:315`、`:335`
- `egui-0.37.0/src/data/output.rs:72`

通常 API は `&mut TexturesDelta` または所有権移譲を受け、**借用適用しただけの非空出力を返さない**。diagnostics は借用観測し、適用権を持つ clone を作らない。

| 経路 | owner と完結条件 |
|---|---|
| 通常 eframe 描画 | backend が FullOutput から既存 pending owner へ移譲。renderer が set を順番に drain、描画 command を submit、その後 free を drain |
| 通常 native overlay | `render_core` の描画 batch が所有。借用二重ループだけでなく set／free を消費する |
| surface なし・hidden 等の無描画配送 | 共有 renderer があれば共通無描画処理が set を drainし upload を submit、当該 batch の描画を行わないことを確定後 free を drain |
| renderer 未準備・存続する出力の早期終了 | 所有権を既存 backend pending／logical batch owner に引継ぐ。未適用のまま局所変数を drop しない |
| acquire 不成立 | 適用済み set と未処理 free を区別。upload を submitし、描画を中止する batch を完結。続行する owner へ引継ぐ場合は残りだけを移す |
| viewport／window 消滅 | 共有 renderer は存続し得るため、delta を当該 viewport と一緒に捨てない |
| renderer／Context の明示終了 | 今後の配送先がないことを owner が確定し、GPU資源・batchを対象単位で終端。診断付きの明示破棄に限る |
| 描画不要の headless test | 必要な output を検査した後 `FullOutput::drop_without_applying_deltas()` |
| 実描画／upload を検証する test | 実 consumer で適用・drain。明示破棄で成功扱いしない |

whole→partial、同一IDの複数partialの順序を保持し、map全体の反復順序へ依存しない。free は当該frameの使用が submitされた後に行う。

早期 return／`?` より前に、delta を完結責任のある owner へ移す。再利用する Context の出力を headless API で捨てて製品を続行することは認めない。

`clear()` はテストの意図的破棄や明示終了の最終処理としてしか使わない。製品で「assertを避けるため最後に無条件clear」は禁止する。

`let _ = ctx.run(...)` 等の test も変更対象。例: `src/ime_focus.rs:795`。出力を検査する test は shapes／platform／viewport 情報を読み終えてから破棄する。0.33上では互換可能な共通 test helper の抽出を先行できるが、新版の消費 API 接続は段3で行う。

#### 無描画配送と submission

本家は早期終了でも queue 内部の write buffer を滞留させない submission guard を持つ。根拠: `egui-wgpu-0.37.0/src/winit.rs:502`、`:514`。ローカル `apply_textures_delta` の旧 update/free だけでは足りない。

共通の完結処理に次を持たせる。

1. renderer がある場合、surface lookup より前に set を適用・drain。
2. mipmap 等で生成した command buffer を含めて submit。
3. command buffer がなくても queue write を行った経路は submission を完結。GPU待機は追加しない。
4. 通常描画を submit 済みなら重複する空 submit を追加しない。
5. free は当該 batch の submit 後。無描画確定時も、upload 等との順序を揃えて drain。
6. acquire／capture 等の途中 return でも、同じ batch owner が完結または引継ぎを行う。
7. 引継ぎで set を再適用せず、未処理部分だけを渡す。

本家の通常 free は submit 後に drain される。根拠: `egui-wgpu-0.37.0/src/winit.rs:768`以降。

検証は「texture サイズが変わった」だけにしない。headless GPU で no-surface、acquire不成立相当、whole＋partial、mipmap、free-only を通し、drain／引継ぎ、submission、readback、解放順を確認する。renderer 終了時以外の未適用破棄がないことを検査する。これは段2bと段3の受入条件である。

#### 表示・性能

AutoVsync／latency1の既存明示設定と環境変数の意味を新 SurfaceConfig へ移す。stencil 対応を理由に新たに buffer を要求しない。callback の format／sample 数を揃える。

RGBA atlas へ diagnostics を合わせる。根拠: `epaint-0.37.0/src/image.rs:16`。

D3D11 用 Naga27→HLSL→DXBC は別経路なので、wgpu30への更新だけで同時更新しない。WGSL の検証と native 生成物の検証を分ける。

### 4.10 egui_kittest、accessibility、snapshot

新版は visible input の accessible name を検査し、read-only を除外する。label／placeholder を認識する。

根拠: `egui_kittest-0.37.0/src/accessibility.rs:14`、`:26`、`:46`、`src/builder.rs:151`。

**検査は settled 後と画像生成前に行われる。** 候補画像を収集してから名前を直す順序では成立しない。根拠: `src/lib.rs:518`、`src/snapshot.rs:739`。

影響箇所:

- `tests/ui_snapshot.rs:28`
- `tests/book_composition_editor_snapshots.rs:132`、`:247`付近以降の snapshot
- `tests/support/ui_harness.rs:14`
- `tests/support/scroll_layout.rs:6`
- lib内の snapshot／描画 fixture
- `src/ime_focus.rs:1268`等の raw TextEdit allowlist

kittest は0.3→0.5。NodeT／`accesskit_node()` は残るが、container の名前・scope変更で query の一致先が変わり得る。

根拠: `kittest-0.5.0/src/node.rs:9`、`egui_kittest-0.37.0/src/node.rs:21`。

方針:

- 名前付与は本体描画側へ行い、0.33上で前倒しする。
- 0.37で増えた検査対象は切替後に候補画像収集前に補完する。
- 共通 builder の check を無効化しない。
- role／親 container で query を限定し、固定 node ID 依存を増やさない。
- 最終passの geometry を採取する。
- DX12|VULKAN、lazy、CPU adapter 優先、renderer 独立を維持する。

更新順序:

1. accessibility を修正し、画像未生成でも検査が完走する状態にする。
2. 旧 baseline を更新せず、両 integration target と lib snapshot の通常比較で候補を生成。
3. 文字ラスタ差、文字幅・行高の差、panel／margin差、操作回帰へ分類。
4. 入力・geometry回帰を直し、外観を採択。
5. 同じ renderer／font方針で全対象を更新。
6. 更新モードを外し、同じ全対象を通常比較。
7. 一台帳に画面群、共通理由、対象ファイル集合、例外、確認者と結果を記録。

445枚は対象漏れ確認の基準にする。目視は変更対象全体をカバーするが、各PNGへ重複した担当欄を作る必要はない。contact sheetを索引にし、文字位置・欠け・例外差分を原寸で見る。入力・到達性の検査をPNGへ置き換えない。

libの対象は `snapshot` というテスト名だけで決めず、実際の PNG 生成呼出と fixture の呼出元から列挙する。列挙した filter を台帳のコマンド欄へ一度だけ記録する。

### 4.11 その他の挙動と複数 viewport

| 変更 | 対応 |
|---|---|
| popup再表示の sizing pass | 計測passで保存・選択commandを発行しない |
| press-open後のrelease即閉じ防止 | 右click、外側終了、wheel owner終端を検査 |
| menu hover-open | menu barの選択・wheel消費を維持 |
| ScrollArea端fade | 旧外観維持を初期案。採用は比較後 |
| Window margin／min size | 小画面の本文・固定操作行を再計測 |
| tooltip／container | 入力ownerやqueryへ混ざらないことを検査 |
| repaint-after削減 | 予定外frameに依存する完了処理を直す。追加repaintで旧頻度に戻さない |
| rootのみ保存／復元 | 独自配置と二重所有しない |
| raw mouse motion | unfocused受信をkeyboard ownerの根拠にしない |
| Screenshot callback | 既存batch owner／worker I/Oへ接続 |
| set_fonts即時適用 | 不要な初期frame追加を整理し、atlas／galley失効を検査 |

根拠: `egui-0.37.0/src/containers/popup.rs:629`、`:660`、`menu.rs:440`、`scroll_area.rs:1704`、`window.rs:633`、`:696`、`context.rs:133`、`:2505`、`:4557`、`eframe-0.37.0/src/native/wgpu_integration.rs:511`、`:903`。

撮影側は `src/test_script/capture.rs:282`、`:375`。callback内でPNG encode／I/Oを行わない。

#### detached凍結

今回の変更は viewport寿命、配置、状態所有へ触れる。以下は行わない。

- rect 捕捉条件追加
- geometry由来 recreate
- 新 pending bool
- 時間窓での入力遮断
- 兄弟cache／surface／入力の一括reset

App分離、非表示入力受付、callback Ui化、scheduler／delta／surface移植は、着手前に ClaudeCode と独立 Codex reviewer が構造修正として合意し、`docs/detached-rework-plan.md` §11へ範囲と理由を記録する。本改訂案を合意済みとは扱わない。

### 4.12 Rust 1.99警告と MSRV

既存ログは f32 fallback445件／36ファイル、lib全体688警告。今回実行した結果ではない。

代表: `src/ui_main.rs:184`、`src/ui_text.rs:519`、`src/ui_adjustment_panel.rs`、`src/ui_fullscreen.rs`、`src/video/native_presenter/overlay_draw.rs`。

| 対象 | 方針 |
|---|---|
| Stroke／PathStroke | 更新後に再集計。0.37の幅はf32 |
| その他の曖昧float | 必要な境界へ局所型注釈 |
| 旧vendor警告 | 新原本で再評価、旧修正を重ねない |
| fetch_update | egui更新では解消しない。MSRV互換を確認 |
| dead code | feature／test／Windowsを区別。機能削除で減らさない |

根拠: `epaint-0.33.3/src/stroke.rs:27`と`epaint-0.37.0/src/stroke.rs:26`、`:140`。

Atomic代表: `src/log_writer.rs:105`、`:165`、`src/everything_ipc.rs:226`、`src/ime_focus.rs:225`、`src/similar_index.rs:1431`、vendor witness。

workspace MSRVは少なくとも1.95へ更新する。開発toolchain1.99固定とは分け、1.95を実際に検証して宣言する。`try_update` の最小安定版を未確認のまま採用してMSRVを引き上げない。必要なら旧APIと対象限定の警告説明を残す。全crateの警告抑制は行わない。

### 4.13 Remote／他crate

| 対象 | 波及 |
|---|---|
| egui-ime | 版、入力owner、helper、testを移行 |
| 4ラボ／vst3-host-tester | App／Ui／panel／RawInputを同じ切替段へ |
| remote-web／remote-ipc | egui直接移行・protocol変更は不要。hidden中の応答を検査 |
| launcher | egui非依存。MSRVと埋め込みcoreの更新 |
| epub-pdf-worker | egui移行なし。companion build対象 |
| comic-core | 独立ラスタライザを維持 |
| local-adjust-core／music-core | egui非依存維持 |
| Susie／LibRaw／ORT／VST bridge | 今回ABI更新を含めない |

ラボの旧eguiと本体の新版型を分裂させない。manifestと必要APIを同時に更新する。

## 5. 利用者に相談する事項

0.37を目標にする決定は受領済み。以下は比較可能な結果を用意して確認する。

| 項目 | 推奨 |
|---|---|
| 字面／baseline／hinting | fallback・保存値を保持して補正を実測整理 |
| color emoji | user textの代表比較を提示 |
| Window本文drag | 必要箇所で従来操作を保持 |
| mouse drag scroll | 旧版で使えた画面は保持 |
| Panel collapse／double-click | 既存操作維持を初期案 |
| 端fade | 旧外観維持を初期案 |
| margin／min size | 到達性を保証した比較案 |
| system font fallback | 既存明示chainを基準 |
| **Ctrl+Shift+C/X/V** | **通常窓・native入力欄とも従来操作を保持**。本家そのままでは通常窓だけ縮小するため明記 |

従来clipboard保持は移行の推奨仕様であり、保持するために版選択の再確認は求めない。縮小案を採る場合だけ、具体的な操作差と費用を示し着手前に相談する。

Ctrl+Q追加、OS直読み撤去、動画描画方式変更、パネル本文省略は自動採用しない。

## 6. 段階設計

### 6.1 共通条件

各段で通常製品ビルドを通し、対象自動検証と残る検証を記録してコミット可能にする。旧baselineの不一致は候補差分であり、成功と記録しない。

依存切替には型が結合する原子範囲がある。旧新版を同一製品graphで混在させる互換層は作らない。レビューは実際の差分の領域・規模に応じて分割し、固定数の票を要求しない。原子範囲がまだ大きすぎる場合は、旧版上の挙動不変抽出を追加する。

規模は推定。原本置換、lockfile、PNG、純移動と実ロジック変更を別集計する。

### 6.2 段0a: 実ウィンドウ試験を自動gateから分離

**最初に実施する前提修正。現 `test-full.ps1` を非対話gateとして先に実行しない。**

`scripts/test-full.ps1:85`の eframe全libには、非ignoredの `synchronous_damage_returns_before_outer_paint_gate` が含まれ、子プロセスで `run_native` を実行する。

根拠: `vendor/eframe/src/native/run.rs:782`、`:809`、`:828`。

AGENTS の規則は、実アプリ起動・foreground切替・入力の前に具体シナリオ、時間、desktop使用、使い捨てdataを示し、明示承認を待つこと。testコマンドという名前、短時間の窓、子プロセスであることは例外にならない。

**範囲**

- live testを既定の自動対象から明示分離する。削除・条件緩和はしない。
- ignored等の実行区分と承認付き専用入口を設け、未実施を検証台帳に残す。
- pure scheduler testは引き続き自動実行。
- 他のdefault testから live入口へ到達しないことを監査。
- test-fullのコメント・文書を実区分に合わせる。

既存 standalone eframe testの実行を承認だけでエージェントへ解禁しない。エージェントがUI検証する場合は、同等の検証を正規の disposable portableシナリオへ載せ、`target/portable-smoke/mimageviewer.exe`と同dataだけを使う。standalone窓試験を残す場合は利用者実行の隔離試験として扱う。

**受入条件:** core check、vendor testのcompile、pure scheduler回帰、runner区分の静的／fake検査。窓は起動しない。

**規模:** 3～6ファイル、80～220行。live結果は段7待ち。

### 段0b: 基準固定

source、toolchain、feature、native fixture、警告、PNG、既知失敗を記録する。detached構造合意の範囲を確定する。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt --check
```

旧版焦点テストと有効な台帳を利用する。過去のAppサイズ制限失敗を現在の失敗と混同しない。上限を緩めて緑にしない。

**規模:** 文書・台帳3～5ファイル、100～250行。

### 段1a: 0.33.3上の小さい準備

- 未使用の直接 `wgpu-hal`／`windows_058` 除去。
- 旧コメント訂正。
- 本体描画側の accessibility 名を付与。
- 文字を追加してlayoutを変える必要がない箇所は、label関連付け・既存説明を利用。
- 新tooltip等の可視差を伴う場合は分けて確認する。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo fmt --check
python scripts/check_ui_glyphs.py
```

変更したUIの焦点試験と旧PNG通常比較。名前の付与だけなら画像不変を確認。manifest変更を含むため、適切な区切りで `test-lib-full.ps1` を実行する。

**規模:** 12～30ファイル、180～500行。複数の小コミットへ分割可能。

### 段1b: 0.33.3上の挙動不変抽出

- service／input／render／post-renderを抽出し、旧updateから同じ順で呼ぶ。
- root／mounted／parkedのservice ownerを明記。
- wheel投影の純関数、clipboard判定、出力完結・headless破棄呼出の共通入口を抽出。
- backend／Win32の非表示受付・終端に使うowner境界を切り出す。
- ここでは新仕様による入力除外を無検証で先行導入しない。

```powershell
cargo test -p mimageviewer --lib tray
cargo test -p mimageviewer --lib startup
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib sidecar_restore
.\scripts\test-lib-full.ps1
```

core check、旧snapshot、境界純関数・状態遷移回帰を含める。新消費契約は新版切替と同時に成立させる。

**規模:** 15～30ファイル、700～1500行。Appの抽出と入力／描画の抽出を分割する。

### 段2: 新版vendor再構築

準備用の実ディレクトリ／manifestで検証し、rootをまだ切り替えない。

| 小段 | 範囲 | 推定規模 |
|---|---|---|
| 2a | eguiの既存API・状態終端、egui-winitのIME／clipboard／非表示受付 | 10～18ファイル、650～1300行 |
| 2b | egui-wgpuのmipmap、delta drain／submit／free、surface、診断 | 10～16ファイル、1100～2100行 |
| 2c | eframe scheduler、visible commit、保留入力、viewport、通常版計測、診断 | 11～18ファイル、1400～2700行 |

upstream置換行数は別計上。

**受入条件**

- standaloneの**非対話区分**lib test。
- Process／repeat／release／focus／allow_ime、Ctrl+Shift clipboard。
- 非表示の受付／終端、同一batch内の復帰新規入力、visible child。
- delta whole／partial／free、no-surface、submission、headless readback。
- per-window scheduler、outer phase、hidden idle、interactive resize。
- pure testで状態・順序を確認し、実Window結果とは呼ばない。
- featureなし／診断featureを分ける。
- 旧rootのcore checkを維持。
-全ローカル差分を台帳に記載。

live testのcompileは行えるが実行は段7。

### 段3: 依存と必須契約の原子的切替

**範囲**

- egui系0.37、wgpu30、ラボ、egui-ime、kittest。
- App trait、root Ui、panel、Context改名、on_exit。
- ModifiersChanged、wheel投影、source／phase。
- backend保留入力とWin32 edgeの非表示受付・終端。
- native overlay／pipeline／surface／present。
- **全製品delta消費・引継ぎと、headless出力の明示破棄。**
- clipboard互換とscreenshot callback。
- 全test targetのcompileに必要なAPI移行。
- 新版accessibilityの追加欠落を候補生成前に補完。

P1の二契約は段4へ残さない。

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

加えて、段2で固定した入力・deltaの回帰を実際の接続点で実行する。compileだけでは受け入れない。graphの旧egui混入、型分裂、古いpathを確認する。

**再見積り**

| 内訳 | 概算変更行 |
|---|---:|
| App／Ui／panel／一般API／ラボ | 1200～2300 |
| event／wheel／native GPU接続 | 650～1300 |
| delta消費／headless破棄接続 | 450～1000 |
| 非表示入力のbackend・Win32接続 | 400～700 |
| 合計 | **2700～5300** |

**100～170ファイル程度**。原本・lockfile・PNGを除く。旧見積りから準備の重複は減るが、欠落していた消費・非表示契約が増えるため、総行数が必ず小さくなるとは見込まない。実装前に抽出後の差分で再評価する。

### 段4a: 入力・非表示・popupの等価経路確認

段3で成立した契約を、close／cancel／focus loss／repeat／復帰新規入力、root／FS／native／兄弟contextへ広げて確認する。

```powershell
cargo test -p mimageviewer --lib ime
cargo test -p mimageviewer --lib focus
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib keymap
cargo test -p mimageviewer --lib specified_scale
cargo test -p mimageviewer --lib fullscreen_overflow
cargo test -p mimageviewer --lib navigator
```

wheelはPoint／Line／Page、横縦修飾、混在修飾、consume前後、popup終了後の残量と新規wheelを含める。clipboardは通常／nativeでCtrl+C/X/V、Ctrl+Shift+C/X/V、既存Insert/Delete、IME、空clipboardを比較する。

filterはsubstringであり実選択件数を記録する。PNGを含むfilterの外観差と機能回帰を区別する。

**規模:** 10～22ファイル、400～1000行。

### 段4b: フォント・layout採択

baseline／fallback／toolbar／HUD、preview、margin／separator／小画面を確認する。

```powershell
cargo test -p mimageviewer --lib ui_fonts
cargo test -p mimageviewer --lib ui_font_catalog
cargo test -p mimageviewer --lib calculated_
cargo test -p mimageviewer --lib startup_dialog
python scripts/check_ui_glyphs.py
```

Fits／Scrolls、末尾到達、固定操作のviewport／clip内到達を維持。既存108条件等を減らさない。§1.380の残件と今回退行を分ける。

**規模:** 8～18ファイル、400～1000行。previewが大きければ独立小段。

### 段4c: GPU・native HUD整合

```powershell
cargo test -p mimageviewer --lib compare
cargo test -p mimageviewer --lib panorama
cargo test -p mimageviewer --lib gpu_lanczos
cargo test -p mimageviewer --lib native_presenter
cargo test -p mimageviewer --lib bottom_hud
```

delta・mipmap・callback、透明合成、font／DPI／galley cache、surface寿命、submit gateを検査する。診断featureの必要なheadless testも実行する。実Windowの試験は分離したままにする。

**規模:** 8～16ファイル、300～800行。

### 段5: 全snapshot更新と警告整理

accessibilityを通した後、通常比較で候補を収集し、外観採択後に更新する。

対象は以下の**すべて**。

1. `ui_snapshot`
2. `book_composition_editor_snapshots`
3. PNG生成呼出から列挙したlib snapshotのfilter集合

更新専用の子PowerShellで実行する。環境変数は全対象の終了後に復元する。各コマンドの非ゼロ終了は伝播させる。

```powershell
$env:UPDATE_SNAPSHOTS = '1'
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 台帳に列挙したlib filterを、同じ更新環境で順に実行
```

更新プロセス終了後、変数なしの別プロセスで両integrationと**同じ全lib集合**を通常比較する。

```powershell
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 同じlib filter集合を更新なしで再比較
```

上のコメント部分は実行時の任意選択ではない。段0～1で生成箇所とtestを対応付けた一台帳から、具体コマンド列を確定して実行する。`--lib snapshot`だけで全件と見なさない。

画面群ごとに旧／新／diffと共通理由・例外を確認する。Id非推奨と残るRust警告も整理する。

**規模:** UI／test10～30ファイル、250～800行、PNG最大445枚。前倒し済みaccessibilityを再計上しない。

### 段6: 非対話全体gateと確認用build

**段0aの分離が完了したrunnerを使う。** 未修正の現test-fullをそのまま実行しない。

```powershell
cargo fmt --check
python scripts/check_ui_glyphs.py
.\scripts\test-lib-full.ps1
.\scripts\test-full.ps1
.\scripts\build-dev.ps1 -PreserveRuntime
```

通常feature、必要なtest-script、vendor非対話区分、MSRV、core／Remote／EPUBを確認する。

- 台帳にsource、command、feature/profile、fixture、exit、件数、ログを記録。
- 同じ条件の成功結果は再利用。
- focused runnerの広域変更exit2や0件選択を成功としない。
- 現在も存在する既知失敗を除外・閾値緩和で隠さない。
- 自動gate成功と、分離したdesktop試験未実施を別に報告。
- build/checkは最低10分、広域testは最低15分以上、重い既存試験には追加余裕。
- timeout後のBrokenPipe等を製品失敗と誤分類しない。

**規模:** 文書・検証補修3～8ファイル、100～300行。

### 段7: 承認付きdesktop検証・利用者実機受入れ

エージェントによるdesktop検証は、具体suite・時間・PC使用・使い捨てdataを提示し、明示承認後だけ行う。承認は今回の改訂依頼から推定しない。

- エージェントは正規portable-smoke成果物のみ。
- standalone vendor窓試験を残す場合は利用者実行の隔離試験として手順を用意。
- scheduler live試験は短い独立枠。環境不成立・未実施をPASSへ変えない。
- native／DPI等の製品受入れは、段6の通常buildを利用者が起動。
- UI確認20～35分程度と、必要なReleaseChecks／live試験を分けて見積る。

## 7. 実機確認手順

通常版・トレイ常駐版を先に終了する。single-instance mutexを共有する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

既定で実利用中の `%APPDATA%\mimageviewer` を開き、設定・データを更新し得る。隔離確認を明示的に選ぶ場合は同じ成果物へ `--data-dir .\target\dev-runtime\data` を渡す。エージェントは通常profileを起動しない。

### 7.1 フォント・ダイアログ

1. 既定、BIZ UDPGothic9pt、Meiryo10pt、Meiryo UI10ptで一覧・詳細・検索・metadata・toolbarを表示。
2. 開発側fixtureの日本語、Latin、数字、CJK、数学英字、記号、emojiを比較。
3. baseline、字面、折り返し、欠け、豆腐化を確認。
4. 動画／音楽HUDの時間、`x1`、`Norm`、dBを確認。
5. 小窓・UI倍率200%で長文dialogを開き、本文・固定ボタン・×へ到達。
6. 採択したWindow移動・mouse／touch scroll・panel操作を確認。

### 7.2 日本語入力

Google日本語入力とMS-IMEで少数の共通シナリオを行う。

1. 注釈本文をEnter確定。確定だけで改行しない。
2. 直後の通常Enterで改行が1つ入る。
3. 検索欄の確定直後Enterで検索。
4. 変換中Escape／候補Tab／Spaceがアプリ操作へ漏れない。
5. 確定直後の通常Escapeが働く。
6. composition中に別窓へfocusを移し、重複・消失・キー残留がない。
7. 長い欄・複数行末尾で候補窓がcursorへ追従。
8. main／F12／nativeタグ入力の代表経路を確認。

### 7.3 shortcut・wheel・clipboard

1. main／F12でCtrl／Shift／Altを押して離し、通常操作へ戻る。
2. 編集のCtrl+矢印、Space pan、筆切替を確認。
3. Enter／NumpadEnter、右Ctrl、Tabの代表割当を確認。
4. popup内のwheel／Ctrl+wheelとclick／key終了で背面が動かない。
5. 終了後の新しいwheelは働く。
6. slider／seekを外で離し、次の操作へdragが残らない。
7. 通常入力欄とnative入力欄でCtrl+C/X/V、Ctrl+Shift+C/X/Vを比較。
8. 複数clipboard条件・IME終了直後でも二重文字が入らない。

### 7.4 非表示・複数窓・Remote

1. 非同期処理を開始してtrayへ隠し、完了後に復帰。
2. 復帰時に隠す前後のkey／wheel／clickが再実行されない。
3. 非表示化前のholdを非表示中に離し、復帰後に残留・release由来の確定がない。
4. 復帰直後の新しい入力は一度だけ働く。
5. root単独非表示と、visible F12ありの両方を確認。
6. root非表示でもvisible childの新規入力は働く。
7. 同じPDF／画像の兄弟reader片方を閉じ、もう片方の表示を保持。
8. root非表示中のRemote操作が応答・完了。
9. idle／wakeは既存ReleaseChecksの承認済み手順で確認。

hidden中の任意キーを実PCへ大量送信する試験は行わない。順序・同一batch・loop未実行の網羅はheadless回帰で担保し、実機は代表に絞る。

### 7.5 動画HUD・GPU

1. 再生、HUD文字、時間、速度、音量、音声トラック。
2. 820px程度から狭幅へ縮小し、重なり・menu・seek／音量drag。
3. pause／resume／seek／track変更／動画→音声→動画。
4. F11／F12往復、focus、透明合成。
5. 比較wipe／diff、panorama、縮小でmipmap・黒表示・境界を確認。
6. fullscreenから一覧へ戻り、黒サムネ・atlas破損がない。

### 7.6 マルチモニターDPI

1. 100%と150%／200%の画面間でmain／F12／動画窓を移動。
2. 字面・baseline、候補窓、HUD hit rect、slider／seek位置。
3. 最大化・復元・F11往復で操作が画面外へ失われない。
4. 子窓終了後もrootの表示・入力・撮影が継続。

## 8. 完了条件と文書更新

完了には次を必要とする。

- 新版graphとvendor全差分が説明できる。
- egui-winit保持理由と将来削除条件が記録される。
- texture deltaのdrain／引継ぎ／明示終了とsubmissionが全経路で成立。
- 非表示入力のbackend／Win32両境界、終端、新規入力、visible childの回帰が通る。
- Ctrl+Shift clipboardの従来操作を通常／nativeで保持。
- accessibilityを通した全snapshot対象を更新・通常比較・目視確認。
- 非対話gate、確認用build、必要な承認付きdesktop試験、利用者実機確認が完了。
- 未実施・環境不成立・未確認機器を確認済みと書かない。
- リリース済み機能・設定・データを黙って縮小・初期化していない。

更新対象は、display-pipeline、video-architecture、coding-conventions、keymap-spec、ime-process-keys-plan、ui-snapshot-policy、development-build-and-test、interactive-release-verification、detached-rework-plan §11、vendor台帳、backlog §1.382。§1.380は個別解決の証拠があるものだけ更新する。

採択した操作・外観変更はマニュアルと製品ページへ反映する。版更新だけを理由に未確認の改善・互換性を告知しない。