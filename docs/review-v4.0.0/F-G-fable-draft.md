# Fable 担当分 下書き (F: 競合比較 / G: ソート不整合の根本原因)

## G. 利用者報告「手動順にしてもツールバーのソートが固定されず、変更しても効かない」

観測者: 利用者 (実機)。以下はコード照合。

### 根本原因 (コード上で確定)

1. `src/app.rs:32940 page_order_locked_for_current_view` は `items_are_reading_history_view` /
   `items_are_rating_view` / `items_are_bookmark_view` / `items_are_global_search_view` /
   `items_are_tag_view` / `items_are_subfolder_expansion_view` / `items_are_smart_folder_view`
   の bool と `current_folder` だけを見る。Collection surface は `TopLevelGridSurface::Collection`
   (`src/app/top_level_grid_view.rs:687`) で表現され bool フラグを持たないため、Collection root では
   `current_folder` が無い限り `false` を返す。よって `grid_sort_lock_reason` (`src/app.rs:51411`) は
   `None` → ツールバーのソートは「固定」表示にならない。
2. ツールバーのソート (`src/ui_main.rs:8817-9030`) と表示メニューのソート順 (`src/ui_main.rs:6493-6530`)
   は、選択時に `settings.sort_order` を書き換えて `apply_sort_change_reload` (`src/app.rs:20502`) を呼ぶ。
   この dispatcher の分岐列 (zip_nav / global search / tag / rating / bookmark / smart folder /
   subfolder expansion / `current_folder` 物理再読込) に Collection root は無く、Collection root では
   `current_folder` が無いので何も起きない。コレクションの並びは DB 定義 (`CollectionDefinition.order_mode`
   / `standard_sort`) が正本で、変更経路は上部「コレクション」メニュー (`src/ui_main.rs:6162`) の
   `SetOrder` だけ。
3. 副作用: Collection root でツールバーのソートを押すと **無関係な global `settings.sort_order` だけが
   書き換わり保存される** (次に物理フォルダを開いたとき並びが変わる)。利用者には「効かない」上に
   「別の場所で勝手に並びが変わる」ように見える。

### 揃えるべき既存の型

- 評価一覧: `RatingViewSort { Normal(SortOrder), RatedAtDesc, RatedAtAsc }` (`src/rating_view.rs:12`)。
  ツールバーは Normal を通常ソートのボタン/コンボに写し、一覧固有の「設定時刻↓/↑」を末尾に追加表示する
  (`src/ui_main.rs:8878-8890`)。
- ブックマーク一覧: `BookmarkViewSort { Normal(SortOrder), CreatedAtDesc, CreatedAtAsc }`
  (`src/bookmark_browser.rs:199`)。同じ型。
- 本 / 閲覧履歴: `GridSortLockReason::PageOrderFixed` でソート UI を無効化し、理由をツールチップ表示。

### 修正設計案 (利用者の希望「手動順をプルダウンに追加」に沿う)

- 一覧固有ソート型の先例に合わせ、Collection root ではツールバー / 表示メニューのソート選択肢に
  **「手動順」を追加** し、`order_mode == Manual` のときそれを選択表示する。Standard のときは
  `definition.standard_sort` を選択表示する (global `settings.sort_order` を表示しない)。
- 選択時は `settings.sort_order` を **書き換えず**、既存の
  `CollectionGridSnapshotAction::SetOrder { mode, sort }` を actor へ送る (メニューと同じ owner)。
  = 「1 つの意味を 2 箇所に書かない」。上部「コレクション」メニューの「手動順 / 通常ソート▸」は
  残してもよいが、同じ action へ収束させる。
- 「手動順」の hover には「並べ替えはコレクションメニューの『現在のコレクションを並べ替え…』」を出す。
- 詳細表示の列ヘッダソート: 評価一覧と同じく一時的な view override として許容し、
  `GridSortLockReason::DetailsHeaderSort` の既存表示に乗せる。Manual コレクションで列ヘッダを押した
  場合に「手動順」へ戻す導線 (Toolbar キーを選び直す) が評価一覧と同じになることを確認する。
- 追加で検討: 「登録順 (追加日時)」を Standard の選択肢に加える (`RatedAt` / `CreatedAt` の先例。
  entry の `created_at` は DB にある)。手動並べ替え後に登録順へ戻せないため、利用者価値が高い。
- 回帰: Collection root で (a) Manual → コンボが「手動順」 (b) Standard(DateDesc) → コンボが日付↓
  (c) コンボで名前順を選ぶ → actor に SetOrder(Standard, FileName) が届き `settings.sort_order` 不変
  (d) 物理フォルダへ戻ったときコンボが global に戻る、を handler-level test で固定。

## F. 競合比較: 初期実装として期待される機能・仕様

対象: NeeView プレイリスト、ZipPla 仮想フォルダ (.sor) / スマートフォルダ (.kdk)、Eagle フォルダ、
Lightroom コレクション、XnView MP カテゴリ、IrfanView スライドショー一覧 (.txt)、YACReader
リーディングリスト、音楽プレイヤーのプレイリスト (M3U)。Web で確認した範囲は NeeView
(ユーザーガイド: .nvpls、ブックとして開く、スライダーマーク、プレイリスト限定の「登録順」ソート)、
ZipPla (窓の杜レビュー: 仮想フォルダ .sor / スマートフォルダ .kdk、複数フォルダを 1 フォルダのように表示)。
それ以外は一般知識で、実機確認はしていない。

### F-1. 追加操作の入口 (最重要)

| 入口 | 競合 | mIV v4.0.0 (コード照合) | 判定 |
| --- | --- | --- | --- |
| ショートカットで選択中 / 表示中を追加先へ追加 | Lightroom (B)、NeeView (コマンド割当可)、本棚 (`KeyAction::AddToBook`) | `KeyAction` 無し。`MenuCommandId::CollectionsAddSelectionToTarget` はメニュー専用 | **P2**: 本棚と同型なのに keymap 非対応。CLAUDE.md「新しいキー操作は原則 KeyAction」 |
| フルスクリーン表示中の画像を追加 | NeeView (+)、Lightroom (loupe で B) | 要確認 (A 担当) | A の結果待ち |
| ドラッグ&ドロップで一覧 → コレクション | Eagle / Lightroom / digiKam / XnView (カテゴリ) | 無し (`collections.rs` の drop target は並べ替え画面内のみ) | **P3 (設計提案)**: 固定ショートカットボタンへの drop。mIV は D&D カーソルを既定 OFF にしている方針があるので v4.0.x 以降 |
| 「新しいコレクションを作って追加」1 手 | Lightroom「選択した写真で新規作成」 | 管理 window で作成 → 追加先に設定 → 追加 (3 手) | **P3**: 追加先コンボに「新規作成…」を置くか、管理 window の新規作成に「作成して追加先にする」 |

### F-2. 一覧・閲覧

| 項目 | 競合 | mIV v4.0.0 | 判定 |
| --- | --- | --- | --- |
| 件数表示 | Eagle / Lightroom はフォルダ名の横に件数 | 管理 window / コンボに件数表示は見当たらない (`collections.rs` grep) | **P3**: 管理 window の行に件数、追加先コンボの hover に件数 |
| 登録元 (場所) の表示 | Lightroom は Grid でフォルダ名を出せる | `DetailsColumnId::Place` が既存 (`src/settings.rs:934`)。Collection root で既定表示かは A 担当 | 既定 ON を提案 (別フォルダの同名ファイルを区別するため) |
| 見つからない項目 | Lightroom は「!」+ 再リンク | placeholder で「見つかりません」、再リンクは §21/22 で撤去、登録解除→再追加 | 仕様どおり。マニュアルへ明記 (E 担当) |
| 一覧内絞り込み (Ctrl+F) | 各社あり | 既存 in-memory 絞り込みが Collection root で効くか A 担当 | A の結果待ち |
| 場所メニュー / フォルダツリーからの入口 | NeeView はサイドパネル、ZipPla は左ペイン | 場所▼ に本棚はあるがコレクションは無い (`src/known_folders.rs:22`) | **P2**: 本棚と同格なら `LocationMenuEntry::Collection` を評価 ▸ と同じ submenu 形式で追加 |

### F-3. 並び順・再生

| 項目 | 競合 | mIV v4.0.0 | 判定 |
| --- | --- | --- | --- |
| 手動順 + 通常ソート | Lightroom (カスタム順)、Eagle (手動) | 実装済み。ツールバーとの不整合は G 参照 | **P1** (G) |
| 登録順 (追加日時) ソート | NeeView (プレイリスト限定の「登録順」) | 無し。Manual の初期順 = 登録順だが、並べ替え後は戻せない | **P2 提案**: `AddedAtAsc/Desc` を Standard に追加 (評価一覧の `RatedAt` と同型) |
| シャッフル / ランダム | NeeView (ランダム)、IrfanView / XnView (スライドショー random)、音楽プレイヤー全般 | **アプリ全体に無い** (`grep shuffle/random` 0 件) | **P2 提案**: 仕様の「BGM 用」「順序なし」を実装上 Standard へ写しただけでは BGM 用途の期待 (シャッフル再生) を満たさない。v4.0.0 に入れない場合は known-issues / マニュアルで「順序なし = 通常ソート」と明記し、シャッフルを v4.x バックログへ |
| 終端の停止 / ループ / 次のフォルダ | 各社あり | 既存設定を流用 (playback plan §5) | OK |
| 再生中の編集の反映 | ほぼ無し (mIV 独自) | latest-next reducer | 優位点。マニュアルで説明する価値あり |

### F-4. 入出力

| 項目 | 競合 | mIV v4.0.0 | 判定 |
| --- | --- | --- | --- |
| テキスト一覧 (1 行 1 パス) | IrfanView (.txt スライドショー一覧)、XnView | 実装済み (BOM / CRLF / 引用符 / 相対) | OK |
| M3U / M3U8 | 音楽プレイヤー全般、VLC | `#` をコメントにしない仕様のため `#EXTM3U` / `#EXTINF` 行が無効行になる | **P3 提案**: 拡張子 `.m3u/.m3u8` のときだけ `#` 行を読み飛ばし、export に `.m3u8` (UTF-8) を追加。BGM 用途と既存プレイヤーの往復に効く。仕様の「`#` はコメントにしない」は `.txt` に限定して維持 |
| 相手ソフトの形式 (.nvpls / .sor) | — | 非対応 | 見送りで妥当。移行ガイドに「テキストへ書き出して取り込む」手順を書く (E 担当) |
| 一覧テキストの D&D 取り込み | 一部 | 無し | P3 以下 |

### F-5. 管理

| 項目 | 競合 | mIV v4.0.0 | 判定 |
| --- | --- | --- | --- |
| 入れ子 / グループ | Lightroom (コレクションセット)、Eagle、YACReader | 対象外 (仕様) | 妥当。schema に親 ID を足す余地があるかは C 担当 |
| コレクション一覧の並べ替え | Eagle / Lightroom (名前順 or 手動) | `catalog_position` 列はあるが UI から並べ替え不可 (`collections.rs` に該当 UI 無し)。固定列だけ手動順 | P3: 数が増えると作成順固定は探しにくい。名前順表示または並べ替え |
| 複製 / 結合 | Lightroom | 無し | 見送りで妥当 |
| 参照解除の Undo | Lightroom (Ctrl+Z) | 既存 undo stack はレーティング / タグ用。参照解除は非対応 (要確認) | P3 |
| データのバックアップ | Lightroom カタログ | `collection.db` は設定復元 / エクスポートの対象外 (計画 §3.3)。テキスト export が唯一の可搬手段 | マニュアル / privacy に保存先を明記 (E 担当) |

### F-6. 命名・移行

- NeeView 利用者は「プレイリスト」、ZipPla 利用者は「仮想フォルダ」で探す。`htdocs/mimageviewer/migrate-zippla.html`
  / `migrate-leeyes.html` / `migrate-mangameeya.html` / `migrate-vix.html` に「仮想フォルダ / プレイリスト」の語は
  現在 0 件 (grep)。対応表へ「コレクション」を追記する (E 担当)。

### 参考 (Web)
- NeeView ユーザーガイド: https://neelabo.github.io/NeeView/ja-jp/userguide.html
- 窓の杜 ZipPla レビュー: https://forest.watch.impress.co.jp/docs/review/1060360.html
