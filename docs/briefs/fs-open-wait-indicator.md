# 開く途中の待ちにも、同じ中央表示を出す

正本: [fs-page-wait-indicator.md](fs-page-wait-indicator.md) と
[fs-page-wait-indicator-fix.md](fs-page-wait-indicator-fix.md)。本書はその**適用範囲の拡張**。

## 1. 症状

利用者報告 (2026-08-20): **一覧で PDF をダブルクリックして開くとき、1 ページ目の読み込みが遅いと
無反応に見える。**しばらくしてから開く。

## 2. なぜ中央表示が出ないか

PDF / ZIP はページ列挙 (enumerate) をワーカーで行い、**その間フルスクリーンへ入らない**。
`fullscreen_idx` が `None` のまま黒地だけを描き、[ui_fullscreen.rs:12958](../../src/ui_fullscreen.rs:12958)
の `render_fullscreen_viewport` は**冒頭で return する**。

先に入れた中央表示は **navigation sequence の存在**を条件にしている。この段階では
まだフルスクリーンに入っておらず sequence が無いので、判定に到達しない。

**「待っている」という事実は同じなのに、経路が違うだけで黙っている。**
[fs-page-wait-indicator-fix.md](fs-page-wait-indicator-fix.md) で直したのと**同型の見落とし**である。

## 3. やること

**開く途中の待ちも、同じ中央表示で見せる。**

- 条件: **deferred な fullscreen open が in-flight** (`fs_nav_after_pdf_enumerate` /
  `fs_nav_deferred_reopen_wait_active` が示す既存の typed state) で、**500ms を超えたとき**。
- 経過時間は **`DeferredFsReopen` 自身に開始時刻を持たせて**測る。
  `FsNavigationSequence::opened_at` と同じ形。**App に新しいフィールドを足さない**。
- **描画は既存の中央表示と同じものを使う。**別の見た目を作らない。利用者が覚える合図は 1 つ。
  黒地の上に出る。
- 列挙が終わってフルスクリーンへ入ったら、そこからは**既存の sequence 条件が引き継ぐ**。
  **2 つが同時に出ない**ことを保証する (どちらか一方だけ)。

## 4. 変えないこと

- 列挙・開く処理そのものの判定を変えない。**提示だけの追加。**
- 既存の左上 / 左下の表示を触らない。
- `request_repaint` / `request_repaint_after` を**新しく追加しない**。この経路は既に
  `request_repaint_after(16ms)` で回っている ([ui_fullscreen.rs:12955](../../src/ui_fullscreen.rs:12955))
  ので、そのフレームに相乗りする。
- 500ms は提示の判断だけに使う。競合判定に使わない。

## 5. テスト

- deferred open が 500ms 未満 → 出ない。
- 500ms 超 → 出る。
- 列挙完了でフルスクリーンへ入った後は、**sequence 側の条件へ引き継がれ、二重に出ない**。
- `fullscreen_processing_status_visible` が false → 出ない。
- 開くのが速い (500ms 未満で完了) 場合、**一度も出ない**。
