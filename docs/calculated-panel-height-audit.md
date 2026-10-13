# 計算高さと実描画高さの監査（2026-10-13）

対象は `master` の `d73d62841` に対する未コミット差分。
実装担当による source inspection・headless egui 検査と、別コンテキストの
GPT-6.1 Sol / xhigh による独立監査／差分レビューの記録である。
実アプリ・native HWND の起動／操作は行っていない。CLAUDE.md は変更していない。

共通helperと、確定した固定行高／仮想行高の不一致修正は実装・自動検証済み。
**全件完了ではない**。可変高のクリップボードカードと長い保存基点名の調整submenuは、
根本修正の境界が広がるため未修正・範囲回答待ちとして残す（下記残件節）。

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

十分な高さの画面で全状態を描く `fs_overflow_panel` 検査と、高さ200ptでの末尾到達を追加した。
状態は Root / NavigatorPosition / SpreadAdjustment。Root は製品の fixed_order 全項目と
本の構成入口、SpreadAdjustment は未解決の基点文面と全体checkboxを含む。
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
| `ui_dialogs/clipboard_capture.rs:378,385,390,490` 画像選択小窓 | show_rows引数166、カード親160。FailedのFrame内errorは折返し自然高で160を超え得る。 | 不一致あり、未修正。166→160だけでは可変高の不変条件を直せない。利用者へ改修範囲を質問中（下記）。 |
| `video/native_presenter/overlay_draw.rs:3773,3796` native ring picker | 旧96/104 +32×件数、実行にspacingが付く。drillは枠cap後も全件描画。 | 実ガレー＋Frame total_margin＋明示gap＋32pt行を計上し行spacing0。制約時は選択周辺を描き末尾到達を検査。未drawのregion fallback予測は維持、描画後の実rectは既存region ownerへ返す。 |
| `app/gamepad_input.rs:1770,2256` 通常／drill picker | nativeと同型の32pt計算、自然label／Frame／spacing。 | 同じ原則で修正。入力stateを変えず小画面で選択を含む表示範囲を描く。 |
| `app/gamepad_input.rs:1871,2039,2158` お気に入り／marker／場所 picker | 旧116/92 +34×表示行、実行にspacingが付く。12行上限とscroll_topを持つ。 | 実chrome＋34pt行へ統一。12行上限／お気に入り両tab／空表示／表示文言／選択・data ownerを維持。visible callbackだけでデータを読む。 |
| `video/native_presenter/render_core.rs:1731,1771,1777` native seek strip menu | 行rect、padding、separatorを共通寸法で手動配置。 | 自動item_spacing累積なし。 |
| `video/native_presenter/overlay_draw.rs:628,1134,6190` 調整／jump／metadata | 固定headerとexplicit child max_rect、残りのbodyをScrollAreaへ確保。 | 枠とbodyのgeometryが対応。自然高の内容が予算を超えるとスクロールする。 |
| `ui_adjustment_panel.rs:16228,16361` 補正panel | 実header領域とbody_rectを分離し、explicit child/ScrollAreaに同じ予算を渡す。 | 画面の残り高さを使う方式。body全内容の固定行数見積りではない。 |
| `ui_erase.rs:2249,2320,2340` 消しゴムpanel | headerを外に固定し、画面予算からbodyを確保。 | 同上。ScrollAreaによる自然内容高を維持。 |
| `ui_conceal.rs:2127,2206,2220` 隠蔽panel | headerを外に固定し、allocate_uiで残りbodyを確保。 | 同上。 |
| `ui_metadata_panel.rs:2390` metadata | headerとbody領域を分離、ScrollAreaは親の実残高に従う。 | 同上。7856行のScrollAreaは製品描画ではなく測定テスト。 |
| `ui_composition_editor.rs:230,236,286,669,776` 構成editor | footer実測とgapを控除。仮想preview/source/unitはsans-spacing行高、unit110pt。 | 算式と配置が対応。 |
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

## 残件と範囲判断

クリップボードの可変高カードは、固定仮想行を実際の可変高へ対応させる必要がある。
全件描画へ変えてvirtualizationを失う、error文を省略して情報を減らす、といった対処は採らない。
AGENTS.md「Bug Fix Policy」の「正しい修正が現在の範囲より大きければ、選択肢を示して止める」
境界に該当すると判断し、今回含める／別作業として記録する、の範囲質問を送っている。
回答待ちの内容を実装済みとは扱わない。

見開きの調整submenuにも別の既存不一致がある。`spread_shift_hud_view` は
RelativePath/ArchiveEntry全文をanchorへ追加し、labelは自然に折り返す一方、bodyは固定288pt。
参照は `src/book_scope.rs:6,15` で最大16KiBまで許す。固定操作3行の144ptを引いた
残り144ptでは、長い保存基点名のstatus/anchor/checkboxを賄えない場合がある。
代表fixtureが通ったことは、このケースの修正証拠ではない（静的監査での指摘、製品未起動）。

定数増加では任意長を保証せず、省略・hoverへの移設は現行の全文表示を変える。
resolve後の同一immutable view/layoutをwheel・foreground・draw・touch除外へ渡す境界整理が
根本案となる。独立に高さを計るだけでは同frameのresolveによる差異を残す。
この所有境界の整理も範囲確認が必要な2件目として記録し、今回勝手に定数やguardを足さない。

## CLAUDE.md 追記案（ファイルへは未反映）

「パネル (Area + Frame::popup)」節へ次の短い項目を提案する。

> 高さを計算で決めるなら、item_spacing・separator・padding・Frame の margin を計算に含めるか
> 該当間隔を0にする。show_rowsはspacingを内部加算するので二重計上しない。
> 収まる画面ではスクロールしないこと、高さ不足では末尾へ到達できることを
> tests/support/scroll_layout.rs の共通helperで検査する。
> snapshotは見た目の記録であり、スクロールの要否の検査にはならない。

`docs/ui-snapshot-policy.md` にはこのルールとhelperの入口を直接追記した。

## 検証

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
