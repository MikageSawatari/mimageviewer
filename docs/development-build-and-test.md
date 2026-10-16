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
| 実アプリ用の軽量ビルド | `.\scripts\build-dev.ps1` | core と remote service を `dev-runtime` でビルド |
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

### 作業中のPCと通常データを保持する配布ビルド

エージェントによる配布ビルドには `.\scripts\build-dist.ps1 -PreserveRuntime` を使う。
稼働中のmImageViewerがあれば停止せず失敗し、通常APPDATAのVST3展開キャッシュも削除しない。
子のrelease/portableビルドへ同じ指定を渡す。引数なしの従来CLI動作は変更しない。
この指定は必須テストや署名を省略するものではなく、アプリの起動・操作を許可するものでもない。

同経路では `test-full.ps1 -SuppressCrashDialogs` を子プロセス内で適用する。
Windowsのクラッシュダイアログだけをそのプロセスで抑え、元のerror modeを終了時に復元する。
テストの非ゼロ終了は失敗のままとし、自動再試行や期待値変更は行わない。
スクリプトの回帰確認は `scripts/test-release-build-safety.ps1` を参照する。

### 長い経過時間を扱う collection テスト

Windows の `Instant` は起動直後に大きな時間を引くと underflow するため、過去の
`Instant` を `checked_sub(Duration)` と `unwrap()` で作らない。collection read の
長時間回帰は `TestReadClock::long_elapsed_since(started)` で観測時刻を24時間進める。
これは `cfg(test)` 限定の thread-local scope で、poll・poll delay と後続 Grid read の時計を差し替え、
製品ビルドの時計・動作を変えない。終了・panic 時の復元とスレッド分離、時間減算の再導入は
`collection_store::test_clock::tests` で検査する。実時間の待機期限は通常の `Instant` を使う。

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

## 軽量化している範囲

### nextest と成功台帳

初回だけ `cargo install cargo-nextest --locked` を利用者が実行する（0.9.143以上、導入確認は
`cargo nextest --version`）。通常の反復は `.\scripts\test-focused.ps1 -Base HEAD`、
本体lib全件は `.\scripts\test-lib-full.ps1` を使い、上の「結果の再利用」に従う。
nextestは1テスト1プロセスでAppのprocess-global lockを分離する。
`.config/nextest.toml` はCPU数並列・retryなし・60秒警告のみ、実測の重い5件を先行実行する。
nextest導入時にはテストの削除・新しいignoreはなかった。アプリ起動等を行う試験は下記のlive区分へ分離する。
最終 `test-full.ps1` も同じnextest設定を使い、
nextest対象外のworkspace doctestは同じfeatureで別段の `cargo test --doc` として実行する。
導入時の件数突合・時間比較とテスト模擬の修正は [検証記録](test-speed-report.md) を参照する。

- 全件: `-Features test-script`、`-CargoProfile test`、`-NextestProfile default` を指定可能。
  `-Force` だけが有効な同一結果を再実行する指定。既定の成功閾値は実行9,000件以上。
  ignore以外の全件を選び、default filter・retry・fail-fastによる取りこぼしを防ぐ。
- 台帳と各実行のcommand・JUnit・stdout/stderrは `target/test-lib-full/`。
  同じworktreeの読み取り専用レビューから参照可能。別worktreeとの共有はしない。
  JUnitの件数整合・exit 0・閾値を検査し、失敗・0件・実行中の入力変更を再利用しない。
  台帳が書けなくても通常のテストは実行し、記録失敗は警告だけにする。
- 識別子はGitが列挙した全追跡ファイルと未追跡ファイルの**現在の内容**をSHA-256化する。
  stagingの有無・HEADの移動だけでは変えず、未コミット編集・削除・新規ファイルを検知する。
  ignoredでも `vendor` / `testdata` / `testimage` 等のnative入力・実fixtureを含める。
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
ツールの導入・commit・アプリ起動・通常設定へのアクセスは行わない。
仕様根拠: [nextest JUnit](https://nexte.st/docs/machine-readable/junit/)、
[filterset](https://nexte.st/docs/filtersets/reference/)。

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
`dev-runtime`・`release`・検証記録は保持した。`target/debug` を消すとテスト実行体の隣の
FFmpeg DLL も消えるので、`vendor/ffmpeg/bin/*.dll` を `target/debug/deps/` へ置き直す。

### dev / test プロファイル

`[profile.dev]` は `debug = "line-tables-only"` + `split-debuginfo = "packed"` としている。
test profile は dev を継承するので、設定は `[profile.dev]` だけに書く。失敗時の関数名・
ソース行付きバックトレース (panic、cdb のスタック) は維持し、完全な型デバッグ情報を含む
巨大な PDB / rlib の生成量を抑える。dev と test で設定を分けると、依存クレートが
`target/debug` に 2 通りビルドされるため、分けない。

デバッガでローカル変数・型情報を見る必要があるときだけ、そのビルドで
`$env:CARGO_PROFILE_DEV_DEBUG = "full"` を指定する。

### 実アプリの開発ビルド

`build-dev.ps1` は次の条件で本体 core と同じprotocolのremote serviceをビルドする。

- `dev-runtime` profile: `opt-level = 2`、LTO なし、codegen unit 64、incremental 有効
- 通常 feature set（`portable` は付けない）: 設定・キャッシュ・ログの既定保存先は
  インストール版／release版と同じ `%APPDATA%\mimageviewer`
- launcher を省略して直接起動できるよう、FFmpeg DLL だけを exe の隣へ配置。他の
  DLL・worker・AI model は通常版と同じ埋め込み・展開経路を使う
- 出力: `target\dev-runtime\mimageviewer-core.exe`
  と `target\dev-runtime\mimageviewer-remote.exe`
- Cargo の `dev-runtime` はビルド時間短縮用の最適化profileであり、アプリのデータprofileを
  切り替えるものではない

スクリプトは必要な FFmpeg DLL を変更時だけ出力先へコピーし、成果物を起動しない。
実行中のアプリを停止できない場合は `.\scripts\build-dev.ps1 -PreserveRuntime` を使う。
出力先と同じ絶対パスの core または remote が実行中なら、停止せずエラーで終了する。
この指定を省略した場合は、従来どおり同じ出力先のプロセスを停止してからビルドする。
停止分岐の回帰は `scripts/test-build-dev-safety.ps1` で実プロセスを操作せず確認できる。
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
`--config-file <repo>/.config/nextest.toml --profile default --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --no-tests fail`
を渡す。既定filterでテストを落とさず、既存ignored以外の全件を実行し、retryしない。
60秒は警告だけで打ち切らず、実測の重い5件の先行実行設定も維持する。

```powershell
cargo nextest run --workspace --features pack-build-tools # + common nextest arguments above
cargo test --workspace --features pack-build-tools --doc --no-fail-fast
cargo nextest run --manifest-path vendor/egui/Cargo.toml --lib # + common arguments
cargo nextest run -p egui-winit --lib # + common arguments
cargo nextest run --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib # + common arguments
cargo nextest run --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib # + common arguments
```

vendor 4 crate は workspace から除外されているため、追加コマンドで無 filter の lib test を実行する。
egui-winit は同梱 upstream lock の旧 image / tiff 依存が offline cache に無いため、
root graph の patched dependency を `-p egui-winit --lib` で無 filter 実行する。
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
