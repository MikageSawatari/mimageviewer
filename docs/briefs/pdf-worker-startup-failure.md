# PDF ワーカーの起動失敗を、縮退ではなく失敗として扱う

着手前に [CLAUDE.md](../../CLAUDE.md) の「バグ修正の一般原則」と
[docs/async-architecture.md](../async-architecture.md) を読むこと。

> **改訂 2026-08-20**: フェーズ 1 調査で初版の前提が 2 つ崩れたため、範囲を変更した。
> 変更点は §7 にまとめてある。**初版を読んで作業に入らないこと。**

## 1. 現状 — 3 つの状態があり、真ん中が誰にも見えない

`PdfWorkerPool` は `POOL_SIZE`(=5) 個の子プロセス起動を試み、**失敗はログ 1 行で流す**
([pdf_loader.rs:1710](../../src/pdf_loader.rs:1710) 付近)。

| 実際に起動できた数 | 現在の挙動 |
| --- | --- |
| 5 | 正常。**唯一テストされている構成** |
| **1〜4** | **黙って縮退動作。利用者には何も見えない** |
| 0 | 5 箇所の `worker_count > 0` 分岐がすべて else 側 (in-process) に落ちる |

**リトライは無い。**

## 2. なぜ直すのか (利用者判断 2026-08-20)

- **テストできない分岐を残さない。** 1〜4 個の構成は実機でもテストでも通せない。
- **`worker_count > 0` の else 側 5 対を通るテストは 1 本も無い。**
- 多機能は目指すが、**処理の分岐を必要以上に増やさない**。

## 3. やること

### 3.1 プール初期化を UI スレッドから外す (先に、単独で)

**現状 UI が最悪 25 秒固まる。** `enumerate_pages_async` は**スレッドを立てる前に**
`get_pool()` を呼び ([pdf_loader.rs:3578](../../src/pdf_loader.rs:3578))、その呼び出し元
`load_pdf_as_folder` は UI スレッド ([app.rs:22196](../../src/app.rs:22196))。
readiness timeout は 1 ワーカー 5 秒 ([pdf_loader.rs:247](../../src/pdf_loader.rs:247)) で
5 個を逐次起動するので、初回 PDF オープンが最悪 25 秒 UI をブロックする。
**§3.2 のリトライを足すとさらに伸びる。先にこれを直す。**

`get_pool()` を spawn したスレッドの内側へ移す。§3.3 で `worker_count > 0` 分岐が
消えるので、この関数の構造は素直になる。

他に UI スレッドから `get_pool()` に同期到達する経路が無いことを確認すること
(`promote_to_high_normal` / `pool_queue_snapshot` / `bump_render_context_epoch` は
`POOL.get()` を使っており初期化しない)。

### 3.2 リトライと「3 未満は失敗」

- **spawn / readiness の失敗は、失敗した枠だけを回数を区切って再試行する。**
  成功した child は `pending_workers` に保持されるので、既存構造に素直に乗る。
  **時間で粘らない (回数で決める)。**
- **親側の DLL 展開失敗 (`dll_ready == false`) は即失敗。** `ensure_dll_extracted` の結果は
  `OnceLock<Result<..>>` に載る ([pdf_loader.rs:258](../../src/pdf_loader.rs:258)) ので、
  同一プロセス内で再試行しても同じ Err が返る。リトライは無意味。
- **最低 3。** 各レーン (Critical 予約 / HighNormal / Normal) が 1 を割らない最小値。
- **3 未満で確定したら、既に起動済みの child を明示的に終了する。**
  まだ `PdfWorkerPool` に格納する前なので、失敗 return だけでは既存の Drop 経路に乗らない。

### 3.3 in-process **フォールバック**を削除する (`render_page_async` は触らない)

⚠️ **`render_page_async` は現役の本番経路であってフォールバックではない。**
`worker_count` を見ずに常に `get_worker()` を使い
([pdf_loader.rs:3516](../../src/pdf_loader.rs:3516))、フルスクリーンの再レンダリング
(ズーム + `ensure_pdf_display_resolution` による表示解像度合わせ、現ページと見開き相方)
が全部ここを通る ([app.rs:50765](../../src/app.rs:50765)、
[ui_fullscreen.rs:13561](../../src/ui_fullscreen.rs:13561))。**今回はこれを一切変更しない。**

削除するのは以下だけ:

- `worker_count > 0` の **else 側 5 対** — [3056](../../src/pdf_loader.rs:3056) /
  [3090](../../src/pdf_loader.rs:3090) / [3136](../../src/pdf_loader.rs:3136) /
  [3318](../../src/pdf_loader.rs:3318) / [3579](../../src/pdf_loader.rs:3579)。
  IPC 経路のみにする。
- **呼び出し元がゼロの** `check_password_needed`
  ([3482](../../src/pdf_loader.rs:3482)) と `check_password_async`
  ([3636](../../src/pdf_loader.rs:3636))。調査で workspace 全体に呼び出し元が無いことを確認済み。

削除後、`get_worker()` / `WORKER` / `PdfWorker` / `WorkerRequest` の**唯一の利用者は
`render_page_async` になる**。これらは残す。ただし **doc comment を「フォールバック」から
「フルスクリーン再レンダリング専用の in-process PDFium スレッド」に書き換える**こと。
今のコメントは実態を偽っており、次に読む人が同じ誤解をする。

`WorkerRequest` のうち `render_page_async` が使わない variant (`GetInfo` / `AnalyzePage` /
`Enumerate`) と、`PdfWorker::handle_request` の対応する腕は削除できるはずなので、
**到達不能になったものはまとめて消す**。

`ensure_dll_extracted` は親・子プロセス・in-process 初期化のすべてが使う共有関数なので残す。
`core_enumerate` / `core_get_info` / `core_analyze_page` / `core_render_with_count` は
子プロセス側が使うので残る。

`enumerate_pages_async` の `pdf-enumerate-nav` スレッド spawn 失敗時
([pdf_loader.rs:3606](../../src/pdf_loader.rs:3606)) は現在 in-process へ落ちている。
削除後は **receiver へ明示的な Err を返す** (無言で何も返さないと呼び出し元が永久に待つ)。

**削除でテストが赤くなる場合は止めて報告する。**

### 3.4 失敗の見せ方

- **アプリの起動は止めない。** プールは起動時ではなく**最初に PDF を触ったときに**
  初期化される。起動時の失敗にするには全員のために起動時に 5 プロセスを立てる必要があり、
  PDF を開かない利用者に代償を払わせる。**遅延初期化のまま、PDF に触る操作だけが失敗する。**
- **状態は 1 つ。** 初期化結果 (`Ok(pool)` / `Err(理由)`) を `OnceLock` が一度だけ確定させる。
  「無効化フラグ」を 15 箇所の呼び出し元へ通さない。
- **理由を 1 回だけ見せる。** 既存の AI worker notice と同じ形を踏襲する:
  型付き notice を `Mutex<Option<..>>` で持ち ([ai/runtime.rs:267](../../src/ai/runtime.rs:267))、
  App の update が poll し ([app.rs:66501](../../src/app.rs:66501))、
  persistent window で見せる ([ui_dialogs/trt_worker_notice.rs](../../src/ui_dialogs/trt_worker_notice.rs))。
  **汎用イベントバスは作らない。**
- 文面には**最後の起動エラー**を含める。spawn エラーとは限らない (親 DLL 展開失敗では
  spawn を一度も試さない)。ログの場所も添える (`<data_dir>/logs`、
  [data_dir.rs:128](../../src/data_dir.rs:128))。
- **前面の操作はエラーを見せ、背面は今までどおり黙る。** サムネイル・全文検索 ingest・
  キャッシュ一括作成は現在も PDF の失敗を無言で飛ばしており、そこを変えると大量の
  トーストになる。**新しい分岐ではなく既存の慣習に従う。**
- **パスワードエラーと取り違えられないこと。** 通常 PDF オープンは Err 文字列に
  `Password` が含まれるかで分岐する ([app.rs:22605](../../src/app.rs:22605))。
  subsystem 失敗の文言がこれに引っかからないことを確認する。
- **キャッシュ済みラスタの表示は止めない。** retained raster があればワーカー無しで
  フルスクリーン表示できる ([app.rs:50082](../../src/app.rs:50082))。
  「PDF 無効化」は**新規の PDFium 処理を行わない**という意味であって、既に手元にある
  画像を見せない理由にはならない。

## 4. 今回の保証範囲

- **起動時に 3 以上を保証する。**
- **起動後にワーカーが死んだ場合は今回の範囲外。** dispatcher は `send_recv_io()` の Err を
  reply するだけで respawn しない ([pdf_loader.rs:2285](../../src/pdf_loader.rs:2285))。
  `docs/async-architecture.md:723` の「検知して再起動」は**実装と食い違っている**ので、
  **ドキュメント側を実態に合わせて直す** (実装を足すのではなく、現状を正しく書く)。

## 5. 制約

- **時間窓で粘らない。** リトライは回数で区切る。
- **silent fallback を新しく作らない。**
- 起動失敗時に**他の機能を巻き添えにしない**。フォルダ走査は PDF を開かないので、
  画像・動画・ZIP の閲覧は無関係に動き続ける。
- ワーカー数はまだ `POOL_SIZE` 定数のまま (設定化は次の作業)。

## 6. テスト

- 起動失敗をシミュレートして、**失敗した枠だけが指定回数リトライされる**こと。
- **DLL 失敗ではリトライしない**こと。
- 3 未満で確定したとき、**起動済み child が終了され**、**理由が 1 回だけ通知される**こと。
- そのとき**画像 / 動画 / アーカイブの経路が影響を受けない**こと。
- **3 / 4 ワーカー構成の lane cap が正しい**こと (`non_critical_lane_caps` は既に純関数)。
  今回から 3〜4 が正式サポート構成になるので回帰対象に含める。
- 初期化中に別スレッドが `get_pool()` に来ても、**UI スレッドがブロックされない**こと。
- `worker_count > 0` の分岐が消え、in-process 経路が `render_page_async` からのみ
  到達可能になっていること (コンパイルが通ること自体が保証になる)。

## 7. 初版からの変更点 (フェーズ 1 調査の結果)

| 初版 | 改訂 | 理由 |
| --- | --- | --- |
| in-process 経路を全部削除 | **else 側 5 対だけ削除。`render_page_async` は残す** | in-process はフォールバックではなく、フルスクリーン再レンダリングの現役経路だった |
| (言及なし) | **プール初期化を UI スレッドから外す** | 初回 PDF オープンが最悪 25 秒 UI をブロックする。リトライを足すと悪化する |
| クラッシュ隔離のため多プロセス | **理由は非スレッドセーフだから** | `async-architecture.md:16` の記述。PDFium のクラッシュ報告は履歴・docs に 1 件も無い |
| PDF を「無効化」する | **初期化結果を 1 つの状態として確定し、PDF 操作が失敗する** | 無効化フラグを 15 箇所へ通すと UI が不揃いになる |
