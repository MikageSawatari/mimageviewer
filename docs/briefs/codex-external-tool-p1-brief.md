# ブリーフ: 外部ツール起動 P1 — 登録 UI・引数展開・起動

正本: [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md)。**着手前に §4.1 / §4.4 / §4.7 /
§4.10 / §4.11 を読むこと。** P0 ([codex-external-tool-p0-brief.md](codex-external-tool-p0-brief.md)) で
入った `ExternalTool` 型と `Settings.external_tools` の上に載せる。

作業場所: この worktree (`C:\home\mimageviewer-extlaunch`, ブランチ `external-tool-launch`)。

## 0. P1 の範囲

**実ファイル 1 件に対して、登録したツールを引数付きで起動できるところまで**を通す。
これで実機確認ができる状態にする。

**やること**

1. 環境設定「起動と連携 > 外部ツール」ページ (追加 / 編集 / 複製 / 上下移動 / 削除)
2. 引数テンプレートの展開 (トークン分割 → 置換 → 空トークン除去)
3. プロセス起動と、**失敗の通知**
4. 既存 `open_with.rs` の不具合修正 (`OsStr` 化、`spawn()` の `Err` 破棄)
5. 関連付けアプリ列挙 (`SHAssocEnumHandlers`) のワーカー化

**やらないこと** (後続 phase)

- 仮想ページ・動画フレームの一時実体化 (P3 / P4)。`{container}` / `{entry}` / `{page}` /
  `{time}` の展開もこの phase では**まだ実装しない** (P4)。
- コンテキストメニュー / ツールバー / キー割り当てへの導線 (P1b・P2)
- 複数選択 (`SelectionPolicy`)、見開き (`SpreadPolicy`) の適用 (P2 / P4)

この phase の対象は**単一の実ファイル**だけ。`PayloadPolicy` が `Container` / `RealFileOnly` /
`AsDisplayed` のどれでも、実ファイル 1 件なら渡すものは同じなので分岐は要らない。
仮想ページが対象のときは**まだ起動できない**ので、その旨を返す `Err` を用意して呼び出し側で通知する。

## 1. 引数の組み立て (`src/external_tool.rs` に純関数で置く)

**正本 §4.4 のとおり、文字列を組み立ててから渡すのではなく、先にトークン列へ分割してから
各トークン内で置換する。** mIV は `Command` に `OsString` 引数列を渡すので、この順序なら
置換値に空白が含まれても壊れず、利用者が引用符を書く必要も無い。

```rust
pub enum ArgToken { Literal(String), /* 置換後に空なら落とす */ }
pub fn split_argument_template(template: &str) -> Vec<String>;
pub fn expand_arguments(tokens: &[String], ctx: &PlaceholderContext) -> Vec<OsString>;
```

- 分割規則は **`CommandLineToArgvW` 互換**。`"..."` でくくった中の空白は分割しない。
  `""` によるエスケープも受ける。分割の単体テストを厚く書くこと。
- この phase で展開するのは `{file}` / `{dir}` / `{name}` / `{stem}` / `{ext}` / `{uri}` の 6 つ。
  残りは P4。**未実装の記法はエラーにせず、空文字として扱う** (トークンごと落とす規則に乗る)。
- **キーワードを 1 つも含まないテンプレートには `{file}` を自動で追加する** (正本 §4.4)。
  「引数を書き換えたらファイルが渡らなくなった」事故を防ぐ。NeeView と同じ。
- **空に展開されたプレースホルダを含むトークンは、そのトークンごと落とす。**
  `-page {page}` を PDF 以外に使ったときに `-page` だけが残らないようにする。
- 未知の `{...}` は**そのままリテラルとして残す** (Windows のパスに `{}` が入り得るため、
  勝手に消さない)。この判断をテストで固定する。

## 2. 起動 (`src/external_tool.rs` または `src/open_with.rs`)

- `Command::new(exe)` + `OsString` 引数列。**`cmd /c` を経由しない** (正本 §4.11)。
- `executable: None` のツールは OS の関連付けで開く。既存の
  [`ui_helpers.rs:1613`](../../src/ui_helpers.rs:1613) の `opener::open()` 経路を再利用する。
  この場合、引数テンプレートは使えないので **UI 側で入力を無効化**すること。
- `working_directory` があれば `Command::current_dir()`。
- `CREATE_NO_WINDOW` は現行どおり維持する。
- **標準入出力は継承しない** (`Stdio::null()`)。
- **`spawn()` の `Err` を必ず通知する。** 文面は「ツール名 + OS エラー」。
  現行 [`open_with.rs:141`](../../src/open_with.rs:141) は `let _ = cmd.spawn();` で捨てているので、
  これは既存不具合の修正でもある。通知は既存の toast (`show_feedback_toast`) に合わせる。
- **パスを `to_string_lossy()` しない。** 現行 [`open_with.rs:143`](../../src/open_with.rs:143) は
  Windows でも `String` 化して `/` を `\` に置換している。`OsStr` のまま扱うよう直す。
  ただし `{file}` をテンプレートへ埋める都合上、置換対象は `OsString` の連結で組み立てること
  (`Path` → `OsString` → push の形。UTF-8 を経由しない)。

### ネットワークパスの確認

EXE パスが `\` で始まる場合は**確認してから**起動する (正本 §4.11)。
存在確認は UI スレッドで行わない (下記 §4)。

## 3. 環境設定ページ

`PreferencesPage::ExternalTools` を追加し、[`preferences.rs:396`](../../src/ui_dialogs/preferences.rs:396)
の `TREE` の「起動と連携」カテゴリへ `PreferencesPage::Startup` の隣に入れる。
描画は `preferences/pages.rs` に `page_external_tools` として追加する。

**⚠ 検索索引が必須。** `search_index.rs` のテストが
「全ページが少なくとも 1 つの索引 entry を持つ」ことを検査するので、
`PREF_SEARCH_INDEX` へ entry を足さないと `cargo test` が落ちる。
pages.rs 側に `anchored("external-tools/...")` を置き、entry の `title` は
pages.rs の表示文字列と**完全一致**させること (これもテストで検査される)。
キーワードには「外部ツール」「アプリケーションで開く」「open with」「関連付け」等を入れる。

### 画面

- 一覧 (表)。列は「名前」「実行ファイル」「引数」程度。行選択で下または右に編集欄。
- ボタン: 追加 / 複製 / 上へ / 下へ / 削除。既存例に倣う
  (VST ページ [pages.rs:6521](../../src/ui_dialogs/preferences/pages.rs:6521)、
  Creative LUT ページ [pages.rs:6358](../../src/ui_dialogs/preferences/pages.rs:6358))。
- **追加の入口は 2 つ**: 「実行ファイルを選ぶ」(既存 `open_with::pick_exe_dialog`) と
  「関連付けアプリから選ぶ」(既存 `open_with::enumerate_handlers`、下記 §4 でワーカー化)。
- 編集欄の項目は正本 §4.1 の全フィールド。`payload` / `video` / `spread` / `selection` は
  ComboBox。`pdf_render_long_edge` は「表示相当 / 2048 / 4096 / 8192」の選択肢
  (正本 §6 決定済み 5。**既定は 4096**)。
- **用途の選択**を追加時に出す。「表示に使う」/「編集に使う」で既定値を出し分ける
  (`ExternalTool::defaults_for_viewing()` / `defaults_for_editing()`、正本 §4.1)。
- 引数欄の下に**プレースホルダ一覧**を出す。この phase で未実装のものは
  「(準備中)」等を付けず、**一覧に載せない** (使えない記法を案内しない)。
- **`executable` が `None` (関連付け) のツールは、引数・作業フォルダー欄を無効化**する。

### IME

引数・名前・作業フォルダーは日本語入力があり得る **single-line TextEdit** なので、
必ず `crate::ime_focus` の helper 経由で描画すること (CLAUDE.md「IME 対応」)。
raw `TextEdit` は allowlist を検査する unit test が禁止している。
Enter / Escape を拾うなら `dialog_enter_pressed` / `dialog_escape_pressed` を使う。

## 4. UI スレッドの同期 I/O を持ち込まない

[docs/ui-responsiveness.md](../ui-responsiveness.md) §4 のチェックリストを通すこと。
この phase で該当するのは次の 3 つ。**いずれも worker + mpsc + キャンセルへ移す。**

1. **関連付けアプリの列挙** (`SHAssocEnumHandlers`)。現行は
   [`context_menu.rs:2087`](../../src/ui_dialogs/context_menu.rs:2087) からメニュー描画経路で同期実行
   されている。設定ページの「関連付けアプリから選ぶ」では**必ず worker** で列挙し、
   結果が届くまでは待機表示にする。
   **現行のコンテキストメニュー側は、この phase では触らない** (P1b で扱う)。
2. **EXE / 作業フォルダーの存在確認**。特にネットワークパスは応答しないことがある。
   保存時ではなく、**編集欄でパスが変わったときに worker で確認**し、結果を注記として出す。
   確認できていないことを理由に保存を止めない。
3. **起動そのもの** (`Command::spawn`)。ネットワーク上の EXE では spawn 自体が待たされ得るので、
   worker で行い、結果 (`Ok` / `Err`) をチャネルで受けて通知する。

定型パターン (`XxxPending { cancel, rx }` + `start_xxx` / `poll_xxx`) は
[docs/ui-responsiveness.md](../ui-responsiveness.md) §2 のテンプレに従う。

## 5. テスト

- `split_argument_template`: 空白 / 引用符 / `""` エスケープ / 連続空白 / 末尾引用符欠落。
- `expand_arguments`: 各プレースホルダ、空展開でトークンごと落ちること、
  未知の `{...}` がリテラルとして残ること、キーワード無しテンプレートに `{file}` が足されること、
  置換値に空白が含まれても 1 引数のままであること。
- 用途別の既定値 (`defaults_for_viewing` / `defaults_for_editing`)。
- 環境設定の索引テストが通ること (`cargo test -p mimageviewer --lib preferences`)。
- 起動そのものはテストしない (プロセスを起こさない)。**引数列を組み立てるところまでを純関数にし、
  そこをテストする**設計にすること。

## 6. 守ること

- コミット前に `cargo fmt` (引数なし)。
- テストは `cargo test -p mimageviewer --lib`。
- UI 文言に環境依存グリフを使わない。追加したら `python scripts/check_ui_glyphs.py` を通す
  (CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」)。
- 見た目を変えたので、スナップショットテストが落ちたら
  [docs/ui-snapshot-policy.md](../ui-snapshot-policy.md) の手順に従う。**勝手に `UPDATE_SNAPSHOTS=1`
  で上書きせず、差分の理由を報告すること。**
- 範囲を広げない。コンテキストメニュー・キー割り当て・実体化は後続 phase。
- 正本と本ブリーフが食い違ったら、実装を止めて指摘すること。

## 7. 完了報告に含めること

- 変更ファイル一覧、追加した関数とテスト、テスト結果の件数
- worker 化した 3 箇所の実装場所
- 実機で確認すべき操作 (利用者に渡す手順)
- 正本と食い違った点、迷った点
