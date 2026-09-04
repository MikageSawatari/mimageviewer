# ブリーフ: 別バージョン発見 Step 2b — `similar.db` と索引ジョブ

対象: 「別バージョンの発見」機能の Step 2b。
実装 = Codex Sol / レビュー・検収 = ClaudeCode。

正本: [docs/duplicate-detection-plan.md](duplicate-detection-plan.md)。
着手前に **§0 (目的)、§9 (UI 方針)、§16 (目的転換)、§17 / §19 (実測値)** を読むこと。
**この機能は削除しない。** 発見と移動のための機能である。

作業ツリー: **`C:\home\mimageviewer-dupe`** (branch `duplicate-detection`)。
**コミットしないこと** (sandbox が worktree の git メタデータに書けない)。
ファイルを書き、`cargo fmt` とテストを通したら止まる。コミットは ClaudeCode が行う。

---

## 0. この Step の位置づけ

計測は終わった。§11 の仮置きは全て実測で埋まっている (§19.4)。
**ここで初めて永続化と製品コードに入る。**

**この Step で作らないもの**:

- **UI** (右パネルのタブ、一覧ビュー、ページ帯) — Step 3
- **全件スイープ** — §16.1 で v1 対象外に確定
- **削除・整理・「どちらを残すか」** — §0 / §8 で対象外に確定
- 幾何的なズレへの対応 — §18 で対象外に確定

---

## 1. 保存するものを最小にする (実測による簡略化)

計測の結果、**当初考えていたより保存量をかなり減らせる**。

| 当初案 | 実測後 | 根拠 |
| --- | --- | --- |
| `phash64` と `phash256` を両方保存 | **PDQ-256 だけ保存** | `Pdq64` は PDQ-256 の**ビット部分集合**なので、必要なら**読み込み後にメモリ上で導出できる**。保存する理由がない |
| 連続値 Luma-32 も保存 (1 KB/件) | **保存しない** | PDQ-256 単体で既知重複 ≤8 / 無関係 p0.1% = 102 と分離が大きい (§15.1)。100 万件で 1 GB を払う根拠がない。検証が要る稀な場面では候補だけ再デコードすればよい |
| `jpeg_quality` を保存 | **保存しない** | 「どちらを残すか」の判定を廃止した (§7) ので使い道がない |

**1 件あたり 32 バイト + メタデータ。** 100 万件で署名部 32 MB。

## 2. `similar.db`

```
%APPDATA%/mimageviewer/similar.db
```

```sql
CREATE TABLE item (
  item_key       TEXT PRIMARY KEY,  -- search_norm::normalize_path と同じ正規化
  kind           INTEGER NOT NULL,  -- 画像 / ZIP ページ / PDF ページ
  container_key  TEXT,              -- 所属する本。ルーズ画像は親フォルダ、無い場合 NULL
  page_index     INTEGER,           -- 本の中での位置。ルーズ画像は名前順
  mtime          INTEGER NOT NULL,
  file_size      INTEGER NOT NULL,
  hash_version   INTEGER NOT NULL,
  pdq256         BLOB    NOT NULL,  -- 32 バイト
  quality        INTEGER NOT NULL,  -- PDQ quality。0 は featureless (§15.3)
  width          INTEGER NOT NULL,
  height         INTEGER NOT NULL,
  format         INTEGER NOT NULL
);

CREATE TABLE container (
  container_key  TEXT PRIMARY KEY,
  kind           INTEGER NOT NULL,
  page_count     INTEGER,
  scan_state     INTEGER NOT NULL,  -- Building / Complete / Failed
  generation     INTEGER NOT NULL,
  mtime          INTEGER NOT NULL,
  file_size      INTEGER NOT NULL
);
```

- `fts_meta.db` に相乗りしない。FTS の `INDEX_VERSION` bump で
  **ハッシュまで道連れに消える**のは損が大きすぎる (再生成に時間がかかる)
- `catalog.db` にも載せない。`thumb_data NOT NULL` でキャッシュ設定 Off/Auto では
  行が作られないため (カラー検索で踏んだ罠)

## 3. 容器の原子性 (§13.1-F。落とすと誤った関係が出る)

**200 ページ中 100 ページでキャンセル・失敗・差し替えが起きた本から
coverage を計算してはいけない。**

- `container.scan_state` = `Building` → `Complete` / `Failed`
- 再索引は**新しい `generation` で全ページを書いてから、原子的に `Complete` へ**
- **検索対象は `Complete` の世代だけ**
- 途中で終わった世代の行は掃除する

これはテストで固定すること (§7)。

## 4. 索引ジョブ

FTS インデクサ ([search-architecture.md](search-architecture.md) §4) と同型にする。

- スコープは**お気に入り配下** (= Ctrl+G と同じ。§11-3 で確定)
- 進捗表示・キャンセル・中断再開・`mtime`/`file_size` による差分更新
- **明示的に開始させる。** 初回は 100 万枚で 11.6 時間の実測 (§15.5、遅い共有ドライブ)
- `hash_version` / `PROXY_VERSION` が変わったら該当行を無効化して再生成する
- ZIP / PDF のページ列挙を含む。PDF は `pdf_loader::render_page` を
  `JobPriority::Normal` / `context_epoch = 0` / `AbortOnCancel` で使う
- **パスワード必須 PDF・破損・0 ページは件数を集計して報告する。無言で捨てない**

### 4.1 正準経路は 1 つ

`bench_dupe` で確立した規則をそのまま製品へ移す。ここがずれると機能全体が無意味になる。

- JPEG は **DCT 縮小デコードの目標を長辺 2048** にする (§17.1。1024 では
  フルデコードとの残差が残り、実在の重複を取り逃した)
- PDF ページは**長辺 1024 でレンダリング**し、縮小は `dupe::proxy` に行わせる。
  **64px でレンダリングさせない** (PDFium 自身の縮小が焼き付く)
- **ファイルを `Proxy` にする関数は 1 つだけ**にする。
  `bench_dupe` が二重経路を持っていたことが §17.1 の原因だった

### 4.2 サムネイル生成との関係 (§16.2)

**保存済みサムネイルからハッシュを作らない** (解像度が設定で変わる、寿命が違う)。
ただし**サムネイル生成は元画像をデコードするので、その場のバッファからハッシュも作れば
追加 I/O はゼロ**になる。索引の 95% は I/O なので (§15.5)、これは大きい。

- 背景索引が**全件を保証**する
- サムネイル生成が**それを前倒しする**
- 生成元は常に元画像なので**正準性は壊れない**

この二本立てにすること。**サムネイル経路だけに依存させない。**

## 5. 在メモリ索引と検索

- 索引は**起動時に読まない**。機能を最初に使うときにワーカーで一括ロードし、
  それまでは「準備中」を返す ([ui-responsiveness.md](ui-responsiveness.md) §4)
- 在メモリ表現は `Vec<(pdq256: [u8;32], row_id: u32)>` = 100 万件で約 36 MB
- 単体クエリは**線形走査**。100 万件 × 256bit で 1.6 ms の実測 (§4.1)。
  **索引構造 (VP-tree / BK-tree / MIH / HNSW) を作らないこと** (§4.2)
- 帯は §9.5: **≤8 = ほぼ同一 / 9〜48 = 別バージョン / 49 以上は返さない**
- `quality = 0` の画像を起点にしたクエリは、0 件ではなく
  **「特徴が少ないため判定できない」を型で返す** (§5.2。無言で 0 件にしない)

## 6. 本単位の関係

`dupe::book` は実装済み。ここでは**入力を用意して呼ぶ**だけにする。

- パラメータは §19.4 の実測値: `radius = 32`、`coverage = 0.5`、
  `min_matched_pages = 3`、`K = 8`、`min_quality = 1`
- **単体画像の帯 (≤8) と本単位の半径 (32) は別物。共有しないこと** (§17.2)
- 本の判定は既存の正本 `folder_scan::is_image_only_book_contents` を呼ぶ。
  **再実装しない** (§14.1)
- ただし設定 `auto_fullscreen_image_folders` の ON/OFF で**索引内容を変えない**。
  ページペアを容器へ畳む集約は常に行い、設定は**呼び方だけ**を変える (§14.1)

## 7. テスト

- **容器の原子性**: `Building` の世代が検索に出ないこと。
  途中終了した世代の行が掃除されること。再索引で世代が入れ替わること
- **`hash_version` / `PROXY_VERSION`**: 版が違う行を再利用しないこと
- **正準性**: 同じ画像を通常ファイル / ZIP 内 / PDF ページとして通しても
  同じ経路を通ること (経路が 1 つであることをテストで固定する)
- **featureless**: `quality = 0` を起点にしたクエリが「判定不能」を返し、
  0 件でも大量結果でもないこと
- **差分更新**: `mtime` / `file_size` が変わった行だけ再計算されること
- **設定非依存**: `auto_fullscreen_image_folders` を切り替えても
  `item` 行が 1 行も無効化されないこと (§14.1)
- **キャンセル**: 索引の途中キャンセルで `Complete` の世代が壊れないこと

## 8. 完了条件

- `cargo fmt` 済み、`cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `cargo test -p mimageviewer --lib` が緑
- **UI への配線は無い** (Step 3)。App からは索引の開始・進捗・クエリの API が
  呼べる状態まで
- 既存アプリの挙動に変更がない

## 9. 判断に迷ったとき

- **閾値を新たに決めない。** §19.4 の値を使う。新しい値が要ると思ったら、
  実装で埋めずに質問として残す
- **silent fallback を作らない。** 判定できない入力は型で表す
- **索引構造を作らない。** 線形走査で足りることは実測済み
- 仕様上どうしても決まらない点は、**実装で埋めずにブリーフへの質問として残す**
