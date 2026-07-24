# 開発ビルドとテストの使い分け

機能追加中の反復では変更範囲に合う最小の確認から始め、配布物を作る段階で全体テストを
必ず通す。通常のリリース設定や配布物の内容は変えず、開発時だけ軽い経路を選べるようにする。

## 開発中の基本サイクル

| 目的 | コマンド | 対象 |
| --- | --- | --- |
| 型・借用・依存関係だけ早く確認 | `cargo check -p mimageviewer --bin mimageviewer-core` | 本体 core |
| 変更したモジュールのテスト | `cargo test -p mimageviewer --lib <filter>` | 本体の指定テストだけ実行 |
| 実アプリ用の軽量ビルド | `.\scripts\build-dev.ps1` | core だけを `dev-runtime` でビルド |
| リリース前の自動テスト一式 | `.\scripts\test-full.ps1` | workspace 全体 + テストを持つ補助 bin |
| 配布成果物を生成 | `.\scripts\build-dist.ps1` | 全体テスト、clean、release、installer、portable |

テスト名フィルタは実行するテストを絞るだけで、Cargo がコンパイルする target の種類は
減らさない。反復を短くしたい場合は `--lib`、`--bin`、`--test` も指定して target を絞る。
本体の全モジュールとアプリ固有テストは `--lib`、`tests/<name>.rs` は
`--test <name>` を使う。`mimageviewer-core` bin は `mimageviewer::run()` を呼ぶだけなので、
通常は bin 単独のテストを選ぶ必要はない。

## 軽量化している範囲

### テストプロファイル

`[profile.test]` は `debug = "line-tables-only"` としている。失敗時の関数名・ソース行付き
バックトレースは維持し、完全な型デバッグ情報を含む巨大な PDB の生成量を抑える。

### 実アプリの開発ビルド

`build-dev.ps1` は次の条件で本体 core だけをビルドする。

- `dev-runtime` profile: `opt-level = 2`、LTO なし、codegen unit 64、incremental 有効
- `portable` feature: DLL・worker・AI model を exe に埋め込まず隣に配置
- 出力: `target\dev-runtime\mimageviewer-core.exe`
- データ: `target\dev-runtime\data`

スクリプトは必要な loose file を変更時だけ出力先へコピーし、成果物を起動しない。
VST3 bridge が `vendor\vst3-host\mimageviewer-vst3-host.exe` にあれば開発出力にも配置する。
C++ bridge 自体を変更した場合は、従来どおり先に CMake で bridge を再ビルドする。

これは実装確認用であり、配布判定や Windows native 機能の最終確認には
`scripts\build-release.ps1` を使う。配布物は必ず `scripts\build-dist.ps1` で作る。

### 修正完了後のユーザー実機確認

アプリ機能・実行時挙動の修正が完了して関連テストが通ったら、原則として
`build-dev.ps1` まで実行し、ユーザーへ次の起動コマンドと具体的な確認手順を渡す。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

エージェント自身はこの成果物を起動しない。データは `target\dev-runtime\data` に隔離される。
release固有・Windows native固有の確認では `build-release.ps1` の成果物を使い、通常設定を
使用することを明記したうえで次のコマンドを渡す。

```powershell
Start-Process -FilePath .\target\release\mimageviewer.exe
```

ドキュメント、テスト、build scriptだけの変更には実機確認用バイナリは不要。

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

`cargo test --workspace --features pack-build-tools --no-fail-fast`

workspace 全体、統合テスト、doc testに加え、単体テストを持つpack builder 2本を同じ
`mimageviewer` libのコンパイル結果で実行する。

`build-dist.ps1` は clean と配布ビルドの前にこのスクリプトを自動実行する。
`-SkipRustTests` は、同一ソースのテストが既に成功し、署名・パッケージングだけを再試行する
場合に限って使う。ソースが変わった後の初回配布ビルドでは使用しない。
