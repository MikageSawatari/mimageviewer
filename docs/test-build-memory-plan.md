# 全体テストのビルドのメモリ対策 (§1.908・§1.909・§1.910)

状態: 実装中 (ブランチ `test-mem-908`、worktree `G:\home\mimageviewer-testmem908`)。
担当: 設計・指示・検収 = ClaudeCode Opus、実装 = Codex gpt-6.1-sol / high、独立レビュー = 別の gpt-6.1-sol / xhigh。
正本のバックログ: [next-release-backlog.md](next-release-backlog.md) の §1.908・§1.909・§1.910。

## 1. 背景

v5.8.0 の配布ビルド (2026-11-26) で、`build-dist.ps1` の中の全体テスト (`test-full.ps1`) のビルドが 2 回、
os error 1455 (ページング ファイルが小さすぎる) とメモリ確保の失敗で止まった (統括の観測)。物理メモリは約 80 GB
空いていて、足りなくなったのはコミットの上限 (実メモリ + ページング ファイル、約 159 GB)。`CARGO_BUILD_JOBS=8` で通った。

テスト用の本体ライブラリ (`target\debug\build\mimageviewer\<hash>\out\libmimageviewer-<hash>.rlib`) は 1.54 GiB
(1,652,467,604 bytes)。統合テストの 22 本などが同時にそれを読み込むことが原因、という見立てで始めた (§3.2 で、読み込む
rustc の本数が原因で、リンクではないことが分かった)。

## 2. 方針と決めたこと

### 2.1 §1.908 統合テストを 1 つのテスト crate にまとめる

- 対象は `tests/` 直下の 22 ファイル (着手時に 21 本と書いたのは数え違い)。
- `tests/integration.rs` を crate root とする 1 つの `[[test]] name = "integration"` にし、`Cargo.toml` の `[package]` に
  `autotests = false` を置く。既存の `tests/<name>.rs` は**移動せず**、`tests/integration.rs` から `mod <name>;` で読む。
  - 移動しない理由: 文書・コメントが `tests/ui_snapshot.rs` などのパスを多数参照している。スナップショット PNG の
    置き場所 (`tests/snapshots/<name>.png`) とファイル名は、テストが渡す名前と `CARGO_MANIFEST_DIR` で決まり、変わらない。
  - `tests/<name>/main.rs` 形式 (自動検出のまま) にしなかった理由: 21 本の移動で上記の参照がすべて変わる。
- `tests/support/*.rs` と `tests/common/` は `tests/integration.rs` で 1 回だけ宣言し、各ファイルは `use crate::…` で使う。
  ファイルごとに `#[path]` で読んでいた共通部分の重複 (同じ support の `#[test]` が 4 本の実行ファイルに 1 回ずつ入っていた)
  は 1 回になる。
- `autotests = false` で新しい `tests/<name>.rs` を足し忘れると、そのファイルは黙ってビルドから外れる。これを防ぐため、
  `tests/` 直下の `.rs` (`integration.rs` 自身を除く) がすべて `tests/integration.rs` に 1 回ずつ宣言されていることを
  確かめるテストを integration crate に置く (足し忘れは全体テストで落ちる)。
- テスト名は `<ファイル名>::<元の名前>` になる。個別に流す手順は次の形に置き換える。
  - 旧: `cargo test -p mimageviewer --test ui_snapshot` → 新: `cargo test -p mimageviewer --test integration ui_snapshot::`
  - 旧: `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot` → 新: `UPDATE_SNAPSHOTS=1 cargo test -p mimageviewer --test integration ui_snapshot::`
  - nextest: `cargo nextest run -p mimageviewer --test integration -E 'test(/^ui_snapshot::/)'`
  - 過去の設計書・指示書・記録に残る `--test <name>` は書き換えず、この対応で読み替える (記録の改変をしない)。
    手順として今も使う文書 (CLAUDE.md、development-build-and-test.md、ui-snapshot-policy.md、gpu-test-groups.md など) だけを直す。
- **1 つのプロセスに入るテストの組み合わせ**: 旧形では `cargo test --test <name>` がファイル単位で別プロセスだった。新形でも
  モジュールで絞れば同じ組み合わせになる。絞らずに `cargo test --test integration` を流すと、別ファイルのテストが同じ
  プロセスに入る (例: `archive_conversion_failure_diagnostics` は `DATA_DIR` とロガーをプロセスで 1 回だけ設定する)。
  integration 全体は nextest (テストごとに別プロセス) で流す。`test-full.ps1`・`test-focused.ps1` は nextest なので変わらない。
  状態を減らす観点: 全体を同じプロセスで通せるようテスト側を直す案は採らない (旧形でも保証していなかった組み合わせを
  新しく扱うことになる)。手順を「全体は nextest、cargo test はモジュールで絞る」の 1 つにする。
- nextest の GPU group (`.config/nextest.toml`) は、binary 名で選んでいた 4 本 (`ui_snapshot`・`book_composition_editor_snapshots`・
  `test_ai_upscale`・`test_tile_blend`) を、`binary(/^integration$/)` とテスト名の先頭のモジュールで選ぶ形に置き換える。
  対象のテストの集合は変えない (§4 で照合する)。
- `test-focused.ps1` の対応表 (`scripts/test-tools-common.ps1`): `tests/<name>.rs` → `-p mimageviewer --test integration`
  と `-E 'test(/^<name>::/)'`。`tests/integration.rs` 自身と `tests/support/`・`tests/common/`・`tests/snapshots/` は広域。
- CI (`.github/workflows/ci.yml`) は統合テストを流していない (fmt と `viewer_context_audit` だけ) ので変更しない。

### 2.2 §1.909 テスト用ビルドのデバッグ情報 — 採らない (利用者決定 2026-11-26)

- 着手時の前提との食い違い: バックログの利用者の判断 (2026-11-26) は「テスト用を `line-tables-only` などにする。ファイル名と
  行番号は残る。落ちたテストはそのテストだけデバッグ情報を増やして流し直せばよい」。ところが `[profile.dev]` は 2026-09-17 から既に
  `debug = "line-tables-only"` で、test はそれを継いでいる。rlib のうち CodeView (行番号表) は約 12% で、残りはメタデータ・
  コード・シンボルなど (§3.1)。
- さらに減らすと `debug = false` になる。計測 (§3.6): rlib 1.539 → 1.303 GiB、冷えた状態からのビルド 630 → 549 秒 (同時数 24)、
  **ピークのコミットはほぼ変わらない** (ピークは rlib を読む rustc の一斉開始と本体 lib テストのコンパイルの重なりで決まる)。
  失うのはバックトレースの関数名・行番号 (panic メッセージの発生箇所のファイル名・行番号は残る)。
- `[profile.test]` だけを変えると dev と test の設定が分かれ、dev で `target/debug` をビルドしたときに依存が 2 通り作られる
  (2026-09-17 に target/debug が 622 GB まで膨らみ、設定を `[profile.dev]` 1 か所にまとめた経緯がある)。分けないなら
  `[profile.dev]` を `debug = false` にすることになり、テスト以外の dev ビルドのバックトレースにも及ぶ。
- 目的 (メモリ) に効かず、利用者が想定していなかった退行を伴うため、設計担当は「採らない (現状維持)」を推奨し、統括経由で利用者に
  確認した。**利用者の決定 (2026-11-26): 採らない (現状維持)。** `[profile.dev]` の `debug = "line-tables-only"` (test はこれを継ぐ) の
  ままとし、Cargo.toml は変えない。計測の記録は §3.1・§3.6 と [テスト待ち時間の記録](test-speed-report.md) に残す。
  バックログの §1.909 は統括が master で閉じる。

### 2.3 §1.910 ビルドの同時数の上限

- §3 の計測に基づき、`test-full.ps1` 内で `CARGO_BUILD_JOBS` の上限を **8** にする。
  呼び出し側が 8 以下の正の整数を指定済みならそれを使い、未設定・上限超過・不正な値は 8 にする。
  不正な値を置き換えた旨と実際に使う値を表示し、終了時は `finally` で元の値 (未設定を含む) に戻す。
- `build-dist.ps1` は別の PowerShell プロセスで `test-full.ps1` を呼ぶので、同じ上限が効く。
  release のビルドは今回の失敗と関係がなく、遅くしないため上限を付けない。
- `test-lib-full.ps1` は本体 lib テストだけで、本体の rlib を読む rustc が一斉に始まる場面がないため上限を付けない。

### 2.4 まれな失敗

ビルドの途中でメモリが尽きた場合の回復の仕組みは作らない。既定の割り切り (ログと失敗の終了コード、流し直し) による。

## 3. 計測

計測は `scripts/measure-test-build.ps1` (1 秒ごとにシステムのコミット量とコンパイラ・リンカの private bytes を記録し、
コミットが上限の 92% を超えたら cargo を止める) を、run-heavy の `-FullTest` (ほかの重い処理と重ねない) で流す。
ビルドは `cargo test --workspace --features pack-build-tools --no-run` (`test-full.ps1` の nextest と同じ target の集合)。
`-CleanDebug` で `target\debug` を消した冷えた状態から測る。release の 6 本は master の v5.8.0 配布ビルドの成果物をコピーした
(launcher の build script が存在を要求するだけで、テストのビルドの中身には影響しない)。
rlib の内訳は `python scripts/rlib_breakdown.py <rlib>`。

### 3.1 rlib の内訳 (master 451ad09db 相当、C:\home\mimageviewer の既存の rlib)

| 部分 | 大きさ | 割合 |
| --- | --- | --- |
| 合計 | 1.539 GiB | 100% |
| rustc のメタデータ (`lib.rmeta`) | 0.374 GiB | 24.3% |
| COFF オブジェクト 256 個 | 0.995 GiB | 64.7% |
| ┗ `.debug$S` (CodeView、行番号表) | 0.184 GiB | 11.9% |
| ┗ `.debug$T` | 0.004 GiB | 0.3% |
| ┗ `.rdata` | 0.259 GiB | 16.8% |
| ┗ `.text` | 0.092 GiB | 6.0% |
| ┗ ヘッダ・シンボル・文字列・再配置 | 0.438 GiB | 28.4% |
| ar の索引・名前表 | 0.170 GiB | 11.0% |

### 3.2 変更前 (master 相当)、既定の同時数 (24) — 上限に達して停止

2026-11-26 20:23、`-Label base-j24 -CleanDebug`、ログは worktree の `target\build-measure\base-j24\`。

- 開始時点の PC 全体のコミットが **88.3 GB** (重い処理は走っていない。上位のプロセスの private bytes を足しても約 25 GB で、
  残りはプロセスの外。原因は範囲外で未調査)。上限 159.3〜162.5 GB までの余裕は約 70 GB。
- 0〜310 秒: 依存 crate と本体 (rlib 用と lib テスト用の 2 本の rustc、各 11.6 GB まで) のコンパイル。コミットは 109 GB まで。
- **310→312 秒**: 本体の rlib ができた直後に、本体を読み込む rustc が 22 本同時に始まり (統合テスト 22 本、`mimageviewer-core` の
  bin テスト、pack builder 2 本、example `bench_pdf_count`、`musicvideo-editor`)、コミットが 1 秒で 101→145 GB、次の 1 秒で
  162.5 GB (上限) に達した。計測スクリプトが cargo を止めた (exit 3)。1 本あたり約 2 GB を始まった瞬間に確保している。
- 見立て (推測、未確認): rustc が依存 crate の rlib を書き込み時コピー (copy-on-write) の形でメモリに対応付けると、Windows は
  対応付けた全体の大きさをその時点でコミットとして数える。1 本あたりの約 2 GB は、本体の rlib (1.54 GiB) と依存 crate の rlib の
  合計に近い。そうなら、読み込む本数 (§1.908) と rlib の大きさ (§1.909) の積がピークを決める。リンカ (rust-lld) の private bytes は
  この時点で 0.24 GB 以下で、リンクが原因という当初の見立ては当たっていない。

### 3.3 変更前 (master 相当)、同時数 8 — 基準

2026-11-26 20:29、`-Label base-j8 -Jobs 8 -CleanDebug`。v5.8.0 で通った設定。

| 項目 | 値 |
| --- | --- |
| 結果 | 成功 (cargo のログに error 0 件。この回は計測スクリプトの不具合で終了コードを取れず、後で直した) |
| 所要時間 (冷えた状態から) | 756 秒 |
| 開始時のコミット / ピーク / 増加 | 87.9 GB / 121.2 GB (347 秒) / **+33.3 GB** |
| コンパイラ・リンカの private bytes の合計のピーク | 34.3 GB |
| rustc 1 本のピーク | 21.5 GB (本体の lib テスト) |
| rust-lld 1 本のピーク | 2.0 GB |
| rlib | 1.539 GiB |
| できた実行ファイル | 105 本、4.08 GiB |

### 3.4 §1.908 の後、同時数 8

2026-11-26 21:22、`-Label a908-j8 -Jobs 8 -CleanDebug`。

| 項目 | 変更前 (§3.3) | §1.908 の後 |
| --- | --- | --- |
| 所要時間 | 756 秒 | **627 秒** |
| 開始時のコミット / ピーク / 増加 | 87.9 / 121.2 / +33.3 GB | 77.4 / 114.6 / +37.2 GB |
| private bytes の合計のピーク | 34.3 GB | 37.2 GB |
| rustc 1 本のピーク | 21.5 GB | 21.4 GB |
| rlib | 1.539 GiB | 1.539 GiB (変わらない) |
| できた実行ファイル | 105 本、4.08 GiB | 84 本、3.54 GiB |

同時数 8 では、rlib の直後に始まる rustc が 8 本で頭打ちになるので、ピークの差は出ない (増加の差は開始時の
PC の状態の揺れの範囲と見る)。時間は統合テストの 22 回のコンパイル・リンクが 1 回になった分だけ短くなった。

### 3.5 §1.908 の後、既定の同時数 (24)

2026-11-26 22:34、`-Label a908-j24 -CleanDebug -AbortFraction 0.88` (足し忘れ検査の r1 修正を含む作業ツリー)。

| 項目 | 変更前 (§3.2) | §1.908 の後 |
| --- | --- | --- |
| 結果 | 312 秒で上限に達して停止 | **成功** |
| 所要時間 | — | 630 秒 |
| 開始時のコミット / ピーク / 増加 | 88.3 / 162.5 (上限) / +74 GB 以上 | 80.4 / 121.0 (294 秒) / **+40.7 GB** |
| private bytes の合計のピーク | 76 GB 以上 | 40.9 GB |
| rustc の同時数の最大 | 24 | 24 |
| rustc 1 本のピーク | 11.6 GB (停止時点) | 21.4 GB (本体の lib テスト) |

同時数 8 (§3.4、627 秒) と所要時間が変わらないのは、本体の lib テストと rlib のコンパイル (それぞれ 1 本の rustc) が
全体の長さを決めているため。§1.908 の後は同時数を上げても速くならず、増えるのはピークのコミットだけ。

### 3.6 §1.909 の試験: テスト用のデバッグ情報なし (`CARGO_PROFILE_TEST_DEBUG=false`)

2026-11-26 22:58 と 23:10、§1.908 の後の作業ツリーで、環境変数だけを変えて測った (ソースは変えていない)。

| 項目 | line-tables-only・24 (§3.5) | なし・24 | なし・8 |
| --- | --- | --- | --- |
| 所要時間 | 630 秒 | 549 秒 | 578 秒 |
| 開始時のコミット / ピーク / 増加 | 80.4 / 121.0 / +40.7 GB | 81.4 / 122.5 / +41.1 GB | 81.8 / 114.2 / **+32.4 GB** |
| private bytes の合計のピーク | 40.9 GB | 41.0 GB | 32.1 GB |
| rustc 1 本のピーク | 21.4 GB | 20.1 GB | 19.9 GB |
| rust-lld 1 本のピーク | 2.0 GB | 1.1 GB | 1.2 GB |
| rlib | 1.539 GiB | 1.303 GiB | 1.303 GiB |

なしの rlib の内訳: COFF オブジェクト 0.760 GiB (`.debug$S` が無くなり、シンボル・再配置も 0.438 → 0.390 GiB)、
メタデータ 0.374 GiB、ar の索引 0.169 GiB。ピークは、rlib ができた瞬間に rlib を読む rustc が一斉に始まり (同時数 24 で約 10 本、
1 秒で +20 GB)、そこへ本体 lib テストのコンパイル (1 本で 10〜21 GB) が重なるところで出る。rlib の 15% の縮小では、この重なりは
ほとんど変わらない。同時数を 8 にすると一斉に始まる本数が減り、ピークが約 9 GB 下がる。§1.909 は採らない (§2.2、利用者決定)。

(計測の続きは各段の後に追記する)

## 4. 確認

- テスト一覧の照合: 変更前後で `cargo nextest list --workspace --features pack-build-tools` の一覧を取り、
  旧 (binary, 名前) と新 (`integration`, `<ファイル>::名前`) を項目で突き合わせる。件数ではなく項目で比べる。
  support の重複分が 1 回になる以外の差が無いこと。
- GPU group の所属: 変更前後で group `gpu` に入るテストの集合を突き合わせる。
- スナップショット: 更新指定なしの通常比較で全件成功し、`tests/snapshots/` に差分が出ないこと。
- 全件テスト: テストの共通基盤を変えるので `test-full.ps1` を流す (CLAUDE.md「テストの段階分け」の例外)。

### 4.1 §1.908 の結果 (2026-11-26)

- 一覧 (`cargo nextest list --workspace --features pack-build-tools`、ignored を含む): 変更前 23,166 件、変更後 23,150 件。
  変更前の (binary, 名前) を新しい名前へ写した集合 (support の重複 17 件が 1 回になり 23,149 件) と比べて、**消えた項目 0 件**、
  増えた項目は足し忘れ検査 `every_integration_test_file_is_declared_once` の 1 件だけ。本体の統合テストは 471 → 455 件。
- GPU group: 変更前 720 件 (写した後 705 件) → 変更後 695 件。増えた項目 0 件。外れたのは共通 support の CPU テスト 10 件
  (`logical_bounds::high_dpi_node_bounds_reject_a_control_outside_the_logical_viewport`、`test_ui::backend_policy::tests::` の 6 件、
  `test_ui::scroll_layout::tests::` の 2 件、`test_ui::tests::requests_product_backends_without_initializing_gpu`)。以前は旧 binary 全体を
  選ぶ形のために入っていた。lib 側の同じ support も group 外で、下の integration 全体の実行 (GPU 生成検査が有効な nextest) で通った。
- integration 全体: `cargo nextest run -p mimageviewer --test integration` で 455 件成功 (285.5 秒)。更新指定なしのスナップショット比較を含み、
  `tests/snapshots/` に差分なし。
- `scripts/test-test-tools.ps1` (focused の対応表の fixture を含む): 成功。
- 実装中の修正: 指示書どおりに nextest の binary 述語を本体用と vendor 用の 2 つの正規表現に分けたところ、workspace graph で vendor 用の式が
  何にも一致せず設定エラーになった (nextest は graph に無い名前の binary 述語をエラーにする)。以前と同じく 1 つの正規表現に両方の名前を入れ、
  モジュール名の prefix で絞る形にした。vendor graph での確認は `test-full.ps1` で行う。
- 独立レビュー (gpt-6.1-sol / xhigh、read-only): r1 は要修正 (P1: 足し忘れ検査がソースの正規表現で、ブロックコメントや `#[path]` で
  外れたテストを見抜けない / P1: `tests/<dir>/main.rs` を検知しない / P2: 全体を `cargo test` 1 プロセスで流す現用テンプレート /
  P3: PNG の場所の誤記)。1 つのマクロで `mod` 宣言と一覧を作る形と、ディレクトリ形式の検知に直した (2df2db9a7)。r2 は ACCEPT。
- 修正後: 冷えた状態から既定の同時数でビルド成功 (§3.5)、integration 455 件成功、`tests/snapshots/` 差分なし、`test-test-tools.ps1` 成功。
