# サムネイル生成 & メモリ管理 再設計メモ

v0.1.0 リリース後、リリースビルドでデコードがデバッグビルドより大幅に速いことが
判明したため、「常にキャッシュする」前提を見直し、キャッシュ生成・メモリ保持方針を
再設計する。本ドキュメントは本計測 (`bench_thumbs`) の結果待ち時点での設計メモ。

---

## 1. 背景と問題意識

### 1.1 これまでの前提

- Phase 1.5 で SQLite + WebP(q=75) サムネイルカタログを導入
- キャッシュ生成は "常時 ON"、全ファイルに対して行われていた
- デバッグビルドではデコードが遅く、キャッシュの効果が明確だった

### 1.2 リリースビルドで見えてきたこと

- `cargo build --release` で動作させたところ、キャッシュが無くても十分サクサク動く
- キャッシュはディスク容量を消費するため、全部作るのはコストに合わない場合がある
- 現状の実装は「常に WebP エンコード → WebP デコード → ColorImage」という往復パス
  で画質が量子化で劣化している

### 1.3 ユーザ要望

1. キャッシュ生成の ON/OFF 切り替え
2. 軽いファイルはキャッシュ無し、重いファイルだけキャッシュするような Auto モード
3. キャッシュが無くても画質を落とさず表示したい
4. 多様なハード環境 (SSD/HDD 混在、CPU 世代差) に追従したい

---

## 2. 本計測結果 (リリースビルド)

`bench_thumbs` を全お気に入り配下 19,324 フォルダ × ランダム 5 ファイル/フォルダで実行。
**77,377 ファイル** (画像 77,279 + 動画 98) を計測した。
ストレージは SSD/HDD 混在、CPU は RTX 4090 マシン。

### 2.1 データセットの特徴

| 項目 | 値 |
|------|-----|
| 総ファイル数 | 77,377 |
| FAIL | 5 (0.006%、全て 0 byte or 壊れファイル) |
| 拡張子分布 | JPEG 89.8%, PNG 8.5%, WebP 0.6%, BMP 0.6%, GIF 0.4%, 動画 0.1% |
| 元ファイル合計 | **28.5 GB** |
| 全件 WebP q=75 変換後合計 | **2.1 GB** (元の **7.4%**) |

### 2.2 処理時間の分布 (成功した画像 77,274 枚)

| 指標 | p50 | p75 | p90 | p95 | p99 | max |
|------|----:|----:|----:|----:|----:|----:|
| `dims_ms` (ヘッダのみ) | 11.8 | 16.6 | 19.8 | 22.2 | 38.9 | 3386 |
| `open_ms` (フルデコード) | **4.2** | 6.3 | **10.8** | 15.0 | 28.2 | 329.8 |
| `resize_ms` (Lanczos3 → 512) | **10.6** | 13.7 | 19.0 | 20.2 | 33.1 | 222.7 |
| `encode_ms` (WebP q=75) | **11.2** | 12.2 | 13.2 | 13.9 | 15.3 | 22.7 |
| **no-cache 合計** (open + resize) | **15.0** | 21.5 | 27.4 | 32.1 | 65.4 | 378.5 |
| **with-cache 合計** (+ encode) | **26.4** | 32.8 | 39.1 | 43.6 | 76.8 | 391.0 |

### 2.3 ファイルサイズ vs decode 時間 (強い相関)

| サイズ帯 | 件数 | p50 decode | p95 | max |
|----------|-----:|-----------:|----:|----:|
| < 100 KB | 12,423 | 1.2 ms | 3.6 | 57.7 |
| 100-500 KB | 45,632 | 4.0 ms | 13.9 | 110.9 |
| 500KB-1MB | 16,272 | 5.9 ms | 19.1 | 132.0 |
| 1-2 MB | 2,385 | 9.4 ms | 30.5 | 145.6 |
| **2-5 MB** | **483** | **26.0 ms** | **102.8** | **260.3** |
| 5-10 MB | 76 | 36.8 ms | 101.1 | 329.8 |
| 10+ MB | 3 | 40.0 ms | 124.6 | 134.0 |

**2 MB が明確な分岐点** — これ以上のファイルで急激に重くなる。

### 2.4 拡張子別の特徴

| 拡張子 | 件数 | p50 decode | p95 | max | 備考 |
|--------|-----:|-----------:|----:|----:|------|
| jpg | 69,445 | 4.3 ms | 14.3 | 185.8 | 標準的 |
| png | 6,542 | 3.7 ms | 17.9 | 329.8 | 標準的 |
| **webp** | **475** | **77.1 ms** | **116.2** | 189.9 | **ダントツで遅い** |
| bmp | 445 | 4.2 ms | 33.0 | 107.5 | 無圧縮 |
| gif | 336 | 0.7 ms | 3.0 | 26.6 | 小さく速い |

**重要: 既存 `.webp` ファイルのデコードは p50 77 ms と異常に遅い**。
恐らく AI 生成 (ComfyUI 出力) の大きな WebP。
「キャッシュ保存形式と同じフォーマットが元画像として遅い」という皮肉な状況。
これらは事前ヒューリスティックで**無条件キャッシュ対象**にすべき。

### 2.5 動画 (Windows Shell API)

| n | min | p50 | p90 | p95 | p99 | max |
|---:|---:|---:|---:|---:|---:|---:|
| 98 | 37 | **233** | 366 | 389 | 523 | 833 ms |

**動画は 100% が遅い** — min でも 37 ms、p50 で 233 ms。
Windows 側のサムネキャッシュが効いた状態でこの速度なので、**無条件キャッシュが妥当**。

### 2.6 Auto しきい値スタディ

`open_ms + resize_ms` (no-cache total) でしきい値判定した場合:

| threshold | cache 率 | cache 枚数 | 推定容量 |
|---:|---:|---:|---:|
| ≥ 10 ms | 69.9% | 54,019 | ~1.5 GB |
| ≥ 15 ms | 50.1% | 38,735 | ~1.1 GB |
| ≥ 20 ms | 28.5% | 22,001 | ~600 MB |
| **≥ 25 ms** | **15.7%** | **12,166** | **~330 MB** |
| ≥ 30 ms | 6.7% | 5,141 | ~140 MB |
| ≥ 40 ms | 1.9% | 1,460 | ~40 MB |

### 2.7 重要な発見

1. **デコードは想定以上に速い** — p50 = 4.2 ms、p90 でも 10.8 ms
2. **resize の方が open より重い** (p50: 10.6 vs 4.2 ms) — 512px Lanczos3 は入力サイズ依存
3. **encode (WebP) はほぼ一定の 11-12 ms** — キャッシュ生成の固定費
4. **no-cache の p50 15 ms → 8 並列で実効 ~2 ms/枚** → 1000 枚で 2 秒
5. **既存 `.webp` ファイルは p50 77 ms と異常**。サイズ判定で拾えない可能性あり → ext 特例必要
6. **動画は 100% 遅い**。無条件キャッシュ対象
7. **ファイルサイズと decode 時間は強い相関 (2 MB 境界)** — 事前ヒューリスティックに使える

---

## 3. 設計方針

### 3.1 3 モード制

```rust
pub enum CachePolicy {
    Off,     // 新規キャッシュ生成しない (既存キャッシュは読む)
    Auto,    // 初回 decode 時間 >= しきい値 なら保存
    Always,  // 現状の動作 (全ファイル保存)
}
```

**Auto モードの利点:**

- 実測ベースのしきい値はハードウェア非依存 (SSD/HDD・CPU 世代差に自動追従)
- 1 枚目のロードは表示のためにどうせ decode するので追加コストなし
- サイズベース判定より正確 (プログレッシブ JPEG や PNG 圧縮レベルの差も拾える)

**判定式 (確定):**

```rust
// 事前ヒューリスティック (実測前に確定)
let pre = ext == "webp" || file_size > 2_000_000 || kind == Video;

// 実測判定: 表示のためにどのみち open + resize はするので、その合計で閾値判定
let measured = (open_ms + resize_ms) >= settings.cache_threshold_ms;

let should_cache = match policy {
    Always => true,
    Off    => false,
    Auto   => pre || measured,
};
```

**しきい値のデフォルト値: 25 ms** (ユーザ設定で変更可能)

理由:
- 本計測で p75 = 21.5 ms, p90 = 27.4 ms の間
- この値で上位 15.7% (約 1.2 万枚/7.7 万枚) がキャッシュ対象
- 容量 ~330 MB — 全件 cache (2.1 GB) の 16%、良好なトレードオフ
- より厳しく (30 ms) / 緩く (20 ms) したいユーザは設定で調整可能

**設定 UI (環境設定ダイアログ):**

- `cache_policy` ラジオ: Off / Auto / Always
- `cache_threshold_ms` スライダー (Auto 時のみ活性): 範囲 10-100 ms、初期値 25、ステップ 5
- `cache_videos_always` チェック (Auto 時のみ表示): 初期 true

**動画の扱い:** 無条件キャッシュ (`cache_videos_always = true`)。
全計測で min 37 ms / p50 233 ms と重く、件数も少ないため判定無意味。

### 3.2 表示パイプラインの最適化 (段階 A)

#### 現状の無駄

```
image::open(path)                        ← フルデコード
  ↓
encode_thumb_webp (resize → WebP encode) ← 10-12 ms + 量子化
  ↓
decode_thumb_to_color_image (WebP decode)← 2-3 ms
  ↓
ColorImage を UI に送信
```

初回表示に使う ColorImage が無駄な往復を通り、WebP q=75 の量子化で画質も落ちている。

#### 改善案

```
image::open(path)
  ├─ (A) 表示用: resize(セル実寸) → ColorImage 直送 ← 高画質・優先
  └─ (B) キャッシュ用: resize(thumb_px) → WebP encode → DB 保存 ← バックグラウンド
```

- (A) を先に送信して UI を更新してから (B) を実行 → 体感速度向上
- Lanczos3 downsample 1 回だけで WebP 量子化無し → 画質向上
- **表示サイズ = セルの実ピクセルサイズ**を使う (2 列 4K での大きなセルでも高品質)

### 3.3 ページ単位の先読み & eviction (段階 B)

#### 現状の問題

- `App::thumbnails: Vec<ThumbnailState>` はフォルダ全件分の TextureHandle を保持
- eviction が一切ない。フォルダ切り替えまで全サムネが VRAM に居座る
- 512x512 RGBA = 1 MB/枚 → 1 万枚 = 10 GB VRAM
- 低スペック GPU (4 GB VRAM) では数千枚で OOM リスク

#### ページ単位方式

既存の `fs_cache` (フルスクリーン画像プリフェッチ) と同じパターンを流用:

```rust
pub struct Settings {
    // 主: ページ単位先読み
    pub thumb_prev_pages: u32,  // default: 2
    pub thumb_next_pages: u32,  // default: 4

    // 従: VRAM 安全ネット (段階 C)
    pub thumb_vram_cap_mb: u32, // default: 2048
}
```

毎フレームの処理:

```rust
let page_rows = (viewport_h / cell_h).ceil() as usize;
let page_items = page_rows * cols;
let cur_first = (scroll_offset_y / cell_h) as usize * cols;
let keep_start = cur_first.saturating_sub(prev_pages * page_items);
let keep_end = (cur_first + (1 + next_pages) * page_items).min(total);

// 範囲外を Evicted 化 → TextureHandle drop → VRAM 解放
// 範囲内の Pending/Evicted を load 依頼キューへ
```

**メリット:**

- メモリ使用量がフォルダサイズに依存しない
- 設定が直感的 (「前 2 / 後 4 ページ」)
- 挙動が予測しやすい
- 既存 `fs_cache` のパターン流用で実装複雑度低

**挙動:**

- スクロールで範囲外に出たセル → TextureHandle drop → 表示は Pending プレースホルダ
- 戻ると再ロード (catalog 経由なら 2-3 ms、直接なら 5-20 ms)
- 8 並列 + 既存の Phase 1/2 パイプラインで体感的には "じわっと埋まる" 程度

### 3.4 データ構造の変更

```rust
pub enum ThumbnailState {
    Pending,
    Loaded {
        tex: egui::TextureHandle,
        rendered_at_px: u32,   // この解像度で作った (upscale 要否判定)
        from_cache: bool,      // catalog から復元したか (段階 D 用)
    },
    Failed,
    Evicted,  // 範囲外に出て drop 済み。再要求可
}
```

### 3.5 アイドル時の画質向上 (段階 D, optional)

2 回目以降のフォルダ訪問では catalog がヒットするが、画質は WebP q=75 で固定される。
バックグラウンドが空いた時に元画像から再デコードして ColorImage を差し替える。

```
[x] アイドル時にサムネイルを高画質化 (元画像から再読み込み)
```

- Phase 3 として追加 (Phase 1/2 が空になったら起動)
- visible 優先、スクロール中は pause、フォルダ切替で即キャンセル
- `Loaded { from_cache: true }` のセルを対象に、元画像を再デコード
- **設定で ON/OFF 可能** (低スペック機では OFF 推奨)

---

## 4. Settings に追加する項目

```rust
pub struct Settings {
    // ── キャッシュ生成ポリシー ──
    pub cache_policy: CachePolicy,       // default: Auto
    pub cache_threshold_ms: u32,         // default: 25, range 10-100 (UI スライダー可変)
    pub cache_videos_always: bool,       // default: true
    pub cache_webp_always: bool,         // default: true (既存 .webp は事前判定対象)
    pub cache_size_threshold_bytes: u64, // default: 2_000_000 (2 MB 以上は事前判定対象)
    // (既存の thumb_px, thumb_quality は維持)

    // ── メモリ / 先読み ──
    pub thumb_prev_pages: u32,           // default: 2
    pub thumb_next_pages: u32,           // default: 4
    pub thumb_vram_cap_mb: u32,          // default: 1024 (本計測ベースの安全ネット)

    // ── 画質向上 (optional) ──
    pub thumb_idle_upgrade: bool,        // default: false (まず off で様子見)
}
```

### 4.1 VRAM キャップのデフォルト値根拠

ページ単位先読み (前 2 / 後 4 ページ) を前提にした試算:

| 表示条件 | 枚数 | 1 枚の VRAM | 合計 |
|---|---:|---:|---:|
| 4 列 × セル 400px | 140 | 0.6 MB | 84 MB |
| 10 列 × セル 200px | 700 | 0.15 MB | 105 MB |
| 2 列 4K × セル 1900px | 28 | 14 MB | 400 MB |

**デフォルト 1024 MB** で通常使用はすべてマージン込みで収まる。
設定選択肢: 256 / 512 / 1024 / 2048 / 4096 MB。

---

## 5. 段階的実装計画

1 つの PR にまとめるには大きすぎるので、段階ごとに独立して投入する。
各段階は単体で機能追加として成立する。

| 段階 | 内容 | 規模 | 依存 | 副作用 |
|------|------|------|------|--------|
| **A** | 表示用 ColorImage を cell size で直接生成、WebP encode は別経路 | 小-中 | なし | 画質向上のみ |
| **B** | ページ単位先読み + Evicted 状態追加 | 中 | A | メモリ挙動変化 |
| **C** | 3 モード制 (Off/Auto/Always) + Auto しきい値 | 小-中 | A | 既存動作は Always 相当で維持 |
| **D** | VRAM 安全ネット (cap) | 小 | B | 低スペック機の保険 |
| **E** | アイドル時の画質向上 | 中 | B, 設定で OFF 可 | 追加負荷 (opt-in) |

### 5.1 推奨順序

1. **A** を先に入れる (画質改善・副作用小) → v0.2.0 候補
2. **C** を追加 (ユーザに選択肢を与える) → v0.2.0 同時投入も可
3. **B** を次期で導入 (メモリ対応のメイン) → v0.3.0
4. **D** を同時または次期 → v0.3.0 or v0.3.1
5. **E** は更にその次 → v0.4.0

---

## 6. 検討メモ

### 6.1 本計測結果ベースで確定した事項

- **Auto モードのしきい値デフォルト: 25 ms** (`open_ms + resize_ms` で判定)
- **しきい値はユーザ設定可能**: 10-100 ms、5 ms ステップ、環境設定ダイアログのスライダー
- **事前ヒューリスティック (Auto モード時)**:
  - `ext == "webp"` → 無条件キャッシュ (元が p50 77 ms と異常に遅いため)
  - `file_size > 2 MB` → 無条件キャッシュ (サイズと decode 時間の相関が強い)
  - `kind == Video` → 無条件キャッシュ (min 37 ms / p50 233 ms)
- **VRAM cap デフォルト: 1024 MB** (低スペック向けに 256/512/1024/2048/4096 選択肢)

### 6.2 セルサイズ変更時の扱い

- 列数変更・ウィンドウリサイズで `rendered_at_px` と現セルサイズが乖離した場合
- 著しく小さい (<0.7 倍) → 再デコード要
- 著しく大きい (>2 倍) → 再デコードで VRAM 節約
- 範囲内 → GPU スケーリングに任せる
- 頻繁なリサイズは debounce (~300 ms)

### 6.3 page 境界でのスクロール挙動

- 現状の行スナップと整合させる必要あり
- page の切れ目で load が一気に走らないよう priority queue に
- visible → next → prev → next+1 → prev+1 ... の順

### 6.4 アイドル検知の指標

- `Phase 1/2` キューが空 `&&` スクロール停止から N ms 経過
- `ctx.input().pointer` の状態監視
- 節電モードでは自動 OFF?

### 6.5 既存キャッシュ DB との互換性

- Settings 変更で `CatalogDb::CATALOG_VERSION` を上げる必要があるか?
- 基本は下位互換を保ち、読み出しは従来通りで OK

---

## 7. 参考

- 本計測ツール: `src/bin/bench_thumbs.rs`
- 本計測結果: `bench_thumbs.tsv` (本計測終了後に追加分析)
- 既存カタログ設計: `docs/catalog-design.md`
- 全体仕様: `docs/spec.md`
