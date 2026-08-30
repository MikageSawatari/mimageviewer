# ブリーフ: 外部ツール起動 P0 — 型と設定 DB と移行

正本: [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md)。**着手前に §4.1 / §4.3 / §4.5 /
§4.10 と §6 の決定済み一覧を読むこと。** 本ブリーフは正本の P0 だけを対象にする。

作業場所: この worktree (`C:\home\mimageviewer-extlaunch`, ブランチ `external-tool-launch`)。

## 0. P0 の範囲

**やること**: `ExternalTool` の型定義、`Settings` への追加、SQLite への永続化、
既存 `custom_open_with_apps` からの一度きりの移行、そのテスト。

**やらないこと** (後続 phase):

- 環境設定 UI、コンテキストメニュー、ツールバー、キー割り当て (P1〜P2)
- 引数テンプレートの展開・プロセス起動 (P1)
- 一時実体化・寿命管理 (P3)
- 動画フレーム・見開き合成 (P4)

型と保存だけを入れる。**UI からは何も見えなくてよい。**

## 1. 型 (新規モジュール `src/external_tool.rs`)

正本 §4.1 の `ExternalTool` を実装する。純データ + 純ロジックだけを置き、
`App` や egui に依存させない (unit test しやすくするため)。

```rust
pub struct ExternalToolId(pub u32);   // Copy + Eq + Hash。0 は未使用の sentinel にしない (Option で表す)
pub struct ExternalTool {
    pub id: ExternalToolId,
    pub name: String,
    pub executable: Option<PathBuf>,  // None = OS の関連付けで開く
    pub arguments: String,            // 既定 "{file}"
    pub working_directory: Option<PathBuf>,
    pub payload: PayloadPolicy,
    pub video: VideoPolicy,
    pub spread: SpreadPolicy,
    pub selection: SelectionPolicy,
    pub pdf_render_long_edge: u32,    // 既定 4096 (正本 §6 決定済み 5)
    pub for_editing: bool,
    pub show_in_context_menu: bool,
    pub keep_temp: bool,
}
```

enum は正本のとおり。**serde では文字列 variant** にする (他の設定 enum と同じ readability 方針)。

| enum | 値 | 既定 |
| --- | --- | --- |
| `PayloadPolicy` | `AsDisplayed` / `Original` / `Container` / `RealFileOnly` | `AsDisplayed` |
| `VideoPolicy` | `File` / `CurrentFrame` | `File` |
| `SpreadPolicy` | `Merged` / `BothPages` / `MainPageOnly` | `Merged` |
| `SelectionPolicy` | `Single` / `Each` / `Batch` | `Single` |

あわせて次の純関数を置き、テストする。

- `ExternalTool::display_name(&self) -> String` — `name` が空なら `executable` の `file_stem`、
  それも無ければ「関連付けアプリ」相当の固定文字列。NeeView と同じ規則 (正本 §2.1)。
- `ExternalTool::defaults_for_editing()` / `defaults_for_viewing()` — 追加ダイアログ用の既定値。
  **編集用は `payload = Original` / `spread = MainPageOnly` / `for_editing = true`** (正本 §4.1)。
- `fn next_id(existing: &[ExternalTool]) -> ExternalToolId` — 既存の最大値 + 1。空なら 1。
  **並べ替えても ID は変わらない**ことをテストで固定する。

引数テンプレートの展開は P1 なので**まだ書かない**。

## 2. `Settings` への追加

- `pub external_tools: Vec<ExternalTool>` を追加し、`Default` で空 `Vec`。
- `settings.rs:7993` 付近の「他インスタンスから設定を取り込む」経路 (`std::mem::take` を並べている所)
  にも追加する。**ここを忘れると、設定画面 OK 時に一覧が消える。**

`recent_open_with_apps` / `custom_open_with_apps` は**この phase では残す**。
`custom_open_with_apps` は移行元として読むだけの legacy とし、以後 mIV からは書かない。
両フィールドにその旨のコメントを付ける (次リリース後に削除予定であること)。

## 3. SQLite への永続化 (`src/settings_db.rs`)

### 3.1 テーブル

`init_schema` の中 ([settings_db.rs:1444](../../src/settings_db.rs:1444) の `custom_open_with_apps` の隣) に足す。

```sql
CREATE TABLE IF NOT EXISTS external_tools (
   id                  INTEGER PRIMARY KEY,
   name                TEXT NOT NULL,
   executable          TEXT,            -- NULL = 関連付けで開く
   arguments           TEXT NOT NULL,
   working_directory   TEXT,
   payload             TEXT NOT NULL,
   video               TEXT NOT NULL,
   spread              TEXT NOT NULL,
   selection           TEXT NOT NULL,
   pdf_render_long_edge INTEGER NOT NULL,
   for_editing         INTEGER NOT NULL,
   show_in_context_menu INTEGER NOT NULL,
   keep_temp           INTEGER NOT NULL,
   sort_index          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS external_tools_sort ON external_tools(sort_index);
```

**`exe_path` を PRIMARY KEY にしている既存 2 テーブルの真似をしないこと。**
外部ツールは「同じ EXE を引数違いで複数登録する」ことが前提 (正本 §4.10 の複製) なので、
実行ファイルパスで一意にしてはならない。主キーは `id`。

### 3.2 read / write

`write_recent_apps` / `read_recent_apps` ([settings_db.rs:2124](../../src/settings_db.rs:2124)) と同じ形で
`write_external_tools` / `read_external_tools` を書く。`sort_index ASC` で順序を保つ。

`COMPLEX_FIELDS` ([settings_db.rs:66](../../src/settings_db.rs:66)) に `"external_tools"` を足し、
`build_settings_from_db` で `map.insert("external_tools", ...)` する。

> ⚠ `COMPLEX_FIELDS` に足すときの既存注意書き (同ファイルの doc comment) を読むこと。
> 「`settings_kv` に残った旧 JSON を空テーブルで上書きしてユーザー設定を消す」問題である。
> **今回は `external_tools` が新規キーで `settings_kv` に存在し得ないので、この問題は起きない。**
> 起こり得るのは `custom_open_with_apps` からの移行漏れの方なので、そちらを 3.3 で扱う。
> この判断の根拠をコード近くのコメントに残すこと。

### 3.3 移行 (リリース済みデータなので必須)

`custom_open_with_apps` は**リリース済み**の設定なので、黙って捨ててはならない。

- 一度きりの移行にする。`schema_meta` に marker row
  (`external_tools_migrated_from_custom_open_with = '1'`) を置き、既にあれば何もしない。
- marker が無いときだけ `custom_open_with_apps` を `sort_index` 順に読み、
  `ExternalTool` へ変換して `external_tools` へ書き、marker を立てる。
- 変換規則:

  | 変換元 | 変換先 |
  | --- | --- |
  | `display_name` | `name` |
  | `exe_path` | `executable = Some(PathBuf)` |
  | — | `arguments = "{file}"` |
  | — | `id` は 1 から順に採番 |
  | — | 他は各 enum の既定値。`show_in_context_menu = true`、`for_editing = false` |

- **`external_tools` に既に行があるときは移行しない** (marker が消えた場合の二重登録を防ぐ)。
- **`custom_open_with_apps` テーブルは消さない。** 以後 mIV は書かないが、行は残す。
  移行がおかしかったときに現物を確認できる方が安全で、消して得られるものが無い。
- 既存の migration 関数 (`migrate_tags_table` [settings_db.rs:1486](../../src/settings_db.rs:1486)、
  `migrate_folder_thumb_sort_default_v2` [settings_db.rs:1604](../../src/settings_db.rs:1604)) の
  呼び出され方と失敗時の扱いに揃えること。**独自の握り潰しを作らない。**

### 3.4 未知値の扱い

既存方針を変えない。enum の未知 variant / 未知 field でのデシリアライズ失敗は
`Corrupted` ではなく `Incompatible` へ落ちること ([settings_db.rs:2195](../../src/settings_db.rs:2195) 付近)。
`external_tools` を足したことでこの分類が変わっていないことをテストで固定する。

## 4. テスト (`cargo test -p mimageviewer --lib`)

最低限これらを書く。既存の settings_db テストの書き方 (in-memory / tempdir) に合わせること。

1. **round-trip**: 全 enum を非既定値にした 3 件を save → load して、順序と全フィールドが一致する。
2. **ID 安定**: 並べ替えて save → load しても各 `id` が変わらない。`next_id` が最大値 + 1 を返す。
3. **移行**: `custom_open_with_apps` に 2 行あり `external_tools` が空の DB を open すると、
   同じ順序で 2 件が `external_tools` に入り、`arguments == "{file}"`、
   `executable == Some(元の exe_path)`、`name == 元の display_name` になる。
4. **移行は一度きり**: 移行後に `external_tools` を編集して再 open しても、再移行されない。
5. **既に登録済みなら移行しない**: marker 無し + `external_tools` に行あり → 何も足さない。
6. **legacy テーブルを消さない**: 移行後も `custom_open_with_apps` の行が残っている。
7. **未知 variant**: `payload` に未知文字列を入れた DB は `Incompatible` になり `Corrupted` にならない。
8. **display_name**: `name` 空 + exe あり → file_stem。`name` 空 + exe 無し → 関連付け用の固定文字列。
9. **編集用の既定**: `defaults_for_editing()` が `Original` / `MainPageOnly` / `for_editing = true`。

## 5. 守ること

- **コミット前に `cargo fmt`** (引数なし、ワークスペース全体)。pre-commit hook が `--check` で弾く。
- テストは最小 target で回す: `cargo test -p mimageviewer --lib`。`--workspace` は不要。
- UI スレッドの同期 I/O 規約 ([docs/ui-responsiveness.md](../ui-responsiveness.md)) に触れる変更は
  この phase には無いはず。もし必要になったら**実装せず報告**すること。
- 症状パッチを入れない。既存の migration / Incompatible 判定の構造に乗せる。
- **範囲を広げない。** UI・起動処理・実体化は次の phase。必要だと思ったら報告だけする。
- 迷ったら正本 `docs/external-tool-launch-plan.md` を正とする。正本と食い違う指示が
  本ブリーフにあれば、それは私のミスなので**実装を止めて指摘すること**。

## 6. 完了報告に含めること

- 変更したファイルと、追加した関数・テストの一覧
- `cargo test -p mimageviewer --lib` の結果 (件数)
- 移行の marker をどこに置いたか
- 正本と食い違った点、判断に迷った点
