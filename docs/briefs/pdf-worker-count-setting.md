# PDF ワーカー数を設定できるようにする

着手前に [CLAUDE.md](../../CLAUDE.md) の「永続データ・スキーマ変更時の判断」と
「マニュアル・製品ページの記述方針」を読むこと。

前段の作業 ([pdf-worker-startup-failure.md](pdf-worker-startup-failure.md)) は実装済み。
そこで入れた「3 未満は失敗」の下限がこの設定の下限と同じ値である。

## 1. 現状

`POOL_SIZE` が `const usize = 5` ([pdf_loader.rs:1645](../../src/pdf_loader.rs:1645))。
利用者は変更できない。doc comment に「値変更は再起動が必要」と書いてあるとおり、
`in_flight_started_at` の固定長 vec と lane cap 計算に焼き付いているため**動的変更はしない**。

## 2. 決めてあること (利用者判断 2026-08-20)

- **範囲は 3〜10、既定 5。**
  - 下限 3 = Critical 予約 / HighNormal / Normal の各レーンが 1 を割らない最小値。
    前段で入れた `MIN_POOL_SIZE` と同じ。2 以下だと `non_critical_lane_caps` の clamp が
    働いて「そのときだけ挙動が変わる」構成になるので、**到達させない**。
  - 上限 10 は、それ以上を求める状況が想定できないため。
- **変更は次回起動から有効。**
- **メモリの代償を UI に書く。**

## 3. やること

### 3.1 設定値

`Settings` に `pdf_worker_count: u32` を追加する。`#[serde(default = "default_pdf_worker_count")]`
で既定 5。前例は `indexer_speed_profile` ([settings.rs:3471](../../src/settings.rs:3471)) —
doc comment に「変更は次回起動時に反映」と書く形も同じにする。

**マイグレーションは不要** (新規フィールド + serde default)。既存の設定 DB は読める。

読み出しは**純関数の clamp を通す**: `clamp_pdf_worker_count(u32) -> usize`。
2 → 3、11 → 10、5 → 5。設定 DB が壊れていても範囲外の値が pool へ渡らないようにする。

### 3.2 「次回起動から有効」を条件なしで成立させる

⚠️ **pool は遅延初期化される**ので、素直に `PdfWorkerPool::start()` から `Settings` を読むと
「まだ PDF を開いていなければ即時反映、開いた後なら次回起動」という**利用者から見て
非決定的な挙動**になる。これを避ける。

`pdf_loader` に `static CONFIGURED_POOL_SIZE: AtomicUsize` (初期値 = 既定 5) と
`set_configured_pool_size(usize)` を置き、**App の起動時に設定から 1 回だけ渡す**。
`PdfWorkerPool::start()` はこの static を読む。既存の `CRITICAL_RESERVATION_ACTIVE`
([pdf_loader.rs:226](../../src/pdf_loader.rs:226)) と同じ形。

これで「変更は必ず次回起動から」になり、分岐が増えない。
setter が呼ばれない実行形態 (`bench_scroll` 等) は既定値で動く。

### 3.3 `POOL_SIZE` の扱い

- `POOL_SIZE` → `DEFAULT_POOL_SIZE` に改名し、既定値と static の初期値だけに使う。
- `in_flight_started_at: vec![None; ..]` は **worker_id の添字**になる。worker_id は
  `0..configured` の範囲を取る (起動に失敗した枠があっても添字は詰めない) ので、
  **実際に起動した数ではなく configured 数で確保する**こと。
- `PdfWorkerPoolStartupFailure::requested_workers` も configured 数にする。
- テスト内の `vec![None; POOL_SIZE]` も追随させる。

### 3.4 UI

`PreferencesPage::Parallelism` ([pages.rs](../../src/ui_dialogs/preferences/pages.rs) の
`page_parallelism`) に追加する。既存のサムネイル並列度の下に区切りを入れて置く。

- `DragValue` で 3..=10、suffix は「 個」など。
- **代償を書く。** ただし**数値を捏造しないこと**。1 ワーカー = 専用のプロセス 1 つで、
  使うメモリは描いている画像の大きさで変わる (サムネイルなら小さく、拡大表示なら大きい) ため、
  固定の MB 値は書けない。書くのは次の 3 点:
  1. 増やすと PDF を同時に描ける数が増える
  2. 1 つにつき専用のプロセスが 1 つ増え、描く画像の大きさに応じたメモリを使う
  3. **変更は次回起動から有効** (`indexer_speed_profile` の「※ …次回起動時に反映されます」と同じ体裁)
- **内部用語を出さない。**「ワーカー」「プロセスプール」「PDFium」は使わない。
  項目名は「PDF の同時処理数」程度にする。「プロセス」は Windows のタスクマネージャーで
  実際にそう見える語なので、代償の説明に限って使ってよい。

### 3.5 ドキュメント

- [docs/spec.md](../spec.md) の設定項目
- [docs/async-architecture.md](../async-architecture.md) の `POOL_SIZE` 記述 (スレッド表と
  `pdf_pool.queue` の行に 5 固定として書かれている)
- [CLAUDE.md](../../CLAUDE.md) の「5 プロセス並列レンダリング」相当の記述
- [htdocs/mimageviewer/manual/settings.html](../../htdocs/mimageviewer/manual/settings.html)
  のパフォーマンス設定表に行を追加 (「並列読み込み」の行の近く)。
  **マニュアルはバージョン表記と内部用語を書かない。**

## 4. やらないこと

- **既定値の変更。** 5 のまま。既定を変えるには実測が要り、その計装は次の作業
  ([next-release-backlog.md](../next-release-backlog.md) §2.13 / §2.14) に含まれる。
- 動的変更 (再起動なしの反映)。
- `MIN_POOL_SIZE` の変更。設定下限と同じ 3 のまま。

## 5. テスト

- `clamp_pdf_worker_count` の境界: 0 / 2 / 3 / 5 / 10 / 11 / u32::MAX
- 設定の round-trip (保存 → 読み出しで値が保たれる)
- `set_configured_pool_size` を通した値が `in_flight_started_at` の長さと
  `requested_workers` に一致すること (pool を起動せずに検証できる形にする)
- 3 と 10 の lane cap (`non_critical_lane_caps` は既に純関数。3 は前段で追加済みなので 10 を足す)
