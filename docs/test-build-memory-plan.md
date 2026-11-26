# 全体テストのビルドのメモリ対策 (§1.908・§1.909・§1.910)

状態: 実装中 (ブランチ `test-mem-908`、worktree `G:\home\mimageviewer-testmem908`)。
担当: 設計・指示・検収 = ClaudeCode Opus、実装 = Codex gpt-6.1-sol / high、独立レビュー = 別の gpt-6.1-sol / xhigh。
正本のバックログ: [next-release-backlog.md](next-release-backlog.md) の §1.908・§1.909・§1.910。

## 1. 背景

v5.8.0 の配布ビルド (2026-11-26) で、`build-dist.ps1` の中の全体テスト (`test-full.ps1`) のビルドが 2 回、
os error 1455 (ページング ファイルが小さすぎる) とメモリ確保の失敗で止まった (統括の観測)。物理メモリは約 80 GB
空いていて、足りなくなったのはコミットの上限 (実メモリ + ページング ファイル、約 159 GB)。`CARGO_BUILD_JOBS=8` で通った。

テスト用の本体ライブラリ (`target\debug\build\mimageviewer\<hash>\out\libmimageviewer-<hash>.rlib`) は 1.54 GiB
(1,652,467,604 bytes)。統合テストの 21 本の実行ファイルなどが同時にリンクされることが原因、という見立て (推測、§3 で測る)。

## 2. 方針と決めたこと

### 2.1 §1.908 統合テストを 1 つのテスト crate にまとめる

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

### 2.2 §1.909 テスト用ビルドのデバッグ情報

- 現状: `[profile.dev]` は既に `debug = "line-tables-only"` (`[profile.test]` は dev を継ぐ)。バックログの「1.65 GB の
  かなりの部分はデバッグ情報」は §3.1 の内訳で確かめる。
- 利用者の判断 (2026-11-26): 落ちたテストは、そのテストだけデバッグ情報を増やして流し直せばよい。手順は
  [開発ビルドとテスト](development-build-and-test.md) に書く。
- 値は §3 の計測で決める (未定)。

### 2.3 §1.910 ビルドの同時数の上限

- `test-full.ps1`・`build-dist.ps1` (必要なら `test-lib-full.ps1`) で `CARGO_BUILD_JOBS` に上限を付ける。値は §1.908・§1.909 の
  後に測って決める (未定)。

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

(計測の続きは各段の後に追記する)

## 4. 確認

- テスト一覧の照合: 変更前後で `cargo nextest list --workspace --features pack-build-tools` の一覧を取り、
  旧 (binary, 名前) と新 (`integration`, `<ファイル>::名前`) を項目で突き合わせる。件数ではなく項目で比べる。
  support の重複分が 1 回になる以外の差が無いこと。
- GPU group の所属: 変更前後で group `gpu` に入るテストの集合を突き合わせる。
- スナップショット: 更新指定なしの通常比較で全件成功し、`tests/snapshots/` に差分が出ないこと。
- 全件テスト: テストの共通基盤を変えるので `test-full.ps1` を流す (CLAUDE.md「テストの段階分け」の例外)。
