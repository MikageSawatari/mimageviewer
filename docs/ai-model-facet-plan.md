# AI モデル名ファセット (スマートフィルタ拡張) 実装計画

ステータス: **v1.7.0 実装済み** — 起案 2026-06-15、実装 2026-06-15
(`src/png_metadata.rs`、`src/app/metadata_ops.rs`、`src/settings.rs`、`src/ui_main.rs`)

## 実装メモ (v1.7.0)

- `AiMetadata::model_names()` / `model_name()` と `generation_tool()` を追加し、A1111/Forge、
  ComfyUI、NovelAI、InvokeAI、SwarmUI、Fooocus、EasyDiffusion の既存メタデータ抽出結果から
  セッション内でファセット値を生成する。
- `FacetFilter` に `model_names` / `generation_tools` を追加し、スマートフィルタバーへ
  「モデル」「生成ツール」軸を追加した。複数モデルを持つ ComfyUI はいずれか一致で通す。
- グローバル Tantivy 索引には入れない。現在グリッドの遅延読み込み済み AI メタデータを使う
  session-local facet として扱うため、`INDEX_VERSION` bump は不要。
- 実サンプル由来の調整として、ComfyUI の `prompt` / `workflow` JSON に出る `NaN` /
  `Infinity` / `-Infinity` は ComfyUI 分岐内だけ `null` に置換して best-effort で読む。
  また Flux 系の `UNETLoader` / `UNETLoaderGGUF` の `unet_name` も主モデル候補として扱う。
- InvokeAI 新形式の `model` object は、object 全体をモデル名として文字列化せず、
  `model_name` / `name` / `model_weights` など既知フィールドから実名だけを平坦化する。
- AI モデル facet の高速化検討用に、遅延メタデータ worker の起動/完了を
  `details_meta.load_start` / `details_meta.load_done`、100ms 以上かかった単件を
  `details_meta.ai_item_slow`、メニュー件数集計を `ui.facet_ai_model_counts_build` /
  `ui.facet_ai_tool_counts_build` として perf log に記録する。`load_done` には
  `ai_total_ms` / `ai_permit_wait_ms` / `ai_extract_ms` と各 max を分けて出し、
  初回 0 件のまま止まって見えるケースが I/O permit 待ちなのか PNG/JPEG メタ抽出なのかを
  切り分ける。AI モデル / 生成ツールの件数は、表示集合または遅延メタキャッシュが変わるまで
  `facet_ai_model_counts_cache` / `facet_ai_tool_counts_cache` で再利用し、メニューを開いたままの
  毎フレーム O(n) 再集計を避ける。

## 0. 背景と目的

NeeView パリティ監査 (2026-06、[competitive-analysis メモ]) で挙がった「検索の表現力」
ギャップの、**mIV の客層 (AI 画像整理) に効く部分だけに絞った**実装。

NeeView は正規表現 / 数値比較 / 任意メタフィールド指定の構造化検索を持つが、設計検討の結果
(ユーザー判断 2026-06-15) 以下に絞り込んだ:

- **採用**: **生成モデル名での絞り込み** (+ 副次的に**生成ツール**での絞り込み)。
- **見送り**: EXIF 条件検索 (本格現像は Lightroom 領域で差別化にならない)、Steps 等の数値
  比較 (需要薄)、正規表現 (任意・低優先)。

理由: mIV は先日 AI メタデータパーサを拡充し ([docs/ai-metadata-parser-expansion-plan.md](ai-metadata-parser-expansion-plan.md))
NovelAI / InvokeAI / SwarmUI / Fooocus 系 / EasyDiffusion / A1111 のパラメータを既に抽出
できる。現状はプロンプト文字列を**全文検索**できるだけで、「モデル X で生成したものだけ」を
絞れない。数千枚の生成画像を整理するユーザーにとってモデル別の絞り込みは中核ワークフローで、
**既存の抽出資産をフィルタに出すだけ**で実現できるため費用対効果が高い。

## 1. 統合先: 既存スマートフィルタを拡張する (新規検索は作らない)

ユーザー方針どおり、新しい検索システムではなく**既存の `FacetFilter` を拡張**する
([src/settings.rs](../src/settings.rs) `FacetFilter`、[src/app/metadata_ops.rs](../src/app/metadata_ops.rs)
`facet_kind_for_item` / `facet_ext_for_item`、詳細表示のチップ式フィルタバー)。
現状ファセット = 種類 / 拡張子 / ★ / タグ / 日付 / サイズ / 状態
([docs/details-view-and-filter-plan.md](details-view-and-filter-plan.md))。ここに:

- **モデル名ファセット** (主)
- **生成ツールファセット** (副。A1111 / ComfyUI / NovelAI / InvokeAI / SwarmUI / Fooocus / EasyDiffusion)

を追加する。スコープは既存ファセットと同じく**現在のグリッド** (現在地フィルタ相当)。
グローバル全文索引への展開は将来 (§6)。

## 2. 「第二弾 (遅延) ファセット」として実装する

モデル名はファイルを読まないと分からない。詳細表示プランの**遅延列 worker** 基盤
([docs/details-view-and-filter-plan.md](details-view-and-filter-plan.md) の「第二弾」=
stat/decode/probe が要る項目) と同じ仕組みに乗せる:

- 可視範囲のアイテムから順に worker でメタデータを読み、モデル名を抽出してキャッシュ。
- Ready になるまでその軸のフィルタ/件数は「読み込み中」を出す (既存の遅延列 UX に倣う)。
- UI スレッドから同期 decode しない ([docs/ui-responsiveness.md §4](ui-responsiveness.md))。

## 3. モデル名の抽出 (形式別の確度)

抽出は [src/png_metadata.rs](../src/png_metadata.rs) の `AiMetadata` から行う純関数
`fn model_name(meta: &AiMetadata) -> Option<String>` を新設する。形式別:

| 形式 | モデルの所在 | 確度 | 備考 |
| --- | --- | --- | --- |
| A1111 / Forge | params の `Model` (+ `Model hash`) | 高 | 既に params に抽出済み。そのまま使える |
| SwarmUI | `sui_image_params.model` | 高 | 抽出済み |
| Fooocus / RuinedFooocus | `base_model` / `base_model_name` | 高 | 抽出済み |
| InvokeAI | `model` (新形式) / `model_weights` (旧 sd-metadata) | 高 | 抽出済み |
| EasyDiffusion | `use_stable_diffusion_model` (パス → basename) | 中 | パスから basename 化が必要 |
| **ComfyUI** | ノードグラフの `CheckpointLoaderSimple.ckpt_name` / `UNETLoader.unet_name` 等 | **低** | **複数チェックポイント / LoRA / refiner があり単一に確定できない**。ベストエフォート (見つかった checkpoint / UNET 名を全部集める / 代表 1 つ / 「(複数)」)。`NaN` 等の非標準数値を含む prompt JSON は ComfyUI 分岐内だけ寛容に読む |
| NovelAI | `Source` に "Stable Diffusion <hash>" のハッシュのみ | 低 | 人間可読なモデル名は取れない。「(NovelAI)」/ハッシュ表示に留める |
| 非 AI 画像 | — | — | モデル軸では「(なし)」グループ |

**正規化の論点** (§7): A1111 は friendly 名 (`Model`) と hash (`Model hash`) の両方を持つ。
friendly 名優先で表示し、同一モデルの表記ゆれ (拡張子有無 `.safetensors`、パス前置) を
どこまで畳むか。畳みすぎると別モデルが混ざるので、まずは**ファイル basename + 拡張子除去**程度の
軽い正規化で始める。

実装メモ (2026-06): A1111 派生メタデータでは `Model` の直後に `Hashes` /
`Variation seed` / `Batch size` / `Template` 系の後続パラメータが続くことがある。
モデル名候補はこれらの境界で切り、LoRA タグや Template 本文などのプロンプト片は
モデル名として採用しない。

## 4. ファセット UI / 挙動
- 既存ファセットチップと同じ見た目で「モデル」「生成ツール」を追加。ドロップダウンに
  現グリッドに存在するモデル名 + 件数を列挙 (タグファセットの件数表示 `facet_tag_counts` に倣う)。
- 候補数やモデル名長が大きいフォルダでも、AI モデル / 生成ツールメニューは最大幅と
  15〜20 行程度 (実装は 18 行) のスクロール領域に収め、ウィンドウ全体を押し広げない。
- 複数選択 = OR (タグファセットと同じ規約)。
- ComfyUI の複数モデルは「いずれかに該当」で複数モデル値にマッチさせる (best-effort)。
- 抽出不能 / 非 AI は「(モデル情報なし)」グループにまとめ、ノイズを 1 か所に隔離。

## 5. 永続データへの影響
- **FacetFilter に軸を追加** ([src/settings.rs](../src/settings.rs))。フィルタ状態の保存は
  settings_db の既存 migration 経路で後方互換に追加。
- **グローバル索引 (Tantivy / fts_meta) は触らない** (本計画は現グリッドのファセットのみ)。
  → `INDEX_VERSION` の bump は不要。モデル別の**全文索引横断**を将来やる場合のみ別途 bump (§6)。
- モデル名抽出結果はセッション内キャッシュ (遅延列と同じ。永続キャッシュは任意、§7)。

## 6. スコープと将来拡張
- **v1.7.0**: 現在のグリッドに対するモデル名 / 生成ツールファセット (上記)。
- **将来 (本計画外)**: Ctrl+G グローバル検索でモデル指定 (Tantivy にモデル field を STORED 追加
  → `INDEX_VERSION` bump + 再構築)。正規表現 / 数値比較演算子の検索構文。需要を見て判断。

## 7. テスト
- `model_name(meta)` の形式別 unit test (合成フィクスチャ + §3 の実サンプルがあるものは
  smoke。サンプル = `H:\home\mimageviewer_old\testimage\metadata`、`MIV_AI_METADATA_SAMPLE_DIR`)。
  - A1111 / SwarmUI / Fooocus / InvokeAI で正しい friendly 名、ComfyUI で best-effort、
    NovelAI / 非 AI で「情報なし」。
- ファセット件数とフィルタ結果の整合 (現グリッドのモデル別件数 = フィルタ後件数)。
- 遅延読み込み中に UI が固まらないこと (worker 化の回帰)。

## 8. ドキュメント同時更新
- [docs/details-view-and-filter-plan.md](details-view-and-filter-plan.md) — 新ファセット軸を追記。
- [docs/search-architecture.md](search-architecture.md) — モデル抽出経路と (将来の) 索引方針。
- [docs/spec.md](spec.md) — スマートフィルタの軸一覧更新。
- htdocs マニュアル / 製品ページ — 「生成モデルで絞り込み」を一般語で記述
  (ツール固有名の扱いは [CLAUDE.md] の方針に従う。生成ツール名の直接表記可否は §9)。

## 9. 論点の仕分け (2026-06-15: 実装前に固定 / 実装後に詰める)

### A. 実装前に決める (これだけ。決めれば残りは無コストで反復可能)
1. **永続化境界 = v1.7.0 は非永続 (セッション/遅延キャッシュ) で確定**。→ 推奨 yes。これにより
   正規化規則・ComfyUI 扱い・生成ツール facet 有無は**すべて derived (その場で再計算) となり、
   後から自由に変更でき、マイグレーション不要**。
2. **スコープ = 現在グリッドのみ** (グローバル索引 = Tantivy へのモデル field 追加は将来。
   それをやるときだけ `INDEX_VERSION` bump + 再構築、§6)。

### B. 実装後に試しながら詰める (A の非永続前提なら全部 cheap・derived)
- モデル名の正規化強度 (basename + 拡張子除去で開始。hash しか無い NovelAI の扱い)。
- ComfyUI の複数モデル表現 (全部を値に持つ / 代表 1 つ / 「(複数)」)。
- 生成ツールファセットを同時に出すか (モデルだけ先行も可)。
- マニュアル/UI に生成ツール固有名 (A1111 等) を出すか (ユーザー方針=グレーでない生成ツール名は
  公開文書に出して可。最終文言は実装後に確認)。

### C. 将来 (本計画外)
- 抽出結果の永続キャッシュ (catalog 等にモデル名列)。大量フォルダで毎回 decode を避けたく
  なったら検討。非永続で性能が足りるかを実装後に観測してから判断。
