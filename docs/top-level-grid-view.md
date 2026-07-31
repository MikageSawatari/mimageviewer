# 最上位一覧ビューの ownership とスマートフォルダ scope

## 1. 正本

検索、★固定、サブ展開、スマートフォルダ、読書履歴、ブックマーク一覧、レーティング一覧は同じ `App.items`
surface を共有する。現在の最上位 surface と一時ビューからの復元先は
`app/top_level_grid_view.rs` の `TopLevelGridView` / `TopLevelGridRestore` を正本とする。
個別の `active` / `items_are_*` flag と synthetic path は描画・既存経路との互換情報であり、
復元先の所有者にはしない。

`begin(surface, return_to)` は次の surface と唯一の復元 snapshot を同時に設定する。
検索・★固定・別スマートフォルダへ直接切り替える場合は、元一覧を途中で再構築せず、
`take_return_to()` で snapshot を次の遷移へ移譲する。戻る時は
`restore_view_return_context()` が実フォルダと synthetic view を同じ型から振り分ける。

ブックマーク一覧は `TopLevelGridSurface::Bookmarks` を所有者とし、動画・音声・本の各ブックマークを
`App.items` の 1 行へ materialize する。ブックマーク ID、登録位置、登録日時、欠落状態、保存済み動画
サムネイルは同じ index の sidecar row に保持する。通常の facet / rating / tag / details 表示を共有する一方、
Delete と右クリック削除は元ファイル操作へ流さず DB 行だけを削除する。
一覧から項目を開いた viewer context は元コンテナと `Bookmarks` 戻り先を保持し、Esc / 閉じる / 親移動で
実フォルダの親ではなく同じ一覧へ戻る。動画・音声の登録時刻への最終 seek pending もプレイヤーと同じ
context が所有し、detached / ParkedLive への移動後に main context へ取り残さない。戻り先 state は
開く直前の stable row ID 列、選択 ID、スクロール offset も所有する。非同期再構築後の ID 列が同一なら
offset をそのまま復元し、増減・並べ替えがあれば開いた ID を選択して ensure-visible する。

## 2. スマートフォルダ

`TopLevelGridSurface::SmartFolder(SmartFolderViewState)` は次を所有する。

- 定義 ID
- root snapshot に表示された実フォルダ entry の順序
- `Root`、または `Scoped { entry_index, entry_root, current, back_stack }`

同じ `TopLevelGridView` が `SmartFolderSession` も唯一所有する。session は完成済みの走査
snapshot、sort 用 metadata、root の materialize 済み grid を保持する。root から配下の
フォルダ / PDF / ZIP / 変換アーカイブを開くときは、一覧を clone せず session へ move し、
root へ戻ると同じ items・サムネイル・選択・スクロール位置を move で戻す。この復帰では
scan も prepare も開始せず、進捗 UI も出さない。

The scroll offset and the complete `AutoAspectState` that determined its row height belong to one
layout snapshot. The resolved aspect, index-keyed samples, cache gate, and switch history all
belong to the root items. Restore those samples together and rebind only `items_generation` to the
newly installed root generation; a child's auto-aspect state must never be paired with this offset.

**レイアウト変更時の復帰 (2026-07-31)**: session は offset の復元値とは別に、比較専用の
有効レイアウト (`Thumbnail { cols }` / `Details`) と window inner size を保持する。子を開いている
間に `settings.grid_cols`、`grid_view_mode`、または window size が変わっていなければ、offset と
`AutoAspectState` を従来どおりそのまま戻し、pixel 単位で同じ位置を復元する。変わっていれば、
ユーザーが変更した設定は戻さず、保存済み offset と選択を install した後で通常フォルダの
戻り先復元と同じ `scroll_to_selected` を立てる。`render_grid` は選択アイテムが新レイアウトでも
見えていれば offset を維持し、外れている場合だけ同じアイテムが見える最小位置へ補正する。
詳細表示中の列数変更は実レイアウトを変えないため不一致にせず、詳細行高は `DETAILS_ROW_H` 固定、
サムネイルの decode target はセル geometry の入力ではないため比較対象に含めない。

セッション内 open はグリッド / 親移動 / Ctrl+上下の request が対象 path を型付きで許可し、
共通の `load_folder_with_scan_claimed` / `start_loading_items_inner` 境界だけがその許可を消費する。
アドレスバー、お気に入り、通常フォルダ、検索、別の最上位 surface には許可がないため、
同じ境界で session が破棄される。`open_smart_folder()` は `begin` を通して同じ定義の session
も必ず破棄し、明示的な再選択を従来どおり full scan にする。履歴に残る
`SmartFolderViewState` は位置だけであり、破棄後の synthetic path 復帰は scan からやり直す。

detached / ParkedLive 用の context 複製は表示 identity を複製しても、main surface が所有する
`SmartFolderSession` は複製しない。巨大 result と worker/cache を sibling context に共有せず、
main の session drop / tombstone を一つの所有境界に保つ。

root は複数検索元を横断したフラット一覧のままにする。実フォルダ entry を開くと scoped
drill に入り、以後の通常フォルダ列挙にはスマートフォルダ条件を再適用しない。アドレス表示は
`スマートフォルダ名 > entry名 > 子フォルダ...` とする。

Backspace / 親移動は `entry_root` より外へ出ない。entry root の親は実ファイルシステムの親では
なく、保持済み root snapshot である。Ctrl+↑/↓ は entry 内だけを DFS し、端へ達した場合だけ
root snapshot の表示順で前後のフォルダ entry へ移る。グリッド、リング、ゲームパッド、画像
フルスクリーン、native 動画は同じ `FolderNavMode::SmartFolder` を使う。

通常の戻る / 進む履歴では scoped drill 内の実パスをそのまま保存せず、同じスマートフォルダの
synthetic path と、履歴 stack の同じ添字に置く `SmartFolderViewState` の組で位置を表す。root と
各 scoped current は state が異なる別の履歴地点として扱うため、root から実フォルダを開いた後に
「戻る」で root、「進む」で実フォルダへ復帰できる。検索、★固定、履歴、別スマートフォルダとの
往復でも `current` と親 stack を含む scope 全体を復元する。

## 3. 更新と失効

root 再準備では新しい実フォルダ表示順を state へ反映する。同じ entry path が残っていれば
並び替え後の index へ追従できる。削除・リネームで entry が消えた場合は stale な実パスを
scope として保持せず root へ戻す。定義削除、worker cancel、別最上位ビューへの遷移は既存の
smart generation / cancel 境界と `TopLevelGridView` の ownership を両方確認する。
未完成の cache-miss restore では、先に設定した SmartFolder target surface 自身を中止時の
復元元にしない。中止時は、背面に完成済みスマートフォルダ一覧が残っていればそれをそのまま
復元し、完成済み一覧も有効 snapshot もない自己参照 origin なら保存済み実フォルダへ戻る。
汎用 restore を再帰的に呼んで同じ scan を開始し直してはならない。

resident session は鮮度更新の単位でもある。rating / tag / adjustment 等の書き込みは表示中の
セル状態を直接更新できるが、membership・順序・再利用 metadata は明示 reopen まで凍結する。
したがって `smart_folder_metadata_refresh_due` は resident session 中には schedule / poll の
どちらでも prepare を開始しない。定義 rule の変更は session を失効させ full scan、grouping
変更は同じ snapshot と凍結 metadata から prepare をやり直す。削除だけは鮮度ではなく
開けないセルを防ぐ正しさなので、tombstone を snapshot に保持すると同時に、退避中の prepared
grid から対象と子孫を除去して index 状態を remap する。

グリッドの Shift+クリック起点は `GridClickSelectionAnchor { index, items_generation }` として
現在の item 配列世代に所属する。一覧全体を差し替える通常経路では失効させ、Ctrl+G の
streaming rebuild では同じ内容キーが残る場合だけ新 index / 新世代へ再マップする。一覧内削除は
アンカー対象が残る場合だけ old→new index へ追従し、対象自体が消えた場合は失効させる。
これにより、別一覧に同じ数値 index が存在しても前の一覧から範囲選択しない。

## 4. 回帰確認

- root → 実フォルダ → 子フォルダ → Backspace → root
- Ctrl+↑/↓ の entry 内 DFS と entry 間移動
- 検索 / ★固定 / 戻る・進む履歴 / 別スマートフォルダとの往復
- root 表示順変更、entry の削除・リネーム・更新
- グリッド、リング、ゲームパッド、画像 fullscreen、native 動画で同じ scope 判定
- 配下フォルダ / PDF / archive から root へ戻ると prepare progress なしで選択・scroll を復元
- 通常フォルダへ出た後の synthetic 履歴復帰と、同じ定義の明示 reopen は full scan
- 配下で削除した path は resident root に復帰しても再出現しない
- resident 中の metadata refresh timer は prepare せず、grouping / rule 変更だけ再構築
