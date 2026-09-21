# v4.0.0 コレクション機能 出荷前レビュー 最終報告

作成: 2026-09-16 / 統括: ClaudeCode Fable 5.1 / 監査: ClaudeCode Opus 5 ×5 (Codex 不使用)

> **2026-09-21 追記**: 本書の指摘に対する修正の再レビューは [re-review-20260921/README.md](re-review-20260921/README.md)。初回の出荷前必須は概ね解消、修正由来の新規 P1 が 2 件 (バックアップのログ欠落、壊れた journal による削除の恒久拒否)。

## 0. 体制と前提

- **対象**: `master` HEAD `0a7139d27`。コレクション実装は `d8b61ff20` (Add persistent collection storage)
  から HEAD までの 36 commit。主要な新規コード: `src/ui_dialogs/collections.rs` (6,109 行)、
  `src/app/collection_navigation.rs` (4,398 行)、`src/app/collection_grid.rs` (3,265 行)、
  `src/remote_ipc/persistent_collections.rs` (1,940 行)、`src/collection_store/` (約 5,000 行)、
  `src/app/top_level_grid_view.rs` (+568 行)、`crates/remote-web/web/app.js` (+1,276 行)。
- **体制**: Fable が統括・設計面の総評・競合比較・重要指摘の再検証を担当し、Opus 5 体が領域別に監査した。
  - A: UI の作り・操作体系の整合性 → [A-ui-consistency.md](A-ui-consistency.md)
  - B: UI スレッド阻害・毎フレームコスト → [B-ui-thread-blocking.md](B-ui-thread-blocking.md)
  - C: 状態所有・ライフサイクル・正しさ → [C-state-correctness.md](C-state-correctness.md)
  - D: mIV Remote 側の統合 → [D-remote.md](D-remote.md)
  - E: 利用者向け文書・設計文書・リリース準備物 → [E-docs-release.md](E-docs-release.md)
  - Fable の下書き (競合比較・ソート不整合の根本原因) → [F-G-fable-draft.md](F-G-fable-draft.md)
- **方法**: 読み取り専用のコード照合と Web での競合仕様確認のみ。**アプリは起動しておらず、cargo も実行していない。
  実行時の観測はゼロ**であり、本書の「〜になる」はすべてコード上の推定である。利用者の実機観測は
  「利用者報告」と明記する。重要指摘は Fable が根拠箇所を読み直して「再検証」欄に記した。
- **触れていないもの**: 作業ツリーにあった別セッション由来の未コミット差分
  (`docs/next-release-backlog.md` の §218 追記、`docs/rating-snapshot-order-plan.md`、行末だけの 2 ファイル) には
  一切触れていない。本レビューが追加したのは `docs/review-v4.0.0/` と `docs/README.md` の索引 1 行だけである。
- **消費**: 監査エージェント 5 体の合計は約 193 万トークン。

## 1. 総合判定

**現状のまま v4.0.0 として出荷することは勧めない。** ただし理由は基盤の欠陥ではない。

- **基盤は構造的に正しい**: 単一 actor と immutable snapshot、request stamp による stale 結果の棄却、
  削除の fail-closed、参照解除と元ファイル操作の分離、rename/move migration の全コレクション単一
  transaction、Remote の read-only と認証境界は、5 領域すべてでデータ破壊・誤削除・権限逸脱に当たる
  P1 が **0 件**だった (C / D の「問題なし」節参照)。
- **出荷を止める理由は 2 つ**:
  1. 利用者に最初に見える操作面の不整合。利用者報告のソート不整合 (A-1) は、効かないだけでなく
     全フォルダ共通のソート設定を黙って書き換える副作用を持つ。詳細表示の列ヘッダソートが手動順を
     黙って上書きする (A-2)、更新中の右クリックで安全側の項目だけが消える (A-3) も同根である。
  2. メジャー版の目玉なのに利用者が読む場所にほぼ存在しない。製品ページ 0 件、マニュアル専用ページ無し、
     Delete キーの説明が偽になる、Remote マニュアル未記載、README / 重要な変更点 / 版表記が未着手 (E-1〜E-7)。

### 1.1 出荷前に直すべきもの (P1 相当)

| ID | 件名 | 出典 | Fable 再検証 |
| --- | --- | --- | --- |
| A-1 | コレクション直下でツールバー / 表示メニューのソートが効かず、`settings.sort_order` を黙って書き換えて保存する | A, G | コード確定 (`app.rs:32940` / `51411` / `20502-20600`, `ui_main.rs:8817-9030`, `6493-6530`) |
| A-2 | 詳細表示の列ヘッダソートが手動順を黙って上書きする。`open_collection_grid` が `reset_details_sort_to_toolbar()` を呼ばない | A | コード確定 (`app.rs:51399`、呼び出し元 4 箇所にコレクション無し) |
| A-3 / C-11 | 一覧更新中 (`Unavailable`) の右クリックで「コレクションから外す」だけが消え「元ファイルをゴミ箱へ移動」が残る。Delete キーは fail closed なのに非対称 | A, C | コード確定 (`context_menu_model.rs:1427-1455`) |
| B-1 | インポート確認画面が全行を仮想化なしで毎フレーム描画。行数・サイズ上限も無い。並べ替え画面は `show_rows` 仮想化済みで扱いが割れている | B | 未再検証 (B の根拠 `collections.rs:3103-3118`, `2884`, `text.rs:97`) |
| M-1 | ファイル名変更の collection migration scope が常に `Tree`。`clear_rename_dialog_state()` が `rename_target_is_file` を先に false へ戻す | C | コード確定 (`rename_item.rs:117` → `:132`, `:202`) |
| E-1〜E-7 | README v4.0.0 節 / 製品ページ / マニュアル専用ページ / `shortcuts.html` の Delete 説明 / `remote.html` / `version_highlights` / 版表記 | E | grep で確認 (製品ページ 0 件、`version_highlights` 最終 3.10.0) |

### 1.2 出荷前に直すことを推奨するもの (P2)

| ID | 件名 | 出典 |
| --- | --- | --- |
| A-4 | キーボード / リング / ゲームパッドからコレクションを操作する入口が無い。本棚は Grid / Fullscreen / Video の 3 面で `Ctrl+B` を持つ | A |
| A-5 | コレクション表示中のウィンドウタイトルが「mimageviewer」だけになる (他の合成一覧は専用文言) | A |
| A-6 | 仕様の「元の場所を開く」(登録元フォルダへ mIV 内で移動) が未実装。検索結果の「フォルダに移動」相当が無い | A |
| B-2 | revision が 1 進むだけで root 全体を再 install し、thumbnail pipeline を作り直す (名前変更でも発火) | B |
| B-3 | Grid の `Snapshot` / `Preparing` 待ちに repaint 駆動が無い。開いた直後にマウスを動かさないと一覧が出ない可能性 (推定) | B |
| B-4 / D-2 | ページ送り 1 回ごとに actor 読込 → 全 entry を再 stat → 既存と同一なら捨てる。件数と記憶装置の遅さに比例して送りが遅れる | B, D |
| B-5 | `migrate_source_batch` が全コレクション全 entry を読み M×N で突き合わせ、その間 UI と Remote の全要求が待つ | B |
| B-6 | コレクション機能に `perf::event` 計装が 1 つも無い。上の指摘を `--perf-log` で検証できない | B |
| C-1 / C-2 / C-3 | 一過性の `Busy` / `Starting` / 並べ替え保存中を恒久的な利用不可と同じ値に潰す。並べ替え保存中にスライドショーが末尾扱いで停止、Grid が終端 `Failed` で固着、管理 window が古いまま止まる | C |
| M-2 | rename journal の読込 I/O 失敗を「空」と同一視し、その後 journal ごと削除する (前セッションの未完了 migration が無言で失われる) | C |
| D-1 | Remote の catalog 要求が core の Home レーン (worker 1 本) を最長 9 秒占有し `/api/home` `/api/list` を巻き添えにする | D |
| D-3 | Remote のコレクション root だけ動画の同名画像 sidecar が効かない (`thumbnail_address: None`)。あわせて `rating: None` 固定で★も出ない | D, Fable |
| D-4 | deep link 着地の ordinal 基準 (`navigable_media`) とシーク要求の基準 (`still_image`) がずれる | D |
| E-8〜E-21 | privacy.html / 「安心して使えます」の保存データ列挙、設計文書 (architecture / async / virtual-folders / keymap-spec / spec.md) の同時更新漏れ、移行ガイドの対応表、sitemap | E |

### 1.3 利用者の判断が要るもの

| 論点 | 概要 | 出典 |
| --- | --- | --- |
| 手動順のソート UI | 「手動順」をソートのプルダウン / ボタン列に追加する案 C (§2) で進めてよいか。上部「コレクション」メニューの「並び順」を残すか | A, G |
| シャッフル | 仕様の「BGM 用・順序なし」は実装上 Standard (通常ソート) に写されただけで、アプリ全体にシャッフル / ランダム再生が無い。v4.0.0 に入れるか、既知の制限として明記して v4.x へ送るか | F |
| 大規模コレクションの前提 | B-1 / B-4 / B-5 / C-10 / D-1 は「数百件まで」なら実害が小さい。Remote 側は上限 100,000 件を持つ。前提件数をどこかに明記しないと、後の報告が設計判断か退行か区別できない | B, D |
| 並べ替え画面の × | dirty 時に自動保存する (テストで固定済み)。本棚の並べ替えモード退出時フラッシュと同型なので妥当と判断するが、マニュアルに明記が要る | C (K-2), Fable |
| `collection.db` の保護 | 世代バックアップも破損時の復旧導線も無い。設定復元の対象外は計画どおり。テキスト export だけが可搬手段であることを明記するか、将来「全ユーザーデータ」操作へ統合するか | C (K-1) |
| マニュアル構成 | `collections.html` + `tut-collections.html` を新設するか (サイドバー 29 → 30 リンクの全ページ同期が要る)。製品ページで「主な機能」カードにするか | E (§F) |
| バックログ §1.118 | 削除するか状態行を書き換えるか (§1.244〜1.246 の周辺未確認項目が残る) | E |

## 2. 利用者報告「手動順にしてもツールバーのソートが固定されず、変えても効かない」

観測者: 利用者 (実機)。以下は Fable と A のコード照合で一致した結論。

### 2.1 根本原因

1. 本 / 閲覧履歴の「固定」判定 `page_order_locked_for_current_view` (`src/app.rs:32940`) は
   `items_are_*` の bool フラグと `current_folder` だけを見る。コレクションは
   `TopLevelGridSurface::Collection` (`src/app/top_level_grid_view.rs:687`) の enum だけで表現され
   bool フラグを持たず、root では `current_folder` が無いので `false` を返す。
   よって `grid_sort_lock_reason` (`src/app.rs:51411`) は `None` となり、ツールバーは固定表示にならない。
2. ツールバー (`src/ui_main.rs:8817-9030`) と表示メニュー (`6493-6530`) のソート選択は
   `settings.sort_order` を書き換えて保存し、`apply_sort_change_reload` (`src/app.rs:20502`) を呼ぶ。
   この dispatcher の分岐列 (zip_nav / 検索 / タグ / 評価 / ブックマーク / スマートフォルダ / サブフォルダ展開 /
   `current_folder` の物理再読込) にコレクション root は無く、何も起きない。
3. コレクションの並びの正本は DB 定義 `CollectionDefinition { order_mode, standard_sort }`
   (`src/collection_store/model.rs:158, 186`) で、変更経路は上部「コレクション」メニュー
   (`src/ui_main.rs:6162-6205`) の `SetOrder` だけ。ツールバーはこの定義を読んでいない。
4. **副作用**: 手順 2 で global の `settings.sort_order` だけが書き換わって保存されるため、コレクションを
   閉じて実フォルダへ戻ると全フォルダ共通の並びが変わっている (推定。利用者報告には含まれていないが
   コード上は必ず起きる)。
5. **同根の A-2**: 詳細表示の列ヘッダソート (`details_header_sort_active`、`src/app.rs:51399`) も
   コレクションを除外せず、`open_collection_grid` が `reset_details_sort_to_toolbar()` を呼ばない
   (呼び出し元は評価 / ブックマークの 4 箇所のみ)。別フォルダで列ヘッダを押したまま手動順の
   コレクションを開くと、手動順が消えてその列順で表示される。

### 2.2 修正設計 (案 C。利用者の「手動順をプルダウンに追加」に沿う)

先例: 評価一覧 `RatingViewSort { Normal(SortOrder), RatedAtDesc, RatedAtAsc }` (`src/rating_view.rs:12`) と
ブックマーク一覧 `BookmarkViewSort` (`src/bookmark_browser.rs:199`) は、通常ソートの選択肢に
一覧固有の項目を同じコンボ / ボタン列へ並べている (`src/ui_main.rs:8876-8905`)。コレクションは
`CollectionOrderMode + standard_sort` という同じ形の型を既に DB 側に持つので、UI はそれを読むだけでよい。

1. **表示値**: コレクション root では `settings.sort_order` を見ず、定義から
   `Manual → 「手動順」 / Standard → standard_sort` を選択表示する。Dropdown の `current_text`
   (`ui_main.rs:8907-8915`) と Buttons の `selected` (`8838-8850`)、表示メニューの ✓ (`6507`) の 3 箇所。
2. **選択肢**: 評価一覧が `RatedAtDesc/Asc` を足しているのと同じ場所に「手動順」を 1 項目追加する。
   hover には「並べ替えはコレクションメニューの『現在のコレクションを並べ替え…』」を出す。
3. **`settings.sort_order` を汚さない**: コレクション分岐では `self.settings.sort_order = order` を実行せず、
   既存の `start_collection_grid_content_action(target, SetOrder { mode, sort })` (`ui_main.rs:6193-6199` と
   同じ呼び出し) だけを発行し、actor の revision 更新で一覧が収束するのを待つ。計画 §2.4 の
   「collection の標準ソートは通常一覧の `SortOrder` 値を明示保存するが、他設定を更新しない」に従う。
   評価 / ブックマークが global を書いてから同期しているのとは意図的に変える。
4. **`apply_sort_change_reload`**: コレクション分岐を足して早期 return する (actor 経由で更新される)。
5. **列ヘッダ**: 手動順のときは `page_order_locked_for_current_view` を true にして `PageOrderFixed` を返し、
   列ヘッダソートを `sort_enabled = false` (`ui_main.rs:15638`) にする (A-2 を同時に閉じる)。通常ソートの
   ときは列ヘッダソートを許すが、`open_collection_grid` と履歴からの Collection 復帰で
   `reset_details_sort_to_toolbar()` を呼ぶ (§1.143(a) の規約どおり)。
6. **上部メニューの「並び順」は残す**。切替が 2 箇所になるが、両方が同じ 1 つの述語
   (`collection_definition().order_mode`) から表示を導けば食い違わない (`src/app.rs:51406-51410` の
   コメントが求める「ツールバーもメニューも同じ 1 つの述語を見る」を成立させる)。
7. **追加検討**: 「登録順 (追加日時)」を Standard の選択肢に加える。entry の `created_at` は DB にあり、
   手動で並べ替えた後に登録順へ戻す手段が現状無い (NeeView はプレイリスト限定で「登録順」を持つ)。
8. **回帰**: root で (a) Manual → 「手動順」表示 (b) Standard(DateDesc) → 日付↓表示 (c) 名前順を選ぶ →
   `SetOrder(Standard, FileName)` が actor に届き `settings.sort_order` 不変 (d) 物理フォルダへ戻ると
   global 表示に戻る (e) 列ヘッダを押した状態から手動順コレクションを開くと手動順が保たれる、を
   handler-level test で固定する。

## 3. 設計面の総評 (領域をまたぐテーマ)

### T1. surface enum と bool フラグの二重表現

他の合成一覧 (Bookmarks / Rating / SmartFolder / ReadingHistory / Search / SubfolderExpansion) は
`items_are_*` の bool を持ち、テストを除いて 533 箇所がそれで分岐する。コレクションだけ enum のみで
表現したため、既存の分岐が「物理フォルダ扱い」に落ちる。A-1 / A-2 / A-5 (タイトル) / A-6 (フォルダに移動) /
A-15 (BS・⬆) はすべてこの型の欠落である。
**提案**: 新しい bool を足すのではなく、`top_level_grid_view.surface()` から導く単一の typed 述語
(例: `fn top_level_view_kind(&self) -> TopLevelViewKind`) を用意し、少なくとも
`page_order_locked_for_current_view` / `grid_sort_lock_reason` / `details_header_sort_active` /
`apply_sort_change_reload` / `main_window_title` / `can_jump_to_folder` / `ContextMenuViewFlags` を
そこへ寄せる。A の「監査したが問題なし」節に、安全側に無効化される経路の一覧がある。

### T2. 一過性と終端の混同

C-1 / C-2 / C-3 / B-3 は同じ型に帰着する。`Busy` / `Starting` / 自分の保存が in-flight という一過性の
状態を、恒久的な `Unavailable` と同じ値 (終端 `Failed`、無言 drop、「次が無い」と区別できない `None`) に
潰している。`collection_store_client_for_migration` (`src/ui_dialogs/collections.rs:1642`) は既に
`Result<Option<_>, CollectionStoreError>` で正しく型付けしているので、read 経路の入口をその型へ揃え、
一過性は `RequestNeeded` + `request_repaint_after` で再駆動するのが構造的修正になる。
CLAUDE.md「バグ修正の一般原則」の「複数 bool / Option / sentinel を単一の typed owner へ」に該当する。

### T3. ページ送りごとの全件再 prepare (仕様 §7.3 のコスト)

`enqueue_collection_navigation` (`src/app/collection_navigation.rs:839`) は next / prev / EOF のたびに
actor へ `load_collection` を送り、`spawn_collection_navigation_prepare` (`:1280`) が別スレッドで
`prepare_collection_grid_install` を走らせる。prepare は entry ごとに直列 `inspect_collection_source`
(`src/collection_store/prepare.rs:383`) を撃ち、最後に installed presentation と比較して同一なら捨てる
(`:2130`)。コードのコメントは「revision 一致だけでは filesystem の変化を拾えない」という意図を明記しており、
仕様 §7.3「次項目を決める直前に…現在の resolved kind / availability を非同期で取得」に忠実である。
しかし **1 回の矢印キーが O(entry 数) の stat になる**コストは仕様検討時に評価されていない。UI スレッドは
止まらないが、送り遅延が件数と記憶装置の遅さに比例する。Remote も同じ (D-2、しかも stat 2 回 + canonicalize)。
**提案**: actor revision が installed と一致し installed presentation が存在する場合は、選ばれた target
entry (と必要なら隣接数件) だけ availability を確認し、全件 stat は revision が進んだときと明示更新に限る。
仕様の「availability を取得」は target 1 件で満たせる。

### T4. 計装ゼロ

コレクション関連ファイルに `perf::event` が 1 つも無い (B-6)。CLAUDE.md §4 チェックリストの
「追加した同期処理の区間には perf::event を必ず差し込む」に反し、B-1 / B-4 / B-5 を `--perf-log` で
検証できない。修正の前に計装を入れ、修正後の効果を同じ指標で測る順序を勧める。

### T5. キー操作の入口ゼロ

本棚は `GridAddToActiveBook` / `FsAddToActiveBook` / `VideoAddToActiveBook` (既定 `Ctrl+B`) と
リングショートカット `AddToBook` を持つが、コレクションは `KeyAction` が 0 件で、追加 / 開く / 管理は
マウスでツールバーか上部メニューを操作するしかない (A-4)。仕様の主用途 (BGM 用・スライドショー用) では
フルスクリーン再生中に「これも入れる」ができない。フルスクリーンの Delete も no-op である (A 追加項目 1)。
最低限 3 面の `AddToCollectionTarget` を `KeyAction` に追加し、keymap 対象外にするなら理由を
`docs/keymap-spec.md` に残す。

### T6. 「本棚同型」の実際の差

管理 window / 並べ替え window の本棚とのパリティ差分 (A-14)、`default_pos` 無し (A-8)、Esc で閉じない modal
(A-17) は小さいが、利用者は「本棚と同じ」と説明されて触るので差が目立つ。並べ替え画面の × の自動保存
(K-2) は本棚と同型なので妥当と判断する。

### T7. 大規模コレクションの前提が未定義

Remote は `MAX_REMOTE_COLLECTION_ENTRIES = 100_000` を持つ一方、PC 側は B-1 (全行描画)、B-4 (全件 stat)、
B-5 (M×N)、C-10 (1 件操作で全件走査 ×4) が件数に比例する。「数百件まで」を前提にするならその旨を
設計文書と known-issues に明記し、そうでないなら B-1 / B-4 を先に直す。

## 4. 各監査の要約

### 4.1 A: UI 整合性 (P1=2 / P2=5 / P3=11 / 設計提案=1)

P1 / P2 は §1 の表のとおり。P3 の件名: 管理 / 並べ替え window に `default_pos` 無し (A-8)、「並び順」submenu が
無効理由を出さない・✓ 表記不統一 (A-9)、場所▼にコレクション無し・本棚はある (A-10)、Ctrl+F 絞り込みが
watch 更新で消える (A-11)、横断一覧なのに「場所」ファセットが強制表示されない (A-12)、エクスプローラからの
D&D が「コピー先のフォルダがありません」で拒否 (A-13)、本棚とのパリティ差分 (A-14)、root で BS・⬆ 無反応
(A-15)、`version_highlights` 未記載 (A-16)、modal が Esc で閉じない (A-17)、Remote と PC の文言差 (A-18)。
追加項目: フルスクリーンの Delete は削除でも参照解除でもなく no-op (入口自体が無い)、表示メニューの
ソート順はツールバーと同じ空振り、「元の場所を開く」は未実装。ついでにブックマーク一覧で表示メニューの ✓ が
`bookmark_view_sort` を見ていない既存不整合も記録されている。

### 4.2 B: UI スレッド・性能 (P1=1 / P2=5 / P3=12)

**同期 I/O が UI スレッドへ到達する経路は、既存の許容済みパターン (rfd のネイティブダイアログ等) を除いて
見つからなかった。** client API は `try_send` + `Receiver` 返し、UI 側は全経路 `try_recv`。指摘の重心は
毎フレームの無駄、大量件数時の O(N)、計装の欠如。Fable 再検証: B-4 はコード確定、B-2 は install 経路に
presentation 一致判定が無いことを確認、B-3 は `poll_collection_grid` が結果到着時と削除時にしか
`request_repaint` せず、待機状態が tail repaint reasons にも `poll_collection_ui` の 50ms 条件にも無いことを
確認した (症状の有無は推定)。P3 の主なもの: toolbar target の毎フレーム HashSet 再構築と `settings.save()`
到達 (B-7)、catalog 名の毎フレーム clone (B-8)、install 時の `prewarm_grid_tags` が UI スレッドで全件 SQLite
SELECT (B-10)、presentation 比較が UI スレッドで全 entry deep 比較 (B-11)、`on_exit` の無期限 `recv()` (B-19)、
`Starting` の 20Hz repaint に上限なし (B-20)。

### 4.3 C: 状態所有・正しさ (P1=0 / P2=5 / P3=12 / 懸念=11)

壊れると重い箇所 (stamp 所有分離、fail-closed、参照解除と実ファイル削除の分離、migration の単一 transaction) は
構造的に正しい。P2 は M-1 (Fable 再検証で確定)、C-1〜C-3 (T2)、M-2。P3 の主なもの: notice 不在を catalog
revision を比べずに「削除」と断定 (C-4)、revision watch subscriber が閲覧だけで単調増加 (C-5、Remote は
16 倍)、`%` を 2 個含むパスの一律拒否 (C-6)、末尾ドットパスの二重登録 (C-7)、UTF-8 以外の import 失敗文言が
英語 (C-8)、終了時 migration 待ちに deadline 無し (C-9)、大規模で全件走査 ×4 (C-10)、決定的
`DuplicateSource` の毎起動リトライ (M-4)、journal tmp の fsync 無し (M-7)。文書と実装の食い違い 1 件:
計画 §16.5「enqueue 済み command は result まで処理する」に対し実装は Shutdown 優先で `Unavailable`
(テストで意図として固定済み) → 文書側を実装に合わせる。テスト棚卸し約 130 件と欠落 16 件 (T-1〜T-16) は
C の §11。

### 4.4 D: Remote (P1=0 / P2=4 / P3=10)

read-only の強制 (protocol に mutation variant が無い)、認証・閲覧範囲、二重 path 検証の逸脱は無し。
Fable 再検証: `EXACT_REQUEST_BUDGET` 9 秒 × `MAX_EXACT_RESTARTS` 16、`work_lane` で
`PersistentCollectionCatalog` が Home レーン、`thumbnail_address: None` と `rating: None` 固定を確認。
公開文書の「通信するのは 3 つの場面だけ」は偽になっていない (既存のリモート閲覧トグルの内側で通信先が
増えていないため)。問題は「端末内に保存されるデータ」の列挙漏れ。P3 の主なもの: `image_count` が二重検証前の
値 (D-5)、`.grid-unavailable` の CSS 不在 (D-6)、catalog タブが `order` を捨て再読込導線が無い (D-7)、
ホームタブ 6 個の 375px 折り返し (D-8)、ZIP をコレクションから開いた後の「上へ」が物理親へ逸れる (D-9)、
`BlockedByRemotePolicy` 行がファイル名を出す (D-10)。

### 4.5 E: 文書・リリース準備 (P1=7 / P2=14 / P3=6)

P1 は §1.1 の表。E の報告書には README v4.0.0 節の下書き (6,859 / 8,192 バイトで `BODY_CAP` 内、v3.10.0 に
含まれる項目は除外済み)、`version_highlights` の `must_read` 4 件 + `highlights` 案、操作 × 記載場所の対応表
(16 操作中 6 つが完全に未記載)、機械チェック結果 (サイドバー 29 × 29 一致、sitemap out of date、
禁止語の新規混入なし) が含まれる。E は Fable の前提 2 点を訂正した: `tutorial.html` の「コレクション」は一般語で
機能説明ではない、`version_highlights` の最終エントリは 3.10.0 (2.2.0 はテスト用データ)。

## 5. 競合比較: 初期実装として期待される機能・仕様

比較対象: NeeView プレイリスト (Web で確認: `.nvpls`、ブックとして開く、スライダーマーク、プレイリスト限定の
「登録順」ソート)、ZipPla 仮想フォルダ `.sor` / スマートフォルダ `.kdk` (窓の杜レビューで確認)、Eagle、
Lightroom、XnView MP、IrfanView、YACReader、音楽プレイヤーの M3U (一般知識。実機確認なし)。

| 領域 | 期待される機能 | mIV v4.0.0 (コード照合) | 判定 |
| --- | --- | --- | --- |
| 追加の入口 | ショートカットで追加 (Lightroom B、本棚 Ctrl+B) | `KeyAction` 無し | **P2** (A-4) |
| | フルスクリーン表示中の画像を追加 (NeeView、Lightroom) | 無し (本棚は `FsAddToActiveBook`) | **P2** (A-4) |
| | 一覧からコレクションへ D&D (Eagle / Lightroom / digiKam) | 無し。並べ替え画面内の drop のみ | P3 (mIV は D&D カーソルを既定 OFF にする方針があるため v4.x) |
| | 「新規コレクションを作って追加」を 1 手で | 管理 window で作成 → 追加先に設定 → 追加 (3 手) | P3 |
| 一覧・閲覧 | 件数表示 (Eagle / Lightroom) | 管理 window / コンボに無し | P3 |
| | 登録元 (場所) の表示 | `DetailsColumnId::Place` は既存だが強制表示されない | P3 (A-12) |
| | 見つからない項目 | placeholder + 登録解除→再追加 (再リンクは仕様で撤去) | 仕様どおり。マニュアル明記 |
| | 場所メニュー / ツリーからの入口 (NeeView / ZipPla のサイドパネル) | 場所▼に本棚はあるがコレクション無し | P3 (A-10) |
| | 元の場所へ移動 (Lightroom「フォルダに移動」) | 未実装 | **P2** (A-6) |
| 並び・再生 | 手動順 + 通常ソート | 実装済み。ツールバーとの不整合あり | **P1** (§2) |
| | 登録順 (追加日時) ソート (NeeView) | 無し。並べ替え後は戻せない | P2 提案 (§2.2 の 7) |
| | シャッフル / ランダム (NeeView、IrfanView、音楽プレイヤー全般) | **アプリ全体に無い** | 利用者判断 (§1.3) |
| | 終端の停止 / ループ / 次のフォルダ | 既存設定を流用 | OK |
| | 再生中の編集を次の 1 件に反映 | latest-next reducer | mIV 独自の優位。マニュアルで説明する価値あり |
| 入出力 | テキスト一覧 1 行 1 パス (IrfanView / XnView 互換) | 実装済み | OK |
| | M3U / M3U8 | `#` をコメントにしない仕様のため `#EXTM3U` 行が無効行になり取り込みが始まらない | P3 提案: `.m3u/.m3u8` 拡張子のときだけ `#` 行を読み飛ばし、export に `.m3u8` を追加。`.txt` の仕様は維持 |
| | 相手ソフトの形式 (.nvpls / .sor) | 非対応 | 妥当。移行ガイドに「テキストへ書き出して取り込む」手順 (E-21) |
| 管理 | 入れ子 / グループ (Lightroom セット、Eagle、YACReader) | 対象外 (仕様) | 妥当 |
| | コレクション一覧の並べ替え | `catalog_position` 列はあるが UI から並べ替え不可。固定列だけ手動順 | P3 |
| | 参照解除の Undo (Lightroom Ctrl+Z) | 既存 undo stack はレーティング / タグ用で非対応 | P3 |
| | データのバックアップ | `collection.db` は設定復元 / エクスポートの対象外。テキスト export が唯一の可搬手段 | 利用者判断 (K-1)。マニュアル / privacy に保存先を明記 |
| 命名・移行 | NeeView 利用者は「プレイリスト」、ZipPla 利用者は「仮想フォルダ」で探す | 移行ガイド 4 本に該当語が 0 件 | P2 (E-21) |

## 6. 推奨する修正順序

Codex のトークン残量が少ない前提で、ClaudeCode 側で実施する場合の順序。いずれも読み取り専用レビューの
結論であり、修正後は `scripts/build-dev.ps1` で実機確認用バイナリを用意して §7 のシナリオを通す。

1. **計装を先に入れる** (B-6)。`open_collection_grid` / prepare / navigation / import / export / migration の
   各区間に `perf::event` を差し、修正前後を同じ指標で測れるようにする。
2. **A-1 + A-2 を案 C で一括修正** (§2.2)。T1 の typed 述語を同時に導入し、A-5 / A-6 / A-15 も同じ述語へ寄せる。
3. **A-3 / C-11**: `Unavailable(reason)` のとき「コレクションから外す」を無効項目 + 理由付きで残す
   (`MenuNode::Item { enabled, disabled_reason }` の前例あり)。
4. **M-1**: `poll_rename_pending` で scope を先に読むか、`rename_pending` に scope を同梱する (1 行 + handler-level
   回帰 1 件。既存テストは `spawn_rename_key_migration` を直接呼ぶため配線を迂回している)。
5. **T2 (C-1 / C-2 / C-3 / B-3)**: read 経路の入口を `Result<Option<_>, CollectionStoreError>` へ揃え、一過性は
   `RequestNeeded` + `request_repaint_after` で再駆動。B-3 は待機状態を tail repaint reasons に 1 行足す。
6. **B-1**: import 確認画面を `show_rows` で仮想化し、行数・サイズの上限を typed に置く。
7. **A-4**: 3 面の `AddToCollectionTarget` を `KeyAction` に追加 (既定 chord は空でよい)。
8. **文書 (E-1〜E-7、E-10 / E-11、E-21)**: README v4.0.0 節を E の下書きから起こして利用者承認 (Phase 0)、
   `collections.html` / `tut-collections.html` 新設、`shortcuts.html` / `remote.html` / privacy / 製品ページ /
   移行ガイド、`version_highlights` 4.0.0 節、設計文書の同時更新。
9. **v4.0.x 以降へ送ってよいもの**: B-2 / B-4 / B-5 / D-1 / D-2 (T3 / T7。前提件数を明記した上で)、D-3 / D-4、
   M-2、A-8〜A-18 の P3、シャッフル、M3U、登録順ソート、D&D、件数表示。

## 7. 修正後の実機確認シナリオ (利用者向け)

エージェントは実機観測を持たないため、修正後に次を確認してほしい。起動前にインストール版 / 常駐 tray 版を終了し、
引数なしの起動は実利用中の `%APPDATA%\mimageviewer` を使うことに注意。

```powershell
.\scripts\build-dev.ps1
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

1. 実フォルダで「日付↓」を選んだ状態で手動順のコレクションを開く → ツールバーに「手動順」が出て、
   ボタン / コンボ / 表示メニューの ✓ が一致する。実フォルダへ戻ると「日付↓」のまま (global が汚れない)。
2. コレクション root でコンボから「名前↑」を選ぶ → 一覧が名前順になり、上部「コレクション」メニューの
   「並び順」も通常ソート ▸ 名前↑ に変わる。「手動順」を選び直すと元の手動順に戻る。
3. 実フォルダで詳細表示の「サイズ」列ヘッダを押した状態から手動順コレクションを開く → 手動順が保たれる。
4. コレクションへ項目を追加した直後に別項目を右クリック → 「コレクションから外す」が (無効でも) 残り、
   誤って「元ファイルをゴミ箱へ移動」だけが並ぶ瞬間が無い。
5. コレクションの「開く」を押した後、マウスを動かさずに待つ → 一覧が出る (B-3 の確認)。
6. コレクションに登録した画像ファイルを mIV 内で名前変更 → コレクション側の参照が追従し、
   同じ名前を prefix に持つ別 entry が書き換わらない (M-1)。
7. 並べ替え画面で 1 件動かして × を押し、保存中にスライドショーの → を押す → 停止しない (C-1)。
8. 1 万行のテキストを import の確認画面まで読ませる → 画面が固まらない (B-1)。
9. Remote で動画入りコレクションを開く → 同名画像 sidecar のサムネと★が PC と一致する (D-3、v4.0.x でも可)。

## 9. 利用者の判断と回答 (2026-09-16)

§1.3 の判断待ち項目への利用者回答。修正は Codex の制限回復後に行う。反映先は
[仕様案「利用者の判断（2026-09-16）」](../collection-spec-proposal.md) と
[実装計画 §23](../collection-implementation-plan.md)。

| 論点 | 判断 | 補足 |
| --- | --- | --- |
| 手動順のソート UI | **確定**: プルダウン / ボタン列へ「手動順」を追加する案 C | §2.2 のとおり |
| シャッフル | **確定 (2026-09-17)**: 方式 C (下記) | 動画トグルの第 4 状態ではなく、コレクションの並び順「シャッフル」(seed 決定的、手動順は不変) |
| 大規模コレクション | **確定 (2026-09-17)**: 1 コレクション 10,000 件 | 本棚は 1 冊 9,999 ページ (`books.rs` `MAX_BOOK_PAGES`)。Remote は 100,000 を同じ定数から導く |
| 並べ替え画面の × | **仕様**: キャンセル無し、閉じる時に保存 | 本棚も「閉じる時は保存」(compile-book-plan §2.2) で同型 |
| `collection.db` の保護 | **確定 (2026-09-17)**: 世代バックアップ (tags.db と同じ helper) + 「すべてのコレクションをテキストで書き出す…」の 2 段 | rating.db は世代バックアップ無し、tags.db はあり。自動の定期書き出しは v4.x |
| マニュアル | **確定**: `collections.html` + `tut-collections.html` 新設 | サイドバー 29 → 30 リンク |
| バックログ §1.118 | 判断待ち | 下記 |

### 9.1 シャッフル方式の比較 (ClaudeCode の提案)

| 方式 | 次の曲が見える | 前後移動 | 手動順を壊す | 実装 |
| --- | --- | --- | --- | --- |
| A. 動画トグルに「シャッフル (ループ)」を足す | 見えない | 再生履歴スタックが要る | 壊さない | 通常フォルダにも効くが、コレクションの latest-next reducer と別に順序 owner が要る |
| B. 一覧をシャッフルして手動順を保存し直す | 見える | 通常 | **壊す** (元に戻せない) | 既存 `ReorderManual` で最小 |
| **C. 並び順「シャッフル」(seed 決定的)** | **見える** | **通常の隣接移動** | **壊さない** | `OrderMode::Shuffle` + `shuffle_seed`、有効順 = `hash(seed, entry_id)` 昇順。reducer 不変、Remote は snapshot を読むだけ |

C を推す。利用者が難点とした「押すたびに順序が固定になる」は、C では「選び直すと seed が変わって並び直る」
であり、固定されている間は次の曲が見えるという利点の裏返しになる。毎周違う順にしたい場合は
「ループ折り返しで seed を引き直す」設定を後から足せる。ループは既存の「連続再生 + ループ」に従う。

### 9.2 大規模コレクションの上限

- 本棚の上限は 1 冊 9,999 ページ (`src/books.rs:15`)。コレクションは同じ桁の 10,000 件を提案する。
- 件数で難しくなるのは並べ替え UI ではない (既に `show_rows` で仮想化済み)。重いのは import 確認画面の
  全行描画 (B-1、仮想化で解決)、ページ送りごとの全件 stat (B-4 / D-2、上限があっても HDD / SMB では秒単位に
  なるため対象 1 件の確認へ縮める)、migration の M×N (B-5、総 entry 数に比例するので上限では抑えられない。
  v4.0.x)。

### 9.3 `collection.db` の保護

- 現状: settings.db と tags.db は `db_backup::rotate_generation_backups` で bak1..bak10 を回している。
  rating.db は回していない。collection.db も回していない。
- 提案: (1) actor 起動時に同じ helper で世代バックアップ (数十行、破損・退行からの復旧)。(2) 上部「コレクション」
  メニューに「すべてのコレクションをテキストで書き出す…」(フォルダへ 1 コレクション 1 ファイル + 一覧ファイル。
  既存 export の流用)。(3) 終了時の自動書き出しと世代管理は v4.x (終了 drain 境界に乗せる設計が要るため)。

### 9.4 バックログ §1.118 の判断材料

- 現状: `docs/next-release-backlog.md` §1.118 は「対応中（2026-09-14）」のまま、仕様案の要点 (確定事項・追加要件・
  MVP 案・テキスト入出力) を長く複製している。実装は Phase 1〜5 と §19〜22 まで完了し、残りは本レビューの修正。
- 運用ルール (同ファイル冒頭): 完了した項目は削除する。設計の正本を plan へ移した項目は「現状と残りだけを短く書いて
  plan へリンク」する。判断待ち・見送りは on-hold へ移す。
- 選択肢:
  1. **今は短い状態行へ書き換える (推奨)**: 「実装完了、v4.0.0 出荷前レビュー (docs/review-v4.0.0) の修正待ち。
     残り = 実装計画 §23」とし、仕様の複製は削除する (正本は仕様案と実装計画)。v4.0.0 公開後に節ごと削除する。
  2. 今すぐ削除する: 修正が残っているので早い。
  3. on-hold へ移す: 着手可能な作業なので不適切。
- 周辺の未確認項目 §1.244〜1.246 (2026-09-15 起票) はコレクション固有ではない。§1.244 (本の中でも列ヘッダソートが
  効く疑い) は A-2 / T1 の「単一述語」修正で同時に閉じる見込みなので、§23.2 の実施時に §1.244 を再確認する旨を
  書き添えるとよい。§1.245 (しおりのページ番号)、§1.246 (Remote の ZIP 並び) は無関係。
- 本レビューでは `docs/next-release-backlog.md` を編集していない。同ファイルには別セッション由来の未コミット差分
  (§218 の追記) が残っており、pathspec commit でも巻き込むため。判断後に書き換える。

## 8. 付録

- 各監査報告書: [A](A-ui-consistency.md) / [B](B-ui-thread-blocking.md) / [C](C-state-correctness.md) /
  [D](D-remote.md) / [E](E-docs-release.md) / [F-G (Fable 下書き)](F-G-fable-draft.md)。
  各報告書の末尾に「監査したが問題なしと判断した項目」があり、再監査の重複を防ぐ。
- 対象コミット範囲: `git log --oneline d8b61ff20^..HEAD` (コレクション以外に ORT 起動修正・VC runtime 同梱・
  TensorRT worker lifecycle・降順ソート・フォルダツリー独立ソートを含む。E の README 下書きが v3.10.0 との
  境界を判定済み)。
- 参照 (Web): NeeView ユーザーガイド https://neelabo.github.io/NeeView/ja-jp/userguide.html 、
  窓の杜 ZipPla レビュー https://forest.watch.impress.co.jp/docs/review/1060360.html
