# アイドル健全性チェック

## 1. 目的

静止中・背面表示中に、同じ work の再投入や不要な repaint が高速で続く回帰をリリース前に
検出する。通常のヒッチ検査は「1 フレームが遅い」問題向けであり、今回の動画ピンのように
各処理は短いが 1 コアを使い続けるループは見逃す。そのため、次の 2 系統を同じ測定区間で
検査する。

- `--perf-log`: `frame.begin`、`ui.tail_repaint`、thumbnail work の反復を解析
- プロセス外 sampler: CPU time、`mimageviewer.log` / `perf_events.jsonl` の増加量を測定

アプリ内に周期 heartbeat を置くと、正常時にもアプリ自身を起こしてしまう。完全に sleep して
測定区間内の perf event が 0 件になる状態を正常として扱うため、wall time と CPU は
PowerShell 側で測る。

## 2. 実行方法

先に release verification binary を作る。

```powershell
.\scripts\build-release.ps1
```

最初のシナリオはスクリプトが `mimageviewer-core.exe --perf-log` を起動する。画面を準備して
Enter を押すと、5 秒 warmup 後に 15 秒測定する。測定中はマウス・キーボードへ触れない。
前面シナリオは Enter 後の warmup 中に mImageViewer へフォーカスを戻す。背面シナリオは
コンソール等を前面のままにする。warmup 中の入力は測定対象外で、測定開始表示後の入力だけが
idle 測定を無効にする。シナリオ名に `foreground` / `background` が含まれる場合は、測定開始・
終了時の foreground process ID も検査し、実際の状態と名前が一致しなければ FAIL にする。

```powershell
.\scripts\check-idle-health.ps1 -Scenario static-foreground
```

同じプロセスで続ける場合は `-NoLaunch` を使う。背面シナリオでは、別ウィンドウを前面へ
移してから Enter を押す。

```powershell
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario static-background
.\scripts\check-idle-health.ps1 -NoLaunch -Scenario video-pin-background
```

既存の対象プロセスを明示する場合は `-ProcessId <PID>` を使う。そのプロセスが
`--perf-log` なしで起動されていた場合、stale perf log を誤解析せずエラーにする。

結果は `target/idle-health/` に 2 ファイル出る。

- `*-perf.json`: update / repaint / work 反復の解析結果
- `*-process.json`: CPU / ログ増加量と、両検査を統合した最終 PASS / FAIL

スクリプトは失敗時に exit 1 を返す。起動した mImageViewer は追加シナリオを続けられるよう
終了させないので、検査後に通常操作で完全終了する。

## 3. リリース前シナリオ

毎リリース、最低限次を実行する。

1. 通常画像フォルダのロード完了後、前面で静止 (`static-foreground`)
2. 同じ状態を背面へ移して静止 (`static-background`)
3. 動画を代表画像に固定したフォルダを keep 範囲内へ置き、背面で静止
   (`video-pin-background`)

ZIP / PDF / スマートフォルダ、AI、動画・音楽などの非同期経路を変更したリリースでは、対象の
ロード・解析が完了した状態も追加する。動画再生中、スライドショー中、索引・解析中は継続
処理が正当なので、静止シナリオへ混ぜない。

## 4. 判定値

正本は `scripts/idle_health_thresholds.json`。初期値は、今回観測した 1 論理コア相当の
ループを十分な余裕で検出しつつ、OS の一時的な repaint を許容する値にしている。

| 指標 | 目標 / 上限 | 意味 |
| --- | --- | --- |
| CPU one-core ratio | 目標 0.05 / 上限 0.10 | `CPU time delta / wall time`。1.0 が論理コア約 1 本 |
| update rate | 目標 2/s / 上限 10/s | 測定区間の `frame.begin` 件数 / wall time |
| repaint reason streak | 上限 2 秒 | 同じ `requested_nonempty` 等が 500ms 未満の gap で継続した時間 |
| same thumbnail work | 上限 3 回 | kind + key + idx + items generation が同一の work 件数 |
| input event | 上限 0 | 測定中に操作が混ざった場合は無効な idle 測定として失敗 |
| 通常ログ増加 | 上限 16 KiB / 15 秒 | 非構造化ログの高速肥大を検出 |
| perf ログ増加 | 上限 256 KiB / 15 秒 | 未知のイベントループも量で検出 |

正常版で各シナリオを 3 回程度測定し、JSON report を残して環境内の分布を確認する。閾値を
調整するときは単に失敗を消すのではなく、`top_repaint_causes`、reason streak、repeated work
から正当な継続処理かを確認する。静止シナリオで継続理由が説明できなければコード側を直す。

## 5. 計装と自動テスト

アイドル高画質化は候補を最終解決した時点で次を emit する (`--perf-log` 時のみ)。

- `thumb.idle_upgrade_enqueue`: 最終 `skip_cache=true` で upgrade queue へ進む
- `thumb.idle_upgrade_ineligible`: 完成済み派生キャッシュのため対象外

どちらも `key`、`idx`、`items_gen` を持つ。同じ identity が入力・items 世代変更なしに
繰り返されると `idle-health` が失敗する。解析ロジックの回帰テストは次で実行する。

```powershell
python scripts\test_analyze_perf.py
```
