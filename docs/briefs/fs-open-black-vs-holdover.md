# 開くときは黒地、切り替えるときは前の画像 — 待ちの見せ方を 1 つの規則にする

正本: [fs-page-wait-indicator.md](fs-page-wait-indicator.md) と
[fs-page-wait-indicator-fix.md](fs-page-wait-indicator-fix.md)。
**[fs-open-wait-indicator.md](fs-open-wait-indicator.md) は前提を誤っていたので破棄し、本書で置き換える。**

## 0. 破棄した版の誤り (記録)

前版は「列挙待ちの間は黒地が描かれているので、そこへ中央表示を足すだけ」と書いた。**誤り。**
黒地を描く分岐は `native_video_in_window_active` で gate されている
([ui_fullscreen.rs:12966](../../src/ui_fullscreen.rs:12966))。通常の PDF ダブルクリックでは成立せず、
**グリッドが表示されたまま時間が過ぎ、その後フルスクリーンへ移る**。
コメントだけ読んで gate を見落とした。**表示だけの追加では済まず、開く順序の変更が要る。**

## 1. 確定した規則 (利用者判断 2026-08-20)

**表示先に既に中身があるか**で分ける。

| 分類 | 状況 | 見せ方 |
| --- | --- | --- |
| **A: 表示先に中身が無い** | 複数ウィンドウで新しいウィンドウが開く / F12 デタッチで新しいウィンドウへ表示 / 同一ウィンドウ内でファイルを開く (グリッド → フルスクリーン) | **黒地を挟んでフルスクリーンへ移る。**開く操作の結果が「切り替わった / 窓が出た」という形で即座に返る |
| **B: 表示先に既に中身がある** | <kbd>Ctrl</kbd>+<kbd>↑</kbd><kbd>↓</kbd> / フル機能 + F12 デタッチでメイン側から操作して切り替え / 既にあるウィンドウを対象にする操作 | **前の画像を保持する。**黒を挟むとちらつく |

どちらの場合も、**待ちが 500ms を超えたら中央に「読み込み中」**を出す (実装済み)。

**この規則は既存の state でそのまま表現できる。** navigation sequence は
`previous: Option<FsDisplayUnitHoldover>` を持つ ([app.rs:7008](../../src/app.rs:7008))。
**`None` = 保持する中身が無い = A、`Some` = B。**新しい分類フラグを作らない。

## 2. やること

### 2.1 まず経路を列挙する (実装前)

**フルスクリーンに中身が現れるまでの経路をすべて挙げ、A / B に分類してから実装する。**
少なくとも: グリッドからの open (通常 / PDF / ZIP の列挙 defer あり)、Enter、ダブルクリック、
再クリック open、F12 デタッチ (新規窓 / 既存窓)、複数ウィンドウの新規窓、
<kbd>Ctrl</kbd>+<kbd>↑</kbd><kbd>↓</kbd> のフォルダ移動、スライドショー、履歴からの復帰。

**分類結果を報告に含めること。**今日、同じ状態に別経路から到達することを 3 回見落としている
(昇格が枠を取らなかった / 中央表示を pending に紐づけた / 開く経路に届いていなかった)。
**経路の列挙を先にやる。**

### 2.2 A の経路で、列挙を待たずにフルスクリーンへ入る

現在は PDF / ZIP のページ列挙が終わるまでフルスクリーンへ入らず、**グリッドのまま待つ**。
これを、**開く操作の時点でフルスクリーンへ移り、黒地を出す**ように変える。

- 速く開ける場合は黒が一瞬挟まる。**これは許容する** (利用者判断)。
  「ダブルクリックしたら必ずフルスクリーンに移る」ほうが予測しやすい。
- 500ms を超えたら、その黒地の中央に既存の「読み込み中」が出る (実装済みの描画を使う)。

### 2.3 B の経路は現状維持

前の画像を保持する現在の挙動を変えない。中央表示は実装済みのものが既に効いている。

## 3. 制約

- **新しい分類フラグを作らない。** `previous.is_none()` から導く。
  A / B を表す bool を App に足すのは禁止 (憲法 §2 規則 3)。
- 中央表示の**描画を複製しない**。既存のものを使う。
- `request_repaint` / `request_repaint_after` を新しく追加しない。
- 500ms は提示の判断だけに使う。競合判定に使わない。
- **detached / viewport 経路に触れる場合**は [detached-rework-plan.md](../detached-rework-plan.md) §2 を
  読み、症状パッチでないことを確認して §11 に記録する。

## 4. テスト

- A: グリッドから PDF を開く → **列挙完了を待たずにフルスクリーンへ入る**。
- A: 500ms 超で中央表示が出る。500ms 未満なら一度も出ない。
- B: <kbd>Ctrl</kbd>+<kbd>↑</kbd><kbd>↓</kbd> で**前の画像が保持され、黒が挟まらない**。
- B: 500ms 超で中央表示が出る (既存テストが担保)。
- A と B の中央表示が**同時に出ない**。
- 分類が `previous.is_none()` から導かれていること (新しいフラグを参照していない)。

---

# 第 1 段階の結果 (2026-08-20) — **仮説は否決。実装は見送り**

経路を列挙・分類した結果、**§1 の「`previous.is_none()` で A / B を導く」は成立しない。**

## なぜ成立しないか

`FsNavigationSequence::previous` が表すのは「**直前の egui 表示単位を `TextureHandle` として
捕捉できたか**」であって、**表示先が空かどうかではない**。

1. **A の production 経路のほとんどに navigation sequence が無い。**共通の
   [open_fullscreen](../../src/app.rs:43378) は sequence を作らない。
   名前に反して [open_fullscreen_from_fs_navigation](../../src/ui_fullscreen.rs:23039) も作らない。
   sequence を作る入口は実質 `begin_fs_folder_navigation_sequence` と
   `begin_fs_page_navigation_sequence` の 2 つだけ。
2. **B の多くにも sequence が無い。**Home/End・大ジャンプ・シークバー、スタックの Shift+↑↓、
   ブックマークジャンプ、連結読みのシーク / 再アンカー、スライドショー、native 動画・音声の
   前後移動、passive snapshot のクリック復帰、remote 解放後の復元、削除後の詰め、
   同一ページの補正 / AI reload — いずれも sequence を通らない。
3. **sequence がある B でも `previous` は `None` になり得る。**capture の成否なので、
   native presenter (egui texture ではない) や捕捉可能な rendition が無い場合に落ちる。
4. `begin_fs_page_navigation_sequence` は連結読みと native media target では
   **意図的に sequence を作らず成功扱いで戻る**。

## 決定的な反例

**グリッド由来で `from_explicit_open == true` なのに B** —
linked detached の既存窓を更新する場合。`fs_open_intent_from_grid` も
`DeferredFsReopen.from_explicit_open` も `previous.is_none()` も、**単独では A を意味しない**。

## 正しい判定地点

teardown 後の `open_fullscreen` ではなく、**表示先の surface / context を選ぶ routing 境界**。
そこで `ViewerPresentation`、viewer session、detached runtime state、mounted bundle の
`fullscreen_idx`、native presenter の owner を見る必要がある。

**`previous` だけへ寄せる案は構造的でなく、sequence を持たない同値経路を再び取りこぼす。**
Codex の判定: 「`previous.is_none()` だけを viewport 側へ足す実装は、
[detached-rework-plan.md](../detached-rework-plan.md) §2 の意味で症状パッチになる」。

## 分類の要約 (詳細は上記の調査記録)

- **A (表示先が空)**: グリッドからの open 全般 (Enter / ダブルクリック / 再クリック / ゲームパッド)、
  スタック集約セルからのフラット open、読書履歴 / ブックマーク / 検索結果 / ★固定からの open、
  画像のみフォルダの自動 fullscreen、ZIP/PDF/変換アーカイブの自動 fullscreen (列挙 defer あり)、
  起動引数 / SendTo / 二重起動 activation (新規表示先の場合)、always-new / 複数ウィンドウの新規窓、
  新規 independent detached への移送、F12 で空の surface へ移す、F11 の embedded ↔ viewport 切替。
- **B (表示先に中身がある)**: linked detached の既存窓更新、Parked linked の再利用、
  通常のページ送り (sequence あり)、Home/End / 大ジャンプ / シークバー、スタック Shift+↑↓、
  ブックマークジャンプ、連結読みのシーク、Ctrl+↑↓ のフォルダ移動 (`FolderItems` sequence あり)、
  independent detached の Ctrl+↑↓ (legacy `FolderNavigation` holdover)、スライドショー各種、
  native 動画・音声の前後 / EOF / swap、passive snapshot クリック、remote 解放後の復元、
  外部 activation での更新、削除後の詰め、同一ページの補正 / AI reload。

## 結論

**規則そのもの (表示先に中身があるか) は正しい。導出元が間違っていた。**
正しく実装するには routing 境界に typed な判定を置く必要があり、それは detached リワーク
**R2 (状態の集約)** が所有する領域と重なる。**独立した作業として今やらず、バックログへ積む。**

今日入れた中央の「読み込み中」により、**無反応に見える問題自体は解消済み**。
残るのは「グリッドが見えたまま待つ (黒地にならない)」という見た目の差だけ。
