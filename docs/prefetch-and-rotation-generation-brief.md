# v3.1.0 出荷前修正 2 件 (リモート generation / 先読み方向)

出荷直前のレビューで見つかった 2 件を直す。どちらも既存の設計に沿った修正で、
新しい概念は増やさない。

**共通ルール**

- 差分は作業ツリーに残し、**コミットしない**。
- `docs/next-release-backlog.md` は**触らない**。
- IPC のワイヤ形式 (`PageGroup` / `/api/page` のレスポンス構造) は変えない。
- 本体側の回転・トリムの仕様 (回転中は表示トリム無効) は変えない。

---

## §1 [P1] 回転変更がリモート状態 generation を更新しない

### 1.1 症状

`crates/remote-web/src/store.rs` の `Library::snapshot()` は、変更監視の対象が
`settings.db` と `view_trim.db` だけになっている。`rotation.db` は
`Library::load` で開いて `LibraryState::rotation` に持ち、`image_rotation()` から
読んではいるが、**generation の計算には参加していない**。

そのためデスクトップ側でページを回転しても `remote_state_generation` が変わらず、

- 端末側は同じ `cacheKey` で旧向きの画像を保持し続ける
  (`pageResourceKey` は `generation` を含むので、generation が動かない限り再取得しない)
- 端末のキャッシュが退去した後に取り直すと、**同じ generation から違う画像**が返る
  (generation の契約が壊れる)

### 1.2 直し方

`snapshot()` の per-page 表示状態の監視に `rotation` を加える。

- 現在の引数 `include_view_trim: bool` は「表示トリムだけ」を指す名前になっているので、
  トリムと回転をまとめた名前 (例: `include_page_display_state`) に改名する。
  呼び出し側の真偽値は現状のまま (`favorites()` = false、`remote_state()` /
  `require_remote_state_generation()` = true)。
- その分岐の中で `view_trim` と同じ形で

  ```rust
  changed |= refresh_observed_database(&mut state.rotation, self.rotation_path.as_deref())?;
  ```

  を追加する。

`image_rotation()` の遅延オープンと `refresh_observed_database()` の遅延オープンが
両方 `open_observed_database()` を通り `data_version` を必ず埋めることは確認済み
なので、どちらが先に開いても整合する。この前提が崩れていたら**止めて報告**すること。

### 1.3 テスト

既存の
`one_generation_observes_favorites_and_view_trim_but_ignores_unrelated_settings`
(store.rs) を拡張し、名前も回転を含む形に改める。

- `Library::load` の前に `view_trim.db` と同じ要領で `rotation.db` を作る
  (`PRAGMA journal_mode=WAL;` + `CREATE TABLE rotations (path TEXT PRIMARY KEY, angle INTEGER NOT NULL);`)。
- 既存の favorites / sort_order / view_trim の各アサーションはそのまま残す。
- 末尾に「`rotations` へ 1 行書くと generation が変わる」ケースを足す。
- `settings.db` の無関係テーブルへの書き込みで generation が動かないことを見る
  既存アサーションも残すこと (回転を足したせいで無関係変更まで拾うようになっていない
  ことの担保)。

### 1.4 旧 `/api/image` 経路のブラウザキャッシュ

`crates/remote-web/web/app.js` の `imageRequest()` の非 address 経路 (集約
コレクションが使う旧 `/api/image`) と `imageInfo()` の `/api/image-info` は、
クエリに `path` / `w` / `epoch` しか持たない。サーバ応答は
`Cache-Control: private, max-age=60` なので、回転後も最大 60 秒は
ブラウザキャッシュから旧向きが出る。

`epoch` と同じ位置に `generation: state.remoteStateGeneration` を足して、
回転で URL が変わるようにする。

- サーバ側 (`api_image` / `api_image_info`) は `path` / `w` しか読まないので、
  追加パラメータは無視される想定。**もしクエリ名の allowlist や strict 検証が
  あって 400 になるなら、そこで止めて報告**すること (勝手に allowlist を広げない)。
- `web/*.test.mjs` に該当する既存テストがあれば追随させる。

---

## §2 [P2] 詳細表示の並び替えで AI 先読みの前後が逆になる

### 2.1 症状

`src/app.rs` の `ai_prefetch_targets()` は `collect_image_indices()`
(= `current_grid_order()` 由来、詳細表示では `details_order`) の**位置**で
前後を決めて対象を選ぶ。ところが表示モデル
`build_fs_prefetch_indicator()` (`src/app/prefetch_policy.rs:265` 付近) は
受け取った **raw item index の大小**で behind / ahead に振り分けている。

詳細表示で降順ソートや任意の列ソートをすると表示順と item index の大小が一致しない
ので、実際の「前ページ」と「次ページ」が左右逆に出る。

例: 表示順が `[5, 4, 3]` で現在ページが `4` のとき、`5` が behind (戻る側)、
`3` が ahead (進む側) だが、現在の実装は逆に置く。

### 2.2 直し方

判定の基準を raw item index から**表示順の位置**へ移す。

1. `prefetch_policy.rs` に位置だけを返す中核関数を切り出す。

   ```rust
   pub(crate) fn interleaved_prefetch_positions(
       pos: usize,
       n: usize,
       pf_forward: usize,
       pf_back: usize,
   ) -> Vec<usize>
   ```

   既存の `interleaved_prefetch_targets` は**シグネチャを変えず**、この関数の結果を
   `image_indices` で引き直す薄いラッパにする (既存呼び出し 3 箇所と既存テストは
   そのまま通ること)。

2. `build_fs_prefetch_indicator` の第 1 引数を `current_idx` → `current_pos` に、
   イテレータ要素を `(idx, state)` → `(pos, state)` に改名し、doc comment と
   「item idx は表示順に増える」というコメントを、**表示順の位置**を基準にする旨へ
   直す。中身の比較ロジック自体は変えない (位置なら単調なので今の比較で正しくなる)。

3. `App::final_ai_prefetch_indicator()` を、`ai_prefetch_targets()` 経由ではなく
   位置を保ったまま組み立てる形にする。`collect_image_indices()` は
   1 フレームに 1 回で済ませること (今の実装は `ai_prefetch_targets()` の中で
   1 回呼んでいるので、呼び出し回数を増やさない)。

   - 現在ページの位置が見つからなければ従来どおり `None`。
   - `(位置, 状態)` を `build_fs_prefetch_indicator` に渡す。
     状態の計算 (`fs_prefetch_page_state` / `is_idx_final_ai_done_or_skipped` /
     `final_ai_pending` からの active 判定) は今のまま raw item index で行う。

4. `ai_prefetch_targets()` は他 3 箇所 (`src/app.rs:39486` / `:51487` / `:51612` /
   `:51729`) がそのまま使うので、**返り値の意味を変えない**。必要なら §2.2-1 の
   位置関数を内部で共有するだけにする。

### 2.3 テスト

- `interleaved_prefetch_positions` の境界ケース (既存の
  `interleaved_prefetch_targets_boundary_cases` と同じ観点) を最低 1 つ。
- **非単調な表示順の回帰テスト**を追加する。これが本題なので必ず入れる。
  - 純関数レベル: 表示順の位置で渡せば behind / ahead が期待どおりになること。
  - App レベル: `settings.grid_view_mode = Details` かつ `details_order` を
    降順にした状態で、先読み対象の (位置, item index) の並びが表示順に沿うこと。
    `src/app/tests.rs` の既存 details_order テスト (7327 行付近 / 49320 行付近) の
    構築手順を流用してよい。App レベルの組み立てが GPU / テクスチャ状態に依存して
    現実的でない場合は、位置を返す新 helper のレベルまでで止めてよい (その旨を報告)。
- 既存の `build_fs_prefetch_indicator` 系テストとスナップショット fixture
  (`draw_fs_prefetch_indicator_snapshot_fixture`) は、単調な並びなので結果が
  変わらないこと。変わったら設計を間違えているので**止めて報告**する。

---

## §3 検証

- `cargo fmt --all`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `cargo test -p mimageviewer --lib prefetch`
- `cargo test -p mimageviewer-remote`
- Web テスト (リポジトリ既定の `node --test --experimental-test-isolation=none`)
- `python scripts/check_ui_glyphs.py`

`.\scripts\build-dev.ps1` は不要 (この後こちらで配布ビルドを回す)。

## §4 報告

1. 変更ファイルと、それぞれ何をどう変えたか
2. テスト結果
3. ブリーフから外れた判断があればその理由 (特に §1.4 と §2.3 の「止めて報告」条件)
