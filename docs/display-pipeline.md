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
   - 重 I/O キュー: `heavy_io_queue` (Folder/ZipFile/ConvertibleArchive/ZipDir — 全体走査、
     ZIP セントラルディレクトリ読み、または ZIP 内 prefix の代表解決が必要。PdfFile は PDF ワーカー IPC なので通常キュー)
4. **アイドル時品質アップグレード**: スクロールが止まって ~1 秒経つと、`from_cache: true` の Loaded に対して `skip_cache: true` で再要求 → 高品質デコード

### 1.3 ワーカー側の流れ

`thumb_loader.rs::process_load_request`:

```
1. キャッシュ DB (catalog.db) に該当エントリがあるか確認
   ├─ ヒット (skip_cache=false): WebP バイト → ColorImage
   └─ ミス or skip_cache=true:
        ├─ ソースデコード (JPEG=turbojpeg, PNG/GIF/WebP/BMP=image crate,
        │                   HEIC/AVIF/JXL/RAW=WIC, PDF=PDFium ワーカー)
        ├─ EXIF Orientation 適用 (通常画像=path / ZIP=bytes、PDF は対象外)
        ├─ Lanczos3 で display_px までリサイズ
        ├─ CacheDecision::should_cache でキャッシュ可否判定
        └─ 必要なら WebP エンコードして catalog.db に保存
2. mpsc で (idx, ColorImage, from_cache, source_dims) を送信
```

### 1.3.1 親コンテナの代表サムネ — 優先順位

親コンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) の代表サムネは次の順で決まる:

1. **手動ピン (`folder_thumb_pins.db`)** — ユーザーがアドレスバー 📌 ボタンや
   右クリックメニュー「📌 代表サムネに固定」で指定した子アイテム。`make_load_request`
   の `apply_folder_thumb_pin` が `LoadRequest` を target アイテム用に書き換え、cache
   key は `{base_key}#pin:{source_id}` に変わる (source_id = kind/rel/entry/page/
   mtime/size の compact 表現)。pin の付け替えや target ファイルの変更で
   source_id が自動的に変わるので古い WebP を catch しない。詳細:
   [virtual-folders.md §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)。
2. **自動代表選定 (`resolve_folder_thumb_image`)** — Settings の `folder_thumb_sort`
   (Numeric / Modified / etc.) + `folder_thumb_depth` (再帰深さ) で先頭画像を選び、
   通常のサムネ生成パイプラインに乗せる。pin が無い場合の既定動作。キャッシュ
   ヒット時は表示速度を優先して毎回の再スキャンは行わないが、cache key には
   自動選定アルゴリズム版・sort・depth を含めるため、番号順ロジックや設定が
   変わったときは自然にミスして再スキャンされる。キャッシュミス時の Folder
   自動選定は、グリッドのブロック順に揃えて「サブフォルダ (folder_thumb_sort 順) →
   直接画像 (sort 順)」で候補を辿る。
   ConvertibleArchive は有効な変換キャッシュ ZIP がある場合だけ、その ZIP の先頭画像を
   `archivethumb:{format}:{identity}` キーで読む。キャッシュ未作成/失効時は要求を出さず
   アイコンに戻す。
3. **フォルダ / ZIP / PDF / アーカイブアイコン fallback** — 中身が空 / 全部エラーで上 2 段が失敗
   したときの最終フォールバック。`grid_item.rs` の draw_cell でアイコン表示。

**Video ピンの特殊経路**: pin source が動画の場合は `seed_folder_video_pin_thumbs`
が起動時に `video_pins` DB の抽出済み WebP を pinned cache key として catalog にミラー
seed する。worker は通常の cache_hit で取り出すので、Shell API を再呼び出ししない。
`skip_cache = false` 固定で idle quality-upgrade の対象外。**動画 folder pin は
`video_pins.db` に WebP がある (= フルスクリーンで `P` キー / HUD でフレーム保存済み)
場合のみ set 可能** — sidecar / Shell 抽出を UI スレッドで同期実行しないための仕様
(Codex post-merge P2)。詳細:
[virtual-folders.md §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)。

**ドライブ一覧用の保存例外**: ドライブルート (`C:\` 等) を通常フォルダとして
表示している間、そのドライブの手動ピン代表そのもののセルだけは
`LoadRequest::force_cache` を立て、`CachePolicy` が Auto/Off でも親 catalog に
サムネを残す。ドライブルート catalog はドライブ文字を保持して分離し、直下同名項目の
取り違えを防ぐ。ドライブ一覧ビュー自体はこの catalog / `video_pins.db` を cache-only で
読むだけで、表示時にドライブルートの `read_dir`、代表探索、metadata 確認、ピン先
デコードは行わない。root catalog に正しいフォルダタイルが無い場合は、子 catalog へ
深掘りせずアイコン fallback にする。

### 1.4 表示時の変換

サムネイルには以下が適用される:

| 変換 | 適用場所 | 備考 |
| --- | --- | --- |
| 回転 (DB) | 描画時の GPU 行列 | `get_rotation(idx)` で毎フレーム参照、結果は `rotation_cache` にキャッシュ |
| EXIF Orientation | **デコード時**に適用 | 通常画像はファイル path、ZIP 内画像はエントリ bytes から読む。PDF は対象外 |
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
    └─ update_prefetch_window(idx)     # フィルタ後の前後数枚を先読み / 範囲外を解放
```

**`keep_fullscreen_viewport_alive`** はフルスクリーン非アクティブ時 (`fullscreen_idx == None`)
に呼ばれ、`fs_viewport_shown == true` の 1 フレームだけ `Visible(false)` cmd を送って hidden 化
する責務を持つ。それ以外のアイドルでは何もしない (2026-05-10、hidden viewport 維持コスト削減)。
再入場時は `render_fullscreen_viewport` が新規 viewport を hidden で作成し、DWM transition
抑止属性を適用してから `Visible(true)` にする。アイドル時の hidden viewport 維持コストを
戻さずに、初期 white client / サイズ遷移フラッシュを抑える。
詳細は [docs/ui-responsiveness.md §9](ui-responsiveness.md) を参照。

Ctrl+↑↓ のフォルダ横断では、遷移開始時に `fs_holdover_tex` へ旧ページのテクスチャを
保持する。ただしこれは、新 target がまだ active になっていない間、または PDF/ZIP
enumerate defer で `fullscreen_idx == None` の間に限って描く。`items_generation` が進み、
新しい `fullscreen_idx` が入った後は `fs_nav_holdover_tex_for_draw` が旧テクスチャを
返さず、サムネ/本画像が来るまで loading 表示に落とす。これにより、タイトルバーや
パス表示だけ新しい ZIP を指しているのに画像だけ前の 7z のまま残る状態を防ぐ。

フルスクリーンの先読み対象は、`items` 全体ではなく `visible_indices` 由来の display list から
作る。★フィルタや Ctrl+F で一覧が疎になっているときも、スライドショー / 前後移動と同じ
次候補を先読みし、フィルタで隠れた画像は対象にしない。

### 2.2 ロードスレッド

`App::start_fs_load` (app.rs) が std::thread::spawn で 1 枚ごとに spawn:

```
          ┌─ GridItem::Image      → image::open() → 失敗時 WIC フォールバック → EXIF 適用
          ├─ GridItem::ZipImage   → zip_loader で bytes 読み出し → image::load_from_memory
          │                          → 失敗時 WIC ストリームフォールバック (SHCreateMemStream)
          │                          → bytes から EXIF Orientation 適用
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

### 2.2.1 ViewerPresentation / detached viewer

v1.4.0 後の別ウィンドウ対応では、同じ viewer session をどこへ表示するかを
`ViewerPresentation::{MainWindow, Fullscreen, DetachedWindow}` で扱う。

現時点の実装は、`open_fullscreen` 入場時に `requested_viewer_presentation_for_open`
で F12 の `detached_viewer_enabled` を反映した要求表示先を導出し、
`effective_viewer_presentation_for_open` で実表示先を決めて `App.viewer_presentation`
へ保持する。静止画 / ZIP画像 / PDFページ / 動画は `DetachedWindow` が有効で、
`render_fullscreen_viewport` の描画本体を装飾付き・taskbar 表示ありの通常 viewport へ
出す。動画もこの egui detached viewport を安定した host として使い、
`NativeVideoPlacement::DetachedViewerChild` と `NativeVideoWindowMode::Child` で
host HWND のクライアント領域へ native presenter を重ねる。DComp presenter /
decoder / audio clock は保持したまま `SwitchPlacement` で MainWindow / Fullscreen /
Detached を切り替える。

`prepare_viewer_presentation_open` / `prepare_viewer_presentation_close` は、main HWND
cloak、native 動画の DWM chrome、foreground reclaim など fullscreen takeover 前提の
処理をまとめた境界。detached viewer では main HWND を cloak せず、fullscreen foreground
reclaim も予約しない。detached session はメイン一覧をブロックしないため、root viewport 側の
通常ショートカットやダイアログは継続して扱える。detached 表示の `×` / Esc / Enter は
`close_fullscreen()` に寄せ、`detached_viewer_enabled` は維持する。

ボーダーレス fullscreen session 中に main viewport がフォーカスを得た場合、既定では
「一覧へ戻りたい」意図とみなして `close_fullscreen()` する。環境設定
`fullscreen_keep_on_app_switch` (表示名: 「メインに戻ったらフルスクリーンへ復帰」) が ON
のときは、Alt+Tab などで mIV の main viewport へ戻っても閉じず、fullscreen viewport /
native presenter 側へ focus / raise を戻す。main/root 側へ届いた fullscreen 操作用キーは
`handle_fullscreen_root_key_input` で先に処理する。メイン一覧も並行操作したい場合は
detached viewer (F12) を使う。

F11 の MainWindow / Fullscreen 選択は、F12 の detached ON/OFF とは独立した
non-detached 側の表示設定として保持する。動画の native presenter から
`DetachedWindow` への `PlacementSwitched` が返っても `settings.video_in_window_mode`
は更新せず、F12 を OFF にしたときは直前の F11 状態へ戻る。静止画 detached 中の
F11 は non-detached 設定を更新せず、通常配置を保持したまま装飾なし・モニター全体の
仮想フルスクリーンをトグルする。動画 detached 中の F11 は無効のままにする。
静止画の fullscreen viewport と detached viewport は装飾・サイズ・taskbar 表示が異なるため、表示形態が
変わるときは既存 viewport を隠して ViewportId 世代を進め、新しい表示先として作り直す。
静止画 / PDF / ZIP 画像の fullscreen viewport と detached viewport はどちらも hidden
状態で作成し、`DWMWA_TRANSITIONS_FORCEDISABLED` を適用してから `Visible(true)` を送る。
これにより OBS などの window capture には映らない DWM の表示フェード / 出現アニメーションを
抑止する。

静止画 fullscreen viewport は true fullscreen API へフォールバックせず、対象モニターの
論理矩形へ装飾なし viewport を配置する。Windows 11 仮想デスクトップで静止画 fullscreen
viewport が現在デスクトップへ付いてくる症状を避けるため、捕捉できた fullscreen viewport
HWND は main HWND と同じ仮想デスクトップへ best-effort で同期する。動画 fullscreen は
native presenter 経路であり、この egui viewport 同期の対象外。

複数 viewport を閉じる / 作り直す直後は、egui の shared texture namespace と viewport ごとの
renderer 側 font atlas texture のサイズが一時的にずれることがある。実機では、日本語フォルダ名の
描画タイミングで `Queue::write_texture` が高さ 32 の古い font atlas texture に `Y 29..44`
の部分 glyph update を送り、フォント崩れ後に wgpu validation panic するログを確認した。
`Visible(false)` で fullscreen/detached viewport を隠した経路では
`request_main_font_atlas_resync` を立て、`configure_fonts_for_texture_resync` で font atlas を
full upload から再開させる。通常の `configure_fonts` は定義が同一だと egui が再読み込みを
省略するため、未使用の一意な font family marker を混ぜて強制リロードさせる。

`configure_fonts_for_texture_resync` の `set_fonts` は egui 仕様で「次 pass の begin_pass で
適用」されるため、フル atlas アップロードが効く前にメイン UI を描くと上記 panic が再発する。
`maybe_defer_for_main_font_atlas_resync` は、`should_defer_main_paint_for_font_atlas_resync` が
true を返す経路 (`fullscreen_viewport_cleanup` / `native_video_backdrop_hide` /
`fullscreen_viewport_recreate`) では `ctx.request_discard()` でその pass を破棄し、egui の
**同一 OS フレーム内 multi-pass** (`max_passes`=2) で `update()` を再走させる。次 pass の
begin_pass で新フォントが適用され、フル atlas アップロードがメイン UI 描画より前に入る。
破棄された pass は paint されないので黒/空白フレームは出ず、2 度目の pass は入力 events が空
(`RawInput::take` 済み) なので close / ナビ等の入力起因処理も二重発火しない。`will_discard()`
が false (multi-pass 予算なし) の稀なケースだけ、従来どおりこの pass の描画を飛ばして次フレーム
で再描画する「1 フレーム黒」フォールバックに落ちる。detached cleanup
(`should_defer_…` が false) は defer せずそのまま描く。

なお、専用ボーダーレス fullscreen viewport を閉じると、別トップレベル窓の `Visible(false)` が
DWM で反映されるまで約 1 フレームかかるため、メインウィンドウが手前に合成されてから fullscreen
窓 (黒) が消えるまでの間、メイン周囲に一瞬黒地が残る。これは上記の panic / 黒コンテンツ問題とは
別の DWM 表示タイミング由来で、単一 viewport 化以外では完全には消せないため現状は許容している。

detached session が開いている間は、`App::update` 終端でそのフレームの最終
`selected` を読み、静止画 / ZIP画像 / PDFページ / 動画であれば viewer 側を同じ項目へ追従させる。
同期済み判定は `idx` だけでなく `metadata_cache_key` 相当の item key と
`items_generation` を含む stamp で行う。これによりフォルダ切替や ZIP/PDF 仮想フォルダの
items rebuild 後に、同じ idx が別項目を指しても再同期できる。detached window が閉じている
場合は、メイン一覧のカーソル移動だけでは再表示しない。
同じ raw idx で stamp が変わった場合は、動画 fast-swap の same-idx no-op を通さず
`open_fullscreen` の通常初期化へ戻して再同期する。

メイン一覧で `Enter` / 明示 open した項目が、開いている detached session の stamp と
同一なら、`open_fullscreen` を再実行せず前面化要求だけを出す。静止画 detached viewport
では `Minimized(false)` / `Visible(true)` / `Focus` を送り、動画 child presenter では
host 内で presenter raise 要求へ寄せる。同じ raw idx でも item key / generation が変わっている場合は、
通常どおり再オープンして表示状態を更新する。

detached session で見開き 2 ページ表示中は、メイン一覧の通常カーソルを現在ページに置き、
相方ページがグリッド / 詳細一覧の可視範囲内に描画される場合だけ破線のサブカーソルを重ねる。
相方がスクロール外なら追加描画は行わない。detached 動画はメインウィンドウを占有しないため、
fullscreen / in-window 動画用の main backdrop や black chrome 判定から除外する。

detached window placement は `settings.detached_viewer_window_placement` に保存する。
保存値の意味は outer position + inner/client size + maximized flag。最大化中は restore
placement を上書きせず、`maximized` だけを更新する。

### 2.2.2 動画フルスクリーンとタイルモード

`GridItem::Video` は画像ロードワーカーを使わず、`App::start_fs_load` の動画分岐で
`FsCacheEntry::Video { player }` を直接 `fs_cache` に挿入する。Windows の native
presenter 有効時は `VideoPlayer` が `NativeVideoOutput` を持ち、動画フレームと
egui overlay は専用 HWND 側で表示される。フルスクリーン viewport は黒い backdrop と
focus/chrome 管理だけを担当する。
Windows のタスクバー hover preview は main HWND を対象にするため、動画 fullscreen 中は
`dwm_iconic_thumbnail.rs` が DWM iconic thumbnail/live preview を main HWND に供給する。
フレーム抽出は worker thread 上の `video::screenshot::capture_frame` で粗く更新し、
DWM の WndProc 要求内では cached bitmap だけを返す。

動画から動画へのフルスクリーンナビゲーションでは、Windows native presenter 経路で
`NativeVideoOutput::SwitchSource` を使い、HWND / D3D11 presenter / overlay を保持したまま
新しい `VideoPlayer` の source binding に差し替える。通常ナビゲーションでは 120ms の
quiet period を置き、その間は decoder を作らず `NativeOverlayNavigationPreview` が
移動先ファイル名と保存済み resume サムネ (無ければ黒背景) を表示する。quiet period 後に
最新 target だけを open し、`native_video_fast_swap_pending` が最初の native frame 表示まで
連続入力を抑制する。navigation preview も最初の native frame が実際に表示されるまで
残し、decoder startup 中に旧動画の frame が露出しないようにする。navigation preview は
source swap 中の受動表示なのでカーソル idle をリセットせず、キーボードでの動画送りだけでは
非表示カーソルを復活させない。画像⇄動画の遷移はこの fast path の対象外で、従来どおり
`open_fullscreen` / `start_fs_load` 経路で扱う。

動画タイルモード (`video_tile_state`) は再生中動画の `VideoInfo` からタイムスタンプ列を
作り、`TileThumbnailWorker` が別 FFmpeg input でサムネイルを抽出する。タイルモードが
active の動画→動画ホイール移動でも同じ `SwitchSource` を使うが、
`video_tile_swap_pending` 中は preparing overlay を出し、新動画の `player.info()` 到着後に
新しい `VideoTileState` を構築してタイルを progressive に埋める。
タイルモード中の P は、キーボード操作で選択中のタイルカーソル位置の timestamp と
抽出済み RGBA を `video_pins.db` へ書き込み、
その動画の代表フレームとして使う。タイル画像がまだ未抽出なら、通常の動画 P と同じ
seek thumbnail 待ちの保険経路にフォールバックする。

タイルモードの fast path では保存済み resume 位置を新 `VideoPlayer` に渡すが、autoplay は false
に固定する。タイル解除後は resume 位置の静止状態を表示し、特定位置から見たい場合は
タイルクリックによる seek を使う。タイルクリックは autoplay 設定に関係なく、その位置へ
seek して再生開始する。切替元の `VideoPlayer` は cpal stream を同期 pause し、audio
buffer を明示クリアしてから native presenter を渡し、前動画の処理済み音声が短く漏れる
のを抑える。

静止画フルスクリーンでも、動画再生中と同じ `fullscreen_cursor_hide_delay_secs` 設定を使う。
UI / パネル / ダイアログが出ていない状態でマウス操作が止まると `CursorIcon::None`
に加えて viewport の `CursorVisible(false)` を送るため、スライドショー再生中も OS カーソルが
画面上に残らない。スライドショーのタイマー送りやキーによるページ送りではこの idle 状態を
次画像へ引き継ぎ、画像切替だけでカーソルを再表示しない。`open_fullscreen` の cursor
リセットは fullscreen 新規入場向けであり、fullscreen 内ナビでは呼び出し元が状態を引き継ぐ。
カーソル非表示中は最後の hover 座標を stale とみなし、上バー / 右パネル /
補正パネル edge hover / panel 内判定など、passive hover 由来の状態遷移では
`!cursor_hidden` で gate する。マウス操作または固定 UI 表示が戻ったら
`CursorVisible(true)` で復帰する。

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

- `fs_cache` / `edit_result_cache` / `final_ai_cache` / `retained_final_ai_cache` /
  `final_composite_cache`
  (および 360 モード等で残る `ai_upscale_cache` / `adjustment_cache`) に入る pixels は
  **常に 8192px 以内**。worker 側 `clamp_dynamic_for_gpu` で担保される。
- UI スレッドの `clamp_for_gpu(&ColorImage)` は異常経路の安全網。通常パスでは
  `Cow::Borrowed` で返り、Triangle リサイズは走らない。発動したらログに
  `clamp_for_gpu (UI-thread fallback)` が出る。
- AI アップスケールは `ai_upscale_size_limit` (長辺 x 短辺、既定 2048 x 2048。
  旧 `ai_upscale_skip_px` からの読み替えは `Settings::ai_upscale_limit()`) で
  対象を制限する。`4096 x 2048` 等の大きい上限では ×4 出力が 8192 を超えるため、
  final pipeline は upscaler に `MAX_TEXTURE_DIM` を渡し、タイル出力を最終クランプ後
  サイズへ直接合成する。`clamp_color_image_for_gpu` は安全網として残すが、通常は
  no-op になる。final AI / 旧 AI 経路とも判定は `ai::upscale::should_process_rect`。
- `apply_adjustments_fast` は pointwise 変換なので入力サイズを保つ → 入力が 8192 以内
  ならば出力も 8192 以内。`edit_result_cache` / final AI 結果を入力に取るので成立する。
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

派生キャッシュ (`edit_result_cache` / `final_ai_cache` / `final_composite_cache` など) や
消しゴム再挿入の entry は `source_dims: None` で良い。ホバー UI は `fs_cache` 側のエントリから原寸を読むため、
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

`ui_fullscreen.rs::resolve_fs_processed_texture` はフレームごとに以下の順で「今表示する
テクスチャ」を選ぶ。通常表示は `edit_result_cache` に `AdjustParams` / AI /
`post_filter` を最終段で重ねた `final_composite_cache` が正系で、edit 系の cache は
常に source 解像度・補正前の空間を保つ:

```
0. 右 Ctrl ホールド中の元画像プレビュー (fs_cache の raw decode)
1. erase / local_adjust / conceal の編集中プレビュー (各 UI の in-memory state)
2. final_composite_cache[edit_key, params_hash, bg]
   (= edit_result_cache + 色調補正 + final AI + スマートシャープ + post_filter。
    スマートシャープは AI アップスケール出力には掛からない —
    preset-and-adjustment.md §2.6)
3. edit_result_cache[edit_key]
   (= raw -> erase -> local_adjust -> conceal -> crop。最終段待ちの fallback)
4. fs_cache[idx] (生デコード結果、raw 専用)
5. フォールバック: サムネイル (低解像度)
```

**この優先順位は動かさないこと**。変更すると「補正を掛けた瞬間に一瞬生画像が見える」
「AI 処理中にプリセットを変えると古い final composite が残る」等の不整合が出る。

実装上は `ui_fullscreen.rs::resolve_fs_processed_texture` を通常表示の共通入口にする。
単ページ、見開き、ルーペなどが `edit_result_cache → fs_cache` のような独自チェーンを
再実装すると、新しい派生レイヤ (消しゴム / 隠蔽加工 / AI など) の横展開漏れが起きる。
360 度パノラマ表示も同じ考え方で、完了済みの `final_composite_cache` を 8K base
アップロード元として優先する。final AI が未完了の間だけ旧 `adjustment_cache` /
`ai_upscale_cache` / `fs_cache` へフォールバックし、AI 完了後は cache_key を変えて
再アップロードする。
保存・比較・クリップボードのようなピクセル出力経路も、`prepare_capture_pixel_job` で
同じ最終 composite pixels を取得する。EXIF Orientation は decode 済みなのでここでは
再適用しないが、`rotation_db` の非破壊 90 度回転は GPU 描画専用のため、crop 後の
ピクセル出力段で焼き込む。補正レイヤーが有効だが `local_adjust_cache` がまだ無い場合、
古い結果や下位画像は保存せず、完了後の再実行を促す。

右 Ctrl ホールドの元画像プレビューは例外的な一時表示で、派生キャッシュは作り直さない。
通常の画像 / ZIP 内画像 / PDF ページだけを対象にし、動画には適用しない。表示元は常に
`fs_cache` の生デコード結果で、補正 / AI / 消しゴム / 隠蔽の派生キャッシュは参照しない。
補正レイヤーの派生キャッシュも同様に参照しない。ただし補正レイヤーモード中の
`Ctrl+Shift` は「選択レイヤーをバイパスし他レイヤーを全て適用したプレビュー」に割り当てるため、
元画像プレビューはこの組み合わせを捕まえず、`resolve_fs_processed_texture` の
local_adjust 分岐に処理を譲る。`Ctrl` 単体は従来どおり元画像プレビューになる。
元画像プレビューの譲渡判定と layer bypass preview の modifier gate は、fullscreen viewport
外側の main `ctx.input` では modifier が取れないため、右 Ctrl と同じく OS キー状態を
参照する。

### 2.3.1 デバッグ出力

フルスクリーン表示中に `Ctrl+Alt+Shift+D` を押すと、現在表示中ページ (見開き時は左右
ページ) のパイプライン段階を `%APPDATA%\mimageviewer\debug-pipeline\...` へ出力する。
PNG エンコードとファイル I/O は `pipeline-debug-export` worker で行い、`manifest.json`
に `input_generation` / `erase_mask_generation` / `local_adjust_generation` /
`conceal_mask_generation`、各 stage の
有無、欠落理由、出力ファイル名を記録する。消しゴム・隠蔽加工モード中でも、通常
ショートカットより先にこのキーだけを処理する。

### 2.4 変換の合成順序

描画時、`draw_fs_image` は以下の順で変換を掛ける:

```
1. テクスチャ選択 (上記の優先順位)
2. 回転 (rotation_db, 0/90/180/270)
3. ユーザーのフリー回転 (fs_free_rotation, 一時的・非永続)
4. 表示トリムの content bbox 決定
5. フィットモード (ページ全体 / 横幅 / 縦幅 / 100%原寸)
6. 自動フィット倍率制限 (`fullscreen_fit_no_upscale` / `fullscreen_fit_no_downscale`)
7. Zoom (fs_zoom, 0.1〜50.0)
8. Pan (fs_pan)
```

`settings.fullscreen_fit_mode` は <kbd>0</kbd> で循環する。ホバーバーのフィットボタンは
クリックで選択メニュー (`fit_popup_open`) を開き、flow で選べるモードを一覧表示して現在モードを
青でハイライトする (見開きボタンのポップアップと同型)。メニュー項目選択は
`set_fullscreen_fit_mode_for_current` を直接呼び、<kbd>0</kbd> 循環 (`cycle_fullscreen_fit_mode`)
とは別系統。メニュー候補は `FullscreenFitMode::selectable_for_flow` が返す
(余白カットフィットを除く4モード)。ページ単位へ切り替えたときはページ全体、
縦連結へ切り替えたときは横幅フィット、横連結へ切り替えたときは縦幅フィットに戻す。連結モード中は
保存値が旧余白カットフィットでもページ全体フィットとして扱う。

フィットメニュー下部の「拡大しない」「縮小しない」は `FullscreenFitScaleLimits` として
自動フィット倍率にだけ適用する。つまりページ全体/横幅/縦幅が求めた `fit_scale`
を 100% で clamp してから、ユーザー操作の `fs_zoom` を掛ける。明示的な Ctrl+ホイール、
中ボタンドラッグ、ゲームパッド等の手動ズームは制限しない。`縮小しない` が ON の場合は
ページ全体フィットでも 100% 表示で画面外へはみ出すことがあるため、`fullscreen_fit_allows_drag_pan`
も true 扱いにしてパンできるようにする。
100% 判定の基準サイズは「今実際に描くテクスチャ」のサイズであり、AI アップスケール完了後は
`final_composite_cache` の寸法で再レイアウトする。単ページだけでなく、見開きと連結読みも
同じ基準を使う。右 Ctrl の元画像プレビューや分析モードでは raw 表示に合わせるため、raw
サイズを優先する。

旧 **余白カットフィット** (`FullscreenFitMode::MarginFit`) は設定互換入口としてだけ残している。
新規 UI / <kbd>0</kbd> 循環 / フィットメニューには出さず、本を開いたタイミングで
`fullscreen_fit_mode = Page` と表示トリム `Auto` へ移行する。表示トリムパネルの
「自動余白カット」が同じ検出結果を使う。
`cached_margin_bbox`(idx) が `margin_fit::detect_content_bbox` で
中身の bounding box (正規化座標) を検出してキャッシュ (`fs_margin_bbox_cache`、
`fs_cache` と同じタイミングでクリア) し、`draw_fs_image` が `fit_scale` を bbox サイズで
求めて中心を bbox 中心へ寄せる (= 余白分ズームイン)。描画時は bbox 外を描かず背景に落とすが、
**ソースピクセルは一切変えない**ので補正/
AI/エクスポートには無影響。

検出は「全部映る」優先の頑健化版 ([margin_fit.rs](../src/margin_fit.rs)):
(1) 検出用に長辺 ~1000px へ面積平均で縮小 (サブピクセルのゴミを潰し線は残す)、
(2) 余白色を縁ピクセルの median で推定 (四隅 1 点ではないので糊汚れ・勾配に強い)、
(3) 広めの luma 許容で色味差を無視、
(4) 中身マスクを 8 連結ラベリングし面積が小さい孤立塊 (点/ゴミ) を捨てて線・文字は残す
(枠外へ伸びる線は本文と連結するので残る)、
(5) 残った成分の union bbox にセーフティパッドを足し、各辺のトリムを `MAX_TRIM_FRAC` (20%)
までに制限 (隅の小さな中身だけに巨大ズームする "やりすぎ" を防ぐ)。
縁の過半 (`BORDER_MARGIN_FRAC`=0.50 未満) が余白でない真のフルブリード / トリム量が極小 のときは
`None` で通常フィットへフォールバック (= 迷ったら切らない)。**漫画の裁ち落とし (絵が一部の辺
だけ端まで届く。例: 左上下はブリード・右だけ余白) は、union bbox が自然に各辺へ追従するので、
余白のある辺だけ詰まる** (縁ゲートを 0.80→0.50 に緩めて、3 辺ブリードのページを丸ごと諦め
ない。縁の余白色は median 推定なので過半が余白なら緩めても誤検出しない)。
診断: 余白カットモードへ切り替えた時に `log_margin_fit_diag` が各成分の面積・位置・各辺の決定要因を
logger へ出力 (`[margin-fit diag]`)。

**見開き (`draw_fs_spread`)** でも有効: 左右各ページの content bbox をページ別に保持し、
見える幅だけを `spread_page_gap_px` に合わせて詰める (bbox 無しのページは全域扱い=切らない)。
中央側をトリムした場合も、切った領域を見開き中央に残さず、左右の見える端が設定 gap で並ぶ。
`fit_scale` は左右の見える幅の合計と上下 bbox の union 高さを基準にし、描画時はページ全体
rect のうち bbox 部分だけを UV 指定して描く。内部的にはトリム量によって左右のページ rect が
重なることがあるため、`FsSpreadLayout` は UV 変換用のページ rect とヒットテスト用の見える rect
を分けて保持する。どちらかが回転していれば通常フィットにフォールバック。

**表示トリム** (`ui_view_trim.rs` / `view_trim.rs` / `view_trim_db.rs`) は、
漫画ビューア用途で読みながら使う表示専用トリム。左端 / 上端 / 右端ホバーで開く左パネルの
`画像補正 / 表示トリム` タブから操作し、選択中タブは `Settings::fullscreen_left_panel_tab`
へ保存する。
本側の基本モードは `ViewTrimApplyMode::{None, Auto, Book}` のラジオで選ぶ。
`None` は bbox を使わず、`Auto` は表示中ページごとに `cached_margin_bbox` を使う。
`Book` は本全体設定を適用する。「このページの個別設定を適用」はチェックボックスで、
チェックした現在ページだけ `Page` として一時適用し、前後ページへ移動すると自動で外れる。
`Book` / チェック中の `Page` では、単ページ / 見開き連動 (上・下・中央側・外側) / 見開き左右別を
0〜20% で調整する UI が展開される。スライダーを動かした対象は自動的にその手動モードで
適用される。手動指定された bbox も、自動検出 bbox も、ページ全体 / 横幅 / 縦幅 /
100% 原寸で fit 基準として使う。
bbox 外は描画せず背景色を見せるだけで、
`export_crop.rs` の切り取り、保存、Ctrl+E エクスポート、補正 / AI キャッシュには影響しない。
見開きの「左右別」切替は値を移行し、連動→左右別では中央側/外側を左右ページへ展開、
左右別→連動では中央側/外側/上下を平均値へ畳む。
自動検出ボタンは `Book` / `Page` の手動設定へ現在ページの検出結果を流し込むための補助であり、
`Auto` とは別物。`Auto` は保存済みの手動値を作らず、ページごとに検出する。
旧 `FullscreenFitMode::MarginFit` が保存されている場合は、表示トリムの適用モード `Auto` と
ページ全体フィットへ移行して保存する。
基本適用モードと本全体設定は `view_trim.db::view_trim_books` に本キー
(`spread_container_key` と同じ粒度。ネスト ZIP は zip_path + 実効 prefix) で保存する。
ページ個別設定は `view_trim.db::view_trim_pages` に `page_path_key` で保存する。
`Page` は本側の適用モードとしては保存せず、チェック状態もセッション中の現在ページに限る。
`Auto` は「モード」だけ保存し、検出 bbox は保存しない。スライダードラッグ中はメモリだけ更新し、
マウスを離したフレームで本設定と表示中ページの個別値をまとめて SQLite へ書き込む。

Spread モード (見開き) の場合は、`draw_fs_spread` が `resolve_spread_pair` で左右の idx と配置
(LTR/RTL/Cover) を決め、両ページを「1 枚の合成画像」とみなしてレイアウトする。
`resolve_spread_pair` は先に表示ユニット列を組み、表紙、横長ページ、末尾端数を単独ユニットにして、
横長ページの次の縦長ページから通常の見開きペアリングを再開する:

1. 各ページの表示サイズ (回転考慮) を算出し、高い方に揃えた連結幅・高さを計算
2. `spread_page_gap_px` を左右ページの画面上の間隔として差し込み、フィットモードに応じた `fit_scale` を求める
3. ズーム/パンを `(fit_scale * fs_zoom, image_rect.center() + fs_pan)` として合成し、合成中心から
   左右ページ矩形を配置する (ズーム/パンは左右ページで共有、ページ間の分割位置は不変)
4. ズーム/パンが有効なフレームでは `image_rect` にクリップして他の UI 領域へのはみ出しを防ぐ

見開きのページ間隔は環境設定から変更でき、既定 4px、0px で左右ページを隙間なく接続する。
見開き中も `rotation_db` の単独ページ回転 (R/L) は左右ページそれぞれに反映する。
`fs_free_rotation` (Ctrl+ドラッグのフリー回転) は見開きに反映しないため、Ctrl+ドラッグは
`handle_fs_wheel_and_click` 側で no-op にしている。
ズーム中または横幅/縦幅/原寸フィット中のパン (非修飾ドラッグ) と Ctrl+ホイールズーム、ダブルクリックリセットのみが見開きで有効。

連結読み (`draw_fs_continuous_reading`) は、`SpreadMode` のページ構成 (単ページ / 見開き)
を表示ユニットとして縦または横へ仮想配置する。巨大キャンバスは作らず、各ユニットの表示矩形を
スクロール位置から毎フレーム計算し、GPU に保持するのは可視範囲と前後少数ページだけにする。
同時可視ページ数は最大 16 ページ程度、`fs_cache` の連結読み用 keep set は最大 20 ページ程度に
抑え、推定総テクセル数でも追加の安全弁をかける。連結読みのページ/見開きユニット間隔は
`continuous_reading_gap_px` (既定 20px) を使い、見開きユニット内部の左右ページ間隔は
通常見開きと同じ `spread_page_gap_px` を使う。見開き構成の縦連結 + 横幅フィットでは、
表紙・横長ページ・端数の単独ページも仮想的な 2 ページ幅で fit scale を求め、実ページを中央寄せする。
これにより表紙だけ横幅いっぱいに拡大されず、後続の見開きページと同じ倍率で読める。
ZIP の章区切り (`ZipSeparator`) は GPU テクスチャ化せず、前後のページ/見開きユニットと同じ
表示サイズの区切りページとして描画する。ページ/見開きペアリングは画像ページ列だけで決めるが、
通常見開き・ページ送り・連結読みは同じ表示ユニット列を使うため、横長ページの前後で単ページ扱いと
ペアの再同期がずれない。
横連結では `ReadingDirection` により
左→右 / 右→左の座標符号を反転する。UI で横方向を変更した場合は `SpreadMode` の
表紙あり/なしを保ったまま LTR/RTL も同期し、ページ単位の見開き方向と横連結方向が
食い違わないように保存する。ホイール 1 ノッチ、矢印キー/D-pad 1 回、左スティック最大入力の
スクロール量は、それぞれ画面幅/高さに対する割合として `continuous_reading_*_percent`
設定に保存する。PageUp/PageDown は従来どおり画面長の 85% で移動する。

---

## 3. 補正・AI・ポストフィルタキャッシュと再描画

### 3.0 処理順序 (= 最終表示への適用順)

ユーザーが表示で見る最終画像は、v1.1.0 以降は以下の順序で各レイヤが重ねがけされる:

```
1. fs_cache (= 生デコード結果)
   ↓
2. 消しゴム (MI-GAN inpaint)
   → ESC / E / × ボタンで確定したとき MI-GAN がマスク領域を補完
   → erase_result_cache[idx,input_gen,mask_gen]
   fs_cache は raw decode 専用で、消しゴム確定結果を書き戻さない。
   ↓
3. 補正レイヤー (local-adjust-core)
   → local_adjust_cache[idx,input_gen,erase_mask_gen,local_gen]
   消しゴム結果があればそれを、なければ raw を入力にして非同期 worker で合成する。
   未生成または stale の間は古い補正レイヤー結果を表示せず、下位レイヤの画像を表示する。
   サムネイルには反映しない。
   ↓
4. 隠蔽加工 (モザイク / 白塗り / 黒塗り / ぼかし)
   → conceal_cache[idx, generation] (= local_adjust_cache / erase_result_cache / raw をベースに合成)
   ↓
5. crop
   → edit_result_cache[EditResultKey]
   ここまでが source 解像度の edit pipeline。AdjustParams / AI / post_filter は含めない。
   ↓
6. 色補正 (色温度・彩度・コントラスト・露出など)
   → apply_adjustments_fast(edit_result)
   ↓
7. AI アップスケール / デノイズ (Real-ESRGAN / Real-CUGAN / NMKD-Siax / 1x denoise)
   → final_ai_cache[FinalAiKey] (pixels + used_upscale)。未完了中は色補正後の画像を暫定表示する。
   ↓
8. スマートシャープ (シャープ化スライダー 0..=100、輪郭中心の最終段シャープ)
   → AI アップスケール出力には掛からない (固定動作)。デノイズのみ / AI なしには掛かる。
     詳細は preset-and-adjustment.md §2.6
   ↓
9. ポストフィルタ (CRT エミュレート / 減色 / モノクロ / 複合エフェクト)
   → final_composite_cache[FinalCompositeKey]
```

**ユーザー向けの言い換え**: 消しゴム / 補正レイヤー / モザイク加工 / crop は元画像の
解像度で先に確定し、その後に明るさ・色・AI 拡大・効果フィルタが最後に乗る。
そのためアップスケール ON/OFF や補正スライダー変更で編集マスクの解像度は変わらない。

### 3.1 詳細

詳細は [preset-and-adjustment.md](preset-and-adjustment.md) に譲る。ここでは要点のみ:

- **補正 (adjustment)**: `ensure_final_composite_texture` が `edit_result_cache` のピクセルへ
  `apply_adjustments_fast` を適用する。色系パラメータ変更では `edit_result_cache` を保持し、
  `final_ai_cache` / `final_composite_cache` だけを落とす。
- **スマートシャープ (シャープ化)**: final AI の後・ポストフィルタの前に
  `apply_final_smart_sharpen` を適用する。AI アップスケール出力には掛からない
  (固定動作)。サムネイルには反映しない。詳細は
  [preset-and-adjustment.md §2.6](preset-and-adjustment.md)。
- **ポストフィルタ**: AI の後段で CPU 処理 (CRT/減色/複合)。rayon 並列化済み。
  `PostFilter::Nearest` のみ NEAREST サンプラー、それ以外は LINEAR でアップロードする。
- **消しゴム/隠蔽加工/分析モード中の一時バイパス**: `App::post_filter_bypassed = true` の間は
  final composite の key から post-filter を外し、表示用最終段だけを切り替える。
  edit 系 generation は進めない。
- **補正レイヤー**: `local-adjust-render` worker で `local-adjust-core` を適用し、
  `local_adjust_cache` に載せる。生成中は古い補正レイヤー結果を使わず、
  `erase_result_cache > fs_cache` の下位画像を表示する。
  Ctrl+E / キャプチャ保存では、補正レイヤーが有効なページは `local_adjust_cache` 完了後だけ
  出力対象にする。
  ブラシ stroke 中は 150ms の idle まで重い再合成を遅延し、release 時に確定世代を進める。
- **AI アップスケール/デノイズ**: final pipeline の別スレッドで推論。完了時に
  `final_ai_cache` に pixels と `used_upscale` を格納し、未完了の
  `final_composite_cache` を捨てて再合成する。
- **AI 結果の保持 LRU**: 完了済みの final AI pixels は `retained_final_ai_cache` にも
  `metadata_cache_key(idx) + edit_size + color_ai_hash + bg` で保持する。entry には
  smart sharpen 判定用の `used_upscale` も保持する。通常の
  `final_ai_cache` は fullscreen keep-set や `close_fullscreen()` で消えるが、保持 LRU は
  `retained_final_ai_cache_max_entries` / `retained_final_ai_cache_max_mib` の範囲で残る。
  再表示時は `final_ai_cache` miss の後に保持 LRU を参照し、ヒットした entry を live cache
  へ戻してから final composite を再生成する。AI 入力が変わる編集では該当ページの保持分も
  破棄し、post_filter / smart_sharpen だけの変更では保持する。fullscreen close / reopen
  に伴う同じ item の `fs_cache` 再ロードでは live cache だけを作り直し、保持 LRU は残す。
  PDF ページの display final AI は、巨大ページで完了前に session close / keep-set eviction
  されると LRU へ store できないため、保持 LRU が有効な場合だけ最大 1 件を
  `retained_final_ai_orphans` として live pending から外し、cancel せず完走させる。PDF ページは
  汎用 `retained_final_ai_cache` へ重複保存せず、PDF ページ専用の統合スロット
  (`Raster` / `FinalAi` のどちらか 1 つ) へ store する。PDF レンダリングが完了した段階では
  `Raster` として保持し、その後 final AI が完了したら同じページスロットを `FinalAi` に昇格して
  raster pixels を解放する。この PDF ページスロットと汎用保持 LRU は同じ枚数 / MiB 予算で
  合算 LRU 退去される。再表示時に `FinalAi` が条件一致すれば PDF worker を起こさず、AI pixels
  から `final_composite_cache` を直接復元する。
  外部変更や AI 設定変更で retained epoch が進んでいた場合、その orphan 結果は store 時に捨てる。
  汎用保持 LRU の store / hit /
  miss / skip / evict / clear は `mimageviewer.log` に `[AI] Retained final AI ...`
  として記録する。PDF orphan の開始 / 回収は `[AI] Final AI retained orphan ...` /
  `[AI] Final AI orphan result stored for retained cache only ...` に出る。epoch 不一致や設定無効で
  store しなかった orphan は `skipped for retained cache only` になる。PDF ページ統合スロットの
  `Raster` / `FinalAi` の store / hit / miss / evict / clear は `[PDF] Retained page ...` として
  記録する。
  ヒット時も元画像ロードと final composite の再生成は必要なので、再入場直後に 1 フレーム程度
  AI 前の暫定表示が出ることはある。ただし PDF の `FinalAi` hit は raw PDF レンダリングを待たずに
  final composite を復元するため、ラスタ化待ちの低解像度表示を避けられる。外部アプリによる現在フォルダの実ディスク変更を
  signature 差分で検出した場合は、同じ path / 同じ寸法の差し替えに備えて保持 LRU を全クリアする。
- **AI 先読み (新パイプライン)**: `App::prefetch_final_ai` がフルスクリーン更新ループ
  終盤で呼ばれ、現在ページの `final_ai_pending` (cancel フラグ除く) が空のときだけ
  隣接ページの `final_ai` 推論を 1 件 spawn する。`ai_prefetch_targets` で前後の
  対象 idx を `ai_upscale_prefetch_forward / back` 件まで取得する。
  ⚠️ **退行注意**: Pipeline P1 リファクタ (be05cfef) で旧 `prefetch_ai_upscale` が
  dead code 化され、新版が未実装のまま 1 リリース過ごした。`App::update` の
  「フルスクリーン work セクション」(= `// AI 先読み (新パイプライン)` コメント) を
  消すと再発するため、リファクタ時は呼び出し元の存在を要確認。
- **元画像プレビュー**: 右 Ctrl を押している間だけ描画時のテクスチャ選択を
  raw 専用の `fs_cache` に切り替える。DB・補正設定・AI queue は変更しない。
- **何かを変えたら正しいキャッシュをクリア**:
  - 色調パラメータ / AI 変更 → `final_ai_cache` / `final_composite_cache` をクリア
    (`edit_result_cache` は保持)。post_filter / シャープ化**のみ**の変更は
    `final_composite_cache` だけクリアして final AI を保持する
    (preset-and-adjustment.md §4)
  - 消しゴム / 補正レイヤー / 隠蔽加工 / crop 変更 → source 解像度の edit cache と final cache をクリア
  - 消しゴム/隠蔽加工/分析モード入出 → 該当 idx の final cache のみクリア (bypass 切替のため)
  - フォルダ切替 → edit / final / thumb 系 cache をグローバルクリア
  - 回転変更 → **キャッシュはクリアしない** (GPU 行列で回すため)

---

## 4. 分岐チェックリスト

修正時、以下の観点で漏れが出やすい。どれかを触るなら全部を確認する:

### 4.1 画像種別分岐

| 処理 | Image | ZipImage | PdfPage | Video |
| --- | --- | --- | --- | --- |
| サムネイルデコード | image/turbojpeg/WIC | image::load_from_memory | PDFium ワーカー | Shell API (別スレッド) |
| フルスクリーンデコード | 同上 + EXIF + 動画判定 | bytes から decode + EXIF | PDFium で 4096px | なし (サムネのみ) |
| EXIF Orientation | ✅ path から読む | ✅ bytes から読む | ❌ | — |
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
