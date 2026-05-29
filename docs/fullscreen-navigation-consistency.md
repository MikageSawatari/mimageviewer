# フルスクリーン / 検索ナビゲーション統一メモ

最終確認日: 2026-05-13

このメモは、画像・ZIP・PDF・動画・検索結果ビューをまたぐ
「前後移動」「Ctrl+↑↓」「境界ヒント」の操作感を揃えるための
実装済みの統一仕様と、残っている改善候補をまとめる。

## 1. 用語

| 用語 | 意味 |
| --- | --- |
| アイテム移動 | 現在の `visible_indices` 内で、前後の表示対象へ移動する操作。フルスクリーンではホイール / ↑↓、画像では ←→ も含む |
| フォルダ横断 | `folder_tree` の DFS 前順で前後のフォルダ / ZIP / PDF を探す操作。通常コンテキストでは Ctrl+↑↓ |
| 兄弟フォルダ移動 | 現在地と同じ親を持つ前後フォルダ / ZIP / PDF そのものへ 1 ステップ移動する操作。通常コンテキストでは Ctrl+PageUp/PageDown |
| 検索スコープ移動 | Ctrl+S / Ctrl+G の検索結果範囲から外に出ない前後移動 |
| 境界ヒント | 先頭 / 末尾に達したとき、次に使える操作を中央または native overlay に出す案内 |

`adjacent_navigable_idx` は現在、`Image` / `Video` / `ZipImage` /
`ZipSeparator` / `PdfPage` を同一フォルダ内の前後移動対象に含める。
フルスクリーンとして実際に開く対象は主に `Image` / `Video` /
`ZipImage` / `PdfPage` で、検索結果の遷移先判定もこの image-like 集合を使う。

## 2. 現状

### 2.1 通常グリッド / Ctrl+F

| 状態 | Ctrl+↑↓ | 境界案内 |
| --- | --- | --- |
| 通常グリッド | `FolderNavMode::Grid` で DFS。画像 / 動画を含む通常フォルダ、画像入り ZIP、PDF が停止対象 | なし |
| 通常グリッド | `FolderNavMode::SiblingGrid` で兄弟限定移動。空フォルダも skip せず、最後の兄弟を越えた場合は右上トースト | 「次/前の兄弟フォルダはありません」 |
| Ctrl+F ローカル検索中 | `visible_indices` は Ctrl+F で絞られる。Ctrl+↑↓ は no-op。検索バーにフォーカスがある間はショートカットをブロック | 必要なら no-op toast |

Ctrl+F は「現在一覧に対するフィルタ」。フォーカスが検索バーから外れていても
Ctrl+↑↓ でフォルダ横断しない。

### 2.2 画像 / ZIP / PDF フルスクリーン

| 操作 | 現状 |
| --- | --- |
| ホイール / ↑↓ / 画像の ←→ | `visible_indices` 内の前後アイテムへ移動 |
| 同一一覧の先頭 / 末尾 | `FsBoundaryHint::Edge` を中央表示し、Home / End と Ctrl+↑↓ を案内。文言は「項目」表現 |
| Ctrl+↑↓ | `FolderNavMode::Fullscreen` で DFS。現在は前後どちらの方向でも移動先フォルダの先頭 image-like に着地する |
| Ctrl+PageUp/PageDown | `FolderNavMode::SiblingFullscreen` で同じ親の前後兄弟へ移動する。移動先に image-like があれば先頭 image-like に着地し、なければ一覧へ戻る |
| 移動先に画像 / 動画が見つからない | `FsBoundaryHint::NoImageFolder` を表示 |
| 兄弟が無い | `FsBoundaryHint::NoSiblingFolder` を表示 |
| Ctrl+G DrilledInto 中の Ctrl+↑↓ | `global_search_ctrl_nav_fullscreen` で検索結果スコープ内を移動し、先頭 / 末尾で `SearchEnd` を表示。移動先は方向に関わらず先頭 image-like |

注意: 以前の docs には「Ctrl+↑ は前フォルダの末尾画像へ」と書かれていたが、現在の実装は「方向に関わらず移動先の先頭」。

### 2.3 native 動画フルスクリーン

Windows の現行動画フルスクリーンは native presenter 経路。

| 操作 | 現状 |
| --- | --- |
| ホイール / plain ↑↓ | 同一 `visible_indices` 内の前後アイテムへ移動 |
| ←→ | seek。Ctrl+←→ は 30 秒 seek、Shift+←→ は 1 秒 seek。S タイルモード中は seek せずタイルカーソル移動、Ctrl+←→ は 1 行分移動、Enter でカーソル位置から再生 |
| Shift+↑↓ | 音量を dB フェーダー目盛りの 1/4 幅で上下 |
| Ctrl+↑↓ | `handle_fullscreen_ctrl_nav_context` 経由で画像系と同じコンテキスト移動 |
| Ctrl+PageUp/PageDown | `handle_fullscreen_sibling_nav_context` 経由で画像系と同じ兄弟限定移動 |
| マウス戻る / 進む | native window / HUD 側の XButton を App 側で Ctrl+↑↓ と同じ経路へ接続 |
| S | 動画タイルモード ON/OFF |
| S タイルモード中のホイール | 前後アイテムへ移動。移動先も動画なら native presenter を保持して source 差し替え |
| S タイルモード中の Ctrl+ホイール | タイル列数変更 |
| S タイルモード中の ←→ / Ctrl+←→ / Enter / P | 強調表示されたタイルカーソルを 1 タイル / 1 行分移動し、Enter で再生開始。P はカーソル位置のタイルを代表フレームとしてピン留め。S / Esc で閉じるだけなら seek しない |
| 同一一覧の先頭 / 末尾 | native overlay toast で「最初/最後の項目です」+ Ctrl+↑↓ の案内を表示 |
| Ctrl+G / Ctrl+S スコープ | `handle_fullscreen_ctrl_nav_context` 経由で検索スコープ移動に入る |

native 動画の境界案内は MVP として一行 toast に揃えている。画像系と同じ中央パネル表示は
将来改善候補。

### 2.4 Ctrl+S / Ctrl+G

| 検索状態 | グリッド Ctrl+↑↓ | フルスクリーン Ctrl+↑↓ |
| --- | --- | --- |
| Ctrl+S 結果一覧 (`nav_stack` 空) | 何もしない | 該当なし |
| Ctrl+S 結果から実フォルダ等を開いた後 | `FolderNavMode::Favsearch { fullscreen: false }`。結果サブツリー内を DFS し、外へ出たら次 / 前の検索結果へ | `FolderNavMode::Favsearch { fullscreen: true }`。画像系 / native 動画とも `nav_stack` スコープを維持 |
| Ctrl+G Aggregated | 何もしない | 該当なし |
| Ctrl+G DrilledInto | `global_search_ctrl_nav`。検索結果ツリー内を DFS し、次 / 前の container root へ跨ぐ | `global_search_ctrl_nav_fullscreen`。画像系 / native 動画とも同じ |

Ctrl+S / Ctrl+G はフルスクリーン側でも専用スコープを維持する。

## 3. 統一仕様案

この領域は以下を統一仕様とする。

### 3.1 移動操作

| 入力 | 画像 / ZIP / PDF | 動画 | 備考 |
| --- | --- | --- | --- |
| ホイール | 前後アイテム | 前後アイテム | 既存どおり |
| plain ↑↓ | 前後アイテム | 前後アイテム | 既存どおり |
| ←→ | 前後アイテム | seek | 動画プレイヤー慣例として差異を許容 |
| Shift+↑↓ | 前後アイテム | 音量 | 動画プレイヤー慣例として差異を許容 |
| Ctrl+↑↓ | 現在コンテキストの前後フォルダ / 前後検索結果 | 同左 | native 動画 / タイルモードでも有効にする |
| Ctrl+PageUp/PageDown | 前後の兄弟フォルダ | 同左 | 空フォルダを skip しない。検索中は実フォルダの兄弟概念に戻さず no-op |
| マウス戻る / 進む | Ctrl+↑↓ と同じ | Ctrl+↑↓ と同じ | native 動画でも同じにする |

Ctrl+↑↓ の「現在コンテキスト」は以下の優先順位で解釈する:

0. Ctrl+F / Ctrl+S / Ctrl+G の検索バーにフォーカスがある間: 文字編集を優先し、既存どおりショートカットをブロックする。
1. Ctrl+G DrilledInto 中: Ctrl+G の検索結果ツリー内を移動する。
2. Ctrl+S で実結果を開いている中: Ctrl+S の `nav_stack` スコープ内を移動する。
3. Ctrl+F active 中: 移動しない。Ctrl+F は現在一覧のフィルタとして扱い、フォルダ横断を開始しない。
4. 通常表示: 実フォルダツリーを DFS で移動する。
5. Ctrl+G Aggregated / Ctrl+S 結果一覧: まだ開いている container が無いので、移動せず案内だけ出す。

Ctrl+F active 中に Ctrl+↑↓ を通常 DFS に流すと、「検索結果を見ていたのにフォルダ移動で
検索が外れる」「移動先でも同じ filter を適用して 0 件になる」など、ユーザーが現在地を
理解しづらい状態になりやすい。自動スキップも HDD 上で複数フォルダ探索を誘発しやすいため、
統一仕様では no-op とし、必要なら短い案内だけ出す。

検索バーは相互排他を前提にする。Ctrl+G / Ctrl+S / Ctrl+F の状態が何らかの理由で重なった
場合は Ctrl+G DrilledInto を最優先し、通常は検索モードを開く時点で他の検索バーを閉じる。

### 3.2 移動先の決め方

- フォルダ横断の停止対象は `folder_should_stop` と同じ:
  画像または動画を含む通常フォルダ、画像入り ZIP、PDF。
- フルスクリーンを維持して移動した場合、方向に関わらず移動先の先頭 image-like
  (`Image` / `Video` / `ZipImage` / `PdfPage`) を開く。
- これにより、`Ctrl+↑` で「前フォルダの末尾」へ行きたいケースは `Ctrl+↑` → `End`
  の 2 操作になる。一方で、連打中に前後方向が混ざっても着地点が安定し、
  「フォルダへジャンプしたら冒頭から見る」という mental model に統一できるため、
  先頭着地を採用する。
- ZIP / PDF ファイルだけが置かれたフォルダでは、ZIP / PDF を仮想フォルダとして開き、
  enumerate 完了後に先頭 image-like を開く。
- `ZipSeparator` は同一一覧の見出しとしては前後移動対象に残してよいが、
  フォルダ横断の着地点にはしない。

### 3.3 境界ヒント

画像系と native 動画で同じ意味の案内を出す。文言はメディア混在に備え、
「画像」固定ではなく「項目」または「画像 / 動画」を使う。

| 状態 | 表示する内容 |
| --- | --- |
| 同一一覧の末尾 | 「最後の項目です」 + `[Home] 最初に戻る` + `[Ctrl]+[↓] 次のフォルダへ` |
| 同一一覧の先頭 | 「最初の項目です」 + `[End] 最後に移動` + `[Ctrl]+[↑] 前のフォルダへ` |
| フォルダ横断で停止対象なし | 「次/前の画像・動画フォルダが見つかりません」 |
| Ctrl+G の検索結果末端 | 「最後/最初の検索結果です」 + 検索を閉じると通常移動に戻る案内 |
| Ctrl+F active 中の Ctrl+↑↓ | 「Ctrl+F 検索中はフォルダ移動しません」などの no-op 案内 |
| Ctrl+S / Ctrl+G の結果一覧で Ctrl+↑↓ | 「検索結果を開いてから Ctrl+↑↓ で移動できます」などの no-op 案内 |

画像系の no-op 案内は中央の `FsBoundaryHint::NavNoOp` で表示する。native 動画の境界 /
no-op 案内は段階を分ける:

- MVP: 既存の一行 toast 文言に `[Ctrl]+[↑/↓]` の次操作を含め、文言を「画像」固定から
  「項目」または「画像 / 動画」に変更する。
- Aspirational: 画像系の `FsBoundaryHint` と同じ情報量を native overlay の egui-wgpu
  上で中央表示する。

### 3.4 pending ナビゲーションと scope 変更

`pending_folder_nav_steps` はモード横断の単一バッファなので、検索 / フルスクリーン /
タイル状態が変わった後に古い入力が旧 scope のまま解決されると分かりにくい。
次の状態変更では pending folder navigation を flush する:

- Ctrl+F / Ctrl+S / Ctrl+G を開く、閉じる、または別検索モードへ切り替える。
- Ctrl+S の `nav_stack` root が変わる。
- Ctrl+G の Aggregated / DrilledInto が切り替わる。
- フルスクリーンを閉じる、または native 動画タイルモードを閉じる。
- Ctrl+F active 中に Ctrl+↑↓ が押された場合は、新しい DFS を開始せず pending も積まない。

### 3.5 native 動画タイル遷移

動画タイル中の Ctrl+↑↓ は、移動先で分岐する:

| 移動先 | 挙動 |
| --- | --- |
| 動画 | native presenter を保持し、既存の source 差し替え経路でタイルモードを維持する |
| 画像 / ZIP / PDF | native 動画とタイルモードを閉じ、通常の画像系フルスクリーンとして開く |

一度タイルモードを閉じて画像系フルスクリーンへ出た後は、次に動画へ移動してもタイルへ
自動復帰しない。タイルに戻るにはユーザーが明示的に S を押す。

動画→動画の source swap が一時的に `Error` / `Missing` になった場合も、S タイルモードは
暗黙に解除しない。ホイール連打中は未対応動画や metadata 未取得の過渡状態をまたぐことがあり、
そこで `video_tile_mode_active` を落とすと次のホイール入力が通常 fullscreen navigation に
流れてタイル表示が勝手に消えるため。代わりに preparing overlay を維持し、次の動画 target へ
進む入力も tile fast-swap 経路で処理する。

## 4. 残っている改善候補

| 改善候補 | 関連箇所 |
| --- | --- |
| native 動画の境界案内は一行 toast MVP。画像系と同じ中央パネルではない | `app/native_video.rs::native_boundary_hint_text` / native overlay |
| native 動画 fullscreen / S タイル / XButton は自動 snapshot で検証しづらい | 手動 sweep または E2E checklist |

## 5. 修正時チェックリスト

- [x] 画像 (`Image`) のフルスクリーン末尾 / 先頭で、境界ヒントと Ctrl+↑↓ が一致する。
- [x] ZIP 内画像 (`ZipImage`) と PDF ページ (`PdfPage`) でも同じ。
- [x] 動画 native フルスクリーンで Ctrl+↑↓ が画像と同じコンテキスト移動になる。
- [x] native 動画のマウス戻る / 進むも Ctrl+↑↓ と同じ App 経路に入る。
- [x] 動画 S タイルモード中も Ctrl+↑↓ が効き、移動先が動画ならタイルモードを維持し、画像 / ZIP / PDF ならタイルを閉じる。
- [x] タイルを一度閉じた後、次の動画移動で暗黙にタイルへ復帰しない。
- [x] Ctrl+F フィルタ中は、同一一覧移動はフィルタ後の `visible_indices`、Ctrl+↑↓ は no-op。
- [x] Ctrl+S で結果を開いた後、グリッド / フルスクリーン / 動画の Ctrl+↑↓ が `nav_stack` スコープを維持する。
- [x] Ctrl+G DrilledInto で、グリッド / フルスクリーン / 動画の Ctrl+↑↓ が検索結果スコープを維持する。
- [x] Ctrl+G Aggregated / Ctrl+S 結果一覧では no-op でもよいが、必要なら案内を出す。
- [x] Ctrl+PageUp/PageDown は通常フォルダで兄弟だけを移動し、空フォルダも skip せず、子や祖先の兄弟へ入らない。検索中は no-op。
- [x] 検索バー / フルスクリーン / タイルなど scope が変わる操作では pending folder navigation を flush する。
- [x] 境界文言は画像だけでなく動画も含む表現にする。
- [x] native overlay はまず toast MVP とし、構造的な中央表示は別段階にする。
- [ ] 画像系の `FsBoundaryHint` 文言変更は snapshot で固定し、native 動画は実機 sweep 手順を残す。
- [x] `docs/keymap-spec.md`、`docs/spec.md`、必要ならユーザー向け manual を同時更新する。

## 6. 実装メモ

- フルスクリーンの Ctrl+↑↓ は `handle_fullscreen_ctrl_nav_context` に集約し、
  画像系 / native 動画 / Ctrl+S / Ctrl+G が同じ優先順位を使う。
- `FolderNavMode::Favsearch { fullscreen }` で Ctrl+S のグリッド移動と
  フルスクリーン維持を分ける。
- native 動画の XButton は Win32 / HUD で `Extra1/Extra2` に変換済みなので、
  App 側 handler で Ctrl+↑↓ と同じ経路に送る。
- マウス進む/戻るボタンは、ハードウェアやドライバの設定によって以下の 3 経路の
  いずれでも届きうる。mIV はすべて同じフォルダ DFS ナビへ集約する。
  1. **WM_XBUTTONDOWN** (native 5 ボタンマウス標準): winit → egui の
     `PointerButton::Extra1/Extra2` として届く。`handle_keyboard` / `update_fullscreen` /
     native video `handle_native_video_mouse_button` が直接 bind 済み。
  2. **WM_APPCOMMAND** (mouse driver / Microsoft IntelliPoint 系が APPCOMMAND_BROWSER_BACKWARD/FORWARD を送る経路、または `WM_XBUTTONUP` を未処理にした際に `DefWindowProc` が自動昇格): winit はハンドリングしないため、mIV が自前で拾う。
     - **メインウィンドウ**: `src/main.rs` の `install_mouse_nav_hook` が `WH_GETMESSAGE`
       フックで観測し、グローバル atomic `PENDING_MOUSE_NAV_BACK/FORWARD` に積む。
       `App::take_pending_mouse_nav` を介して `handle_keyboard` / `update_fullscreen` が
       消費し、既存の `ctrl_up/down` 経路に OR 合成。
     - **native 動画 HWND**: `src/video/native_window.rs` の wndproc が `WM_APPCOMMAND`
       を `NativeVideoWindowEvent::KeyDown(vk=0xA6 or 0xA7)` に変換して event_tx に流す。
  3. **WM_KEYDOWN VK_BROWSER_BACK (0xA6) / VK_BROWSER_FORWARD (0xA7)** (mouse driver や
     AutoHotkey がキーストローク化して送る経路): egui-winit は `Key::BrowserBack` のみ
     翻訳し `BrowserForward` は drop するため、mIV は (2) と同じ `WH_GETMESSAGE` フック /
     native video wndproc の前段で VK を直接拾って同じ atomic / 合成 KeyDown 経路に流す。

  この多経路サポートは、Chrome / Edge / Explorer が「どの経路でも進む/戻る」ができる
  ユーザー体験と揃えるための実装。実機検証は本書 §7 の手動 sweep に含まれる。
- 動画タイルは動画→動画なら `video_tile_mode_active` を維持したまま source 差し替え
  または reopen pending で復元し、`video_tile_state` が一時的に無くなってもモードは落とさない。
  画像 / ZIP / PDF へ出たら閉じる。閉じた後の暗黙復帰はしない。
- タイル上の mouse hover はタイルカーソルに同期しない。S タイル中の P はキーボードで
  強調表示しているタイルを代表フレームとしてピン留めし、マウス操作はタイルクリックだけが
  seek として反応する。
- native overlay の `FsBoundaryHint` 相当中央表示は未実装。必要になったら
  toast MVP から分離して実装する。

## 7. テスト方針

- 画像 / ZIP / PDF の境界ヒント文言は `egui_kittest` snapshot で固定する。
- native 動画 fullscreen / S タイル / XButton は snapshot だけでは検証できないため、
  手動 sweep 手順を docs または E2E checklist に追加する。
- Ctrl+F active 中の Ctrl+↑↓ no-op は、検索バー focus 有無の両方で確認する。
- Ctrl+S / Ctrl+G の scope 変更時は pending が残らないことを、可能なら App-level test
  または narrow unit test で確認する。

手動 sweep では最低限、次を確認する:

1. Ctrl+F 絞り込み中に動画を fullscreen で開いて Ctrl+↓ を押すと、移動せず toast で理由が分かる。
2. 動画 S タイル中に Ctrl+↑↓ で動画 → 画像 → 動画と渡っても、一度閉じたタイルが暗黙復帰しない。
3. Ctrl+S 結果から動画を開いて XButton1 / XButton2 を押すと、favsearch スコープ内を移動する。
4. Ctrl+F バーが開いたまま画像を fullscreen で開いて Ctrl+↓ を押すと、中央に「Ctrl+F検索中はフォルダ移動しません」が出る。
5. Ctrl+S / Ctrl+G の結果一覧から fullscreen へ入った状態で Ctrl+↓ を押すと、中央に「検索結果を開いてからCtrl+↑↓で移動できます」が出る。
