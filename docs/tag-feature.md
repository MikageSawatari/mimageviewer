# タグ機能 設計ドキュメント

> **現行実装メモ (2026-06-12)**:
> タグ正本は `tags.db` に移行中です。通常タグ操作はメディア本体 / XMP サイドカー /
> Tantivy `tags` フィールドへ書き込まず、`tags.db:item_tags` だけを更新します。
> Ctrl+G/Ctrl+F のタグ検索ソースも閉じられており、タグでの絞り込みは facet の
> タグフィルタが担当します。旧 XMP `dc:subject` ベースの記述は v1.0 仕様の履歴として
> 残しています。新仕様の詳細は [tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md) を参照。

mimageviewer に「ハッシュタグ型」の分類タグ機能を追加する。
Ctrl+G 検索の絞り込みキーとして、ユーザーがあらかじめ登録したタグを
画像ファイルに付与／削除できるようにする。

## 1. スコープ

### 対象
- 通常画像ファイル (`.jpg` / `.jpeg` / `.png` / `.webp`) — **本体に XMP 埋め込み**
- 動画ファイル (`SUPPORTED_VIDEO_EXTENSIONS` と同じ集合: `.mp4` / `.mkv` / `.mov` / `.avi` / `.wmv` / `.mpg` / `.mpeg`)
  — **同名 `.xmp` サイドカーファイル** に書き込み (Lightroom / Bridge / Premiere と互換)
- 主要なユースケース: **AI 生成画像、写真、XMP ツイート情報付き画像、動画ライブラリ**

### 非対象 (v1.0)
- **ZIP 内画像 / PDF ページ** — アーカイブの再構築が必要、かつ ZIP/PDF ファイル自体を
  後で移動するとタグ情報の紐付けが消えるため、機能そのものを無効化する。UI 上で
  タグ付与操作を選択不可 (グレーアウト) にする。
- **HEIC / HEIF** — Rust エコシステムに安全な XMP 書き込み手段が現時点で存在しない
  (§8 参照)。v1.0 では読み取りのみ、書き込みは非対応。v1.1 候補。
- **RAW** — 現像ソフト前提のワークフローでタグ付けするユースケースが希薄。v1.0 では
  書き込み対象外 (読み取りは既存 XMP 経路で行われる)。
- **TIFF / GIF / BMP / JXL / AVIF** — 対象外。UI でタグ付与操作を選択不可にする。

### 動画はサイドカー方式
動画ファイル本体 (avformat の re-mux) を書き換えると、4K/HEVC で数 GB 規模の書き直しに
なり実用的でない (書き込み中の電源断でファイル破損リスクも大きい)。動画では Adobe
Lightroom / Bridge / Premiere が標準採用している方式と互換な、同名 `.xmp` サイドカー
(`video.mp4` → `video.mp4.xmp`) を動画と同じディレクトリに配置する形式で保存する。

サイドカーファイルの中身は通常の XMP packet (= `<x:xmpmeta>...</x:xmpmeta>` を `<?xpacket?>`
で囲んだもの) なので、画像の埋め込み XMP に対する `dc:subject` 編集ロジック (`edit_xmp_packet`)
をそのまま再利用できる。差分はコンテナ I/O だけ:

- 読み取り: 動画パスの場合、ファイル本体ではなく `<path>.xmp` を直接読む
- 書き込み: 動画パスの場合、ファイル本体を触らず `<path>.xmp` にアトミック書き込み
- ファイルが無い場合: `MINIMAL_XMP_TEMPLATE` から packet を構築して新規作成

サイドカーが分離する (= 動画ファイルだけを別フォルダに移動する) と mIV からはタグが
見えなくなる。マニュアルで「動画移動時は `.xmp` も一緒に動かす」を明記する運用とする。

### 非機能要件
- **既存メタデータを破壊しない** — 既存の `dc:subject` 要素、他の XMP プロパティ、
  EXIF、AI メタ (A1111 parameters)、XMP ツイート情報 (`xtw:*`) を絶対に触らない
- **アトミック書き込み** — 一時ファイル + `rename` で電源断耐性
- **バックアップは用意しない** — 編集内容が限定的なので省略。ユーザーには
  ダイアログで「ファイル内容を書き換えます」と明示する
- **UI スレッドをブロックしない** — 書き込みは worker で実行

## 2. ユーザーシナリオ

### 2.1 タグを登録する
1. メニュー「タグ」→「タグを編集…」でタグ編集ダイアログを開く
2. 「原神」「ドール」「プリキュア」等を追加
3. ダイアログ初回オープン時に「タグを付与するとファイル内容を書き換えます」
   警告を 1 度だけ表示 (チェックで再表示抑制)
4. OK で保存 → メニューとツールバーに「タグ: 原神 ドール プリキュア」として並ぶ

### 2.2 タグを付与する
1. グリッドで画像を選択 (複数選択可)
2. メニューまたはツールバーで「原神」をクリック
3. 選択中の全ファイルの XMP `dc:subject` に `#原神` を Bag 要素として追加
4. 進捗は左下のステータスラインに「タグ付与中 5/12」のように表示
5. 完了後、検索インデックスにも反映される (次回 Ctrl+G から `#原神` で検索可能)

### 2.3 トグル (複数選択時の挙動)

| 選択中ファイルの状態 | クリック時の動作 |
|---|---|
| 全てに `#原神` が付与済み | 全てから `#原神` を削除 |
| 一部のみ付与済み、または全て未付与 | 全てに `#原神` を付与 |

単一選択ならシンプルにトグル (付与済みなら削除、未付与なら付与)。

### 2.4 タグをすべてクリア
- メニュー「タグ」→「タグをすべてクリア」
- 選択中ファイルの `dc:subject` から `#` で始まる要素を削除
- `#` が付かない既存要素 (他ソフトで付けたタグ) は一切触らない
- **他ソフトがたまたま `#` で始まるタグを付けていた場合は巻き込んで削除する**
  (この挙動をダイアログで明示)

### 2.5 検索で絞り込む
- Ctrl+G を開く
- `#原神` と入力 → `dc:subject` に `#原神` 要素を持つ画像のみヒット
- `#原神 #ドール` で AND 検索
- `-#原神` で除外

## 3. データモデル

### 3.1 XMP `dc:subject` への書き込み規約

XMP パケットの `dc:subject` プロパティは RDF Bag (順不同集合) 型で、
複数のタグを `rdf:li` 要素として列挙する。mimageviewer が追加する
タグは **必ず `#` プレフィックス付き**:

```xml
<dc:subject>
  <rdf:Bag>
    <rdf:li>既存タグ1</rdf:li>          <!-- 他ソフト由来 → 触らない -->
    <rdf:li>Photographer</rdf:li>       <!-- 他ソフト由来 → 触らない -->
    <rdf:li>#原神</rdf:li>              <!-- mIV 追加 → クリア対象 -->
    <rdf:li>#風景</rdf:li>              <!-- mIV 追加 → クリア対象 -->
  </rdf:Bag>
</dc:subject>
```

- **付与時**: タグ名 `原神` → Bag に `#原神` を 1 要素として追加。既存要素は保持。
  同じ要素が既にあれば追加しない (冪等)。
- **削除時**: `#原神` 要素を Bag から取り除く。他要素には触らない。
- **すべてクリア時**: Bag 内で `#` で始まる要素のみ取り除く。

### 3.2 `dc:subject` 以外の書き込み先

- **IPTC Keywords (2:25)** — v1.0 では書き込まない。Lightroom / Bridge が読むなら
  XMP 経由で互換性は確保される。将来 `xmpMM` ↔ IPTC 同期オプションを検討。
- **Exif XPKeywords (0x9C9E)** — v1.0 では書き込まない。Windows エクスプローラーが
  `dc:subject` を読むので不要。
- **`lr:hierarchicalSubject`** — 階層タグは v1.0 非対応。将来オプションで書き出し互換を検討。

### 3.3 タグ名のルール

- **使用可能文字**: `#` と空白を除く任意の Unicode 文字 (日本語可)
- **先頭文字**: `#` は使えない (mimageviewer 側で自動付与)
- **空白**: 使えない (タグビューの複数タグ AND 検索で区切りとして使うため)
- **長さ**: 1〜64 文字
- **大文字小文字**: タグ定義時の表記を保存、検索時は **大文字小文字を区別しない**
  (既存 Ctrl+G の慣例に合わせる)
- **重複**: タグ定義内で同名タグを禁止 (ダイアログでバリデーション)

### 3.4 タグ定義の永続化 (Settings)

[src/settings.rs](../src/settings.rs) の `Settings` に以下を追加:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TagDef {
    pub id: Uuid,        // 並べ替え・削除時の安定識別子
    pub name: String,    // 表示名 (プレフィックス `#` なし)
}

pub struct Settings {
    // ...既存フィールド
    #[serde(default)]
    pub tags: Vec<TagDef>,

    #[serde(default = "default_show_toolbar_tags")]
    pub show_toolbar_tags: bool,

    #[serde(default)]
    pub tag_write_warning_acknowledged: bool,  // 初回警告の既読フラグ
}
```

`FavoriteEntry` と同じ UUID パターン。`#[serde(default)]` で後方互換性を確保
(旧 settings.json から読み込むときは空配列)。

## 4. UI 仕様

### 4.1 タグ編集ダイアログ

[src/ui_dialogs/favorites_editor.rs](../src/ui_dialogs/favorites_editor.rs) を
モデルケースに `src/ui_dialogs/tag_editor.rs` を新設。

**構成** (上から順に):
1. 警告文 (常時表示): 「タグを付与すると画像ファイルの XMP メタデータを書き換えます。
   書き換えに失敗した場合、ファイルが破損する可能性があります」
2. 「了解しました。次回から警告を表示しない」チェックボックス
   (= `tag_write_warning_acknowledged = true`)
3. タグ一覧テーブル
   - 各行: 名前入力欄、並べ替え用ドラッグハンドル (↑↓ ボタンで代用可)、削除ボタン
   - 空行が末尾に 1 つ自動で追加される (Lightroom 風のインライン追加)
4. 下部: OK / キャンセルボタン

**バリデーション** (OK 押下時):
- 空文字列のタグは無視して保存
- 同名タグがある場合はエラー表示、保存しない
- `#` を含む名前はエラー

**IME 対応**: 必ず `dialog_enter_pressed` / `dialog_escape_pressed` を使う
(CLAUDE.md の規約)。

### 4.2 メニュー

[src/ui_main.rs:23-107](../src/ui_main.rs) の `render_menubar()` に
「タグ」メニューを追加 (お気に入りメニューの直後):

```
タグ
├─ タグを編集…
├─ ─────────
├─ すべてクリア (選択中のファイル)
├─ ─────────
├─ 原神              ← 選択中に全付与されていればチェックマーク表示
├─ ドール
├─ プリキュア
└─ アニメ
```

各タグ項目クリックで 2.3 のトグルロジックを実行。選択ファイルが無い場合は
タグ項目をグレーアウト。

### 4.3 ツールバー

[src/ui_main.rs:288-450](../src/ui_main.rs) の `render_toolbar()` に
「タグ: 原神 ドール プリキュア アニメ」セクションを追加。

- 「タグ:」ラベル + 各タグをボタンとして横並び (お気に入りと同じレイアウト)
- ボタンはクリックで 2.3 のトグル、現在の選択に全付与されていればハイライト
- セクション表示オン/オフは `show_toolbar_tags` フラグで制御
- 設定は `toolbar_settings.rs` の「ツールバー項目」チェックリストに追加

### 4.4 メタデータパネル (フルスクリーン)

[src/ui_metadata_panel.rs](../src/ui_metadata_panel.rs) に「タグ」セクションを追加。

現在はフルスクリーン右パネルからも mIV タグを付与 / 解除できる。タグの正本は
`tags.db` で、通常タグ操作ではメディア本体 / XMP / 検索索引へ書き込まない。

- 単ページ画像: 既存の `Image Info` 右パネル最上段にタグボタンを表示する。
- ZIP 内画像 / PDF ページ / 変換アーカイブ内ページ: 仮想ページではなく親の
  ZIP / PDF / 元アーカイブをタグ対象にし、パネルに `タグ対象: この本 (book.zip)` のように表示する。
- 通常画像フォルダ: `フォルダ` 行は画像フォルダ本体、`ページ` 行は単ページなら現在の画像、
  見開きなら表示中の左右 2 ファイルを対象にする。見開きで片側だけ付与済みのタグは mixed 状態で表示し、
  クリックすると左右両方へ付与する。
- 動画: native overlay の `動画メタ情報` 右パネル最上段に同じタグセクションを表示する。overlay 側は
  DB を直接読まず、App から渡された `tags_cache` / ピン留めタグ / タグ候補カタログを表示し、クリックは
  `NativeOverlayCommand::ToggleTag` / `AddTag` / `RemoveTag` / `OpenTagViewForTag` で App 側へ戻す。

表示するタグボタンは以下の順で組み立てる:

- ピン留めタグ: 常時表示する。
- 現在の対象に付いている未ピン留めタグ: ピン留めされていなくても表示する。
- 右パネルを開いている対象で一度表示した未ピン留めタグ: その対象を見ている間は、削除後も OFF 状態のボタンとして残す。

各タグ行の `＋` ボタンから、タグを検索 / 入力して明示的に付与できる。静止画の右パネルでは
フォルダ行とページ行の `＋` はそれぞれの対象にだけ作用する。動画フルスクリーン / F12 別ウィンドウの
native overlay では OS/egui の別ダイアログを重ねず、同じ右パネルをタグ選択ビューへ切り替える。
入力欄が空のときは候補を `ピン留め` / `最近` タブで切り替える。`最近` は最後に付与した時刻
(`last_applied_at`) の降順で、タグを外しただけでは更新しない。入力中はタブより検索を優先し、
タグ候補カタログ全体から前方一致で絞り込む。`付ける` / `外す` を実行するとタグ選択ビューを閉じ、
元のメタデータ表示へ戻る。

付与済みの mIV タグは緑、未付与は通常色、見開き mixed は中間色で表示する。

### 4.5 キーバインド

v1.0 では新規キーバインド追加なし。将来検討:
- `T` キーで最後に使ったタグをトグル
- `1`〜`9` で登録タグの 1〜9 番目をトグル

## 5. 書き込みパス

### 5.1 XMP パケット編集

既存の [src/xmp_reader.rs](../src/xmp_reader.rs) と同じ形式別抽出ロジックを
書き込みにも拡張する。新規モジュール: `src/xmp_writer.rs`。

**フロー**:
1. ファイルから XMP パケット (RDF/XML) を抽出 (無ければ最小 XMP パケットを合成)
2. `quick-xml` で RDF を編集 (`dc:subject` の Bag に要素追加/削除)
3. 形式別にファイルへ再埋め込み
4. 一時ファイル → `rename` でアトミック置換

### 5.2 XMP パケットの編集規約 (既存メタデータ保持)

既存 XMP パケットを「再シリアライズ」すると書式差分が生じて他ソフトで
再解釈が走るので避ける。**最小限の差分編集**を徹底する。

**ハイブリッド方式**:

1. **既存 `<dc:subject>` 要素がある場合**:
   - そのバイト範囲を特定 (quick-xml の event offset で取得)
   - 新しい `<rdf:Bag>` + `<rdf:li>` 形式で**丸ごと置換**
   - 周辺の他プロパティ (例: `xmp:Thumbnails` の `rdf:parseType="Resource"` ショートハンド等) には触らない
2. **`<dc:subject>` が存在しない場合**:
   - `<rdf:Description>` の閉じタグ直前に element-based で新規挿入
3. **属性だけの自己閉じタグ `<rdf:Description .../>` の場合**:
   - 開始タグ + 空本体 + 閉じタグの形に展開してから挿入
4. **`xmp:MetadataDate` を毎回更新** (XMP Spec Part 1 §8.4 準拠):
   - 存在すれば現在時刻に差し替え、無ければ `<rdf:Description>` 内に新規追加
   - これを怠ると Lightroom の同期や DAM ソフトが「変更を検出できない」挙動になる

**補足**:
- `dc:subject` は Bag 型なので **仕様上属性形式 (ショートハンド) では書けない**。
  実運用で遭遇する画像は element-based のみと想定できる。
- 万一 `dc:subject` が仕様外の属性形式で書かれていた場合は警告ログを出して
  element-based で上書きする割り切り。
- 他プロパティ (`rdf:parseType="Resource"` 等) のショートハンドは温存される。

### 5.3 形式別の XMP 埋め込み規格

| 形式 | 埋め込み位置 | 特記事項 |
|---|---|---|
| **JPEG** | APP1 セグメント (`http://ns.adobe.com/xap/1.0/\0` シグネチャ) | 64KB 超は Extended XMP (5.4 参照) |
| **PNG** | `iTXt` チャンク (keyword `XML:com.adobe.xmp`) | IHDR 以後・IDAT 以前、非圧縮 |
| **WebP** | RIFF `XMP ` チャンク (VP8X 拡張コンテナ必須) | 単純 WebP は VP8X へ昇格 |

HEIC は v1.0 では非対応 (§8 参照)。

### 5.4 JPEG Extended XMP (Standard のみ差し替え方式)

XMP パケットが 64KB (厳密には 65503 バイト) を超える場合、JPEG は Standard XMP +
Extended XMP (複数 APP1 チャンク、GUID で紐付け) に分割して格納される
(Adobe XMP Specification Part 3 §1.1.3)。XMP ツイート情報を持つ画像は
`xtw:Description` (日本語数百文字) + 複数の `xtw:*` フィールドを持つため、
Extended XMP を使っている可能性が高い。

**採用方針: Standard XMP のみ差し替え、Extended XMP はバイト保持**

`dc:subject` は XMP Core プロパティで、Standard XMP 側に配置されるのが慣例
(Lightroom / Bridge / ExifTool / iPhone すべて同じ)。したがって以下で十分:

1. ファイルの APP1 群を 3 種類に分類:
   - **Standard XMP APP1**: シグネチャ `http://ns.adobe.com/xap/1.0/\0`
   - **Extended XMP APP1 群**: シグネチャ `http://ns.adobe.com/xmp/extension/\0`
   - **その他** (EXIF の APP1 含む)
2. Standard XMP 内の `<dc:subject>` のみ差し替え
3. **`xmpNote:HasExtendedXMP` プロパティは消さずに保持** (これが落ちると
   Extended XMP が孤児化してツイート本文が消失する)
4. Extended XMP の APP1 群はバイト列のまま再書き込み。**GUID 再計算不要**
   (内容変更がないので元の MD5 ハッシュがそのまま有効)
5. 書き出し順序: APP0 → APP1(Standard XMP) → APP1(Extended XMP 群) → その他 (Adobe 推奨)

**Standard XMP が 64KB を超えた場合** (通常 `#タグ` 追加程度では発生しない):
- v1.0 ではエラーとして書き込み拒否、ユーザーに警告表示
- v1.1 以降で Standard ↔ Extended 再分割 (マージ + 再分割 + GUID 再計算) を検討

### 5.5 アトミック書き込み

```rust
// 擬似コード
fn write_xmp_atomically(path: &Path, new_xmp: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".{}.miv-tmp", uuid::Uuid::new_v4()));

    // 元ファイル全体を読み込み、メモリ上で新 XMP を埋め込んでから tmp に書き出す
    let original = std::fs::read(path)?;
    let modified = embed_xmp_for_format(path, &original, new_xmp)?;
    std::fs::write(&tmp, &modified)?;

    // 同一ボリューム上の rename は Windows でもアトミック
    // ReplaceFileW 経由 (std::fs::rename は Windows で ReplaceFile を使う)
    std::fs::rename(&tmp, path)?;
    Ok(())
}
```

**失敗時の挙動**:
- tmp 書き出し失敗 → tmp を削除、エラー通知
- rename 失敗 → tmp を削除、エラー通知
- 元ファイルの mtime は書き込み後に変化する (検索インデックスの再取り込みが走る)

### 5.6 書き込み worker

UI スレッドをブロックしないため、タグ書き込みは専用 worker thread で実行する。
既存の [src/ingest_worker.rs](../src/ingest_worker.rs) の `IngestSession` と
類似のパターン:

- `TagWriteJob { path, op: TagOp::Add("#原神") | TagOp::Remove("#原神") | TagOp::ClearMiv }` を
  mpsc で worker に流す
- worker は 1 ファイルずつシリアル処理 (並列書き込みは FS ロック競合リスク)
- 進捗は `Arc<AtomicUsize>` で UI に報告し、左下ステータスラインに表示
- 完了時に `fts_meta.db` UPSERT + Tantivy upsert を行う

詳細な worker テンプレは [docs/ui-responsiveness.md](ui-responsiveness.md) §2 に
従う。キャンセルトークン・生成世代チェックは不要 (タグ書き込みは短時間で完結する)。

## 6. 検索統合

### 6.1 fts_meta.db スキーマ拡張

[src/fts_meta.rs](../src/fts_meta.rs) の `files` テーブルに `tags` カラムを追加:

```sql
ALTER TABLE files ADD COLUMN tags TEXT NOT NULL DEFAULT '';
```

- スペース区切りで `#原神 #ドール` のような形式 (プレフィックス `#` 込み)
- 既存行はデフォルト値 `''` で埋める
- `INDEX_VERSION` を bump して全再インデックスをトリガー

### 6.2 Tantivy スキーマ拡張

[src/fts_index.rs](../src/fts_index.rs) に `tags` フィールドを追加:

- 型: `TEXT | STORED` (tokenizer は空白分割)
- インデックス更新時にスペース区切り文字列として投入
- 検索時は `tags:#原神` のような term query

### 6.3 クエリパーサ拡張

[src/search_query.rs](../src/search_query.rs) の `parse()` に
`#タグ名` プレフィックス構文を追加:

```rust
enum TokenKind {
    Keyword,
    Tag,       // 新規: #プレフィックス付き
}

// "#原神" → Token { kind: Tag, text: "#原神", exclude: false }
// "-#原神" → Token { kind: Tag, text: "#原神", exclude: true }
// "#原神 #ドール" → AND 検索
```

- **オプション A** (ユーザー選択): `#プレフィックス付きで明示的に入力した場合のみ
  タグ検索**。プレフィックスなしのキーワード検索ではタグを対象にしない
- Tantivy 側のクエリ生成: `tags:"#原神"` の term query
- 既存の `all_text` フィールドにはタグをコピー**しない** (オプション A のため)

### 6.4 読み取り経路: XMP 既存タグの取り込み

書き込み機能の前段として、**既存の XMP `dc:subject` をインデックス時に取り込む**
ことで、以下の Quick win を得る:

- Windows エクスプローラーや他ソフトで付けた `#` 始まりのタグが
  Ctrl+G で即検索可能になる
- mimageviewer 自身でタグを付与する前からユーザーに価値を提供できる

実装:
- [src/xmp_reader.rs](../src/xmp_reader.rs) に `read_dc_subject(path: &Path) -> Vec<String>`
  を追加
- [src/ingest_worker.rs](../src/ingest_worker.rs) のメタ抽出フェーズで呼び出し
- `dc:subject` 要素のうち `#` で始まるものだけを `tags` カラムに入れる
- (オプション) `#` なしの `dc:subject` 要素は `all_text_norm` に連結しておくと、
  通常キーワード検索でもヒットする

### 6.5 インデックス更新タイミング

- **ユーザーがタグ付与 → ファイル mtime 変化** → 既存の notify-rs 監視が発火 →
  ingest worker が再走査 → `tags` カラム更新
- **書き込み worker が直接インデックス更新**することで、mtime 検出のレイテンシを
  回避する高速経路も追加する (2.2 の「完了後、検索インデックスにも反映される」)

## 7. エラーハンドリング

### 7.1 書き込みエラー

| エラー | 挙動 |
|---|---|
| ファイル読み取り不可 (権限等) | そのファイルはスキップ、他のファイルは継続 |
| 形式が未対応 | UI でボタン自体を無効化しているのでここには来ない想定 |
| XMP パース失敗 (破損ファイル) | スキップ、エラーログに記録 |
| tmp 書き出し失敗 | スキップ、tmp を削除 |
| rename 失敗 | スキップ、tmp を削除 |

一括処理後にエラーが 1 件以上あれば、ステータスラインに「12 件中 2 件失敗」と
表示 + 詳細ボタンでエラーログ表示。

### 7.2 読み取り専用ファイル

書き込み前に `metadata().permissions().readonly()` をチェックし、読み取り専用なら
スキップ + エラー記録。

### 7.3 ファイル破損時の回復

バックアップは取らないため、書き込み失敗時の自動回復機構は持たない。アトミック
rename により、成功すれば完全に新ファイル、失敗すれば完全に旧ファイルのまま、
という 2 択を保証することでデータロスを防ぐ (中途半端な書き込みは発生しない)。

## 8. XMP 書き込みライブラリ選定

### 8.1 検討結果 (2026-04 時点)

| 方式 | ビルド依存 | HEIC | 既存メタ保持 | crt-static 互換 | 判定 |
|---|---|---|---|---|---|
| `little_exif` (v0.6.23) | 純 Rust | (API不足) | ✗ (XMP API 自体が未整備) | ◎ | ✗ |
| `xmp_toolkit` (Adobe SDK) | C++/cmake | ✗ (Adobe SDK Issue #32 未解決) | ◎ | ⚠ | ✗ |
| **自前実装 (`quick-xml`)** | 純 Rust | ✗ | ◎ | ◎ | **◎** |
| `libheif-rs` | 外部 DLL (libde265/x265) | △ (再エンコード必須近い) | — | ✗ (単体 exe 崩す) | ✗ |

**調査結果**:
- `little_exif` は HEIC 読み書きはあるが **XMP 書き込み API 自体が未整備**
  (Issue #95 で設計段階)。`dc:subject` の Bag 追加は現状不可能。
- `xmp_toolkit` の Adobe SDK は HEIC ハンドラが **2021 年から未実装** (Issue #32 open)。
  加えて C++ 依存と `+crt-static` の整合確認が必要。
- HEIC の自前実装は ISOBMFF の `meta/iinf/iloc` 全再計算 + iPhone HDR の
  tmap アイテム対応で、ExifTool でさえ 2024 年末までバグ修正が続いていた難所。
  2-3 週間以上の工数 + 互換性検証が必要。

### 8.2 v1.0 採用: 自前実装 + `quick-xml` (JPEG/PNG/WebP)

- `quick-xml` は既に [src/xmp_reader.rs](../src/xmp_reader.rs) で使用中なので
  追加依存なし
- JPEG APP1 / PNG iTXt のセグメント操作も同モジュールに読み取り実装があり、
  書き込み側もそれを対称に拡張する形で書ける
- WebP は新規実装が必要だが、RIFF チャンク操作は明快で 100 行程度
- CRT 静的リンク (`+crt-static`) との互換性問題なし

### 8.3 HEIC: v1.0 非対応

v1.0 では HEIC / HEIF へのタグ書き込みを非対応とする。UI 上でタグ付与ボタンを
ZIP/PDF と同じく無効化 (グレーアウト)。v1.1 以降で `little_exif` の XMP API 成熟を
待って再検討する。

## 9. 実装フェーズ

### Phase A — 読み取り + 検索 (書き込みなし) [最優先]
1. `xmp_reader.rs` に `read_dc_subject()` 追加
2. `fts_meta.db` スキーマに `tags` カラム追加 (migration)
3. `fts_index.rs` (Tantivy) に `tags` フィールド追加、INDEX_VERSION bump
4. `ingest_worker.rs` のメタ抽出で `dc:subject` を取り込み
5. `search_query.rs` に `#tag` 構文追加
6. メタデータパネルにタグセクション (閲覧のみ)

**この時点で「他ソフトで付けた `#` タグが Ctrl+G で検索可能」になる。**

### Phase B — タグ定義と UI 骨組み
1. `Settings` に `tags: Vec<TagDef>` + `show_toolbar_tags` 追加
2. `ui_dialogs/tag_editor.rs` 新設 (favorites_editor のパターン踏襲)
3. メニューバーに「タグ」メニュー追加
4. ツールバーに「タグ: xxx yyy zzz」セクション追加
5. `toolbar_settings.rs` に表示オプション追加

### Phase C — 書き込み (JPEG/PNG/WebP)
1. `xmp_writer.rs` 新設
   - 形式判定 + XMP パケット差分編集 + アトミック rename
   - JPEG Extended XMP 対応
2. 書き込み worker (`tag_write_worker.rs`) 新設
3. 2.3 のトグルロジック実装
4. メニュー/ツールバーからの付与/削除/クリア実行フロー
5. 書き込み完了時の検索インデックス即時反映

### Phase D — HEIC 対応 (`little_exif` 調査後に判断)

### Phase D — Ctrl+G 検索でのタグピッカー

Ctrl+G 検索ボックスの右側にタグプルダウンを配置し、登録済みタグから
選択するだけで検索ボックスに `#タグ名` が挿入される UI。

- Global search UI (`src/global_search_ui.rs`) に `egui::ComboBox` または
  `Menu` 形式のピッカー追加
- タグをクリックしたら検索クエリテキストに ` #タグ名` を追記 (末尾)
- 既に同じタグがクエリ内にあれば何もしない (冪等)
- 既存タグが 0 件の場合はピッカーを無効化 (グレーアウト)

### Phase E (将来) — 将来拡張
- `lr:hierarchicalSubject` 書き出し互換オプション
- `T` キー / 数字キーのショートカット
- メタデータパネルからのタグ候補の並べ替え / ピン留め切替
- バッチ処理用の進捗ダイアログ

## 10. テスト方針

### 10.1 単体テスト
- `xmp_writer` の形式別埋め込み: 既存 `xmp_reader` のテストパターンを踏襲
- 既存 XMP パケット保持の回帰テスト (他ソフト由来のタグが消えないこと)
- Extended XMP 分割・結合の対称性
- 空 `dc:subject` への新規追加、既存 `dc:subject` への追記、削除、すべてクリア
- `#` プレフィックス削除で他要素が保持されること

### 10.2 結合テスト
- 実ファイル (JPEG/PNG/WebP) に書き込み → 再読込で `dc:subject` が正しいこと
- Windows エクスプローラーのタグ欄で表示されること (手動確認)
- 書き込み後に ExifTool で XMP を ダンプして他プロパティ破壊がないこと (手動)
- アトミック書き込みの確認: tmp 書き出し中にプロセス kill → 元ファイル無傷

### 10.3 UI スナップショット
- タグ編集ダイアログ、タグツールバー表示を `egui_kittest` でスナップショット化
  ([docs/ui-snapshot-policy.md](ui-snapshot-policy.md) 参照)

### 10.4 検索
- `#原神` 検索でタグ付き画像のみヒット
- `#原神 #ドール` AND, `-#原神` 除外
- プレフィックスなし `原神` ではタグによるヒットがないこと (オプション A)

## 11. 関連ドキュメント更新 (コード実装時)

実装着手時は以下も同時に更新する:
- [docs/spec.md](spec.md) — タグ機能の仕様セクション追加
- [docs/architecture-overview.md](architecture-overview.md) — モジュール追加
  (`xmp_writer`, `tag_write_worker`, `ui_dialogs/tag_editor`)
- [docs/async-architecture.md](async-architecture.md) — タグ書き込み worker の追加
- [docs/search-expansion-design.md](search-expansion-design.md) — `#tag` 構文の追加
- [htdocs/mimageviewer/index.html](../htdocs/mimageviewer/index.html) — 機能紹介
- [htdocs/mimageviewer/manual/](../htdocs/mimageviewer/manual/) — 操作マニュアル

## 12. オープン課題

- **進捗表示**: 大量ファイル一括付与時のキャンセル UX
- **XMP ツイート情報との相性**: `xtw:*` 名前空間と `dc:subject` は同じ XMP パケット
  内に共存可能。書き込み時に `xtw:*` プロパティを破壊しないことをテストで担保する
  (§5.4 の Extended XMP 保持方針で技術的には担保できる想定)
- **Standard XMP が 64KB 超になった場合**: v1.0 はエラー扱い。v1.1 で
  Standard ↔ Extended 再分割を検討
- **拡張子ケース**: `.JPG` と `.jpg` 両方対応済み確認 (既存読み取りは対応済み)
