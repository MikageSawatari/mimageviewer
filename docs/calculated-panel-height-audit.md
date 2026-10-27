# 計算高さと実描画高さの監査（2026-10-13）

監査基点は `master` の `d73d62841`。実装差分は `fit-panels` の `113cf9711` にコミット済み。
実装担当による source inspection・headless egui 検査と、別コンテキストの
GPT-6.1 Sol / xhigh による独立監査／差分レビューの記録である。
実アプリ・native HWND の起動／操作は行っていない。CLAUDE.md は変更していない。

共通helperと、確定した固定行高／仮想行高の不一致修正は実装・自動検証済み。
**全件完了ではない**。後続の独立レビュー（GPT-6.1 Sol / xhigh）は **REVISE**。
構成editorのunit行の誤判定と一覧の未掲載箇所を本追補で訂正する。製品コード・テストは変更しない。
クリップボードカードと長い保存基点名の調整submenuは、設計担当の判断で別作業として記録する。
構成editorのunit行はnext-bookでの右側再設計に引き継ぐ（下記残件節）。

## 検査の契約

`tests/support/scroll_layout.rs` を `tests/support/ui_harness.rs` から共有する。
lib は `crate::test_ui::scroll_layout`、integration は `test_ui::scroll_layout` を使う。
製品コードへ常時の assert は追加しない。

- `Fits`: 実 `content_size.y <= inner_rect.height() + 1物理pixel`、offset 0、
  正の viewport が screen/panel 内にあることを検査。
- `Scrolls`: fixture の内容が実 viewport より高いことを検査。
  実 wheel 入力後の `assert_scrolled_to_end` で offset の進行と末尾到達を検査。
  state の offset を直接書き換えて到達性の証拠にしない。
- ScrollArea を持たず選択周辺をページングする picker は、同じ `Fits` で実内容高を検査し、
  追加で Area の実枠・選択末尾の実矩形・表示行数を調べる。これを ScrollArea の検証とは呼ばない。
- 実 egui で48pt行を5個割り当てる負対照を持つ。計算240ptへ未算入の行間3ptを加えると
  helper は失敗する。snapshot の期待画像を更新しても、この検査は通らない。

十分な高さの画面で全状態を描く `fs_overflow_panel` 検査と、高さ200pt／180ptでの末尾到達を持つ。
状態は Root / NavigatorPosition / SpreadAdjustment / MusicNoteColor。Root は製品の fixed_order 全項目と
本の構成入口、SpreadAdjustment は未解決の基点文面と全体checkboxを含む。
2026-10-14のマージ後レビューで、180pt画面の本文52ptよりScrollAreaの既定最小高64ptが大きく、
スクロール終端でも末尾12ptが実クリップ外へ残る退行を確認した。
行間0・共通高さ式を維持して`min_scrolled_height(0.0)`を復元し、全4状態×3画面の検査で
実ScrollArea本文のclip内にinner_rectが収まることと、実wheel後の内容下端がclip内へ届くことを確認する。
同日の修正確認は全Cargoへ`MSBUILDDISABLENODEREUSE=1`を設定して実行。
共通検査1件（12条件）、overflow 54件（ignored 1）、ui_fullscreen 746件（ignored 1）、
ui_snapshot 176件、`cargo fmt --check`がすべてexit0。PNGは通常比較に一致し、更新なし。
初回の検査用リンクは別テストが実行体を使用中で失敗し、終了を待って修正後の検査を実行した。
実行履歴は`target/overflow-180-validation/`に保存。git add／commitとdev-runtime操作は行っていない。
同日の§1.341結合では、本文spacing0・行数×48の高さ式・min_scrolled_height(0)を保持した。
共通検査をambient spacing0／13の24条件へ拡張し、固定行のRoot／Navigator／Musicでは
実content高と十分なviewport高の両方が行数×48に一致することも検査した。
これにより、Fitsだけでは検出できない高さ予算への行間の二重加算も検出する。関連overflow検査58件成功（1ignored）。
表示モード popup は通常／抽出一覧／本の構成状態を、1800pt／260ptの高さで描く。
既存のズーム／フィット popup の検査も共通 helper へ接続し、実枠の自然高検査を維持した。

## 洗い出しと判定

表の行番号は監査基点 **d73d62841** のもの。修正後は追加されたテストなどで移動する。
`src` の ScrollArea / show_rows / Area / popup / Window と、row_count・row_height・
header/body/content/panel/popup の高さ式を横断検索し、計算から実 widget 配置まで追跡した。
`src/ui_widgets` というモジュールは存在しないため `ui_helpers` / `ui_toolbar_layout` も調べた。

| 対象（基点ファイル:行） | 計算・実描画の根拠 | 判定／対応 |
| --- | --- | --- |
| `ui_fullscreen.rs:5453,5477,5661` その他の機能 | padding 8×2 + header44 + 状態行数×48。Root7行、Navigator5行。区切り線は painter、実行割当48、body spacing0。 | d73d62841 の修正と一致。3状態の十分／不足画面を実描画検査。 |
| `ui_fullscreen.rs:5746` 見開きの調整 submenu | 同じ枠式の6×48予算。3個の48pt操作行と自然高の状態・基点label・checkboxを描く。 | 代表 resolved/unresolved 文面は予算内。既存8 snapshot と状態検査で維持。任意長の基点pathを固定行予算で保証する設計ではない（下記残件）。 |
| `ui_fullscreen.rs:46052,46059,46092,7579` 表示モード | 8pt外余白 + **7**×28見出し + 行数×36。実見出しはページ構成／連結方式／横方向／末尾表紙／先頭・末尾単ページ／表紙次・最終ページの**8**個。行spacing0、行内の絵は32ptだが割当36。 | 不一致を修正：見出し8個を計上。修正前テストは28pt超過で失敗。通常／抽出／構成状態の十分・不足を検査。 |
| `ui_fullscreen.rs:47085,47108,47112` ズーム／フィット | 画面上限から Frame total_margin を控除。実24pt見出し、32pt行＋spacing4、separatorは描画のみ、編集説明は折返し自然高。 | 内容全体の行数で枠を決める方式ではない。既存十分／不足・実枠自然高検査を helper へ接続。 |
| `ui_fullscreen.rs:6523,6535,6559` 静止画 seek strip menu | 行高×行数 + padding6×2 + separator7。内容矩形を1回だけ割当し、各選択肢は手動 rect。小画面で行高を圧縮／上限をかける。 | item_spacing の累積なし。既存 normal/narrow/short/tiny snapshot・操作検査を維持。 |
| `ui_fullscreen.rs:6413` 回転 menu | egui menu が自然高を所有。 | 手動の行数による枠高さなし。 |
| `ui_fullscreen.rs:45545` slideshow popup | 固定330×350、内側を12/10pt縮め、spacing7で通常UIを配置。 | 固定サイズ方式で行数計算＋ScrollAreaではない。今回の高さ式不一致修正対象外。極小画面の全面到達を今回保証しない。 |
| `ui_fullscreen.rs:45835` panorama投影 | 方式数×40 + 見出し36、ラベルは実測幅。painter/手動rectで行を配置。 | 行間の自動累積なし。ScrollAreaではない。 |
| `ui_main.rs:1280,1319,1376,1387,1402` facet選択 | viewport は min(件数,18/24)×max(interact_y,22) + gap×(行数−1) + 前後余裕。checkboxはsans-spacing行高。仮想版だけ show_rows にgap込み行高を渡していた。 | 仮想版の二重spacingを修正。26件／24行上限、実総高と末尾到達を検査。少数件と非仮想版は自然配置で計算と対応。 |
| `ui_main.rs:1935,12285` sort dropdown | groupごとの2行、6ptseparatorとspacingを計上。Button高さは実フォントとpaddingから求める。 | 算式と配置が対応。snapshotを維持。 |
| `ui_main.rs:6203,6292` details列表示 menu | header実測と前回の実内容高、画面上限・丸め余裕から予算を作る。separatorはlayoutに参加しない。 | 実測高を再利用する方式。既存検査を維持。 |
| `ui_main.rs:4408,9467,9511` 本のページ並べ替え | 手動総高／Endは rows×(tile.y+8)。実Gridはgap8。show_rowsが親spacingをさらに足していた。 | 固定slot owner `show_fixed_slot_rows` で親spacing0、Grid gap8は維持。ambient spacing13でも総高430、十分／不足／末尾到達を検査。 |
| `ui_dialogs/collections.rs:1426,4797,4842` collection並べ替え | 本の並べ替えと同型。 | 同じ固定slot ownerへ接続。drag/keyboard/データ所有の動作は維持。 |
| `ui_text.rs:4106,4297` font選択小窓 | 実カードは70pt、親horizontalの間隔はUi spacing。仮想版は70+8をsans-spacing引数に渡していた。 | 8ptの過大計上を修正。製品のcard allocatorとscroll関数を共用した実描画検査を追加。font parse/texture uploadは検査しない。 |
| `ui_dialogs/clipboard_capture.rs:378,385,390,490` 画像選択小窓 | show_rows引数166、カード親160。FailedのFrame内errorは折返し自然高で160を超え得る。 | 不一致あり、未修正。166→160だけでは可変高の不変条件を直せない。設計担当の判断で別作業として記録する（下記）。 |
| `video/native_presenter/overlay_draw.rs:3773,3796` native ring picker | 旧96/104 +32×件数、実行にspacingが付く。drillは枠cap後も全件描画。 | 実ガレー＋Frame total_margin＋明示gap＋32pt行を計上し行spacing0。制約時は選択周辺を描き末尾到達を検査。未drawのregion fallback予測は維持、描画後の実rectは既存region ownerへ返す。 |
| `app/gamepad_input.rs:1770,2256` 通常／drill picker | nativeと同型の32pt計算、自然label／Frame／spacing。 | 同じ原則で修正。入力stateを変えず小画面で選択を含む表示範囲を描く。 |
| `app/gamepad_input.rs:1871,2039,2158` お気に入り／marker／場所 picker | 旧116/92 +34×表示行、実行にspacingが付く。12行上限とscroll_topを持つ。 | 実chrome＋34pt行へ統一。12行上限／お気に入り両tab／空表示／表示文言／選択・data ownerを維持。visible callbackだけでデータを読む。 |
| `video/native_presenter/render_core.rs:1731,1771,1777` native seek strip menu | 行rect、padding、separatorを共通寸法で手動配置。 | 自動item_spacing累積なし。 |
| `video/native_presenter/overlay_draw.rs:628,1134,6190` 調整／jump／metadata | 固定headerとexplicit child max_rect、残りのbodyをScrollAreaへ確保。 | 枠とbodyのgeometryが対応。自然高の内容が予算を超えるとスクロールする。 |
| `ui_adjustment_panel.rs:16228,16361` 補正panel | 実header領域とbody_rectを分離し、explicit child/ScrollAreaに同じ予算を渡す。 | 画面の残り高さを使う方式。body全内容の固定行数見積りではない。 |
| `ui_erase.rs:2249,2320,2340` 消しゴムpanel | headerを外に固定し、画面予算からbodyを確保。 | 同上。ScrollAreaによる自然内容高を維持。 |
| `ui_conceal.rs:2127,2206,2220` 隠蔽panel | headerを外に固定し、allocate_uiで残りbodyを確保。 | 同上。 |
| `ui_metadata_panel.rs:2390` metadata | headerとbody領域を分離、ScrollAreaは親の実残高に従う。 | 同上。7856行のScrollAreaは製品描画ではなく測定テスト。 |
| `ui_composition_editor.rs:230,236,286,669,776,805` 構成editor | footer実測とgapを控除。unitはshow_rowsへ110ptを渡し、110ptをallocateした後、行内をscope_builderで描く。scope終了時に子の自然高（min_rect）で親カーソルを置き直すため、実pitchは110 + item_spacingに固定されない。 | **不一致**。「算式と配置が対応」は誤判定。行の重なり・仮想行切替時のずれが起こる。next-bookで右側を作り直す際に修正し、隣接行の実矩形を測る回帰検査を追加する。ここでは直さない。 |
| `ui_dialogs/file_organize.rs:219,302,329` 整理source/destination | show_rowsにsans-spacing行高、実row・keyboard offsetはgap込みpitchを共用。 | 二重spacingなし。 |
| `ui_dialogs/collections.rs:4498` import preview | 行高を show_rows と add_sized で共用。320ptは一覧の上限。 | 二重spacingなし。 |
| `ui_music_timeline.rs:445,2223` 音楽timeline | painterによるrow_h+row_gapの明示rect。 | 自動spacingの追加なし。内容はtimelineとして正当に画面予算を超える。 |
| `ui_dialogs/mod.rs:103,210` 起動時共通本文 | font/wrap/footer/Frameを実測して予算を作る。 | 自然高／固定操作到達の既存geometry検査を維持。 |
| `ui_dialogs/content_restore.rs:200` 復元 | footer64を予約し、本文はactual available heightを使用。 | 内容の推定総高ではなく上限制約。 |
| `ui_dialogs/preferences.rs:3146,3199,3574` 設定小窓／本文 | actual remaining heightと親ScrollArea。 | 同上。 |
| `ui_dialogs/favorites_editor.rs:567` お気に入り設定 | 本文単一ScrollArea、window available height。 | 同上。 |
| `ui_dialogs/smart_folder_editor.rs:629,672,769` smart設定 | footer42を予約し残りchildにScrollArea。 | 同上。 |
| `ui_dialogs/settings_restore.rs:1757,1768` 設定復元 | header58/footer42を予約、本文ScrollArea。 | 同上。 |
| `ui_dialogs/stats_dialog.rs:49`、`context_shortcuts.rs:425`、`cache_creator.rs:70`、`context_menu.rs:195` | screen−160/180/260/180などのmax_height。Window/Modalは自然高、本文は可変。 | 固定見出し等を引く予算候補として点検。全本文が収まるという計算式ではない。新たな誤計数の確定根拠なし。 |
| `video/native_presenter/overlay_draw.rs:1773,1967,6644,6657` help／一括bookmark／VST | dialog−104/340等の本文上限。VSTはslot件数から毎描画で固定枠を計算し、slot一覧の上限を別に決める。内部slot rowは自然高でoverflow時にscrollする。 | 内容高との同値を前提にしない上限制約候補。今回の実描画geometry検査は未追加。 |
| `ui_analysis_panel.rs:831` 分析表示 | painter固定13pt×行数。 | 自動縦並び／ScrollAreaによる増分なし。 |

### REVISEで補充した候補（113cf9711の行番号）

下表は追加のsource inspectionであり、今回新たな実描画テストは行っていない。
「画面予算型」は可変本文を収めるviewportの上限を求める方式で、全本文の自然高との同値を
計算しているわけではない。内容高一致検査の対象外でも、小画面での枠・sink・操作の到達性を
保証したとは扱わない。静的照合で分かった不一致／候補は下記残件へ記録する。

| 対象（ファイル:行） | 計算・実描画の根拠 | 判定／対象外の理由 |
| --- | --- | --- |
| `ui_text.rs:2720,2728,3220` テキスト注釈・左一覧 | body = clamp(画面高 − 60×2 − 124, 220, 1600)。sinkはbody + 108を画面高 − 64でcapし、最低120を確保。実header・追加操作・separatorを描いた後、一覧上限はclamp(sink下端 − 実cursor上端 − 操作予約40, 80, 2000)、縦auto_shrinkあり。 | 画面予算型で、オブジェクト行数から全文高を見積もる方式ではない。実cursor控除は追跡できるが、残高80未満でも最低80を使うため、小画面で一覧＋操作行がsinkを越える不一致候補を残す。 |
| `ui_text.rs:2720,2744,3315` テキスト注釈・右詳細 | sinkはbody + 72を画面高 − 64でcapし、最低120を確保。一方、Frame内の見出し・separatorの後にbody全高をallocate_uiし、ScrollAreaへ同じbody高を渡す。 | **小画面の予算不一致**。sinkのcapを本文割当に反映していない。例えば画面高260ではbody220、sink196となり、本文割当だけでsinkを超える。自然本文高の行数計算とは別の枠／入力領域の残件。 |
| `ui_sns_split.rs:498,803,863` SNS分割 | 外高 = max(画面下端 − panel上端 − 14, 240)。Frameのheader・separator描画後、body = max(panel下端 − 実cursor上端 − 12, 100)を親領域とScrollAreaへ渡す。 | 画面予算型。実header位置を引く方式で、全文高の一致を要求する行数計算ではない。最低外高240／本文100により極小画面の到達性は保証しない。margin・最低値を含む実枠検査は未実施。 |
| `ui_crop.rs:10,206,280,307` 切り取り | 外高 = clamp(画面下端 − panel上端 − 14, 220, 440)。Frame内のUiへset_max_height(外高)を設定し、header・separator・可変のcontrolsを自然配置する。 | 内容高一致helperの直接対象外（ScrollAreaなし）。**予算不一致候補**：外高をFrame内Uiの上限へ渡し、Frame marginを控除していない。set_max_heightだけでは内容の伸長・clippingを保証せず、220ptの下限時などの実枠／操作検査が必要。 |
| `ui_view_trim.rs:55,1122,1523,1550,1571` 表示トリム | 外高は250～560にclamp。Frame内Uiへset_max_height(外高)、本文上限はmax(外高 − 42, 160)。本文は可変controlsのScrollArea。 | 画面予算型で、自然全文高との同値は対象外。**予算不一致候補**：固定控除42が実header・separator・Frame total_marginから求められておらず、内側の上限と外枠を同じ高さとしている。親available_heightによる縮小を含めた実枠の照合が必要。 |
| `app/gamepad_input.rs:512,517,580,619` マウスジェスチャガイド | 外高 = min(96 + max(行数,1)×32, usable_h)。行は上端 + 46からpainterの手動rectで32ずつ進む。高さ不足ではfloor((外高 − 96)/32)相当の行数を選択行周辺に描く。 | 行pitchは式と一致（静的照合）。自動item_spacing・Frame margin・折返しUi・ScrollAreaを使わないため、ScrollArea helperの直接対象外。usable_h最低120による極小画面のはみ出しと文字の横幅は未検証。 |
| `ui_adjustment_panel.rs:15067,15183` 部分補正の一覧／選択レイヤー小パネル | 一覧bodyは画面下端 − 実cursor上端 − bottom margin、右の編集bodyはtool_panel_rect高 − 14。それぞれ最低body高とScrollAreaを持つ。 | 主補正panelと別の未掲載経路。画面予算型／可変本文なので全文高一致の対象外。最低値・右の固定控除14とFrameの実枠一致は未検証。 |
| `ui_folder_pane.rs:54,311,501` フォルダpane／一覧 | SidePanelの実available_sizeを一度allocateし、同じbody_rectでnew_childとclipを作る。子でScrollAreaを描く。 | 自然な一覧高に対するviewport予算型。行数から外枠高を計算する方式ではなく、親領域の境界を共用する。 |
| `ui_music_panels.rs:1043,1127,1149` 音楽の情報panel | 固定title矩形を除いたcontent_rectを左右12・上下8縮め、new_child＋clip＋ScrollAreaへ使う。 | 可変のmetadata本文に対するviewport予算型。全文高の行数計算ではない。 |
| `ui_music_panels.rs:2430,2458` 音量ノーマライズ進捗 | 固定外高110。見出し・bar・percentage・×はpanel_rectを基準にpainter／interactの手動座標で配置する。 | 固定サイズ／手動描画で、縦widget列やScrollAreaを使わない。item_spacingによる高さ差の対象外。 |
| `ui_fullscreen.rs:3046,3079` 静止画navigator | 正方形canvasの辺を画面予算でcapし、外高はcanvas + header26。header_rectとcanvas_rectを同じ寸法から分割する。 | canvas＋手動headerで式とrectが対応。縦widget列やScrollAreaはなく、内容高一致helperの直接対象外。 |
| `ui_analysis_panel.rs:991,1013` 分析panel本体 | 上HUDを除いた画面領域からpanel_rectを作り、固定headerとグラフを手動座標で描く。 | 831行の文字表示とは別の未掲載経路。画面領域／painter方式で、ScrollAreaの内容高見積りではない。 |
| `ui_image_folder_zip.rs:49,60,64` 画像folder→ZIP modal | 初期高はmin(400, max(画面高 − 70,120))。実available_heightから操作予約（interact_size.y + spacing×2）を引き、本文ScrollAreaへ渡す。 | 可変path・本文に対するWindowの画面予算型。全文の自然高を行数で計算しない。従来の小画面controls検査は過去の検証証拠であり、本追補では再実行しない。 |

## snapshot の扱い

既存の `fullscreen_overflow_panel_root.png` は720×440。上端56pt・下余白12ptなので
パネルに使える高さは372pt、計算高さ396ptより24pt小さい。バーは正当なものとして維持する。
Navigator submenu と tall zoom/fit の既存PNGも目視した。
新しい十分な高さ720×520のRootを少数の代表PNGとして追加し、同じ描画に `Fits` を掛ける。
追加した `fullscreen_overflow_panel_root_tall.png` を目視し、全7行・下端のルーペ固定・
固定タイトル／×が枠内にあり、スクロールバーがないことを確認した（headless生成PNGの観測）。
既存Root／Navigator／Collection並べ替えは変更なしで通常比較に通った。
Collection並べ替えとfacet sortの既存PNGも目視した。
初回 `ui_snapshot` 全体比較では175件成功／1件失敗。失敗は既存の見開き調整
resolved/unresolved × normal/narrow × light/dark の8枚で、d73d62841のbody spacing0修正への
未追随だった。基点文面・jump・解除・checkboxの位置だけが行間の除去分だけ上がる。
差分PNG・生成された8枚すべてを目視し、文字・操作・×・枠内への収まりとバーなしを確認して更新した。
これは前コミットの整合修正を期待画像へ反映したもので、今回新しく製品の描画を変えて通したものではない。
この8枚に元々不要なバーがあったという証拠ではない。
追加はRoot tallの1枚（34,687 bytes）だけで、既存の小画面Rootの正当なバーは維持している。
更新した見開き調整8枚の合計は288,757 bytes。新規PNGは1枚だけなので純増は約34KiB。

## 残件と引き継ぎ（設計担当の判断確定）

クリップボードの可変高カードは、固定仮想行を実際の可変高へ対応させる必要がある。
全件描画へ変えてvirtualizationを失う、error文を省略して情報を減らす、といった対処は採らない。
**設計担当の判断で別作業として記録する**。本差分では修正しない。

見開きの調整submenuにも別の既存不一致がある。`spread_shift_hud_view` は
RelativePath/ArchiveEntry全文をanchorへ追加し、labelは自然に折り返す一方、bodyは固定288pt。
参照は `src/book_scope.rs:6,15` で最大16KiBまで許す。固定操作3行の144ptを引いた
残り144ptでは、長い保存基点名のstatus/anchor/checkboxを賄えない場合がある。
代表fixtureが通ったことは、このケースの修正証拠ではない（静的監査での指摘、製品未起動）。

定数増加では任意長を保証せず、省略・hoverへの移設は現行の全文表示を変える。
resolve後の同一immutable view/layoutをwheel・foreground・draw・touch除外へ渡す境界整理が
根本案となる。独立に高さを計るだけでは同frameのresolveによる差異を残す。
この所有境界の整理も、**設計担当の判断で別作業として記録する**。本差分では修正しない。

構成editorのunit行は、110pt割当後のscope_builderが子の自然高で親カーソルを置き直す
不一致である。`vendor/egui/src/ui.rs:2505,2515` のscope_dynは
`advance_cursor_after_rect(child_ui.min_rect())` を呼ぶため、仮想行の110 + item_spacingと
実際の隣接行の位置が一致する保証がない。元の「構成editorは一致」という判定を撤回する。
**next-bookブランチで本の構成editorの右側を作り直す作業**（§1.242「組の作り直し」、
2026-10-13利用者決定）の中で直し、**隣接行の実矩形**を測る回帰検査をそこで追加する。
行の重なりと仮想行の切替前後のずれを検査し、内容総高の比較だけで済ませない。ここでは直さない。

追補の静的照合では、テキスト注釈右詳細のsink capとbody割当の小画面不一致も見つかった。
左一覧の最低80pt、切り取り・表示トリムの内外枠予算も不一致候補として残す。
これらは今回修正しない。別途、Frameの実枠・sink・ScrollArea viewport・固定操作の実矩形を
同じ描画passで測り、最低値や固定chrome控除を含む境界を調べて修正する必要がある。
候補について実アプリの発生頻度や実測超過量を確認したとは扱わない。

## CLAUDE.md 追記案（ファイルへは未反映）

「パネル (Area + Frame::popup)」節へ次の短い項目を提案する。

> 高さを計算で決めるなら、item_spacing・separator・padding・Frame の margin を計算に含めるか
> 該当間隔を0にする。show_rowsはspacingを内部加算するので二重計上しない。
> 収まる画面ではスクロールしないこと、高さ不足では末尾へ到達できることを
> tests/support/scroll_layout.rs の共通helperで検査する。
> snapshotは見た目の記録であり、スクロールの要否の検査にはならない。

`docs/ui-snapshot-policy.md` にはこのルールとhelperの入口を直接追記した。

## 検証

以下は113cf9711の実装時の検証記録である。今回のREVISE追補は本Markdownのみを変更し、
製品コード・テスト・snapshotは変更しない。新たなテスト／ビルドは実行せず、
文書差分と`git diff --check`を確認する。過去の成功件数は未修正の残件の保証には使わない。

- 修正前: `cargo test -p mimageviewer --lib calculated_`。overflowは成功、spread popupは
  `unnecessary vertical scrolling` / excess28で失敗。
  ログ: `target/height-layout-tests.log`。
- 初期修正後: 同filterの6件成功。ログ: `target/height-layout-tests-fixed.log`。
  その後の追加picker/font/snapshotを含む最終結果は以下。
- 最終関連lib: `cargo test -p mimageviewer --lib -- calculated_
  specified_scale_popup_uses_available_height_and_scrolls_only_when_needed fullscreen_overflow_panel
  spread_popup book_reorder_drag_tests collection_reorder_ native_ring
  --skip fullscreen_overflow_panel_snapshot_root_tall_without_scrollbar --nocapture`。
  **35件成功、exit 0**。ログ: `target/height-layout-related-lib-tests.log`。
  calculated_*の10本と、従来の操作／geometry／代表snapshotを含む。
  pickerの計64条件・Spread popupの3状態×2高さなどは各テスト内の反復条件であり、件数へ水増ししない。
  1物理pixelの座標丸め診断も同ログに残し、selectedだけの厳密比較による誤判定を修正した。
- 新Root tall: `UPDATE_SNAPSHOTS=1` の環境で
  `cargo test -p mimageviewer --lib fullscreen_overflow_panel_snapshot_root_tall_without_scrollbar`。
  **1件成功、exit 0**。ログ: `target/height-layout-new-snapshot.log`。生成PNGは目視済み。
- 見開き調整snapshot更新: `$env:UPDATE_SNAPSHOTS = '1'` を設定した子PowerShellで
  `cargo test -p mimageviewer --test ui_snapshot spread_shift_hud_snapshots`。
  **1件成功、8枚更新、exit 0**。ログ: `target/height-layout-spread-snapshot-update.log`。
- 最終 `cargo test -p mimageviewer --test ui_snapshot`: **176件成功、exit 0**。
  ログ: `target/height-layout-ui-snapshot-final.log`。
- 通常比較 `cargo test -p mimageviewer --lib fullscreen_overflow_panel_snapshot_`:
  **3件成功、exit 0**。新Root tallも更新モードなしで比較した。
  ログ: `target/height-layout-overflow-snapshot-final.log`。
- `cargo fmt --all --check` / `git diff --check` は成功。
  `python scripts/check_ui_glyphs.py` は `ok: no dangerous glyphs found in UI strings.`。
- `.\scripts\build-dev.ps1 -PreserveRuntime`: **exit 0**。
  通常featureの `dev-runtime` core／Remote service／EPUB PDF workerを作成し、起動はしていない。
  PE prerequisiteもruntime4／PE3で成功。検証用coreは `target/dev-runtime/mimageviewer-core.exe`。
  実行結果はtool transcript、PEレポートは `target/vcrt-pe-reports/dev-runtime.json`。
- `test-full.ps1` とcommitは利用者指定どおり実施しない。

利用者が起動する場合は、先にインストール版／tray常駐版を閉じる（single-instance mutex共有）。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

既定の `%APPDATA%\mimageviewer` を使い、実際の設定・データを更新し得る。
十分な高さで静止画HUDのRoot／Navigator／調整submenuと表示モードを開き、短い本文が収まること、
小画面では末尾までスクロールできることを確認する。ゲームパッドの通常／drillと
お気に入り・marker・場所一覧は、小画面で最終選択項目とフッタの到達を確認する。
長い保存基点名とクリップボードの可変高は上記の未修正事項であり、確認済みの挙動とは扱わない。
