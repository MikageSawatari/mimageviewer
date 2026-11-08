# 夜間の組み合わせ実アプリ試験 (設計案)

2026-11-08 作成。状態: **設計案・独立レビュー前・未実装**。設計担当 ClaudeCode Opus 5.5。

## 1. 目的

「mIV は不安定」という声がある。機能が多く、設定 × 開く内容 × 操作順の組み合わせを
出荷前にほとんど試せていないことが原因の一つと考える。単体テスト・headless の複数
viewport テスト (第 1 層)・承認つき実アプリ smoke (第 2 層、[ui-smoke](ui-smoke-automation-plan.md))
は「決めた筋書き」しか通らない。

この設計は、**利用者の操作や承認なしに**、隔離した使い捨て環境で次を夜間に繰り返す仕組みを作る。

1. 設定・内容・操作の組み合わせを機械的に作って実アプリで流す
2. 落ちた・固まった・不変条件が破れたものを集め、同じ原因ごとにまとめる
3. 再現する手順を短く縮めて、朝に一覧で渡す

見つけた不具合の修正は、従来どおり通常の開発手順 (設計・Codex 実装・独立レビュー) で行う。
この仕組みは**見つけるところまで**を受け持つ。

## 2. 確認済みの前提

### 2.1 Claude Code の権限まわり (公式ドキュメント、2026-11-08 時点)

- Windows の CLI 版 Claude Code では画面操作 (computer use) を使えない。CLI 版は macOS 専用で、
  `claude -p` (無人実行) でも使えないと明記されている。Windows で使えるのは Desktop アプリ版だけで、
  アプリごとの承認が出る。確認なしで動くモードでこの承認が省かれるかは記載がない。
- `--permission-mode bypassPermissions` は Windows でも使える。公式は「隔離された環境
  (コンテナ・VM) でのみ使う」よう求めている。ネイティブ Windows では Bash のサンドボックスも無い。
- 以上から、**試験の実行ループに Claude を入れない**。実行は PowerShell と mIV 自身の
  `--test-script` だけで完結させ、Claude は結果の整理 (§8) と任意の探索試験 (§9) だけを受け持つ。

### 2.2 Windows Sandbox での無人実行 (サブ PC)

メイン PC には Windows Sandbox が無い (`WindowsSandbox.exe` 不在)。サブ PC (DESKTOP-4060TI) にはある。

| 日付 | 内容 | 結果 | 観測者 |
| --- | --- | --- | --- |
| 2026-11-07 | `C:\miv-sandbox\v500.ps1`: ホストのスクリプトが `.wsb` を起動 → Sandbox 内の LogonCommand がインストール・起動・監視・画面保存・ログ回収 → ホストへ結果を戻す | 人の操作なしで完走 (約 5 分半) | サブ PC 担当 (スクリプト出力) |
| 2026-11-08 | `C:\miv-sandbox\probe\`: Sandbox 内から SendInput でフォームへクリックと文字入力。1 回目は Sandbox 窓を表示、2 回目はホストのスクリプトが窓を最小化 | 両方で入力が届いた。最小化中の画面保存も正常 (黒画面ではない)。約 50 秒 | サブ PC 担当 (スクリプト出力)、メイン PC の ClaudeCode が保存 PNG を確認 |

この 2 件から分かること:

- Sandbox 内の LogonCommand は**管理者として**動く (2026-11-08 の画面保存で窓の題が
  `Administrator: Windows PowerShell`)。利用者の通常の起動と揃えるため、mIV は昇格なしで起動する
  (11-07 のキットは `explorer.exe` 経由で起動している)。
- Sandbox 内の入力 desktop は `Default`、mIV が前面を取れる。Codex サンドボックスで起きた
  「入力 desktop の不一致」([実行確認](interactive-release-verification.md) 末尾) は Sandbox 内では起きていない。
- ホストの Sandbox 窓が最小化されていても、Sandbox 内の入力と描画は続く。

**まだ確認していないこと** (S0 で確かめる):

- サブ PC の利用者セッションが**切断・ロック**されているとき (RDP を閉じた後) に、タスク
  スケジューラから Sandbox を起動でき、中の入力が届くか。2 件とも RDP 接続中の実行。
- `--features portable,test-script` の mIV が Sandbox 内で `--test-script` を完走するか。
  vGPU 有効 / 無効 (WARP) の両方。
- RDP 無しでメイン PC とサブ PC の間で成果物と結果を受け渡す方法 (`\\tsclient` は RDP 接続中だけ見える)。

### 2.3 mIV 側の自動操作の仕組み (コードから)

既存の `--test-script` (Rhai、[src/test_script.rs](../src/test_script.rs)) をそのまま実行器にする。要点:

- 起動: `--test-script <file.rhai> --test-evidence-dir <dir> --data-dir <dir>` と、開く場所の位置引数。
  `--settings-override <json>` で `Settings` の直列化済み項目を深い merge で上書きできる
  (allow-list なし、存在しない項目は exit 2)。単一起動の確認と常駐 listener は script 実行時に省かれる。
- 結果: exit code (0 成功 / 1 script 失敗・`wait_until` timeout / 2 環境失敗)、`logs/mimageviewer.log` の
  `[test-script] finished` 行、`--perf-log` 時の perf event、失敗時の全 viewport capture。
- 操作語彙: `tap_key` / `hold_key`、`run_action(<KeyAction ini 名>)` (consumer の受理まで待つ)、
  `click_widget` / `double_click_widget` / `hover_widget`、`select_root` / `select_window`、
  `root_history_input`、`always_on_top_smoke` などの smoke 操作、native 動画の実マウス入力、clipboard 系。
  観測は `snapshot()` と `wait_until(述語, timeout)`。
- 制約: `import` / `eval` は無効 (script は 1 ファイルに連結する)。任意の path を開く命令は無い
  (`release_check_open` は release-check session 内だけ)。共通の「安定した」判定は無い (各 script が自前で待つ)。
- 失敗の痕跡 (いずれも `--data-dir` 配下): `logs/panic.log` (panic hook、`UI THREAD HANG suspected`)、
  `logs/native-exception.log`、`logs/mimageviewer.log`、`logs/perf_events.jsonl`。**minidump は作らない**。
- 試験用ビルドは `prepare-portable-smoke.ps1 -TestScript` → `build-portable.ps1 -SmokeTestScript`
  (`target-portable-test-script`、`.test-script-build.json` に source の指紋と exe の SHA256)。
- 組み合わせの生成器・pairwise 展開はまだ無い。[複数ウィンドウのシナリオ試験](multiwindow-scenario-test-plan.md)
  §3.4 / §6 に「fixture × 設定 × 操作順を pairwise で」という残作業 (T2) として書かれている。この設計はその T2 を
  夜間の無人実行として実装する。

## 3. 全体構成

```
メイン PC                               サブ PC (ホスト)                  Windows Sandbox (使い捨て)
-----------                             ----------------                  ---------------------------
試験用ビルドを作る ──(受け渡し)──▶  タスクスケジューラ (夜間)  ──.wsb──▶  LogonCommand (管理者)
fixture pack を作る                      └ combo-host.ps1                    ├ WER LocalDumps を設定
                                            ├ 回ごとに Sandbox を起動          └ explorer 経由で runner を昇格なしで起動
朝: 結果を読む ◀──(受け渡し)──────   ├ 結果を回収                          combo-runner.ps1
   ClaudeCode が整理 (§8)                  └ 時間枠の終わりで止める               ├ 生成器で scenario を作る
                                                                              ├ mIV --test-script を 1 本ずつ起動
                                                                              ├ 判定 (§6) と痕跡の採取
                                                                              └ 失敗は縮小・再現確認 (§7)
```

- **1 回の Sandbox 起動 = 1 batch** (目安 60 分、上限を設定)。batch が終わると Sandbox は自分で
  シャットダウンし、ホストが結果を回収して次の batch を新しい Sandbox で始める。環境の汚れは
  batch 単位で必ず消える。
- 1 batch の中では scenario ごとに `--data-dir` を作り直す。scenario 同士で設定・キャッシュ・DB を共有しない。
- Sandbox は同時に 1 つしか起動できない。並列化はしない。

## 4. 生成器: scenario の作り方

### 4.1 scenario の中身

1 本の scenario は次の 4 つで決まる。これを `scenario.json` として保存し、同じものを何度でも再実行できる。

| 要素 | 内容 |
| --- | --- |
| build | 試験用ビルドの `.test-script-build.json` の SHA256 |
| fixture | fixture pack の版 (§4.4) |
| settings | 既定値からの差分 (settings-override JSON) |
| steps | 操作の線形な列。各 step は「操作」と「その操作を出してよい状態の条件」 |

### 4.2 設定の組み合わせ: 2 つずつを漏れなく

設定は**軸**のカタログ `scripts/combo/axes.toml` に書く。1 軸 = 利用者から見た 1 つの選択
(例: 見開き、読み方向、ビューワモード、サムネイル比率、AI 処理なし/軽量、キャッシュ方針、ファイル種類の表示)。
各値は settings-override の断片に対応させる。

- 毎晩、全軸の**2 つずつの値の組 (pairwise)** を最低 1 回含む設定集合を、seed つきの貪欲法で作る。
  総当たりより桁違いに少ない数で、2 つの設定の組み合わせで起きる不具合を拾える。
- 既知の危ない組 (過去の不具合の組) は「必ず含める組」としてカタログに明記する。
- **カタログの検証は cargo test にする**。全軸・全値の断片を既定の `Settings` に本番と同じ
  merge 関数で当て、`UnknownSetting` や型違いが無いことを確かめる。設定名を変えたら試験が落ちて気づける
  (§1.239 で Stills が exit 2 になった再発を防ぐ)。
- 未リリースの設定・`serde(skip)` の項目は対象外 (上書きできない)。

### 4.3 操作の列: 状態に応じて選び、実行した列を記録する

生成器は App の実行時状態を知らないので、**操作の選択は script 内で行う**。

- 生成器は seed と重みつきの操作語彙を埋め込んだ Rhai script を 1 本作る (helper は連結する)。
- script は step ごとに `snapshot()` を見て、その状態で出せる操作 (例: 一覧なら開く・列数・並べ替え・
  F12、フルスクリーンならページ送り・見開き切替・閉じる、別窓があれば窓の選択) から、
  script 内の決定的な擬似乱数で 1 つ選ぶ。
- 実行した操作は 1 行ずつ `log()` で記録する。**再現と縮小は、この記録した線形の列で行う**
  (seed からの再生成に頼らない。タイミングで状態が変わると同じ seed でも別の列になるため)。
- 1 本の長さは 20〜60 step、時間上限 3 分を目安にする。長い 1 本より短い多数の方が縮小が速い。

### 4.4 内容 (fixture pack)

既存の fixture 生成器 (`scripts/ui-smoke/` と `scripts/ui-smoke/page-turn/` の Python、`testdata/`) を流用し、
メイン PC で 1 つの fixture pack にまとめる。中身は 1 つの root フォルダの下に、静止画フォルダ、
ZIP、PDF、RAR/CBR、動画 (ffmpeg で生成)、音声、sidecar つきフォルダ、画像だけのフォルダを並べたもの。
コレクション・★は `--seed-db` で data に入れる。

- 開く入口は「起動時の位置引数 = pack の root」とし、そこからの移動は一覧の操作で行う。
  任意 path を開く命令 (`open_path`) は、一覧操作でたどれない入口が必要になったときだけ足す
  ([複数ウィンドウ試験](multiwindow-scenario-test-plan.md) の方針と同じ)。
- pack は SHA256 つきの版で配り、scenario に版を記録する。

### 4.5 GPU の有無も軸にする

`.wsb` の `vGPU Enable/Disable` を batch 単位の軸にする。Disable は WARP (ソフトウェア描画) になり、
GPU の弱い環境に近い。ただし描画が遅いので、待ち時間の上限は GPU 無しの batch だけ長くする (§6.3)。

## 5. 実行器

### 5.1 ホスト側 (サブ PC): `combo-host.ps1`

- タスクスケジューラから起動。時間枠 (例 01:00〜07:00) の中で batch を繰り返し、枠の終わりで新しい
  batch を始めない。
- batch ごとに: 受け渡し場所から最新の試験用ビルドと fixture pack を Sandbox 用フォルダへ置く →
  `.wsb` を起動 → 終了印を待つ (上限つき) → 結果を受け渡し場所へ戻す。
- 上限を超えたら Sandbox を止めて「batch timeout」として記録し、次へ進む。再試行の仕組みは作らない。

### 5.2 Sandbox 内: LogonCommand と `combo-runner.ps1`

- LogonCommand (管理者) は 2 つだけ行う: WER LocalDumps (`HKLM\...\Windows Error Reporting\LocalDumps\mimageviewer-core.exe`)
  を設定して落ちたときの dump を取れるようにする、`explorer.exe` 経由で runner を**昇格なしで**起動する。
- runner は**Sandbox の中でしか動かない**ことを自分で確かめる (利用者名 `WDAGUtilityAccount` と
  キットの印)。合わなければ何もせず終わる。これで実 PC で誤って動かすことを構造的に防ぐ。
- runner は scenario を 1 本ずつ: data を作り直す → mIV を起動 → 終了または timeout を待つ →
  判定と痕跡の採取 → 次へ。batch の残り時間が少なくなったら新しい scenario を始めない。
- 既存の `ui-smoke.ps1` は流用しない (exe と data の場所が固定、承認 flag 前提、Sandbox を想定していない)。
  起動引数・痕跡の保存形式・timeout 時の exact PID kill など、共通にできる部分は関数として切り出して両方から使う。

### 5.3 受け渡し

RDP 無しで動くよう、受け渡しは共有フォルダにする。案: サブ PC に共有 `miv-combo` (中に `drop\` と `results\`) を作り、
メイン PC が `drop\` へ書き、`results\` を読む。共有の作り方は利用者の環境判断 (§11 Q3)。

## 6. 判定 (何を不具合とみなすか)

### 6.1 共通の不変条件 (script 内、各 step の後)

各 step の後に**共通の安定待ち**を置く (複数ウィンドウ試験の残作業「安定化判定の共通化」をここで作る)。
安定 = 読み込み待ち・サムネイル待ちが空、現在ページの描画が現在の状態と一致、の両方。
安定した時点で次を確かめる。

| 記号 | 内容 |
| --- | --- |
| C1 | 安定待ちが上限内に終わる (終わらなければ「収束しない」として失敗) |
| C2 | フルスクリーンならページ位置が範囲内で、描画が現在ページと一致する |
| C3 | 開いた窓は、描くか自分の窓のエラー表示まで消えない (既存の I1) |
| C4 | 窓の切り替えだけで、どの窓の描画も変わらない (既存の I2、別窓がある step だけ) |
| C5 | `run_action` が consumer に届く (届かなければ既存どおり環境失敗) |

最後に全部を閉じて、6 秒の終了 watchdog 内に exit 0 で終わることも確かめる。

### 6.2 プロセスの外から見る痕跡 (runner)

| 痕跡 | 判定 |
| --- | --- |
| exit code ≠ 0 / timeout | 失敗 (exit 2 は環境失敗として別に数える) |
| `panic.log` に新しい記録 | 失敗。`docs/panic-acknowledged.tsv` にある既知のものは既知として分ける |
| `UI THREAD HANG suspected` | 失敗 (固まり) |
| WER dump の生成 | 失敗 (native crash) |
| `native-exception.log` | 単独では失敗にしない (first-chance 例外は正常系でも出る)。失敗時の手がかりとして保存する |

### 6.3 待ち時間の上限

上限は「遅い」を不具合と誤判定しない値にする (GPU 無しの batch は長くする)。性能の退行は
この仕組みの対象外 (§10)。

### 6.4 同じ原因をまとめる (署名)

失敗ごとに署名を作り、同じ署名をまとめて数える。panic は発生場所 (file:line)、native crash は
dump の例外コードと module+offset、不変条件違反は C 番号と直前の操作名、固まりは止まっていた
直前の操作名。

## 7. 縮小と再現確認

失敗した scenario は、同じ batch の残り時間で次を行う (時間が足りなければ翌晩へ持ち越す)。

1. **再現確認**: 記録した線形の列をそのまま 3 回流す。1 回も再現しなければ「1 回だけ」として別欄に載せ、縮小しない。
2. **操作の縮小**: 列を半分・4 分の 1…と削っていき (ddmin)、同じ署名で落ちる最短の列を探す。
3. **設定の縮小**: 既定値からの差分を 1 項目ずつ既定に戻し、落ちなくなる項目だけを残す。
4. 縮小結果を `minimal.rhai` + `settings-override.json` として保存し、手元の `ui-smoke` と同じ形式で
   再実行できるようにする。

縮小の再実行回数には上限を置く。上限に達したら、その時点で最短のものを出す。

## 8. 朝の報告と Claude の役割

- batch の結果は `results/<日付>/` に、`summary.json` (scenario 数、成功・失敗・環境失敗の数、署名ごとの件数) と、
  失敗ごとのフォルダ (縮小済み script・設定・ログ・画面・dump) で置く。
- 朝、メイン PC の ClaudeCode が `summary.json` と新しい署名を読み、次を書いた短い報告を作る:
  新しく見つかった署名、増えた・消えた署名、原因の当たり (該当コードの参照)、バックログ案。
  読むだけの作業なので確認なしのモードは要らない (Desktop の定時タスクか、利用者が会話で頼む)。
- バックログへの登録・修正の着手は利用者の判断を経てから行う。修正の検証は、縮小済み script を
  夜間に流し直して「その署名が消えた」ことで確かめる。
- 報告の事実は出どころを書く (CLAUDE.md「観測の出どころを落とさない」)。この仕組みの観測者は
  「夜間試験 (Sandbox、ビルド <SHA>)」と書く。

## 9. 任意: Claude による探索試験 (補助)

決められた語彙では届かない操作 (メニューを開いて項目を選ぶ、ダイアログの中を触る) を、Claude が
画面を見ながら試す段。§2.1 のとおり Windows の CLI に画面操作は無いので、次の形にする。

- Sandbox 内で Claude Code CLI を `bypassPermissions` で動かす (Sandbox は使い捨てで、公式の
  「隔離環境」の条件に合う)。
- 画面操作は**自前の小さな MCP サーバ**で渡す: 画面保存、クリック、キー入力 (SendInput)、
  mIV の終了。対象は mIV の窓だけに限る。2026-11-08 の確認で、Sandbox 内の SendInput は届いている。
- Claude には「操作して、おかしな挙動を見つけたら手順を記録する」役割だけを与え、見つけた手順は
  §7 と同じ縮小にかける (再現できるものだけ報告する)。

前提と費用 (利用者判断、§11 Q5):

- Claude Code の実行ファイルは Sandbox の起動ごとに置く必要がある。サブ PC はネットから取得しない
  決まりなので、メイン PC から受け渡し場所へ置く。
- 認証は利用者が `claude setup-token` で作る専用のトークンを受け渡し場所に置く。トークンは取り消せるものを使い、
  Sandbox はネットワーク有効にする必要がある (探索試験の batch だけ)。
- 画面を 1 回見るごとに利用枠を使う。決定的な試験 (§4〜§7) より費用がかかるので、決定的な試験が回り始めてから
  1 晩あたりの上限を決めて始める。

## 10. 範囲外

- 性能の退行 (遅くなった) の検出。待ち時間の上限は誤判定を防ぐためだけに使う。perf の比較は既存の
  ReleaseChecks に任せる。
- 見た目の正しさ (色・配置)。スナップショット比較は既存の `ui_snapshot` に任せる。
- mIV Remote、MCP、ネットワークを使う機能、AI モデルのダウンロード。決定的な batch は Sandbox の
  ネットワークを無効にする。
- 利用者の実データ・実設定での試験。

## 11. 利用者に決めてもらうこと

| # | 内容 | 設計の案 |
| --- | --- | --- |
| Q1 | [実行確認の規則](interactive-release-verification.md) は実アプリ試験に毎回の承認を求めている (理由: 利用者の PC の前面・入力を奪う)。サブ PC の Windows Sandbox 内に限り、常設で実行してよいか | Sandbox 内に限って常設の許可とし、その範囲を同文書に追記する。runner は Sandbox の外では動かない (§5.2) |
| Q2 | 実行する時間帯 | サブ PC を使わない夜間 (例 01:00〜07:00) |
| Q3 | 試験するビルドの渡し方 | 案 A: 利用者か ClaudeCode が master に入れた区切りで試験用ビルドを作って置く (手動、確実)。案 B: メイン PC で毎晩 master を専用 worktree からビルドして置く (メイン PC を夜も起動しておく必要あり)。まず A で始める |
| Q4 | 受け渡しの共有フォルダ | サブ PC に共有を作る (§5.3) |
| Q5 | §9 の探索試験をするか、するなら 1 晩の上限 | 決定的な試験が回ってから判断 |

## 12. 段階

| 段 | 内容 | 終わりの条件 |
| --- | --- | --- |
| S0 | §2.2 の未確認 3 点の確認 (セッション切断・ロック中の起動と入力、試験用ビルドの Sandbox 内での完走 (vGPU 有無)、共有フォルダでの受け渡し) | 3 点の結果を本書に記録。切断中に入力が届かない場合は、自動ログオンした常時接続のセッションか Hyper-V VM へ切り替える案を検討する |
| S1 | 最小の一周: 既存の `multi-window-stills.rhai` 1 本を、タスクスケジューラ → Sandbox → 結果回収で流す | 人の操作なしに 1 晩で結果が戻る |
| S2 | 生成器 v1: 設定カタログ + pairwise、状態に応じた操作の選択と記録、共通の安定待ち、C1〜C5、痕跡の判定、署名、`summary.json` | 1 晩に数百本流れ、失敗が署名ごとにまとまる。カタログ検証の cargo test が通る |
| S3 | 縮小と再現確認 | 失敗に最短の再現 script と設定が付く |
| S4 | 語彙と軸を広げる (F12 別窓、コレクション、動画・音声、`click_widget` の対象を増やす、GPU 軸) | 段ごとに語彙を足し、追加した語彙を使う scenario が流れる |
| S5 | (Q5 次第) Claude による探索試験 | 別途の設計で決める |

## 13. 設計の簡素化について (CLAUDE.md の方針)

- **組み合わせを減らす**: 環境は batch ごとに必ず捨て、scenario ごとに data を作り直す。環境の
  前の状態を引きずる組み合わせを考えなくてよい。scenario の中では操作を 1 つずつ出し、安定するまで
  次を出さない (操作の重なりは対象外。重なりを試したくなったら、別の操作語彙として明示的に足す)。
- **まれな失敗は割り切る**: 実行器の失敗 (Sandbox が起動しない、受け渡しの失敗、batch の timeout) は、
  ログを残して次の batch か翌晩へ進むだけにする。再試行・途中状態の保存・失敗の種類分けは作らない。
  失うのは 1 晩分の試験結果だけで、利用者のデータは関わらない。
- 再現確認 (§7) は「不具合の判定」のための仕組みで、実行器の失敗を回復する仕組みではない。
