# 表示パイプライン (サムネイル / フルスクリーン)

**これが一番事故が多い領域**。画像補正・AI アップスケール・回転・アニメーション・消しゴムマスクのすべてが、
「どのテクスチャを画面に出すか」を巡って絡み合う。修正前にこのドキュメントを読むこと。

---

## 1. サムネイル表示パイプライン

### 1.1 状態機械

`GridItem` 1 個につき 1 つの `ThumbnailState` (grid_item.rs) を持つ:

```
Pending ──────────(ワーカーがデコード)──────────▶ Loaded { tex, from_cache, rendered_at_px, source_dims }
                                                       │
                                           (keep_range 外に出ると)
                                                       ▼
                                                    Evicted ─(再び可視範囲へ)─▶ Pending (再要求)
```

Failed は単発の終端ステート。デコードエラー時のみ。

### 1.2 2 フェーズ優先ロード

`App::update()` 毎フレーム:

1. **keep_range の再計算** (`update_keep_range_and_requests` in `app.rs`)
   - 可視範囲 + `thumb_prev_pages` + `thumb_next_pages` を含む範囲を算出
   - `keep_start_shared` / `keep_end_shared` (`Arc<AtomicUsize>`) に書き込み → ワーカーが参照
2. **エビクション**: keep_range 外の Loaded を `Evicted` に遷移 (GPU テクスチャを drop)
3. **要求投入**: keep_range 内の Pending / Evicted に対して `LoadRequest` を作り
   - 通常キュー: `reload_queue` (Image/ZipImage/PdfPage)
   - 重 I/O キュー: `heavy_io_queue` (Folder/ZipFile/PdfFile — 全体走査が必要)
4. **アイドル時品質アップグレード**: スクロールが止まって ~1 秒経つと、`from_cache: true` の Loaded に対して `skip_cache: true` で再要求 → 高品質デコード

### 1.3 ワーカー側の流れ

`thumb_loader.rs::process_load_request`:

```
1. キャッシュ DB (catalog.db) に該当エントリがあるか確認
   ├─ ヒット (skip_cache=false): WebP バイト → ColorImage
   └─ ミス or skip_cache=true:
        ├─ ソースデコード (JPEG=turbojpeg, PNG/GIF/WebP/BMP=image crate,
        │                   HEIC/AVIF/JXL/RAW=WIC, PDF=PDFium ワーカー)
        ├─ EXIF Orientation 適用 ※通常画像のみ (ZIP/PDF 不可)
        ├─ Lanczos3 で display_px までリサイズ
        ├─ CacheDecision::should_cache でキャッシュ可否判定
        └─ 必要なら WebP エンコードして catalog.db に保存
2. mpsc で (idx, ColorImage, from_cache, source_dims) を送信
```

### 1.4 表示時の変換

**サムネイルに補正は掛からない**。適用されるのは:

| 変換 | 適用場所 | 備考 |
| --- | --- | --- |
| 回転 (DB) | 描画時の GPU 行列 | `get_rotation(idx)` で毎フレーム参照、結果は `rotation_cache` にキャッシュ |
| EXIF Orientation | **デコード時**に適用 (通常画像のみ) | ZIP/PDF 経由のエントリには適用不可 |
| プリセット補正 | **適用されない** | パネル UI ではプリセット割当バッジのみ表示 |
| AI アップスケール | **適用されない** | |

---

## 2. フルスクリーン表示パイプライン

### 2.1 エントリポイント

```
ユーザーが Enter / ダブルクリック
    │
    ▼
App::fullscreen_idx = Some(idx)
    │
    ▼  (次フレーム)
ui_fullscreen.rs::render_fullscreen_viewport
    │
    ├─ fs_cache[idx] がなければ → start_fs_load(idx) を発火
    ├─ テクスチャ選択 (後述の優先順位)
    ├─ spread_mode に応じて 1 枚 or 2 枚並べる
    ├─ rotation + zoom + pan + free_rotation を合成して描画
    └─ update_prefetch_window(idx)     # 前後数枚を先読み / 範囲外を解放
```

### 2.2 ロードスレッド

`App::start_fs_load` (app.rs) が std::thread::spawn で 1 枚ごとに spawn:

```
          ┌─ GridItem::Image      → image::open() → 失敗時 WIC フォールバック → EXIF 適用
          ├─ GridItem::ZipImage   → zip_loader で bytes 読み出し → image::load_from_memory
          │                          ※WIC 不可 (パスが必要)、EXIF 不可
          └─ GridItem::PdfPage    → pdf_loader::render_page (4096px、PDF ワーカープロセス)
                                     ※zoom 分析モードの時はさらに高解像度で再レンダリング

アニメーション (通常画像のみ):
  ├─ .gif      → fs_animation::decode_gif_frames
  └─ .png/APNG → fs_animation::decode_apng_frames

結果: FsCacheEntry (Static / Animated / Failed) を fs_cache に格納
```

### 2.3 表示テクスチャの優先順位 (決定版)

`ui_fullscreen.rs` はフレームごとに以下の順で「今表示するテクスチャ」を選ぶ:

```
1. erase モードで編集中のマスクプレビュー   (ui_erase.rs)
2. adjustment_cache[idx]                   (プリセット補正済み)
3. ai_upscale_cache[idx]                   (AI アップスケール/デノイズ済み)
4. fs_cache[idx]                           (生デコード結果)
5. フォールバック: サムネイル (低解像度)
```

**この優先順位は動かさないこと**。変更すると「補正を掛けた瞬間に一瞬生画像が見える」
「AI 処理中にプリセットを変えると AI 結果で上書きされる」等の不整合が出る。

### 2.4 変換の合成順序

描画時、`draw_fs_image` は以下の順で変換を掛ける:

```
1. テクスチャ選択 (上記の優先順位)
2. 回転 (rotation_db, 0/90/180/270)
3. ユーザーのフリー回転 (fs_free_rotation, 一時的・非永続)
4. Zoom (fs_zoom, 0.1〜50.0)
5. Pan (fs_pan)
6. アスペクト比フィット (余白はレターボックス)
```

Spread モード (見開き) の場合は、この処理を左右の画像それぞれに行ってから並べる。
`resolve_spread_pair` が左右の idx と配置 (LTR/RTL/Cover) を決める。

---

## 3. 補正・AI キャッシュと再描画

詳細は [preset-and-adjustment.md](preset-and-adjustment.md) に譲る。ここでは要点のみ:

- **補正 (adjustment)**: CPU 側で LUT 計算 → ColorImage → GPU テクスチャ。
  `maybe_apply_adjustment(idx)` が毎フレーム呼ばれ、必要なら同期的に適用する。
- **AI アップスケール/デノイズ**: 別スレッドで推論。完了時に `ai_upscale_cache` に格納。
- **何かを変えたら正しいキャッシュをクリア**:
  - 補正パラメータ変更 → `adjustment_cache[idx]` のみクリア
  - AI モデル変更 → `adjustment_cache` + `ai_upscale_cache` 両方をクリア + 実行中ジョブをキャンセル
  - フォルダ切替 → 両方をグローバルクリア
  - 回転変更 → **キャッシュはクリアしない** (GPU 行列で回すため)

---

## 4. 分岐チェックリスト

修正時、以下の観点で漏れが出やすい。どれかを触るなら全部を確認する:

### 4.1 画像種別分岐

| 処理 | Image | ZipImage | PdfPage | Video |
| --- | --- | --- | --- | --- |
| サムネイルデコード | image/turbojpeg/WIC | image::load_from_memory | PDFium ワーカー | Shell API (別スレッド) |
| フルスクリーンデコード | 同上 + EXIF + 動画判定 | bytes から decode のみ | PDFium で 4096px | なし (サムネのみ) |
| EXIF Orientation | ✅ | ❌ (パス不可) | ❌ | — |
| アニメーション | GIF/APNG のみ ✅ | ❌ | ❌ | — |
| 回転 (rotation_db) | ✅ | ✅ (path+entry キー) | ✅ (path+page キー) | — |
| プリセット補正 | ✅ | ✅ | ✅ | — |
| AI アップスケール | ✅ | ✅ | ✅ | — |
| 消しゴム (inpaint) | ✅ | ✅ | ✅ | — |

### 4.2 サムネイル / フルスクリーンの整合性

- サムネイルに適用する変換を増やすなら、フルスクリーン側も同じ処理が走っているか確認
- 逆も同様。**フルスクリーンで補正が効くのにサムネは生のまま** という現状は仕様なので、サムネ側に安易に追加しない (グリッド全体で CPU/GPU 負荷が跳ね上がる)

### 4.3 キャンセル安全性

ワーカー内部のループは以下を頻繁にチェック:

- `cancel_token` (フォルダ切替時に true になる)
- `keep_range` (自分の idx が範囲外なら結果を捨てる)

新しいワーカーを追加するときは同じパターンに従う。詳細は [async-architecture.md](async-architecture.md)。
