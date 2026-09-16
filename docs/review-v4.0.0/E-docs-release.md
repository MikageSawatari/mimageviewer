# E: ユーザー向け文書・設計文書・リリース準備物の監査 (v4.0.0 出荷前)

対象 HEAD: `0a7139d27`（master、2026-09-16 時点）。読み取り専用で実施。アプリは起動していない。
本報告の事実はすべて (a) コードの参照 (b) read-only チェックコマンドの実出力 のいずれか。実行時挙動の観測は含まない。

## 要約

| 重要度 | 件数 | 意味 |
| --- | --- | --- |
| **P1** | **7** (E-1〜E-7) | 公開時に偽になる記述、または Phase 0/1 の必須成果物がまだ存在しない |
| P2 | 14 (E-8〜E-21) | 設計文書・製品ページ・移行ガイドの整合漏れ。公開はできるが腐りが残る |
| P3 | 6 (E-22〜E-27) | 表記ゆれ・判断待ち・将来の混乱要因 |

最も重いのは **「v4.0.0 の目玉であるコレクションが、利用者の読む場所にほぼ存在しない」** こと。
`htdocs/mimageviewer/index.html` に 0 件、マニュアルは `settings.html` の 5 行だけ、`remote.html` の
できること表からは漏れ、`shortcuts.html` の <kbd>Delete</kbd> 説明はコレクション直下で偽になる。
README 更新履歴・`version_highlights` の v4.0.0 節も未作成。

---

## P1

### E-1 / P1 / README 更新履歴に v4.0.0 節が無い（Phase 0 の必須成果物）

- 根拠: `README.md:136` が `### v3.10.0 (2026-09-14)` で最新。`v4.0.0` の節は存在しない。
  `git log v3.10.0..HEAD --oneline` は 56 commit（うち製品コード変更 20 前後）。
- 現状: Phase 0（更新履歴の作成 → 利用者レビュー）が未着手。ここが終わらないと Phase 1 以降へ進めない。
- あるべき状態: `### v4.0.0 (YYYY-MM-DD)` 節を追加し、利用者の承認を得る。
- 修正方向: 本書「§A. README v4.0.0 更新履歴 下書き案」をたたき台にする。**バイト数 6,859 / 8,192**
  （`update_check.rs:106` の `BODY_CAP = 8 * 1024` 以内。残り約 1,300 バイトしかないので、項目を足すなら
  どれかを削るか短縮版 `docs/release-body-4.0.0.md` の作成を検討する）。

### E-2 / P1 / 製品ページ `index.html` にコレクションの記述が 0 件

- 根拠: `grep -c -i 'コレクション' htdocs/mimageviewer/index.html` → **0**。
  「主な機能」（`htdocs/mimageviewer/index.html:1019-1107`）にも
  「そのほかの便利な機能」（同 `:1108-1165`、製本・スマートフォルダ・お気に入りは掲載済み）にも無い。
- 現状: メジャー版の目玉機能が製品ページから見えない。移行検討者（§1.118 の出典は ZipPla / NeeView 移行者の
  「仮想ディレクトリが無い」という指摘）が探しに来ても見つからない。
- あるべき状態: 「主な機能」に 1 枚カードを追加する（`:1088` タグ・レーティング / `:1093` 横断検索の並び）か、
  最低でも「そのほかの便利な機能」へ 1 行。製本（`:1110` 付近）と紛らわしいので
  「参照だけを束ねる／元ファイルはコピーしない」という差を明記する。
- 修正方向（文案の例）: 「**コレクション** — 離れた場所のファイル・フォルダ・本を、元の場所を変えずに
  名前付きの一覧へまとめ、手動順で並べ替え・スライドショー・連続再生。テキストからの取り込みにも対応」

### E-3 / P1 / マニュアルにコレクションの専用ページが無く、機能の大半が未文書

- 根拠: `grep -i 'コレクション' htdocs/mimageviewer/manual/*.html` のヒットは
  `settings.html:197, 205, 206, 207, 211, 487, 496` の **7 行のみ**。
  `tutorial.html:188, 281` のヒットは「増えていくコレクション」「大量のコレクション」という
  **一般語としての用法**で、機能の説明ではない（親レビュアーの前提とは異なる）。
- 現状: 設定リファレンスの中にツールバー／メニューの操作だけがある状態。本棚（`books.html` +
  `tut-books.html`）、ブックマーク（`tut-bookmarks.html`）と比べて明らかに薄い。
  未文書の項目は本書「§B. 操作 × 記載場所 対応表」の「**無し**」行を参照（16 項目中 9 項目が未記載）。
- あるべき状態: `manual/collections.html`（機能ページ）と `manual/tut-collections.html`（チュートリアル）を
  新設する。`tut-collections.html` は `tutorial.html` の「整理」カテゴリ（`tutorial.html:188` の
  `tut-cat-desc` 配下、`tut-books.html` / `tut-bookmarks.html` の隣）へ置くのが自然。
- 修正方向・付随作業:
  - サイドバーは現在 **29 ページすべてが 29 リンク**で揃っている（下記 §D-2 の実測）。
    `collections.html` を足すと **30 ページ × 30 リンク**になる。`changelog.html` のサイドバーは
    `gen-changelog-html.py` が `getting-started.html` からコピーするので、他 29 ページを直してから再生成する。
  - `tut-*.html` はサイドバーを持たない別レイアウトなので対象外（`tut-collections.html` は
    `tutorial.html` のカード一覧にだけ追加する）。

### E-4 / P1 / `shortcuts.html` の <kbd>Delete</kbd> 説明がコレクション直下で偽になる

- 根拠: `htdocs/mimageviewer/manual/shortcuts.html:179`
  「<kbd>Delete</kbd>：選択/チェック済みファイルをごみ箱へ移動」。
  実装は `docs/collection-implementation-plan.md:1017-1021`（§20）および
  `src/context_menu_model.rs:184` の `RemoveFromCollection`「コレクションから外す」で、
  **コレクション root では checked 優先 / selected fallback の参照登録だけを外し、元ファイルは変更しない**。
  `docs/keymap-spec.md:340` にはこの分岐が既に書かれている（設計文書側は正しい）。
- 現状: 利用者向けの唯一のキー一覧が、新しい意味を反映していない。「ごみ箱へ移動」と書いてあるのに
  ファイルが消えない、という逆向きの驚きになる。
- あるべき状態: 同行に「ただしコレクションの一覧では、参照の登録だけを外します（元ファイルは変更しません）」
  を追記する。あわせて「元ファイルをゴミ箱へ移動…」が別操作である旨を書く。

### E-5 / P1 / `remote.html` の「できること」表にコレクションが無い

- 根拠: `htdocs/mimageviewer/manual/remote.html:204`
  「一覧を見る｜お気に入り / スマートフォルダ / 場所（読書履歴・レーティング・本棚・ブックマーク）」。
  実装では Remote ホームに 6 番目のタブが増えている（`crates/remote-web/web/app.js:3805-3811` の
  `["collections", "コレクション"]`、空表示は同 `:5169`「コレクションはまだありません。mIV 本体で
  作成・編集できます。」）。Phase 5 の記録は `docs/collection-implementation-plan.md:918-930`（§18、protocol 56）。
- 現状: Remote の能力一覧が不完全。**利用者はリモートでコレクションを見られることを知る手段が無い**。
- あるべき状態: 同行に「コレクション」を追加し、**読み取り専用**（作成・編集・並べ替え・取り込み /
  書き出しはできない）ことを明記する。`docs/collection-remote-plan.md:35`「Remote 初回版は読み取り専用」が根拠。

### E-6 / P1 / `version_highlights` に v4.0.0 の節が無く、操作・既定の変更が告知されない

- 根拠: `src/version_highlights.rs` の `TABLE`（`:228` 開始）の最終エントリは `:853` の `"3.10.0"`。
  （`:877` 以降は `mod tests` 内のテスト用データ。親レビュアーの「2.2.0 が最後」は誤り。）
- 現状: v4.0.0 には**操作・既定の変更が複数ある**（ツールバーに新セクションが既定 ON で増える＝
  `src/settings.rs:6839` の `show_toolbar_collections: true`／メニューバーに「コレクション」が
  既存の保存済み順へ自動補完される＝`src/keymap.rs:3055-3060`／<kbd>Delete</kbd> の意味が
  コレクション直下で変わる／ソート順の表示名が「ファイル名順」→「名前（昇順）」に変わる／
  <kbd>Ctrl</kbd>+<kbd>↑↓</kbd> の順序がフォルダツリー専用設定に移る）。
- あるべき状態: `TABLE` に `"4.0.0"` を追加する。案は本書「§C. version_highlights v4.0.0 案」。
- 確認: 追加後に `cargo test --lib version_highlights::` を通す（CLAUDE.md Phase 1 手順 5.5）。

### E-7 / P1 / バージョン表記が全て 3.10.0 のまま（Phase 1 未着手）

- 根拠（実測）:
  - `Cargo.toml` → `version = "3.10.0"`
  - `installer/mimageviewer.iss:5` → `#define MyAppVersion "3.10.0"`
  - `installer/readme.txt:2` → `Version 3.10.0`（BOM あり、正常）
  - `installer/readme_portable.txt:2` → `ポータブル版 Version 3.10.0`（BOM あり、正常）
  - `htdocs/mimageviewer/index.html:1240` → `mImageViewer v3.10.0`
  - `htdocs/mimageviewer/index.html:1241` → `最終更新: 2026-09-14`
  - `htdocs/mimageviewer/index.html:1246` → ポータブル zip の href が `mImageViewer_portable_v3.10.0.zip`
  - `htdocs/mimageviewer/index.html:34` → JSON-LD `"softwareVersion": "3.10.0"`
  - `htdocs/mimageviewer/manual/index.html` のマニュアル版表記（Phase 1 手順 5）
- 現状: Phase 1 が未実施なので当然だが、**`index.html:34` の JSON-LD だけはチェックリストに項目が無い**（E-8）。
- あるべき状態: Phase 1 手順 1〜5 を実施。

---

## P2

### E-8 / P2 / リリース手順 Phase 1 手順 4 に `index.html` の JSON-LD `softwareVersion` が列挙されていない

- 根拠: `CLAUDE.md` Phase 1 手順 4 は「ダウンロードセクションのバージョン表記」「最終更新」「ポータブル版の
  link href」だけを挙げる。`htdocs/mimageviewer/index.html:34` の `"softwareVersion": "3.10.0"` は
  構造化データ（schema.org SoftwareApplication）で、検索結果に出得るのに手順から漏れている。
- あるべき状態: 手順 4 に「JSON-LD の `softwareVersion`」を追記する。
- 補足: `htdocs/sitemap.xml` と同じく「手順のどこからも参照されていないので腐る以外の道が無い」型。

### E-9 / P2 / `htdocs/sitemap.xml` が out of date

- 根拠: `python scripts/gen-sitemap-xml.py --check` の出力 →
  `sitemap.xml is out of date. Run: python scripts/gen-sitemap-xml.py`
- 現状: v3.10.0 以降に `htdocs/` の 8 ファイルが変わっている（`git diff --stat v3.10.0..HEAD -- htdocs/`）。
- あるべき状態: Phase 1 手順 6.6 のとおり、**htdocs の編集をコミットした後**に再生成する
  （`<lastmod>` を git の最終コミット日から取るため）。`collections.html` / `tut-collections.html` を
  追加する場合は、それらのコミット後に回す。

### E-10 / P2 / `privacy.html` に コレクション（`collection.db`）が無い

- 根拠: `grep -i 'コレクション\|collection' htdocs/mimageviewer/privacy.html` → **0 件**。
  「端末内に保存されるデータ」は `htdocs/mimageviewer/privacy.html:134-164`、英語版は `:253-276`。
  実装の保存先は `src/lib.rs:1291` の `data_dir::get().join("collection.db")`。
- 現状: 保存されるデータの列挙（設定 / サムネイルキャッシュ / タグ・★・回転・履歴 / 別バージョン索引 /
  PDF パスワード / リモート PIN / リモート動作記録 / 一時書き出し）に、**利用者のファイルパスを保存する
  新しいストアが入っていない**。CLAUDE.md「通信・データ保存に関わる機能を追加したときは 2 か所を
  突き合わせる」に該当する。
- あるべき状態: 日本語・英語の両方の箇条書きへ
  「コレクション（一覧の名前と、登録したファイル・フォルダの場所）」/ "Collections (list names and the
  locations of the files and folders you add)" を追加する。保存先 `%APPDATA%\mimageviewer` の記述は
  そのままで正しい。
- 「ネットワーク通信」節は**変更不要**: コレクションは通信経路を増やさず、Remote の既存経路を読むだけ
  （`docs/collection-remote-plan.md:35-45`）。

### E-11 / P2 / `index.html`「安心して使えます」の保存データ列挙にコレクションが無い

- 根拠: `htdocs/mimageviewer/index.html:1218-1224`「💾 データはあなたの PC の中だけ」＝
  「設定・サムネイルキャッシュ…・タグ・★評価・閲覧履歴・別バージョンを探すための索引」。
- 現状: E-10 と対になる箇所。CLAUDE.md は「片方だけ更新されると、同じ事実について 2 つの文書が食い違い、
  しかもそれが一番見つかりにくい」と明示している。
- あるべき状態: 同じ列挙へ「コレクション」を追加する。
- なお `:1200-1207`「🌐 通信するのは 3 つの場面だけ」は **v4.0.0 でも偽にならない**（監査済み・問題なし）。

### E-12 / P2 / `docs/web-remote-plan.md` の protocol version が v49 のまま（実装は 56）

- 根拠: `docs/web-remote-plan.md:1874`（§13.5）「現行版は v49」。実装は
  `crates/remote-ipc/src/lib.rs:31` の `pub const PROTOCOL_VERSION: u32 = 56;`。
  同文書内で 49 より新しい版に触れるのは `:2757`（§15.4、51→52）だけで、§13.5 本文は未更新。
- 現状: Remote の正本に**明確に誤った数値**が残っている。`docs/collection-remote-plan.md:528` は
  「実装完了時に … `web-remote-plan.md` … の checkpoint を同期する」と自分で書いているが未同期。
- あるべき状態: §13.5 を 56 へ更新し、53〜56 の変更点（永続コレクション read-only を含む）を 1 行ずつ残す。
  加えて、`docs/web-remote-plan.md` 全文に `persistent` / 永続コレクションの記述が **0 件**なので、
  索引に「永続コレクションの Remote 対応は `collection-remote-plan.md`」への導線を足す。

### E-13 / P2 / `docs/architecture-overview.md` に v4.0.0 の新モジュール・新ストア・新 surface が無い

- 根拠（すべて「記述なし」）:
  - モジュールマップ（`:58-307`）に `collection_store` / `app/collection_grid.rs` /
    `app/collection_navigation.rs` / `remote_ipc/persistent_collections.rs` が無い。
    `:79` の `remote_ipc/collections.rs` は**集約コレクション**（ドライブ一覧・本棚・★・スマートフォルダ）の説明。
  - 永続化ストア一覧（`:338-383`）に `collection.db` が無い。
  - `:76` の `app/top_level_grid_view.rs` の surface 列挙に `Collection` が無い
    （実装は `src/app/top_level_grid_view.rs:687` の `TopLevelGridSurface::Collection`）。
  - 関連ドキュメント表（`:533-547`）に collection 系 4 文書へのリンクが無い。
- 参考: 同じ v4.0.0 期の `91441ce43`（見開き単ページ配置）は `docs/architecture-overview.md` と
  `docs/display-pipeline.md` を**同時に更新している**ので、コレクションだけが取り残されている。
- あるべき状態: CLAUDE.md「モジュールが増減した、永続化ストアを追加した等の構造変化」に該当。
  `docs/README.md:7`「迷ったらまず architecture-overview.md から」という入口から collection 実装へ辿れるようにする。

### E-14 / P2 / `docs/async-architecture.md` にコレクションの actor / worker / watch が 1 行しか無い

- 根拠: 全文で collection への言及は `docs/async-architecture.md:36` の 1 行のみ
  （`collection-grid-prepare` / `collection-navigation-prepare` worker）。
  実装のスレッドは `collection-store`（`src/collection_store/runtime.rs:109`）、
  `collection-navigation-preflight`（`src/app/collection_navigation.rs:1387`）、
  `collection-pdf-password-preflight`（同 `:1218`）、
  `collection-export` / `collection-import-read` / `collection-source-prepare`
  （`src/ui_dialogs/collections.rs:2497 / 2883 / 2917`）、
  watch fan-out は `CollectionRevisionWatch` + `subscribe()`（`src/collection_store/runtime.rs:41-56, 446-471`）。
  §2.2 チャネル（`:142-165`）・§2.3 ワーカーキュー（`:166-213`）・§3 キャンセル規約（`:241-642`）には記述なし。
- あるべき状態: 単一 owner の actor（`collection-store`）と revision watch の fan-out、
  分類 / import / export worker のキャンセル規約を追記する。
  CLAUDE.md「ワーカーを増やした、共有アトミック/チャネルを追加した、キャンセル規約を変えたとき」に該当。

### E-15 / P2 / `docs/virtual-folders.md` に collection surface の full-path キャッシュキー例外が無い

- 根拠: `docs/virtual-folders.md` の collection ヒット **0 件**。
  実装では `src/app.rs:74679-74682` の `use_full_path_cache_keys()` が
  `TopLevelGridSurface::Collection(_)` を含み、別フォルダの同名画像・動画サイドカーを混同しないよう
  full path で識別する（記録は `docs/collection-implementation-plan.md:995-1000`）。
- 現状: 「キャッシュキーの命名規則を勝手に変えないこと」（`docs/virtual-folders.md:509`）と
  `#pin:` 例外（同 `:530`）はあるが、full-path 化する surface の列挙自体が無い。
- あるべき状態: full-path キー対象 surface（検索 / タグ / 履歴 / ★ / Collection）を 1 箇所に列挙する。

### E-16 / P2 / `docs/spec.md` のメニューバー表と設定項目一覧に漏れ

- 根拠:
  - §2.1 メニューバー表（`docs/spec.md:35-43`）に **「コレクション」の行が無い**
    （実装は `src/keymap.rs:2406` の `TopMenuId::Collections`、表示名は同 `:2434`）。
  - 設定項目一覧: `show_toolbar_collections`（`:2015`）/ `toolbar_collections_display`（`:2034`）/
    `toolbar_collection_target_id`（`:2035`）/ `toolbar_collections_collapsed`（`:2036` の合成行）は
    載っているが、**`pinned_collections`（`src/settings.rs:4793`）だけ行が無い**
    （docs 全体で `pinned_collections` のヒット 0 件）。
- あるべき状態: 両方を追記する。コレクション節本体（`:360-381`）の内容は実装と一致している（§F 参照）。

### E-17 / P2 / `docs/keymap-spec.md` にコレクション操作が KeyAction 対象外である理由が無い

- 根拠: `docs/keymap-spec.md` で collection に触れるのは `:340`（Delete の意味分岐）の 1 行だけ。
  §「固定入力 / KeyAction 対象外の整理」の表（`:263-282`）に collection の行が無い。
  `TopMenuId::Collections` 配下の 8 コマンド（`src/keymap.rs:2499-2506`）はすべて `action: None`。
  `docs/keymap.ini.default:186` は `GridDelete` のコメント 1 行のみ。
  `docs/key-customization-impl-plan.md` は 0 件。
- 比較: 同種の専用ダイアログである**製本の並べ替え画面は `docs/keymap-spec.md:373-375` に専用節があり**、
  「グローバルショートカットではないため `KeyAction` には追加しない」と理由が明記されている。
  コレクションの専用並べ替え画面（`MenuCommandId::CollectionsReorderCurrent`）には同等の節が無い。
- 公平な但し書き: `action: None` は Collections 固有ではなく、`MenuCommandSpec` 51 件中 41 件がそう。
  つまり「メニューコマンドは既定で KeyAction を持たない」構造であり、Collections だけの例外ではない。
- あるべき状態: CLAUDE.md「新しいキー操作は原則 `KeyAction` + keymap helper 経由」「固定扱いにする入力は
  理由を `docs/keymap-spec.md` に残す」に従い、製本と同じ形の 1 節を足す。
  可能なら「MenuCommandId は既定で KeyAction を持たない」という上位の説明も同時に置く。

### E-18 / P2 / `docs/next-release-backlog.md` §1.118 の状態行が実装完了を反映していない

- 根拠: `docs/next-release-backlog.md:1991`
  「**対応中（2026-09-14）**: … 具体設計と基盤実装へ移行」、`:1993`「コレクション実装は最後」。
  実際は Phase 1〜5 と §19〜§22 まで完了している
  （`docs/collection-implementation-plan.md:556, 586, 653, 902, 918, 930, 1004, 1039, 1072`）。
- 現状: バックログの運用ルール（`:12`「着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから
  削除する」）に反した状態。
- あるべき状態: 出荷確定時に §1.118 を削除するか、残す場合は「v4.0.0 で出荷。正本は
  `collection-implementation-plan.md`」に書き換える。

### E-19 / P2 / `grid.html` のメニューバー / ツールバー列挙にコレクションが無い

- 根拠: `htdocs/mimageviewer/manual/grid.html:84`「『ファイル』『お気に入り』『スマートフォルダ』『製本』
  『変換』『動画』『タグ』『設定』『ヘルプ』などのメニュー」、同 `:85`「列数・詳細切替・サムネイル比率・
  ソート順・レーティング・お気に入り・スマートフォルダ・タグ・本棚などのセクション」。
- 現状: 「など」が付いているので**偽ではない**が、既定 ON で見えている新セクション・新メニューが
  列挙から漏れている。
- あるべき状態: 両方に「コレクション」を追加する。

### E-20 / P2 / `books.html` の「プレイリストではなく」からコレクションへの導線が無い

- 根拠: `htdocs/mimageviewer/manual/books.html:81`
  「番号順の画像フォルダとして整理する機能です。元ファイルを参照し続けるプレイリストではなく、（コピー）」。
- 現状: v3.x では「mIV にプレイリストは無い」という含意で正しかったが、v4.0.0 では**まさにそれが
  コレクション**。読者を宙吊りにする。
- あるべき状態: 「元ファイルを参照し続ける一覧が欲しい場合は
  <a href="collections.html">コレクション</a> を使います」を 1 文足す。

### E-21 / P2 / 移行ガイドの「仮想フォルダ / プレイリスト」対応欄にコレクションが無い

- 根拠:
  - `htdocs/mimageviewer/migrate-zippla.html` の機能比較表「ブックマーク/スマートフォルダ」行が
    ZipPla の「スマートフォルダ（.kdk）で複数フォルダを 1 つの仮想場所のようにまとめ」に対して、
    mIV 側を「お気に入り」と「スマートフォルダ」だけで答えている。
  - `htdocs/mimageviewer/migrate-leeyes.html:255`
    「…左側ツリーは実際のフォルダ構造を表示するもので、お気に入り配下の仮想ツリーにはなりません。」
- 現状: §1.118 の出典は「ZipPla / NeeView からの移行に『仮想ディレクトリ』が不足する」という外部指摘
  （`docs/next-release-backlog.md:2002-2005`）。**その指摘への答えが、当の移行ガイドに書かれていない。**
- あるべき状態: 両ガイドに「手動で選んだファイル・フォルダを名前付きで束ねる**コレクション**」を追記する。
  Leeyes 側は「仮想ツリーにはなりません」に「ただしコレクションで手動の一覧は作れます」を添える。
- 補足: NeeView の移行ガイドは存在しない（`htdocs/mimageviewer/migrate-*.html` は vix / leeyes /
  mangameeya / zippla の 4 本）。新設は今回の範囲外。

---

## P3

### E-22 / P3 / `known-issues.html` にコレクションの制限を載せるか（判断が要る）

- 根拠: `htdocs/mimageviewer/manual/known-issues.html` の現在の掲載は 3 件
  （横断検索の 2 文字以下英数字 / 別ウィンドウ動画のゲームパッド十字キー / 一部 MPEG のシーク）。
  いずれも v4.0.0 で直っていないので**削除対象は無い**（Phase 1 手順 6.5 の棚卸しは「変更なし」で成立）。
- 判断: コレクションの制限（Remote は閲覧のみ／再リンク無し／入れ子不可／ZIP 内ページ単独登録不可）は、
  CLAUDE.md の掲載基準「②不具合だと思う見た目をしている」に**当たらない**（仕様として明示された範囲）。
  → **`known-issues.html` ではなく `collections.html` の「できないこと」節に書くのが正しい。**
- 例外的に検討の余地があるもの: `docs/next-release-backlog.md:2983`（§1.244、詳細表示の列ソートが本の中の
  読み順まで並べ替える疑い）は、v4.0.0 で入ったサイズ順・降順ソートの周辺。**未確認（観測者なし）**なので
  現時点で掲載基準を満たさないが、出荷前に確認が取れて直さない場合は掲載対象になる。

### E-23 / P3 / `docs/README.md` に「コレクションを触るなら読む」案内が無い

- 根拠: 4 文書はすべて索引済み（`docs/README.md:35-38`）。ただし説明文は実装・検収台帳の要約で、
  他領域にある「〜を触るとき」形式（`:48` ZIP/PDF、`:54` リモート、`:59` 検索、`:64` keymap）になっていない。
  `:61` の `top-level-grid-view.md` の説明にも Collection surface が含まれる旨の追記が無い。
- あるべき状態: 領域別案内を 1 行足す。CLAUDE.md の「触る領域 → 読むドキュメント」表にも
  コレクションの行が無いので、あわせて追加を検討する。

### E-24 / P3 / `tutorial.html` の「コレクション」が一般語として使われており、機能名と衝突する

- 根拠: `htdocs/mimageviewer/manual/tutorial.html:188`「増えていくコレクションに印を付けて」、
  同 `:281`「大量のコレクションを快適に扱うための」。
- 現状: v4.0.0 から「コレクション」は製品の機能名になる。同じページのカテゴリ説明で一般語として
  使われていると、`tut-collections.html` を足したときに読者が混乱する。
- あるべき状態: 「増えていくライブラリ」「大量の画像」等へ言い換える。

### E-25 / P3 / `CLAUDE.md` の Project Structure ツリーが v4.0.0 以前のまま

- 根拠: `CLAUDE.md:153-270` のツリーに `collection_store` / `app/collection_grid.rs` /
  `app/collection_navigation.rs` / `ui_dialogs/collections.rs` が無い。
- 但し書き: このツリーは collection 以前から広範に古く（同区間に `remote_ipc/` も `app/` サブディレクトリも
  `keymap.rs` も無い）、**コレクション固有の漏れではない**。単独で直すより、ツリー全体の棚卸しとして扱う。

### E-26 / P3 / `settings.html:197` の「選択中またはチェック済み」が実装の優先順と逆

- 根拠: `htdocs/mimageviewer/manual/settings.html:197`「選択中またはチェック済みのファイル・フォルダへの参照を」。
  実装は `docs/collection-implementation-plan.md:939`「checked があれば checked を優先、無ければ selected」。
- 影響: 同時に成立する場面（チェック済みがあるのにカーソルは別の項目）でどちらが登録されるか分からない。
- あるべき状態: 「チェック済み（無ければ選択中）」の順に書き換える。本棚の行（`:196`）の書き方
  「選択中・カーソル位置・表示中ページ」とも揃える。

### E-27 / P3 / 禁止語ポリシー: 新規混入は無いが、既存 1 件が `.rs` に残っている

- 根拠: `git grep -i -E 'yt-?dlp|youtube-?dl|pixiv|fanbox|fantia|dlsite|nijie' -- '*.rs' 'htdocs/**' 'README.md' 'installer/*.txt'`
  のヒットは `src/filename_stack.rs:4` / `src/filename_stack_script.rs:577` / 同 `:599` の 3 箇所のみ。
- 判定: `git log -S 'pixiv' --oneline v3.10.0..HEAD` が**空** → v4.0.0 で新規混入した語は無い。
  また `htdocs/`・`README.md`・`installer/*.txt`・`git log v3.10.0..HEAD` のコミットメッセージには
  **0 件**で、公開文書は clean。
- 残件: 上記 3 箇所はモジュール doc コメントとテスト関数名（内部）。CLAUDE.md の投稿サイト名ポリシーは
  `*.rs` も機械確認の対象にしているので、次に触るときに一般語（「画像ダウンローダ」「連番の接頭辞」）へ
  置き換えることを提案する。**v4.0.0 の出荷条件にはしない。**

---

## §A. README v4.0.0 更新履歴 下書き案

- 書式は既存節（`README.md:136` の v3.10.0）に合わせた。見出しの日付はタグ公開日を入れる。
- **バイト数 6,859 / 上限 8,192**（`src/update_check.rs:106` の `BODY_CAP`）。余裕は約 1,300 バイト。
  項目を追加する場合はここを再測すること（測り方: `awk '/^### v4\.0\.0( |$)/{f=1} /^### v3\.10\.0( |$)/{f=0} f' README.md | wc -c`）。
- 内部用語（Tantivy / SQLite / actor / revision / protocol / worker）は使っていない。
  バージョンタグは見出しの 1 回だけ。特定の投稿サイト名・ダウンローダ名は含まない。
- v3.10.0 に既に含まれている項目（右クリックメニュー編集、V の縮小、切り取りの半透明表示、見開き片側配置の
  導入そのもの、別バージョン索引の起動短縮）は **重複させていない**。タグ `v3.10.0` = `1fd6f8636`
  以降の commit だけを対象に選んだ。

```markdown
### v4.0.0 (2026-09-XX)
- **好きなファイル・フォルダ・本を集めた「コレクション」を作れるようになりました。** 離れた場所にあるファイル・フォルダ・ZIP・PDF・本を、元の場所に置いたまま 1 つの一覧としてまとめられます。名前を付けていくつでも作れるので、BGM 用・スライドショー用のように用途ごとに分けて使えます。ツールバーの「コレクション:」セクションで追加先を選び、「追加」で今の選択を登録、「開く」で一覧を表示します。管理画面で固定したコレクションは名前のボタンとして並び、左クリックで開く / 右クリックで追加できます。上部の「コレクション」メニューからは、作成・名前変更・削除・追加先の指定・固定を行う管理画面と、表示中のコレクションの並び順・並べ替え画面・テキストの取り込み / 書き出しを開けます。
- **コレクションは元のファイルを一切変更しません。** 登録しているのは場所への参照だけなので、コレクションから外してもファイルは消えません。一覧で <kbd>Delete</kbd> を押すか右クリックの「コレクションから外す」を選ぶと、参照の登録だけが外れます。元のファイルを操作したいときは「元ファイルをゴミ箱へ移動…」または「元ファイルの Windows メニュー」を使います。参照先が見つからない項目も一覧に残り、理由を表示します。
- **コレクションの中でも、ページ送り・スライドショー・動画や音声の連続再生が続きます。** 登録した画像・動画・音声は今の並び順で次へ送られ、元のファイルがあるフォルダの隣の画像へ逸れません。<kbd>Ctrl</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd> は登録したフォルダ・本を順に移動し、スライドショーの「次のフォルダへ」も同じ順で進みます。再生中にコレクションを編集しても再生は止まらず、次の 1 件を選ぶときに最新の内容が使われます。
- **並び順は「手動順」と「通常ソート」から選べます。** 既定の手動順では、本棚と同じ専用のサムネイル画面でドラッグやキー操作でページのように並べ替えられます。通常ソートに切り替えても手動順は保存されたままで、戻せばそのまま使えます。新しく追加した参照は手動順の末尾に入ります。
- **1 行 1 パスのテキストからコレクションへ取り込み / 書き出しできます。** UTF-8（BOM の有無・改行の種類は問いません）のテキストを用意し、絶対パスかテキストの置き場所からの相対パスを 1 行に 1 つ書きます。空行は無視し、行全体を囲む二重引用符は Windows の「パスのコピー」に合わせて外します。取り込む前の確認画面までは対象のファイルへ一切アクセスせず、件数・追加先・重複・ネットワーク上の参照・書式の誤りを確認してから登録します。書式の誤りがある行が 1 つでもあると取り込みは始まりません。書き出しは今の並び順のまま、絶対パスを 1 行 1 件で出力します。
- **外出先の mIV Remote からも、コレクションを一覧・閲覧できます。** ホーム画面に「コレクション」が加わり、PC で作ったコレクションを開いて中のファイル・フォルダ・本をそのまま読めます。リモートからの作成・編集・並べ替えはできません。
- **一覧の並べ替えに、名前・番号の降順とファイルサイズ順が加わりました。** ソート順は「名前（昇順・降順）」「番号（昇順・降順）」「日付（古い順・新しい順）」「サイズ順（小さい順・大きい順）」になりました。これまでの「ファイル名順」は「名前（昇順）」、「番号順（区切り無視）」は「番号（昇順）」という表示に変わります。選んでいた並び順はそのまま引き継がれます。サイズが分からないフォルダや仮想ページは、どちらの方向でも同じカテゴリの末尾に並びます。
- **左のフォルダツリーに、専用の並び順が付きました。** ツリー上部で「名前」「番号」「日付」の昇順・降順を選べます。この並びは <kbd>Ctrl</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd> と <kbd>Ctrl</kbd>+<kbd>PageUp</kbd>/<kbd>PageDown</kbd> のフォルダ移動にも使われ、右側の一覧のソートを変えても影響を受けません。切り替えた直後は、開いている枝を残したまま背後で並べ直します。
- **Visual C++ ランタイムが入っていない Windows で、起動画面のまま応答しなくなる問題を修正しました。** AI 機能用の DLL が読み込めないときに、失敗を知らせる処理そのものが止まってしまい、AI を使わない設定でも起動できませんでした。必要なランタイムをアプリに同梱し、読み込みに失敗した場合も AI 機能なしでそのまま起動を続けます。
- **TensorRT の起動に失敗した後、AI 処理のたびに起動をやり直す問題を修正しました。** 一度確定的に失敗した場合は、設定や追加コンポーネントを変えるまで再試行しません。これまでは AI を使うたびに起動と失敗を繰り返し、処理が遅くなっていました。
- **検索結果から「フォルダに移動」したとき、移動先が正しく開かないことがある問題を修正しました。** 移動に失敗したり別の操作で取り消されたりしたときに、終了した検索結果の行がフォルダの一覧として残ることがなくなりました。検索集約に出てくる ZIP も、フォルダとしてではなく書庫として開きます。
- **設定の復元・初期化が、バージョンを下げた後などに失敗することがある問題を修正しました。** 設定ファイルを置き換える直前だけ、アプリ内のほかの読み取りを止めるようにしました。あわせて、バックアップの一覧に表示するバージョンが実際の保存内容と一致するようになりました。
- **見開きの先頭・末尾に来る横長のページを、中央に配置するようになりました。** 「見開きの先頭・末尾の単ページを片側に配置」を ON にしている場合でも、1 枚で見開き幅を占める横長ページは中央に置きます。
```

### 下書きを作るときに確認した「v3.10.0 に含まれるか」の判定

`git log v3.10.0..HEAD --oneline`（56 件）を全件見て、製品コードを変えた commit だけを拾った。
文書・バックログのみの commit（`882434a9f` / `b4690317b` / `0b6f24a0a` / `776d034be` / `1fd972f5a` /
`6075da125` / `ae917edf6` / `dc411b528` / `7ba7e2c08` / `684735a92` / `47a25ee72` / `349cac513` /
`7b14b13f6` / `08d02b27f` / `658ad96b1` / `0a0771d55` / `c6a7fd80d` / `8fe257d39` / `105da9810` /
`c4ec44676` / `0a7139d27` / `34ad80c2b` / `14045a95a` / `836aff584` / `4d985e4f2` / `298911360`）と
テスト専用（`4a558bbdd`）は更新履歴に入れていない。
`50fffaedd`（マンガミーヤ移行ガイドの訂正）は公開済みページの修正なので更新履歴の対象外とした。

---

## §B. 操作 × 記載場所 対応表（項目 1）

「現状」は 2026-09-16 の HEAD。`settings.html` の行番号は現行ファイルのもの。

| # | 利用者が知るべき操作 | 実装の根拠 | 現在の記載場所 | 判定 |
| --- | --- | --- | --- | --- |
| 1 | コレクションの作成 / 名前変更 / 削除 | `ui_dialogs/collections.rs:2617, 2716, 2693`（管理 window） | `settings.html:206, 487` | 記載あり（1 文だけ） |
| 2 | 追加先の指定・固定（ツールバーへ） | `collections.rs:2707, 2727`、`settings.rs:4789, 4793` | `settings.html:205, 206, 487` | 記載あり |
| 3 | 追加（ツールバー / メニュー / 名前の右クリック） | `keymap.rs:2793`、`ui_main.rs:6104` | `settings.html:197, 205, 487` | 記載あり（優先順が逆＝E-26） |
| 4 | 開く（ツールバー / メニュー / 名前の左クリック） | `keymap.rs:2799`、`ui_main.rs:6116` | `settings.html:197, 205, 487` | 記載あり |
| 5 | 手動順 / 通常ソートの切替 | `ui_main.rs:6162-6203`（「手動順」/「通常ソート」▸ 全ソート順） | `settings.html:206, 487` | 記載あり（選べるソート種別は未記載） |
| 6 | 専用サムネイル画面での並べ替え | `keymap.rs:2828`「現在のコレクションを並べ替え…」、実装記録 `collection-implementation-plan.md:1080-1086` | `settings.html:206, 487`（「本棚と同じ専用サムネイル画面」のみ） | **不足** — 複数選択・ドラッグ・左右移動・競合時の復帰手段が未記載 |
| 7 | テキストのインポート / エクスポート **の形式仕様** | `collection_store/text.rs:38-98`、確認モーダル `collections.rs:3099-3137` | 「操作できます」の一言のみ（`settings.html:206, 487`） | **無し** — UTF-8 / BOM / CRLF / 相対パス基準 / 引用符 / 空行 / URL 拒否 / 重複 / **無効行が 1 つでもあると取り込めない** が全て未記載 |
| 8 | <kbd>Delete</kbd> = 参照解除（元ファイルは変更しない） | `collection-implementation-plan.md:1017-1021`、`context_menu_model.rs:184` | `settings.html:206, 207` | 記載あり。ただし `shortcuts.html:179` が**矛盾**（E-4） |
| 9 | 「元ファイルをゴミ箱へ移動…」 | `context_menu_model.rs:1209, 1449` | `settings.html:207` | 記載あり |
| 10 | 「元ファイルの Windows メニュー」submenu | `native_context_menu.rs:108` | `settings.html:207` | 記載あり |
| 11 | 見つからない / 未対応の項目を一覧に残す | `collection-spec-proposal.md`「見つかりません」 | `settings.html:207` | 記載あり |
| 12 | 履歴の戻る / 進むにコレクションが入る | `collection-implementation-plan.md:1006-1012`（`FolderNavHistoryTarget`） | `settings.html:207, 496` | 記載あり |
| 13 | <kbd>Ctrl</kbd>+<kbd>↑↓</kbd> / スライドショー / 連続再生の範囲 | `collection-playback-plan.md:11-17`、`collection-implementation-plan.md:902-916` | — | **無し** |
| 14 | Remote から閲覧できる（読み取り専用） | `collection-implementation-plan.md:918-930`、`app.js:3811, 5169` | — | **無し**（`remote.html:204` からも漏れ＝E-5） |
| 15 | 保存先 `collection.db` / 設定の復元やメタ情報の移行の対象外 | `src/lib.rs:1291`、`settings_restore.rs` に collection 参照なし（grep 0 件） | — | **無し**（`privacy.html` からも漏れ＝E-10） |
| 16 | できないこと（入れ子 / ZIP 内ページ単独登録 / 再リンク / Remote 編集） | `collection-spec-proposal.md`「実装前に固定する境界」、`keymap.rs:2962` | — | **無し** |

→ 16 項目中 **6 項目が完全に未記載**、2 項目が不足、1 項目が他ページと矛盾。
専用ページ `collections.html` を新設し、`settings.html` は設定リファレンスとしての最小記述 +
`collections.html` へのリンクに寄せるのが構造的に正しい。

---

## §C. version_highlights v4.0.0 案（項目 4）

`src/version_highlights.rs` の `TABLE` 末尾（現在の `:853` の `"3.10.0"` の後）へ追加する。
`must_read` = 操作・既定の変更、`highlights` = 主な新機能。内部用語は使わない。

**must_read（4 件）— いずれも「既定で見え方・意味が変わる」もの**

1. `title`: 「ツールバーとメニューに『コレクション』が増えます」
   `body`: 「新しい『コレクション』機能のために、ツールバーに『コレクション:』セクション、メニューバーに
   『コレクション』メニューが追加されます。ツールバーのセクションを隠したい場合は、ツールバーの空き領域を
   右クリックしてチェックを外してください。メニューは環境設定の『表示 → 通常メニュー』で隠せます。」
   （根拠: `settings.rs:6839` の既定 true、`keymap.rs:3055-3060` の保存済み順への補完）

2. `title`: 「コレクションの一覧では <kbd>Delete</kbd> の意味が変わります」
   `body`: 「コレクションの一覧で Delete を押すと、コレクションへの登録だけが外れます。元のファイルや
   フォルダは削除しません。元のファイルを操作するときは『元ファイルをゴミ箱へ移動…』を選んでください。
   コレクションから開いたフォルダの中では、これまでどおり元ファイルの操作になります。」
   （根拠: `collection-implementation-plan.md:1017-1021`、`keymap-spec.md:340`）

3. `title`: 「ソート順の名前が変わり、降順とサイズ順が増えます」
   `body`: 「『ファイル名順』は『名前（昇順）』、『番号順（区切り無視）』は『番号（昇順）』という表示に
   なります。あわせて名前・番号の降順と、サイズ順（小さい順・大きい順）を選べるようになりました。
   これまで選んでいた並び順はそのまま引き継がれます。」
   （根拠: `38092874a` / `3438f9727` と `manual/grid.html:269-282` の差分）

4. `title`: 「フォルダツリーの並び順が一覧のソートから独立します」
   `body`: 「左のフォルダツリー上部で『名前』『番号』『日付』の昇順・降順を選べるようになりました。
   Ctrl+↑↓ と Ctrl+PageUp/PageDown のフォルダ移動はこの並びに従い、右側の一覧のソートを変えても
   変わりません。」
   （根拠: `614715726` と `manual/tut-navigation.html` の追記）

**highlights（新機能）**

- コレクション（名前付きの一覧・手動順・元ファイル非変更）
- コレクションでのスライドショー・連続再生・Ctrl+↑↓
- テキストからの取り込み / 書き出し
- mIV Remote からのコレクション閲覧（読み取り専用）
- 起動できなかった環境の修正（VC++ ランタイム同梱）

※ `V3_10_HIGHLIGHTS` と同じく `const V4_0_HIGHLIGHTS` を切って参照する形が既存の書き方に沿う。
追加後は `cargo test --lib version_highlights::` を通す。

---

## §D. リリース準備物の機械チェック結果（項目 8）

| チェック | コマンド | 結果 |
| --- | --- | --- |
| D-1 サイトマップ | `python scripts/gen-sitemap-xml.py --check` | **out of date**（E-9） |
| D-2 サイドバーリンク数 | CLAUDE.md Phase 1 手順 6 のループ | **29 ページすべてが 29 リンクで一致**（問題なし） |
| D-3 `installer/readme.txt` | `head` | BOM あり・`Version 3.10.0`（Phase 1 で更新） |
| D-4 `installer/readme_portable.txt` | `head` | BOM あり・`ポータブル版 Version 3.10.0`（Phase 1 で更新） |
| D-5 `docs/panic-acknowledged.tsv` | `ls` | **存在する**（3,540 バイト、2026-09-07 更新）。Phase 2 手順 6.4 の `check-panic-log.ps1` は本監査の範囲外（アプリのログを読むため未実行） |
| D-6 更新履歴のバイト数上限 | `wc -c` on 下書き | **6,859 / 8,192**（§A） |
| D-7 禁止語 | `git grep` + `git log -S` | 公開文書・コミットメッセージに **0 件**。既存 3 箇所は `.rs` 内部のみ（E-27） |

---

## §E. 監査したが問題なしだったもの

- **`index.html:1200-1207`「🌐 通信するのは 3 つの場面だけ」** — コレクションは通信経路を増やさない
  （`docs/collection-remote-plan.md:35-45` のとおり既存 Remote 経路を読むだけ）。全称表現は v4.0.0 でも偽にならない。
- **`remote.html:353`「共有されるのはファイルだけです」** — ネットワークドライブ割り当ての説明であり、
  mIV Remote の話ではない。コレクション追加の影響を受けない。
- **`docs/spec.md:377`「再リンク UI は提供しない」は実装と一致している** — 別担当の調査で
  「再リンクは撤去されていない」との報告があったが、**反証できた**。
  `src/keymap.rs:2960-2967` の `menu_command_is_available_in_build` が
  `MenuCommandId::CollectionsRelinkCurrent => false` を返し、`menu_commands_for_parent`（同 `:2970-2975`）が
  全メニューから除外する。`src/ui_main.rs:6207-6210` のハンドラは
  「Kept as a stable saved-menu command ID for settings migration. It is filtered from all rendered menus.」
  というコメントだけの空実装。テスト `src/keymap.rs:9951-9958` が非表示を固定している。
  `collection_store` 側の `relink` は残るが UI からの到達経路は無い（`grep -rn 'Relink' src/` で
  `collections.rs` / `keymap.rs` / `collection_store/` 以外に呼び出し元なし）。
  → **`settings.html:206, 487` の「参照先の変更は登録解除→再追加で行います」も正しい。**
- **`docs/spec.md:367` の表示形式「展開 / 折りたたみ」** — 実装（`collection-implementation-plan.md:1056-1060`、
  旧 `プルダウン` / unknown は `展開` へ正規化）と一致。`settings.html:211, 487` も一致。
- **`docs/spec.md:2015, 2034, 2035, 2036`** — `show_toolbar_collections` /
  `toolbar_collections_display` / `toolbar_collection_target_id` / `toolbar_collections_collapsed` の 4 設定は掲載済み。
- **`docs/top-level-grid-view.md:104-134`** — Collection surface の所有境界が十分に記述されている。
  今回確認した設計文書 11 本のうち、コレクションが適切に反映されている唯一の文書。
- **`docs/README.md:35-38`** — collection 系 4 文書はすべて索引に載っている（案内文の形式だけが E-23）。
- **`docs/keymap-spec.md:340`** — コレクション直下の <kbd>Delete</kbd> の意味変化は設計文書側に記述済み。
- **`docs/keymap.ini.default:186`** — `GridDelete` のコメントに同じ分岐が入っている。
- **モザイク・成人向け表記ポリシー** — v4.0.0 の差分（`git diff v3.10.0..HEAD`）に隠蔽加工の
  文言変更は無く、新規の投稿サイト名・基準名の混入も無い。
- **`docs/display-pipeline.md:440`** — Remote のコレクションと回転キーの共有は記述済み
  （ただし永続コレクション固有の記述は無い＝E-13 の範囲）。

---

## §F. 出荷前に決めてほしいこと（判断が要る 3 点）

1. **`manual/collections.html` と `tut-collections.html` を新設するか**。新設しない場合、
   §B の未記載 6 項目（特に取り込みテキストの形式と Remote 対応）をどこへ書くかを決める必要がある。
   新設する場合はサイドバーの 29 → 30 リンク同期が全ページに波及する（E-3）。
2. **`index.html` にコレクションを「主な機能」カードとして出すか、「そのほかの便利な機能」の 1 行にするか**。
   メジャー版の目玉であること、§1.118 の出典が移行検討者であることを考えると前者を推す（E-2）。
3. **`docs/next-release-backlog.md` §1.118 を削除するか書き換えるか**（E-18）。
   バックログの運用ルール上は「完了したら削除」だが、§1.244〜1.246 のような周辺の未確認項目が
   残っているので、状態行の書き換えのほうが安全かもしれない。
