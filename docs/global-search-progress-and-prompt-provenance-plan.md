# Ctrl+G 検索の進捗表示と AI プロンプトの出所ルール — 計画

- 状態: **設計確定 / 未実装** (2026-09-18)。
- 担当: 実装・テストは Codex Sol xhigh、ブリーフ・レビュー・統合は ClaudeCode。
  Codex のトークン上限がリセットされてから着手する。
- 関連: [search-architecture.md](search-architecture.md) §4.11 / §5.3、
  [search-container-item-redesign.md](search-container-item-redesign.md)、
  [next-release-backlog.md](next-release-backlog.md) §1.252 / §1.253。
- 作業は 2 つに分ける。作業 1 は再索引なしの小さな塊、作業 2 は INDEX_VERSION を上げる
  データ形式の変更。順に出す。

## 0. 経緯と観測 (出どころ付き)

### 0.1 利用者の報告 (2026-09-18)

Ctrl+G で `glasses -genshin` を検索すると「検索中」のまま進まないように見えた。
数分後に結果が出て、実際には処理中だったと利用者が確認した。

### 0.2 perf ログ (`%APPDATA%\mimageviewer\logs\perf_events.jsonl`、同日セッション、t=60120〜60465)

| 経過 (Enter = 0 s) | 観測 |
| --- | --- |
| 0.0 s | `text-input-key key=Enter` (通常ログ)。Enter はクエリ再実行扱いで 300 ms デバウンスに入る |
| 0.0〜4.4 s | `frame.begin` の `prev_outside_ms=4282`、`prev_update_ms=0.57`。UI スレッドは update の外で寝ていた。直前フレームの `prev_frame_causes` に `global_search_ui.rs:2611` (デバウンス用 `request_repaint_after`) と即時 repaint の要求が同居 |
| 4.4 s | 検索ワーカー起動 (`respawn skip … done=false` の rebuild ログ) |
| 4.7 s〜5 分 06 s | `search/rating_bulk_lookup count=0` が約 2.6 秒間隔で 115 回。= 候補 2,000 件ごとの空進捗バッチで、ヒット 0 件のまま約 23 万件を走査 |
| 5 分 06 s〜5 分 44 s | ヒットが 1〜92 件ずつ 95 バッチ。この区間は 2,000 件あたり 0.1〜0.5 秒 |
| 約 5 分 44 s | 完了 |
| 全期間 | `tail_repaint` が `global_search_ui.rs:1851` (検索中の毎フレーム `request_repaint`) を理由に約 5 ms 間隔で続く |

### 0.3 索引の実測 (`%APPDATA%\mimageviewer\fts_index`、同日)

| 項目 | 値 |
| --- | --- |
| ディレクトリ全体 | 58 GB |
| 最大セグメントの `.store` (STORED 原文) | 60.8 GB |
| `.idx` (posting) 合計 | 0.78 GB |
| `meta.json` の `max_doc` 合計 / 削除済み | 790,864 / 12,099 |

1 文書あたり圧縮後およそ 77 KB。設計時のプロトタイプ ([bench_search.rs](../src/bin/bench_search.rs)) は
1 文書約 2 KB の合成コーパスで計測しており、実データは 2 桁大きい。

### 0.4 サンプル PNG をスクリプトで読んだ結果 (4 枚、別フォルダ・別月)

いずれも tEXt チャンクは `parameters` (約 1 KB) / `prompt` (約 280 KB) / `workflow` (約 290 KB)。
`prompt` JSON の中で最大の文字列は 4 枚とも同一の **266,470 バイト・1,371 行**で、
`DPRandomGenerator` ノードの `inputs.text` (ワイルドカードのテンプレート)。CLIPTextEncode の
`text` はこのノードへのリンクになっている。テンプレートには `glasses` が含まれる。
`parameters` は A1111 形式で、正側プロンプト・`Negative prompt:`・`Steps: … Model: … Version: ComfyUI`
まで揃った**生成後の値**だった。

### 0.5 ソースで確認した構造

- [global_search.rs](../src/global_search.rs) `run`: Tantivy クエリは含める語だけで組む。除外語は
  候補 1 件ずつ `doc_text_for_target` で原文を引いて `matches_with_mode` で判定する後段フィルタ。
  `HARD_MAX=10_000` は採用ヒット数の上限なので、除外でほぼ全滅する検索は候補を最後まで歩く。
  空の進捗バッチは `offset % (PAGE_SIZE*4) == 0` のときだけ送る (2,000 件ごと)。
- [fts_index.rs](../src/fts_index.rs) `search_page`: `TopDocs::with_limit(500).and_offset(offset)` で
  毎ページ全候補を採点し直す。候補 1 件につき `searcher.doc` が 2 回 (path 取得と本文取得)。
- [png_metadata.rs](../src/png_metadata.rs) `extract_text_from_ref_node`: CLIPTextEncode の `text` が
  リンクなら参照先ノードの `text` / `string` / `value` … を**そのまま**プロンプトとして採用する。
  参照先が何をするノードかは見ない。
- [global_search_ui.rs](../src/global_search_ui.rs) `poll_global_search_debounce`: `ctx` を受け取らず、
  起床の再要求をしない。`request_repaint_after(DEBOUNCE_MS)` は `query_changed` のパスで 1 回だけ。
- vendor egui `Context::request_repaint_after` は、そのパスで既により短い遅延 (即時を含む) が
  要求済みなら backend へ転送しない。さらに `begin_pass_repaint_logic` が毎パス `repaint_delay` を
  `MAX` に戻す。**1 回きりの `request_repaint_after` は、次のパスが先に来ると消える。**

## 1. 問題の分解

| # | 問題 | 影響 | 作業 |
| --- | --- | --- | --- |
| A | 走査の進捗が UI に出ない。アドレス欄は `(n 件 / 検索中)`、空グリッドは「検索中」だけ | 処理中か固着か見分けがつかない | 1 |
| B | デバウンスの起床が消え、OS イベントが来るまで検索が始まらない | 今回 4.4 秒。表示が「0 件」のまま止まって見える | 1 |
| C | 検索中に毎フレーム `request_repaint` | 5 ms 間隔で描画し続け、UI スレッド 1 コアを検索時間中占有 | 1 |
| D | ワイルドカードのテンプレート全文をプロンプトとして索引・保存 | 索引 58 GB、候補 1 件 1 ms 超、`glasses` が眼鏡の有無と無関係に全画像へ一致 | 2 |
| E | 除外語が後段フィルタでしか効かない | D と組み合わさると候補全件走査 (今回約 40〜60 万件) | 2 で候補が縮めば実害は消える。除外を Tantivy へ押し込む案は採らない (下記) |

E について: `NgramTokenizer` は position を常に 0 で吐くため、NOT を `MustNot` で Tantivy に渡すと
「全 bigram を含むが部分文字列としては含まない」文書を過剰除外する
([search-architecture.md](search-architecture.md) §5.3 末尾)。位置情報を持たない現行索引では
安全に押し込めないので、E は D を直して候補集合そのものを正すことで解消する。

## 2. 作業 1: 進捗表示・デバウンス再武装・描画間引き (再索引なし)

### 2.1 表示仕様

- ワーカーは既に各 `Batch` で `scanned_candidates` / `valid_hits` を送り、UI は
  `global_search.total_scanned` / `total_valid` に保持している。表示に使っていないだけ。
- アドレス欄 (`update_global_search_address`): 検索中は
  `🌐 アイテム検索: "glasses -genshin"  (3 件 / 検索中 · 412,000 件を確認)` の形にする。
  分母が取れる場合 (2.3) は `412,000 / 780,000 件を確認`。桁区切りを入れる。
- 空グリッドのプレースホルダ ([ui_main.rs](../src/ui_main.rs) の `"検索中"` 2 か所): 同じ件数を
  `検索中… 412,000 件を確認 (ヒット 3 件)` の形で出す。ヒットが出ればグリッドに items が出るので、
  以降はアドレス欄だけが進捗を持つ。
- 集約ビュー / ドリルダウン中も `is_searching()` なら同じ文言をアドレス欄に付ける。
- 完了時は従来どおり `(n 件)`。打ち切り (`truncated`) の表示は既存挙動を維持する。

### 2.2 進捗送信の間隔 (ワーカー側)

- 空バッチの条件 `offset % (PAGE_SIZE * 4) == 0` を「1 ページごと」または「前回送信から 300 ms 経過」に
  変える。今回の実データでは 2,000 件 = 約 2.6 秒に 1 回しか動かず、進捗表示としては粗い。
- 送信は `crossbeam_channel::unbounded` なのでワーカー側のコストは無視できる。UI 側は
  `MAX_EVENTS_PER_FRAME=8` の枠内で drain する (既存)。

### 2.3 分母 (任意、計測してから採否を決める)

- ページングの前に同じ `BooleanQuery` を Tantivy の `Count` コレクタで 1 回数え、
  `SearchStreamEvent::Batch` に `total_candidates: Option<usize>` を足す (または `Started` イベント)。
  採点なしの 1 走査なので軽いと見込むが、**数値は未計測**。perf イベント `search/count_candidates`
  (ms, total) を入れ、今回と同規模 (候補数十万件) で 200 ms を超えるなら不採用にする。
- 分母は「Tantivy の候補数」であり最終ヒット数ではない。表示は「件を確認」で統一し、
  「件中」のような残件表現は使わない。

### 2.4 デバウンス起床の再武装

- `poll_global_search_debounce(&mut self, ctx: &egui::Context)` にし、変更が pending で期限前なら
  毎パス `ctx.request_repaint_after(remaining)` を要求する。`query_changed` 側の 1 回きりの要求は
  残してもよいが、所有者は poll 側とする (起床の責任を 1 か所に置く)。
- 根本原因は 0.5 の最後の項目。**同じ罠は他の 1 回きり `request_repaint_after` にもある**ので、
  実装時に `grep -n "request_repaint_after" src/` で同型を列挙し、毎パス再要求していない箇所を
  一覧にして報告する (この作業で全部直す必要はない。§1.252 に残す)。
- CLAUDE.md の「UI / スクロール」節または [ui-responsiveness.md](ui-responsiveness.md) に
  「egui の `request_repaint_after` は 1 パス限り。期限まで毎パス再要求する」を 3 行で追記する。

### 2.5 検索中の描画間引き

推奨: **ワーカーが UI を起こす**。`IndexerManager::spawn_search` に
`wake: Option<Arc<dyn Fn() + Send + Sync>>` を足し、`global_search::run` は `tx.send` の直後に
呼ぶ。UI 側は `ctx.clone()` から `move || ctx.request_repaint()` を渡す
(`egui::Context` は別スレッドからの `request_repaint` を公式にサポートし、`outstanding` カウントは
パスをまたいで残る)。`poll_global_search_events` は

- このフレームでイベントを処理し、まだ残っている可能性がある場合 (`MAX_EVENTS_PER_FRAME` 到達、
  rating lookup による 1 バッチ制限で break) だけ `ctx.request_repaint()`。
- それ以外は即時要求をやめ、保険として `ctx.request_repaint_after(1000ms)` を毎パス要求する
  (起床消失時の上限を 1 秒に抑える。2.4 と同じ規律)。

代替: ワーカーを触らず `request_repaint_after(100ms)` に置き換える。結果の遅延は最大 100 ms で
許容範囲だが、寝ている時間も 100 ms ごとに起きるので推奨案より劣る。

`IndexerManager` は egui に依存しないまま保つ (callback 型で受ける)。

### 2.6 テスト

- 状態遷移: 合成 `Batch { hits: [], scanned_candidates: 412_000, valid_hits: 3 }` を流し、
  `update_global_search_address` 後のアドレス文字列に `412,000` が入ること。
  既存 `aggregated_address_marks_unsettled_results_as_searching` ([src/app/tests.rs](../src/app/tests.rs))
  の隣に置く。完了 (`Done`) 後は件数表現が消えること。
- 進捗間隔: `global_search::run` を小さな一時索引で走らせ、除外語で全滅するクエリでも
  空バッチが 1 ページごと (または 300 ms ごと) に届くこと。
- デバウンス: `poll_global_search_debounce` が pending 中に `request_repaint_after` を要求することを
  `egui::Context` の `has_requested_repaint` 相当で確認する (kittest を使う既存テストの流儀に合わせる)。
- 描画間引き: 検索 pending 中でイベントが無いフレームで即時 `request_repaint` を要求しないこと。
- 手動 (利用者): `glasses -genshin` 相当の重い検索でアドレス欄の件数が増え続けること、
  検索開始までの空白が体感で消えていること、検索中の CPU 使用率が下がっていること。

### 2.7 更新する文書

- [search-architecture.md](search-architecture.md) §5.3: 進捗イベント、起床方式、アドレス欄の表示。
- [spec.md](spec.md) の Ctrl+G 節: 検索中の表示文言。
- `htdocs/mimageviewer/manual/` の検索ページ: 「検索中は確認済み件数が表示されます」の 1 行。
- CLAUDE.md または ui-responsiveness.md: 2.4 の規律。

## 3. 作業 2: AI プロンプトの出所ルール (INDEX_VERSION 9 → 10)

### 3.1 原則

**ファイルが「生成に使われた」と証明できるテキストだけを索引する。ノードが変換する前の入力は
索引しない。** ComfyUI の `prompt` JSON はノードの入力しか記録せず出力は残らない。
CLIPTextEncode の `text` がリンクなら、そこにあるのは上流ノードの入力 (テンプレート、seed、
ファイル名) であって生成に使われた文字列ではない。これはワイルドカード、結合、置換、翻訳、
ファイル読み込みなど**テキストを変換するノード全部に共通**の性質で、特定ノード名の問題ではない。
乱数ノードを名指しで除く設計は採らない (禁止リストは際限がなく、利用者のテンプレートに依存する)。

### 3.2 出所ルール (グラフ構造だけで判定)

`extract_text_from_node` / `extract_text_from_ref_node` の結果に出所を付ける。

| 出所 | 扱い |
| --- | --- |
| CLIPTextEncode の `text` がリテラル | **解決済み**。従来どおり索引 |
| リンク先が素通しノード (許可リスト: `PrimitiveNode`、文字列リテラル系。実装時に fixture と現物で確定) | 解決済み |
| リンク先がそれ以外 (未知を含む) | **未解決入力**。3.3 の代替があればそちらを使い、無ければ 3.4 の安全弁を通す |

許可リストは「素通し」だけなので小さく安定する。未知ノードは安全側 (未解決) に倒れる。
既存 fixture (`tests/fixtures/ai_metadata/comfyui_*.png`、`sd_parsers_upstream/ComfyUI/*.png` の 9 枚) は
CLIPTextEncode が全部リテラルなので (スクリプトで確認済み)、このルールで抽出結果は変わらない。

### 3.3 解決済みソースの優先 (ファイル単位)

- ComfyUI の正側プロンプトが解決済みで取れない場合、同じファイルの A1111 形式 `parameters`
  チャンクがあれば、その正側プロンプトを `png_prompt_text` の本文にする。Negative は従来どおり
  索引しない。モデル名 / サンプラーなどの facet 用パラメータは ComfyUI JSON から従来どおり取る
  (`parameters` の `Model:` と重複しても害はない)。
- 現状 `parameters` は「消費されなかったチャンク」として ComfyUI 本文の後ろに生で足されている。
  順序と役割を入れ替える (主本文 = 解決済みソース、ComfyUI JSON = パラメータ)。
- ワイルドカード系で出力を保存するノード (キー名が `populated_text` のような出力を表すもの) は、
  そのキーを解決済みとして扱う。キーの一覧は許可リスト同様に小さく保つ。

利用者の現物はこの節だけで解決し、ノード名の知識は要らない。

### 3.4 安全弁 (内容で判定、最後の砦)

3.3 の代替が無い未解決入力を索引に入れる条件:

- テンプレート構文を含まない。Dynamic Prompts の variant ブロック `{ … | … }` (改行を挟む形を含む) と
  `__name__`。**`{word}` 単独は NovelAI の強調構文なので弾かない**。`(word:1.2)` も弾かない。
- サイズ・行数の上限内 (初期値 8 KiB / 64 行。実装時に fixture と現物の分布から決め、定数に理由を書く)。
- 「消費されなかったチャンクを全部足す」経路にもチャンク単位の上限 (初期値 16 KiB) を付ける。

上限で落ちたテキストは索引に入らないだけで、ファイルからは消えない。

### 3.5 データ構造と表示

- `ComfyUIMetadata` に「解決済みプロンプト」と「未解決入力 (出所付き)」を分けて持つ。
  `build_searchable_text` は解決済みだけを使う。
- メタデータパネル ([ui_metadata_panel.rs](../src/ui_metadata_panel.rs) は `extracted_prompts` を
  そのまま表示している) は、解決済みプロンプト (3.3 の代替を含む) を「プロンプト」として出し、
  未解決入力は「未解決の入力 (テンプレート、N KB)」のような見出しで折りたたむ。
  **テンプレートを「プロンプト」と表示しない**ことが要件。折りたたみの詳細は実装時に決める。
- Ctrl+F (フォルダ内検索) も同じ判別器を使うので、挙動が揃う。

### 3.6 再索引

- `fts_meta.rs` の `INDEX_VERSION` を 10 にする。起動時に `files` テーブルが落ちて自動で全件
  作り直しになる (既存の移行機構)。リリース済みデータの形式変更なので、この自動再構築が移行手段。
- コスト: 取り込みは PNG を丸ごと読む ([ingest_text.rs](../src/ingest_text.rs) `read_metadata_bytes`)。
  利用者の索引は約 79 万件、1 枚約 5 MB なので**およそ 4 TB の読み取り**。
- 軽くする案 (別件として計測してから採否): PNG は最初の `IDAT` までで打ち切る。tEXt/iTXt/zTXt は
  通常 `IDAT` より前にあるが、XMP (`iTXt XML:com.adobe.xmp`) を後ろに置くツールが無いとは言えない。
  採用前に、利用者のコレクションからサンプルして「`IDAT` の後にテキストチャンクがある PNG の割合」を
  スクリプトで測る。0 でなければ「先頭で見つからなかったときだけ全読み」の 2 段にする。
- ディスク: 旧セグメント 60 GB は再構築中も残り、Tantivy のマージ / GC で消える。ピークは
  旧 + 新 (新は 1 GB 級の見込み)。README の更新履歴に「初回起動時に索引を作り直す」注意を書く
  (⚠️ プレフィックスの対象)。

### 3.7 テスト

- fixture を足す (合成 PNG、`tests/fixtures/ai_metadata/`):
  1. CLIPTextEncode → リンク → 変換ノード (小さな `{a|b}` テンプレート 100 行) + `parameters` あり。
     期待: 本文 = `parameters` の正側プロンプト、テンプレートの語は索引に入らない、
     モデル名は ComfyUI から取れる。
  2. 同じ構造で `parameters` なし。期待: テンプレートは落ち、リテラルの CLIPTextEncode があればそれだけ残る。
  3. リンク先が `PrimitiveNode` (素通し)。期待: 従来どおり本文になる。
  4. 未解決だが短い平文 (結合ノード相当)。期待: 安全弁を通って残る。
  5. `parameters` が A1111 形式で `Version: ComfyUI`、`Model hash:` 空。期待: A1111 パーサが通る。
- 既存 9 枚の ComfyUI fixture の抽出結果が変わらないこと (回帰)。
- 安全弁の単体テスト: NovelAI の `{word}` は弾かない、`{a|b}` と `__x__` は弾く、行数上限。
- `search/ingest` 側: 1 文書の STORED 本文サイズを perf イベントか統計に出し、再索引後に
  `fts_index` の `.store` 合計が 1 桁 GB 未満に落ちたことを利用者環境で確認する。

### 3.8 更新する文書

- [search-architecture.md](search-architecture.md) §4.11: 出所ルール、解決済みソースの優先、安全弁、INDEX_VERSION=10。
- [spec.md](spec.md) 2143 行付近 (KSampler の positive / negative を辿る記述): リンク先の扱いを追記。
- `htdocs/mimageviewer/manual/` のメタデータ / 検索ページ: 「検索対象になるのは生成に使われた
  プロンプトで、ワイルドカードのテンプレートは対象外」の趣旨を実装用語なしで 1〜2 行。
- README 更新履歴 (リリース時): 索引作り直しの注意と、検索精度・索引サイズの改善。

### 3.9 非対象 / 将来

- 除外語の Tantivy 側評価 (E)。位置情報を持つ索引に変えない限り扱わない。
- `TopDocs` の offset ページングの二次コスト。候補集合が正しくなれば実害は小さい。
  残るなら「前ページ最後のスコアで cursor を切る」方式を別件で検討。
- 動画・PDF・サイドカーの本文は本計画の対象外 (同じ上限規律は将来揃える)。

## 4. 実施順と Codex への渡し方

1. 作業 1 を 1 コミット群で出す (2.1〜2.7)。再索引なし。実機確認は利用者。
2. 作業 2 は 3.6 の計測 (IDAT 後のテキストチャンクの割合) を先に済ませ、fixture を揃えてから
   INDEX_VERSION を上げる。Codex に出す前に作業 1 をコミットしておく
   (CLAUDE.md「区切りごとに小さくコミットする」)。
3. Codex ブリーフは本書を正本にし、`codex exec --model gpt-5.6-sol -c 'model_reasoning_effort="xhigh"'`
   で「本書 §2 を実装、§2.6 のテストを追加、§2.7 の文書を更新」と指示する。作業 2 も同様に §3。
4. レビュー観点 (ClaudeCode): 0.5 の各経路が計画どおり変わったか、症状パッチ (guard / retry /
   一括 reset) が混ざっていないか、`IndexerManager` が egui に依存していないか、
   fixture 5 種と既存 9 枚の期待値、`request_repaint_after` 同型の一覧報告。
