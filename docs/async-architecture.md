# 非同期・並列アーキテクチャ

「どの処理がどのスレッド/プロセスで走るか」「どうやってキャンセルするか」「キャッシュ競合をどう避けるか」
の一覧。並列処理を追加・変更するときの設計テンプレートとして使う。

---

## 1. ワーカー一覧

| ワーカー | 実装 | 個数 | 用途 |
| --- | --- | --- | --- |
| サムネイル (通常) | `std::thread` + mpsc | `parallelism - 重I/O` | Image / ZipImage / PdfPage の軽いデコード |
| サムネイル (重 I/O) | `std::thread` + mpsc | 1〜2 (総数 ≤4 なら 1) | Folder / ZipFile / PdfFile の全体走査 |
| フルスクリーンロード | `std::thread` (使い捨て) | 1 枚ごとに spawn | フルサイズ画像デコード + アニメ展開 |
| PDF ワーカー | **別プロセス** (`--pdf-worker`) | 3 (`PDF_WORKERS`) | PDFium は非スレッドセーフ → マルチプロセスで並列化 |
| PDF ページ列挙 | `std::thread` | 1 (PDF 開く都度) | PDF ワーカーに列挙要求を送る |
| AI 推論 | `std::thread` + mpsc | 1 (全モデル共通) | ort (DirectML) の upscale/denoise/inpaint |
| 動画サムネイル | `std::thread` | 1 | Windows Shell API を逐次呼び出し |
| フォルダナビゲーション | `std::thread` | 1 (Ctrl+↑↓ 都度) | 深さ優先で次フォルダを検索 |
| キャッシュ一括生成 | `rayon` | (ユーザー設定) | ダイアログから起動するバッチ処理 |

**rayon は通常サムネイル生成には使っていない** (逐次ワーカーの方がキャンセル制御しやすいため)。

---

## 2. スレッド間通信

### 2.1 共有アトミック

| 名前 | 型 | 書き手 | 読み手 | 用途 |
| --- | --- | --- | --- | --- |
| `cancel_token` | `Arc<AtomicBool>` | UI (フォルダ切替) | 全ワーカー | 停止シグナル |
| `scroll_hint` | `Arc<AtomicUsize>` | UI (スクロール) | サムネワーカー | 優先度計算の基準 |
| `keep_start_shared` / `keep_end_shared` | `Arc<AtomicUsize>` | UI | サムネワーカー | 範囲外の要求を破棄する境界 |
| `visible_end_shared` | `Arc<AtomicUsize>` | UI | サムネワーカー | 可視範囲の終端 (exclusive)。先読み forward 側の距離計算に使用 |
| `display_px_shared` | `Arc<AtomicU32>` | UI (設定変更) | サムネワーカー | 生成時の目標ピクセル数 |
| `cache_gen_done` | `Arc<AtomicUsize>` | キャッシュ生成 rayon | UI | 進捗カウンタ |

**ルール**: アトミックは単発の値伝搬にのみ使う。リスト/辞書の共有は `Arc<Mutex<...>>` か mpsc。

### 2.2 チャネル

| 名前 | 方向 | 内容 |
| --- | --- | --- |
| `tx / rx` (App) | ワーカー → UI | `ThumbMsg`: (idx, ColorImage, from_cache, source_dims) |
| `fs_pending[idx].1` | フルスクリーンスレッド → UI | `FsLoadResult`: Static / Animated / Failed |
| `ai_upscale_pending[idx].1` | AI スレッド → UI | `UpscaleResult` |
| `pdf_enumerate_pending` | PDF 列挙スレッド → UI | `(pages, password_needed)` |
| PDF ワーカー stdin/stdout | UI プロセス ↔ PDF ワーカープロセス | 長さプレフィクス付きバイナリプロトコル (Enumerate / Render / Shutdown) |

### 2.3 ワーカーキュー

| キュー | 型 | 内容 |
| --- | --- | --- |
| `reload_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | 通常サムネイル要求 |
| `heavy_io_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | Folder/ZipFile/PdfFile 要求 |
| `texture_backlog` | ローカル Vec (App) | GPU アップロード未完の ColorImage。MAX_TEXTURES_PER_FRAME=8 超過分 |

ワーカーが要求を取り出すときは **優先度 (priority フラグ) → 距離 → forward/backward** でソート。
距離計算は可視範囲の端からの歩数: backward は `scroll_hint - idx`, forward は `idx - visible_end + 1`
で、同距離では forward (次ページ方向) が先。これは `fs_cache` 先読み / AI アップスケール先読み /
サムネイルグリッドワーカーの全てで統一されており、`+1, -1, +2, -2, ...` の順 (forward 先) となる
(共通ヘルパ: `interleaved_prefetch_targets`)。

---

## 3. キャンセル規約

### 3.1 フォルダ切替時

`load_folder()` が呼ばれたら:

1. 旧 `cancel_token` に `true` をセット
2. 新しい `cancel_token` を作って `Arc` を差し替え
3. 旧 mpsc 受信は drop (新しい tx/rx に置き換え)
4. 新しいワーカーを新トークン付きで spawn
5. 各種キャッシュ (`fs_cache`, `adjustment_cache`, `ai_upscale_cache`, `rotation_cache` …) をクリア

**旧プールを毎回捨てる**のが肝。同じプールを使い回さないので競合を気にしなくてよい。

### 3.2 フルスクリーン / AI のキャンセル

1 枚ごとに `Arc<AtomicBool>` を `fs_pending[idx]` / `ai_upscale_pending[idx]` に持たせる。
要求を取り下げるときは個別にこのフラグを立てる。
ワーカーは大きな処理の合間 (タイル推論の各タイル、フレームデコード直後、など) でフラグを確認する。

### 3.3 フルスクリーン読み込みの優先度制御

`start_fs_load` はプールを持たない使い捨て `std::thread::spawn` なので、素朴に先読みを
並列起動すると現在表示中の画像のデコードが先読みスレッドに CPU を奪われて遅延する。
これを防ぐため `update_prefetch_window` は以下のルールで動く:

1. 現在画像が `fs_cache` に入っていない (デコード中) 間は、**他の全ての pending スレッドを
   キャンセル**する (KEEP 範囲内でも)。現在画像が CPU を独占する。
2. 同時に、先読みの新規 spawn も **延期**する。
3. `poll_prefetch` が現在画像の完了を検出したら、再度 `update_prefetch_window` を呼び、
   そこで初めて先読みが起動する。

AI アップスケール (`maybe_start_ai_upscale`) も同様: 同時実行は 1 枚のみで、現在画像が
来たら古い先読みをキャンセル。

### 3.3 新ワーカー追加時のテンプレ

```rust
let cancel = Arc::clone(&self.cancel_token);  // フォルダ単位のキャンセル
let my_cancel = Arc::new(AtomicBool::new(false));  // 個別キャンセル (必要なら)
let tx = self.tx.clone();
std::thread::spawn(move || {
    // 大きな処理の合間で両方チェック
    if cancel.load(Relaxed) || my_cancel.load(Relaxed) {
        return;
    }
    // ... 処理 ...
    let _ = tx.send(result);
});
```

送信失敗 (受信側 drop) は無視する。フォルダ切替で既に捨てられているだけ。

---

## 4. GPU テクスチャ予算

### 4.1 keep_range ベースの退去

- 可視範囲 + prev/next ページ分のみ GPU に保持
- 範囲外に出た瞬間に `TextureHandle` を drop
- `egui_ctx.load_texture` でアップロードするコマ数を MAX_TEXTURES_PER_FRAME=8 に制限 (フレームレート維持)
- 超過分は `texture_backlog` に積んで次フレーム以降に処理

### 4.2 VRAM キャップ

- `gpu_info.rs` で取得した VRAM 量から動的にテクスチャ上限バイト数を決定
- 超過しそうなら keep_range を両端から狭める (古い側から evict)

新しいテクスチャキャッシュ (例: 将来の補正 LUT プレビュー) を追加する時は、
この退去ロジックにも登録すること。

---

## 5. よくある事故パターン

### 5.1 キャンセル忘れ

新機能を作った時、`cancel_token` を参照し忘れると、フォルダ切替後もゾンビとして動き続ける。
→ 最悪 mpsc が満杯になるか、UI に古い結果が届く。必ずテンプレに従う。

### 5.2 キャッシュの部分更新

「補正は変わったけど AI は変わってない」のような時、`adjustment_cache` だけクリアして
`ai_upscale_cache` を残す。両方同時に消すと AI の再実行 (数秒) が発生してユーザーを待たせる。
詳細は [preset-and-adjustment.md](preset-and-adjustment.md) の無効化ルール表。

### 5.3 UI スレッドで重処理

`App::update` 内で CPU 重めの処理をすると fps が落ちる。
- 補正の LUT 計算: 軽いので同期 OK (`maybe_apply_adjustment`)
- AI 推論: 絶対に別スレッド
- 画像デコード: 絶対に別スレッド

### 5.4 PDF ワーカーの想定外終了

ワーカープロセスがクラッシュしたら、親は検出して再起動する仕組みになっている。
新しい PDF 操作を追加する時はタイムアウト処理を忘れずに (stdout 読み取りで詰まらない)。

---

## 6. 参考 (実測値)

`docs/bench-scroll-report.md` に詳細あり。要点:

- キャッシュヒット時のサムネ読み込み: 2〜3 ms/枚
- PDF レンダリング: 3 ワーカー並列で Cold 1441ms → 10ms (2 枚目以降)
- JPEG デコード: turbojpeg で 1.5〜2.4 倍高速化 (5MB 超は image crate にフォールバック)
- キャンセル遅延: 最大 1 枚デコード分 (数百 ms)
