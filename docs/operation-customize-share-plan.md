# 操作カスタマイズ 共有・差分・世代取り込み 設計プラン

> ステータス: **設計中 / 未実装** (2026-07-08)。バックログ `docs/next-release-backlog.md` §4.6 の正本。
> 関連: [key-customization-impl-plan.md](key-customization-impl-plan.md) (keymap の型・解決・永続化)、
> [ring-shortcut-plan.md](ring-shortcut-plan.md) (リング / ジェスチャ)、
> [key-command-catalog-plan.md](key-command-catalog-plan.md) (メニュー構成 `MenuLayoutSettings`)、
> `src/settings_restore.rs` / `src/ui_dialogs/settings_restore.rs` (既存の設定復元)。

## 0. 目的と背景

操作カスタマイズ (キー割り当て・右ドラッグ/リング/マウス/ゲームパッド・メニュー構成) は、
時間をかけて作り込む設定なので、次を求める需要がある:

1. **共有**: 自分の操作カスタマイズを 1 ファイルに書き出し、他人に配れる (例:「○○ブラウザ風」
   プリセットの配布)。逆に他人のファイルを取り込める。
2. **差分確認**: ある設定が「標準」や「現在」からどれだけ変わっているかを表で見たい。
3. **巻き戻し**: 「なんか変な割り当てをしてしまった。2 日前の状態に戻したい」を、
   お気に入り・タグ・動画再開位置などを巻き込まずに、操作カスタマイズだけ戻したい。

### 現状 (調査結果)

- 操作カスタマイズは独立ファイルではなく `settings.db` の中の 3 フィールドに保存される
  (`Settings.keymap: KeymapSettings` / `Settings.ring_shortcuts: RingShortcutSettings` /
  `Settings.menu_layout: MenuLayoutSettings`)。3 つとも `serde` + `PartialEq/Eq` 対応。
- 操作カスタマイズダイアログの「保存」は `apply_operation_customize_state()`
  (`src/ui_dialogs/preferences.rs`) → `settings.save()` で、他設定と同じ経路。
- `settings.db` には世代バックアップ `settings.db.bak1..bak10` があり、
  設定メニュー「設定の復元…」(`src/ui_dialogs/settings_restore.rs`) で丸ごと復元できる。
  keymap もこの中に含まれる。
- **世代のローテーションはプロセス起動ごとに 1 回だけ** (`BACKUP_DONE_THIS_SESSION`,
  `src/settings.rs`)。つまり `bak1` = 今セッション開始時点、`bak2..bak10` = 過去 9 回分の
  セッション開始時点。同一セッション中の複数編集は世代に残らない。
- 現状、操作カスタマイズ専用のバックアップ / 差分 / エクスポート / インポート機能は **無い**
  (旧 `keymap.ini` は初回移行元として読むだけ)。

### 方針の要点 (ユーザー確定)

- **既存の設定バックアップ (settings.db 世代 + 設定の復元) は現状維持**。新しい永続化・
  スキーマ変更はしない。世代を **読むだけ** + ファイル入出力 + 純関数の差分で作る。
- 操作カスタマイズの共有・差分・世代取り込みは、既存「設定の復元」ダイアログに集約する
  (「設定の一括変更」という同種の概念なので 1 か所にまとめた方が分かりやすい)。
- 縦に長くなるので、ダイアログは **タブ切り替え** にする: `設定の復元` / `操作カスタマイズ`。
- **インポートは置換のみ** (マージしない)。共有プリセットは完成した 1 セットとして扱う。
- 世代からの取り込みも可能にする (2 日前の操作カスタマイズを現在へ適用)。

## 1. スコープ (共有・差分・取り込みの単位)

操作カスタマイズダイアログが編集する 3 点セットを 1 単位として扱う:

| フィールド | 型 | 内容 |
| --- | --- | --- |
| `keymap` | `KeymapSettings` | キー割り当ての上書き (`overrides: Vec<KeyBindingOverride>`) |
| `ring_shortcuts` | `RingShortcutSettings` | 右ドラッグ mode / リング / マウスジェスチャ / マウス戻る進む / ゲームパッド X リング |
| `menu_layout` | `MenuLayoutSettings` | top menu 順序 / メニュー内コマンド順序 / 非表示コマンド |

- エクスポート / インポート / 差分は常にこの 3 点まとめが対象。「キーだけ」の部分入出力は
  初版では作らない。
- 初版で差分表示の主役はキー割り当て。リング / メニューは要約セクション扱い (§4)。

## 2. 共有ファイル形式

人が読めて共有しやすい JSON。`KeyBindingOverride` は `action: String` / `chords: Vec<String>`
(例 `"Ctrl+E"`) で保持しているのでそのまま可搬。

```jsonc
{
  "format": "mimageviewer.operation-customize",
  "format_version": 1,
  "app_version": "2.2.0",          // 互換警告用 (schema_meta.app_version と同じ発想)
  "exported_at": "2026-07-08T12:34:56Z",
  "label": "○○ブラウザ風",          // 任意。取り込み画面のタイトルに出す
  "keymap": { ... },               // KeymapSettings をそのまま serialize
  "ring_shortcuts": { ... },
  "menu_layout": { ... }
}
```

- 拡張子は固有のもの (例 `.mivkeys.json`) にして取り違えを避ける。
- `label` はエクスポート時にユーザー入力 (省略可)。
- `format_version` を最初から持たせ、将来のフィールド追加に備える。読み込み時に
  未知バージョンなら警告して可能な範囲で取り込む。
- `app_version` は互換警告用。取り込みを拒否する材料にはしない (未知アクションは §3 の
  warn-and-skip で吸収する)。

## 3. 取り込み (インポート / 世代取り込み) の意味論

**取り込み元は「ファイル」でも「settings.db の世代」でも良い**。どちらも
`OperationCustomizeBundle` (§5) に正規化してから同じ経路に流す。

- **置換方式 (replace)**: 取り込んだ 3 点セットで現在の操作カスタマイズを丸ごと差し替える。
  マージはしない。`overrides` は「標準からの差分」なので、置換 = 「このファイル/世代の
  overrides を採用し、それ以外は標準に戻す」という予測しやすい結果になる。
- **適用前に差分プレビュー**: 「取り込み元 vs 現在の設定」を §4 の差分ビューで見せてから確定。
  他人のファイルや古い世代を盲目的に適用させない。
- **未知要素は warn-and-skip**: 未知アクション名 / 変換不能キー / バージョン不一致は、
  keymap パーサ既存の warnings 収集 (`Keymap::from_ini_str` / `KeymapSettings::import_legacy_ini_if_needed`
  と同系統) に載せ、取り込み自体は続行。プレビューに「無視した項目 n 件」を表示。
- **ライブ適用 (再起動不要)**: 適用は既存の `apply_operation_customize_state()` 相当を通す
  (`settings.keymap` 等を差し替え → `Keymap::from_settings` 再構築 →
  `install_global_native_video_shortcuts()` → `settings.save()`)。
  **設定全体の復元と違い、アプリ終了は不要**。ここが「丸ごと復元 (要再起動)」との明確な差で、
  タブとしても別に分けて区別する。
- **取り消し用の自動退避**: 適用直前に、現在の操作カスタマイズをエクスポートと同じ形式で
  `data_dir/operation-customize.before-import-<unix秒>.mivkeys.json` に自動保存する。
  変なファイル/世代を入れても、この退避ファイルを再取り込みすれば戻せる
  (世代 bak のローテ都合に依存しない確実な undo 経路)。退避は最新数世代だけ残す。

## 4. 差分の意味論

### キー割り当て = 実効チョード (effective chords) 単位で比較

実効チョード = 上書きがあれば上書き、無ければ `KeyAction::default_chords()`
(`Keymap::from_settings(&KeymapSettings).effective_chords(action)` で取得)。

各 `KeyAction` について 2 つの設定 A / B の実効チョードを比べ、異なるものだけ表に出す:

| 分類 | 条件 | 表示 (変更前 → 変更後) |
| --- | --- | --- |
| 追加 | A 空 → B 有 | (なし) → `新キー` |
| 削除 (無効化) | A 有 → B 空 | `旧キー` → (なし) |
| 変更 | 双方有・内容違い | `旧` → `新` |

- 表の列: **コマンド (context + 表示名) / 変更前 / 変更後**。`ini_name()` / `description()` /
  `context()` があるので日本語ラベルにできる。
- 「空」= 有効チョードなし (標準がそもそも無割り当て、または `none` で無効化)。

### リング / メニューは要約セクション

`RingShortcutSettings` / `MenuLayoutSettings` は構造が違うので、キー表とは別に小さな
要約セクションにする (例:「リング/マウス: 変更 n 件」「メニュー構成: 変更あり」+ 展開で詳細)。
初版はキー表を主役にし、リング/メニューは「変更あり/なし + 件数」から始めて、詳細表示は
段階的に足す。

### 比較対象マトリクス (行/取り込み元ごとに選べる)

| 対象 | vs 標準 (デフォルト) | vs 現在の設定 | vs 前世代 |
| --- | :--: | :--: | :--: |
| 各 bak 世代 | ○ | ○ (= 取り込みプレビュー相当) | ○ (※空が多い) |
| 現在の設定 | ○ | — | ○ (= セッション開始時点との差) |
| 取り込むファイル | ○ | ○ (= 適用プレビュー) | — |

- 「前世代との差分」は世代ローテが起動ごとのため **変更なしになりがち**。既定表示は
  **vs 標準**、取り込みプレビュー用途では **vs 現在** を使う。

## 5. モジュール構成 (新規 / 変更)

### 新規: 純ロジック (`src/operation_customize_share.rs` 想定)

- `OperationCustomizeBundle { keymap: KeymapSettings, ring_shortcuts: RingShortcutSettings,
  menu_layout: MenuLayoutSettings }` + ヘッダ meta。`serde`。
  - `Bundle::from_settings(&Settings) -> Bundle`
  - `Bundle::apply_to(&self, &mut Settings)` (置換)
  - `Bundle::defaults() -> Bundle`
- `to_json(&Bundle) -> String` / `parse_json(&str) -> ParsedImport { bundle, warnings }`
- `diff(a: &Bundle, b: &Bundle) -> OperationDiff` (キー表 + リング要約 + メニュー要約)。
  キー表は `KeyAction::all()` を回し、`effective_chords` を A/B で突き合わせる。
- すべて App 非依存の純関数 (unit test しやすい。CLAUDE.md の方針)。

### 変更: `src/settings_restore.rs`

- `load_operation_customize(data_dir, source: &BackupSource) -> Result<Bundle, RestoreError>`
  を追加。既存 `validate_in_dir` と同じ「bak を temp コピー → `SettingsDb::open` →
  `load_into_settings` → 3 フィールド抽出」パターンを流用 (world 状態を汚さず read-only)。
- `Current` は稼働中の `Settings` から直接 `Bundle::from_settings` で取れる (App 側で対応)。

### 変更: `src/ui_dialogs/settings_restore.rs`

- ダイアログをタブ化 (§6)。`操作カスタマイズ` タブの一覧描画・差分モーダル・
  ファイルダイアログ (`rfd`) 配線・取り込みのライブ適用 (`apply_operation_customize_state`
  相当を呼ぶ) を追加。

### 再利用 (ほぼ変更なし)

- `src/keymap.rs`: `KeyAction::all()` / `default_chords()` / `description()` / `context()` /
  `Keymap::from_settings` / `effective_chords` / warnings 収集。
- `rfd`: 既存のエクスポートで使っているネイティブファイルダイアログ経路。

## 6. UI (設定の復元ダイアログをタブ化)

```
[設定の復元]  ← メニュー「設定の復元…」から開く
┌───────────────────────────────────────────────┐
│ ( 設定の復元 )  ( 操作カスタマイズ )   ← タブ  │
├───────────────────────────────────────────────┤
│ タブ1: 設定の復元 (既存のまま)                  │
│   世代 / 日時 / サイズ / お気に入り / タグ /     │
│   動画再開 / 操作[この時点に戻す…]              │
│   ... 設定を完全リセット…                       │
│   ※ 設定全体を差し替え・要アプリ終了            │
├───────────────────────────────────────────────┤
│ タブ2: 操作カスタマイズ (新規)                  │
│   世代一覧 (現在 + bak1..bak10):                │
│     各行 [差分を見る…] [書き出す…] [取り込む…] │
│   下部: [ファイルから読み込む…]                 │
│   ※ 操作カスタマイズだけ・再起動不要と注記      │
└───────────────────────────────────────────────┘
```

`操作カスタマイズ` タブの行アクション:

- **差分を見る…**: 比較対象 (標準 / 現在 / 前世代) を選べるサブモーダルで §4 の表を表示。
- **書き出す…**: その世代 (現在含む) の操作カスタマイズを `.mivkeys.json` に保存 (`rfd` save)。
  主用途は「現在」行 = 自分の設定を共有。
- **取り込む…**: その世代の操作カスタマイズを現在へ適用 (置換 + プレビュー + ライブ適用 +
  自動退避)。「現在」行では出さない (自分自身なので no-op)。

下部の全体アクション:

- **ファイルから読み込む…**: `rfd` pick → パース + 警告収集 → 「ファイル vs 現在」プレビュー →
  [取り込む]/[キャンセル]。取り込み = ライブ適用。

補足:

- `操作カスタマイズ` タブは「操作カスタマイズだけを扱う・再起動不要」と明示注記し、
  `設定の復元` タブ (全体・要再起動) と役割を分ける。
- **編集ダイアログ (メニュー「操作カスタマイズ…」) との区別**: あちらは *編集*、
  このタブは *共有 / 差分 / 世代取り込み*。混同を避けるため、編集ダイアログ側に
  「共有・差分…」ボタンを置いてこのタブへ誘導するのは任意 (初版は省略可)。
- **遅延ロード**: 各世代の Bundle 抽出は行アクション押下時に初めて bak を read-only で開く。
  ダイアログ起動時に全世代をロードしない (一覧は既存の軽い集計のまま)。

## 7. 実装フェーズ

| Ph | 内容 | 触る所 | 規模 |
| --- | --- | --- | --- |
| 1 | 純ロジック (`OperationCustomizeBundle` / JSON 入出力 / `diff`) + unit test | 新 `operation_customize_share.rs` | 中 |
| 2 | 世代/現在からの Bundle 抽出 (`load_operation_customize`) + エクスポート | `settings_restore.rs` / UI | 小〜中 |
| 3 | 差分ビュー (vs 標準/現在/前世代) モーダル | UI | 中 |
| 4 | 取り込み (ファイル + 世代): プレビュー → ライブ適用 → 自動退避 → 警告 | UI + App glue | 中 |
| 5 | 設定の復元ダイアログのタブ化・注記・遅延ロード統合 | `ui_dialogs/settings_restore.rs` | 中 |
| 6 | マニュアル/製品ページ更新 (共有機能)、glyph lint、`cargo fmt`、追加テスト | docs / htdocs | 小 |

- 各フェーズ完了時点でビルド通過 & 既存挙動維持。Ph1〜Ph3 (読み取り系) までは
  既存設定を一切書き換えないので安全に段階出荷できる。取り込み (Ph4) から書き込みが入る。

## 8. テスト計画

`cargo test --bin mimageviewer-core` (App 系は `--lib` に出ない: MEMORY 参照)。

- **JSON ラウンドトリップ**: `to_json` → `parse_json` で Bundle が一致。`format_version` /
  未知フィールドの前方互換。
- **差分の分類**: 追加 / 削除(無効化) / 変更 / 変更なし を effective chords で正しく判定。
  上書きあり/なし、`none` 無効化、複数チョードの順序差を代表ケースにする。
- **未知アクション warn-and-skip**: 未知 action 名 / 変換不能キーを含むファイルを取り込むと、
  warnings に載り、既知分だけ適用される。
- **置換セマンティクス**: 取り込み後の `Settings.keymap` が「元の overrides を捨てて
  取り込み元の overrides に置き換わる」こと。他フィールド (お気に入り等) が無傷なこと。
- **世代抽出**: `load_operation_customize(Bak(n))` が bak から 3 フィールドを取り出せる。
  壊れた bak は `RestoreError` で失敗し、稼働中の状態を汚さない。
- **自動退避**: 取り込み前に `before-import-*` が作られる。再取り込みで元に戻せる。
- **リング/メニュー要約**: 変更あり/なしと件数が正しい。
- **スナップショット**: タブ UI / 差分表を 1〜2 枚追加 (`UPDATE_SNAPSHOTS=1`)。

## 9. 永続データ / マイグレーション / 凍結ルール

- **マイグレーション不要**: `settings.db` のスキーマは変更しない (既存世代を読むだけ +
  ファイル入出力)。共有ファイルは初版から `format_version` を持たせて将来互換に備える。
  (CLAUDE.md「永続データ・スキーマ変更時の判断」= スキーマ非変更なので該当なし。)
- **detached viewer 凍結ルールとは無関係**な領域 (設定 UI / keymap 永続化) なので、
  着手制約なし。
- **ドキュメント / マニュアル**: ユーザー向け (`htdocs/mimageviewer/manual/` +
  `index.html`) には内部用語 (型名 / DB 名 / serde 等) を出さず、「操作カスタマイズを 1 つの
  ファイルに書き出して共有できます」「過去の状態と見比べて、操作カスタマイズだけ戻せます」
  の粒度で書く (CLAUDE.md 記述方針)。keymap 系設計ドキュメントの索引 (`docs/README.md`) と
  バックログ (`docs/next-release-backlog.md` §4.6) を本書とリンクさせる。

## 10. 決定済み事項 (このプランの前提)

- タブ化 (`設定の復元` / `操作カスタマイズ`)。縦積みにしない。
- インポートは置換のみ (マージなし)。
- 世代からの取り込みを可能にする (2 日前の操作カスタマイズを現在へ、再起動なしで適用)。
- 差分・エクスポート・インポートはすべて「設定の復元」ダイアログに集約 (編集は従来の
  「操作カスタマイズ…」ダイアログのまま)。

## 11. 未決 / 実装時に確定する細部

- 共有ファイルの拡張子・MIME 的な扱い (`.mivkeys.json` 案)。二重クリック関連付けはしない。
- リング / メニュー差分の詳細表示をどこまで作り込むか (初版は件数 + 変更有無から)。
- 編集ダイアログからこのタブへの導線ボタンを付けるか (初版は任意)。
- 自動退避 (`before-import-*`) の保持世代数と掃除タイミング。
- 取り込みプレビューで「無視した項目」をどこまで詳細表示するか。
