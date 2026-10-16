> 状態: 設計 第4.2版 (2026-10-16)。Sol (gpt-6.1-sol / xhigh) の独立レビュー 3 回と、利用者の希望による追加の独立レビュー Astra (gpt-6-astra / xhigh) 4 回を経て、Astra が ACCEPT (残る P1/P2 なし)。実装前。目標は利用者決定により egui 0.37 系。正本の backlog は §1.382。

# 改訂履歴

## 第4.2版 — 2026-10-16

対象は第4.1版、HEAD `ea54a4ca0`。現在の設計書、Astra再確認全文、本家・mIVの該当ソースを読み取り専用で照合した。前回のIME中断要求2件の対応を維持する。本版の独立レビュー、実装、ビルド、テスト、製品起動は未実施。

- **Astra再確認 P1 — 反映:** 最小化した子のCloseを、UI復帰待ちの通常入力から分ける。非描画の受付から既存のmodal判断・終了ownerへ一度だけ渡し、拒否もその要求の消費として扱う。承認後は子callbackや親の描画passを待たず対象viewportを退役させる。wgpu／glow、App接続、構造合意・§11記録、回帰・実機手順・見積りへ反映。

タスクバーから最小化した別窓を閉じる操作は通常利用で到達するため、まれな失敗として割り切らない。追加repaint、最小化中の子へのUI pass強制、新しいpending boolや時間窓は使わない。

## 第4.1版 — 2026-10-16

対象は第4版、HEAD `535968858`。Astra再確認全文と本家・mIVの該当ソースを読み取り専用で照合した。前回4件の対応を維持し、追加2件を反映する。本版の独立レビュー、実装、ビルド、テスト、製品起動は未実施。

- **Astra再確認 P1 — 反映:** Memory全体のIME中断要求を既存のviewport別Focusへ移管する。親だけ／子だけが要求する同一Contextの入れ子run_ui回帰を追加し、detached構造合意・§11記録の範囲へ含める。
- **Astra再確認 P2 — 反映:** 同一viewportの未配送PlatformOutputを集約するとき、最新のIME矩形等を採用しながら中断要求を保持する。実Context::run_ui内でdiscardする回帰を段2aへ追加する。

App側の補助フラグ、追加repaint、新しいqueue、retryは追加しない。段2aと段3の見積りを更新する。

## 第4版 — 2026-10-16

対象は第3.1版、HEAD `4e097de68`。現在の設計書、Astra追加レビュー全文、該当するmIV・本家ソースを読み取り専用で照合した。本版の独立レビュー、実装、ビルド、テスト、製品起動は未実施。

- **Astra P1-1 — 反映:** 単一の`pending_raw_input`をそのまま使う方針を訂正。既存の保留領域をviewport ownerへ移し、同じviewportだけが消費する。skipする子はStateから入力を取り出さない。交互実行・子のskip後復帰の回帰と、detached構造変更の合意・§11記録を追加。
- **Astra P1-2 — 反映:** 本家0.37が捨てる`winit::Ime::Disabled`を正規化まで届ける互換処理を追加。段2aの冒頭で、State変換から実TextEditまでの取消回帰を成立させる。
- **Astra P2-3 — 反映:** native経路のIME中断出力を、既存`NativeWindowIntent`・control配送・対象HWND ownerで処理する契約を追加。非対話回帰と実機手順を追加。
- **Astra P2-4 — 反映:** release射影の保証と本家クリック分類を分離。連続クリックの距離条件6 points・時間条件変更を相談事項へ追加し、旧判定や別タイマーを足さずfixtureを更新する方針を明記。

4件とも採用する。P1-1の修正は保留入力の所有境界に限定し、第2版で提案した汎用入力フィルタ・終端配送・一律pruneは復活させない。

## 第3.1版 — 2026-10-16

第3版の独立レビューはACCEPT WITH CHANGES。設計担当が次の2件を直接反映した。

- **第3版レビュー P2-1 — 反映:** 復帰後は保持中のgamepad軸値で操作を続ける現行挙動を保持。新しい中立通過要求は採らず、変更する場合は利用者相談とする。
- **第3版レビュー P2-2 — 反映:** logicでdrainした未配送edge・意味終端を同じ物理取込みownerで保持し、UI配送へ引き継ぐ。複数logicに分かれたpress→release、追跡ON／OFF、multi-passの既存契約を回帰対象へ追加。

本版でもこの2件を維持する。

## 第3版 — 2026-10-16

対象は第2版`b693945bb`。

- **再レビュー P2-1 — 反映:** SW_HIDE、最小化、visible child、RedrawRequested期限超過を区別。全入力共通の受付・終端配送とeguiへの汎用終端API追加を撤回。
- **再レビュー P2-2 — 反映:** gamepadの物理取込みとUI配送を分離し、WH_GETMESSAGE由来pendingを入力源一覧へ追加。
- **再レビュー P2-3 — 反映:** hide時の`pending`／`frame`／`operation_frame`一律pruneを撤回。受信時点のviewport所有とfocus喪失後も有効な離散キーを保持。

「未消費RawInputは保持する」という方針は維持する。ただし「本家の単一保留領域を変更しない」という第3版の実装方針は、第4版で訂正する。

## 第2版 — 2026-10-16

対象は初版`226985787`。

- **P1-1 — 反映:** TexturesDeltaのdrain・引継ぎ・意図的破棄を経路別に定義し、段3の必須移行へ追加。
- **P1-2 — 反映後に訂正:** 汎用の非表示受付・終端機構を設計したが、第3版で撤回。第4版では実在するviewport間の保留所有違反を限定修正する。
- **P2-3 — 反映:** 無描画配送のsubmissionと適用後freeを追加。
- **P2-4 — 反映:** unsmoothed wheelの取得元・換算・修飾・消費順を定義。
- **P2-5 — 反映:** 通常窓・native入力欄のCtrl+Shift+C/X/V保持を推奨。
- **P2-6 — 反映:** 実ウィンドウ試験を自動gateから分離する準備段を追加。
- **P2-7 — 反映:** accessibility先行、両integration targetとlib snapshotの更新を必須化。
- **P3-8 — 反映:** 通常版immediate viewport計測を独立したvendor保持項目へ追加。
- **P3-9 — 反映:** 0.33.3上の準備段を分割。固定票とPNGごとの重複記録を撤去。

# egui 0.37 更新の影響調査と段階設計

## 0. 調査範囲と結論

調査・改訂日: 2026-10-16  
対象: `G:\home\mimageviewer-egui036`、ブランチ`egui-036`  
製品ソースの基準: `05b6a15de26897369a35e5aa00377d174ca3760d`  
今回照合時HEAD: `4e097de68`

リポジトリ、指定された更新履歴要約、Cargo registryの0.33.3／0.37.0、Rust 1.99の既存checkログ、各版の設計書とレビューを読み取り専用で照合した。ファイル変更、依存解決、ビルド、テスト、ネットワーク利用、製品起動は行っていない。

実行時影響はコードからの見立てである。既存文書に記録された他担当の結果と、今回のソース確認を区別する。

目標は利用者決定によりegui系0.37.0。実装開始時に0.37.xのパッチ版へ合わせる場合は、その版で本書の判断を再照合する。

主な結論:

1. Appの責務分離、root Ui、入力イベント、texture delta、wgpu surfaceの移行が必要。
2. `vendor/egui-winit`は削除できない。Hidden／Forwarded配送保証に加え、Disabled取消終端の互換処理が必要。
3. `vendor/eframe`も削除できない。既存差分のほか、本家の保留RawInputをviewport別の所有へ直す。
4. `ui_fonts.rs`の外形・行計測は既にttf-parser。ab_glyphを直接使うフォントプレビューとは分ける。
5. 動画本体はD3D11 native presenter。wgpu更新の主対象はHUD overlay、比較、panorama、静止画縮小。
6. `raw_scroll_delta`は、消費位置で未消費のeventsから作るunsmoothed投影へ移す。
7. TexturesDeltaのdrain・引継ぎ・submission・freeとheadless明示破棄は、段3の必須契約とする。
8. SW_HIDEと最小化は同じ経路ではない。汎用の非表示入力フィルタは追加しない。
9. gamepadは物理状態更新と操作配送を分離する。離散pressの遅延採用を防ぎ、保持軸の復帰後継続は維持する。
10. native経路はIME cursor位置だけでなく、composition中断要求もOSへ届ける。
11. 連続クリックに距離条件が加わる。release射影は本家分類をそのまま返し、操作差は利用者相談とする。
12. snapshotは445枚。accessibility修正後、両integration targetとlibを更新する。
13. f32 fallback警告は既存ログで445件。旧版上の一括修正はせず、更新後に残りを整理する。

### 参照表記

本家の`egui-0.37.0/src/...:line`等は、次のregistry rootからの相対パス。

```text
C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\
```

mIVの行番号は今回照合した製品ソースに対するもの。件数は特記しない限り`src`、`crates`、`tests`、`tools`のRustソースの一致行数で、コメントを含む。ASTによる呼出数ではない。

## 1. 不変条件と簡素化方針

- 必要なwakeに応じて、非表示でもトレイ復帰、非同期完了、Remote、保存終端、メディア制御を進める。常時tickは追加しない。
- 未消費入力は、受信したviewportだけが消費する。rootの保留をchildへ渡さず、skipしたchildの入力を落とさない。
- 正当に受信済みの入力は、所有者と順序を保つ。未消費という理由だけで失効させない。
- viewportへ配送済みの離散キーは、後のfocus喪失だけで取消さない。現在のキーlevelを読む許可とは分ける。
- release、focus、IME終端、復帰後の新規入力、兄弟viewportの入力を保持する。
- IME取消はproducerから正規化・TextEditまで到達させる。nativeの中断要求も対象HWNDへ届ける。
- gamepadの物理drain・release・軸・disconnectを止めない。未配送edgeと意味終端を最新状態で上書きしない。
- gamepadの離散press抑止と、保持軸による復帰後継続を区別する。
- viewportごとの入力、IME、cache、texture、要求世代、workerの所有を維持する。
- popup所有wheelは、終了frameと平滑化残量を含めて背面へ漏らさない。
- texture deltaを所有者不在にしない。uploadはsubmitまで完結させ、使用中textureをsubmit前にfreeしない。
- release射影は本家の分類結果を返す。別の連続クリック判定を作らない。
- フォントfallback、選択設定、リリース済み機能・データを保持する。
- snapshot更新で操作・到達性・所有権の回帰を承認しない。

状態の組合せを減らす案の比較:

| 対象 | 採った案と理由 | 採らなかった案と理由 |
|---|---|---|
| 非表示処理 | serviceをlogicへ分離し、入力処理は既存ownerへ残す | 全入力共通の「通常／終端のみ」配送。必要性がなく状態を増やす |
| 未消費RawInput | 既存保留領域をviewport ownerへ移す。skipするchildはStateからtakeしない | 単一保留領域＋viewport guard、App側の第二queue。誤配送元が残る |
| 受信済みキー | event時点のviewport所有と既存permitを保持 | hide時の一律prune。focus喪失後のdiscrete採用契約を壊す |
| IME取消 | State変換から既存正規化へ終端を届ける | timeout、focus喪失時の即時一律取消。正当な後続確定を失う |
| native IME出力 | 既存intent・control配送・HWND ownerを利用 | rendererから直接Win32呼出、新pending flag、上書き可能snapshotだけへの格納 |
| gamepad | 一つの物理ownerと既存batch配送を接続 | 全入力源への受付層拡張、復帰時のforegroundだけによる選別 |
| クリック | 本家分類と射影保証を分離 | 旧期待値を通すための旧判定、別タイマー |
| texture delta | 描画・無描画を共通の配送・完結処理へ揃える | 借用適用後の無条件clear |
| wheel | 消費位置の未消費eventsから投影 | raw snapshot復元cache。消費済み入力が復活する |
| snapshot | 画面群の共通理由と例外を一台帳で管理 | PNGごとの重複票、固定数のレビュー票 |
| 長い変換・保存 | 既存モーダル境界を維持 | 更新専用の並行・回復経路 |
| 非表示中のUI | 本家のpass省略と既存wakeを使う | UIを回し続ける、窓を再生成して入力を捨てる |

新しいpending bool、時間窓、retry、追加repaint、silent fallbackは設けない。RawInput保留の修正は既存領域の所有移管であり、保留経路を二重化しない。native IMEのintentは一回の命令であり、Appの保持状態へ重ねない。

まれな失敗は割り切り、ログ＋通知［＋作り直し］を基本とする。更新専用の回復機構は作らない。

## 2. 前提・更新履歴要約との相違

| 前提・要約 | ソース確認 | 反映 |
|---|---|---|
| egui-winitは削除できる見込み | Hidden／Forwardedとの差に加え、本家はDisabledを捨てる | vendor維持 |
| eframeはhidden repaint修正だけ | scheduler、visible commit、delta、通常版計測、診断がある | 全差分を台帳化 |
| 本家の未消費RawInput保持をそのまま使える | EpiIntegrationに一つだけあり、deferred childも取り出す | viewport所有へ移管 |
| childはUIをskipしても入力が残る | take後に保存せず戻る経路がある | skip時はtakeしない |
| ui_fontsはab_glyph計測 | 外形・行計測はttf-parser。previewがab_glyph | 別々に移行 |
| `CentralPanel::show`廃止 | Contextを受ける形がなくなり、Ui版は残る | 親Uiを移行 |
| `AsId`削除 | 0.37.0では非推奨として残る | 削除済みと書かない |
| Windowはタイトルバーのみ移動 | 既定OnTouchではタッチ時に本文からも移動する | 入力機器別に確認 |
| clipboardは余分な修飾キーなし | X/C/Vは`command_only()` | Ctrl+Shift操作を保持 |
| texture deltaはmap化だけ | 非空drop assertionと明示破棄APIもある | 消費契約を必須移行 |
| 非表示はすべてlogic-only | SW_HIDE、最小化、期限超過で異なる | 条件表で区別 |
| native IME出力はcursor位置だけ | 0.37は中断要求も出す | intent配送を追加 |
| 連続クリックは時間条件だけ | クリック間距離とtripleの時間条件が変わる | 相談・fixture更新 |
| snapshot約350枚 | 追跡PNG445枚 | 工数を修正 |

根拠: `egui-winit-0.37.0/src/lib.rs:656`、`:785`、`:1277`、`:1538`、`eframe-0.37.0/src/native/epi_integration.rs:188`、`:337`、`:384`、`:418`、`src/native/wgpu_integration.rs:700`、`:716`、`egui-0.37.0/src/data/input/raw_input.rs:162`、`src/input_state/mod.rs:1385`。

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
| `fetch_update\(` | 製品・テスト14／12、別途vendor 1件 | `src/log_writer.rs:105` |

```powershell
rg -n '\bId::new\s*\(|ui\.id\(\)|\bAsId\b' src crates tests tools -g '*.rs'
rg -n 'SidePanel::|TopBottomPanel::|CentralPanel::|show_viewport_' src crates tests tools -g '*.rs'
rg -n 'RawInput|raw_scroll_delta|Event::MouseWheel|\.modifiers' src crates tests tools -g '*.rs'
rg -n 'textures_delta|drop_without_applying_deltas|\.run\(|run_ui|run_pass' src crates tests tools vendor -g '*.rs'
rg -n 'sample_gamepad_input|dispatch_gamepad_batch|suppress_pending_actions|take_pending_mouse_nav' src -g '*.rs'
rg -n 'ImeEvent::|WindowEvent::Ime|NativeWindowIntent|ime_cursor_area_intent' src crates vendor/egui-winit -g '*.rs'
rg -n 'double_clicked\(|triple_clicked\(|released_buttons_with_click_counts|click_count' src vendor/egui -g '*.rs'
```

`.run(`の1274一致行／130ファイルにはHarness・workerも含むため、Context移行件数には使わない。FullOutput破棄、IME入力・出力、クリックconsumerは呼出元と所有を追って棚卸しする。

## 4. 項目別設計

### 4.1 文字描画、補正、fallback、絵文字

#### 本家の変更

0.34以降の文字描画交換と0.35以降のshapingを経た0.37は、skrifa、vello_cpu、harfrustを使う。

| 内容 | 本家ソース |
|---|---|
| units-per-em基準の倍率 | `epaint-0.37.0/src/text/font_face.rs:88` |
| outline／color glyph選択 | 同`:96` |
| color glyph描画 | 同`:190` |
| shaping data | 同`:332` |
| メトリクス・offset | 同`:596` |
| hinting／subpixel binning既定有効 | `epaint-0.37.0/src/text/mod.rs:77` |
| 行内配置 | `epaint-0.37.0/src/text/text_layout.rs:1100` |

FontTweakのscale／offsetは残り、hinting target、variation coordinates等が加わる。根拠: `epaint-0.37.0/src/text/font_tweak.rs:6`。

#### 影響箇所と方針

| 箇所 | 内容・方針 |
|---|---|
| `src/ui_fonts.rs:4`、`:7`、`:13` | user／HUD／toolbar familyを維持 |
| 同`:235`、`:420` | fallback順と主フォント登録を保持 |
| 同`:360`、`:373` | 主フォントの視覚中央補正を再検証 |
| 同`:546`、`:567` | fallback補正を新rendererと比較 |
| 同`:652`、`:687`、`:699` | ttf-parserのbbox／行計測。全面交換しない |
| `src/ui_font_catalog.rs:6`、`:78` | ab_glyph previewを本体表示へ近づける |
| `src/video/native_presenter/bottom_hud.rs` | 実galleyの計測・描画共有と失効を確認 |
| `build.rs:700`、`src/comic_stamp.rs` | Twemoji SVGと生成物を保持 |

`miv-user-text`／`miv-hud-text`、選択フォント、face index、利用者の縦位置調整値を保持する。toolbar固定3.5pt、主フォント補正、CJK／数学／emoji補正は個別に判断する。

color glyphはoutline bboxと可視範囲が異なり得る。補正を全部残す／全部捨てる方針にはしない。同じ文字列・font・point size・DPIについて、行高、baseline、galley矩形、描画範囲を比較する。

previewは既存worker準備を維持する。本体と異なるラスタライザによる見本のずれを解消する方式は小設計としてレビューする。`comic-core`の注釈焼き込み用`ab_glyph_rasterizer`は別経路なので変更しない。

#### default_fontsとsystem fallback

egui単体のdefault featureは空だが、eframe defaultには`default_fonts`が残る。現root指定で全default fontが直ちに消えるわけではない。

根拠: `egui-0.37.0/Cargo.toml:62`、`eframe-0.37.0/Cargo.toml:61`、`epaint-0.37.0/src/text/font_definitions.rs:110`。

eframeのsystem font fallbackは既定有効。featureと`NativeOptions::system_font_fallback`を明示し、既存chainを基準にする。環境依存探索を無条件に重ねない。

根拠: `eframe-0.37.0/src/epi.rs:384`、`src/native/epi_integration.rs:260`。

`monochrome_emoji_fonts`は別feature。Twemojiは`vendor/twemoji`由来の注釈用SVG資産であり、文字fallbackや色付き文字対応で代替しない。

**見た目:** 字面、太さ、字幅、合字、折り返し、行高、baseline、emoji色が変わり得る。代表比較を用意し、§5と段4bで採択する。

### 4.2 App／Context、非表示処理と入力所有

#### 本家の呼出契約

App::logicはroot UIの前と、rootのUI省略時に呼ばれる。常時tickではない。logic-onlyではegui passが進まず、Context::inputはviewport情報の一部を除き最後の表示passの値を持つ。

根拠: `eframe-0.37.0/src/epi.rs:152`、`egui-0.37.0/src/context.rs:1174`。

schedulerのHWND可視判定と、UI省略の`ViewportInfo::visible()`を区別する。後者はminimized／occludedの合成であり、IsWindowVisibleそのものではない。

根拠: `eframe-0.37.0/src/native/run.rs:216`、`:243`、`src/native/wgpu_integration.rs:682`、`:702`、`egui-0.37.0/src/data/input/viewport_info.rs:98`。

#### SW_HIDE／最小化／子窓／期限超過の条件表

以下はソースからの見立てであり、製品起動による確認結果ではない。

| 条件 | scheduler／pass | logic・UI | 未消費入力 |
|---|---|---|---|
| 非最小化rootを直接SW_HIDE | 隠れた窓の要求済みrepaintをFullで直接実行 | 通常はroot logic＋UI | SW_HIDEだけを理由に保留が累積するとはいえない |
| root最小化、visible扱いの子なし、強制paintなし | Full要求でもshow_ui=false | root logic-only | root ownerに保持 |
| root最小化、visible扱いの子あり | Fullなら親UIも実行 | root logic＋UI | 各viewportが自身の入力を消費 |
| screenshot／RequestPaintWhileHiddenあり | Fullでwants_paintが成立 | UI実行可能 | 既存capture ownerを維持 |
| rootのRedrawRequested期限超過 | 可視性・子の可視性と別にLogicOnly | root logicのみ | root ownerに保持 |
| childのUIが最小化・期限超過等でskip | child callbackなし。App::logicも呼ばれない | 子UIなし | 通常入力は子のStateからtakeせず保持。Closeは下記lifecycle経路で処理 |
| 最小化childへCloseRequested | 本家ではCloseを積み、子と親へrepaintを要求するだけ | 子UIの省略は解除されない | 第4.2版ではUI復帰待ちにせず、既存終了ownerへ配送・消費 |
| repaint・wake等の要求なし | Wait等でsleep | 呼出なし | 常時UI／logicとは扱わない |

根拠: `eframe-0.37.0/src/native/run.rs:216`、`:243`、`:266`、`src/native/wgpu_integration.rs:682`、`:702`、`:717`、`:1031`、`:1189`、`egui-winit-0.37.0/src/lib.rs:1507`。

winit Windowsのis_visibleはIsWindowVisible、is_minimizedはIsIconicを読む。WindowsではOccludedは未対応で、WM_SIZEも生成しない。

根拠: `winit-0.30.13/src/platform_impl/windows/window.rs:147`、`:668`、`util.rs:143`、`event_loop.rs:1397`、`winit-0.30.13/src/event.rs:418`。

mIVのトレイ退避は`src/tray_integration.rs:276`、`:323`の直接SW_HIDEである。

Closeの配送・拒否・承認・退役は、以下の「非表示子のCloseとlifecycle要求」に従う。通常入力を保持する契約だけでは、終了要求の進行を保証できない。

#### root LogicOnlyとvisible childが共存する経路

**単純な「root最小化＋既にvisible扱いのchild＋Full要求」では、親UIも実行される。** この条件だけを保留入力誤配送の根拠にはしない。

一方、期限超過経路は次の構造になっている。

1. 可視rootへのrequest_redrawと同時に、root固有のdeadlineを登録する。
2. rootのRedrawRequestedが未到着でdeadlineを超えると、rootをLogicOnlyで実行する。
3. この選択ではvisible childの有無を検査しない。
4. wgpu側のshow_uiは`mode == Full`を必要とするため、visible childがいてもLogicOnlyをFullへ変更しない。
5. deferred childは独立したwindow eventから更新でき、同じEpiIntegration::updateを使う。

根拠: `eframe-0.37.0/src/native/run.rs:233`、`:266`、`:281`、`:421`、`src/native/wgpu_integration.rs:647`、`:702`、`:773`。

したがって、**rootの期限超過LogicOnly → deferred childのFull → rootのFullという呼出経路はソース上到達可能**であり、相互排他条件はない。Windowsでの発生頻度や、このPCでの再現は未確認である。

mIVは実際にdeferred childを使う。根拠: `src/ui_fullscreen.rs:22139`。同`:22244`付近の子が文字入力を持たなくても、rootの入力がそこで消費されることは許容できない。

また、childのskip時入力喪失は、rootとの共存条件に依存しない。このため、両方を同じviewport所有境界で直す。

#### 影響箇所

| 箇所 | 内容 |
|---|---|
| `src/app.rs:94680`、`:97631` | update・App trait |
| 同`:94969`、`:97661`以降、`:97700`以降 | tray／media、非同期完了、post-render終端 |
| `src/tray_integration.rs:276`、`:323` | hide／restore |
| `src/key_input.rs:2023`、`:2099`、`:3381`、`:3418`、`:3960`以降 | queue・drain・Win32終端 |
| `src/keyboard_input.rs:106`、`:159` | discrete／current level permit |
| `src/app.rs:55249`、`src/ui_fullscreen.rs:32137` | permit消費 |
| `src/gamepad.rs:170` | gilrs producer |
| `src/app.rs:95772` | gamepad sample |
| `src/app/gamepad_input.rs:2110`以降 | sample・配送・抑止・終端 |
| `src/lib.rs:786`、`:801`、`:836` | WH_GETMESSAGE pending |
| `src/ui_fullscreen.rs:22139`、`:22364`、`:24745` | deferred／immediate viewport |
| `src/video/native_presenter/render_core.rs:12505` | 独立Context |
| `vendor/eframe/src/native/epi_integration.rs` | 新版の保留処理を移植時に修正 |
| `vendor/eframe/src/native/wgpu_integration.rs` | viewport入力取得・skip・復帰 |
| `vendor/eframe/src/native/glow_integration.rs` | 共有integrationの同等経路 |
| 各ラボ・`vst3-host-tester` | App実装 |

#### Appの責務分離

| 処理 | 移行先 |
|---|---|
| worker結果、取消・保存終端、Remote完了、トレイ要求 | UI入力を読まないlogic service |
| 子のOS／programmatic Close要求 | 非描画のlifecycle受付から、対象windowの既存modal判断・終了ownerへ。一度処理した要求を子UIへ再配送しない |
| gamepad物理drain・保持更新 | logicから進めるApp-global単一owner |
| gamepad操作batch・multi-pass配送 | 既存batch ownerとmounted context |
| key／wheel／pointer／focus／IMEのpass処理 | ui／対象viewport |
| panel／dialog／viewport登録・paint | ui |
| 承認済みviewportの退役 | 既存寿命ownerの終了確定からbackend／Contextの対象限定cleanupへ。描画passを前提にしない |
| 表示済み前提のACK等 | 実際の描画側 |
| contextごとの結果採用 | 既存registry／mount owner |
| native動画frame | 既存presenter thread |

旧updateを丸ごとlogic／uiへ改名しない。logicは古いevents・キーlevel・時刻を操作として読まず、UI pass counterをservice進行条件にしない。

post-render終端を前へ移す際は、そのframeのmodal gateを早く外さない。

Close専用の非描画受付は、通常入力の消費や子UIの代行ではない。既存の終了判断・取消・cleanupを呼ぶ入口を揃えるものであり、App側に別のclose待ち状態を追加しない。

#### 非表示子のCloseとlifecycle要求

本家0.37のwgpu経路では、最小化等でshow_ui=falseの子についてcallbackを呼ばずに戻る。一方、CloseRequestedはviewport.info.eventsへCloseを積み、子と親にrepaintを要求するだけである。Closeはwants_paintではなく、UI省略を解除しない。

根拠: `eframe-0.37.0/src/native/wgpu_integration.rs:682`、`:702`、`:716`、`:720`、`:1031`。

WindowsのWM_CLOSEもCloseRequestedへ変換して戻り、その場ではwindowを破棄しない。根拠: `winit-0.30.13/src/platform_impl/windows/event_loop.rs:1251`。

mIVのpassive deferred子は、子callback内でCloseを読み、Frameイベントとして親へ送り、親がclose_idsへ採用する。

- `src/ui_fullscreen.rs:22184`: 子callbackでCloseを読む。
- 同`:22246`: 親向けFrameイベントを生成。
- 同`:21531`: close_idsへ採用。
- 同`:21756`: modal判断とbatch適用。
- 同`:21876`: 既存のwindow終了処理。
- 同`:22035`付近: deferredイベントのdrainが描画関数内にある。

したがって、最小化した別窓をタスクバーから閉じる通常操作で、復元までCloseが処理されない経路が残ると見立てる。実機再現は今回行っていない。まれな失敗の割切り対象にはしない。

glowにも同じUI省略とClose蓄積がある。根拠: `eframe-0.37.0/src/native/glow_integration.rs:689`、`:692`、`:708`、`:1061`、`:1075`。修正はwgpuだけに限定しない。

**採る案は、Close専用の非描画受付から既存終了ownerへ渡す方式とする。**

親logicで更新済みRawInput.viewportsを読む案も検討した。run_logicが最新のwindow情報を渡すこと自体は確認できるが、snapshotを読むだけでは、元のviewport.info.eventsの消費、子Fullとの競合、拒否後の再配送を一つの所有境界で解決できない。また、親がlogic-onlyのままでは通常の描画後GCにも到達しない。このため、snapshot監視を新たなclose検出器として重ねる案は採らない。

根拠: `egui-0.37.0/src/context.rs:1174`以降、`eframe-0.37.0/src/native/wgpu_integration.rs:808`、`:1330`、`src/native/glow_integration.rs:853`、`:1533`。

##### 受付・判断・消費

1. vendor/eframeに、子Close専用の非描画App hookを追加する。仮称は`App::on_viewport_close_requested`とし、対象ViewportIdを明示して渡す。子の描画callbackではない。
2. hookはbackendのViewports／glutinのborrow・lockを解放した状態で呼ぶ。描画passの中から再入して呼ばず、window eventまたはviewport commandの処理を終えた非描画境界で配送する。
3. OSのCloseRequestedと、既存ViewportCommand::Closeから生じる要求を同じ受付へ揃える。rootの終了・tray契約は従来どおりとし、この子専用変更で置き換えない。
4. mIVは既存のViewportId→window／context所有から対象を決める。rect、現在foreground、現在表示中のwindowから推定しない。
5. passive deferred子は既存のmodal判断と`apply_detached_image_window_event_batch`／`close_detached_image_windows_by_ids`へ合流させる。必要な部分を描画に依存しない終了serviceとして抽出し、同じ判断を複製しない。
6. active／parkedのegui子についても、既存のwindow／context所有と終了・取消経路へ委譲する。active fullscreen、列挙待ち、parked mediaの終了処理をpassive画像のcleanupで代用しない。
7. hookの返値は、その一回の要求の処理結果とする。mIVが管理する子では「既存ownerが終了を確定した」「既存modal等により今回は拒否した」を区別する。新しいpending boolや復帰待ち状態は作らない。
8. mIVが所有しない子には明示的な従来UI経路への委譲を用意し、既定Appの互換性を保つ。これはmIVの管理対象を黙って見逃すfallbackには使わない。
9. mIVが処理したCloseは、承認・拒否のどちらでもその一回で消費する。元のinfo.events、保留RawInputのviewport情報、子callbackへ同じ要求を残して再発火させない。通常のkey／pointer／focus／IME列は変更しない。
10. 拒否後に新しく届いたCloseは別の要求として通常どおり判断する。拒否済みを再試行したり、以後のCloseを抑止するラッチを置いたりしない。

既存のViewportCommand::CloseはCloseイベントを追加する処理であり、承認済みの即時破棄命令ではない。CancelCloseもegui-winit内では別途処理が必要とされている。これらを送るだけで配送・消費・退役が済んだとは扱わない。

根拠: `egui-winit-0.37.0/src/lib.rs:1955`、`:1958`。

passive FrameのOS Close読取りは新受付へ集約し、同じ要求をFrame経由でも処理する二重ownerを残さない。独自barの×、watcher由来Closeも、既存の要求源を保持したまま同じApp側のmodal判断・終了serviceへ揃える。watcherは画面上のclick検出であり、タスクバーから届く全WM_CLOSEの代替ではない。

根拠: `src/app.rs:3247`、`:3524`、`src/app/detached_window_manager.rs:169`、`:265`。

##### 承認後の対象限定退役

Appが登録を外しても、親の次のFull passだけを待ってbackendを掃除する設計にはしない。rootもlogic-onlyの場合を含め、承認した子を復元せずに終了できることを契約とする。

- App側は既存終了ownerで対象のClosing／登録解除・worker／media終端を行う。
- backend側は承認結果から、対象window、WindowId対応、egui-winit State、surface、保留入力を通常終了と同じ所有境界で退役させる。
- eguiのviewport登録・親子登録も同じ終了に合わせて取り除き、後の兄弟FullOutputから閉じた子が再作成されないようにする。
- 現在は通常Full後に行われるviewport GCを、対象を明示した共通退役処理へ抽出する。Context側にはそのためのeframe向け非描画退役入口を新設し、既存GCと処理範囲を共有する。UI passを実行してGCを起こす方法は採らない。
- 退役入口はpass外で、Appの終了ownerが確定した対象にだけ使う。rootや存続する兄弟の入力・memory・cache・surfaceを一括resetしない。
- 対象viewportに未配送texture deltaがあれば、§4.9の共有renderer ownerへ引き継いでから退役する。window終了を理由に共有deltaを捨てない。
- 既存終了処理が持つ保存・取消の意味を維持する。まれな失敗用のretryや新しい回復状態は追加しない。

本家のbackend GCは`eframe-0.37.0/src/native/wgpu_integration.rs:1330`、`src/native/glow_integration.rs:1533`。Context登録の通常GCは`egui-0.37.0/src/context.rs:3317`以降。現本家にこの非描画退役入口があるとは扱わず、今回の限定追加としてvendor台帳と段2へ計上する。

##### Close以外の要求・通知の棚卸し

| 対象 | 確認した経路・扱い |
|---|---|
| ViewportEvent | 0.37.0の離散variantはCloseだけ。別の同種lifecycle要求は見当たらない。根拠: `egui-0.37.0/src/data/input/viewport_info.rs:6` |
| programmatic Close | ViewportCommand::CloseもClose生成源。上記と同じ受付へ揃える |
| focus・minimize・maximize・配置・DPI・occlusion | window状態／入力情報。終了要求ではない。通常入力・metadataの所有契約を維持し、Close queueへ変換しない |
| focus喪失等による右drag取消 | passive callback内の`capture_right_drag_event`から既存Cancelを生成する経路がある。`src/app.rs:2040`、`src/ui_fullscreen.rs:21622`付近。callback依存の状態終端として回帰監査対象に含めるが、全focus／releaseを汎用終端配送へ移す理由にはしない。Close時は対象の既存gesture ownerも終了することを検査する |
| FirstCallback・初期配置適用・表示ACK | 実際のcallback／表示を知らせる通知。`src/app.rs:1901`、`:2010`付近、`src/ui_fullscreen.rs:21495`。Closeのために偽造しない。初回callback前のCloseも終了できることを検査する |
| 独自barの×・watcher Close | 既存App要求源。最小化中のOS Closeの代替ではない。同じ終了判断へ合流 |
| ActivationTokenDone | backendがFrameへ応答を保持する経路。子callbackだけの読取りではない。wgpu `:987`付近、glow `:1024` |
| native presenterのClose／Destroyed | 既存native pump・WindowEpoch経路。`src/video/native_window_pump.rs:1465`以降。egui子Closeを重ねて配送しない |

今回確認した範囲では、Close以外に追加の離散ViewportEvent要求はない。focus由来のgesture取消等は性質を分けて記録し、既存順序・所有を保つ回帰を行う。具体的な失敗が示されていない状態まで、新しい汎用lifecycle機構へ広げない。

##### 必須回帰

- child最小化→Close→子を復元せず終了。子callbackの呼出回数が増えていないこと。
- rootもlogic-onlyのままで、承認した子のwindow・backend／Context登録が退役すること。
- modal拒否→modal終了→子復元で、同じCloseが再発火しないこと。
- 拒否後の新しいCloseは受理できること。
- 子Full／親logicとの順序が変わっても、同じ要求を二重処理しないこと。
- 初回子callback前のCloseを処理でき、表示ACKを偽造しないこと。
- 兄弟の入力、IME、保留RawInput、texture、surface、寿命を維持すること。
- 承認後の兄弟Full／root Fullで閉じた子を再作成しないこと。
- OS Close、programmatic Close、bar／watcherの既存App入口で同じmodal判断・終了ownerを使うこと。

追加repaintやRequestPaintWhileHiddenで閉じる動作を成立させる試験にはしない。既存終了処理が通常の状態反映として行うrepaintは保持できるが、その到着やFull passを終了完了の必要条件にしない。

この受付・消費・非描画退役はdetachedの構造修正として、着手前の双方合意と§11記録を必要とする。

#### 保留RawInputの最小修正

本家EpiIntegrationはアプリに一つであり、`pending_raw_input`も一つである。updateはdeferred callback付きでもprepare_raw_inputを呼び、update_logic_onlyも同じ領域を使う。appendはイベントを連結しつつviewport_idを新しい側へ置き換える。

根拠: `eframe-0.37.0/src/native/epi_integration.rs:175`、`:188`、`:337`、`:384`、`:418`、`egui-0.37.0/src/data/input/raw_input.rs:162`、`:168`。

このため、共有保留領域をそのまま残す方針は訂正する。

**推奨実装は、既存保留領域をbackendのViewport ownerへ移す方式とする。**

1. EpiIntegrationのアプリ共通`pending_raw_input`を除く。
2. backendの各Viewportが、自身の未消費RawInputを所有する。既存保留領域の移管であり、App側や別registryへ第二queueを作らない。
3. 入力取得側が、同じViewportの保留分と新規State入力だけを古い順に連結する。最新metadataを使うappendの意味は維持する。
4. Full処理はその入力を一度だけeguiへ渡す。別viewportの保留へアクセスしない。
5. root LogicOnlyはlogic実行後の未消費RawInputを呼出元へ返し、同じroot Viewportへ戻す。
6. childでUIを実行しないときは、show_ui判定をStateのtakeより前へ置き、入力をStateに残す。childのための別保留queueは追加しない。
7. immediate経路も同じViewportの入力取得入口へ揃える。deferred／immediateの更新経路が変わっても、別viewportへ流れないようにする。
8. viewportの明示終了時だけ、そのownerの未消費入力を終端する。hide／minimizeでは消去しない。兄弟やrootの保留へ触れない。

修正対象となる本家境界は`eframe-0.37.0/src/native/wgpu_integration.rs:700`、`:716`、`:773`、`:1251`付近、および`src/native/glow_integration.rs:689`、`:708`。glowを製品rendererへ変更する意図はないが、共有EpiIntegrationの署名とskip契約を同じ形へ合わせる。

所有者の取り違えをguardで黙って捨てる方式にはしない。入力を組み立てる段階から同じViewportの領域しか選べない形にし、テストでviewport_idとevent列を検査する。

ユーザーコード実行中にViewportsのborrow／lockを保持しない。logicからUIを呼んで保留を消費する対策も採らない。

#### raw_input_hookの契約

hookは、**同じviewportの保留分と新規分を連結した後**に従来どおり呼ぶ。root LogicOnly／Fullとdeferred Fullの既存呼出位置を維持し、skipしたchildを保存するためだけの追加hook呼出はしない。

hookを汎用復帰フィルタにしない。古い保留と復帰後の新規入力を一括削除せず、host固有正規化にもContextの古いeventsを使わない。

#### 保留入力の必須回帰

段2cで、窓を作らない実integration／入力取得helperの回帰を置く。

| 入力列 | 期待結果 |
|---|---|
| root LogicOnly(A) → child Full(B) → root Full(C) | childはBのみ。rootはA＋Cを順序どおり一度だけ消費 |
| root LogicOnlyを複数回 → child Fullを挟む | rootの各入力がrootへ残り、childへ混入しない |
| child skip → 他viewport Full → child復帰 | childのrelease、WindowFocused、ModifiersChanged、IME終端が保持される |
| child skipを複数回 | 入力の欠落・重複・逆順がない |
| child復帰時に新規入力も到着 | 終端と新規入力の順序を保持し、新規分を捨てない |
| child終了、兄弟は存続 | 終了対象の所有だけを終端し、兄弟の保留を変えない |
| deferred／immediate入力取得 | 同じViewport所有の入口を使用する |
| hook観測 | 各呼出で正しいviewport_idと同じviewportのevent列だけを受ける |

focus／IMEの回帰は、イベント列の存在だけでなく、復帰pass後のkeys_down・composition・実TextEditの状態も検査する。

これはdetachedに関わる構造変更である。段2c着手前に設計担当と独立レビュー担当が合意し、`docs/detached-rework-plan.md` §11へ範囲・所有不変条件・判断理由を記録する。

#### 入力源ごとの最小契約

| 入力源 | 方針 |
|---|---|
| 通常OSキー／pointer | 非表示直前のbatch、遅着release、capture終端を所有者へ保持 |
| focus／Modifiers／IME | viewportごとの順序・正規化を保持 |
| Win32 edge queue | 既存enqueue／focus／destroy終端とdrain条件を保持 |
| gamepad | 物理更新と操作受付を局所分離 |
| WH_GETMESSAGE | 既存main／fullscreen配送先・drain条件を保持 |
| 合成入力、Remote、トレイ、IPC | 明示制御経路を通常hardware入力と一括して捨てない |

#### Win32 edgeと受信時点の所有権

RoutedKeyEdgeは対象viewportへ既に届いた離散キーの証拠である。後から同じframe内でfocusを失ってもdiscrete入力として採用する。OS直読み・frame最終levelにはfocused permitが必要である。

根拠: `src/keyboard_input.rs:106`、`:159`、`src/app.rs:55249`、`src/ui_fullscreen.rs:32137`。

保持事項:

- hide時に`pending`／`frame`／`operation_frame`を一律pruneしない。
- 受信済みキーを現在のGetAsyncKeyStateでfresh／stale判定しない。
- ReturnKeyStateのenqueue更新、WM_KILLFOCUS・destroy終端を共用する。
- 兄弟viewportの記録をroot hideで変更しない。
- `routed_edge_keeps_only_discrete_input_after_focus_loss`の期待値を維持する。

根拠: `src/key_input.rs:2089`、`:3971`、`src/keyboard_input.rs:809`。

focus喪失の合成releaseは`winit-0.30.13/src/platform_impl/windows/keyboard.rs:100`、egui-winitのfocus処理は`egui-winit-0.37.0/src/lib.rs:453`、`:483`。

PointerGoneは押下解除ではなく、WM_CAPTURECHANGEDもwinitではreleaseを生成しない。回帰対象とするが、この事実だけで汎用終端APIを追加しない。

根拠: `egui-0.37.0/src/input_state/mod.rs:1219`、`winit-0.30.13/src/platform_impl/windows/event_loop.rs:1927`。

#### ゲームパッドの移行

gilrs threadはforegroundに関係なくqueueへ追加しrepaintを要求する。現sampleはpressから操作を作り、配送時にforeground／modalを判定する。

根拠: `src/gamepad.rs:170`、`src/app/gamepad_input.rs:2158`、`:2373`、`:2510`。

UI側だけでdrainすると最小化中のpressを復帰後の候補にし得る。sample全体のlogic移動も、frame番号・pass indexによる再配送制御があるため採らない。根拠: 同`:2110`。

移行契約:

1. 物理drain・押下／軸／disconnect更新を、UI passが進まなくても実行可能な単一ownerへ分ける。
2. 操作batchとmulti-pass配送は既存経路に残し、logicとuiの二重drainを作らない。
3. 操作先不在でもrelease・disconnect・中立更新を保持する。非前面を設定OFFと同一視してthreadを停止しない。
4. logic未実行の短い退避→復帰も、gilrs取込み側で既存OS前面判定を共用して扱う。復帰時のforegroundだけで選別しない。
5. 受付事実はPadイベント／同じ保持ownerに局所化する。全入力共通層、新App pending bool、時間窓を作らない。
6. 正式な操作先・modal判断はmounted contextの既存配送側に残す。producerからApp／egui lockを取得しない。
7. 既存continuous timer抑止と`suppress_pending_actions`を再利用し、受付不可期間の離散press／repeatを復帰後に発火させない。
8. **軸は物理値を更新し続け、復帰後は保持中の軸値で操作を続ける。中立通過を新たに要求しない。** 別アプリで倒したまま復帰したときの継続はリリース済み挙動であり、離散press抑止と分ける。
9. 短いpopup後の軸継続等、既存modal抑止の挙動を一括変更しない。
10. ringの意味終端・Undo構築は既存context所有で行う。root投影で確定したり、disconnectの終端を捨てたりしない。
11. **logicで受けた未配送edgeと意味終端を、配送側が取り出すまで同じ物理ownerで順序保持する。** press→releaseが複数logicに分かれても、最終保持状態や最新batchで置き換えない。UI入口で一度取り出し、multi-pass用batchを固定する。

現SampledPadTransactionはframe番号付き単一batchであり、この未配送保持をそのまま担わせない。操作追跡OFFでdiscard後のpassから新しいreleaseをpollする既存契約も維持する。

根拠: `src/gamepad.rs:375`、`:380`、`:497`、`:653`、`src/app/gamepad_input.rs:73`、`:509`、`:2115`、`:2260`、`:2342`、`:2398`、`:2732`、`:2764`、`:2809`、`:3657`、`:9809`。

段1bでは旧順序の抽出に留め、logic接続と受付事実利用は段3で行う。引継ぎowner整理後に規模を再評価する。

#### WH_GETMESSAGE pending

`src/lib.rs:786`のWM_APPCOMMAND、`:801`のBrowserキーはpending countへ入る。`:836`のhookは現在のUI thread限定であり、他アプリの全キーを拾うglobal hookではない。

消費側は`src/app.rs:55234`、`:59997`、`src/ui_fullscreen.rs:32101`。既存配送先・重複防止drainを保持する。

#### 追加しない機構

- 全入力共通の「通常配送／終端のみ配送」
- eguiへの「clickを作らないrelease」API
- hide境界のRawInputフィルタ
- Win32 queueの一律prune
- 復帰直後の一回捨て、時間窓、常時追加repaint
- App側の第二RawInput保留queue

**リリース済み挙動:** 正当な離散キー・release確定・gamepad軸継続を保持する。保留所有修正は入力の誤配送・欠落を防ぐための構造修正であり、入力を取消す仕様変更にはしない。

#### API移行

- 描画はrun_uiと受け取ったroot Ui。
- headlessはrun_pass、必要箇所だけroot_ui。
- viewport callbackは`&mut Ui`、Contextはui.ctx()。
- Contextのstyle系はglobal_style系へ。Ui局所styleは一括置換しない。
- UiのDerefで解決するforwarding methodを無意味に書き換えない。
- on_exit署名をglow featureと揃え、終了処理本体を保持する。

根拠: `egui-0.37.0/src/context.rs:950`、`:983`、`:1024`、`:2599`、`:4662`、`src/ui.rs:77`、`eframe-0.37.0/src/epi.rs:227`、`src/app.rs:97792`。

### 4.3 Panel統一と計算高さ

影響箇所:

- `src/ui_main.rs:7400`、`:10291`、`:12590`、`:15558`: menu／toolbar／検索
- 同`:20434`: 下部情報
- `src/ui_folder_pane.rs:218`: side panel
- `src/ui_main.rs:17580`、`:19160`: central
- `src/ui_text.rs:3002`、`:3331`: Area固定パネル
- `tests/support/scroll_layout.rs:6`、`:31`、`:65`: geometry

Panel::showへ親Uiを渡し、default_size／min_size等へ移す。CentralPanel::showもUi版へ接続する。

根拠: `egui-0.37.0/src/containers/panel.rs:252`、`:372`、`:425`、`:1377`。

順序・残り領域を一つの親Uiへ揃える。slide、drag-to-close、端double-click collapseは既存操作と比較して明示設定する。根拠: 同`:337`、`:454`、`:741`。separatorは同`:348`。

Area＋Frame＋ScrollAreaの固定HUDを全面交換しない。新版も親available sizeを上限にするため、separator修正だけで独自計算高さは直らない。根拠: `egui-0.37.0/src/containers/scroll_area.rs:842`。

| backlog §1.380 | 扱い |
|---|---|
| clipboard失敗カード | `src/ui_dialogs/clipboard_capture.rs:378`、`:490`。可変高と固定仮想行の問題は残る |
| 長い保存基点名 | 固定本文予算の残件として扱う |
| 注釈右詳細・左一覧 | `src/ui_text.rs:2745`、`:2761`、`:3252`、`:3348`。sink・最低高・本文高を同じpassで検査 |
| 切り取り・表示トリム | 内外枠・固定操作の実矩形を確認 |
| 構成editor行 | §1.242の既存作業へ残す |

Fits／Scrolls、1物理pixel許容、wheelによる末尾到達を維持する。

**見た目・操作:** separator、余白、collapse、panel操作が変わり得る。既存操作保持を初期案とし、変更採用は§5へ送る。

### 4.4 フォーカス、wheel、drag、クリック、Ctrl+Q

#### フォーカス・drag

影響: `crates/egui-ime/src/lib.rs:242`、`:305`、`:329`、`src/tab_focus.rs:23`、`src/app.rs:22538`、`:22543`、`src/ui_music_panels.rs:706`、native HUD seek／音量。

Area／Window／ScrollAreaの非focus化と入力部品のfocus回復を区別する。Ctrl+矢印改善だけでhelperを撤去しない。全域Tab traversal無効の方針を保持する。本家Tab処理の根拠: `egui-0.37.0/src/memory/mod.rs:590`。

dragは重なり、widget外移動・release、disabled復帰、新規pressを検査する。背景Senseを増やしてclickを奪わない。

ScrollAreaはDragScroll::OnTouchが既定。旧mouse drag scrollを保持する箇所はAlwaysを明示する方針。根拠: `egui-0.37.0/src/containers/scroll_area.rs:224`、`:286`。

#### 連続クリック分類とrelease射影

0.33.3は時間間隔で連続クリックを分類する。0.37は直前clickとの位置距離も使い、既定max_click_distは6 pointsである。比較は距離が6 points未満かどうかであり、6 physical pixelsという意味ではない。

tripleも、旧版の「今回から2回前までがdelayの2倍未満」から、double成立に加えて直近2区間それぞれの時間条件を満たす形へ変わる。

根拠: `egui-0.33.3/src/input_state/mod.rs:1213`、`egui-0.37.0/src/input_state/mod.rs:114`、`:1385`。

既存のpress→release間の移動許容と、今回追加されたclick間距離は区別する。広い同じwidget上でも、click位置が離れれば連続clickと分類されなくなり得る。

mIVの影響箇所:

- `src/ui_fullscreen.rs:2506`: adjudicated release列
- 同`:37876`、`:37976`、`:45128`: double click操作
- `src/ui_main.rs:18440`: resize reset
- 同`:28189`付近: 連続clickの分類を利用するreader処理
- `src/video/native_presenter/overlay_draw.rs:7495`、`:7568`: 音量／速度reset
- `src/test_script.rs:5841`: Response判定のfixture
- `vendor/egui/src/input_state/mod.rs:1798`、`:1824`: 旧分類期待値

release射影APIが保証するのは、**本家が当該passで分類した全button releaseを順に返し、非clickもNoneで残すこと**である。旧版と同じcountを作ることではない。

移植時は次を分ける。

- 射影回帰: 全button・非click・focus区間・順序・discard後の非再生を維持。
- 分類回帰: 本家0.37の近接click、離れたclick、6 points境界、時間条件、tripleを検査。
- 製品回帰: double click resetやreader操作が採択した分類で働くことを検査。

旧fixtureの`(20,20)→(30,30)→(40,40)→(50,50)`に対する`1,2,3,3`は固定保証にしない。この位置列では新版分類に合わせた期待値に変更し、別の近接位置fixtureで`1,2,3,3`を含む射影も検査する。順序・非click等の検査を減らさない。

**方針:** 本家0.37分類の採用を推奨するが、リリース済み操作差なので§5で利用者に相談する。旧期待値を通すための旧判定、別タイマー、距離条件の無効化を追加しない。

#### raw_scroll_deltaの代替契約

影響:

- `src/app.rs:58738`、`:58760`: 一覧・列数
- `src/ui_main.rs:1462`: menu消費
- `src/ui_helpers.rs:414`、`:475`: 筆半径
- `src/ui_fullscreen.rs:25754`、`:27352`: 前面owner・残量終端
- runtime_ops、dialog、ラボ、テスト

旧値は単なる生delta合計ではなく、単位換算・修飾軸変換を含む。根拠: `egui-0.33.3/src/input_state/mod.rs:440`。

取得元は**消費位置でInputState.eventsに残るMouseWheel**。raw.events、smooth値、OS現在modifiersは使わない。

| 処理 | 契約 |
|---|---|
| Point | logical pointとして使用 |
| Line | line_scroll_speed × delta |
| Page | 当該passのviewport高さ × delta |
| 横修飾だけ一致 | `(x+y, 0)` |
| 縦修飾だけ一致 | `(0, x+y)` |
| 両方／どちらも不一致 | 軸保持 |
| modifiers | 各イベントの値 |
| phase／source | 保持。境界イベントを余分な移動にしない |

0.37 WheelStateのgesture中modifier latchとmIV離散操作を分ける。根拠: `egui-0.37.0/src/input_state/wheel_state.rs:104`。

混在modifiersは受理先と条件を保った連続区間で処理する。同じ条件では従来集約単位を維持し、notch数だけ一覧移動を増やさない。筆半径等の独自notch換算は保持する。

消費順:

1. 既存viewport ownerが入力を受ける。
2. fullscreen前面ownerをキー・navigatorより前に確定。
3. 必要なsnapshotはowner内に限定。
4. menu／popupがeventsと対応smooth値を消費。
5. App一覧処理はmenu等の後、grid直前で残ったeventsから投影。
6. 受理consumerがeventsとsmooth値を消費。
7. popup終了時だけdiscard_pending_scroll新版相当で内部残量を終端。

consume後にraw snapshotから復活させない。診断snapshotと配送投影を混同しない。

#### Ctrl+Q

本家はend-passでquit_shortcutsからCloseを送る。根拠: `egui-0.37.0/src/memory/mod.rs:337`、`src/context.rs:2935`。

各製品Contextで本家既定を明示無効化し、既存keymap／終了・tray ownerへ揃える。新しい終了操作として採る場合は相談する。

**挙動:** wheel所有・Tab・keymapを保持する。drag、連続クリック、終了操作の差をsnapshot更新だけで受け入れない。

### 4.5 非推奨API削除とIME

本家はIME owner、下線／cursor、候補窓位置を改善する。

根拠: `egui-0.37.0/src/memory/mod.rs:1064`、`src/widgets/text_edit/builder.rs:617`、`:1049`、`:1382`、`egui-winit-0.37.0/src/lib.rs:1291`。

これらはmIVのshortcut gate、取消補完、focus contract、Tab方針、Process filterを一括代替しない。

| 対象 | 方針 |
|---|---|
| 300ms grace／timestamp／旧pending-event API | 撤去済み。復活させない |
| 確定Enter除去／IME専用Tab lock | 撤去済みのまま |
| 全域Tab、dialog Enter／Escape | 保持 |
| viewport別composition owner | 保持 |
| Disabled取消終端 | producerから正規化へ必ず届ける |
| 空Preedit補完 | 実TextEdit取消回帰を維持 |
| focus recovery／次pass claim | 本家との重複を特定してから縮小 |
| Win32 0xE5 press／repeat除外 | 保持 |
| Hidden／Forwarded | 保持 |
| IME中断要求の生成 | Memory共通フラグを既存のviewport別Focusへ移管 |
| 同一viewportのIME出力集約 | 最新の矩形等を採用し、未配送の中断要求は保持 |
| nativeのcomposition中断出力 | 対象HWND ownerへ配送 |

影響: `crates/egui-ime/src/lib.rs:15`、`:78`、`:99`、`:116`、`:242`、`:305`、`:329`、`src/ime_focus.rs:1142`、`src/tab_focus.rs:23`、`src/app.rs:22538`、`:22543`、`src/key_input.rs:3960`以降。

中断要求の生成・集約に関する追加の影響箇所は、`crates/egui-ime/src/lib.rs:360`、`:378`のfocus要求、`src/ui_fullscreen.rs:22364`、`:24745`等のimmediate viewport、`src/video/native_presenter/render_core.rs:13702`、`:14007`の出力consumer。各呼出側へ補助フラグを散布せず、egui内の生成・集約境界を修正する。

#### Disabled取消終端の入力契約

本家egui-winit 0.37はIme::EnabledとDisabledをイベント化せず捨てる。根拠: `egui-winit-0.37.0/src/lib.rs:785`。

一方、WindowsのWM_IME_ENDCOMPOSITIONには、結果文字列が得られず空Preedit／Commitを出さずにDisabledだけを出す経路がある。

根拠: `winit-0.30.13/src/platform_impl/windows/event_loop.rs:1587`、`:1591`、`:1607`。

現vendorはDisabledを配送し、共有正規化が必要な空Preeditを補う。根拠: `vendor/egui-winit/src/lib.rs:393`、`:414`、`:605`、`crates/egui-ime/src/lib.rs:116`。

したがって、helper移植だけでは足りない。

**互換方針:** Stateの実IME変換入口でDisabledを下流へ配送し、既存正規化が取消に必要な空Preeditを挿入する。

0.37にもegui::ImeEvent::Disabledは非推奨として残るため、初期移行ではこの通知を限定的な互換境界として使う。必要な非推奨許容はこの変換箇所と対応matchに理由付きで限定する。crate全体の警告抑制にはしない。

根拠: `egui-0.37.0/src/data/input/ime_event.rs:6`。

Enabledを含む既存通知の扱いも共有policyの同等性fixtureへ含め、警告削減だけを理由に落とさない。新版Preeditのactive_range_chars変換は維持する。

本家TextEditはEnabled／Disabled自体を本文取消として処理しない。取消用の空Preeditが実TextEditまで届くことを検証する。

根拠: `egui-0.37.0/src/widgets/text_edit/builder.rs:1417`。

最小回帰は合成eguiイベントから始めず、**winit::Ime → 実State変換 → egui-ime正規化 → 実TextEdit**を通す。段2aの冒頭で新版準備graphに移行中の共有正規化を接続し、手書きの模倣normalizerは使わない。実windowは作らない。

必須列:

- 非空Preedit → Disabledのみで取消。
- 同一pass／別passの取消。
- focus喪失 → DisabledでUnfocusedCompositionを終端。
- 非空Commit → Disabledで確定文字を保持。
- compositionなしのDisabledで通常選択文字を削除しない。
- 重複Disabled／空Commitで取消を重複適用しない。
- 兄弟State／viewportへ取消が漏れない。
- 直後の通常Enter／Escapeが従来どおり働く。

#### Hidden／Forwardedを残す理由

本家HashSetは配送済みpressの履歴と同等ではない。

根拠: `egui-winit-0.37.0/src/lib.rs:483`、`:656`、`:1258`。

現vendorの保証を移植する。

- 配送済みpressを後続Process repeatでHiddenへ戻さない。
- releaseはallow_ime／logicalに依存せず記録をremoveし、Hiddenだけ遮断。
- focus喪失は当該Stateだけ終端。
- 通常press→Process repeatと逆方向の再切替でもreleaseを保持。

Disabled配送を追加しても、このkey hold履歴を消す契機にしない。IME終端と物理キーreleaseは別契約である。

traceは既定無効、文字秘匿、worker I/Oを保持する。vendor削除は、配送保証・取消終端が本家と同等になり、traceも不要と判断できた時点とする。

#### IME中断要求のviewport別所有

本家のrequested_interrupt_imeはMemory全体に一つのboolであり、各viewportのbegin_passでfalseへ戻る。end_passはその共有値を当該viewportのIMEOutputへ反映する。

根拠: `egui-0.37.0/src/memory/mod.rs:124`、`:799`、`:1079`、`egui-0.37.0/src/context.rs:3173`。

immediate子窓は同じContextで入れ子のrun_uiを実行する。子から戻る際にviewport IDは復元されるが、中断要求の共有boolは復元されない。

根拠: `eframe-0.37.0/src/native/wgpu_integration.rs:1264`、`egui-0.37.0/src/context.rs:3281`、`:3335`、`egui-0.37.0/src/memory/mod.rs:826`。

このため、親の要求が子のbegin_passで消える場合と、子の要求が親のIME出力へ混入する場合がある。mIVにもimmediate経路がある。製品上の具体的な発生操作は未検証であり、今回の判断はソースからの見立てである。

**最小修正は、既存フラグを既存のviewport別Focusへ移す方式とする。**

1. Memory全体のrequested_interrupt_imeを除く。
2. 既存のviewport別Focus entryに同じpass内の要求を所有させる。新しい独立mapやApp側の補助フラグは作らない。
3. interrupt_ime、request_focus、次frameへのfocus要求は、現在viewportのentryだけを更新する。
4. begin_passは開始対象viewportの要求だけをfalseへ戻す。親や兄弟のentryをresetしない。
5. end_passは終了対象viewportの要求だけをIMEOutputへ反映する。
6. 子から親への既存viewport復帰により、親のFocus entryが再び選ばれる。親子間の手動退避・復元処理を重ねない。
7. 非永続のpass状態として扱い、保存設定やmemoryの新しい永続形式を作らない。

既存のviewport別Focusは`egui-0.37.0/src/memory/mod.rs:503`、そのbegin_passは`:569`、Memory側のviewport選択は`:787`。focusからの中断要求は`:921`、`:931`、中断APIは`:1078`。

**最小回帰は同一Contextの実際の入れ子run_uiで行う。**

親子に別ViewportIdと実TextEditを用意し、事前passでfocusを整える。試験対象passの初期focus設定が、意図しない中断要求として混ざらないようにする。親run_ui内で子run_uiを実行し、親子のFullOutputを別々に検査する。

| 試験対象passの要求 | 親のIMEOutput | 子のIMEOutput |
|---|---|---|
| 親のみ。子を呼ぶ前に要求 | true | false |
| 子のみ | false | true |
| 両方 | true | true |
| どちらもなし | false | false |

親子ともSome(IMEOutput)を出していることを確認し、Noneによる見かけの成功を避ける。次の要求なしrunではfalseへ戻ることも検査する。Memoryのviewport IDをテストから手で切り替えるだけでは受入証拠にしない。

各FullOutputは検査後にheadlessの明示破棄を行う。親子のPlatformOutputをappendして試験結果を混ぜない。

この修正はdetachedに関わる構造変更として、着手前の合意と`docs/detached-rework-plan.md` §11記録に含める。App側で要求を出し直す、追加repaintで後から中断する、といった対策は採らない。

#### discard再passのIME出力集約

Context::run_dynは各passのFullOutputをappendする。しかしPlatformOutput::appendは`self.ime = ime.or(self.ime)`であり、後のSome(IMEOutput)が前の値全体を置き換える。TextEditは通常、各passのIMEOutputを中断要求falseで生成する。

根拠: `egui-0.37.0/src/context.rs:1111`、`egui-0.37.0/src/data/output.rs:242`、`egui-0.37.0/src/widgets/text_edit/builder.rs:1056`。

したがって、第1passで中断要求true → discard → 第2passで要求なし・IME出力ありの場合、最終出力の要求がfalseになり得る。nativeのcontrol配送を直しても、生成・集約段階で失われた要求は復元できない。

**同一viewportの未配送出力について、次の集約契約へ修正する。**

- purpose、rect、cursor_rect等は、新しいSome(IMEOutput)の値を採用する。
- should_interrupt_compositionは、集約前と新出力の要求のORを保持する。
- 新出力がNoneなら既存のSomeを保持し、従来のOption集約の意味を維持する。
- 両方NoneならNoneのままとする。
- 集約済み出力をbackendへ一度移譲した後は、次の新規出力へ要求を持ち越さない。
- OSのACK待ち、新しいqueue、retry、Context外のsticky flagは作らない。

| 旧出力 | 新出力 | 集約結果 |
|---|---|---|
| Some・要求true | Some・要求false | 新metadata・要求true |
| Some・要求false | Some・要求true | 新metadata・要求true |
| Some | None | 旧Someを保持 |
| None | Some | 新Some |
| None | None | None |

PlatformOutputはviewport IDを持たないため、呼出側も別viewportのplatform出力を混ぜない。共有texture deltaの集約とは区別する。

**必須回帰は実Context::run_ui内でdiscardさせる。**

1. 同じviewportで実TextEditを描く。
2. 第1passだけ中断を要求し、request_discardする。
3. 第2passでは中断を要求せず、TextEditからIMEOutputを出す。配置を変え、矩形等を第1passと区別できるようにする。
4. 実際に再passしたことを確認する。
5. 最終出力が第2passのIME metadataと中断要求trueを持つことを検査する。
6. 次の新規run_uiでは要求falseとなることを検査する。

PlatformOutput::append単体の補助試験だけでは受け入れない。nativeの複数logical batch試験とも別の回帰とする。出力は検査後にheadless明示破棄し、製品側の無条件clearは追加しない。

回帰名には共通の`ime_interrupt`を含め、段2aと正式graph切替後の両方で選択・実行できるようにする。修正前失敗・修正後成功の証拠は実装時に取得するものであり、今回実行済みとは扱わない。

この2件はmIV固有ではなく、最小再現と修正前失敗・修正後成功の証拠を添えて本家へ報告する価値がある。報告するかは利用者判断とし、今回は送信しない。

#### native経路のIME出力契約

0.37ではMemory::request_focusがIME中断を要求し、IMEOutput::should_interrupt_compositionへ出る。通常egui-winitはset_ime_allowedの切替で処理する。

根拠: `egui-0.37.0/src/memory/mod.rs:921`、`src/context.rs:3176`、`src/data/output.rs:113`、`egui-winit-0.37.0/src/lib.rs:1277`。

以下の配送契約は、上記のviewport別生成とdiscard再pass集約を修正した出力を前提とする。配送側で親子混入や生成済み要求の消失を補修しない。

mIV nativeはegui-winitを通らず、現在はcursor矩形だけをintentへ変換する。

| mIV箇所 | 内容 |
|---|---|
| `src/video/native_presenter/render_core.rs:13702`、`:14007` | IMEOutput→intent |
| `src/video/native_window_host.rs:149`、`:564`、`:618` | intent型・HWND owner・候補窓更新 |
| `src/video/native_window_pump.rs:1697` | WindowEpochに属するhostへの適用 |
| `src/video/native_window.rs:2308`、`:2341` | OS IME入力・終端 |
| `src/video/native_presenter/render_core.rs:9928` | native IME→egui入力 |
| `src/video/mod.rs:5159`等 | render出力のpump配送 |

移行契約:

1. IMEOutputからcursor位置と中断要求の両方を取り出す。
2. 中断を表すNativeWindowIntentを追加し、既存WindowEpochの対象hostへ送る。rendererから直接USER32／IMM32を呼ばない。
3. 対象は既存IME cursor更新・keyboard入力が所有するHWNDとする。GetForegroundWindowでその場の別窓を選ばない。
4. owner thread上でcompositionを中断する。現在のIMM32経路では、対象HIMCへのImmNotifyIMEの取消操作を基本とし、contextを解放する。通常のIME許可・候補窓位置を恒久的に無効化しない。
5. OSから返るPreedit／Commit／Disabledは既存native入力queueと正規化へ戻す。偽の通常キーや成功扱いのCommitを生成しない。
6. intentはlogical outputから一度だけ移譲する。再presentやmulti-passの同じ出力から再発行しない。
7. **中断要求を上書き可能なvisual snapshotだけに載せない。** 現publish_visualは最新値で置換し、try_lock不成立時には保存しないため、pulseを保証できない。
8. 中断intentは既存control queueの命令として配送する。新queue・pending bool・retryを作らず、cursor／HUD矩形等の最新値配送は既存経路へ残す。
9. control命令は同じWindowEpochへ適用し、終了したhostの要求を新HWNDへ付け替えない。
10. channelやIMM32のまれな失敗は既存のログ・通知・fault終端へ揃える。再試行や待機を追加しない。

上書き経路の根拠: `src/video/native_window_pump.rs:339`、`:1604`。既存control送信は同`:207`付近。logical batchのintent集約は`src/video/native_presenter/render_core.rs:3772`。

非対話回帰では、実TextEditのfocus／cursor移動から中断flagが出ること、intent変換、controlへの一度だけの配送、WindowEpoch所有、複数logical pass、visual更新を挟んでも消えないことをfake hostで検査する。OS上の取消結果は段7の実機で確認する。

段3では、段2aの生成・集約回帰が正式graphで使われることと、集約済み要求が通常backend／nativeの正しい配送先へ一度だけ渡ることを接続確認する。

**挙動・見た目:** 日本語確定直後の通常操作、取消、兄弟IME独立性を保持する。下線・cursor・候補窓位置は外観差があり得る。nativeでは新APIの中断要求を正しく実行する。

### 4.6 Modifiersイベント化、keymap、clipboard

RawInput.modifiersはなくなり、ModifiersChangedがlevelを更新する。イベントごとのmodifiersは残る。

根拠: `egui-0.37.0/src/data/input/event.rs:94`、`src/input_state/mod.rs:434`、`egui-winit-0.37.0/src/lib.rs:548`。

| 箇所 | 内容 |
|---|---|
| `src/app.rs:59309` | OS resync |
| `src/key_input.rs:2459`、`:2466` | 合成入力 |
| `src/video/native_presenter/render_core.rs:2716`、`:9780`付近、`:12505` | native queue／独立Context |
| `src/ui_fullscreen.rs:67268`、`src/ui_adjustment_panel.rs:10985` | テスト |
| `src/ui_erase.rs:815`、`src/ui_conceal.rs:742`、`:1341` | stale対策 |
| `src/ui_adjustment_panel.rs:12014`、`:12510` | Space／Ctrl編集 |
| `src/keymap.rs:7468`、`src/keyboard_input.rs:305` | chord／consume |

modifier変化を対象イベントより前へ並べる。frame末尾に最終値だけ追加せず、過去edgeのmodifiersをOS resyncで書き換えない。

FSのOS直読み、focused permit、物理ラッチ、左右Ctrl、Enter／NumpadEnterは更新だけで撤去しない。ModifiersChangedはKeyUp欠落を保証しない。

新しい修飾Key・Windows固有Keyを理由にKeyActionの保存名・既定割当を変更しない。

Ctrl+Shift+C/X/Vは通常窓・nativeとも保持する。

- 現vendor: `vendor/egui-winit/src/lib.rs:2314`
- 本家command_only: `egui-winit-0.37.0/src/lib.rs:1538`
- native: `src/video/native_presenter/render_core.rs:9863`

共通の純粋なclipboard条件へ揃え、I/Oは各integrationに残す。Process、IME、release、空clipboard時の二重Text防止を保持する。Ctrl+AltやInsert／Delete系をついでに拡大・縮小しない。

### 4.7 Id、scope、永続化

- Id::newはId::uniqueを呼ぶ非推奨API。
- AsIdは非推奨として残る。
- AsIdSalt／IdSaltが導入され、Id::withのhash構成も変わる。
- Ui::idはscope_id。
- From<&str>を使う移行にはしない。

根拠: `egui-0.37.0/src/id.rs:8`、`:91`、`:102`、`:121`、`:142`、`src/id_salt.rs:43`。

root identityはunique、局所塩はIdSalt。ui.id()を機械的にunique_id()へ変えず、scopeの意味ならscope_idを使う。生成・lookupを同時移行する。

代表: `crates/egui-ime/src/lib.rs:159`、`src/ui_music_panels.rs:706`、`:1563`、`tests/ui_snapshot.rs:443`、`tests/book_composition_editor_snapshots.rs:405`。件数は§3。

切替直後のId非推奨は短期間許容し、段5で整理する。保留しても旧Id互換を保証しない。

現manifestでeframe persistenceは有効でなく、独自egui memory保存も見当たらない。本家保存はfeature内。根拠: `eframe-0.37.0/src/native/epi_integration.rs:470`。

settings.dbは初期化しない。リリース済みId永続経路が実装時に発見された場合だけ、旧読込・変換・互換試験を設計する。独自窓配置とsession memoryを区別する。

### 4.8 vendorパッチ台帳

分類:

- **(a)** 本家同等で旧差分を外せる。
- **(b)** 保証・責務が残り付け直す。
- **(c)** 旧実装は不要。必要機能は新APIへ移す。
- **(b・新規)** 今回判明した移行上の所有・互換修正。

元src差分はeframe 9ファイル・約2270追加／124削除、egui-wgpu 9ファイル・約2004追加／192削除。READMEだけを台帳にしない。

#### egui

| 項目 | 分類・方針 |
|---|---|
| release／click count射影 | (b) `vendor/egui/src/input_state/mod.rs:1485`。順序・非click・focus・discardを保持。本家分類と分離 |
| 旧分類に依存するfixture | (c) 旧count固定を外し、近接・距離境界・時間条件を別検査 |
| discard_pending_scroll | (b) 同`:595`。新WheelStateへ適応 |
| repaint delay witness | (b) `src/context.rs:1794`とfeature |
| IME中断要求のviewport所有 | (b・新規) Memory共通フラグを既存のviewport別Focusへ移管。親子のbegin／endで消失・混入させない |
| IME中断要求のpass出力集約 | (b・新規) 最新のIME矩形等を採用し、同一viewportの未配送要求をORで保持 |
| 入れ子run_ui・実discard回帰 | (b・新規) §4.5の親だけ／子だけ要求と、実run_ui再passの最終出力を検査 |
| 承認済み子viewportの非描画退役入口 | (b・新規) 既存Context viewport GCを共通化し、pass外で対象登録を退役できるeframe向け入口を追加。兄弟やrootを変更しない |
| テスト・license・standalone | (b) 新構造へ移植 |

新wheel ownerは`egui-0.37.0/src/input_state/mod.rs:228`、`wheel_state.rs:54`。汎用非表示終端APIは追加しない。

IME修正の本家境界は`egui-0.37.0/src/memory/mod.rs:124`、`:503`、`:799`、`:1078`、`src/context.rs:1111`、`:3173`、`src/data/output.rs:242`。既存フラグの所有移管と既存appendの集約契約修正に限定し、App側の補助フラグ、追加repaint、新しいqueue、retryを追加しない。

親子要求の分離はdetached構造合意・§11記録の範囲へ含める。出力集約では別viewportのPlatformOutputを混ぜず、配送後の次frameへ要求を残さない。

非描画退役は、Appの既存終了ownerが承認したviewportの寿命終了に限る。通常のGC境界は`egui-0.37.0/src/context.rs:3317`以降。最小化・hideだけでは呼ばず、UI pass強制、全memory reset、退役済みIDを抑止する別ラッチで代用しない。通常GCとの処理範囲共有、兄弟不変、後続出力での非再作成を回帰化する。

#### egui-winit

| 項目 | 分類・方針 |
|---|---|
| Process基本機構 | (a) 本家を土台 |
| Hidden／Forwarded、release／focus終端 | (b) 保持 |
| Disabled取消通知のproducer配送 | (b) 旧版の通知保証を0.37変換へ付け直す |
| State→正規化→実TextEdit回帰 | (b・新規) 段2a冒頭で成立 |
| trace・文字秘匿 | (b) 保持 |
| 配送／State網羅回帰 | (b) 新modifier／clipboardへ接続 |
| Windows clipboard互換 | (b) Ctrl+Shiftを保持 |
| 旧300ms補正 | (c) 撤去済み。戻さない |

汎用非表示受付は追加しない。Disabledをkey hold全消去の契機にしない。

#### eframe

| 項目 | 分類・方針 |
|---|---|
| #7905 hidden repaint基本修正 | (a) 本家採用 |
| dirty／painting、surface generation、outer phase | (b) `native/run.rs:45`、`:118`、`:425` |
| 非ゼロresize inline provenance | (b) 同`:1282`、`native/wgpu_integration.rs:978` |
| 初回visible commit／Frame getter | (b) `src/epi.rs:736`、`native/epi_integration.rs:305` |
| startup window observer | (b) `src/startup_window_observer.rs:8` |
| max_texture_side統一 | (b) `native/wgpu_integration.rs:735`、`:1235`。同等性回帰後に撤去判断 |
| 未消費RawInputのviewport所有 | **(b・新規)** EpiIntegration共通保留をViewportへ移管。§4.2 |
| child skip時の入力保持 | **(b・新規)** take前判定。wgpu／glow同等経路 |
| 子Closeの非描画受付・消費 | **(b・新規)** 子callbackを待たずAppの既存modal判断・終了ownerへ渡す。拒否も一回の要求消費として扱う |
| 承認済み子の非描画退役 | **(b・新規)** 通常の対象限定cleanupを共通化し、window・surface・入力・Context登録を親Full待ちにせず終了 |
| 非描画・消滅viewport delta | (b) 同`:828`、`:1313`。共通完結ownerへ |
| immediate child screenshot | (b) callbackへ移行。旧Event方式は(c) |
| 通常版immediate計測 | (b) 同`:1206`、`:1396`。段階別時間・delta bytes・遅延ログ |
| test-script witness／capture・focus診断 | (b) callback-local identity |
| 実ウィンドウ試験隔離 | (b) 段0a |
| manifest／Win32 feature／standalone | (b) 新原本基準 |

RawInput修正は保持のviewport所有に限定する。汎用フィルタ・復帰pruneは追加しない。交互実行、child skip→復帰、終端、新規入力、retireの回帰を付ける。

CloseはUI復帰待ちの通常入力と区別する。wgpuのUI省略・Close生成は`eframe-0.37.0/src/native/wgpu_integration.rs:702`、`:716`、`:1031`、glowの同等経路は`src/native/glow_integration.rs:692`、`:708`、`:1061`。両backendで同じ受付・消費・退役契約を実装する。

App hookはbackend borrow／lockの外で呼ぶ。承認・拒否した要求をinfo.eventsや保留snapshotから再配送せず、新しいCloseは受け付ける。rootの終了・tray契約と、mIV非管理Appの従来UI経路を維持する。

対象限定cleanupの本家境界はwgpu `:1330`、glow `:1533`。承認済みCloseを実際のwindow／surface退役とegui登録終了まで接続し、兄弟の入力・寿命・共有deltaを維持する。追加repaintや子UI pass強制で代用しない。

通常版immediate計測はstartup／test-script診断と別責務として保持し、常時I/Oへ拡大しない。

本家pending deltaは`eframe-0.37.0/src/native/wgpu_integration.rs:795`、`:1290`。保持だけを完結と扱わない。

#### egui-wgpu

| 項目 | 分類・方針 |
|---|---|
| managed mipmap／再生成／sampler | (b) `src/renderer.rs:712`、`:777`、`:1151` |
| 公開MipmapGenerator／texel count | (b) `src/mipmap.rs`、`src/mipmap.wgsl`、`src/lib.rs:35` |
| atlas診断／renderer identity／texture_size | (b) 新delta列へ |
| 無描画delta | (b) `src/winit.rs:523`。drain／submit／free |
| 対象surfaceだけの破棄 | (b) 本家は全surface clear |
| startup GPU／root present observer | (b) `src/startup_diagnostics.rs`、`src/lib.rs:322` |
| screenshot／readback診断 | (b) callbackへ |
| shader空行等 | (c) 再適用しない |
| license／test接続 | (b) 新原本基準 |

本家surface差: `egui-wgpu-0.37.0/src/winit.rs:203`、`:218`、`:229`、`:247`。

既存atlas overflow skip-writeは根治策として拡張しない。診断を保ち、配送回帰でoverflowが生じないことを受け入れる。既存skip撤去は履歴を含め別判断とする。

#### 取り込み手順

1. 新原本・checksum・VCS・licenseを記録。
2. 論理項目ごとに差分を付け直す。
3. 旧manifestをコピーせず新manifestを基準にする。
4. root／standaloneの実体を統一。
5. PATCHES／READMEを実差分と一致させる。
6. `.cargo-ok`、targetを取り込まない。
7. 他DLL・モデル・SDKを巻き込まず、reparse pointを作らない。

### 4.9 wgpu、texture delta、native presenter

#### 現行経路・Windows依存

実コードに直接hal／windows_058利用は見当たらず、旧共有経路コメントが残る。現nativeはD3D11 OpenSharedResource1を使う。

- `src/video/native_presenter/render_core.rs:8347`、`:8379`: shared resource
- 同`:6708`: 動画Present
- `src/video/native_presenter/overlay_gpu.rs:43`: overlay
- `src/app.rs:17366`付近: 旧説明

未使用直接依存とコメント訂正は旧版上へ前倒しする。未使用windows_062別名を追加しない。hal30のWindows依存は0.62系。根拠: `wgpu-hal-30.0.1/Cargo.toml:518`。D3D11本体のwindows 0.61は全面更新しない。

#### API影響

| 箇所 | 内容 |
|---|---|
| `Cargo.toml:368` | wgpu30 |
| `src/lib.rs:495`、`:526` | surface |
| `overlay_gpu.rs:31` | Instance／DeviceEpoch |
| `render_core.rs:12089` | configure |
| 同`:13840`、`:13957` | delta |
| 同`:13891`以降 | render pass／submit／present |
| `src/compare_wgpu.rs:274`、`:539` | pipeline／callback |
| `src/panorama_wgpu.rs:332`、`:669`、`:506`、`:833` | pipeline／mipmap |
| `src/gpu_lanczos.rs:680`、`:708` | 縮小 |
| `build.rs:140`、`:230`、`Cargo.toml:495` | Naga27→HLSL→DXBC |

本家の必須差分:

- Instance値渡し: `wgpu-30.0.1/src/api/instance.rs:63`
- BGL Option配列／immediate_size: `pipeline_layout.rs:38`、`:44`
- multiview_mask: `render_pipeline.rs:238`、`render_pass.rs:682`
- color space／CurrentSurfaceTexture: `surface.rs:90`、`:148`
- Queue::present: `queue.rs:377`
- WgpuConfiguration.surface: `egui-wgpu-0.37.0/src/lib.rs:325`

configure write lock、submit／present read lock、Surface／DeviceEpoch寿命を維持する。新presentをgate外へ出さない。surface状態だけでdevice-lost latchやretryを増やさない。

#### TexturesDeltaの所有・消費

新版setはIDごとのdelta列、freeはsetで、非空dropはdebug assertionになる。

根拠: `epaint-0.37.0/src/textures.rs:284`、`:315`、`:335`、`egui-0.37.0/src/data/output.rs:72`。

consumerはmut参照または所有権を受け、借用適用しただけの非空出力を返さない。診断cloneに適用権を持たせない。

| 経路 | 完結責任 |
|---|---|
| 通常eframe | 既存pending ownerへ移譲。set drain→描画submit→free drain |
| native overlay | render batchがset／freeを消費 |
| surfaceなし・無描画 | rendererでset適用・upload submit→無描画確定後free |
| renderer未準備・早期終了 | 既存pending／logical batch ownerへ残りを引継ぐ |
| acquire不成立 | uploadをsubmitし、未処理freeを完結または引継ぐ |
| viewport消滅 | 共有renderer存続を考慮しdeltaを同時廃棄しない |
| renderer／Context終了 | 配送先不在をownerが確定し、対象単位で明示終端 |
| 描画不要headless | 検査後drop_without_applying_deltas |
| upload／描画test | 実consumerで適用・drain |

whole→partial、同一ID内の順序を維持する。freeは当該使用のsubmit後。早期return前に完結ownerへ移す。

製品の無条件clearは禁止する。headless APIで出力を捨てて同じ製品Contextを継続しない。

例: `src/ime_focus.rs:795`。testもoutput検査後に明示破棄する。

#### 無描画submission

本家は早期returnにもqueue submission guardを持つ。根拠: `egui-wgpu-0.37.0/src/winit.rs:502`、`:514`。

共通処理で次を満たす。

1. rendererがあればsurface lookupより前にset適用・drain。
2. mipmap commandを含めsubmit。
3. queue writeだけでもsubmissionを完結。GPU待機は追加しない。
4. 通常submit済みなら重複空submitを足さない。
5. freeはsubmit後にdrain。
6. acquire／capture途中終了も同じownerで完結または引継ぐ。
7. 引継ぎ時にsetを二重適用しない。

本家freeはsubmit後。根拠: `egui-wgpu-0.37.0/src/winit.rs:768`以降。

headless GPUでno-surface、acquire不成立相当、whole＋partial、mipmap、free-onlyを通し、submission・readback・解放順を検査する。サイズ更新だけを証拠にしない。

#### 表示・性能

AutoVsync／latency 1の明示設定と環境変数を新SurfaceConfigへ移す。stencil対応だけを理由にbufferを追加しない。callback format／sample数を揃える。

RGBA atlasへ診断を合わせる。根拠: `epaint-0.37.0/src/image.rs:16`。

D3D11のNaga27→HLSL→DXBCは別経路であり、wgpu更新だけで同時更新しない。

**挙動・見た目:** 動画本体方式、アップスケール方式、VSync方針を維持。HUD文字・透明合成・frame latency・mipmapを段4cと実機で確認する。IME出力は§4.5を満たす。

### 4.10 egui_kittest、accessibility、snapshot

visible inputのaccessible nameを検査し、read-onlyを除外する。label／placeholderを認識する。

根拠: `egui_kittest-0.37.0/src/accessibility.rs:14`、`:26`、`:46`、`src/builder.rs:151`。

検査はsettled後・画像生成前なので、accessibility修正を候補画像収集より先に行う。

根拠: `egui_kittest-0.37.0/src/lib.rs:518`、`src/snapshot.rs:739`。

影響:

- `tests/ui_snapshot.rs:28`
- `tests/book_composition_editor_snapshots.rs:132`、`:247`
- `tests/support/ui_harness.rs:14`
- `tests/support/scroll_layout.rs:6`
- libのPNG生成fixture
- `src/ime_focus.rs:1268`等のraw TextEdit allowlist

kittest 0.3→0.5。NodeT／accesskit_nodeは残るが、名前・scope変更でquery先が変わり得る。根拠: `kittest-0.5.0/src/node.rs:9`、`egui_kittest-0.37.0/src/node.rs:21`。

名前は本体描画側へ付け、旧版で前倒しする。builder検査を無効化せず、role／親containerでqueryを限定する。最終pass geometry、DX12|VULKAN、lazy、CPU adapter優先、renderer独立を維持する。

順序:

1. accessibility修正。
2. baseline未更新で両integration＋libの通常比較。
3. 文字ラスタ、文字幅・行高、panel・margin、操作回帰へ分類。
4. 入力・geometry回帰を修正し外観採択。
5. 同じ条件で全対象更新。
6. 更新なしで全対象再比較。
7. 画面群の共通理由・対象集合・例外・確認者・結果を一台帳へ。

445枚を対象漏れ基準にする。contact sheetを索引に、欠け・位置・例外を原寸確認する。PNGごとの重複票や固定レビュー票は要求しない。

lib対象は実PNG生成呼出から列挙し、`--lib snapshot`だけで全件と見なさない。

### 4.11 その他の挙動と複数viewport

| 変更 | 方針 |
|---|---|
| popup sizing pass | 計測passで保存・選択commandを発行しない |
| release即閉じ防止 | 右click、外側終了、wheel終端を検査 |
| menu hover-open | 選択・wheel所有を維持 |
| ScrollArea端fade | 旧外観維持を初期案 |
| Window margin／min size | 小画面の本文・固定操作を再計測 |
| tooltip／container | 入力owner・query混入を検査 |
| repaint-after削減 | frame頻度依存を直し、追加repaintで戻さない |
| rootのみ保存／復元 | 独自配置との二重所有を避ける |
| raw mouse motion | unfocused受信をkeyboard ownerの根拠にしない |
| Screenshot callback | 既存batch／worker I/Oへ |
| set_fonts即時適用 | atlas／galley失効を確認 |

根拠: `egui-0.37.0/src/containers/popup.rs:629`、`:660`、`menu.rs:440`、`scroll_area.rs:1704`、`window.rs:633`、`:696`、`context.rs:133`、`:2505`、`:4557`、`eframe-0.37.0/src/native/wgpu_integration.rs:511`、`:903`。

mIV代表は§4.3のpanel、§4.4の入力、`src/app.rs:97661`以降の完了処理、`src/ui_fullscreen.rs:22139`以降のviewport。captureは`src/test_script/capture.rs:282`、`:375`。callbackでPNG encode／I/Oをしない。

#### detached凍結

App分離、callback Ui化、gamepad、保留RawInputのviewport所有、IME中断要求のviewport別所有と入れ子pass、同一viewportのdiscard再pass出力集約、native IME intent、非表示子Closeの受付・消費・既存終了ownerへの合流・非描画退役、scheduler／delta／surfaceは構造合意対象とする。

着手前に設計担当と独立レビュー担当が症状パッチではないことに合意し、`docs/detached-rework-plan.md` §11へ範囲・理由を記録する。本改訂案自体を合意済みと扱わない。

IME追加分では、親子の要求が消失・混入しないこと、最新の候補窓矩形等を採りながら未配送要求を保持すること、配送後に要求を残さないことを不変条件として記録する。

Close追加分では、対象window／contextの既存modal判断と終了処理を一つのownerに揃えること、承認・拒否した一回の要求を再配送しないこと、子や親のUI復帰を待たず承認済み対象を退役させること、兄弟の入力・資源・寿命を変更しないことを記録する。Context側の非描画退役入口とwgpu／glowのcleanup共通化も合意範囲へ含める。

rect捕捉条件、geometry由来recreate、新detached pending bool、時間窓、兄弟資源の一括resetを追加しない。

### 4.12 Rust 1.99警告とMSRV

既存ログはf32 fallback 445件／36ファイル、lib全体688警告。今回の実行結果ではない。

代表: `src/ui_main.rs:184`、`src/ui_text.rs:519`、ui_adjustment_panel、ui_fullscreen、native overlay_draw。

| 対象 | 方針 |
|---|---|
| Stroke／PathStroke | 新版のf32型で解消分を再集計 |
| その他float | 必要境界へ局所型注釈 |
| vendor警告 | 新原本で再評価 |
| fetch_update | egui更新では解消しない。MSRV互換を確認 |
| dead code | feature／test／Windowsを区別し、機能削除で減らさない |
| IME互換通知の非推奨 | 必要箇所に理由付きで限定許容。等価取消契約なしに削除しない |

根拠: `epaint-0.33.3/src/stroke.rs:27`、`epaint-0.37.0/src/stroke.rs:26`、`:140`。

Atomic代表: `src/log_writer.rs:105`、`:165`、`src/everything_ipc.rs:226`、`src/ime_focus.rs:225`、`src/similar_index.rs:1431`、vendor witness。

egui／eframe 0.37のMSRVは1.95。根拠: 各`Cargo.toml:14`。workspace 1.88を更新し、開発toolchain 1.99固定と分けて1.95で検証する。

try_updateの最小安定版を未確認のまま採用しない。全crateの警告抑制を行わず、残りは段5で整理する。

### 4.13 Remote／他crate

| 対象 | 波及 |
|---|---|
| egui-ime | 入力・focus・取消・testを移行 |
| 4ラボ／vst3-host-tester | App／Ui／Panel／RawInputを同時切替 |
| remote-web／remote-ipc | egui直接移行・protocol変更不要。非表示応答を確認 |
| launcher | egui非依存。MSRV・埋め込みcore更新 |
| epub-pdf-worker | egui移行なし。companion build |
| comic-core | 独立ラスタライザ維持 |
| local-adjust-core／music-core | egui非依存維持 |
| Susie／LibRaw／ORT／VST bridge | ABI更新を含めない |

root・各crate・ラボmanifestを棚卸しし、旧新版eguiの型を分裂させない。

## 5. 利用者に相談する事項

0.37を目標にする決定は受領済み。以下の外観・操作差は比較可能な結果を用意して確認する。

| 項目 | 推奨 |
|---|---|
| 字面／baseline／hinting | fallback・保存値を保持して比較整理 |
| color emoji | 代表比較を提示 |
| Window本文drag | 必要箇所で従来操作保持 |
| mouse drag scroll | 旧版で使えた画面は保持 |
| Panel collapse／double-click | 既存操作維持を初期案 |
| **一般の連続クリック分類** | **本家0.37の距離6 points未満・新時間条件の採用を推奨。利用者確認前に操作差を確定しない** |
| 端fade | 旧外観維持を初期案 |
| margin／min size | 到達性を保証した比較案 |
| system fallback | 既存明示chain基準 |
| Ctrl+Shift+C/X/V | 通常／nativeとも保持 |
| 受信済み入力・release確定 | 保持。取消仕様へ変えない |
| gamepad保持軸の復帰後継続 | 現行どおり。中立通過を要求しない |

連続クリックでは、同じ広いwidget上でも位置が6 points以上離れる操作や、旧triple条件だけ成立する操作が変わる。音量・速度reset、reader、resize reset、文字選択の代表例を示す。旧判定や別タイマーによる互換化は推奨しない。採用不可の場合は実装前に範囲を再設計する。

受信済みキー取消、release確定抑止、gamepad軸への中立要求は、具体的な操作差を示して相談する。本版は必須化しない。

clipboard保持や保留所有修正のために版選択を再確認しない。Ctrl+Q追加、OS直読み撤去、動画方式変更、パネル本文省略は自動採用しない。

## 6. 段階設計

### 6.1 共通条件

各段で通常製品ビルドを通し、自動検証・残る検証を記録してコミット可能にする。旧baseline不一致を成功と記録しない。

依存切替の型結合部分は原子的に扱う。レビューは領域分割し、固定票を要求しない。原子範囲が大きすぎる場合は旧版上の挙動不変抽出を追加する。

規模は原本、lockfile、PNG、純移動を分けた推定。ファイル数は段間重複を含む。

反復はtest-focusedまたは明示filter、無filter main libはtest-lib-full、最終gateは実ウィンドウを分離したtest-fullを使う。

### 段0a: 実ウィンドウ試験を自動gateから分離

最初に実施する。未修正のtest-fullを非対話gateとして先に実行しない。

`scripts/test-full.ps1:85`は、子processでrun_nativeを呼ぶ非ignored試験を含む。根拠: `vendor/eframe/src/native/run.rs:782`、`:809`、`:828`。

live試験は削除・条件緩和せず、ignored等で既定対象から分離し、承認付き入口を用意する。pure scheduler試験は自動継続する。

AGENTS.mdに従い、起動・foreground切替・入力の前に具体suite、時間、desktop使用、使い捨てdataを示して明示承認を得る。test名・短時間・子processは例外でない。

エージェントは正規portable-smoke成果物のみ起動する。standalone vendor窓試験を残す場合は利用者実行の隔離試験とする。

**受入:** core check、vendor test compile、pure scheduler、runnerの静的／fake検査。窓は起動しない。

**規模:** 3～6ファイル、80～220行。

### 段0b: 基準固定

source、toolchain、feature、fixture、警告、PNG、既知失敗を記録する。detached構造合意にRawInput所有、IME中断要求のviewport別所有・入れ子pass・discard再pass集約、native IME配送、非表示子Closeの受付・消費・既存終了ownerへの合流・非描画退役を含める。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt --check
```

有効な旧版検証台帳を再利用する。既知失敗を上限緩和で隠さない。snapshot生成箇所とlib filter集合を列挙する。

実施記録: [旧版基準台帳](egui-0.37-baseline.md)と[detached §11下書き（合意待ち）](detached-rework-plan.md#2026-10-16-egui-037-更新に伴う構造修正-合意待ち)（2026-10-16）。

**規模:** 文書・台帳3～5ファイル、100～250行。

### 段1a: 0.33.3上の小さい準備

未使用wgpu-hal／windows_058除去、旧コメント訂正、accessibility名付与を行う。可視文字追加を伴うものは分ける。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo fmt --check
python scripts/check_ui_glyphs.py
```

焦点試験・旧PNG通常比較。名前だけなら画像不変を確認する。適切な区切りでtest-lib-fullを行う。

**規模:** 12～30ファイル、180～500行。

### 段1b: 0.33.3上の挙動不変抽出

- service／input／render／post-renderを旧順序のまま抽出。
- gamepadの物理owner、未配送edge・意味終端の引継ぎowner、UI batchを整理。
- foreground判定、wheel投影、clipboard、output完結入口を抽出。
- native IME出力変換・intent配送の既存入口を整理。
- Win32 queue・mouse-nav pendingのdrain条件を維持。

新しい入力除外・IME中断動作はまだ導入しない。

```powershell
cargo test -p mimageviewer --lib tray
cargo test -p mimageviewer --lib startup
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib sidecar_restore
cargo test -p mimageviewer --lib gamepad
.\scripts\test-lib-full.ps1
```

core check、旧snapshot、focus喪失後のrouted edge、gamepad終端を含める。

**規模:** 14～30ファイル、700～1500行。service、gamepad、描画出力を分けてコミットする。

### 段2: 新版vendor再構築

準備用の実ディレクトリ・standalone graphで新版を検証し、root patchは旧vendorのまま維持する。新版egui-imeの共有正規化も準備graphへ接続する。正式置換は段3。reparse pointは使わない。

| 小段 | 範囲 | 推定規模 |
|---|---|---|
| 2a | egui既存API、IME中断要求のviewport所有・pass出力集約、承認済みviewportの非描画退役入口、クリックfixture、egui-winit IME配送・Disabled互換・trace・clipboard、共有IME fixture | 11～20ファイル、900～1850行 |
| 2b | egui-wgpu mipmap、delta完結、surface、診断 | 10～16ファイル、1100～2100行 |
| 2c | eframe scheduler、viewport入力所有、skip保持、子Close受付・消費・非描画退役、visible commit、delta、計測・診断 | 14～24ファイル、1700～3350行 |

原本置換は別計上する。

第4.1版では段2aに約200～400行を追加した。主な増分はegui内の限定修正と、実入れ子run_ui・実discardのheadless回帰である。App側の新しい入力状態や配送queueは含めない。

第4.2版では、通常GCと共有する非描画退役入口・回帰として段2aへ約100～200行、wgpu／glowのClose受付・消費・対象限定退役と回帰として段2cへ約250～500行を追加した。App側の終了owner接続は段3へ別計上する。

#### 段2aの順序・受入条件

1. **冒頭でState変換から実TextEditまでのDisabled取消回帰を置く。**
2. 非空Preedit→Disabledのみ、focus喪失後、Commit後、重複通知、通常選択文字、兄弟Stateを検査。
3. Hidden／Forwardedの網羅回帰を移植。
4. Ctrl+Shift clipboard、Modifiers、trace秘匿を検査。
5. release射影の順序・非click・discardを保持し、分類fixtureを本家0.37へ合わせる。
6. **IME中断要求を既存のviewport別Focusへ移管し、同一Contextの親だけ／子だけ要求する実入れ子run_uiで、それぞれのIMEOutputを検査する。**
7. **PlatformOutputの同一viewport集約を修正し、実Context::run_uiの第1passだけで要求・discardした場合に、第2passのIME矩形等と中断要求trueが最終出力へ残ることを検査する。**
8. 親子ともIMEOutputが存在すること、次の新規runでは要求が残らないことを確認する。Memoryのviewport ID手動切替やappend単体試験だけで代用しない。
9. 生成・集約回帰の名前に共通の`ime_interrupt`を含め、準備graphと正式graphで選択できるようにする。修正前失敗・修正後成功の証拠を実装時に取得する。
10. IME中断要求の所有修正について、着手前の構造合意とdetached計画§11記録があることを確認する。
11. 承認済みviewportの非描画退役入口を通常GCと共通化し、対象登録の終了、兄弟不変、後続root／兄弟出力からの非再作成を検査する。hide／minimizeだけでは退役しない。

本家クリック分類の製品採用は§5の判断を要する。vendor fixtureで本家の事実を検証することと、製品操作差の受入れを混同しない。

段2aはDisabled入力互換、IME中断要求の生成・集約、既存パッチ移植をレビュー可能なコミットへ分ける。各回帰は実windowを作らず、FullOutput検査後のheadless明示破棄を行う。App補助フラグ、追加repaint、新しいqueue、retryは追加しない。

非描画退役の抽出はIME修正と別コミットに分け、Close lifecycleの構造合意・§11記録に含める。

#### 段2bの受入条件

whole／partial／free、no-surface、submission、mipmap、headless readback、対象surfaceだけの破棄、通常／診断featureを検査する。

#### 段2cの受入条件

- RawInput保留がViewport所有となり、共通EpiIntegrationから別viewportが取り出せない。
- root LogicOnly(A)→child Full(B)→root Full(C)でA／B／Cが正しく分離・順序保持される。
- child skip→他viewport更新→child復帰でrelease・focus・ModifiersChanged・IME終端・新規入力が残る。
- skipを繰り返しても欠落・重複がない。
- hookへ正しいviewportの入力だけが渡る。
- immediate／deferred、wgpu／共有glow経路の入力取得・寿命処理が一致する。
- viewport終了が兄弟の保留を消さない。
- scheduler、outer phase、SW_HIDE、最小化、visible child、期限超過をpure testで確認。
- 子を最小化→Close要求→復元せず終了する。子callbackやUI passを実行して成功させない。
- rootもlogic-onlyの場合に、承認済み子のbackend／Context登録を退役できる。
- modalで拒否したCloseが、modal終了・子復元・後続Fullで再発火しない。
- 拒否後の新しいCloseは通常どおり受け付ける。
- 子Fullと親処理の順序が変わっても二重処理せず、初回callback前のCloseも扱える。
- OS Closeとprogrammatic Closeが同じ受付を通る。rootの終了・tray契約と既定Appの従来委譲を維持する。
- 承認後の兄弟Full／root Fullで対象を再作成せず、兄弟の入力・IME・texture・surface・寿命を維持する。
- 拒否した要求だけを消費し、通常のkey／pointer／focus／IME列を消さない。
- 未配送deltaを共有rendererへ引き継いでから対象を退役する。
- wg​​pu／glowのClose配送・消費・退役が同じ契約を満たす。
- §11の構造合意・記録がある。

各段でstandalone非対話試験と旧root core checkを維持する。live compileは可能だが実行は段7。全差分を台帳化する。

Close回帰はbackendの実際の受付・消費・退役helperとfake App／resource ownerを接続して行い、単に「close予定になった」だけで成功としない。追加repaintや強制UI passを必要とせず登録・資源が退役することを非対話で検査し、実HWNDの結果は段7で確認する。

### 段3: 依存と必須契約の原子的切替

範囲:

- egui系0.37、wgpu30、ラボ、egui-ime、kittest。
- App／Ui／Panel／Context／on_exit。
- logic service、Modifiers、wheel。
- viewport所有RawInputとIME producer互換の正式接続。
- IME中断要求のviewport別生成とdiscard再pass出力集約の正式接続。
- 子Closeの非描画受付、既存modal判断・終了ownerへの合流、承認後の対象限定退役。
- gamepad物理更新・受付事実・未配送edge保持・UI batch。
- native GPU／surface／present、IME中断intent・control配送。
- 全delta消費・引継ぎ・headless明示破棄。
- clipboard、screenshot callback、全test compile、追加accessibility。

必須契約を段4へ残さない。汎用非表示機構は作らない。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo test -p egui --lib ime_interrupt
cargo test -p egui-ime
cargo test -p egui-winit --lib
cargo test -p mimageviewer --lib --no-run
cargo test -p mimageviewer --test ui_snapshot --no-run
cargo test -p mimageviewer --test book_composition_editor_snapshots --no-run
cargo fmt --check
```

`ime_interrupt`は段2aで追加する生成・集約回帰の共通filterとし、正式graphでの実選択件数を記録する。0件選択を成功と扱わない。

実接続点の必須回帰:

- deltaの描画・無描画・早期終了・headless。
- root／child交互実行とchild skip後の終端保持。
- 最小化したpassive deferred子のCloseが、既存modal判断・終了ownerへ届き、復元せず終了する。
- active／parkedの対象contextへの終了委譲を維持し、passive画像のcleanupで代用しない。
- modalで拒否したCloseが後から再発火せず、新しいCloseは受け付ける。
- OS Close、programmatic Close、bar／watcherのApp入口を同じ終了判断へ揃え、子FrameのOS Close読取りによる二重採用を残さない。
- rootもlogic-onlyの場合に承認済み対象を退役でき、兄弟の入力・資源・寿命を維持する。
- 対象Close時に既存gesture／media／worker ownerを終了し、共有deltaを失わず、後続root／兄弟Fullで子を再作成しない。
- Disabled取消がproducerから実TextEditまで到達。
- 同一Contextの親子で中断要求が消失・混入せず、正式graphで修正済みeguiが使われる。
- 実run_uiのdiscard再passで最新のIME矩形等と中断要求が保持され、次の新規runへ要求が残らない。
- nativeのfocus／cursor移動から中断出力、intent、control、対象WindowEpochへ一度だけ配送。
- 集約済み中断要求が通常backend／nativeの正しい配送先へ一度だけ渡る。別viewportのPlatformOutputを混ぜない。
- native中断がvisual上書き・複数logical passで消失／重複せず、終了したhostから新hostへ漏れない。
- focus移動前の受信済み離散キー。
- gamepadの同じframe番号中の複数logic、短い退避復帰、press→release引継ぎ、disconnect／ring／Undo。
- gamepad保持軸の復帰後継続。既存`src/app/gamepad_input.rs:9697`、`:9763`、`:9809`の期待値を維持。
- WH_GETMESSAGE既存drain。

CloseのApp側接続では、`src/ui_fullscreen.rs:21756`、`:21876`の既存modal判断・終了処理を描画非依存のserviceとして再利用する。`src/app.rs:3524`のwatcher Closeやbarの×も同じ判断へ合流させる。新しいclose pending bool、復帰待ちqueue、時間窓は追加しない。

graphの旧egui混入・型分裂・古いpathを確認する。compileだけでは受け入れない。

#### 段3の再見積り

段1b・段2完了後の接続差分。原本・lockfile・PNGを除き、内訳は重複計上しない。

| 内訳 | 概算変更行 |
|---|---:|
| App／Ui／Panel／一般API／ラボ | 1200～2300 |
| event／wheel／native GPU接続。delta・IME出力を除く | 650～1300 |
| delta消費／引継ぎ／headless | 450～1000 |
| gamepad引継ぎ・受付・UI接続・必須回帰 | 200～450 |
| viewport入力所有・IME producerの本体接続回帰 | 100～250 |
| native IME出力・control配送・必須回帰。生成・集約済み出力の接続確認を含む | 250～550 |
| 子CloseのApp終了owner接続・入口統一・接続回帰 | 200～400 |
| **合計** | **3050～6250** |

**105～185ファイル程度。**

第3.1版のgamepad引継ぎを再評価し、Astra指摘の接続・回帰を追加した。段2a／2cのvendor実装は上表へ重ねて計上しない。

第4.1版では、IME生成・集約からbackend配送までの接続確認として約50～100行を追加した。egui内のフラグ所有・append修正と最小回帰は段2aへ計上し、ここでは重複計上しない。

第4.2版では、子Closeの既存終了ownerへの接続、OS／bar／watcher入口の統一、modal拒否後の非再発火と兄弟不変の接続回帰として約200～400行を追加した。非描画退役のvendor実装・単体回帰は段2a／2cへ計上する。

段1b後に実差分で再評価する。gamepadやnative配送の抽出が不足する場合は、旧版上の挙動不変抽出を追加し、原子切替へ未整理の大改修を押し込まない。

### 段4a: 入力・非表示・popupの等価経路確認

```powershell
cargo test -p mimageviewer --lib ime
cargo test -p mimageviewer --lib focus
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib keymap
cargo test -p mimageviewer --lib gamepad
cargo test -p mimageviewer --lib specified_scale
cargo test -p mimageviewer --lib fullscreen_overflow
cargo test -p mimageviewer --lib navigator
```

root／FS／native／兄弟contextのclose、cancel、focus loss、repeat、復帰新規入力へ広げる。IME入力・出力、pointer capture、クリック分類、wheel単位・混在修飾・消費・残量、通常／native clipboardを確認する。

filter選択件数を記録し、PNG外観差と機能回帰を分ける。

**規模:** 10～22ファイル、400～1000行。

### 段4b: フォント・layout採択

```powershell
cargo test -p mimageviewer --lib ui_fonts
cargo test -p mimageviewer --lib ui_font_catalog
cargo test -p mimageviewer --lib calculated_
cargo test -p mimageviewer --lib startup_dialog
python scripts/check_ui_glyphs.py
```

baseline、fallback、toolbar、HUD、preview、margin、separator、小画面を比較。Fits／Scrolls・末尾到達・固定操作を保持し、§1.380残件と今回退行を分ける。

**規模:** 8～18ファイル、400～1000行。previewが大きければ小段化。

### 段4c: GPU・native HUD整合

```powershell
cargo test -p mimageviewer --lib compare
cargo test -p mimageviewer --lib panorama
cargo test -p mimageviewer --lib gpu_lanczos
cargo test -p mimageviewer --lib native_presenter
cargo test -p mimageviewer --lib bottom_hud
```

delta、mipmap、callback、透明合成、font／DPI／galley、surface寿命、submit gateを検査する。native IME回帰は段3成立済みを再利用し、実Windowは分離したまま。

**規模:** 8～16ファイル、300～800行。

### 段5: 全snapshot更新と警告整理

accessibility通過後に通常比較し、外観採択後に更新する。

対象は両integrationと列挙済み全lib snapshot。

```powershell
$env:UPDATE_SNAPSHOTS = '1'
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 台帳に列挙した全lib filterを同じ更新環境で実行
```

更新専用の子PowerShellを使い、各非ゼロ終了を伝播する。終了後、環境変数なしの別processで同じ全対象を比較する。

```powershell
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 同じ全lib filterを更新なしで再比較
```

実装担当が候補・分類を用意し、設計担当が外観採択・目視結果を記録する。独立レビューは共通理由・例外・回帰証拠を確認する。一台帳で変更445枚全体をカバーし、固定票・PNGごとの重複票は作らない。

Id非推奨・残るRust警告を整理する。IME互換通知を警告削減だけで削除しない。

**規模:** UI／test 10～30ファイル、250～800行、PNG最大445枚。

### 段6: 非対話全体gateと確認用build

段0a完了後のrunnerを使う。

```powershell
cargo fmt --check
python scripts/check_ui_glyphs.py
.\scripts\test-lib-full.ps1
.\scripts\test-full.ps1
.\scripts\build-dev.ps1 -PreserveRuntime
```

通常feature、必要なtest-script、vendor非対話区分、MSRV、core／Remote／EPUBを確認する。

source、command、feature/profile、fixture、exit、件数、ログを記録し、同条件の成功は再利用する。0件選択やfocused runnerの広域変更exitを成功にしない。

既知失敗を除外・閾値緩和で隠さない。自動gate成功とlive未実施を別報告する。check/buildは最低10分、広域testは最低15分以上を確保し、timeout後のBrokenPipeを製品失敗と誤分類しない。

**規模:** 文書・検証補修3～8ファイル、100～300行。

### 段7: 承認付きdesktop検証・利用者実機受入れ

具体suite、時間、PC使用、使い捨てdataを提示し、明示承認後に実施する。改訂依頼を起動承認と扱わない。

エージェントは正規portable-smokeのみ。standalone vendor窓試験は利用者実行の隔離手順。native・DPI等は段6の通常buildを利用者が起動する。

UI確認は追加IME・クリック比較を含め25～40分程度。ReleaseChecks／scheduler live試験は別枠。未実施・環境不成立をPASSにしない。

各段の自動検証済みcheckpointを残し、Windows native変更の最終受入れ・コミット時期はAGENTS.mdの実機確認規則に従う。

## 7. 実機確認手順

通常版・トレイ常駐版を先に終了する。single-instance mutexを共有する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

既定で実利用中の`%APPDATA%\mimageviewer`を開き、設定・データを更新し得る。隔離確認を明示的に選ぶ場合は同成果物へ`--data-dir .\target\dev-runtime\data`を渡す。エージェントは通常profileを起動しない。

### 7.1 フォント・ダイアログ

1. 既定、BIZ UDPGothic 9pt、Meiryo 10pt、Meiryo UI 10ptで一覧・詳細・検索・metadata・toolbar。
2. 日本語、Latin、数字、CJK、数学英字、記号、emojiを比較。
3. baseline、折り返し、欠け、豆腐化を確認。
4. 動画／音楽HUDの時間、x1、Norm、dBを確認。
5. 小窓・UI倍率200%で長文dialogの本文・固定ボタン・×へ到達。
6. 採択したWindow drag、scroll、Panel操作を確認。

### 7.2 日本語入力・取消・中断

Google日本語入力とMS-IMEで共通の少数シナリオを行う。

1. 注釈本文をEnter確定し、確定だけでは改行しない。
2. 直後の通常Enterで改行一つ。
3. 検索欄は確定直後の通常Enterで検索。
4. 変換中Escape／候補Tab／Spaceがアプリへ漏れない。
5. Escape取消後に未確定文字が残らず、直後の通常Escapeが働く。
6. composition中に別窓へfocusを移し、重複・消失・キー残留がない。
7. 長い欄・複数行末尾で候補窓がcursorへ追従。
8. main／F12／native入力の代表を確認。
9. **native入力欄で変換中にcursor位置をclick移動し、古いcompositionの更新・確定が新位置へ残らない。**
10. **nativeで別入力欄へfocus移動し、前の欄の未確定文字が新しい欄へ混入しない。**
11. 中断後に新しく日本語を入力・確定し、IME許可や候補窓位置が戻る。
12. native窓を閉じて別動画を開き、旧窓の中断要求が新しい入力を取消さない。

OSがどのIME列を出したかと、画面結果を区別して記録する。未発生のDisabled-only経路はheadless回帰で担保し、実機発生を確認済みと書かない。

IME中断要求の親子分離とdiscard再pass保持は、段2a・段3の非対話回帰を主証拠とする。実機では上記のmain／F12／native代表経路で、正当な中断が働き、兄弟への不要な取消がないことを確認する。利用者にdiscardの発生や厳密なpass順序を手操作で再現させる追加マトリクスは要求しない。

### 7.3 shortcut・wheel・clipboard・クリック

1. main／F12でCtrl／Shift／Altを押して離し、通常へ戻る。
2. Ctrl+矢印、Space pan、筆切替。
3. Enter／NumpadEnter、右Ctrl、Tabの代表割当。
4. popup内wheel／Ctrl+wheelと終了で背面が動かない。
5. popup終了後の新wheelは働く。
6. slider／seekを外で離し、dragが残らない。
7. 通常／nativeでCtrl+C/X/VとCtrl+Shift+C/X/V。
8. IME終了直後も二重文字が入らない。
9. **音量・速度reset、reader、resize resetを近い位置のdouble clickで操作。**
10. **同じ広い部品上でclick位置を離した場合の差を、採択した0.37分類と比較。**
11. **文字のdouble／triple click選択を確認。**

6 pointsの厳密な境界・時間条件はfake入力回帰で検査し、利用者にpixel精度の操作を要求しない。

### 7.4 非表示・複数窓・Remote・gamepad

1. 非同期処理中にトレイへ隠し、完了後復帰。
2. 別にrootを最小化し、完了処理と復帰後新規入力を確認。
3. visible F12を残してrootをhide／minimizeし、子の入力・処理を確認。
4. childを最小化・復帰し、押下・focus・IMEの残留がない。
5. root最小化中、別アプリでgamepadボタンを操作して復帰。過去press／repeatが発火しない。
6. **軸を倒したまま復帰すると現行どおり継続し、中立・release後の新操作も働く。**
7. ring終了・disconnect後の対象contextとUndo。
8. composition中focus移動、drag外側releaseと復帰。
9. 同じPDF／画像の兄弟reader片方を閉じても他方を保持。
10. root非表示中のRemote応答・完了。
11. idle／wakeは承認済みReleaseChecks。
12. **別窓を2つ用意し、一方をpassive状態でタスクバーから最小化する。そのwindowのタスクバーメニューから「ウィンドウを閉じる」を選び、復元せず終了することを確認する。もう一方の表示・入力は維持する。**
13. **対象子で既存のCloseを拒否するmodal条件を作り、最小化したまま同じClose操作を行う。modalを終え、子を復元しても勝手に閉じないこと、その後の新しいCloseでは終了できることを確認する。**
14. rootも最小化した代表ケースで子をタスクバーから閉じ、root・子の復元を必要とせず対象だけが終了することを確認する。rootを戻した後も子が再作成されないことを確認する。

root LogicOnly→child Full→root Full、短い退避、同一batch順序、期限超過は非対話回帰を主証拠とする。実PCへ大量の任意入力を送らない。「前後入力をすべて捨てる」を期待結果にしない。

modal拒否・初回callback前・兄弟Fullとの順序網羅は非対話回帰を主証拠とする。実機のmodal条件は既存機能で再現できる代表一例に絞り、試験のための保存失敗や設定破損を作らない。エージェント実行の場合は追加したCloseシナリオも承認suiteへ明示する。

### 7.5 動画HUD・GPU

1. 再生、HUD文字、時間、速度、音量、track。
2. 820px程度から狭幅へ縮小し、重なり・menu・seek／音量drag。
3. pause／resume／seek／track変更／動画→音声→動画。
4. F11／F12往復、focus、透明合成。
5. 比較wipe／diff、panorama、縮小のmipmap・黒表示・境界。
6. fullscreenから一覧へ戻り、黒サムネ・atlas破損なし。

### 7.6 マルチモニターDPI

1. 100%と150%／200%間でmain／F12／動画窓を移動。
2. 字面・baseline、候補窓、HUD hit rect、slider／seek位置。
3. 最大化・復元・F11往復で操作が画面外へ失われない。
4. 子終了後もrootの表示・入力・撮影が継続。

## 8. 完了条件と文書更新

完了条件:

- 新版graphとvendor全差分を説明できる。
- egui-winit保持理由・削除条件・Disabled取消互換を記録している。
- 未消費RawInputを同じviewportだけが消費し、root／child交互実行とchild skip後復帰の回帰が通る。
- RawInput所有修正について構造合意とdetached計画§11記録がある。
- IME中断要求のviewport別所有と入れ子passについても構造合意とdetached計画§11記録がある。
- 子Closeの非描画受付・消費・既存終了ownerへの合流・backend／Context退役について構造合意とdetached計画§11記録がある。
- 最小化した子へのCloseが、復元や子callback実行を必要とせず既存終了ownerへ届く。
- modalで拒否した要求はその一回で消費され、modal終了・復元後に再発火しない。新しいCloseは受け付ける。
- rootもlogic-onlyの場合に承認済み子を退役でき、後のroot／兄弟Fullで再作成しない。
- 子終了で兄弟の入力・IME・保留RawInput・texture・surface・寿命を変更しない。
- wg​​pu／glowで同じClose配送・消費・退役契約を満たし、rootの終了・tray契約を維持する。
- 同一Contextの親だけ／子だけが中断要求を出す実入れ子run_uiで、要求の消失・混入がない。
- 実run_uiのdiscard再passで、最新のIME矩形等を採用しながら未配送中断要求を保持する。
- 集約済み出力を配送した後、次の新規runへ中断要求が残らない。
- serviceがUI省略時にも必要なwakeで進み、古いContext入力を操作として読まない。
- SW_HIDE／最小化／visible child／期限超過を区別する。
- 受信済み離散キー、Win32終端、mouse-nav pendingの既存契約を保持する。
- gamepad未配送edge・意味終端が複数logicで失われず、保持軸の復帰後継続を維持する。
- IME取消がproducerから実TextEditまで到達する。
- native IME中断要求が一度だけ正しいHWNDへ届き、出力上書き・旧hostからの漏れがない。
- 通常backend／nativeとも、生成・集約済みの正しいviewportのIME出力だけを配送する。
- release射影の順序保証と本家分類が分離され、連続クリックの操作差が採択されている。
- texture deltaのdrain／引継ぎ／明示終了・submission・freeが全経路で成立する。
- 汎用入力フィルタ、一律prune、復帰時一回捨て、旧クリックタイマーを追加していない。
- Closeを処理するための追加repaint、最小化子へのUI pass強制、新しいclose pending bool・復帰待ちqueue・時間窓を追加していない。
- IME中断要求を補修するApp側の補助フラグ、追加repaint、新しいqueue、retryを追加していない。
- Ctrl+Shift clipboardを通常／nativeで保持する。
- accessibility通過後、全snapshotを更新・通常比較・目視確認する。
- 非対話gate、確認用build、必要な承認付きdesktop試験、利用者実機確認が完了する。
- 未実施・環境不成立を確認済みと書かない。
- リリース済み機能・設定・データを黙って縮小・初期化しない。

更新対象はdisplay-pipeline、video-architecture、coding-conventions、keymap-spec、ime-process-keys-plan、ui-snapshot-policy、development-build-and-test、interactive-release-verification、detached-rework-plan §11、vendor台帳、backlog §1.382。

§1.380は個別解決の証拠があるものだけ更新する。採択した外観・操作差はマニュアルと製品ページへ反映し、版更新だけを理由に未確認の改善・互換性を告知しない。
