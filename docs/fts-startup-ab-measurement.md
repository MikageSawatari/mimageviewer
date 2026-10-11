# FTS 起動点検の A/B 計測 (§1.367)

[受理済み設計 §7.4](fts-integrity-plan.md#74-起動点検の明示的な採用条件-未達なら見送り) の
採用 gate。点検は `fts-startup-integrity` feature の opt-in、既定 OFF。以下の手順と解析器の追加は
計測を実施した証拠ではない。起動を遅くしないと判断できるまで製品へ採用しない。

## 保存データとビルドを固定する

利用者自身が起動する手順であり、agent が実アプリを起動する承認を与えるものではない。
agent に実行を依頼する場合は別途、具体的な条件・時間・desktop入力・使い捨て portable 範囲を
示して了承を得る。通常 profile/事故原本を実験に使わない。

1. 同じ source revision、同じ toolchain/profile/最適化/共通 features で A (点検 OFF) と
   B (`--features fts-startup-integrity`) をビルドする。feature 以外の差を作らない。
   未コミット差分で測る場合、HEAD だけでは実装を識別できないため、変更ファイルと新規ファイル
   (ローカル Tantivy を含む) の内容 hash も保存し、manifest の source_revision に併記する。
   一つの binary の runtime toggle はないため、これは同じソースの対照ビルドになる。
   例えば `cargo build -p mimageviewer --bin mimageviewer-core --release` と、同じ command
   に `--features fts-startup-integrity` だけを追加した build を使う。target は上書きされるため、
   A/B それぞれの exe を必要な runtime DLL を含む独立した測定ディレクトリへ保存する。
   `scripts/build-release.ps1` が用意した DLL 等を使い、exe と DLL の hash/ビルド条件を記録する。
   標準の検証用 exe はそのままにし、手順末尾で既定 OFF の build に戻す。
   測定前に `cargo test -p mimageviewer --lib --features fts-startup-integrity startup_integrity`
   で feature 付き入口 (cleanupより先の比較を含む) の回帰を完了する。実装時の通常構成の
   helper/fixture の成功をこの入口の成功へ読み替えない。実装時に残った一時 directory のACLによる
   環境 block は [実装記録 §12](fts-integrity-plan.md) と `target/I-1367-impl-msg.txt` を参照。
2. 新規の使い捨て profile にテスト対象の root のみを登録し、840,000 行以上の正常な両ストアを
   準備する。ユーザーの settings.db をコピーしない。FS ↔ SQLite の署名、所有 UUID、scan 印と
   設定を一致させ、live Tantivy 文書は各 Ok 行に一つずつ対応させる。tag/rating/bookmark 等は不要。
   fixture の作成・更新は必ずアプリを終了した状態で行う。準備後の seed を変更せず、各 run の前に
   seed から同じ `--data-dir` の実体を復元する。DB/index 単体の差し替えはしない。
3. Failed=0、少数、大量 (全 files の過半数) の三つの正常 seed を用意する。Failed には対応する
   live document を残さず、`live_docs == owner_rows - not_ok_rows` を満たす。Failed reconciliation
   による通常起動の費用は A/B 両側で同じにする。文書欠落を起こして異常再構築の費用を測らない。
   fixture ID/hash、全行数/Failed 件数、segments、生成手順を台帳に残す。

コピー作成/SQL の microbenchmark は別途先に行う。Python SQL の速度を Rust の起動費用や
true cold cache の証明に読み替えない。

Rust の非対話 fixture 計測入口は次の ignored lib test。SQLite/Tantivy 各840,464件の正常対応、
Failed=0/84/756,418 を作り、既存 ownership scan と COUNT+num_docs の追加時間を stdout に記録する。
担当者が十分な時間と使い捨て出力容量を確保して実行し、結果を保存する。

```powershell
cargo test -p mimageviewer --lib --features fts-startup-integrity startup_integrity_large_copy_fixture_cost -- --ignored --nocapture
```

この test は cold/warm・負荷・skip・UI-ready を測れず、以下の採用 A/B gate の代用にはならない。

実装時の非対話試行では、正常構成の lib test 実行体の同じ `cfg(test)` helper を使い、
Failed=0/84/756,418 の追加 COUNT+num_docs は 1.788/0.221/15.141ms だった。
各1回・全体 gate と同時実行の値で、idle/cold/warm や起動時間の上限を示さない。
件数・segments・ownership SQL scan と原ログは
[実装記録 §12.1](fts-integrity-plan.md#121-非対話-fixture-の費用-採用判定ではない) を参照。
これを理由に feature を採用しない。

## 手動測定の条件と起動

`cold / warm × skip_offline_change_scan OFF / ON × Failed zero / few / many × I/O idle / contention`
の24条件をすべて測る。各条件を少なくとも3 pair、AB → BA → AB と隣接・交互順で繰り返す。
これは最低限の反復数であり、分布が安定しなければ増やす。B が遅く見える条件を選んで捨てない。

- cold は再起動等、利用者が実施可能な明示した OS/file-cache 手順を A/B 両側で同じにする。
  seed copy 直後の読込は cache を温めるため、その後に同じ cold 手順を行う。単にアプリを
  再起動しただけを cold と呼ばない。実施方法を証明できなければ cold は未測定として採用しない。
- warm は同じ seed/パスで決めた予備読込を行う。各 arm 前に同じ予備読込を繰り返す。
  予備読込の perf log は本測定から外す。skip は profile seed 内で事前設定し、測定開始後に変更しない。
- idle は余分な背景負荷なし。contention は測定ディレクトリ内の**独立した使い捨てファイル**への
  再現可能な read/write 負荷とする。DB/index 本体へ書かない。負荷ツール/引数/ファイル容量/
  配置/開始時刻/終了時刻/実測スループットを保存し、同じ物理 drive、同じ開始条件で A/B を測る。
  通常 profile やユーザーの画像への負荷は作らない。
- 一回につき完全起動から supervisor 開始を観測するまで待ち、通常の終了で perf writer の shutdown
  checkpoint を保存する。強制 kill、途中終了、欠落/破損したログは採用証拠にしない。
  UI-ready と Full 完了は別指標なので、長い Full の完走を起動準備時間へ混ぜない。

実行前に installed/tray-resident mImageViewer を終了する (single-instance mutex は共有)。
以下は利用者が PowerShell で実行する例。A/B のディレクトリ、絶対 data-dir、run 名を変更する。

```powershell
Start-Process -FilePath .\target\fts-ab\A\mimageviewer-core.exe -ArgumentList @('--data-dir', 'C:\home\mimageviewer-nextsmall\target\fts-ab\run-data', '--perf-log')
```

**`--data-dir` を省かない。** この core は portable feature ではなく、引数を省くと通常の
`%APPDATA%\mimageviewer` を使い実設定/データを更新し得る。B も同じ絶対 data-dir を使う。
終了後に `run-data\logs\perf_events.jsonl` の当該 session を保存してから次の seed を復元する。
`perf_events.1.jsonl` 等は前回起動の世代であり、別 session を連結しない。

## イベントと manifest

`t` は perf session の起点からの秒、3桁へ丸めるため分解能は少なくとも1ms。
A/B で起点を揃え、duration の `ms` と混同しない。

| startup event | 比較値/証拠 |
| --- | --- |
| `indexer_init_result_send` | t、accepted=true、ready=true |
| `indexer_overlay_cleared` | t、ready=true (UI-ready の観測点) |
| `metadata_search_ready` | t (Ctrl+G 用 manager 採用時。Full 完了ではない) |
| `metadata_supervisors_started` | t、supervisors、integrity_feature_enabled |
| `metadata_out_of_range_cleanup` | integrity_feature_enabled、owner_rows、rows=0、success=true、query_ms |
| `fts_integrity_check` (B のみ) | integrity_check_ms、owner_rows/not_ok_rows/ok_rows、segments/live_docs、missing_suspected=false、feature_enabled=true |

同じ session/PID/process creation の `session.start` と末尾の `log.drain_checkpoint` によって
保存完了・coverage・drop zero を確認する。解析器は既存 `analyze_perf.validate_delivery` を再利用する。

manifest の `runs` は実施順。以下の1行を24条件×3 pair×2 armへ拡張する。
pair 内は A/B 同じ fixture、次の pair は BA、次は AB。ログ path は manifest の相対 path。

```json
{
  "schema": 1,
  "source_revision": "exact git revision",
  "build_profile": "release; exact toolchain recorded",
  "common_build_options": "same options; B adds only fts-startup-integrity",
  "fixture_preparation": "seed ID/hash and reproducible construction record",
  "cache_protocol": "exact cold reset and warm pre-read procedure",
  "contention_protocol": "tool/arguments/file size/drive/start-stop/throughput record",
  "measurement_resolution_ms": 1,
  "runs": [
    {"cache":"cold","skip_offline_change_scan":false,"failed_population":"zero","io":"idle",
     "pair":1,"arm":"A","log":"cold-off-zero-idle-1-A.jsonl","failed_rows":0,
     "fixture_id":"zero-seed-hash","binary_sha256":"actual A exe SHA256",
     "cache_evidence":"per-run cache reset record","io_evidence":"idle observation record"}
  ]
}
```

```powershell
python scripts/analyze_fts_startup_ab.py target/fts-ab/manifest.json --output target/fts-ab/report.json
```

解析器は endpoint ごとに A/B の n/min/median/p95/max と pair ごとの B−A の分布、点検時間と
件数を出す。分解能は観測限界として報告し、許容遅延として扱わない。**いずれかの pair/endpoint に
正の B−A があれば** regression または noisy/inconclusive として `not_adopt` (exit 1)。
全24条件/反復/交互順/正常 fixture/保存証拠が揃い、全比較で追加起動時間を観測しない場合だけ
`evidence_ready_for_review` (exit 0)。有限回の分解能内での観測であり、数学的ゼロや全整合の証明ではない。
担当者は cache/負荷の記録と分布も確認し、再現性のない条件、未測定/欠落/判断不能/悪化が一つでも
あればこのリリースでは採用しない。固定の許容 ms を勝手に設けたり barrier/index を追加したりしない。
修正本体は default OFF で進められる。採用判断は計測証拠とともに設計計画へ追記する。
