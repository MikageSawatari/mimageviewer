# v3.1.0 出荷前修正: 旧画像 API に generation 契約を持たせる

**共通ルール**

- 差分は作業ツリーに残し、**コミットしない**。
- `docs/next-release-backlog.md` は**触らない**。
- `/api/page` の既存挙動は変えない。**手本として真似る対象**であり、改造対象ではない。

## §1 何が足りていないか

直前の修正で、集約コレクションが使う旧経路
`/api/image` と `/api/image-info` の URL へ `generation` を足した
([app.js](../crates/remote-web/web/app.js) の `imageRequest()` 非 address 分岐と
`imageInfo()`)。しかしサーバ側
([http.rs](../crates/remote-web/src/http.rs) の `api_image` / `api_image_info`) は
その値を**読んでいない**。

結果、generation はブラウザのキャッシュキーを変えるだけで、
「同じ generation から異なる画素を返さない」というサーバ側の契約になっていない。
具体的には次が起こり得る。

1. 端末が generation `G` で画像を取得する
2. `rotation.db` が更新される
   (本体のメタデータ取り込みは背景 worker で走り、リモート取得 barrier は
   AI 活動 lease しか待たないので、**セッション中に起こり得る**)
3. ping が更新を観測する前に、`generation=G` を載せた次の要求が届く
4. `/api/image` が新しい回転を適用した画素を `G` の URL で返す

`/api/page` は同じ状況を
[http.rs](../crates/remote-web/src/http.rs) の `api_page` で塞いでいる。
`require_remote_state_generation()` を**処理の前と後の両方**で呼び、
応答に `X-mIV-Remote-State-Generation` を付ける。旧経路も同じ形にする。

## §2 直し方

### 2.1 サーバ

`api_image` と `api_image_info` を `api_page` と同じ形にする。

- `query_value(query, "generation")` を `api_page` と**同じ検証**
  (`valid_page_wire_identity`) で読む。欠落・不正は `api_page` と同じく 400。
- 画像処理の**前**に `state.library.require_remote_state_generation(&generation)`。
- 画像処理の**後**にもう一度同じ検査。どちらも失敗時は
  `store_error_response(error)` (= 409 `remote_state_generation_mismatch`)。
- 成功応答へ `X-mIV-Remote-State-Generation` を付ける。
- `Cache-Control: private, max-age=60` はそのまま。URL に generation が入ったので、
  版が動けばキャッシュキーごと変わる。

`api_page` が共通ヘルパを使っているならそれを使う。無ければ 2 経路で重複しない
程度に小さくまとめてよいが、`api_page` 側の既存コードは書き換えない。

### 2.2 クライアント

`imageRequest()` の**非 address 分岐**が返すオブジェクトへ
`remoteStateGeneration` を載せる。`requirePageResponseGeneration(request, response)` は
`request.remoteStateGeneration != null` のときだけ走るため、今は旧経路の応答が
**無検証**になっている。URL に載せる値と同じ変数を使い、両者がずれない形にすること。

`updateViewerImage` の次ページ先読み (`preload.src = imageRequest(...).url`) は
生の `<img>` なので 409 を読めない。これは cache を温めるだけの経路なので
**変更しない** (失敗しても表示には影響しない)。

409 の受け側は既にある。`pageResourceResponseError()` が
`remote_state_generation_mismatch` を見て
`applyRemoteStateGeneration(..., { reloadViewer: true })` を呼ぶので、
旧経路が 409 を返しても表示は自動で追従する。**この経路が実際に効くことを
確認してから**サーバ側を必須検証にすること。効かない形になっていたら
**止めて報告**する (壊れた画像が残るのは今より悪い)。

### 2.3 止めて報告する条件

- `api_image` / `api_image_info` を他の経路 (サムネイル・補正プレビュー・
  リモート以外の利用者) が generation 無しで叩いている場合。400 で壊れるので、
  勝手に「generation があれば検査する」という緩い形へ落とさず報告する。
- 初回 ping 前の空 generation で `/api/page` と違う挙動になる場合。
  **`/api/page` と同じ挙動に揃える**のが正で、旧経路だけ特例を作らない。

## §3 テスト

- HTTP レベルで、`/api/image` と `/api/image-info` が
  **stale generation を 409 `remote_state_generation_mismatch` で拒む**こと。
  `/api/page` に同種のテストがあれば同じ形で書く。
- 正しい generation なら 200 で返り、`X-mIV-Remote-State-Generation` が付くこと。
- Web テスト: 旧経路の request オブジェクトが `remoteStateGeneration` を持ち、
  応答検証が走ること。

## §4 検証

- `cargo fmt --all`
- `cargo test -p mimageviewer-remote`
- Web テスト (リポジトリ既定の `node --test --experimental-test-isolation=none`)
- `python scripts/check_ui_glyphs.py`

## §5 報告

1. 変更ファイルと変更内容
2. テスト結果
3. §2.3 の条件に触れた場合はその内容
