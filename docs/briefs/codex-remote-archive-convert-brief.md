# リモート閲覧: 変換対象アーカイブ (RAR 等) を開く

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 0. 着手前に読むこと

この増分は **リモートから始まる初めての「ファイルを作る」操作**であり、かつ **初めての
長時間処理**でもある。既にある物を作り直さないこと。特に §2.6 (長時間処理の受け皿) と
§2.7 (再利用できる層) を先に読むこと。

- 本体側の経路を**終端まで**読んでから設計する。分岐の確定が別の関数にあることを想定する
- 新しい仕組みを作る前に、本体の既存経路・共通部品・設定を確認する。無いと判断したら
  その根拠を報告に書く
- ドキュメントとコードが一致しても裏付けにしない。実装を正とする

以下 §2 は 2026-08-09 に現物を読んで確認済み。行番号と関数名から始めてよい。

## 1. 観測されている状態

RAR / 7z / LZH は**一覧に出るのに開けない**。
`GridItem::ConvertibleArchive` → `RemoteEntryKind::Archive` の写像があるので一覧には出るが
([src/remote_ipc/container.rs:1724](../../src/remote_ipc/container.rs))、開く側が未対応。

利用者要望: 直接読めるものは直接開き、変換が要るものは確認のうえ変換して開きたい。
確認なし設定なら自動で変換するが、**時間がかかるのでリモート側に進行を出す**こと。

## 2. 調査済みの事実 (再調査不要)

### 2.1 本体の 3 択と、`Ignore` の現状

`Settings::archive_file_handling_resolved()` が `Ask` / `Convert` / `Ignore` を返す。
判定は既存 helper (`archive_convert_suppresses_confirm()` /
`archive_file_handling_ignores_convertible()`) を使い、設定値をリモート側で読み替えない。

**`Ignore` は既に効いているはず。** `folder_scan::scan_directory_with_settings` が
`include_convertible_archives: !settings.archive_file_handling_ignores_convertible()` を
渡しており、リモートの一覧も同じ materializer を通る。**確認して「既に正しい」と報告する
だけでよい。** 効いていなければ直す。

### 2.2 本体が開くときの順序

[src/app.rs:16748](../../src/app.rs) 付近:

1. 拡張子から `ArchiveFormat` を判定
2. `Ignore` 設定なら開かずに通知して終了
3. **RAR** → `try_archive_cache_lookup` の結果を `fallback_cached_zip` として
   `request_rar_open_owned`。RAR だけ `allow_direct_read: true`
4. **RAR 以外** → `try_archive_cache_lookup` が当たれば**確認なしでそのまま開く**。
   外れたときだけ `request_archive_convert_owned`

**変換済みのアーカイブは確認を出さずに開く。** これを落とさないこと。

### 2.3 RAR を直接読めるかの判定

`crate::rar_loader::inspect_for_direct_read(path)` が正本。
`inspection.decision == RarDirectReadDecision::Direct` なら変換せずに開けて、
`inspection.resolved_path` が開く対象になる。**この述語を複製しないこと。**

### 2.4 進捗は実数が取れる

`archive_converter::convert_to_zip(src, dst, format, cancel, progress)` の `progress` は
`Option<&dyn Fn(ConvertProgress)>` で、`ConvertProgress { files_done, files_total,
bytes_written }` が各ファイル完了ごとに来る。**「作業中」表示で妥協する必要はない。**

事前スキャンの `ArchiveImageSummary` に `total_uncompressed_bytes` と
`nested_archive_count` があり、確認ダイアログの注記に使われている。

### 2.5 キャンセルできる

`cancel: &AtomicBool` を各エントリ境界で見る。検出時は `ConvertError::Cancelled`。
中間ファイルへ書いて atomic rename するので、失敗・中止で壊れた ZIP は残らない。

### 2.6 ★ 長時間処理の受け皿は既にある — 作り直さないこと

**変換を IPC 要求の中で同期実行してはいけない。** 直前の増分で学んだのと同じ問題で、
`state.ipc_admission.run(IpcClass::Heavy, ...)` の枠を分単位で握ることになる。

リモートには既に**ジョブ方式の長時間処理**がある。AI アップスケールの
`/api/ai/jobs` 一式 ([crates/remote-web/src/http.rs](../../crates/remote-web/src/http.rs) の
`api_ai_*`、IPC 側は `remote_ai_start` / `remote_ai_state` / `remote_ai_cancel` /
`remote_ai_result` / `remote_ai_recoverable`)。開始・状態取得・中止・結果取得が分かれており、
進捗と失敗理由を運ぶ形が既にできている。

**まずこれを読み、同じ形に載せられるかを判断すること。** 載せられるなら新しい仕組みを
作らない。載せられないなら、どこが合わないかを報告に書いてから別の形にする。

### 2.7 再利用できる層と、再利用できない層

**再利用できる (UI に依存しない):**

- `archive_cache::ArchiveCacheDb` — `open()` / `lookup()` / `peek()` / `record()` /
  `reserve_cache_zip_path()`
- `ArchiveCacheDb::begin_convert()` — **プロセス全体で変換を直列化する `MutexGuard`**。
  本体 UI と共有する。**2 つ目のロックを作らないこと**
- `archive_converter::convert_to_zip` / `convert_to_zip_with_password`
- `rar_loader::inspect_for_direct_read`

**再利用できない:**

- `ui_dialogs::archive_convert::ArchiveConvertState` — `App` 上に載る **UI ダイアログの
  状態機械**で、egui のフレームで駆動される。`Scanning` → 確認 → `Converting` という
  phase を持つ。remote_ipc の worker には `App` が無い
- したがって**リモートはダイアログを動かすのではなく、上の再利用できる層を自分で駆動する**

### 2.8 パスワード付きアーカイブ

`ConvertError::PasswordRequired` があり、本体は `password_input` で入力を受ける。
IPC 側には既に `ThumbnailErrorCode::PasswordRequired` がある (PDF 用)。

**この増分で対応するかを決めて報告すること。** 対応しないなら、無言で失敗させず
「パスワード付きは未対応」と分かる形で止める。

## 3. やること

### 3.1 開く

| 設定 | リモートの挙動 |
| --- | --- |
| `Ask` | リモート側に確認を出す。変換して開くこと・時間がかかることが分かる文言にする |
| `Convert` | 確認なしで変換を開始し、**進行を表示する** |
| `Ignore` | 一覧に出さない (§2.1、既に効いているはず) |

- **変換済みなら確認を出さずに開く** (§2.2)
- **直接読める RAR は変換せずに開く** (§2.3)
- 変換後は既存の変換済み ZIP と同じ経路で開く。リモート専用のキャッシュを作らない

### 3.2 進行の表示

- 変換中はリモート側に進行が見えること。無反応の時間を作らない
- **進捗は実数が取れる** (§2.4)。件数で出す
- 失敗したときは理由が分かる形で止める。無言で一覧へ戻さない
- 利用者が待つのをやめられること (中止できること。§2.5 の cancel がある)

### 3.3 書き込みの位置づけ

- 書くのは**本体側**。remote-web が変換結果を書かない (「本体が唯一の writer」を崩さない)
- 変換先は本体の既存アーカイブキャッシュ (`reserve_cache_zip_path`)。専用の保存先を作らない
- session owner を持たない要求から変換が始まらないこと
- 変換中に session owner が別端末へ移ったときの扱いを決めること

## 4. やってはいけないこと

- `ArchiveFileHandling` をリモート側で読み替えること
- 変換の可否・直接読めるかの判定をリモート側に複製すること (§2.3)
- remote-web が変換結果をディスクへ書くこと
- 変換を IPC 要求の中で同期実行すること (§2.6)
- 変換の直列化ロックを新設すること (§2.7 の `begin_convert` を使う)
- 進行表示なしで長い変換を始めること
- 確認なしで変換を始めること (`Convert` 設定を除く)
- 変換済みアーカイブに確認を出すこと (§2.2)

## 5. テスト

- `Ask` / `Convert` / `Ignore` の 3 値それぞれで §3.1 の表と一致すること
- 変換済みアーカイブが確認なしで開くこと
- 直接読める RAR が変換されずに開くこと
- 変換の成功・失敗・中止それぞれの経路
- 進捗が単調に進むこと
- 変換中に session owner が別端末へ移ったときの扱い
- 一覧が `Ignore` を尊重すること

## 6. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`cargo test -p mimageviewer-ipc`、
  `cargo test -p mimageviewer-remote`、`node --test crates/remote-web/web/*.test.mjs`
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- 報告に含めること:
  - §2.1 の `Ignore` が既に効いているか
  - §2.6 の AI ジョブ方式に載せられたか。載せられないならどこが合わなかったか
  - §2.8 のパスワードをどう扱ったか
  - ブリーフの記述と現物が食い違っていた箇所 (あれば)
- 決定は `docs/web-remote-plan.md` へ書き戻すこと。ブリーフは git 管理外なので、
  そこにしか無い決定は次のセッションが読まない
- ビルドとコミットは行わない。`htdocs/` は触らない
