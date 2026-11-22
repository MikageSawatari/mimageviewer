# 開発ビルドとテストの使い分け

機能追加中の反復では変更範囲に合う最小の確認から始め、配布物を作る段階で全体テストを
必ず通す。通常のリリース設定や配布物の内容は変えず、開発時だけ軽い経路を選べるようにする。

## 開発中の基本サイクル

| 目的 | コマンド | 対象 |
| --- | --- | --- |
| 型・借用・依存関係だけ早く確認 | `cargo check -p mimageviewer --bin mimageviewer-core` | 本体 core |
| 変更したモジュールのテスト | `cargo test -p mimageviewer --lib <filter>` | 本体の指定テストだけ実行 |
| 変更ファイルから反復テストを選ぶ | `.\scripts\test-focused.ps1` | nextest、選択を毎回表示、0件一致は非ゼロ |
| 本体lib全件と成功結果の再利用 | `.\scripts\test-lib-full.ps1` | nextest、同じ内容・条件の成功台帳があれば実行を省略 |
| 診断用の窓固定・入力観測のテスト | `cargo test -p mimageviewer --lib <filter> --features test-script` | `test_script` / `native_ui_smoke`等の対象。通常core checkも別に行う |
| 実アプリ用の軽量ビルド | `.\scripts\build-dev.ps1` | core・remote・EPUB worker・MCP adapter・music-video editor を `dev-runtime` でビルド |
| 自動操作用の使い捨て環境を準備 | `.\scripts\prepare-portable-smoke.ps1 -TestScript` | 診断portableを別出力先でbuildし、固定sandboxへ配置。起動はしない |
| 了承済みのリリース前実アプリ検証 | `.\scripts\ui-smoke.ps1 -Scenario <対象> -InteractiveApproved` | [実行確認](interactive-release-verification.md)後だけ。通常の開発反復・test-fullへ自動連結しない |
| 毎リリースのidle／perf正規確認 | `.\scripts\ui-smoke.ps1 -Scenario ReleaseChecks -InteractiveApproved` | 2026-10-11採用。利用者がPowerShellで実行。隔離seed＋5 idle＋cached／uncached perf、全結果は `target/ui-smoke/runs/<run-id>/` |
| リリース前の自動テスト一式 | `.\scripts\test-full.ps1` | nextestでworkspace 全体 + テストを持つ補助 bin + 除外された vendor 4 crate の lib test、cargo testでworkspace doctest |
| 承認済みの standalone eframe窓試験 | `.\scripts\test-live-windows.ps1 -InteractiveApproved` | 利用者が隔離 desktop/session で実行する ignored live 試験1件。自動 gate には接続しない |
| 配布成果物を生成 | `.\scripts\build-dist.ps1` | 全体テスト、clean、release、installer、portable |

ReleaseChecksは非対話テストではなく、desktopを使う了承済み検証枠で実行する。
他アプリ窓を最小化し、build待ちを含め終了までPCに触れない。seed込み約5〜8分＋初回build、
FFmpeg CLIがPATHに必要。同一ソースの診断artifactが検証できる場合だけ `-SkipBuild` を追加する。
`prepare-portable-smoke.ps1 -TestScript` は事前buildのみで起動しない。
suite採用後もエージェントのnormal-profile起動禁止、明示了承、使い捨てportable／設定保護を維持する。
full gate／buildからの自動起動はしない。手動 `perf_smoke.ps1`／`check-idle-health.ps1` は
fallback／診断として残す。[正規手順と採用証跡](release-check-automation-plan.md) と
[idle条件・閾値](idle-health-check.md) を参照し、警告付きPASS・FAIL・環境不成立を区別して記録する。

テスト名フィルタは実行するテストを絞るだけで、Cargo がコンパイルする target の種類は
減らさない。反復を短くしたい場合は `--lib`、`--bin`、`--test` も指定して target を絞る。
本体の全モジュールとアプリ固有テストは `--lib`、`tests/<name>.rs` は
`--test <name>` を使う。`mimageviewer-core` bin は `mimageviewer::run()` を呼ぶだけなので、
通常は bin 単独のテストを選ぶ必要はない。

`test-script`のApp連携やnative診断の回帰はfeatureなしの全体gateだけでは実行されない。
この領域を変更した場合は、対象テストを上記feature付きで追加実行する。backendの実Window
witnessは`cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu,miv-test-script-window-witness --lib`
で確認する。これらは通常coreの代わりにはせず、featureなしのcore checkと全体gateを維持する。
実アプリを使う検証範囲は [ui-smoke-automation-plan.md](ui-smoke-automation-plan.md) を参照。
リリース前は `.\scripts\test-lib-full.ps1 -Features test-script` でこのfeatureのlib全件も回す
([リリース手順](release-checklist.md) 9.8)。2026-10 に、このfeatureだけのテストが2件、気づかれないまま落ちていた (backlog §1.392)。

#### 診断用の一覧読み込み計数 (`folder_load_requests`)

`App::test_script_folder_load_requests` (snapshot の `folder_load_requests`) は、
**一覧の読み込みの新しい owner が受理された回数**を数える。完了ではなく受理で数えるので、
完了前に取り消された重複の開始も検出できる (RatingSort smoke が「更新で読み込みがちょうど1回始まる」を検査)。
パスワード入力後・変換完了・キャッシュや descriptor の水和など、既存 owner の継続は数えない。

数える場所は、新しい owner を作る受理点 (直接ロードの `load_folder_with_scan_claimed`、
`admit_physical_history_phase` の新規要求、直接書庫要求の受理、Smart / Collection の各 owner、
画像フォルダ ZIP 化の `ParentScan`、detached の新規 ZIP/PDF descriptor と proven backing 直行) に置く。
新しく一覧を開く経路を足したら、その受理点にも計数を足し、`app::tests` の計数テストに1本加える。

1か所の境界で数える形も検討したが、採らなかった (2026-10-20、§1.392)。一覧の読み込みを実際に始める
関数は数十あり、多くはサムネイル・メタデータ・Remote などの補助処理や継続からも呼ばれるため、
そこで数えると継続や補助を数えてしまう。一方、新しい owner を作る点は約10の種類に分かれ、共通の型が無い。
共通の owner 型を新設すれば1か所にできるが、診断用の計数のために製品コードの所有構造を作り替えることになり
範囲に見合わない。代わりに、loader 開始関数の全呼び出し元と、一覧を採用する全箇所の両方向から
受理点の網羅を確かめた。

### 作業中のPCと通常データを保持する配布ビルド

エージェントによる配布ビルドには `.\scripts\build-dist.ps1 -PreserveRuntime` を使う。
稼働中のmImageViewerがあれば停止せず失敗し、通常APPDATAのVST3展開キャッシュも削除しない。
子のrelease/portableビルドへ同じ指定を渡す。引数なしの従来CLI動作は変更しない。
この指定は必須テストや署名を省略するものではなく、アプリの起動・操作を許可するものでもない。

release 出力は `<repo>\target\release` に固定する。`build-release.ps1` / `.sh` は追加 Cargo 引数を
停止/build 前に allow-list で検査する。許すのは `--features <x>` / `--features=<x>`、`--all-features`、
`--no-default-features`、`--locked`、`--frozen`、`--offline`、`-v` / `--verbose`、`-j <n>` / `--jobs <n>`。
各 build は `--message-format=json-render-diagnostics` を使い、`compiler-artifact.executable` が
固定出力と一致しなければ署名/次の build/内包へ進めない。環境変数・利用者の Cargo 設定もこの照合で検出する。
launcher と MCP、browser-host については、その build が書く exe と image path が完全一致する process だけを
停止/拒否の対象にする。インストール先の AI 連携と path 不明の process はこの3名の対象外。

workspace 全体/launcher のテストは、埋込入力6本を先に作る必要がある:

```powershell
cargo build --release --bin mimageviewer-core
cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets
cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf
cargo build --release -p mimageviewer-mcp --bin mimageviewer-mcp
cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo
cargo build --release -p mimageviewer-browser-host --bin mimageviewer-browser-host
```

release wrapper は core → remote → EPUB worker → MCP → editor → browser-host → launcher の順でビルドする。

同経路では `test-full.ps1 -SuppressCrashDialogs` を子プロセス内で適用する。
Windowsのクラッシュダイアログだけをそのプロセスで抑え、元のerror modeを終了時に復元する。
テストの非ゼロ終了は失敗のままとし、自動再試行や期待値変更は行わない。
スクリプトの回帰確認は `scripts/test-release-build-safety.ps1` を参照する。

### テスト用一時領域

PID だけの固定名を使わず、原則 `tempfile::TempDir` をテストの接続・worker より長く所有する。
プロセス共有の既定 data_dir だけは終了フックで回収する。
所有・終了順と PID 使用箇所の監査記録は [テスト用一時領域](test-tempdir-ownership.md) を参照。

### App fixture の入力所有（§1.814）

Windows の `phase_c_support::setup_app_with_options` と履歴の `setup_with_ai_runtime` は、
data-dir lock より先に `lock_test_input()` を取得する。戻り値の `AppTestEnv` / `CaseEnv` は
入力 guard を最後のフィールドに持ち、App、AI 後処理、override、TempDir、data-dir lock の後で解放する。
fixture に `Drop` は実装せず、既存の App の部分 move・設定再読込でも残りのフィールドが lease を保持する。

同じスレッドの入力 guard は一つの scope を共有する。明示 guard が fixture 作成の前後どちらにあっても
実 mutex を取り直さず、synthetic / frame 状態も reset しない。最後の guard の破棄だけが reset・解放する。
App を駆動する子スレッドはその中で fixture を生成する。入力 guard はスレッドを移動できず、
親が保持したまま子の取得・終了を待つことはできない。fixture を使わない入力テストは従来どおり
自分で guard を保持する。物理キー入口の未取得 panic は維持し、実キーボードへは戻さない。

### App fixture の AI ランタイム（§1.817）

`AppTestConfig.ai_runtime` は既定値を持たず、`AiRuntimeFixture::NotStarted` / `Start` を
必ず選ぶ。共通の `setup_app()` と履歴 fixture の `setup()` は `NotStarted` を選び、
構築直後の同じ owner を Dormant から説明付き Failed へ変える。owner の Arc と、constructor で
設定した TensorRT lifecycle は保持する。Dormant 以外への適用は fixture の順序違反として panic する。

実際の runtime / 推論を必要とするテストだけ `setup_app_with_ai_runtime(AiRuntimeFixture::Start)`
または履歴 fixture の `setup_with_ai_runtime` を使う。`Start` は初期化を前倒しせず、通常の最初の
Full frame / AI producer から開始する。片付けは App → AI 初期化の terminal 待ち → epoch / override →
TempDir → 共有 lock の順。Initializing の場合は資源を保持したまま、既存 Condvar で期限なしに
Ready / Failed を待つ。ハングは nextest の process 単位の timeout（既定60秒×5）で process ごと
打ち切り、初期化中の TempDir を unwind で削除しない。製品の App drop は初期化 thread を join しないまま維持する。

設定の再読込では `replace_app_from_settings(settings, fixture)` を使う。先に `drop(env.app)` して
保存済みデータを編集するテストは、部分 move を保ったまま
`env.app = env.ai_runtime_cleanup.app_from_settings_after_drop(settings, fixture)` で再構築する。
両方とも旧 owner の待機を保持し、新しい App の owner へ片付けの対象を更新する。
App 単体の設定読込 helper も policy の引数は必須とし、`Start` の場合は必ず env の待機 lease と組にする。

`app::tests::ai_runtime_fixture` は初回 Full の非初期化、待機内 hook で解放する合成 initializer の
片付け、再生成時の owner とソース走査を検査する。コメント・文字列を除き、App constructor の
参照、Self 経由、UFCS、型別名・import alias を検出する。test helper 2か所、製品の constructor
連鎖と既存の非生成表記だけを正確な path / anchor / 件数で許可し、新しい参照と古い例外を失敗にする。
補助の監査の限界: マクロ展開、型推論による `let app: App = Default::default();`、別 crate からの呼出し等は
解析しない。Rust の名前解決を行う検査ではなく、コードレビューを補助する。
背景: 2026-11-20 の本体 lib 全件の最中の PC の停止で、AI 初期化の DLL の読み込みと TempDir の削除の重なりが第一候補になった (原因は実証していない。経緯は §1.817 の報告)。

### 長い経過時間を扱う collection テスト

Windows の `Instant` は起動直後に大きな時間を引くと underflow するため、過去の
`Instant` を `checked_sub(Duration)` と `unwrap()` で作らない。collection read の
長時間回帰は `TestReadClock::long_elapsed_since(started)` で観測時刻を24時間進める。
これは `cfg(test)` 限定の thread-local scope で、poll・poll delay と後続 Grid read の時計を差し替え、
製品ビルドの時計・動作を変えない。終了・panic 時の復元とスレッド分離、時間減算の再導入は
`collection_store::test_clock::tests` で検査する。実時間の待機期限は通常の `Instant` を使う。

### 負荷に左右されない時間・完了検査（§1.389、2026-10-23）

PDF poolのcancelテストは製品と共通の`wait_for_pool_reply`へ1回の受信を注入し、
Abortはcancelを観測した最初のtimeoutで返ること、Harvestはreply／disconnectまで待つことを
回数で検査する。複製ループ・固定sleep・経過時間の上下限は使わない。
metadata importは公開の`import_at`を維持し、内部のbatch判定時計だけを注入する。
件数境界と取消済みbatchの検査は時計を固定し、時間境界は499ms／500msと次batchの時計リセットを
手動時計と別接続から見たcommit状態で検査する。製品の時計とperf用計測は従来の`Instant`のまま。

§1.375のheadless helperは製品の採用条件（sidecar復旧中はstack結果を採用しない）を揃える。
外部削除の模擬は自身のfullscreen readのwaiting／running／cancellingが全て0になるまで待つ。
OS／別threadの到達を待つ期限は従来の20秒とし、未到達は失敗する。
実watcherの登録を検査した後の合成通知は専用channelへ切り替え、遅着した実OS通知が
合成イベントのdebounceを延ばす順序依存を持ち込まない。

## 検証の担当と結果の再利用

モデル分担・委任・レビューの粒度は [AGENTS.md](../AGENTS.md#model-roles-and-coordination) を正本とする。
節約するのは重複した調整・実行であり、品質条件や必要な検証範囲は維持する。

- 一まとまりの変更ごとに検証担当を決め、他担当は同じCargo targetへの重複実行を避ける。
  早期の型確認・焦点テストは行い、関連変更が統合された区切りで広い検証と確認用buildを行う。
  検証対象ソースの編集は実行中に重ねず、無関係な調査は継続できる。
- 結果には対象commitまたは未コミット差分、command、feature/profile、必要な環境条件、
  exit code、実行件数、ログの所在を残す。0件一致・中断・準備成功をテスト成功と扱わない。
  既存の台帳を更新し、同じ結果を担当ごとに別文書へ転記し続けない。
- 対象コード・依存関係・build設定・fixture・実行条件が引き続き有効な成功結果は、親と
  レビュー担当も共有して使う。担当交代や文書のみの更新を理由に同じ検証を再実行しない。
  古い結果の有効範囲が不明なら確認し、変更の影響に応じて必要な部分を再実行する。
- 再実行の理由は、変更による失効、失敗の修正、新しい懸念、条件不成立などを短く残す。
  テスト側の不備と製品の不具合を分け、期待値の緩和で失敗を隠さない。既知の非ゼロ失敗を
  放置したまま「節約」を理由に完了としない。
- 全体gate・feature固有テスト・実機検証用buildの要件は既存のまま。検証済み範囲と
  未検証範囲を分けて報告し、実アプリ操作は明示了承を得た検証枠でのみ実施する。

## テストの段階分け (2026-11-20 利用者決定)

並行して開発するブランチが多いので、本体 lib 全件 (12 並列で 1 回 20〜30 分、全件ロックを占有) を
ブランチごとに流さない。全件は master への取り込みの後とリリース前に寄せる。利用者の言葉:
「テストを分類し、開発した機能に関係しそうな部分だけを部分的にテストするようにはできないでしょうか。
リリース前に、全体のテストを実行する形にします」(案を示したうえで「この案で入れる」)。

| 段階 | 流すテスト | 担当 |
| --- | --- | --- |
| ① ブランチ (開発中・master へ入れる前) | 変えたモジュールのテスト + 足したテスト + 指示書が名指しした関連テスト | 各ブランチの担当 |
| ② master へ取り込んだ後 | 本体 lib 全件を 1 回 (数本まとめて取り込んだ区切りで 1 回でよい) | 統括 |
| ③ リリース前 | `test-full.ps1` と `test-lib-full.ps1 -Features test-script` (従来どおり) | 統括 |

- ①の範囲は、統括が指示書に書く (機能の入口・状態の producer / consumer・同じ型の入口のテストの filter)。
  `test-focused.ps1` の自動の選択は補助として使い、`src/app.rs` などで exit 2 (広域) になっても、
  それだけを理由に lib 全件へ広げない。指示書の filter と、ほかに変えたモジュールのテストを流す。
- ①でも lib 全件を流す例外: App の状態の持ち方・非同期の完了や取り消し・複数ウィンドウ / viewport・
  vendor の egui / eframe / wgpu・テストの共通基盤 (`tests/support`、App の test fixture) など、
  共有の土台を変えるブランチ。迷ったら統括が決める。
- ②で落ちたら、マージコミットごとに流し直して原因のブランチを絞り、そのブランチの担当へ返す。
  ②が通るまで、次の取り込みの区切りを重ねない。
- 代わりに失うもの: 退行がブランチではなく master で見つかり、切り分けの手間が増える。
  リリース前の全件 (③) は残るので、出荷物の検査範囲は変わらない。
- 経緯: 2026-11 は 6〜7 本の開発が並行し、各ブランチの取り込み前の lib 全件が全件ロックの順番待ちを作っていた。
  以前は §1.680 (修正を重ねたブランチで関連 filter だけでは退行を見逃した) を受けて、取り込み前に
  ブランチごとに全件を流していた。その役割は②が担う。

## 軽量化している範囲

### nextest と成功台帳

初回だけ `cargo install cargo-nextest --locked` を利用者が実行する（0.9.143以上、導入確認は
`cargo nextest --version`）。通常の反復は `.\scripts\test-focused.ps1 -Base HEAD`、
本体lib全件は `.\scripts\test-lib-full.ps1` を使い、上の「結果の再利用」に従う。
nextestは1テスト1プロセスでAppのprocess-global lockを分離する。
`.config/nextest.toml` はCPU数並列・retryなし・60秒ごとの警告と300秒での打ち切りを使う。
実 GPU device / GPU 実行 session を作るテストは `gpu` test-group（同時2プロセス）で制限する。
DXGI / VRAM / SM の情報照会だけ、wgpu Instance 作成だけのテストは対象外とする。
同時 device / session 作成と PC 停止の関係は未確認の仮説であり、原因確定を意味しない。
新しい GPU helper とそのテストは原則 `gpu_tests` サブモジュールへ置く（filter が自動で拾う）。
製品側の device / session 作成点にも `#[cfg(test)]` 限定で `assert_gpu_test_group()` を置く。
テスト専用入口も検査する。nextest 上で `gpu` 以外から到達すると、設定への追加を促して
生成前にテストプロセスを非ゼロ終了させ、background workerのpanic握り潰しも防ぐ。
既存 helper の新しい呼び出し元も所属を確認する。スナップショット等の4統合テストは
binary 全体を選び、混在するCPUテストも含める。通常の `cargo test` では入口検査を行わない。
`cfg(test)` はdependencyへ伝播しないため、別integration / 別crateから通常libを呼ぶGPU経路は、
そのbinaryの所属または自crate側の検査でも覆う。製品ビルドの動作は変えない。
group 内だけの実行では group 外の呼び出し漏れは検証できず、取り込み後の全件実行で検出する。
経路別の対象・継承・一覧照合と時間見積もりは [GPU test-group](gpu-test-groups.md) を参照。
workspaceの全package・補助bin・integrationと、standalone vendorのegui／egui-winit／egui-wgpu／
eframe（wgpu・glow）の5実行を棚卸しし、成功実行で60秒を超えた12件は120秒ごとの警告・600秒で打ち切る。
本体libだけの「最長約60秒」を全体の根拠にはしない。workspaceのLibRaw検査174.018秒、
過去の物理走査429.813秒などを含む[テスト別・graph別の実測と割当](test-speed-report.md#review1420)を根拠とする。
長い処理を早く開始して終盤に残さない目的で、12件ともpriority 100とする。
overrideは`test(=<完全なテスト名>)`のORだけで1節にまとめ、全graphでルートの同じ設定を使う。
同名テストにも600秒・priority 100が適用されることは許容し、package／binary IDの限定や設定分割は行わない。
通常300秒／長い枠600秒の値自体は変更しない。打ち切られたテストの原因は
別に扱い、時間上限の延長やretryで通さない。grace-period等はnextestの既定のまま。
nextest導入時にはテストの削除・新しいignoreはなかった。アプリ起動等を行う試験は下記のlive区分へ分離する。
最終 `test-full.ps1` も同じnextest設定を使い、
nextest対象外のworkspace doctestは同じfeatureで別段の `cargo test --doc` として実行する。
導入時の件数突合・時間比較とテスト模擬の修正は [検証記録](test-speed-report.md) を参照する。

- 全件: `-Features test-script`、`-CargoProfile test`、`-NextestProfile default` を指定可能。
  `-Force` だけが有効な同一結果を再実行する指定で、ファイルハッシュのキャッシュも使わず再計算する
  （実行前・実行後の照合とも）。既定の成功閾値は実行9,000件以上。
  ignore以外の全件を選び、default filter・retry・fail-fastによる取りこぼしを防ぐ。
- 台帳と各実行のcommand・JUnit・stdout/stderrは `target/test-lib-full/`。
  同じworktreeの読み取り専用レビューから参照可能。別worktreeとの共有はしない。
  JUnitの件数整合・exit 0・閾値を検査し、失敗・0件・実行中の入力変更を再利用しない。
  台帳が書けなくても通常のテストは実行し、記録失敗は警告だけにする。
  stdout/stderrは受信した文字列をchunkごとにflushして保存するため、停止中もSLOW行とテスト名を確認できる。
  LF／CRLF／末尾改行なしを変換せず、戻り値の全文とlogの内容を一致させる。
  実行ごとの`miv-tool` profileは指定profileを継承し、defaultの打ち切りとoverrideも引き継ぐ。
- 識別子はGitが列挙した全追跡ファイルと未追跡ファイルの**現在の内容**をSHA-256化する。
  stagingの有無・HEADの移動だけでは変えず、未コミット編集・削除・新規ファイルを検知する。
  ignoredでも `vendor` / `testdata` / `testimage` 等のnative入力・実fixtureを含める。
  各ファイルのSHA-256は `target/test-lib-full/source-hashes-v1.tsv` にキャッシュする。
  相対パスをキーに、サイズ・`LastWriteTimeUtc` ticks・`CreationTimeUtc` ticksがすべて一致し、
  更新日時がハッシュ記録時刻より2秒超古い場合だけ再利用する。入れ子repository内も同じ規則。
  記録直後の更新日時（racyなエントリ）や未来の更新日時は再計算し、HITで記録時刻を進めない。
  通常列挙されたファイルのreparse pointは、リンク先の変更を逃さないよう毎回内容を読み、
  キャッシュしない。入れ子repository内のreparse pointは従来どおり照合を拒否する。
  毎回のGit列挙と存在確認は省略せず、キャッシュの有無でsource識別子の形式・値を変えない。
  キャッシュはUTF-8のタブ区切り（パスだけBase64）とDictionaryを用い、JSON変換を避ける。
  読み書き失敗・不正形式は警告し、その照合をキャッシュなしの全ハッシュで行う。
  自動再試行・回復状態は追加せず、壊れた原本を失敗処理で削除・再作成しない。
  **サイズ・更新日時・作成日時をすべて保った内容変更は検知できない**
  （racyとして再計算する場合を除く）。同じstat情報を信頼するgitと同様の前提であり、
  疑わしい場合は `test-lib-full.ps1 -Force` で全ハッシュ・テストをやり直す。
  rootの`target` / `target-*`、入れ子の`target/`、native build/object、snapshot比較用の
  `.new/.old/.diff.png`、計測出力 `docs/test-speed-report.md` は除外する。
  ただし実行体の隣のnative DLLは別の識別子へ含める。`cargo metadata` のtarget directoryから、
  指定profile（test/dev→debug、bench→release）とtarget triple配下の同profileの直下・`deps/`にある
  DLLのパス集合と全内容を照合する。FFmpeg 6本を含み、追加・差し替え・削除で台帳はMISSになる。
  Cargo生成の`-<16桁hex>.dll`は除外する。Git外のDLL探索先・システムDLLは対象外。
  index・作業ツリー・Git管理領域へは書かない。compiler/nextest版と主要build/runtime環境変数も照合する。
  `.cargo/config.toml` の`[env]`宣言名を自動で読み取り、そのprocess環境での未設定／値を識別する。
  `LIBSQLITE3_FLAGS`、`FFMPEG_DIR`、`RUSTFLAGS` / `RUSTDOCFLAGS` / `CARGO_*`、
  `LIBCLANG_PATH`、`MIV_*` / `MIMV_*`等を含む。環境変数の値は台帳へ出さずhashのみ保存する。
  対応できないCargo env宣言構文は再利用せず、警告して通常実行する。
- Git外のglobal Cargo設定、registry側の手編集、ドライバー・ハードウェア・外部サービスの変更、
  上記以外のignored入力は検知しない。その条件が変わったときは `-Force`。
  判読できない入力や入れ子Git repositoryがあれば再利用を止め、通常実行する。
- focusedはHEAD（または`-Base <commit>`）と作業ツリーの差分＋未追跡ファイルから選ぶ。
  `src/foo/bar.rs`→`test(~foo::bar::)`、`mod.rs`は親module、`tests/name.rs`はそのintegration target、
  他workspace crateはそのpackage全target。`-Path src/foo.rs`で明示選択、`-PlanOnly`で実行前の確認も可能。
  `lib.rs` / `app.rs` / `src/app/tests.rs`、Cargo/build/runner設定、vendor、共通test support・snapshotは
  広域として扱う。それ以外も`testdata/**`、`scripts/**`、workspace外patch crate、文書など、
  対応付けできない変更が1つでもあれば全該当パスを表示し、部分実行せずexit 2で
  `test-lib-full.ps1`と必要な専用検査を案内する。自動で全件へ拡大しない。
  変更パスなし・対応したfilterの0件一致はexit 4。
  module aliasや他crateへの影響を完全には推定しないため、表示された選択と追加featureを担当者が確認する。

道具の回帰確認は `powershell -NoProfile -File scripts/test-test-tools.ps1`。
使い捨て小規模crateで本物のGit/nextestを使い、台帳hit/miss・差分・失敗・0件・選択を検査する。
実子processの末尾改行なし・LF・CRLF・stdout/stderr交互出力・複数buffer分の出力を、
従来のReadToEndAsync経路と逐次経路の戻り値・disk logで照合する。
終了前のSLOW行と改行前の部分出力が両logへflushされることも通知で検査する。
statキャッシュの識別子一致・サイズ/作成日時/racy更新・`-Force`・破損/I/O失敗も検査する。
`-Benchmark` を付けると、`target/` 配下の使い捨てrepositoryに21,247件・約1.39 GBの
ignored fixture（うち1,247件は入れ子repository）を生成し、キャッシュなし/ありと台帳HITを測る。
fixtureとログは残し、約1.4 GBの空き容量が必要。実画像59.1 GBを持つ本体の最終測定とは区別する。
ツールの導入・commit・アプリ起動・通常設定へのアクセスは行わない。
仕様根拠: [nextest JUnit](https://nexte.st/docs/machine-readable/junit/)、
[filterset](https://nexte.st/docs/filtersets/reference/)。

### 1字ずつ入力するテスト（§1.832）

共通の道具は `src/app/tests/typing/` に置く。テスト名はすべて `typing_` で始める。
`eframe::HeadlessEpiIntegration` で本物の `App::logic` と `App::ui` を動かし、
App のフレーム内の処理順をテストへ書き写さない。GPUや実ウィンドウは作らない。
App は既存の `setup_app` fixture で作り、worker・一時領域・入力leaseの寿命を任せる。
テスト自身が物理キーの状態を読んだり、別の入力leaseを取ったりしない。
元の画像一覧は `load_folder` で開き、対象の行が採用され、準備中のownerが終わったことを
期限付きの述語で確認してから欄を開く。`items` などの配列だけを手で差し替えて、
一覧の所有・識別情報が揃わない状態から開くことはしない。
検索系はCtrl+F/G/S/T/Rのpress／releaseをRawInputへ送り、実際のキー処理で開く。
開くためのpressだけは、その操作のKeyActionが記録されることを要求する。
文字入力中の記録が空であるという検査には例外を足さない。

`Harness::type_text` は英字の Key press と1文字の Text を同じフレームへ送り、次の
フレームで release を送る。かななどの Text に Key は付けない。`compose_ime` は
Enabled、複数の Preedit、Commit、Disabled を別フレームへ送る。Windows の IME が
処理済みの Process press は送らない。各段で、フォーカス・有効状態・App の下書き・
TextEdit のカーソル範囲・IME の所有・ショートカットへの漏れを検査する。
欄を開いた更新をフレーム0として、描画・有効状態・フォーカス・保存されたカーソル範囲の
4条件を毎フレーム記録する。準備フレーム数は `mod.rs` の欄ごとの定数で指定し、
その時点から4条件をすべて必須にする。カーソル範囲が無くてもよいという例外は設けない。
入力後はキーのreleaseや結果待ちを含む毎フレームで同じ検査を行う。
定数は共有runnerの実測で4条件がすべて初めて成り立つフレームに合わせる。
2026-11-21の測定では、Ctrl+F/G/Sの一時バーとCtrl+Tは1、固定欄は2、
Everythingは一時バー0・固定欄1、アドレス／履歴は2、タグの候補は1、
コレクション作成名と両スマートフォルダ編集欄は2だった。
固定欄はフォーカス要求を描画後に受け渡し、フォーカスが1、カーソル範囲が2で現れる。
IME helperが `TextEdit::show` 後にフォーカスを要求し、TextEditがフォーカスのある描画で
カーソルを保存する経路による。一時バーより1フレーム遅く、準備後は安定している。
Everythingは製品側でカーソル範囲を保存するため別の値になる。根拠のソース位置と実測値は
`mod.rs` の定数のコメントに置く。
クリックのreleaseやアドレスの空欄化は準備後の入力操作として続け、その間も4条件を必須にする。
これらの操作を終えるまでのフレーム数を準備フレーム数には使わない。

```powershell
cargo nextest run -p mimageviewer --lib --no-capture -E 'test(/(^|::)typing_opening_readiness_measurement$/)'
```

この測定は16種類の欄をそれぞれ新しいfixtureで開き、フレーム0〜9を出力する。
準備が整うまで回す待ちではない。通常テストも、決めたフレームで条件を満たさなければ失敗する。
測定テストは定数と「4条件がすべて初めて成り立つフレーム」の一致、およびそこから9までの
全フレームで4条件が保たれることを検査する。値が変わったら欄名・定数・実測フレーム・
条件が落ちたフレームを出して失敗する。測定中の入力手順は定数に依存させない。
記録の `backend_frame` は1始まりの更新回数、`pass` はeguiの0始まりの累積pass番号である。
再描画があれば1更新に複数passを含むため、両者の数字だけを比較して描画漏れとは判断しない。
固定欄には、フォーカス要求の発行・描画直前・描画直後を読むテスト専用の記録も置く。
有効状態と矩形はhelperが受け取ったResponseをそのpass中に記録する。
egui 0.37はend-passでwidget表を入れ替えるため、フレーム終了後の `read_response` は
古いpassの値を先に拾う場合がある。1フレーム内の再描画もすべて記録し、文字入力中に
無効で描かれたpassを後の有効なpassで隠さない。診断には描画時の値とchromeの停止理由も残す。
egui 0.37 の内部 `cursor_purpose` は非公開なので、未確定文字を含む下書き、保存された
変換範囲と共有IME状態を検査する。カーソル状態は書き換えない。

欄ごとの差は adapter に置く。入力ごとに処理する欄では、テスト専用の記録で
「この入力の処理が始まった」と「その結果が採用された」の両方を検査する。
0件も採用結果として数える。Ctrl+G の最小長未満の語は、0件とは区別して
拒否通知を受けただけでは完了にせず、同じ語の一覧投影が採用されるまで検査する。
実際の準備済み投影をテスト用の保留口で止め、通知だけでは待ちが終わらないことも検査する。
待ちの成功条件は述語で、壁時計の期限は
10秒。1msの待機は空回りを避けるためだけに使う。eframe は `RawInput.time` を
実時間で上書きし、Ctrl+G も `Instant` で300ms待つので、時刻の早送りはしない。
Commit が最後の Preedit と同じ文字列なら、新しいworkerの開始を要求せず、直前の結果が
今も採用されていることを確認する。

| 欄 | 入力データと検査 |
| --- | --- |
| Ctrl+F 固定欄／一時バー | 空欄から `alp`、`rzz`、IME。Enter送信なので入力中の欄と未送信状態を検査 |
| Ctrl+G 固定欄／一時バー | PNGの名前を実FTS索引へ登録。`a`／`al`は拒否、`alp`は2件、`rzz`は0件。IMEも検査 |
| Ctrl+S 固定欄／一時バー | 一時お気に入り配下の索引。`alp`は各途中入力でも2件、`rzz`は0件。IMEも検査 |
| Ctrl+T | 元の一覧とは別のフォルダの画像とタグを実tags.dbに登録。同じASCII入力とIMEで、結果一覧の採用まで検査 |
| Everything 固定欄／一時バー | ASCIIとIME。Enter／検索ボタンだけがIPCを始めるため、入力中は欄と未送信状態を検査 |
| アドレス／履歴 helper | ASCIIとIME、履歴を開いてからの入力。自動補完はなく、入力すると明示的な履歴リストを閉じる |
| タグを付ける画面 | 登録タグ `alpha`／`alpine`／`かな本`／`koi`。候補更新の開始と採用を検査 |
| ファセットの名前フィルタ | 同じ4画像を `alp`／`rzz` で絞り、150msのdebounce後の適用と正規化cache workerの採用を検査。IMEの各preeditでは実際の期限を越えても適用せず期限を更新すること、Commit後に採用することを検査 |
| ファセットのタグ候補 | 実tags.dbへ同じ4タグを登録。タグメニューを実際にクリックして開き、`alp`／`rzz`／IMEごとのprefix検索とcache更新を検査 |
| コレクション | 絞り込み欄はない。作成名の欄を軽く検査。改名と名前モーダルは確定時だけ処理するため理由付き除外 |
| スマートフォルダ | ルール追加／管理の「名前に含む」。下書きと条件の説明だけを更新し、件数プレビュー検索はしないため軽い検査 |

ASCIIは3文字、IMEは `k` → `か` → `かな` → 確定。IMEの各途中入力にも候補を用意する。
通常入力には既定の単一キー操作である L／P／R を含め、その既定割り当ても検査する。
アドレスとタグ候補には、IMEを使わずTextだけで `かな本` を1字ずつ送るケースもある。

ショートカットの記録は `key_debug::record_pressed_action`／`record_consumed_action` の
ログ有効判定より前に置く。通常のpressed／consume、first-action、rating、回数付き入力、
ページ送り、編集履歴、native key queue／動画の判定がここを通る。
専用履歴キー・操作ヘルプ・nativeのparts判定・test-script注入は `keymap.rs` の
テスト専用hookで補う。記録はテスト単位のthread-localで、通常テストでは無効。
診断専用のpeek、押下判定をしないchord解決、固定キー、KeyHold／ModifierHoldは記録対象外。
これらのテストはRawInputの入口を検査し、実Win32イベントの配送そのものは検査しない。

Ctrl+S／Ctrl+T の7本は現在ignoreなしで通常実行する。
修正前の土台では7本とも最初の非空結果採用で赤を確認した。修正を含むmasterのmerge後、
ignoreを解除してtyping_全40本と関連708本が成功した（共有runnerでの当時の検証記録）。
0件の2本には、別fixtureの「結果あり」の対照ケースを先に入れている。0件だけでは旧不具合が
起きなかったため、この対照も保持し、現在は非空結果・0件の両方が成功することを確認する。
他の欄で赤が出たら、テストの誤りか製品の不具合かを切り分ける。製品の不具合なら、
この作業では製品を直さず、経路を記録して指定の「未修正の赤」というignore理由を付ける。

`inventory.rs` の `INPUT_TESTS` と `EXEMPTIONS` は、`ime_focus` の4つの1行欄helperの
呼び出しを全件分類する。呼び出しから6行の中の固有anchorと件数を照合し、重複・未登録・
古い項目・存在しないテスト名を失敗にする。コメントや文字列内のコード例は除き、
修飾付き・素の名前のimport・grouped import・複数段のalias・super経由の呼び出しも拾う。

`test-focused.ps1` は通常の選択に加えて、
`-p mimageviewer --lib -E 'test(/(^|::)typing_/)'` を選ぶ。
書式は [nextestのfilterset仕様](https://nexte.st/docs/filtersets/reference/) に従う。
対象は `test-tools-common.ps1` の `MivTypingProductPaths` 配列の描画・入力・結果採用ファイル、
IME／キー入力の土台、`src/app/tests/typing/` 以下、`crates/egui-ime/` 以下、
および変更されたRustファイルにhelper呼び出しかimportがある場合。
新しい欄を足すときは同じ配列も更新する。広域判定とexit codeは変えず、
`src/app.rs` などが含まれれば従来どおりexit 2で実行を止める。

### テスト前の FFmpeg DLL 配置

新しい worktree・`cargo clean` 後・`cargo test` を直接打つ前には、次を1回実行する。
`vendor/ffmpeg/bin` の必須6 DLLを Cargo の base output directory (`target/debug/`) へ
実体コピーする。同じ内容なら書き直さず、不足時はセットアップ案内付きで停止する。

```powershell
.\scripts\setup-test-dlls.ps1
# 別 profile のテストでは、その Cargo profile 名を指定する。
.\scripts\setup-test-dlls.ps1 -CargoProfile dev-runtime
```

`test` / `dev` は `debug`、`bench` は `release`、他は同名の出力先になる。
出力 root は `cargo metadata --no-deps --format-version 1` の `target_directory` を使う。
明示する場合は `-TargetDir <path>` を指定できる。テストは target triple を指定せずに流す前提で、
`--target` / `CARGO_BUILD_TARGET` を付けた場合の `target/<triple>/<profile>/` には置かない
(リリース側の `build-output-safety.ps1` も `CARGO_BUILD_TARGET` を拒否している)。
`test-lib-full.ps1` / `test-focused.ps1` / `test-full.ps1` / `build-dev.ps1` は自分で呼ぶ
(`test-focused.ps1 -PlanOnly` はコピーしない)。

Cargo 1.100 ではテスト実行体が `target/debug/deps/` から
`target/debug/build/<package>/<hash>/out/` へ移った。`deps/` に置く従来の方法では
実行体の隣にならず、`cargo test` の DLL 解決には効かない。Cargo と nextest が PATH に
加える base output directory に配置する。スクリプトは `deps/` の既存ファイルに触れない。

### ビルドキャッシュの容量管理

リリース成果物と検証記録を確保した区切りで、`target`の容量内訳を確認する。
容量が大きい場合、同じ出力先を使うCargo/rustcが停止した後に
`target/debug/incremental`と`target/dev-runtime/incremental`を優先して整理する。
これらは再生成可能だが、削除後の初回ビルドは遅くなる。通常の開発反復では毎回削除しない。

`target`には配布・確認用exeだけでなく、`v370-work`等の検証記録、使い捨て環境、
過去の調査用コピーやdata directoryも存在する。`target`全体を無条件に削除したり、
引数なしの`cargo clean`を定期処理へ組み込んだりしない。`deps`や古いコピーの追加整理は、
再ビルドの負担と保持対象を確認して別に判断する。
削除前に絶対パスが当該repositoryの対象profile配下であることとreparse pointを確認し、
他系統のビルドや証跡・利用者データに触れない。実施前後の容量と削除対象を記録する。

実施記録 (2026-09-17): C: の空き 531 GiB → 1,728 GiB。削除対象は `target/debug` (622 GB、
うち `deps` 394 GB / `incremental` 197 GB)、`target-codex`、`target-portable`、
`target-portable-test-script`、調査用の入れ子 cargo 出力 3 つ (`codex-v330-p1/debug`、
`section243-focused-20260915/debug`、`v370-work/debug`)、`mimageviewer-musicvideo` と
`mimageviewer-xpost` の `target`、統合済み worktree `mimageviewer-dupe` 一式。reparse point は 0 件。
`dev-runtime`・`release`・検証記録は保持した。`target/debug` を消すとテスト用の
FFmpeg DLL も消えるので、`.\scripts\setup-test-dlls.ps1` で置き直す (上の配置手順を参照)。

### dev / test プロファイル

`[profile.dev]` は `debug = "line-tables-only"` + `split-debuginfo = "packed"` としている。
test profile は dev を継承するので、設定は `[profile.dev]` だけに書く。失敗時の関数名・
ソース行付きバックトレース (panic、cdb のスタック) は維持し、完全な型デバッグ情報を含む
巨大な PDB / rlib の生成量を抑える。dev と test で設定を分けると、依存クレートが
`target/debug` に 2 通りビルドされるため、分けない。

デバッガでローカル変数・型情報を見る必要があるときだけ、そのビルドで
`$env:CARGO_PROFILE_DEV_DEBUG = "full"` を指定する。

### 実アプリの開発ビルド

`build-dev.ps1` は次の条件で core → remote service → EPUB worker → MCP adapter → music-video editor → browser-host をビルドする。

- `dev-runtime` profile: `opt-level = 2`、LTO なし、codegen unit 64、incremental 有効
- 通常 feature set（`portable` は付けない）: 設定・キャッシュ・ログの既定保存先は
  インストール版／release版と同じ `%APPDATA%\mimageviewer`
- launcher を省略して直接起動できるよう、FFmpeg DLL だけを exe の隣へ配置。他の
  DLL・worker・AI model は通常版と同じ埋め込み・展開経路を使う
- 出力: `target\dev-runtime\mimageviewer-core.exe`
  と `mimageviewer-remote.exe` / `mimageviewer-epub-pdf.exe` / `mimageviewer-mcp.exe` / `mimageviewer-musicvideo.exe` / `mimageviewer-browser-host.exe`。
  MCP は1dの登録先として core と同じフォルダに必要。
- Cargo の `dev-runtime` はビルド時間短縮用の最適化profileであり、アプリのデータprofileを
  切り替えるものではない

スクリプトは必要な FFmpeg DLL を変更時だけ出力先へコピーし、成果物を起動しない。
実行中のアプリを停止できない場合は `.\scripts\build-dev.ps1 -PreserveRuntime` を使う。
出力先と同じ絶対パスの core / remote / EPUB worker / MCP / browser-host が実行中なら、停止せずエラーで終了する。
この指定を省略した場合は、従来どおり同じ出力先のプロセスを停止してからビルドする。
停止分岐の回帰は `scripts/test-build-dev-safety.ps1` で実プロセスを操作せず確認できる。
エディタはこの停止対象に含めず、出力先のエディタが稼働中なら `-PreserveRuntime` の有無にかかわらず、保存・終了を求めて中止する。
通常profileを汚さない隔離確認が明示的に必要な場合は、同じバイナリへ
`--data-dir .\target\dev-runtime\data` を渡す。build flavor自体をportableへ変えない。
C++ VST3 bridge 自体を変更した場合は、従来どおり先にCMakeでbridgeを再ビルドする。

これは通常profile上での実装確認用であり、launcher・release最適化・埋め込みasset展開・
変更したVST3 bridge・署名・packagingまで含む確認には `scripts\build-release.ps1` を使う。
配布物は必ず `scripts\build-dist.ps1` で作る。

### 修正完了後のユーザー実機確認

アプリ機能・実行時挙動の修正が完了して関連テストが通ったら、原則として
`build-dev.ps1` まで実行し、ユーザーへ次の起動コマンドと具体的な確認手順を渡す。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

エージェント自身はこの成果物を起動しない。引数なしでは実利用中の
`%APPDATA%\mimageviewer` を使うため、設定・キャッシュ・ログを更新し得ることをユーザーへ
明記する。single-instance mutexも共有するため、起動前にインストール版／常駐tray版を終了して
もらう。Windows native coreの確認も原則この経路でよい。

launcher・release設定・exact release performance・埋め込みasset・変更したVST3 bridge・
署名・packagingに依存する確認では `build-release.ps1` の成果物を使い、同じく通常設定を
使用することを明記したうえで次のコマンドを渡す。

```powershell
Start-Process -FilePath .\target\release\mimageviewer.exe
```

ドキュメント、テスト、build scriptだけの変更には実機確認用バイナリは不要。

### エージェントによる自動操作

実アプリを起動・操作する検証は、原則リリース前の検証枠で、対象・所要時間・PC使用範囲を
提示して利用者の明示了承を得てから行う。会話中の予告だけでは実行しない。
通常は実装・非対話テスト・buildまで進め、実アプリ確認を検証待ちとして残す。
詳細は[実アプリ検証の実行確認](interactive-release-verification.md)。

エージェントはnormal-profileの開発・release・installed実行ファイルを起動しない。
`prepare-portable-smoke.ps1 -TestScript`が準備した
`target\portable-smoke\mimageviewer.exe`と、同`data`だけを使う。
`build-portable.ps1 -SmokeTestScript`は`portable,test-script`を別targetへbuildし、
`target\portable-smoke-package`へ配置する。通常dist・zip・署名には混ぜない。
`-SkipBuild`でもsource/feature/exeの証跡を照合し、元packageのdataはコピーしない。

シナリオの実装・検証状況は [ui-smoke-automation-plan.md](ui-smoke-automation-plan.md)。
準備scriptが成功しただけでは、複数窓PDF・列drag・動画zoomの検証成功とは扱わない。

### 補助 bin

`src/bin` のベンチマーク、probe、パック生成ツールは `dev-tools` feature の対象で、通常の
build/test graph には含めない。必要なときは次の形式で実行する。

```powershell
cargo run --release --features dev-tools --bin bench_search -- --docs 50000
```

補助 bin のうち `build_editing_pack` と `build_trt_pack` が持つ単体テストだけは、
`test-full.ps1` が内部用 `pack-build-tools` feature で同じtest graphへ加える。
ほかの補助 bin はコンパイルされず、本体libを別featureでもう一度コンパイルすることもない。

### crate root の一本化

以前は `main.rs` と `lib.rs` が同じモジュール群をそれぞれ宣言し、全体テスト時に同じ
ソースをbin crateとlib crateとして二重コンパイルしていた。現在は実装と全モジュール宣言を
`lib.rs` に集約し、`main.rs` は `mimageviewer::run()` を呼ぶだけの薄い入口にしている。
これにより本体unit testはlib test executableに一本化され、lib用の互換`app` stubも不要になった。

## リリースゲート

`test-full.ps1` の前提: launcher の build.rs は `target/release/` に core、remote、
EPUB worker、MCP adapter、音楽ビデオエディタ、browser-host の release exe が揃っていることを要求する。
新しい clone や clean 後は `.\scripts\build-release.ps1 -PreserveRuntime`、または次の順で準備する。

```powershell
cargo build --release --bin mimageviewer-core
cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets
cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf
cargo build --release -p mimageviewer-mcp --bin mimageviewer-mcp
cargo build --release -p musicvideo-editor --bin mimageviewer-musicvideo
cargo build --release -p mimageviewer-browser-host --bin mimageviewer-browser-host
```

launcher はこの 6 本の後にビルドする。`musicvideo-editor` は workspace 全体テストに含まれる。
launcher の内包元は Cargo の `OUT_DIR` の profile と同じ出力 tree の `release` を導く。debug-profile テストも release 6 本を前提とし、`[env] CARGO_TARGET_DIR` による子プロセスの環境変更では内包元を変えない。想定外の `OUT_DIR` は fallback せず失敗し、`build.build-dir` による出力先の分離は wrapper が開始前に拒否する。
build-release／build-dist／build-portable／build-dev は開始前に実際の Cargo 実行場所・環境で `cargo metadata --format-version 1 --no-deps --offline` の `target_directory` と `build_directory` を固定出力先と照合し、不一致と `CARGO_BUILD_TARGET` を `-PreserveRuntime` に関係なく拒否する（`build_directory` 未出力時は `target_directory` と同じ扱い、portable 自身の固定 `--target-dir` は維持、設定ファイルの `build.target` は検査対象外）。
配布は test-full → clean → core → remote → EPUB worker → MCP → editor → browser-host → launcher → installer → portable。
エディタは全 build スクリプトで自動終了しない。出力先が使用中なら、保存・終了後に再実行する。
portable の配布フォルダ置換は `-SkipBuild` / `-KeepRunning` でも同じ保護対象となる。

`test-full.ps1` は次のコマンドを実行し、失敗すれば非ゼロで終了する。

### 非対話 gate と live Windows 試験

アプリ起動（standalone eframe Appの`run_native`を含む）、前面切替、稼働中UIへの
実入力送出へ到達する試験は `#[ignore = "live Windows test: ..."]` とし、既定の
`cargo test` / nextest / `test-full.ps1` から外す。pure scheduler、合成イベント、
`egui_kittest` のheadless Harness、wgpu offscreen描画は非対話区分として自動実行を維持する。
アプリ起動等に該当する場合、テスト名、短時間、子プロセスであることは承認の例外にしない。
本体のmessage-only HWND、非表示HWNDのz-order、表示前に空regionを設定した非アクティブpopupと
フォーカス要求probeによる8件は、前面切替・実入力・利用者に見える表示を行わない
headless unit / fake-input試験として、既定の自動gateを維持する。
headlessアプリの前面化操作はテスト専用fake/probeへ置換する。DSPなどの受動的な前面読取は隔離対象外。

専用入口は `scripts/test-live-windows.ps1`。対象はeframeの
`synchronous_damage_returns_before_outer_paint_gate`（damage/paint順序）1件のみ。
exact名のallowlistと `--run-ignored only`、直列実行、retry 0、
0件選択の失敗を使い、他のignored hardware試験や実Everything probeを巻き込まない。
所要は実行約1分＋初回コンパイル数分。使い捨ての窓・メモリ内fixture・loopback IPCだけを使い、
mIV本体、通常profile、実Everythingには接続しない。利用者が隔離desktop/session上で実行し、
その間はキーボード・マウスを使用しない。

**エージェントはこのstandalone runnerを起動しない。** 利用者実行であっても、具体suite、
所要時間、desktop/input使用、使い捨てdata範囲を示して明示承認を得る。
`-InteractiveApproved` は承認取得済みの表明であり、同意を得るプロンプトではない。
既存の正規portable-smoke以外をエージェントが起動しない規則は維持する。
未実施のlive試験は検証台帳に必ず「未実施」と残し、自動gateのPASSと区別する。

runner自体は `powershell -NoProfile -File scripts/test-live-windows-runner.ps1` で
静的／fake Cargo検査できる（窓もRust test executableも起動しない）。
段0aの監査と検証結果は [検証台帳](egui-0.37-stage0a-verification.md) を参照。

### 自動 gate の実行内容

`test-full.ps1` はnextestの存在を確認してから次のコマンドを順に実行する。
未導入なら `cargo install cargo-nextest --locked` を案内して非ゼロ終了し、cargo testへ代替しない。
各段のexit codeを検査し、失敗した段より後は実行しない。nextest各段には共通で
`--profile default --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --no-tests fail`
を渡す。`--config-file <repo>/.config/nextest.toml`も全段で共通とする。
既定filterでテストを落とさず、既存ignored以外の全件を実行し、retryしない。
通常は60秒ごとの警告・300秒で打ち切り、全graphの実測から選んだ12件は120秒ごとの警告・600秒で打ち切る。
12件は完全なtest名のORで指定し、priority 100で先行実行する。
打ち切りは名前付きの失敗としてJUnitに記録され、gateを止める。
上限の延長やretryで通さず、そのテストの原因を別に調べる。

```powershell
cargo nextest run --workspace --features pack-build-tools # + common nextest arguments above
cargo test --workspace --features pack-build-tools --doc --no-fail-fast
cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib # + common arguments
cargo nextest run --manifest-path vendor/egui-winit/Cargo.toml --lib # + common arguments
cargo nextest run --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib # + common arguments
cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,wgpu --lib # + common arguments
cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features default_fonts,glow --lib # + common arguments
```

vendor 4 crate は workspace から除外されているため、追加コマンドで無 filter の lib test を実行する。
egui-winitも現在はstandalone graphを使い、同graphのpatchから正式vendor crateを参照する。
standalone vendor graphもルートのnextest設定を明示して使う。
egui の判定済み release 列 API、IME 処理済み Process press の遮断と診断、GPU mipmap、
native repaint scheduler の回帰をこの経路で保持する。
`vendor/egui` は既存と同じ 0.33.3 に読み取り API だけを加えたもので、root と standalone の
`vendor/eframe` / `vendor/egui-wgpu` に同じ path patch を置く。更新時は各 dependency graph が
同じ local egui を参照することを確認し、直接依存と transitive 依存の型を分裂させない。

workspace 全体、統合テスト、doc testに加え、単体テストを持つpack builder 2本を同じ
`mimageviewer` libのコンパイル結果で実行する。
doctestは旧gateのworkspace範囲を別段で保持し、vendorは従来どおりlib testのみ。
`-SuppressCrashDialogs` のWindows error modeはnextestのテスト子とそのhelperにも継承されることを
PowerShell 5.1 / nextest 0.9.148の非対話probeで検査済み。ゲートの成功・各段失敗時の復元と
未導入時の打ち切りは `scripts/test-release-build-safety.ps1` で検査する。

`build-dist.ps1` は clean と配布ビルドの前にこのスクリプトを自動実行する。
`-SkipRustTests` は、同一ソースのテストが既に成功し、署名・パッケージングだけを再試行する
場合に限って使う。ソースが変わった後の初回配布ビルドでは使用しない。
