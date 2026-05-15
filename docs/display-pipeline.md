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

### 1.3.1 親コンテナの代表サムネ — 優先順位

親コンテナ (Folder/ZipFile/PdfFile) の代表サムネは次の順で決まる:

1. **手動ピン (`folder_thumb_pins.db`)** — ユーザーがアドレスバー 📌 ボタンや
   右クリックメニュー「📌 代表サムネに固定」で指定した子アイテム。`make_load_request`
   の `apply_folder_thumb_pin` が `LoadRequest` を target アイテム用に書き換え、cache
   key は `{base_key}#pin:{source_id}` に変わる (source_id = kind/rel/entry/page/
   mtime/size の compact 表現)。pin の付け替えや target ファイルの変更で
   source_id が自動的に変わるので古い WebP を catch しない。詳細:
   [virtual-folders.md §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)。
2. **自動代表選定 (`resolve_folder_thumb_image`)** — Settings の `folder_thumb_sort`
   (Numeric / Modified / etc.) + `folder_thumb_depth` (再帰深さ) で先頭画像を選び、
   通常のサムネ生成パイプラインに乗せる。pin が無い場合の既定動作。
3. **フォルダ / ZIP / PDF アイコン fallback** — 中身が空 / 全部エラーで上 2 段が失敗
   したときの最終フォールバック。`grid_item.rs` の draw_cell でアイコン表示。

**Video ピンの特殊経路**: pin source が動画の場合は `seed_folder_video_pin_thumbs`
が起動時に `video_pins` DB の抽出済み WebP を pinned cache key として catalog にミラー
seed する。worker は通常の cache_hit で取り出すので、Shell API を再呼び出ししない。
`skip_cache = false` 固定で idle quality-upgrade の対象外。**動画 folder pin は
`video_pins.db` に WebP がある (= フルスクリーンで `P` キー / HUD でフレーム保存済み)
場合のみ set 可能** — sidecar / Shell 抽出を UI スレッドで同期実行しないための仕様
(Codex post-merge P2)。詳細:
[virtual-folders.md §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)。

### 1.4 表示時の変換

サムネイルには以下が適用される:

| 変換 | 適用場所 | 備考 |
| --- | --- | --- |
| 回転 (DB) | 描画時の GPU 行列 | `get_rotation(idx)` で毎フレーム参照、結果は `rotation_cache` にキャッシュ |
| EXIF Orientation | **デコード時**に適用 (通常画像のみ) | ZIP/PDF 経由のエントリには適用不可 |
| プリセット補正 (色調のみ) | **UI スレッド同期適用** | `thumb_adjust_tex[idx]` に保持、§1.5 参照 |
| ポストフィルタ | **適用されない** | コスト/実装維持のためサムネは色調のみ |
| AI アップスケール | **適用されない** | 1 枚 10 秒級のためサムネでは非現実的 |

### 1.5 サムネイル補正パイプライン (色調)

「黄ばんだ紙のスキャンをモノクロ漫画補正して見る」等、サムネ一覧とフルスクリーンの
見え方を揃えるため、グリッド描画時に色調補正を適用する。LUT 路ならサムネサイズ
(600 px 級) で ~3ms/枚 で済むが、70 枚同時に UI スレッドで掛けると 200ms 級の
フリーズになるため、以下の構造にしている。

**対象**: 画像系グリッドアイテムのみ (`Image` / `ZipImage` / `PdfPage`)。
フォルダ / 動画 / ZipFile / PdfFile / ConvertibleArchive / ZipSeparator の
代表サムネには適用しない (`adjusted_tex` は `None` で素通し)。

**データ構造** (`App` 内):

- `thumb_pixels: HashMap<usize, Arc<ColorImage>>` — 補正のソースとなる生ピクセル。
  `poll_thumbnails` でテクスチャアップロードと同時に `Arc::new` し格納する。
  `keep_range` 外に出たら drop (§1.2 のエビクション時)。`is_video` な idx は対象外。
- `thumb_adjust_tex: HashMap<usize, TextureHandle>` — 補正済みテクスチャ。
  `effective_params(idx).is_color_identity()` が `true` の idx は**エントリを持たない**
  (= 生サムネがそのまま描画される)。ポストフィルタは判定に含めない (サムネ非適用のため)。

**適用タイミング**:

1. **可視セル**: `ui_main::render_grid` がセル描画直前に `maybe_apply_thumb_adjustment(idx)`
   を同期呼び出し。`thumb_adjust_tex[idx]` がすでにあるか identity ならスキップ。
2. **先読み分 (keep_range 内, 非可視)**: `update()` 終盤で `process_thumb_adjust_budget(ctx, 8)`
   が最大 8 枚/フレーム処理する (600px で ~3ms/枚 × 8 = 24ms 予算)。
3. **スライダードラッグ中** (`App::adjustment_dragging == true`): 両経路ともスキップし、
   `draw_cell` も `adjusted_tex = None` を渡して生サムネを表示。
4. **ドラッグ解放** (`adjustment_dragging` が `true → false` 遷移): `update_thumb_adjust_drag_state`
   が `thumb_adjust_tex.clear()` → 次フレームで visible 優先で再生成される。

**キャッシュ無効化** ([preset-and-adjustment.md §4](preset-and-adjustment.md) の早見表に追補):

- `clear_adjustment_caches(idx)` 経路で `thumb_adjust_tex[idx]` も落とす。
- `clear_all_adjustment_and_ai_caches(idx)` 経路も `thumb_adjust_tex[idx]` を落とす。
- バルク系 (`apply_params_to_all_pages` / `clear_all_page_params` / `copy_params_to_global`) は
  `thumb_adjust_tex.clear()` で全部落とす (対象 idx を絞る判定より単純かつ安全)。
- `start_loading_items` (フォルダ切替) で `thumb_pixels` と `thumb_adjust_tex` を全クリア。
- **ピクセル (`thumb_pixels`) は keep_range を出るまで保持**。補正テクスチャだけ
  捨てて差し替えるのが基本、ピクセルを捨てるのは範囲外 evict とフォルダ切替のみ。

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

**`keep_fullscreen_viewport_alive`** はフルスクリーン非アクティブ時 (`fullscreen_idx == None`)
に呼ばれ、`fs_viewport_shown == true` の 1 フレームだけ `Visible(false)` cmd を送って hidden 化
する責務を持つ。それ以外のアイドルでは何もしない (2026-05-10、hidden viewport 維持コスト削減)。
代償として `close_fullscreen` 後の再入場で 1x1 → フルサイズの DWM 遷移フラッシュが毎回出る。
詳細は [docs/ui-responsiveness.md §9](ui-responsiveness.md) を参照。

### 2.2 ロードスレッド

`App::start_fs_load` (app.rs) が std::thread::spawn で 1 枚ごとに spawn:

```
          ┌─ GridItem::Image      → image::open() → 失敗時 WIC フォールバック → EXIF 適用
          ├─ GridItem::ZipImage   → zip_loader で bytes 読み出し → image::load_from_memory
          │                          → 失敗時 WIC ストリームフォールバック (SHCreateMemStream)
          │                          ※EXIF Orientation 不可
          └─ GridItem::PdfPage    → pdf_loader::render_page (4096px、PDF ワーカープロセス)
                                     ※zoom 分析モードの時はさらに高解像度で再レンダリング

アニメーション (通常画像のみ):
  ├─ .gif      → fs_animation::decode_gif_frames
  └─ .png/APNG → fs_animation::decode_apng_frames

→ EXIF 適用済の DynamicImage に `clamp_dynamic_for_gpu` を掛けて長辺 8192 以内に縮小
   (wgpu デフォルト上限)。7K-9K クラスの画像で過去に UI スレッドで 5s 級の
   Triangle リサイズが走ってしまい応答なしになった実害から、worker で先に縮小する。

→ 結果: FsCacheEntry (Static / Animated / Failed) を fs_cache に格納
```

### 2.2.1 動画フルスクリーンとタイルモード

`GridItem::Video` は画像ロードワーカーを使わず、`App::start_fs_load` の動画分岐で
`FsCacheEntry::Video { player }` を直接 `fs_cache` に挿入する。Windows の native
presenter 有効時は `VideoPlayer` が `NativeVideoOutput` を持ち、動画フレームと
egui overlay は専用 HWND 側で表示される。フルスクリーン viewport は黒い backdrop と
focus/chrome 管理だけを担当する。

動画から動画へのフルスクリーンナビゲーションでは、Windows native presenter 経路で
`NativeVideoOutput::SwitchSource` を使い、HWND / D3D11 presenter / overlay を保持したまま
新しい `VideoPlayer` の source binding に差し替える。通常ナビゲーションでは 120ms の
quiet period を置き、その間は decoder を作らず `NativeOverlayNavigationPreview` が
移動先ファイル名と保存済み resume サムネ (無ければ黒背景) を表示する。quiet period 後に
最新 target だけを open し、`native_video_fast_swap_pending` が最初の native frame 表示まで
連続入力を抑制する。navigation preview も最初の native frame が実際に表示されるまで
残し、decoder startup 中に旧動画の frame が露出しないようにする。画像⇄動画の遷移はこの fast path の対象外で、従来どおり
`open_fullscreen` / `start_fs_load` 経路で扱う。

動画タイルモード (`video_tile_state`) は再生中動画の `VideoInfo` からタイムスタンプ列を
作り、`TileThumbnailWorker` が別 FFmpeg input でサムネイルを抽出する。タイルモードが
active の動画→動画ホイール移動でも同じ `SwitchSource` を使うが、
`video_tile_swap_pending` 中は preparing overlay を出し、新動画の `player.info()` 到着後に
新しい `VideoTileState` を構築してタイルを progressive に埋める。

タイルモードの fast path では保存済み resume 位置を新 `VideoPlayer` に渡すが、autoplay は false
に固定する。タイル解除後は resume 位置の静止状態を表示し、特定位置から見たい場合は
タイルクリックによる seek を使う。タイルクリックは autoplay 設定に関係なく、その位置へ
seek して再生開始する。切替元の `VideoPlayer` は cpal stream を同期 pause し、audio
buffer を明示クリアしてから native presenter を渡し、前動画の処理済み音声が短く漏れる
のを抑える。

#### 2.2.2 右パネルへの動的状態反映

`VideoInfo.dynamic: Arc<VideoDynamicState>` は decoder thread / present thread / UI で
共有する atomic 群。`VideoPlayer::open` で 1 度生成し、以下の経路で同じ Arc が伝搬する:

- `decoder::spawn` → `run_video_decode` (video-decode スレッド) が `deinterlace_status` /
  `interlace_detected` を更新
- `NativeVideoOutput::spawn` → `run_native_video_output` → `PresenterSourceState::new`
  → `NativeFullscreenPresentStats::new(dynamic)` で保持し、`record_present` が
  `present_path` を per-frame 更新
- 動画→動画 fast-swap 時は `SwitchSourcePayload.dynamic` に新 source の Arc が乗り、
  旧 presenter が新 PresenterSourceState (新 Arc を握る present_stats) に切り替わる

UI 側 (`app/native_video.rs::sync_native_video_metadata`) は overlay 同期時に Acquire load
して `NativeOverlayMetadata` の動的フィールド (`last_present_path` /
`deinterlace_status` / `interlace_detected`) に snapshot 化、右パネル
`overlay_draw.rs::draw_native_metadata_panel` が「フレーム表示」「デインターレース」行
として描画する。

per-frame 経路 (`d3d11_shared` / `cpu_upload`) はプレゼン側の判定で、デコード自体が HW
かどうかとは独立。デインターレース (FFmpeg `bwdif`) を使う場合は HW デコード後に D3D11
テクスチャを CPU メモリへ転送するため、`record_present` は `cpu_upload` を観測する
(= 右パネル「フレーム表示」が CPU、perf overlay の CPU カウンタが増える)。
既存の静的フラグ `gpu_path_active` (= GPU video device の有無) は能力フラグとして
右パネル「GPU経路」(改名前は「経路」) にそのまま残し、per-frame の動的状態は別行で
表示する。

**リサイズ実装 (`src/fast_resize.rs`)**:

リサイズは `image::imageops::resize` (スカラー) ではなく、`fast_image_resize`
(AVX2 / SSE4.1 SIMD) ラッパの `crate::fast_resize` 経由で呼ぶ。実測で 3-10 倍速く、
7K-9K クラスの画像でも数百 ms で完了する。フィルタは `Quality::Bilinear` (≈ Triangle) と
`Quality::Lanczos3` の 2 択。使用箇所:

- `clamp_dynamic_for_gpu` — GPU 上限クランプ (Bilinear、縮小前提で速度優先)
- `thumb_loader::resize_to_display_color_image` — 表示用サムネ (Lanczos3、品質重視)
- `catalog::encode_thumb_webp` — キャッシュ用サムネ (Lanczos3、品質重視)

新規リサイズ経路を増やすときは `image::DynamicImage::resize(_exact)` を直接呼ばず、
`fast_resize::resize_dynamic_fit` / `resize_dynamic_exact` を使うこと。`image` crate の
`resize` は UI スレッドに乗ったとき秒単位の応答なしを招きやすい。

**GPU テクスチャ上限の規約 (MAX_TEXTURE_DIM = 8192)**:

- `fs_cache` / `ai_upscale_cache` / `adjustment_cache` に入る `Static.pixels` は
  **常に 8192px 以内**。worker 側 `clamp_dynamic_for_gpu` で担保される。
- UI スレッドの `clamp_for_gpu(&ColorImage)` は異常経路の安全網。通常パスでは
  `Cow::Borrowed` で返り、Triangle リサイズは走らない。発動したらログに
  `clamp_for_gpu (UI-thread fallback)` が出る。
- AI アップスケールは `ai_upscale_skip_px` (既定 2048) で長辺 2047 以下のみ処理、
  ×4 倍で最大 8188 なので 8192 を越えない。
- `apply_sync_adjustment` は pointwise 変換なので入力サイズを保つ → 入力が 8192 以内
  ならば出力も 8192 以内。AI 結果 / fs_cache の `pixels` を入力に取るので成立する。
- 消しゴム (MI-GAN) / PDF 再レンダ (`request_pdf_rerender` の `.clamp(256, 8192)`) も
  同じ上限を尊重する。
- GIF / APNG アニメーションは `fs_animation::clamp_rgba_frame_for_gpu` で各フレームを
  `MAX_TEXTURE_DIM` 以下に縮めてから `ColorImage` 化する (巨大 animated 画像で
  `ctx.load_texture` が panic するのを防ぐ安全網)。

新しい経路で `FsCacheEntry::Static` を作るときは、`pixels` が 8192 以内であることを
自分で保証するか、`clamp_dynamic_for_gpu` を掛けてから格納する。UI スレッド側の
同期 Triangle リサイズを増やさないこと。

**原寸表示とダウンスケール警告 (`source_dims`)**:

`FsCacheEntry::Static.source_dims: Option<[usize; 2]>` は **clamp 前** の原寸。
fs_load ワーカーが `clamp_dynamic_for_gpu` を掛ける直前に記録して送る。ホバーバーの
画像サイズ表示はこれを優先して使い、`pixels.size` と不一致なら「⚠ ダウンスケール
表示中」マーカーを出す (利用者が縮小表示に気づけるように)。

派生キャッシュ (`ai_upscale_cache` / `adjustment_cache`) や消しゴム再挿入の entry は
`source_dims: None` で良い。ホバー UI は `fs_cache` 側のエントリから原寸を読むため、
派生側は参照されない。ただし消しゴム inpaint / マスク解除で `fs_cache` を上書きする
ケースは既存 entry の `source_dims` を必ず引き継ぐこと (上書きで原寸情報が消えて
警告が出なくなる事故を防ぐ)。

**先行 dims ヒント (`fs_early_dims` / `FsLoadResult::DimsOnly`)**:

フルデコードには数百 ms-数秒かかるが、画像の**寸法だけ**ならヘッダ数バイトで
取れる (PNG の IHDR、JPEG の SOF マーカー等、通常 2-10 ms)。この時間差を活かして
「ロード中はホバーバーに何も出ない」状態を短縮する経路を設けている:

1. `start_fs_load` ワーカーは perf `decode_begin` 直後、ローカル画像パス限定で
   `fast_resize::probe_dims(&path)` を呼び、成功したら `FsLoadResult::DimsOnly {
   source_dims }` を先行送信する (PDF / ZIP は probe がそもそも遅いので対象外)。
2. `poll_prefetch` は各 `fs_pending` に対して **drain ループ** で try_recv を回し、
   `DimsOnly` を受信したら `App::fs_early_dims: HashMap<usize, [usize; 2]>` に格納し、
   fs_pending はそのまま残す (本デコードが続く)。
3. ホバーバー (`build_fs_frame_state`) は `fs_cache` にエントリがないケースで
   `fs_early_dims` を見に行き、あれば原寸をそのまま表示。原寸が MAX_TEXTURE_DIM を
   超えていればダウンスケール警告もこの時点で出せる (本デコード完了まで待たない)。
4. 本体 (`Static` / `Animated` / `Failed`) 受信で `fs_early_dims[idx]` は削除される。
   `load_folder` / キャンセル時も一緒に drop して HashMap が肥大化しないようにする。

この設計は `DimsOnly` 省略時 (probe 失敗 / PDF / ZIP) でも問題なく動く。drain ループが
終端メッセージ 1 個で `completed` に積んで抜けるだけなので、従来挙動と互換。

### 2.3 表示テクスチャの優先順位 (決定版)

`ui_fullscreen.rs` はフレームごとに以下の順で「今表示するテクスチャ」を選ぶ:

```
0. 右 Ctrl ホールド中の元画像プレビュー (消しゴム補完前の erase_base_cache or fs_cache)
1. erase モードで編集中のマスクプレビュー   (ui_erase.rs)
2. adjustment_cache[idx]                   (プリセット補正済み)
3. ai_upscale_cache[idx]                   (AI アップスケール/デノイズ済み)
4. fs_cache[idx]                           (生デコード結果)
5. フォールバック: サムネイル (低解像度)
```

**この優先順位は動かさないこと**。変更すると「補正を掛けた瞬間に一瞬生画像が見える」
「AI 処理中にプリセットを変えると AI 結果で上書きされる」等の不整合が出る。

右 Ctrl ホールドの元画像プレビューは例外的な一時表示で、派生キャッシュは作り直さない。
通常の画像 / ZIP 内画像 / PDF ページだけを対象にし、動画には適用しない。消しゴム補完済みの
ページでは `erase_base_cache` から補完前のテクスチャを lazy 作成し、それ以外は `fs_cache`
の生デコード結果をそのまま使う。

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

Spread モード (見開き) の場合は、`draw_fs_spread` が `resolve_spread_pair` で左右の idx と配置
(LTR/RTL/Cover) を決め、両ページを「1 枚の合成画像」とみなしてレイアウトする:

1. 各ページの表示サイズ (回転考慮) を算出し、高い方に揃えた連結幅・高さを計算
2. `image_rect` にフィットする `fit_scale` を求める
3. ズーム/パンを `(fit_scale * fs_zoom, image_rect.center() + fs_pan)` として合成し、合成中心から
   左右ページ矩形を配置する (ズーム/パンは左右ページで共有、ページ間の分割位置は不変)
4. ズーム/パンが有効なフレームでは `image_rect` にクリップして他の UI 領域へのはみ出しを防ぐ

見開き中は `fs_free_rotation` (Ctrl+ドラッグのフリー回転) と `rotation_db` の単独ページ回転 (R/L)
は描画に反映されないため、Ctrl+ドラッグは `handle_fs_wheel_and_click` 側で no-op にしている。
ズーム中のパン (非修飾ドラッグ) と Ctrl+ホイールズーム、ダブルクリックリセットのみが見開きで有効。

---

## 3. 補正・AI・ポストフィルタキャッシュと再描画

詳細は [preset-and-adjustment.md](preset-and-adjustment.md) に譲る。ここでは要点のみ:

- **補正 (adjustment)**: CPU 側で LUT 計算 → ColorImage → [ポストフィルタ (post_filter::apply)]
  → GPU テクスチャ。`maybe_apply_adjustment(idx)` が毎フレーム呼ばれ、必要なら同期的に適用する。
- **ポストフィルタ**: 色調補正の後段で CPU 処理 (CRT/減色/複合)。rayon 並列化で 4K 画像でも
  40〜80ms 程度。`PostFilter::None` 以外はテクスチャサンプラーを NEAREST にして
  スキャンライン/ドットを維持する。
- **消しゴム/分析モード中の一時バイパス**: `App::post_filter_bypassed = true` の間は
  `apply_sync_adjustment` が post-filter 段をスキップし color-only の `adjustment_cache` を生成。
  モード解除時に false に戻し該当 idx をクリアして post-filter 適用状態で再生成させる。
- **AI アップスケール/デノイズ**: 別スレッドで推論。完了時に `ai_upscale_cache` に格納。
- **元画像プレビュー**: 右 Ctrl を押している間だけ描画時のテクスチャ選択を
  `fs_cache` / `erase_base_cache` に切り替える。DB・補正設定・AI queue は変更しない。
- **何かを変えたら正しいキャッシュをクリア**:
  - 補正パラメータ変更 → `adjustment_cache[idx]` のみクリア
  - ポストフィルタ変更 → `adjustment_cache[idx]` のみクリア (色系変更と同じ扱い)
  - AI モデル変更 → `adjustment_cache` + `ai_upscale_cache` 両方をクリア + 実行中ジョブをキャンセル
  - 消しゴム/分析モード入出 → `adjustment_cache[idx]` のみクリア (bypass 切替のため)
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
- 逆も同様。サムネは**色調のみ**を適用する (§1.5)。ポストフィルタ / AI は意図的に非対象。
  フルスクリーンでポストフィルタ / AI を掛けても、サムネは色調止まり — ユーザの
  「ざっくり一覧で雰囲気をつかむ」用途に揃えた割り切り。

### 4.3 キャンセル安全性

ワーカー内部のループは以下を頻繁にチェック:

- `cancel_token` (フォルダ切替時に true になる)
- `keep_range` (自分の idx が範囲外なら結果を捨てる)

新しいワーカーを追加するときは同じパターンに従う。詳細は [async-architecture.md](async-architecture.md)。
