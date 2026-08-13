# 表示パイプライン (サムネイル / フルスクリーン)

**これが一番事故が多い領域**。画像補正・AI アップスケール・回転・アニメーション・消しゴムマスクのすべてが、
「どのテクスチャを画面に出すか」を巡って絡み合う。修正前にこのドキュメントを読むこと。

---

## 1. サムネイル表示パイプライン

### 1.1 状態機械

`GridItem` 1 個につき 1 つの `ThumbnailState` (grid_item.rs) を持つ:

```
Pending ──────────(ワーカーがデコード)──────────▶ Loaded { tex, from_cache, rendered_at_px,
                                                           source_dims, layout_dims }
                                                       │
                                           (keep_range 外に出ると)
                                                       ▼
                                                    Evicted ─(再び可視範囲へ)─▶ Pending (再要求)
```

Failed は単発の終端ステート。デコードエラー時のみ。

`Loaded` へ遷移した時点で、`source_dims`（無ければロード済み画像寸法）と PDF の `layout_dims` を
per-context の `PageDimsCache` の別 map に記録する。`source_dims` は常にピクセル座標、
`layout_dims` は PDF page box の 1/1000 point であり、単位を混ぜない。`Evicted` は GPU texture
だけを破棄し、同じ `items_generation` で判明済みの両寸法は残す。idx 空間が変わるときだけ
`invalidate_idx_state_and_queues` で clear し、cache 自身の generation 不一致も `None` へ
fail-closed する。

### 1.2 2 フェーズ優先ロード

`App::update()` 毎フレーム:

1. **keep_range の再計算** (`update_keep_range_and_requests` in `app.rs`)
   - 可視範囲 + `thumb_prev_pages` + `thumb_next_pages` を含む範囲を算出
   - `keep_start_shared` / `keep_end_shared` (`Arc<AtomicUsize>`) に書き込み → ワーカーが参照
2. **エビクション**: 同一 items 世代では前回 keep_set から外れた Loaded だけを
   `Evicted` に遷移 (GPU テクスチャを drop)。items 差し替え後の初回だけ全件照合する
3. **要求投入**: keep_range 内の Pending / Evicted に対して `LoadRequest` を作り
   - 通常キュー: `reload_queue` (Image/ZipImage/PdfPage)
   - 重 I/O キュー: `heavy_io_queue` (Folder/ZipFile/ConvertibleArchive/ZipDir — 全体走査、
     ZIP セントラルディレクトリ読み、または ZIP 内 prefix の代表解決が必要。PdfFile は PDF ワーカー IPC なので通常キュー)
   - 1 件以上キューへ投入したフレームは `update_keep_range_and_requests` 自身も repaint を要求する。
     通常は `App::update` 末尾の `requested_nonempty` と同じ役割だが、フルスクリーン cleanup や
     font atlas resync で末尾まで到達しないフレームでも worker 結果を入力待ちにしないため。
4. **アイドル時品質アップグレード**: スクロールが止まって ~1 秒経つと、`from_cache: true` かつ `from_edit_preview: false` の Loaded に対して `skip_cache: true` で再要求 → 高品質デコード。ただし、`make_load_request` が親コンテナや手動ピンを最終 target へ解決した後も `req.skip_cache == true` を保つ要求だけを upgrade queue へ入れる。編集プレビューと、動画ピンから seed された完成済み WebP キャッシュは元画像から改善できない派生画像なので対象外。特に動画ピン要求は `apply_folder_thumb_pin` が意図的に `skip_cache = false` へ変換し、アイドル高画質化へ投入しないことでフォルダ自動代表画像による上書きと無限再投入を防ぐ。対象外と確定した idx は現在の Loaded サムネイル / viewer context に紐づけて記憶し、repaint ごとの pin 解決も行わない。この記憶は新しい items 世代、サムネイルの退去・再ロード、編集プレビュー更新で破棄する。`--perf-log` 時は最終判定を `thumb.idle_upgrade_enqueue` / `thumb.idle_upgrade_ineligible` として記録し、同一 key / idx / items 世代の反復をリリース前 `idle-health` 検査で拒否する
   - 一覧ロード直後は `start_loading_items` が履歴スクロール復元後の位置で idle 判定をリセットし、
     親一覧へ戻った瞬間に古い idle 時刻で高品質再生成が走らないようにする。
   - **フレーム内境界レース対策 (2026-06-19)**: アップグレードの起動条件 (input/scroll が
     500ms idle) は `enqueue_idle_upgrades` がフレーム先頭で、`thumb_idle_upgrade_recheck_delay`
     がフレーム末尾で評価する。同一フレーム内で経過時間が 500ms 境界をまたぐと、enqueue は
     skip・recheck は `None` を返して egui が就寝し、**次のユーザー入力までアップグレードが
     走らない** (ESC で親一覧へ直帰した直後にサムネが低画質/灰色のまま固まる。perf-log で
     約 7.9 秒のフレーム停止 = `tail_repaint action=none reasons=[]` を確認)。`recheck_delay`
     は閾値に `MARGIN` (50ms) を足して wake フレームを閾値より後ろへずらし、起床フレーム先頭の
     enqueue が確実に enqueue できるようにする。BS で 1 段ずつ戻ると境界をまたがず発生しにくいが、
     ESC 直帰では再現性が高かった。

### 1.2.1 黒サムネ回帰 (v1.8.0) と font-atlas-resync 窓での upload 先送り

**症状 (v1.8.0 で発生 / v1.7.0 では起きない)**: フルスクリーン (静止画 / PDF ページ) を
Esc / BS で閉じてグリッドへ戻った直後、一部のサムネが**真っ黒**になり ~0.5 秒固着する
(その後アイドル高画質化の再レンダで自然回復)。

**原因**: v1.8.0 のフルスクリーン viewport cleanup (`show_viewport_immediate` +
`Visible(false)` + recreate) により、**close 直後の 1 フレームだけメインウィンドウの
wgpu surface が消える**。その frame に `ctx.load_texture` で queue したテクスチャ upload
(delta) は eframe の `paint_and_update_textures` の no-surface early return で**捨てられる**。
font atlas は `main_font_atlas_resync` が数フレーム再 upload して救済していたが、**同じ窓で
作られた grid サムネのテクスチャは upload だけ捨てられて `ThumbnailState::Loaded` のまま
残る** → GPU データ空 = `draw_thumb_texture` の黒 backdrop だけが描かれる。テクスチャの
**中身は正常** (perf 計装 `thumb/suspect_black` で 0 件確認) で、純粋に GPU upload が落ちただけ。

**修正 (2026-06-19, `poll_thumbnails`)**: `main_font_atlas_resync_pending` が立っている間は
サムネのテクスチャ化を `texture_backlog` へ**先送り**し、surface が戻ってから upload する。
ColorImage は破棄せず保持するので数フレーム遅れて正しく表示される (黒化しない)。
perf イベント `thumb/upload_deferred_for_resync` で先送り件数を記録。font atlas 側の
resync (`MAIN_FONT_ATLAS_RESYNC_REPEAT_FRAMES`) と同じ「surface 復帰まで待つ」方針を
ユーザーテクスチャにも広げた形。

### 1.3 ワーカー側の流れ

`thumb_loader.rs::process_load_request`:

```
1. 編集済みページでは `edit_preview_cache.db` の対応表を先に確認
   ├─ ヒット (skip_cache=false): 最大辺 2048px の edit-result 下地 WebP (q=90) と
   │   注釈レイヤー WebP (lossless) を同一寸法で display_px へ縮小 →
   │   下地＋注釈の ColorImage と表示時補正用の分離 payload
   └─ ミス: 通常の catalog へ続行
2. キャッシュ DB (catalog.db) に該当エントリがあるか確認
   ├─ ヒット (skip_cache=false): WebP バイト → ColorImage
   └─ ミス or skip_cache=true:
        ├─ ソースデコード (JPEG=turbojpeg, PNG/GIF/WebP/BMP=image crate,
        │                   HEIC/AVIF/JXL/RAW=WIC, PDF=PDFium ワーカー)
        ├─ EXIF Orientation 適用 (通常画像=path / ZIP=bytes、PDF は対象外)
        ├─ Lanczos3 で display_px までリサイズ
        ├─ CacheDecision::should_cache でキャッシュ可否判定
        └─ 必要なら WebP エンコードして catalog.db に保存
3. mpsc で (idx, ColorImage, from_cache, from_edit_preview,
   edit_preview_adjustment, source_dims, layout_dims) を送信
```

### 1.3.1 非破壊編集プレビューキャッシュ

フルスクリーン編集を閉じる直前または別ページへ移動する直前に、source 解像度の
`edit_result_cache`（消しゴム／補正レイヤー／隠蔽加工まで）を snapshot する。専用 worker 上で
テキスト／スタンプを z 順の注釈ラスターレイヤーへベイクし、下地と各注釈レイヤーへ同じ保存済み
crop を実切り出しする。最大辺 2048px の下地は WebP q=90、透明縁と Multiply 係数を持つ注釈は
lossless WebP として分離保存する。保存先は `<data_dir>/edit_preview_cache/`、対応表と LRU は
`<data_dir>/edit_preview_cache.db` に保持し、crop 後の縦横寸法を一覧のアスペクト比として使う。
元画像と各編集 DB は変更しない。
読み込み worker は保存サイズを品質原本として残したまま、現在の `display_px` へ下地と
全注釈レイヤーを Lanczos3 で同一寸法へ縮小してから合成する。UI / GPU には表示寸法の
画像だけを渡し、ページ数ベースの eviction と共有 VRAM 会計を実際のテクスチャ寸法に一致させる。
透過境界は `Color32` の premultiplied alpha を保ったまま縮小し、WebP 保存の直前だけ
straight alpha へ戻す。これにより Normal 注釈や透過画像の縁に透明黒が混じることを防ぐ。

注釈済みの `final_composite_cache` は流用しない。そこには色調補正・final AI・スマートシャープ・
ポストフィルタまで含まれるため、scene / font / stamp cache を worker へ snapshot し、
`edit_result_cache` から下地と注釈だけを作る。グリッドでは下地へだけ色調補正を 1 回適用した後、
Normal / Multiply の注釈レイヤーを z 順に合成する。これにより fullscreen と同じ
`edit -> color -> comic` を保ち、文字・スタンプの色がページ補正で変化しない。

色調補正をキャッシュに含めないため、グリッドの既存 `thumb_adjust_tex` を一度だけ適用でき、ページ
補正を後から変えても重い編集プレビューを再生成せず追従する。編集データを更新した時点で旧 preview
を非同期削除し、終了時に最新の完成済み結果だけを保存する。source の mtime / size が変わった場合も
load 時に失効する。削除／全消去通知を受けた UI も、メモリ上の古い編集 preview を即時破棄する。
保存完了通知を受けた UI は該当セルを `Evicted` に戻して読み直し、同時進行して
いた raw decode が後着しても `from_edit_preview` が立つまで再試行する。

閲覧だけで viewer を閉じた場合は、永続 preview 行が実在して削除されたときだけ UI へ失効を通知する。
行が存在しない通常サムネイルまで `Evicted` に戻すと、開閉したセルだけが灰色へ戻って再 decode される
ためである。一方、編集データの明示的な更新では、preview 行が未生成でもメモリ上の派生表示を破棄する
必要があるため、従来どおり必ず失効を通知する。この二つの削除契約は service command で区別する。

親コンテナの代表サムネイルを手動 pin した場合も、cascade 解決後の leaf が固定の
Image / ZipEntry / PdfPage なら同じ preview を通常 catalog より優先する。直接ページは
page mtime + size、ZIP/PDF の親代表は保存 worker が記録した container size と thumbnail
worker が読んだ現在の container mtime + size で stale 判定する。変換アーカイブでは元
アーカイブではなく、実際にページを読んだ cache ZIP の key / identity を使う。編集 preview
通知は直接ページだけでなく、その leaf を固定している親セルにも伝播する。固定 leaf の個別
色調補正 key は編集 preview の有無とは独立して load request へ渡し、thumbnail worker が下地へ
補正を適用してから注釈を合成する。一括補正の変更 / 解除も該当 leaf を固定する親セルを失効させる。
自動選定代表への反映は別 backlog とする。

既定は有効・上限 1GB。上限超過時は最終アクセスが古い WebP から削除する。encode、ファイル I/O、
SQLite 更新、LRU prune はすべて専用 worker 上で行い、UI スレッドをブロックしない。

`display_px` の算出には main egui Context の実効 `pixels_per_point` (= OS DPI × UI 表示倍率) を
使う。そのため「設定 → スケーリング」を上げるとサムネイル / 表示用デコード解像度も上がり、
高倍率ではメモリ使用量が増える。要求サイズは 256〜2048px に clamp されるため、UI 表示倍率を
200% にしても 2048px を超えて増え続けることはない。

### 1.3.2 親コンテナの代表サムネ — 優先順位

親コンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) の代表サムネは次の順で決まる:

1. **手動ピン (`folder_thumb_pins.db`)** — ユーザーがアドレスバー 📌 ボタンや
   右クリックメニュー「📌 代表サムネに固定」で指定した子アイテム。`make_load_request`
   の `apply_folder_thumb_pin` が `LoadRequest` を target アイテム用に書き換え、cache
   key は `{base_key}#pin:{source_id}` に変わる。直接ページなら source_id は
   kind/rel/entry/page/mtime/size の compact 表現、子のフォルダ / ZIP / PDF / ZipDir に
   さらに代表 pin があれば、最終ページまでの経路 hash + leaf identity になる。pin の
   付け替え、途中コンテナ、target ファイルの変更で source_id が自動的に変わるので
   古い WebP を catch しない。固定 leaf が Image / ZipEntry / PdfPage なら、そのページの
   編集 preview と個別色調補正も親コンテナまで引き継ぐ。詳細:
   [virtual-folders.md §3.1.1](virtual-folders.md#311-親コンテナの代表サムネピン-folder-thumb-pinv09x)。
2. **自動代表選定 (`resolve_folder_thumb_image`)** — Settings の `folder_thumb_sort`
   (Numeric / Modified / etc.) + `folder_thumb_depth` (再帰深さ) で先頭画像を選び、
   通常のサムネ生成パイプラインに乗せる。pin が無い場合の既定動作。キャッシュ
   ヒット時は表示速度を優先して毎回の再スキャンは行わないが、cache key には
   自動選定アルゴリズム版・sort・depth を含めるため、番号順ロジックや設定が
   変わったときは自然にミスして再スキャンされる。キャッシュミス時の Folder
   自動選定は、グリッドのブロック順に揃えて「サブフォルダ (folder_thumb_sort 順) →
   直接画像 (sort 順)」で候補を辿る。途中の子フォルダに非 Image の代表 pin がある場合は、
   子が直上一覧で既に生成した完全一致 WebP だけを読み取り専用で再利用する。WebP が無い・
   stale・破損の場合は PDF/ZIP/動画を生成せず、そのまま従来の画像候補探索を続ける。
   再利用した WebP は `ThumbLoadOrigin::FinalCache` として UI へ渡し、idle
   quality-upgrade が元 PDF 等を再生成しないようにする。既に有効な上位キャッシュはこの
   再探索より先に通常の cache hit となるため、従来どおりそのまま使う。
   ConvertibleArchive は有効な変換キャッシュ ZIP がある場合だけ、その ZIP の先頭画像を
   `archivethumb:{format}:{identity}` キーで読む。キャッシュ未作成/失効時は要求を出さず
   アイコンに戻す。
3. **フォルダ / ZIP / PDF / アーカイブアイコン fallback** — 中身が空 / 全部エラーで上 2 段が失敗
   したときの最終フォールバック。`grid_item.rs` の draw_cell でアイコン表示。ZIP / PDF /
   RAR 等の形式は中央アイコンと独立した左下バッジでも示す。形式バッジは従来サイズの約
   70%（小さいセルでは可読性のため 7pt を下限）とする。フォルダ名バッジは可変長 CJK の
   可読性を残す専用スタイルで、旧サイズの 85%、8.5pt 下限・13.5pt 上限、横 padding
   0.30em・縦 padding 0.12em とする。形式バッジと機械的に同じ 70% にはしない。

サムネイル四隅 overlay の計測・padding・配置は `src/thumb_overlay_layout.rs` の
`ThumbnailOverlayLayout` が所有する。v2.9.1 では左上と左下を移行済み。左上は
ブックマーク時刻 → 動画 `UP` → 編集状態 / pin → タグの順に実測幅を予約し、幅不足時は
タグだけを省略または非表示にする。描画とタグ hover / click は同じ `BadgePlacement.rect` を
使う。左下はフォルダ名または形式バッジとファイル名プレートを同じ下段へ配置し、その実測行の
上へ評価を積む。色・角丸・フォントなど各要素の見た目は `ui_helpers.rs` の個別描画関数が持つ。
右上のチェック / スタック枚数と右下の絞り込み件数は未移行で、バックログ §2.2 第3段階に残す。

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
| 非破壊編集結果 | **編集プレビューキャッシュ** | 消しゴム・補正レイヤー・隠蔽加工・テキスト／スタンプ・切り取り。編集終了時に非同期生成し、次回一覧から利用 |
| プリセット補正 (色調のみ) | **UI スレッド同期適用** | `thumb_adjust_tex[idx]` に保持、§1.5 参照 |
| カラー化 | **適用されない** | 最終表示専用。トーン濃淡変換を含むため worker で処理 |
| ポストフィルタ | **適用されない** | コスト/実装維持のためサムネは色調のみ |
| AI アップスケール | **適用されない** | 1 枚 10 秒級のためサムネでは非現実的 |

### 1.5 サムネイル補正パイプライン (色調)

「黄ばんだ紙のスキャンをモノクロ漫画補正して見る」等、サムネ一覧とフルスクリーンの
見え方を揃えるため、グリッド描画時に色調補正を適用する。LUT 路ならサムネサイズ
(600 px 級) で ~3ms/枚 で済むが、70 枚同時に UI スレッドで掛けると 200ms 級の
フリーズになるため、以下の構造にしている。

**対象**: 画像系グリッドアイテム (`Image` / `ZipImage` / `PdfPage`)。自動選定されたフォルダ /
ZipFile / PdfFile / ConvertibleArchive の代表サムネには適用しない (`adjusted_tex` は `None` で
素通し)。手動固定した親代表だけは、解決済み leaf の個別色調補正を thumbnail worker 内で
下地へ適用する。

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

frame 冒頭では、先読み窓ぶんの `fs_cache` を 1 回だけ走査し、`Static` / `Animated` / `Video` が
現在の縦横判定に使う寸法を `PageDimsCache` へ回収する。見開きの判定順は `fs_cache` →
ロード済みサムネイル → `PageDimsCache` → 未知なら縦長扱いであり、一度判明した寸法は live cache
退去後も同じ viewer context / items 世代に残る。全 thumbnails の毎 frame 走査は行わない。

paged 表示でキーリピート由来の未消費ページ送り edge が同じ input frame に残る場合は、現在の
表示 unit をカタログサムネイルで 1 frame 描き、processed texture と完成済み worker result の
GPU upload を次の frame へ保留する。単ページは現在ページ、見開きは通常描画と同じ
`SpreadDisplayUnit` resolver が返す全ページについて `ThumbnailState::Loaded` を要求し、1 ページでも
欠ける場合や unit を解決できない場合は通常の実体化へ fail-closed する。ただし、その display unit の
全ページについて `current_final_composite_texture` が既に返せる場合は、未消費 edge があっても
`Materialize` を維持し、完成表示からサムネイルへ降格しない。この在住確認は cache lookup だけで、
`resolve_fs_processed_texture` や新しい worker / GPU upload を起動しない。時間閾値や前 frame の
pending state は使わず、連結読みは対象外とする。

**`keep_fullscreen_viewport_alive`** はフルスクリーン非アクティブ時 (`fullscreen_idx == None`)
に呼ばれ、`fs_viewport_shown == true` の 1 フレームだけ `Visible(false)` cmd を送って hidden 化
する責務を持つ。それ以外のアイドルでは何もしない (2026-05-10、hidden viewport 維持コスト削減)。
再入場時は `render_fullscreen_viewport` が新規 viewport を hidden で作成し、DWM transition
抑止属性を適用してから `Visible(true)` にする。アイドル時の hidden viewport 維持コストを
戻さずに、初期 white client / サイズ遷移フラッシュを抑える。
詳細は [docs/ui-responsiveness.md §9](ui-responsiveness.md) を参照。

Ctrl+↑↓ のフォルダ横断と、同じ表示ユニットの final-effect source reload は、単一 field
`fs_holdover_tex: Option<FsHoldover>` を共有する。値は typed state で相互排他になっている:

- `FolderNavigation(previous)`: `previous.pages` に画面順の 1 ページ、または `[left, right]` の
  見開き 2 ページをまとめて保持する。ページごとの fallback ではなく、直前に見ていた
  **表示ユニット単位**の hold であり、カラー化の有無とは無関係に
  `capture_fs_nav_holdover` が作る。各 page は texture に加えて capture 時点の rotation、
  canonical source size、実表示に使った単ページ / 見開き側別 content bbox を所有する。
- `FinalEffectSourceReload { target_idx, previous, started_at }`: PDF の Z ズーム再レンダなど、同じ
  表示ユニットの source が差し替わる直前に、`previous.pages` へ単ページまたは見開き全体を保持する。
  `capture_final_effect_source_reload_holdover` だけが作り、左右別 slot は持たない。`started_at` は
  作業中表示の 400ms UX 閾値を variant 自身に持たせる。通常のページ移動はこの state を作らず、
  移動先の色忠実な低解像度 rendition を final composite 到着まで表示する。

`fs_nav_holdover_for_draw` が有効な間、`fullscreen_idx == None` の PDF/ZIP enumerate
gap と、`fullscreen_idx == Some` の nav ロック継続中のどちらも、カラー化側と共通の
`draw_fs_display_unit_holdover` で旧単ページまたは旧見開き全体を `image_rect` に重ねる。
画面順を payload に保存するため、LTR は `[小 idx, 大 idx]`、RTL は `[大 idx, 小 idx]` の
物理的な左右順を items 差し替え後も維持する。texture handle と geometry snapshot を unit
自身が所有するため、旧 `fs_cache` が drop されても描画寿命は変わらず、同じ数値 idx が
新フォルダの別 item を指し始めても rotation / source size / trim を再解決しない。

フォルダ横断後、新しい `items_generation` のページで full / final / edit / thumbnail の
いずれかを一度でも表示候補に選べたら、`fs_nav_holdover_for_draw` は
`FolderNavigation` の unit と全 handle を破棄して一方向にラッチする。表示確定待ち世代を持つ
`fs_nav_locked_gen` は `poll_fs_nav_lock` が別途解除するため、両者の寿命は同一でなくてよい。
AI final の invalidation / install の過渡フレームで表示解決が再び `None` になっても、
旧フォルダの unit は既に存在せず、nav holdover が復活することはない。

`fs_nav_locked_gen` は入力を拒否する lock ではなく、どの `items_generation` の表示確定まで
旧ビューの holdover を維持するかを表す presentation generation である。Ctrl+↑↓ と
Ctrl+PageUp/PageDown は lock 中も、snapshot / Ctrl+G / ZIP tree など副作用を持つ context
routing へ再入せず、読み取り専用 resolver が DFS 系 request だけを `start_folder_nav` へ渡す。
表示待ち中に次の DFS を新規開始した場合は handle を再キャプチャせず、locked generation だけを
現在世代へ進める。これにより中間ページの表示完了が先に届いても、次の folder load 前に
holdover を解放しない。スライドショー停止、detached session routing、検索分岐、ZIP 内移動は
最初の入力でだけ実行し、lock 中のリピートでは二重実行しない。

DFS 実行中の同種入力は `start_folder_nav` の単一受付口で
`pending_folder_nav_steps` へ符号付きで累積し、前後入力は相殺する。追加ステップの上限は
`MAX_PENDING_NAV = 5`。worker 完了時に `poll_folder_nav` が残数を `FolderNavResult` へ移し、
その request が起こした folder load の適用成功時だけ次の DFS へ引き渡す。
`start_loading_items` 自体は従来どおり App 上の accumulator を clear するため、アドレスバー、
お気に入り、ツリークリックなど無関係なフォルダ切替をまたいで burst が残ることはない。

フルスクリーン画像領域の合成順は、ページ画像 → ページ単位の編集オーバーレイ
(消しゴム / クロップ / キャプチャ領域 / ルーペ) → ビュー単位の holdover →
インジケータ / HUD / パネルとする。holdover は退避時に見えていたビュー全体なので、
移動先ページの編集矩形を旧ビューの上に重ねないよう、編集オーバーレイより後に描く。

`--perf-log` 診断では `fs.paint` に `source` / `idx` / `items_generation` /
`texture_id` に加え、実描画 transform の `x` / `y` / `w` / `h`、texture 寸法、
`scale_x` / `scale_y` を記録する。重複抑止署名は texture identity だけでなくページごとの
描画矩形と軸別 scale も含み、同じ texture のまま幾何だけが変わる frame も記録する。
浮動小数点の揺れでログを増やさないため、矩形は 0.25 point、scale は 0.001 を超える差だけを
変化とする。表示済みの同じ idx が解決不能へ落ちた遷移は
`fs.texture_choice` (`source=none_after_paint`) として 1 回だけ記録し、nav holdover /
continuous transition が実際に選ばれた場合も同イベントへ `branch` と source を残す。

フルスクリーンの先読み対象は、`items` 全体ではなく `visible_indices` 由来の display list から
作る。★フィルタや Ctrl+F で一覧が疎になっているときも、スライドショー / 前後移動と同じ
次候補を先読みし、フィルタで隠れた画像は対象にしない。

### 2.2 ロードスレッド

`App::start_fs_load` (app.rs) が std::thread::spawn で 1 枚ごとに spawn:

```
          ┌─ GridItem::Image      ┐
          ├─ GridItem::ZipImage   ┴→ canonical_image_loader::decode_canonical_image
          │                          → image crate → WIC → Susie の順で static fallback
          │                          → EXIF Orientation 適用済み native image
          └─ GridItem::PdfPage    → pdf_loader::render_page_for_display
                                     (実 viewport×ppp、PDF ワーカープロセス)
                                     ※zoom 時は倍率に応じて再レンダリング

アニメーション:
  ├─ .gif      → fs_animation::decode_gif_frames (通常画像のみ)
  ├─ .png/APNG → fs_animation::decode_apng_frames (通常画像のみ)
  └─ .webp    → fs_animation::decode_webp_frames / decode_webp_frames_from_bytes
                (通常画像 / ZIP 内画像)

Animated (`FsCacheEntry::Animated`) は playback-only として扱う。表示時は常に
`current_frame` の raw テクスチャを直接選び、`edit_result_cache` /
`final_composite_cache` / comic composite などフレーム非対応の派生キャッシュには
乗せない。編集モード・AI アップスケール・ポストフィルタは Animated では無効。
これは edit/final cache key が idx + generation ベースで、フレーム番号を含まないため、
1 フレームを派生キャッシュに入れると以後の `current_frame` 更新が画面へ出ず
アニメーションが停止して見えるため。

`decode_canonical_image` は fullscreen の現行分類も共有する。通常ファイルの GIF/APNG と、
通常ファイル・ZIP 内の WebP は Animated を返す。ZIP 内 GIF/APNG は animated 拒否ではなく、
従来どおり static fallback で先頭フレームを読む。Static は EXIF 適用後・8192 clamp 前の
native `DynamicImage` と `source_dims` を返す。panorama tee はこの native image を受け取り、
非 panorama の GPU raster 化だけが `CanonicalStaticImage::into_gpu_raster` で従来と同じ
Bilinear 8192 clamp を行う。

PDF 初回レンダは、描画先の `fullscreen_media_rect` の論理 point 寸法へ、その
viewport context の effective `pixels_per_point` (OS DPI × UI scale) を掛けて物理 viewport
`(Vw, Vh)` を作る。ページ寸法 `(Pw, Ph)` は PDF worker が content type と同じ PDFium
open 内で取得し、ページ全体表示なら
`S = min(Vw / Pw, Vh / Ph)`、`target_long = ceil(max(Pw, Ph) × S × 1.10)` とする。
横幅 / 縦幅 fit はそれぞれ `Vw / Pw` / `Vh / Ph`、90°/270° 回転時はページ軸を交換する。
1.10 は丸め・filter・1-frame の layout 差で等倍表示を眠くしないための品質余裕である。

- raster-only PDF は同じ worker 内で判明した埋め込み画像の native 長辺を絶対上限にする。
- 全経路の上限は 8192。100% 原寸 / no-downscale は従来の表示密度を下げないよう
  vector の最低長辺 4096 を維持し、raster native 上限は常に優先する。
- `start_fs_load` は実 viewport が確定する前の PDF を enqueue しない。fullscreen、detached、
  in-window の各 context は同じ typed display target を所有し、先読みもそれを再利用する。
- 自動 / 手動の表示 trim bbox は初回 raster 到着後に確定する。bbox 部分が viewport へ fit した
  ときの必要 texel 密度を同じ式で再計算し、不足する場合だけ priority 再レンダする。
- zoom 再レンダは display-fit 長辺を基準に倍率を掛け、8192 と raster native で clamp する。
  AI が必要な raster は display-fit が native より小さい場合も大きい場合も native へ収束させ、
  その間の final AI / AI 先読みを保留する。
WebP は `ANIM` chunk の背景色を `WebPDecoder::set_background_color` に渡してから
展開する。`image-webp` は dispose-to-background の処理を持つが、背景色未設定のままだと
dispose が実質 no-op になり、透明差分フレームで前フレームの軌跡が残る。

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
は更新せず、F12 を OFF にしたときは直前の F11 状態へ戻る。detached 中の
F11 は non-detached 設定を更新せず、通常配置を保持したまま装飾なし・モニター全体の
仮想フルスクリーンをトグルする。動画 detached では native presenter を fullscreen
presenter に作り直さず、detached viewport host を広げて child presenter を親 client rect
へ追従させる。
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
v2.7.0 の UI フォント変更も同じ経路を使う。resync は固定の既定定義ではなく
`Settings.ui_font` に対応する準備済み定義へ marker を加えるため、切り替え直後や
detached cleanup 後に既定フォントへ戻らない。

`configure_fonts_for_texture_resync` の `set_fonts` は egui 仕様で「次 pass の begin_pass で
適用」されるため、フル atlas アップロードが効く前にメイン UI を描くと上記 panic が再発する。
`maybe_defer_for_main_font_atlas_resync` は、`should_defer_main_paint_for_font_atlas_resync` が
true を返す全 resync 経路 (`detached_viewer_cleanup` / `fullscreen_viewport_cleanup` /
`native_video_backdrop_hide` / `fullscreen_viewport_recreate` / `still_window_mode`) で `ctx.request_discard()` を使って
その pass を破棄し、egui の
**同一 OS フレーム内 multi-pass** (`max_passes`=2) で `update()` を再走させる。次 pass の
begin_pass で新フォントが適用され、フル atlas アップロードがメイン UI 描画より前に入る。
破棄された pass は paint されないので黒/空白フレームは出ず、2 度目の pass は入力 events が空
(`RawInput::take` 済み) なので close / ナビ等の入力起因処理も二重発火しない。`will_discard()`
が false (multi-pass 予算なし) の稀なケースだけ、従来どおりこの pass の描画を飛ばして次フレーム
で再描画する「1 フレーム黒」フォールバックに落ちる。detached cleanup も、メイン UI を stale
font atlas のまま描くとフォント崩れが残ることがあるため、同じ保守経路へ乗せる。

ただし `request_discard` を使う font atlas resync は、その pass で描かれた
`show_viewport_immediate` の child viewport を egui 側で未描画扱いにし、passive detached
window の OS 窓を破棄→再生成させることがある。したがって active / passive detached viewport
が 1 つでも生きている間は、`detached_viewer_cleanup` だけでなく
`fullscreen_viewport_cleanup` / `native_video_backdrop_hide` /
`fullscreen_viewport_recreate` / `still_window_mode` も同じ pending resync に合流させ、
`detached_cleanup_font_atlas_resync_is_safe()` が真になるまで実際の resync を遅延する。
pending は最初の reason を保持し、detached が完全 idle になった時点でその reason のまま
`request_main_font_atlas_resync` を発火する。

**緩和策: クリア色をテーマ連動にする (`App::clear_color` / `main_window_clear_color`)**。
上記「1 フレーム黒」フォールバック、および detached 窓 close 直後の no-surface フレームで
メインウィンドウ surface に見えるのは eframe 既定の `clear_color` 値だが、これは
near-black `(12,12,12, a=180)` 固定。ダークテーマでは panel_fill と馴染むが、ライトテーマでは
「一瞬黒」が目立つ (再現性が高いのは `auto_fullscreen_zip_pdf` ON で PDF を直接フルスクリーンに
開き、Esc → `pending_return_to_parent` で親フォルダを `load_folder_or_convert_archive` 再読込する
経路。重い再読込が描画スキップフレームを安定して踏む)。`eframe::App::clear_color` を
オーバーライドして `visuals.panel_fill` (= メニューバー / パネル背景) を不透明で返すことで、
ライト/ダークどちらでも地と馴染ませる。これは **font atlas resync の defer 自体には触れない緩和策**
であり、上記の「stale font atlas で描かない = フォント崩れ回避」の保守トレードオフはそのまま維持する
(= 根本修正ではなく、黒の視認性だけを下げる)。回帰ガードは `main_window_clear_color_follows_theme_panel_fill`。

静止画の F11 / ホバーバーボタンで専用 fullscreen viewport から in-window 表示へ戻す経路は、
最初の embedded 描画がメイン `egui::Context` で走る。古い fullscreen viewport を隠した後に
resync を予約すると 1 フレームだけ stale font atlas でメイン UI を描いてしまうため、
`toggle_still_window_mode` で `still_window_mode` resync を先に予約する。

#### resync は数フレーム再発行する (close 直後の ROOT surface 欠落対策)

2026-06-18 に、マルチモニタで「画像フルスクリーン → ウィンドウに戻る」と **メインウィンドウの
UI 文字 (ツールバー・情報オーバーレイ等) が 100% 文字化けし、フォルダ移動で直る** 再発不具合を
egui-wgpu への一時計装で根因特定した:

- フルスクリーンを閉じる際、egui の shared font atlas は near-full から fresh atlas へ
  recreate されて **縮む** (例: 高さ 64 → 32)。これは `pos=None` の full(realloc) delta を生む。
- ところがその **同じ 1 フレームだけ、メインウィンドウ (ROOT viewport) の wgpu surface が
  一時的に消えており** (close / cloak / DWM 遷移中)、`egui-wgpu` の `paint_and_update_textures`
  が `surfaces.get(viewport_id) == None` で early return する。そのフレームの full(realloc) は
  **GPU へ届かず捨てられる**。
- egui の atlas は 32、GPU texture は 64 のまま固着し、glyph UV (atlas=32 で正規化) が
  64 高のテクスチャを参照して全 glyph が縦 2 倍ずれ → 文字化け。atlas が clean なので egui は
  delta を再発行せず、後で atlas が再び育って full delta が surface のあるフレームに当たるまで
  (= フォルダ移動で新規 glyph 追加) 直らない。

対策 = **resync を 1 フレームきりでなく数フレーム連続で再発行**する
(`MAIN_FONT_ATLAS_RESYNC_REPEAT_FRAMES`)。`request_main_font_atlas_resync` が
`main_font_atlas_resync_repeats_left` をセットし、`maybe_defer_for_main_font_atlas_resync` が
`ctx.cumulative_frame_nr()` で **1 OS フレーム 1 発行** にゲートしながら毎フレーム set_fonts を
再発行する。surface が戻ったフレームで full upload が確実に ROOT へ届く。フレームゲートは、
discard 再パスと `update_early` / `pre_main_ui` の 2 箇所呼び出しで同一フレーム内に repeats を
使い切る / 黒フレームになるのを防ぐ。連続再発行で font ファイルを毎フレーム読み直さないよう
`configure_fonts_for_texture_resync` は定義をプロセス内キャッシュ (定義は固定パス + 決定的
メトリクスで不変)。

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

`settings.detached_viewer_open_images_in_window` または detached 画像ビューア上バーのピン留めが
有効な画像 / ZIP画像 / PDFページ session は、メイン一覧との自動同期を行わない。次の画像を
開くとき、現在の active detached viewer は `TextureHandle` と表示位置だけを持つ passive
detached image window として退避される。passive window は最後の画像を表示するだけで、active
viewer cache / AI / 先読み / スライドショーは単一 active session のまま扱う。編集状態は
detached bundle 間で保持・確定しない方針とし、always-new / ピン留めの連動なし窓では
消しゴム・補正レイヤー・隠蔽加工・テキスト注釈・切り取り等の編集機能を起動できない。
通常 F12 の linked detached viewer では従来どおり編集できるが、編集中はピン留めできない。
passive snapshot は単ページでは 1 枚の texture を保持する。縦連結読みと見開き表示では、
pause 時点で可視だった各ページの `TextureHandle` と正規化済み表示矩形を frozen page list に
保持し、passive window 側で同じレイアウトを再描画する。見開きの片側がまだ未デコードで
texture を取得できない場合は multi-page frozen を作らず、従来の現在ページ 1 枚 snapshot へ
フォールバックする。
CUT 後、linked window は passive 化しない。設定 OFF の F12 linked window は最大 1 枚で
メイン一覧に追従し、別の independent / ParkedLive window を Active 化する場合は閉じる。
設定 `detached_viewer_open_images_in_window` が ON のときだけ、画像 / ZIP画像 / PDFページを
independent passive window として残す。動画は複数窓化せず、専用の detached 動画 window を
再利用する。

メイン一覧で `Enter` / 明示 open した項目が、開いている detached session の stamp と
同一なら、`open_fullscreen` を再実行せず前面化要求だけを出す。静止画 detached viewport
では `Minimized(false)` / `Visible(true)` / `Focus` を送り、動画 child presenter では
host 内で presenter raise 要求へ寄せる。同じ raw idx でも item key / generation が変わっている場合は、
通常どおり再オープンして表示状態を更新する。

F12 linked detached session で見開き 2 ページ表示中は、メイン一覧の通常カーソルを現在ページに置き、
相方ページがグリッド / 詳細一覧の可視範囲内に描画される場合だけ破線のサブカーソルを重ねる。
相方がスクロール外なら追加描画は行わない。複数ウィンドウモード / independent detached session は
メイン一覧との見開き連動表示ではないため、破線のサブカーソルを描画しない。detached 動画はメインウィンドウを占有しないため、
fullscreen / in-window 動画用の main backdrop や black chrome 判定から除外する。

detached window placement は `settings.detached_viewer_window_placement` に保存する。
保存値の意味は outer position + inner/client size + maximized flag。最大化中は restore
placement を上書きせず、`maximized` だけを更新する。

静止画フルスクリーンの左右パネル召喚は Settings の `FsSidePanelMode` を正本にする。
`Hover` は左端だけが補正パネル、右端だけがメタデータパネルを召喚し、上端は上バーの
独立経路だけを動かす。`ClickToShow` は端ホバーでパネルを開かず、通常ホバー帯より狭い
最端帯で呼び出しバーを表示する。ClickToShow の左右開状態は per-file transient で、
ファイル移動とフルスクリーン退出で閉じる。右は `App.fs_click_info_open`、左は面ごとの runtime
flag を使い、Settings には保存しない。端判定とモード別召喚判定は `ui_helpers` の純関数へ置く。
動画では左をジャンプ / ブックマーク、右をメタデータ / タグとして独立させ、`Hover` は従来の
二段ラッチ、`ClickToShow` は presenter-local な左状態と App から同期した右状態で可視性を決める。
egui 音楽ビューも同じ mode と App runtime を使う。3 面とも ClickToShow では通常の端ホバーを
無効化し、最端の painter 描画 callout から明示的に開閉する。
左右いずれかの実パネルが表示中なら上 + 下のクロームも同時表示する。動画は既存の
`panel_chrome_visible`、音楽は上下常時表示、静止画は render 経路で集約した
`side_panel_visible` を上バーと `FS_SEEK_BAR` の force 条件に使う。辺ごとの召喚分離は維持し、
右端ホバーだけで左編集パネルを開くことはない。
静止画では callout / 右パネルの描画可否と wheel・click の当たり判定を同じ述語で決める。
補正編集、表示トリム、分析、比較 wipe、360、ズーム、アニメ中など描画抑止中は、非表示の
右パネル矩形や ClickToShow の最端帯も入力を奪わない。

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

mIV Remote の動画ストリーミングは fullscreen cache / native presenter を使わず、remote session
state が headless `VideoPlayer` を所有して UI frame ごとに tick する。decoder と video/audio tap
は継続するが、folder load、`open_fullscreen`、detached presentation は要求しない。そのため本体の
既存表示を変更せず、「リモート接続中」modal を表示したまま配信できる。音声モードの `Music`
surface は従来の music-only predicate のままで、remote streaming 専用 surface は持たない。

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

表示用 / catalog 用サムネイルの整数寸法は `fast_resize::aspect_accurate_fit_dimensions` を
共用する。長辺を上限へ固定して短辺を 1 回だけ丸めると、PDF のように raster ごとの丸めが
異なるページで 0.1% 程度の縦横比誤差が残る。そこで width / height の両軸から整数候補を
列挙し、元の縦横比に対する相対誤差 0.05% 以下を満たす最大解像度の候補を選ぶ。PDF page box
(`layout_dims`) や JPEG DCT 縮小の元ピクセル寸法 (`source_dims`) のように decoded raster より
正確な比がある場合は、その比率を選択器へ渡す。PDFium の `fit_to_target` も page box を同じ選択器へ渡し、低解像度 raster を
最初から選択済み整数寸法で描画する。上限を超える拡大はせず、既存 catalog 行も強制再生成
しないため、この改善は新規生成分から適用する。

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
- 消しゴム (MI-GAN) / PDF 再レンダも同じ 8192 上限を尊重する。PDF vector は長辺
  256 以上、raster は native 上限を優先するため極小原稿では 256 未満になり得る。
- GIF / APNG / WebP アニメーションは `fs_animation::clamp_rgba_frame_for_gpu` で各フレームを
  `MAX_TEXTURE_DIM` 以下に縮めてから `ColorImage` 化する (巨大 animated 画像で
  `ctx.load_texture` が panic するのを防ぐ安全網)。

新しい経路で `FsCacheEntry::Static` を作るときは、`pixels` が 8192 以内であることを
自分で保証するか、`clamp_dynamic_for_gpu` を掛けてから格納する。UI スレッド側の
同期 Triangle リサイズを増やさないこと。

**mip chain を保持する経路**:

- `DISPLAY_IMAGE_TEXTURE_OPTIONS` を指定した managed texture は、ローカル差し替えした
  `vendor/egui-wgpu` が level 0 upload 後に完全な mip chain を GPU render pass で生成する。
- 対象は raw static、PDF/ZIP page、編集・補正・AI・注釈・比較用の表示 texture。Windowsの
  wipe/diff比較と360度パノラマの独自`Rgba8Unorm` textureも同じGPU生成器を使う。パノラマは
  水平フル/垂直cropではU=Repeat、水平cropではU=ClampToEdgeを選び、低LODで部分画像の
  反対端が混ざらないようにする。経度シームではU微分を周期補正して`textureSampleGrad`へ渡し、
  シームだけ過度に粗いmipが選ばれることを防ぐ。比較callbackは現在のpinned/current textureと
  mip chainを1組だけ保持し、入力失効・別サイズの再準備時に旧組を解放する。Cの通常表示へ戻る操作では
  準備済みpinned buffer / managed textureを保持し、同じcurrentへ再表示するときはworkerを起動しない。本文 / ナビゲータのuniform bufferと
  bind groupはtyped slotごとに分ける。右下のピン表示は72x54以下の専用textureを使う。1つの
  `TextureHandle` 内に全 level を保持するので、表示 texture の優先順位、論理サイズ、zoom、
  見開き、連結読み、ルーペ、pixel grid の座標系は変更しない。
- animated frame、動画、サムネイル、mask、checker、UI texture は対象外。明示的な
  `PostFilter::Nearest` も level 0 + nearest sampler のまま。
- 通常静止画の縮小は level 0 を入力に §2.4.1 の Lanczos3 出力を生成する。通常静止画向けの
  LOD bias / level 0 切替 uniform と renderer setter は持たない。wipe/diff 比較は固定の
  implicit mipmap sampling、360度パノラマは周期補正した勾配による `textureSampleGrad` を使う。
  「縮小時のなめらかさ」は通常静止画の Lanczos3 支持幅だけを変更し、比較・パノラマには影響しない。
- mip texture の partial update では全下位 level を再生成する。完全な chain の VRAM は level 0
  の約 1/3 増えるため、フルスクリーンの既存 prefetch / eviction 境界を越えて保持しない。

旧 LOD 設計の履歴は [downscale-moire-lod-plan.md](downscale-moire-lod-plan.md)、現在の決定は
[dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) を参照。

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
0. 元画像表示の割り当てをホールド中 (既定は右 Ctrl。fs_cache の raw decode)
1. erase / local_adjust / conceal の編集中プレビュー (各 UI の in-memory state)
2. final_composite_cache[edit_key, params_hash, bg]
   (= edit_result_cache + 色調補正 + final AI + スマートシャープ + カラー化
    + Creative LUT + post_filter。
    スマートシャープは AI アップスケール出力には掛からない —
    preset-and-adjustment.md §2.7)
3. edit_result_cache[edit_key]
   (= raw -> erase -> local_adjust -> conceal。crop を含まない最終段待ちの fallback)
4. fs_cache[idx] (生デコード結果、raw 専用)
5. フォールバック: サムネイル (低解像度)
```

AI 待ちの `complete=false` final composite も表示専用の候補である。final AI が到着しても
同一 `FinalCompositeKey` の entry は消さず、AI 後の final-effect worker が完成するまで
カラー化済みの暫定 texture を表示し続ける。完成結果の `insert` が同じ key を上書きするため、
表示はキャッシュ欠落を挟まず原子的に差し替わる。コピー / 書き出しと nav lock 解除は従来どおり
`complete=true` だけを受け付ける。

#### ページ単位表示のキーリピート

v2.13.0 で一度削除した通過表示は、2026-08-12 に単一 / 見開き共通で再実装した。physical
page-turn key level が held で、かつ実際の display-unit 遷移が成立したバースト中は、色忠実な
低解像度 rendition を全ページ一様に選び、final-effect worker の完了回収、`fs_upload_backlog`、
full decode / AI producer を保留する。境界で遷移しなかった入力では通過表示に入らず、release 後
(または held 中に境界へ達した後) は current display unit を `resolve_fs_processed_texture` の
通常経路へ戻す。詳細な要件と失敗から得た知見は §2.5 を正本とする。

`colorize_display_requires_final_effect(idx)` は materialized 経路で、設定が有効かだけでなく、
そのページで待たずに出せる画素と final-effect worker の出力が視覚的に変わるかを表す gate である。
`AllImages` は常に gate する。`MonochromeOnly` の高速パスは色調補正が identity の場合だけ使い、
`edit_result_cache`（raw → erase → local_adjust → conceal）の画素から作った近モノクロ要約
（主成分軸からの p95 直交残差）が現在の `mono_tolerance` を超えるページでは gate を外す。
これにより編集レイヤーが raw の chroma を落とした場合も、worker が見る入力と同じ編集結果を
基準にできる。色調補正が非 identity なら、その後の adjusted 画素が別物になるため安全側に gate
する。final AI は復元・拡大処理で入力の近モノクロ性を反転させないものとして扱う。
この条件を満たすカラー画像でカラー化が no-op になるページだけ、raw / edit / thumbnail fallback
を許可する。
通過 rendition ではこの保留 gate を「カラー化を適用する」という判定に流用しない。常駐する
完成 composite の実判定、既存 full-size summary、調整後の低解像度画素の順に 1 つの適用可否を
解決して rendition に保持し、着地点の final-effect worker も同じ判定を使う (§2.5.3)。
Creative LUT も**単独で** gate 条件に入れる。LUT もカラー化と同じ「色が変わる最終段」であり、
待たないと LUT 未適用の絵が 1 フレーム見えてから色が変わる (ユーザー報告 2026-07-29)。
gate している間の Ctrl+↑↓ は捨てず、folder-nav の accumulator が受け取る
(`handle_fullscreen_ctrl_nav_context` の lock 分岐と `KeyAction::press_multiplicity`)。
一度この gate を LUT から外した経緯があるが、当時の症状 (押下が消える) の原因は accumulator 側で、
別途修正済みである。post_filter 単独は同期合成なので gate 対象にしない。

近モノクロ要約は `idx → { EditResultKey, p95_residual }` で memo 化し、`poll_prefetch` の末尾から
1 frame 最大 4 件、現在の `fullscreen_idx` 最優先で計算する。producer は既存の
`edit_result_cache` entry だけを走査し、重い編集合成を新規生成しない。`EditResultKey` は source
generation と erase / local-adjust / conceal の各編集世代を含むため、再デコード、PDF 再レンダ、
編集操作のいずれでも自然に stale になる。memo miss、現在 key の edit result 不在、または
`EditResultKey` 不一致は安全側の `true` とし、要約が完成するまで従来どおり生画像へ
フォールバックしない。`edit_result_cache` から消えた entry は次の reconcile で memo からも落とす。

ページ枠での最終的な fallback 順は、`final/processed` → ページ単位表示で final-effect 待ち中の
色忠実 rendition → 連結読みの `continuous_page_transition_texture` → 未処理サムネイル →
`読込中...` である。見開きの final-effect 待ちはページ別に fallback せず、片側でも final が
未完了なら左右とも rendition、全ページが揃った frame で左右とも final へ切り替える。

`fs_holdover_tex` はこの優先順位には入らない。`FolderNavigation` /
`FinalEffectSourceReload` ともページ画像と編集 overlay の後に、共通描画で黒背景 +
退避した単ページ / 見開き全体を重ねる。ただし解放条件は統合しない。フォルダ移動は
`fs_nav_locked_gen` と新 anchor の表示 readiness、source reload は**同じ表示ユニット**の
全ページで final-effect 適用済み表示（または終端の読込失敗）が揃った描画 frame を使う。

Ctrl+↑↓ の nav lock 解放判定も、描画側と同じ
`fs_display_bypasses_final_pipeline` を使う。通常表示でカラー化が有効なページは
`complete=true` の final composite まで待つ一方、元画像表示と分析モードは
raw `fs_cache` を直接描くため、Static / Animated またはサムネイルの到着で解放する。
この表示モード判定を描画側と lock 側へ別々に実装してはならない。表示だけ raw に
切り替わって composite が生成されない場合、lock 側だけが完成を待ち続けるためである。

**この優先順位は動かさないこと**。変更すると「補正を掛けた瞬間に一瞬生画像が見える」
「AI 処理中にプリセットを変えると古い final composite が残る」等の不整合が出る。

実装上は `ui_fullscreen.rs::resolve_fs_processed_texture` を通常表示の共通入口にする。
単ページ、見開き、連結読み、ルーペが `edit_result_cache → fs_cache` のような独自チェーンを
再実装すると、新しい派生レイヤ (消しゴム / 隠蔽加工 / AI など) の横展開漏れが起きる。
見開きと連結読みも、画面全体で 1 枚を解決するのではなく、描画対象の **各ページ idx**
についてこの入口を呼ぶ。ページ単位の見開きで final-effect が未完了なら、各 idx の結果を
個別公開せず左右とも色忠実 rendition に揃える。同一 unit の source reload 中だけは
`FinalEffectSourceReload` が両方の final 完成まで旧 unit overlay を全体へ重ねる。
ルーペも hit-test 後に選ばれたページ idx で同じ入口を呼ぶ。

local-adjust / conceal の materialization は worker で進む。必要な local 結果が未完成なら
conceal は erase / raw を入力にして先へ合成せず `None` を返し、edit / final assembly も
未完成の上位結果を作らない。表示側はその間だけ現在有効な下位レイヤーへフォールバックし、
古い local / conceal cache を再表示しない。保存・比較・クリップボードの pixel job はこの
表示用フォールバックを完成結果として扱わず、必要な edit materialization の完了を待つ。
360 度パノラマ表示も同じ考え方で、完了済みの `final_composite_cache` を 8K base
アップロード元として優先する。final AI が未完了の間だけ旧 `adjustment_cache` /
`ai_upscale_cache` / `fs_cache` へフォールバックし、AI 完了後は cache_key を変えて
再アップロードする。
保存・比較・クリップボードのようなピクセル出力経路も、`prepare_capture_pixel_job` で
同じ最終 composite pixels を取得する。EXIF Orientation は decode 済みなのでここでは
再適用しないが、`rotation_db` の非破壊 90 度回転は GPU 描画専用のため、crop 後の
ピクセル出力段で焼き込む。補正レイヤーが有効だが `local_adjust_cache` がまだ無い場合、
古い結果や下位画像は保存せず、完了後の再実行を促す。

`FsOriginalPreviewHold` の元画像プレビューは例外的な一時表示で、派生キャッシュは作り直さない。
通常の画像 / ZIP 内画像 / PDF ページだけを対象にし、動画には適用しない。表示元は常に
`fs_cache` の生デコード結果で、補正 / AI / 消しゴム / 隠蔽の派生キャッシュは参照しない。
補正レイヤーの派生キャッシュも同様に参照しない。ただし補正レイヤーモード中の
`Ctrl+Shift` は「選択レイヤーをバイパスし他レイヤーを全て適用したプレビュー」に割り当てるため、
元画像プレビューはこの組み合わせを捕まえず、`resolve_fs_processed_texture` の
local_adjust 分岐に処理を譲る。元画像表示の割り当て単体は従来どおり元画像プレビューになる。
元画像プレビューの譲渡判定と layer bypass preview の modifier gate は、fullscreen viewport
外側の main `ctx.input` では modifier が取れないため、元画像表示と同じく OS キー状態を
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

各ページの実表示幾何の正本は
[`DisplayedImageTransform`](../src/displayed_image_transform.rs) である。単ページでは
`draw_fs_image` が、Z ズームでは `ResolvedZTransform` が `DisplayedImageTransform::resolve`
を 1 回だけ呼び、次の入力を同じ transform に確定する:

```
1. 上記の優先順位で解決済みのページテクスチャと source / texture size
2. 表示トリムの実効 content bbox
3. 回転 (rotation_db, 0/90/180/270) と一時的なフリー回転
4. viewport 固有の `pixels_per_point`、フィットモード、自動フィット倍率制限
5. Zoom / Pan、または Z ズームの確定済み placement
6. 物理ピクセル境界へ整列済みの paint rect / hit rect / UV / source↔screen 写像 / total scale
```

`Original` (100% 原寸) と「拡大しない」「縮小しない」の 100% 基準は論理 1.0 倍ではなく
`1.0 / pixels_per_point` とする。`pixels_per_point` は描画対象 viewport の `egui::Context`
から取得し、main と DPI が異なり得る detached / frozen viewport では main の値を流用しない。
実効物理倍率 `total_scale * pixels_per_point` が整数に十分近い場合だけ、最終描画矩形の
**位置**を `round(position * pixels_per_point) / pixels_per_point` へ整列する。矩形サイズは
倍率とアスペクト比を保つため丸めない。見開きは物理 px 単位で配置し、ページ間隔も
`round(gap * pixels_per_point) / pixels_per_point` へ量子化する。連結読みは各ページ位置の
累積ごとに同じ量子化を行い、長いページ列でも端数誤差を持ち越さない。
見開きは `Original` (100% 原寸) のときだけ各ページの固有寸法を保ち、それ以外の
フィット方式では倍率にかかわらず高いページへ高さを合わせる。高さ合わせ中の原点整列は、
ページごとの実効倍率 `total_scale * height_match_scale` をこの判定に使う。

fit / trim / rotation / zoom-pan を overlay や入力処理が再計算してはならない。
`draw_fs_image` は解決した transform 自身で描画し、その同じ値を次の consumer へ渡す:

- 消しゴム、隠蔽加工、補正レイヤー、テキスト注釈、crop の overlay と pointer 座標変換
- ルーペのカーソル位置から source pixel への逆変換
- 範囲キャプチャの対象ページ判定と source rect 変換
- フレーム内のページ hit-test 用 `FullscreenPageLayout`

Z ズームも別の screen/source 計算を持たず、`ResolvedZTransform` が cover / contain と
pan を決めた後の `DisplayedImageTransform` を描画と上記 consumer が共有する。このため
ルーペはズーム前の矩形ではなく、実際に描いた Z transform を逆変換に使う。

`FullscreenPageLayout` は毎フレーム `Single` / `Spread` / `Continuous` の種別で初期化し、
**実際に描けたページ**を paint 順に `DisplayedImageTransform` ごと登録する。
`hit_test(pos)` は 3 モード共通でページと transform を返す。ページ間 gap はどのページにも
属さず、gap が 0 の共有境界は先に描いたページへ決定論的に割り当てる。ルーペはここで得た
page idx の processed texture をページ単位で解決し、範囲キャプチャも同じ hit-test を使う。
見開き / 連結読みのページ配置計算は引き続き `ui_fullscreen.rs` が担当する。ただし配置側が
確定した page rect と、最終的に選ばれた texture の比率は progressive load / PDF 再 raster /
加工済み texture の差し替え中に異なり得るため、live paint は
`resolve_fs_transform_in_layout_rect` で両者を同時に束縛してから共通レイアウトへ登録する。
page rect は包含枠として保持し、実 texture はその内側へ contain する。これにより source/layout
比と texture 比が異なる frame でも `scale_x == scale_y` を保ち、矩形をそのまま別比率の raster
へ渡して横または縦だけを伸縮させない。

フルスクリーンのナビゲータも、このフレームで描画済みの `FullscreenPageLayout` を正本にする。
単ページ / 見開きの各 `DisplayedImageTransform` を同じ比率で縮小した座標空間へ写し、
現在範囲は `visible_source_uv_rect(clip_rect)` の UV をその縮小 transform へ戻して描く。
画像全体が見えている場合もナビゲータは隠さない。表示範囲枠は全体枠に置き、実表示がフィット基準
より小さいときは縮小画像だけを内側へ連続的に縮める（下限 40%）。フィット丁度では画像 = 枠 =
全体となり、ズームアウトをまたいで大きさが飛ばない。
ナビゲータ上の位置は `screen_to_source_normalized` で UV へ変換し、
`pan_to_center_source_normalized` で倍率を変えずに画面中央へ置く `fs_pan` を求める。
このためフィット、表示トリム、90度回転、見開きの配置をナビゲータ側で再計算しない。
縮小画像は、そのフレームの単ページ / 見開き描画が解決済みの
`FullscreenPaintResource::source_texture()` を使う。これは加工済みの full-image texture であり、
ズーム中に可視 source 領域だけを持つ Lanczos 拡大出力 (`paint_texture_id()`) は使わない。
加工済み composite が未準備のページだけは既存のサムネイルカタログへフォールバックし、黒や
点滅を挟まない。nav / final-effect source-reload holdover を重ねたフレームは、ナビゲータへ渡す idx → resource も
その display unit 全体へ置き換え、本文とナビゲータが新旧ページで食い違わないようにする。
ナビゲータ自身は texture producer を呼ばず、UI スレッドで画像読み込みやデコードも行わない。
比較表示では本文と同じ単一比較キャンバスをレイアウト正本とし、Wipe / Diff はナビゲータの
画像矩形へ `zoom_pan=None` で同じ `CompareShaderCallback` を再描画する。PinnedNormal は
準備済み pinned texture を直接描画する。同一フレームの2 callback は同じ `pair.key` と寸法を
渡すため GPU texture / mip chainを再利用する。一方、`egui-wgpu`は全callbackのprepare後にpaint
するため、callbackごとに異なるuniformとbind groupは`CompareShaderSlot::{Main, Navigator}`ごとに
保持し、後のprepareが先の描画状態を上書きしない。
本文の primary 描画は `CompareFramePrimaryDraw` が排他的に所有する。現在ページに一致する
`ComparePreparationState::Ready` があるフレームは準備済み比較だけを描き、`Unprepared` /
`WaitingForSource` / `Preparing` / `Draining` と別ページの `Ready` は通常ページだけを描く。準備中に raw pinned
texture や Wipe の切れ端を通常ページへ重ねず、準備済み比較を描いた後も nav / source-reload holdover を
重ねない。
比較を準備・描画できるのは `SpreadMode::Single` の単ページ表示だけとする。X / C / Shift+C / Alt+C
の全入口は見開き表示モードで案内toastを表示して状態を変更しない。表紙が1枚だけ見える瞬間も
layout結果ではなく表示モードで拒否する。ページ移動や表示モード変更で比較中に見開き表示モードへ
入った場合は、フレーム先頭で比較を終了してから通常の見開きを描く。
見開き側の primary 分岐には比較準備・比較描画を置かない。
比較キャンバスは current 側の縦横比を正本とし、その比率を維持したまま pinned 側の入力寸法も
不必要な縮小なしで収まる解像度まで引き上げる。縦横比が違う pinned 側は等比で中央配置する。
このとき pinned の範囲外はフルスクリーン viewport の既定背景と同じ不透明な黒で
埋め、WGPU の alpha blend と CPU fallback のどちらでも下層の current を透過させない。Diff は
この黒背景と current の画素差を表示するため、current が黒でない余白は「差あり」として見える。
単ページ画像は decode 時点で `MAX_TEXTURE_DIM = 8192` 以下だが、比較workerも完成キャンバスが上限を
超えた場合は全体を Lanczos で等比縮小する backstop を持つ。上限内は寸法も画素も変更しない。
独自 WGPU upload も同じ判定を texture 作成の直前に通し、別入口が増えても上限超過の texture を
作らない。
比較準備workerは同時に1本だけ所有し、失効した途中cancel不能workerは`Draining`で
完了を回収してから次要求を開始する。workerは結果送信直後にeguiへrepaintを要求し、100ms tickを
待たずに完了を回収する。PinnedNormalはcurrentの完成寸法だけをworker開始前に使い、不要なcurrent RGBAを
生成しない。描画方式ごとのtyped CPU payloadにより、WindowsのWipe / Diffは
pinned/current RGBA 2枚だけを保持し、Diff用CPU画像 / managed textureはCPU fallbackのDiff時だけ作る。
本文の比較 callback は、ズーム / パン後の実画像矩形と viewport の交差だけを callback rect にし、
切り落とした範囲を uniform の UV 窓で元の合成画像座標へ戻す。テクスチャ採取と Wipe 境界判定は
復元後の座標を共有するため、白線の基準である実画像矩形と一致する。ナビゲータは実画像矩形が
パネル内に収まり UV 窓が常に全域となるので、このクリップによる表示変更はない。
Wipe の白線はドラッグ中、またはポインタが`compare_wipe_grab_hit`と同じgrab band内にある間だけ描く。
画像上のそれ以外の場所では隠し、touch chrome latchは表示条件に使わない。タッチ由来のprimary dragも
`compare_wipe_dragging`を共有するため、指でドラッグしている間は表示する。境界 fraction は CPU の線 / clip と GPU uniform の双方で `0.0..=1.0` とし、左右端まで
ドラッグできる。
通常の単ページ / 見開きで利用者入力から `fs_pan` を更新するときは、直前の
`FullscreenPageLayout` に記録された実表示矩形を使い、少なくとも 1 ページが viewport と
各軸 48 logical point（ページまたは viewport がそれより小さい軸では、その小さい方の全幅）
以上重なる範囲へ制約する。48 point はポインタ / タッチで再びつかめる標準的な操作領域を
残すための値であり、制約範囲内の pan は変更しない。これにより Space ドラッグ、ゲームパッド、
ホイールの pivot 補正、ナビゲータの移動 / 範囲拡大のどの入口でも、画像が完全に画面外へ出ない。
ナビゲータの `visible_source_uv_rect` が得られない場合の自動非表示は安全機構として扱わない。
部分拡大ズーム（Z）は既存の専用制約を使い、連結読みのスクロールもこの制約の対象外とする。

ナビゲータが実際に表示されている間は、そのパネル矩形を配置された側の左右端と上下端まで
それぞれ広げた領域を、同じ側のサイドパネルと上下ホバーバーの受動的な表示判定から除外する。
反対側の画面端は除外せず、ナビゲータが実効非表示なら除外領域を作らない。
360度パノラマでは `FullscreenPageLayout` を使わず、equirect の元画像をレターボックス表示する。
現在範囲は viewport の NDC 外周を `panorama::ndc_to_equirect_uv` へ通した折れ線とし、
`aspect = viewport_w / viewport_h` と `tan_half = tan(fov_y / 2)` は `panorama_wgpu` と同じ導出を使う。
yaw の継ぎ目では隣接点の U 差が 0.5 を超える線分を分割する。クリック / ドラッグは
`PanoramaState` の yaw / pitch / fov_y だけを更新し、GPU 描画経路には介入しない。
平面・パノラマとも、利用者が固定表示またはホールド表示を要求している間は全体可視でも表示する。
`FullscreenPageLayoutKind::Continuous` は全体図が極端に細長くなるため引き続き対象外とする。

### 2.4.1 フルスクリーン静止画の GPU Lanczos3 縮小・標準拡大と選択式拡大

通常静止画は、上記の表示優先順位と
`edit -> color -> final AI -> smart sharpen -> post_filter` の合成をすべて解決した**後**に、
最終 paint 専用の `FullscreenPaintResource` へ包む。この段階は source cache や合成結果を
置き換えず、ピクセル内容の優先順位・合成順序にも介入しない。

`FullscreenPaintResource` は `Direct / Resampleable / Lanczos` の typed state で、全 variant が
元の `TextureHandle` を保持する。レイアウト、trim UV、回転、hit-test、pixel grid は元ハンドルの
`size_vec2()` から従来どおり `DisplayedImageTransform` が解決する。縮小時、標準拡大時、シャープ拡大時、アニメ塗り拡大時だけ
GPU resampler の native texture を別所有する (C-1)。`Lanczos` という resource variant 名はこの
共有出力の既存所有境界を示し、実際の shader は `scale_branch` で Lanczos3 / NIS / Anime4K を区別する。縮小出力は従来どおり `TextureId` だけを
差し替え、拡大出力はその texture が表す元画像 UV も typed resource に保持して同じ
`DisplayedImageTransform` 上の対応位置へ描く。level 0 の `Rgba8Unorm` source を vertical
`Rgba16Float`、horizontal `Rgba8Unorm` の 2 pass で直接リサンプルする。box mip 前縮小、
倍率閾値、ヒステリシスは持たない。Lanczos4 は採用しない。
画像補正パネルの「縮小時のなめらかさ」は 0〜100%・10% 刻みで、
`blur = 1.0 + percent × 0.003`（1.00〜1.30）へ変換する。各軸の
`filter_stretch = max(1, 1 / scale) × blur` とし、支持幅・重み・CPU 側の推定 fetch 数を
同じ値から導出する。既定の 0% は blur 1.00 である。

分岐は `total_scale * pixels_per_point` と
`physical_scale_is_near_integer` を共通基準にする。

- 1.0 未満: CPU 参照と同じ `floor(source × physical_scale)` の目標寸法へ Lanczos3。
- 1.0: 元 `TextureId` を直接 paint し、段階1・2のドットバイドットを維持。
- 1.0 超かつ `PostFilter::None`: 表示 trim と画面に映っている範囲の積だけを、表示解像度の
  Lanczos3 texture へ生成する。整数倍も含む。拡大では blur 1.00 固定とし、カーネルを
  1.0 より広げない。パン先読み領域は持たず、可視範囲が変われば再生成する。
- 1.0 超かつ `PostFilter::Nearest`: 元 `TextureId` を直接 paint し、NEAREST を維持。
- 1.0 超かつ `PostFilter::UpscaleSharp`: `PostFilter::None` と同じ可視 source 領域と目標寸法を
  NVIDIA Image Scaling で生成する。公式 SDK の 64 位相係数と 6×6 support を 1 pass fragment
  shader へ移植し、倍率上限は持たない。alpha は bilinear のまま USM の対象外とし、
  premultiplied RGB を alpha 以下に保つ。
- 1.0 超かつ `PostFilter::UpscaleAnime`: 同じ可視 source 領域と目標寸法を、17 枚の
  `RGBA16Float` 中間 texture を使う x2 VL 多段処理で生成する。alpha は bilinear のまま保ち、
  premultiplied RGB を alpha 以下に保つ。可視 source 領域の長辺が設定上限（2048px /
  4096px / 制限なし、既定 4096px）を超える場合は、中間 texture を確保する前に同じ領域の
  `UpscaleLanczos` へ切り替える。上限ちょうどは処理対象とする。
- 1.0 超かつ CRT / レトロ / セピア等の効果 post-filter: 元 `TextureId` を直接 paint し、
  従来の LINEAR を維持。拡大方式と効果は独立選択にしない。

拡大出力は一辺 8192px、総画素 4096×4096 相当までとし、いずれかを超える場合は
resampler texture を生成せず従来の LINEAR 表示へ戻す。フォールバックは同じ source generation
と branch につき一度 `gpu/lanczos_upscale_limit_fallback` へ記録し、3 種の拡大は
`scale_branch` フィールドで区別する。

単ページ、見開きの各ページ、縦 / 横連結読み、page transition、nav / source-reload holdover、
detached single / spread / continuous frozen snapshot と keep-alive backstop は同じ typed
resource を通る。見開きは高さ合わせ係数を含むページ別実効倍率を使う。ルーペは鮮明な元画素が
必要なので `resolve_fs_processed_texture -> TextureHandle` の元経路、pixel grid は元論理寸法の
まま。比較表示 (wipe/diff) と 360 度パノラマは既存 callback ownership、thumbnail、animated、
動画、mask、checker、UI preview は direct 経路のままである。

GPU resampler cache は viewer context ごとに所有し、key は page idx、元 `TextureId`、
`items_generation`、ページ別 `input_generation`、目標寸法、正規化済み smoothing percent、
拡大 / 縮小 branch から成る。拡大 entry だけは可視 source UV も key に含め、縮小 entry は
従来どおり full source 固定である。設定値が変わると context 内の Lanczos 出力 cache を消去し、
typed resource に保持した旧 percent と一致しない出力も再利用しない。source ごとの直近 2 寸法
(拡大 / 縮小 branch 別)、context 全体 64 entry の LRU とし、
fullscreen close / invalidation / context park・swap / 連結読み keep-set に追従する。native
`TextureId` は cache、holdover、snapshot が共有する `Arc` lease の最終 drop で free する。
出力は `LanczosOutputs` として実寸・mip なしで VRAM 会計する。拡大 cache だけは context ごとに
総画素 4096×4096 相当の 2 枚分を追加上限として古い entry を落とし、縮小 cache の保持規則は変えない。
perf log の
`gpu/lanczos_regenerate` に source / target、smoothing percent / blur、推定 fetch 数、
encode + submit CPU 時間、累積再生成回数、拡大 / 縮小の別を記録する。Lanczos3 / NIS /
Anime4K は event 名を共用し、`scale_branch` (`upscale` / `upscale_nis` / `upscale_anime`)
で区別する。拡大時はさらに、
生成元となった source pixel 領域の x / y / width / height を記録する。

静止画の最終フィット矩形は `fullscreen_media_rect` が所有する。下部ページシークバー固定時は
`FS_SEEK_BAR_HEIGHT`、上部情報バー固定時は `TOP_BAR_HEIGHT` をそれぞれ `full_rect` から除外し、
両方固定なら上下を同時に除外した同一矩形を、単ページ・見開き・連結読み・入力座標へ渡す。
固定領域の予約は各バーの描画可否と同じ述語を使う。特に編集／注釈・範囲キャプチャ・音楽ビューで
上部バーを抑止するときは `TOP_BAR_HEIGHT` も予約せず、非表示バー由来の黒帯を残さない。
上端の原画プレビュー・スライドショー進捗インジケータと、下端の比較ピンインジケータも
この同じ矩形を配置基準にし、固定バーの高さを個別の定数補正として重ねない。
透過背景 (Shift+B) の変更通知は共通の feedback toast だけが担い、専用インジケータを持たない。
同じ文言・同じ表示時間 (`FEEDBACK_TOAST_DURATION`) の表示を 2 系統に分けると、上部バー固定時に
右上で重なるため。持続表示が要る原画プレビューと違い、モード変更の一時通知はトーストの役目である。
上部情報バーの見開き2ページ情報は表示済み `fs_cache` / `ThumbnailState` / `image_metas` だけから
構築し、HUD 描画のために同期ファイル I/O やアーカイブ読み込みを追加しない。AI 処理名と
処理後解像度も current page の一時状態ではなくページ別の実効補正値・cache から解決し、
単ページと見開き左右の各行へ同じ規則で表示する。バーが固定・ホバー・popup のいずれでも表示されない
フレームでは、この文字列と AI ラベル自体を構築しない。固定暗色面に載る右下ページ番号と
シークバーのページラベルはテーマや文字コントラスト設定に左右されない専用の明色を使う。
混在メディア要約など通常 HUD の文字は暗色 UI のセマンティック文字色へ追従する。

静止画の上部バーは、通常閲覧中のページ解決 (`SpreadPair::Single` / `Double`) や
`ReadingFlow` の違いだけでは `draw_bar_button` の個数と x 座標を変えない。分析入口は右上の
「その他の機能」パネルへ移し、`…` は `×` の直後となる右から 2 番目に置く。360 度入口は
常に同じ slot を確保して、見開き 2 ページ表示、
連結読み、非 360 画像では disabled 描画と理由 tooltip にする。実描画を 4 組
（見開き単ページ / 見開き 2 ページ × ページ単位 / 連結読み）で走らせ、登録済み button rect の
個数と全 x 座標が一致することを回帰テストで固定する。

「その他の機能」パネルは `FsOverflowPanelState` が閉 / root / ナビゲータ位置を排他的に所有し、
上部バーの下へ 12pt、画面右端から 12pt 離して配置する。項目はナビゲータ ON / OFF →
ナビゲータ位置 → 分析ツール → ピクセルグリッド → ルーペ固定の固定順にする。行高は 48pt、
ヘッダーと閉じる hit target は 44pt とし、短い viewport では本文だけをスクロールする。同じ純粋な
panel rect resolver を描画と `touch_excluded` が共有する。表示中は
`App::common_modal_dialog_open` に含めて wheel / key を止め、背面キャンバスの pointer / navigator
操作も止める。touch recognizer 自体は terminal frame まで駆動して ownership を残さない。状態表示や
実効 shortcut label は既存 state / keymap の読み取りだけで組み立て、UI 描画のための同期 I/O は行わない。

ページシークバーの実効左右方向は `fullscreen_seek_direction` と `reading_direction` から一度だけ
決定し、ラベル配置、pointer fraction、つまみ位置、進捗塗りへ共有する。見た目だけを反転して
クリック先が逆になるような独立判定を置かない。
通常の左右カーソルキーによるページ移動は
`fullscreen_horizontal_cursor_direction` で、従来のページ表示方向か、このシークバー実効方向かを
選ぶ。対象はページ移動になる左右キーだけで、横連結中の左右スクロール、Shift / Ctrl+左右、
`FsPagePrev/Next`、PageUp / PageDown、画面端クリック、ホイールには適用しない。

**ZipPla 風 全画面ズームモード (<kbd>Z</kbd>、v2.0.0)** は、上記の通常ズーム/パンとは別系統だが
**解決後の `DisplayedImageTransform` と描画は共用**する (`draw_fs_zoom_mode`)。`KeyAction::FsZoomMode` (KeyHold、既定 Z、
keymap カスタマイズ可) のホールドで動き、
ズーム中は「cover 倍率 (画像が画面を覆う最小倍率) × `fs_zoom_factor`」を `zoom_pan` に変換して
ページ全体フィット指定で描く (`zip_cover_zoom_pan`)。**既定 `fs_zoom_factor = 1.0` = cover** で、
縦長画像では横幅目一杯 (ZipPla 単ページの既定) になる。ホイールは照準中 (Z 押下中) のみ倍率変更で、
ズーム確定後は前後ページ移動 (倍率変更は Ctrl+ホイール、ZipPla 準拠)。
**ズームアウト下限 = contain** (`fs_zoom_factor < 1.0` を許可): 片方の軸が画面に収まり、もう片方に
余白が出始める点まで縮小でき、それ以上 (上下左右どちらにも余白が出る状態) には縮まない
(`total = (cover × factor).max(contain)`)。`factor` の dead zone を避けるため、描画側 (`draw_fs_zoom_mode` /
`draw_fs_spread`) で画像ごとに `fs_zoom_factor` を `[contain/cover, 16]` (見開きは合成ページ基準) へ
clamp する。`adjust_fs_zoom_factor` は広めに `[0.05, 16]` で受け、描画側 clamp が最終的な下限を決める。
パンはカーソル位置を**操作帯 (`pan_band`)** 基準で元画像範囲へ写す (`zip_cursor_image_px`)。操作帯は
画面から上下のホバーバー領域 (上部バー `TOP_BAR_HOVER_Y` / 下部 `FS_SEEK_BAR_HEIGHT`) を内側へ
詰めた矩形で、**カーソルがホバー領域へ入る前に画像の上端・下端へ到達**する (実機 FB 2026-06-21)。
ズーム中は左右の補正/メタデータパネルを抑止して (`adjustment_active` / メタデータ描画を
`!fs_zoom_active` ゲート、左端ホバーの `adjustment_mode` も抑止) パン操作を邪魔しない。
照準中 (Z 押下中) は**トリム後コンテンツを contain 表示**して (表示トリムが無ければ画像全体)
ズーム範囲の枠 (`zip_aim_frame_rect`) を重ねる (パン操作帯と同じ写像なので、離した瞬間の表示範囲と
枠が一致)。倍率・状態は settings に
保存せずセッション内のみ保持し、ページ送りをまたいで維持・グリッドへ戻ると解除。
単ページ・見開きの通常閲覧で動作 (連結 / パノラマ / 動画 / 分析モードでは無効)。**見開き** は
`draw_fs_spread` 側で合成ページを対象に `zip_spread_zoom_pan` で同様にズーム/パン
(既定 = 単ページ幅の約 1.2 倍)。**表示トリム中はトリム後合成ページ (`fit_w × fit_h`) を対象**にし、
`zip_spread_zoom_pan` の pan/clamp をトリム後座標で行うことで `content_center_offset` +
`layout_spread_page_rects` のトリム配置と座標系が一致する (= トリム後コンテンツだけを拡大、余白を
見せない)。`content_center_offset` (トリム中央寄せ) は確定ズーム中も維持する。トリムが無ければ
`fit_w = combined_w` で従来と一致。**PDF** (単ページ) は
`maybe_rerender_pdf(zoom)` でズーム倍率に応じ高解像度へ再レンダ ((idx, zoom) が変わったときだけ要求し、
in-flight キャンセルの無限ループを避ける)。**既知の制限**: PDF を見開きでズームしても再レンダせず
fit 解像度のまま (画像見開きは問題なし。PDF 見開きズームは niche のため後続)。**表示トリム** 中は
ズーム対象をトリム後 bbox に絞り (上記コンテンツ領域 `content_min`/`content_size`)、確定ズームは余白を
クリップ、照準オーバービューも `draw_fs_image` へ `content_bbox` を渡してトリム後コンテンツを表示する
(= ズームのどの段でも切り落とした余白を見せない)。`zip_aim_frame_rect` はそのコンテンツ contain 表示
矩形を基準に枠を置く。操作仕様は [keymap-spec.md](keymap-spec.md) を参照。
見開きの geometry は Z ズームの解決前にフィット方式だけから確定するため、
`zip_spread_zoom_pan` の倍率・パン解決は 1 回だけ行う。`Original` 以外は従来の高さ合わせ
composite をそのまま使う。

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
を物理 100% (`1.0 / pixels_per_point`) で clamp してから、ユーザー操作の `fs_zoom` を掛ける。明示的な Ctrl+ホイール、
中ボタンドラッグ、ゲームパッド等の手動ズームは制限しない。`縮小しない` が ON の場合は
ページ全体フィットでも 100% 表示で画面外へはみ出すことがあるため、`fullscreen_fit_allows_drag_pan`
も true 扱いにしてパンできるようにする。
100% 判定の基準サイズは、単ページ / 見開きでは「今実際に描くテクスチャ」のサイズであり、
AI アップスケール完了後は `final_composite_cache` の寸法で再レイアウトする。
連結読みではスクロール中の配置ジャンプを避けるため、processed テクスチャが raw と同じ
アスペクト比なら raw/source サイズをレイアウト基準として保つ。別の処理で processed 側の
アスペクト比が変わった場合だけ processed 側のサイズを使う。crop は通常表示では暗転 overlay
だけなので、レイアウト基準も描画 UV も変えない。元画像プレビューや分析モードでは
raw 表示に合わせるため、raw サイズを優先する。

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
自動余白カットでは、左右ページで上下の検出量が違うと見開き全体の基準が崩れるため、
上 / 下は左右の少ないカット幅を共通採用し、横方向 (左 / 右、中央側 / 外側) はページ別の検出を残す。
中央側をトリムした場合も、切った領域を見開き中央に残さず、左右の見える端が設定 gap で並ぶ。
`fit_scale` は左右の見える幅の合計と上下 bbox の union 高さを基準にし、描画時はページ全体
rect のうち bbox 部分だけを UV 指定して描く。トリム量によってページ全体 rect が重なり得ても、
各ページの `DisplayedImageTransform` が実際の UV / paint / hit rect を保持し、
`FullscreenPageLayout::hit_test` はその見える hit rect だけを判定する。どちらかが回転していれば
通常フィットにフォールバック。

**表示トリム** (`ui_view_trim.rs` / `view_trim.rs` / `view_trim_db.rs`) は、
漫画ビューア用途で読みながら使う表示専用トリム。左端 / 上端 / 右端ホバーで開く左パネルの
`画像補正 / 表示トリム / ブックマーク` タブから操作し、選択中タブは `Settings::fullscreen_left_panel_tab`
へ保存する。
本側の基本モードは `ViewTrimApplyMode::{None, Auto, Book}` のラジオで選ぶ。
`None` は bbox を使わず、`Auto` は表示中ページごとに `cached_margin_bbox` を使う。
ただし見開きの `Auto` は左右をペアで調停し、上下だけ少ないカット幅に揃える。
`Book` は手動設定の基底モードで、各 `idx` に enabled なページ個別行があれば `Page`、
無ければ本全体設定を適用する。したがってページを移動して戻っても、保存済みの個別行は
選択操作なしで自動適用される。連結読みと見開きは左右を含む各 `idx` を独立して解決し、
個別行のあるページと無いページが同時に描画されても Page / Book を混在できる。
`None` / `Auto` の間は保存済みページ個別行を参照しない。
UI の `適用範囲` は enabled な個別行の有無から毎フレーム導出する。
`このページ` は個別行を作成し、`本全体` は対象ページの個別行を DB から削除する。
`Auto` から `手動設定` → `このページ` へ入った場合は、現在表示中の自動検出 bbox を
ページ個別値へ流し込み、見た目を保ったまま手動調整へ移る
(`手動設定` を経由する 2 クリックになったため、`Auto` から入った事実を UI 一時状態で持ち越す)。
`Book` / `Page` では、単ページ / 見開き連動 (上・下・中央側・外側) / 見開き左右別を
0〜20% で調整する UI が展開される。スライダーを動かした対象は自動的にその手動モードで
適用される。手動指定された bbox も、自動検出 bbox も、ページ全体 / 横幅 / 縦幅 /
100% 原寸で fit 基準として使う。
bbox 外は描画せず背景色を見せるだけで、
`export_crop.rs` の切り取り、保存、Ctrl+E エクスポート、補正 / AI キャッシュには影響しない。
見開きの「左右別」切替は値を移行し、連動→左右別では中央側/外側を左右ページへ展開、
左右別→連動では中央側/外側/上下を平均値へ畳む。`Page` での「左右別」切替は
本全体設定を書き換えず、各ページ個別行の値と表示側 semantics に反映する。ページ個別値のうち、見開き連動 UI から
作られたものは作成時の表示側を記録し、表紙設定や 1 ページずらしで左右が入れ替わっても
中央側 / 外側の意味を保って適用する。左右別 UI や Auto から固定した値は画像座標のまま保持する。
自動検出ボタンは `Book` / `Page` の手動設定へ現在ページの検出結果を流し込むための補助であり、
`Auto` とは別物。ただし検出元は `Auto` と同じ `cached_margin_bbox` に揃え、表示中の自動余白カットと
手動値へ流し込む自動検出で左右の結果がずれないようにする。見開き連動で左右を別々に調整しない場合は、
手動値へ流し込む自動検出も上下 / 中央側 / 外側の共通値に左右の少ないカット幅を採用する。
`Auto` は保存済みの手動値を作らず、ページごとに検出する。
旧 `FullscreenFitMode::MarginFit` が保存されている場合は、表示トリムの適用モード `Auto` と
ページ全体フィットへ移行して保存する。
基本適用モードと本全体設定は `view_trim.db::view_trim_books` に本キー
(`spread_container_key` と同じ粒度。ネスト ZIP は zip_path + 実効 prefix) で保存する。
ページ個別設定は `view_trim.db::view_trim_pages` に `page_path_key` で保存する。
`Page` は本側の適用モードとしては保存しない。代わりに enabled なページ個別行そのものが
そのページの永続 Page 選択を表す。`effective_view_trim_apply_mode_for_idx(idx)` が基本モードと
当該行を一箇所で解決し、`view_trim_single_bbox` / `view_trim_spread_bbox` と各 content bbox
解決が同じ規則に従う。見開きの片側だけに個別行がある状態も保持し、反対側は Book へ戻す。
この DB はリリース済みデータなのでマイグレーションや既存行の削除は行わない。従来残っていた
enabled 行も、新仕様では該当ページを表示した時点で自動適用される。
`Auto` は「モード」だけ保存し、検出 bbox は保存しない。スライダードラッグ中はメモリだけ更新し、
マウスを離したフレームで本設定と表示中ページの個別値をまとめて SQLite へ書き込む。

mIV Remote のページ描画は protocol v29 で現在の container address、画面上の
`single / spread_left / spread_right`、見開き相手 address を Page 要求へ添え、本体と同じ本キー・ページキー・
左右 semantics で `None` / `Book` / enabled な `Page` を解決する。remote 側の
`ViewTrimDb` は既存 `view_trim.db` を `SQLITE_OPEN_READ_ONLY` だけで開き、DB が無い場合も
作成・DDL・migration は行わない。表示 crop は committed な最終合成（加工用 crop を含む）の
後、端末用 resize / JPEG encode の直前だけに適用するため、補正 / AI の materialized pixels と
その cache は表示トリム前のまま保持する。`Auto` は本体と同じく補正前の raw raster を
`detect_content_bbox` へ通す。見開き request は自ページの bbox を先に小さな LRU へ公開し、
相手 bbox が未検出なら相手 request を待たず、同じ worker / cancel token で相手 raw raster だけを
復号する。これにより heavy worker が 1 本でも deadlock せず、2 本なら並行 request が互いの結果を
再利用できる。LRU key は `page_key + source mtime/size + target_px` で、補正・AI・表示トリム設定を
含めない。本体の `(load_seq, pixels_ptr)` と owner / 無効化条件は同一ではなく、remote は既存の
decode / composite cache と同じ source stamp または decode 上限が変わった時に再検出する。同一 size / mtime
を保った source 差し替えは既存 remote cache と同じ制約を持つ。端末からのモード変更は別段とし、remote は
設定を書き込まない。

本体側だけで表示トリム設定を変更した直後は、端末の `PageResourceCache` と HTTP の
`private, max-age=60` が既存 JPEG を返し得る。この既存制約は Auto でも同じで、response cache hit
では IPC と bbox 検出自体が走らない。IPC まで到達した要求は最新の read-only DB 状態を読み、raw bbox
LRU は設定値を保持しないため stale 期間を延長しない。即時反映には、本体の設定 revision を remote / SPA
へ通知して page render revision を進める別の無効化経路が必要になる。

Spread モード (見開き) の場合は、`draw_fs_spread` が `resolve_spread_pair` で左右の idx と配置
(LTR/RTL/Cover) を決め、両ページを「1 枚の合成画像」とみなしてレイアウトする。

フォルダ / 仮想ビューの読み込み時は、`page_order_locked_for_items` が install 前の
`source_path` と確定済み `items`、対象ビュー種別から「この読み込みは本か」を一度だけ判定する。
`spread.db` に明示値があれば本 / 非本のどちらでもその値を優先する。未保存の場合だけ、製本フォルダ、
閲覧履歴、ZIP / CBZ / PDF / 直接閲覧・変換アーカイブ、設定 ON の画像のみフォルダには Settings の
見開き・連結方式・読み方向の既定値を使い、それ以外の通常フォルダと合成ビューには
単ページ + ページ単位 + 左→右を使う。ネスト ZIP の本キーから root ZIP キーへの fallback と、
旧 `SpreadMode::Vertical` の読み替えは従来どおりである。

`resolve_spread_pair` は先に表示ユニット列を組み、表紙、横長ページ、末尾端数を単独ユニットにして、
横長ページの次の縦長ページから通常の見開きペアリングを再開する。
2 ページのペア化対象は `GridItem::Image` / `ZipImage` / `PdfPage` だけである。
`Video` / `Audio` はナビゲーション列に含まれる場合も必ず単独表示ユニットにし、静止画との混在でもペア化しない。
前後移動も同じ表示ユニット列を使い、`FsPageNav::Target(index)` または
`FsPageNav::Boundary` として解決する。見開き末尾から次へ進む入力を raw page delta に戻さないため、
内部 index が同じ見開きの片側へ移ることはなく、最初の入力で末尾ヒントを表示する。キー、ホイール、
画面クリック、ゲームパッド、スライドショーはこの契約を共有する。
Ctrl+←/→ の「1 ページずらし」は `spread_mode` を保存し直さず、セッション中の
`spread_shift_anchor_idx` で一時的なペアリング開始位置を持つ。アンカーは単純な移動先ページではなく、
移動先から前方向へ見て直近の横長ページ直後、または先頭まで戻した同じ偶奇の位置にする。
これにより、表紙ありの本で一度ずらした後に前ページへ戻っても、アンカー直前のページだけが
単独表示へ吸われず、先頭または横長ページ境界から一貫した見開き列として戻れる。
横長ページ自体は単独ユニットのまま境界として扱い、自動的な表紙扱いにはしない:

1. フィット方式を先に解決し、`Original` なら各ページの固有寸法、それ以外なら高いページへ
   揃えた geometry を選ぶ
2. 選んだ geometry と各ページの content bbox から見える合成サイズを計算する
3. `spread_page_gap_px` を左右ページの画面上の間隔として差し込み、フィットモードに応じた
   `fit_scale` とズーム後の `total_scale` を求める
4. ズーム/パンを `(fit_scale * fs_zoom, image_rect.center() + fs_pan)` として合成し、選択した
   geometry の content center から左右ページ矩形を配置する
5. ズーム/パンが有効なフレームでは `image_rect` にクリップして他の UI 領域へのはみ出しを防ぐ

ページ単位表示のカラー化遷移も、この `SpreadDisplayUnit` / `resolve_spread_pair` と同じ境界を
使う。表紙、末尾端数、横長ページは 1 ページ unit、通常見開きは画面順 2 ページ unit として
旧表示を保持する。1 ページずらしの `spread_shift_anchor_idx` と LTR/RTL の画面順も同じ resolver
から得るため、遷移状態だけが別のペアを作ることはない。

見開きのページ間隔は環境設定から変更でき、既定 4px、0px で左右ページを隙間なく接続する。
実配置では対象 viewport の物理 px へ gap を丸め、物理整数倍率では DPI 125% / 150% でも
各ページ原点を物理ピクセル境界へ着地させる。
`Original` では手動ズーム後も各ページの固有寸法比を保つため、元画像の高さが異なればノドに
段差が生じる。ページ全体 / 横幅 / 縦幅フィットでは倍率にかかわらず高さ合わせを維持するため、
ズーム中に片側だけが不連続に拡大・縮小しない。
見開き中も `rotation_db` の単独ページ回転 (R/L) は左右ページそれぞれに反映する。
`fs_free_rotation` (Ctrl+ドラッグのフリー回転) は見開きに反映しないため、Ctrl+ドラッグは
`handle_fs_wheel_and_click` 側で no-op にしている。
ズーム中または横幅/縦幅/原寸フィット中のパン (非修飾ドラッグ) と Ctrl+ホイールズーム、ダブルクリックリセットのみが見開きで有効。

連結読み (`draw_fs_continuous_reading`) は、`SpreadMode` のページ構成 (単ページ / 見開き)
を表示ユニットとして縦または横へ仮想配置する。巨大キャンバスは作らず、各ユニットの表示矩形を
スクロール位置から毎フレーム計算する。描画対象はマージン 0 の厳密可視ユニットだけに保ち、
final pipeline の処理対象には前後 1 ユニットの準備帯も加える。準備帯は描画にも可視ページ数上限の
判定にも含めない。GPU に保持するのは可視範囲と前後少数ページだけにする。
同時可視ページ数は最大 16 ページ程度、`fs_cache` の連結読み用 keep set は最大 20 ページ程度に
抑え、推定総テクセル数でも追加の安全弁をかける。推定にはrawだけでなく、erase、local-adjust、
conceal、edit、final composite、comic、補正の完全mip chainをTextureId重複排除つきで含め、
同じkeep setでevictionする。連結読みのページ/見開きユニット間隔は
`continuous_reading_gap_px` (既定 20px) を使い、見開きユニット内部の左右ページ間隔は
通常見開きと同じ `spread_page_gap_px` を使う。見開き構成の縦連結 + 横幅フィットでは、
表紙・横長ページ・端数の単独ページも仮想的な 2 ページ幅で fit scale を求め、実ページを中央寄せする。
これにより表紙だけ横幅いっぱいに拡大されず、後続の見開きページと同じ倍率で読める。
物理整数倍率では unit / page の累積位置を追加のたびに物理 px へ丸め、端数を次ページへ
持ち越さない。連結読みの見開きユニットも通常見開きと同じ geometry 選択を使い、`Original` では
倍率にかかわらずページ固有の高さを保って縦中央へ配置し、それ以外では常に高さ合わせを維持する。
表示トリムが有効なページは、各ページの見える bbox だけをユニット幅/高さとページ間 gap の基準にし、
描画時も同じ bbox を UV に使う。見開きユニットでは左右の見える端が `spread_page_gap_px` で並ぶ。
final composite / comic composite は厳密可視ページと準備帯を処理対象にし、キャッシュ済みの
最終テクスチャは厳密可視ページだけへ即描画する。未生成ページだけを 1 フレーム 1 枚ずつ final
pipeline に流し、現在ページがキャッシュ済みの場合は viewport 中心に近い未生成ページ、続いて
画面外の準備帯へ処理枠を回す。これによりスクロール中の大量 GPU アップロードを避けつつ、
次ユニットが可視になる前に final composite の生成を始める。連結表示自身が 16ms 後の再描画を
所有するのは、live な raw decode / upload backlog、解決済みページの次候補、または保存済み
編集マーカーを同期的に整理して状態が進んだ場合だけとする。`resolve_fs_processed_texture` の
`None` には raw 読込失敗や worker 起動失敗などの終端状態も含まれるため、`None` だけを根拠に
再描画を続けない。AI / final-effect / 各編集 worker は完了時の repaint をそれぞれの owner が
要求し、結果到着後に処理枠を再評価する。AI アップスケールのように
processed テクスチャは配置サイズへ反映せず、raw/source のアスペクト比を連結読みの
レイアウト基準として保つ。crop は通常表示では暗転 overlay だけなので、連結読みの
配置サイズや UV を変えない。
通常見開き・ページ送り・連結読みは同じ画像表示ユニット列を使うため、横長ページの前後で
単ページ扱いと一時的なずらしアンカーの扱いがずれない。
ただし source reload の遷移 texture は所有粒度を意図的に分ける。
`capture_final_effect_source_reload_holdover` は `ReadingFlow::Paged` の現在 unit 全体を保持し、
連結読みは従来どおり keep-set 内の `continuous_page_transitions[idx]` をページ別に使う。
通常の paged ページ移動は holdover ではなく移動先 rendition が所有する。スクロールで複数 unit が
同時可視になる連結読みへ単一の paged holdover を重ねないため、縦 / 横連結の描画挙動は変わらない。
横連結では `ReadingDirection` により
左→右 / 右→左の座標符号を反転する。UI で横方向を変更した場合は `SpreadMode` の
表紙あり/なしを保ったまま LTR/RTL も同期し、ページ単位の見開き方向と横連結方向が
食い違わないように保存する。ホイール 1 ノッチ、矢印キー/D-pad 1 回、左スティック最大入力の
スクロール量は、それぞれ画面幅/高さに対する割合として `continuous_reading_*_percent`
設定に保存する。PageUp/PageDown は従来どおり画面長の 85% で移動する。

連結読みの左ドラッグは、通常時は従来どおり連結方向の成分だけを
`fs_vertical_scroll` へ流す。Ctrl を押しているフレームでは軸固定を解除し、主軸成分は同じ
スクロールへ、直交成分だけは `fs_pan` (`Horizontal` は y、`Vertical` は x) へ加算する。
主軸側の `fs_pan` は変更せず、パン量の clamp も行わない。修飾状態はフレームごとの pointer
delta に適用するため、ドラッグ途中の Ctrl 押下 / 解放でも別の drag state へ移行しない。
ダブルクリックは既存の共通経路で `fs_zoom` / `fs_pan` / `fs_free_rotation` をリセットするが、
読書位置である `fs_vertical_scroll` は維持する。可視ページ数の安全弁が必要な場合はリセット後も
`fs_zoom` を安全な下限まで再び引き上げるが、`fs_pan` はゼロのまま中央へ戻る。連結方式から
ページ単位へ切り替える場合も既存の連結 transform リセットで `fs_pan` を消去する。

連結読み中も、フルスクリーン左パネルの「画像補正」「表示トリム」「ブックマーク」を
利用できる。編集対象はシークバーやページ番号表示が示す `fullscreen_idx` を基準に
`current_adjust_target_idx()` で解決するため、見開きでは選択中の左ページまたは右ページだけが
対象になる。左パネルを開いている間は、`FullscreenPageLayoutKind::Continuous` に記録された
対象ページの `DisplayedImageTransform::paint_rect` を枠線で示す。同じ transform は直交パン後の
配置から作るため、枠線もページと一緒に移動する。枠線は画像領域でクリップし、
ズームによってページの一部が画面外にある場合も追加の補正は行わない。枠線は暗い外縁と
高輝度の青い内線を重ね、白い紙面と暗い画像のどちらでも対象ページを判別できるようにする。

スクロールによる現在ページの再アンカーでは、対象変更前に未確定の表示トリムを保存するが、
左右パネルの開閉状態は維持する。表示トリムでユニット寸法が変わった場合は、古いスクロール
アニメーションを破棄して現在ユニットを表示基準位置へ再アンカーし、編集対象を画面内に保つ。
画像補正値と表示トリム、ブックマークは連結読みでも変更できる。一方、消しゴム、補正レイヤー、
隠蔽加工、切り取り、テキスト注釈、エクスポートはページ単位表示専用であり、連結表示中は
ヘッダーアイコンを無効化して「ページ単位表示でのみ使用できます」と案内する。対応する
ショートカットを押した場合も、同じ文言に機能名を付けた no-op 表示で理由を知らせる。

---

### 2.5 ページ送り中の表示規則 (実装の正本)

> **v2.13.0 では通過表示をいったん削除し、2026-08-12 にページ単位表示へ再実装した。**
> `page_turn_decision_for_inputs` / `FsPageTurnDecision` と `fs/page_turn_ready` /
> `fs/page_turn_decision` は現行コードに存在する。単一ページ / 見開きとも physical key level が
> held で実際の表示単位遷移が成立したバースト中は、全ページ一様に低解像度の色忠実 rendition を
> 使い、release 後または境界 no-op 後に完成画像へ戻す。
> `colorize_display_requires_final_effect` / `waiting_for_colorize`、またはページ送り判定を変更する前に、
> この節を読むこと。
> **この領域は繰り返し壊れており、壊れ方が毎回違う。**
> 2026-08-11 の 1 日だけで 3 回退行した (すべて「支配している規則を読まずに判定へ条件を
> 足した / 反転させた」もの)。

#### 2.5.1 満たすべき要件

| ID | 要件 | 出どころ |
| --- | --- | --- |
| **R1** | **ページ表示自体を飛ばさない。** キーを押しっぱなしで通り過ぎるページも、一瞬でも必ず 1 回は画面に出す | 利用者要件 (§1.58、2026-08-07) |
| **R2** | **白黒 → カラーの切り替わりを見せない。** カラー化や LUT が乗る絵で、色が付く前の状態を一瞬でも出さない | 利用者報告 2026-07-29 (「LUT 未適用の絵が 1 フレーム見えてから色が変わる」)。確定要件 |
| **R3** | **押しっぱなしで引っかからない。** 実際のページ遷移が続いている間、UI スレッドでフルサイズのアップロードや最終合成を行わない | 実測 (§1.58): 1 枚あたり upload 21ms + final_composite_build 21ms で、キーリピート間隔 34ms に構造的に追いつかない |
| **R4** | **キーを離したら完成画像で終わる。** 押し終わったページは通常の画質・加工で表示する | 同上 |

R1〜R4 は同時に満たす。**どれか 1 つのために他を崩す修正を入れない。**

##### 2.5.1.1 何のための機能か — 解像度は変わってよい、色は変わってはいけない

**利用者の意図 (2026-08-11 に明示)**: ページ送りは**音声のスクラブと同じ**もの。今どのあたりに
いるかを何となく見ながら高速に切り替えて、目的のページを探すためにある。

そこから 2 つの軸の扱いが決まる。**この区別が §2.5 全体の前提になる。**

| 軸 | 通過中の扱い | 理由 |
| --- | --- | --- |
| **解像度** | **変わってよい。サムネイル画質で構わない。** 止まったページでフルサイズに差し替われば良い。サムネイルキャッシュが無いページを、一旦サムネイル画質で出してから差し替えるのも可 | 通り過ぎる絵に精細さは要らない。探すのに必要なのは「何が写っているか」だけ |
| **色** | **変わってはいけない。** 通過表示にも完成画像と同じ色処理 (色調補正 / カラー化 / LUT) を乗せる | 見た目の差が大きく、ページごとに色が入れ替わると破綻して見える (R2) |

したがって **通過表示に必要なのは「フルサイズを速くする」ことではなく「色を合わせた低解像度を出す」こと**。
2026-08-11 の 5 回の失敗は、通過表示という発想が誤りだったのではなく、
**描画元をページごとに変わる条件で選び、しかも色を合わせていなかった**ことによる (§2.5.2.1)。

> 実測 (2026-08-11、`scripts/page-turn/measure.rhai` + 24MP JPEG fixture): 1 ページの
> UI スレッドコスト 28.2ms は**ほぼ全量がフルサイズのテクスチャアップロード**で、元画像の
> 画素数にほぼ比例する (11.8MP で 13.9ms)。表示は精々 1000×1500 程度なので、通過中に
> フルサイズを上げていること自体が費用の源。

#### 2.5.2 2 つの軸を混ぜない

判定は**独立した 2 つの問い**からなる。1 つの真偽値で兼ねてはいけない
(2026-08-11 の退行はこれが原因)。

| 軸 | 問い | 誰が読むか |
| --- | --- | --- |
| **A. 描画元** (`paint_source`) | このフレームで**何を描くか** — カタログサムネイルか、完成画像か | `prepare_fullscreen_state` |
| **B. 重い処理の保留** (`defer_ui_uploads`) | このフレームで**UI スレッドを使うか** — final-effect 結果の回収 / `fs_upload_batch` の消化 | `poll_final_effects` / `fs_upload_backlog` の消化 |

- **B は R3 の本体**。「ページ送りキーが held で、実際の表示単位遷移からなるバーストが active
  か」で決める。描画元の cache readiness を混ぜない。
- **A は B の結果ではない**。ただし現行のページ送り規則では、実遷移が成立した held バースト全体で
  A を `pass_through` に固定する。常駐している完成画像を貼る費用自体は安いが、
  **「完成画像が cache にあるか」はページごとに違うため、通過中の A をその条件で選んではいけない。**
  B は同時に `true` だが、A と B は別々の field / consumer のまま維持する。

#### 2.5.2.1 「ページ送り中か」は**フレーム間で安定した信号**でなければならない

**この節は 2026-08-11 に 4 回連続で退行させた原因そのもの。判定を触る前に必ず読むこと。**

通過表示に入るかどうかは「いまページ送り中か」で決まる。この信号が**フレームごとに変わると、
描画元も一緒に変わってちらつく**。

§1.58 の初期実装は、これを **「このフレームに未消費のページ送りキー edge が残っているか」**
で判定していた。キーリピートは約 30 回/秒、描画は 60fps なので、**おおむね 1 フレームおきに
true と false が入れ替わる**。その結果、同じページのまま描画元が毎フレーム往復する。

実測 (2026-08-11、`analyze_perf.py page-turn --check`):

```
I1 violation: burst t=45.222..45.987 idx=123
  final_composite -> thumbnail -> final_composite -> thumbnail -> ... (28 往復)
```

ページは動いていない。**入力信号が 30Hz で振動しているだけ**。

したがって判定には、**そのフレームに edge が来たか**ではなく、
**ページ送りキーが今押されているか** (`keymap::key_held_chord` /
`key_held_via_os` の OS 直読み) を使う。押下状態は物理状態そのものなので、
リピート周期に関係なくバーストの間 true のまま安定する。

ただし physical held は**必要条件であって十分条件ではない**。描画より後に解決される navigation
結果が実際に display unit を変えた時点で `FsPageTurnBurstState::Active` にし、その target idx と
items generation を所有させる。先頭で戻る、末尾で進むなど target が無い / current idx と同じ
入力はバーストを作らず、すでに active な hold が境界 no-op に達した場合はその場で
`Idle` に戻して着地点を materialize する。これは page / cache readiness ごとの描画条件ではなく、
**navigation が遷移を作ったかというコマンド結果**である。edge、時間閾値、ヒステリシスによる
平滑化は使わない。

§1.58 が edge 方式を選んだのは「時間閾値・押下開始時刻・前フレームからの pending 状態を
判定に使わない」ためだが、**押下状態の直読みはそのいずれでもない** (履歴を持たない現在値)。
制約の意図を保ったまま、信号だけを安定させられる。

**禁止**: この不安定さを、時間閾値・ヒステリシス・「前フレームの決定を覚えておく」で
埋めないこと。原因は信号の選び方であって、平滑化で隠す対象ではない。

#### 2.5.3 サムネイルを代役に使ってよい条件

通過表示は「カタログサムネイルから作る低解像度 rendition をそのページの代役にする」最適化で
ある。rendition がまだ得られなくても通過モード自体をページ別に解除せず、直前の表示単位を保った
まま得られる画質で差し替える。加工の扱いは**見た目の差**と**処理時間**の 2 つで決める。

| 加工 | サムネイルとの見た目の差 | フルサイズの実測 | 通過表示の扱い |
| --- | --- | --- | --- |
| 色調補正 (明るさ / コントラスト / 彩度など) | 色が変わる | `adjust_ms` **0.00ms** | **サムネイルに適用して使う** |
| Creative LUT | 色が変わる | `creative_lut_ms` **0.00ms** | **サムネイルに適用して使う** |
| カラー化 | **白黒 → カラー**。差が最も大きい | 下表のとおり寸法依存。347×506 で **1.67ms** | **サムネイルに適用して使う** (実測済み、下記) |
| 消しゴム / 隠蔽 / テキスト注釈 | 内容が変わる | 最悪 **1 秒以上** かかりうる | **サムネイルを使う (加工なしで通過してよい)**。待つと R1 / R3 を壊すため、意図的にスキップする |

**消しゴム・隠蔽・注釈をスキップする理由は「色ではないから」ではなく「時間がかかりすぎるから」。**
将来この 3 つが十分速くなったら、扱いを再判断する余地がある。

**サムネイルキャッシュが無いページの扱い**: 通過表示に入れないのではなく、**出せる最低画質で
一旦出してから差し替えてよい** (§2.5.1.1)。「そのページのサムネイルがあるか」で描画元を選ぶのは
§2.5.2.1 が禁じている「ページごとに変わる条件」そのものであり、5 回の失敗の 1 つ目の形。

##### 2.5.3.1 寸法別の色処理コスト (2026-08-11 実測)

`cargo test --release -p mimageviewer --lib colorize::tests::thumbnail_effect_cost_measurement
-- --ignored --nocapture` (24 スレッド、`legacy4color` / `luminance100` / `gaussian` /
`radius1.0` / `strength100` + `contrast:+20`)。

| 寸法 | 画素数 | 色調補正 中央値 | カラー化 中央値 | 最大 | うちトーンぼかし |
| --- | --- | --- | --- | --- | --- |
| **347×506** | 0.18MP | **0.25ms** | **1.67ms** | 4.75ms | 1.25ms |
| 800×1200 | 0.96MP | 1.22ms | 5.76ms | 7.12ms | 4.67ms |
| 1123×1648 | 1.85MP | 2.40ms | 9.97ms | 10.34ms | 8.29ms |
| 2480×3508 | 8.70MP | 11.74ms | 41.24ms | 47.62ms | 32.49ms |

**§2.5.6 が警告していた固定費は実在した。** 面積比だけなら 347×506 は 1123×1648 の 0.095 倍
なので 0.95ms のはずだが、実測は 1.67ms (0.167 倍)。差はトーン階調化のぼかしで、
サムネイル寸法でもコストの 4 分の 3 を占める。

それでも **サムネイル 1 枚あたり 色調補正 + カラー化 ≒ 2ms** で、キーリピート間隔 34ms に対して
十分小さい。**したがって通過表示の色処理は UI スレッドで同期的に行ってよい。** 非同期にすると
「色が後から付く」状態が生まれ、それは R2 の失敗そのものである。同期実行なら**同じフレームで
色が揃うことが構造的に保証される**。

`colorize_display_requires_final_effect(idx)` ([ui_fullscreen.rs](../src/ui_fullscreen.rs)) は、
materialized 経路で raw / edit fallback を見せず final-effect を待つための保守的な gate である。
**この gate の true を、通過 rendition にカラー化を適用する意味へ読み替えてはいけない。**
`MonochromeOnly` で full-size summary が未到着、または色調補正が classifier 入力を変える場合、
gate は「未確定なので待つ」という意味で true を返すためである。

通過 rendition の `MonochromeOnly` 適用可否は、(1) 同じページ / 編集世代 / params の常駐する
完成 composite が保持する実 worker 判定、(2) 色調補正が identity で同じ `EditResultKey` の
full-size summary、(3) 色調補正後の低解像度画素に対する既存の `colorize::should_apply`、の順に
1 回だけ解決し、`colorize_applied` を rendition cache entry に保持する。着地点の final-effect
worker は同じ entry の判定を再利用する。既存 final があるページでは rendition が final に合わせ、
final が無いページでは final が rendition に合わせるため、thumbnail と full-size の classifier が
食い違っても settle で色が変わらない。raw decode 到着だけで進む source generation はまたいでよいが、
編集世代 / params が違う entry は再利用しない。この判定は pass-through / materialized の mode
選択には使わない。

#### 2.5.3.2 実測: 軽い本では見開きだけに露出した (2026-08-11〜12)

**利用者が引っかかりを確認したフォルダ** (`aimodel1`、306 枚、**1.0〜1.6MP**) を、同じフォルダ・
同じ設定で単一ページと見開き (RTL) の両方で計測した。この軽い本では解像度は支配項でなかった。

| モード | ナビ命令 | 実際に描かれた枚数 | 画面滞留時間 | 逆行 | 再訪 |
| --- | --- | --- | --- | --- | --- |
| **単一ページ** | 144 | **144** (1:1) | 中央値 34.7ms | 0 | 0 |
| **見開き (RTL)** | 142 | **23** (6:1) | 中央値 35ms / **最大 2267ms** | 0 | 0 |

見開きでは 142 回のページ送り命令に対して **23 枚しか描かれない**。描かれた idx は
`14 → 57 → 70,71,72,73 → 80,…,94 → 229 → 234,235,238` と飛び、**1 枚が 746ms / 2267ms
画面に留まる**。これが利用者報告の「たまに一瞬同じ画像が止まる」の実体。

**ナビゲーション自体は正しい。** `input/fs_key` は `Target(2), Target(4), … Target(14),
Target(15), Target(17), …` と単調増加で、逆行は 0 件。つまり**入力も遷移も壊れておらず、
描画が単位ごと落ちている**。画面には前の表示単位が残り続けるので、止まって見えたあと飛ぶ。

> 「ページ数が巻き戻るようにちらつく」については、**逆行そのものは観測できていない**。
> 止まって飛ぶ挙動が巻き戻りとして知覚されている可能性が高いが、未確認。

単一ページはこの条件では 1 命令 1 描画を維持し、すべて `final_composite` で追いついていた。
一方、見開きの主症状はスループットではなく、表示単位が final 完成まで旧 unit に覆われることによる
描画欠落だった (§2.5.4)。どちらも R1 は同じであり、単一が常に安全という意味ではない。

その後、4.1MP の実 ZIP (`原神.zip`) を単一ページで測ると、materialized 経路は **174 命令に
対して 29 表示 (約 1:6)** しか出なかった。`aimodel1` の 1:1 は完成処理が 34ms の repeat に
たまたま追いついていただけで、高解像度では単一ページにも同じ R1 違反が露出する。この実測を受け、
利用者判断で単一ページも見開きと同じ pass-through 規則へ統一した。

計測メモ: **RTL 見開きでは Right が「戻る」方向**。前進は Left。RTL で Right を押し続けると
`page_nav=Boundary { at_end: false }` が 144 回並ぶだけになる (実際にそうなった)。

##### 方向による差 (2026-08-11、利用者の「戻りの方が引っかかる気がする」を検証)

同一 run で 3 回のホールドを取った。**集計値は方向でほぼ差が無い。**

| ホールド | ナビ命令 | 描画枚数 | 滞留 中央値 | 最大 |
| --- | --- | --- | --- | --- |
| 前進 | 142 | 23 | 35ms | 2266ms |
| 戻り (直前に通ったページ) | 143 | 23 | 36ms | 2284ms |
| 戻り (末尾から) | 143 | 22 | 36ms | 2280ms |

ただし**最初の無反応時間は戻りの方が長い**。末尾からの戻りは idx 303 から始まり、
**最初の 1.29 秒は 1 枚も描かれず**、238 に達して初めて描き始めた (前進の初回停止は 746ms)。

**そして 2 周目以降に大量の再デコードが起きる。**

| ホールド | decode 開始 | 対象ページ | **既に composite 済みなのに再デコード** | 同一ホールド内で 2 回以上 |
| --- | --- | --- | --- | --- |
| 前進 (初回) | 261 | 261 | 0 | 0 |
| 戻り (末尾から) | 353 | 264 | **228** | 44 |

初回前進は 261 ページをデコードして 23 枚しか出さない。末尾からの戻りは 264 ページ中 **228 ページが
「以前 final composite を作ったのに再デコードされている」**。これが backlog の対策 2
(再デコードの抑止) の実データ。

**方向そのものの影響か、単に 2 周目だからかは切り分けていない** (前進の 2 周目を測っていない)。
利用者が戻りで引っかかりを強く感じるのは、実使用では前進で読み進めた後に戻るため、
この再デコード経路に常に入ることで説明が付く可能性がある。

**フレーム自体は詰まっていない。** 5 秒のホールド中に 588〜610 フレーム描かれており
(間隔中央値 6〜7ms)、その中で表示画像が変わったのは 22〜23 回だけ。
つまり**描画が遅いのではなく、表示単位が更新されない**。

##### 根本原因と 2026-08-12 の修正

当時の source inspection で、見開きの表示欠落は `open_fullscreen` が前の
`ColorizeDisplayUnitHoldover` を capture し、`colorize_display_unit_holdover_for_draw` が対象見開きの
全ページで final texture が解決するまでその旧 unit を通常描画の上へ重ね続ける経路と確定した。
ナビゲーション先の frame は描かれていたが、旧 unit の overlay に隠されていたため、final が同時に
揃った表示単位しか画面上で観測されなかった。

再デコードは key mismatch ではない。ページ送りのたびに `open_fullscreen` が
`ensure_fs_page_load` と `update_prefetch_window` を起動し、通過ページまで full decode / final pipeline
へ投入していた一方、`fs_cache` / `final_composite_cache` の小さい keep window が先に通ったページを
退避した。2 周目は正しい同じ key であっても resident entry が既に無く、もう一度 source decode へ
入っていた。

修正後は、単一ページ / 見開きで physical key level が held かつ実遷移バーストが active の間、
次を一体で行う。

- 表示単位の全ページを、色調補正 → カラー化 → Creative LUT 済みの低解像度 rendition として
  同じ frame で原子的に解決する。ready 状態は通過モードへ入る条件に使わない。
- `open_fullscreen` は通過先の full decode / prefetch を開始せず、進行中の context-owned
  decode / final effect / AI producer を cancel する。resident cache と完成済み upload backlog は保持する。
- release frame は current display unit だけを通常の eager materialization へ戻し、その後の通常
  prefetch を再開する。
- 通過終了後は着地点の色忠実 rendition を final 到着まで維持する。通常ページ移動は旧
  `ColorizeDisplayUnitHoldover` を作らず、同一 unit の source reload だけが typed holdover を使う。

最初の見開き修正 (96faeee6) は利用者実測で、**267 / 267 ページを順番どおり表示** (間隔中央値
30ms)、hold 中の `decode_begin` **0 / 0 / 0**、release 後 **29〜56ms** で左右同時に完成画像へ
settle、`page-turn --check` は **checked bursts=3 / violations=0** を確認した。

単一ページ側の source inspection では、`fs_page_turn_ordinary_context_blocker` の
`single_page_materialized` が pass-through を明示的に除外し、`open_fullscreen` の eager
materialization へ流していた。4.1MP ZIP では次の final texture が間に合わず旧ページが残るため、
174 命令のうち完成した 29 ページしか `fs/paint` に現れなかった。この blocker を除き、active
burst 中は resident final の有無を見ず rendition だけを選ぶようにした。4800f2cd の利用者実測では、
単一 `aimodel1` が 144 命令 / 145 表示、単一 4.1MP ZIP が 174 命令で全 30 ページ表示、見開きが
144 命令 / 267 ページ表示で、hold 中 decode は全ケース 0〜1、I1〜I5 は violations=0 だった。

2026-08-12 の追加 source inspection で、残る 3 件の経路を次のように確定した。

- **境界での画質低下**: `render_fullscreen_viewport` は navigation 解決より前に physical held だけで
  pass-through を選び、その frame の後段で `adjacent_navigable_idx` が `None` /
  `FsPageNav::Boundary` を返していた。idx が変わらなくても rendition を描く順序だった。navigation
  結果を burst owner へ publish し、実遷移だけを active、boundary no-op を idle とするよう直した。
- **縦横比の差**: 単一の `draw_fs_image` は rendition texture の `size_vec2()`、見開きの layout も
  左右 texture の整数丸め後寸法を fit に使っていた。`ThumbnailState::Loaded::source_dims` は既に
  保持していたが、この経路が参照していなかった。通過中の layout は source/header 寸法で解決し、
  texture 寸法は sampling と upload 判定だけに使う。
- **MonochromeOnly の誤カラー化**: `ensure_passthrough_rendition` が
  `colorize_display_requires_final_effect` の保守的な「待つ」結果を
  `final_color_effect_required` として builder へ渡し、builder が true を「カラー化を適用する」と
  解釈していた。full summary 未到着のカラーページほど強制カラー化される意味の反転だった。
  常駐 final の実判定、既存 full-size summary、rendition 自身の adjusted pixels の順に 1 つの判定を
  解決し、landing final と共有するよう直した。

#### 2.5.3.3 実測: 通過表示の上限は約 100 ページ / 秒 (2026-08-12)

外部からの「mIV は 125Hz リピートで 50〜52fps しか出ない」という指摘を受け、4K 165Hz の
foreground で 500 ページの FHD ZIP (1200×1700 = 2.04MP、STORED、335MB) を各リピートレート
1 ホールドずつ、毎回新しいプロファイルで測った。合成入力は `scripts/page-turn/measure.rhai`。

| リピート | 入力 / 秒 | **表示ページ / 秒** | **fps** | フレーム間隔 中央値 | p99 | 最大 |
| --- | --- | --- | --- | --- | --- | --- |
| 30Hz | 29 | **29** (1:1) | **165.2** | 6.0ms | 8.0ms | 9ms |
| 125Hz | 119 | **100** | **164.1** | 6.0ms | 8.0ms | 18ms |
| 165Hz | 156 | **100** | **164.4** | 6.0ms | 8.0ms | 15ms |

**フレームレートは 3 条件とも表示リフレッシュに張り付いている。** 「50fps 止まり」は
この構成では再現せず、修正前の描画欠落 (§2.5.3.2 の 142 命令 / 23 表示) を fps として
観測したものと考えるのが自然。

**表示ページレートは約 100 / 秒で頭打ちになる。** 律速は通過表示の素材づくりで、165Hz
ホールド中の内訳は次のとおり:

| イベント | 件数 | 中央値 | 合計 |
| --- | --- | --- | --- |
| `thumb/decode_end` | 460 | **32.8ms** | 14356ms |
| `details_meta/load_done` | 487 | 1.69ms | 796ms |
| `fs/decode_end` | 5 | 26.97ms | 129ms |

5 秒のホールド中に 460 枚のサムネイルをワーカー並列で生成しており、460 / 5 秒 ≒ 92/秒が
表示ページレートとほぼ一致する。**カタログキャッシュが無い本でも通過表示は成立する**
(その場で生成される) が、生成スループットが上限を決める。

`fs/paint` の実測 geometry は `w=1016.83, h=1440.0, texture_w=334, texture_h=473,
scale_x=3.0443971, scale_y=3.0443973` で、論理 1440 × ppp1.5 = 実 2160px の 4K 描画、かつ
`scale_x` と `scale_y` が一致する (e7191ad2 の縦横比保持が効いている)。

**出荷時の言い方**: 「連続ページ送り中はサムネイル画質へ落として高速に切り替える。
その状態でおよそ 100 ページ / 秒」。フル画質のまま同じレートを出せるかは未測定
(ホールド中は full decode / upload を意図的に抑止しているため、別構成での測定が要る)。

#### 2.5.4 表示単位 (単一 / 見開き / 連結)

単一ページは 1 ページ、見開きは左右 2 ページを表示単位として、**表示単位全体を 1 回で解決する**。
physical key level が held で実遷移バーストが active なら、その burst 内の全 unit が通過モードであり、
resident final や rendition の有無でページごとに mode を反転させない。見開きは左右 rendition が
揃った frame だけを原子的に描き、
片側だけ完成画像、もう片側だけ rendition という組み合わせを描かない。rendition がまだ無ければ
未解決 unit として直前表示を保ち、後続 frame の rendition へ差し替える。

通過 rendition の外側の**ページ矩形**は、低解像度 texture の整数寸法ではなく source/header 寸法
から求める。PDF は thumbnail / fullscreen の各 raster 寸法が別々に整数丸めされるため、PDFium が
読む page box を `layout_width/layout_height` に 1/1000 point の固定小数点レイアウト寸法として
保存し、通過 rendition と完成 texture のページ矩形を同じ位置・大きさに固定する。一方、
`source_width/source_height` は PDF を含め常に raster のピクセル寸法であり、注釈・クリック判定・
編集座標等に使う。両者は別の軸で、相互の fallback として単位を混ぜない。

縦連続表示も同じ PDF レイアウト寸法を `vertical_reading_base_size` の最優先 source とする。
2026-08-12 の実ログでは page 0 の final raster が `4540×6920` へ切り替わった frame に、直前の
cached thumbnail `226×422` の比 (`0.5355`) が連続表示だけに残り、ページ矩形が
`561.1×1048` (`0.5354`) になっていた。page box は `468.600×714.360 pt` (`0.6560`) であり、
同じ final raster の比 (`0.6561`) と一致する。原因は paged path の canonical layout owner を
連続表示が迂回し、かつ `Evicted` で PDF layout metadata を失えたことだった。現在は同一の
`layout_dims` を paged / continuous の両方へ供給し、generation-scoped cache が texture eviction
後も保持する。

ただし低解像度 raster 自体をそのページ矩形へ非等方に引き伸ばしてはならない。rendition の実描画
矩形は texture 自身の縦横比を保つ contain fit とし、ページ矩形の中央へ置く。完成 texture だけは
従来どおりページ矩形全体へ描く。整数丸め差に相当する最大数 pixel の余白は許容し、低解像度の
中身を伸縮して settle 時に位置が動くことを避ける。単一 / 見開きとも同じ規則を使い、見開きの
source 選択と差し替えは引き続き表示単位で原子的に行う。

縦・横の連結読みは通過表示の対象外 (スクロールは実体化済みのページを見せるだけなので、
キーごとの実体化コストが元から出ない) だが、ページ矩形の canonical layout 契約は共通である。

#### 2.5.5 トレース不変条件 (機械判定)

現行実装は上の規則を `fs/page_turn_ready` / `fs/page_turn_decision` の並びとして計装し、
`python scripts/analyze_perf.py <jsonl> page-turn --check` で検査する。event が無いログを成功扱い
しないため、本番計測では `checked bursts>0` も完了条件に含める。
**規則を変えたらこの不変条件も同時に更新する。**

| ID | 不変条件 | 対応する要件 |
| --- | --- | --- |
| I1 | 1 バースト内で、同じ idx の `source` が `final_composite` → `thumbnail` へ戻らない | R2 |
| I2 | 1 バースト内で `source` がページをまたいで混ざらない | R2 |
| I3 | バーストの最初と最後の間のすべての idx が 1 回以上出る | R1 |
| I4 | key-up または境界 no-op 後 1.5 秒以内に、着地点 idx の `materialized` が現れる | R4 |
| I5 | active な通過バーストで rendition が ready の間、`defer_ui_uploads=true` が維持される | R3 |

`fs/paint` は単一ページでは既存の `(idx, texture)` producer が rendition も記録し、見開きでは
holdover を含む最終表示単位の解決後に同じ page list から記録する。`source=thumbnail` が通過
rendition、`source=final_composite` が完成画像である。前表示単位と同じ `(idx, texture)` は重複して
出さない。event の `x` / `y` / `w` / `h` は実際に texture を描いた矩形、`texture_w` /
`texture_h` は回転後 texture 寸法、`scale_x` / `scale_y` はその描画倍率である。これにより
`fs/paint` の表示ページ数と `fs/page_turn_ready` を直接照合でき、同じ idx の rendition → final で
非等方伸縮が混入していないかもログから確認できる。

開始時から境界で、1 回も display unit が変わらない hold は ready signature も変えないため、
その押下に対応する `fs/page_turn_ready` を 1 件も出さない。

endpoint 例外は設けない。active burst が端へ達して次の repeat が no-op になった時点で、physical
key がまだ held でも burst を閉じて着地点を materialize する。したがって I4 は端でも必ず要求する。
通常の移動中 burst の settle 窓は従来どおり key-up にアンカーし、burst 内の最後には要求しない。

2026-08-12 の実機報告にあった「戻り hold の端で黒背景 + 中央の白文字」は、
`ColorizeDisplayUnit` の「カラー化中…」ではなかった。active burst の最後は rendition を描いた後、
navigation 解決で `Idle` になる。次 frame の `prepare_fullscreen_state` は materialized を選ぶが、
着地点の final composite はまだ無いため `resolve_fs_processed_texture=None` となり、R2 のため未処理
thumbnail も抑止する。残る `draw_fs_image` の終端が黒背景 + 中央「読込中...」だった。
pass-through open は旧 page-transition holdover を消しているので、この frame に
`ColorizeDisplayUnit` と「カラー化中…」は存在しない。

修正後は materialized settle も同じ着地点 rendition を final 到着まで使う。見開きは片側だけ final に
せず、片側でも final-effect 待ちなら両ページとも rendition、両 final が揃った frame で原子的に
差し替える。開始時から端の no-op は既存 final が解決済みなので表示 source を一切変えない。
通常ページ移動は旧ページ holdover を作らず、「カラー化中…」は同一ページの PDF 再レンダ等で
`FinalEffectSourceReload` が 400ms を超えた場合だけ表示する。その表示開始は
`fs/colorize_wait_indicator` (`reason=source_reload`) に一度だけ記録する。

#### 2.5.6 やり直しの作業順 (2026-08-11 に更新)

§2.5.1.1 で意図が確定したので、次版は「色を合わせた低解像度を出す」ことに集約する。

1. ~~サムネイル寸法での色処理コストを実測する~~ → **実測済み (§2.5.3.1)。約 2ms / 枚で、
   UI スレッド同期実行が可能。**
2. ~~通過表示に色処理を適用する~~ → 色調補正 → カラー化 → LUT を UI スレッドで同期実行する
   context-local rendition cache を実装済み。
3. ~~通過中はフルサイズのアップロードとデコードを始めない~~ → 実遷移バーストが active の間は
   producer 起動と UI upload 回収を保留し、既存 producer を cancel する。
4. ~~止まったページだけ実体化する~~ → release または境界 no-op frame で current display unit を
   eager path へ戻し、final 到着までは着地点 rendition を維持する。

単一ページの `aimodel1` / 4.1MP 実 ZIP と見開きの実ログ gate は利用者実測で合格済み。境界 no-op、
rendition の表示矩形、PDF + `MonochromeOnly` は上記の回帰テストと再実測で維持する。

##### ボトルネックは 1 つではない (2026-08-11 実測)

計測対象によって支配項が違う。**どちらか一方だけ直しても足りない。**

| 対象 | ページの画素数 | 合成コスト | ページ間隔 | 追いつくか |
| --- | --- | --- | --- | --- |
| 実物 ZIP (1792×2304 PNG、664 枚) | 4.1MP | 4.7ms | **33.5ms** | materialized 単一は ❌ **29 / 174** |
| 合成 fixture JPEG | 11.8MP | 13.9ms | **35ms** | ✅ 追いつく |
| 合成 fixture JPEG | 24.0MP | 28.2ms | **67ms** | ❌ 追いつかない |
| 実物の本 (`-Setup`、カラー化 + AI 有効) | 小さい | — | — | デコード 72.9ms + AI タイル 57.5ms×44 が支配。upload は **1.38ms** |

合成 fixture 単体のコストは画素数にほぼ線形だったが、実アプリでは decode / upload / final-effect
回収を含むため、4.1MP ZIP でも materialized 経路は追いつかなかった。したがって合成 fixture の
11.8〜24MP だけから実用上の閾値を決めてはいけない。

実物の本では upload は問題ですらなかった (ページが小さいため)。合成 fixture では upload が
支配的だった。**共通して効くのは「通過するページでフル解像度の作業を始めない」こと**であって、
upload だけを速くすることではない。

**AI アップスケールは通過中に走らせない** (利用者判断 2026-08-11)。止まったページで推論して
遅れて差し替えてよい。57.5ms/タイルは通過表示の予算に対して桁違いで、通り過ぎる絵に拡大結果は
要らない。

**カラー化 Disabled + 色調補正のみ**のページは、`colorize_display_requires_final_effect`
が `false` を返す (`is_color_identity` を見ているのが `MonochromeOnly` の枝の中だけのため)。
上の 2 を入れれば自然に解消する。

なお **デコードも律速**である。24MP JPEG の実測でデコードは 172ms/ページで、ページ間隔 67ms は
ワーカー並列でようやく成立している。UI スレッドの 28ms を消しても 34ms まで落ちるとは限らない。
通過するページのデコードを始めないこと (上の 3) が効くはず。

---

## 3. 補正・AI・カラー化・ポストフィルタキャッシュと再描画

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
   raw thumbnail 自体は書き換えないが、永続 edit preview 経路では補正レイヤーを含む
   編集結果を一覧へ表示できる。
   ↓
4. 隠蔽加工 (モザイク / 白塗り / 黒塗り / ぼかし)
   → conceal_cache[idx, generation] (= local_adjust_cache / erase_result_cache / raw をベースに合成)
   ↓
5. edit_result_cache[EditResultKey]
   ここまでが source 解像度の edit pipeline。crop / AdjustParams / AI / post_filter は含めない。
   ↓
6. 色補正 (色温度・彩度・コントラスト・露出など)
   → apply_adjustments_fast(edit_result)
   ↓
7. AI アップスケール / デノイズ (Real-ESRGAN / Real-CUGAN / NMKD-Siax / 1x denoise)
   → final_ai_cache[FinalAiKey] (pixels + used_upscale)。未完了中は色補正後の画像を暫定表示する。
   ↓
8. スマートシャープ (シャープ化スライダー 0..=100、輪郭中心の最終段シャープ)
   → AI アップスケール出力には掛からない (固定動作)。デノイズのみ / AI なしには掛かる。
     詳細は preset-and-adjustment.md §2.7
   ↓
9. カラー化
   → 近モノクロ判定、対象画像だけの濃度・コントラスト自動補正、必要なら
     スクリーントーン濃淡変換、カスタム階調 LUT
   → `final_effect_pending` worker。カラー化前の provisional texture は公開しない。
     前後ページは final composite まで先読みし、完成済みならページ移動直後からカラー表示する。
     先読みが未完了なら、ページ単位表示では移動先の色忠実な低解像度 rendition を表示し、
     対象 unit の全ページが揃った frame で完成画像へ一括で切り替える。
     PDF の Z ズーム再描画など、同じ表示ページの source 解像度が更新される場合は、旧世代の
     完成済み texture を display-only holdover に退避してから live cache を無効化し、
     新世代の final composite 完成時に置き換える。AI 待ちの incomplete composite は
     final AI 到着後も同一 key の cache entry として維持し、AI 後の完成結果で直接上書きする。
     `complete=true` の final だけが source-reload holdover / nav lock を解放する。
     `FinalEffectSourceReload` が 400ms 続いた場合だけ、中央の暗いラウンド矩形に
     「カラー化中…」を表示する。通常ページ送りとフォルダ移動の `FolderNavigation` では表示しない。
   ↓
10. Creative 3D LUT
   → 環境設定で登録した `.cube` を適用量付きで最終表示へ適用する。
     入力色空間変換ではなく表示用ルック専用。カラー化と同じ `final_effect_pending`
     worker で処理し、サムネイルには反映しない。
   ↓
11. ポストフィルタ (CRT エミュレート / 減色 / モノクロ / 複合エフェクト)
   → final_composite_cache[FinalCompositeKey]
   ↓
12. capture / export 時だけ crop で実切り出し
```

**ユーザー向けの言い換え**: 消しゴム / 補正レイヤー / モザイク加工は元画像の
解像度で先に確定し、その後に明るさ・色・AI 拡大・効果フィルタが乗る。crop は
通常表示では範囲外を暗転するだけで、capture / export の最終段で初めて切り出す。
そのためアップスケール ON/OFF や補正スライダー変更で編集マスクの解像度は変わらない。

製本追加は表示 cache を読まず、UI thread で固定した `BakedEditSnapshot` を book worker の
headless compositor が復元する。順序は raw → erase → local_adjust → conceal → adjustment →
comic → rotation → export crop。表示専用の global AI upscale / denoise / smart sharpen /
colorize / Creative LUT / post-filter は意図的に飛ばすため、grid / fullscreen / stack の
どこから追加しても原寸の edit composite になる。編集が 1 つも無い File / ZIP entry は
この経路へ入れず byte copy を維持する。

### 3.1 詳細

詳細は [preset-and-adjustment.md](preset-and-adjustment.md) に譲る。ここでは要点のみ:

- **補正 (adjustment)**: `ensure_final_composite_texture` が `edit_result_cache` のピクセルへ
  `apply_adjustments_fast` を適用する。色系パラメータ変更では `edit_result_cache` を保持し、
  `final_ai_cache` / `final_composite_cache` だけを落とす。
- **スマートシャープ (シャープ化)**: final AI の後・カラー化の前に
  `apply_final_smart_sharpen` を適用する。AI アップスケール出力には掛からない
  (固定動作)。サムネイルには反映しない。詳細は
  [preset-and-adjustment.md §2.7](preset-and-adjustment.md)。
- **カラー化**: スマートシャープ後・ポストフィルタ前に適用する。カスタム色、近モノクロ判定、
  対象画像だけの着色前自動レベル補正、スクリーントーン濃淡変換を含む。重い画素処理は
  viewer context 所有の worker へ送り、
  stale 結果を `FinalCompositeKey` と items 世代で拒否する。AI 先読みと同じ前後枚数の
  `final_composite_cache` を背景で 1 枚ずつ作る。先読み中は provisional texture を upload
  せず、同ページが表示対象になったときは進行中 job を昇格して再利用する。通常ページ送りでは
  完成まで移動先の色忠実 rendition を保持し、生画像・未処理サムネイルへフォールバックしない。
  同一ページの source reload holdover が 400ms を超えた場合だけ「カラー化中…」を表示する。
  `open_fullscreen` はページ入場だけでは final composite / worker を無効化しない。カタログの
  サムネイル自体は書き換えない。連結読みは `FinalEffectSourceReload` を通らないため、
  このインジケータの対象外とする。
- **Creative LUT**: カラー化後・ポストフィルタ前に適用する。登録済み `.cube` の parse 済み
  table を worker へ `Arc` で渡し、ファイル I/O は UI thread と final-effect worker の外で行う。
  LUT の選択・適用量は final composite key に含め、変更時も edit / final AI cache は保持する。
- **ポストフィルタ**: AI の後段で CPU 処理 (CRT/減色/複合)。rayon 並列化済み。
  `PostFilter::Nearest` のみ NEAREST サンプラー、それ以外は LINEAR でアップロードする。
- **消しゴム/隠蔽加工/分析モード中の一時バイパス**: `App::post_filter_bypassed = true` の間は
  final composite の key からカラー化・Creative LUT・post-filter を外し、表示用最終段だけを切り替える。
  edit 系 generation は進めない。
- **補正レイヤー**: `local-adjust-render` worker で `local-adjust-core` を適用し、
  `local_adjust_cache` に載せる。生成中は古い補正レイヤー結果を使わず、
  `erase_result_cache > fs_cache` の下位画像を表示する。
  Ctrl+E / キャプチャ保存では、補正レイヤーが有効なページは `local_adjust_cache` 完了後だけ
  出力対象にする。
  ブラシ stroke 中は 150ms の idle まで重い再合成を遅延し、release 時に確定世代を進める。
  `Repair` の周囲パッチ探索 / クローン / 色とテクスチャのなじみ処理もこの worker で行い、
  決定した結果ピクセルは専用レイヤーに保持せず、パラメータとマスクから再生成する。Repair の
  マスク境界なじませは、ぼかす前のマスクで修復元探索 / テクスチャ生成を行い、ぼかしたマスクは
  最終合成 alpha にだけ使うため、なじませ幅を変えても参照パッチ自体は変化しない。
- **AI アップスケール/デノイズ**: final pipeline の別スレッドで推論。完了時に
  `final_ai_cache` に pixels と `used_upscale` を格納し、未完了の
  `final_composite_cache` を表示用に残したまま同一 key で再合成する。final-effect worker
  完了時の `insert` が未完了 entry を上書きするため、再合成中も直前のカラー化済み表示が
  継続する。
- **共有 final composite 層**: tone / smart sharpen / colorize / Creative LUT / post_filter の
  CPU 実行順は `final_composite::FinalCompositePlan` と `execute_final_composite` が App / remote
  で共有する。final AI は tone 後・smart sharpen 前の独立した非同期 cache 層なので plan には
  含めず、adapter が AI 前後の source と `used_upscale` 解決を所有する。raw / edit result の
  source 選択も materialize と世代管理を持つ adapter の責務で、共有 executor は選択済み pixels
  だけを受け取る。
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
  から `final_composite_cache` を直接復元する。`Raster` lookup は固定 4096 ではなく、現在の
  display target と保持 raster の縦横比から必要長辺を再計算し、±10% 内だけを再利用する。
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
  対象 idx を `ai_upscale_prefetch_forward / back` 件まで取得する。同じ更新入口から
  カラー化の final composite 先読みも行い、AI 使用ページは `final_ai_cache` 完成後、
  AI 不使用ページは edit pixels 準備後に 1 件ずつ final-effect worker へ送る。
  ⚠️ **退行注意**: Pipeline P1 リファクタ (be05cfef) で旧 `prefetch_ai_upscale` が
  dead code 化され、新版が未実装のまま 1 リリース過ごした。`App::update` の
  「フルスクリーン work セクション」(= `// AI 先読み (新パイプライン)` コメント) を
  消すと再発するため、リファクタ時は呼び出し元の存在を要確認。
- **元画像プレビュー**: 割り当てた ModifierHold (既定は右 Ctrl) の間だけ描画時のテクスチャ選択を
  raw 専用の `fs_cache` に切り替える。DB・補正設定・AI queue は変更しない。
- **何かを変えたら正しいキャッシュをクリア**:
  - 色調パラメータ / AI 変更 → `final_ai_cache` / `final_composite_cache` をクリア
    (`edit_result_cache` は保持)。post_filter / シャープ化**のみ**の変更は
    `final_composite_cache` だけクリアして final AI を保持する
    (preset-and-adjustment.md §4)
  - 消しゴム / 補正レイヤー / 隠蔽加工変更 → source 解像度の edit cache と final cache をクリア。
    特に全ページ共通の隠蔽パラメータ変更は、edit 世代を安定キーに含めない
    `retained_final_ai_cache` も全失効し、変更前の AI 結果の復元を防ぐ
  - crop 変更 → `edit_result_cache` / `final_ai_cache` / `final_composite_cache` は保持。
    crop 済み下地と注釈を保存する永続 edit preview WebP だけを非同期失効し、
    capture / export の切り出し範囲を更新する
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
| フルスクリーンデコード | 同上 + EXIF + 動画判定 | bytes から decode + EXIF | PDFium で実 viewport×ppp に fit (raster は native 上限) | なし (サムネのみ) |
| EXIF Orientation | ✅ path から読む | ✅ bytes から読む | ❌ | — |
| アニメーション | GIF/APNG/WebP ✅ | WebP ✅ | ❌ | — |
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
