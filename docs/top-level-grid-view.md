# 最上位一覧ビューの ownership とスマートフォルダ scope

## Everything 検索のsourceと物理child（§1.374、未リリース）

Everythingは新しい最上位surfaceを増やさず `TopLevelGridView::Search` を共有する。
検索開始前の唯一の `return_origin` は通常Searchと同じownerが保持し、query送信やsource変更で
復元先を別の一覧へすり替えない。draft `SearchSource` とaccepted `SearchAccepted` は別で、
未実行queryや失敗した新producerが表示済みdatasetの出所を上書きしない。
取得/prepare中はmainだけのmodalとし、完成payloadだけを既存adoption closureで一回公開する。

Everythingのaccepted位置は `Root` / `PhysicalChild { logical_source, return_anchor, route }`。
Rootからの物理ZIP/PDF/EPUB/変換書庫openは通常経路へ委譲し、実際に物理表示が採用された境界でだけ
childへ進む。内部ZIP階層は既存zip navを使い、外側の帰路は保持datasetを共通prepareで再投影する。
その復帰でIPC、元queryの再実行、新しいorigin captureをしない。検索終了は既存close/restoreを通す。

sort/★/tag/編集準備の入力はimmutable datasetと送信時stampであり、表示中itemsを第二の正本にしない。
mainの準備/採用は同contextのthumbnail/PDF ownerだけを更新する。detachedにはSearchのdatasetや
return_originを複製せず、既存fork/park/promoteの物理viewer ownerを使う。
RemoteへSearchSource/query/result surfaceを追加しない。自動gate・独立レビュー・実機確認は未完了。
[正本・検証台帳](everything-search-plan.md#14-実装記録2026-10-12)。

## 1. 正本

検索、★固定、サブ展開、スマートフォルダ、閲覧履歴、ブックマーク一覧、レーティング一覧は同じ `App.items`
surface を共有する。現在の最上位 surface と一時ビューからの復元先は
`app/top_level_grid_view.rs` の `TopLevelGridView` / `TopLevelGridRestore` を正本とする。
個別の `active` / `items_are_*` flag と synthetic path は描画・既存経路との互換情報であり、
復元先の所有者にはしない。

`begin(surface, return_to)` は次の surface と唯一の復元 snapshot を同時に設定する。
検索・★固定・別スマートフォルダへ直接切り替える場合は、元一覧を途中で再構築せず、
`take_return_to()` で snapshot を次の遷移へ移譲する。戻る時は
`restore_view_return_context()` が実フォルダと synthetic view を同じ型から振り分ける。

検索由来の ★固定を別 viewer context へ fork する場合、`TopLevelGridView` / snapshot と、
Ctrl+S / Ctrl+G の synthetic subfolder restore payload は同じ `ViewerContextBundle` に複製する。
rating filter の一時解除 anchor も context 所有であり、一方の snapshot 解除が sibling の
anchor や fallback を `take()` してはならない。canonical `return_to` が存在する遷移では、
未使用の legacy fallback slot は consume しない。

検索結果の「フォルダに移動」は通常の検索 close と異なり、canonical `return_to` を consume して
復元せず、typed request が移動先と exact target を一緒に所有する。物理 directory は
`FolderOpenScanPurpose::JumpToPhysicalFolder` の worker scan と同じ寿命で target を保持し、成功後に
実パスで選択する。検索集約の ZIP は directory scan へ渡さず archive open 経路へ送る。request が
勝った時点で検索行を空の Folder surface へ差し替えるため、scan failure、別 navigation による cancel、
archive conversion の cancel 後に、終了済み検索の行が Folder surface として残らない。

ブックマーク一覧は `TopLevelGridSurface::Bookmarks` を所有者とし、動画・音声・本の各ブックマークを
`App.items` の 1 行へ materialize する。ブックマーク ID、登録位置、登録日時、欠落状態、保存済み動画
サムネイルは同じ index の sidecar row に保持する。通常の facet / rating / tag / details 表示を共有する一方、
Delete と右クリック削除は元ファイル操作へ流さず DB 行だけを削除する。
一覧から項目を開いた viewer context は元コンテナと `Bookmarks` 戻り先を保持し、Esc / 閉じる / 親移動で
実フォルダの親ではなく同じ一覧へ戻る。動画・音声の登録時刻への最終 seek pending もプレイヤーと同じ
context が所有し、detached / ParkedLive への移動後に main context へ取り残さない。戻り先 state は
開く直前の stable row ID 列、選択 ID、スクロール offset も所有する。非同期再構築後の ID 列が同一なら
offset をそのまま復元し、増減・並べ替えがあれば開いた ID を選択して ensure-visible する。

### セッション内のグリッド位置（§1.328）

通常フォルダの位置は既存の `folder_history` が持つ。レーティング一覧（★段ごと）、閲覧履歴、
サブ展開の位置は viewer context の `TopLevelGridView` が `SessionGridPosition` として所有する。
選択の安定キー・scroll offset・確定 Auto 比率の seed を対にし、古い index-keyed samples は持ち越さない。
サブ展開は正規化した root / roots が一致する位置だけを復元する。位置を restore snapshot や
`SubfolderExpansionRestoreState` に複製せず、context の Clone / fork には継承しない。
「場所を忘れる」は `clear_session_grid_positions` でこれらの位置を消す。永続データは変更しない。

保存境界は二つに分ける。検索入口、★一覧を閉じる入口、非同期 Collection 入場の入口では
`note_session_grid_positions` で暫定保存し、元の行を操作できる間は離れたと扱わない。
行を置き換える採用境界（main adoption、一覧 install、ZIP の即応空表示、検索結果採用、
Collection 採用、Drive 入場、★固定作成直前）で `save_departing_session_grid_positions` を呼ぶ。
`departed_items_generation` により同じ行世代では最初の一度だけ保存し、宛先 favorite 適用後の
再保存や ZIP 列挙待ちの空表示で元の seed / 選択を上書きしない。新しい行の install で世代が進む。
`install_loading_items_inner` は root / flags の移譲・消去、宛先の表示設定採用より前に保存する。

保存対象の行は既存 `items_are_*` で判定するが、Snapshot 由来の `projection_recipe` は常に除外する。
固定解除時は surface が先に Folder に戻っても Snapshot の行が残り得るため、surface だけで判定しない。
空の中間表示と detached physical context も保存しない。detached の述語・viewport 経路は変更しない。

閲覧履歴は `ReadingHistoryEntrance::Menu / Return` を呼び出しの目的から運ぶ。
`AddressBarNav::ReadingHistory(entrance)` も生成元で目的を確定し、main-frame / immediate dispatcher は
同じ `enter_reading_history_with_entrance` へ渡す。BS・親ボタン・Pad B・Esc 直帰は Return、
場所メニュー・明示コマンドは Menu。入力 lane からメニュー入場と推測しない（§1.328 の BS 退行）。
検索中の history 操作の書換えより前に区別し、Menu は最後の保存後に位置を捨てて先頭に入る。Return は shell の hydration
continuation に seed を渡し、実 rows の hydrate 後に可視な安定キーの行を選び offset と ensure-visible を戻す。
履歴への入場時に Ctrl+F を閉じて取り消す既存仕様は維持する。shell 待ち中の検索再適用は変更しない。
ブックマーク一覧の専用入口・位置 owner は変更しない。

サブ展開は `SubfolderInstallPurpose::New / Return` を scan → prepare → install の adoption が運ぶ。
履歴 replay と一時ビューからの帰路だけ Return とし、新規展開・並び替え reinstall は先頭に入る。
prepare は synthetic 宛先の favorite 表示設定を読み取り投影し、その sort / カテゴリ順で行を作る。
本棚直下の番号順例外と既存の評価読取失敗 fallback は維持する。install では閲覧履歴・Rating と同様に
宛先の列数・表示モード・比率を初回 layout 前に採用する。Return の位置と seed は同じ install で戻す。
Ctrl+F 表示中は既存仕様どおり ←/→ の履歴移動が無効なので、検索カーソル方針と R9 は設計担当の判断で撤去した。
他の入口から検索バーを開いたまま戻る場合も、自動再適用による先頭復帰を従来どおり受け入れ、検索処理は変更しない。

簡素化として位置 owner と採用境界を共通化し、戻り予約・rollback・追加の待機や再 ensure loop は作らない。
準備中は既存 modal なので並び設定の stamp / 再比較を足さない。shell 中の検索割込み、保存後の稀な失敗、
Remote からの準備中設定変更は既定の割り切り方針による。保存キーが消失・非可視なら先頭へ戻し、近傍探索しない。

## 2. スマートフォルダ

`TopLevelGridSurface::SmartFolder(SmartFolderViewState)` は次を所有する。

- 定義 ID
- root snapshot の最終表示順から作る、Folder/PDF/ZIP/変換書庫の単一ナビゲーション列（単体画像・動画は除外）
- `Root`、`Scoped { entry_index, entry_root, current, current_kind, back_stack }`、または root 行の PDF/ZIP 用 `Container { root_entry, current }`

同じ `TopLevelGridView` が `SmartFolderSession` も唯一所有する。session は完成済みの走査
snapshot、sort 用 metadata、root の materialize 済み grid を保持する。採用済み session は
`Root` / `Child` の排他 phase、新しい open は別の `SmartFolderTransition` が所有する。未採用の
open は現在表示中の一覧・scope・worker と検索/Collection の owner を保持する。root から配下の
フォルダ / PDF / ZIP / 変換アーカイブの可視結果を採用するときだけ、一覧を clone せず session へ move し、
root へ戻ると同じ items・サムネイル・選択・スクロール位置を move で戻す。worker が実際に
構築した sort/display/grouping stamp と現在の表示設定が同じなら scan/prepare は開始しない。
設定を明示変更した場合だけ保存 snapshot から再準備し、元 root 行の path を選択 anchor とする。

root 復帰時の選択は、退避時の選択を無条件に戻すのではなく、session の採用済み
`Child.logical_path` を包含する root-level entry で置き換える。子孫へ深く降りている場合も、
通常フォルダの親復帰が「戻り先直下の子」を選ぶのと同様に、その子孫を所有する最深の
root entry を選ぶ。選択の適用は通常の parent navigation と同じ select-after-load /
`scroll_to_selected` / ensure-visible 経路を使う。entry が削除されて root items から消えた
場合は退避済み選択を維持し、その選択も非表示なら既存の可視選択補正（直前優先、なければ
直後）へ委ねる。entry 自体は残るがローカル絞り込みで非表示の場合も同じ可視選択補正を使う。
退避済み選択が既に同じ可視 entry なら不要な scroll request は立てず、同一レイアウトの
pixel offset をそのまま保つ。

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

セッション内 open はグリッド / 親移動 / Ctrl+上下と同一 smart scope 内の直接 folder load が
対象 path を型付きで stage し、Folder scan 成功後または PDF/ZIP の可視 install 境界だけが
その要求を採用する。scan Err、変換取消、cache-cold PDF password cancel は元表示を維持する。
scope 外への独立ナビゲーションは未採用 transition を退役させる。`open_smart_folder()` の明示的な
再選択も full scan を新要求に stage し、成功採用時だけ旧 session を破棄する。履歴に残る
`SmartFolderViewState` は位置と Scoped の採用済み物理 kind だけを持ち、session 破棄後の復帰は
root を offscreen で scan→prepare してから子を開く。元表示と履歴は子の採用まで保持する。

detached / ParkedLive 用の context 複製は表示 identity を複製しても、main surface が所有する
`SmartFolderSession` は複製しない。巨大 result と worker/cache を sibling context に共有せず、
main の session drop / tombstone を一つの所有境界に保つ。

root は複数検索元を横断したフラット一覧のままにする。実フォルダ entry を開くと scoped
drill に入り、以後の通常フォルダ列挙にはスマートフォルダ条件を再適用しない。アドレス表示は
`スマートフォルダ名 > entry名 > 子フォルダ...` とする。

Backspace / 親移動は `entry_root` より外へ出ない。entry root の親は実ファイルシステムの親では
なく、保持済み root snapshot である。Ctrl+↑/↓ は Folder entry 内の DFS を維持し、端へ達した場合は
root snapshot の表示順で前後の Folder/PDF/ZIP/変換書庫 entry へ移る。root 直下の本からも同じ列で
前後へ移動する。Grid は直下画像のない root Folder もその entry 自体へ入り、Fullscreen は
同 Folder の配下を前進時は先頭、後退時は末尾から再生可能な子を探す。空の scope を尽くしたら
次の root entry を調べ、skip 上限に達した場合のみ従来の空 Folder fallback を使う。
グリッド、リング、ゲームパッド、画像
フルスクリーン、native 動画は同じ `FolderNavMode::SmartFolder` を使う。

通常の戻る / 進む履歴では scoped drill や root container 内の実パスをそのまま保存せず、
`SmartFolderViewState` の位置を型付きで保持する。root と各 Scoped/Container current は別の
履歴地点なので「戻る」で root、「進む」で元の子へ復帰する。Container は root 行の本を一つの
entry として保持し、Ctrl+上下では隣の表示対象 entry へ移る。session 不在の
履歴復元は scan→prepare で root を offscreen 再構築した後に子を開く継続要求と pop 前の履歴 peek を
同じ owner が保持し、取消・未採用失敗時には元表示・履歴を変更しない。検索、★固定、別スマート
フォルダとの往復でも scope 全体を復元する。

### コレクションrootと物理子のowner

`TopLevelGridSurface::Collection` のrootはactor snapshot、accepted / wanted revision、items generationを
`CollectionGridSession`で所有する。root cellからFolder / ZIP / PDFを開くrequestは、root entry identity、
source path、surface stamp、accepted / wanted revision、items generationを一つの
`CollectionGridPhysicalLoadOwner::Root`へ捕捉する。非同期scanの完了時にもこのownerを再検証し、途中で
wanted revisionが進んだ旧root requestは採用しない。

物理子へ採用した後は同じ型の`PhysicalSource` variantがroot entry anchor、root source、現在の物理pathを
所有する。配下container openと同一pathのsort / 外部更新reloadはこのownerを運び、collection watchの
wanted revision進行や定義削除だけでは現在の子表示を置き換えない。rootへ戻るときにlatest revisionへ
収束する。アドレス入力、履歴、お気に入り、folder pane等の独立navigationはownerを持たず、scan成功や
ZIP / PDF非同期開始などvisible loadの採用境界でだけsurfaceを`Folder`へ切り替えてsessionを破棄する。
scan失敗、scope拒否、stale async result、変換dialog取消ではcollection rootのsurfaceとitemsを分裂させない。

collection root installはviewer bundle所有の通常thumbnail poolを新items generationへ作り直し、画像・Folder・
ZIP・PDFの既存producerを起動する。collection専用video workerだけをsessionがcancelし、bundleの共有pool tokenを
session dropで止めない。複数の物理folderを横断するrootではcache keyと同名画像sidecarを正規化full pathで扱い、
pin / sidecar / Shellの動画サムネイル優先順位を通常一覧と揃える。watch再表示とlatest再生root着地は同じ
prepare済みthumbnail source snapshotをinstallする。

## 3. 更新と失効

folder Back/Forwardは`PathBuf`と並行metadataを別々に持たず、`FolderNavHistoryTarget`の
Path / Rating / SmartFolder / Collection variantをnormalとA/Bの各stackで共有する。Collectionはstable ID、
revision hint、viewport anchorを一entryで保持し、synthetic filesystem pathへ変換しない。明示Openだけが
history transitionを作り、Add、watch refresh、rootからの正規child閲覧は作らない。authoritativeなReady catalogで
削除済みIDが判明した時だけ全stackから除き、rollback snapshotを戻す時にも同じcatalogを適用する。

明示的な一覧更新は `reload_top_level_grid` を唯一の router とし、
`TopLevelGridSurface` の網羅 match から通常フォルダ、各検索、スマートフォルダ、サブ展開、
レーティング、履歴、ブックマーク、ドライブ一覧の既存再入場経路へ振り分ける。★固定は凍結を
維持して no-op とする。スマートフォルダ / サブ展開の走査中は二重起動せず、通常フォルダの
選択と Ctrl+F フィルタ、表示中のフォルダツリーペインは更新後に復元 / 再適用する。

root 再準備では新しい実フォルダ表示順を state へ反映する。同じ entry path が残っていれば
並び替え後の index へ追従できる。削除・リネームで entry が消えた場合は stale な実パスを
scope として保持せず root へ戻す。定義削除、worker cancel、別最上位ビューへの遷移は既存の
smart generation / cancel 境界と `TopLevelGridView` の ownership を両方確認する。
cache-miss の履歴復帰は、採用前に target SmartFolder surface を公開しない。旧 visible owner を
保ったまま root/child を準備し、成功時に一回だけ表示と履歴を切り替える。取消時の同期 reload や
汎用 restore の再帰呼出しは必要ない。

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
