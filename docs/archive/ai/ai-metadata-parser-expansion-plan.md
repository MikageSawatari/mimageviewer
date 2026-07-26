# AI メタデータパーサ拡充 実装計画

ステータス: **v1.3.0 実装済み** / 起案 2026-06-11 / 実装 2026-06-11

## 実装メモ (2026-06-11)

- `src/png_metadata.rs` に `AiToolKind` / `MetadataOrigin` / `consumed_keys` を導入し、
  メタデータパネル、Ctrl+F、Ctrl+G の 3 経路で同じ判別結果を使う形にした。
- `parameters` が JSON の場合は汎用 JSON より AI 生成メタデータのリーダーを優先し、
  未知の生成 JSON は生 JSON を prompt として扱わない。
- NovelAI / InvokeAI (現行 + 旧 `sd-metadata` / `Dream`) / SwarmUI / Fooocus 系 /
  RuinedFooocus / A1111 互換平文 / Midjourney 相当の正規化に対応した。
- JPEG/JFIF の EXIF UserComment を AI 生成メタデータとして扱い、AI として認識できた
  UserComment は EXIF 検索テキストから除外して二重取り込みと Negative 混入を避ける。
- INDEX_VERSION は 9 に更新済み。更新後の初回起動時にアイテム検索索引が再構築される。
- 実サンプルはリポジトリに入れず、`H:\home\mimageviewer_old\testimage\metadata` に
  配置する (他のテスト画像置き場と同じツリー)。再取得用スクリプトは
  `scripts/collect-ai-metadata-samples.ps1` (既定出力先も同パス)。スモークテストは
  `MIV_AI_METADATA_SAMPLE_DIR` にこのパスを設定して
  `cargo test --bin mimageviewer-core optional_external_` で実行する。
- 実サンプルの出典 (2026-06-11 追加): d3x-at/sd-parsers (MIT) の
  tests/resources — A1111 (PNG/JPEG EXIF/stealth)、Fooocus (`fooocus_scheme` 付き
  parameters JSON)、InvokeAI 3 形式 (Dream / sd-metadata / invokeai_metadata)、
  NovelAI v3、zTXt parameters が IDAT より後ろにあるエッジケース。
- 追加実装 (2026-06-11、サンプル/リーダー実装の裏取り後):
  - **EasyDiffusion**: sdkit `save_dicts(output_format="embed")` がフィールドごとに
    独立 tEXt チャンクを書く形式 (キーは内部名/表示名の 2 系統)。判別は
    receyuki / DiffusionToolkit と同じ negative_prompt チャンクの存在。
  - **Fooocus-MRE / 新 Fooocus の Comment JSON**: `Comment` チャンクに JSON を書く
    系列 (DiffusionToolkit は非 NovelAI の Comment JSON を MRE と判別)。NovelAI
    判定の後に Comment JSON → Fooocus 系の分岐を追加し、未対応だった negative
    混入経路を塞いだ。`real_prompt` / `real_negative_prompt` (MRE) も処理。
  - **旧 sd-metadata の配列 prompt**: `image.prompt` が `[{"prompt": ...}]` 配列の
    場合に要素の prompt のみ抽出し、Dream 同様 `[..]` を negative として分離。
- サンプル未入手のまま残るのは stealth pnginfo (アルファ LSB、スコープ外。
  ただし stealth 画像をテキストチャンク無しとして誤検出しないことはスモークで確認)。

## 0. 背景と目的

2026-06 の競合分析 (deep-research) で以下が確定した:

- AI 生成画像整理ニッチの直接競合 **DiffusionToolkit** (無料/MIT/Windows) は
  A1111 系 (Tensor.Art / SDNext 含む)・InvokeAI・**NovelAI**・EasyDiffusion・
  Fooocus 系・Stable Swarm のメタデータを解釈でき、生成ツール形式のカバレッジで
  mIV を上回る (mIV 優位は ComfyUI と「閲覧+編集+検索の一体性」)。
- mIV が製品ページ・マニュアルで公称している **NovelAI プロンプト表示は実装に
  存在しない** ([src/png_metadata.rs](../src/png_metadata.rs) のパーサは
  A1111/Forge・ComfyUI・Midjourney のみ)。

本計画は (1) 公称と実装の乖離解消、(2) 生成ツール形式カバレッジで
DiffusionToolkit と同等以上に並ぶこと、(3) 現状パーサの**検索品質バグ
(Negative prompt の索引混入)** の構造的解消、を目的とする。

基本アプローチはユーザー方針どおり「**フォーマットごとの抽出器 (テキスト変換部)
を足し、共通の正規化構造に流し込む**」。表示・検索への伝搬は既存経路が
共通化されているため、抽出器の追加だけで全経路に波及する (§1)。

## 1. 現状実装の整理 (変更対象の全体像)

AI メタデータは [src/png_metadata.rs](../src/png_metadata.rs) に集約されており、
**全 3 利用経路が同じ `detect_and_parse()` を通る**。抽出器を足せば 3 経路すべてに
自動で効く:

| 経路 | 入口 | 用途 |
| --- | --- | --- |
| フルスクリーン メタデータパネル | `app.rs` の metadata load worker (`extract_metadata` / ZIP 内は `extract_metadata_from_bytes`) | `ui_metadata_panel.rs` の `draw_a1111_panel` / `draw_comfyui_panel` / `draw_unknown_panel` |
| Ctrl+F (現在地フィルタ) | `app.rs` → `build_searchable_from_path` | その場検索のマッチ対象テキスト |
| Ctrl+G 全文索引 | `ingest_text.rs` → `build_searchable_from_bytes` → `png_prompt` フィールド | Tantivy ingest (bytes 一回読みで XMP/PNG/dc:subject を共有) |

現在の判別 (`detect_and_parse`、上から順):

1. `prompt` キーが JSON object → **ComfyUI**
2. `parameters` キーあり → **A1111/Forge** (`parse_a1111`、任意の非空テキストで成功する)
3. `Description` キーあり → `parse_a1111` を試し、ダメなら **Unknown**  (Midjourney 用)
4. 残りの非標準チャンク → **Unknown** / なければ None

検索テキスト構築 (`build_searchable_from_chunks`) は「認識済みフォーマットなら
Negative prompt を除外、非 AI チャンク (Author/Comment 等) は素通しで追加」という
設計で、**静的リスト `AI_METADATA_KEYS` に起点キーを足し忘れると Negative が
検索に混入する**という既知の設計弱点がある (ファイル冒頭コメントに自己言及あり)。

## 2. 現状の既知不具合 (本計画で同時に直す)

新フォーマット追加は単なる機能追加ではなく、以下の実バグ修正を兼ねる:

1. **NovelAI の誤判別と Negative 混入**: NovelAI PNG は `Description` (正プロンプト
   平文) + `Comment` (JSON、`uc` = Negative 含む) + `Software`="NovelAI" を持つ。
   現状は判別 3 で「A1111 扱い (prompt=Description)」になり、さらに `Comment` が
   非 AI チャンクとして素通しされるため **`uc` (Negative) が Ctrl+F / Ctrl+G の
   検索対象に混入している**。
2. **`parameters` が JSON のツールの誤表示**: Fooocus (fooocus scheme) /
   RuinedFooocus / SwarmUI は `parameters` チャンクに **JSON** を書く。現状は
   判別 2 の `parse_a1111` が任意テキストで成功するため「prompt = 生 JSON 全文」
   として表示され、JSON 内の negative も検索テキストに混入する。
3. **JPEG の生成情報が AI メタ扱いされない**: A1111 系は JPEG 保存時に
   EXIF `UserComment` へ同じ parameters テキストを書くが、`extract_metadata` は
   拡張子 png 以外で即 None。なお EXIF は別経路 (`ingest_text.rs` の
   `append_exif`) で索引化されており、**UserComment 原文 (Negative prompt 含む)
   が exif 列に素通しで入っていないか Phase 0 で要確認** (入っていれば leak)。

## 3. 追加対応フォーマットと判別キー

各フォーマットの「判別条件」と「取り出し方」。確度 = 形式仕様への確信度。
**確度が「要サンプル」のものは Phase 0 で実サンプルを確保してから着手する**
(ネット上の形式記述だけで実装しない)。

| ツール | 判別条件 (PNG tEXt/iTXt) | positive | negative | params | 確度 |
| --- | --- | --- | --- | --- | --- |
| **NovelAI v3** | `Software`=="NovelAI"、または `Comment` が JSON object で `uc` キーを持つ | `Description` (なければ `Comment.prompt`) | `Comment.uc` | `Comment` の steps / sampler / scale (=CFG) / seed / width×height / noise_schedule 等 | 高 |
| **NovelAI v4** | 上に加え `Comment.v4_prompt` あり | `v4_prompt.caption.base_caption` (+ char_captions) | `v4_negative_prompt.caption.base_caption` | 同上 | 中 (v4 構造はサンプルで確認) |
| **InvokeAI (現行)** | `invokeai_metadata` キー (JSON) | `positive_prompt` | `negative_prompt` | seed / steps / cfg_scale / model 等 | 高 |
| **InvokeAI (旧)** | `sd-metadata` (JSON) / `Dream` (1 行テキスト `"prompt" -s50 -W512 ...`) | JSON: `image.prompt` / Dream: 引用部 | JSON 内 / Dream: `[...]` 表記 | フラグ群 | 中 |
| **SwarmUI** | `parameters` が JSON で `sui_image_params` キーを持つ | `sui_image_params.prompt` | `.negativeprompt` | steps / cfgscale / seed / model 等 | 中 |
| **Fooocus** (fooocus scheme) | `parameters` が JSON で `prompt` + (`base_model` or `guidance_scale` 等の指紋) | `prompt` (or `full_prompt`) | `negative_prompt` | steps / guidance_scale / sampler_name / base_model 等 | 中〜要サンプル |
| **RuinedFooocus** | `parameters` が JSON で `software`=="RuinedFooocus" | `Prompt` | `Negative` | 残りキー | 要サンプル |
| **Fooocus-MRE** | (形式未確認) | — | — | — | 要サンプル |
| **EasyDiffusion** | (形式未確認。複数 tEXt キー分散 or JSON) | — | — | — | 要サンプル |
| **A1111 互換ファミリ** (Tensor.Art / SDNext / Forge) | 既存 `parameters` 平文パーサで動く想定 | 既存 | 既存 | 既存 (known_keys の不足分を追加) | テストのみ |
| **JPEG: A1111 系** | EXIF `UserComment` のデコード結果が `parse_a1111` 形式 | 既存パーサ流用 | 同 | 同 | 高 |
| **(将来) stealth pnginfo** | アルファチャンネル LSB 埋め込み | — | — | — | Phase 4 で調査のみ |

判別の新しい決定木 (順序が重要):

```
1. `prompt` が JSON object                  → ComfyUI (現状維持)
2. `parameters` が JSON object に parse 可能 → JSON 系へ分岐:
     2a. sui_image_params あり              → SwarmUI
     2b. software=="RuinedFooocus"          → RuinedFooocus
     2c. fooocus 指紋                       → Fooocus
     2d. どれでもない                        → Unknown(parameters JSON) ※生 JSON を prompt 表示しない
3. `invokeai_metadata` / `sd-metadata` / `Dream` → InvokeAI (新旧)
4. `Software`=="NovelAI" or `Comment` JSON に `uc` → NovelAI  ※Description 分岐より前が必須
5. `parameters` (平文)                       → A1111/Forge (現状維持)
6. `Description`                             → Midjourney → Unknown fallback (現状維持)
7. その他                                     → Unknown / None (現状維持)
```

## 4. 設計方針

### 4.1 正規化構造: `A1111Metadata` を共通プロンプト構造として流用

新フォーマットの大半は「positive / negative / (Key, Value) params / raw」に正規化
できるため、**新しい enum variant をツールごとに増やさず**、既存の
`A1111Metadata` (実態は汎用プロンプト構造) に `tool: AiToolKind` フィールドを
1 つ足して全ツールの抽出結果を載せる:

```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AiToolKind {
    A1111,        // Forge / Tensor.Art / SDNext 含む
    NovelAI,
    InvokeAI,
    SwarmUI,
    Fooocus,      // RuinedFooocus / MRE 含む (細分は params で分かる)
    Midjourney,
    JpegExif,     // EXIF UserComment 由来 (中身は A1111 形式)
}
```

- `AiMetadata` enum は `A1111` / `ComfyUI` / `Unknown` の 3 variant のまま
  (UI dispatch の churn ゼロ)。`A1111` variant 名はそのまま「正規化プロンプト
  構造」として読み替える (rename は別途リファクタ判断、本計画ではしない)。
- ComfyUI はノードグラフという構造が特殊なので現状の専用 variant を維持。
- 各抽出器は `fn parse_novelai(chunks) -> Option<A1111Metadata>` の形で追加する
  小さな純関数。ユーザー方針の「テキスト変換部だけ実装すれば良い」に一致。

### 4.2 `AI_METADATA_KEYS` 静的リストの廃止 → consumed_keys 方式

「新分岐を足したら静的リストにも足す」という運用前提 (忘れると Negative leak)
を構造的に潰す。`detect_and_parse` の戻りを以下に変え、検索テキスト構築の除外を
**判別結果が実際に消費したキー**で駆動する:

```rust
pub struct ParseOutcome {
    pub meta: AiMetadata,
    /// このフォーマットが解釈に使った tEXt キー。
    /// build_searchable_from_chunks はこのキーを素通し追加から除外する
    /// (Negative を含みうる生値の再混入防止)。
    pub consumed_keys: Vec<&'static str>,
}
```

- 例: NovelAI は `["Description", "Comment", "Software", "Source", "Title"]` を
  consume → `Comment` (uc 含む) が素通しされなくなり §2-1 の leak が直る。
- 非 AI チャンク (Author 等) は従来どおり素通し追加 (挙動互換)。
- `AI_METADATA_KEYS` 定数とファイル冒頭の警告コメントは削除できる。

### 4.3 ZIP 内 PNG / 経路の自動追従

bytes 経路 (`extract_metadata_from_bytes` / `build_searchable_from_bytes`) は
`detect_and_parse` を共有しているので、ZIP 内画像・索引 ingest・Ctrl+F とも
**追加実装なしで新フォーマットに追従**する。`docs/virtual-folders.md` の分岐表に
影響なし (キー体系・キャッシュキーの変更がないため)。

### 4.4 JPEG/JFIF の EXIF UserComment (Phase 3)

- `extract_metadata` の「png 以外は None」ガードを拡張し、jpg/jpeg/jfif は EXIF
  `UserComment` を取り出して charset prefix (8 bytes: `UNICODE\0` → UTF-16、
  `ASCII\0\0\0` → ASCII、全ゼロ → 不明) をデコード → `parse_a1111` に通す。
- rexif (`exif_reader.rs`) が UserComment の生バイトをどう表現するか Phase 0 で
  確認 (文字列化済みなら charset 処理が落ちている可能性あり。必要なら
  UserComment だけ自前で IFD から取る)。
- WebP は RIFF コンテナの `EXIF` チャンク walk が必要なため、本フェーズでは
  スコープ外とする。
- ingest 側 (`ingest_text.rs`) は「EXIF として素通し」と「AI メタとして
  Negative 除外」の二重取り込みにならないよう、AI メタ認識時は UserComment を
  exif 列から除外する (§2-3 の確認結果に従う)。

### 4.5 性能

- 追加されるのは worker / インデクサスレッド内の小さな JSON parse のみ。UI
  スレッドへの新規同期 I/O はない ([docs/ui-responsiveness.md](../../ui-responsiveness.md)
  §4 チェックリスト上の新規項目なし)。
- ingest の「bytes 一回読み」構造 (`read_metadata_bytes` 共有) は変えない。
- `Comment` JSON parse は NovelAI 判別時のみ走る (チャンクが小さいので誤差)。

## 5. フェーズ分け

### Phase 0: サンプル確保と現状の固定化 (完了)

- 実サンプル収集: NovelAI (v3/v4)、InvokeAI、Fooocus、RuinedFooocus、SwarmUI、
  EasyDiffusion、A1111 JPEG 保存。ユーザー所有の生成画像 + 必要なら各ツールで
  1 枚ずつ生成。**「要サンプル」フォーマットはサンプル未入手なら Phase 2 から
  落とす** (形式記述の推測実装をしない)。
- characterization test: 現状の誤動作 (§2-1, §2-2) をテストで固定 → 修正後に
  期待値を反転させて回帰防止に使う。
- §2-3 の確認: A1111 JPEG の UserComment 原文が exif 索引列に Negative ごと
  入っているかを実機 + `ingest_text` テストで確認。

### Phase 1: 判別基盤 + NovelAI (完了)

- §4.2 consumed_keys リファクタ (`ParseOutcome` 化、既存テスト維持)。
- §3 決定木の順序整理 (`parameters` JSON 振り分けの骨格。2a〜2c が未実装の間は
  2d の Unknown 行きにして「生 JSON を prompt 表示する」現状バグだけ先に止める)。
- `parse_novelai` 実装 (v3 確実 + v4 はサンプル確認後)。
- `AiToolKind` 導入 (`A1111Metadata.tool`)。

### Phase 2: parameters-JSON 系 + InvokeAI (完了)

- SwarmUI / Fooocus / RuinedFooocus 抽出器 (サンプルあるものから)。
- InvokeAI 現行 (`invokeai_metadata`) → 旧形式 (`sd-metadata` / `Dream`) の順。
- EasyDiffusion / Fooocus-MRE はサンプル入手できた場合のみ。
- A1111 `known_keys` の追加分 (新しめの WebUI が吐く Hashes / Module 等) を
  サンプル突き合わせで補充。

### Phase 3: JPEG EXIF UserComment (完了。WebP EXIF は未対応)

- §4.4。JPEG/JFIF を対象とする。WebP は RIFF walk が必要なため将来対応。
- ingest の二重取り込み解消 (§2-3 の結果次第)。

### Phase 4 (将来検討、本計画スコープ外)

- stealth pnginfo (アルファチャンネル LSB)。画素デコードが必要で、メタデータ
  読みの「画素を読まない」原則 (`read_png_text_chunks` のコメント) を破るため
  コスト構造が別物。需要を見て別計画にする。

### リリース時 (全 Phase 共通で 1 回)

- **INDEX_VERSION bump** ([src/fts_index.rs](../src/fts_index.rs)、現行 8 → 9)。
  ingest 出力テキストが変わるため旧索引は stale。既存機構で bump 時に自動
  再構築される (`global_search_ui.rs` 参照) ので移行コードは不要だが、
  **リリースノートに「⚠️ 初回起動時に検索索引を再構築します」を入れる**。
- bump は開発中に都度やらずリリース直前に 1 回 (開発機の再構築試行は手動削除で)。

## 6. 永続データへの影響 (リリース済み/未リリース判断)

| ストア | リリース状況 | 影響 | 対応 |
| --- | --- | --- | --- |
| Tantivy 全文索引 + fts_meta.db | リリース済み (v0.8.x〜) | ingest テキスト変化で stale | INDEX_VERSION bump → 既存の自動再構築機構。移行コード不要 |
| メタデータパネル表示 | 永続キャッシュなし (セッション内 HashMap のみ) | なし | なし |
| その他 DB (catalog / rotation / tags…) | — | 触らない | なし |

## 7. UI 変更

- パネル構造 (prompt / Negative / params の 3 部) は現状維持。新フォーマットは
  すべて `draw_a1111_panel` (正規化構造) に乗るため **UI 追加実装はほぼゼロ**。
- **生成ツール固有名を UI に新規追加しない** (CLAUDE.md「モザイク・成人向け」節
  末尾のポリシー: ツール固有フォーマット名はパーサ内部の実装詳細に留め、UI には
  出さない)。`AiToolKind` はログ・テスト・内部分岐用であり、パネル見出しは
  中立な既存文言のまま。
- Unknown パネル: `parameters` JSON が未知系 (決定木 2d) の場合に生 JSON を
  そのまま出すのは現状と同じだが、折りたたみ JSON 表示
  (`draw_collapsible_json_section` 流用) に変えると可読性が上がる (任意)。

## 8. テスト計画

- `png_metadata.rs` 内 unit test (既存テスト群の隣に追加):
  - 各フォーマットの**合成チャンクフィクスチャ** (実サンプルから値を縮約) で
    判別・prompt・negative・params を検証。
  - **Negative 非混入テスト**: 各フォーマットで `build_searchable_from_chunks`
    の結果に negative 文字列が含まれないこと (consumed_keys の回帰防止。
    これが本計画で最も重要なテスト)。
  - 判別衝突テスト: NovelAI vs Midjourney (`Description` 共有)、
    `parameters` JSON vs 平文、ComfyUI `prompt` との優先順位。
  - 既存 A1111 / ComfyUI テストが無修正で green のまま (正規化リファクタの互換確認)。
- 実機 smoke: 各ツールの実 PNG/JPEG でメタデータパネル表示 + Ctrl+F ヒット +
  Ctrl+G 再インデックス後のヒットを確認。
- テスト実行: `cargo test --bin mimageviewer-core` (app::tests を含む経路)。

## 9. ドキュメント / 公開文書の更新

| 対象 | 内容 |
| --- | --- |
| [docs/spec.md](../../spec.md) | 対応メタデータ形式の一覧更新 |
| [docs/search-architecture.md](../../search-architecture.md) | ingest の png_prompt 経路と consumed_keys 方式、INDEX_VERSION=9 |
| docs/README.md | 本計画の登録 (起案時に実施済み) |
| htdocs マニュアル (AI メタデータページ) | 「PNG テキストチャンク / EXIF UserComment に埋め込まれた生成情報」という標準仕様レベルの記述で対応範囲を更新 (§10-2 の方針確定後) |
| ../../README.md 更新履歴 | リリース時。⚠️ 索引再構築の注意書き |

## 10. 残件 / 将来検討

1. ~~**未入手サンプル**: EasyDiffusion / Fooocus-MRE は実サンプル未入手のため未対応。~~
   → **2026-06-11 解消**: 形式の正は各ツール自身の書き込み実装で裏取りした
   (EasyDiffusion = sdkit `save_dicts` + easydiffusion `TASK_TEXT_MAPPING`、
   Fooocus-MRE = DiffusionToolkit `ReadFooocusMREParameters` のキー構成)。両形式とも
   実装済み・合成フィクスチャでテスト済み。実画像サンプルは sd-parsers (MIT) の
   テストフィクスチャ 9 点を取得済み (実装メモ参照)。NovelAI v4 の実画像 smoke のみ
   引き続きサンプル待ち (X/Discord 投稿はメタデータが剥がされるため入手困難。
   NovelAI 利用機会があれば 1 枚生成して `H:\home\mimageviewer_old\testimage\metadata`
   へ置き、smoke のケース表に追加する)。
2. **公開文書の表記 (ユーザー判断確定 2026-06-11)**: 利用規約上グレーなツール
   (動画ダウンローダ等) を避ける趣旨であり、**画像生成ツールの固有名は公開文書に
   直接書いてよい**。マニュアル (fullscreen.html) の対応形式表に
   InvokeAI / SwarmUI / Fooocus 系 / Easy Diffusion / JPEG EXIF を明記済み。
   アプリ UI のパネル見出しは従来どおり中立文言のまま (ツール名表示の必要が出たら別途)。
3. **WebP EXIF**: `rexif` は WebP RIFF コンテナを直接読まないため今回未対応。
   対応する場合は WebP の `EXIF` チャンクを取り出して TIFF バイト列として渡す
   小さな RIFF walk を別途追加する。
4. **未知 JSON の安全側変更 (2026-06-11)**: `prompt` + `negative_prompt` を持つ JSON は
   未知亜種でも生成メタデータとして解釈する (Fooocus 系の汎用指紋に
   `negative_prompt` を追加)。prompt キーすら無い未知 JSON は従来どおり表示専用
   Unknown で検索には入れない。
