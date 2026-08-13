# コレクションを読書順にする (スマートフォルダをまたいで読む)

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。**`C:\home\mimageviewer` ではない。**

- **1 件 = 1 コミット** (3 コミット)。
- `docs/briefs/HANDOFF.md` と他の未追跡 brief は触らない。
- **commit は行わなくてよい** (worktree の `.git` は親リポジトリ側にあり sandbox から書けない)。
  変更を残したまま報告すればこちらでコミットする。
- `cargo fmt --all` を通し、末尾のテストを走らせる。

---

## 何が起きているか (実機で確認済み)

サブ展開を併用したスマートフォルダ (フォルダ 77 + 画像 66,934 = 67,011 件) で:

- **本体**: 画像を開くとシークバーは **66,934** (スマートフォルダ全体の画像)。見開きで読める。
- **リモート**: 一覧のスクロールバーは 67,011 で正しいが、画像をタップすると **1,163** になる。

原因は追跡済み。コレクションの項目 (`RemoteEntry`) は `address` フィールドを持たないため:

1. `openGridEntry` (`app.js:3110`) が `entry.address` 無しと判定し `{kind: "image", path}` の旧経路へ
2. `executeOpenCommand` が `state.collection` を見て `navigate(imageHash(path))` (`app.js:2965` 付近)
3. `navigate` がルートを dispatch (`app.js:2511`)
4. `route.kind === "image"` が **`loadContainer(親フォルダ)`** を呼ぶ (`app.js:2366`)
5. `applyContainerData` が `state.collection = null` にして `state.images` / `state.pageGroups` を
   実フォルダのものへ置き換える (`app.js:4018`)

つまり 1,163 = タップした画像が入っている実フォルダの画像数。**設計としてそうなっている**
(`rootOpenReturnHash` が「戻る」先にコレクションを保持している) が、サブ展開は
**フォルダをまたいで連続で読むための機能**なので、本体に合わせる。

---

## コミット 1: コレクションのページグループを本体で組む

### 何を足すか

`CollectionPayload` (`crates/remote-ipc/src/lib.rs:1125`) に、コンテナと同じ読書用の情報を足す:

- `page_groups: Vec<PageGroup>` — **address で持つ** (`ContainerPayload` と同じ。index 空間の
  食い違いが起きない)
- `configured_spread_mode` / `effective_spread_mode` / `reading_direction` / `spread_page_gap_px`
- `image_count` (シークバーの総数表示に使うなら)

`CollectionRequest` にも `spread_mode` / `reading_direction` / `force_single_page` を足す
(コンテナ要求と同じ形)。`PROTOCOL_VERSION` を 1 つ上げ、round-trip テストを更新する。
**この feature は未リリースなので migration は不要**。

### ページグループの組み方

**本体と同じ関数を使う**こと。`crate::ui_fullscreen::build_remote_spread_page_groups` は
`&[GridItem]` + `SpreadMode` + `&[bool]` (横長フラグ) を取る。コンテナ側の
`spread_payload` (`src/remote_ipc/container.rs:3616`) が既にこれを呼んでいるので、
**同じ経路に寄せる**。ペア判定を書き直さない。

- コレクションの項目のうち **画像だけ**が対象。ZIP / PDF / フォルダ / 動画はページではない。
- 横長フラグは `cached_landscape_flags` と同じ考え方で catalog から引く。ただしコレクションの
  項目は**複数のフォルダにまたがる**ので、**親フォルダごとにまとめて catalog を開く**こと。
  1 項目ごとに開かない。
- **必ず新しい `CatalogDb::load_source_dims()` を使う** (`bb8c62b9` で追加)。
  `load_all()` は thumbnail の blob も運ぶ。実測 4,628 枚で 157 MiB、
  `load_source_dims` なら 0.43 MiB。77 フォルダ分を `load_all` で開くのは論外。
  寸法列が空の古い行だけ `load_one` で blob から復元する経路も同関数の使い方に合わせる。
- 見つからない項目は本体と同じく**縦長扱い** (= ペア可能) にする。

### 見開きの状態

- 今回は**コレクションごとの見開き状態を DB へ永続化しない**。要求に載ってきた
  `spread_mode` / `reading_direction` と、本体設定の既定から解決する
  (`resolve_spread_state` の非 book 既定と同じ扱い)。理由: コレクションには
  `spread_db` のコンテナキーに相当する安定した鍵が無く、鍵の設計を今回の範囲に入れない。
  **この判断をコード中のコメントに残すこと。**
- セッション中の切り替えは、クライアントが現在値を要求に載せるので効く。

### スコープ外 (やらないこと)

- `resume_page` (続きから開く)。コレクションには保存位置の鍵が無い。
- コレクション内の ZIP / PDF をページとして展開すること。

### テスト

- 画像 + 動画 + ZIP が混ざったコレクションで、`page_groups` が**画像だけ**を含む。
- 横長画像が入るとそこでペアが切れる (本体の `build_remote_spread_page_groups` の
  既存テストと同じ性質を、コレクション経路でも 1 つ固定する)。
- 複数の親フォルダにまたがる項目で、catalog を**親フォルダごとに 1 回だけ**開く
  (開いた回数を数えられる形にするのが難しければ、複数フォルダの項目で正しい
  横長フラグが出ることを固定する)。
- 単ページ指定 (`force_single_page`) では 1 グループ 1 ページになる。

---

## コミット 2: コレクションから開いたとき実フォルダへ移らない

### 実装

- `openGridEntry` (`app.js:3110`) の画像分岐で、**`state.collection` があるときは
  コンテナ経路へ落とさない**。`state.images` の index を使って、`media` 経路と同じように
  ビューアを直接描く。
- コレクションの payload 適用 (`app.js:3733` / `:3772` / `:3811` 付近と `:4057`) で、
  `state.pageGroups` を**サーバが返した `page_groups` から**組む。今の
  「画像 1 枚 = 1 グループ」の組み立てを置き換える。**コンテナ側の
  `setContainerPageGroups` と同じ経路を使う**こと (2 つ目の実装を作らない)。
- `state.spreadMode` / `state.effectiveSpreadMode` / `state.readingDirection` /
  `state.spreadPageGapPx` を、コンテナのときと同じようにコレクションの payload から入れる。
- 見開き切り替えなど、現在コンテナ再取得で反映している操作は、コレクションでは
  `/api/collection` の再取得で反映する。

### 壊さないこと

- 「戻る」でコレクションへ返る挙動 (`rootOpenReturnHash`) は維持する。
- コレクション内の **ZIP / PDF / フォルダ** をタップしたときは従来どおりコンテナを開く。
  変えるのは**画像**だけ。
- 動画 / 音声は従来どおり。

---

## コミット 3: コレクションを含んだ URL で復元できるようにする

### 何が問題か

今の `#image/<path>` は画像のパスしか持たないので、リロードや共有 URL からは
**必ず親フォルダに落ちる** (`app.js:2366`)。コミット 2 を入れても、リロードすると
1,163 に戻ってしまう。

### 実装

- コレクション + 画像を表す hash を追加する。既存の
  `#collection/<kind>[/<id>]` を拡張する形にする。
  **`parseRoute` の既存パターンは `([^/]+)` が id を取るので、素朴に
  `/image/<...>` を足すと `image` が id として食われる。**
  `/image/` の位置で先に分割するなど、曖昧にならない解き方にすること。
- 復元経路: コレクションを読み込む → `state.images` から該当画像を探す →
  `renderImageViewer(index)`。コンテナの復元経路と同じ形。見つからないときは
  コレクション一覧を表示する (エラー画面にしない)。
- コミット 2 でビューアを開くときに、この hash を `history.pushState` する。

### テスト

- `parseRoute` の単体テスト: `#collection/smart/<uuid>`、
  `#collection/smart/<uuid>/image/<encoded>`、`#collection/rating/5`、
  `#collection/rating/5/image/<encoded>`、`#collection/reading_history/image/<encoded>` が
  それぞれ正しく解ける (**id と `image` を取り違えない**)。
- パスに `/` や `#` を含む画像でも往復する (percent encoding)。

---

## 実行するテスト

```
cargo test -p mimageviewer --lib remote_ipc
cargo test -p mimageviewer --lib catalog::
cargo test -p mimageviewer-remote
cargo test -p mimageviewer-ipc
node --test
cargo fmt --all -- --check
```

`node --test` が sandbox の `spawn EPERM` になる場合は
`node --experimental-test-isolation=none --test` でよい (その旨を報告に書くこと)。

## 報告してほしいこと

- 3 つの変更それぞれで何をしたか (コミットはこちらで行う)。
- 横長フラグを親フォルダごとにまとめる実装で、catalog を何回開くことになるか。
- 66,934 件のコレクションで `page_groups` を組むのにかかる時間 (テストで測れる範囲でよい)。
- ブリーフと意図的に違えた点があれば、その理由。
