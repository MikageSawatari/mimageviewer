# リリース前 idle health / perf smoke 自動化計画

2026-10-09、line F の Step 1（設計案、未実装・未実行）。`next-qa` / `840c309ec` の
文書・ソースを参照した。backlog [§1.361 / §1.197](next-release-backlog.md) を、この検証範囲について
一つの手順へまとめる。§1.197 全体の複数窓・native 入力自動化を完成扱いにはしない。
`f9d095bec` の独立レビュー REVISE（P2 2件）を受け、tray の source / backlog 成立証拠と
外部 sampler の時刻変換を追補した。§8 の3点は2026-10-09の利用者回答により決定済み。

正本は [CLAUDE.md](../CLAUDE.md) の Phase 2 9.6 / 9.7、設定データ保護、実機検証用バイナリの準備、
[idle-health-check.md](idle-health-check.md)、[release-operations.md](release-operations.md)。
既存の [ui-smoke 設計](ui-smoke-automation-plan.md) と [実行了承の運用](interactive-release-verification.md) を継承する。
v4.4.0 / v4.5.0 の [検証記録](release-verification-records.md) は過去の利用者測定であり、
本案の fixture / 診断 portable の測定値ではない。

## 1. 目的・境界・簡素化

提案する入口は次の一つ。**以下は実装後の予定コマンドで、現在は存在しない。**

```powershell
.\scripts\ui-smoke.ps1 -Scenario ReleaseChecks -InteractiveApproved
# 同一ソースの診断ビルドが確認できる場合だけ -SkipBuild を追加
```

生成 → 準備 → idle 5 ケース → perf の cached / uncached 2 ケース → 終了 → 解析 →
貼り付け用 Markdown まで実行し、途中の Enter や利用者の操作は不要にする。
既存の runner に suite を追加し、汎用 E2E framework、独自 input backend、別のキャッシュ管理器は作らない。

**一度に一つの scenario / 一つの App process** とし、ケース間は完全終了して、停止済みの生成データを
同じ絶対パスへ戻して再起動する。cache / pin の準備に必要な再起動も既存経路で行う。
live 設定変更、ケースの並列化、失敗ケースの自動 retry / resume は採用しない。
これでキャッシュ汚染・foreground 競合・途中取消の組み合わせを消す。失敗時は証跡を残して止め、
新しい使い捨て run でやり直す。製品の loading や索引をモーダル化・遅延化して再現を作ることはしない。

line A が所有する **§1.360 の要求寿命 / tray 再投入修正と `modifier_probe` の入力誤集計修正** は依存。
F は `analyze_perf.py` の入力件数ロジックを変更しない。A の統合前も診断はできるが、
既知 FAIL を suite の成功へ読み替えない。実装時に analyzer の所有を調整し、F の変更は perf verdict 部分だけにする。
§1.175 の ui_snapshot / lib 実行体アクセス違反、§1.334 / §1.294 の full-suite flake は F の後続作業で、
本計画の実装・検証範囲外。既存の full gate をこの smoke で置き換えない。

## 2. 再利用する部品と不足する継ぎ目

| 部品 | 再利用 / 必要な変更（後続実装） |
| --- | --- |
| `prepare-portable-smoke.ps1 -TestScript` | `portable,test-script` の release 診断コピー、exact exe/data、marker、manifest / hash、reparse 拒否。元 package の `data` はコピーしない |
| `ui-smoke.ps1` / Rhai | fixture 準備、deadline、target 選択、receipt / snapshot、ログ・artifact 保存、失敗 / 環境不成立。`Idle198Convergence` の静止区間と外部 lifetime sample も参考にする |
| `--settings-override` | test-script 限定、明示 `--data-dir` 必須。tray ON、窓 / 列 / 比率、fixture 内だけの索引 root、起動場所等を JSON で固定する |
| `check-idle-health.ps1` | CPU / wall time、ログ増加、PID / command line、foreground と窓列挙、threshold 読取、統合 verdict を再利用。現状はログ先が APPDATA 固定、解析境界も OS StartTime 基準なので、そのまま呼ばない（§4で変換を定義） |
| `analyze_perf.py idle-health` / `hitches` | idle 判定と frame n / 計測区間の相関を再利用。後続で `perf-smoke` verdict / JSON 出力を追加し、既存 CLI は維持する |
| 不足 API | root の native close 要求、close 時点の loading 証跡、tray 終了経路への要求、正常経路による動画代表 pin、検索用文字入力 / 結果完了の観測。test-script 限定で最小追加する |

`check-idle-health.ps1` には明示的な隔離測定モード（案: `-PortableSmoke`）を追加する。
`-ProcessId` / `-SkipPrompt` と併用して既に runner が起動した exact PID のみを測り、
exe と data / logs を `target\portable-smoke` 内へ固定する。通常手動モードは維持するが、
新 suite は normal-profile launch / APPDATA log / latest-process 検出へ fallback しない。
tray の二つの case ID は report 上で区別し、既存 analyzer の `tray-residency` 判定を共有する。

## 3. 隔離と生成 fixture

起動は `target\portable-smoke\mimageviewer.exe` だけ、引数に
`--data-dir <repo>\target\portable-smoke\data --perf-log --test-script ... --settings-override ...` を必ず付ける。
起動前の marker / manifest / source fingerprint / hash 照合に加え、起動後の data-dir 記録、
`session.start.pid`、Win32 command line / executable path と runner の PID / start time を照合する。
一致が取れなければ環境不成立。空の perf 窓の PASS は既存の完全 sleep 証拠条件を維持する。

利用者の `%APPDATA%`、既存 portable の設定・索引・キャッシュ・媒体は読取元にもしない。
稼働中の tray app の停止 / activation / open-path 送信 / 設定変更をしない。
現在の scripted run は通常 instance listener を避け、通常起動の namespace も data_dir 由来。
suite は global instance event を使わず exact PID / root HWND だけを操作する。
診断ビルド経路（`-SmokeTestScript`）は既存 viewer の停止をしない。通常の `build-release.ps1` を
suite 内で呼ぶと常駐版を停止し得るため呼ばない。別の smoke が動いていれば置換せず拒否する。
後始末で終了させる対象も、この run が所有する process / helper に限定する。

fixture generator は既存 PNG generator と同梱 FFmpeg を使い、次を作る（件数は初期案）。
全 path は使い捨て tree 内、manifest に件数・寸法・hash・期待結果・recipe version を記録する。

| fixture | 構成・成立証拠 |
| --- | --- |
| static | 884×444 の byte-distinct 画像を含む画像フォルダ。窓 1328×900 / 7 列を出発点に、実要求が 374px 付近になる条件を記録する。DPI で変わるので列数だけで成立としない |
| many-folders | 512 サブフォルダ、各 4 画像以上、keep 範囲だけでも 8 枚の upload 予算を超える配置。相異なる画像で生成し、初期 `lf_scan` とフォルダ代表サムネの負荷を持つ。枚数だけでは backlog 発生の証拠にせず、tray は§4の実観測を必須にする |
| cached / uncached | 同じ構成の別 subtree。cached は正常ロード・保存・終了後の cache 読取を確認。uncached は準備時に開かず、当該キーの cache が無いことと初回 `from_cache=false` を確認。tray 用はさらに保存省略・close直前の cache 不在・格納後 source 再生成を必須にする（§4） |
| video-pin | 640×360 の短い無音生成動画を child folder の代表に正常 pin 経路で固定。保存完了して App を再起動し、親のタイルが cache から読み直されたことを確認する。動画再生は始めない |
| navigation / search | 順序の決まった画像フォルダを 6 個以上。生成 PNG の対応 metadata に固定検索語を入れ、fixture だけを索引して Ctrl+G の期待結果集合を作る。現行 indexer で生成し DB / Tantivy を手書きしない |

cache と pin の seed は現在の App / catalog の書込 API を通す。必要な test-only 呼出は正常 producer を
呼ぶだけとし、worker completion を偽造しない。検索 seed は現行索引で scan 完了・期待語の hit を確認する。
uncached の folder thumb を索引側で先に生成しないよう、thumbnail 自動索引は OFF、検索 root は検索 fixture だけ。
一般の索引 readiness は有効のまま、`index.initial_scan_settled` を待つ。

生成した seed profile は App 完全終了後にのみ保存 / 複製し、ケースごとに同じ data / fixture 絶対パスで使う。
DB の WAL を動作中にコピーしたり、path を移して cache の有効性を推測したりしない。
script の exit 0 だけでは正常終了の証拠にしない。tray ON では script 終了時の root Close が
再び格納され得るため、測定後は既存 tray「終了」の処理へ test-only 要求を通し、on_exit / flush と
process 終了を確認する。forced-watchdog / runner 強制終了は後始末であり、seed / exit 試験を PASS にしない。
未読 subtree は case ごとに独立させる。生成不能・pin / index の準備 timeout は環境不成立として理由を残す。
fixture の root と全親の絶対パス・reparse を確認してからコピー / 削除し、既存の非空 tree を無条件で消さない。

## 4. idle シナリオと窓操作

| case ID | 操作と測定前の証拠 |
| --- | --- |
| `static-foreground` | cached static を開き完了、exact root が前面で静止 |
| `static-background` | 同じ static 条件を runner 所有の無害な別窓の背面へ移す。測定中の foreground PID は App 以外 |
| `video-pin-background` | 再起動後に親を開き、pin タイルを keep 内に置いて背面へ。TargetKey、当該キーの cached work、idle upgrade 評価を記録 |
| `tray-close-while-loading` | 保存省略を固定した未キャッシュ many-folders を開き、当該世代の要求開始を確認して root へ close。実際の格納時に未完了 work があることと、下記 source / backlog 証拠を必須にする |
| `tray-close-after-loading` | 別の保存省略を固定した未キャッシュ many-folders を開き、ロード完了後も対象キーの cache が無いことを確認して root へ close。格納後の source 再生成 / backlog 証拠を必須にし、cache 読込では代替しない |

### `[×]` と同じ経路にする

現状の `always_on_top_smoke("hide_to_tray")` は `src/app.rs` で **直接 `hide_to_tray()`** を呼ぶ。
実際の `[×]` は `maybe_intercept_close()` → 設定 / tray / modal 判定 → `CancelClose` → `hide_to_tray()`。
GPU 解放等の格納本体は同じだが、入口は同じではない。この既存 API だけで「実 `[×]` 検証済み」とは書かない。

suite は登録済み exact root HWND へ **`WM_CLOSE` を一度だけ post** し、winit の close event と
`maybe_intercept_close()` を通す（OS のタイトルバー hit-test 自体は対象外）。`SW_HIDE` の直呼び、
最小化、`Visible(false)` では代用しない。test-only の証跡には close 受理 / tray hide の sequence、
folder path / generation とその時点の未完了要求・backlog・完了条件を残す。
読込完了は `items_len > 0 && pending_thumbs == 0` だけでは足りない。A が直す要求所有者と
UI backlog の終端、初期 enqueue / 採用済み状態まで合わせ、keep 内の現ロードが完了したことを確かめる。
while-loading は close **送信時ではなく受理・格納時**に未完了であることを確認する。
既に完了していたらそのケースは環境不成立。固定 sleep、worker の遅延、再現できるまで retry は使わない。

### tray の source 再生成と backlog 成立証拠

通常ロードで `CacheDecision` が保存を選ぶと、初回が `from_cache=false` でも格納による GPU 解放後は
cache hit へ変わる。`thumb_loader.rs` の通常 cache hit は画像を送って return し、source 生成の
`Finalized` を通らない。これを§1.360の再現成功に数えない。
tray 2ケースだけは起動時 overlay で既存の `cache_policy=off` を指定し、通常の保存判定を通して
保存を省略する。static / video-pin / perf の overlay には適用しない。`force_cache` がこの設定を
上書きする要求は対象にせず、判定・worker・upload 予算を test-only に変更して再現を作らない。

両 tray ケースについて、同じ対象キー / idx / items generation と要求を相関させ、次を必須にする。

1. source 要求の実保存判定 `qa/source.should_save=false`、close直前（受理・格納境界）の catalog / cache_map
   両方で有効な対象 cache が無いことを、読取専用の証拠で確認する。while-loading の未完了要求は
   保存判定到達後の証拠も照合する。初回の cache miss だけでは代替しない。
2. 格納後、GPU 解放で対象が再要求され、`from_cache=false` の source 生成と `Finalized` が
   実際に発生したことを確認する。格納前の decode / Finalized や格納後の cache hit では成立しない。
3. その **同じ要求の画像が UI の `texture_backlog` に残った状態で、UI が `Finalized` を受け取った**
   証拠を必須にする。read-only test-script 計装で受理時の要求・画像 backlog・状態と frame を記録し、
   A の要求寿命と対応付ける。worker の送信時刻や keep 内の枚数だけでは代替しない。

cache が作られた、source 再生成がない、Finalized 受理時に当該画像が backlog にない等の場合は
再現条件不足として SKIPPED / 環境不成立。idle 値が低くても PASS にしない。
成立した後の旧版で同一要求の再投入反復、修正版で収束を確認する。**読み込み中／完了後をそれぞれ
独立に旧版 FAIL・修正版 PASS** とするのが採用条件で、一方の成功で他方を代替しない。
通常フローの速度を変えず両版で同じ read-only 計装・fixture / overlay を使う。

### foreground と無操作区間

v4.4.0 のキー系 SKIPPED を踏まえ、Focus / `SetForegroundWindow` の要求成功だけで判断しない。
既存 Idle198 の上限 20 秒を使い、入力前に exact root HWND / PID と実 foreground を照合する。
OS が前面を許可しなければ SKIPPED（環境不成立）、強制的な focus 制限解除や別窓へのキー送信はしない。
背面用には runner 所有の無害な窓を使い、ユーザーの他アプリを操作しない。
static 前面 / 背面は開始・終了の PID を必須とし、既存 lifetime sampler 同様に測定中も外部から確認する。
tray は既存の helper 除外つき top-level 列挙を使い、両境界で total ≥1 / visible=0、プロセス存続を必須にする。

準備完了 marker の後、Rhai worker は無入力の `sleep` に入る。`sleep` / snapshot 読取自体は UI を起こさないが、
`RunnerBridge::send`、`wait_until` の完了通知、保持キー、script 終了は repaint を要求し得る。
測定中はこれらも capture / ログ marker 送信も行わず、外部 sampler だけが動く。
既定 5 秒 warmup + 15 秒測定に余裕を付けた一つの sleep 区間を確保し、その中に実測窓が入ったことを検証する。
script が早く終了 / 区間を外れた場合は環境不成立。tray の終了も測定後に行う。

### 外部測定境界を perf の時刻へ変換する

perf の `t=0` は core `run()` 入口の `Instant` であり、OS の `Process.StartTime` とは異なる。
PID 一致だけでは両者の測定窓は一致しない。対象 session の `session.start.wall_unix_ms`（UTC Unix ms）と
`wall_t`（その壁時計取得時の perf 相対秒）を一組の anchor とし、外部 sampler の CPU / ログ増加を
採る開始・終了境界の UTC Unix ms `B` を、次の同じ式で analyzer の `--start-t` / `--end-t` へ渡す。

```text
perf_t(B) = session.wall_t + (B - session.wall_unix_ms) / 1000
```

`session.start.t` を anchor の代わりにせず、`Process.StartTime` は process 同一性確認にだけ使う。
sampler report には外部開始・終了 UTC、anchor、変換後 start_t / end_t と Stopwatch の実測 wall 秒を保存する。
Rhai の sleep 開始・終了境界も worker 側で記録し、UTC なら同じ式で perf 時刻へ変換する。
**sleep区間内に変換後の測定区間全体が含まれること**を確認し、warmup・終了通知・終了操作を混入させない。
境界の通知到着時刻を実際の sleep 境界に置き換えず、測定中の marker 送信も増やさない。
anchor 欠落 / 不正、session 不一致、時刻逆転、包含不成立は環境不成立とし、OS StartTime へ fallback しない。
測定中の壁時計変更は不可。外部 UTC の差と Stopwatch の経過が時計精度を超えて食い違う場合も保留する。

回帰検査には、OS の作成時刻から core `t=0` まで **7秒の非ゼロ差**、さらに perf 初期化時の
`wall_t=5.25` を持つ合成ログを使う。anchor の壁時計は OS 作成時刻+12.25秒、外部測定境界は
OS作成時刻+17秒 / +32秒とし、変換後は `[10,25]` になることを確認する。
CPU / ログ増加の外部窓に対応して、t=11 / 24 の work は含め、t=26 の終了 input は除外する。
誤った OS 経過秒 `[17,32]` では前者を落とし後者を混入させるので、この差を検査する。
同じ変換で sleep 区間への包含、anchor 欠落、非包含の拒否も検査し、StartTime の7秒ずれを使った
誤った判定が通らないことを保証する。

判定は `idle_health_thresholds.json` を唯一の正本とする。CPU 目標 .05 / 上限 .10、
update 目標 2/s / 上限 10/s、reason streak ≤2s、同一 work ≤3、input=0、
通常 / perf log 増加 ≤16KiB / 256KiB（15秒）を変更しない。
index settle の既定上限は 180 秒。現行 sampler は timeout を warning にして測るが、
生成 fixture suite では準備未完了として保留し、背景の索引負荷を idle regression と誤判定しない。
video-pin は最新 folder-load 以降の work 証拠に加え cached 読取を求め、upgrade 評価未確認の warning は
自動 suite の完了証拠不足として保留する。ピン直後 `from_cache=false` の状態を PASS にしない。
なお CLAUDE 9.7 は `idle_upgrade_ineligible` を必須と記述する一方、idle 正本 / 現行 analyzer は
対象 work を必須、enqueue / ineligible の評価未確認を warning としている。本案は後者を基礎に
suite の証拠不足を保留し、採用時にこの文書差も統一する（ineligible だけを唯一の成立証拠にはしない）。

## 5. perf smoke の操作・自動判定

cached / uncached を別 session で同じ台本で測る。起動の初期 many-folders load もログに含める。
画像 folder を開く → **Ctrl+Down を 5 回、各入力の receipt / 世代・場所の変化を待つ** →
Ctrl+G を開き固定語を入力 → debounce / 検索終了と期待 hit 集合を確認 → 通常終了。
hold repeat の回数を 5 回と見なさない。default keymap を seed に固定して通常 keymap consumer を通す。
現在 `hold_key` は Down + ctrl を扱えるが、synthetic key enum に G がなく、汎用検索文字入力 API も未整備。
後続で G の chord と text event の最小配送 / 消費証跡、検索の読み取り専用観測を追加する。
`run_action("GlobalMetadataSearch")` だけでは Ctrl+G 入力の確認に読み替えない。
egui / key_input 注入は通常 handler を検査するが、実 OS キーボード・IME の配送試験ではない。

追加 analyzer は JSONL の行順ではなく session PID、frame n、t と既存 `hitches` の outer / inner update
区間で結び付ける。連続 `frame.begin` の **100ms 超 gap を全件**出し、同時に update 自体の
100ms 超も検査する。最後の update は inner 計測で補い、outer が無いことを明記する。

| 分類（上から優先） | 根拠と扱い |
| --- | --- |
| `known-startup-scan` | 最初の起動 folder-load 区間の同 frame で `lf_scan >100ms` が update の主成分。t が小さいだけでは認定しない。v4.5.0 の tail=`none` / scan 572ms も拾う。件数・gap / update 最大・scan 内訳を別記 |
| `known-first-ai` | 明示した初回 AI 操作区間で `request_repaint_after_ai_upscale` と同 frame の fullscreen viewport >100ms が主成分で、初回 AI 開始の証拠が揃うもの。後続 AI、証拠不足、同期 I/O の別段は対象外。GPU 競合という原因までは断定しない |
| `normal-sleep` / `normal-idle-upgrade-wake` | 上記 / 遅い update がなく、直前 frame の tail が `none`、または idle upgrade の予定遅延 wake。gap は sleep 込みなので正常。tail を結合できない場合は正常にしない |
| `unexplained` | それ以外の 100ms 超 gap / update。`request_repaint` 中の停止、未知の遅延 wake、同型でも通常操作中の scan、AI の反復停止を含め FAIL |

主成分は遅い update の計測時間の過半を占める段とする。既知形の中に別の100ms超の段がある場合は
unexplained も残す。既知形自体も warning とし、増悪は最大値と旧 run 比較で責任者へ報告する。
gap と update の同一停止を二重に
件数加算しないよう frame と区間を出力し、両指標の件数を別々に表示する。
tail=`none` を理由に長い UI update を捨てない。`diagnostic_dropped`、session 不一致、欠損 / 壊れたログ、
必要な tail / breakdown / 操作証跡がない場合は環境不成立として PASS を禁止する。

| 指標 | 閾値（利用者決定2026-10-09、サンプル数は下記暫定） |
| --- | --- |
| unexplained 100ms 超 | 0 件を必須。既知形は別報告し、一般の免除にしない |
| 描画 frame の <16ms 比率 | **直前 frame の tail が厳密に `request_repaint` の gap だけ**を分母にする。n、<16ms 件数、比率、p95 / max を cached / uncached 別に出す。全 frame 比率は gate にしない |
| 比率の閾値 | 初期は **97%以上を目標、未満は WARNのみ**。v4.4.0 / 4.5.0 の98.4 / 98.0%は別条件であり baseline に流用しない。固定 fixture / 同じ PC で各3回測定後、cached / uncached 別の hard gate を決める |
| `pre_grid.total_ms` | session 全サンプルの **p95 ≤0.3ms を必須**、超過は FAIL。p50 / p95 / max、遅い段と frame を出す。既知 startup を含む raw 値を保存し、除外して閾値を通さない |

分母 0 / 有効サンプル不足は PASS にしない。暫定で描画 n≥200 を目安にし、Phase 2 で fixture と
台本の自然なサンプル数を確認して固定する。足りないから idle に repaint を足して水増ししない。
AI は基本台本に含まれないため `known-first-ai=0 / AI未実施` を明記する。
利用者決定により、AI / GPU 経路を変更する版だけ同じ入口の明示オプション（案: `-IncludeFirstAi`）で、
生成静止画の初回 AI を追加する。
モデル / backend / pack と準備時間を記録し、モデル unavailable を正常形に数えない。

## 6. 結果・検証記録への出力

各 case は PASS / FAIL / SKIPPED（環境不成立）を出す。全必須 case と証拠が揃ったときだけ suite PASS。
警告は `PASS_WITH_WARNINGS` として残す。終了コードは 0=完了（警告は別表示）、1=製品 / 性能 FAIL、
2=未完了 / 環境不成立。FAIL と SKIPPED が混在すれば 1、内訳は消さない。
既知形の報告は自動 verdict に添えるが、出荷許可ではない。waiver はリリース責任者 / 利用者が別に判断する。

`target/ui-smoke/runs/<run-id>/` の既存 evidence 保存に、fixture / seed recipe、設定 overlay、
各 session の perf / app log、sampler JSON、case JSON、`summary.json` と `release-record.md` を追加する。
App の session evidence は厳密に `target/ui-smoke/runs/<run-id>/<case>/` とし、
Rust の `test_script/evidence_path.rs` が screenshot と release fixture のパス契約を所有する。
既存の単独 scenario は `target/ui-smoke-runs/<run-id>/` を維持する。いずれも repository ごとの
固定 root・固定階層数を要求し、traversal と全祖先の reparse point を canonicalize 前に拒否する。
canonicalize 後も同じ root・階層数を照合し、Win32 の末尾 dot／space 等による別階層への alias を許可しない。
run-id と case、対象 SHA / dirty fingerprint、exe hash / feature / profile、OS / CPU / GPU / DPI、窓サイズ、
閾値 version、操作証跡、tray の保存省略 / cache 不在 / source再生成 / Finalized-backlog 相関、
外部 / perf 時刻と sleep 包含結果を残す。再起動の log rotation 前に保存する。release-record は正本文書へ自動追記しない。
**次のブロックは出力書式の例で、実測結果ではない。**

```markdown
- **release-check automation (隔離診断 portable、<実施者>、<日時>)**: <suite verdict>。
  source <SHA / dirty fingerprint>、artifact <exe SHA256>、release + portable,test-script、fixture <version>。
  環境 <OS / CPU / GPU / DPI / 窓>、run <証跡へのパス>、了承範囲 <suite / AI有無>。
- **idle health**: static-foreground <判定 CPU / update / streak / same-work / input / log増加>、
  static-background <同上>、video-pin-background <同上 + TargetKey / cached / upgrade証跡>、
  tray-close-while-loading <同上 + close受理時未完了 / 保存省略・cache不在・格納後source・Finalized-backlog / total-visible開始終了>、
  tray-close-after-loading <同上 + close受理時完了 / 保存省略・cache不在・格納後source・Finalized-backlog / total-visible開始終了>。
  warmup / measure <秒>、index settle <結果>、閾値 <version>、SKIPPED / warning <理由>。
  sampler UTC境界 <開始終了> / session clock anchor <wall_unix_ms, wall_t> /
  perf測定窓 <start_t, end_t> / sleep包含 <結果>、tray旧版FAIL・修正版PASS <両ケースの証跡>。
- **perf smoke cached / uncached**: <各判定>、folder open / Ctrl+Down x5 / Ctrl+G結果 / exit <証跡>。
  各 total frames <n>、drawn <16ms <k>/<n> (<%>) / p95 <ms> / max <ms>、
  pre_grid total p50 / p95 / max <ms>。100ms超 gaps <n> / updates <n>、
  normal-sleep <n>、idle-upgrade wake <n>、known-startup <n / 最大 / lf_scan>、
  known-first-ai <n / 最大 / viewport / backend、または未実施>、unexplained <n / 詳細パス>。
- **手動確認**: 実データ規模 <未実施 / 利用者の結果>、実[×] / tray復帰 <結果>、
  当該版のnative描画・IME・DPI等 <必要な経路 / 結果>。診断portableを配布coreの実測とは扱わない。
```

## 7. 手動で残すもの・エージェント実行

利用者の普段の大量フォルダ / HDD / network / 既存索引・キャッシュでの傾向は生成 fixture だけでは
代表できない。利用者決定により、UI / I/O 経路を変更する版だけ、普段のフォルダで短い確認を残す。
通常ビルドは利用者自身が起動し、生成 suite と実データの結果を別に記録する。
実タイトルバー `[×]` と tray icon からの復帰、窓位置・サイズの復元は native hit-test / Shell 操作の確認として残す。
当該版で変更した実 IME、native video scanout、音声、multi-monitor DPI 等は、対象経路の既存必須検証を維持する。
本 suite の自動化成功を、これらや他の §1.197 scenario、full gate の成功へ読み替えない。
diagnostic feature の overhead / portable 非同梱（VST / EffeTune）もあり、配布版の厳密性能・包装検証とは区別する。

**隔離 data の起動はエージェント実行可**。ただし実 PC の前面・入力を使うため、既存の実行了承を維持する。
suite が具体化した後、5 idle ケース・2 perf ケース、見込み **5〜8分 + Appを使うseed準備時間（初回buildは別）**、
窓切替・synthetic キー・root WM_CLOSE、使い捨て保存先、AI 追加の有無を示し、対象 run の明示了承後だけ
`-InteractiveApproved` を付ける。今回の設計依頼は実行了承ではなく、アプリは起動しない。
ビルド / 媒体fixture生成 / 解析テストは非対話で進められる。cache / pin / index の App seed 作成は
同じ実行了承の対象に含め、了承前に起動しない。`test-full.ps1` / 配布 build に自動連結しない。
エージェントは normal-profile executable を起動せず、実設定が必要なら利用者へ build と手順を渡す。

## 8. 段階と利用者決定

1. **設計確定（今回）**: 本書を design lead と独立 reviewer へ渡す。A との所有境界、tray の入口と完了条件、
   閾値・手動範囲を合意する。コード変更・コミット・実アプリ実行はしない。
2. **fixture / 判定器を非対話で実装**: generator / seed 検証、analyzer JSON / verdict と sampler の隔離モード。
   v4.5.0 型の tail=none + scan、正常 sleep、初回AI / 反復AI、未知 gap、欠損ログ、比率の分母、
   p95 境界、非ゼロのOS作成時刻差と `wall_t`、測定窓 / sleep包含を合成ログで回帰検査。
   path 拒否・approval guard・exact PID は fake テスト。
3. **台本を接続**: 最小の test-only close / pin / G + text / read-only 証跡、suite の逐次実行と記録出力。
   **Phase 3b は A が master に統合された後**に行い、A の request ID / read-only 投影へ接続して
   loading 受理条件、保存省略 / cache不在 / 格納後source / Finalized-image-queue と終端を照合する。
   feature 有無の compile / 焦点テスト、完了独立レビューを実施。
4. **了承済み検証枠で採用**: 各 case 3 回で成立率 / 実測閾値と所要時間を確認し、既知回帰 fixture が
   trayの読み込み中／完了後それぞれで旧版 FAIL / 修正版 PASS を出す証拠を取る
   （同じ read-only test-only 計装・fixture / overlay、製品挙動は変更しない）。
   foreground 不成立はそのまま記録。採用後に CLAUDE Phase 2、idle 正本、運用・索引を更新する。

**利用者決定（2026-10-09）**: 当初の3質問は、すべて推奨案を採用する回答を受領した。

- **比率の hard gate**: 初期は WARNのみ。固定 fixture / 同じ PC の各3回測定後に
  cached / uncached 別の hard 値を決める。pre_grid p95 >0.3ms と未知100ms超の gap は初期から FAIL。
- **AI の範囲**: 基本 suite から分離し、AI / GPU 経路変更版だけ明示追加する。
  モデル・pack 準備を毎回の idle 検査の前提にしない。
- **手動実データ確認**: UI / I/O 経路変更版だけ普段のフォルダで短い確認を残す。
  通常ビルドの起動は利用者自身、生成 suite と実データの結果は別に記録する。

回答済みの3点について再確認は不要。設計受理時点では実装・実アプリ検証は開始していない。
その後の実装指示と今回の範囲は §9 に記録する。

## 9. Phase 2 / 3 の実装（2026-10-09）

利用者の追加指示で Phase 2 / 3 を実装する。Phase 4 の実アプリ起動・採用測定は今回の対象外。

- `scripts/ui-smoke/generate_release_checks_fixture.py` が固定形の画像・動画・索引用画像、
  overlay、manifest を生成／検証する。cache / pin / index は了承済み seed session の通常 producer が作る。
  媒体生成は PATH 上の FFmpeg CLI を使い、実行体 hash と引数を manifest に記録する。
  アプリ同梱の FFmpeg は DLL のため CLI と区別し、CLI 不在は環境不成立とする。
- `scripts/release_check_analysis.py` が clock-anchor 変換、成立証拠、perf / idle verdict、
  suite JSON と貼付用 `release-record.md` を出す。`analyze_perf.py` への変更は新 subcommand の接続だけ。
  初期 many-folder は停止済み DB の読み取り専用 `pre-launch-cache.json` と起動前 diagnostic arm で相関させる。
  早期 return の tail は test-only の egui scheduler 読み取りで補い、未知の delayed repaint を正常扱いしない。
- `scripts/ui-smoke/release-checks/{Run,Sampler,Support}.ps1` と `release-checks.rhai` が
  exact portable artifact / PID / HWND を検査し、seed の完全終了・復元を挟んで逐次実行する。
  path / desktop / lock / artifact 不成立は SKIPPED / 2。App の ScriptFailure / 1 は FAIL / 1 として
  元の process exit code と失敗ログの場所を保持し、後続の環境・保存エラーで上書きしない。
  強制終了は正常完了証拠に含めない。

**レビュー修正と Phase 3b（A の master 統合後）**:

- tray の close / hide、格納後の同一 request の source 生成・保存省略、画像 queue に要求が残った
  `Finalized` の全成立証拠を、UTC anchor で変換した**計測開始時刻まで**に要求する。
  計測中・計測終了後の Observe が起こした処理、時刻証拠なしは SKIPPED とする。
- F の TLS 採番、`ThumbMsg.diagnostic_id`、backlog 再構築時の引き継ぎ、旧 `texture_backlog` の
  診断参照・独自 Finalized observer は撤去する。loader / queue に別の所有者や代替 ID を追加しない。
  Analyzer は read-only JSON 投影の `request_id`、key / idx / generation、source / save、
  `image_backlog_contains_request` を検査する契約を維持する（この field は実画像 queue の所属を表す）。
- A の `ThumbnailRequestId` と typed request owner が master に統合された後、その正本 ID と
  read-only 投影で request begin / evaluated_display_px、source / cache origin、保存判定、loading 状態、
  `Finalized` 時点の実画像 queue 所属を接続する。F の現行 terminal / requested 観測もそこで見直す。
  測定開始前に成立を待つ worker と無操作 sleep の境界を整合させ、同じ ID の正例・別 ID / late 証拠の
  負例を再検証する。新たな所有 state や UI-thread I/O は作らない。
- Phase 3b 接続前は tray close hook が EnvironmentFailure / 2 を返し、static の requested_px と
  uncached の source 証明も未接続だった。下記の接続後も、欠損証拠を免除せず、Phase 4 の実測なしに
  suite を PASS／採用済みとは記録しない。
- 診断用 G は `cfg(any(test, feature="test-script"))` で variant と全 match arm を限定する。

実行窓の明示了承後の入口は `scripts/ui-smoke.ps1 -Scenario ReleaseChecks -InteractiveApproved`。
AI / GPU 経路変更版だけ `-IncludeFirstAi` を追加する。出力は各 run の `summary.json` と `release-record.md`。
通常 data、既存常駐版の停止、release-verification-records 正本への自動追記は行わない。

非対話検証は Python の analyzer / fixture / preflight 回帰、`scripts/test-release-check-safety.ps1`、
Rust の diagnostic / recipe / payload 焦点テストと feature 有無の check。検証証跡の引き継ぎ先は
`target/F-1361-impl-msg.txt`。tray 2ケースの旧版 FAIL / 修正版 PASS、固定 PC 各3回の成立率・閾値、
実 foreground は Phase 4 に残す。

### Phase 3b 接続（2026-10-10、master 0a5bf3689 後）

- A の `ThumbnailRequestId::Registered(u64)` をそのまま使う。`ThumbnailRequests` の test-only
  `diagnostic_projection` は ID / idx / generation を照合し、既存 Phase と実画像 queue の所属、
  worker 終端待ちを読むだけ。追加の request 所有 state、TLS 採番、`diagnostic_id`、旧 backlog 観測はない。
- worker の cache 判定前に `qa/request_begin` と実際の `evaluated_display_px` を記録する。
  source 生成の通常保存判定位置で `qa/source` の origin / `should_save` / `force_cache` を記録する。
  cache receipt と Finalized は ROOT が正本 owner に受理した結果だけを記録し、Finalized の queue 所属は
  受理直前の同一 ID の投影を使う。outer item の key を導出できない経路は証拠を捏造しない。
- tray close は loading mode と正本の worker activity を確認し、実 `WM_CLOSE` を送る。
  その後の準備待ちでは Observe を許すが、close / hide / 同一 ID の格納後 worker begin / source-save skip /
  Finalized 実画像 queue 所属がすべて成立するまで `ready.json` を出さない。成立後の一つの idle sleep では
  Observe / UI command を送らない。外部計測開始への UTC anchor 変換による deadline 判定も維持する。
- §1.360 の passive `modifier_probe` は master の idle 集計を再利用し、入力集計コードには変更を加えない。
  §1.356 の配送 gate に合わせて perf 判定は正常終了後の保存済み shutdown checkpoint を使う。
  sampler は外部 CPU / log / window の測定と時計変換のみを担当し、稼働中に配送未確定の perf 判定をしない。
  欠損配送は SKIPPED / 2、独立に観測済みの失敗は環境不成立を併記しても FAIL / 1 を維持する。
- tray fixture は perf fixture と分離した `release-checks-v2`。各 mode に **600 subfolder × 1枚の
  400×300 PNG、10列、Cache Off** を用意する。停止時の読み取り専用 preflight に tray 親 catalog 2個を追加し、
  close 直前は kept target の cache 不在を再確認する。perf の 512×4、static / navigation / video / search は維持する。
  PNG 内容は既存の決定的 encoder を使用する。A の実機記録
  `C:\home\mimageviewer-nexttray\target\A-1360-agent-hwcheck.md` はこの小画像形状で旧版 FAIL / 修正版 PASS
  （両 mode）、6000×4000 では非再現を確認済みだが、F suite での再確認は Phase 4 に残す。

非対話検証は analyzer / 既存 perf / fixture / preflight Python、safety fake、正本 projection と tray barrier の
feature 有無の Rust 焦点テスト、normal / portable check、full lib、fmt / diff check。
引き継ぎ先は `target/F-1361-3b-msg.txt`。新規ファイルは test-only の
`src/test_script/release_checks/tray_witness.rs`。アプリ起動・Phase 4・commit はこの接続作業に含めない。

### Phase 4 初回起動の blocker 修正（2026-10-11）

- 了承済みの初回隔離 run `release-checks-20261010-151540-f99fc454` は seed 起動で停止。
  Run の `<run-id>/<case>` と screenshot の旧 `ui-smoke-runs` 直下 guard が不一致だった。
  上記の固定パス契約に集約し、ReleaseChecks の階層は維持する。任意の descendant は許可しない。
- Run の既存 process handle 保持修正は維持。`ui-smoke.ps1` の準備／解析 helper、初回起動
  （button-helper 起動を含む）、AlwaysOnTop 再起動にも終了前の handle 保持を適用する。
  再起動の null ExitCode を整数 0 へ変換して成功と扱わない。
- 常駐版と共有する Remote pipe の起動拒否は環境注記として
  [検証記録](release-verification-records.md) に残す。
  この suite の入力・証拠は Remote IPC に依存しない。pipe 変更、retry、常駐版停止は追加しない。
- 今回は非対話の safety fake、Python analyzer／fixture／preflight、焦点 Rust、fmt、
  normal／portable check のみ。アプリ起動、ui-smoke、commit、独立レビューは行わない。
  初回 run を成功へ読み替えず、修正後の Phase 4 実測・採用確認は未実施。
  修正差分と検証証拠の引き継ぎ先は `target/F-phase4-fix-msg.txt`。

### Phase 4 seed の root 選択修正（2026-10-11）

- 利用者実行の `154647-a89c39d2` は最初の snapshot より先に `select_root`、
  `160117-cd52b366` は窓の公開後・identity の公開前に選択して失敗した。
  `160307-9c1116e4` は root に detached 専用の `host_incarnation` を要求して timeout。
  timeout 診断には実 root owner が既に存在し、27 frame の後に約29秒静止していた。
- root identity は main context serial / HWND / backend token。`host_incarnation` は常に absent。
  serializer の `host_ready` は `select_root` が使う実 identity の存在を表すので、
  全 case 共通の開始待機を `root_selectable(snapshot)`（root かつ host_ready）へ合わせる。
  snapshot の受動待機だけでよく、bootstrap repaint、偽 incarnation、identity の検証緩和は不要。
- `idle198-convergence.rhai` も同じ host_ready 契約を使う。`always-on-top.rhai` は選択前に
  操作・capture を行うため、即時 select の startup race の参考にはならない。
- 実 serializer による root / identity 未公開 / stale scalar / detached の回帰と、
  遅延公開まで wake せず実 recipe の bootstrap と select_root を実行する headless 回帰を追加。
  Rust の製品 plumbing と窓公開経路は変更しない。

後続の静的確認（実アプリの完走証拠ではない）:

| 対象 | 公開・待機の経路 |
| --- | --- |
| 共通 index settle | 選択後の Observe は fresh owner 検証と UI queue の wake を行い、実 index 状態を読む |
| seed cache / video pin / search | Open 後の Observe で folder / loading / index を待ち、通常 pin producer の保存を確認。Ctrl+G の受理と TextEdit focus 後に text event を送り、検索の query / done / searching / hits を確認 |
| static 2件 / video-pin-background | 同じ Open / settle を使い、pause 前に実 readiness を確認 |
| tray 2件 | close 前の実 loading / cache 不在と exact PID/HWND を確認。非表示後も update の早期 return より前に UI request が処理され、既存 request ID witness が成立するまで Observe を行う |
| perf 2件 | startup の many-folder を再 open せず、ready 後に navigation の実 Ctrl+Down と generation/path の変化を待つ。検索も seed と同じ通常入力経路 |
| optional first-ai | root 選択後、通常 fullscreen / AI action と Observe で実 ready / backend を確認。今回の必須8 sessionとは別の了承範囲 |
| idle / evidence / exit | `ready.json` 後は既存の単一 interrupt wait と sampler 完了待ちだけで、Observe・UI command を送らない。計測完了後に evidence を読むための wake と通常 tray Quit を行う |

非対話検証と診断 portable の再準備は `target/F-phase4-fix2-msg.txt` へ記録する。
修正後の実 suite 完走・採用は利用者実行待ち。アプリ起動、ui-smoke、commit は行わない。
