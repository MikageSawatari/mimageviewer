# 依存物の更新 — 2026-10-28

backlog §1.460 の段A（Rust・通常crate・PDFium・LibRaw）、段B（ort / ONNX Runtime）、段C（tantivy）の記録。
版、入力資産の同一性、非対話検証を扱い、公開・配布の完了記録ではない。
設計・検収は ClaudeCode、実装・テストは Codex が担当する。

## 結果

| 依存物 | 更新前 | 今回の入力 |
| --- | --- | --- |
| Rust | 1.99.0 | 1.99.1 (`58cf8f9c3`, LLVM 23.1.1) |
| 通常crate | `dc40771ac` の Cargo.lock | ClaudeCode の更新済みlock（`02b73070c`）を使用。ただしzipはレビューで判明した退行のため2.4.2へ固定 |
| PDFium | `chromium/8100`（backlogの更新前記録） | `chromium/8125`, `158.0.8125.0` |
| LibRaw | 0.22.2 | 0.22.3。zlib 1.3.1、turbojpeg-sys 1.2.0 は維持 |
| FFmpeg | `n7.1.5-16-g9a4bb2c579` | 据え置き |
| Tantivy | 0.26.2 + mIV patch | 0.27.0 + 全mIV patch + 追加AND経路改変。追加改変のA/Bは基準未達、扱いは検収待ち |

段Aの結果は以下、段Bは[末尾の記録](#dependency-stage-b)を参照。
段C（tantivy）は[追加AND改変まで実装・検証したが、A/Bの性能基準は未達](#stage-c-and-route)。段D（EffeTune Mixwright / VST3 SDK）はこれから行う。

初回に阻害されたLibRawのアクセス権とbench基準の欠落は、ClaudeCodeが修復・配置した。
LibRaw全225ファイルを照合済み。独立レビューでzip 2.6.1の追加退行2件が確認され、
ClaudeCodeの設計判断に従い、本体とEPUB workerをzip `=2.4.2` に固定した。
固定前に成功したテスト・bench・生成コード確認と、固定後の再検証は下で分けて記録する。
固定後のcheck・ZIP統合18件・EPUB46件・pack6件は成功。本体lib全件は14,551成功 / 1失敗 /
65 skippedで、失敗したページめくりtestの単独再確認は成功した。全件成功とは扱わない。
固定前の検証用exeはzip 2.6.1を含むため、今回の最終依存関係を確認する成果物には使わない。
製品バイナリは起動していない。コミットも行っていない。

## PDFium 資産の同一性

選択資産は V8 / XFA なしの `pdfium-win-x64.tgz`。
ClaudeCode が事前取得したアーカイブを読み、展開済みvendorとバイト単位のSHA-256で照合した。

| 対象 | bytes | SHA-256 |
| --- | ---: | --- |
| 公式 `pdfium-win-x64.tgz` | 3,904,987 | `9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b` |
| `vendor/pdfium/bin/pdfium.dll` | — | `4b0dec92555d69437bef0df37be630181ab3e83a6823ca264ede355a4f2ef994` |

DLL・ヘッダ・import library・CMake metadata・license・VERSION等、資産内の全49ファイルが一致。
照合後の `vendor/pdfium/` に資産外ファイルは残っていない。
DLLのFileVersion / ProductVersionはともに `158.0.8125.0`。
`args.gn` は `target_cpu = "x64"`、`pdf_enable_v8 = false`、`pdf_enable_xfa = false` を維持。
照合結果は `target/dependency-update-20261028/assets.json`。

選択DLLをPythonのctypesから直接ロードする非対話probeを実施した。
通常PDFとRC4-128暗号化PDFを各2ページ生成し、列挙・寸法・4ページの描画を確認。
通常版と復号版の各ページのraster SHA-256は一致し、パスワードなし・誤入力はエラー4で拒否。
結果は `target/dependency-update-20261028/pdfium-probe.json`。
これはDLL単体の検査であり、mIVのPDF worker / pool統合検証の代わりではない。

## LibRaw 対応ソースとnotice

| 対象 | bytes | SHA-256 |
| --- | ---: | --- |
| 公式 `LibRaw-0.22.3.tar.gz` | 612,705 | `7edf96c5b70cff88980a02ab0fc8af125cb6a4a377985573c654439d9d02355c` |
| `htdocs/mimageviewer/libraw-0.22.3-source.tar.gz` | 612,705 | `7edf96c5b70cff88980a02ab0fc8af125cb6a4a377985573c654439d9d02355c` |

取得済みtarballのhashを固定値と比較後、再圧縮せずhtdocsへコピーし、対応する
`libraw-0.22.3-source.tar.gz.sha256` を作成した。アーカイブ本体はgit管理外。
旧0.22.2の追跡済みchecksumは残し、製品ページに旧版ソースへのリンクも保持した。
mikage.toへの公開・公開後の再取得照合はリリース担当の作業で、今回は行っていない。

静的リンクなのでLibRaw DLLは存在しない。CDDL-1.0を選択する方針は変わらない。
0.22.3アーカイブ内の `LICENSE.CDDL` / `COPYRIGHT`、DCB・X3Fソースのlicense headerが
`LIBRAW-LICENSE.txt` に改変なく含まれることを確認し、先頭の版だけ更新した。
`ZLIB-LICENSE.txt` は展開済みzlibのLICENSEとバイト一致。

`Changelog.txt` はbugfix-onlyと明記。X-Trans補間、Sony YCbCr、CRX / x3f / TIFF / DNG等の
境界・計算修正を含む。X-Trans補間には画像メモリを2倍使用する旨の上流注記がある。
機能削除や現像設定の変更は行っていない。

初回の展開済みLibRawは225ファイル中32ファイルを照合できたが、残り193ファイルは
`src` / `libraw` / `internal` 等のサブディレクトリのアクセス拒否で未照合。
ビルドも `vendor/libraw/src/libraw_datastream.cpp` を参照できず、
`crates/libraw-sys/build.rs` のsource missing検査で停止する。
tarball自体の欠落・hash不一致ではなく、展開物へのアクセスが阻害されている。
CodexはACL変更や別ツリーの変更を行わず、ClaudeCode側での修復を依頼した。
ClaudeCodeが `icacls vendor\libraw /reset /T /C` で継承を復元した後、
再照合で全225ファイルのSHA-256が公式tarball内の内容と一致した。

## 更新crateの履歴・影響確認

ネットワークは使わず、ローカルregistryのCHANGELOG / READMEのRelease Notesと旧新ソースを比較。
履歴が同梱されないcrateはソース差分で確認した。`dc40771ac` のlockを基準に、package名ごとの
version集合を比較した。事前更新（`02b73070c`）は196 package（既存188、新規6、消滅2）であり、
初回記録の194は誤りだった。zip固定後は **195 package（既存187、新規6、消滅2）**。
新規はcastaway / compact_str / core_detect / jiff-core / multiversion_no_op / zlib-rs、
消滅はsocks / tinyvec_macros。zipの版集合が更新前と同じに戻った分だけ既存の変更数が1減った。
集計結果は `target/dependency-update-20261028/package-counts-zip242.json`。主な確認結果は次のとおり。

| crate | 更新 | 確認結果・対応 |
| --- | --- | --- |
| zip | 2.4.2 → **2.4.2据え置き（exact pin）** | 2.6.1のdata_start境界検査消失、local header破損の黙ったpage欠落、Io診断詳細の消失を避ける。固定理由・解除条件は次項 |
| flate2 | 1.1.9 → 1.1.10 | 不完全deflateが `UnexpectedEof` になる。maskテストの旧診断文依存を修正し、完全streamの宣言長不一致も検査。既定backendはminiz_oxide（0.8系→0.9系）。lockのzlib-rs追加だけではbackend切替を意味しない |
| encoding_rs | 0.8.35 → 0.8.42 | READMEのRelease Notes。MSRV1.88、ASCII処理・UTF-8検証・legacy decoder境界修正。Shift-JIS表の意味変更は見当たらず。ZIPのCP932名復号に関係 |
| rhai | 1.25.1 → 1.27.0 | AST / 関数cache / progress処理の整理。任意のGrain VM / compact_strが追加されたが既定はAST / smartstringを維持。分類scriptの制限・取消・sandbox回帰が必要 |
| turbojpeg | 1.4.0 → 1.5.1 | CHANGELOG。colorspace / 圧縮機能 / header属性の追加。既存codec呼出しの既定変更は見当たらず。turbojpeg-sysは1.2.0のまま。JPEG保存・RAW埋込プレビューが対象 |
| notify / notify-types | 8.2.0 → 8.2.1 / 2.1.0 → 2.2.0 | Windows watcher本番実装は実質同一。追加API・MSRV1.88等。windows-sys依存が0.61.2へ移動 |
| arboard | 3.6.1 → 3.7.0 | Windows backendは同一、主な変更は他OS。clipboard経由で使用 |
| cc | 1.2.67 → 1.10.0 | CHANGELOG。Windows並列・環境・flag・object指定等の変更。既存crt-static→/MTとAVIF shimのstatic_crt指定は維持。LibRawを含む本体check・release / dev-runtimeビルドとCRT検査は成功 |
| tokio | 1.53.3 → 1.54.1 | CHANGELOG。I/O / scheduler等の修正、1.54.1はtimer変更revert。lock上の親はwasm-bindgen-futuresのemscripten限定依存で、Windows製品runtimeには入らない |

wgpu / core / hal 30.0.1、naga 27.0.3 / 30.0.1、image 0.25.10、windows本体
0.54.0 / 0.61.3 / 0.62.2、rusqlite 0.31.0 / libsqlite3-sys 0.28.0は版更新なし。
windows-sys 0.60.2は依存木から消えた。画像経路の間接依存まで無変更という意味ではない。
zipだけを2.4.2へexact pinし、その他の更新版・圧縮featureは変えていない。
`cargo tree --target x86_64-pc-windows-msvc` でもtokio / zlib-rsが本体のnormal / build依存に
入らないことを確認した。flate2の実際のfeatureは `rust_backend` / `miniz_oxide` /
`runtime_detection` であり、zlib-rs backendへの切替ではない。

### zip 2.4.2への固定と解除条件

本体 `Cargo.toml` と `crates/epub-pdf-worker/Cargo.toml` のzipを `=2.4.2` とし、
固定理由と解除条件を2行のコメントで添えた。
`cargo update -p zip --precise 2.4.2 --offline` は成功し、既存cacheだけでlockを更新した。
2.4.2への変更以外にregistry packageの版・featureを変更していない。

独立レビューのP2 2件をClaudeCodeと実装者が旧新のregistryソースで確認した。
2.4.2の `central_header_to_zip_file` は `find_data_start` を呼び、
`data_start > central_directory.directory_start` なら `InvalidArchive` にする。
2.6.1はこの検査と初期走査を削除し、遅延先 `find_data_start` にも境界検査が無い。
またlocal headerの検査を初回entryアクセスへ遅延したため、2.4.2で `ZipArchive::new()` が
拒否した破損を2.6.1は受け入れる。本体 `enumerate_recursive` の `by_index` エラー時の
continue、書庫変換 `expand_zip` の非strict時のcontinueにより、そのページだけが黙って抜け得る。
既報の `ZipError::Io` のDisplayが内側の詳細を省いて `"i/o error"` だけになる退行も合わせ、
今回の更新では3つの契約をmIV側に再実装せず、旧版を維持する。

ローカル同梱CHANGELOGの2.4.2→2.6.1には安全上の修正の記載が無い。
2.5.0はtime / jiff対応と重複名診断、2.6.1は開く際のlocal header走査の省略が主な記載。
この確認は同梱履歴の範囲であり、ネットワークで追加調査はしていない。

通常の全件一覧作成 `enumerate_recursive` は `0..archive.len()` の全entryを、
ファイル種別・除外・拡張子判定より前に `by_index(i)` で開く。入れ子ZIPも再帰先で同様。
したがって全件一覧を作る経路では遅延したlocal header走査も結局全件行い、
初期走査を省く利点は乏しいという見立ては妥当。ただし `first_image_recursive` /
`read_first_image_recursive` は最初の画像で終了するため、先頭画像だけを得る経路では
未参照entryの走査を省ける余地がある。性能向上が全経路でゼロという意味ではない。

固定を外すときは、候補版または一貫した受入境界で次をすべて満たすことを確認する。

- data_startが中央目次より後ろにならない境界を検査する。
- local header破損によって、一覧・非strict変換でページが黙って抜けない。
- Ioの詳細診断を保つ。

2.6.1対応として一度入れた共有エラーcrateと31変換境界、ZipFile型のgeneric化、
reader性能testの条件緩和と関連文書は、固定に伴ってすべて取り除き、更新前へ戻した。
Rust / LibRaw / PDFium、flate2のmask test適合、版表示snapshot 3枚は維持する。

`git diff dc40771ac -- src crates/epub-pdf-worker crates/zip-error docs/architecture-overview.md docs/virtual-folders.md`
はEPUB manifestのexact pinと理由コメントのみ。ソース・設計文書の差分は0、共有crateは存在しない。
Git管理領域のindex.lock作成はアクセス拒否だったため、indexを操作するrestoreの代わりに
`git show dc40771ac:<path>` の原文をG:の対象11ファイルへ書き戻した。他の作業ツリーは変更していない。
この撤去範囲・単一lock版・全件一覧とfirst-image経路の区別・package再集計は、
別contextのGPT-6.1 Sol / xhighによる限定レビューでも指摘なし。テストは実装者が担当した。

### 同梱版表示のスナップショット

`raw_license_information_light` / `dark` / `expanded_dark` の3枚は同梱版を実際に描くため、
既存baselineとの比較で版の数字だけが不一致になった。新画像を3枚とも目視確認し、
Light / Darkは53 pixels（x=135..141, y=58..67）、expanded Darkは83 pixels
（版表示とlicense先頭行、全体bbox x=85..141, y=58..212）の差分だけであることを確認した。
`UPDATE_SNAPSHOTS=1` でこの3枚だけを更新し、目視済み候補とのbyte一致、
更新モードを外した通常比較3件の成功を確認した。新3枚の合計は238,380 bytes。
App・製品exeは起動せず、既存の純粋な描画fixtureを使っている。

## FFmpeg 据え置きとDLL同一性

`vendor/ffmpeg/VERSION` は `ffmpeg-n7.1.5-16-g9a4bb2c579-win64-lgpl-shared-7.1.zip`。
6本のDLLのProductVersionはすべて `n7.1.5-16-g9a4bb2c579-20260816` と一致した。
ClaudeCodeの事前確認ではsetupのcheckが示す `n7.1.5-12` はこの `-16` より古いbuild。
[前回の記録](dependency-update-20260912.md) と同じ誤検知のためsetupは実行しなかった。
今回ネットワークで最新版の再問い合わせは行っていない。

| 同梱DLL | SHA-256 |
| --- | --- |
| avcodec-61.dll | `44aad335daead4776a02f75c4924339507b90e07b9a0c3864fbbeaf0b4ada978` |
| avfilter-10.dll | `5f252396a306bf60c698ad39db2c0558406f3050b79781b4ce69f4c3938c31f3` |
| avformat-61.dll | `bcecd2d4fc1bebd7a2945f63a940ebc6ad866e1eec0c780189f10f5f365e7443` |
| avutil-59.dll | `76acf8caba651f389a49290ca2bea12bc0e97681f51979a7166680598d0eb4ff` |
| swresample-5.dll | `c56871583c89039c19ed0eca0e99006acc60f311b3ffd43c4690101e0cf60b18` |
| swscale-8.dll | `c21e3a8ceabec551e982f6822e905bbead1fa0fd93f267c84b51838cc9e27ed8` |

## 固定前の検証記録（zip 2.6.1、経緯）

全build / testに `MSBUILDDISABLENODEREUSE=1`、Cargoにofflineを指定。
初回check以降のCargoは `CARGO_BUILD_JOBS=1`。ログは `target/dependency-update-20261028/`。

| コマンド・検査 | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core --locked` | 初回はturbojpeg-sysのMSBuild段で失敗（詳細errorなし、timeoutではない）。jobs=1の再実行では同段を通過したがLibRawのsource missingでexit 101。`check-j1.log` |
| `scripts/test-focused.ps1` | exit 2。共有・未対応pathのため選択テストなし。lib全件へ移行 |
| `cargo test -p local-adjust-core mask_codec --locked` | 初回13成功 / 1失敗。flate2の確定した診断変更による失敗で、不安定testではない。test更新後14成功 / 0失敗。`mask-codec.log`, `mask-codec-fixed.log` |
| `cargo test -p local-adjust-core --lib --locked` | 全269件成功、0失敗。`local-adjust-lib.log` |
| `cargo check -p epub-pdf-worker --locked` | 成功。更新後のzip / quick-xml等を使う別workspace packageのcompileを確認。`epub-check.log` |
| `cargo test -p epub-pdf-worker --lib package::tests --locked` | 26件成功、0失敗。安全なpath / ZIP名 / EPUB解析の回帰。`epub-package-tests.log` |
| `scripts/test-lib-full.ps1` | 642.73秒でexit 101。test実行前にLibRaw source missingでcompile停止、実行0件。timeout・テストの不安定さによる失敗ではない。`lib-full.log` と `target/test-lib-full/runs/20261028T041627399Z-8d6427f9/` |
| `scripts/test-check-dependency-updates.ps1` | 全offline回帰成功（GitHub JSON / VST3 tag / bash発見 / setup行解析） |
| `python scripts/check_site_cards.py` | 49件、超過0 |
| `python scripts/check_ui_glyphs.py` | 危険なUI glyph 0件 |
| `python target/dependency-update-20261028/probe_pdfium.py` | 4ケース成功、計4ページ描画。製品起動なし |
| `cargo fmt`（引数なし） | 成功。新しいrustfmtによる無関係なコード差分なし |
| `git diff --check` | 成功 |

上表の本体check / lib全件の失敗は初回のアクセス修復前の記録。
修復後の本体checkは成功し、RAW / PDF・lib全件・bench / dumpbinの最終結果は下に記録した。
`vendor/bench_baseline.json` もClaudeCodeが既存基準を配置済み。
配置された基準のSHA-256は `2b2d0b88e3ff5edb56c1d8640853e0d35e8634d99e71f8bb8f7ff70f5ce23b0a`。
新規baseline登録や上書きは行わない。
`test-full.ps1` は指示どおり段D後へ延期。

### アクセス修復・ZIP共通変換後の検証（固定前）

| コマンド | 結果 |
| --- | --- |
| `cargo check -p libraw-sys --locked` | 成功。`libraw-check-repaired.log` |
| `cargo test -p miv-zip-error --offline` | 2件成功、0失敗。doctest 0件。`zip-error-tests.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --locked` | 成功。283 warningは整理せず維持。`core-check-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/raw::/)' --locked` | 67件成功、0失敗。27 CC0 sampleの実現像・プレビュー向き検証を含む。実行120.204秒。初回compile・待機を含むwallは977.52秒。`raw-tests-final.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/pdf_loader::/)' --locked` | 113件成功、0失敗。実行0.937秒。`pdf-tests-final.log` |
| `cargo test -p epub-pdf-worker --lib package::tests --locked` | helper接続後も26件成功。`epub-package-tests-final.log` |
| `cargo test -p mimageviewer --features pack-build-tools --bin build_trt_pack --bin build_editing_pack --locked` | 各3件、計6件成功。`pack-tests-final.log` |
| `cargo test -p mimageviewer --test zip_integration --test test_zip_enumerate --test archive_conversion_failure_diagnostics --locked` | 8 / 3 / 1、計12件成功。`zip-integration-final.log` |
| `scripts/test-lib-full.ps1`（アクセス修復後1回目） | 14,551成功 / 1失敗 / 65 ignored。実行520.503秒。zip 2.6.1では交互seekが消え、2領域による追加25%削減の期待値が不成立。一度緩和したが2.4.2固定に伴い元へ戻した。timeout・不安定testではない。`lib-full-final.log`、run `20261028T045838430Z-c97da99c` |
| `cargo test -p mimageviewer --lib positioned_read_window --locked` | 修正後4件成功、0失敗。実行0.14秒。2領域の再利用・取消・clone所有も含む。`zip-window-tests-final.log` |
| `scripts/test-lib-full.ps1`（期待値修正後） | **14,552件成功 / 0失敗 / 65 ignored**。実行502.639秒、wall508.33秒。`lib-full-verified.log`、run `20261028T051629377Z-077f3f5e`。成功receipt `target/test-lib-full/acccfa1df153232f85a16f091e3c2eae4e72b59d470b04e575f67d1d45295c43.json` |
| `cargo test -p mimageviewer --test ui_snapshot raw_license_information --locked -- --test-threads=1` | 更新前は3件とも旧版の数字との差分で失敗。3 baselineの更新後、更新指定なしで3件成功、0失敗。`raw-license-snapshots-check.log` / `-update.log` / `-verified.log` |

上表はzip 2.6.1での履歴。ZIP共通変換とparser期待値の変更は後に取り除いた。
固定後はlockと製品ソースが変わるため、成功済みの全件結果を最終結果として再利用せず再実行する。
snapshotの変更は維持し、固定後のlib全件とは別の3件の検証として記録する。

### bench・製品生成コードのビルド条件（固定前の記録）

検索benchと製品coreは、release最適化を維持してline table / symbolを残す同じ設定を使う。
依存物のcompile成果物を共有するため `target/core-sym` に揃える。
製品coreのbuildには `dev-tools` を付けない。benchの計測時は他のbuild / testを止めた状態にする。

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CARGO_NET_OFFLINE = 'true'
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_PROFILE_RELEASE_STRIP = 'false'
$env:CARGO_PROFILE_RELEASE_DEBUG = 'line-tables-only'
cargo build --release --features dev-tools --bin bench_search --target-dir target/core-sym --locked
cargo build --release --bin mimageviewer-core --target-dir target/core-sym --locked
```

最初のbench用release buildはcoldで2447.68秒。これはcompile所要時間であり、
検索時間の計測結果ではない。製品coreのsymbol buildも成功（1977.15秒、待機を含む）。

製品coreは341,706,752 bytes、SHA-256
`bc04519d09e2e3ae7df38fa1fd66d5090e1b35323e449c0d9dfe88e7609b72ed`。
選択したPDFium DLLの完全一致copyはoffset 312,855,448に1個だけ存在した。

`dumpbin /disasm:nobytes target/core-sym/release/mimageviewer-core.exe` は成功。
全出力 `target/dependency-update-20261028/core-disasm.txt` から、次の関数全体を抜粋した。

| 対象 | code範囲 | `addss` / `divss` / `mulss` | `unpcklps` / `addps` / `divps` |
| --- | --- | --- | --- |
| `intersection::impl$3::score<TermScorer, Box<dyn Scorer>>` | `0x144C73C90`–`0x144C73DCA` | 4 / 1 / 1 | 0 / 0 / 0 |
| `intersection::score_out_of_line<TermScorer>` | `0x144C6A3E0`–`0x144C6A46B` | 1 / 1 / 1 | 0 / 0 / 0 |

Intersectionのleft / rightは両方とも第1genericの `TermScorer`、第2genericはothers用。
したがって上記は§9.5の二語Term採点を含むspecialization。
`0x144C73D12` の右項helperへのcallも残っている。
問題のpacked浮動小数点演算は再導入されず、`movaps` / `xorps` 等の移動・zeroingとは区別した。
抜粋は `intersection-term-score.txt` / `term-helper.txt`、判定とhashは `generated-code-check.json`。
この2関数の範囲・specialization・命令数は独立reviewでも指摘なし。

全build / test終了後に、既に生成した同じbenchバイナリを直接実行した。
追加のCargo rebuildを計測へ混ぜず、5万件のcorpusと既存の基準を比較している。

```powershell
& .\target\core-sym\release\bench_search.exe --docs 50000 --json target/dependency-update-20261028/bench-run1.json
python scripts/check_bench_regression.py vendor/bench_baseline.json target/dependency-update-20261028/bench-run1.json
```

benchバイナリのSHA-256は `03df19f7f9d1073f74a5d2d3900eda1dca968011de50f9b12d826836148319a8`。
有効な計測JSONは `bench-run1.json`（SHA-256
`881867649ca9b2ba94d4961515769fc2feeffd5399545ed70ec41f3dfa006913`）。
補助runnerの表示encodingと、実行記録JSONと計測JSONの名前の衝突を修正し、同じバイナリで
再計測して保存した。衝突時の比較はquery欠落エラーであり、性能回帰の結果とは扱っていない。
有効な1回の計測で10クエリすべてが規定の回帰条件（+30%超かつ+1.0ms超）に該当せず、
比較scriptはexit 0。baselineのSHA-256は配置時と同じで、`--save` は使用していない。

| クエリ | baseline ms | 今回 ms | 差 ms | 判定 |
| --- | ---: | ---: | ---: | --- |
| en_ai_phrase | 1.3500 | 1.5809 | +0.2309 | OK |
| en_common | 16.8501 | 16.0483 | -0.8018 | OK |
| generic_jp | 25.0732 | 25.3344 | +0.2612 | OK |
| medium_jp | 7.2017 | 6.9304 | -0.2713 | OK |
| medium_jp_3 | 0.7442 | 0.8319 | +0.0877 | OK |
| medium_jp_and | 1.6528 | 1.9525 | +0.2997 | OK |
| rare_jp | 1.0340 | 1.0036 | -0.0304 | OK |
| rare_jp_and | 0.1543 | 0.2406 | +0.0863 | NOISE（+55.9%だが絶対差がfloor以下） |
| super_generic | 0.0062 | 0.0067 | +0.0005 | OK |
| unique_id | 1.1204 | 1.3189 | +0.1985 | OK |

実行ログは `bench-execution1.log` / `bench-compare-final.log`。

### 固定前の検証用ビルド（最終成果物としては失効）

`scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` は成功（wall 2514.34秒）。
core / remote / EPUB workerを通常feature・`dev-runtime`最適化profileで生成し、
FFmpeg 6 DLL・公式CRT 4 DLL・既存EffeTune bundleを配置した。
他のnative buildと重ならない段階で開始し、残留MSBuild nodeを待つ処理は省略した。
preflightと生成物のPE検査は成功（runtime=4、PE=3）。
`portable` / `test-script` featureは使っていない。

| 検証用exe | bytes | SHA-256 |
| --- | ---: | --- |
| `target/dev-runtime/mimageviewer-core.exe` | 337,879,040 | `10d04d4c917e091b3b86c35f677acfb702f7c8a211f7c7d8e8e23fd1de7c6ca2` |
| `target/dev-runtime/mimageviewer-remote.exe` | 14,836,736 | `703713e0976d8e8030b592981297de3eadac81f4a57e6b19c383ca81ebd69a21` |
| `target/dev-runtime/mimageviewer-epub-pdf.exe` | 5,100,032 | `7d9280dd28fcf2a77a39503a0f7c55748e4c2b7a0731900261d1adfb49c9751e` |

通常profile coreにも選択PDFium DLLが完全一致で1個だけ埋め込まれている。
hash・配置検査は `verification-artifacts.json` / `target/vcrt-pe-reports/dev-runtime.json`。
このexeはzip 2.6.1を含む固定前の成果物であり、最終依存関係の実機確認には使わない。
製品を起動する操作は行っていない。今回の再確認では、toolchainが変わらないため
bench / dumpbinの再実行は不要というClaudeCodeの指定に従った。
残る全workspace / vendor / doctestの `test-full.ps1` と最終依存関係の製品buildは後続段で行う。

## zip 2.4.2固定後の最終検証

全build / testに `MSBUILDDISABLENODEREUSE=1`、`CARGO_NET_OFFLINE=true`、
`CARGO_BUILD_JOBS=1` を指定。固定前の全件成功台帳は再利用しない。
ログは `target/dependency-update-20261028/zip242-*.log`。

| コマンド | 結果 |
| --- | --- |
| `cargo update -p zip --precise 2.4.2 --offline` | 成功。registry版変更はzipだけ、共有workspace crateを除去 |
| `cargo check -p mimageviewer --bin mimageviewer-core --locked` | 成功、wall 71.51秒。既存283 warningの整理は行わない。`zip242-core-check.log` |
| `cargo test -p mimageviewer --test zip_integration --test test_zip_enumerate --test test_archive_convert --test archive_conversion_failure_diagnostics --locked` | **18件成功 / 0失敗**（8 / 3 / 6 / 1）。入れ子ZIP・名前復号・7z変換・cancel・publish失敗時の診断と既存出力保護。compile込みwall 269.03秒。`zip242-integration.log` |
| `cargo test -p epub-pdf-worker --lib --locked` | **46件成功 / 0失敗**（package 26件を含む）。path / ZIP / EPUB解析・生成HTML・PDF結合・WebView設定等の非対話テスト。wall 4.70秒、test実行0.30秒。`zip242-epub.log` |
| `cargo test -p mimageviewer --features pack-build-tools --bin build_trt_pack --bin build_editing_pack --locked` | **各3件、計6件成功 / 0失敗**。compile込みwall 228.17秒。`zip242-pack.log` |
| `scripts/test-lib-full.ps1` | **14,551成功 / 1失敗 / 65 skipped、exit 100**。test実行516.467秒、compile等込みwall 983.16秒。`zip242-lib-full.log`、run `20261028T064756861Z-289cf1ad`。全件成功receiptは生成されない |
| `cargo nextest run -p mimageviewer --lib -E 'test(=ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions)' --config-file .config/nextest.toml --profile default --ignore-default-filter --run-ignored default --retries 0 --no-tests fail --locked` | 失敗した1件の単独再確認は **1成功 / 0失敗**、14,616 skipped。実行2.696秒、wall 4.81秒。`zip242-page-turn-rerun.log` |
| `cargo fmt --check` | 成功。`zip242-fmt-check.log` |
| `cargo fmt`（引数なし、最後のCargoコマンド） | 成功、無関係なコード差分なし。`zip242-fmt-final.log` |
| `git diff --check` | 成功 |

全件JUnit内の `zip_loader::` 63件、`archive_converter::` 39件、`image_folder_zip::` 14件は
すべて成功（各0 failed / skipped）。元の追加25%削減条件へ戻したreader性能testも成功した。
抽出結果は `zip242-junit-summary.json`。全件runのsource IDは
`1d22d2beba7ff220b5c2c798d76b59ccaae508dfc3a9a64dddaeaa164a01f3a7`。

唯一の失敗は `ui_fullscreen::page_turn::draw_submit_tests::page_turn_mainwindow_submits_visible_vacant_leaf_both_directions`。
`draw_submit_tests.rs:401` で期待 `Started` に対し実際 `Finished`（実行2.718秒）。
command runnerのtimeoutやchannelエラーではなく、テスト内のassert失敗である。
同名testは [backlog §1.447](next-release-backlog.md) と
[egui段0aの記録](egui-0.37-stage0a-verification.md) に全件時の失敗・単独時の成功履歴がある。
ただし過去はold9可視性assertであり、今回とは失敗箇所が異なる。

コードでは同testが `Instant::now() - duration * progress` を設定してから実描画fixtureを呼び、
本体の `MotionClock::progress` は実時計で経過を再計算する。fixtureのdurationは600msで、
progress 0.75では残り150msの間にも描画・schedule時間が進む。今回のFinishedはこの時間依存と
整合するが、失敗フレームの実経過を測っていないため、負荷だけが根因とは断定しない。
同じソース・lockでの単独再確認は成功し、このUI test / 製品経路は本作業で変更していない。
既知の間欠失敗履歴としてClaudeCodeへ引き継ぎ、期待値・待ち・retry・ignoreで隠す変更はしない。
**判断事項は、この1件を既存§1.447の調査へ引き継いで段Aを受け入れるかどうか。**
単独成功で全件成功へ読み替えず、全件再実行で成功するまで繰り返すこともしない。

全件終了後の編集は本記録のみ。製品ソース・lock・fixtureは維持している。
toolchainは1.99.1から変えず、bench / dumpbinは再実行しない。

## 変更ファイル

事前反映済みの `scripts/setup-libraw.sh` は再編集していない。
`Cargo.lock` は事前更新からzipだけを2.4.2へ戻した。共有エラーcrateの登録は残っていない。

| ファイル | 理由 |
| --- | --- |
| `rust-toolchain.toml` | 1.99.1へ固定 |
| `Cargo.toml` / `Cargo.lock` | zipを2.4.2へexact pinし、固定理由・解除条件を記録。共有crate登録は撤去 |
| `crates/epub-pdf-worker/Cargo.toml` | 本体と同じzip exact pinと理由・解除条件を記録 |
| `crates/libraw-sys/build.rs` | 必須ソース版・復旧診断を0.22.3へ合わせる |
| `crates/local-adjust-core/src/mask_codec.rs` | 不完全deflateの診断変更へ既存test・説明を適合し、宣言長検査も維持 |
| `scripts/bootstrap-vendor.sh` | setupの案内版を更新 |
| `scripts/test-check-dependency-updates.ps1` | 現行LibRaw版のoffline mockを更新 |
| `LIBRAW-LICENSE.txt` | 先頭の同梱版を更新。上流notice本文は維持 |
| `installer/readme.txt` | 同梱版の対応ソースURLを更新 |
| `installer/readme_portable.txt` | 同上 |
| `htdocs/mimageviewer/index.html` | 同梱版・新ソースリンクを更新し、旧版リンクを保持 |
| `htdocs/mimageviewer/libraw-0.22.3-source.tar.gz.sha256` | 新しい公式ソースの追跡checksumを追加 |
| `docs/libraw-source-distribution.md` | 現行版・取得／staging手順・固定hashを更新 |
| `docs/native-dependencies.md` | 現行LibRaw / PDFium版、旧checksum保持を明記 |
| `docs/raw-libraw-plan.md` | 現行版への案内を追加。採用時0.22.2の調査根拠は保持 |
| `docs/README.md` | 本記録への索引を追加 |
| `docs/dependency-update-20261028.md` | 本記録 |
| `tests/snapshots/raw_license_information_light.png` / `raw_license_information_dark.png` / `raw_license_information_expanded_dark.png` | 同梱版とlicense先頭行の0.22.3表示を反映。3枚を目視・差分範囲で確認 |

<a id="dependency-stage-b"></a>

## 段B: ort / ONNX Runtime

作業基準は `deps-update-1460` の `e16e323da`。本体manifestのrc.15指定とpatch解除、
vendor DLL更新はClaudeCodeが事前実施し、本段でworkspace統合・API追従・fork撤去を行う。
他worktree、TensorRT pack、通常profileのデータは変更せず、コミット・製品起動・ネットワーク取得は行わない。

| 対象 | 更新前 | 段B |
| --- | --- | --- |
| ort / ort-sys | 2.0.0-rc.12（ortはローカルbackport） | registry 2.0.0-rc.15、C API 24 |
| ONNX Runtime DirectML | 1.24.2 | 1.24.4 |
| TensorRT packのONNX Runtime GPU | 1.24.2 | 据え置き。pack再生成なし |

### 入力資産とfeature

取得済み `Microsoft.ML.OnnxRuntime.DirectML` 1.24.4 nupkg のSHA-256は
`57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681`。
ローカル資産を読み、次の2本がnupkg内 `runtimes/win-x64/native/` とバイト一致することを確認した。
`vendor/ort/VERSION` は1.24.4。nuspecの依存は `Microsoft.AI.DirectML` 1.15.4で、
`DirectML.dll` は従来どおり同梱しない。結果は `target/dependency-update-20261028/stage-b/assets-and-lock.json`。

| DLL | bytes | SHA-256 |
| --- | ---: | --- |
| `onnxruntime.dll` | 17,328,152 | `e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40` |
| `onnxruntime_providers_shared.dll` | 22,040 | `265c8daf29637cb259cac8be9f08f2cd45f3883f0f0e4949cbfddd5b4cbec3b6` |

rc.15の既定featureは `api-31`。`load_dynamic::init` はDLLのminor版が
`MINOR_VERSION = ort_sys::ORT_API_VERSION` 未満なら `BadVersion` を返すため、
DirectML 1.24.4と既存GPU 1.24.2を使う本workspaceでは `api-24` を明示する。
`tools/local_adjust_lab` も `=2.0.0-rc.15` / `default-features = false` とし、
`std` / `ndarray` / `tracing` / `api-24` / `load-dynamic` を指定した。
本体の `directml` / `cuda` / `tensorrt` は維持する。

`cargo tree -e features -i ort --workspace --offline` とort-sys側の同検査で、
API featureは17〜24の累積だけとなる。`api-25` 以上、`download-binaries`、
`tls-native`、`copy-dylibs` は入らない。`preload-dylibs` はrc.15の
`load-dynamic`が有効にするローカルDLL preload helperで、ネットワーク取得ではない。
lockは不要となったTLS/download依存等22 packageを削除し、新しいpackageや他crateの版変更はない。
依存名の曖昧さが消えたことによるversion suffixの省略も含まれる。

### API差分と§1.241の維持

旧forkとローカルregistryソースを比較した。rc.15の `load_dynamic::LoadError` は
公開別名 `ort::LoadDynamicError` を維持し、`Dlopen` / `MissingApi` / `BadVersion`
と追加の `InvalidVersionString` を直接構築する。構築・Display・DropでORT APIを呼ばず、
失敗中の `G_ORT_LIB` へ再入しない。upstream `17ed727` の修正目的を満たすため、
`crates/ort-patched/` 全84 fileとworkspace excludeを撤去した。
§1.241調査・当時の検収記録とTensorRT packの過去リリース記録は変更しない。

| 差分・確認対象 | 対応・維持する挙動 |
| --- | --- |
| `EnvironmentBuilder::commit() -> bool` → `build() -> Result<Environment>`、暗黙のglobal強参照廃止 | 既存 `ORT_INIT` の成功型を `InitializedOrt { backend, environment }` にする。process lifetimeの強参照を保持し、labも既存OnceLockで環境を保持する |
| `Session::builder()` → `Session::builder(&Environment)` | constructorで確定した同じ環境を渡す。SessionBuilder/Session自身も環境cloneを保持する。AiRuntimeにOption等の別状態を足さない |
| 環境生成のエラー | workerで生成し既存Failed terminalへ返す。TRT DLLの `init_from` 成功後はDLL選択が固定済みなので、build失敗をDirectMLへ再初期化しない。init_from自体のErrに対する既存fallbackは維持 |
| DirectML / CUDA / TensorRT EP登録 | DMLのdevice filter/preference、TRT→CUDAの順、cache/FP16/workspace/builder level/hardware compatibilityのoptionsと値は維持。`Error<SessionBuilder>::recover()` と既定のEP登録失敗時CPU fallbackも維持。rc.15内部のRegisterError統合はアプリ側で新しい分岐を要しない |
| tensor / dtype | owned ndarray f32とTensorRef入力、dtype検査付き抽出を維持。変換・コピー・精度設定の追加なし |
| ログ | `tracing`を明示維持。`ORT_LOG`とseverity→tracing levelの対応は同一。環境buildエラーには段階を付す。ORT内部EPログにsource等の変更はあるが、アプリ側の通知・原因伝播は維持 |
| process共通AI owner / typed failure | Dormant→Initializing→Ready/Failed、constructor一度、cancel/wake、RuntimeInit→親の非retry分類は変更しない。環境の所有だけを同じ初期化結果へ追加する |

rc.12も環境の実生成は最初のSession等まで遅延していた。rc.15では明示build時となるため、
この失敗は初期化worker内で早く確定する。GUI threadで待たず、追加retryやguardは導入しない。
独立GPT-6.1 Sol / xhighレビューは、この環境寿命の設計に着手前合意し、
製品差分とprobe修正の限定再確認でもblocking / should-fixなし。テストの実施は実装担当が所有し、
レビュー担当は再実行していない。

### 段Bの検証

全build/testは `MSBUILDDISABLENODEREUSE=1`、`CARGO_BUILD_JOBS=1`、
`CARGO_NET_OFFLINE=true` で実行。直接Cargoコマンドには `--locked --offline` を指定する。
ログは `target/dependency-update-20261028/stage-b/`。製品バイナリは起動しない。

| コマンド | 件数・結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core --locked --offline` | 初回はcommit削除・builder引数追加の3 compile error。API追従後成功、283 warnings（段Aと同数）。`core-check.log` / `core-check-fixed.log` |
| `cargo check --workspace --all-targets --locked --offline` | exit 101。launcherが埋め込むrelease版core / remote / EPUB workerの3 exeが無く、既存build.rsの前提検査で停止。ORTのAPIエラーや取得資産の欠落ではない。`workspace-check.log` |
| `cargo check --workspace --all-targets --exclude mimageviewer-launcher --locked --offline` | 成功。上記の制約を分ける補足検査で、元のworkspace全体コマンドの成功とは扱わない。`workspace-check-without-launcher.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/ai::/)' --locked --offline` | 初回64件中63成功 / 1失敗。追加probeがbuild情報に版番号を含むと誤想定していたため。DLLロード・環境生成は成功しており、不安定testではない。想定修正後は64件全成功、0失敗（実行1.134秒）。`ai-tests.log` / `ai-tests-fixed.log` |
| `.\scripts\test-lib-full.ps1` | **14,553成功 / 0失敗 / 65 skipped**、実行495.566秒、exit 0。今回は§1.447のページめくりを含め失敗なし、単独再確認なし。`lib-full.log` と下記台帳 |
| `python scripts/check_ui_glyphs.py` | 危険なUI glyph 0件。`ui-glyphs.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 成功、exit 0。core 11分40秒、remote 0.33秒、EPUB worker 12.25秒。通常feature / dev-runtime、portable無効。内部CRT/PE gateはruntime 4 / PE 3成功。`build-dev.log`、成果物hashは `verification-build.json` |
| `cargo fmt`（引数なし）、`git diff --check` | 最終実施でともに成功。fmt前後で変更対象Rust 2 fileのSHA-256は不変。`fmt.log` / `diff-check.log` |

既存のロード失敗testは別processで欠落DLLとexport不足DLLを渡し、
`Dlopen` / `MissingApi` が5秒以内に返ることを確認する。
追加した `vendor_ort_loads_api24_environment_session_and_f32_tensor` も別processを使い、
vendor DLLの実ロード、API 24、build情報取得、環境生成、session builder、
owned ndarray f32の値・shape・不正dtype拒否を確認する。通常data_dirを使わず、
このprobeだけtelemetryを無効にする。BadVersion/InvalidVersionStringの動的fixtureはなく、
これらのAPI非依存エラー構築は上流ソース比較で確認する。

`GetBuildInfoString` の実際の値は
`ORT Build Info: git-branch=HEAD, git-commit-id=2d924974ef, build type=RelWithDebInfo`。
版番号を含む契約は無いため、testはAPI呼出しで非空文字列を取得できることを検査する。
DLLの版・同一性は上記nupkg / hash照合で確認する。

lib全件の成功台帳は
`target/test-lib-full/f301d8ea71a7968575ae0a95cf15d76e8b0164704fffe1cd5e0f2f1b5d9c29fe.json`、
run directoryは `target/test-lib-full/runs/20261028T073601174Z-c129d6a8/`。
検証source IDは `5970a776e8a3cfbb78ee5ead01ecebd9e4ea424f278acf1f3e4555f3debe5333`。
全件後の編集はこの記録と索引 `docs/README.md` だけで、製品・testソースとlockは維持する。

workspace全体checkの残りはrelease埋込素材3 exeを用意した後の再実行。
別profileのexeやダミーを置いてlauncherの前提検査を通すことはしない。
このreleaseビルド前提を次段へ引き継ぐか、段Bの検収前に用意するかはClaudeCodeへ委ねる。
本段で `test-full.ps1` / bench / 単独dumpbin検査は追加しない。

### 利用者の実機確認（未実施）

通常profileの確認用coreを用意した。coreのSHA-256は
`d1515357fbcf65750ba16a790c64d0368a72ba939858cb3c2978e3e55cdb80b8`、337,954,816 bytes。
remote / EPUB workerも同じ作業ツリーから更新済み。3 exeのhashは
`target/dependency-update-20261028/stage-b/verification-build.json`。

インストール版・常駐tray版を終了してから、repository rootで利用者が実行する。
single-instance mutexを共有し、引数なしでは通常の `%APPDATA%\mimageviewer` を使うため、
実際の設定・データを更新し得る。エージェントはこのbinaryを起動していない。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

1. DirectMLで既存モデルのアップスケール・デノイズ・MI-GAN消しゴム・editing pack被写体分離を実行し、結果とエラー通知が従来どおりか確認する。
2. 既存TensorRT pack（GPU ORT 1.24.2）で既存engineを読み、推論・DirectML非対応モデルの既存経路・backend切替を確認する。pack再生成やキャッシュ削除は本段で行わない。
3. Remote AIと動画upscaleで初期化後の処理を確認する。初期化失敗があれば原因が通知され、45秒timeoutの反復やUI停止にならないことを確認する。

製品の実GPU推論・画質・速度・配布形態は上の非対話検査だけでは検収していない。

### 段Bの変更ファイル

| ファイル | 理由 |
| --- | --- |
| `Cargo.toml` / `Cargo.lock` | fork exclude撤去、API 24の理由コメント、不要なdownload/TLS依存のlock整理。本体のrc.15指定・patch解除は事前反映済み |
| `tools/local_adjust_lab/Cargo.toml` | 本体とrc.15 / default無効 / API 24を揃え、workspace統合でAPI 31が混入するのを防ぐ |
| `src/ai/runtime.rs` | process共通の環境所有と明示session builder、環境生成エラーの既存terminal伝播、vendor実ロード回帰を追加 |
| `tools/local_adjust_lab/src/main.rs` | 既存OnceLockで環境を保持し、session builderへ渡す |
| `crates/ort-patched/**` | 上流に修正が入ったローカルfork全84 fileを撤去 |
| `scripts/setup-ort.sh` | DirectML既定1.24.4とAPI 24選択の根拠を更新 |
| `scripts/setup-tensorrt-pack.sh` / `.ps1` | GPU ORT 1.24.2のpinを維持し、rc.15 + API 24との互換をコメント |
| `scripts/test-test-tools.ps1` | workspace外patchを未対応pathとして扱う既存fixture名を、撤去したfork名から一般名へ変更。検査内容は同じ |
| `CLAUDE.md` / `docs/native-dependencies.md` / `docs/project-reference.md` | 現行版・feature契約・環境所有・fork不使用へ更新 |
| `docs/dependency-update-20261028.md` | 本段の入力同一性、差分、検証、実機未確認を記録 |
| `docs/README.md` | 更新記録の索引を段A/Bへ合わせる |

第三者ライセンス表示・追跡noticeにはort / ONNX Runtimeの旧版番号は無く、
`src/ui_dialogs/about.rs` のONNX Runtime / MIT表示は変更不要だった。

<a id="dependency-stage-c"></a>

## 段C — tantivy 0.27.0とmIV改変

作業基点は `deps-update-1460` / `5acf4c310`、開始時はclean。コミット・push・製品起動なし。
ほかの作業ツリーは変更せず、生成crateだけを指定されたworkspace外scratchpadへ作った。
ビルド・テストは `MSBUILDDISABLENODEREUSE=1` / `CARGO_NET_OFFLINE=true`、Cargo CLIも原則 `--offline`。
script内部にもoffline環境を継承した。以下は初回候補の記録で、性能・全体gateへのClaudeCodeの判断と
AND経路の追加改変・再検証は[追補](#stage-c-and-route)を参照。

### 入力・依存版・マージ

ClaudeCode取得済みの `scratchpad/dl-tantivy/` のcrateを再照合した。registryのソースと原本は改変していない。

| 入力 | SHA-256 |
| --- | --- |
| tantivy 0.26.2 | `861facfabd71044968f364837f9a083b56464ba5a59079f88706ee5c451ca069` |
| tantivy 0.27.0 | `ad1ebacf6bc124b741ffbfb088f9835cb55e1e2ff57e182e9219e3efa1377b97` |

0.27.0のpackaged VCS commitは `67889269cae67063c372bc236b102d632a021568`。
base=0.26.2原本 / ours=旧vendor / theirs=0.27.0原本として改行を正規化し3者マージした。
上流追加39ファイルを配置、上流削除7ファイルを削除。非追跡の単体Cargo.lockを除く原本405ファイル中398ファイルが
最終照合でもbyte一致し、相違はmanifestのfeatureとローカル改変6ソースだけだった。
ローカル専用 `.cargo-ok` / `MIV-PATCH.md` / `atomic_write_diagnostics.rs` も維持した。

rootの通常/dev依存2か所を0.27へ変更し、`cargo update -p tantivy --offline` はexit 0。

| crate | 旧 → 新 |
| --- | --- |
| tantivy-columnar / sstable / stacker | 0.7.0 → 0.27.0 |
| tantivy-common / bitpacker / query-grammar | 0.11.0 / 0.10.0 / 0.26.0 → 0.27.0 |
| tantivy-tokenizer-api | 0.7.0据置 |
| datasketches / lru / lz4_flex / murmurhash32 | 0.2.0 / 0.16.4 / 0.13.1 / 0.3.1 → 0.5.0 / 0.18.5 / 0.14.0 / 0.4.0 |
| 追加 | base64 0.23.1（既存0.22.1も残る）、serde-tuple-vec-map 1.0.1、unwrap-infallible 1.0.0 |

root lockのSHA-256は `a8e9298560d97ce90779f6aaa5717691db9bc887cc3f06d2a70a9844d135925e`。
単体vendor test専用のfrostem/dev依存はroot graphへ持ち込んでいない。
以下のログは全て `target/dependency-update-20261028/stage-c/` を基点とする。
原本差分・中間物・一覧は `merge-inventory.json` / `port-audit-final.json` に保存した。

| 変更対象 | 理由・判断 |
| --- | --- |
| root Cargo.toml / Cargo.lock | 通常/devの2指定と依存graphを0.27へ更新。既存featureを維持 |
| vendor Cargo.toml / Cargo.toml.orig / .cargo_vcs_info.json | 0.27.0原本へ置換。normalized manifestだけに既存miv-test-hooksを保持 |
| directory/mmap_directory/mod.rs / directory/mod.rs | persistだけの4回制限・段別診断・hookを保持。上流のtry_exists・非atomic writerのflush整理も反映。競合なし |
| indexer/index_writer.rs | rollbackの所有引数をIndexWriter::new(self.index.clone(), ...)とし、observer継承を保持（手動競合解消） |
| indexer/segment_updater.rs | try_schedule_task / diagnostic scope / executed-task hookと上流debug!を両立（手動競合解消）。plugin-aware merge/GCも維持 |
| indexer/segment_manager.rs | source集合拒否のprivate typed outcomeを保持。競合なし |
| query/intersection.rs | 右辺だけのscore_out_of_lineを保持。ローカルtest ctorのnum_docsをsegment_num_docsへ追従 |
| query/boolean_query/boolean_weight.rs（追補） | ClaudeCode指定で純Term ANDのpruningだけ旧Intersection経路へ戻し、密な2語/4語の採点・順位回帰を追加 |
| その他のvendor source / examples / notices等 | 全て0.27.0原本へ置換・追加・削除。新block_wand_intersectionにも追加のmIV改変なし |
| tests/fts_tantivy_compat.rs / tests/fixtures/fts_tantivy_0262/** | 実0.26.2生成索引と通常open/update/merge/reopenの固定回帰 |
| .gitattributes | index/**を-textとし、JSONを含むfixture原本のcheckout時変換を防ぐ |
| vendor/tantivy/MIV-PATCH.md | 基準0.27.0と移植・検証を追記。0.26.xの履歴を保持 |
| docs/search-architecture.md / docs/README.md / 本文書 | 現行候補版・検収状態、更新索引、入力/差分/検証を記録 |

別contextのGPT-6.1 Sol / xhighが旧新原本からの差分を照合した。
persist・diagnostic・observer・source-set拒否のローカル追加/削除行は旧版と一致し、intersectionの差はtest ctor名追従のみ。
rollback継承、plugin merge失敗の通知、lock解放後のcallback、typed拒否と公開Future互換性に指摘なし。
新しい回復処理、guard、fallback、retry拡大、warning掃除は行っていない。

### §1.424 — 採点経路とdumpbin

新BooleanWeightは純粋な複数TermScorerかつReadFreqのANDをTermIntersectionとし、top-levelの
for_each_pruningで新block_wand_intersectionを使う。bench_searchの純Term ANDはこの経路になる。
mIVのbigram AND→source OR→token AND→Basic filterという通常の入れ子では、子weightのscorer() /
into_box_scorerが従来Intersectionを作るため、既存barrierは今も必要である。
一方、各tokenがunique bigramを1つ持ち、sourceが1つ、追加Basic filterなし等で子が単一TermScorerまで
展開される場合は、mIVも新経路へ到達する。benchだけで旧製品経路の確認を代替できない。

新経路はleaderのBM25を候補bufferへ先に計算し、別passでsecondaryをseek/採点する。
旧版の左右BM25を同一式で評価する構造はない。既存barrierは削除せず、新barrierも追加していない。
以下の実生成コードでこの判断を確認した。

symbol coreはRust 1.99.1 / release / 通常製品feature、CARGO_INCREMENTAL=0 /
CARGO_PROFILE_RELEASE_STRIP=false / CARGO_PROFILE_RELEASE_DEBUG=line-tables-only。
`cargo build --release -p mimageviewer --bin mimageviewer-core --target-dir target/core-sym --locked --offline`
はexit 0、12分07秒。`dumpbin /disasm:nobytes target/core-sym/release/mimageviewer-core.exe` はexit 0、27.94秒。
core SHA-256は `4df2aa47adc5858150e0a324c869061eaedb3fe582a04da5b733eaba58635517`。

| 製品関数 | instruction数 | unpcklps / addps / divps | scalar mulss / addss / divss |
| --- | ---: | --- | --- |
| score_out_of_line（Box） | 4 | 全0 | 0 / 0 / 0（vtableへ渡す） |
| score_out_of_line（TermScorer） | 33 | 全0 | 1 / 1 / 1 |
| block_wand_intersection | 1156 | 全0 | 6 / 19 / 6 |
| Intersection::score（Box） | 39 | 全0 | 0 / 3 / 0 |
| Intersection::score（TermScorer） | 72 | 全0 | 1 / 4 / 1 |

上流の追加でimpl$3はimpl$5となった。同じScorer実装を型・symbolで同定した。
旧helperへのcall、新blockへのBooleanWeightのcallも残る。
証跡はdumpbin-scoring.json / scoring-function-{1..5}.txt、full dumpはcore-disasm.txt（約1.16GB）。
独立レビューでも集計・scalar計算・callを確認し、追加barrier不要に同意した。
ただし呼出し先が全てscalarという主張ではない。直接callee BlockSegmentPostings::block_max_scoreは
addps 4 / divps 2を含むが、8文書の全laneをmovdqu・変換・broadcastで初期化する意図的SIMDであり、
既知のstale upper lanesとは異なる。性能全体の保証とは分けて扱う。

性能回帰を確認した後にbench自身も追加dumpした。5対象関数は全て上記3命令が0、
新blockは1213命令（製品とは展開が異なる）、scalar数は6 / 19 / 6だった。
bench-dumpbin-scoring.json / bench-scoring-function-{1..5}.txtを参照。
この確認も性能回帰の根因を確定するものではない。

### 0.26生成索引の互換性

同梱CHANGELOGと旧新原本・local registryを比較し、形式の不互換は見つからなかった。
下位crateに独立CHANGELOGがない範囲はソースで比較した。

| 形式 | 根拠 |
| --- | --- |
| 本体 | INDEX_FORMAT_VERSION=7 / oldest=4、directory/footer.rsの許容範囲を維持 |
| columnar | format_version.rsはbyte同一。writer V2 / reader V1,V2。reader製品コードも変更なし。変更はmapping/batch read/linear overflow修正等 |
| sstable | writer v3、reader v2/v3を維持。旧dictionary→新indexへ移動、ordinal修正等 |
| docstore | store/footer.rs / writer.rs / schema/document/de.rsは同一、V2を維持。LZ4のu32 LE size + block形式も維持 |
| postings | term_info.rs同一。serializerは参照型・flush等の変更で形式変更は見当たらず。readerはblock探索最適化等 |
| common / stacker等 | common serialize/vintに差分なし、stacker変更はarena境界等。tokenizer-apiは据置、query-grammarは入力解析変更 |
| schema/meta | text_options.rsは旧bool / with_tokenizerのJSONを保持。新custom extension列は旧metaで空へdefault |

上流v6/v7 fixtureは旧新で全SHA-256一致し、format testsも成功した。ただし上流v7は0.23.0生成なので、
ClaudeCodeの承認に従い、実0.26.2で小さなmIV schema fixtureを追加した。
指定scratchpad配下のmiv-fts-0262-generatorに独立crateを作り、registry =0.26.2 /
default無効 + mmap,lz4-compressionを使用した。lockのsource/checksumとstore footerの0.26.2 / format7を確認済み。
root patchを使わず、workspaceにも追加していない。
全15フィールド・順序・属性・mimv_bigram（NgramTokenizer(2,2,false)+LowerCaser）がmIVと一致する。

固定indexは3文書・1segment・10ファイル・8,279 bytes。隣のREADME.md / generator.rs / SHA256SUMSに
manifest、再生成手順・コード・由来・hashを残した。testは必ずtemp directoryへコピーし、通常FtsIndex::open_atで開く。
現行新規schemaとの一致、旧日本語bigram検索・全stored原文・数値、upsert/delete/add→commit/reload、
旧新segment双方の存在を確認した明示merge、再open後の正確なpath集合・件数・内容を検査し成功した。
原本はtest後も全10ファイルのSHA-256一致。Git filter後のblob hashもrawと同じ（fixture-audit.json）。
独立レビューのCRLF変換防止指摘に対しindex/**を-textにし、属性unsetを確認した。ほかに重要な指摘なし。
このfixtureはmIV schemaの回帰であり、未使用FAST等を含む全Tantivy schemaの網羅保証ではない。

### 自動検証

| コマンド・対象 | 件数・結果 / ログ |
| --- | --- |
| cargo check -p mimageviewer --bin mimageviewer-core --locked --offline | 前回checkpointでexit 0、1分08秒。同じ製品コード・root lockの証拠を再利用。以後の追加はtest/docs。check.log |
| cargo test -p mimageviewer --test fts_tantivy_compat --test fts_tantivy_integrity --locked --offline | 新互換1成功（0.17秒）+ integrity14成功（2.39秒）、exit 0、compile 3分07秒。fts-integration-resume.log |
| vendor nextest --manifest-path vendor/tantivy/Cargo.toml --lib --features miv-test-hooks --locked --offline -E 'test(/query::intersection::tests\|block_wand_intersection::tests\|compat_tests::test_format\|directory::mmap_directory::tests\|indexer::segment_manager::tests/)' | 31成功 / 1322 skipped、4.876秒。採点bits/順序/負ゼロ、block property、mmap、format6/7。tantivy-tests-resume.log |
| vendor nextest 同条件 -E 'test(/indexer::index_writer::tests\|indexer::segment_updater::tests/)' | 59成功 / 1294 skipped、8.527秒。tantivy-indexer-tests.log |
| cargo nextest run -p mimageviewer --lib --locked --offline -E 'test(/search\|fts\|tantivy/)' | 588成功 / 14030 skipped、17.042秒、compile 7分18秒。search-lib.log |
| cargo tree -p mimageviewer -e features,normal,build -i tantivy --locked --offline | exit 0、mmap/lz4のみでmiv-test-hooksを含まない。tantivy-normal-features.log |
| .\scripts\test-lib-full.ps1 | exit 0、14553成功 / 65 skipped / 失敗0、512.168秒。lib-full.log |
| .\scripts\test-full.ps1 -SuppressCrashDialogs | exit 100、workspace 15934成功 / 7失敗 / 69 skipped。下記詳細。test-full.log |
| 最終cargo fmt（引数なし）、cargo fmt --check、git diff --check | 全てexit 0。fmt.log / fmt-check.log / diff-check.log |

lib全件の3件SLOWは音楽144.453秒、RAW223.058秒、大規模scan305.215秒で、既存600秒枠内で成功。
§1.447のページめくり間欠失敗はこの実行で観測していない。
JUnitはtarget/test-lib-full/runs/20261028T084453964Z-e26f91d9/、
source IDは64a7f570a41bb90531df45259defca6735724141f453656081794bbc7341b789。
実行中の文書追記でInputs changedとなり再利用台帳は保存されなかった。製品/test/fixture/lockは変更していない。
exit 0と全件成功JUnitは有効で、文書編集だけを理由に同じlib全件を再実行していない。
全体gateはworkspace/pack-build-toolsとvendorの別graphを確認するため追加した。

### bench回帰（失敗、baseline未変更）

Rust 1.99.1 / release / dev-tools / symbol条件でbench_searchをビルド（exit 0、15分46秒）し、
自分のビルド・テスト終了後に同じbinaryで --docs 50000 --json <runN.json> を3回実行した。
binary SHA-256はa4629d520d84f8322a30f06836a7b8b0256d9f716b0b26989a91444a28cad5a1。

| generic_jp（カメラ、2-bigram AND） | total ms | post ms |
| --- | ---: | ---: |
| baseline | 25.0732 | 5.6843 |
| run1 | 38.5156 | 4.3448 |
| run2 | 38.2961 | 4.5563 |
| run3 | 38.5754 | 4.6907 |

python scripts/check_bench_regression.py vendor/bench_baseline.json <run1.json> <run2.json> <run3.json> はexit 1。
最良値でも+52.7% / +13.2229 msで、+30%かつ+1msを超える。3回の幅は0.2793msで、今回はノイズ扱いにしない。
他9queryは判定閾値内（rare_jp_andは+35.3% / +0.0545msのNOISE）。
--saveは使わず、baseline SHA-256は前後とも2b2d0b88e3ff5edb56c1d8640853e0d35e8634d99e71f8bb8f7ff70f5ce23b0a。
JSON / log / command / hashはbench-run{1..3}.json / bench-comparison.log / bench-result.json。

total-postは約33.7〜34.2ms対baseline19.3889msで、post遅延では説明できない。
新block経路の候補score/buffer、secondary別pass、block_max/suffix/threshold処理がdenseな2termで
余分な作業になることは原因候補だが、実行頻度やprofileを測っておらず根因は未確定。
独立レビューも性能gate失敗を保持し採否判断へ返す結論に同意した。
hits=10000はHARD_MAX打切り件数であり、結果集合/順位の同一証明ではない。
benchは単一field純Term ANDで、通常mIV入れ子queryの速度差を直接示すものでもない。
製品にも到達し得る新経路なのでgate失敗は有効である。回避patch/guard/fallbackは追加していない。

### 中断・準備失敗と今回のbuild

前回、rootの-p tantivy単体testは非workspace packageのdev依存制約でexit 101、
manifest指定nextestはmetadata解決の未cache frostemでexit 102、いずれも実行0件だった。
その時点の「足りなければ止めて報告」に従い、実装者が開始済みの統合testとsymbol core sessionへ
Ctrl-Cを送り中断した。runner exit 1はtimeout/製品test失敗ではなく、中断を成功へ読み替えていない。
今回ClaudeCodeがcache補充後、cargo generate-lockfile --manifest-path vendor/tantivy/Cargo.toml --offlineは成功し、
互換範囲のfrostem 1.20260821.7/dev依存を選んだ。単体test後のvendor Cargo.lockは削除済み。
証跡はvendor-test-resolved.lockへ保存し、commit対象に含めない。

通常release wrapperは-PreserveRuntime -SkipVst3Bridge --locked --offlineで稼働中processを検出し、
cargo開始前にexit 1。process停止やAPPDATAのcache操作は行っていない。
target/releaseに成果物がまだないことを確認し、同repositoryの新規出力へcore→remote→EPUB→launcherを
直接cargoで作った。初回jobs=4はturbojpeg-sysのCMake/MSBuild段でexit 101（MSBuild exit 1、詳細compiler errorなし）。
段Aの通過実績を基にjobs=1で再実行し、全4段成功した。timeoutではなく、製品不具合とは分類しない。

| release直接build（--locked --offline、incremental=0、jobs=1） | 結果 |
| --- | --- |
| core | exit 0、43分09秒 |
| remote --features embedded-web-assets | exit 0、3分34秒 |
| EPUB worker | exit 0、1分59秒 |
| launcher | exit 0、約46秒 |

release-j1-result.json / release-j1-stage-*.logを参照。通常feature、portable無効、未署名の検証用成果物。
公式VC runtime 4本を出力先へ配置し、製品4 exeのPE gateはruntime=4 / pe=4成功。
初回PE確認は実装者の指定がtarget/release全体と広すぎ、build-scriptも走査して失敗した。
製品4 exeに訂正した再実行はexit 0（release-pe-selected.log / release-pe.json）。
アプリは起動していない。

| 検証用release成果物 | bytes | SHA-256 |
| --- | ---: | --- |
| mimageviewer.exe | 506586112 | 8d913f989e36cdbcdaa3a1900a0677c9ba74e5d114e4c722286df3e274009e1d |
| mimageviewer-core.exe | 342678016 | 21d7a1ba69eb93e268fa2b688226496052cf9d79040f73923b2d1000144cc4dc |
| mimageviewer-remote.exe | 13839872 | 26d2ec4860b9a8c2ead8a8f049c289f60c0d2e4bfdd7fc20a39240729d126e5e |
| mimageviewer-epub-pdf.exe | 5023232 | d57f1ddd81a98cacd9c558f4c36043eb46a7943b6de86d79fe6831a39e450602 |

詳細はrelease-artifacts.json。実機確認は未実施。性能回帰を判断した後、必要なら利用者がrepository rootで実行する。
通常の %APPDATA%\mimageviewer を使い実際の設定・データを更新し得る。single-instance mutexも共有するため、
インストール版・常駐tray版を先に終了する。エージェントはこのbinaryを起動していない。

```powershell
Start-Process -FilePath .\target\release\mimageviewer.exe
```

確認するのは、既存索引を開き、2語AND・source/favorite filterと通常の索引更新が従来どおり動くこと。
既存索引の削除・再構築や通常設定のresetを検証準備として要求しない。

### 全体gateの失敗と限定再確認

全体gateはexit 100。workspace / pack-build-toolsの56 binariesで15934成功 / 7失敗 / 69 skipped
（15941実行、668.814秒）。compileは22分49秒、script全体2059.40秒。
第1段で停止したため、後続workspace doctestとstandalone egui系vendor 5 graphは未実施
（vendor Tantivyの90件とは別）。全体gateを成功扱いにしない。test-full.log / test-full-result.jsonが元の結果。

| 失敗 | 観測と限定再確認 |
| --- | --- |
| video::audio::tests::headless_seek_readiness_keeps_audio_tx_draining_without_a_device | 5秒期限後にaudio queueが5件残り、0件のassertに失敗。同じworkspace featureの2件限定runでは成功。既存lib全件でも成功したが、同名の既知backlogは見つからず原因未確定 |
| video::clockless_transcode::tests::finite_source_reaches_end_after_filling_the_live_window_without_a_next_fetch | producer待ち5秒Timeout。同名の負荷時timeoutはbacklog §1.389に既記録。上と同じ限定runで成功。§1.447のページめくり失敗ではない |
| test_ai_upscale: test_upscale_all_models / test_upscale_full_size / test_runtime_and_model_loading | onnxruntime.dll extractがos error 5。既存integration testにdata_dir overrideがなく、非cfg(test) libraryのdefault APPDATAへ展開する経路をソースで確認 |
| test_tile_blend: test_screentone_patterns / test_screentone_tile_blend | 同じORT DLL抽出のアクセス拒否 |

音声2件は同じworkspace / pack-build-tools / --locked --offline条件で完全名filter、retries=0を指定し、
2成功 / 16008 skipped、1.246秒（timing-failures-isolated.log）。
実時間5秒に依存する検査と全件時のみの失敗は確認できるが、両方を一括して既知不安定と断定しない。
閾値延長・ignore・再試行設定・製品変更は加えていない。

AI5件は通常profileのまま再実行せず、子processのAPPDATAだけをrepository target内の新規使い捨てdirectoryへ向け、
同じworkspace featureで失敗5件だけをtest-threads=1 / retries=0で再確認した。
5成功 / 2 skipped（ほか54 binaries除外）、17.765秒。
commandと隔離先はai-failures-isolated-result.json、出力はai-failures-isolated.log。
通常設定のコピー・reset・rename・ACL変更は行っていない。
この隔離再確認は元の全体gate失敗を取り消さない。integration testの通常profile依存をTantivy移植とは
別のtest setup課題として残し、実profileへの再試行で通さない。
この全体実行でも§1.447のページめくり失敗は出ていない。

### 初回候補の最終照合とClaudeCodeへの判断事項

最終cargo fmt（引数なし）/ cargo fmt --check / git diff --checkは全てexit 0。
ログはfmt.log / fmt-check.log / diff-check.log。workspace fmt前後で全RustソースのSHA-256は不変。
単体generatorだけはworkspace外のため別途rustfmtをかけ、保存ソースとscratchpadを同期した。
fixture全10ファイル、root lock、bench baselineのhashは最終照合でも不変。vendor Cargo.lockは存在しない。
最終文書QAでPowerShell→Pythonのstdin文字コードによる日本語破損を検出し、UTF-8経路で修復した。
製品/test/fixtureへの影響はなく、文書の日本語・placeholder・差分も再確認した。

ClaudeCodeに判断を返すのは、generic_jpの性能回帰を伴う0.27.0候補を採用するか。
採用保留・別の性能調査chunk・遅延を受容したbaseline更新の判断は設計/検収側で行う。
実装者はbaselineを更新せず、性能回避の症状patchも追加していない。コミットはClaudeCodeへ委ねる。
加えて、全体gateの安全な隔離条件と時間依存2件の検収・別課題扱いを検収側へ返す。
限定再確認の成功を元の全体gate成功へ読み替えない。

<a id="stage-c-and-route"></a>

### 追補 — 純Term ANDのpruning経路を0.26相当へ戻す

ClaudeCodeは0.27.0を採用し、`boolean_weight.rs` の `for_each_pruning` にある
`SpecializedScorer::TermIntersection` の腕だけを従来経路へ戻すと決定した。
0.26.2原本はこのspecializationを持たず、通常のboxed scorerを `for_each_pruning_scorer` に渡す。
0.27では `into_box_scorer(TermIntersection(...), &self.score_combiner_fn, num_docs)` で
同じIntersectionを作り、その採点ループへ渡す。union、scorer構築、閾値更新や厳密な `>` 比較は変えない。
既存 `score_out_of_line` を維持し、`block_wand_intersection.rs` と上流テストは原本のまま残した。
製品経路から呼ばれなくなった関数・再exportのwarningは掃除せず残している。

理由は初回のgeneric_jpだけの遅延と経路変更の一致。ただし同時間帯に他worktreeの5本のbuildが
動いていたとの情報があり、負荷を切り分けるため保存した改変前binaryと改変後binaryを交互に測る。
撤去条件は「上流の経路がmIVの密なbigram ANDで0.26と同等以下の速さになったら」。
改変箇所の短いMIVコメントと `MIV-PATCH.md` にも理由・初回測定値・撤去条件を記した。

追加testは513文書、2語/4語ANDでposting block境界を越え、通常scorerとの全doc ID・score bitsの一致と、
TopDocsのlimit 1/10/全件におけるscore・同点順序まで検査する。
初回は0.27 TopDocs APIの `.order_by_score()` を追加testが呼び忘れcompile失敗した。
修正後は1件成功（0.036秒）。製品コードの失敗や間欠失敗ではない。
別contextのGPT-6.1 Sol / xhighによる独立レビューでは追加改変に重要な指摘なし。

test-fullは今回再実行せず、AI5件はsandboxの通常APPDATA展開拒否による環境要因、時間依存2件は段D後にClaudeCodeがsandbox外の最終test-fullで再確認する（ClaudeCode判断）。

以下の追補ログは `target/dependency-update-20261028/stage-c-and/` を基点とする。

| 追補の自動検証（全てlocked/offline、node reuse無効） | 件数・結果 |
| --- | --- |
| vendor nextest --manifest-path vendor/tantivy/Cargo.toml --lib --features miv-test-hooks -E 'test(/query::boolean_query::\|query::intersection::tests\|compat_tests::test_format\|directory::mmap_directory::tests\|indexer::segment_manager::tests\|indexer::index_writer::tests\|indexer::segment_updater::tests/)' | 122成功 / 1232 skipped、12.220秒。新回帰、上流block property、既存改変を含む。vendor-tests.log |
| cargo test -p mimageviewer --test fts_tantivy_integrity --test fts_tantivy_compat | 互換1成功（0.45秒）+ integrity14成功（8.50秒）、compile 6分18秒。fts-integration.log |
| cargo nextest run -p mimageviewer --lib -E 'test(/search\|fts\|tantivy/)' | 588成功 / 14030 skipped、26.470秒、compileを含む704.75秒。search-lib.log |
| cargo tree -p mimageviewer -e features,normal,build -i tantivy | mmap/lz4のみ、miv-test-hooksなし。tantivy-normal-features.log |

上記にテスト失敗・中断はなく、§1.447の間欠失敗も出ていない。
単体テスト用vendor Cargo.lockは削除済み。完全なcommand・exit code・wall timeはverification-results.json。

追加改変後の製品coreは、通常featureで次を実行し成功した（45分52秒、build-core.log）。
`MSBUILDDISABLENODEREUSE=1` / `CARGO_NET_OFFLINE=true` / `CARGO_BUILD_JOBS=1` /
`CARGO_INCREMENTAL=0` / `CARGO_PROFILE_RELEASE_STRIP=false` /
`CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` を指定した。

```powershell
cargo build --release -p mimageviewer --bin mimageviewer-core --target-dir target/core-sym --locked --offline
```

core SHA-256は `dc5ccb2ec046359e35c59d160eb6da9af55a6f27a4790a7f34a3e24d76ee2649`。
コンパイラ引数もopt-level=3 / lto=thin / codegen-units=16 / line-tables-onlyを確認した。
ビルドは長時間CPU処理を続けて完了し、timeout・中断・再起動はしていない。
PE gateも製品coreだけを指定してruntime=4 / pe=1、exit 0（core-pe.log / core-pe.json）。

`dumpbin /disasm:nobytes` はexit 0だが、MSVCのPDB readerは今回のPDBをInvalid file formatとして拒否し、
関数名なしの命令列になった。作業directoryをPDBの隣へ変えても同じで、製品の失敗ではない。
LLVMのllvm-pdbutilではPDBを読め、PEのRSDSとGUID/ageが一致した（GUID
63EE3BC6-E7FB-B49C-4C4C-44205044422E、age 1）。Tantivyの16 moduleからS_GPROC32のsection/offset/code sizeを取得し、
PE section RVAへ変換して、同じcoreのdumpbin出力から各関数の全命令を抽出した。
MSVC readerが拒否する具体的な理由までは特定していない。PDB SHA-256は
`fefa9b5bbba4cc8b4d18802fd5e48a0e5f16fe42ac53312dbb01c1c03f8b0fe2`。

| 前回の5関数との照合 | 命令数 | unpcklps / addps / divps | mulss / addss / divss |
| --- | ---: | --- | --- |
| score_out_of_line（Box） | 4 | 0 / 0 / 0 | 0 / 0 / 0 |
| score_out_of_line（TermScorer） | 33 | 0 / 0 / 0 | 1 / 1 / 1 |
| Intersection::score（Box, Box） | 39 | 0 / 0 / 0 | 0 / 3 / 0 |
| Intersection::score（TermScorer, Box） | 72 | 0 / 0 / 0 | 1 / 4 / 1 |
| block_wand_intersection | 製品から除去 | 対象コードなし | 対象コードなし |

pruningのTermIntersection分岐は0x144CE0938でinto_box_scorer（0x144CD8220）を呼び、
その腕は0x144CD8292でintersect_scorers（0x144C06BE0）を呼ぶ。
返すvtable 0x1534E2408 / 0x1534E2540のscore slot +0x98は、それぞれ上表の2種のIntersection::scoreを指す。
pruningループもこの+0x98をcallし、閾値を比較・更新する。新block関数へのcallはない。
両scoreからhelperへのcallも残る。ICFによりhelperのアドレスはexclude::score（Box）/
TermScorer::scoreと共有されるため、別名だけを見てbarrier消失と誤認せず、PDBの同一addressを照合した。
証跡はdumpbin-scoring.json、functions-{1..4}.txt、routes-{1..5}.txt、core-disasm.txt、
core-pdb-summary.txt、core-pdb-tantivy-*.txt、resolve-pdb.py / check-callgraph.py。
独立レビューでもPE/PDBのhash・GUID/age、16 moduleの2572 proc recordと各範囲、元dumpbinからの
9抽出ファイルの全命令、4 direct callのE8 rel32、両vtableのQWORDを別途照合し、重要な不備なし。
この確認は指定採点経路の証拠であり、全calleeがscalarであることや実行性能の証明ではない。

前節のtarget/release成果物は追加改変前。今回の改変を含む通常製品coreは
`target/core-sym/release/mimageviewer-core.exe` であり、エージェントは起動していない。

#### 交互A/B 6回（採否基準未達）

改変前benchは元のSHA-256 `a4629d520d84f8322a30f06836a7b8b0256d9f716b0b26989a91444a28cad5a1` を確認し、
`target/dependency-update-20261028/bench_search_unpatched.exe` へコピーして保存した。
改変後はcoreと同じrelease/symbol環境で次を実行し、exit 0、17分30秒（build-bench.log）。

```powershell
cargo build --release -p mimageviewer --features dev-tools --bin bench_search --target-dir target/core-sym --locked --offline
```

改変後bench SHA-256は `46c2ebae6eff91d6a1c624a93aa01ecd61a3dde89f29defd059e4cd1510c80b3`。
同じdirectoryへbench_search_patched.exeとして保存もした。
管理下のbuild/test/dump/review処理を終えてから、2026-10-28 20:55:25〜20:55:48 JSTに
改変前→後→前→後→前→後を実行した。全回同じcwd・`--docs 50000 --json <run.json>`。
ホスト全体の無負荷は測定しておらず、外部負荷なしとは扱わない。
bench本体は全6回exit 0、各回をcheck_bench_regression.pyで個別比較した結果は全てexit 1。

generic_jpのbaselineはtotal 25.0732 ms / post 5.6843 ms。

| 順 | binary | generic_jp total ms | post ms | baseline比 | 比較exit / 回帰query数 |
| --- | --- | ---: | ---: | ---: | --- |
| 1 | 改変前 | 46.9803 | 5.7155 | +87.4% | 1 / 4 |
| 2 | 改変後 | 54.1618 | 6.1193 | +116.0% | 1 / 5 |
| 3 | 改変前 | 60.3189 | 6.8732 | +140.6% | 1 / 6 |
| 4 | 改変後 | 54.7336 | 6.2259 | +118.3% | 1 / 4 |
| 5 | 改変前 | 55.1229 | 6.5629 | +119.8% | 1 / 5 |
| 6 | 改変後 | 58.2245 | 6.7648 | +132.2% | 1 / 5 |

各群3回のquery別最良値による比較も前exit 1 / 後exit 1。

| query | baseline ms | 前best ms | 後best ms | 後の判定 |
| --- | ---: | ---: | ---: | --- |
| en_ai_phrase | 1.3500 | 1.7715 | 2.2528 | NOISE |
| en_common | 16.8501 | 26.0123 | 28.2095 | REGRESSION |
| generic_jp | 25.0732 | 46.9803 | 54.1618 | REGRESSION |
| medium_jp | 7.2017 | 10.6922 | 11.2843 | REGRESSION |
| medium_jp_3 | 0.7442 | 1.3624 | 1.1450 | NOISE |
| medium_jp_and | 1.6528 | 2.9555 | 2.3803 | NOISE |
| rare_jp | 1.0340 | 1.0231 | 1.3516 | NOISE |
| rare_jp_and | 0.1543 | 0.2800 | 0.3362 | NOISE |
| super_generic | 0.0062 | 0.0086 | 0.0078 | OK |
| unique_id | 1.1204 | 1.7855 | 1.5392 | NOISE |

改変後generic_jpのbestでも+116.0% / +29.0886 msで、閾値内への回復はない。
best同士では後が前より+15.3%だが、ペア差は+15.3% / -9.3% / +5.6%と符号が混在する。
前の幅は13.3386 ms、後は4.0627 ms、中央値は前55.1229 / 後54.7336 ms。
このばらつきと他queryの両群での遅延を含む計測から、改善・悪化の因果や新block経路の寄与率は確定しない。
経路変更だけを元の遅れの原因として切り分けられたとも扱わない。
前の回帰4件→後3件というquery別bestの合成結果だけで全体改善とは言わない。
hits=10000も打切り件数であり、検索結果同一性の証明には使わない（正しさは上記の回帰テストで検査）。

独立レビューでも実binary/baselineのhash、6 JSONの順・値・exit、best比較を照合し、
「機能・生成コードは通過、性能は未達、追加改変は採否保留の候補として保持」という扱いに同意した。
6回以外の追加計測や性能patch、baselineの--saveは行っていない。
baseline SHA-256は前後とも `2b2d0b88e3ff5edb56c1d8640853e0d35e8634d99e71f8bb8f7ff70f5ce23b0a`。
ログはab-{1..6}-{unpatched,patched}.json / .log / -comparison.log、
ab-{unpatched,patched}-combined.log、ab-results.json / ab-summary.json / run-ab.py。

#### 追補の照合と引継ぎ

原本405ファイルとの最終照合は397一致、相違はmanifestと既存6ソースにboolean_weight.rsを加えた7ソース。
新block本体・moduleは原本とbyte一致し、source-audit.json / port-audit.jsonに記録した。
0.26.2固定索引・root lock・baselineは追加改変で変更していない。

ClaudeCodeへ返す判断点は、性能基準未達だった追加AND経路改変の扱いと、負荷を管理した再計測などの次の条件。
0.27.0採用方針自体を再度問い合わせず、指定の追加改変を未採用候補として残す。
実装者の判断で別のguard/fallback/最適化を足したり、今回の候補をcommitしたりしない。
段D後のsandbox外test-fullはClaudeCode担当のまま。

必要な実機確認に使うなら、通常版・常駐tray版を先に終了し、repository rootで次を実行する。
通常の %APPDATA%\mimageviewer を使い、実際の設定・データを更新し得る。エージェントは起動しない。
確認対象は既存索引の2語AND、source/favorite絞込み、通常の索引更新。
性能基準未達の候補であり、実機操作の成功を性能gateの代用にはしない。

```powershell
Start-Process -FilePath .\target\core-sym\release\mimageviewer-core.exe
```

最終のcargo fmt（引数なし）/ cargo fmt --check / git diff --checkは全てexit 0。
照合したRust 1106ファイルはfmt前後で不変。fixture全10ファイルのSHA-256とGit filter後のblob、
root lock・baseline・保存した改変前benchのhashも一致し、vendor Cargo.lockは存在しない。
追補のUTF-8も確認した（final-audit.json、fmt.log / fmt-check.log / diff-check.log）。
ブランチはdeps-update-1460、HEADは5acf4c310のまま。コミット・push・製品起動は行っていない。
