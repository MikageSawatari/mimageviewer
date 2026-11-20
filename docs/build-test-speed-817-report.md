# ビルド・テストの時間とメモリ (コミット) の調査 — §1.817 (2026-11-20)

担当: ClaudeCode Opus (計測・設計)。worktree `G:\home\mimageviewer-build817`、ブランチ `build-817-test-speed` (master `ca6686d93` から)。
以前の計測と経緯は [test-speed-report.md](test-speed-report.md)。

この文書の観測者は、とくに書かない限り ClaudeCode (このセッションの計測) である。
推定は「推定」と書き、根拠を添える。

## 1. 結論 (先に)

- **os error 1455 の主な原因は、本体の rlib (1.6 GB) を読む rustc が一度に多数起動すること**と見ている。
  全体テストのビルドでは、本体の rlib が終わった瞬間に、それに依存する rustc が 24 本同時に起動した。
  自分のビルドのコミットは 3 秒で 13 GB から 91 GB に、PC 全体では 209 GB に跳ねた (§2.2)。
  1 本あたり 3.5 GB をコミットするが、実際に触っている量 (作業セット) は 0.5 GB ほどしかない。
- **一番効く手は、結合テストのバイナリ 20 本を 1 本にまとめること**。山は約 91 GB から約 50 GB に下がり、
  リンクも 20 回から 1 回に減る見込み (§3.1)。ビルドの壁時計はほぼ変わらない。
  最長経路が別のところ (本体 lib のテスト用コンパイル) にあるため。
- **時間の大半は、テストの実行とビルドの最長経路**にある。
  - 本体 lib 全件の実行は、11-19 の master で 21.6 分 (19,160 件)。テスト時間の合計 30,676 秒を 24 並列で割った値 (21.3 分) とほぼ同じ。
    つまり、最後に残る長いテストを待っているのではなく、総量で決まっている。
    App を使うテスト 6,244 件 (中央値 1.34 秒) が総量の 59% を占める (§2.4)。
  - 全体テストのビルドの最長経路は、本体 lib のテスト用コンパイル (1 本の rustc) で、
    incremental の記録が無いときは 1,111 秒かかった (§2.3)。
- リリース前には、本体 lib のほぼ同じ約 19,000 件を 2 回流している。全体テストと、test-script 版の lib 全件 (約 22 分) である。
  2 回目を「test-script のときだけあるテスト」に絞れば、約 20 分縮む見込み (§3.5、利用者・統括の判断)。
- §1.814 (d)(e)(f) は原因を特定し、直した (§4)。(d) は製品の不具合 (EPUB の本の隣への保存のリネーム、`7588cfe36`)。
  (e)(f) はテスト側の問題 (`6e8c88b0e`)。
- 11-20 早朝の PC の停止の第一候補 (App のテストごとの AI 初期化の DLL 書き出し・読み込みと、TempDir の削除の重なり) に、
  テストの fixture 側で手当てした (`3e29c0db9`・`b4060ff60`、§6.2)。AI を使わない App のテストは、AI ランタイムの初期化を始めない。
  修正後の本体 lib 全件は 19,018 件すべて成功。テストの時間が縮んだとは言えない (修正前の記録の揺れの中)。
- この版でブランチに入れたのは上の 3 つ。結合テストのまとめ (§3.1) は入れていない (理由は §3.1)。

## 2. 計測

### 2.1 条件

- 2026-11-20 00:11〜00:31。Windows 11、論理 CPU 24、RAM 127 GiB。
  ページファイルは C: に 32 GiB、自動管理 (`AutomaticManagedPagefile=True`)。このときのコミット上限は 159 GB。
- コマンド: `cargo test --workspace --features pack-build-tools --no-run --timings`。test-full の 1 段目のビルドと同じ範囲。
  run-heavy のスロット経由で、ほかのセッションの重い処理 1 本と並行した。
- 直前に、同じ作業ツリーで依存のほとんどがビルド済みだった。turbojpeg-sys の MSBuild で失敗した 1 回目の残りで、
  既知の常駐ノードの問題 (`MSBUILDDISABLENODEREUSE=1` で解消) による。
  そのため、この計測は「依存は済み、本体は incremental の記録なし」の状態に近い。
- コミットの計測: 1〜3 秒おきに `Win32_PerfRawData_PerfOS_Memory` (CommittedBytes・CommitLimit・AvailableBytes) を記録した。
  あわせて、この作業ツリーの rustc・rust-lld・build script とその子プロセスの `PrivatePageCount` (コミット) と
  `WorkingSetSize` を記録した。大きいクレートの rustc は引数を `@argfile` で受けるので、そのファイルを読んでクレート名を取った。
  計測用のスクリプトはこのセッションの一時フォルダにあり、リポジトリには入れていない。

### 2.2 コミットの山

| 時刻 (開始から) | 自分のビルドのコミット | 作業セット | rustc の数 | PC 全体のコミット / 上限 | 主なもの |
| ---: | ---: | ---: | ---: | ---: | --- |
| 350 s | 20.5 GB | — | 2 | 137 / 159 GB | lib のテスト用 11.1 GB・rlib 用 9.4 GB |
| 368 s | 47.7 GB | 9.7 GB | 24 | 129 / 159 GB | rlib 完了、依存する rustc が起動 |
| 371 s | **91.1 GB** | 12.4 GB | 24 | **201 / 203 GB** | lib のテスト用 11.5 GB + 結合テスト・bin・example 各 3.5 GB |
| 373 s | 89.7 GB | 15.7 GB | 24 | **209 / 211 GB** | 同上 |
| 386 s | 36.1 GB | 41.0 GB | 24 + リンク 19 | 155 / 202 GB | リンクが重なる (リンク合計 12.3 GB) |
| 405 s 以降 | 12 GB 前後 | 9 GB 前後 | 1 | 128 / 165 GB | lib のテスト用だけが残る (〜1,181 s) |

- 上限が 159 GB から 211 GB に上がったのは、自動管理のページファイルが広がったため。11-19 の 1455 は、
  広がりが追いつかなかったか、広げられる限界 (C: の空き・自動管理の上限) に当たったものと推定する (11-19 の計測は無い)。
- **1 本 3.5 GB の出どころ (推定)**: 依存する rlib の大きさの合計とほぼ一致する。本体の rlib が 1,596 MB、
  `target\debug\deps` の rlib 全体が 606 個で 4.0 GB。一方で作業セットは小さい。
  rustc は上流の rlib を書き込み時コピーで mmap して metadata を読む、と推定している。Windows では、書き込み時コピーの
  mapping の大きさがそのままコミットに数えられる。11-19 のエラー行が「rlib を mmap する所」だったこととも合う。
  rustc のソースでは確かめていない。
- PC 全体のコミットは、ビルドが無いときでも 95〜105 GB あった。内訳はインストール版の mIV (10.9 GB)・ほかのセッションの rustc・
  Claude・Chrome など。ビルドに使える余地は、上限 159 GB のままなら 55〜65 GB しかない。
  run-heavy の 2 本目のビルドが同じ時刻に山を迎えると、これを超える。

### 2.3 ビルドの時間 (cargo --timings)

| 単位 | 開始 | 所要 |
| --- | ---: | ---: |
| mimageviewer lib (テスト用、`--test`) | 70 s | **1,111 s** (最長経路) |
| mimageviewer lib (rlib) | 70 s | 298 s |
| rlib に依存する 32 単位 (結合テスト 21・本体 bin とそのテスト用・pack 用 bin 4・example 1・musicvideo-editor 2 など) | 367 s | 各 13〜40 s、410 s までに全部終わる |
| 合計 | | 1,181 s |

- rlib に依存する単位は、最長経路の裏で並行して終わる。まとめても壁時計はほとんど縮まない。
  縮むのは、コミットの山・リンクの回数・その間の CPU の取り合い。
- `mimageviewer-core` の bin のテスト用 (`src/main.rs`、`#[test]` は 0 件) も 1 単位として作られている。

### 2.3.1 本体だけ変わった後のビルド (取り込み後の状態、2026-11-20 02:39〜02:42)

§1.814 (d)(e)(f) の変更 (本体の 5 ファイル) を入れた後の、同じ範囲のビルド。依存と incremental の記録はある状態。

| 項目 | 値 |
| --- | ---: |
| 合計 | **196 s** (34 単位) |
| lib のテスト用 (incremental) | 194 s (最長経路) |
| rlib | 67 s |
| 自分のビルドのコミットの山 | **84.3 GB** (69.6 s、rustc 24 本、作業セット 10.6 GB) |
| PC 全体のコミットの山 / 上限 | **150.9 / 159.3 GB** (71.2 s、残り 8 GB) |

- 取り込み後の全体テストのビルドでも、rlib が終わった直後に同じ山が出る。このときは自動管理のページファイルが広がる前で、上限の残りは 8 GB だった。
  11-19 の 1455 は、この状態で残りを使い切ったものと見ている (11-19 の計測は無いので推定)。

### 2.4 テストの実行時間 (既存の記録から)

`C:\home\mimageviewer\target\test-lib-full\runs\*\junit.xml` (統括の checkout の本体 lib 全件の記録、読み取りのみ) を集計した。

| 実行 | 件数 | 壁時計 | テスト時間の合計 | 60 秒超 |
| --- | ---: | ---: | ---: | ---: |
| 2026-11-19 (test-script 版) | 19,160 | 1,294 s | 30,676 s | 25 件 (合計 2,589 s) |
| 2026-11-17 | 18,688 | 1,208 s | 28,622 s | 26 件 |
| 2026-11-16 | 17,888 | 1,147 s | 27,116 s | 27 件 |

- 合計 ÷ 24 ≒ 壁時計なので、24 並列が最後まで埋まっている。上位の長いテスト (最長 294 秒) は壁時計より十分短い。
  長いテストを分けても壁時計は縮まない。縮めるには、テストの仕事の総量を減らすしかない。
- 分布 (11-19): p10 0.16 s、p50 0.39 s、p75 1.31 s、p90 3.18 s、p99 17.4 s。最短は 0.10〜0.12 s で、
  これが 1 テスト = 1 プロセスの起動の費用にあたる (1.7 GB のテスト実行体)。19,160 件 × 約 0.12 s ≒ 2,300 秒 (総量の約 7.5%) で、主因ではない。
- 最上位モジュール別: `app` 6,244 件・18,123 s (59%、中央値 1.34 s)、`ui_fullscreen` 1,558 件・4,592 s (15%)、
  `ui_dialogs` 751 件・1,264 s (4%)。App を作るテストの 1 件あたりの固定費が大きい見込み。内訳は未計測 (§3.4)。
- 件数は 1 か月で 17,888 → 19,160 (+7%) に増え、壁時計も同じ割合で伸びている。

## 3. 案と見込み

### 3.1 結合テストのバイナリをまとめる (コミットの山に一番効く。この版では入れていない)

- この版で入れなかった理由: 結合テストの名前と選び方 (`--test <名前>`、nextest の GPU の test-group、test-focused.ps1 の対応表、
  CLAUDE.md・開発文書の手順) が全部変わり、確かめに全体テスト (`test-full.ps1`、全件ロックを約 30 分取る) が要る。
  11-20 の PC の停止の後で全件ロックの順番待ちが詰まっていたので、停止の第一候補への手当て (§6.2) を先にした。
  次の版の候補として、統括の判断に回す。

- 対象: `tests/*.rs` 22 本のうち、スナップショットの 2 本 (`ui_snapshot`・`book_composition_editor_snapshots`) を除く 20 本。
  この 2 本を残すのは、`UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot` の手順が CLAUDE.md と ui-snapshot-policy に書かれているため。
  20 本を `tests/integration/main.rs` から各モジュールとして読む 1 本にする。
- 見込み: rlib に依存して同時に起動する rustc が 24 本 → 約 13 本。コミットの山は約 91 GB → 約 50 GB
  (11.5 GB + 3.5 GB × 11 前後)。リンクは 20 回 → 1 回 (結合テスト 1 本 80 MB 前後のリンクが 20 回減る)。
  ビルドの壁時計はほぼ同じ (§2.3)。
- 注意点:
  - nextest は 1 テスト 1 プロセスなので、プロセス共有の状態 (`DATA_DIR`・ロガー・環境変数) を使う 4 本
    (`archive_conversion_failure_diagnostics`・`fts_tantivy_integrity`・`susie_integration`・`ui_snapshot`) も、まとめた後に混ざらない。
    `cargo test` (スレッド並列) でまとめた全体を流すと混ざり得るので、開発中はモジュール名で絞る
    (`cargo nextest run -p mimageviewer --test integration -E 'test(/^susie_integration::/)'` のように) 手順を文書に書く。
  - `.config/nextest.toml` の GPU の test-group は `binary(/^(...|test_ai_upscale|test_tile_blend|...)$/)` で選んでいる。
    まとめた後は `test(/^test_ai_upscale::/)` などのモジュール名で選ぶ形に直す。
  - CLAUDE.md の `cargo test -p mimageviewer --test <integration-test-name>`、development-build-and-test、
    test-focused.ps1 の対応表、ほかの文書の `--test <名前>` を直す。
  - `tests/common` と `tests/support` は、各ファイルが `mod` で読んでいるので、1 本にまとめるとき二重に読まないよう整理する。
- あわせて: `[[bin]] mimageviewer-core` に `test = false` を付ける (main.rs に `#[test]` が無い)。依存する単位がもう 1 本減る。

### 3.2 同時に動く重い処理の数の決め方 (run-heavy、利用者・統括の判断)

- 今の run-heavy は同時 2 本。全体テストのビルドの山 (今は約 91 GB、§3.1 の後は約 50 GB) が、2 本目の山と重なると上限を超え得る。
- 案 A: 結合テストをまとめた後で、もう一度山を測ってから決める (推奨)。
- 案 B: 全体テスト・配布ビルド・本体 lib 全件を走らせている間は、2 本目のスロットを止める
  (test-lib-full のロックと同じ扱いに広げる)。待ちは増える。
- 案 C: `CARGO_BUILD_JOBS` を下げる。rlib に依存する rustc の数は jobs 以下になるが、
  最長経路の lib のテスト用のコンパイルの codegen も同じ jobserver の枠を使うので、ビルドが遅くなる。2026-11-12 にも、並列度を下げるだけでは足りなかった記録がある。

### 3.3 ページファイル (利用者の判断。エージェントは変えない)

- 自動管理のページファイルは、この計測では C: 上で上限 159 → 211 GB まで広がった。11-19 はこれが追いつかなかったと推定している。
- 選択肢: (a) 自動管理のまま (C: の空きに左右される)。(b) 固定の大きさにする (例: 初期 64 GB・最大 96 GB)。
  上限が最初から 190 GB 前後になり、広がるのを待たない。(c) C: 以外のドライブに置く。
  C: の読み書きの混み (11-15 の固まり) を避けられる。G: は SSD で空きが十分か、利用者が確かめる必要がある。
- 費用: (b)(c) はディスクを常に確保する。Windows の設定は再起動が要る。

### 3.4 テストの実行の総量を減らす (調査が要る、次の版以降)

- 2026-11-20 夜に計測した。結果と案は §9。

- App を作るテストの 1 件あたりの固定費を測る。`AppTestConfig::new_for_test` が作るもの (設定の読み込み・DB・フォント・egui の Context・
  RAW 用の executor など) の内訳を、1 件の空のテストで取る。1 件 0.5 秒縮めば 6,244 件 × 0.5 s ÷ 24 ≒ 2 分の短縮。
- 組み合わせのテスト (例: `history_parent_1473` は 1 関数で 2 × 4 × 5 = 40 通りを、毎回新しい App で回して 60〜153 秒) は、
  分けても総量は変わらないので、時間のためだけには分けない。
- nextest の並列度 (24) が最適かは未確認。テストの多くが内部でスレッドを使うので、全件中の CPU 使用率を測ってから判断する。

### 3.5 配布ビルドの全体 (約 1 時間 50 分) の内訳 (未計測)

- 内訳は全体テスト (27〜30 分) と release ビルドの 2 系統 (`target` と `target-portable`、本体は各 thin LTO)・Inno・署名。
  段ごとの時刻は残っていなかった。11-19 の配布ビルドのコンソールの時刻を統括が持っていれば、それで埋める。
  無ければ、再開後に build-release の本体を 1 回計測する。
- リリースでは、本体 lib の 19,000 件前後を 2 回流している。1 回目は全体テスト (`--features pack-build-tools`、build-dist の中)。
  2 回目はリリース手順 9.8 の `test-lib-full.ps1 -Features test-script` (約 22 分、§1.392 の経緯で足した段)。
  `test-script` feature は 401 か所の `cfg` で、テストと eframe の witness を足す。
  案: test-script 版では、両方のビルドの `cargo nextest list` の差 (test-script のときだけある名前) だけを流す。
  見込みは約 20 分の短縮。
  費用・注意: 同じ名前のテストが feature で別の経路を通る場合は、2 回目で流さなくなる。
  `cfg(feature = "test-script")` が製品の共有コードの挙動を変える箇所を先に洗い出し、無ければ採る。利用者・統括の判断。

## 4. §1.814 (d)(e)(f)

### (d) EPUB の本の隣への保存 — 製品の不具合

- 場所: `src/epub_cache.rs` の Windows 版 `publish_sibling_file`。`FILE_RENAME_INFO` の `FileName` に保存先を `FileNameLength` 分だけ写し、
  NUL を置いていない。バッファは `u64` 単位で 0 埋めしているので、`20 + 2 × 名前の長さ` が 8 の倍数のときだけ、名前の直後に NUL が無くなる。
- 実験 (ClaudeCode、PowerShell から同じ API を呼んだ): `SetFileInformationByHandle(FileRenameInfo)` は `FileNameLength` で止まらず、NUL まで読む。
  名前の直後が `XYZ\0` なら、`MyNovel-validtail.pdfXYZ` にリネームされた。`*?<` ならエラー 123 で失敗した。
  §1.814 (d) の失敗 (`Io: ファイル名、ディレクトリ名、またはボリューム ラベルの構文が間違っています。 (0x8007007B)`) と一致する。
- ときどきしか起きない理由: 保存先のパスの長さ (一時フォルダの名前の pid の桁数など) で NUL の有無が変わり、
  NUL が無いときは、バッファの後ろのヒープの中身で結果が変わる。
- 製品で起こり得ること (コードと実験から。実機での観測は無い): 保存先のパスの長さによって、保存に失敗するか、
  まれに名前の後ろに余計な文字が付いたファイルができる。v4.10.0 (`4345f8597`) から。
- 同じ組み立ての列挙: 名前を渡す `SetFileInformationByHandle` は、この関数と `src/rename_key_migration.rs` の 2 か所だけ。後者は NUL を含む。
  ほかの呼び出し (`book_fs_journal.rs`・`image_folder_zip.rs`・`epub_cache.rs` の削除系) は `FileDispositionInfo(Ex)` で、名前を持たない。
- 直し方: `rename_key_migration.rs` と同じ形 (NUL を足し、その NUL を含む大きさで渡す)。統括の了承済み (2026-11-20)。
- 実装 (`7588cfe36`、実装 = Codex): バッファの組み立てを `sibling_rename_info` に分け、名前が空・NUL を含む場合は拒否する。
  テスト: 純粋なテスト (名前の長さの 4 の剰余を全部含め、渡す大きさの内側で名前の直後が 0) と、実際のリネーム (剰余を全部含む保存先で、
  できたファイルの名前が期待どおり) の 2 本。
- 破壊検証 (ClaudeCode が流した): NUL を足す行を外すと、純粋なテストが長さ 2 で落ちた。

### (e) `archive_conversion_failure_diagnostics` — テスト側

- ログは `log_writer` の非同期の書き込みスレッドが書き、ファイルとフォルダもそのスレッドが作る。テストは変換の直後に、書き終わりを待たずに
  `logs_dir()/archive-conversion-failure-test.log` を読む。負荷で書き込みスレッドが遅れると、フォルダがまだ無く NotFound (os error 3) になる。
- 直し方: ログを読む前に書き込みスレッドを止めて書き切らせ、その結果を assert する。結合テストからは今の `finish_logs()` (期限 2 秒固定・結果を返さない) しか見えない。
  そこで `finish_logs_until` を bool を返す公開関数にする (製品の呼び出し元の挙動は変えない)。
- 実装 (`6e8c88b0e`): テストは 3 回の変換を全部終えてから `finish_logs_until(20 秒後)` を assert し、その後でログを読む。

### (f) `previous_run_diagnostics::...diagnostic_device_latch_is_saved_by_normal_maintenance...` — テスト側

- 書き込みスレッドは、device の観測を追記した後で、tempfile の persist (`MoveFileExW(REPLACE_EXISTING)`) で `state.json` を置き換える。
  テストは、観測が見えた直後に `state.json` を開いて読む。
- Windows では、置き換え先を誰かが開いていると、共有モードに DELETE があっても `MoveFileExW` が ERROR_ACCESS_DENIED (5) になる
  (ClaudeCode の実験で確認)。書き込みスレッドは `replace(...)?` で終わり、`completion = 2` になる。`finish_until` は待たずに false を返す
  (失敗が 0.44 秒だったことと合う)。
- 製品は、この置き換えの失敗を「ログ 1 回・再試行なし」で割り切る設計なので変えない。テストが、置き換えと同時に `state.json` を開かないようにする。
  方法: device の観測を受けた保存が済んだことを示す `#[cfg(test)]` の数えを `Shared` に足し、それを待ってから読む。
  その後、書き込みスレッドはファイルに触らず park する (`Schedule::due`: boundary は変わらず、active が 0)。
- 実装 (`6e8c88b0e`): `Shared` の `#[cfg(test)] device_maintenance_saves` を、device の観測を受けた RUNNING の分岐で置き換えが成功した後に数える。
  テストはそれが 1 以上になるのを待ってから `state.json` を読む。

### (d)(e)(f) の確かめ (ClaudeCode が流した)

- 関連のテストと、3 件それぞれの 20 回の stress が成功。
- 破壊検証 A (修正を外し、窓を広げる): (d) は長さ 2 で落ち、(e) は NotFound、(f) は `finish_until` の assert で落ちた。
  窓の広げ方: ログの書き込みスレッドの `LogFile::open` の前に 1.5 秒、`state.json` の置き換えの前に 0.3 秒止め、テスト側で `state.json` を 0.8 秒開いたままにした。
- 破壊検証 B (修正を戻し、窓は広げたまま): 3 件とも通った。破壊検証の改変は、コミットの前に全部外した。

## 5. 負荷のときだけ落ちるテストを 1 つの節にまとめる案

対象: §1.814・§1.765・§1.455・§1.449・§1.447・§1.416・§1.389・§1.334・§1.321・§1.294。番号は振り直さない。
新しい節 (番号は統括が取る) に台帳を置き、各節は台帳への参照を 1 行足すだけにする。

台帳の列: テスト名 / 出た日とソース / 型 / 状態 / 元の節。型は次の 6 つに分ける。直し方の方針が型で決まるため。

| 型 | 中身 | 例 | 直し方の方針 |
| --- | --- | --- | --- |
| A. 非同期の完了を実時間の上限で待つ | `wait_until` 10 秒・`yield_now` 回数など | §1.765 (sidecar)・§1.455 (音声ワーカー)・§1.389 の多く・§1.294 (LUT)・§1.321 (修正済み) | 完了の通知・同期実行を待つ形にする。上限は延ばさない |
| B. 書き込み・初期化の完了を待たずに読む / 同時に開く | 非同期の書き込み、schema の作成の順序、置き換え中のファイル | §1.814 (e)(f)・§1.416 | 完了の境界 (barrier・数え) を待ってから読む |
| C. 実時刻・実機の入力に依存 | GetAsyncKeyState、実時計のめくり | §1.814 (a) (修正済み)・§1.765 (page_turn の ordinal)・§1.447 | 偽の入力・テスト用の時計に差し替える |
| D. プロセスが終わらない | テスト成功後の終了待ち | §1.455 の 1 件目 | 終了時に残るスレッド・join を調べる |
| E. プロセス共有の状態を並行するテストと共有 | 共有キャッシュ・連番 | §1.334・§1.294 の 1 件目 (修正済み) | 2026-10-16 から本体 lib 全件は nextest (1 テスト 1 プロセス) なので、ゲートでは起きなくなった可能性が高い。開いている項目は、nextest での再発の有無で閉じるか決める |
| F. 製品の不具合 | — | §1.814 (d) | 製品を直す (統括経由で相談) |

- 新しい失敗が出たら、まず台帳に 1 行足して型を付ける。同じ型の 2 件目からは、その型の直し方を横断で当てる。
- 全件で落ちたテストを単独で流し直して切り分ける運用 (今の §1.765 の方針) は、台帳の前提として残す。

## 6. 付記: 固まった件 (11-20 早朝) との関係

- 統括の共有によると、§1.816 の本体 lib 全件の最中に PC が固まった。live kernel dump の障害バケットは `NtfsAcquireSharedVcb`
  (ドライブ全体のロック待ち) で、nextest のログは `app::tests::history_parent_1473` の群が SLOW のところで止まっていた。
- この群のうち `section1473_other_synthetic_roots_parent_inventory` は、本物のドライブ一覧 (`enter_drive_list_from_navigation(None)`) に入る
  (`src/app/tests/history_parent_1473.rs` 241 行)。11-19 の全件の記録では、この関数が 153 秒、同じ群のほかの 6 件が 60〜93 秒かかっている。
- §2.2 のコミットの山は、ドライブのロック待ちとは別の現象に見える。判断は統括のダンプの解析待ち。
  テストが本物のドライブ一覧を読むこと自体は、決定性の点で §5 の型 C に近い (実機の状態に依存する)。

### 6.1 ダンプの解析 (統括) を受けたコードの洗い出し (2026-11-20)

- 統括の解析: 本体 lib のテスト実行体 (pid 0x9a0c) の中の 2 本の std::thread が、同じファイルで止まっていた。
  - B: user mode からの `NtCreateSection(SEC_IMAGE = 0x01000000)`。既存の data section を NTFS が閉じる途中で、本体の ERESOURCE を待つ。
  - A: 同じファイルのハンドルの `NtClose` (IRP_MJ_CLEANUP)。本体の ERESOURCE を独占したまま、ページングの ERESOURCE を待つ。
- 第一候補 (コードで確認。再現・実機の観測は無い):
  - B: App の AI ランタイムの初期化スレッド `ai-runtime-init` (`src/ai/runtime.rs` 112〜137 行)。`data_dir::get()` (テストでは一時フォルダ) に
    `onnxruntime.dll` (17 MB) を書き出し (`extract_embedded_file`: .tmp に書いて rename、SHA-256)、すぐ `ort::init_from` (LoadLibraryExW) で読み込む (同 457〜468 行)。
  - A: テストのスレッドの後片付け。`AppTestEnv` (`src/app/tests.rs` 15355 行付近) は App の後に TempDir を消す (`remove_dir_all`)。
    AI の初期化スレッドの終わりは待たない (`runtime.rs` 1095〜1120 行のテストが、owner の drop が join しないことを確かめている)。
  - 起点: App の最初の full フレーム (`initialized == false`) の `ensure_ai_runtime()` (`src/app/update_phases.rs` 530 行)。AI を使わないテストでも始まる。
- 第二候補 (確度は低め): テスト用の PDF 経路 `remote_pdf_test_run` (`src/pdf_loader.rs` 940〜951 行)。操作のたびに vendor の `pdfium.dll` を bind・解放する。
  pdfium-render の `Library::new` はグローバルの mutex の外なので、複数の worker スレッドから同時に読み込み得る。
  - 観察のみ (2026-11-20 追記、直さない): 読み込むのは `vendor/pdfium/bin/pdfium.dll` だが、テストのプロセスの中でこのファイルを
    データとして開く経路は見つからなかった (`PDFIUM_DLL_BYTES` は `include_bytes!` でコンパイル時に埋め込む)。
    `PdfWorkerPool::start` (同 3866 行付近) は `data_dir` へ `pdfium.dll` を書き出すが、読み込むのは子プロセス (別の pid) なので、
    ダンプの「同じプロセスの 2 本のスレッド」とは合わない。同じ DLL を複数のスレッドから同時に読み込むのは image section の
    作成どうしであって、data section を閉じる途中との重なりではない。よって、第二候補の確度は低いと見る。
  - 懸念として残すこと: `extract_embedded_file` (`src/data_dir.rs` 428 行) は、既存のファイルがあるとその内容 (またはハッシュの
    sidecar) を読んで比べる。同じ data_dir で App を 2 つ作るテストでは、片方の AI 初期化の読み取り・書き出し (データ) と、
    もう片方の `ort::init_from` (読み込み) が同じ `onnxruntime.dll` で重なり得る。これもダンプの形に合う。
    AI の fixture の手当て (既定で始めない) を入れれば、この形もなくなる。
- 調べて外したもの: 製品の PDFium (読み込むのは子プロセス)、TensorRT pack (テストのデータに無い)、CreateProcess だけの経路
  (VST3 host・Susie・VLM・各子プロセス。section は kernel mode で作られる)、FFmpeg (起動時の静的 import)、
  システムの DLL (`nvcuda.dll`・`vulkan-1.dll`)、WIC・シェルのサムネイル (アプリが書かない DLL)、memmap2 (データのファイル)。
- 直し方の案 (テスト側、統括へ相談中): App のテストの fixture では AI ランタイムの初期化を既定で始めない。AI を使うテストだけが明示的に始め、片付けの前に終わりを待つ。
  副次効果として、AI を使わない App のテストごとの 17 MB の書き出し・SHA-256・ORT の読み込みが無くなる。時間の短縮は、§6.2 の比較では見えなかった。

### 6.2 App のテストの fixture で AI ランタイムを既定で始めない (`3e29c0db9`)

統括の了承 (2026-11-20) を受けた手当て。実装 = Codex (gpt-6.1-sol / high)、コンパイル・テスト・破壊検証 = ClaudeCode。

- 方針: App のテストの fixture では AI ランタイムの初期化を既定で始めない。AI を使うテストだけが明示的に始め、片付けの前に初期化の終わりを待つ。
  止め方は fixture の型 (`AiRuntimeFixture`) で選ぶ。製品のコードには、環境変数や「テスト中なら」の分岐を足していない。
- 形:
  - `AppTestConfig.ai_runtime` は必須で、既定値を持たない。`NotStarted` は、App を作った直後に同じ owner を Dormant から Failed にする
    (`AiRuntimeInitOwner::fail_dormant_for_test`、cfg(test))。owner の Arc と、構築時に設定した TensorRT の lifecycle はそのまま。
    メッセージは「App test fixture does not start the AI runtime; use AiRuntimeFixture::Start」で、AI を使うテストが落ちたときに直し方が分かる。
  - `Start` は今までどおり、最初の Full frame から始まる。片付けは App → 初期化の終わりの待ち (Initializing のときだけ) →
    epoch / override → TempDir → lock の順。待ちに期限は置かない。初期化が止まったままなら、nextest のプロセス単位の timeout
    (60 秒 × 5) がプロセスごと打ち切るので、初期化中の TempDir は消されない (独立レビューの P2 を受けて、60 秒で panic する形から改めた)。
  - テストの中で App を直接作っていた 17 か所 (`src/app/tests.rs` 4・`startup_restore.rs` 9・`collection_navigation.rs` 1・`preferences.rs` 3) を、
    方針を受ける helper 経由にした。設定を読み直して App を作り直すときは、片付けの待ちの対象を新しい App の owner に移す。
  - 取りこぼしの検査: `src/` と `tests/` のソースを走査し、helper 以外から App の製品の constructor を呼んでいないことを確かめるテスト
    (raw TextEdit を禁止しているテストと同じ流儀。例外は path・目印・件数で固定し、古い例外も失敗にする)。
    constructor の呼び出しに加えて、関数としての参照・`Self::new_from_settings*`・UFCS・型別名と import の別名も拾い、走査器そのものも
    見本の入力で検査する (独立レビューの P3)。マクロ経由・型推論による `Default`・別 crate からの呼び出しは拾えない (補助の監査)。
- `Start` が要るテスト: 既存のテストでは `app::tests::ai_crash_655::ai_crash_655_headless_zip_navigation` (ignored の実推論の harness) の 1 件だけ
  (Codex のコード読み。修正後の全件で、`NotStarted` のままで落ちたテストは無かった。ignored の harness は全件では流れない)。ほかの AI 関連のテストは、偽の owner (`ready_for_test` / `failed_for_test`)・結果の channel に直接入れる形で、
  Ready の runtime を要らない。
- 新しいテスト 8 件 (`app::tests::ai_runtime_fixture`): `NotStarted` の App で最初の Full frame を回しても `onnxruntime.dll`・
  `onnxruntime_providers_shared.dll` が data_dir に作られず、初期化も始まらない (統括の検証の 2)、片付けが初期化の終わりを待ってから
  TempDir を消す (AppTestEnv・CaseEnv・App の置き換えの 2 経路)、ソース走査とその自己検査。
- 破壊検証 (ClaudeCode が流した、P2 の対応の前): `NotStarted` の切り替えを外すと 2 件、片付けの待ちを外すと 5 件が落ちた
  (5 件のうち 1 件は、P2 の対応で外した期限切れのテスト)。
  1 回目は、片付けの待ちを確かめる 3 件が待ちを外しても通った (drop の前に initializer を解放していたので、worker が先に書き終えれば通る)。
  片付けの待ちの中から初めて解放する形に直し、2 回目で 5 件とも落ちることを確かめた。
- 独立レビュー (Codex gpt-6.1-sol / xhigh、ブランチの 3 コミット全体): 1 回目 REJECT (P2: 期限切れの panic の後に TempDir が消える。
  P3: ソース走査が間接の呼び出しを拾わない)。両方を直し (`b4060ff60`)、2 回目 ACCEPT。(d)(e)(f) には指摘なし
  (同じ形の経路・製品の終了順・数えの位置を確かめた、との回答)。
- 時間の比較の基準 (修正前、既存の記録。統括の指示で、修正前のソースに戻した全件は流していない):

  | 実行 | ブランチ | 件数 | 壁時計 | テスト時間の合計 | `app::` の件数 / 合計 |
  | --- | --- | ---: | ---: | ---: | ---: |
  | 11-20 04:02 | deps-816 | 19,095 | 1,820 s | 21,830 s | 6,222 / 13,569 s |
  | 11-20 06:33 | mcp-740 | 19,120 | 1,645 s | 19,733 s | 6,222 / 11,791 s |

  件数は junit の testcase の数 (skipped を含む)。どちらも 12 並列 (run-heavy の `NEXTEST_TEST_THREADS=12`)。
  ブランチが違うので、件数と中身はこのブランチと少し違う。
- 修正後の本体 lib 全件 (検証を兼ねて 1 回、ClaudeCode が流した。`b4060ff60`、11-20 10:50〜11:21、12 並列):
  **19,018 件すべて成功** (88 件 skip)。壁時計 1,788 s、テスト時間の合計 21,445 s、`app::` 6,230 件 / 12,878 s。

  | 両方にあるテストだけで比べる | 件数 | deps-816 → 修正後 | mcp-740 → 修正後 |
  | --- | ---: | ---: | ---: |
  | 全体の合計 | 19,006〜19,007 | 21,830 → 21,435 s (-1.8%) | 19,675 → 21,435 s (+8.9%) |
  | `app::` の合計 | 6,210 | 13,569 → 12,869 s (-5.2%) | 11,791 → 12,869 s (+9.1%) |
  | `app::` の中央値 | 6,210 | 0.910 → 0.934 s | 0.884 → 0.934 s |

- 読み方: 修正前の 2 回のあいだでも合計が 10% 動いており、差はその揺れの中に収まる。**この変更で時間が縮んだとは言えない**。
  AI の初期化は App の別スレッドで動き、テストはその終わりを待っていなかったので、もともとテストの時間にはほとんど乗っていなかったと見る。
  この変更の効き目は時間ではなく、AI を使わない App のテストごとの DLL の書き出し・読み込みと TempDir の削除の重なりを無くすこと
  (11-20 の停止の第一候補) にある。停止が再び起きないかは、今後の全件で見るしかない (この 1 回では停止は起きなかった。ClaudeCode の観測)。

## 7. 残りの計測

1. 済み: 本体を変えた後の incremental の全体テストのビルド (§2.3.1)。
2. 未: §3.1 を入れた後の、同じビルドの山 (§3.1 を入れていないため)。
3. 未: 本体 lib 全件の実行中の CPU 使用率 (並列度の判断のため)。11-20 から 12 並列に下げたので、測るなら 12 並列で取り直す。
4. 未: build-release の本体の時間とコミット (§3.5)。
- 11-20 の PC の停止の後は、全件ロックの順番待ちを増やさないことを優先し、2〜4 は次の版以降に回した。

## 8. 更新履歴の案 (統括の確認用)

- 製品に効くのは (d) だけ。ほかはテストと開発手順の変更なので、更新履歴には書かない。
- (d): 「EPUB を本の隣に PDF として保存するとき、保存先のパスの長さによって、保存に失敗したりファイル名の後ろに余計な文字が付いたりすることがある問題を修正しました。」
  (観測は負荷のときのテストの失敗だけで、利用者の実機での報告は無い。v4.10.0 から。)

## 9. App を作るテストの 1 件あたりの固定費 (§3.4 の計測、2026-11-20 夜)

統括の依頼 (利用者の判断「固定費の計測を今進める」)。計測だけで、製品の挙動は変えていない。
ブランチ `measure-817-app-cost` (master `39e5e2c6e` から、`35baa1cf6` を含むことを確認、`setup-test-dlls.ps1` 済み)。
計測用の probe (`8432031ff`) はこのブランチだけに置き、master には入れない。観測者は ClaudeCode (run-heavy 経由、全件ロックは取っていない)。

### 9.1 方法

- `app::tests::fixture_cost_817` に、同じ形のテストを 48 件ずつ置いた。
  - noop: 何もしない (プロセス起動の費用)。
  - empty: 共通 fixture と同じ手順 (lock → TempDir → override・epoch → `App::new_for_test` → drop → TempDir の削除) を段ごとに時刻を取る。
  - frame: empty に、最初の Full frame を 1 回足す。
  - reopen: 新しい data_dir で App を作って drop → その data_dir の DB だけを別のフォルダへ複写 → 複写先で App を作る (雛形の案の見込み)。
- それぞれを直列 (`--test-threads 1`) と 12 並列で流した。12 並列は「同じ種類のテストが 12 本同時」で、全件の混ざり方とは違う。
- constructor の内訳は、製品に既にある perf の startup の計測 (`db_open_*`) をテストの中で有効にして取った (直列 8 件)。
- App の drop は、`Drop::drop` の本体の終わりに cfg(test) の時刻を置き、本体とフィールドの片付けに分けた。

### 9.2 内訳 (中央値)

| 段 | 直列 | 12 並列 |
| --- | ---: | ---: |
| プロセス起動 (noop の 1 件の時間) | 82 ms | 86 ms |
| 空の App テスト 1 件 (nextest の 1 件の時間) | 378 ms | 902 ms |
| ├ lock・TempDir の作成・override・epoch | 1 ms 未満 | 1 ms 未満 |
| ├ **App の constructor** | **271 ms** | **704 ms** |
| ├ App の drop: `Drop::drop` の本体 | 0.1 ms | 0.1 ms 未満 |
| ├ App の drop: フィールドの片付け (worker の停止など) | 15 ms | 28 ms |
| └ TempDir の削除 | 2 ms | 4 ms |
| 最初の Full frame を 1 回 | +156 ms | +162 ms |

- constructor の内訳 (直列): 約 26 個の SQLite DB を新しく作って開くのが合計約 230 ms (85%)。大きいもの: `db_open_rating` (rating と tags) 41 ms、
  `db_open_video_tile_cache` 34 ms、`db_open_spread` 23 ms、`db_open_adjustment` 14 ms、`db_open_book_resume` 13 ms、`db_open_video_chapter_thumbs` 12 ms。
- constructor の CPU 時間は直列 125 ms・12 並列 133 ms で、ほとんど増えない。壁時計が 271 → 704 ms に伸びるのは、DB の新規作成のファイル I/O の待ち (作成・同期) と見る。
- 1 件の時間のうち、テストの中で測った時間の外 (プロセスの起動と終了、テストの枠組み) は約 90 ms。

### 9.3 雛形の data_dir を複写する案の見込み (reopen の計測)

雛形には DB のファイル (26 個、計約 0.5 MB) だけを入れ、`settings.db` (と -wal・-shm)・`logs\`・`video_upscale_tasks.json` は入れない
(設定と状態を持ち込まないため)。

| | 直列 | 12 並列 |
| --- | ---: | ---: |
| 新しい data_dir で constructor | 283 ms | 760 ms |
| 雛形の複写 (26 ファイル) | 11 ms | 32 ms |
| 複写した data_dir で constructor | 41 ms | 86 ms |
| drop (新規 → 複写) | 17 → 4 ms | 41 → 8 ms |
| **1 件あたりの差** | **約 245 ms** | **約 675 ms** |

- 雛形の中身 (sqlite3 で全テーブルを数えた): 行があるのは `folder_thumb_pin_revision` 1 行と `spread_meta` 1 行だけ (どちらも schema の管理用)。
  data_dir のパスなど、テストごとに違う値は入っていなかった。
- 全件での見込み: master の今日の test-full の junit で、本体 lib のテストの時間は二山に分かれ、0.7 秒以上が 7,738 件 (App を作るテストの数の目安)。
  7,700 件 × 約 0.65 秒 ≒ 5,000 秒のテスト時間 → 12 並列の壁時計で **約 7 分**の短縮 (本体 lib 全件 約 30 分の 2 割強)。
  リリースでは本体 lib を 2 回流すので約 14 分。App を 2 回作るテスト (再起動のテスト) はさらに縮む。
  見込みの幅: 12 並列の計測は App のテストだけが同時に動く条件なので、全件の混ざり方では差が小さくなり得る。
- 副次: App のテストごとの DB の新規作成 (ファイル 26 個の作成と同期) が無くなり、全件で約 20 万回のファイル作成が減る。
  ディスクの負荷 (11-20 の停止はファイルシステムの中の待ちだった) にも効く方向だが、停止との関係は実証していない。

### 9.4 雛形の案の形 (統括の 2 点への回答)

**雛形が古くならない仕組み**

- 雛形は、そのテストの実行体自身が、今の constructor の経路で作る。手で作った雛形やリポジトリに置いた雛形は使わない。
- 鍵: テストの実行体の path・大きさ・最終更新時刻 + fixture の条件 (similar の capability・portable の配置など、DB の作り方を変え得るもの) + 雛形の形式の版。
  cargo はソースが変われば必ずリンクし直すので、同じ鍵なら同じソース・同じ schema のコードで作ったものになる。実行体 (1.7 GB) のハッシュは遅いので取らない。
- ビルドごとに 1 回作る: 雛形が無いプロセスは、自分の一時フォルダで今の経路のまま App を作って drop し、DB だけを `target\<profile>\app-test-template\<鍵>.tmp-<pid>` に置いてから
  `<鍵>` へ rename する (先に置いた方が勝ち、負けた方は捨てる)。12 本が同時に始まると最初だけ数本が重複して作るが、結果は同じ。
- 古い鍵の雛形は、新しい雛形を置くときに消す (消せなくても次の鍵には影響しない。まれな失敗は割り切り)。
- 複写に失敗したら、そのテストを失敗にする (今の経路へ黙って戻さない)。

**新規作成・移行の経路の検査が残ること**

- 雛形を使うのは、共通の入口 (`setup_app()`・`setup_app_for_test()` など) で、data_dir を前もって用意しない (`prepare_data_dir` が空) テストだけ。
- 今の経路のまま残すもの:
  1. `prepare_data_dir` で古い形式の DB やファイルを置くテスト (移行のテスト)。雛形と混ぜると意味が変わるので、自動で今の経路にする。
  2. App の起動で DB ができること・移行すること・初回の起動の挙動を確かめるテスト。fixture の型 (`AiRuntimeFixture` と同じ流儀の、既定値を持たない
     `AppDataDirFixture::{Template, Fresh}` など) で `Fresh` を明示する。洗い出しは、data_dir の中のファイルや `user_version` を見るテストの grep と、
     雛形を入れた後の全件で落ちたテストの切り分けで行う。
  3. 各 DB のモジュールの単体テスト (`open`・移行を直接呼ぶもの)。App の fixture を通らないので、変わらない。
- 雛形の検査: 雛形で作った App と、今の経路で作った App の data_dir を比べ、各 DB の `sqlite_master` (表・索引の定義) と `user_version` が同じことを確かめるテストを 1 本置く。
  雛形の作り方がずれたら、ここで決定的に落ちる。

### 9.5 候補ごとのまとめ

| 候補 | 12 並列の壁時計の短縮 (本体 lib 全件) | 退行のリスク | v5.5.0 |
| --- | ---: | --- | --- |
| 雛形の data_dir の複写 (テストの fixture だけ、製品は変えない) | 約 7 分 (リリースでは約 14 分) | 中〜低。初回起動・移行のテストを雛形側に入れてしまうと、その経路の検査が抜ける (上の分け方と schema の比較で抑える) | 入れられる。実装 (Codex)・本体 lib 全件 1 回・独立レビューが要る。全件ロックが空けば 11-21 中に終わる見込み |
| 最初の Full frame (156 ms) | 未計測 (frame を回すテストの数と内訳が要る) | — | 次の版以降 |
| App の drop のフィールドの片付け (15〜41 ms) | 1 分未満 (雛形で 4〜8 ms に下がる分を含む) | — | 雛形の案に含まれる |
| プロセス起動 (82〜86 ms × 約 19,000 件) | 削れない (nextest の 1 テスト 1 プロセスの前提) | — | — |
| DB を遅延して開く (製品の constructor を変える) | 雛形と同程度 | 高い (製品の起動の順番が変わる) | 入れない |

## 10. 雛形の data_dir: 実装の設計メモ (2026-11-20 夜)

統括の依頼 (利用者の判断「実装し、取り込みは公開後 (推奨)」)。v5.5.0 の公開までは master に入れない。
ブランチ `app-template-817` (master `9d0d84797` から。計測用の probe `8432031ff` は含めない)。
§9.4 の 2 点 (古くならない仕組み・新規作成と移行の経路の検査) をここで具体化する。
初版 (`f23dc4a02`) は実装前の独立レビュー (GPT-6.1 Sol / xhigh) で REJECT になり、指摘を受けて下の形に改めた (10.9)。

### 10.1 範囲

- 変えるのはテストの fixture だけ (`src/app/tests.rs` の `AppTestConfig`・`App::new_for_test`・`phase_c_support`、
  `src/app/tests/history_parent_1473.rs`、新しいテストのモジュール)。製品のコードには、環境変数・「テスト中なら」の分岐を足さない。
- App の constructor の経路そのもの (`new_from_settings_with_load_meta_book_query_repaint_and_similar_capability`) は変えない。
  雛形を使う場合も、使わない場合も、同じ constructor を通る。違いは「constructor を呼ぶ前に data_dir に空の DB が置いてあるか」だけ。

### 10.2 型: どちらを使うかを fixture ごとに明示する

- `AppDataDirFixture { Template, Fresh }` を `AppTestConfig` の必須のフィールドにする。既定値は持たせない
  (`AiRuntimeFixture` と同じ流儀)。
- `Template`: constructor を呼ぶ前に、雛形の DB を data_dir へ複写する。`Fresh`: 今と同じ (空の data_dir で constructor を呼ぶ)。
- 入口ごとの既定:
  - `setup_app()`・`setup_app_with_ai_runtime()`・`setup_product_similar_app()`・`setup_app_with_similar_capability()` → `Template`。
  - `setup_paused_similar_app_with_fixture(settings, prepare_data_dir)` (data_dir を前もって用意する入口) → `Fresh`。
  - `history_parent_1473::setup_with_ai_runtime` → `Template`。
  - 初回の起動・DB の新規作成・起動時の移行を確かめるテスト用に、`Fresh` を明示する入口 (`setup_app_with_fresh_data_dir()` など) を足す。
- `Template` を指定したのに、constructor を呼ぶ時点の data_dir が空でない (`new_for_test` が書く `settings.json` を除く) 場合は、
  panic でテストを失敗にする (雛形と前もって置いたファイルを混ぜない。黙って `Fresh` へ切り替えない)。
- 再読み込み (`replace_app_from_settings`・`app_from_settings_after_drop`) は同じ data_dir で App を作り直すだけなので、変えない。

### 10.3 雛形の作り方: 同じ実行体の子プロセスで作る

- 同じプロセスの中で雛形用の App を先に作ると、process-global の状態が雛形の作業フォルダに固定される
  (例: `src/ai/model_manager.rs` の `MODELS_DIR` の `OnceLock` は、最初の `ModelManager::new()` で `<data_dir>\models` に決まり、
  `OverrideGuard` では戻らない)。そのため、雛形の有無で本来のテストの状態が変わる。これを避けるため、雛形は子プロセスで作る。
- 手順 (雛形が無いときだけ):
  1. 親 (App の fixture) が `<置き場所>\<鍵>.tmp-<pid>\` を作り、その下の `data\` を作業用の data_dir にする。
  2. 親は同じテストの実行体 (`std::env::current_exe()`) を子として起動し、`#[ignore]` を付けた雛形作成用のテスト 1 本だけを
     `--exact --ignored` で走らせる。作業用の data_dir と capability は、テストのコードだけが読む環境変数で渡す。
  3. 子は、fixture の `Fresh` と同じ手順 (lock・override・epoch の scope・`AiRuntimeFixture::NotStarted`・settings は `None`) で
     App を作って drop し、最後に作業フォルダへ完了の印 (`builder-done`) を書いて終了する。
     プロセスが終わるので、drop の後も残る worker や接続もすべて閉じる。
     雛形作成用のテストの完全な名前は 1 か所の定数にし、親はその定数で子を起動する。
  4. 親は、子の終了コードが 0 で、完了の印が在り、複写する DB が 1 個以上あることを確かめる (どれかが欠ければ panic)。
     libtest は exact の名前が一致しないと 0 件の実行で正常終了するので、終了コードだけでは雛形作成用のテストが走ったことにならない。
     作業用の data_dir の直下の `*.db` (`settings.db` を除く) を
     1 つずつ rusqlite で開き、`PRAGMA wal_checkpoint(TRUNCATE)` をしてから閉じ、`-wal` が残っていない (または 0 バイト) ことを確かめてから、
     `<鍵>.tmp-<pid>\` の直下へ複写する。複写した DB の名前の一覧を `manifest.txt` に書く。作業用の data_dir は消す。
  5. `<鍵>.tmp-<pid>` を `<鍵>` へ rename する。先に置いたほうが勝ち、負けたほうは自分の tmp を消す。
     12 本が同時に始まると最初だけ数本が重複して作るが、どれも同じ手順の結果になる。
- 雛形を使う側は、毎回 `<鍵>` を計算し、`<鍵>\manifest.txt` が在れば、その一覧の DB を data_dir へ複写する。
  `OnceLock` で場所を覚えない (鍵から毎回求める)。
- 公開済みの `<鍵>` は書き換えない・消さない (不変)。実行時の古い鍵の掃除はしない
  (同じ stem の別の世代が同時に動いていると、使用中の雛形を消し得るため)。1 世代あたり約 0.5 MB の残りは、
  `cargo clean`・worktree の撤去で消える。必要になったら、テストを止めた状態で消す手順を別に作る。

### 10.4 鍵と置き場所

- 鍵: `<実行体の file stem>` / `<実行体の大きさ>-<最終更新時刻 (ns)>` / `v<雛形の形式の版>-<SimilarFeatureCapability>`。
  形式の版は、雛形の作り方 (10.3) を変えたときに手で上げる定数。
- 保証の範囲: 通常のローカルの cargo のリンクで作られた実行体だけを対象にする。cargo はソースが変われば実行体をリンクし直し、
  大きさか最終更新時刻が変わる。同じ stem・同じ大きさ・同じ最終更新時刻を持つ別の内容の実行体への置き換えは対象外とする。
  実行体の中身のハッシュ (1.7 GB) は遅いので取らない。
- 置き場所: `<std::env::current_exe() の親のフォルダ>\app-test-template\<鍵>`。実行体の場所は cargo の版で変わり得るので、決め打ちせず `current_exe()` から求める。
  file stem ごとに分かれるので、feature 違いの別のテスト実行体の雛形とは混ざらない。

### 10.5 雛形に入れるファイル

- 入れる: data_dir の直下の `*.db` (`settings.db` を除く)。`settings.db` は除く (settings DB の既存 DB・JSON からの移行・
  clean install の分岐を、本来のテストの側に残すため)。
- 入れない: `settings.db`・`-wal`・`-shm`・`logs\`・`*.json`・そのほかの DB でないファイル (どれも constructor が今と同じく作る)。
- 将来 DB が増えても `*.db` なので自動で入る。DB でない状態ファイルが増えても入らない (今の経路のまま作られる)。
- 雛形を使ったテストどうしは、`folder_thumb_pin_revision.instance_id` (DB を新しく作るたびの UUID) が同じ値になる。
  テストごとに data_dir は別なので混ざらない。DB を作り直したときに instance_id が変わることを確かめるテストは `Fresh` にする。

### 10.6 検査

- (a) 新規作成の起動の契約 (`Fresh`、決定的): 空の data_dir で App を作り、drop の前に、constructor が開く DB の owner がすべて
  在る (`None` になっていない) こと、`spread_db` などの open が成功していることを確かめる。constructor の多くの DB の open は
  失敗を `.ok()` で `None` にするので、新規作成の時だけ失敗する退行は、雛形のテストでは見えない。ここで検出する。
  - owner が在ることは thread の起動の成功しか意味しない (worker の中で DB を開くもの: `book_bookmarks`・`content_identity`・
    `auto_aspect_cache`・resume writer など)。そうした owner は、実際の応答で初期化の成功を確かめる
    (例: content identity の初期の読み込みの成功、bookmarks の `list_all` の成功、auto-aspect の Count の応答、resume writer の読み出しの結果)。
    `is_busy() == false` のような「止まっている」ことの確認では、失敗して終わった worker と区別できないので使わない。
    constructor が作る、worker の中で DB を開く owner を列挙して、すべてに対して行う。
- (b) 雛形と今の経路の比較 (決定的): 同じテストの中で、`Fresh` の App と `Template` の App をそれぞれ作って drop し、2 つの data_dir を比べる。
  - 雛形の `manifest.txt` の DB の集合が、`Fresh` で作られた DB の集合 (`settings.db` を除く) と同じであること
    (雛形に無い DB を constructor が作り直して比較を通ってしまう場合を区別する)。
  - `manifest.txt` に載った各 DB の `sqlite_master` (表・索引・trigger の定義)、`user_version`、すべての表の行が同じであること
    (`settings.db` は雛形に入れないので比べない。移行の時刻と `book_root` の保存値が違うため)。
    正規化するのは次だけ: `folder_thumb_pin_revision.instance_id` (UUID の形式であること・1 行・revision 0 を別に確かめ、値は比べない)。
    ほかに違う値が出たら、正規化を足す前に、その値がテストごとに違ってよいものかを確かめて記録する。
  - DB でないファイルの名前の集合も比べる。`settings.json.migrated-<時刻>` の時刻の部分だけ正規化する。
  - settings が `None` と `Some(default + book_root)` の両方で行う。
- (c) 鍵の単体テスト: 実行体の大きさ・時刻・capability・形式の版が違えば鍵が違うこと。
- (d) `Template` 指定で data_dir が空でないときに panic すること。子の終了コードが 0 でないとき・`-wal` が残ったときに雛形の作成が失敗すること。

### 10.7 `Fresh` にすべきテストの洗い出し

- 次の起動時の初期化・移行の分岐は、雛形を使うと App の起動では通らなくなる (どれも DB のモジュールの単体テストは別に在る)。
  これらを App の起動として確かめているテストを `Fresh` にする。
  - `src/folder_thumb_pins.rs:842` 付近: crop 列の追加・revision 行の作成・trigger の導入 (`migration_ran`)
  - `src/spread_db.rs:764` 付近: marker が無いときの endpoint の移行。移行に失敗したときの read-only の経路 (`:192` 付近)
  - `src/edit_preview_cache.rs:146` 付近: version による作り直し。`:270` 付近の DB でないキャッシュの削除
  - `src/video/tile_thumb_cache.rs:134` 付近: 古い表の形式の移行、`:223` 付近の一度だけの無効化
  - `src/content_identity.rs:686` 付近: 今の schema の検証と、新規作成・upgrade の分岐
  - `src/book_resume_db.rs:216`・`:454` 付近: identity 列・meter 列の追加
  - `src/rating_db.rs:299`・`src/adjustment_db.rs:98`・`src/search_index_db.rs:915` 付近: 列・既存の schema に依存する追加と移行
- data_dir の中のファイル・`user_version`・DB が作られたことを見る App のテストを grep で探す。
- 雛形を入れた後の本体 lib 全件で落ちたテストを切り分ける。雛形のせいなら、`Fresh` にするか、テストの前提の誤りとして直す
  (テストの意味を変えないほうを選び、どちらにしたかを記録する)。全件で落ちないことだけを `Fresh` の判断の根拠にはしない。

### 10.8 状態の組み合わせ・まれな失敗

- 本来のテストのプロセスでは、雛形の有無にかかわらず、最初に作る App が本来のテストの App になる (雛形は子プロセスで作る)。
  process-global の状態の組み合わせは、今と変わらない。
- 雛形の作成は `test_override_lock` を持ったまま行う。子プロセスは別のプロセスなので、親の lock・override とは関係しない。
- まれな失敗 (複写・rename の失敗、ディスクの不足、子の異常終了など) はテストの失敗にする (割り切り)。
  再試行・今の経路への切り替えは入れない。

### 10.9 実装前のレビューで改めた点 (初版からの差分)

- 同じプロセスで雛形を作る → 子プロセスで作る (P1: `MODELS_DIR` などの process-global が雛形の作業フォルダに固定される)。
- drop の後の DB の比較だけ → 新規作成の起動の契約 (10.6 (a)) と、雛形の DB の集合の検査を足した (P1: 新規作成の時だけの失敗が `.ok()` で隠れる)。
  起動時の初期化・移行の分岐の一覧 (10.7) を足した。
- 実行時の古い鍵の掃除と `OnceLock` の場所の記憶 → 外した (P2: 同じ stem の別の世代が使用中の雛形を消し得る)。
- 全行の一致 → `instance_id` と移行の退避名の時刻だけを正規化する (P2)。
- 鍵の保証の範囲を、通常のローカルの cargo のリンクに限ると明記した (P3)。
- -wal が 0 バイトであることの確認だけ → 子プロセスの終了の後に checkpoint してから複写する (drop の後も接続を持つ worker があるため)。
- 2 回目のレビュー (ACCEPT WITH CHANGES) で足した点: 新規作成の起動の契約に worker の初期化の成功の確認を足した (P1)。
  子の終了コードに加えて完了の印と DB が 1 個以上あることを確かめる (P2)。比べる DB を manifest に載ったものに限った (P3)。
  置き場所を `current_exe()` から求める (P3)。

### 10.10 計測

- 本体 lib 全件を、雛形なしと雛形ありで比べる。雛形なしは、同じソースの master の全件
  (統括の `orchestrator/master-lib-full-after-823`、同じ PC・12 並列) の記録を使う。雛形ありは実装後にこのブランチで 1 回流す。
  条件の違い (流した時刻・ほかの処理の負荷) は報告に書く。どちらも run-heavy の `-FullTest`。v5.5.0 の作業の全件を優先し、待っている全件があれば先を譲る。
