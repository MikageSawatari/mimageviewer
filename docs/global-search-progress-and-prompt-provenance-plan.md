# Ctrl+G 検索の進捗表示と AI プロンプトの出所ルール — 計画

- 状態: **作業 1 / 作業 2 とも実装・自動検証・独立レビュー済み** (2026-09-19)。
- 担当: 作業 1 の実装・テストは Codex Sol xhigh。2026-09-09 の役割決定後に着手したため、
  旧記載の ClaudeCode ブリーフ・レビュー・統合は `AGENTS.md` に従い Codex 親の設計主導・統合と
  実装担当とは別の Codex reviewer による独立レビューへ読み替え、編集前に構造合意を得た。
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

**作業 1 の採否 (2026-09-19)**: 分母は不採用。利用者の実索引を起動・変更せず、候補数十万件で
`Count` が 200 ms 以内かを安全に実測できないため。再索引を伴わない既存の累計
`scanned_candidates` だけを表示し、分母追加は実索引で条件を測れる別の検証枠へ残す。

### 2.4 デバウンス起床の再武装

- `poll_global_search_debounce(&mut self, ctx: &egui::Context)` にし、変更が pending で期限前なら
  毎パス `ctx.request_repaint_after(remaining)` を要求する。`query_changed` 側の 1 回きりの要求は
  残してもよいが、所有者は poll 側とする (起床の責任を 1 か所に置く)。
- 根本原因は 0.5 の最後の項目。**同じ罠は他の 1 回きり `request_repaint_after` にもある**ので、
  実装時に `grep -n "request_repaint_after" src/` で同型を列挙し、毎パス再要求していない箇所を
  一覧にして報告する (この作業で全部直す必要はない。§1.252 に残す)。
- CLAUDE.md の「UI / スクロール」節または [ui-responsiveness.md](ui-responsiveness.md) に
  「egui の `request_repaint_after` は 1 パス限り。期限まで毎パス再要求する」を 3 行で追記する。

**同型監査 (2026-09-19)**: `src/` の `request_repaint_after` 215 出現を関数単位で照合した。
今回直した Ctrl+G debounce のほか、facet 名フィルターの debounce と native video の遅延状態機械は
pending owner が毎パス残り時間を再要求している。残件は
`PreferencesState::mark_ui_font_changed` の UI フォントプレビュー 1 件。変更時の 160 ms 要求が
1 回きりで、`poll_ui_font_tasks` は 150 ms 未満のパスで残り時間を再要求しない。この作業では
検索外の挙動を変えず、[next-release-backlog.md](next-release-backlog.md) §1.252 に残した。

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

### 2.8 実装記録 (2026-09-19)

- `Batch` は候補ページごとに累計確認件数を送り、空ヒットでは rating DB lookup を行わない。
  worker の全イベントは channel send 成功後だけ UI wake callback を呼ぶ。callback は UI 層で
  ROOT viewport を明示し、`IndexerManager` は egui 非依存を保つ。
- debounce owner は期限まで毎パス再武装する。stream receiver は drain 残りがあり得るときだけ
  即時 repaint、それ以外の pending 中は 1 秒 backstop を毎パス再武装する。terminal 後は
  repaint を継続しない。
- アドレス欄は一覧・集約・ドリルダウンの検索中にヒット数と確認済み件数を表示し、空グリッドにも
  同じ進捗を表示する。Done 後は確認済み件数を外す。
- headless snapshot は追加していない。既存 snapshot harness に Ctrl+G streaming fixture がなく、
  今回の可視差分は App の address 文字列と `egui::FullOutput` の repaint deadline を直接確認する
  headless 回帰テストで固定した。
- focused は global search 20 件、address / repaint 8 件、metadata E2E 12 件が成功し、core check も
  exit 0。`scripts/test-full.ps1 -SuppressCrashDialogs` は本体 8,637 件成功・失敗 0・ignored 45、
  UI snapshot 52 件、IPC 57 件、Remote 122 件、vendor egui / egui-wgpu / eframe 25 / 9 / 15 件を含め
  `[test-full] PASS`、exit 0。fmt、UI glyph (0 件)、viewer-context audit、diff check も exit 0。
  証跡は `target/search-progress-252-logs/` に保存した。
- 独立 reviewer は worker / receiver / ROOT viewport の所有、drain と backstop、空 Batch、
  debounce 再武装、一覧・集約・ドリル表示、terminal 後の停止、同型監査を確認し、
  blocking / should-fix 指摘なし。確認用 build は作業 2 と同じ依頼内でまとめて 1 回行うため、
  作業 1 の checkpoint では実行していない。アプリと実索引は起動・変更していない。

## 3. 作業 2: AI プロンプトの出所ルール (INDEX_VERSION 9 → 10)

### 3.1 原則

**ファイルが「生成に使われた」と証明できるテキストを優先して索引する。ノードが変換する前の入力は
解決済みプロンプトへ格上げしない。** ComfyUI の `prompt` JSON はノードの入力しか記録せず出力は残らない。
CLIPTextEncode の `text` がリンクなら、そこにあるのは上流ノードの入力 (テンプレート、seed、
ファイル名) であって生成に使われた文字列ではない。これはワイルドカード、結合、置換、翻訳、
ファイル読み込みなど**テキストを変換するノード全部に共通**の性質で、特定ノード名の問題ではない。
乱数ノードを名指しで除く設計は採らない (禁止リストは際限がなく、利用者のテンプレートに依存する)。

ただし、既存の短い平文 fallback を失わないため、positive topology から到達した未解決入力のうち
§3.4 の安全弁を通ったものは、**未解決という出所を保ったまま**検索だけに採用する。UI では必ず
「未解決の入力」として折りたたみ、解決済みの「プロンプト」には表示しない。

### 3.2 出所ルール (グラフ構造だけで判定)

`extract_text_from_node` / `extract_text_from_ref_node` の文字列 `Vec` を、次の typed result へ置き換える。

- `resolved_prompts`: text、positive / negative role、`CLIPTextEncode` の直接 literal・許可済み
  passthrough・`parameters` positive fallback のいずれかを示す provenance を持つ。
- `unresolved_inputs`: text、positive / negative role、参照元 node class、実際に候補文字列を読んだ
  input key、CLIP `text` link の output index、§3.4 の内容 safety 判定 (検索採用 / template / 過大等) を
  持つ。topology 外の link は収集せず、存在しない unknown role 状態を safety に重ねて表現しない。
- 検索本文は positive の `resolved_prompts` を優先する。positive resolved が 1 件もなく、
  `parameters` fallback も無い場合だけ、positive かつ safety-accepted な `unresolved_inputs` を
  最後の砦として使う。negative は表示用に保持しても検索へ入れない。

| 出所 | 扱い |
| --- | --- |
| CLIPTextEncode の `text` がリテラル | **解決済み**。従来どおり索引 |
| リンク先が素通しノード (許可契約: node class + input key + link output index を fixture で固定) | 解決済み |
| リンク先がそれ以外 (未知を含む) | **未解決入力**。3.3 の代替があればそちらを使い、無ければ 3.4 の安全弁を通す |

許可リストは「素通し」だけなので小さく安定する。未知ノードは安全側 (未解決) に倒れる。
`populated_text` のようなキー名だけ、または任意ノードの `text` / `string` / `value` だけでは
解決済みにしない。今回 fixture で契約を固定できる `PrimitiveNode` の class / `value` / output 0 だけを
許可し、保存済み出力を持つ別ノードは class・key・output の実例を追加できるまで未解決のままにする。

KSampler class を認識した `saw_supported_sampler_topology` と「参照配列が有効か」「抽出文字列が空か」を
別に持つ。positive / negative が欠落・不正配列・未解決のどれでも、既知 KSampler topology がある限り、
空 `Vec` を根拠に全 `CLIPTextEncode` を positive として拾わない。topology 自体が無い従来 fallback だけは、
直接 literal の `CLIPTextEncode` に限定して解決済みとする。これにより既知 negative と無関係ノードを
positive へ救済しない。循環は role ごと (または root traversal ごと) の visited node と深さ上限で止め、
同じ node が positive / negative の両方から到達しても一方の role を潰さない。重複排除も
role + provenance + source 契約を保つ。
既存 fixture (`tests/fixtures/ai_metadata/comfyui_*.png`、`sd_parsers_upstream/ComfyUI/*.png` の 9 枚) は
CLIPTextEncode が全部リテラルなので (スクリプトで確認済み)、このルールで抽出結果は変わらない。

### 3.3 解決済みソースの優先 (ファイル単位)

- ComfyUI を認識したファイルでは、同居する `parameters` を parse 成否にかかわらず必ず consumed key に
  する。これを「未使用の非 AI チャンク」として生で再混入させない。
- ComfyUI の正側プロンプトが解決済みで取れず、同じファイルの `parameters` が A1111 形式として
  parse できた場合だけ、その **positive prompt** を resolved fallback にする。Negative と raw 本文は
  索引しない。ComfyUI 側に resolved positive が既にある場合も `parameters` は consumed のままで、
  positive / negative / raw を追加しない。
- モデル名 / サンプラーなどの facet 用パラメータは ComfyUI JSON から従来どおり取る。

利用者の現物はこの節だけで解決し、ノード名の知識は要らない。

### 3.4 安全弁 (内容で判定、最後の砦)

positive topology から到達した未解決入力を、未解決のまま検索に入れる条件:

- テンプレート構文を含まない。Dynamic Prompts の variant ブロック `{ … | … }` (改行を挟む形を含む) と
  `__name__`。**`{word}` 単独は NovelAI の強調構文なので弾かない**。`(word:1.2)` も弾かない。
- サイズ・行数の上限内 (初期値 8 KiB / 64 行。実装時に fixture と現物の分布から決め、定数に理由を書く)。
- 「消費されなかったチャンクを全部足す」経路にもチャンク単位の上限 (初期値 16 KiB) を付ける。
- negative 入力は、内容が短くても検索へ入れない。topology 外の link は収集しない。

上限で落ちたテキストは索引に入らないだけで、ファイルからは消えない。

### 3.5 データ構造と表示

- `ComfyUIMetadata` に `resolved_prompts` と `unresolved_inputs` を分けて持つ。
  `build_searchable_text` は positive resolved があればそれだけを使い、0 件の場合だけ
  safety-accepted な positive unresolved を使う。
- メタデータパネル ([ui_metadata_panel.rs](../src/ui_metadata_panel.rs)) は typed provenance を role 別に描き、
  positive の解決済みプロンプト (3.3 の代替を含む) を「プロンプト」として出し、
  未解決入力は「未解決の入力 (テンプレート、N KB)」のような見出しで既定閉じにする。
  **テンプレートを「プロンプト」と表示しない**ことが要件。折りたたみの詳細は実装時に決める。
- `detect_and_parse_outcome` を path / bytes / UI / Ctrl+F / Ctrl+G の単一判別境界に保つ。
  Ctrl+F と Ctrl+G は同じ `build_searchable_*` を通し、UI も同じ typed metadata を描画する。

### 3.6 再索引

- `fts_meta.rs` の `INDEX_VERSION` を 10 にする。v9 → v10 は `files` が空でも semantic rebuild を要求する。
  `files` の drop / 再作成、`user_version=10`、SQLite 内の durable `tantivy_rebuild_pending=1` を
  **単一 transaction** で確定し、DROP だけ済んだ crash 状態を作らない。`rebuilt_on_open()` は
  「この open で files を再作成した」という診断値に限定し、manager の判断は再起動後も残る pending を
  正本にする。meta DB が無い一方で旧 `fts_index` がある場合も、同じ transaction で pending を立てる。
- `open_stores_with_rebuild_sync` は legacy tag import 後、pending なら旧 `fts_index` directory を
  `remove_dir_all` する。NotFound は成功とし、それ以外の失敗では旧索引を開かない。空索引の open に
  成功した後だけ pending を clear する。clear 失敗も manager を生成せず marker を残す。したがって
  wipe/open/clear の途中終了は次回起動で再 wipe でき、旧文書を新しい検索意味で公開しない。
- startup は `StartupInitPending` の完了 payload を typed outcome にし、pending migration の失敗を
  `App::poll_startup_init` へ返す。既存 `show_feedback_toast` owner で「全文検索索引を再構築できず、
  次回起動時に再試行する」旨を 1 回表示して通常 UI へ進む。App に別の error field は足さず、
  その後の Ctrl+G は既存の generic unavailable 表示を使う。全面的な startup state 再設計は行わない。
- コスト: 取り込みは PNG を丸ごと読む ([ingest_text.rs](../src/ingest_text.rs) `read_metadata_bytes`)。
  利用者の索引は約 79 万件、1 枚約 5 MB なので**およそ 4 TB の読み取り**。
- **IDAT 打ち切りは不採用**。read-only sample 48 枚 / 14 folder では IDAT 後の text chunk は 0 件だったが、
  これは一般保証にならず、repository に `enc_a1111_text_after_idat.png` の既知 fixture がある。
  path reader が IDAT payload を seek で飛ばしつつ後置 chunk を読む契約と、ingest の full read を維持する。
- ディスク: 旧 60 GB と新索引は併存しない。通常移行は旧 directory の wipe を先に行ってから再構築する。
  wipe 失敗時は旧索引の open / 検索を止め、durable pending により次回起動で再試行する。

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
- topology の単体テスト: 既知 positive が未解決でも全 CLIP fallback を発火せず、known negative /
  無関係 CLIP を positive にしない。`parameters` は parse 失敗時も consumed、成功時は positive だけを
  fallback に使い、resolved Comfy positive がある場合も raw / negative を混入させない。
- 既存の不正画像、非標準 JSON 数値、循環参照、巨大 JSON / zlib 上限の安全性を維持する。
- NovelAI `{word}` 強調は検索に残り、negative は ComfyUI / `parameters` / NovelAI の全経路で混入しない。
  同じ fixture から UI metadata、Ctrl+F 用 `PerSourceText`、Ctrl+G の STORED `png_prompt_text` が
  同じ判定結果を使うことを固定する。
- v9 → v10 移行は tempdir 内に旧 `user_version` / `files` 行と有効な Tantivy directory marker を作り、
  `open_stores_with_rebuild_sync` を通して files drop、version bump、directory wipe + reopen、pending clear を
  確認する。files が空でも v9 は wipe すること、meta DB 不在 + 旧 index も wipe することを固定する。
  wipe 失敗は注入可能な小さい state transition で決定的に作り、manager / index を公開せず pending を維持し、
  次回成功時だけ clear することを確認する。typed startup failure は既存 toast owner に 1 回だけ表示され、
  worker の send 失敗 / disconnect / 同期 fallback の終了契約を壊さないことも固定する。
- `search/ingest` 側は 1 文書の STORED 本文 byte 数 (総量と png prompt) を perf 計測へ出し、fixture で
  巨大テンプレートが STORED text に残らないことを確認する。利用者環境での再索引後サイズ確認は別途行う。
- メタデータパネルに、resolved prompt と既定閉じの「未解決の入力」が同時に出る headless snapshot を足し、
  画像を目視確認する。

### 3.8 更新する文書

- [search-architecture.md](search-architecture.md) §4.11: 出所ルール、解決済みソースの優先、安全弁、INDEX_VERSION=10。
- [spec.md](spec.md) 2143 行付近 (KSampler の positive / negative を辿る記述): リンク先の扱いを追記。
- `htdocs/mimageviewer/manual/` のメタデータ / 検索ページ: 「検索対象になるのは生成に使われた
  プロンプトで、ワイルドカードのテンプレートは対象外」の趣旨を実装用語なしで 1〜2 行。
- release lead 向け引き継ぎ: README 更新履歴へ「初回起動時に索引を作り直す」注意 (⚠️ 対象) と
  検索精度・索引サイズの改善を書く。今回の開発作業では version / README 更新や公開は行わない。

### 3.9 実装記録 (2026-09-19)

- ComfyUI の抽出結果を `resolved_prompts` / `unresolved_inputs` に分け、role、node class / id、
  実際に読んだ input key、link output index、内容 safety を保持した。検索本文は resolved positive、
  A1111 `parameters` の positive 代替、短い平文の unresolved positive の順で選び、Negative、raw
  `parameters`、テンプレート、過大入力を混入させない。
- KSampler topology の有無と抽出結果の空を分離し、既知 topology の欠落・不正参照で全 CLIP fallback を
  発火させない。直接 literal と fixture で固定した `PrimitiveNode.value` / output 0 だけを resolved とした。
  5 枚の新 fixture と既存 9 枚、不正 / 循環 / 巨大 JSON、NovelAI 強調、path / bytes の共通判定で固定した。
- v9→v10 は `files` 再作成、version bump、durable pending marker を単一 SQLite transaction で確定する。
  cross-store owner は legacy tag import 後に旧 Tantivy directory を wipe し、新 index の open 成功後だけ
  marker を clear する。wipe / open / clear 失敗では旧 index を公開せず、startup の typed outcome から既存
  toast を 1 回表示して次回起動へ再試行を残す。
- ingest の perf event `stored_text_bytes` は文書ごとの STORED 原文総 byte 数と PNG prompt byte 数を出す。
  PNG full read と IDAT 後方の text 対応は維持した。read-only sample 48 枚 / 14 folder で後置 text 0 件でも
  一般保証にはせず、既存 post-IDAT fixture を理由に IDAT 打ち切りを採用していない。
- メタデータパネルは resolved prompt を通常表示し、未解決入力を既定閉じの別区画にした。headless golden は
  実装担当と親が目視確認し、独立 reviewer も typed provenance、検索優先順、negative 非混入、移行の
  fail-closed、startup 通知、STORED 計測、full-read、文書を確認して blocking / should-fix 指摘なし。
- focused は parser 56 件、ingest 16 件、indexer manager 11 件、fts meta 16 件、startup toast 1 件、
  UI snapshot 1 件が成功。`test-full.ps1 -SuppressCrashDialogs` は本体 8,653 件成功・失敗 0・ignored 45、
  UI snapshot 53 件、検索 E2E 12 件、IPC 57 件、Remote 122 件、vendor egui / egui-wgpu / eframe
  25 / 9 / 15 件を含め `[test-full] PASS`。fmt、UI glyph、viewer-context audit、diff check と
  `build-dev.ps1 -PreserveRuntime` も exit 0。証跡は `target/search253-logs/` に保存した。
- アプリ、元画像、通常設定、実索引は起動・変更していない。利用者環境での再索引時間と新しい index size は
  未測定。リリース lead は README 更新履歴へ初回再索引の注意と検索精度・索引サイズ改善を記載する。

### 3.10 非対象 / 将来

- 除外語の Tantivy 側評価 (E)。位置情報を持つ索引に変えない限り扱わない。
- `TopDocs` の offset ページングの二次コスト。候補集合が正しくなれば実害は小さい。
  残るなら「前ページ最後のスコアで cursor を切る」方式を別件で検討。
- 動画・PDF・サイドカーの本文は本計画の対象外 (同じ上限規律は将来揃える)。

## 4. 実施順と Codex への渡し方

1. 作業 1 を 1 コミット群で出す (2.1〜2.7)。再索引なし。実機確認は利用者。
2. 作業 2 は read-only sample 48 枚 / 14 folder と既存 post-IDAT fixture を照合し、full read 維持を
   確定した。5 fixture と v9 → v10 の isolated migration test を揃えてから INDEX_VERSION を上げる。
   作業 1 は `abeb513d9` として先にコミット済み。
3. Codex ブリーフは本書を正本にし、`codex exec --model gpt-5.6-sol -c 'model_reasoning_effort="xhigh"'`
   で「本書 §2 を実装、§2.6 のテストを追加、§2.7 の文書を更新」と指示する。作業 2 も同様に §3。
4. レビュー観点: `AGENTS.md` の役割対応により Codex 親 + 実装担当とは別の reviewer が、typed provenance、
   positive / negative topology、parameters consumed、fixture 5 種と既存 9 枚、共通判別境界、
   v9 → v10 wipe 移行、full-read 維持、UI snapshot を確認する。
