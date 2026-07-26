# 縦書きの OpenType 対応 — 調査結果と設計

Status: **実装済み（案B）** — 2026-06-04
作成: 2026-06-04 セッション

対象: `crates/comic-core`（縦書きテキストの字形・配置）。

## 実装サマリ（2026-06-04）

案B を実装完了。`crates/comic-core` の字形/ラスタ層を全面的に置換した:

- **`font.rs`**: パーサを `ab_glyph` → `rustybuzz`（+ 再エクスポートの `ttf_parser`）に統一。
  `LoadedFont` は raw バイトを保持し、必要時に `rustybuzz::Face::from_slice` を生成。
  - `shape_run(text, size, vertical)` / `vertical_glyph(ch, size)`: `Direction::TopToBottom`
    でシェイプ → フォントの `vert`（無ければ rustybuzz の UAX#50 フォールバック）が自動適用され、
    `ShapedGlyph { gid, x_offset, y_offset, x_advance, y_advance }`（px）を返す。
  - `rasterize_gid(gid, size, dilate)`: `ttf_parser` のアウトラインを `OutlineCollector` で集め、
    `ab_glyph_rasterizer` の `Rasterizer`（draw_line/quad/cubic + for_each_pixel）でカバレッジ化。
    袋文字 dilation は `dilate_coverage`（8 近傍 max 拡張）に切り出し。
- **`layout.rs`**: `Cluster::Single` の縦組み配置を、文字ごとの置換表ではなく
  `vertical_glyph` のシェイプ結果で行う。セルは従来どおり一様 `glyph_step`（漫画組み）、
  シェイプの origin（= 縦の vertical origin）をセル上端中央に置き、`x_offset`/`y_offset`
  でセル内位置を決める（rustybuzz の vertical origin 規約: `x_offset -= h_advance/2`,
  `y_offset -= v_origin`、y は font 系 y-up なので layout の y-down に符号反転）。
  - 削除: `vertical_substitute`（Unicode 縦書き表示形の手書き表）/ `is_vertical_corner_punct`
    / `is_vertical_rotate` / `vertical_corner_position` / `GlyphForm::RotateCw`。
  - `GlyphPlacement` は `glyph_ch: char` → `glyph_id: u16`（シェイプ済み GID）に変更。
    `GlyphForm` は `Upright` / `Sideways`（明示 横倒し）のみ。
- **`raster.rs`**: `font.rasterize(glyph_ch)` → `font.rasterize_gid(glyph_id)`。
- **回帰テスト（実フォント = Yu Gothic Medium を読む）**: `。`/`、` が右上、`…`/`「」` が
  縦書き表示形（横 cmap GID と異なる）に置換されることを assert。ゴールデン列
  （`「……。」` / `えっ!?` / `びっくり!!!!` / `あー、そう……？` / `（テスト）【重要】《確認》` /
  `小さいっゃゅょ、ゎ。` / `2026年6月4日`）が縦書きで panic なく焼けることを smoke。計 58 tests green。
- **依存変更**: `comic-core/Cargo.toml` から `ab_glyph` を削除、`rustybuzz` + `ab_glyph_rasterizer` を追加。

### 書記素 (grapheme) 単位のセル化 — 結合文字 / IVS 対応 (2026-06-04 追補)

当初の「1 セル 1 スカラー」実装では、**結合文字列 (分解形 `か`+U+3099) や IVS
(異体字セレクタ、例 人名漢字 `辻`+U+E0100) が別セルに割れ**、異体字も反映されず
既定字形 + 空セルになっていた (Codex レビュー P2 指摘)。人名・地名を縦書きで入れると
壊れる現実的なケースなので対応した。

- **セル区切りを「スカラー」から「書記素」へ**: `is_grapheme_extender(c)` (結合マーク +
  異体字セレクタの範囲判定。フル Unicode crate 非依存) で、基底文字に後続の extender を
  くっつけて 1 セル (`Cluster::Grapheme(Vec<char>)`) にまとめる。`push_upright_cells` が
  auto / Upright 経路の両方で grouping する。約物・縦中横・横倒しの判定は従来どおり。
- **グリフ選択はシェイパー任せ**: `place_upright_grapheme` が書記素の文字列を **まとめて**
  TTB シェイプ (`shape_run`) する。これで `vert` + **cmap format 14 (UVS = IVS 異体字選択)**
  + 結合マークの GPOS 配置がフォント側で適用される。**自前で IVS→グリフ対応表は持たない**
  (役割分担: 自前=セル grouping の小さな判定 / シェイパー+フォント=実際の字形選択)。
- **1 スカラーのセルは従来と完全同一**: `Cluster::Single(char)` は `place_upright_grapheme`
  に 1 文字スライスを渡すだけで、累積描画ループが 1 グリフ時に旧 `vertical_glyph` 経路と
  ビット等価な配置になる (golden 回帰テストはそのまま green)。
- 回帰テスト: `辻`+IVS / `か`+U+3099 が **1 セル** に収まる (列高 = 単一文字と同じ) ことを
  実フォントで assert。フォントが当該異体字を持たない場合も「基底字形 + 空セル無し」に
  改善される (悪化はしない)。
- 残課題 (低優先): フル UAX#29 grapheme 分割ではなく extender 範囲ベースなので、
  ZWJ 連結列などはカバーしない (絵文字 = スタンプ扱いなので実害なし)。

### perf: 解析済み Face のキャッシュ (2026-06-04 追補) ⚠

`LoadedFont` は当初バイト列だけを保持し、`face()` が **呼び出しごとに**
`rustybuzz::Face::from_slice`(`.ttc` のテーブルディレクトリ解析)を再実行していた。実測で
h_advance ≈46µs/回、bake ≈6.4ms/回 (release) と重く、ドラッグ中の再 bake で「操作が重い」
主因になっていた。`self_cell` で **バイト列 + 解析済み `rustybuzz::Face` を 1 度だけ保持**
(自己参照を安全に)するよう変更し、metric/shape/raster 呼び出しから解析コストを除去
(h_advance ≈1000×、shape/rasterize ≈20-28×、bake ≈2× 高速化)。`shape_run`/`rasterize_gid`/
`glyph_*` はキャッシュ済み `&Face` を使う。依存に `self_cell` を追加。

### 縦中横: 全角サイズ + 列幅可変 (2026-06-04 追補)

当初は縦中横ランを `size*0.5` に縮小してセル幅に収めていた。実機要望で
**本文と同じフォントサイズのまま横並び**にし、縮小しない方式へ変更。列幅を
`max(cell, 縦中横ラン幅)` (= `cluster_width`) とし、列を右→左へ**可変幅でパッキング**
(`col_left`/`widths`/`slots`)。桁数が多くセル幅を超える縦中横は**その列の左右間隔が広がって**
隣の列と重ならない (縮小ではなく行間で吸収)。縦中横はレイアウト機能なので、この変更は
シェイパーではなく `layout.rs` の列組みに閉じる (§4 の方針どおり)。

### その他の制約

- 縦アドバンス (`y_advance`/VORG) は **セル内配置にのみ** 使い、セル送り自体は一様
  `glyph_step` のまま (漫画組みの意図的選択。プロポーショナルな縦組みにはしていない)。

## 決定（2026-06-04・ユーザー承認）
- **採用 = 案B**（最初から）: `rustybuzz` でパース＋TTB シェイプ＋アウトライン取得し、
  カバレッジ生成は **`ab_glyph_rasterizer`**（ab_glyph 本体の rasterizer のみ切り出した crate）に流す。
  **フォントパーサは rustybuzz ただ1つ**（2×メモリなし）、AA 品質は ab_glyph 同等、袋文字/`rotate_cw`/
  `rotate_blit`/blend は coverage バッファ上の処理なので無改変で流用。`ab_glyph` 本体は外す。
- **案C（skrifa フルスタック）は不採用**: 工数大に加え、ラスタ再検証/視覚回帰、カラー塗りの自前実装
  （COLRv1 のミニレンダラ、PNG デコード、OT-SVG は resvg 別途）、harfrust 成熟度 vs 二重パーサの
  ジレンマ、**カラー絵文字のテストが困難（フォント版依存ゴールデン）**、COLRv1 塗りのクラッシュ面
  （循環 paint→stack overflow 等）が見合わない。
- **絵文字は案C で取り込まず、画像スタンプ機能として別建て**（環境非依存で一定・拡縮/回転/縁取り可・
  決定的でテストしやすく・未知フォント塗りのリスク無し）。仕様は
  [archive/comic/stamp-feature-design.md](archive/comic/stamp-feature-design.md)。

## 0. 結論（先に）

- 縦書きで `。、…ー` や括弧の位置/向きが崩れるのは、**`ab_glyph` が OpenType の縦組み機能
  （GSUB `vert`）を適用できず、横組み字形をそのまま中央に置く**ため。文字ごとの場当たり対応
  （Unicode 縦書き表示形への置換・回転・右上寄せ）は**フォント依存で必ず漏れる**。
- **正攻法 = シェイパーを通す**こと。**`rustybuzz`（HarfBuzz の純 Rust 移植）に縦方向(TTB)で
  シェイプさせる**と、フォントの `vert` が自動適用され、**正しい縦組み字形(GID)＋縦アドバンス**
  が得られる。これが Typst・Koharu(漫画翻訳ツール) など実在 Rust プロジェクトの採用形。
- **推奨アーキテクチャ = (b)**: `rustybuzz` でシェイプ（縦組み字形・縦メトリクス取得）＋
  **既存の段組み/縦中横/横倒しレイアウトは維持**＋ラスタライズは当面 `ab_glyph` を GID 指定で流用。
- **縦中横はレイアウト機能**(JLREQ / JIS X 4051 §4.8 / CSS `text-combine-upright`)であり
  フォント機能ではない → **これはレイアウトエンジン側に残す**（シェイパーに移さない）。

> ※ 出典の検証フェーズはリサーチハーネスのバグ（検証エージェントが StructuredOutput 未呼出で
> 全 abstain → 機械的に "killed"）で空振りしたが、収集 claim 自体は下記の一次資料に紐づく。

---

## 1. 必要な OpenType テーブル/機能（仕様）

| 機能/テーブル | 役割 | 出典 |
|---|---|---|
| **GSUB `vert`** | 縦組み字形への置換（`。、`を右上へ、`ー`/括弧/ダッシュを回転、`…`を縦3点へ）。**横→縦の代替字形**を選ぶ標準機能。多くは 1:1 single-sub。HarfBuzz は**縦方向のとき既定で自動適用**。 | MS spec, harfbuzz.github.io, Adobe "Tale of Three Features" |
| GSUB `vrt2` | 旧来の「縦回転」機能。字形の向き判断をフォント側に委ねる。**CSS は `vrt2` を使わず**、向きは UAX#50 で自前決定。Adobe も `vert` 推奨。 | W3C CSS Writing Modes, ccjktype(Adobe) |
| **`vmtx` + `vhea`** | 縦書きの**字送り(advance height)＋上サイドベアリング**。縦に積む仕組み。 | MS spec/vmtx |
| **`VORG`** | 縦書き原点。CFF（bbox 無し）では特に必要。 | MS spec/vmtx |
| `BASE` | ベースライン。縦書きで synthesize 可。 | W3C |
| GPOS `vkrn`/`vpal`/`vhal` | 縦書きのカーニング/位置調整（あれば）。 | MS spec |
| cmap × `vert` | cmap で基底 GID → `vert` GSUB で縦字形 GID に置換。 | MS spec |
| **UAX#50 Vertical_Orientation (R/U/Tu/Tr)** | フォントに縦字形が無いときの**回転/正立フォールバック**。CSS `text-orientation: mixed` の判定根拠。R=90°時計回り回転 / U・Tu・Tr=正立。 | unicode.org/reports/tr50, W3C |
| JLREQ / JIS X 4051 | 縦書きの**視覚要件(WHAT)**を規定。実装機構(字形置換)は範囲外＝シェイパー/フォント層の責務。**縦中横は §4.8 のレイアウト規則**(フォント機能ではない)。 | w3.org/TR/jlreq |

要点: 「縦書き用フォント」というより、**通常の日本語フォントが `vert`/`vmtx`/`VORG` で縦組み
字形・縦メトリクスを内包**している。`vert` を適用して縦字形 GID を取り、`vmtx`/`VORG` で積む、
が正しい流れ。

---

## 2. Rust エコシステム（2024–2026 現況）

| crate | 縦書き対応 | GID 直ラスタライズ | 位置づけ | 出典 |
|---|---|---|---|---|
| **rustybuzz** | ◎ TTB 方向でシェイプ→`vert` 自動適用、縦アドバンス返す。HarfBuzz テスト 2221/2252(~98.6%) パス。`Face` が `Deref<ttf_parser::Face>`→`outline_glyph(gid,builder)`/`glyph_ver_advance`(vmtx)/`glyph_y_origin`(VORG) を露出。**パーサ＋シェイパー＋縦メトリクスが1つ**。 | ○(`outline_glyph` + 自前ラスタ) | **本命**。純 Rust、ttf-parser ベース。 | github harfbuzz/rustybuzz, docs.rs Face |
| harfrust | ◎ HarfBuzz v13 パリティ目標、rustybuzz から fork し parsing を read-fonts(fontations) に移行。 | ○(skrifa と併用) | rustybuzz の後継候補。 | github harfbuzz/harfrust |
| fontations(`read-fonts`/`skrifa`) | read-fonts は GSUB/vmtx/VORG/vhea/BASE/GPOS を**読む**(シェイプはしない)。skrifa は GID→アウトライン。 | ◎(skrifa) | ab_glyph 置換候補。harfrust と組む。 | github googlefonts/fontations |
| ttf-parser | 低レベル table 読取のみ(シェイプ無)。rustybuzz の土台。 | ○ | rustybuzz 経由で間接利用。 | — |
| ab_glyph | アウトライン・ラスタライズのみ(シェイプ無、`vert` 不可)。**現状の問題の根**。 | ◎(GID 可: `Glyph{id}`) | ラスタライザとして当面流用可。 | — |
| cosmic-text | 縦書きは弱い/未対応(横書き前提)。 | — | 不採用。 | — |
| swash | 機能適用可だが**縦書きレイアウトは呼び出し側責務**。 | ◎ | 単体では縦書き解決にならない。 | — |
| allsorts | 横書き志向。 | — | 不採用。 | — |

**実在の参照実装**:
- **Typst** PR #7399: `unimplemented!("vertical text layout")` を廃し、**rustybuzz の方向を TTB に設定**
  して縦書き日本語を実装。縦アドバンスは rustybuzz の `y_advance` ＋ ttf-parser `glyph_ver_advance`
  (vmtx) フォールバック。
- **Koharu**(Rust 漫画翻訳・組版): まさに comic-core と同じ用途。**`vert`+`vrt2` を有効にして TTB
  シェイプ**(harfrust)し、**アウトラインは skrifa で別途ラスタライズ**(fallback fontdue)。シェイプと
  ラスタライズを分離。
- WebKit `ComplexTextControllerHarfBuzz.cpp`、HarfBuzz discussions #3294（"TTB に設定すれば
  HarfBuzz が縦位置を処理、手動の origin/advance 調整は不要"）。

---

## 3. ab_glyph 併用 vs フルスタック移行（Q3）

- rustybuzz の `Face` は `Deref<Target = ttf_parser::Face>` なので、**シェイプした GID をその場で
  `outline_glyph(gid, &mut OutlineBuilder)` でアウトライン化**でき、原理上は「2 つの別パーサ」では
  なく**1 パーサの上にシェイパが乗る**形。縦メトリクス(`glyph_ver_advance`/`glyph_y_origin`)も同 Face から取れる。
- 一方 **ab_glyph も GID 指定でラスタライズ可**(`ab_glyph::Glyph{ id: GlyphId, .. }`)。同一フォントなら
  GID は両パーサで一致するので、**rustybuzz でシェイプ→GID を ab_glyph でラスタライズ**しても破綻しない。
  唯一の難点は同じバイト列を 2 回パースするコスト(数 MB×フォント数、lab では許容)。
- **推奨**: まず **(i) ab_glyph ラスタライズ流用**(既存 `rasterize`/袋文字/`rotate_cw` を GID 入力に拡張)で
  着手し、将来 **(ii) rustybuzz/skrifa アウトラインに一本化**して ab_glyph を外す(任意)。

実装上の注意(ライフタイム):
- rustybuzz `Face::from_slice(&bytes, index)` は**バイト列を借用**する。現 `LoadedFont` は ab_glyph
  `FontVec` がバイトを内部所有。→ `LoadedFont` に **元バイト列 `Vec<u8>` を保持**し、シェイプ時に
  `rustybuzz::Face::from_slice` を**レイアウト1回につき1度**生成して縦レイアウトに渡す(自己参照を回避)。
  ab_glyph FontVec は別途バイトのコピーを持つ(フォント数個なら 2×バイトのメモリ増は許容)。

---

## 4. 縦中横は「レイアウト機能」（Q4・確認）

- JLREQ / JIS X 4051 §4.8、CSS `text-combine-upright` が示すとおり、**縦中横はフォント機能ではなく
  組版規則**。→ **comic-core のレイアウトエンジンに残す**（シェイパーには載せない）。
- ベストプラクティス: 2〜3 桁の数字・`!?` 等の短い横並びを 1 文字分の縦位置に詰める。サイズは
  「列幅に合わせて縮小」or「同サイズで列間を広げる」のいずれか（**前回ユーザー要望=同サイズ＋列間
  自動調整**は別タスクとして本設計の上に実装する）。横倒し(縦に寝かせる)も同様にレイアウト側。

---

## 5. 推奨アーキテクチャと責務分担

**採用: (b) rustybuzz シェイプ ＋ 既存レイアウト維持 ＋ ab_glyph ラスタライズ流用。**

責務:
- **シェイパー(rustybuzz)**: 1 列の「縦に積む通常文(upright run)」を **TTB でシェイプ** →
  `vert` 適用済みの **縦字形 GID 列 ＋ 縦アドバンス(y_advance / vmtx)** を返す。句読点・括弧・長音・
  三点リーダの位置/向きは**フォントの縦字形がそのまま正解**になる（per-char 表は不要）。
- **レイアウトエンジン(自前・維持)**: 列の右→左配置、列の禁則折返し、**縦中横**(横並び詰め)、
  **横倒し**(マーカー)、行揃え、最終的な `GlyphPlacement` 生成。各 upright run をシェイパーに渡し、
  返った GID＋y_advance で積む。縦中横クラスタは横方向(LTR)で別途シェイプして 1 セルに収める。
- **ラスタライザ(ab_glyph 流用→将来一本化)**: `GlyphPlacement` の **GID** を描画（袋文字・回転は現行流用）。

UAX#50 フォールバック: フォントに縦字形が無い文字は、rustybuzz が回転を埋めない場合に限り
**UAX#50 の R クラスを 90°回転**で補う（最後の保険。基本はフォント任せ）。

---

## 6. 段階的移行計画

1. **依存追加**: `rustybuzz`。`LoadedFont` に元バイト列を保持（rustybuzz Face 生成用）。
2. **GlyphPlacement を GID 化**: `glyph_id: GlyphId` を持たせ、`ch` は診断/テスト用に残す
   （現 stopgap の `glyph_ch`(char)→ `glyph_id`(GID) へ置換）。
3. **font.rs**: `shape_vertical_run(face, chars, size) -> Vec<ShapedGlyph{gid, ch, y_advance, x_off, y_off}>`
   と `shape_horizontal_run(...)`（縦中横/横書き用）を追加。ラスタライズは GID 入力に拡張。
4. **layout.rs(縦書き)**: 各列を run(通常/縦中横/横倒し)に分割。通常 run は rustybuzz TTB シェイプ→
   y_advance で積む。縦中横は LTR シェイプ→1 セルに配置。横倒しは現行 `rotate_cw`。
5. **raster.rs**: GID 指定で描画（`draw_layout_glyphs` を GID ベースに）。
6. **stopgap 撤去**: `vertical_substitute`/`is_vertical_corner_punct`/`is_vertical_rotate`/
   `vertical_corner_position` と presentation-form ベースのテストを、シェイピングベースの検証に置換。
7. **(任意・後)**: 横書きも rustybuzz(LTR) でシェイプしてカーニング/合字を正しく。最終的に
   ab_glyph→skrifa 一本化を検討。

工数感: font/layout/raster の glyph identity を GID へ寄せる中規模リファクタ＋新規依存 1 つ。
ただし**場当たり対応の漏れを根絶する正しい基盤**。

## 7. Codex レビューとの統合（合意事項・2026-06-04）

Codex も独立に**同じ結論**（文字ごとの例外表を増やさず、縦書き text shaping の段を作る＝
rustybuzz ベース）に到達。両者の合意として、§5/§6 に以下を確定追記する:

1. **`TextLayout` の単位を `char` から `glyph_id + font_id + cluster range + advance/offset` へ**。
   font_id を持たせるのはフォントフォールバック（複数 face）を将来扱うため。現 stopgap の
   `glyph_ch`(char) はこの `glyph_id`(GID) へ置換する。
2. **縦書き本文 run = `Direction::TopToBottom` / language `ja` / script は Unicode script に従う**で
   shape。`vert` / vertical metrics / `y_advance` を**シェイピング結果として受け取り**、`。`/`…` を
   手で動かさない（per-char 補正は撤去）。
3. **Latin / 長い英単語 = CSS `text-orientation: mixed` 相当**: 横組みで shape して **run ごと 90°
   回転**（横倒し経路を流用）。**縦中横は別 run** として横組み shape し 1em セル内に収める
   （縦中横はレイアウト機能なので維持）。
4. **`vrt2` は既定で使わない**。CSS Writing Modes と同様、向き判断はレイアウト側（CJK upright +
   horizontal-only script は sideways + `vert`）。`vrt2` は調査対象に留める。
5. **ラスタライザ**: 第一候補 `rustybuzz + glyph-id rasterizer`（ab_glyph を GID 指定に拡張）。
   将来 **`swash` の raster/cache** まで使うと自然。**`cosmic-text` は丸ごと任せない**（縦組み
   column layout は comic-core 側で持ち、fallback / raster cache の部品としてのみ評価）。
6. **stopgap の文字別補正は撤去対象だが、テストケースは残す**（shaping 実装が同じ約物崩れを
   再発させないかの回帰ガード）。

### golden 検証セット（JLREQ ベース、回帰テスト/実機チェック共通）
`「……。」` / `あー、そう……？` / `（テスト）《確認》` / `2026年` / `{LOVE}` / `びっくり!!!!` /
`えっ!?` / `小さいっゃゅょ、ゎ。`。実機チェックは
[comic-lab-validation-checklist.md](comic-lab-validation-checklist.md) §2 に集約済み。

### 関連する別 P0/P1（Codex 指摘、チェックリストに記載済み）
- **フォントフォールバックが弱い**（`ab_glyph` 単一 face、未対応 glyph は `.notdef`）。本設計の
  font_id/フォールバック方針とあわせて将来対応。→ checklist §1 P1 / §8 P1。
- **本体統合時「表示には出るが Ctrl+E / キャプチャ保存で消える」経路差分**。統合時 P0。
  → checklist §1 P1 / §11。

## 現状の暫定実装について
リサーチ中に Codex がツリーへ per-char 暫定実装＋回帰テストを入れた（`GlyphForm`/`glyph_ch`/
`vertical_substitute`＝Unicode 縦書き表示形 ︙﹁﹂… への置換＋句読点右上配置＋回転 fallback、
[layout.rs](../crates/comic-core/src/layout.rs) / [raster.rs](../crates/comic-core/src/raster.rs)、
56 tests green）。**症状を止める応急処置**として有効だが、フォント依存で漏れるため本設計
(rustybuzz)導入時に**補正ロジックは撤去・テストは回帰ガードとして残す**（合意 §7-6）。

## 参照ソース
- MS OpenType spec (vmtx 他): learn.microsoft.com/typography/opentype/spec/vmtx, /features_uz
- HarfBuzz: harfbuzz.github.io/shaping-opentype-features.html, discussions #3294, issues #573
- rustybuzz: github.com/harfbuzz/rustybuzz, docs.rs/rustybuzz Face
- harfrust / fontations: github.com/harfbuzz/harfrust, github.com/googlefonts/fontations
- Typst PR #7399 (vertical Japanese via rustybuzz TTB)
- Koharu: koharu.rs/explanation/text-rendering-and-vertical-cjk-layout/
- W3C: css-writing-modes-3, jlreq；Unicode UAX#50 (tr50)；Adobe ccjktype "Tale of Three Features"
- WebKit ComplexTextControllerHarfBuzz.cpp
