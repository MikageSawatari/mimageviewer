# カラー検索（色で絞り込み）機能 設計計画

> ステータス: **Phase 1/2 + Phase 3 実装済み（Codex 実装 + ClaudeCode レビュー/追補、2026-06-23）**。今回リリースの対象ビューは通常フォルダ / ZIP / PDF 表示に固定（集約ビューは見送り確定）。マニュアル（grid.html）・製品ページ（index.html）更新済み。UI snapshot は任意（既存 facet バー snapshot は無く、追加は forward-looking なので未実施）。
> Eagle for Windows のカラー検索を参考に、mIV のグリッド表示を「指定した色を主要色として
> 含む画像」で絞り込む機能を追加する。

> **設計方針の変遷（重要）**
> - 初版: サムネ生成時にパレットを事前計算し `catalog.db` に永続化する案。
> - Codex レビュー (2026-06-23): `thumbnails.thumb_data NOT NULL` + cache Off/Auto 非保存で
>   行が作れない（P1）、遅延バックフィルのデッドロック（P1）、多フォルダビューでの
>   データ取得破綻（P1）など、**永続化前提ゆえの問題**が多数。
> - **現行方針 (2026-06-23、ユーザー判断): 永続化をやめ、オンデマンドスキャンにする。**
>   色フィルタを使おうとした瞬間に、現在の表示の全アイテムをスキャン（サムネがあればそれを、
>   無ければ必要時デコード後に縮小で）してパレットを**在メモリで**作り、進捗表示 + キャンセル可能にする。
>   Ctrl+F の検索と同じ操作感。**メタデータの永続保存・スキーマ・設定追加が一切不要になり、
>   Codex の P1 指摘群が構造的に消える**（§1.3 に対応表）。
> - オンデマンド案の再レビュー (2026-06-23) を反映: 色状態を `Settings`(`FacetFilter`) に載せると
>   `settings.save()` で無言永続化されるため **transient `ColorFilterState` + `rebuild_visible_indices`**
>   にする（§6, P1）。スキャンは raw `rayon` でなく **bounded・cancellable な専有 worker**（§3.2, P1）。
>   非 JPEG はフル decode になり得るので**上限/進捗/大量時確認**（§3.1-3.2, P2）。キーは bare
>   `filename` でなく **cache key 由来の `ColorPaletteKey`**（§4.2, P2）。cache_map ロック最小化 +
>   `load_all()` 二重ロード回避（§3.1, P2）。フルスクリーン palette 表示は UI スレッドで decode
>   しない（§7.1, P2）。
> - 3 巡目レビュー (2026-06-23) を反映: `cache_key_for_request` の通常画像 fallback は `file_name()`
>   だけでキーに鮮度が無いため、**`mtime`/`file_size` を palette 値に必須保持し読むたび検証**へ格上げ
>   （§4.2, P2。同名差し替え/LRU 再訪の stale 防止）。フルスクリーン palette は **CPU decode 済み
>   バッファのみ再利用、GPU テクスチャしか無ければ readback せず worker**（§7.1, P2）。未スキャンは
>   初版**「完了時一括反映・キャンセルで未適用」固定**（§7, P3）。README 要約の decode 表現を緩和、
>   `事前スキャン` typo 修正（P3）。
> - 4 巡目レビュー (2026-06-23) を反映: `folder_signature` だけの「済/未済」判定では同一フォルダ内の
>   items 変化（スタック切替・ファセット併用・ファイル追加・検索/タグビュー）を取り逃すため、
>   `ScanPalettes` を**候補キャッシュ化し、現在 items の missing/stale だけ差分スキャン → 完了後一括
>   反映**へ（§2, §6, P2）。テスト方針に**鮮度検証（key 同一でも mtime/size 変化で再利用しない /
>   stale WebP 拒否→元画像）を明記**（§10, P2）。palette マップのキーは実在しない `stackthumb:` でなく
>   **`GridItem::perf_key()`**（`stack::` 等）を正に（§4.2, P3）。本文の「低解像度」旧表現を一掃（P3）。
> - 5 巡目レビュー (2026-06-23) を反映: 結果適用整合を folder generation 依存から
>   **`scan_id` + `ScanScopeSignature`** へ。**palette 結果は候補キャッシュへ無条件 merge 可だが、
>   一括フィルタ反映は起動時 scope 一致時だけ・不一致なら現 items で missing/stale 再判定**
>   （古いスコープの結果で UI を確定させない、§3.2, P2）。§3.1 の見出し表現を「デコードし縮小/
>   サンプリング」に統一（P3）、Phase 2 の旧名 `FolderPalettes`→`ScanPalettes`（P3）。
> - 6 巡目レビュー (2026-06-23) を反映: 取り残し掃除。`ScanPalettes` 型例から `folder_signature` を
>   外し **`active_scan_id` / `last_scope_signature` / `map`** に（粗いフォルダ識別を持つ場合は
>   LRU/所属判定専用で整合には使わないと明記、§4.2, P2）。テスト方針を旧「フォルダ世代不一致で破棄」
>   から **「palette は merge 可・scan_id/ScanScopeSignature 不一致なら visible 一括反映しない」を直接
>   検証**へ（§10, P2）。§3.2 に 1 か所残った「無いものだけ縮小デコード」を統一表現へ（P3）。
> - **以降の進め方 (2026-06-23 ユーザー指示)**: 実装は Codex 担当、コードレビューは ClaudeCode 担当。
> - **Codex 初期実装 (2026-06-23)**: `src/color_search.rs`（抽出・照合・テスト）、
>   `src/app/color_filter.rs`（専有 worker・進捗/キャンセル・差分スキャン・scope 整合）、
>   `src/ui_main.rs`（色ファセット UI）を追加。通常フォルダ / ZIP / PDF 表示で開始し、
>   Ctrl+G/タグ/お気に入り検索など集約ビューは初版では非表示。
> - **ClaudeCode レビュー反映 + Phase 3 一部実装 (2026-06-23)**: UI 名称を「画像色」に寄せ、
>   動画/フォルダ等は対象外で画像だけを同時に絞り込むことを示す。メタデータパネルに
>   decode 済み CPU バッファ由来のスウォッチ列を表示し、クリックで画像色フィルタを起動。
>   `color/*` perf 計装（scan/filter/fullscreen palette）と、大量 missing 時の明示確認 UI を追加。
>   多フォルダビューは今回リリースでは対象外に確定し、集約ビューからスウォッチクリックした場合は
>   案内トーストを出して有効化しない。snapshot、マニュアル/製品ページ更新は未実装。
> - **ClaudeCode 追補 (2026-06-23、Phase 3 残作業)**: 整合判定（`scan_id` + `ScanScopeSignature`）を
>   純関数 `color_search::scan_result_disposition`（`Drop`/`Apply`/`Restart`）へ切り出し、`poll_color_scan`
>   をそれ経由に（挙動不変、Drop 時は O(N) scope 算出を遅延スキップ）。`color_search` の unit test を
>   4→10 件に拡充: scan_id/scope の Apply・Restart・Drop 判定、`fresh_entry` の mtime/file_size 鮮度棄却、
>   scope signature の item 集合/view 種別変化検知（§10 の中核不変条件を回帰テスト化）。集約ビューの
>   fullscreen でスウォッチを押したときは `apply_image_color_filter_from_swatch` 冒頭で
>   `color_filter_available_in_current_view()` を見て案内トースト + 非有効化（不活性チップ防止）。
>   マニュアル（grid.html「画像色で絞り込む」節）・製品ページ（index.html 機能カード）を更新。
>   残: 集約ビュー開放（見送り確定）、UI snapshot（任意）。

関連: [catalog-design.md](catalog-design.md)、[search-architecture.md](search-architecture.md)
（Ctrl+F `execute_search` の worker パターン）、[ui-responsiveness.md](ui-responsiveness.md)（§4）、
[details-view-and-filter-plan.md](details-view-and-filter-plan.md)（`FacetFilter` 共通絞り込み）。

---

## 1. 目的・背景

### 1.1 やりたいこと

Eagle のように「カラーピッカーで色を選び、精度（許容範囲）を指定すると、その色を**主要色として
多く含む**画像に絞り込まれる」機能。mIV では**たまに使う絞り込み機能**として提供する。

### 1.2 Eagle の仕組み（調査からの観測・推定）

**公式は内部実装・パレットの正確なスキーマを一切公開していない。** 以下は公開ドキュメント・
公式ブログ・開発者 API の記述・コミュニティ解析・一般手法からの**観測と推定**であり断定ではない:

- 取り込み時に各画像から代表色パレット（**代表色 RGB + 占有率 `ratio`**）を生成して保存している
  と推定される（`metadata.json` の `palettes` フィールド、read-only と明記）。
- UI のツールチップ `#212E63 (54.0%)` が「色 + 占有率」を持つことを裏付ける（ユーザー提供
  スクリーンショット）。占有率が 54% と偏ることもある＝主要色ゲートが効いている。
- 検索 UI は「組み込みパレットを tap/drag」「HEX/RGB/HSL 入力」「精度（許容範囲）指定」を提供。

mIV は Eagle の「**事前計算して永続保存**」はまねしない（後述）。借りるのは
**「画像を数色のパレットに要約し、選択色との知覚距離で照合する」**というコア手法だけ。

### 1.3 永続化をやめてオンデマンドにする理由

「色絞り込みはたまに使う機能」なので、全フォルダ・全画像のパレットを常時事前計算・永続化するのは
コストに見合わない。**使う瞬間にだけ、その表示分をスキャンする**方が、実装も運用もすっきりする。

Codex がレビューで挙げた問題が、オンデマンド化でどうなるか:

| Codex 指摘 | 永続化案での問題 | オンデマンド案では |
| --- | --- | --- |
| P1-① thumb_data NOT NULL / cache Off カバレッジ | 行が作れず色情報を持てない | **解消**（永続行に依存しない。必要画素を都度デコード） |
| P1-② 遅延バックフィルのデッドロック | NULL が隠れて埋まらない | **解消**（起動時に全アイテムを事前スキャン＝P1-② の推奨形そのもの） |
| P1-③ 多フォルダビューでデータ取得破綻 | per-folder DB を引けない | **構造的ブロッカー消滅**（アイテムの画素を直接読む。スコープは性能判断のみ §9） |
| P2-④ `save` の保持/空の曖昧さ | INSERT OR REPLACE で誤消去 | **解消**（保存しない） |
| P2-⑤ ratio スケール (u8/u16) | BLOB 互換が揺れる | **緩和**（在メモリ f32。表示整形だけの話） |
| P2-⑥ 量子化のビン割れ | 主色が ratio_floor 落ち | **そのまま有効**（抽出品質の話。§4 で対応） |
| P3-⑦ Eagle 内部の断定 | 誤解を招く | **そのまま有効**（§1.2 を「推定」表記に） |

残コストは「フォルダを離れて戻ると再スキャン」のみ（§6 で在メモリ候補キャッシュ + 必要時デコード
（JPEG のみ DCT 縮小）で緩和）。たまに使う機能なら許容範囲。将来不満が出たら**その時に**
オプションで永続キャッシュを足せばよく、最初から持つ必要はない。

---

## 2. 全体フロー（Ctrl+F 風）

```
ユーザーがファセットバーで「色」フィルタを開く / 色を選ぶ
  ↓
① 現在の表示アイテム（items）に対し、在メモリ「候補キャッシュ」を突き合わせる（§6）
    - 各アイテムの安定キー（GridItem::perf_key）で候補キャッシュを引き、mtime/file_size が
      一致する palette があれば再利用。**missing / stale なアイテムだけ**を ② のスキャン対象に。
    - missing / stale が 0 件なら即 ③ へ（スキャン不要）。スタック切替・ファセット併用・
      ファイル追加で items が変わっても、変わった分だけ拾えるので取り逃がさない。
  ↓
② 色スキャン worker（missing/stale のみ・キャンセル可能 + 進捗）
    - 各アイテムについて画素を入手 → パレット抽出（§3, §4）→ 候補キャッシュへ
    - 画素入手の優先順位: in-memory cache_map(WebP) → catalog(WebP) → 必要時デコード後に縮小
      （JPEG のみ DCT 縮小 decode 可、PNG/WebP/WIC はフル decode になり得る §3.1）
    - 「スキャン中… 320/1000」を表示。Esc / ✕ でキャンセル
  ↓
③ 在メモリパレット × 選択色で絞り込み（§5）
    - 色・許容範囲スライダを動かしても ② は再実行しない。③ の再フィルタのみ（一瞬）
  ↓
フォルダ移動 → 在メモリパレットを破棄。フィルタ解除だけなら現在フォルダ分は再利用してよい
（別の色をすぐ試す時に再スキャンしないため。将来は直近数フォルダを LRU 保持も可）
```

- **Ctrl+F (`execute_search`) との対比**: 入力を契機に worker で現在の表示分を処理し、進捗・
  キャンセルを出し、結果で絞り込む——という骨格は同じ。色スキャンはその「色版」。
- **設定ページ・永続データは追加しない**。色・許容範囲は**フィルタ UI の一時状態**であって
  保存設定ではない（環境設定を増やさない）。

---

## 3. 色スキャン worker

### 3.1 画素の入手（既存資産の再利用、UI スレッドを止めない）

各アイテムについて、安い順に画素を得る:

1. **in-memory `cache_map`**: `Arc<RwLock<HashMap<String, CacheEntry>>>`。`CacheEntry.jpeg_data` は
   実体 WebP バイト（[src/thumb_loader.rs](../src/thumb_loader.rs) 参照）。あれば WebP デコード。
   - **ロック保持を最小化（Codex P2）**: read ロック下では**必要な WebP バイトだけ clone（or Arc
     共有）して即座にロックを離す**。デコードはロックの外で行う。ロックを持ったまま decode しない。
2. **catalog の WebP**: cache_map に無いものだけ catalog から引く。**`CatalogDb::load_all()` で
   フォルダ全 BLOB を再ロードしない（Codex P2）**。cache_map が既に持っているなら二重ロードで
   数百 MB を一時的に重複させてしまう。必要キーだけ個別 SELECT する。
3. **どちらも無い**: 元画像を**デコードし、縮小/サンプリング**する。
   - **JPEG**: turbojpeg の DCT スケール（1/8 等）で**入口から縮小デコード**でき軽い。
   - **PNG / WebP / WIC（HEIC/AVIF/JXL/TIFF/RAW）**: ほとんどの経路は**いったんフル解像度
     デコードしてから縮小**する（Codex P2）。つまり「低解像度指定」でも decode コスト自体は
     フル decode のことが多い。→ §3.2 の上限/進捗/キャンセル/大量時の確認が重要。
   - ZIP/PDF/動画アイテムはサムネ（1 枚目 / 1 ページ目 / 代表フレーム）を画素源にする。

> パレットは**色分布**さえ取れればよく解像度は不要。画素入手後は長辺 **64–128px 相当**まで縮小
> （or 間引きサンプリング）してから量子化する。JPEG は DCT で入口から軽くできるが、それ以外の
> 形式は decode 自体がフルになり得るので「サムネがあれば必ず再利用」を優先する。

### 3.2 専有 worker（rayon ではなく bounded・cancellable）— Codex P1

raw `rayon` でフォルダ全件を一気にデコードすると、**サムネ生成 / PDF レンダ / インデクサの I/O と
取り合い**になり、キャンセル/バックプレッシャも弱い。大量フォルダを HDD/NAS で開くと「Ctrl+F より
重い」体感になりかねない。代わりに:

- **専有の bounded worker（限定並列度 + キャンセルチェック付きキュー）**にする。既存の
  優先度キュー / セマフォ方式（サムネ I/O ワーカー、PDF プール）に倣う
  （[async-architecture.md](async-architecture.md)。`try_lock + sleep` は使わない＝CLAUDE.md 並行処理方針）。
- 並列度は I/O 競合を避けるため絞る（例: サムネワーカーと同程度 or それ以下）。色スキャンは
  ユーザー起動の一時タスクなので、既存の常時 I/O を優先させる。
- **キャンセル**: `Arc<AtomicBool>` トークン。フォルダ移動・フィルタ解除・Esc で中断
  （Ctrl+F / サムネロードと同じ規約）。各アイテム処理前にチェック。
- **進捗**: `done / total` を atomic で更新し、UI に「スキャン中… N/M」+ プログレスと
  キャンセルボタン。
- **大量アイテム時の確認しきい値（Codex P2）**: items 数が非常に多い（例: 数千〜数万、特に
  サムネ未生成 + 非 JPEG が多いフォルダ / 多フォルダ集約ビュー §9）場合は、即スキャンせず
  「N 件をスキャンします」確認を挟む or 明示起動にする。無言で長時間ブロックしない。
- **結果適用の整合（folder generation だけに頼らない・Codex 5 巡目 P2）**: 候補キャッシュ化後は、
  同一フォルダ内でもスタック切替・ファセット変更・ファイル追加で **folder generation は変わらない**
  ことがある。そこで worker 起動時に **`scan_id`（単調増加）+ `ScanScopeSignature`**（§6。view kind +
  item count + Σ(perf_key, mtime, file_size) のハッシュ）を焼き付け、完了時は次の 2 段で扱う:
  - **palette 結果は候補キャッシュへ無条件 merge 可**。`ColorPaletteKey`(=perf_key) + mtime/file_size で
    同定されるので、スコープが変わっても個々の palette は正しい（読む時に §4.2 の鮮度検証が効く）。
    途中で別フォルダへ移った場合のみ、その fold の `ScanPalettes` は破棄（LRU 方針に従う）。
  - **一括フィルタ反映（visible 絞り込みの確定）は、起動時 `ScanScopeSignature` が現在と一致する時だけ**。
    不一致（items が変わった）なら反映せず、**現在の items で missing/stale を再判定**して必要なら
    差分スキャンを起動し直す（古いスコープの結果で UI を確定させない）。
- **既存サムネ生成との競合回避**: フォルダを開いた直後でサムネ生成が走っている最中でも、
  色スキャンは「今あるサムネ + 無いものだけ必要時デコード後に縮小/サンプリング」で独立に進める。
  重複 decode は起こり得るが cache_map 経由でなるべく共有する。

### 3.3 UI スレッド禁止事項（[ui-responsiveness.md](ui-responsiveness.md) §4 準拠）

- 画素デコード・WebP デコード・パレット抽出は**すべて worker**。`App::update` から同期到達しない。
- perf 計装: `color/scan_start`・`color/scan_item`（or バッチ）・`color/scan_done`・
  `color/filter_apply` に `perf::event`。スキャン総時間・1 枚あたり時間を後で解析できるように。

---

## 4. パレット抽出（在メモリ）

### 4.1 アルゴリズム（量子化 + 知覚マージ + 再割当）— Codex P2 対応

1. 縮小/間引き済み画素を粗いビンに量子化（例: RGB 各 4bit = 4096 ビン、or HSV ビン）。
2. ビンごとに画素数集計 → 上位を代表色候補に。
3. **【必須】知覚マージ + 全画素再割当**: 粗ビンはグラデーション/圧縮ノイズで主色が隣接ビンに
   割れ、各片の `ratio` が `ratio_floor` を下回って消える。対策:
   - 候補同士を **Lab（または OKLab）距離**でクラスタリングし、ΔE が merge しきい値未満の
     ものを 1 色に統合。
   - **全画素を最終代表色へ最近傍で再割当**し、その画素数から `ratio` を確定（ビンの素の集計
     ではなく再割当後の実数）。
4. `ratio` 降順で上位 **8 色**（Eagle は実機 ~8–9 色）を採用。

> 決定性: ビン定義・マージしきい値・色数を固定し、実行時状態（空きメモリ等）で結果を変えない。
> 同じ画像は常に同じパレットになること（決定性優先の方針。詳細は CLAUDE.md の並行/決定性方針）。

### 4.2 在メモリ表現と**キー設計**（永続化しないので BLOB 不要）

```rust
pub struct PaletteColor {
    pub rgb: [u8; 3],
    pub ratio: f32,   // 0.0..=1.0（占有率）。永続化しないので素直に f32 でよい
    pub lab: [f32; 3] // 抽出時に一度だけ計算してキャッシュ（クエリの ΔE 用）
}
pub struct Palette { pub colors: Vec<PaletteColor> } // 通常 8、上限 10、ratio 降順

// スキャン結果の在メモリマップ
struct PaletteEntry {
    mtime: i64,       // 必須: 鮮度検証用（キーに焼かれていないため）
    file_size: i64,   // 必須: 鮮度検証用
    palette: Palette,
}
struct ScanPalettes {
    map: HashMap<ColorPaletteKey, PaletteEntry>, // 候補キャッシュ本体（perf_key → palette）
    active_scan_id: u64,        // 進行中スキャンの id。完了結果はこれと一致時のみ「最新」扱い
    last_scope_signature: u64,  // 直近で visible 一括反映した ScanScopeSignature（§3.2, §6）
    // 注: 粗いフォルダ識別（folder_key 等）を持つ場合は **LRU / 所属フォルダ判定専用**とし、
    //     結果適用整合には使わない（整合は active_scan_id + last_scope_signature が担う）。
}
```

**キーは bare `filename` にしない（Codex P2）。** ZIP entry / PDF page / フォルダ代表サムネ /
フルパス / ファイル名スタック / 将来の多フォルダビューが絡むと衝突するため、
**`GridItem` 由来の安定キーから `ColorPaletteKey` を導出する**:

- 全 variant 統一の per-item 安定キーは **`GridItem::perf_key()`**（[src/grid_item.rs:321](../src/grid_item.rs)）。
  実際の prefix は `dir::` / `zipfile::` / `pdffile::` / `zip::{path}#{entry}` / `zipdir::{path}#{prefix}` /
  `archive::` / `searchdir::` / `searchzip::` / `pdf_page_perf_key(...)` / **`stack::{representative}`**
  （スタック代表、[src/grid_item.rs:356](../src/grid_item.rs)）/ 通常画像はフルパス。これを
  `ColorPaletteKey` に使えば、混在しても 1:1 で取り違えない。
  - 注: 当初プラン文書で挙げた `stackthumb:` prefix は**実コードに存在しない**（スタック代表は
    通常サムネを再利用し、安定キーは `stack::...`）。`GridItem::perf_key()` を正とする。
  - 画素を `cache_map` / catalog から引く時のキー（= サムネ保存キー）は別概念で、
    `thumb_loader::cache_key_for_request(&LoadRequest)`（`CACHE_KEY_ZIP/PDF/ARCHIVE/SEARCH_REP` /
    `#pin:` / `adjustment_db::zip_entry_key` 等）を使う。**palette マップのキー（identity）と
    画素入手キー（storage）を混同しない**。
- **鮮度検証は必須（Codex 再レビュー P2）**: 通常画像の `cache_key_for_request` は fallback が
  `req.path.file_name()` **だけ**で（[src/thumb_loader.rs:119](../src/thumb_loader.rs)）、キーに
  `mtime`/`file_size` は焼かれていない。既存サムネも `CacheEntry.mtime/file_size` を**別途比較**して
  鮮度を見ている（[src/thumb_loader.rs:916](../src/thumb_loader.rs) の
  `entry.mtime == req.mtime && entry.file_size == req.file_size`）。色パレットも同様に、
  **`mtime`/`file_size` を値（`PaletteEntry`）に必須保持し、読むたびに検証する**。検証対象は:
  - 在メモリ `map` の再利用時（LRU で直近フォルダを残す場合、同名ファイル差し替え後に古い palette を
    返さない）。
  - 画素を `cache_map` / catalog から読むとき（その WebP が現ファイルと同じ mtime/file_size か）。
    不一致なら cache を使わず元画像から再抽出する（既存サムネと同じ判定）。
  - ZIP は entry 名、PDF は page もキー側で識別済みだが、容器の更新検知は同様に mtime/file_size で行う。
- これにより、通常画像・ZipImage・PdfPage・Stack 代表・pin 代表が混在しても取り違えず、
  同名差し替え・LRU 再訪でも stale を返さない。

- `ratio` の u8/u16 スケール問題（前案 Codex P2-⑤）は**消える**（在メモリ f32、表示は `*100.0` で `%`）。
- `lab` を持たせ、色/許容を動かすたびの再フィルタを乗算なしの距離計算だけにする。

---

## 5. クエリ / マッチング

- **クエリ色 → LAB**: ピッカー/HEX/RGB の sRGB を CIELAB へ。
- **距離**: **ΔE76**（LAB ユークリッド）固定で十分。ΔE2000 は当面入れない（設定を増やさない方針）。
- **マッチ条件**: 画像 `i` がヒット ⇔
  ```
  ∃ c ∈ palette(i) :  ΔE(query_lab, c.lab) < tolerance  ∧  c.ratio ≥ ratio_floor
  ```
  - `tolerance` = 許容範囲スライダ（「厳密 ←→ 緩い」表示、内部で ΔE 値へ写像）。
  - `ratio_floor` = 主要色ゲート（既定 8% 程度の定数）。Eagle の「**多く使われている**色」挙動を再現。
- **再フィルタは一瞬**: ② のスキャン結果（在メモリ）に対して ΔE を回すだけ。1 万枚 × 8 色でも
  数 ms。色・許容を動かしてもスキャンは再実行しない。
- 並べ替え: 初版はヒット/非ヒットの二値（グリッドから外す絞り込み）。将来 `best ΔE × ratio` で
  スコアソートも可（§10）。

---

## 6. 状態の持ち方と無効化（transient・設定保存しない）— Codex P1

**色フィルタの状態は `Settings` に入れない。** `FacetFilter` は `Settings` のフィールド
（[src/settings.rs:1622](../src/settings.rs)）で、`render_facet_filter_bar` は facet 変更時に
**`self.settings.save()` を呼ぶ**（[src/ui_main.rs:5395](../src/ui_main.rs)）。色を `facet_filter` に
載せると、色・許容範囲を動かすたびに**設定が無言でディスク保存**され、「設定を増やさない」方針に
反する。

そこで:

- 色フィルタは **`App` の transient 状態 `ColorFilterState`**（選択色・tolerance・候補キャッシュ
  `ScanPalettes`・worker ハンドル等）として持つ。`Settings` には一切載せない＝`save()` しない。
- 変更時のフックは **`rebuild_visible_indices()`（[src/app.rs:22157](../src/app.rs)）のみ**。色や
  許容を動かしたら `rebuild_visible_indices` を呼んで可視集合を作り直すだけ（スキャンは再実行しない）。

**「候補キャッシュ + 差分スキャン」モデル（Codex 再々レビュー P2）**: 粗いフォルダ識別
（folder generation）だけで「このフォルダは済」と判定すると、**同一フォルダ内でも scan 対象アイテム
集合が変わるケース**（スタック表示の ON/OFF、既存ファセットとの併用、検索/タグビュー、
ファイル追加後）を取り逃がす。そこで `ScanPalettes` を「済/未済」フラグではなく
**候補キャッシュ**として扱う:

- フィルタ起動（や items 変化）のたびに、**現在の `items` の各 `GridItem::perf_key()` を引いて
  missing / stale（mtime/file_size 不一致）なものだけを抽出 → ② でそれだけスキャン → 完了後に
  一括反映**（§2 ①②、§7 の「完了時一括反映」）。
- これで「いつ・どの単位で再スキャンするか」を folder 世代の粗い判定に頼らず、**アイテム単位の
  missing/stale 判定**で正確に決められる。スタック畳み・ファセット併用・ファイル追加でも安全。
- 補助的に `ScanScopeSignature`（view kind + item count + Σ(perf_key, mtime, file_size) のハッシュ等）を
  持って「前回スキャンと同一スコープか」を O(1) で先判定し、変化時だけ差分計算に入ってもよい。
- **保持/破棄**: フォルダ移動・フィルタ解除でクリア。**任意**で直近 1–2 フォルダ分を LRU 保持
  （メモリは 1 万枚 × ~数十バイトで <1MB、安い）。LRU 再利用時も §4.2 の mtime/file_size 検証は必須。
- 再訪時、サムネが catalog に残っていればスキャンは速い（WebP デコードのみ）。

---

## 7. UI 統合

- **エントリ**: ファセットバー（[details-view-and-filter-plan.md](details-view-and-filter-plan.md)）に
  「画像色」の絞り込みコントロールを**見た目上は並べる**が、状態は `ColorFilterState`（§6）に持ち、
  `settings.facet_filter` / `settings.save()` は経由しない。種類/拡張子/★/タグ等と同じ
  「現在の表示を絞り込む」枠の隣に出すイメージ。動画/フォルダは対象外なので、UI 名も
  画像のみの色フィルタであることが伝わる表現にする。
- **コントロール**: カラーピッカー（egui、HEX/RGB 入力可）+ 許容範囲スライダ + クリア。
- **スキャン中 UI**: 色フィルタを有効化した瞬間に「画像色をスキャン中… N/M」+ プログレス +
  キャンセル。キャンセルしたらフィルタ未適用に戻す。
- **大量時確認 UI**: missing/stale が非常に多い場合は即スキャンせず、「未スキャンの画像 N 件」を
  明示して開始/キャンセルを選ばせる。初期実装では 2,000 件以上で確認。
- **未スキャンの扱い（初版は固定仕様・Codex 再レビュー P3）**: オンデマンドなので
  「フィルタ ON = 即スキャン」。**スキャン完了時に一括で絞り込みを反映**し、それまでは現在の
  一覧を据え置く（途中で順次反映してグリッドが動き続けると体感が不安定なため）。**キャンセル時は
  未適用に戻す**（フィルタを掛ける前の一覧のまま）。進捗バー + キャンセルで待ち時間を可視化する。
  「順次反映」は将来オプションとして検討（初版では採らない）。
- **グリフ/配色**: UI 文言は `scripts/check_ui_glyphs.py` を通す。見た目変更は egui_kittest
  スナップショット更新対象（[ui-snapshot-policy.md](ui-snapshot-policy.md)）。

### 7.1 パレット表示（companion・UI スレッドで decode しない）— Codex P2

Eagle はサムネ/プレビュー下に**抽出済みパレットをスウォッチ列で表示**し、ホバーで
`#212E63 (54.0%)` を出す。mIV でも**現在フルスクリーンで見ている 1 枚**のパレットを
メタデータパネル（[src/ui_metadata_panel.rs](../src/ui_metadata_panel.rs)、AI/EXIF と同じ枠）に
スウォッチ + ホバー `#HEX (xx.x%)` で出せる。永続化不要。

- **UI スレッドで大画像を同期 decode/scan しない（必須）**。画素源の優先順は（Codex 再レビュー P2）:
  - **CPU 側に decode 済みバッファが残っている場合のみ**それを再利用して抽出する（追加 decode なし）。
  - **GPU テクスチャしか無い場合は readback しない**（UI スレッドで GPU→CPU 読み戻しは禁止）。
    この場合は **1 アイテム分の worker** を spawn し、サムネ or 元画像から抽出して完了後に表示する。
  - いずれにせよ、メタデータパネルの描画関数の中で同期デコード / GPU readback は絶対にしない。
- スウォッチクリック → その色で画像色フィルタ起動（「この色に似た画像を探す」が 1 クリック）。
- 在メモリスキャン済みフォルダなら、その画像のパレットを再利用（再抽出も不要）。

---

## 8. リリース / 永続データ判断

- **永続データを追加しない**。スキーマ変更・マイグレーション・設定永続化が**いずれも不要**。
  ([catalog-design.md](catalog-design.md) の `thumbnails` も触らない。)
- よって「リリース済み/未リリース」の移行判断は**発生しない**（在メモリのみ）。

---

## 9. 対象ビュー

- per-folder DB を引かなくなったので、**原理的には Ctrl+G / タグ / 読書履歴ビューでも動く**
  （各アイテムの画素を直接スキャンするだけ）。Codex P1-③ の構造的ブロッカーは消えた。
- ただし今回リリースでは**通常フォルダ / ZIP / PDF 表示のみ有効**に固定する。
  多フォルダ集約ビュー（`items_are_global_search_view` / `items_are_tag_view` /
  `favsearch.on_results_grid()`、読書履歴、ドライブ一覧）では、画像色メニューを出さない。
  フルスクリーンのスウォッチから起動しようとした場合も、案内トーストを出して有効化しない。
- 理由: 多フォルダ集約ビューはアイテム数が非常に多くなり得るうえ、検索結果をさらに画像色で
  絞る時の期待挙動・進捗説明・マニュアル記述が重くなるため。開放は件数上限 / 確認 UI /
  ソートや結果説明をまとめて後続で判断する。

---

## 10. テスト方針

- **抽出の決定性**: 既知の小画像（単色 / 2 色 / グラデーション / 透過縁）で期待パレットと `ratio`
  を unit test。`App` 構造体に紐づくテスト（`src/app/tests.rs`）は `--lib` では走らないので
  `cargo test --bin mimageviewer-core` で実行する（純ロジックは通常の `cargo test` でよい）。
- **知覚マージ**: グラデーション画像で主色がビン割れせず 1 色にまとまり ratio が閾値を超えること。
- **ΔE / マッチ**: 既知 sRGB ペアで ΔE 検証。tolerance / ratio_floor のゲート境界。
- **スキャン worker**: キャンセルで即停止、進捗カウントの単調増加。
- **結果適用整合（核心・Codex 5 巡目 P2）**: スキャン完了時、
  - **palette 結果は `ScanScopeSignature` が変わっていても候補キャッシュへ merge される**こと
    （perf_key + mtime/file_size で同定でき、無駄にならない）。
  - ただし **`scan_id` / `ScanScopeSignature` が起動時と不一致なら visible 一括反映しない**こと。
    具体的には: スキャン中にスタック切替/ファセット変更/ファイル追加で items を変えると、古い
    スキャンの完了でグリッドが確定せず、現在 items で missing/stale を再判定して再スキャンが走る。
- **画素入手フォールバック**: cache_map ヒット / catalog ヒット / どちらも無し（必要時デコード→縮小）の
  3 経路で同等のパレットが得られること。
- **鮮度検証（必須仕様・Codex 再々レビュー P2）**:
  - 同じ `ColorPaletteKey`（perf_key）でも **`mtime`/`file_size` が変わったら在メモリ palette を
    再利用しない**（同名差し替え / LRU 再訪のシナリオ）。
  - **`cache_map` / catalog の stale WebP を拒否**（mtime/file_size 不一致）して元画像へフォールバック
    することを検証（[src/thumb_loader.rs:916](../src/thumb_loader.rs) と同じ判定が効いていること）。
- **候補キャッシュの差分スキャン**: items にアイテム追加 / スタック切替で集合が変わったとき、
  missing/stale だけがスキャンされ、既存の fresh な palette は再利用されること（§6）。
- **多フォルダ gate**: 集約ビューでの有効/無効/上限挙動（§9 の決定に追従）。
- **UI スナップショット**: 色ファセット UI + スキャン進捗（[ui-snapshot-policy.md](ui-snapshot-policy.md)）。
- **perf smoke**: スキャン中に UI ヒッチが出ないこと（`scripts/perf_smoke.sh`）。

---

## 11. 段階実装（フェーズ）

1. **Phase 1 — 抽出ロジック（UI なし）** 初期実装済み
   - `extract_palette`（量子化 + 知覚マージ + 再割当）+ sRGB→LAB + ΔE76 + マッチ関数。純ロジック。
   - 画素入手ヘルパー（cache_map / catalog / 必要時デコード→縮小 のフォールバック）。unit test。
2. **Phase 2 — 色スキャン worker + 最小 UI** 初期実装済み
   - キャンセル/進捗付きスキャン（`execute_search` パターン）。候補キャッシュ `ScanPalettes` +
     差分スキャン（missing/stale のみ）+ `scan_id` / `ScanScopeSignature` による結果適用整合（§3.2, §6）。
   - 色ファセット（ピッカー + 許容範囲スライダ）でグリッド絞り込み。スキャン中の進捗 UI。
   - 通常フォルダ / ZIP / PDF 限定で開始。
3. **Phase 3 — companion + 仕上げ** 一部実装済み
   - 実装済み: メタデータパネルのパレット表示（§7.1、decode 済み CPU バッファのみ）+
     スウォッチクリックで画像色フィルタ起動、perf 計装、大量 missing 時確認 UI、画像色表記。
   - 後続: 集約ビュー開放の可否判断（§9）、スナップショット、マニュアル/製品ページ更新。

---

## 12. 将来拡張・未解決論点

- **オプションの永続キャッシュ**: 再訪時の再スキャンが実用上つらいと分かったら、その時に
  `catalog.db` 等への保存を**オプトイン**で追加検討（最初は持たない）。
- **複数色 AND/OR**: 「赤 AND 青を両方含む」。パレットマッチを色ごとに AND するだけ。
- **色の強さでソート**: `best ΔE × ratio` でスコア化して並べ替え。
- **HSV 円 / 明度 / 彩度フィルタ**: 「暖色系」「モノクロ」「鮮やか」のレンジ指定。
- **動画**: 代表フレーム 1 枚から抽出すれば同じ枠で扱える。

---

## 13. まとめ

- Eagle は「取り込み時にパレットを事前計算 → 検索時はパレット照合のみ」で速い（推定）。
- mIV は**事前計算・永続化はせず**、色フィルタを使う瞬間に現在の表示分を**オンデマンドで
  スキャン**して在メモリのパレットを作る（Ctrl+F と同じ操作感、進捗 + キャンセル）。
- これにより**スキーマ・マイグレーション・設定追加が一切不要**になり、Codex の P1 指摘群が
  構造的に解消する。残コストは「再訪時の再スキャン」のみで、在メモリ候補キャッシュ + 必要時
  デコード（JPEG のみ DCT 縮小）で緩和、たまに使う機能としては妥当。
- 抽出は量子化 + **知覚マージ + 再割当**（ビン割れ対策、Codex P2）、照合は CIELAB ΔE76 +
  ratio_floor（主要色ゲート＝Eagle の「多く使われている色」挙動）。
- 重い処理はすべて worker、UI スレッドは止めない。
