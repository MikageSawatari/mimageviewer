# Tantivy 0.26.2 で生成した mIV FTS 索引

`index/` は crates.io の **tantivy =0.26.2**（mIV patchなし）で生成した3文書・1segmentの固定fixture。
`tests/fts_tantivy_compat.rs` は必ず一時directoryへコピーしてから、mIVの `FtsIndex::open_at`、
query / upsert / delete helperを使い、読取・更新・旧新segment merge・再openを検査する。
原本をTantivyで直接開いたり、現在のTantivyで再生成したりしない。

生成元は隣の [generator.rs](generator.rs)。2026-10-28にRust 1.99.1 / Windows x64 MSVCで生成した。
生成crateはworkspace外のClaudeCode scratchpadに置き、rootの `[patch.crates-io]` を使っていない。
生成crateのlockでregistry版0.26.2とcrate checksum
`861facfabd71044968f364837f9a083b56464ba5a59079f88706ee5c451ca069` を確認した。
本体のfooterもversion 0.26.2 / index_format_version 7を記録している。
入力は全15フィールド・その順序・stored/indexed属性・WithFreqs・`mimv_bigram`登録
（NgramTokenizer(2,2,false) + LowerCaser）が `src/fts_index.rs` と同じ。
IndexSettingsは既定、docstoreは製品と同じlz4-compression。writerは64MiB / 1threadで、
3文書を1回commitし、NoMergePolicyを使う。すべて架空のpath・metadataで実ファイルへアクセスしない。

| path末尾 | name | exif_text |
| --- | --- | --- |
| sakura.jpg | 桜 青空.jpg | 東京 公園 |
| rain.jpg | 雨 夜景.jpg | 大阪 雨 |
| mountain.jpg | 山 青空.jpg | 長野 森林 |

全pathは `c:/fixture/` 配下、favorite_idは `11111111-1111-4111-8111-111111111111`、
container=fs / kind=image / mtime=100 / file_size=1024。
ほかの6テキストフィールドもgenerator内の固定文字列を持つ。
testはschemaを現在の新規mIV索引と照合し、stored原文・数値・検索・更新後の正確なpath集合を検査する。
schema変更時は旧fixtureを書き換えず、新しいfixtureを別に追加する。

再生成する場合は、新しいworkspace外directoryに次の `Cargo.toml` を作る。
fixture側にCargo.tomlを置かず、workspace memberにも追加しない。

```toml
[package]
name = "miv-fts-0262-generator"
version = "0.0.0"
edition = "2024"

[dependencies]
tantivy = { version = "=0.26.2", default-features = false, features = ["mmap", "lz4-compression"] }
```

repository rootからの実行例（上記manifestは `$generator/Cargo.toml` に事前配置する）。

```powershell
$generator = Join-Path $env:TEMP 'miv-fts-0262-regenerate'
New-Item -ItemType Directory -Force (Join-Path $generator 'src') | Out-Null
Copy-Item .\tests\fixtures\fts_tantivy_0262\generator.rs (Join-Path $generator 'src\main.rs')
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CARGO_NET_OFFLINE = 'true'
cargo run --manifest-path (Join-Path $generator 'Cargo.toml') --offline --target-dir .\target\fts-0262-generator -- (Join-Path $generator 'new-index')
```

出力先は存在しないdirectoryに限定する。古いfixtureを自動削除・上書きする処理はない。
再生成のsegment ID / opstampは変わり得るため、意味を再現する手順でありbyte同一性は保証しない。
`SHA256SUMS` は今回の固定index全10ファイル（lock用の空ファイル2つを含む）の同一性を記録する。
index合計は8,279 bytes。Tantivy 0.26.2生成物を保持することが目的なので、通常のtestで更新しない。
