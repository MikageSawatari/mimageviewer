# ブリーフ: 外部ツール起動 P1c — 関連付けアプリをシェルに起動させる

正本: [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md)。
P0 / P1 / P1b は実装済み。**この phase は実利用者が踏んだ不具合の修正**で、
出荷前に必ず要る。

作業場所: この worktree (`C:\home\mimageviewer-extlaunch`, ブランチ `external-tool-launch`)。

## 0. 症状と原因 (調査済み。ここは推測ではない)

利用者の環境で「アプリケーションで開く…」から関連付けアプリを選んでも**何も起きない**。
実際に `%APPDATA%\mimageviewer\settings.db` の `recent_open_with_apps` を読んで確認した:

| 表示名 | 保存されている `exe_path` | 実在 |
| --- | --- | --- |
| フォト | `フォト` | パスですらない |
| TsubameViewer | `TsubameViewer` | パスですらない |
| ペイント | `C:\Program Files\WindowsApps\Microsoft.Paint_...\PaintApp\mspaint.exe` | **No** (WindowsApps は ACL 保護) |

原因は [open_with.rs](../../src/open_with.rs) の `enumerate_handlers_inner` が
**`IAssocHandler::GetName()` の戻り値を exe パスとして保存している**こと。
この API が実行ファイルのパスを返すのは素の Win32 アプリのときだけで、
UWP / Store アプリでは ProgID やパッケージ識別子が返る。
`Command::new("フォト")` は当然失敗し、旧コードは `let _ = cmd.spawn();` で
エラーを捨てていたので無言で終わっていた (P1 で通知は出るようになったが、起動はしない)。

**利用者が自分で選んだ EXE は正常に起動する**ことは確認済み。壊れているのは関連付け経路だけ。

## 1. 直し方 — パスを取り出さず、シェルに起動させる

関連付けアプリは `Command` で spawn せず、**`IAssocHandler::Invoke`** (または
`CreateInvoker` → `Invoke`) に対象ファイルの `IDataObject` を渡す。
これで UWP、引数付きの verb、保護されたパスがすべて扱える。

- ハンドラの再取得: 起動時に対象ファイルの拡張子で `SHAssocEnumHandlers` を回し直し、
  保存しておいた識別子と `GetName()` が一致するものを選ぶ。COM ポインタは永続化できない。
- 見つからない場合は**黙って何もしない**のではなく、ツール名を添えて通知する
  (「関連付けアプリが見つかりません」)。アプリのアンインストール後などに起きる。
- `IDataObject` の作り方、COM apartment の初期化 (`CoInitializeEx`)、
  ハンドラ列挙が既に COM を触っていることは既存コードを参考にすること。
- **Win32 呼び出しは小さな 1 関数に閉じる。** その外側 (識別子の突き合わせ、対象の解決、
  エラー文面の組み立て) は純関数にしてテストする。COM 呼び出し自体は unit test しない。

## 2. 型の変更

`ExternalTool.executable: Option<PathBuf>` は「spawn できる EXE」を前提にしていて、
関連付けハンドラを表現できない。次のように分ける。

```rust
pub enum ExternalToolLaunch {
    Executable(PathBuf),              // 引数テンプレートが効く
    Association { handler_id: String }, // シェルが起動する。引数は使えない
    OsDefault,                        // 既存の executable: None 相当
}
```

- **`external_tools` テーブルはまだ出荷していない** (P0 で今日入れたばかり)。
  CLAUDE.md「永続データ・スキーマ変更時の判断」に従い、**マイグレーションは不要**。
  スキーマを作り直してよい。**その旨をコミットメッセージに一言残すこと。**
- 一方 **`recent_open_with_apps` はリリース済み**なので移行が要る。
  `exe_path` に何が入っているか分からない既存行を、一度だけ分類して保存し直す:
  - 実在するファイルパス → `Executable`
  - それ以外 → `Association { handler_id: <その文字列> }`
  分類は純関数にしてテストする。移行は `schema_meta` の marker で一度きり
  (P0 の `external_tools_migrated_from_custom_open_with` と同じ作法)。
- `custom_open_with_apps` からの移行は**利用者が自分で選んだ EXE** なので、
  従来どおり `Executable` にする。

## 3. UI

- 設定ページで、`Association` と `OsDefault` のツールは**引数欄と作業フォルダー欄を無効化**する
  (シェルが起動するので効かない)。`OsDefault` で既にそうなっているはずなので、同じ扱いに寄せる。
- 一覧の「実行ファイル」列は、`Association` なら「関連付けアプリ (<表示名>)」のように、
  **spawn できるパスでないことが読んで分かる**表示にする。今の `OS の関連付け` と同系統の文言で。
- `payload` / `spread` など他のポリシーは `Association` でも意味があるので残す
  (どのファイルを渡すかの話なので)。

## 4. テスト

- 既存行の分類 (実在パス / 非パス文字列 / 存在しないパス) の純関数。
- `recent_open_with_apps` 移行が一度きりであること、marker が消えても二重に走らないこと。
- `ExternalToolLaunch` の round-trip (3 種すべて)。
- `Association` / `OsDefault` のツールで引数テンプレートが使われないこと。
- ハンドラ識別子が見つからないときにエラー文面が出ること (突き合わせ部分の純関数)。

## 5. 守ること

- コミット前に `cargo fmt` (引数なし)。テストは `cargo test -p mimageviewer --lib`。
- UI 文言を足したら `python scripts/check_ui_glyphs.py`。
- UI スレッドで同期 I/O を足さない。**ハンドラの再列挙と `Invoke` は起動 worker の中で行う**
  (既に P1 で worker 化してある経路に載せる)。
- 範囲を広げない。複数ファイルを 1 つの `IDataObject` にまとめる話は P2 の複数選択で扱う。
- 正本と食い違ったら実装を止めて報告すること。

## 6. 完了報告に含めること

- 変更ファイル一覧、追加した純関数とテスト、テスト結果の件数
- `IAssocHandler::Invoke` を呼んでいる場所と、その外側の純関数の境界
- 実機で確認すべき操作。**利用者の環境には「フォト」「TsubameViewer」「ペイント」の
  3 件が recent に残っている**ので、それが起動できるようになったかが受け入れ条件になる
- 迷った点
