# 開発ビルドとテストの使い分け

機能追加中の反復では変更範囲に合う最小の確認から始め、配布物を作る段階で全体テストを
必ず通す。通常のリリース設定や配布物の内容は変えず、開発時だけ軽い経路を選べるようにする。

## 開発中の基本サイクル

| 目的 | コマンド | 対象 |
| --- | --- | --- |
| 型・借用・依存関係だけ早く確認 | `cargo check -p mimageviewer --bin mimageviewer-core` | 本体 core |
| 変更したモジュールのテスト | `cargo test -p mimageviewer --lib <filter>` | 本体の指定テストだけ実行 |
| 診断用の窓固定・入力観測のテスト | `cargo test -p mimageviewer --lib <filter> --features test-script` | `test_script` / `native_ui_smoke`等の対象。通常core checkも別に行う |
| 実アプリ用の軽量ビルド | `.\scripts\build-dev.ps1` | core と remote service を `dev-runtime` でビルド |
| 自動操作用の使い捨て環境を準備 | `.\scripts\prepare-portable-smoke.ps1 -TestScript` | 診断portableを別出力先でbuildし、固定sandboxへ配置。起動はしない |
| 了承済みのリリース前実アプリ検証 | `.\scripts\ui-smoke.ps1 -Scenario <対象> -InteractiveApproved` | [実行確認](interactive-release-verification.md)後だけ。通常の開発反復・test-fullへ自動連結しない |
| リリース前の自動テスト一式 | `.\scripts\test-full.ps1` | workspace 全体 + テストを持つ補助 bin + 除外された vendor 3 crate の lib test |
| 配布成果物を生成 | `.\scripts\build-dist.ps1` | 全体テスト、clean、release、installer、portable |

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

## 軽量化している範囲

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

### テストプロファイル

`[profile.test]` は `debug = "line-tables-only"` としている。失敗時の関数名・ソース行付き
バックトレースは維持し、完全な型デバッグ情報を含む巨大な PDB の生成量を抑える。

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

`test-full.ps1` は次のコマンドを実行し、失敗すれば非ゼロで終了する。

```powershell
cargo test --workspace --features pack-build-tools --no-fail-fast
cargo test --manifest-path vendor/egui/Cargo.toml --lib
cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --features winit --lib
cargo test --manifest-path vendor/eframe/Cargo.toml --no-default-features --features wgpu --lib
```

vendor 3 crate は workspace から除外されているため、各 manifest から無 filter で lib test を実行する。
egui の判定済み release 列 API、GPU mipmap、native repaint scheduler の回帰をこの経路で保持する。
`vendor/egui` は既存と同じ 0.33.3 に読み取り API だけを加えたもので、root と standalone の
`vendor/eframe` / `vendor/egui-wgpu` に同じ path patch を置く。更新時は各 dependency graph が
同じ local egui を参照することを確認し、直接依存と transitive 依存の型を分裂させない。

workspace 全体、統合テスト、doc testに加え、単体テストを持つpack builder 2本を同じ
`mimageviewer` libのコンパイル結果で実行する。

`build-dist.ps1` は clean と配布ビルドの前にこのスクリプトを自動実行する。
`-SkipRustTests` は、同一ソースのテストが既に成功し、署名・パッケージングだけを再試行する
場合に限って使う。ソースが変わった後の初回配布ビルドでは使用しない。
