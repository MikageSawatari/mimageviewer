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
| PDF ワーカー | **別プロセス** (`--pdf-worker`) + 各プロセス専用のディスパッチャースレッド | 3 (`POOL_SIZE`) | PDFium は非スレッドセーフ → マルチプロセスで並列化。要求は JobQueue に enqueue |
| PDF ページ列挙 | `std::thread` | 1 (PDF 開く都度) | PDF ワーカーに列挙要求を送る |
| AI 推論 | `std::thread` + mpsc | 1 (全モデル共通) | ort (DirectML) の upscale/denoise/inpaint |
| 動画サムネイル | `std::thread` | 1 | Windows Shell API を逐次呼び出し |
| フォルダナビゲーション | `std::thread` | 1 (常時 ≤ 1 本) | 深さ優先で次フォルダを検索。連打は `pending_folder_nav_steps` に累積され、完了ごとに連鎖実行する (並行 DFS による FS 競合を避ける) |
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
| `tx / rx` (App) | ワーカー → UI | `ThumbMsg`: (idx, ColorImage, from_cache, source_dims, canceled)。from-source 経路 (cache miss) では **2 シグナル**: ① 第 1 シグナル = display ColorImage (canceled=false) → UI は Loaded 化、`requested` は保持 ② 第 2 シグナル = cache save 完了通知 (None + canceled=true) → UI は `requested` を抜く。cache hit は 1 ショット (canceled=false で即 remove)。`canceled=true` は STALE でも送信され、その場合 state は Evicted (retriable、Failed にしない) |
| `fs_pending[idx].1` | フルスクリーンスレッド → UI | `FsLoadResult`: Static / Animated / Failed |
| `ai_upscale_pending[idx].1` | AI スレッド → UI | `UpscaleResult` |
| `pdf_enumerate_pending` | PDF 列挙スレッド → UI | `(pages, password_needed)` |
| PDF ワーカー stdin/stdout | UI プロセス ↔ PDF ワーカープロセス | 長さプレフィクス付きバイナリプロトコル (Enumerate / Render / Shutdown) |

### 2.3 ワーカーキュー

| キュー | 型 | 内容 |
| --- | --- | --- |
| `reload_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | 通常サムネイル要求 |
| `heavy_io_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | Folder/ZipFile/PdfFile 要求 |
| `pdf_pool.queue` | `Arc<(Mutex<JobQueue>, Condvar)>` | PDF ワーカーへのレンダ/列挙要求。`critical` / `normal` VecDeque + `normal_in_flight` + `workers_busy` を同一 Mutex で保護 |
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

### 3.1.5 フォルダナビゲーション (Ctrl+↑↓) のキャンセル + アキュームレート

Ctrl+↑/↓ はフォルダツリーを DFS で辿って次の「画像/動画/ZIP/PDF があるフォルダ」を
見つけるが、キーリピート (30Hz) で連打すると、過去は毎プレスで新スレッドを spawn +
旧スレッドに cancel を投げる設計だった。ただし `navigate_folder_with_skip` 自体は
cancel を見ていなかったので、cancel 済みスレッドも DFS を最後まで走り切り、
並行 DFS が FS を奪い合って単発 DFS が 200ms → 1s 級に遅延する事故を起こしていた
(2026-04 セッションで実測、PDF だらけの scan フォルダで顕著)。

現在の挙動 (2026-04 修正後):

- `navigate_folder_with_skip` と `folder_should_stop` は `Option<&AtomicBool>` を受け取り、
  各 DFS ステップとディレクトリエントリ走査のたびに cancel をチェックする。旧スレッドは
  cancel 検出時点で `None` を返して即終了 → FS 競合が消える。
- `start_folder_nav` は in-flight 中の追加プレスを `pending_folder_nav_steps: i32` に
  累積する (forward=+1 / backward=-1)。**新スレッドは spawn しない**。
  累積は `±MAX_PENDING_NAV = 5` で飽和する (それ以上のプレスは捨てる) ので、
  キーを離した後に「離したのに動き続ける」違和感が出ない (drain は最長 ~500ms)。
- 現 nav が完了 → `load_folder` → `chain_folder_nav_if_pending` で累積が残っていれば
  1 消費して次のステップ (新しい current からの DFS) を連鎖起動する。
- 連打中に別経路のナビ (click / favsearch / address / BS) が入ると累積はクリアされ、
  in-flight もキャンセルされる (`load_folder` → `start_loading_items` の既存処理)。

これにより 30 回連打は 30 ステップ分の DFS を逐次的に進める (並行ではなく直列)。
各 DFS 間で cancel チェックが入るので、途中で方向が反転しても即座に対応できる。

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

### 3.4 サムネイルワーカーの STALE 取消と重複エンキュー抑制

サムネイルは「keep_range 内かどうか」が毎フレーム変化するため、単純なキャンセルでは
**同じ idx が in-flight なのに scroll 戻りで再エンキューされ、PDF 再レンダが二重に走る**
事故を起こす。2026-04 のセッションで以下のルールを確立した:

- **`update_keep_range_and_requests` は `self.requested` を範囲外一括 remove しない**。
  ワーカー処理中の idx まで抜いてしまい再エンキューを誘発するため。step 1 は Loaded→Evicted
  の遷移だけ行う。
- **`requested` の cleanup 経路は 4 本**:
  1. エンキュー済・pop 前の取消 → step 2 の `q.retain` が dropped idx を `requested.remove`
  2. ワーカー pop 後の STALE → ワーカーが `ThumbMsg` に `canceled=true` を載せて送信 →
     `poll_thumbnails` が `requested.remove` + `Evicted` (Failed にしない)
  3. cache hit 正常完了 → 第 1 シグナルで `poll_thumbnails` が `requested.remove`
  4. cache miss 正常完了 → **第 1 シグナル (display ColorImage) では remove しない**、
     第 2 シグナル (cache save 完了、canceled=true) で remove。from_cache=false の判定で分岐
- **STALE チェックはワーカーパイプラインの 3 箇所**:
  - `spawn_worker` が pop 直後 (app.rs): キャッシュ lookup すら不要な明白な範囲外
  - `process_load_request` の heavy I/O resolve 後 (thumb_loader.rs): ZIP/folder の
    I/O (秒単位) 完了後に範囲外になっていないか
  - `process_load_request` の PDF レンダ直前 (thumb_loader.rs): cache miss で PDFium
    に投げる前。これがないと scroll 往復で同じページの 1 秒レンダが重複する
- 3 箇所とも `canceled=true` を送信して `requested` cleanup する。`continue` だけでは
  `requested` に残って「再エンキューされない idx=Pending」状態で固まる。

**なぜ 2 シグナル方式か**: `load_one_cached` は decode → tx.send (display) → WebP encode →
DB save → cache_map.insert の順で処理する。もし第 1 シグナル到着時に `requested` を抜くと、
cache save 進行中 (数百 ms) は `requested` 空かつ cache_map にも未登録の窓が開き、
その間に scroll 往復が起きると別 worker が同じ idx を cache miss 扱いで取得し重い decode
(ZIP 取り出し・PDFium レンダ等) を二重に走らせる。第 2 シグナルで cache save 完了後に
初めて `requested` を抜くことで、cache save 中の再エンキューは `requested.contains_key=true`
で弾かれる。

### 3.5 新ワーカー追加時のテンプレ

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

### 5.5 try_lock + sleep ポーリングループ (禁止パターン)

「`Mutex` を `try_lock` して、失敗したら sleep して再試行」というループは、**複数スレッドが
同じ Mutex を奪い合う場面では飢餓 (starvation) を起こす**。10ms の sleep 中に fresh arrival が
割り込んで Mutex を横取りできるため、先に待ち始めたスレッドが秒単位で待たされる。

2026-04 に PDF ワーカープールで実際にこの現象が発生し、Critical 要求が 10 秒ブロックされた
(1 ワーカーに 62 件の連続ディスパッチが集中、他の 2 ワーカーは完全にアイドル)。

**代わりに使うべき設計**: **Mutex + Condvar で保護した優先度キュー + 専用ディスパッチャー
スレッド**。

```rust
// リソース要求側 (UI スレッド等)
fn execute(&self, job: Job) -> Result<R> {
    let (tx, rx) = mpsc::channel();
    {
        let (mtx, cv) = &*self.queue;
        let mut q = mtx.lock().unwrap();
        q.push(job);           // critical / normal などにソート
        cv.notify_one();       // ディスパッチャーを 1 つ起こす
    }
    // タイムアウト付き受信で cancel チェックを挟む
    rx.recv_timeout(Duration::from_millis(50))
}

// ディスパッチャースレッド (ワーカーごとに 1 本)
fn dispatcher(queue: Arc<(Mutex<JobQueue>, Condvar)>, resource: Resource) {
    loop {
        let job = {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            loop {
                if q.shutdown { return; }
                if let Some(j) = q.pop_with_priority() { break j; }
                q = cv.wait(q).unwrap();    // Condvar で起床
            }
        };
        // Mutex 外でリソースを使って処理
        let result = resource.process(job);
        let _ = job.reply.send(result);
    }
}
```

**この設計の利点**:
- 同一優先度内で **FIFO 公平性** (Condvar が queue に並んだ順で起こす)
- 10ms ポーリングの無駄なスピン消費がなくレイテンシも低い
- ワーカー選択が「先に空いた方の勝ち」ではなく「空いた瞬間に push されたジョブを pop」になる
- `shutdown` フラグと `notify_all()` だけで停止シグナルが全スレッドに伝わる
- cancel は pop 時と requester 側 (`recv_timeout` ループ) の両方でチェック可能

実装は `src/pdf_loader.rs` の `PdfWorkerPool` / `JobQueue` / `run_dispatcher` を参照。

**いつ try_lock を使って良いか**: 非ブロッキングな best-effort 取得 (「取れたら使う、取れなければ
今回は諦める」) のみ。`try_lock` の後に sleep して再試行する構造は避ける。

---

## 6. 参考 (実測値)

`docs/bench-scroll-report.md` に詳細あり。要点:

- キャッシュヒット時のサムネ読み込み: 2〜3 ms/枚
- PDF レンダリング: 3 ワーカー並列で Cold 1441ms → 10ms (2 枚目以降)
- JPEG デコード: turbojpeg で 1.5〜2.4 倍高速化 (5MB 超は image crate にフォールバック)
- キャンセル遅延: 最大 1 枚デコード分 (数百 ms)

---

## 7. パフォーマンス計装 (perf.rs)

「キー入力 → 画面表示」レイテンシを後から解析するための構造化イベントログ。
既存 `logger.rs` (人間可読) はそのまま残り、`perf.rs` が JSON Lines を別ファイルに書く。

### 7.1 有効化

- **CLI 引数**: `mimageviewer.exe --perf-log` を付けたときのみ ON
- **無効時のコスト**: `perf::is_enabled()` の Atomic 1 回読みのみで `perf::event` は即 return
- **出力先**: `%APPDATA%\mimageviewer\logs\perf_events.jsonl` (起動毎に truncate)

### 7.2 `input_seq` の伝搬規約

`App` が `input_seq: u64` を持ち、**ユーザー入力イベント発生時のみ** `bump_input_seq()` で +1 する。
フレーム境界では増えない。0 は「相関なし」として予約。

| 発火箇所 | 種別 | 備考 |
| --- | --- | --- |
| `ui_fullscreen.rs::render_fullscreen_viewport` | `fs_key` / `fs_wheel` / `fs_close_*` | nav_delta / wheel_nav / close が確定した直後 |
| `app.rs::handle_keyboard` | `grid_key` | カーソルキーで selected が変わった時 |
| `app.rs::process_scroll` | `grid_wheel` / `grid_cols` | スクロールオフセットまたは列数が変わった時 |
| `app.rs::open_fullscreen` | `fs_open` | フルスクリーン遷移 |

**ワーカーへの伝搬**: UI スレッドは enqueue 時点の `input_seq` をタスク構造体にコピーする。

- `thumb_loader::LoadRequest.input_seq` — サムネイルワーカー用
- フルスクリーン非同期ロード: `start_fs_load` が `perf_seq` をクロージャにムーブする
- AI アップスケール / 色調補正ジョブ: 同様にクロージャへ
- PDF ワーカー IPC は seq=0 (プロセス間相関は現状非対応)

### 7.3 イベント構造

```json
{"t":12.345,"tid":5,"cat":"fs","kind":"paint","key":"C:\\a.jpg","seq":42,"idx":3}
```

主なカテゴリ:

- `input`  — ユーザー入力 (seq が振られる唯一のカテゴリ)
- `frame`  — 毎フレーム begin。`n` はフレーム番号
- `fs`     — フルスクリーン画像: `load_begin` / `decode_begin` / `decode_end` / `ready` / `paint`
- `thumb`  — サムネイル: `enqueue` / `pick` / `skip` / `decode_begin` / `decode_end` / `ready`
- `pdf`    — PDF ワーカー IPC: `pool_send` / `pool_recv` / `inproc_*` / `enumerate_send`
- `ai`     — AI: `upscale_begin` / `upscale_tile` / `upscale_end` / `denoise_*` / `job_start` / `job_ready`

### 7.4 解析

`scripts/analyze_perf.py` で集計。主要サブコマンド:

```bash
python scripts/analyze_perf.py <path>/perf_events.jsonl summary   # 件数/カテゴリ breakdown
python scripts/analyze_perf.py <path>/perf_events.jsonl latency   # seq → ready/paint ms
python scripts/analyze_perf.py <path>/perf_events.jsonl priority  # 優先度違反検出
python scripts/analyze_perf.py <path>/perf_events.jsonl thumbs    # decode 時間分布
python scripts/analyze_perf.py <path>/perf_events.jsonl dump 42   # 特定 seq の全イベント
python scripts/analyze_perf.py <path>/perf_events.jsonl timeline  # ガントチャート (matplotlib)
```

### 7.5 新ワーカー追加時のテンプレ

1. ワーカーに渡すタスク構造体に `input_seq: u64` フィールドを追加
2. UI スレッドの enqueue 箇所で `req.input_seq = self.input_seq` を設定
3. UI 側で `perf::event("<cat>", "enqueue", key, self.input_seq, &[...])` を emit
4. ワーカー側で `perf::event("<cat>", "begin"/"end", key, req.input_seq, &[...])` を emit
5. Ready 遷移 (texture upload 完了) で `perf::event("<cat>", "ready", ...)` を emit
6. `docs/async-architecture.md` のこの表にエントリを追加
