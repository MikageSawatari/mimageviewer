# RR-04: 一覧の上限を 10 万件へ上げる

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**
master は `841296e1` で取り込み済み。

利用者の判断: **上限は 10 万件**。本体の検索 (5000) / タグ (10000) に合わせるのではなく、
それより緩い上限にする。100 万件は下の byte 上限に確実にぶつかるので採らない。

**上限を上げる前に、先に 2 つの壁を潰す。順に別コミットにする (3 コミット)。**

- `docs/briefs/HANDOFF.md` と他の未追跡 brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、末尾のテストを走らせる。

---

## 前提: いま何が上限を必要にしているか (実測済み)

| 層 | 件数に対する費用 | 出所 |
|---|---|---|
| クライアント DOM | **なし** — `VirtualGrid` が可視セルだけ materialize する | `app.js` |
| サムネイル | **なし** — セルが materialize された時だけ 1 枚要求する | `bindThumbnail` |
| コンテナ列挙 (フォルダ / ZIP / PDF) | **ほぼなし** — 本体が一覧用に作る listing から `.take()` するだけ | `container.rs:3297` ほか |
| コレクション列挙 (検索 / タグ / レーティング / 履歴 / スマートフォルダ) | **1 件ごとに `canonicalize`**。実測 warm 30〜40 µs、cold 125 µs | `collections.rs:894` |
| wire JSON | **250 B/件** (実測) | — |
| HTTP 圧縮 | gzip 済み (`5e65e23e`)、15〜20 倍 | — |

つまり残る壁は **① コレクションの canonicalize** と **② IPC フレームの byte 上限**の 2 つだけ。

---

## コミット 1: コレクションの 1 件ごと `canonicalize` を並列化する

### 現状

`remote_entry_from_candidate` (`src/remote_ipc/collections.rs:893`) が 1 件ごとに
`path_guard::resolve_existing` を呼び、その中で `std::fs::canonicalize` が走る。
呼び出し元は `to_remote_entries_bounded` (`:882`) と、タグ経路のループ (`:787-799`)。

10 万件だと **warm 3〜4 秒、cold 12 秒**。remote-web 側の IPC 応答期限は
`RESPONSE_TIMEOUT = 10 秒` (`crates/remote-web/src/ipc_client.rs:39`) なので、
**cold では上限を上げた瞬間に期限切れになる**。

### 実装

`canonicalize` を**やめない**。住所の意味が変わる (DB の綴りがそのまま公開住所になる) ので、
一覧の項目と後で開いたときの identity がずれ得る。**同じ値のまま、並列で取る。**

- `rayon` は既に本体の依存にある。`to_remote_entries_bounded` の map を
  `into_par_iter().map(...).collect()` にする。**順序は保つこと**
  (`ParallelIterator::collect` into `Vec` は入力順を保つ)。
- `take(mapped_limit)` が先に効いて、**limit を超える分は canonicalize しない**現在の性質を保つ。
  並列化で「全件 canonicalize してから切る」形にしない。
- タグ経路のループ (`:787-799`) も、フィルタしてから上限までを集めて同じ並列 map に通す。
  `break` で早く止める性質を失わないこと。
- `to_remote_entries` (`:878`、`usize::MAX`) の呼び出し元 (`:1201`, `:1262`) は件数が小さいので
  そのままでよい。判断した理由をコメントに残す。

### テスト

- 並列化前後で**同じ順序・同じ内容**の `Vec<RemoteEntry>` になること (実在パスの一時ディレクトリで)。
- 上限を超える候補を渡したとき、`canonicalize` の呼び出し回数が上限 + 1 を超えないこと
  (呼び出し回数を数えられる形にするか、それが難しければ「limit までしか map しない」ことを
  型か構造で示すテストにする)。

---

## コミット 2: 応答が IPC フレーム上限を超えないようにする

### 現状

`MAX_RESPONSE_FRAME_BYTES = 64 MiB` (`crates/remote-ipc/src/lib.rs:31`)。
`write_frame` は `u32::MAX` までしか見ないので、本体は 64 MiB 超のフレームも書いてしまう。
受け側の `read_frame` は `FrameError::TooLarge` を返し、`reader_loop` が
接続を broken にして張り直す (`crates/remote-web/src/ipc_client.rs:1853-1875`)。
**壊れはしないが、その一覧は何度やっても失敗する**。

10 万件 × 250 B ≈ 25 MB なので通常は収まる。ただしコンテナは `page_groups` が
**同じ住所をもう一度持つ** (`ContainerPayload.entries` と `page_groups[].pages`) ので実質 2 倍。
深いパスや長い ZIP entry 名が重なると 64 MiB に届き得る。

### 実装

**producer 側 (本体) で、応答が上限を超えないことを保証する。**

- コンテナとコレクションの entry 構築時に、**直列化後の概算 byte 数を積算**し、
  予算を超えたらそこで打ち切る。予算は定数にし、`MAX_RESPONSE_FRAME_BYTES` に対して
  明確な余裕を残す (目安 40 MiB)。なぜその値かをコメントに残す。
- **コンテナは `page_groups` の分も数える**こと。entries だけで予算を組むと 2 倍で溢れる。
- 打ち切ったときは既存の `truncated` / `entry_limit` でそのまま報告する。
  Web は `「件数が多いため先頭 {entryLimit} 件を表示しています。」`と出す
  (`crates/remote-web/web/app.js:5132-5141`) ので、**byte で切れたときは `entry_limit` を
  実際に返した件数にする**。定数を返すと表示が嘘になる。
- 概算は「実際より小さく見積もらない」側に倒すこと (path / name の byte 長 + 固定 overhead)。

### テスト

- 長いパス (例: 400 文字) の候補を 10 万件与えたとき、`serde_json::to_vec` した応答が
  **`MAX_RESPONSE_FRAME_BYTES` 未満**であること。コンテナ (page_groups あり) と
  コレクションの両方で確認する。
- そのとき `truncated == true` で、`entry_limit == entries.len()` であること。
- 短いパスで 10 万件なら**切れない**こと (`truncated == false`、`entries.len() == 100_000`)。

---

## コミット 3: 上限を 10 万件へ上げる

- `CONTAINER_ENTRY_LIMIT` (`src/remote_ipc/container.rs:24`) を `100_000`。
- `MAX_REMOTE_COLLECTION_ENTRIES` (`src/remote_ipc/collections.rs:17`) を `100_000`。
- 既存テストが `MAX_REMOTE_COLLECTION_ENTRIES + 1` 件の fixture を作っている
  (`collections.rs:1286`, `:1739`, `:1783`, `:1943`)。**10 万件 + 1 の Vec を毎回作ると
  テストが重くなる**ので、上限を注入できる形にするか、小さい上限で同じ分岐を突くテストへ
  書き換える。件数そのものを検証している意図は保つこと。
- `MAX_REMOTE_TAG_CHOICES` (タグ名の一覧、2000) は**触らない**。項目一覧とは別物。
- 本体側の `SEARCH_RESULT_LIMIT` (5000) と `TAG_VIEW_RESULT_LIMIT` (10000) も**触らない**。
  これらは本体自身の上限で、リモートはそれ以上を受け取れない。結果として検索は 5000、
  タグは 10000 のままになる — それでよい (ローカルと同じ範囲になる)。
- **`page_groups` も同じ上限から作られる**ので、1000 ページ超の PDF / 画像フォルダが
  リモートでも最後まで読めるようになる。これがこの変更の主目的の 1 つ。

### ドキュメント

- `docs/web-remote-plan.md` の §12.3「コンテナは最大 1000 項目を返し」を新しい上限と
  byte 予算の説明へ更新する。「ページングはこの増分では行わない」は維持でよい。
- `htdocs/mimageviewer/manual/remote.html` の「できないこと」に 1000 件上限の記述があれば
  新しい上限へ直す。無ければ足さない (10 万件はほぼ全ての利用者に当たらないため、
  マニュアルを不安にさせる情報で埋めない)。

### テスト

- コンテナ / コレクションそれぞれで、10 万件までは truncate されないこと。
- 10 万件 + 1 で `truncated == true` になること。

---

## 実行するテスト

```
cargo test -p mimageviewer --lib remote_ipc
cargo test -p mimageviewer-remote
cargo test -p mimageviewer-ipc
node --test
cargo fmt --all -- --check
```

`cargo test -p mimageviewer --lib` の前に、必要なら
`cp vendor/ffmpeg/bin/*.dll target/debug/deps/`。

⚠️ `cargo test -p mimageviewer --lib` を**全件並列で**走らせると、
`remote_auto_trim_page_responses_share_the_harmonized_spread_height` が
実利用の APPDATA カタログを開く都合で稀に落ちる (マージ由来ではない既知の flake)。
落ちたら単独実行で確認すること。

## 報告してほしいこと

- 3 つの変更それぞれで何をしたか (コミットはこちらで行う)。
- byte 予算の値と、その根拠。
- 並列化で canonicalize 回数が上限を超えないことをどう保証したか。
- 10 万件のテストにかかる時間 (テストが極端に遅くなっていないか)。
