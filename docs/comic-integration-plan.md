# テキスト注釈(comic)機能 — 本体 mIV 統合計画

Status: 計画ドラフト v2.1（Codex 計画レビュー反映済み・Inc 0 はマージ保留中）
更新: 2026-06-05（安全プレップ: `scale_scene` 先行実装 §5.4 / master 現況点検 §5.5）
対象ブランチ: `lab`（master 未マージ。master 統合は別途指示で実施）

ラボ（`tools/comic_lab` + `crates/comic-core`）で完成させた吹き出し/テキスト/スタンプ/
メッセージウィンドウ注釈機能を、本体 mImageViewer（`C:\home\mimageviewer`、master）へ
統合するための計画。**実装は Claude、各インクリメントのレビューは Codex**（前回の手戻りを
防ぐ運用）。

> v2 で Codex の計画レビュー（P1×6 / P2×3 / P3×1）を反映: パリティ表をモデル項目ベースに拡充、
> パイプライン順序を一意化、座標変換コントラクトを明文化、回転座標系を着手前に確定、
> バックアップ・トグルの不整合とユーザー画像スタンプの持ち運びの穴を塞ぎ、Inc 0/8 を是正、
> スキーマ版数を追加。

---

## 0. このドキュメントの使い方・運用ルール

- **これは契約書**。実装はインクリメント順・受け入れ基準に従う。「全部結合して」を避けるためのもの。
  差分が出たら本書を先に更新してから実装する。
- **着手前にユーザーがレビュー＆承認**（特に §4 設計決定・§7 パリティ表・§8 インクリメント）。
- mIV 側の参照行番号は **目安**（master が並行更新中。関数名で探し実装時に再確認）。
- ラボの設計詳細は既存ドキュメント（§12）が正本。本書はそれを再掲せず参照する。

---

## 1. 前回（補正レイヤー統合）の反省と再発防止の3原則

前回の手戻り＝「一部の機能だけ／違う形で結合された」。原因は **曖昧な指示** と **網羅リスト・
完成定義の不在**。対策:

1. **再実装しない（再利用する）**: ロジックは `comic-core`（egui 非依存・テスト済）。mIV から依存
   追加して**書き直さず**呼ぶ。再実装しなければ「違う形」になりようがない。
2. **モデル項目ベースのパリティ表**（§7）で一つずつ消し込む。「一部だけ」を機械的に防ぐ。
3. **小インクリメント＋受け入れ基準＋毎回レビュー**（§8〜9）。各単位で「ラボと同じ見え/挙動」を
   実機確認 → Codex レビュー → コミット。

---

## 2. 基本方針: comic-core を書き換えず再利用（最大の de-risk）

- `crates/comic-core` は **egui 非依存の純ロジック**（model / layout / font / tessellate / raster）。
  出力は `RgbaOverlay`（行優先 RGBA8）。テスト 86 本。
- mIV の `Cargo.toml` に `comic-core = { path = "crates/comic-core" }` を追加するだけで
  `bake_overlay_with_stamps` / `composite_stamp_sticker` / `bubble_geometry` / `layout_text*` 等を
  そのまま呼べる。
- `tools/comic_lab`（egui アプリ）は **UI 参照実装**。mIV 側 UI は mIV 作法で書き直す。
- **許容される comic-core への追加**: ロジックの書き換えは不可だが、**純粋・加法的なヘルパー**
  （例: シーン全体の一様スケール `scale_scene`、§5.4）の追加は可（既存挙動を変えず、単体テスト付き）。
- **decode 非依存**: スタンプ画像（絵文字 SVG / ユーザー画像）は呼び出し側がデコードして
  `StampImages`（`HashMap<id, RgbaOverlay>`）で渡す契約。mIV にこのデコード層を用意する
  （ラボ `tools/comic_lab/src/stamp.rs` 相当）。

---

## 3. 機能の全体像（ラボで完成済み）

吹き出し（16 形状）/ しっぽ / テキスト（縦横・縦中横・袋文字・記法・単体）/ スタンプ（GPU quad）/
メッセージウィンドウ / 飾り / プリセット。詳細は §7 のモデル項目ベース表と §12 の設計ドキュメント。

---

## 4. 確定した設計決定（ユーザー承認済み・2026-06-05）

| # | 項目 | 決定 |
|---|---|---|
| D1 | 合成位置 | **最前面**。パイプライン最終段（§5.1 の canonical 順序の末尾）。AI/色補正の影響を受けず常にくっきり。 |
| D2 | UI 形態 | フルスクリーン編集モード（消しゴム・隠蔽と同系列）。**モード名「テキスト」**。 |
| D3 | サムネ反映 | サムネは基本補正＋AI のみ。**テキストは非反映**（隠蔽と同基準＝フルスクリーン専用）。 |
| D4 | 保存方式 | 正本=中央 `comic.db`（SQLite）。「設定のバックアップ」ON 時のみ `mimageviewer.dat` ミラー（§6）。per-image `.miv` 追加無し。 |
| D5 | 書き出し | 通常は非破壊オーバーレイ。**Ctrl+E で焼き込み出力**（テキスト込み）。 |
| D6 | ロジック | comic-core を再利用（§2）。 |
| D7 | Undo/Redo | エディタ専用スナップショットスタック（`meta_undo` に載せない。隠蔽/消しゴムと同様）。 |
| D8 | 座標系（回転） | 注釈は **canonical（非回転）ソース画素座標**で保持（EXIF/PDF レンダ/clamp 後・`rotation_db` 適用前。＝消しゴム/隠蔽マスクと同じ空間）。表示/書き出しは既存の最終画素と同じ回転ポリシーを適用。回転表示中の編集はポインタを逆回転して canonical へ写像。 |
| D9 | スタンプ画像の持ち運び | 絵文字スタンプ=同梱アセットへのキー参照（軽量）。**ユーザー画像スタンプ=ダウンスケール RGBA を注釈データに埋め込み**（フォルダ移動・別マシンでも保持）。元ファイル欠落時は欠落プレースホルダ。サイズ上限あり（§6）。 |

---

## 5. アーキテクチャ配置マップ（mIV のどこに何を差すか）

雛形は既存の「フルスクリーン編集モード」（消しゴム=`ui_erase`、隠蔽=`ui_conceal`、補正レイヤー=
local_adjust）。テキストモードを「4つ目の編集モード」として同じ骨格で追加。

### 5.1 canonical パイプライン順序（一意化・旧記述を上書き）

```
raw → erase → local_adjust → conceal → crop → color → AI(upscale) → post-filter → comic(テキスト)
```

- comic は **最終段（最前面）**。これは `docs/display-pipeline.md` の `... crop → color → AI →
  post-filter` の **後ろに 1 段足す** もの。
- **この順序が唯一の正**。`docs/comic-lab-validation-checklist.md` 等の旧記述
  「conceal → annotation → crop」は **本決定（D1）が上書きする**（実装時はそちらを参照しない）。
- comic は canonical（非回転）空間で合成し、その後に既存と同じ回転を適用（D8）。

### 5.2 表示・焼き込みの合成経路（共有）

- 表示テクスチャ選択は `resolve_fs_processed_texture()`（目安 `src/ui_fullscreen.rs:921` 付近）。
- **第一候補（表示と Ctrl+E 出力で見えを一致させる）**: 最終合成済み画像（canonical, post-filter 後）
  の **上に comic オーバーレイを CPU 合成して 1 枚のテクスチャ**にし、回転を掛けて表示。Ctrl+E は
  同じ合成結果を回転適用してファイル出力。
- 別案（描画時に `painter.image` で quad 重ね）は表示専用になり Ctrl+E と経路が割れるため非推奨。
- テクスチャ生成は `ctx.load_texture`（8192px 上限 `clamp_for_gpu`、アップロードは 1 フレ 1 枚律速
  `fs_upload_backlog`）。**comic オーバーレイのベイク＋アップロードは Inc 1 から worker 化＋律速**
  （§10。重い処理を最初から正しく）。

### 5.3 編集モード（D2）

- 雛形 = `src/ui_conceal.rs`（enter/exit・入力捕捉・左パネル）と `src/ui_erase.rs`。
  - フラグ `text_mode: bool` を追加（`is_overlay_edit_mode_active()` に組み込み）。
  - `enter_text_mode(idx)` / `reset_text_mode()`（スプレッド解決・`clear_meta_undo`・
    DB/サイドカーから注釈ロード/保存。conceal に倣う）。
  - 座標変換 `text_screen_to_image` / `text_image_to_screen`（`conceal_*` / `erase_image_layout` と
    同式）。**回転表示中はポインタを逆回転して canonical へ**（D8）。
- パネル UI: `egui::Area` + `Frame::popup`。ラボ右パネル（セリフ/本体/しっぽ/飾り、プリセット、
  フォント見本、スタンプピッカー）を mIV 作法へ移植。

### 5.4 座標変換コントラクト（最前面ベイクの厳密仕様）— P1 対応

注釈は canonical ソース画素座標（D8）で保持。最終出力解像度でくっきり描くため、ベイク前に
**シーン全体へ相似変換**を適用する（pivot だけでなく全ての絶対量をスケールする）。

- 出力 canonical 画像の寸法 `(out_w, out_h)`（crop と AI アップスケール後・回転前）と、
  変換 `T = scale(S) ∘ translate(-crop_origin)` を決める。`S = out_long / cropped_source_long`。
- **一様スケール対象（×S）**: pivot, `half_w`/`half_h`, `rx`/`ry`, `outline.width_px`,
  `text.size_px`, `padding_px`/`Insets`, しっぽ `tip`/`width_px`/base 由来量, スタンプ
  `half_w`/`half_h`, 装飾の絶対量, ウィンドウ寸法。**比率（`size_ratio` 等）は不変**。
  **`rotation_rad` は不変（オブジェクトローカル）**。
- translate: pivot/tip を `-crop_origin*S`。crop 外のオブジェクトは overlay 範囲でクリップ。
- `bake_overlay_with_stamps(scaled_objects, out_w, out_h, ...)` で **出力解像度直書き**（低解像度
  ベイク→拡大ではない＝くっきり）。
- **再ベイク条件**: 出力寸法が変わるたび（crop / AI on-off / post-filter / 表示ズームの export 時）。
- 実装: 上記相似変換は **comic-core の純加法ヘルパー `comic_core::scale_scene(&[obj], s) -> Vec<obj>`
  として実装済み**（`crates/comic-core/src/transform.rs`、2026-06-05 の安全プレップで先行実装、§2 許容範囲）。
  全絶対長を ×s、`density` のみ ÷s（飾り個数を一定に保つ）、`rotation_rad`/比率/カウントは不変。
  `s==1.0` は厳密恒等、`s<=0`/非有限は no-op。単体テスト 7 本（恒等・no-op・倍率全項目・単体テキスト・`BubbleShape` enum 全 15 変種・往復・
  実ベイク面積）。**crop translate（`-crop_origin*S`）は別段で合成**（scale_scene は原点中心スケール専用）。
- 表示（非 export）は表示解像度でベイクしてよい（速度と鮮明さの両立）。Inc 1 で速度を見て決める。

### 5.5 master 現況の反映（2026-06-05 read-only 点検）

統合着手前に master（`C:\home\mimageviewer`）の表示パイプラインを点検した結果のメモ。
**master は並行作業中（snapshot/crop 系が未コミット）なので、最終確定は Inc 1/7 で当時の
master に対して再確認する**（このスナップショットに固定しない）。

- **パイプライン順序は計画 §5.1 と一致**: master の `docs/display-pipeline.md` §3.0 の確定順序は
  `raw → 消しゴム → 補正レイヤー → 隠蔽 → crop → 色補正 → AI → post-filter`。comic を最終段に足す
  D1 はこの順序の自然な延長で、矛盾なし（D1 の前提が裏付けられた）。
- **表示テクスチャ入口は `resolve_fs_processed_texture()`**（§5.2 の参照は有効）。優先順位
  `元画像プレビュー > 編集中プレビュー > final_composite_cache > edit_result_cache > fs_cache > サムネ`
  は「動かすな」と明記。comic オーバーレイ合成はこの **最終段（final_composite 相当）の後ろ or 内側**に
  差す形になる（Inc 1 で `resolve_fs_processed_texture` のどの分岐に挿すか確定）。
- **crop がリファクタ進行中（重要）**: 直近コミット `79992d13`「Crop を独立モード化し、最後段の
  暗転 overlay + save 時切り出しに」/ `83afa557`（+ 未コミット WIP）で、crop は
  「**表示時は全体画像＋crop 外を暗転する overlay**、**実切り出しは save 時**」へ変わった。
  - 表示ベイク: comic は **非 crop（全体）座標**でベイクし最前面に重ねる。`S = display_long/source_long`、
    crop translate なし。
  - export（Ctrl+E）ベイク: §5.4 の `S = out_long/cropped_source_long` ＋ `translate(-crop_origin)` で
    **切り出し後の出力解像度**に直書き（既に §5.4 が想定済み）。
  - **未確定（Inc 1 表示 / Inc 7 export で確定）**: crop の「暗転 overlay」と comic テキストの重なり順。
    crop 外に置いたテキストを暗転対象にするか（=これから切られる領域として暗くする）、常に最前面
    （D1 純守）にするか。master の crop overlay 実装が固まってから決める。
- **キャッシュ無効化に comic 段を足す**: master §3.1 のルール「crop 変更 → edit+final cache クリア」
  「補正/AI/post_filter 変更 → final cache クリア」に倣い、**comic 編集 → `final_composite_cache`（comic は
  post-filter より後段）をクリア＋ `comic_generation`（`conceal_mask_generation` 相当）を進める**配線を
  Inc 1/2 で用意する。サムネは非反映（D3）なので thumb cache は触らない。

---

## 6. 永続化設計（D4 / D8 / D9 詳細）— P1/P2/P3 対応

**方針: 既存の消しゴムマスク/隠蔽/補正レイヤーと同じ二層方式**。「隠蔽と同基準」「移動で失わない」
「ZIP/PDF 対応」を同時に満たす。

### 6.1 正本 = 中央 SQLite `comic.db`
- キーは **必ず既存ヘルパー `App::page_path_key(idx)` / `sidecar_relative_key(idx)` /
  `sidecar_folder(idx)` を使う**（ZIP/PDF・ネスト ZIP・PDF 0 始まり・区切り文字の差異は helper に委譲。
  本書ではキー文字列フォーマットを手書きしない＝ドリフト防止）。
- 1 画像 = 注釈ドキュメント JSON（`Vec<AnnotationObject>` を serde 直列化）を 1 行で保持。
- **スキーマ版数**: テーブルに `PRAGMA user_version`、JSON に `doc_version: u32`。読み込み時の
  no-row=空注釈、壊れ JSON=空＋ログ（クラッシュさせない）。
- **マイグレーション**: 新機能なので旧 mIV データからの移行は不要。ラボ `.comic.json` からの取り込みは
  **別途指示があるまで行わない**（doc_version は将来用に予約）。

### 6.2 ポータブルバックアップ = フォルダ `mimageviewer.dat`
- `SidecarEntry`（`src/sidecar.rs`）に **`comic`（または `text`）フィールドを追加**し、`adjust` /
  `mask` / `conceal` / `local_adjust_layers` / `export_crop` と並べて dual-write。
- **「設定のバックアップ」ON のときだけ** 読み書き（OFF 中は触らず、既存ファイルも消さない）。
- 書き込み経路は既存 `with_sidecar_mut(idx, op)` / `save_*_with_sidecar` を踏襲した
  `save_comic_with_sidecar` / `delete_comic_with_sidecar`。
- ZIP/PDF はフォルダ単位サイドカー（ZIP の隣の `mimageviewer.dat`）に `page_path_key` 系で入るので、
  ZIP/PDF 内画像のテキストも移動で保持される。

### 6.3 バックアップ OFF→ON の整合（P1 対応）
- OFF 中は `comic.db` だけ更新され、フォルダ `mimageviewer.dat` は **古いまま**になりうる。後で ON に
  したフォルダを「全ページ再保存する前に」移動すると、**古いサイドカーが import される**事故が起きる。
- **対策（実装）**: 「設定のバックアップ」を OFF→ON にした時点で、**現在フォルダの comic 項目を
  `comic.db` から `mimageviewer.dat` へバックフィル**してから運用する。これを受け入れテストで担保。
  既存編集（mask/conceal/local_adjust）が同じ穴を持つなら、その挙動に合わせる/併せて是正する。

### 6.4 ユーザー画像スタンプの持ち運び（D9 / P1 対応）
- `StampSource::File(PathBuf)` は JSON 化できるが、フォルダ移動・別マシンで**画像が見つからなくなる**
  （絵文字は同梱なので生き残るのに、ユーザー画像だけ壊れる）。
- **対策**: ユーザー画像スタンプは **ダウンスケール済み RGBA（PNG 圧縮など、長辺 ≤ オンキャンバス
  表示相当、サイズ上限あり）を注釈ドキュメントに埋め込む**。絵文字は同梱アセットへのキー参照のまま。
  元ファイル欠落時は欠落プレースホルダ表示。埋め込みサイズ上限と圧縮方式は Inc 4c で確定。

### 6.5 要検証タスク（ユーザー懸念）
- 補正レイヤー（`local_adjust_layers`）が実際に `mimageviewer.dat` に dual-write されているか実機確認
  （調査では `SidecarEntry` に含まれ要件は残っていそう）。テキストの dual-write 実装時に併せて確認。

---

## 7. 機能パリティ・チェックリスト（モデル項目ベース／「一部だけ結合」防止）— P1 対応

`comic-core` の `model.rs` の項目を基準に網羅。各項目が mIV でラボと同じ見え/挙動になったらチェック。
**[deferred]** は意図的に v1 対象外（実装しないことを明示）。

### 7.1 TextBlock（全 kind 共有）
- [ ] text / font_key / size_px / color
- [ ] orientation 横書き / 縦書き（OpenType `vert`、`。、「」…ー` 縦字形）
- [ ] align（start/center/end）/ line_gap / letter_gap
- [ ] outline 袋文字（色・太さ）
- [ ] bold / italic
- [ ] auto_tcy 自動縦中横（数字 2-3 桁 / `!?`）
- [ ] markup_enabled ＋ 記法 3 セット（`[]{}` / `〈〉《》` / `〚〛〘〙`、縦中横/横倒し）
- [ ] preset_link（適用で点灯・個別編集で解除）

### 7.2 BubbleObject
- [ ] shape 全 16: Ellipse / RoundRect / Burst / Cloud / Polygon / Diamond / Heart / Arrow / Soft /
      MotionLines(集中線) / SpeedLines(流線) / Concentration(意識) / Strokes(線) / DoubleStroke(二重線) /
      MindEllipse(思考楕円=Ellipse+Thought) / TextOnly(なし)
- [ ] 形状別パラメータ（rx/ry, half, corner, spikes/jag/seed, lobes/amp, sides, dir_rad, count, gap …）
- [ ] fill（色）/ fill_opacity / outline（色・太さ）/ padding_px
- [ ] auto_size（文字に合わせる、ON/OFF 切替時の fit 凍結）
- [ ] merge_with_below（union、線幅維持＝2×、非対応形状は無効化＋無効ホバー）
- [ ] shape_preset_link

### 7.3 Tail（しっぽ）
- [ ] kind: Spike（輪郭一体 splice）/ Thought（円トレイル）
- [ ] tip / width_px / base_t / base_auto（自動付け根＝対象方向）
- [ ] 既定 左下45°、表示トグル（off→on で stash 復元、非対応形状で無効化＋無効ホバー）

### 7.4 StampObject
- [ ] source: Emoji（同梱）/ File（ユーザー画像、§6.4 で埋め込み）
- [ ] half_w/half_h（アスペクト保持の一様スケール）/ opacity / flip_h / flip_v
- [ ] outline（ステッカーフチ）
- [ ] style_preset_link
- [ ] ピッカー（カテゴリ / 検索 / 最近）/ ユーザー画像追加 / source 差し替え
- [ ] デコードキャッシュ / 欠落プレースホルダ / GPU テクスチャ quad（多数複製でも軽い）
- [ ] 絵文字アセット同梱＋CC-BY 帰属表記（ソフトウェア情報 / readme）

### 7.5 MessageWindowObject
- [ ] frame: None / SolidRounded / DoubleLine（+ frame_gap_px, outline）
- [ ] fill: None / Solid / Translucent / GradientScrim（+ gradient_to 線形グラデ / scrim_dense_side）
- [ ] position: Top / Bottom / Center / Free
- [ ] size: FullWidth / Inset / AutoFitText
- [ ] vanchor / Insets（per-side padding）/ shadow（ドロップシャドウ）
- [ ] name_plate: mode / 名前テキスト / 色 / fill / outline / corner / padding / offset、名前ヘッダ（上部常時）
- [ ] portrait: side / width / fill / outline / margin（本文との非重なり）
- [ ] indicator: kind ＋ indicator_auto（溢れた時だけ表示）
- [ ] 日本語禁則ワードラップ / AutoFitText / オーバーフロー警告（赤枠）
- [ ] [deferred] 9-slice 画像枠 / 実立ち絵画像 / Beveled 枠 / per-corner 角丸 / choice・NVL 複数エントリ

### 7.6 DecorationLayer（飾り）
- [ ] kind: Sparkle(星) / Flower(花) / Bubble(泡)
- [ ] placement: Outline / Outside / Inside / Tail
- [ ] density / size_ratio / color / seed
- [ ] outline_width / outline_color / center_color（花中央）/ points（星）/ petals（花）/ gradient（泡）

### 7.7 共有 UI 挙動
- [ ] オブジェクト一覧（上へ/下へ/複製/削除）、z normalize、enable/disable トグル
- [ ] 選択 / 移動 / 四隅スケール / 回転ノブ / しっぽハンドル（当たり判定＝comic-core ジオメトリ一致）
- [ ] Undo / Redo（エディタ専用、coalesce）
- [ ] プリセット: セリフ / 本体 / ウィンドウ（system + user、適用 / 更新 / 削除 / 改名 / リンク点灯）
- [ ] 追加ダイアログ（吹き出し / ウィンドウ）= レスポンシブ手動グリッド
- [ ] フォント見本ダイアログ（見本＝編集中セリフ先頭行、名前見切れ無し、ファイル追加）
- [ ] スタンプピッカー
- [ ] 構造トグル（結合 / しっぽ / 飾り、非対応で無効化＋無効ホバー）、自動サイズトグル
- [ ] IME 安全なテキスト編集（Enter/Escape 横取りしない）、記号挿入ボタン

### 7.8 統合・永続化挙動
- [ ] comic.db 保存 / 読込（スキーマ版数）
- [ ] mimageviewer.dat ミラー（バックアップ ON 時のみ）、OFF→ON バックフィル（§6.3）
- [ ] ZIP / PDF 内画像での編集・保存（page_path_key）
- [ ] フォルダ移動で保持（バックアップ ON）、ユーザー画像スタンプも保持（§6.4）
- [ ] 非破壊オーバーレイ表示（最前面 D1）/ Ctrl+E 焼き込み出力（表示と一致）
- [ ] サムネ非反映（D3）/ 非破壊回転 DB との整合（D8）

---

## 8. インクリメント分割（独立してビルド・テスト・実機確認できる単位）— P2 対応

各 Inc の完了条件: **ビルド緑＋テスト緑＋実機でラボと同じ見え＋Codex レビュー P1/P2 なし＋
コミット**。受け入れを満たさなければ次へ進まない。**「ラボ完全パリティ」の最終署名は Inc 6 完了後**
（プリセット/ダイアログ/ハンドルが揃って初めて成立する）。

- **Inc 0: lab→master マージ ＋ 依存配線 ＋ アセット方針**（master の修正一段落＋ユーザー「マージ」後）
  - `lab`→`master` マージで `crates/comic-core` + `tools/comic_lab` を master に持ち込む。ワークスペース
    /Cargo メタを整え、mIV に `comic-core` 依存追加。`comic-core` のテストを master 上で実行。
  - 絵文字 Twemoji（CC-BY）の vendor 化・同梱・帰属方針を決める（`scripts/setup-twemoji.sh` の取得物）。
  - 受け入れ: master でビルド緑・既存テスト緑・comic-core テスト緑。mIV から comic-core 参照可。
- **Inc 1: 最前面オーバーレイ表示（読み取り専用）＋ 座標/回転/worker 基盤**
  - 固定 or ファイルの `Vec<AnnotationObject>` を §5.4 の変換でベイクし最終画像の上に合成表示（D1）。
    canonical 座標＋回転ポリシー（D8）を確定・実装。**ベイク＋アップロードを worker 化＋1 フレ 1 枚律速**。
  - 受け入れ: crop/AI/色補正/回転を変えてもテキストが正しい位置・解像度・回転で最前面に乗る（くっきり）。
    `analyze_perf.py hitches` で編集外のヒッチ無し。
- **Inc 2: 永続化**
  - `comic.db` 読み書き（スキーマ版数）＋ `mimageviewer.dat` ミラー（バックアップ ON 時、OFF→ON
    バックフィル §6.3）。`page_path_key` で通常/ZIP/PDF。
  - 受け入れ: 保存→再起動→復元。ZIP/PDF 内画像でも。バックアップ OFF 時は dat を触らない。
    フォルダ移動で保持。Image/ZipImage/ネスト ZIP/PdfPage/移動 import の受け入れテスト。補正レイヤー
    dual-write の実機確認（§6.5）。
- **Inc 3: 編集モード骨格 ＋ IME**
  - 「テキスト」モード enter/exit、キャンバス入力捕捉、回転下の逆写像（D8）、選択・移動、IME 安全な
    Enter/Escape ゲート。
  - 受け入れ: 操作感が消しゴム/隠蔽と同等。選択・ドラッグがラボ一致。回転表示中も正しく掴める。IME 変換が
    壊れない。
- **Inc 4: オブジェクト種別の描画＋インライン編集を移植**（4a→4e、各独立コミット）
  - 4a 吹き出し（形状・しっぽ・結合・自動サイズ・本体パネル）/ 4b 単体テキスト（セリフパネル、IME）/
    4c スタンプ（ピッカー＋編集＋GPU quad＋絵文字アセット＋帰属＋§6.4 埋め込み）/ 4d ウィンドウ
    （枠/塗り/名前/立ち絵/指標/ワードラップ）/ 4e 飾り
  - 受け入れ（各）: **当該種別の描画とインライン編集コントロール**がラボ一致。§7 の該当項目を消し込む。
    （プリセット/追加ダイアログ/フォント見本/変形ハンドルは Inc 5/6 に依存するので、ここでは
    描画＋編集コントロールに限定して判定）。
- **Inc 5: プリセット ＋ 追加/見本/ピッカー ダイアログ**
  - セリフ/本体/ウィンドウ プリセット、吹き出し/ウィンドウ追加ダイアログ（レスポンシブ）、フォント見本、
    スタンプピッカー。スナップショットテスト・glyph lint。
  - 受け入れ: 保存/適用/更新/削除/リンク点灯がラボ一致。名前見切れ無し。
- **Inc 6: 変形ハンドル ＋ Undo/Redo ＋ パリティ最終署名**
  - 四隅スケール/回転ノブ/しっぽハンドル、エディタ専用 Undo。**ここで §7 全消し込みを最終確認**。
  - 受け入れ: Ctrl+Z/Y がモード内で動き `meta_undo` と干渉しない。§7 完了。
- **Inc 7: 書き出し統合（Ctrl+E）**
  - 焼き込み画像出力（テキスト込み、§5.2 と経路共有）。crop/回転整合。命名/形式は既存 export 準拠。
  - 受け入れ: 出力画像がフルスクリーン表示と一致（くっきり・回転反映）。
- **Inc 8: 仕上げ・回帰**
  - 残りの perf 詰め、非破壊回転 DB の総合整合確認、マニュアル/製品ページ更新、スナップショット総点検。
  - 受け入れ: docs 同期、回帰緑。
  - 注: UI 応答性/IME/回転/律速は **Inc 1/3/4 で都度** 担保済（Inc 8 に溜めない）。

---

## 9. 各インクリメントの作業ループ

1. 該当 Inc の受け入れ基準を確認（必要なら本書を更新）。
2. 実装（comic-core は再利用＋加法ヘルパーのみ。mIV 側 UI/配線）。
3. `cargo build` / `cargo test` / `cargo fmt`。
4. **ユーザーが実機で目視確認**（ラボと同じ見え/挙動か）。
5. **Codex レビュー**（read-only、同一 Inc は resume）。P1/P2 対応。
6. コミット（pathspec、`Codex P<N> 対応` 記載）。§7 を消し込み。

---

## 10. mIV 制約の遵守チェックリスト

- **UI スレッド同期 I/O 禁止**（`docs/ui-responsiveness.md` §4）: 注釈ベイク（フル解像度 RGBA）＋
  `ctx.load_texture` は重い。**Inc 1 から worker 化＋1 フレ 1 枚律速**。編集中の連続ベイクはアダプティブ
  スロットル（ラボ `last_bake_dur` を移植）。
- **read_dir は `entry.file_type()`**。
- **IME**: Enter/Escape は `dialog_enter_pressed`/`dialog_escape_pressed` 経由。新ビューポート入口で
  `update_ime_state`。
- **ダイアログ**: `default_pos`（`anchor` 禁止）、`.open(&mut open)`。手動グリッドはラボ方式流用。
- **スナップショットテスト**（`docs/ui-snapshot-policy.md`）: 配色/レイアウト変更時に更新・目視。新パネルは
  純描画関数化。
- **Unicode グリフ**（`scripts/check_ui_glyphs.py`）: 固定 UI 文言に環境依存グリフ禁止。
- **永続キー**: `page_path_key` 系 helper のみ使用（§6.1）。
- **未リリース機能**: 新規＝マイグレーション不要。コミットにその旨記載。

---

## 11. 未確定の実装詳細（該当 Inc で確定）

- ~~**§5.4 のスケールヘルパー位置**~~: **確定** — comic-core 加法ヘルパー
  `comic_core::scale_scene`（`transform.rs`）として実装済み（2026-06-05 安全プレップ）。crop translate の
  合成だけ Inc 1 で mIV 側に書く。
- **表示ベイク解像度**: 表示は表示解像度か（速度）、常に出力解像度か（鮮明）。実測で（Inc 1）。
- **スタンプ GPU quad の mIV 統合**: まず 1 枚 CPU ベイク（`StampImages`）で動かし、後段でラボの GPU quad
  最適化を移植。テクスチャ eviction を mIV 流儀へ（Inc 4c / Inc 8）。
- **フォント列挙**: mIV のフォント列挙（`winreg`）再利用か comic-core 側か（Inc 4b）。
- **ユーザー画像スタンプ埋め込みの上限・圧縮**（§6.4、Inc 4c）。
- **絵文字アセットの vendor 化方式**（Inc 0）。

---

## 12. 参照ドキュメント

### ラボ側（機能の正本）
- `docs/speech-bubble-text-tool-plan.md` / `docs/speech-bubble-tool-design.md`
- `docs/comic-lab-frameplanner-shapes.md` / `docs/stamp-feature-design.md` /
  `docs/message-window-design.md` / `docs/vertical-text-opentype-plan.md`
- `docs/comic-lab-validation-checklist.md`（注: 合成順序は本書 §5.1 が上書き）
- `docs/comic-lab-progress.md`（進捗・本体統合メモ）

### mIV 本体側（統合先の作法）
- `docs/architecture-overview.md` / `docs/display-pipeline.md`
- `docs/local-adjustment-layer-v1.1.0-plan.md`（編集モード＋永続化の最も近い前例）
- `docs/preset-and-adjustment.md` / `docs/virtual-folders.md` / `docs/ui-responsiveness.md` /
  `docs/ui-snapshot-policy.md`
- 編集モード雛形: `src/ui_conceal.rs` / `src/ui_erase.rs`
- 永続化: `src/sidecar.rs` / `src/adjustment_db.rs` / `src/mask_db.rs` / `src/app.rs`
  （`page_path_key` / `sidecar_relative_key` / `with_sidecar_mut` / `save_*_with_sidecar`）
- Undo: `src/undo_stack.rs` / `src/undo_ops.rs`
