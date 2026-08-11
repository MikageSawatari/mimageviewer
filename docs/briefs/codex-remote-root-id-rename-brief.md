# リモート閲覧: 住所の錨を `favorite_id` から `root_id` へ改名する (意味は変えない)

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode。

## 1. これは何のための段か

次の増分で、リモートが開ける範囲を「お気に入りの中だけ」から
「**mIV の一覧に出るものは開ける**」へ広げる。タグ・レーティング・ブックマーク・本棚・
スマートフォルダ・閲覧履歴に出てくる項目は、お気に入りの外にあっても開けるようにする
(利用者判断。家で見ていたものが外出先で見られず、しかも外出先で初めて気づくため)。

そのとき `RemoteAddress.favorite_id` には**お気に入り以外の起点の ID も入る**。
名前を残したままにすると、後から読む人が `favorites.find(id)` と書いて静かに失敗する。

**この段では名前だけを変える。挙動は 1 ミリも変えない。** 意味の拡張は次の増分で行う。
2 段に分ける理由は、500 箇所の機械的な置換と意味の変更が同じ差分に混ざるとレビューが
成立しないため。

## 2. やること

`favorite_id` / `favoriteId` / `favorite` (**住所の錨としてのもの**) を `root_id` /
`rootId` / `root` へ改名する。

- `crates/remote-ipc` の `RemoteAddress.favorite_id`、`RemoteEntry.favorite_id`
- 本体側 (`src/remote_ipc/`) の解決・写像・identity 生成
- remote-web (`crates/remote-web/src/`) の HTTP query 名 (`favorite=` → `root=` 等) を含む
- ブラウザ (`crates/remote-web/web/`) の `favorite_id` / `favoriteId` / route 組み立て

## 3. 改名しないもの

**「お気に入りそのもの」を指す語は変えない。** 変えるのは「住所の錨」として使われている
ところだけ。以下は `favorite` のまま:

- `settings::FavoriteEntry` とその `id` / `path` / `name`
- お気に入り一覧 API (`/api/favorites`)、`FavoriteSummary`、ホームの「お気に入り」タブ
- `RemoteFavoriteRoots`、`resolve_existing_favorite_roots`、`map_existing_to_resolved_favorite`
  など、今はお気に入りだけを扱っている関数名 (次の増分で扱う対象が増えたときに改める)
- `FavoriteSearchRequest` など、直前の増分で入れたお気に入り横断検索の型

迷ったら「この識別子は**お気に入り以外の値も入り得るか**」で決める。入り得るなら `root`、
入り得ないなら `favorite` のまま。

## 4. 不変条件

- **挙動は変わらない。** 既存テストは改名以外の変更なしで通ること
- **プロトコル版数を 1 つ上げる** (フィールド名が変わるので両側同時更新が要る)。
  `docs/web-remote-plan.md` §13.5 の版数も更新する
- 型は `String` のまま。次の増分で意味が広がる前提で、**enum にはしない**
  (どの種類の起点かを気にする場所は解決器 1 か所だけにする)

## 5. 型に doc を付ける

`RemoteAddress.root_id` に、次の増分を見越した doc comment を残す:

- これは「許可された起点」の ID であること
- 今はお気に入りの UUID しか入らないが、**お気に入り以外の起点も入り得る**設計であること
- したがって、この値を直接お気に入り一覧から引く処理を**解決器の外に書かないこと**

## 6. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`crates/remote-ipc` / `crates/remote-web`、
  web テスト一式 (245 件)
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- **ビルドとコミットは行わない**
- **`htdocs/` と `README.md` は触らない**
- 既存の未追跡 brief (`docs/briefs/*.md`) には触れない
- 改名した識別子の一覧と、§3 に従って**あえて改名しなかった**ものを報告する
