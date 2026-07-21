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
context が所有し、detached / ParkedLive への移動後に main context へ取り残さない。

## 2. スマートフォルダ

`TopLevelGridSurface::SmartFolder(SmartFolderViewState)` は次を所有する。

- 定義 ID
- root snapshot に表示された実フォルダ entry の順序
- `Root`、または `Scoped { entry_index, entry_root, current, back_stack }`

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

## 4. 回帰確認

- root → 実フォルダ → 子フォルダ → Backspace → root
- Ctrl+↑/↓ の entry 内 DFS と entry 間移動
- 検索 / ★固定 / 戻る・進む履歴 / 別スマートフォルダとの往復
- root 表示順変更、entry の削除・リネーム・更新
- グリッド、リング、ゲームパッド、画像 fullscreen、native 動画で同じ scope 判定
