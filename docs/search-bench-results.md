# 検索プロトタイプ計測結果 (§15.1.1)

> 対象: docs/search-expansion-design.md §15.1.1 プロトタイプ計測
> 実施日: 2026-04-21
> 環境: Windows 11, RTX 4090 マシン (詳細 CPU 不明)、SSD
> ベンチ: [src/bin/bench_search.rs](../src/bin/bench_search.rs)
> Tantivy: 0.26.0 (`default-features = false, features = ["mmap", "lz4-compression"]`)

## 1. 目的

§15.1.1 の 3 つの論点を実測で確認:

1. Tantivy + bigram + post-filter で速度が出るか
2. **`TopDocs::and_offset` の offset 肥大時の worst case** が許容範囲か
3. SQLite 側 post-filter (500 件 `WHERE path IN (...)`) の往復コスト

## 2. 合成コーパス

画像メタ 1 件を約 2 KB のテキスト (EXIF 風 + 日本語 generic/medium/rare + 英語 common/AI) で模擬。
JP_RARE (美濃焼・斑鳩宮 等) は 20 件に 1 件だけ混入する頻度分布。

| 規模 | 生成時間 | 合計テキスト |
| --- | --- | --- |
| 100,000 docs | 0.3s | 191.0 MB |
| 500,000 docs | 1.4s | 955.0 MB |

## 3. インデックス構築

| 規模 | Tantivy 構築 | Tantivy サイズ | raw 比 | SQLite 構築 | SQLite サイズ |
| --- | --- | --- | --- | --- | --- |
| 100K | **1.4s** (73K docs/s) | **14.5 MB** | **0.08x** | 3.8s | 396.7 MB |
| 500K | **6.0s** (84K docs/s) | **69.7 MB** | **0.07x** | 24.7s | 1984.3 MB |

### 所見

- **Tantivy インデックスは設計見積もり (1.5〜3x) より桁違いに小さい**。実測 **0.07〜0.08x**。
  bigram 特有の postings 圧縮が効いていると思われる。
  → **設計ドキュメント §12.1 のサイズ見積もりは大幅に修正が必要**
- SQLite 側 (path + all_text_norm) は元テキストの 2x 相当で妥当
- 構築スループット 73〜84K docs/s は快適。10 万件で 1.4s、50 万件で 6s

## 4. クエリ計測 (500K docs)

`TopDocs::with_limit(500).and_offset(offset)` でページング取得、HARD_MAX=10,000 で打ち切り。
Searcher snapshot はループ外で 1 回取得し固定 (§9.1)。

| ラベル | クエリ | pages | 候補数 | total | first page | worst page | post-filter | 備考 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| rare_jp | `美濃焼` | 5 | 2,275 | **11.6ms** | 0.9ms | 0.9ms | 9.5ms | JP_RARE 1/20 |
| rare_jp_and | `美濃焼 斑鳩宮` | 0 | 0 | **0.4ms** | — | — | — | 両方含む doc なし |
| medium_jp | `夕焼け` | 20 | 10,000 | **36.9ms** | 1.6ms | 1.6ms | 14.7ms | HARD_MAX TRUNCATED |
| medium_jp_and | `夕焼け 海辺` | 20 | 10,000 | **52.6ms** | 1.9ms | 2.3ms | 15.6ms | HARD_MAX TRUNCATED |
| medium_jp_3 | `夕焼け 海辺 紅葉` | 3 | 1,239 | **10.4ms** | 2.1ms | 2.1ms | 5.2ms | |
| generic_jp | `カメラ` | 20 | 10,000 | **161.0ms** | 4.5ms | 9.0ms | 23.9ms | **超偽陽性ケース** |
| super_generic | `の` | 0 | 0 | 0.0ms | — | — | — | 1 文字 → bigram 空 |
| en_common | `photo` | 20 | 10,000 | **106.5ms** | 3.6ms | 5.5ms | 18.5ms | HARD_MAX TRUNCATED |
| en_ai_phrase | `lora sampler` | 10 | 4,905 | **46.0ms** | 2.8ms | 3.4ms | 18.4ms | |
| unique_id | `img_042000` | 2 | 608 | **3.2ms** | 0.9ms | 0.9ms | 1.7ms | |

### 所見

- **全クエリの total が 200ms 以下**。最悪でも 161ms (汎用 bigram の HARD_MAX 到達ケース)。
  streaming で最初のページが ~5ms で返るので、UX 上は問題ない
- **offset 肥大時の penalty はごく軽微** (first page 4.5ms → worst page 9.0ms で 2x)。
  Codex が懸念した「offset 線形増加」は **実測で観測されず**、この規模では問題にならない
  → `TopDocs::and_offset` は暫定 → **本採用** で OK
- HARD_MAX (10,000) で TRUNCATED された 4 クエリはすべて 50〜161ms。実運用では
  ユーザに「結果が多すぎます」表示が出るケースなので、UI 体感として問題なし
- 超汎用 bigram (カメラ) でも 161ms。 Lucene 系ならもっと良い結果が出る余地はあるが、v1 十分

## 5. SQLite post-filter 一括 lookup (500K docs)

500K doc の SQLite から `WHERE path IN (?,?,...)` で一括取得:

| batch サイズ | best time | per-row |
| --- | --- | --- |
| 100 | 1.7ms | 0.02ms |
| **500** | **9.1ms** | 0.02ms |
| 1,000 | 13.9ms | 0.01ms |
| 2,000 | 28.6ms | 0.01ms |

### 所見

- **500 件バッチで 9ms**。§9.1 のページング (page_size=500) と完全に噛み合う
- バッチサイズの増加に対してほぼ線形。10K 件を一度に取得しても 100ms 程度と予測
- per-row 10〜20μs は SQLite 一括 lookup としては極めて速い部類

## 6. §12.1 見積もりとの比較

| 項目 | §12.1 見積もり (10 万件) | 実測 (10 万件) | 誤差 |
| --- | --- | --- | --- |
| Tantivy `fts_index/` | 300〜600 MB | **14.5 MB** | **20〜40x 小さい** ✓ |
| fts_meta.db メタ | 20 MB | — (未計測) | — |
| fts_meta.db `all_text_norm` | 200 MB | 396.7 MB (DB 総サイズ) | 2x 大きめ |

### 設計ドキュメント §12.1 の修正必要点

- Tantivy インデックスサイズは **raw テキスト × 0.08** と大幅に下方修正
- 10 万件で **約 15 MB**、50 万件で **約 70 MB** に訂正
- ユーザが想定する 10〜50 万規模なら Tantivy 側は無視できるサイズ
- SQLite 側 (all_text_norm テキスト保存) は見積もり通り 2x 程度

## 7. 結論

### PASS 判定

- **Tantivy + bigram + post-filter + offset pagination は本採用**
- Searcher snapshot 固定と HARD_MAX=10,000 打ち切りで十分
- custom Collector + continuation token への切り替えは **不要** (§15.1.9 確定)

### §15.1.1 論点への回答

| 論点 | 結果 |
| --- | --- |
| Tantivy + bigram + post-filter の速度 | ✅ OK (最悪 161ms、典型 < 50ms) |
| `TopDocs::and_offset` の offset 肥大 worst case | ✅ OK (first page 比 2x の劣化にとどまる) |
| SQLite 500 件 lookup コスト | ✅ OK (9ms/バッチ) |

### 次のステップ

§16 実装順序に従い、以下の順で着手:

1. ✅ プロトタイプ計測 (本文書)
2. `FavoriteEntry` に `id: Uuid` + 3 フラグ追加 + マイグレーション
3. お気に入りエディタ / 追加ダイアログ拡張
4. `fts_meta.db` スキーマ作成 + CRUD 層
5. ... (以下 §16 の順)

## 8. 再現手順

```bash
cargo run --release --bin bench_search                  # 10 万件デフォルト
cargo run --release --bin bench_search -- --docs 500000 # 50 万件
cargo run --release --bin bench_search -- --docs 100000 --keep  # index を残す
```
