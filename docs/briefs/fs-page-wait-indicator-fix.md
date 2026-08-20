# 中央「読み込み中」が出なかった件の修正 (時間の測り方が誤り)

正本: [fs-page-wait-indicator.md](fs-page-wait-indicator.md)。本書はその**訂正**。

## 1. 何が起きたか (実測)

利用者環境で **18 秒間ページが変わらなかったのに、中央表示が出なかった。**

perf log (2026-08-20):

- `fs/navigation_target_phase` が **t=11.6 から 29.7 まで 13,888 件、`materialized_ready: false`**。
  対象はページ **#1**。つまり **navigation sequence は 18 秒間ずっと存在し、retire されていなかった**。
- 同じ 18 秒間に pending だった `fs/load_begin` は **#3, #5〜#13** (すべて `Normal` = 先読み)。
  **対象ページ #1 自身の Display 読み込みは pending に居なかった。**
- frame は途切れていない (最大 0.49 秒)。**アプリは固まっておらず、絵が変わらなかっただけ。**

## 2. 原因 — 正本の指定が誤っていた

正本 §3 に「経過時間は**その pending 自身の開始時刻から導く**」と書いた。この指定のせいで
述語が **「対象ページに pending な Display 読み込みがあること」を要求**する形になった
([ui_fullscreen.rs:1295](../../src/ui_fullscreen.rs:1295))。

**しかし「まだ出ていない」ことと「読み込みが pending であること」は同じではない。**
対象ページの読み込みが既に完了していても、GPU アップロード待ちや提示側の都合で
materialize されていない間は「まだ出ていない」。この場合 pending は空になり、**指標が黙る**。

これは実装の誤りではなく、**指定の誤り**である。

## 3. 正しい測り方

**navigation sequence が retire されずに存在し続けている時間を測る。**

- `FsNavigationSequence` ([app.rs:7006](../../src/app.rs:7006)) に **`opened_at: Instant` を追加**し、
  構築時に入れる。**App に新しいフィールドを足すのではなく、既存の typed state に持たせる**
  (憲法 §2 規則 3 が求めている形)。
- 述語から **`pending_display_loads` の要求を外す**。sequence が対象 idx / generation を持ち、
  `now - opened_at >= 500ms` なら表示する。
- retire された frame で消えるのは従来どおり (sequence が消えるため自動的にそうなる)。

### なぜ pending を条件から外してよいか

sequence は**対象ページ全部が live provenance で描かれて初めて retire される**。
つまり sequence の存在そのものが「頼んだページはまだ出ていない」を意味する。
**なぜ出ていないか (読み込み中 / アップロード待ち / その他) を指標が区別する必要はない。**

### 副作用として受け入れること

sequence が何らかの理由で retire されない不具合があれば、中央表示は**出続ける**。
これは正しい。何かが実際に詰まっているのだから、**黙るより出続けるほうがよい**。

## 4. 変えないこと

- 既存の左上 (`高解像度 読込中...`) と左下 (`PDF 再レンダリング中...`) は触らない。
- 高解像度化のときに出ないこと (sequence が無いため) は維持する。既存テストで固定済み。
- `request_repaint` / `request_repaint_after` を追加しない。
- 500ms は提示の判断だけに使う。競合判定に使わない。

## 5. テスト

- **sequence が存在し 500ms 超だが、対象ページの pending 読み込みが無い → 出る**
  (今回の実測ケース。現行では出ない)。
- sequence 存在・500ms 未満 → 出ない。
- retire された frame で消える。
- 見開きで片方だけ未提示 → 出る。
- 高解像度化 (sequence 無し) → 出ない。
- `fullscreen_processing_status_visible` が false → 出ない。
