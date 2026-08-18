# backlog §5.4 — idle health のゲートが手順どおりでも落ちる 3 件

対象: [next-release-backlog.md](../next-release-backlog.md) §5.4。v3.1.1 のリリース前確認で
`check-idle-health.ps1` を 3 回走らせて 3 回とも FAIL したが、**製品の静止性は 3 回とも正常**
だった。落ちているのはゲート側である。次版 (v3.1.2) で片付ける。

3 件は独立した欠陥で、**それぞれ別の原因**を持つ。まとめて 1 つの guard で塞がない。

## A. 測定対象プロセスの同一性判定が循環している

`-NoLaunch` は perf log の先頭から `session.start` を読み、その `pid` のプロセスへ接続する
([check-idle-health.ps1:191](../../scripts/check-idle-health.ps1:191))。そして同じ PID を
`--expected-pid` として analyzer へ渡し、analyzer は「perf log の session PID が
expected_pid と一致するか」を確認する ([analyze_perf.py:1889](../../scripts/analyze_perf.py:1889))。

**log から取った値を log と突き合わせているので、この検査は常に成立する。何も証明していない。**

v3.1.1 の 1 回目はこれで `%APPDATA%\mimageviewer\runtime\3.1.1\` の core
(ランチャー版が展開した別 instance、`--perf-log` 無し) に接続し、perf を 1 行も書かない
プロセスに対して `events=0` を「完全 sleep」と報告した
([analyze_perf.py:1907](../../scripts/analyze_perf.py:1907) の warning 文言は
「同一 session PID は確認済み」と、確認していない内容を主張している)。

### やること

1. **log と独立した証拠でプロセスを検証する**。`Get-CimInstance Win32_Process -Filter
   "ProcessId=<id>"` の `CommandLine` に `--perf-log` が含まれることを要求する。
   含まれなければ **throw** する (計測を始めない)。文面は「そのプロセスは perf log を
   書かないので測定できない。`--perf-log` 付きで起動した instance を指定するか
   `-NoLaunch` を外す」。
2. **ランチャー展開コピーを測定対象にしない**。exe path が
   `%APPDATA%\mimageviewer\runtime\` 配下なら throw する。これは配布ランチャーが
   展開した実体で、この確認の対象ではない。
3. 検証は **3 つの接続経路すべて** (`-ProcessId` / `-NoLaunch` / 自前 launch) で通す。
   自前 launch では自明に成立するが、経路ごとに分岐を増やさず 1 箇所で行う。
4. process report JSON に `command_line` と `path` を残す。
5. analyzer の warning 文言を、**実際に確認した内容**へ直す。「同一 session PID は確認済み」
   ではなく「外部 sampler が同一プロセスであることを確認済み」相当にする。
   (analyzer 単体では PID 一致しか見ていないので、断定しない書き方にする。)

**やらない**: perf log growth が 0 であること自体を FAIL 条件にしない。正しく sleep した
測定では growth 0 が期待値であり、v3.1.1 の 3 回目は実際に growth 0 で PASS 相当だった。
区別すべきは「書かない instance か」であって「書かなかったか」ではない。

## B. 起動直後の初回索引スキャンと測定がぶつかる

v3.1.1 の 2 回目は起動 11 秒後に測り、全文検索の initial scan (52 万 / 11 万 / 6 万ファイル)
と並走して CPU one-core ratio 2.302 で FAIL した。索引は text log にしか書かないので
perf 側は静かなまま CPU だけが跳ね、**原因が報告から読み取れない**。

手順書は「起動後に準備して Enter」としか言っておらず、**手順どおりに操作しても落ちる**。

### やること

1. **初回スキャンの完了を perf event にする** (製品側、小さい)。
   - 各 supervisor の完了地点で 1 件ずつ:
     [indexer_supervisor.rs:329](../../src/indexer_supervisor.rs:329) (`initial_scan_done = true`
     の直後) と [name_index_supervisor.rs:306](../../src/name_index_supervisor.rs:306)。
     `cat="index"`, `kind="initial_scan_done"`、属性に索引種別 (`fts` / `name`) と
     favorite id、所要 ms を付ける。**worker thread から出す** (frame に依存しない)。
   - 集約の 1 件: App 側で「FTS 全 supervisor が `all_supervisors_idle()` かつ
     name_index 全 supervisor が `initial_scan_done`」が**初めて true になった 1 回だけ**
     `cat="index"`, `kind="initial_scan_settled"` を出す。
   - 置き場所は [poll_housekeeping_arm](../../src/app.rs:19045)。ただし現在の実装は
     `housekeeping_armed` が false なら先頭で return するので、**housekeeping の one-shot と
     perf event の one-shot を別のフラグにする**。housekeeping の発火条件は変えない
     (FTS だけを見る現状のまま)。name_index を housekeeping の条件に足さないこと。
   - supervisor が 0 個のとき (auto index するお気に入りが無い) は述語が即 true になり、
     イベントも即出る。これが正しい。
2. **script は prompt の前にこのイベントを待つ**。perf log を読み、
   `index.initial_scan_settled` が現れるまで待機する。
   - **待ちの上限で黙って先へ進まない**。上限に達したら `Write-Warning` で
     「初回索引スキャンの完了を確認できないまま測定する」と明示し、process report に
     `index_scan_wait = "timeout"` を残す。FAIL にはしない (索引を使わない構成でも
     このチェックは回せるべきなので)。確認できた場合は `"settled"`、
     既に完了済みだった場合も `"settled"` でよい。
   - 上限値は `idle_health_thresholds.json` に `index_settle_wait_seconds` として置く
     (既定 180)。script にハードコードしない。
   - **これは競合を時間窓で吸収する話ではない** (憲法 §2 規則 5 に当たらない)。完了を表す
     イベントを待っており、上限は「待つのをやめて人へ報告する」ための上限でしかない。
     この区別をコメントに残すこと。

## C. video-pin の evidence 窓が狭い

ゲートは「evidence 窓の**中で**対象タイルが keep 範囲へ入ったこと」を要求するが、
シナリオが必要とするのは「測定中に対象タイルが keep 範囲に**あること**」。先に入って
居残っているタイルも等しく正しいセットアップである。

`-NoLaunch` で 3 シナリオを連続実行すると、対象タイルの work は 20 秒前に終わっていて
窓の外になり、`matched=0` で必ず FAIL する。v3.1.1 の 3 回目がこれ
(1 つ前の窓では `matched=64` を記録済みだった)。

### やること

1. evidence 窓の下限を **セッション全体**へ広げたうえで、**`end_t` 以前で最後の
   `nav.load_folder_begin` (`cat="nav"`) 以降**に対象 key の thumbnail work があることを
   条件にする。これで「今表示している場所のタイルである」と言える。
2. `nav.load_folder_begin` が 1 件も無い場合は session 先頭 (`t=0`) を下限にし、
   判定根拠を report に残す。
3. report JSON に `evidence_floor_t` と
   `evidence_floor_basis` (`"last_load_folder_begin"` / `"session_start"`) を出す。
4. **`--evidence-start-t` は削除する** (analyzer と script の両方から)。下限は analyzer が
   events から導出するので、外から渡す口を残さない。死んだ引数を残さないこと。
5. `--require-work-key` の空文字禁止と、`idle_upgrade_*` が無いときの warning は現状維持。

## テスト

`scripts/analyze_perf.py` の idle-health は Python で単体テストできる。既存テストの場所に
合わせて追加する (無ければ関数を直接呼ぶ test を作る)。

1. C: 最後の `nav.load_folder_begin` より前にしか対象 work が無い → FAIL。
   後にあれば測定区間の外でも PASS。`nav.load_folder_begin` が無い log では session 先頭が
   下限になる。
2. C: `evidence_floor_basis` が両方の場合で正しく出る。
3. A: analyzer の warning 文言が変わったこと (文言 assert ではなく、
   「PID 一致だけを根拠に sleep と断定しない」ことを確認できる形で)。
4. B: 製品側。`initial_scan_settled` が**1 回だけ**出ること、supervisor 0 個で即 true に
   なること、housekeeping の発火条件が変わっていないこと。
5. 既存の idle-health テストが無修正で通ること。**赤くなったら報告して止まる。**

PowerShell 側はテストを書かない (実行環境依存)。代わりに `-SkipPrompt` で
A の 2 つの throw 条件を手で確認し、結果を報告に含める。

## やらないこと

- 閾値 (`max_cpu_core_ratio` 等) を緩めない。今回の FAIL は閾値の問題ではない。
- B のために製品の索引スケジュールを変えない。イベントを足すだけ。
- housekeeping の発火条件 (FTS のみ) を変えない。
- 時間窓で競合を吸収しない。B の待ち上限は「人へ報告する上限」であり、判定条件ではない。

## ドキュメント

- [idle-health-check.md](../idle-health-check.md) §3 を実装に合わせる。特に
  「連続実行時は開き直す」という手順依存の注意書きは、C を直したら**不要になるので消す**。
  B の待ちについて、何を待っていて timeout 時に何が起きるかを書く。
- [next-release-backlog.md](../next-release-backlog.md) §5.4 に結果を追記して閉じる
  (エントリ末尾に追記。冒頭の記述を消さない)。

## 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
python scripts/analyze_perf.py --help
```

commit / stage はしない。ブランチは `master`。
報告には変更ファイル一覧、追加テストの一覧、A の手動確認結果を含める。
