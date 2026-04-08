# サムネイルカタログ 設計書

## 1. 目的・背景

画像ファイルからサムネイルをリアルタイム生成する場合、ファイルサイズによっては
1枚あたり 100〜500ms かかる。初回表示時のワンテンポ遅延を解消するため、
生成済みサムネイルをファイルにキャッシュし、次回以降は即座に読み込む。

### 現状の計測値（ログ実測）

| 処理 | 時間（目安） |
|------|------------|
| JPEG デコード（1枚・RTX4090環境） | 100〜500ms |
| Lanczos リサイズ（→900px） | 5〜20ms |
| GPU テクスチャ登録 | 1ms 以下 |
| カタログ読み込み目標（1枚） | 5〜20ms |

---

## 2. カタログファイルの配置

```
（画像フォルダ）/
  image001.jpg
  image002.png
  ...
  .mimageviewer/
    thumbs.db       ← サムネイルカタログ本体
    thumbs.db-wal   ← SQLite WALファイル（自動生成）
```

- 1フォルダ1ファイル
- 隠しフォルダ `.mimageviewer/` に格納（ユーザーの邪魔にならない）
- フォルダに書き込み権限がない場合は `%APPDATA%\mimageviewer\cache\` にフォールバック

---

## 3. ファイル形式の選定

### 候補比較

| 形式 | Rustクレート | 特徴 |
|------|------------|------|
| **SQLite** | `rusqlite` | 実績最大。ランダムアクセス・差分更新が容易。WALモードで高速書き込み |
| カスタムバイナリ | なし（自前） | 最速だが実装コスト高。インデックス管理を自前で行う必要あり |
| MessagePack + ファイル | `rmp-serde` | シンプルだが1ファイル全体を読み書きする必要があり差分更新が難しい |
| LMDB | `lmdb` / `heed` | 非常に高速なKVS。依存がやや複雑 |

### → **SQLite (rusqlite)** を採用

理由：
- 差分更新（ファイルが追加・変更・削除された場合の部分更新）が容易
- WALモードで並列読み書きが安全
- 実績・安定性が高い
- `rusqlite` クレートは純粋Rustバインディングで安全

---

## 4. データベーススキーマ

```sql
-- カタログのバージョン管理
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- key='version' value='1'

-- サムネイルエントリ
CREATE TABLE thumbnails (
    filename    TEXT    NOT NULL PRIMARY KEY,  -- ファイル名（パスなし）
    mtime       INTEGER NOT NULL,              -- ファイル更新日時（Unix秒）
    file_size   INTEGER NOT NULL,              -- ファイルサイズ（バイト）
    width       INTEGER NOT NULL,              -- サムネイル幅（ピクセル）
    height      INTEGER NOT NULL,              -- サムネイル高さ（ピクセル）
    thumb_data  BLOB    NOT NULL               -- JPEG圧縮サムネイルデータ
);

-- filename でのルックアップを高速化（PRIMARY KEY で自動インデックス済み）
```

### 無効化ロジック（キャッシュ整合性）

フォルダを開くたびに以下を確認する：

```
for each file in directory:
    row = SELECT mtime, file_size FROM thumbnails WHERE filename = ?
    if row not found:
        → 新規ファイル: サムネイル生成してINSERT
    elif row.mtime != file.mtime OR row.file_size != file.file_size:
        → 更新されたファイル: サムネイル再生成してUPDATE
    else:
        → キャッシュ有効: thumb_data をそのまま使用

for each row in thumbnails:
    if file not in directory:
        → 削除されたファイル: DELETE
```

---

## 5. サムネイルデータの圧縮形式

カタログ内のサムネイルは **JPEG（品質80〜85）** で保存する。

| 形式 | 圧縮率 | デコード速度 | 画質 |
|------|--------|------------|------|
| **JPEG q=80** | 高 | 速い | 十分 |
| WebP lossy | 高 | やや遅い | やや良い |
| PNG | 低 | 普通 | 可逆 |

→ サムネイルは表示用途なので JPEG で十分。デコード速度優先。

サムネイルサイズは **固定 512px**（長辺）とする。
（4K・4列のセル幅 ≈ 900px には少し小さいが、読み込み速度を優先。
  必要なら設定で変更可能にする。）

---

## 6. 読み込みフロー（カタログあり）

```
フォルダを開く
  ↓
① ファイル一覧を取得（瞬時）
  ↓
② .mimageviewer/thumbs.db を開く
  ↓
③ 全キャッシュエントリを一括SELECT（1クエリ）
  ↓
④ 各ファイルのキャッシュ有効性を確認
  ↓
  ├─ 有効: JPEG バイト列 → image::load_from_memory() → GPU テクスチャ
  │         （並列処理。デコード高速なので数十ms以内）
  │
  └─ 無効/未キャッシュ: 元ファイルからサムネイル生成
                         → カタログに書き込み
```

### 期待される速度改善

| | 初回（キャッシュなし） | 2回目以降（キャッシュあり） |
|--|---------------------|------------------------|
| 12枚（4列×3行） | 0.5〜2秒 | 0.1〜0.3秒 |
| 100枚 | 数十秒 | 0.5〜1秒 |

---

## 7. 実装コンポーネント

### 新規ファイル

```
src/
  catalog.rs      # CatalogDb 構造体、read/write/validate ロジック
```

### 依存クレートの追加

```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
# "bundled" = SQLite をソースからビルド（システムインストール不要）
```

### CatalogDb インターフェース（案）

```rust
pub struct CatalogDb { /* SQLite接続 */ }

impl CatalogDb {
    /// .mimageviewer/thumbs.db を開く（なければ作成）
    pub fn open(folder: &Path) -> Result<Self>;

    /// フォルダ内ファイルと照合し、有効なキャッシュと要更新リストを返す
    pub fn validate(
        &self,
        files: &[FileInfo],       // ファイル名・mtime・サイズ
    ) -> (Vec<CachedThumb>, Vec<FileInfo>); // (有効キャッシュ, 再生成が必要なもの)

    /// サムネイルを保存（INSERT OR REPLACE）
    pub fn save(&self, entry: &ThumbEntry) -> Result<()>;

    /// 存在しなくなったファイルのエントリを削除
    pub fn purge_deleted(&self, existing_filenames: &[&str]) -> Result<()>;
}

pub struct FileInfo {
    pub filename: String,
    pub mtime: i64,
    pub file_size: i64,
}

pub struct CachedThumb {
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub jpeg_data: Vec<u8>,
}
```

---

## 8. app.rs への統合方針

`load_folder` の変更点：

```rust
// 現在
// → 全画像を rayon で並列デコード・リサイズ

// カタログ追加後
// 1. CatalogDb::open(folder)
// 2. CatalogDb::validate(files) → (cached, needs_reload)
// 3. cached → JPEG → GPU テクスチャ（高速。並列でOK）
// 4. needs_reload → 元ファイルデコード → カタログ保存 → GPU テクスチャ
```

メインスレッド側の変更は最小限。loader.rs または catalog.rs にロジックを閉じ込める。

---

## 9. 注意事項・制限

- カタログファイルはユーザーが削除して再生成できる（手動キャッシュクリア）
- ネットワークドライブや読み取り専用フォルダでは書き込みが失敗する → フォールバックを実装
- カタログのバージョンが変わった場合（スキーマ変更時）は全削除して再生成
- SQLite のWALモードを使用することでロック競合を最小化
