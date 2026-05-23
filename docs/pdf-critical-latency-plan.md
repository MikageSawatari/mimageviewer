# PDF Enter→list 2-3秒待ち解消プラン (v1.0.0 直前)

## 症状
- グリッドで PDF を Enter で開くと、ページサムネ一覧画面遷移までに 2-3 秒待たされる。
- perf-log では `pool_cancel_queued waited_ms=2630-2676ms` が Enter 直後に出る。
- 最初の 1 つは 158ms と速く、連続して開くと遅くなる (= キャッシュ無関係)。

## 既知の構造 (src/pdf_loader.rs)
- PDF は PDFium スレッドセーフでない → 3 個の worker プロセス pool で並列化。
- `JobPriority::Critical` / `JobPriority::Normal` の 2 段優先度 + `Mutex<JobQueue>` +
  `Condvar` + 各 worker 専用ディスパッチャースレッド。
- 既存の `CRITICAL_RESERVATION_ACTIVE`: `true` のとき `normal_in_flight` を
  `worker_count - 1 = 2` までに制限し、1 ワーカーを Critical 用に予約する。
- **現状は `set_critical_reservation(true)` を呼ぶのはフルスクリーン open 時のみ。**
  グリッド表示中は `false` で 3 ワーカー全部が Normal を pickup する設計
  (← グリッドの一括サムネ生成スループット重視)。

## 根本原因
- グリッドで PDF サムネ先読みは Normal 優先度 (`thumb_loader.rs:1114`)。
- 3 ワーカー全員が 1 PDF 1 ページのレンダ (700ms-3s) を IPC 中になりやすい。
- ユーザーが Enter を押した瞬間に `enumerate_pages_async` が Critical job を enqueue。
- **in-flight の Normal IPC は preempt できない** (PDFium 内部、stdin/stdout プロトコル
  に割り込み手段なし)。Critical は最も早く空く worker を待つ → tail 2-3s。
- queue 上の他 Normal は `PdfEnumerateHandle::Drop` → cancel token で pop 時に捨てられるが、
  in-flight には届かない。

## 修正案 (Plan A: 予約常時 ON)

最小変更で 99% 改善する。

### 1. `CRITICAL_RESERVATION_ACTIVE` のデフォルトを `true` に変更

```rust
static CRITICAL_RESERVATION_ACTIVE: AtomicBool = AtomicBool::new(true);  // was false
```

### 2. `set_critical_reservation()` の呼び出し 2 箇所を削除

- `src/app.rs:11131` (open_fullscreen 内の `set_critical_reservation(true)`) — もう不要
- `src/app.rs:13718` (close_fullscreen 内の `set_critical_reservation(false)`) — 削除

→ 予約は常に有効。3 ワーカーのうち 1 つは常に Critical 用に温存。

### 3. (任意) API 自体の削除

`pub fn set_critical_reservation()` と `CRITICAL_RESERVATION_ACTIVE` を削除し、
`run_dispatcher` の `max_n` 計算を `worker_count - 1` 固定にする。シンプル化。

ただし、将来「キャッシュ作成中だけ 3 ワーカー全開放」とかの拡張余地を残すなら
flag は残しておいてもよい。

→ **採用: API は残す (フラグは存在し続けるが、初期値 true・呼び出しはなし)。
   将来の bulk 操作向け knob として温存。**

## 効果

- 1 ワーカーは常に idle で `Condvar::wait` 中。Critical 到着で即 `notify_one` → pickup。
- Critical の wait 時間: 2-3s → 0ms (即実行) ~ 300ms (Critical 同士が偶発的に重なる時のみ)。
- グリッド一括サムネのスループット: 3 並列 → 2 並列 (-33%)。
  - 実害は限定的: サムネ生成は非同期、ユーザーは待ち時間として体感しない (見える順に
    届く)。重い用途は手動「キャッシュ作成」だが、これも -33% に収まる。

## やらない案 (検討した)

### Plan B: in-flight worker を kill して新規 spawn
- Pros: 5s 級の重いレンダ中でも Critical は ~150ms (cold open) で割り込める。
- Cons:
  - PDFium プロセス kill のリソース後始末が要る (zombie 防止)。
  - cold open ~150ms は依然遅い (1 ワーカー予約案は 0ms)。
  - 実装複雑、テスト難しい。
- → 採用しない。

### Plan C: PDF サムネ用 worker と nav 用 worker を物理分離
- Pros: 完全な隔離。
- Cons: 4 ワーカープロセスになる (起動コスト・メモリ +33%)。コード分岐が増える。
- → 採用しない (Plan A の Reserve 1 で十分)。

### Plan D: Normal IPC を細切れに分割
- ページレンダを途中で割り込めるよう PDFium 呼び出しを部分実行。
- → PDFium にそんな API なし。不可。

## 差分の規模

- `src/pdf_loader.rs`: 1 行 (atomic 初期値 false → true)
- `src/app.rs`: 2 行削除 (呼び出し撤去) + 既存コメント整理

## リスク・副作用

1. **キャッシュ作成 (一括) のスループット -33%**: 受容範囲。長時間バッチなので時間に
   余裕があるシーン、ユーザーが見ている用途ではない。
2. **PDF 含まないフォルダで 1 ワーカー idle 浪費**: 実害なし (idle worker は
   Condvar 待ちで CPU/RAM 消費ほぼ 0)。
3. **既存ドキュメント (`docs/pdf-issues.md`, コメント)**: 予約が「フルスクリーン時のみ」
   と書いてある箇所を更新する。

## テスト

1. `cargo build --release` 通る
2. `cargo test` 通る (ユニットテストは pool 状態に依存しない)
3. 手動: PDF 沢山入ったフォルダで:
   - Ctrl+↑↓ で次々と PDF を開く → Enter で 1 つ開く → 即座にページ一覧に遷移する
   - perf-log を取って `pool_cancel_queued waited_ms` が 100ms 未満に下がっていることを確認
4. (任意) bench: `cargo run --release --bin bench_scroll` で PDF 多いフォルダの初期 batch
   時間を測り、`+50%` 程度に収まっていることを確認 (3→2 並列なので理論上 +50%)。

## まとめ

- 修正は 3 行 (atomic 初期値変更 + 呼び出し 2 つ削除)。
- 体感 2-3s 待ち → ほぼゼロ。
- batch スループット -33% は受容範囲。
- v1.0.0 リリースに含めて支障なし。
