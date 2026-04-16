# パフォーマンス調査 引き継ぎメモ

次セッション向けの自己完結ブリーフィング。2026-04 のセッションで PDF ワーカープール絡みの
引っかかりを潰したあとの続き。**AI アップスケール優先度** と **スクロール中の重複エンキュー抑制**
の 2 つを分析・修正したい。

## これまでに入った変更 (このブランチの直近 3 コミット)

| コミット | 内容 |
| --- | --- |
| `d4e14de` | perf イベントログ (`src/perf.rs`, `scripts/analyze_perf.py`) 追加、`input_seq` 相関、PDF pool に `JobPriority::{Critical, Normal}` と cancel-in-wait 修正 |
| `ea7d499` | PDF ワーカープール書き換え: `JobQueue` (Mutex+Condvar) + 専用ディスパッチャースレッド × 3 + Critical/Normal 優先度 + `critical_reservation_active()` でフルスクリーン中のみ 1 ワーカー予約 |
| `44cc981` | `/simplify` レビューでの整理 (thread_id ヘルパ共通化、未使用 `input_seq_shared` 削除、PDF perf_key ヘルパ集約) |

結果: フルスクリーン表示の `fs.ready` が p50 555ms / max 681ms (= PDFium のレンダ時間
そのもの) まで改善。10 秒ハングは消滅。

## 調査インフラ (このまま使える)

### perf event log

- CLI: `mimageviewer.exe --perf-log --log`
- 出力: `%APPDATA%\mimageviewer\logs\perf_events.jsonl` (起動毎 truncate)
- 1 行 1 JSON レコード: `{"t":..., "tid":..., "cat":..., "kind":..., "key":..., "seq":..., ...}`
- `input_seq` で「ユーザー入力 → 表示」を相関付けできる
- カテゴリ: `input` / `frame` / `fs` / `thumb` / `pdf` / `ai`
- 無効時のコスト: `perf::is_enabled()` の Atomic 1 回読みのみ
- **ホットパスでは外ガード `if crate::perf::is_enabled() { perf::event(...) }` を維持すること**
  (`serde_json::Value` の extras 構築を避けるため)

### 解析 CLI

```powershell
$Perf = "$env:APPDATA\mimageviewer\logs\perf_events.jsonl"

python scripts\analyze_perf.py $Perf summary      # カテゴリ別件数
python scripts\analyze_perf.py $Perf latency      # seq → ready/paint レイテンシ
python scripts\analyze_perf.py $Perf thumbs       # decode 時間分布
python scripts\analyze_perf.py $Perf priority     # 優先度違反検出
python scripts\analyze_perf.py $Perf dump <seq>   # 特定 seq のイベント列
python scripts\analyze_perf.py $Perf timeline     # ガントチャート (matplotlib)
```

PowerShell で簡易集計したい場合の定型:

```powershell
Get-Content $Perf | Where-Object { $_ -match '"kind":"pool_dispatch"' } |
    ForEach-Object { ($_ | ConvertFrom-Json).pid } |
    Group-Object | Sort-Object Name

Get-Content $Perf | Where-Object { $_ -match '"kind":"pool_dispatch"' } |
    ForEach-Object { [double]($_ | ConvertFrom-Json).wait_ms } |
    Measure-Object -Minimum -Maximum -Average
```

### 過去セッションで取ったログ

比較用に以下のログを残してある:

- `perf_before_fix.jsonl` — 修正前 (5〜10 秒ハング発生)
- `perf_fix1.jsonl` — cancel-in-wait 修正後
- `perf_fix2.jsonl` — 予約セマフォ導入後 (グリッドが遅くなった世代)
- `perf_fix3.jsonl` — lazy worker 選択 (10 秒ハング発生で飢餓判明)

---

## 調査課題 1: AI アップスケールの優先度制御

### 現状の挙動

- 実装は `src/ai/upscale.rs` + `src/app.rs::maybe_start_ai_upscale` / `poll_ai_upscale`
- タイル推論の各タイル間で `cancel.load()` をチェック → **中断は機能している**
- `maybe_start_ai_upscale` は「現在ページ優先で、先読みの AI ジョブをキャンセルする」
  既存ロジックを持つ (コード上の `ai_upscale_pending` の部分で cancel を立てる)
- AI は in-process (GPU、ort + DirectML EP)、同時 1 枚

### 疑っていること

フルスクリーン表示中の **AI 先読みが現在ページの要求の邪魔をしている可能性**。特に:

1. 先読み中のタイル推論を終えて次の画像の AI を開始する判断の遅延はないか
2. ユーザーがナビで別ページに移ったとき、新ページの AI 要求は即座に優先されるか
3. ユーザー入力直後 (500ms のクールダウン中) は AI が走り出さないようになっているか

### 調査手順

1. `mimageviewer --perf-log` で AI 有効で運用しながらフルスクリーンを操作
2. `ai.*` カテゴリのイベント (`job_start`, `upscale_begin`, `upscale_tile`, `upscale_end`,
   `job_ready`) の時系列を分析
3. `input_seq` との相関: ユーザー入力 → AI 開始までの遅延、AI 完了 → paint までの遅延
4. 先読み AI が走っている最中に `input_seq` が進んだとき、どれくらい早くキャンセル→新規 AI が
   開始できているか (タイル粒度: 50〜500ms のはず)

### 修正候補 (現時点の仮説、裏取り必要)

- `maybe_start_ai_upscale` と `prefetch_ai_upscale` の判定順序を整理: **現在ページが
  AI 待ち中は先読み AI を開始しない** を強化する
- `enqueue_idle_upgrades` と同じ「入力後 500ms クールダウン」を AI 先読みにも適用する
- AI 推論スレッドも PDF pool 同様、キュー + ディスパッチャー方式に寄せる (やりすぎかも)

### 関連ファイル

- `src/ai/upscale.rs` — タイル推論ループ、cancel チェック
- `src/ai/denoise.rs` — 同上
- `src/app.rs::maybe_start_ai_upscale` (l.3918 付近)
- `src/app.rs::poll_ai_upscale` (l.3855 付近)
- `src/app.rs::prefetch_ai_upscale` (l.4200 付近)
- `src/app.rs::update()` の AI セクション (l.4710 付近)

---

## 調査課題 2: スクロール中の重複エンキュー抑制

### 現状の挙動

前セッションのログ解析で観測された現象:

> idx=0 が 5.5 秒間に **11 回** 再エンキューされる (スクロール中)

原因: `update_keep_range_and_requests` の振る舞い。スクロール中に `keep_range` が
頻繁に変化し、同じ idx が `Evicted → Pending → ... ` を何度も行き来する:

1. keep_range 内 → idx が `reload_queue` に enqueue
2. スクロールで keep_range がずれる → 範囲外 → `requested.remove(&i)` + `Evicted` にする
3. スクロールで再度戻ってくる → 再 enqueue
4. 繰り返し

キャッシュヒット時は 3ms 程度なので**実害は小さい**が、ポーリングの浪費と無駄な
ワーカー起床がある。cache miss で PDF 再レンダまで走ってしまうケースは致命的に遅い。

### 調査手順

1. `--perf-log` で PDF フォルダ内グリッドを激しくスクロールしたログを取得
2. `thumb.enqueue` イベントを `idx` 別に集計し、**1 つの idx が短時間に何回 enqueue されて
   いるか** 分布を出す
3. `thumb.decode_end` の `from_cache` 別カウントで、再エンキューが実際にデコード浪費に
   つながっている割合を測る
4. 特に PDF ページ (PDFium レンダが必要) で重複 enqueue → 実レンダが走っているケースが
   あれば影響が大きい

### 修正方針の候補

以下は仮案。実装前にログで裏取りする:

1. **`requested.remove` の撤去**: `update_keep_range_and_requests` の範囲外ループで
   `self.thumbnails[i] = ThumbnailState::Evicted` だけして、`requested.remove(&i)` は
   しない。ワーカーが処理中の idx が再 enqueue されなくなる
2. **スクロール中のデバウンス**: `scroll_offset_y` が変化している間は `enqueue` 部分を
   skip し、安定してから 1 回だけ enqueue する (`last_scroll_change_time` を利用)
3. **`requested` を時刻付きにする**: `HashMap<usize, (bool, Instant)>` にして、直近 1 秒
   以内に enqueue したものは再 enqueue しない

候補 1 は副作用 (ワーカーが終えた結果が範囲外だった場合の扱い) を要確認。
候補 2 は既存の `scroll_idle` 判定 (`SCROLL_IDLE_SECS = 0.5`) と方式が揃って一貫する。

### 関連ファイル

- `src/app.rs::update_keep_range_and_requests` (l.2060 付近) — メインロジック
- `src/app.rs::poll_thumbnails` (l.1830 付近) — 受信側で Evicted に落とす処理
- `src/app.rs::enqueue_idle_upgrades` (l.2280 付近) — クールダウン実装の参考
- `src/thumb_loader.rs::process_load_request` — worker pick 時の STALE チェック

### 副次的に見つけたもの (別件、今回はスコープ外)

- `pdf.enumerate_send` が**同じ PDF に対して連続 7 回** 発火しているケースがあった
  (`perf_before_fix.jsonl` の seq 187 付近)。`update_visible_indices` などで
  同じ PDF の enumerate が重複呼び出しされている可能性。これも余力があれば調査

---

## セッション再開時のおすすめアプローチ

1. **コンテキスト設定**: このファイルと `docs/async-architecture.md` を先に読む
2. **新しいログ取得**: 実機で `--perf-log` を使って、調査課題に対応するシナリオを
   再現して perf_events.jsonl を集める
3. **仮説検証**: このドキュメントの「修正方針の候補」をログで裏取りしてから実装に入る
4. **修正は小さく分ける**: 1 コミットで 1 課題 (AI / 重複エンキュー を分離)
5. **検証**: 同じシナリオで修正前後のログを比較し、数値で効果を確認

## 重要な設計原則 (前セッションで確立)

- **try_lock + sleep ポーリングは禁止** (`CLAUDE.md` + `docs/async-architecture.md §5.5`)
  → 代わりに `Mutex + Condvar` 優先度キュー + ディスパッチャー専用スレッド
- **`perf::event` 呼び出しは外ガードを残す** (extras の `serde_json::Value` 構築を防ぐ)
- **cancel は複数段階で機能する**: requester 側 (recv_timeout ループ) と
  dispatcher 側 (pop 時) の両方でチェックする
