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

### 2.1 中央集約型レイアウト

```
{cache_dir}/
  {xx}/                   ← フォルダパスハッシュ（hex）の先頭2文字
    {full_hash}.db        ← フォルダ1つ分のサムネイルカタログ（SQLite）
    {full_hash}.db
    ...
  {xx}/
    ...
```

- `cache_dir` は設定で指定（デフォルト: `%APPDATA%\mimageviewer\cache`）
- 先頭2文字サブディレクトリにより、最大256フォルダへ分散（1フォルダあたり平均4〜5ファイル＠1000フォルダ）
- 画像フォルダへの書き込みが不要（ネットワークドライブ・読み取り専用フォルダでも動作）

### 2.2 ハッシュキーの計算

```
key = SHA-256( normalize(folder_path) )

normalize(path):
  1. ドライブ文字を除去（"C:\Photos\2024" → "\Photos\2024"）
  2. 小文字化（"\photos\2024"）
  3. バックスラッシュをスラッシュに正規化（"/photos/2024"）
```

**リムーバブルデバイス対応**: ドライブ文字を除去することで、同じリムーバブルデバイスが
別のドライブレターでマウントされても同じキャッシュにヒットする。

> 注意: `C:\photos` と `D:\photos` は同一ハッシュになる。これは意図的な設計で、
> パスが同じならキャッシュを共有する（mtime/file_size で整合性チェックするため実害なし）。

### 2.3 ハッシュ計算例

```
"D:\Photos\2024\夏休み"
→ normalize: "/photos/2024/夏休み"
→ SHA-256: "3f8ac1b2d4e5..."
→ ファイル: {cache_dir}/3f/3f8ac1b2d4e5....db
```

---

## 3. ファイル形式の選定

### SQLite（rusqlite）を採用

| 形式 | 読み込み速度 | 実装コスト | 備考 |
|------|-----------|----------|------|
| **SQLite（フォルダ1DB）** | 速い | 低 | 差分更新が容易。今回採用 |
| グローバル1DB | 速い | 低 | 全キャッシュが1ファイル。巨大化するが SQLite は問題なし |
| フラットファイル（JPEG + メタ） | 最速 | 中 | SQLite不依存だが実装コスト高め |

採用理由：
- 差分更新（ファイル追加・変更・削除の部分更新）が容易
- WALモードで並列読み書きが安全
- フォルダ単位でキャッシュを削除しやすい
- `rusqlite` (bundled) で外部依存なし

---

## 4. データベーススキーマ

```sql
-- カタログのバージョン管理
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- key='version' value='2'
-- key='folder_path' value='/photos/2024/夏休み'（正規化済みパス）

-- サムネイルエントリ
CREATE TABLE thumbnails (
    filename    TEXT    NOT NULL PRIMARY KEY,  -- ファイル名（パスなし）
    mtime       INTEGER NOT NULL,              -- ファイル更新日時（Unix秒）
    file_size   INTEGER NOT NULL,              -- ファイルサイズ（バイト）
    width       INTEGER NOT NULL,              -- サムネイル幅（ピクセル）
    height      INTEGER NOT NULL,              -- サムネイル高さ（ピクセル）
    thumb_data  BLOB    NOT NULL,              -- WebP圧縮サムネイルデータ
    source_width  INTEGER,                     -- 元画像の幅（ピクセル）
    source_height INTEGER                      -- 元画像の高さ（ピクセル）
);
```

### 無効化ロジック（キャッシュ整合性）

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

カタログ内のサムネイルは **WebP（lossy, 品質75）** で保存する。

| 形式 | 圧縮率 | デコード速度 | 画質 | サイズ目安（512px） |
|------|--------|------------|------|------------------|
| JPEG q=80 | 高 | 速い | 十分 | ~20-50KB |
| **WebP lossy q=75** | 高 | やや遅い | やや良い | ~15-40KB |
| QOI | 低 | 非常に速い | 可逆 | ~100-200KB |
| PNG | 低 | 普通 | 可逆 | ~200-500KB |

→ WebP は JPEG より圧縮効率が高く、同サイズで画質が良い。
  `webp` クレートでエンコード、`image` クレートでデコード。

サムネイルサイズは **固定 512px**（長辺）とする。

---

## 6. 読み込みフロー（カタログあり）

```
フォルダを開く
  ↓
① ファイル一覧を取得（瞬時）
  ↓
② normalize(folder_path) → SHA-256 → DB パスを計算
  ↓
③ {cache_dir}/{xx}/{hash}.db を開く（なければ作成）
  ↓
④ 全キャッシュエントリを一括SELECT（1クエリ）
  ↓
⑤ 各ファイルのキャッシュ有効性を確認
  ↓
  ├─ 有効: WebP バイト列 → image::load_from_memory() → GPU テクスチャ
  │         （並列処理。デコード数ms以内）
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
  catalog.rs      # CatalogDb 構造体、パスハッシュ計算、read/write/validate ロジック
```

### 依存クレートの追加

```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
sha2 = "0.10"   # SHA-256 でパスをハッシュ化
webp = "0.3"    # サムネイル WebP エンコード
```

### CatalogDb インターフェース（案）

```rust
pub struct CatalogDb { /* SQLite接続 */ }

impl CatalogDb {
    /// cache_dir 配下の適切な場所にDBを開く（なければ作成）
    /// folder_path はドライブ文字付きの実際のパス
    pub fn open(cache_dir: &Path, folder_path: &Path) -> Result<Self>;

    /// フォルダパスを正規化してSHA-256ハッシュ化し、DBパスを返す
    pub fn db_path(cache_dir: &Path, folder_path: &Path) -> PathBuf;

    /// フォルダ内ファイルと照合し、有効なキャッシュと要更新リストを返す
    pub fn validate(
        &self,
        files: &[FileInfo],
    ) -> (Vec<CachedThumb>, Vec<FileInfo>);

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
    pub webp_data: Vec<u8>,
}
```

### パスハッシュ計算の実装例

```rust
use sha2::{Sha256, Digest};

fn normalize_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    // ドライブ文字（"C:"）を除去、小文字化、バックスラッシュをスラッシュに
    let no_drive = if s.len() >= 2 && s.chars().nth(1) == Some(':') {
        &s[2..]
    } else {
        &s
    };
    no_drive.to_lowercase().replace('\\', "/")
}

fn path_to_db_path(cache_dir: &Path, folder_path: &Path) -> PathBuf {
    let normalized = normalize_path(folder_path);
    let hash = format!("{:x}", Sha256::digest(normalized.as_bytes()));
    let prefix = &hash[..2];
    cache_dir.join(prefix).join(format!("{}.db", hash))
}
```

---

## 8. app.rs への統合方針

`load_folder` の変更点：

```rust
// 現在
// → 全画像を rayon で並列デコード・リサイズ

// カタログ追加後
// 1. CatalogDb::open(cache_dir, folder)
// 2. CatalogDb::validate(files) → (cached, needs_reload)
// 3. cached → JPEG → GPU テクスチャ（高速。並列でOK）
// 4. needs_reload → 元ファイルデコード → カタログ保存 → GPU テクスチャ
```

---

## 9. 設定項目（追加）

| 設定名 | 型 | デフォルト | 説明 |
|--------|-----|---------|------|
| `cache_dir` | String | `%APPDATA%\mimageviewer\cache` | サムネイルキャッシュの格納先 |

---

## 10. 注意事項・制限

- カタログファイルはユーザーが削除して再生成できる（手動キャッシュクリア）
- `cache_dir` が存在しない場合は自動作成する（サブディレクトリ含む）
- カタログのバージョンが変わった場合（スキーマ変更時）はそのDBを削除して再生成
- SQLite の WAL モードを使用することでロック競合を最小化
- 同一パス（ドライブ文字違い）のキャッシュ衝突は mtime/file_size チェックで実害なし
