# §1.403 前面 UI と静止画 raw 入力の調査

> 番号: ブランチ `slideshow-1396` では当初 §1.402 として起票したが、master で別項目 (成功通知の簡潔化) が先に §1.402 を取ったため、master 取り込み時 (2026-10-19) に §1.403 へ振り直した。この日のコミットメッセージや `target/section402-*.log` の「1.402」はこの項目を指す。

2026-10-19。基点は `slideshow-1396` の `c2485f2f0`。
プロダクトコードの修正は行わない。追加するのは調査記録、headless 再現テスト、
`#[cfg(test)]` のモジュール登録だけ。実アプリ・OS のメニューは起動しない。

§1.396 の [監査表](section396-overflow-input-audit.md) の consumer を調べ直した。
以下は overflow を閉じた状態であり、`fb14a73a1` 相当の他 popup の入力関係を対象とする。
ソースで到達条件が揃うことと、実描画テストで状態変更を観測したことは区別する。
表中のソースパスは、特記しない限り `src/` からの相対パス。

## 1. 前面 UI の棚卸し

「前面」は見た目・操作対象の分類であり、必ず別の egui layer にあるという意味ではない。
`Order` は通常の fullscreen CentralPanel を基準に記載する。親 Ui の layer を変更した場合は継承する。
共通 modal は `App::common_modal_dialog_open`（`src/app.rs:22675`）に由来する。
これは canvas 全体を描画・実行しない仕組みではなく、consumer ごとの gate である。

| ID | 前面 UI / 描画元 | layer / Order | 開いている間に使われる既存 gate / 分類 |
| --- | --- | --- | --- |
| Bar | 上 HUD 本体（再生・回転等のボタン、評価等） | 親 Ui、通常 Background | `ui_fullscreen.rs:49288` の `draw_fs_hover_bar`。上バーは visible なとき secondary の chrome 除外 / touch 除外を持つ。固定時だけ画像枠を予約し、非固定時は画像と重なり得る。 |
| F | フィット一覧 `ui_fullscreen.rs:50463` | 親 Ui、通常 Background。`scope_builder`、独立 Area なし | `fit_popup_open`。下の K / W / S / T / C を参照。項目の Response と背景の paint は別。 |
| S | 表示モード・見開き / 連結読み一覧 `ui_fullscreen.rs:50119` | 同上 | `spread_popup_open`。先頭・末尾の単ページ配置もこの一覧の部品（`:50305`）で、独立 Popup ではない。指定倍率の入力欄 / Slider は F に含み、Response / keyboard focus も使う。 |
| R | 回転メニュー `ui_fullscreen.rs:49730` | Foreground Popup | `fs_rotation_popup_open`（`:7103`）。`os_theme.rs:223` の `Popup::menu`。 |
| P | 360 投影一覧 `ui_fullscreen.rs:49855` | 親 Ui、通常 Background。直接 painter / interact | `panorama_projection_popup_open`。active panorama 中だけ（`:49823`）。 |
| Q | 下部フィルムボタンのシーク設定 `ui_fullscreen.rs:7133` | Foreground Popup | `fs_still_seek_strip_popup_open`（`:7109`）。K / W / S / T / C に明示登録。 |
| O | 「…」全サブパネル `ui_fullscreen.rs:36064` | Foreground Area | §1.396 の `capture_fs_overflow_pointer_input`。今回の再現テストでは全 pass で Closed を検査する。 |
| N | fullscreen 右クリックメニュー：Windows native | egui layer なし | `ui_dialogs/context_menu.rs:2076` → `native_context_menu.rs:610` の `TrackPopupMenuEx`。OS の同期メニューループが入力を受ける。同時実行される egui Popup と区別する。 |
| X | 同メニューの egui fallback | Tooltip Window | `ui_dialogs/context_menu.rs:2108`。`fs_context_menu_idx` は S / T / C に登録。ナビゲータ / ワイプの gate には入らない。 |
| U | metadata タグ / Similar 項目の右クリックメニュー | Foreground Popup | `ui_metadata_panel.rs:3287,4014` の `Response::context_menu`。上記5種の popup flag、`fs_context_menu_idx` には登録されない。 |
| U | ComboBox、色 picker、TextEdit の右クリック menu | Foreground Popup | 例：`ui_adjustment_panel.rs:10903,9938`、`ui_crop.rs:368`、`ui_text.rs:61`、`ui_helpers.rs:1412`。一般には上記 flag に登録されず、Response / keyboard focus / 編集固有 gate に依存する。 |
| B | 左補正 / ブックマーク、右 metadata / Similar、分析右操作欄 | 親 Ui、通常 Background | `ui_adjustment_panel.rs:16322`、`ui_metadata_panel.rs:2046`、`ui_analysis_panel.rs:839`。別の上位 popup ではなく、canvas 内の consumer / 操作パネル。矩形除外と個々の Response を使う。 |
| Bt | fullscreen タグ picker | metadata と同じ親 Ui | `fullscreen_tag_picker_open` は `app.rs:22731` で common modal。独立 Window ではないため、他の D の「異なる layer による遮蔽」をそのまま当てはめない。 |
| V | 表示トリム `ui_view_trim.rs:1836` | Foreground Area | `view_trim_mode`。値操作は Response、raw down は保存終了の判定用。HUD / seek と併存する。 |
| E | 消しゴム / 隠蔽 / 補正レイヤー / 注釈 / 切り取り / SNS の自前パネル | Foreground Area | `ui_erase.rs:2414`、`ui_conceal.rs:2258`、`ui_adjustment_panel.rs:15445,15623`、`ui_text.rs:3072,3329`、`ui_crop.rs:287`、`ui_sns_split.rs:809`。各編集 owner の一部。各 canvas は自前 panel 矩形を除外する。 |
| D | 注釈の6種子 Window | Foreground | `ui_text.rs:3573,3683,3784,3979,4295,4884`。`text_subdialog_open`（`app.rs:22656`）と canvas 呼出 gate（`ui_fullscreen.rs:27357`）。 |
| D | 補正レイヤー追加 / マスク変更 / 効果選択 | Debug Window | `ui_adjustment_panel.rs:15012,15116,15214`。3種の bool と `common_modal_dialog_open`、canvas 内 `:12683` で停止。 |
| D | viewer 内へ明示描画する Window / Modal 群 | 下表 | fullscreen 末尾 `ui_fullscreen.rs:28499–28561`。mode を閉じずに描画されるものもあるため、common modal だけから raw 全体の安全を導かない。 |
| G | gamepad picker / favorite list | Foreground Area | `app/gamepad_input.rs:8176,8365`。gamepad の picker state。既知5種の popup flag には登録されない。 |
| H | touch 初回操作ガイド | 親 Ui、通常 Background、全画面 click-and-drag sink | `ui_fullscreen.rs:20266`。専用 help phase / touch command の消費。`common_modal_dialog_open` ではない。 |
| Pb | 360 品質 status の切替ボタン / 大画像確認 banner | 親 Ui、通常 Background | `ui_fullscreen.rs:46900,47137,47208,47303`。active panorama に属するボタン付きの面。`panorama_projection_popup_open` とは別で known popup gate に入らない。 |
| Za | 左下 AI status | Foreground Area | `ui_fullscreen.rs:51388`。表示専用だが Area の `interactable` は無効化していない（既定値は `vendor/egui/src/containers/area.rs:142`）。「表示専用だから hit-test も透過」とは扱わない。 |
| Zt | 保存先を表示できる feedback toast | Tooltip layer の child Ui、独立 Area なし | `ui_fullscreen.rs:51643,51732`。reveal path がある場合は `Sense::click` を持ち、保存ファイルを表示する。一般 popup flag はない。Area だけの棚卸しでは取りこぼす。 |
| Z | 通常文字 tooltip / reveal path のない toast | Tooltip / Tooltip child Ui | `ui_helpers.rs:1141`、`ui_fullscreen.rs:10560,51732`。通常の文字 tooltip は noninteractable（`vendor/egui/src/containers/tooltip.rs:158`）。意図して押下を通す表示と、上記ボタン付き表示を区別する。 |
| Z | ルーペ、ページ番号、シーク preview、境界 / 処理中表示、リング / gesture ガイド | 主に親 painter | `ui_fullscreen.rs:47802,23993,51829,51423,51475,51604`、`app/gamepad_input.rs:760,949,983`。独立した入力面ではなく、既存 canvas / recognizer の表示。 |
| B | ナビゲータ本体、seek 本体、touch side-panel handles | 親 Ui / painter | `ui_fullscreen.rs:37364,24214,20302,20346`。これら自身も consumer。音楽専用の Tooltip Area（`:18846,:20578`）を静止画の前面 UI と混同しない。 |

### viewer 内の Window / Modal の内訳

| UI | Order / 根拠 | App の gate |
| --- | --- | --- |
| VST manager / chain editor | Middle（既定）、`ui_dialogs/vst3_manager.rs:172` | `any_dialog_open` に含むが fullscreen 用 common modal からは意図的に除外（`app.rs:22853–22865`）。 |
| 編集追加パック取得 | Foreground、`ui_dialogs/editing_addon.rs:278` | `editing_addon_install_state`。 |
| 編集内容貼付確認 / 一括処理 | Middle、`edit_bundle_app.rs:378,402`、`edit_bundle_bulk.rs:1309,1346,1432,1461` | paste / apply / bulk pending。 |
| ショートカットヘルプ | Middle、`ui_dialogs/context_shortcuts.rs:442` | `show_context_shortcuts_help`。編集モード中の `?` からも開く。 |
| slot 保存 / favorite 標準解除確認 | Middle / Debug、`ui_adjustment_panel.rs:17283,16283` | slot / favorite confirmation。 |
| エクスポート / 進捗 | Middle、`ui_fullscreen.rs:54333,54605` | export dialog / pending。 |
| Remote session / sidecar restore | Foreground Modal、`remote_ipc/ui.rs:3877`、`ui_dialogs/sidecar_restore.rs:7` | session / restore owner。 |
| 本の構成エディタ / 別 owner 通知 | Foreground Window + Modal、`ui_composition_editor.rs:151,263`、`app/book_composition_editor.rs:841` | composition owner、viewer mutation modal。 |
| 画像フォルダ ZIP / 外部ツールの進捗・確認・選択 | Foreground Modal、`ui_image_folder_zip.rs:33,55`、`external_tool.rs:3163,3381,3459,3548` | operation owner / external-tool modal reason。 |
| mounted PDF password / EPUB conversion | Middle、`ui_dialogs/pdf_password.rs:22`、`ui_dialogs/epub_convert.rs:710` | document-open owner。専用 viewer で明示描画。 |

通常 ROOT にしか描かれない環境設定等を、同一 canvas 上の全 Window に含めない。
embedded fullscreen では main update の early return（`app.rs:95676`）より後の一般 dialog 群へ
到達せず、専用 viewer では ROOT と別 viewport になる。上表は viewer 側の明示描画を確認したもの。
OS ファイル選択等の native dialog も、別 HWND / OS modal loop として区別する。

## 2. consumer と既存 gate

| 記号 | consumer | 状態変更へ至る条件と gate の根拠 |
| --- | --- | --- |
| A | ナビゲータの予約 / 直接処理 / 右範囲ズーム | `ui_fullscreen.rs:37772` は common modal のみ。`:35611` は focus / mode / item、`:36740` 以降は保存済 raw snapshot の位置 / ボタン列。popup layer や既知 popup flag を見ない。 |
| B | 比較ワイプ | 呼出 `ui_fullscreen.rs:38618` は overflow 所有と `cursor_in_panel` のみ。`:35474` の raw primary と境界 hit で `CompareWipeInteraction` / fraction を変更する。 |
| C | 分析パン / ガイド / 色固定 | `ui_analysis_panel.rs:854–924` は overflow 所有と画像矩形のみ。別 popup / Window / common modal の gate はない。 |
| D | 通常パン / Ctrl 回転 / 連結スクロール / panorama drag | `ui_fullscreen.rs:18521` の `FsWheelFrameSemanticInput` は `fs_response` の drag 所有に依存。`:39227` の primary 抑止、編集 / 分析分岐を経て `:39263,:39280,:39315` へ進む。popup flag による一括拒否ではない。 |
| Ck | 通常ページクリック / double-click reset | `ui_fullscreen.rs:39450` 以降。context-menu と popup5種でページクリックを止める（`:39467,:39494`）。ただし transform reset は popup gate より前の `response.double_clicked`。 |
| S | 右クリック / 長押し / リング / gesture | `ui_fullscreen.rs:39650–39670,:39774`。known5種、context-menu、dialog、analysis gate。前面 UI の全種類を網羅する集合ではない。 |
| T | touch の tap / pan / pinch command | `ui_fullscreen.rs:6863,:38334`。known5種＋dialog / context-menu / IME / text focus / 編集 / wipe / trim / zoom owner。driver は terminal frame まで処理。未登録の Popup / Area 自体を全画面拒否する条件はない。 |
| Q | seek bar / strip / lock / toggle | `ui_fullscreen.rs:23507` で dialog / 6編集 / 分析を拒否。`:24521,:24800` の Response が操作を所有する。raw down だけでは新規 seek を開始しない。 |
| M | metadata hold preview | `ui_metadata_panel.rs:4765–4794,4982` は `Response::is_pointer_button_down_on` と pointer pos を要求。raw press の診断観測だけでは preview を開始しない。 |
| J | ブックマーク行 jump | `ui_adjustment_panel.rs:16127–16135` は `inner.response.contains_pointer`＋raw `primary_clicked`＋子部品非選択。`contains_pointer` は同 layer 内の別 widget への所有を保証しない（`vendor/egui/src/hit_test.rs:121–164`）。 |
| Md | 中ボタン短押し / drag zoom | `ui_fullscreen.rs:38632,:14660`、`app/gamepad_input.rs:5618`。開始除外は `cursor_in_panel`、popup5種の gate なし。§1.396 の左右所有の対象でもない。 |

known5種は F / S / R / P / Q。K（キー）の早期 return は modal と Q のみ
（`ui_fullscreen.rs:33120,:33129`）。W（wheel）は `fullscreen_front_popup_wheel_target`
（`:35959`）と `should_handle_fullscreen_wheel`（`:38737`）で別に配送する。
`ctx.egui_wants_keyboard_input()` は popup 全般の判定ではなく focus の有無
（`vendor/egui/src/context.rs:3913`）。wheel / keyboard の gate を raw button 所有の根拠に代用しない。

## 3. 到達表の読み方

以下の「可」は、必要な mode・画像 / ハンドル・前面 UI が同時に存在し、押下位置が重なるときに
ソース上で背面の操作状態へ到達することを指す。すべての組合せを実機で再現したという意味ではない。
「Response」は前面 widget が egui の押下 owner になれば背面の新規操作を開始しないこと、
「排他」は consumer の描画 / mode 条件を満たさないことを表す。
popup の項目選択に伴う意図した設定変更・mode 切替は背面漏れに数えない。

安定して表示された UI での新規押下と、旧 UI で開始した押下を保持した mode 遷移は区別する。
noninteractable と確認した文字 tooltip 等の純描画は意図して入力を通すため、以下の遮断対象から除く。
AI status の Area やクリック可能 toast はこの除外に含めない。

### 3.1 raw 操作への到達

セル末尾の A / B / C / J / Md は §2 の根拠を参照する。
「layer」は egui の異なる layer による遮蔽（`vendor/egui/src/hit_test.rs:121–164`）、
「mode」は下記の定常表示の排他条件による。`実再現` は §5 の失敗を観測済み。

| 前面 UI | ナビゲータ A | 比較ワイプ B | 分析 C | 編集 canvas | bookmark J | 中ボタン Md |
| --- | --- | --- | --- | --- | --- | --- |
| Bar 上 HUD のボタン | nav と重なれば gate なし A | 境界 / 画像と重なれば gate なし B | 画像と重なれば gate なし C | 定常非表示 E0 | HUD と行が重なれば同 layer J | panel 外なら gate なし Md |
| F フィット / S 表示一覧 | 重なれば可 A | 境界と重なれば可 B | 単ページなら可 C | 定常非表示、遷移 E0 | 同 layer で重なれば可 J | panel 外なら可 Md |
| R 回転 | 重なれば可 A | 境界と重なれば可 B | **実再現** | 定常非表示、遷移 E0 | layer で遮蔽 | panel 外なら可 Md |
| P 360 投影 | panorama navigator と重なれば可 A | mode 排他 P0 | mode 排他 P0 | mode 排他 P0 | mode 排他 P0 | panorama は状態変更せず消費 Md |
| Q シーク設定 | **実再現** | **実再現** | 非描画 Q0 | 非描画 / キー gate Q0 | layer で遮蔽 | panel 外なら可 Md |
| N native 右クリック menu | OS menu loop | OS menu loop | 通常入口は mode 排他 X0 | 通常入口は mode 排他 X0 | OS menu loop | OS menu loop |
| X fallback 右クリック menu | 重なれば可 A | 境界と重なれば可 B | 通常入口は mode 排他 X0 | 通常入口は mode 排他 X0 | layer で遮蔽 | panel 外なら可 Md |
| U パネル / 編集由来の context-menu・ComboBox・色 picker | 右 metadata 由来で重なれば可 A、左補正中は A の mode gate | 定常は元パネルと mode 排他 B0、遷移は別条件 | 定常は元パネルと mode 排他 B0 | 種別ごと E1–E5 | layer で遮蔽 | 自前 panel 外へはみ出せば可 Md |
| D 共通 modal Window / Modal | modal gate A | 共存する Window が境界に重なれば可 B | 共存する Window が画像に重なれば可 C | 種別ごと E1–E5 | layer で遮蔽 | panel 外なら gate なし Md |
| D VST manager（modeless） | common modal から除外、重なれば可 A | 共存時、境界に重なれば可 B | 共存時、画像に重なれば可 C | canvas が残る場合 E1–E5 | layer で遮蔽 | panel 外なら gate なし Md |
| B 左補正 / 右 metadata / 分析の操作欄 | 左補正は mode gate、右 metadata は nav と重なる場合 A | パネル矩形内は `cursor_in_panel` B | 分析欄は画像矩形外 C、通常左右パネルは mode 排他 B0 | 通常左右パネルは mode 排他 B0 | 自身の操作。上位面とは別分類 | パネル矩形内は除外 Md |
| Bt タグ picker | common modal A | パネル矩形内は B で除外 | metadata は分析と描画排他 B0 | 通常編集とは描画排他 B0 | 狭い viewport で左行と重なるなら同 layer J の候補 | パネル矩形内は除外 Md |
| E 編集の自前パネル | mode gate A | mode 排他 E0 | mode 排他 E0 | 自前矩形で開始除外 E1–E5。保持移動は別条件 | mode 排他 E0 | 自前 panel 矩形内は除外 Md |
| V 表示トリム Area | mode gate A | panel 矩形内は B で除外 | 分析描画との分岐 B0 | 他編集パネルとの描画分岐 | 左補正との描画分岐 B0 | panel 矩形内は除外 Md |
| G gamepad picker / favorite list | 共存して重なれば可 A | 共存して境界に重なれば可 B | 共存して画像に重なれば可 C | 対象 mode の canvas が残る場合 E1–E5 | layer で遮蔽 | panel 外なら gate なし Md |
| H touch 初回ガイド | nav と同時表示なら gate なし A | help を解除 H0 | help を解除 H0 | help を解除 H0 | 同時表示なら同 layer の J は拒否条件なし | panel 外なら gate なし Md |
| Pb 360 品質のボタン / banner | pano nav と重なれば gate なし A | mode 排他 P0 | mode 排他 P0 | mode 排他 P0 | mode 排他 P0 | panorama は消費のみ |
| Za AI status / Zt クリック可能 toast | 同時表示し nav と重なれば gate なし A | 同時表示し境界と重なれば gate なし B | 同時表示し画像と重なれば gate なし C | mode / 位置次第 E1–E5 | 異なる layer で遮蔽 | panel 外なら gate なし Md |

ここで「gate なし / 共存時」は **raw handler へ渡された後の判定**であり、任意の Window や
gamepad menu を任意の mode へ強制表示したという実験結果ではない。
実際に開ける owner / mode を持ち、表の幾何条件を満たす組合せだけが対象になる。
各 modal の開始受付が mode を閉じる場合はその組合せは成立しない。
少なくとも編集ヘルプは mode を保って開ける具体例がある（E1 / E2）。

### 3.2 Response / popup gate による配送

| 前面 UI | 通常パン等 D | ページクリック / reset Ck | 右クリック / ring / gesture S | seek Q | touch T | metadata hold M |
| --- | --- | --- | --- | --- | --- | --- |
| Bar 上 HUD のボタン | ボタンの Response | ボタンの Response。背景余白は所有次第 | visible な上 bar の chrome 除外 S | 通常は上下に分離、狭い viewport は Response | visible な top-bar 除外矩形 | ボタンの Response / 通常は上下に分離 |
| F / S 親 Ui 上の一覧 | 項目の Response は優先。背景余白は一括 gate なし D | ページ送りは popup gate。reset は Response 条件で先行 | known popup gate | 重なれば項目の Response。余白は一括 gate なし | known popup gate | 項目の Response が所有すれば開始しない。余白は所有次第 |
| R 回転 | 異なる layer / Response | 異なる layer / Response＋popup gate | known popup gate | 異なる layer / Response | known popup gate | 異なる layer / Response |
| P 360 投影 | 項目の Response は優先。余白で panorama drag が開始する条件は D | panorama mode で Ck 拒否 | known popup gate | 重なれば項目の Response。余白は所有次第 | known popup gate | mode 排他 P0 |
| Q シーク設定 | 異なる layer / Response | 異なる layer / Response＋popup gate | known popup gate | 異なる layer / Response | known popup gate | 異なる layer / Response |
| N native menu | OS menu loop | OS menu loop | OS menu loop | OS menu loop | OS menu loop | OS menu loop |
| X fallback menu | 異なる layer / Response | `fs_context_menu_idx`＋Response | `fs_context_menu_idx` | 異なる layer / Response | `fs_context_menu_idx` | 異なる layer / Response |
| U 未登録の Foreground popup | 異なる layer / Response | 異なる layer / Response。known popup 集合には未登録 | known 集合には未登録、raw S の候補（mode / dialog 条件による） | 異なる layer / Response | popup 自体の gate はない。focus / 自前 panel 除外 / mode 条件次第 | 異なる layer / Response |
| D 共通 modal | 異なる layer / Response | 異なる layer / Response | `any_dialog_open` | `fullscreen_seek_overlay_allowed` | `any_dialog_open` | 異なる layer / Response |
| D VST manager | 異なる layer / Response | 異なる layer / Response | `any_dialog_open` | `any_dialog_open` | `any_dialog_open` | 異なる layer / Response |
| B canvas 内の操作パネル | 部品の Response、連結読みは `cursor_in_panel`。通常パンは Response 依存 | パネル矩形、分析 / 編集 mode 条件 | side panel 全体の拒否はない S（下部 seek / 上部バーは chrome 除外） | bar と side panel は通常高さから分離 Q1 | side panel の除外矩形 | パネル自身の操作。部品の Response |
| Bt タグ picker | 部品の Response / パネル矩形 | 部品の Response / パネル矩形 | `any_dialog_open` | `any_dialog_open` で非描画 | `any_dialog_open` | パネル自身の操作。部品の Response |
| E 編集自前 panel | 編集 mode 分岐で通常パンを実行しない | 編集 mode gate | EditMode 固有の操作、通常 viewer short-click は対象外 X0 | 非描画 Q0 | overlay edit gate | 通常 metadata 非描画 B0 |
| V trim Area | 異なる layer / Response | 異なる layer / Response | known popup には未登録、raw S の候補 | 幾何非重複 Q1 / Response | `view_trim_mode` | 通常右パネルと併存しても Response が所有 |
| G gamepad Area | 異なる layer / Response | 異なる layer / Response | known popup には未登録、raw S の候補 | 異なる layer / Response | 一般 gate はなく focus / 既存除外次第 | 異なる layer / Response |
| H 初回ガイド | 全画面 Response sink | 全画面 Response sink | 専用 help は S の gate に未登録 | ガイドの全画面 Response sink | help phase がコマンドを消費 | 全画面 Response sink |
| Pb 360 品質のボタン / banner | ボタンの Response、余白は所有次第 D | panorama mode gate | known popup には未登録 S | 通常位置は上下に分離、余白 / 狭い viewport は所有次第 | popup gate / banner 固有除外なし。pano の pinch / pan が残る T | mode 排他 P0 |
| Za AI status | 別 Area / Response | 別 Area / Response | known popup には未登録 S | 別 Area / Response | Area 自体の一般 gate なし T | 別 Area / Response |
| Zt クリック可能 toast | toast の Response | toast の Response | known popup には未登録 S | 重なる場合 toast の Response | toast 固有の除外矩形なし T | toast の Response |

Response で開始を防ぐ行は、**表示済みの前面 widget が所有する新規押下**についての判定。
同 layer の描画だけの余白、初回 sizing、表示 / 閉鎖 pass、すでに開始した drag の移動・解放まで
一律安全とする根拠にはしない。未確認の境界を「再現済み」として扱わない。

### 3.3 mode・幾何条件の根拠

- **P0**：360 ON は分析・比較・補正・編集を OFF にする（`app.rs:91513–91542`）。
  描画側も panorama 中の分析 / 通常左右パネルを拒否（`ui_fullscreen.rs:26912,:27749`）。
  中ドラッグは panorama owner では状態を書き換えず入力だけ消費（`:14665`）。
- **Q0 / E0**：overlay 編集6種は `ui_conceal.rs:73–80`。上 HUD は
  `ui_fullscreen.rs:23592,:27848`、seek は `:23507,:24232` で非描画。
  分析中も seek 非描画。seek popup 中はキー handler が `:33129` で return する。
  ただし F / S / R の一般キー拒否はなく、旧 popup 上の押下＋編集キーが同じ pass に来ると、
  キー処理（`:26782`）の後で新しい編集 canvas（`:27326`）へ進み得る。
  入口は popup bool / memory をすべて clear するわけではない。これは定常共存とは別の保護対象。
- **B0**：分析 / trim / 左補正 / 通常 metadata は描画分岐で区別する
  （`ui_fullscreen.rs:27631–27775`）。比較ワイプ時は補正を閉じ（`:38147`）、
  通常 metadata も `!compare_wipe_active` 条件（`:27757`）。したがってパネル由来 popup と
  wipe / analysis を、定常状態で無条件に組み合わせない。
- **X0**：fullscreen menu の通常の右入力入口は analysis を拒否（`:39657`）。
  EditMode では passive secondary owner を作らず（`:470`）、短押し設定も適用しない
  （`ring_shortcut.rs:2172`）。パネル固有の `Response::context_menu` はこの menu とは別の U。
- **H0**：wipe / 編集等で touch を無効化した場合、または analysis / panorama で tap zone を
  無効化した場合は初回 help を消す（`ui_fullscreen.rs:38358–38366`）。
- **Q1**：bar / strip / side panel の高さは同じ `StillSeekGeometry` に由来する。
  side panel の下端は `seek_height` を引く（`ui_fullscreen.rs:7447–7484`）。
  固定 bar と画像が重ならない設定でも、floating popup の下端や大きい navigator まで
  常に非重複とはならない。再現テストでは実 layout の正の面積・位置の交差を assert する。
- ナビゲータ本体上の入力はその consumer の正常操作であり、後続 canvas 側には
  `navigator_consumed` の早期 return がある（`ui_fullscreen.rs:38259`）。
  side panel の hover / hit 除外にも navigator の矩形を使う（`:7507`）。
  「nav 自身に入力されること」と「nav を覆う別の前面 UI の押下が nav へ漏れること」は区別する。

## 4. 編集・capture の詳細

| ID | consumer | 前面 UI 上の新規押下 / 状態変化の判定と根拠 |
| --- | --- | --- |
| E1 | erase | overflow 所有と自前 panel を除く（`ui_erase.rs:1622,1675,1693`）。他の Window / popup を一般に除外しない。画像上に重なれば brush / 選択等の候補。Brush は raw down から開始（`:1783–1796`）し、パネル開始の保持移動も別の漏れ候補。 |
| E2 | conceal | E1 と同じ（`ui_conceal.rs:1326,1366,1386,1672–1686`）。erase / conceal の `?` でヘルプを開く入口（`ui_erase.rs:710` / `ui_conceal.rs:636`）と viewer 内の同 Window 描画（`ui_fullscreen.rs:28528`）がある。共通 modal でもこの raw canvas は拒否しない。 |
| E3 | local adjust | 子 Window 3種は bool で停止（`ui_adjustment_panel.rs:12683–12691`）。それ以外は自前2 panel だけ除外（`:12907`）、raw press は `:13014` 以降。`local_adjust_effect_ui.rs:21,401` の ComboBox / 色 picker が両 panel 外の画像 / ハンドルへはみ出す場合は候補。 |
| E4 | text | `ui_text.rs:65` の Middle / Foreground layer 判定で新規選択 / drag を拒否（`:2453,:2493`）。6種 subdialog 中は呼出し自体を止める（`ui_fullscreen.rs:27357`）。Debug layer は helper の対象外。拒否前に `text_smart_guides` を clear（`ui_text.rs:2491`）するので補助状態を含む完全不変とは書かない。 |
| E5 | crop / SNS split | 呼出側の `!any_dialog_open` と自前 panel を除外（`ui_fullscreen.rs:27426,:27392`）。非modal popup のはみ出しは別。crop の ComboBox（`ui_crop.rs:368`）が画像 / 32pt ハンドルへ重なれば raw create / edit drag の候補（`:629–700`）。SNS も raw press で `sns_split_drag`（`ui_sns_split.rs:1177,1200,1239`）。 |
| V | view trim | 値変更は Slider 等の Response（`ui_view_trim.rs:813–845,1409`）。raw primary down（`:1312,1810`）は保存 batch の終了用で、新しい canvas 操作を始めない。 |
| Cap | 範囲 capture | `ui_fullscreen.rs:52740` の Response の返値を操作許可に使わず、`:52766` から raw event を処理。metadata panel 内は取消、それ以外は target / start / current を設定（`:52800`）。single は画像外も端へ clamp（`:52645`）。上 HUD は capture 中非表示だが seek の許可に capture 除外はない（`:23507,:23594`）。従って Q や同 viewport の Window と実際に共存し選択可能な位置なら新規開始 / 取消候補。今回の実描画テスト対象外。 |

編集系・capture・未登録 popup の追加候補はソース調査であり、§5 の3件と同じ確度の
実描画再現結果ではない。初回 hit 情報や popup が親 panel からはみ出す位置の実測は今後の検証対象。

## 5. 実描画再現テストと利用者向け症状

新規 `src/ui_fullscreen/front_popup_input_tests.rs`。
全件 `render_fullscreen_viewport` を headless 実行し、実ページ layout が非空、
実 Popup Area が押下位置の最前面、実 consumer の画像 / 境界と popup 項目が重なることを検査する。
popup の項目選択で設定値が更新され、popup が閉じる対照操作も、不変条件の assert より前に行う。
単に flag を立てて raw handler を直接呼ぶテストではない。

| テスト | 確認した操作と症状 | 現状での失敗（再現） |
| --- | --- | --- |
| `front_popup_seek_row_does_not_pan_the_actual_top_right_navigator` | 1093×614、縦長画像を拡大、右上ナビゲータを大きく表示 → 下部フィルムからシーク設定 → ナビゲータと重なる項目上で左ドラッグすると、背面の画像表示位置まで動く。 | `fs_pan` が `[0,0]` → `[0,79.5]`。設定の「小」行の選択・popup 終了も通過。 |
| `front_popup_seek_row_does_not_move_the_actual_comparison_wipe` | 1093×614、横長画像の比較ワイプ境界を右へ移す → シーク設定を開く → 境界と重なる項目上で横ドラッグすると、背面の比較境界も動く。 | fraction が `0.72584295` → `0.7653166`。比較表示中の設定行選択対照も通過。 |
| `front_popup_rotation_row_does_not_pan_the_actual_analysis_canvas` | 480×614、分析表示で上部の回転メニューを開く → 画像と重なる項目上で縦ドラッグすると、背面の分析画像も動く。 | `analysis_pan` が `[0,0]` → `[0,25]`。「180°」項目の回転設定・popup 終了も通過。 |

正常な挙動（背面状態不変）を assert した ignore なしの初回実行で **3件とも失敗**し、
実際の不変条件違反を確認した。失敗箇所は layout / 項目選択等の fixture 前提ではない。
再現しなかったケースはない。その後、3件に指定の属性を付けて残した。

```rust
#[ignore = "backlog 1.403: 前面 popup の押下が背面へ届く (再現テスト)"]
```

これらは既知問題を記録する失敗テストであり、今回修正したことを示すものではない。
修正時は ignore を外し、同じ不変条件で成功させる。

## 6. 修正する場合の単位についての所見（設計未決定）

1. **物理的な押下列の開始時に、一度だけ前面の操作面を決める単位**が適している。
   ナビゲータは早期処理・予約・保存済 snapshot を持ち、後段だけの popup gate 追加では遅い。
   押下後に矩形外へ動く / popup を閉じる / 編集キーで mode を変える場合も、同じ列の終端まで
   owner を参照する必要がある。新規開始を止めることと、既存 drag の終端を駆動することを分ける。
2. §1.396 の `FsOverflowPointerFrameInput` と同じ **pass で確定した結果を全 consumer へ配る形**は
   再利用候補。ただし単なる「最前面の Foreground Area を除く」への置換では足りない。
   F / S / P は親 Ui と同 layer であり、描画矩形・interactive surface の明示情報が必要になる。
   Area を持たないクリック可能 toast も明示情報の対象になる。
   noninteractable な文字 tooltip、表示だけの toast / loupe は入力面と区別する。
3. known popup bool を nav / wipe / analysis へ個別に足す案は、3再現経路の新規開始だけなら
   対応できるが、未登録の context-menu / ComboBox / 色 picker / Window と編集遷移は残る。
   全面拒否では popup 外から始まった閲覧操作も変わる。既存の wheel / keyboard gate は
   意図的に別契約なので、そのまま pointer の共通 owner に代用しない。
4. 共通化する対象（左右 / middle / touch、popup の外側を通すか、modal の扱い、same-layer の余白、
   first / sizing / close pass）、前面を手放す境界、複数 viewer の owner 寿命は設計担当の判断事項。
   今回は所有型・popup gate・detached / viewport 経路を一切変更しない。

## 7. 検証記録と引き継ぎ

すべて `MSBUILDDISABLENODEREUSE=1` / `CARGO_BUILD_JOBS=16`。

| 実行 | 結果 / ログ |
| --- | --- |
| `cargo nextest run --no-fail-fast -p mimageviewer --lib -E 'test(front_popup_input_tests)'`（ignore 付与前） | **3失敗 = 3再現**。`target/section402-reproduction-active.log`。§5 の背面不変 assert で失敗し、項目選択対照は通過。 |
| `cargo nextest run --no-fail-fast -p mimageviewer --lib -E 'test(/front_popup_input_tests\|overflow_input_tests/)'`（ignore 付与後） | **35成功 / 0失敗**。新規3件は通常選択から除外。`target/section402-normal-nextest.log`。 |
| `cargo test -p mimageviewer --lib front_popup_input_tests` | **0失敗 / 3 ignored**。3件それぞれの指定理由も表示。`target/section402-normal-ignored.log`。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功。`target/section402-check.log`。 |
| `cargo test --test ui_snapshot` | **186成功 / 0失敗**、PNG 変更なし。`target/section402-ui-snapshot.log`。 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 成功。 |
| `python scripts/check_ui_glyphs.py` | 危険グリフ 0 件。 |

将来、修正前の再現だけを再実行する場合は次を使う（現状の3失敗を期待する）。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run --no-fail-fast -p mimageviewer --lib --run-ignored only -E 'test(front_popup_input_tests)'
```

本件は調査・テストのみ。確認用 build-dev と実アプリ起動は行わない。コミットは ClaudeCode が担当する。
`src/ui_fullscreen.rs` の差分は新規 test module の `#[cfg(test)]` 登録2行のみ。
他の変更ファイルは新規 test、調査記録、README の索引、backlog の調査状態である。



## 8. 修正の設計案（ClaudeCode、2026-10-19 改訂 2、独立レビュー合意済み・利用者決定済み）

**利用者決定 (2026-10-19)**: D1 改訂 2 で進める。メニューを開いている間の最初の押下が閉じるだけになる変化は了承。
着手は §1.396 の master 取り込み後。

初版 (3238eaede) を独立レビュー (Sol / xhigh、SID 01a151e2-ed45) の P2 4 件・P3 1 件で改訂した。レビューは
「利用者が外側操作の変更を承認するなら D1 を支持」、着手時期 (§1.396 の master 取り込み後) にも同意。

### 8.1 推奨案 D1: メニューを提示している間に始まった押下は、メニューを閉じるだけ

**規則**: 前面メニューを **提示している** 状態で始まった物理的な押下列 (左・右・中・touch) は、終端まで
「前面の所有」とし、背面の raw consumer (§2 の A〜Md、編集 canvas、範囲 capture) へ渡さない。
押下がメニューの外なら、mIV が描くメニュー (F / S / P / Q / R / X) はその押下の時点で閉じ、パネル内の egui popup (ComboBox 等) は
外側のクリックで閉じる (2 番)。どちらもその押下は背面へ渡らない。メニューの項目は今どおり Response で操作する。
「…」パネルは §1.396 の規則 (パネル上だけ所有、外側は通す) のまま。wheel / keyboard は既存の別契約のまま
(ただしキーによるページ移動は下の寿命規則の対象)。

1. **「提示している」の述語 (レビュー P2-1)**: 開状態の bool ではなく、そのメニューをこの pass で描く条件と
   **同じ述語**を使う。例: フィット一覧は `fit_popup_open` かつ上部バーを描ける (`fullscreen_top_bar_chrome_allowed`
   相当) とき。`E` で消しゴムへ入ると上部バーは描かれないので、残った bool で編集 canvas の押下を止めない。
   述語は描画側と 1 か所で共有し、pass 先頭 (ナビゲータの予約より前) で 1 回評価する。対象は
   (1) 上部 / シークの F / S / R / P / Q、(2) fullscreen 右クリックメニュー、(3) egui の memory 管理 Popup
   (`Popup::is_any_open`。パネル内の context menu・ComboBox・色 picker)。レビューで (3) が現行の U を拾えること、
   通常 tooltip 等を誤って拾わないこと、egui の外側 close より前に評価できることを確認済み。
2. **外側の押下で閉じる (レビュー P2-2、改訂 2)**: 所有は pass 先頭で一括確定し (背面への配送はこの結果だけで決める)、
   閉じる処理は **各メニューの既存の描画境界**に共通の方針として置く。中央で「内側か」を判定し直さない。
   - mIV が描くメニュー (F フィット・S 表示モード・P 360 投影・Q シーク設定・R 回転・X 右クリックの egui fallback) は、
     描画境界で「外側で押された」ときに閉じる。**内側には起動ボタンと、開いている子メニューを含める** (起動ボタンの押下で
     閉じて、離したときのトグルで開き直すことを防ぐ)。S / P はすでに press 基準 (`ui_fullscreen.rs:49931,:50390` 付近の
     `any_pressed`) なので変えない。click 基準の F と、egui Popup を使う Q / R / X を press 基準へ揃える。
   - パネル内の ComboBox・色 picker・context menu など、egui が開閉を持つ memory popup は **egui 既定の click 基準のまま**
     にする。外側でドラッグしても閉じないが、D1 の所有によりその押下は背面へ渡らず、次のクリックで閉じる。
     起動ボタンや子メニューの扱いを egui 側に任せられ、`Popup::close_all` (他の viewport の popup も閉じる) を使わずに済む。
   - 閉じる対象は、その押下を受けた viewer の viewport のメニューだけ。他の窓のメニューは閉じない。
   これで mIV のメニューは「外側の最初の押下 (クリック・ドラッグとも) で閉じる」、egui のパネル内 popup は「外側のクリックで
   閉じる」になり、どちらもその押下は背面へ渡らない。
3. **所有の寿命 (レビュー P2-3)**: 前面が所有した物理列は、**同じ viewer 内のページ移動では終わらない**。
   今の `fs_primary_suppression` は `open_fullscreen` のページ移動でリセットされ、`FullscreenSecondaryPress` も
   ページ identity の変更で終わるため、押したまま矢印キーで移ると保持中の入力が背面パンへ戻り得る。
   前面の物理列の所有は、ページ操作の取消とは別の値として持ち、**物理的な release、前面の handoff / close、
   focus loss** でだけ終える。touch は最初の指ではなく接点群の最後の End / Cancel を終端とする。
   §1.396 の `OverflowStream` も同じ寿命の問題を持つ (「…」上で押したまま矢印キー) ので、この値へ移す。
   置き場所は §1.396 と同じく前面 viewer の一時状態 (App 全体、既存の前面境界で解放) とし、bundle へは移さない。
4. **閉じるクリックを double-click の 1 発目にしない (レビュー P2-4)**: egui は click count に前の click を数えるので、
   外側クリックでメニューを閉じた直後に同じ場所をもう一度押すと transform reset が成立する。背面の double-click は
   **背面が受理した click の対**だけで成立させ、前面が所有した click との境界を記録する (独自タイマーは足さず、
   egui の click count を境界以降だけで読む)。
5. **対象と範囲外を分ける (レビュー P3)**: D1 が閉じるのは上の (1)〜(3) のメニュー。次は D1 では閉じず、
   **既存 gate で安全とも扱わない未解決**として別項目に分ける: 共通 modal の Window (例: 消しゴム中の `?` ヘルプ上の押下が
   背面の筆へ届く経路が残る)、意図して modeless の VST manager、gamepad picker、上部バー / 360 品質ボタンの余白
   (Bar / Pb)、AI status / クリック可能 toast。D1 の受入テストは対象メニューの組み合わせに限る。

**状態の組み合わせを減らす理由**: 「メニューの上か外か」の判定をメニューごとに増やさず、「提示中のメニューがあるか」を
描画と共有する 1 つの述語で決める。押下列の所有も overflow と前面メニューで 1 つの値・1 つの確定結果にまとめ、
寿命の終わりを物理的な終端と前面の境界だけにする。

**利用者から見た変化 (2026-10-19 利用者了承)**: 今は、フィット・表示モード・360 投影などのメニューを開いたまま
メニューの外の画像をドラッグすると、パンなどが効く経路がある。D1 では、メニューを開いている間の最初の押下 (クリック・
ドラッグとも) はメニューを閉じるだけになる (パネル内のドロップダウン等は外側のクリックで閉じる)。一般的なメニューと同じ動きで、ページ送りのクリックは今もそうなっている。

### 8.2 代案 D2: メニューの上で始まった押下だけを止める

§1.396 と同じく押下位置の最前面で所有を決め、メニュー外の操作は通す。今の挙動は変えないが、同じ layer に描く
F / S / P や Area を持たない toast は layer 判定で拾えず、各メニューが操作面の矩形を毎フレーム登録する仕組みが要る。
8.1 の 3・4 (寿命と double-click) は D2 でも同じく必要。

### 8.3 着手時期と検証

§1.396 (ブランチ `slideshow-1396`) を master へ取り込んでから着手する (同じ所有の仕組み・consumer を触る)。
§5 の 3 件の `#[ignore]` を外して成功させ、8.1 の 1〜4 それぞれに実描画テストを足す (非提示の bool で編集 canvas を
止めない / F・S・P・Q・R・X は外側ドラッグ 1 回で閉じ、パネル内 popup は外側ドラッグで開いたまま背面不変・その後の外側クリックで閉じる / 押したままキーでページ移動しても背面へ戻らない / 閉じるクリック直後の
同位置クリックで reset しない)。「…」の `OverflowStream` を新しい寿命へ移したことの回帰も確認する。

## 9. 実装記録

2026-10-19、`front-popup-1403`。設計・検収は ClaudeCode、実装・関連自動検証は Codex。
§1〜§7 は修正前の調査・再現記録として残す。本節が実装後の記録である。

### 9.1 変更点

- `FsFrontPointerOwnership` を App 全体の前面 viewer 一時状態として追加。旧
  `FullscreenPrimarySuppression::OverflowStream` / `FullscreenSecondaryPress::Overflow` を除去し、
  ページ操作の取消と物理入力列の寿命を分けた。bundle へ移さず、同 viewer のページ移動で保持する。
- `capture_fs_front_pointer_input` がキー・root overflow 描画・navigator 予約より前に一度だけ
  `FsFrontPointerFrameInput` を確定する。overflow は最前面の現在矩形上で左・右・touch だけ、
  提示中のメニューは位置によらず左・右・中・touch を所有する。touch は最後の End / Cancel で終える。
  同 pass の release → 新 press は新列として判定し、他 viewport の非フォーカスでは解放しない。
- 上部メニューは描画と `FsTopMenuEligibility` / `fullscreen_top_bar_chrome_allowed` を共有。
  Q は `fullscreen_seek_menu_eligible`、X は `fs_context_menu_presented` を共有する。
  egui の `Memory::any_popup_open` / `close_popup` は `viewport_id` をキーとしていることを vendored source で確認した。
- 既存監査表の全 consumer に同じ結果を配り、中ボタン短押し / drag zoom と範囲 capture も接続。
  隠蔽のパイプライン前 hold は前 pass の同じ物理 owner を viewport 一致で読む。
  memory popup と共存する切り取り canvas の raw 入力も同じ結果へ接続した。
  同 pass に旧所有列の release と新しい背面 press がある場合は、確定結果の release mask に旧終端を残し、
  新しい down / press と分ける。navigator の ordered event、編集の raw release、中ボタンの完了、
  背面 Response の click / drag-stop に旧終端を再利用しない。
- F / S / P / Q / R / X は各描画境界で外側 press close。起動ボタンと開いた子メニューを除外する。
  egui memory popup は既定 click close を維持し、`Popup::close_all` は追加していない。native menu loop は変更しない。
- `FsBackingClickBoundary` は連続して背面が受理した primary click 数（3で飽和）を記録し、
  egui の `released_buttons_with_click_counts` が返す count と小さい方を読む。2のときだけ double-click とする。
  前面所有開始・未受理 click は境界を切る。閉じる click を含む egui の3打目も、背面での2打目なら reset できる。
  通常の背面3打目では重ねて reset しない。独自タイマーはない。
- 共通 modal Window、VST manager、gamepad picker、Bar / Pb の余白、AI status / toast は対象外のまま。

### 9.2 触れた前面境界

以下は既存の前面切替・リセット境界で、同種の一時入力状態も解放する追加のみ。
detached の述語・viewport routing・geometry・focus 判定・lifecycle は変更しない。
初回の4関数は§1.396と同じ構造的修正として独立レビューでも合意済み。追加5関数も同じ規則に従い、
ClaudeCodeの記録・レビューへ渡す。

- `App::reset_detached_pause_foreground_modes`
- `App::reset_active_detached_viewport_runtime_for_new_window`
- `App::adopt_active_detached_viewport_runtime_from_passive`
- `App::close_fullscreen_now_with_metadata_boundary`

独立レビュー後の修正で追加した境界（同じ終了規則、詳細は9.5）:

- `App::toggle_still_window_mode`（F11）
- `App::apply_egui_viewer_presentation_for_current_item`（F12等）
- `App::prepare_viewer_presentation_open`（解決した表示先が変わる場合のみ）
- `App::apply_video_presentation_switched`（native presentation確定、表示先が変わる場合のみ）
- `App::handoff_active_detached_viewport_to_passive`（live mediaを含む前面引渡し）

### 9.3 テストと検証

`front_popup_input_tests.rs` の既存3件の ignore を解除した。追加テスト:

- `front_popup_all_six_menus_close_on_outside_press_and_own_the_entire_drag`（6種 × 左右中）
- `front_popup_launch_button_closes_once_without_reopening_on_release`
- `front_popup_nonpresented_toolbar_bools_do_not_block_the_real_erase_brush`
- `front_popup_and_overflow_hold_survive_real_arrow_page_navigation`
- `front_popup_dismissal_click_cannot_be_the_first_click_of_a_canvas_reset`
- `front_popup_memory_combo_preserves_outside_drag_then_closes_on_outside_click`
- `front_popup_memory_close_is_local_to_the_rendered_viewport`
- `front_popup_actual_seek_press_close_keeps_the_other_viewports_menu_open`
- `front_popup_touch_ownership_lasts_until_the_last_contact_end_or_cancel`
- `front_popup_touch_cancel_then_new_contact_in_one_pass_starts_a_new_canvas_stream`
- `front_popup_actual_context_submenu_child_press_keeps_its_parent_open`
- `front_popup_actual_fit_unit_combo_child_press_keeps_its_parent_open`
- `front_popup_owned_release_and_new_canvas_press_in_one_pass_keep_their_own_edges`

X の子メニューは親 Window 内のインライン展開なので、実 min rect へ含まれる。F の単位 ComboBox は
実描画から返した開いている子 popup rect を外側 close 判定へ渡す。中央の所有判定で内側を再判定しない。

`front_pointer_ownership_tests.rs` には他 viewport の非フォーカス、同一 pass の旧 release → 新 press、
旧 touch Cancel → 新 contact group の回帰を追加。
名前は `front_popup_owner_ignores_another_viewports_focus_loss`、
`front_popup_old_release_and_fresh_press_in_one_pass_have_distinct_owners`、
`front_popup_old_touch_terminal_and_new_group_in_one_pass_are_distinct`。
既存 overflow テストは旧 variant の直接検査を新 owner の検査へ移し、上位 Q の押下も前面所有する仕様に更新した。
項目の選択・外側開始の通常動作を維持する対照は残す。

すべて `MSBUILDDISABLENODEREUSE=1`。実アプリ起動やデスクトップ入力は行わない。

| 検証 | 結果 / ログ |
| --- | --- |
| 関連 nextest（下記 filter） | **619成功 / 0失敗**。`target/section403-related-final.log`。実描画16件・所有境界3件、既存 overflow35件、スライドショー・navigator・比較・分析・編集系を含む。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功。`target/section403-check-final.log`。 |
| `cargo test --test ui_snapshot` | **186成功 / 0失敗**。PNG変更なし。`target/section403-ui-snapshot.log`。 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 成功。 |
| `python scripts/check_ui_glyphs.py` | 危険グリフ0件。 |

関連 nextest の再実行コマンド:

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run -p mimageviewer --lib -E 'test(/front_popup|overflow|slideshow_|fullscreen_secondary|fullscreen_primary|touch_panel|foreground_reclaim|wheel_frame|navigator|compare|analysis|page_edit_release|export_crop|sns_split|erase|conceal|text_canvas|local_adjust_canvas/)'
```

対象範囲の実装上の未解決事項はない。範囲外の入力問題は §8.1 の5に残す。
lib 全件、build-dev、独立完了レビュー、コミットは ClaudeCode 担当。実機確認は未実施。

### 9.4 変異確認

`target/section403-mutation-final.log`。次の2変異を同一ビルドへ入れ、対応する2テストを実行して
**0成功 / 2失敗**を確認した。寿命テストの保持中にはクリック終端がなく、double-click テストにはページ移動がないので、
それぞれ別の不変条件違反を検出している。

| 変異 | 検出したテスト / 失敗 |
| --- | --- |
| `open_fullscreen_with_metadata_boundary` で `fs_front_pointer` もリセットする | `front_popup_and_overflow_hold_survive_real_arrow_page_navigation`。`page navigation must preserve the Primary physical owner` で失敗。 |
| count=2を前面境界で制限せず採用する（正常な境界後のtriple→double対照は維持） | `front_popup_dismissal_click_cannot_be_the_first_click_of_a_canvas_reset`。閉じた直後のクリックで zoom / pan が `(2.0, [30,15])` から `(1.0, [0,0])` へ変わり、不変 assert で失敗。 |

初回の double-click 変異は、メニュー起動クリックが egui の triple-click count に残るためテストを通過した。
起動クリックの時間窓が残る / 切れた両方の実描画ケースを加え、実際の multi-click count を assert してから、
閉じた直後の最初の背面 click は reset せず、その次の背面 click で正しく reset する対照まで強化した。
上表は強化後の再確認である。

変異した2ファイルは SHA-256 一致で復元を確認した。復元後の関連一式は619件すべて成功した。


### 9.5 独立レビューの P2 4件への対応（2026-10-19）

初回実装 `8051885d8` の lib 全件13,744件成功は ClaudeCode から受領した検証記録。
以下はその後の差分であり、上記9.3の初回検証とは区別する。

1. **表示先変更後の owner**: 入力のない pass でも保持する viewport identity はそのままにし、
   F11 / F12 / 表示先open解決 / native presentation確定 / active→passive引渡しの実境界で
   旧 `FsFrontPointerOwnership` を終了する。同じ表示先の再確認や同 viewer のページ移動では終了しない。
   無関係な viewport の描画・非フォーカスでの解除は追加していない。触れた前面境界は9.2の全9関数。
2. **ナビゲータの double-click**: flat / panorama とも `accept_navigator_click` を通し、
   前面境界以降の背面受理 click 数と egui の adjudicated count の小さい方を読む。
   一つの pass に複数の release がある場合も受理順に進める。canvas と同じ境界であり、独自タイマーはない。
   `src` 全体を `double_clicked` / `click_count` / count=2 の直接参照で検索し、fullscreen背面の直接参照は
   この2経路だった。main windowのresizeとnative media overlayの音量・速度は今回の背面canvasの対象外。
3. **同じ pass の複数物理列**: 確定結果を共有する `Arc` に、元のeventと各eventの所属を記録する。
   ナビゲータのprojectionはその所属でボタンedgeを除く一方、位置は後続wheelや非所有ボタンのため残す。
   範囲captureも同じ確定event列から所有event/moveを除いて読む。最終down/pressとrelease maskは別に維持し、
   後半の背面pressから前半の所属を再計算しない。menu提示述語はpass入口で一度評価するので、
   同pass内の後続pressもmenu所有。overflow単独の後続pressは、その押下時に確定した位置所属に従う。
4. **overflowの中ボタン**: 位置による所有は§1.396の左右/touchに戻した。
   中ボタンは提示中の前面menuだけが所有する。overflow上の短クリック割り当てとdrag zoomは維持する。

追加した実描画テスト（`src/ui_fullscreen/front_popup_input_tests.rs`）:

- `front_popup_actual_f11_round_trip_retires_the_old_viewport_pointer_owner`
  （実F11配送、fullscreen→ROOT→fullscreen、旧列を離さず移譲、新表示先のdrag）
- `front_popup_navigator_double_click_counts_only_backing_clicks_for_flat_and_panorama`
  （閉じるclick→背面1打目は不変、背面2打目は中央移動、flat/panorama両方。
  起動clickの時間窓が残る/切れた4ケースで、後者は背面1打目のegui生count=2も検査）
- `front_popup_and_overflow_same_pass_complete_owned_stream_cannot_leak_before_a_fresh_press`
  （owned press→move→release→新press、menu/overflow両方、後続背面dragの対照）
- `front_popup_overflow_middle_short_click_and_drag_zoom_preserve_released_canvas_behavior`
  （overflow閉/開の対照、中短クリックの回転割り当て、中drag zoom）

追加した境界テスト（`src/app/tests.rs`）:

- `front_popup_f12_round_trip_and_passive_handoff_retire_the_foreground_owner`
- `front_popup_presentation_open_keeps_page_owner_until_the_display_target_changes`

既存 `front_popup_and_overflow_hold_survive_real_arrow_page_navigation` はmenuの左右中、overflowの左右を検査する。
overflowの中まで所有する誤った期待だけを除き、menuの中ボタンとtouch、他viewportの既存回帰は維持した。

修正後の検証（すべて `MSBUILDDISABLENODEREUSE=1`、実アプリ起動なし）:

| 検証 | 結果 / ログ |
| --- | --- |
| 関連 nextest | **635成功 / 0失敗**。`target/section403-p2-related-final.log`。9.3のfilterに、F11 / F12 round trip / native presentation確定 / detached pauseの既存回帰を追加した。新規6件、実描画20件・所有境界3件を含む。 |
| `cargo test --test ui_snapshot` | **186成功 / 0失敗**、PNG変更なし。`target/section403-p2-ui-snapshot.log`。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功。`target/section403-p2-check-final.log`。 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 成功。`target/section403-p2-fmt.log`。 |
| `python scripts/check_ui_glyphs.py` | 危険グリフ0件。`target/section403-p2-glyphs.log`。 |

最終nextestコマンド:

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run -p mimageviewer --lib -E 'test(/front_popup|overflow|slideshow_|fullscreen_secondary|fullscreen_primary|touch_panel|foreground_reclaim|wheel_frame|navigator|compare|analysis|page_edit_release|export_crop|sns_split|erase|conceal|text_canvas|local_adjust_canvas|still_image_root_f11|still_image_window_mode|f12_round_trip|apply_video_presentation_switched|video_presentation_switch_updates|detached_pause/)'
```

変異確認:

| 変異 | 対応するテストの検出結果 |
| --- | --- |
| F11の旧owner終了を除く | F11実経路テストが `F11 must retire the old viewport owner` で失敗。 |
| `open_fullscreen_with_metadata_boundary` で物理ownerもリセットする | 押下保持→ArrowRightテストが `page navigation must preserve the Primary physical owner` で失敗。 |
| ナビゲータprojectionでeventごとの所属を無視する | 同pass複数列テストが、前面motionで背面pan `[0,0]` → `[0,-458.9]` となり失敗。 |
| overflow位置所有を中ボタンにも広げる | 中ボタン対照テストが `overflow must not capture Middle` で失敗。 |
| ナビゲータで背面受理数の制限を外しegui生countを読む | 単独変異で生count=2を確認した背面1打目が中央移動し、pan `[0,0]` → `[313.4,-205.7]` となり失敗。 |

最初の5変異同時確認は **0成功 / 5失敗**（`target/section403-p2-mutation.log`）。
この時ナビゲータはtripleを境界後のdoubleとして読めない対照側で落ちたため、起動clickを失効させるケースを追加し、
ナビゲータの単独変異を再確認した。上表最後の誤成立を検出し **0成功 / 1失敗**
（`target/section403-p2-navigator-mutation.log`）。両回とも変異3ファイルのSHA-256一致で復元し、
復元後の最終ソースで上記635件とsnapshotを成功させた。

実装上の未解決事項はない。修正差分のlib全件・build-dev・独立再レビュー・検収・コミット、
detached計画§11への前面境界記録はClaudeCodeへ引き継ぐ。実機確認は未実施。

### 9.6 範囲キャプチャの focus loss と fixture の修正（2026-10-19）

`4960cda74` の再レビューで前回4件の解消と前面境界9関数の構造的修正が合意された。
その合意の記録は `916fddeb5`（detached計画§11）。一方、ClaudeCodeのlib全件で範囲captureの6件が失敗し、
focus lossの早期returnとdefault確定結果を渡すfixtureの2件のP2を受領した。
9.5までの関連filterに `capture_region` がなく、これらを検証から漏らしていた。以下はその修正記録。

- `FsFrontPointerOwnership::capture` は、最終focus levelがfalseでも確定イベント列を返す。
  `WindowFocused(false)` の位置で永続ownerを終了し、先行releaseの所属・イベント順序を保持する。
  focusを失っている間の新規pressは所有せず、同passの再focus後の新しい列はそのviewportに結び直す。
  focus levelだけが通知された場合も、イベント列を保持したままpass末尾で永続ownerを終了する。
- `run_capture_region_input_frame` と toolbar armテストは、描画前に `capture_fs_front_pointer_input` を通す。
  consumerは確定列だけを読み、raw入力へのfallbackは追加していない。
- `FsFrontPointerFrameInput::default()` と型の構築箇所を `src` 全体で検索した。
  追加で見つかったシークのtest-onlyラッパー `draw_fullscreen_seek_overlay` も実際の確定処理へ接続した。
  残るdefault returnは別viewportを描くときの既存の非所有結果だけで、test consumerへのdefault注入はない。
- 今回、9.2の前面境界9関数、detachedの述語・viewport routing・geometry・focus判定・lifecycleは変更しない。
  変更した本体関数は `FsFrontPointerOwnership::capture`。他はテストと文書の更新。

既存6件はfixtureを本番の入力契約へ合わせて検証する。

- `capture_region_focus_and_escape_terminals_preserve_event_order`
- `capture_region_release_before_focus_loss_copies`
- `capture_region_release_then_panel_press_keeps_copy_and_reverse_order_cancels`
- `capture_region_toolbar_arm_after_overlay_does_not_replay_old_focus_events`
- `capture_region_wait_release_then_same_pass_drag_copies_once`
- `capture_region_canvas_drag_releases_at_panel_with_clamped_copy_after_pointer_gone`

先頭2件は `RawInput.focused=false` とviewportのfocus=falseを指定し、実際の `InputState.focused=false` もassertする。
release→focus lossはコピー、focus loss→releaseは取消となる両方の順序を検査する。
追加した所有境界テスト:

- `front_popup_focus_loss_keeps_ordered_release_provenance_and_retires_the_owner`
  （非フォーカスpassでもreleaseを保持し、loss前後の所属を区別、永続ownerは終了）
- `front_popup_focus_round_trip_binds_a_new_stream_to_the_same_viewport`
  （loss中のpressは所有せず、同passの再focus後のpressは元viewportの新列として所有）

今回のlib全件は、利用者の明示指示どおり `MSBUILDDISABLENODEREUSE=1` で
`cargo nextest run -p mimageviewer --lib` を直接実行した。関連filterで代替していない。

| 検証 | 結果 / ログ |
| --- | --- |
| 範囲capture・front popup・overflow・navigator・seek | **689成功 / 0失敗**。`target/section403-capture-focused.log`。 |
| 最終ソースのcapture / focus回帰 | **16成功 / 0失敗**。`target/section403-capture-focused-final.log`。 |
| lib全件・初回 | **13,751成功 / 1失敗 / 既存ignored59件**、403.476秒。`target/section403-capture-lib-full.log`。JUnitは `target/section403-capture-lib-full-first.junit.xml`。報告された範囲capture6件はすべて成功したことをJUnitでも確認。 |
| 初回失敗1件の単独確認 | **1成功 / 0失敗**。`target/section403-capture-unrelated-isolated.log`。 |
| 同一ソースのlib全件・再確認 | **13,752成功 / 0失敗 / 既存ignored59件**、411.457秒。`target/section403-capture-lib-full-confirm.log`。JUnitは `target/section403-capture-lib-full-confirm.junit.xml`。 |
| `cargo test --test ui_snapshot` | **186成功 / 0失敗**、PNG変更なし。`target/section403-capture-ui-snapshot.log`。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功。`target/section403-capture-check.log`。 |
| `cargo fmt` / `cargo fmt --check` / `git diff --check` | 成功。`target/section403-capture-fmt.log`。 |

初回の全件実行だけで `similar_index::tests::startup_configuration_away_back_then_normal_stop_cannot_reuse_old_marker`
が `src/similar_index.rs:11083` の `db.scanned_once_matches(&fingerprint)` のassertで失敗した。
類似索引のコード・テストは今回変更していない。単独実行では成功し、コードを変えずに実行した2回目の全件でも成功した。
原因は未特定であり、今回の範囲capture修正で類似索引の問題も直ったとは扱わない。
最初の失敗ログを保持し、テストのignore・retry設定・並列数の変更は行っていない。

今回のP2 2件の実装・自動検証は完了。実アプリ起動は行わず、build-dev・独立再レビュー・検収・コミットはClaudeCodeへ渡す。

## 10. 実アプリでの確認 (2026-10-19)

観測者: ClaudeCode (利用者の依頼により、隔離 data-dir `target/sstest/data` で起動した build-dev の `mimageviewer-core.exe`
(`0a089b1c8`) を画面操作)。素材は番号入りの生成画像。メインのフルスクリーン、画面幅に合わせた表示 (縦にパンできる状態)。

- メニューなし: 画像のドラッグでパンする (基準)。
- 回転メニューを開いたまま、メニューの外の画像をドラッグ: メニューが閉じ、画像は動かない。続くドラッグでパンする。
- 回転メニューを開いたまま、メニューの外を素早く 2 回クリック: 1 回目は閉じるだけ、2 回目は通常どおりページ送り (この表示ではクリックがページ送りのため)。
- 回転メニューを開いたまま右クリック: メニューが閉じ、フルスクリーンは閉じない。
- 「…」パネルを開いたまま外側をドラッグ: §1.396 どおり画像がパンし、パネルは開いたまま。

未確認 (実アプリ): ダブルクリックによる表示リセットの境界 (この表示モードではクリックがページ送りになるため。実描画テストと変異確認のみ)、
シーク設定 × ナビゲータ / 比較ワイプ・分析 × 回転メニューの重なり (§5 の実描画テストのみ)、中ボタン、タッチ、F11 / F12 往復、パネル内ドロップダウン。

最終確認 (ClaudeCode、`d5307c1ec`): `test-lib-full.ps1` の 1 回目は 13,751 成功 / 1 失敗で、§9.6 と同じ `similar_index::tests::startup_configuration_away_back_then_normal_stop_cannot_reuse_old_marker` (`src/similar_index.rs:11083` の `scanned_once_matches` の assert) が落ちた。このブランチは `similar_index` を変更していない。単独実行は 8 回とも成功、2 回目の全件 (`-Force`) は 13,752 件すべて成功。全件実行の負荷下でだけ落ちる既存の不安定なテストと見ているが、原因は未特定 (別項目で扱う)。
