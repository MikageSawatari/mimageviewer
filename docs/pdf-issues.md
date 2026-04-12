# PDF サポート — 未解決の問題 (2026-04-12)

## 概要

PDF サポートの基本機能 (サムネイル表示・フルスクリーン・ナビゲーション・パスワード・キャッシュ) は
動作しているが、以下の 2 つの問題が未解決。

---

## 問題 1: フルスクリーンでの PDF ズーム再レンダリングが反映されない

### 症状
- Z キーで分析モードに入り、マウスホイールで拡大すると、画像が荒いまま (GPU スケーリングのみ)
- 左下に「PDF 再レンダリング中...」が表示されるが、完了後もテクスチャが差し替わらない
- 高解像度でレンダリングされた画像が実際に fs_cache に反映されていない

### 現状のフロー
```
ズーム変更 (ui_fullscreen.rs)
  → request_pdf_rerender() (app.rs ~L2280)
    → render_page_async() でワーカーにリクエスト
    → std::thread::spawn で軽量スレッドがワーカー応答待ち
    → 応答受信 → DynamicImage → ColorImage → FsLoadResult::Static(ci) を fs_tx で送信
    → fs_pending に (cancel, fs_rx) が登録済み

poll_prefetch() (app.rs ~L2430)
  → fs_pending の各エントリに try_recv()
  → FsLoadResult::Static を受信 → ctx.load_texture → fs_cache に insert
```

### 調査ポイント
1. **poll_prefetch がフルスクリーン中に呼ばれているか？**
   - `render_fullscreen_viewport()` の先頭で `self.poll_prefetch(ctx)` を呼ぶコードは
     追加済み (ui_fullscreen.rs ~L84)
   - ただし `show_viewport_immediate` 内の `ctx` はフルスクリーンビューポートのもの。
     テクスチャは共有されるはずだが、repaint の伝播が正しいか要確認
   
2. **fs_pending のエントリが正しく登録されているか？**
   - `request_pdf_rerender` で `fs_pending.insert(idx, (cancel, fs_rx))` している
   - `fs_rx` は `mpsc::channel::<FsLoadResult>()` で作成
   - spawn したスレッドは `render_rx.recv()` (ワーカー応答待ち) → `fs_tx.send()` の流れ
   - **問題の可能性**: spawn スレッドが `render_rx.recv()` でブロックされたまま、
     ワーカーが他のリクエスト (高画質化) を処理中で、応答が来ない
   
3. **ワーカーのキュー順序**
   - ワーカーは FIFO でリクエストを処理する
   - サムネイル高画質化 (idle upgrade) のリクエストがキューに大量に入っていると、
     ズーム再レンダリングが後回しにされる
   - **対策案**: ワーカーに優先度キューを導入するか、高画質化のキャンセル機構を活用

4. **デバッグ方法**
   - `--log` 付きで起動し、ズーム操作後のログを確認:
     - `pdf rerender done:` が出力されるか (spawn スレッドがワーカー応答を受信できたか)
     - `poll_thumbnails` で rerender 結果が拾われているか
   - `fs_pending` のエントリ数をログに出力して、正しく登録・消費されているか確認

---

## 問題 2: Ctrl+上下フォルダ移動時のブロッキング

### 症状
- 左下に「高画質化」と表示されている最中に Ctrl+上下を押すと、
  約 1 ページのレンダリング分 (100-1200ms) UI がフリーズする
- 高画質化が走っていないときは即座に切り替わる

### 現状のアーキテクチャ
```
pdf-worker スレッド (1本): Pdfium インスタンスを独占所有
  ← mpsc チャネルでリクエスト受信
  → 1件ずつシリアルに処理

サムネイルワーカー (12本):
  → render_page() (ブロッキング版) でワーカーにリクエスト → .recv() で待ち

UI スレッド:
  → load_pdf_as_folder() → enumerate_pages_async() → try_recv() でポーリング (非同期化済み)
```

### 非同期化の状況
- `load_pdf_as_folder` は `enumerate_pages_async` + `poll_pdf_enumerate` で非同期化済み
- しかし **ブロッキングが残っている可能性のある箇所**:

1. **check_password_needed()** — パスワードダイアログ表示判定に使用
   - 現在は `load_pdf_as_folder` から削除し、enumerate の結果でエラー判定する方式に変更済み
   - ただし `show_pdf_password_dialog_window` (パスワード検証時) では同期版を使用

2. **サムネイルワーカーの render_page()** — ワーカーが高画質化レンダリング中の場合:
   - UI が Ctrl+上下 → `load_pdf_as_folder` → `enumerate_pages_async()` → ワーカーにリクエスト送信 → 即 return
   - ワーカーは現在処理中のレンダリングを完了してからenumerateを処理
   - UI は `poll_pdf_enumerate` で `try_recv()` するが、ワーカーが忙しい間は Empty が返る
   - **ここまでは UI はブロックしない**
   
   - ただし `start_loading_items()` (app.rs ~L810) が呼ばれるのは enumerate の結果受信後
   - その間に `cancel_token` を設定して旧ワーカーを止める処理も遅れる
   - **旧サムネイルワーカーが依然として render_page() でワーカーキューを埋めている**
   
3. **本当のブロック箇所の特定が必要**
   - `--log` でブロック前後のタイムスタンプを記録して特定すべき
   - `load_folder` → `load_pdf_as_folder` の入り口にタイムスタンプを入れ、
     `poll_pdf_enumerate` で結果受信時のタイムスタンプと比較

### 対策案

#### A. ワーカーキューのドレイン
- フォルダ切替時に、ワーカーの pending リクエストを一括キャンセルする仕組み
- 現在のキャンセルトークンは `Render` リクエストの `cancel` フィールドだが、
  サムネイルワーカーから送られるリクエストには cancel が `None` で渡されている
- **修正**: サムネイルワーカーからの `render_page()` にも cancel トークンを渡す

#### B. Enumerate/CheckPassword の優先処理
- ワーカーに 2 つのチャネルを持たせる: 高優先 (Enumerate/CheckPassword) + 通常 (Render)
- ワーカーは Render 処理後に高優先チャネルを先にチェックし、あれば優先処理
- Render 中は割り込めないが、次のリクエスト開始前に Enumerate が割り込める

#### C. Enumerate 専用の軽量 PDF パース
- PDFium を使わずに PDF のページ数だけを取得する関数を実装
- PDF ファイルのヘッダ/トレイラーを直接パースして `/Count` を読む
- ワーカーを経由しないため一切ブロックしない
- ただし実装コストが高く、パスワード付き PDF には対応できない

---

## 関連ファイル

| ファイル | 役割 |
|---------|------|
| `src/pdf_loader.rs` | PdfWorker (専用スレッド) + 同期/非同期 API |
| `src/app.rs` | `load_pdf_as_folder`, `request_pdf_rerender`, `poll_pdf_enumerate`, `poll_prefetch` |
| `src/ui_fullscreen.rs` | ズーム操作 → `request_pdf_rerender` 呼び出し、進捗表示、`poll_prefetch` 呼び出し |
| `src/thumb_loader.rs` | `load_one_cached` の PDF 分岐 → `render_page()` (ブロッキング) |
| `src/folder_tree.rs` | `folder_has_images()` で PDF は楽観判定 (`return true`) |

## デバッグ手順

1. `cargo build --release` → `mimageviewer.exe --log` で起動
2. PDF フォルダを開き、サムネイルが表示されるまで待つ
3. Z キーで分析モード → マウスホイールで拡大 → ログで `pdf rerender done:` を確認
4. Ctrl+下で次の PDF に移動 → フリーズ時間をログのタイムスタンプから計測
5. `mimageviewer.log` を確認して報告
