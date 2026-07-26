# comic_lab 進捗・ハンドオフメモ

Status: 作業中スナップショット (コンパクト耐性用)
更新: 2026-06-05 セッション (フォント生成オノマトペ試作 / FramePlanner 形状 Phase1-3 完了 / worktree 運用)

吹き出し・テキスト注釈ツールの **lab 試作** (`tools/comic_lab` + `crates/comic-core`)。
設計の正本: [speech-bubble-text-tool-plan.md](speech-bubble-text-tool-plan.md) (Claude 案) /
[speech-bubble-tool-design.md](speech-bubble-tool-design.md) (Codex 案)。

## ⚠ 作業環境 (次セッション必読・worktree 運用)

ラボ作業は **専用 worktree `C:\home\mimageviewer-lab` (branch `lab`)** で行う。本体アプリ
セッションは `C:\home\mimageviewer` (branch `master`) を使い、index/HEAD が分離するので
コミットの取り合いが起きない (1ツリー共有時に多発した事故の回避策。CLAUDE.md「複数の
Claude Code セッションを並行で動かす」節)。

- Claude Code の cwd は毎コマンド main repo にリセットされるので、ラボ操作は**明示パス**で:
  - git: `git -C C:\home\mimageviewer-lab <cmd>`
  - cargo: `cargo <cmd> -p comic_lab|comic-core --manifest-path C:\home\mimageviewer-lab\Cargo.toml`
    (本体パッケージをビルドしないので **vendor 不要・build.rs 走らない**。bare `cargo build` は禁止)
  - fmt: `(cd C:\home\mimageviewer-lab && cargo fmt)` — `--manifest-path` だけだと main 側を見るので cd 必須
  - 編集: `C:\home\mimageviewer-lab\...` の絶対パス
- `lab` ブランチは **master 未マージ**。実機確認OK後に一度だけ `lab`→`master` を merge する
  (ユーザー判断待ち)。退避ブランチは不要 (worktree 隔離で十分)。

## 2026-06-05: フォント生成オノマトペ試作 (branch lab)

- 左パネルに **オノマトペ追加** を追加。画像素材系のオノマトペは従来どおり **スタンプ追加** 側に残し、
  OFLフォント + スタイルプリセットで生成するものだけをオノマトペ追加から入れる整理にした。
- オノマトペは新モデルを増やさず `AnnotationKind::Text` として生成する。保存/Undo/右パネル編集/高解像度
  bake は既存テキスト機能に乗る。プリセットは文言・サイズ・色・袋文字・初期角度・候補フォント名を持つ。
- `tools/comic_lab/assets/fonts` に置いた TTF/OTF/TTC を起動時に自動登録する lab 用フォント置き場を追加し、
  Google Fonts の OFL 日本語フォントを 18 ファイル (約 61MiB) 同梱。各フォントの OFL 文面も同ディレクトリに置く。
- プリセットは「フォントごとに1つの言葉」を割り当てる方式に変更。`Otomanopee One` / `Dela Gothic One` /
  `Reggae One` / `RocknRoll One` / `Rampart One` / `Stick` / `Train One` / `DotGothic16` /
  `Hachi Maru Pop` / `Darumadrop One` / `Yusei Magic` / `Klee One` / `Kaisei Decol` /
  `Zen Kurenaido` / `Kaisei Tokumin` / `Zen Maru Gothic` / `M PLUS 1` / `Shippori Mincho`
  をサンプルとして選べる。見つからない環境では既定日本語フォントへフォールバックする。
- オノマトペ追加ダイアログのプレビューは egui の通常テキストではなく、選択時と同じ `comic-core`
  bake 経路で実フォントを画像化して表示するようにした。字形比較しやすいようカードも大型化。
- 単体テキスト / フォント生成オノマトペにも、スタンプ同様の矩形ハンドルを追加。四隅ドラッグで
  `TextBlock.size_px` を均一スケールし、上部の回転ハンドルで回転できる。単体テキストの回転中心は
  文字矩形の中央に変更し、新規テキスト/オノマトペは追加位置の中心に文字矩形が来るよう初期配置する。

## 2026-06-04 末: FramePlanner 形状追加 (branch lab、コミット済み)

[FramePlanner](https://github.com/jonigata/FramePlanner2) (MIT) 参考に吹き出し形状を拡充。
詳細は [comic-lab-frameplanner-shapes.md](comic-lab-frameplanner-shapes.md)。

- **Phase1 ベクター形状** (5bfdd0d3): `BubbleShape` に Polygon/Diamond/Heart/Arrow/Soft。
  既存 tessellate→塗り/枠/しっぽ/本文/自動サイズ/ハンドルに乗る。lab `BubblePreset` に
  やわらか/多角形/ダイヤ/ハート/矢印 追加。
- **Phase2 線群エフェクト** (929af8c6): `MotionLines`(集中線)/`SpeedLines`(流線)。塗りでなく
  多数の線 + 中央クリア楕円(`LINE_FIELD_CLEAR_RATIO=0.55`)。`draw_line_field` (raster)。
- **Codex P1/P2 対応** (34ccded7): 流線を外接/クリア楕円の交点計算に書換 (斜めでも AABB 内)、
  自動サイズを内接率ベースに、count/sides 上限クランプ。
- **Phase3** (233bebf8): 意識(`Concentration` ぼかし縁楕円, per-pixel feather+soft ring)/
  線(`Strokes` 手描き風多重 stroke)/二重線(`DoubleStroke` 同心 2 本)/思考(楕円)
  (`MindEllipse` = Ellipse+Thought)/なし(`TextOnly` テキストのみ)。◯?=既存形状+Thought。
- **Phase3 Codex 2 周対応** (16b22a2c, 23d491da): 意識の alpha 二重適用修正・走査クランプ・
  merge では do_decotext で 1 回描画 / 二重線の内側 ring を merge erase 後に描画 /
  しっぽ非対応形状 (`shape_renders_tail`) の AABB・hit-test・UI gate / merge チェーンから
  非多角形形状を除外 (`shape_is_mergeable`)。Codex 最終確認 P1/P2 なし。
- テスト comic-core **83** / comic_lab 4 green。FramePlanner 形状は **Phase1-3 完了**。
- 実機確認用 release バイナリ: `target/release/comic_lab.exe` (`cargo run --release -p comic_lab`)。

## 2026-06-04 末: 実機フィードバック対応 (UI / 性能、branch lab、コミット済み)

- **追加ダイアログのレスポンシブ化** (c405acd2): 吹き出し追加ダイアログを viewport 幅
  クランプ + set_max_width + 縦 ScrollArea で固定幅サムネが折り返すように。
- **ピッカーサムネ修正**: 集中線/流線=線群描画、意識=ぼかし楕円、矢印=既定しっぽ廃止。
- **集中線/流線の文字可読性**: セリフに白い袋文字(フチ)を既定 ON (text_outline)。
- **絵文字**: `scripts/setup-twemoji.sh` で vendor/twemoji/svg に 125 個取得 (gitignore)。
- **スタンプ負荷の根治** (a49fa1bb, 1c0b3bb0): ドラッグ/拡縮/回転中はスタンプを CPU bake
  から除外し **GPU テクスチャ quad** で描画 (拡縮・回転は GPU でほぼ無償)。source は
  `stamp_textures` に一度だけアップロード。ドラッグ終了で完全 bake に戻り z/輪郭が正確。
  ドラッグ中 bake はアダプティブスロットル併用。decode 失敗/無効スタンプは bake に残す
  (消えない)。Codex 3 周対応で P1/P2 なし。
- 累積コミット: 5bfdd0d3→929af8c6→34ccded7 (P1-2) → 233bebf8→16b22a2c→23d491da→07515f7c
  (P3) → c405acd2→a49fa1bb→1c0b3bb0 (実機 FB)。lab は **master 未マージ**。

## 2026-06-04 末: スタンプ性能 + ダイアログ (実機 FB 第3弾、コミット済み)

実機計測 (perf HUD: F1 / COMIC_LAB_PERF) で composite(CPU) が upload(GPU) の約 10 倍 =
スタンプの CPU ラスタライズが支配的と判明。これを根治。

- **スタンプ性能** (a5ee93ed→754cd325→50cbffc9): 枠なしスタンプを **常時 GPU テクスチャ
  quad** で描画し CPU bake から除外 (`gpu_stamp_ids` = 最上位 z から連続する枠なし・
  デコード可スタンプ = top-z run、z 整合を維持)。bake はスタンプを一切ラスタライズ
  しないので枚数/サイズ/回転によらず軽い。掴み時カクつき・ドラッグ終了時の位置ズレも
  解消 (CPU↔GPU 持ち替えが無い)。枠付きスタンプのみ CPU bake (halo 維持)。
  perf HUD (bake 合計/composite/upload + 数) を追加。
- **追加ダイアログ** (786bfd83): 折り返さない/全幅/リサイズ変、を Codex 相談で解決。
  horizontal_wrapped の未折り返し min 幅フィードバックが原因 → **手動グリッド**
  (available_width から列数算出、ui.horizontal_top 行) に変更。`.max_width` は記憶済み
  サイズを縮められないため Window `.id` を新規化して破棄。`.pivot(CENTER_CENTER)` の
  リサイズ挙動を避け default_pos 中央寄せに。共有定数 PRESET_CELL_W/WINDOW_PRESET_CELL_W/
  grid_cols() を追加。
- 実機確認 OK (速度改善・ズレ解消・ダイアログ折り返し/リサイズ正常)。
- Codex を計 9 ラウンド (同一セッション resume) 活用、最終 P1/P2 なし。

(以下は worktree 移行前=master 上の履歴。同じ内容が lab ブランチにも入っている)

## 2026-06-04 後半: 縦書き OpenType 化 (案B) + スタンプ機能 (未コミット)

実機未確認のまま (a)(b) を一括実装。検証は `cargo test -p comic-core` (62 green) +
`cargo check -p comic_lab` (clean) + `cargo test -p comic_lab --bin comic_lab` (stamp 3 green)。

- **(a) 縦書きの根本対応 = 案B**: フォント層を `ab_glyph` → `rustybuzz`(パーサ+シェイパー)
  + `ab_glyph_rasterizer`(カバレッジ) に置換。縦書きは `Direction::TopToBottom` でシェイプし、
  フォントの `vert`(無ければ rustybuzz の UAX#50 fallback)が自動適用 → `。、「」…ー` が
  正しい縦書き字形 + 縦メトリクスに。文字ごとの置換表/回転/右上寄せ stopgap は全削除。
  `GlyphPlacement` は `glyph_ch:char` → `glyph_id:u16`、`GlyphForm` は `Upright`/`Sideways` のみ。
  実フォント(Yu Gothic)を読む golden 回帰テストで `。`右上 / `…`「」縦字形置換 / ゴールデン列の
  焼き込みを担保。詳細: [../../vertical-text-opentype-plan.md](../../vertical-text-opentype-plan.md) 実装サマリ節。
- **(b) スタンプ(画像ステッカー)**: `AnnotationKind::Stamp` を追加。comic-core は decode-free の
  まま `bake_overlay_with_stamps(.., stamps: HashMap<id, RgbaOverlay>)` で合成 (scale/flip/opacity/
  ステッカー縁取り/欠落プレースホルダ、回転は既存 bake_into)。lab はピッカーダイアログ
  (カテゴリ/検索/最近/絵文字グリッド/画像ファイル追加) + 右パネル編集 + ハンドル(一様スケール) +
  履歴永続化。emoji は curated catalog(~120) を SVG→resvg で展開、`scripts/setup-twemoji.sh` で
  取得 (CC-BY 帰属は本体統合時に必要)。詳細: [stamp-feature-design.md](stamp-feature-design.md) 実装サマリ節。
- **依存追加**: comic-core = `rustybuzz` + `ab_glyph_rasterizer` (`ab_glyph` 削除)。
  comic_lab = `resvg`、`image` に webp/gif/bmp。
- **本体未統合**: どちらも lab のみ。本体 `src/` は触っていない。

## 2026-06-04 実機フィードバック対応 (perf / 縦中横 / 検証サンプル)

実機確認後の 3 点を対応:

- **⚠ 重大 perf 修正 (face caching)**: `LoadedFont` が **呼び出しごとに**
  `rustybuzz::Face::from_slice`(.ttc 解析)を再実行していた (h_advance ≈46µs/回、bake
  6.4ms/回・release)。`self_cell` で **バイト列 + 解析済み Face を 1 度だけ保持**するよう変更
  (h_advance 0.04µs/回 ≈ 1000×、shape/rasterize ≈20-28×、bake ≈2×)。これが「操作が重い」の
  主因。テストスイートも 1s → 0.05s に短縮 (テストも再解析に支配されていた)。
- **縦中横を「縮小せず全角サイズ + 列幅可変」に変更**: 従来は `size*0.5` に縮小してセルに収め
  ていたのを、**本文と同じサイズのまま横並び**にし、列幅 = `max(cell, 縦中横ラン幅)` として
  列を右→左へ可変幅パッキング。桁数が多いと**その列の左右間隔が広がり**隣の列と重ならない
  (`cluster_width` + per-column `col_left`/`widths`)。回帰テスト
  `tcy_full_size_widens_column_no_overlap` を追加、`mixed_punct_tcy_cluster` を全角サイズ assert に更新。
- **テキストのドラッグ移動**: hit-test の bounds は実測で正しく(glyph ink ⊂ bounds)、移動
  ロジックも全 kind で pivot を動かす。**動かなく見えたのは上の perf 起因のラグ**(drag 中は
  0.09s throttle で再 bake、bake が重すぎて追従しなかった)。face caching で解消見込み。
- **検証サンプル**: `docs/comic-lab-sample-scene.comic.json`(読み込めるシーン) +
  `docs/comic-lab-sample-text.md`(コピペ用) + `scripts/gen_comic_sample.py`(生成元)。
  IVS は実測で Yu Gothic/Meiryo/MS Gothic 共通対応の `辻 葛 芦` を採用。
  `sample_scene_loads` テストでシーンが SidecarDoc 経路でロードでき IVS/結合文字が保持される
  ことを保証。

## 構成
- `crates/comic-core` — pure (egui 非依存)。model(+Stamp) / layout(横/縦書き+縦中横+横倒し) /
  font(`rustybuzz` shape + `ab_glyph_rasterizer` coverage, `rotate_cw`) /
  tessellate(形状+しっぽ splice + 装飾) / raster(bake + stamp 合成)。
- `tools/comic_lab` — eframe/egui 試作。**常に bake して表示** (WYSIWYG)、ハンドルのみライブ描画。
- 検証: `cargo check -p comic_lab` と `cargo test -p comic-core`。整形: `cargo fmt -p comic-core -p comic_lab`。
  **実行中の comic_lab.exe があると exe がロックされ build が LNK 失敗するので check を使う**。
- 触ってはいけない: 本体 `src/`(Codex がパイプライン再構成を別途作業中)、`vendor/`、`build.rs`。
- コミットは指示があるまでしない(全て未コミット)。

## landed・検証済み (このセッション)
- 形状: 楕円 / 角丸・矩形 / トゲトゲ(Burst, seed ジッター) / 雲(Cloud)。
- しっぽ: Spike(輪郭一体 splice) / Thought(◯ トレイル)。移動で tip 追従、表示トグルは stash 保存。
- テキスト: 横/縦書き、自動縦中横(数字2-3/`!?`、同種連続記号は正立スタック)、袋文字、単体テキスト。
- マーカー記法: `parse_runs`/`cluster_column_from_runs`、横倒し=`rotate_cw`。
- フォント: システム列挙(winreg レジストリ) + 遅延ロード(`ensure_font_loaded`) + ファイル追加。
- プリセット: テキスト/形状の system+user、`presets.json` 保存。**明示リンク**(`preset_link`/
  `shape_preset_link`)=適用で点灯・個別編集で解除・同名上書きでリンク中全オブジェクトへ一括反映。
- UI: ダークテーマ(local_adjust_lab 準拠)、左=オブジェクトカード+`上へ/下へ/複製/削除`、
  追加=プリセットサムネダイアログ、右=テキスト主役→吹き出し詳細、`normalize_z`(重なり一意化)、
  Undo/Redo(commit-on-settle)、Delete は編集中無効、記号挿入ボタン、ドラッグ throttle、装飾(きら/花/泡)。
- Codex レビュー 2 回実施済 (P1 なし)。直近修正: Undo順 / z正規化 / glyph_step clamp / tail_stash prune。

## 7項目バッチの結果 — 大半が未完了 (要再実行)
前回の「7項目一括」エージェントは重すぎて途中停止(完了通知なし)。実際に landed したのは:
- **task1 IME 改行修正 ✅** (`consume_ime_enter` @ comic_lab main.rs:1083、`update` 先頭 :1125 で呼ぶ)
- **task2 マーカー △ データのみ**: `default_markup_rules` を `[..]`=縦中横 / `{..}`=横倒し に変更、
  正立(`〔〕`/InlineDir::Upright のデフォルト)廃止。**ただし 3セット選択UIは未実装**。
  ※これで旧マーカー前提の 3 テストが壊れたが**修正済み**(layout.rs `markup_braces_make_sideways_run` /
  正立テスト削除 / lib.rs sideways テストを `{LOVE}` に)。comic-core 30 tests green。

**未実装(再実行が必要) = 下の「未着手キュー」3〜7 に統合済み。**
教訓: 次回は 1〜2 タスク単位の小さいエージェントに分割する(7一括は失敗した)。

## 完了 (このセッション、未コミット)
- **task1 マーカー3セット選択UI ✅**: comic-core に `markup_rules_brackets/angle/white` 追加 +
  re-export。lab: 記法 ON 時に ComboBox で `[]{}`/`〈〉《》`/`〚〛〘〙` を選択 (`marker_pairs_eq` で
  現セット判定、カスタムも表示)。記号挿入ボタンは選択セットに追従し dir ラベル付き。
  test `marker_sets_have_distinct_pairs_and_fixed_dirs`。
- **task2 吹き出し自動サイズ ✅**: `BubbleObject.auto_size`(serde 既定 ON) 追加。comic-core
  `tessellate::fit_bubble_shape`(楕円√2 / Burst は /jag / Cloud は /(1-amp)) + `raster::
  effective_bubble_shape`(font 解決して fit、空文字/未ロードは stored shape)。bake_object と
  lab の `object_bounds`(hit-test) が共通の effective shape を使うので pixel と当たり判定が一致。
  UI: 「文字に合わせて自動サイズ」チェック、ON 中はサイズスライダ非表示・style param は維持、
  OFF 切替時に現 fit 寸法を `b.shape` へ確定書き込み(undo churn 回避でオンデマンド方式)。
  build_bubble の struct literal に `auto_size: true` 追記。test `fit_preserves_variant_and_contains_text`。
- **task3 しっぽ付け根 自動+ハンドル ✅**: `Tail.base_auto`(serde 既定 ON) 追加。comic-core
  `auto_base_t`(中心→先端 ray と outline の**最外**前方交点 → arc-length 比) /
  `resolve_tail_base`(描画/当たり判定用に base 点を解決) / `nearest_base_t`(ドラッグ点→最近傍
  outline の arc-length 比)。`bubble_geometry` が base_auto を解決して splice/thought に渡す。
  lab: `DragKind::TailBase` 追加、cyan の付け根ハンドルを `resolve_tail_base` 位置に描画+ヒット、
  ドラッグで base_auto=false & base_t=nearest。UI「付け根を自動 (対象方向)」チェック、OFF 時のみ
  base_t スライダ表示。tail 構造体リテラル 3 箇所に `base_auto` 追記。
  test `auto_base_t_points_toward_tip` / `nearest_base_t_snaps_to_outline`。
- **task4 装飾調整 ✅**: `DecorationLayer` に `outline_width`(0=なし,px)/`outline_color`/
  `center_color`(花中央)/`points`(きら)/`petals`(花)/`gradient`(泡) を追加 (serde default 付き)。
  raster: `draw_decoration` を `&DecorationLayer` 受け取りに変更し各 param を反映、`draw_soap_bubble`
  (同心リングの半透明グラデ+左上ハイライト) 追加。`PlacedDeco` はジオメトリのみ維持。
  lab: 装飾カードに 縁取り太さ/色・種類別(とがり数/花びら数+中央色/泡グラデ) コントロール追加。
  test `decorations_with_styling_bake_without_panic`。
- **task6 横倒し列幅フィット ✅**: `LoadedFont::glyph_height`(outline px_bounds の高さ) 追加。
  layout `sideways_size(font, run, size, cell)` = run 内最大 ink height が `cell*0.9` 超なら
  全体を一律縮小 (font-global ascent+descent でなく per-glyph ink で測り過剰縮小回避)。
  `cluster_height` に cell 引数追加、Sideways 配置も ssize 使用。
  test `sideways_glyphs_fit_within_column` (回転後 ink height <= cell*0.9 を全 sideways glyph で検証)。
- **task5 フォントサンプルダイアログ ✅**: 「見本から選択」ボタンで中央モーダル。`show_font_dialog`
  /`font_dialog_target`/`font_dialog_filter`/`font_dialog_sample`/`font_sample_cache` を ComicLab に追加。
  `open_font_dialog`(sample=対象テキスト先頭行 or `あア亜Ag 12!?`, cache clear) /
  `render_font_sample`(comic-core で 1 行 bake→ColorImage、resolved key 不一致は fallback 回避) /
  `font_sample_texture`(遅延 ensure_font_loaded+bake→texture を (name,sample) でキャッシュ) /
  `draw_font_dialog`(ScrollArea::show_rows でグリッド、可視行のみ rasterize) / `draw_font_card`
  (見本 + フォント名、クリックで font_key 設定+ensure+preset_link 解除)。見本テキスト編集で cache clear。
  TextSectionResult に `open_font_dialog` 追加、update で `draw_font_dialog` dispatch。
- comic-core **37 tests green / lab cargo check OK / fmt クリーン / `src/` 無傷**。
- **キュー 1〜6 完了 (A/B も 2 セッション目で完了済み、下記参照)。**

### Codex レビュー (1〜6 バッチ、同一セッション resume) — 対応済み
P1 なし。指摘 5 件のうち 4 件修正 + 1 件は仕様許容:
- **P2-1 ✅** layout: `line_advance`/`col_advance` を `.max(1.0)` (負の line_gap で bounds 崩壊→
  auto-size が極小化するのを防止)。
- **P2-2 △ 部分対応** hit-test: `object_bounds` を `bubble_geometry().outline`+thought 円+tip+
  枠線半幅 の AABB に拡張 (本体・しっぽ・トゲはクリック可に)。**装飾(外側スパークル等)の
  はみ出しは hit-test 未包含のまま** = lab プロトタイプとして許容 (毎回 place_decorations する
  コスト回避)。将来 A で本格ハンドル化する際に再検討。
- **P2-3 ✅** フォント見本: 初期 sample 24 字 / render 40 字 + overlay を w<=2000,h<=400 にクランプ
  (長文 sample × 多数フォントで巨大テクスチャ→ハングを防止)。
- **P3-1 ✅** 思考しっぽの装飾 base: geo.tail=None でも `resolve_tail_base` で base を解決。
- **P3-2 ✅** 泡グラデ: `draw_soap_bubble` を per-pixel (cov=0.10+0.85·t², 縁が濃い) に修正 +
  1px AA + 左上ハイライト。test `soap_bubble_is_denser_at_rim_than_center`。

## 完了 (2 セッション目、未コミット) — フォント絞り込み + A + B
- **フォント絞り込み ✅**: comic-core `LoadedFont::covers(ch)`。lab: 背景スレッドで全フォントを
  `classify_font_file`(throwaway parse、`covers('あ')&&covers('日')`→JP / `covers('A')`→Latin /
  else Other) → mpsc で `drain_font_scripts` が `font_script` に流し込み (UI 無凍結、結果で絞り込み
  が refine)。右パネル一覧は **日本語可のみ** (未分類は楽観表示)。見本ダイアログに **日本語/英語/
  すべて** カテゴリ radio。`add_font_file` も分類して挿入。
- **task A 編集ハンドル ✅**: comic-core で **bake 回転対応** — `bake_object` が rotation≠0 のとき
  `object_local_aabb` でサイズした temp buffer に unrotated bake → `rotate_blit`(双線形・premult)で
  pivot 周りに回転合成 (text/shape/tail/deco 一括回転、per-glyph 回転不要)。`bake_object_unrotated`
  に旧経路、`bubble_fill`/`bubble_stroke`/`bubble_decorations` を抽出。
  lab: `DragKind::Corner(idx)`/`Rotate` 追加。`bubble_handle_points`(回転後四隅+回転ノブ)、
  `draw_selection_handles` を回転 quad+四隅四角+回転ノブ+回転対応しっぽハンドルに刷新。四隅ドラッグ=
  local 軸へ逆回転して half-extents 設定+auto_size off、回転ドラッグ=rotation_rad 設定、しっぽ
  tip/base ドラッグは pointer を逆回転。`object_bounds` も点を回転して当たり判定を bake と一致。
  `set_bubble_half_extents`/`rotate_about`/`inv_rotate_about` helper。
  test `rotation_turns_wide_bubble_tall`。
- **task B 吹き出し結合 ✅ (回転対応済み)**: model `BubbleObject.merge_with_below`。raster は
  rotation 排除を撤廃し、**回転メンバーでも結合**するよう刷新:
  - `bake_into(overlay, obj, fonts, draw)` = 汎用回転ラッパ (rotation≈0 は直接 draw、回転時は temp
    に draw → `rotate_blit`)。`bake_object` = bake_into(全 part)。
  - `draw_bubble_parts(overlay, pivot, bubble, fonts, do_fill, do_stroke, do_decotext, opaque_fill)`
    で part 選択描画。
  - `bake_merge_group` は **塗り全 → 枠全 → 塗り全 → deco+text 全** の4パスを各メンバー `bake_into`
    経由で描く → 回転メンバーの fill/stroke/text が回転位置で合成され、union erase が成立。
  - `bake_overlay` の chain 化は rotation を問わず連続 bubble + 上側 merge_with_below で判定。
  - lab に「下の吹き出しと結合」チェック。test `merge_erases_interior_outline` /
    `merge_works_for_rotated_members`。
- comic-core **40 tests green / lab cargo check OK / fmt クリーン / `src/` 無傷**。

### Codex レビュー (2 セッション目バッチ、resume) — 全対応済み
P1 なし。指摘 5 件すべて修正し resume 再レビューで確認:
- **P2 ✅** merge の不透明前提: `bubble_fill(opaque)` 追加、merge 経路は opaque=true (半透明の二重
  塗り暗化を回避、内側を確実に消す)。
- **P2 ✅** フォント見本が UI スレッド負荷: `draw_font_dialog` で 1 フレーム最大 6 件のみ build、
  超過は `font_sample_cached` + `request_repaint` で次フレーム以降に分散。
- **P2 ✅** 回転思考円の AABB 過小: `object_bounds` を plain min/max + 明示回転に変更、円は中心を
  回転して r で拡張 (回転不変)。
- **P3 ✅** 回転装飾の temp AABB 過小: `object_local_aabb` の deco extent を `d.size*1.1 +
  outline_width` に。
- **P3 ✅** 追加フォント未分類: `add_font_file` で `font_script` に分類挿入。

## 完了 (3 セッション目、未コミット) — 右パネル再構成「基本常時＋詳細タブ」
ユーザー決定どおり **「基本常時＋詳細タブ」** + **カテゴリ色分けバー** で実装。`cargo check`
OK / comic-core 40 tests green / fmt クリーン / `src/` 無傷。

- **`PropTab { Serifu, Body, Tail, Deco }`** enum を追加 (`color()`=青#5AAAFF/緑#5FD08C/
  橙#FFA03C/金#FFD24B、`label()`)。`ComicLab.prop_tab` グローバル 1 個 (補正レイヤー同様)。
- **常時表示 (タブ外、上部)**: 種類ラベル → 本文(`draw_text_body`: TextEdit + 記法 ON 時の
  記号挿入ボタン) → フォント+サイズ(`draw_text_font`) → 文字プリセット + (吹き出しなら)形状プリセット。
- **詳細タブ (下部)**: `prop_tab_button`(カテゴリ色を常時帯びるボタン、選択=フル彩度+白枠) で
  4 タブ。選択中タブのみ `draw_section_bar`(左色帯、`local_adjust_lab` の `draw_panel_section`
  移植) でくるんで描画:
  - `セリフ`=`draw_serifu_tab`(自由関数): スタイル / 文字色 / 組方向 / 縦中横 / 記法(3セット選択) /
    行揃え / 行間字間 / 袋文字。**記号挿入ボタンは本文欄(常時表示)に移動**。
  - `本体`=`tab_body`(method): 自動サイズ + per-shape param / 結合 / 塗り / 枠 / 内側余白。
  - `しっぽ`=`tab_tail`(method): 表示 / 形式 / 先端 / 付け根自動 / 太さ。
  - `飾り`=`tab_deco`(method): 装飾レイヤー追加・各カード。
- **テキスト単体**は `prop_tab=Serifu` に強制し セリフ タブのみ描画 (本体/しっぽ/飾りは非表示)。
- **旧関数を分割**: `draw_text_section`→`draw_text_body`/`draw_text_font`/`draw_serifu_tab`、
  `draw_bubble_section`→`tab_body`/`tab_tail`/`tab_deco`。プリセットリンク解除ロジックは
  完全保存 (文字編集→`preset_link`解除、本体/しっぽ編集→`shape_preset_link`解除、装飾は dirty のみ)。

### Codex レビュー (3 セッション目バッチ) — A/B とも重大なし、P3 1 件修正
read-only。対象を「(A) 右パネル再構成 (comic_lab main.rs)」「(B) merge 回転対応 (raster.rs)」に
明示限定。結果:
- **A: 重大な指摘なし** — 分割でリンク解除ロジック・記号挿入カーソル復元・テキスト単体の
  セリフのみ描画がすべて保存されていると確認。
- **B: merge 回転ロジックに問題なし** — 4 パス erase / `sample_bilinear` の premultiply /
  回転 bubble を含む chain 化、いずれも正しいと確認。
- **P3 ✅ 修正** `object_local_aabb` の吹き出し内テキスト extent が `layout_text().bounds`
  だけで **テキストの袋文字(outline)幅を加味していなかった** → 回転時に temp buffer が太い
  袋文字のハロをクリップしうる。単体テキスト枝と同様に `bubble.text.outline.width_px` 分を
  pad して修正 (raster.rs `object_local_aabb` 吹き出し枝)。

## 完了 (4 セッション目、未コミット) — 右パネル微調整 + 装飾バグ修正
ユーザー指摘の UI 微調整 5 点 + 装飾重ねバグを修正。`cargo check` OK / fmt クリーン / `src/` 無傷。

- **フォント一覧をセリフタブへ移動**: 常時表示から外し `tab_serifu` 内に収納(`draw_text_font`
  を tab_serifu が呼ぶ。font_filter take/restore + ensure_font_loaded/open_font_dialog/add_font
  も tab_serifu が処理)。常時表示の本文欄は `draw_text_body` のみ残す。
- **プリセット名称をタブと合わせ + 色帯**: 文字プリセット→**セリフプリセット**(青帯)、
  形状プリセット→**本体プリセット**(緑帯)。`draw_section_bar` でくるんで常時表示部に残す
  (ユーザー選択 = プリセットは2種のまま, しっぽ/飾り専用プリセットは作らない)。status 文言も改名。
- **構造トグルをタブより上へ**: `draw_bubble_toggles`(新 method)で「下の吹き出しと結合」
  「しっぽを表示」「飾りを使う」をタブ上に常時表示。`tab_body` から結合、`tab_tail` から表示
  トグルを撤去。「飾りを使う」は新規(decorations 空=off、`deco_stash` で off→on 復元)。
- **しっぽ/飾りタブの disable**: `prop_tab_button` に `enabled` 引数追加。tail None で しっぽ、
  decorations 空で 飾り を `add_enabled(false)` でグレーアウト。選択中タブが無効化されたら
  Body へフォールバック。
- **装飾重ねバグ修正 ✅**: 原因 = 追加レイヤーが全て `DecorationLayer::default()`(seed=0・同配置)
  で `place_decorations` が決定的なため**完全に同位置へ重なり**1枚に見えていた。`tab_deco` の
  「装飾を追加」で `seed = max(既存seed)+1` を振って回避(全レイヤーは元々描画されていた)。
- model 変更なし(lab の `deco_stash` フィールド追加のみ)。`open_image` で deco_stash もクリア。

## Part2: メッセージウィンドウ — 調査完了・設計確定・実装未着手
ユーザー要望 = ドラクエ/FF 風ウィンドウ、ソシャゲ下部メッセージ(枠あり/なし)、ノベルゲー
ウィンドウを吹き出しと別タイプで追加。バックグラウンド調査エージェントで Ren'Py / RPG Maker /
TyranoScript / JRPG / コミックキャプションを調査し、**設計書 [../ui-input/message-window-design.md]
(../ui-input/message-window-design.md) に確定**。要点:
- 新 `AnnotationKind::MessageWindow(MessageWindowObject)`。`TextBlock`/`Rgba`/`StrokeStyle`/
  `round_rect`/`bake_into`(回転)/`fill_polygon`/`stroke_polygon` を流用。
- 軸: frame(None/SolidRounded/DoubleLine、9-slice は将来) × fill(None/Solid/Translucent/
  GradientScrim) × position(Top/Bottom/Center/Free) × size(FullWidth/Inset/AutoFitText) ×
  name plate × portrait × continue ▼。
- `pivot` は絶対座標(comic-core は画像サイズ非依存)。位置プリセット→pivot 変換は lab 側。
- 右パネルは既存4タブを読み替え(セリフ/枠(本体)/名前・立ち絵(橙)/飾り)。
- **唯一の非自明作業 = `bake_text` の矩形内揃えラッパ `bake_text_in_rect`** + `fill_scrim_rect`。
- v1 スコープ = 手続き枠 + 単色/半透明/スクリム + 位置プリセット + 名前プレート + ▼。
  9-slice 画像・実立ち絵画像・Beveled・ドロップシャドウは deferred。
- **Codex 突き合わせ済**: 同じ調査を Codex(gpt-5.5)にも独立依頼し、両報告の和集合を設計書
  **§4 統合機能チェックリスト**にまとめた。Codex が拾った主な gap = 本文ワードラップ/最大行
  ガイド・セーフエリア・inner content rect 区別・ローカルDim/線形グラデ・per-corner角丸・
  名前プレートのモード拡充・続き指標の種類・単純ドロップシャドウ v1 化・WindowStylePreset 新設・
  lab 統合実務(追加ダイアログサムネ/一覧ラベル)・前方互換 enum(choice/NVL/リッチテキスト等)。
- **着手前に決める点 (§4.3)**: ①本文の自動ワードラップを v1 に入れるか(layout.rs 折返し追加=
  非自明)/手動改行+ガイドのみか ②ドロップシャドウ v1 か後回しか ③AutoFitText/線形グラデ/
  per-side padding の v1 可否 ④タブ構成(橙=部品 読み替え) ⑤プリセット ~10 種。
- **§4.3 決定 (ユーザー、2026-06-04)**: ①**ワードラップは v1 必須**(日本語の禁則処理込み。
  「日本語に適したワードラップがないと微妙」)。②ドロップシャドウ含め **2〜3 も実装**(リリース
  時に既存ツール機能をなるべく全カバー方針)。③AutoFitText/線形グラデ/per-side padding も v1。
  ④タブは吹き出し流用の読み替えで OK。⑤プリセットは初期 ~10 種。

### v1 実装 ✅ 完了 (Step 1〜5、未コミット)
- **Step 1 日本語禁則ワードラップ** (comic-core `layout.rs`): `layout_text` 無改変 + 折返し版
  `layout_text_wrapped(block, font, wrap_main_axis: Option<f32>)` (横=最大行幅/縦=最大列高)。
  `wrap_line`(横)/`wrap_column`(縦) = greedy fill → **追い出し(push-out)禁則** →
  **Latin 単語整合 (最終調整)**。`is_line_start_prohibited`(行頭禁則)/`is_line_end_prohibited`
  (行末禁則)/`is_latin_word_char`。長すぎる英単語は分割せず1行オーバーフロー。
- **Step 2 model** (`model.rs`): `MessageWindowObject` + enum (`FrameStyle`/`FillMode`/
  `WindowPosition`/`SizeMode`/`VAnchor`/`NamePlateMode`/`PortraitSide`/`IndicatorKind`) +
  struct (`Insets`/`ShadowStyle`/`NamePlate`/`PortraitSlot`)。`AnnotationKind::MessageWindow`
  追加、text_block/コンストラクタ/全 match 拡張。`pivot` = 矩形中心の絶対座標。
- **Step 3 raster** (`raster.rs`): `draw_message_window_parts`(影→fill→立ち絵→枠→名前→本文→指標)、
  `bake_text_in_rect`(折返し+矩形内 align/v_anchor+クリップ)、`fill_polygon_shaded`(scrim/
  グラデ)、`effective_window_half_extents`(AutoFitText)、`draw_window_indicator`(▼等ポリゴン)、
  `object_local_aabb`/`bake_into` 回転対応枝。`bake_text` を `draw_layout_glyphs` に共通化(clip 付)。
- **Step 4 lab** (`main.rs`): 「ウィンドウ追加」/ `apply_window_placement`(位置→pivot 解決) /
  object_bounds・ハンドル(`window_handle_points`+汎用 `handle_points`)・Corner/Move ドラッグ・
  オブジェクト一覧ラベル / `draw_properties` 3分岐(吹き出し/テキスト/ウィンドウ) + ウィンドウ
  タブ(セリフ/枠/部品、飾りなし) `tab_window_body`/`tab_window_parts` / `WindowStylePreset`
  (system 10種 + user 保存) + `draw_window_preset_area`。
- **検証**: comic-core **51 tests green** (wrap 6 + window 4 + 既存) / `cargo check -p comic_lab` OK /
  fmt クリーン / `src/` 無傷。
- **Step 5 Codex レビュー** (3 round, P1 なし): R1=5件→R2=3件→R3=1件 すべて修正。主な修正:
  wrap の長英単語分割回避(kinsoku→単語整合の順序入替)、AutoFitText ウィンドウの位置固定時の
  再アンカー(`apply_window_placement` を全ウィンドウ編集後 + テキストプリセット適用/上書き後に
  呼ぶ、Free は早期 return)、AutoFitText の高さに名前プレート分/立ち絵 width+2*margin を加味
  (`name_plate_content_offset`)、回転 AABB に Above プレート全体 + stroke/halo slack、本文スタイル
  編集・move-to-Free で `style_preset_link` も解除。

### v1 仕上げ (ユーザー要望 6 点、未コミット)
1. **名前を右パネル上部へ**: 話者名は頻繁に変えるので `draw_window_name_header`(モード ComboBox +
   名前 TextEdit + 文字色)を本文の上に常時表示。`tab_window_parts` からは名前のモード/テキスト/色を
   撤去し、プレート装飾(サイズ/塗り/枠/角丸/余白/offset)のみ残す。
2. **オーバーフロー警告**: comic-core `message_window_overflows`(+ `window_content_rect` /
   `body_overflows` を draw と共有)。溢れたら lab が本文欄を赤枠 + 「(!) テキストが枠に収まって
   いません」表示。AutoFitText は対象外。
3. **続き指標の自動表示**: `MessageWindowObject.indicator_auto`。ON で「テキストが溢れた時だけ」▼等を
   bake(ゲーム的な"続きあり"挙動)。`WindowStylePreset` にも含める。
4. **フォント選択フロー刷新**: 詳細パネルの一覧を廃止 → 「フォントを選択(見本)」ボタンで見本ダイアログ
   のみ。ダイアログ右上に「フォントファイルを開く」を追加(ファイル選択は低頻度なのでそちらへ集約)。
   `font_filter`/`font_is_japanese`/`TextSectionResult.add_font`/`font_chosen` を削除。
5. **左パネル追加ボタン**: 「+」を外し 1 行 1 ボタンの全幅、吹き出し → ウィンドウ → テキストの順。
6. **ウィンドウ追加ダイアログ**: 吹き出し同様、`draw_add_window_dialog` で system プリセット10種を
   `paint_window_preview`(塗り/scrim/グラデ/枠/テキスト行)サムネで一覧表示 → クリックで適用追加。
- **Codex レビュー (このバッチ)**: P1 なし。P2×2 + P3 修正 = `body_overflows` を両軸チェック(折返し
  OFF の横長行クリップ検出)、`indicator_auto` を WindowStylePreset に追加、center scrim プレビュー対応。
- **検証**: comic-core **51 tests green** / lab コンパイル OK / fmt クリーン / `src/` 無傷。

### v1 仕上げ 2 (実機フィードバック、未コミット)
- **ウィンドウ追加ダイアログのレスポンシブ化**: 画面右端を超えていた → 幅を viewport に追従
  (`(content_rect.width()-24).clamp(170,560)`)+ `pivot(CENTER)` で中央配置 + `set_max_width` で
  サムネ grid を折返し。Codex P3 で floor を 280→170(1 列幅)に。
- **吹き出しの見栄え改善** (comic-core tessellate):
  - `fit_bubble_shape` に **アスペクト比クランプ**(MAX_ASPECT=1.8): 1 行縦書き等で極端な縦長/
    横長にならないよう短辺を広げる(拡大のみ=文字は必ず収まる)。test `fit_clamps_extreme_aspect`。
  - `bubble_geometry` に **しっぽ根本幅をふきだしの垂直方向 extent に比例して cap**
    (`effective_tail_base_width` = `0.85*perp_half`): 縦長ふきだしの下しっぽ=細く / 横しっぽ=太く。
    test `tail_base_width_capped_by_perp_extent`。
  - lab `default_bubble_tail`: tip +200→+150(短く)、width 48→32(細く)。
  - Codex P2(既存バグ)修正: Burst の auto-fit が高 jag で文字をはみ出す(fit の jag clamp が
    描画側 0.4..=0.75+jitter と不一致)→ fit を最小谷比 `(clamp(jag,0.4,0.75)-0.05)` で割るよう修正。
- **検証**: comic-core **53 tests green** / lab コンパイル OK / fmt クリーン。

### v1 仕上げ 3 (実機フィードバック、未コミット)
- **自動サイズを常時表示へ**: 「吹き出し自動サイズ」を 本体タブから出し、記号挿入の下(常時表示)に
  `draw_bubble_autosize_toggle` で配置。OFF 時の fit サイズ凍結・link 解除は維持。本体タブは
  スライダ gate のみ残す(チェックは撤去)。
- **プリセット登録/削除 UX 改修** (セリフ/本体/ウィンドウ 全て):
  - プリセットボタンの個別「×」を撤去(邪魔だった)。
  - 登録ボタンの横に「削除」ボタン: 名前欄と同名のユーザープリセットがある時だけ有効、押すと削除+欄クリア。
  - プリセット適用(クリック)時に名前欄へその名前を自動入力 → 同名で登録=更新 / 改名して登録=新規 /
    削除=その名前のを消す、という流れ。`apply_*_preset_by_index` で name 欄を prefill。
  - Codex P3 修正: 削除は同フレームのテキスト編集を考慮し、クリック直前に**現在の欄の値から再解決**。
- **検証**: comic-core 53 tests green / lab コンパイル OK(警告なし) / fmt クリーン。

### 縦書き約物の根本対応 — 設計確定・実装待ち（合意）
縦書きの `。、…ー` 括弧の崩れは `ab_glyph` が OpenType `vert` を適用できないのが原因。文字ごとの
例外表は漏れるため、**rustybuzz に TTB シェイプさせて縦字形(GID)＋縦メトリクスを得る**方式へ移行する
（Claude のリサーチ＋Codex レビューが独立に同結論）。設計と移行計画は
[../../vertical-text-opentype-plan.md](../../vertical-text-opentype-plan.md)（§7 が合意事項）。実機/回帰の golden は
[../../comic-lab-validation-checklist.md](../../comic-lab-validation-checklist.md)。
- 現状ツリーには Codex の per-char 暫定実装＋回帰テスト（56 tests green）が入っている。応急処置として
  有効。rustybuzz 導入時に**補正ロジックは撤去・テストは回帰ガードとして残す**。
- **決定(2026-06-04)**: 案C(skrifa)不採用、**最初から案B**（rustybuzz パース+シェイプ+アウトライン ＋
  `ab_glyph_rasterizer`／単一パーサ・ab_glyph 本体は外す）。`TextLayout` を char→GID+font_id+cluster+
  advance/offset 化する中規模リファクタ＋新規依存 rustybuzz/ab_glyph_rasterizer。着手は承認待ち。

### スタンプ（画像ステッカー）機能 — 仕様提案・要承認
カラー絵文字はフォント混植せず**画像スタンプ**として別建て（環境非依存・拡縮回転縁取り可・テスト容易・
クラッシュ面なし）。第4の `AnnotationKind::Stamp`。仕様は [stamp-feature-design.md](stamp-feature-design.md)。
要点: comic-core は decode/SVG せず**ラボが rasterize 済み RGBA を bake に渡す**／挿入はピッカー
ダイアログ(カテゴリ+検索+最近)＋右パネルで編集／四隅=一様スケール・回転ノブ・移動はハンドル流用／
同梱絵文字は SVG(resvg)推奨・ユーザー画像は image crate。**要確認**: ①SVG+resvg or 高解像度PNG
②絵文字セット(Twemoji CC-BY / Noto Apache / OpenMoji CC-BY-SA) ③一覧UI(ダイアログ推奨) ④ユーザー画像 v1可否。

### 残 (v1 仕上げ・任意)
- 実機での見た目調整(プリセット色味・余白)。ユーザー確認後に微調整。
- deferred 項目 (§4.2 後回しタグ): 9-slice 画像 / 実立ち絵画像 / Beveled枠 / ぼかし影 /
  per-corner 角丸 / choice・NVL複数エントリ / リッチテキスト 等。
- 本体 mImageViewer 統合 (lab 確定後)。

## 残タスク (任意・後回し)
- **真の Boolean union**: 現 merge は不透明塗り前提の軽量方式。半透明・異色の合成や、union 外周を
  1 本の輪郭線として描く正式版は将来課題 (Codex 設計 §4.8 の「将来」)。
- **回転 × しっぽ**: bake は object 全体 (しっぽ含む) を回転する。しっぽを「相手方向固定 (回転しない)」
  にしたい要望が出たら別途設計。
- **装飾はみ出しの hit-test**: 外側スパークル等は object_bounds 未包含 (body+tail+thought は包含済)。
- **本体 mImageViewer への統合** (大きな残り): lab はプロトタイプ。機能はほぼ出揃ったので、次の山は
  comic-core を本体パイプライン(隠蔽加工後・crop 前の最前面オーバーレイ)に組み込み、フルスクリーン
  編集UI・永続化・キャッシュ・エクスポートを繋ぐ作業 (下「本体統合メモ」)。

## レビュー状況メモ
- merge 回転対応 (`bake_into`/`draw_bubble_parts`/`bake_merge_group` 4パス) + 右パネル再構成は
  **3 セッション目バッチで Codex レビュー済** (上記「Codex レビュー (3 セッション目バッチ)」)。
  A/B とも重大なし、P3 1 件 (`object_local_aabb` テキスト袋文字 pad) 修正済。
  これで未レビューの作業残なし (全 landed 分レビュー完了)。

## 確定済みの設計判断
- マーカー: 先頭=縦中横・2番目=横倒し。既定 `[]{}`、選択3セット。正立は廃止(数字/!? の per-run 正立は
  「縦中横トグルOFF」で代替)。
- プリセットのリンクは**明示リンクID方式**(値一致でなく)。
- フォント追加はファイル選択でなく**システム列挙**が主、サンプルダイアログで選ぶのが理想。
- 自動サイズ・付け根自動は既定ON、手動操作で解除。
- リッチテキストは使わず**記号で囲む**方式(本体統合時も同方針)。

## 本体統合メモ
- Codex が本体パイプライン再構成中(`docs/archive/editing/local-adjust-pipeline-refactor-plan.md`、edit pipeline /
  edit_result+final_composite の2段キャッシュ)。吹き出しは「AI/色補正の影響を受けない最前面
  オーバーレイ」として最終段合成の位置づけ。本体統合は lab 確定後。
