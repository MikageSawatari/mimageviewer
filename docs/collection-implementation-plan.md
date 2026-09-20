# コレクション機能 実装計画（§1.118）

## 1. 目的と正本

本書は [`collection-spec-proposal.md`](collection-spec-proposal.md) の合意済み仕様を、
現在のコード所有へ接続する実装計画である。仕様を狭めず、複数の名前付きコレクション、
手動順と通常ソート、PC の編集 UI、登録元を開く表示、ナビゲーション・スライドショー・
再生との統合、Remote の一覧・閲覧、テキスト import/export を一つの型付き所有モデルで実現する。

段階はレビューと回帰を小さくするための内部開発順であり、途中段階を完成機能として公開しない。
初段の永続型・DB・worker・snapshot・import parser は、後段で捨てたり別モデルへ置き換えたりせず、
PC、複数 viewer context、Remote が共通利用する正本とする。

## 2. 現コードの前提照合

### 2.1 表示面と項目型

- `src/app/top_level_grid_view.rs` の `TopLevelGridSurface` は Folder / DriveList /
  Search / Snapshot / SubfolderExpansion / SmartFolder / ReadingHistory / Bookmarks /
  Rating を明示的に所有する。コレクションも独立した `Collection` surface として加え、
  Folder や既存 Remote の aggregate collection へ擬装しない。
- `src/grid_item.rs` の `GridItem::drag_source_path()` は Folder / Image / Video /
  Audio / ZipFile / PdfFile / ConvertibleArchive の実体 path だけを返し、ZipImage /
  ZipDir / PdfPage / Stack / SearchContainer を除外する。登録可否はこの物理項目境界と
  typed kind を組み合わせ、path の有無だけから仮想項目を再推定しない。
- 通常一覧は 4 行カテゴリへ materialize する。通常ソート時は既存カテゴリ割当と
  `SortOrder` comparator を再利用する。手動順ではカテゴリ再配置を行わず、保存した
  entry の順序をそのまま一列の有効順として描画する。
- Folder / ZIP / PDF / convertible archive を開いた後の内部項目順は、既存の Folder、
  archive、book viewer の所有を維持する。collection entry の手動順や通常ソートを
  book 内部へ伝搬しない。

### 2.2 path identity と metadata

- `src/path_key.rs::normalize_keep_drive` は drive 保持、separator、ASCII case の既存規則を提供するが、
  `.` / `..`、trailing separator、verbatim/device prefix、relative base を単独では扱わない。
  新しい純粋な `CollectionSourcePathKey` builder が Windows component を lexical に解決し、
  許可した absolute drive / UNC path だけを同じ規則へ正規化する。direct registration、import、
  UNIQUE、rename、latest fallback は必ずこの一つの builder を使う。UI thread の
  `canonicalize` / `stat`、表示文字列を key に使わない。
- stable source kind は現在の全 entry に共通する `FileSystemPath` namespace とする。
  Image / Video / Audio / Folder / Zip / Pdf / ConvertibleArchive / Unresolved は、DB に保持する
  last-known kind および prepare 後の resolved kind であり、stable key の一部にしない。
  正本の「source kind を key に含む」は namespace を typed key に含める意味で満たす。
- direct registration は `GridItem` が既に持つ resolved kind を渡す。text import で存在しない
  path は `Unresolved` として保持し、missing entry を捨てない。source が後で見つかった場合の
  kind 更新または relink は entry ID を維持する明示 mutation とする。
- 同一コレクション内の「同じ参照を一度だけ」は
  `(FileSystemPath, normalized_path)` の unique 制約で守る。resolved kind が時間差で変わっても、
  missing → present、削除後の同 path 再登録、latest fallback の identity が分裂しない。

### 2.3 既存 DB / worker / Remote

- `rating_db.rs` は専用 SQLite と migration、`reading_history_db.rs` は App 所有の
  sender / `JoinHandle` を持つ writer の参考になる。ただしコレクションは read、write、
  revision snapshot、Remote を一つの直列 owner で扱うため、単一 actor が writable
  connection を所有する。
- `src/remote_ipc/collections.rs::CollectionEngine` と `crates/remote-ipc` の
  `CollectionKind` は、DriveList / ReadingHistory / Rating / Bookshelf / Bookmarks /
  SmartFolder をまとめる既存 aggregate read model である。永続コレクションをこの
  storage とみなさず、protocol には stable `collection_id` を持つ別の typed variant /
  payload を追加する。
- `rename_key_migration.rs` はファイル操作成功後に rating、history 等の key を移す。
  コレクションも同じ成功境界から actor command を受けるが、worker 所有中の DB を別接続で
  書き換えない。ファイル操作失敗時は参照を先に動かさず、DB migration 失敗時は旧参照と
  missing 表示を保って再試行可能な error とする。

### 2.4 Toolbar と Settings

- `ToolbarSectionId` と `ordered_with_fallback` に `Collections` を追加し、新規 profile の
  default order では Bookshelf に隣接させる。既存の利用者順は維持し、新 section は既存
  fallback 規則で補完する。
- Settings が持つのは toolbar の表示・折り畳みなど UI preference だけである。
  collection 定義・entry・revision を Settings JSON / settings.db に複製しない。
- FavoriteSortOrder、通常一覧 SortOrder、folder tree sort、代表サムネイル sort は独立のまま。
  collection の標準ソートは通常一覧の `SortOrder` 値を明示保存するが、他設定を更新しない。

## 3. 永続 model と DB family

### 3.1 型

現行の型は次を正本とする（名称は Rust 実装に合わせる）。

```text
CollectionId(UUID)
CollectionEntryId(UUID)
CollectionSourceNamespace = FileSystemPath
CollectionSourcePathKey { namespace, normalized_path }
CollectionResolvedKind = Image | Video | Audio | Folder | Zip | Pdf |
                         ConvertibleArchive | Unresolved
CollectionOrderMode = Manual | Standard | Shuffle
CollectionDefinition { id, name, order_mode, standard_sort, shuffle_seed, revision, ... }
CollectionEntry { id, collection_id, source_path, source_key, manual_position, ... }
CollectionCatalogSnapshot { catalog_revision, definitions: Arc<[...] > }
CollectionSnapshot { collection_id, revision, definition, entries: Arc<[...] > }
CollectionSortFacts { entry_id, name_key, mtime, size, resolved_category }
```

- ID は表示名、path、配列 index、SQLite `rowid` から作らない。作成時に UUID を一度発行し、
  rename / reorder / relink / 再起動後も維持する。
- `source_path` は利用者へ表示・open する path、`CollectionSourcePathKey` は比較専用である。
  last-known resolved kind も別列に永続化し、unknown を size 0 や空文字などの sentinel で表さない。
- `CollectionSortFacts` は prepare worker が既存 metadata/classifier から作る aligned な一時値で、
  DB snapshot や UI が filesystem 値を再取得しない。標準 sort helper は snapshot とこの facts を
  明示入力に取り、DB entry の path だけから date/size/category を推測しない。
- snapshot は immutable `Arc` とし、DB actor が transaction commit 後だけ新 revision を公開する。
  no-op、duplicate 拒否、rollback では revision を進めない。

### 3.2 schema と transaction

専用 `collection.db` を data directory に置く。概念 schema は次のとおりである。

```text
collection_meta(schema_version, catalog_revision)
collections(id PK, name, order_mode, sort_order, shuffle_seed TEXT, revision, created_at, updated_at)
collection_entries(
  id PK, collection_id FK, source_namespace, source_path, normalized_path, resolved_kind,
  manual_position, created_at,
  UNIQUE(collection_id, source_namespace, normalized_path),
  UNIQUE(collection_id, manual_position)
)
```

- create / rename / delete、batch add、remove、manual reorder、order mode 変更、relink は
  一 collection 単位の SQLite transaction とする。
- source migration は同じ file/folder を登録する**全 collection**を一 command / 一 transaction で
  更新する。affected collection の各 revision と catalog revision を同じ commit で進め、process crash、
  conflict、constraint error で一部 collection だけ新 path にならない。
- mutation ごとに対象 collection revision と catalog revision を commit 内で進める。
  crash 後に定義と entry、順序と revision が半端に見えない。
- manual position は常に保存し、Standard / Shuffle へ切り替えても書き換えない。両モード中の追加も
  manual tail へ追加するため、Manual へ戻したとき元の順序と追加 tail が復元する。
- migration は schema version で直列化し、未知の新 schema は read/write せず typed
  incompatible とする。DB corruption や open failure を空コレクションへ黙って置換しない。

### 3.3 settings family との関係

`collection.db` は `settings.db` family へ含めない。コレクションは rating、bookmark、
reading history と同じ利用者データであり、Settings の「復元」「完全リセット」で定義や entry を
巻き戻したり削除したりしない。従って §234 settings-family lease、settings backup rotation、
settings reset の close/drain 対象にも追加しない。

collection DB は自身の単一 actor、transaction、WAL lifecycle を持つ。text export は利用者が実行した
時点の参照一覧を移す手動 portability であり、自動 backup や未export変更の保護とは記述しない。
将来「全ユーザーデータ削除」へ統合する場合は、collection actor を
先に drain / close して DB family 一式を扱う別 operation とし、settings reset に便乗しない。

## 4. actor と終了所有

### 4.1 単一 owner

`CollectionStoreRuntime` は App process が一つだけ所有し、actor thread は writable SQLite
connection と in-memory immutable snapshot cache を所有する。UI、各 viewer context、Remote は
clone 可能な `CollectionStoreClient` だけを受け取る。

- UI の request は bounded nonblocking enqueue と one-shot response で、満杯時は typed Busy。
  UI thread は DB response を待たず、frame ごとに private receiver を poll する。
- actor は短い DB read / transaction と snapshot publish だけを行う。filesystem existence、
  archive classification、thumbnail、export file write を actor 上で行わない。
- startup は thread spawn 後すぐ UI へ戻り、Ready / Failed を非同期通知する。open failure 中も
  他の viewer 機能を止めず、collection surface だけに recovery error を表示する。
- client sender の clone 数を actor lifetime に使わない。runtime と全 client clone は shared typed
  admission (`Starting | Running | Closing | Closed`) を参照し、Closing 以後の enqueue を即座に
  Unavailable とする。App owner は独立 Shutdown を送り、actor が current short transaction を終えて
  connection / WAL を drop した ACK を受けてから `JoinHandle` を回収する。
- revision 通知は client clone 間で一つの receiver を奪い合わない。`subscribe()` ごとに private
  latest-watch mailbox と wake receiver を発行し、actor は全 subscriber へ同じ最新 revision notice を
  fan-out する。coalescing 中も newest notice を slot に上書きし、遅い context は中間通知を省略して
  最新 catalog/collection revision へ収束する。drop 済み subscriber は weak owner から掃除する。

### 4.2 起動・終了順

後段統合時の順序を次に固定する。

1. App が collection actor を開始し、client を UI に保持する。
2. Phase 5 の Remote serverはraw clientではなく、同じclientを包むshared typed producer controlを
   注入される。RemoteがDB connectionを別所有しない。
3. 通常終了はRemote collection producer admissionを閉じ、in-flight短期requestのdrain ACKを得る。
4. App が actor の全request admissionを閉じてShutdownを送り、ACK後にjoinする。
5. actor connectionがdropした後、Appより長生きするRemote guardを含むprocess終了へ進む。

通常 frame、dialog close、viewer closeでは join しない。App final exit の drain だけが待機可能な境界である。
Remote startup partial failure は注入 client を破棄するだけで actor owner を失わず、App が通常どおり
shutdown する。actor panic / disconnect は collection unavailable とし、古い snapshot を編集可能として
公開し続けない。

## 5. request、revision、複数 viewer context

DB actor の command は stable ID と期待 revision を持つ。

```text
ListCatalog
LoadCollection { collection_id }
Create / Rename / Delete
AddBatch / Remove / ReorderManual / SetOrderMode / Relink
MigrateSources
Shutdown
```

- mutation は必要に応じ `expected_revision` を検査し、競合を Last Writer Wins にせず
  typed Conflict と最新 snapshot を返す。UI は最新を再表示して利用者の操作を再適用できる。
- App の pending owner は DB request ID に加え `ViewerContextId`、top-level surface generation、
  `CollectionId`、requested collection revision を一つの stamp として保持する。
- result は同じ context が同じ Collection surface / collection ID / generation を所有するときだけ
  apply する。別 window、同 window の folder switch、surface close、collection delete、再要求後の
  stale result は捨てる。DB actor は viewer context を global state として持たず、request stamp を
  opaque に往復させる。
- collection の編集 commit は各 context の private latest-watch へ catalog/collection revision event を
  fan-out する。
  各 context は自身の表示準備を非同期で再要求し、別 context の texture、cursor、open/close owner を
  reset しない。

## 6. materialize、表示、編集

### 6.1 prepared snapshot

DB snapshot と表示用 `GridItem` は分離する。prepare worker は snapshot の entry 全件を現在の
filesystem / archive classifier と既存 metadata cache で解決し、次を返す。

```text
CollectionPreparedSnapshot {
  collection_id,
  collection_revision,
  effective_order,
  entries: Arc<[PreparedCollectionEntry]>,
  context_stamp
}
```

`PreparedCollectionEntry` は `entry_id` と `source_key` を常に保ち、missing / unsupported /
resolved GridItem を typed に持つ。UI は表示時に `stat` / canonicalize / folder scan をしない。
missing は一覧から消さず、欠損表示と remove / relink action を出す。

- Manual は DB の manual position 順をそのまま effective order とする。
- Standard は既存の通常一覧 4 カテゴリ割当と選択した `SortOrder` を worker 内で適用する。
  source metadata unknown の比較規則も通常一覧の typed comparator を使う。
- collapsed Stack や archive page を新しい collection entry として合成しない。登録 entry と
  描画 cell の対応は `entry_id` で保持する。

### 6.2 UI と toolbar

- toolbar に専用 Collections section を追加し、collection 選択、作成、rename、delete、
  order mode、import/export へ到達させる。
- list は既存 thumbnail/details painter、選択、hover、context menu を再利用するが、surface は
  `TopLevelGridSurface::Collection` とする。toolbar 自体へ DB 内容を同期 copy しない。
- collection entry の remove と物理 source delete は別 command / confirmation とする。
  remove は DB 参照だけ、source delete は既存 filesystem operation 成功後に collection migration /
  removal を行う。
- collection 編集中も現在再生を中断しない。編集結果は revision event として次の navigation
  decision にだけ使う。

## 7. navigation、book、再生

### 7.1 parent / child

Folder / archive / book entry を開くと既存 child surface / viewer を使い、戻り先として
`{ collection_id, collection_revision_at_open, entry_id, source_key }` を typed に保持する。
戻るときは最新 snapshot で entry ID、次に source key を引き直し、同じ collection の root 選択へ戻す。
book 内部のページ順、archive 列挙、通常の前後ページ操作は既存 owner のままである。

### 7.2 Ctrl+上下と slideshow NextFolder

Collection surface / child origin が有効なときだけ、外側の folder/book traversal を最新
prepared snapshot の effective order から作る。登録された Folder / Zip / Pdf /
ConvertibleArchive の root entry を順に使い、Folder の子を outer sequence へ再帰追加しない。
collection 外、通常 Folder、Search、SmartFolder の sibling navigation は既存経路のまま。

### 7.3 再生中編集

次項目を決める直前に最新 revision snapshot と、同じ entry ID 集合へ prepare worker が付けた現在の
resolved kind / availability を非同期で取得し、次の純 reducer を使う。DB の last-known kind は候補判定へ
使わず、effective order と prepared facts の ID 集合が snapshot と一致しなければ stale として拒否する。

1. 現在の `CollectionEntryId` が残っていればその index。
2. 無ければ同じ `CollectionSourcePathKey` の entry。
3. それも無ければ現在 effective order の先頭。
4. 現在項目が残り末尾なら既存 stop / loop 設定に従う。

別 collection の revision event は current session を動かさない。next request の応答前にさらに revision が
進んだ場合は stale target を開かず、最新 snapshot を再要求する。target media kind の選択、動画・音声・
画像の既存 playback owner は変えない。

## 8. import / export

### 8.1 純 parser と preview

`parse_collection_text(text, source_text_path)` は文字列と base path だけを受ける純関数とする。
この段階で filesystem、shortcut、archive、thumbnail、DB、network へアクセスしない。

- UTF-8 BOM 有無、CRLF / LF、一行一 path、blank 無視、whole-line quote を扱う。
- `#` は comment にせず通常文字として扱う。
- relative path は text file の parent に lexical join し、`CollectionSourcePathKey` builder で
  `.` / `..` と trailing separator を整理する。
- URL、verbatim/device path、wildcard、環境変数・command expansion を reject し、入力を実行しない。
  absolute drive / UNC の判定も同じ builder が行う。
- typed source key で最初の行だけを採用し、後続 duplicate を line result として表示する。
- preview は parsed / invalid / duplicate の line result だけを表示する。exists、kind、missing、
  link 解決、thumbnail を preview の事実として表示しない。

### 8.2 confirm 後の登録

Confirm 後に cancel token 付き prepare worker が exists / supported physical kind / network access error を
分類する。既存 path は exact kind、存在しない path は `Unresolved` として missing entry を作る。
worker は進捗を UI へ送り、cancel 済みの未登録 batch を actor へ送らない。

actor の `AddBatch` transaction が同 collection の typed source-key uniqueness と revision を最終決定する。
preview 後に別入口で追加された duplicate は actor result で表示し、上書きしない。network error と
invalid は区別し、読み取り失敗を「存在しない」と偽らない。

### 8.3 export

export は開始時の immutable latest collection snapshot を使い、現在の full effective order を一度固定する。
検索・selection・viewport の local filter を適用せず、missing / unresolved も含める。pure serializer が
quoted path を生成し、別 worker が選択先へ書く。UI thread で file write しない。export 中の編集は開始時
snapshot の出力を変えず、完了時に使用 revision を表示できる。

## 9. Remote 境界

- protocol に persistent collection catalog / snapshot 用の別 typed request を追加し、
  `CollectionId`、collection revision、entry ID、source key、missing state を運ぶ。
- 初回 Remote は list / view / open の read-only。create / rename / reorder / import / remove を
  command として公開しない。これは PC 編集機能を制限するものではなく、合意済み Remote 初回範囲である。
- Remote request は UI を経由せず同じ actor client / immutable snapshot を読む。actor Busy、Conflict、
  Incompatible、Unavailable を既存 protocol の generic failure に潰さない。
- Remote viewer session も collection ID / entry ID / source key / revision を持ち、PC と同じ latest-next
  reducer を使う。既存 aggregate `CollectionKind` の意味と wire compatibility は維持する。

## 10. rename、cleanup、data protection

- filesystem rename / move の成功後だけ、App の既存 migration owner が collection actor へ typed
  path mapping を送る。App は mapping と request ID を全 affected collections の commit ACK まで一つの
  `PendingCollectionSourceMigration` に保持し、viewer close や context switch で捨てない。actor error 時は
  旧参照を全 collection に残し、同じ mapping の明示 retryとerror表示を所有する。
  file は exact key、folder は既存 path-prefix 規則と同じ境界で登録 source を更新する。
- actor の一 transaction は該当する全 collection entry の raw path、typed key、必要なら resolved kind を更新し、
  stable entry ID と manual position を保つ。unique conflict は勝手に merge / delete せず報告する。
- metadata cleanup や missing-file pruning で collection entry を削除しない。missing 保持が正本である。
- Settings import/export、settings recovery、sidecar cleanup に collection DB を混ぜない。
  collection text import/export は参照 list の明示操作で、rating や adjustment DB を転送しない。

## 11. 段階実装

### Phase 1: 永続基盤（最初の実装 chunk）

想定所有:

- 新規 `src/collection_store.rs` または `src/collection_store/{model,db,worker,import}.rs`
- `src/lib.rs` の module 登録
- module 内 unit / TempDir integration tests
- 本書の実装・検証記録

実装するもの:

1. stable IDs、source kind/key、definition/entry/snapshot、order/revision/error 型。
2. versioned SQLite schema と transaction API。
3. App 統合に依存しない単一 actor runtime/client、async startup、explicit shutdown。
4. pure import parser / export serializer と、confirm 後workerが渡す登録値のtyped境界。
5. pure manual/standard effective-order helper と latest-entry resolver。
6. rename/relink/batch registration command 境界。

この phase では App startup から actor を起動せず、実 profile に `collection.db` を作らない。
基盤を headless TempDir で検証してから、同じ API を Phase 2 以降へ接続する。

### Phase 2: PC catalog / toolbar / import-export UI

- App-global runtime lifecycle、Settings の toolbar UI flags、Collections toolbar section。
- create / rename / delete / order / reorder / add / remove / relink UI。
- pure preview → confirm 後 worker → actor batch の import と、snapshot → worker write の export。
- error / progress / cancel / Conflict 表示。UI thread の I/O は増やさない。

#### Phase 2 の具体的な統合境界

**production lifecycle**

- `src/lib.rs` の production 起動だけが `data_dir/collection.db` を指定して
  `CollectionStoreRuntime` を開始する。`App::default` や snapshot / headless harness は inert のままで、
  module load や test construction だけでは通常 profile を開かない。
- 起動順は collection runtime → App への runtime move とする。Phase 2 では raw
  `CollectionStoreClient` cloneをApp外へ保持せず、Remoteへもまだ渡さない。Phase 5は同じactor clientを内部に持つ
  `CollectionRemoteProducerControl`（名称は実装時に既存Remote ownerへ合わせる）をAppとRemote serverへ注入する。
  Remote requestは必ずこのshared admissionを通り、App finalがproducerをClosingへしてin-flight短期requestを
  drainできるACKを受けてからcollection actorをShutdownする。現状の`RemoteIpcServer` guardが`run_native`外で
  Appより長生きしても、Remote側がraw clientでpost-shutdown requestを送れる構造にはしない。Phase 5では
  producer先行close→actor shutdown、Remote guard遅drop、startup partial failureの順序を直接回帰する。
  thread spawn 失敗または非同期 DB open 失敗は
  collection UI だけを `Unavailable` にし、通常 folder / viewer / Remote の起動を継続する。
- App は runtime event と revision watch を `update` の viewport / fullscreen 早期 return より前で drain する。
  `Ready` 後だけ catalog request と編集操作を有効化する。revision notice は再読込の通知であり、toolbar や
  dialog が DB snapshot を独自に書き換える合図にはしない。
- App final `on_exit` は collection の request admission を閉じ、actor の短い transaction 完了と connection / WAL
  drop ACK を待って join する。通常 frame、toolbar、dialog closeでは join しない。App field の通常 Drop は
  `begin_shutdown` までの非blocking backstopとし、未完了 filesystem import/export worker は cancelしてdetachする。
- collection runtime / DB は §234 settings-family lease、復元、完全リセットの対象外である。設定復元中も actor と
  snapshot/revision は生存し、settings toolbar preferenceだけが復元対象になる。settings operation の success / cancel /
  recoverable failureの各後も collection DB bytes と catalog を変更しない。成功後のprocess終了では上記App final
  boundaryがcollection actorを通常終了させる。

**App-owned catalog と dialog request**

- `CollectionUiState` を一つのApp-global ownerとし、runtime lifecycle、immutable catalog、選択中
  `CollectionId`、revision watch、管理windowとそのtyped requestを保持する。collection選択は管理対象の選択だけで、
  Phase 3 の `TopLevelGridSurface::Collection` が無い間は `load_folder` や既存grid openへ変換しない。
- catalog / snapshot requestは、一つだけ保持するprivate receiver自体をrequest tokenとして、
  `{collection_id, requested catalog revision, requested collection revision}` と同じownerに持つ。
  選択変更、再要求、window close、collection deleteでreceiverをdropし、遅着resultは現在のreceiver ownerとstable
  ID/revisionが全て一致するときだけ採用する。newer revision noticeを見たら最新catalog/snapshotを再要求し、古い応答をapplyしない。
- dialog操作は `Idle | EditingName | ConfirmDelete | ConfirmRemove | ReadingImport | PreviewImport |
  Classifying | Submitting | ExportSnapshot | ExportWriting | Result` のような相互排他的typed ownerにする。
  作成/rename/delete/order/reorder/remove/relinkはactorのone-shot receiverをそのvariantが所有し、commit成功前に
  catalog/snapshotをoptimisticに変更しない。`Conflict` は操作内容と最新再読込導線を表示し、勝手に再送しない。
- delete はcollection definitionと参照だけ、removeは選択entryの参照だけを消す。どちらもfilesystem delete、
  ごみ箱、metadata cleanupを呼ばない。relinkは利用者が選んだ新sourceをclassifierへ通して同じentry IDへactor
  mutationし、元sourceをrename/move/deleteしない。

**toolbar と editor**

- `ToolbarSectionId::Collections` を新規defaultではBookshelf直後へ置く。既存の保存済みsection順は
  `ordered_with_fallback` の現契約どおり新sectionを末尾補完する。専用display/collapsed/show設定だけをSettingsへ
  保存し、catalog、active collection、entry、revisionをSettingsへ複製しない。
- sectionはcatalog名と「管理」を表示し、名前選択は同じ管理windowの対象を切り替える。Phase 2では閲覧一覧を
  開いたと表示せず、「一覧で開く」は出さない。create/rename/delete、order mode/sort、manual move、add/remove/relink、
  import/exportは管理window内へ集約し、空・長名・大量entryをscroll可能にする。
- 手動reorderはManualだけ有効、Standard中は保存済みmanual positionを表示上も破壊しない。Standard sort選択は
  collection definitionだけを更新し、通常一覧/Favorite/folder tree/folder representativeの設定を変更しない。

**import と direct add**

- native file pickerはpath選択だけをUIで行う。text bytes読込はcancel token付きworker、解析は既存の純
  `parse_collection_text` をworker内で行う。最初のpreviewはparsed/invalid/duplicate、解決済みlexical path、
  network表記だけで、targetへ `stat` / canonicalize / link解決 / thumbnail要求をしない。
- previewの明示Confirm後だけ、共通 `prepare_collection_sources` workerがaccepted sourceを一件ずつ現在の
  physical kindへ分類する。既存file/dirはImage/Video/Audio/Folder/Zip/Pdf/ConvertibleArchive、not-foundは
  `Unresolved`、permission/network/unsupportedはtyped line errorとする。folderを再帰展開せず、reparse targetを
  stable keyへ書き換えない。direct file/folder addとrelinkもこのclassifierを共有し、UIでkindを再推定しない。
- progressはlatest valueとして表示し、cancelはworker tokenを立てreceiverをdropする。actorへbatchをenqueueする
  前が取消可能なcommit boundaryであり、cancel済みworkerの遅着resultをenqueueしない。`Submitting`に入った後は
  cancelを完了済みtransactionの取消と表示せず、短いactor結果を待つ。actorはpreview時のcollection revisionを
  expected revisionとして最終unique/conflictを決める。
- previewに無効行がある間はConfirmを無効にする。Confirm後の分類でもunsupported/access errorが一件でもあれば、
  成功項目だけを部分登録せずbatch全体をactorへ送らない。分類成功とmissingだけで構成されたbatchを一transactionで
  登録し、duplicateはactorのunique判定結果として表示する。
- import source text、登録source、relink旧/新sourceはいずれも読取り/参照だけで内容を変更しない。cancel、parse
  failure、classifier error、revision conflictでcollection DBを部分更新せず、元データのhash/bytesを維持する。

**export**

- Exportクリック時にactorへ選択collectionの最新snapshotを要求し、dialogが保持する古いsnapshotをそのまま
  書かない。Manualは保存manual順を使用する。StandardはPhase 3でも共用する上記prepare workerで全entryの
  current kind、4行category、name/mtime/typed size factsを作り、snapshotのexact entry ID集合を検証して
  `effective_collection_order`へ渡す。`GridDisplayOrder`はworker開始時のimmutable settings snapshotを渡し、
  worker/UIが通常一覧設定を変更しない。categoryは
  `CollectionPreparedCategory::Display(GridItemDisplayKind) | UnresolvedTail`（名称は実装時調整可）として
  availabilityと分ける。現在missingでもDBのlast-known kindが既知なら、そのkindのDisplay categoryへ置き、
  metadataだけunknownにする。最初から`Unresolved`または現時点でも分類不能でlast-known kindが無いentryは、
  custom `GridDisplayOrder`の4行すべての後にdeterministic tailとして置き、同じtail内は選択`SortOrder`と
  filename tieを適用する。missing/unavailableをcategory 0やsize 0へ畳まず、昇順/降順でも全件を残す。
- immutable `{collection_id, collection revision, effective entry order}` から
  `serialize_collection_text` を呼び、absolute pathを一行一件で全件出力する。local filterや現在editorの選択は
  除外条件にしない。Phase 3のGrid prepareも同じ分類/facts builderを再利用し、export専用の別sort正本を作らない。
- 書込workerはdestination同directoryのrequest-owned temporary fileへwrite/flushし、cancel確認後だけatomic replaceする。
  failure/cancelでは既存destinationを維持してtemporaryをcleanupする。filesystem writeとthread joinをUIで行わず、
  結果はcollection ID/revision/request token一致時だけ表示する。exportは参照textであり、source、collection DB、
  rating/adjustment/settingsを変更しない。

**Phase 2 の直接回帰**

- production injectionだけがdata-dirのruntimeを開始し、App default/snapshot testはinert。Ready/Failed、catalog refresh、
  revision通知、late catalog/snapshot/mutation result、selection switch、dialog cancel、actor conflictをfake clientまたは
  TempDir runtimeで固定する。
- App final exitだけがactorをdrain/joinし、通常frame/dialog closeは待たない。settings restore/resetのsuccess/cancel/
  recoverable経路でcollection snapshot/revision/DB familyが不変かつruntime request可能である。
- create/rename/delete/order/manual reorder/add/remove/relinkのactual handlerを通し、stable ID/manual順、definition削除cascade、
  source非変更、removeとdeleteの文言/command分離を確認する。
- importはpreview中no-access、Confirm前accessなし、Confirm後classificationのexists/missing/unsupported/network error、
  progress/cancel、cancel遅着、actor enqueue境界、duplicate/conflict/rollback、source bytes不変を確認する。
- exportは最新snapshot、Manual/Standard effective order、custom 4行category、known-kind missingとUnresolved tail、
  全`SortOrder`でmissing全件保持、local filter非依存、atomic replace failure/cancel、
  parse/serialize roundtrip、destination/source/DB非変更をTempDirで確認する。
- toolbar fallback/default、専用設定roundtrip/customization copy、Favorite/Bookshelf/Smart/通常sortの不変、editorの
  empty/long name/scroll/import preview/progress/error/conflictをdark/light snapshotで確認する。Phase 3未接続のため
  toolbar name選択がfolder/grid openを起こさないhandler回帰も置く。

### Phase 3: collection grid / context / source migration

- `TopLevelGridSurface::Collection`、prepared snapshot、thumbnail/details、missing cell。
- physical source open と parent return、remove と source delete の分離。
- existing rename/move pipeline から actor migration へ接続。
- multi-window / detached context stamp と stale result 拒否。

### Phase 4: navigation / slideshow / playback

- Ctrl+上下、slideshow NextFolder の collection outer sequence。
- entry ID → source key → head の latest resolver と stop/loop。
- book 内部順、通常 folder/search/smart route、別 collection の不変回帰。
- 2026-09-15 に製品実装と独立 Sol / xhigh completion reviewを完了。非同期 owner、
  container preflight、通常 page / native / slideshow / 3媒体EOFの詳細と検収記録は
  [`collection-playback-plan.md`](collection-playback-plan.md)を正本とする。

### Phase 5: Remote read-only

- `remote-ipc` protocol、core service、web UI の persistent collection list/view/open。
- PC と同じ immutable snapshot / resolver、既存 aggregate collection との識別。
- protocol version、old/new client error、Remote session revision 回帰。

## 12. 検証計画

### 12.1 Phase 1 focused

- DB create / reopen で collection/entry ID、manual position、revision が不変。
- create/rename/delete、duplicate path、batch rollback、no-op revision、expected revision conflict。
- Manual → Standard → Manual で手動順不変、Standard 中の追加が manual tail。
- pure source-key builder の namespace / case / separator / drive / UNC / `.` / `..` / trailing、
  device/verbatim拒否、同 path unique、Unresolved → resolved/relink後もstable key。
- actor async Ready/Failed、FIFO transaction、queue Busy、client clone 残存中の explicit Shutdown /
  post-Closing admission拒否、connection drop / join、panic/disconnect unavailable。複数subscriberが
  同revisionを独立受信し、wake満杯時もlatestへ収束する。headless test が通常 data directory を開かない。
- import: BOM、有/無 quote、CRLF/LF、blank、`#`、relative、duplicate、URL/device/wildcard/env reject。
  存在しない path を多数渡しても parser が access しない fake/no-access backend 回帰。
- export full effective order、missing 含有、filter 非依存、parse/serialize roundtrip。
- latest resolver: ID 維持、ID 削除+same source、両方なし→head、tail stop/loop、別 collection revision 無視。

### 12.2 統合 phase

- production builder/handler-level で create/edit/import/remove/relink と actual grid materialize。
- confirm 後classifierのexists/missing/network/error/cancel、progressとactor batch競合。Phase 1は
  targetへ触れないpreview/parserとregistration command境界までで、filesystem workerはPhase 2で接続する。
- context A の遅延 result が context B、再open 後 generation、別 collection へ apply されない。
- Manual はカテゴリ regroup なし、Standard は通常 4 行カテゴリ、book 内部順不変。
- rename 成功/失敗/DB conflict、missing 保持、cleanup 非削除。
- settings restore/resetの実行中・成功・recoverable error後もcollection actor / DB / revision /
  snapshotが不変で、settings backup一覧にcollection DBが入らない。
- Ctrl+上下 / slideshow / playback の編集競合両順序を barrier 回帰。
- Remote と PC の同 revision preorder、protocol old/new、read-only admission。
- visible UI は dark/light snapshot、長い名前、空、missing、progress/error、scroll 到達性。

### 12.3 最終 gate

各 phase は narrow unit / integration → `cargo check` → `cargo fmt --check` を行う。共有 App、
Grid、navigation、Remote を接続した phase では repository の full gate、glyph、viewer-context audit、
diff check を同一 source freeze で実行する。user-runnable behavior が揃った時点で
`scripts/build-dev.ps1 -PreserveRuntime` を実行するが、agent は GUI を起動しない。

## 13. 不変条件

- UI thread に SQLite、stat、canonicalize、folder scan、archive classify、file read/write、join を追加しない。
- entry ID と collection ID は rename / reorder / relink / restart で変えない。
- commit していない state や stale context result を表示・navigation の正本にしない。
- manual order を通常 sort で破壊せず、book 内部順へ collection order を流さない。
- missing を cleanup や open failure で黙って削除せず、同名 path を推測 relink しない。
- collection DB を Settings family の復元/resetへ混ぜず、DB actor 外から書き換えない。
- collection integration が Favorite、Rating、ReadingHistory、Bookshelf、SmartFolder、通常 Folder、
  Search、Remote aggregate collection の既存所有・可否・順序を変えない。
- detached / linked viewer は context stamp と既存 lifecycle を使い、App-global current collection bool や
  delay、polling、症状 reset を追加しない。

## 14. Phase 1 実装記録（2026-09-14）

Phase 1 の永続基盤を `src/collection_store/` に実装した。App startup、通常 data directory、toolbar、
collection grid、confirm 後の filesystem worker、Remote にはまだ接続していないため、module load や test だけで
実 profile に `collection.db` は作られない。

- `CollectionId` / `CollectionEntryId`、filesystem namespace の `CollectionSourcePathKey`、resolved kind、
  manual / standard order、catalog / collection revision、latest resolver を共通型にした。
- SQLite v1 schema と単一 actor を実装した。request admission と enqueue、Closing 遷移と Shutdown publish は
  同じ owner で線形化し、actor は Shutdown 後の queued command を実行しない。client clone ごとの latest-watch は
  receiver を奪い合わず、explicit shutdown 後に新規 request を受け付けない。
- create / rename / delete / order / batch add / remove / manual reorder / relink / source migration を transaction にした。
  tree migration は全 collection を単一 transaction で更新し、affected revision と catalog revision を同じ commit で進める。
- trusted physical source と external text admission を分離した。合法な drive / UNC / extended 表記は同じ key へ畳み、
  external text の device namespace、DOS device component、ADS、control character、URL、wildcard、展開式を拒否する。
  parser / serializer は filesystem access を行わない。
- 既存 DB の `user_version` は read-only probe で先に確認し、未知 schema には WAL 設定や schema write を行わない。
  thread spawn failure は typed error、actor panic / unexpected exit は `Closed` と `Failed` event へ収束する。
- Phase 1 は pure import preview/parser と登録 command 境界までである。exists / supported kind / network error の
  classifier、progress、cancel、confirm 後の target access は Phase 2 の worker と handler-level 回帰で実装する。

Focused verification:

- `cargo test -p mimageviewer --lib collection_store::tests -- --nocapture`: 15 passed, 0 failed。
- covered: path normalization/admission、pure import/export、DB reopen/revision/manual order、atomic migration、
  delete cascade/catalog compact、relink/remove/invalid reorder、aligned standard order、prepared-facts latest resolver、
  subscriber fanout、Shutdown races、startup failure、actor panic、unknown schema no-write。
- GUI、通常 profile、real data、verification build は未使用。Phase 1 は user-runnable surface 未接続なので、
  build handoff は toolbar/grid を接続する Phase 2 以降で行う。

## 15. Phase 2 実装記録（2026-09-14）

Phase 2 は `collection.db` actorをproduction Appへ接続し、PCのtoolbarと管理windowからcatalogと参照を編集できる
段階まで実装した。collection内容を通常Grid、context menu、navigation、slideshow、playback、Remoteで閲覧する接続は
Phase 3以降であり、toolbarのcollection名は通常folderへ変換せず管理対象だけを切り替える。

- production creatorだけが通常data directoryの`collection.db` runtimeを開始してAppへ一度だけmoveする。
  App defaultとheadless fixtureはinertである。startup errorはcollection UIだけをUnavailableにし、runtime event、
  catalog/snapshot private receiver、latest revision watchはfullscreen/native-videoの早期returnより前でpollする。
  final `App::on_exit`だけがrequest admissionを閉じてactorをdrain/joinし、通常frameとdialog closeは待たない。
- `CollectionUiState`がruntime、immutable catalog/snapshot、選択ID、管理window、相互排他的operationを所有する。
  create/rename/delete/order/reorder/add/remove/relinkはactor応答のcommit後だけsnapshotへ反映し、Conflictは再送せず
  latest catalog/snapshotを取り直す。remove/deleteは参照だけを消し、元sourceを削除・移動・書換えしない。
- actorの`Failed`/`Closed`後は同じtyped Ready判定で全mutation/import/export入口を無効化し、古いsnapshotは
  read-only表示に留める。toolbarは`準備中`と`利用不可`を区別し、利用不可でも管理windowのerrorへ到達できる。
- direct add/relinkとtext import confirm後は共通classifier workerを使う。text bytesの読込とpure previewはworker内、
  preview中はtargetへI/Oせず、明示confirm後だけcurrent existence/kindを調べる。progress/cancel ownerをoperationへ保持し、
  ReadingImport/Classifying/ExportWritingは明示的な取消buttonを持つ。cancel/選択変更/window close後の遅着結果はactorへ
  enqueueしない。Submitting/ExportSnapshotは短いactor処理なので取消可能とは表示しない。
- exportはクリック時にactorへ最新snapshotを要求し、workerが同じprepared category/order正本を通して全参照を並べ、
  destination同directoryの一時fileへwrite/flush/sync後にatomic replaceする。cancel/書込/replace失敗は既存destinationを保持し、
  source、collection DB、通常Grid設定を変更しない。
- Settingsへ追加したのはtoolbarのshow/display/collapsed preferenceだけである。既存profileの保存済みsection順には
  `ordered_with_fallback`で新sectionを末尾補完し、新規defaultはBookshelf直後、表示形式はDropdownとした。
  catalog/active collection/revisionはSettingsへ複製しない。settings復元/完全reset中もcollection DB/runtimeは独立して残る。

Focused verification（final gate前の実装checkpoint）:

- `cargo test -p mimageviewer --lib ui_dialogs::collections::tests -- --nocapture`: 9 passed, 0 failed
  （handler 7件、production managerのdark/light snapshot 2件）。
- `cargo test -p mimageviewer --lib collection_store::prepare::tests -- --nocapture`: 5 passed, 0 failed。
- `cargo test -p mimageviewer --lib settings_full_reset_does_not_change_collection_family -- --nocapture`: 1 passed, 0 failed。
- `cargo test -p mimageviewer --lib collection_toolbar_defaults_roundtrip -- --nocapture`: 1 passed, 0 failed。
- handler回帰はcreate→direct add→manual reorder→Standard sort→stable ID relink→reference remove→
  import preview/confirm→deleteを実actorへ通し、source bytes不変、Conflict再読込、toolbar名選択で通常folder不変、
  final shutdown後admission拒否を確認する。加えてFailed時の全編集共通拒否、toolbar利用不可表示、classifier/preview
  error時の部分登録なし、明示取消buttonによるlate result owner破棄、stale dialog snapshotからのexportがactor最新revisionと
  全entryを出力することを確認する。
- snapshotは長いcollection名、missing row、選択/check、管理操作、no-access import preview、network/invalid lineを
  production manager描画で確認する。GUI、通常profile、real dataは使用していない。

Final automated verification:

- 初回 `scripts/test-full.ps1` は main `8463 passed / 2 failed / 45 ignored`、exit 101 だった。新しいcollection名入力が
  IME helperを通らない既存契約違反と、Bookshelf直後へCollectionsを追加した後も旧隣接関係を期待していたtoolbar
  reorder testを特定した。入力を `ime_focus::add_singleline` へ統一し、no-op期待を新default順へ補正した後、該当回帰は
  それぞれ1/1 passed。初回ログは `target/collection-phase2-20260914/test-full.{stdout,stderr}.log` と
  `test-full.exit.txt` に失敗証拠として分離保持した。
- 最終source freezeは `target/collection-phase2-20260914/source-freeze-final.sha256.txt`
  （manifest SHA-256 `1AD6B020E59471D74814691838CB84221FBD65DA3559A94DD32A0716C06C0F51`）。同freezeの
  `scripts/test-full.ps1` は exit 0、main `8465 passed / 0 failed / 45 ignored`、UI snapshot 50/50、vendor egui /
  egui-wgpu / eframe 25/25・9/9・15/15、`[test-full] PASS`。ログSHA-256は stdout
  `2F563ED13D9AB5BBEA1EA46159843294ED012AB0BCF5055D2D58956682A17ADB`、stderr
  `5CECC9B45ACB4B1E2966D8D8525FEAF4F10BD076C859DCE9312EA0440968F78E`、exit
  `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
- 同freezeで `cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、
  `python scripts/check_ui_glyphs.py`、`cargo run -p viewer_context_audit --quiet`、`git diff --check` はすべてexit 0。
- resident 0をbuild前後に確認し、`scripts/build-dev.ps1 -PreserveRuntime` はexit 0。agentによるAppの起動・停止は
  行っていない。verification artifactはcore SHA-256
  `6B634E07BB0B71071209AB399215BCF878F67DDCA91580FDF281152474DF1761`、remote SHA-256
  `192E2F800704A833C46831C2A42C47F24558B17A19C80F3DC23BEC21CE7D057D`。buildログSHA-256はstdout
  `E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855`、stderr
  `67CB9CD919B566F1251ED28CD7122DE180C8B1689284F7DDB2E64CBC025441E5`、exit
  `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
- GUI確認は未実施。Phase 3のcollection Grid/context/source migration、Phase 4のnavigation/slideshow/playback、
  Phase 5のRemote閲覧は未接続である。

## 16. Phase 3 実装設計: collection Grid / context / source migration

この節は Phase 3 着手前に現行コードへ照合した実装境界である。Phase 2 の
`CollectionUiState` は process-global な DB runtime、catalog、管理windowを所有している一方、
表示中の一覧とその戻り先は `ViewerContextBundle` 内の `TopLevelGridView` が所有している。
collection を開いたかどうかを App-global な bool や selected collection ID から推定せず、この既存の
viewer-context 境界へ collection surface を追加する。

### 16.1 context-owned surface と非同期prepare

- `TopLevelGridSurface::Collection(CollectionGridIdentity)` と
  `TopLevelGridRestore::Collection(CollectionGridRestore)` を追加する。identity / restore は
  `CollectionId`と選択復元用の`CollectionGridViewportAnchor { entry_id, source_key }`を持つ。surfaceのstable identityは
  `CollectionId`だけであり、accepted / wanted revisionをsurface identityへ入れてgenerationを動かさない。
  `CollectionGridRestore::revision_at_open`は再入場時のminimum hintであってexact snapshot identityではない。
  rootからchildを開く時点でこのanchorをcaptureし、復帰prepare後にentry ID、次にsource keyでindexを解決して
  `selected`と既存`scroll_to_selected`へ一度だけ接続する。途中の`load_folder`がscroll offsetをresetしても、
  次のGrid layoutが選択cellをviewport内へ確実に入れる。entryが既に無い時だけselection無しへ落とす。
  collection名も表示用snapshotでありstable identityに
  使わない。collection rootから物理folder / book / archiveを開くと、同じviewer contextの`return_to`へ
  `CollectionGridRestore`を保存する。戻る時は最新snapshotを取り直し、entry ID、次にsource key、無ければ
  selection無しの順で位置を解決する。
- `TopLevelGridView`にmove-onlyな`CollectionGridSession`を持たせる。sessionはclone可能な最後の
  immutable installed bindingと、`SnapshotRequest | Preparing | Ready | Failed | Deleted`の一つのrequest owner、
  private receiver、cancel token、cloneごとに独立したrevision watchを所有する。viewer contextを複製する時は
  表示済みimmutable snapshotとaccepted / wanted revisionをコピーし、receiver / watchはコピーしない。次にそのcontextがmountされた
  時に同じcollection IDのwatchと最新snapshot requestを作り直す。parked contextの結果を別contextへ適用しない。
  request terminalと表示済みbindingは分離し、refresh失敗時は古い行をmutation可能として見せずtyped errorを表示する。
  terminal failureは毎frame再requestせず、明示reload / reopen / 新しいrelevant noticeだけがretry ownerになる。
  `CollectionGridSession::Drop`はPreparing cancelを必ず立て、surface退出 / collection切替 / context retireでfilesystem分類を
  完走させない。cloneはconsume済みnoticeのwanted revisionも保持し、復帰時にlatest要求を失わない。
- actor `SnapshotRequest` は
  `{ViewerContextId, surface generation, CollectionId, minimum / wanted revision}`を持ち、返却snapshotが
  wanted以上なら受理する。そこから開始したfilesystem `Preparing`は返却snapshotのexact revisionをstampし、
  完了時にaccepted revisionと一致し、かつその後のrevision noticeでwantedが進んでいない時だけ受理する。
  初回wanted=0から最新revisionが返る場合と、prepare中に新noticeが来る場合を同じexact比較へ畳まない。
  mounted context / surface / generation / collection IDも両段で再照合する。違うcontext、surface退出、
  collection切替、reload、cancel、より新しいwanted後の旧結果は捨てる。数値だけの別ownerや
  App-global current-collection stateは追加しない。
- toolbarのcollection名を選ぶ操作は、Phase 2の管理対象選択だけから
  `open_collection_grid(CollectionId)`へ切り替える。「管理…」は従来どおり管理windowを開く。
  openはactorへsnapshotを要求するだけでUI threadからDB、`stat`、canonicalize、thumbnail I/Oを行わない。
  request中はcollection surface自身が準備中を表示し、以前のfolder itemsをcollection itemとして操作可能に
  しない。actorがUnavailable / Closedなら古いprepared snapshotはread-onlyの表示候補にできるが、remove、
  relink、source migration等のmutationは共通Ready gateで拒否する。
- revision watchは各sessionのlatest-wins receiverである。同じcollectionのwanted revisionが進んだcontextだけを
  再requestし、別collectionと通常Folder / SmartFolder / Search等は触らない。filesystem deleteのようにDB
  revisionを進めないsource変化は、delete開始時に各対象を`Exact | Tree`へ型付けしてworker ownerへ渡す。
  成功したExact pathに一致する参照、または成功したTree path自身とその子孫を含むcollection sessionだけを
  viewer-context registryの既存context transactionで`NeedsPrepare`へする。削除後の`is_dir`でscopeを推定しない。
  mounted / parkedの各sessionが自分のstampで再prepareし、
  process-globalなfilesystem epochは追加しない。

### 16.2 immutable listing と既存Gridへのmaterialize

- Phase 2のexport専用名になっているprepare成果を共通
  `CollectionPreparedSnapshot`へ引き上げる。これはcollection ID / revision、effective orderと、orderにalignedな
  `PreparedCollectionEntry { entry_id, source_key, source_path, last_known_kind,
  current availability, GridItem / display metadata }`を`Arc`で保持する。prepare workerだけがsourceを調べ、
  UIはimmutable成果を`items`、`image_metas`、thumbnail request、details rowへmaterializeする。
- ManualはDBのmanual positionをexactに保ち、categoryで再配置しない。StandardだけがPhase 2 exportと同じ
  `effective_collection_order`、4行`GridDisplayOrder`、選択`SortOrder`を使う。available / missing / unsupported /
  access errorを全件保持し、last-known kindがあるmissingはその表示category、真のUnresolvedは4行後のtailへ
  置く。book / ZIP / PDFを開いた後の内部順へcollection sortを渡さない。
- available sourceは既存`GridItem`へ変換し、既存thumbnail / details / fullscreen / container loaderを使う。
  missing、unsupported、access errorは新しい物理操作不能なplaceholder cellで表し、通常Image等へ偽装しない。
  placeholderはpath名と「見つかりません」またはtyped理由を描き、thumbnail decode、rating / adjustment、drag、
  clipboard、rename、source delete、external toolを開始しない。`GridItem`側には専用の
  `FileOperationRefusal`を持たせ、collection-specificなentry ID / source key / availabilityは同じcontextの
  aligned bindingだけが所有する。表示metaの0や拡張子からavailabilityを再推定しない。
- collection bindingは`items_generation`とprepared stampを持ち、cell indexからentry ID / source keyを得る
  唯一のownerにする。selection、checked、hover、scrollは既存viewer context fieldをそのまま使う。
  thumbnail / details painterはavailable cellだけ既存要求を出し、placeholder badge / textを描く。別contextの
  binding、違うitems generation、遅着thumbnailを参照しない。local filterを使う場合も表示projectionだけであり、
  DB manual order、export全件、Phase 4 navigation正本を変更しない。
- root bindingの更新時は旧selectedをentry ID、次にsource keyでremapし、checkedはEntryId exactだけでremapする。
  remove後に同じsourceを新entry IDで再登録してもcheck権限を移さない。物理child閲覧中またはroot leaf fullscreen中は
  revision / delete noticeのwantedだけを進め、root rowsをinstall / clearしない。typed returnまたはfullscreen close後に
  latest rootへ収束し、collection deleteもそのpresentation境界で初めてemptyへ置換する。delete-during-refreshは
  installed bindingでExact / Treeを照合してin-flight分類をcancelし、削除前のAvailable結果を採用しない。

### 16.3 open、戻り先、context menu

- toolbarの明示Openとcollection間の明示切替は、folder Back/Forwardと共通のtyped history
  `FolderNavHistoryTarget`へ`CollectionGridRestore`を積む。履歴entryはPath / Rating / SmartFolder /
  Collectionのいずれか一つを所有し、collectionをfilesystem風のsynthetic pathへ変換しない。
  A→collection C→BはBackでC→A、ForwardでCへ戻り、C1/C2はstable CollectionIdで区別する。
  rename後は同じIDからlatest snapshotへ収束し、Ready catalogで削除済みと確定したIDだけをnormal+A/Bの
  back/forwardからpruneする。rollback snapshotのinstall時も同じReady catalogで再pruneし、Starting / Failed /
  一時的なcatalog不在では履歴を捨てない。Add、manager選択、watch refresh、collection-owned child/reloadは履歴を積まない。
- collection rootから独立したphysical navigationへ出る時は、scan / archive adoptionがvisible結果を採用した境界で
  collection restoreを履歴へcommitする。scan失敗、scope拒否、stale completion、sidecar restoreではroot surfaceと
  履歴を維持する。collection-owned child / descendant / same-folder reloadは従来どおりsessionを保持し、root復帰に
  folder historyを使わない。

- collection rootのphysical leafは既存fullscreen openへ渡し、そのviewer contextのcollection originを
  `{collection_id, revision_at_open, entry_id, source_key}`として保持する。folder / book / ZIP / PDF /
  convertible containerは既存loaderへ渡す直前に同じoriginを`TopLevelGridRestore::Collection`へ保存する。
  子一覧では既存の本内部順、archive page順、auto fullscreen、media機能を変えない。BS / address-bar parentで
  collection restoreへ戻る時だけ最新snapshotを非同期prepareし、物理parentへ逸らさない。
- detached physical contextを作る既存factoryは、呼出元がcollection cellの時だけtyped
  `TopLevelGridRestore::Collection`を新contextへ渡す。main側collection sessionはmain bundleに残り、detached側は
  自身のcontext ID / generationで戻り先とprepareを所有する。既存viewport生成、host identity、placement、focus、
  geometry、mount / park / retire predicateは変更しない。linked viewもmount済みbundleの同じsurface / originを使う。
  これはviewer-owned top-level originを既存bundleへ追加する構造拡張であり、detached症状用guardではない。
  folder candidate / cache hit / async mixed-folder scan / ZIP / PDF / convertible archiveの全ownerがrequest時のrestoreを
  completionまで運ぶ。特にarchive変換はcontext ID / surface generation / collection ID / accepted+wanted revision /
  entry ID+source keyを一つのownerで照合し、処理中にcontextまたはcollectionが切り替わった旧completionを新sessionへ
  commitしない。
- context menuへstable command「コレクションから外す」を追加し、collection bindingが現在cellを所有する時だけ
  表示する。collection rootの通常Deleteも同じhandlerへ入り、複数checkedは同じcollection / generationに属す
  entry IDだけを一transactionの`remove_entries`へ渡す。checkedがなければselectedを使う。stale / 未install bindingは
  fail closedで通知し、元ファイル削除へfallbackしない。commandは元file、folder、book、archive、metadataを変更しない。
  missing cellも参照解除でき、既存managerのrelink導線を維持する。PhysicalSource childのDeleteは従来のfile deleteである。
- removeをactorへenqueueした後のresponseは、surfaceを閉じても「取消済み」にはできない。process-global
  `CollectionUiState`のtyped mutation ownerが
  `{request token, origin ViewerContextId, surface generation, CollectionId, expected revision, entry IDs, receiver}`を
  commit / errorまで保持する。successはwatch noticeだけを表示正本として全contextへfanoutし、originが同じsurface /
  generationの時だけselection等のlocal補助stateを更新する。surface退出、context retire、兄弟context、新generationへ
  responseを直接applyしない。Conflict / persistence errorは失わず管理windowの該当collectionへ再読込 / retry可能な
  resultとして残し、actorへenqueue済みtransactionをUI cancelと表示しない。
- available physical sourceの「元ファイルをゴミ箱へ移動…」は既存確認 / delete workerをそのまま使い、menu文言で
  source実体を消す操作だと区別する。collection rootのWindows dynamic menuはglobal Inline設定にかかわらず
  「元ファイルのWindowsメニュー」submenuへ置き、Shellの削除、関連付け、プロパティ、拡張commandを元ファイルへ
  従来どおり適用する。PhysicalSourceと通常Folderはglobal Inline/Submenu設定を維持する。成功してもcollection entryを消さず、前節のsource変化invalidateで
  placeholderへ更新する。failure / cancelではcollection listingを変更しない。collection removeとsource deleteの
  handlerを相互に呼び出さない。

### 16.4 rename / move 成功後のcollection source migration

- migration requestはprocess-global DB actorに対するためAppのtyped FIFO ownerが持つ。
  viewerの現在surfaceとは独立し、queue / exact `InFlight {receiver, batch}`と
  `RenameMigrationJournalAdmission::{Durable, Waiting, Failed}`が順序、response、保存ACK、bounded retryを所有する。
  ただしmemory queueだけを耐障害性の正本にしない。既存
  `rename_migration_journal`のentryをtyped multi-map / scopeとgeneric-store / collection-actor各stageの完了状態まで
  拡張し、collection actorのexact batch ACKを受けるまで消し込まない。起動時は未完stageだけを冪等に再開する。
  journal writer / workerでI/Oし、UI threadで待たない。actor Ready前 / Busy / Unavailableはsource operationを
  巻き戻さずjournalに保持する。duplicate / actor persistence failureも旧collection参照を勝手に消さず、missing表示と
  errorを残して次回起動へ渡す。journal自体の保存失敗は後述のbounded自動retryとexit final attemptへ渡し、
  恒久障害を次boot回復可能または保存成功と偽らない。generic metadata migrationが終わってもcollection stage未ACKなら
  durable entryを消さない。完全な現在queue snapshotの保存成功ACKを各stageの開始条件にし、保存失敗時は
  exact failed snapshotとApp queueを保持して100ms / 1s / 4sのbounded async retryを行う。retryはその時点の
  App全snapshotで古い失敗snapshotを置換し、旧集合を復活させない。Windowsの更新保存は
  `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`で既存journalをatomic置換する。
- 通常のShell renameは操作成功後だけ、元がfileならExact、folderならTreeのmigrationをenqueueする。
  Shell failure / abortではenqueueしない。tree判定はrename開始時のtyped operation ownerが保持し、rename後の
  `is_dir`で推定しない。
- 本名変更はbook rootのTree migration、book間Moveとpage reorderは`BookPathMapping`のExact集合を、各book
  operation成功結果からenqueueする。Copyはmigrationしない。複数mappingやswap / cycleを一件ずつcommitすると
  partial pathとUNIQUE衝突を作るため、actor APIを`CollectionSourceMigrationBatch`へ拡張し、一command / 一SQLite
  transactionで全collectionの最終key集合を検証してから更新する。DB内の一時keyはtransaction外へ公開せず、
  rollback時は全旧参照を保つ。単一renameも同じbatch正本を使う。
- migration commitはaffected collection revisionとcatalog revisionを同じtransactionで進め、既存fanout noticeを
  発行する。各viewer sessionは自分のwatchから最新snapshotを取り直す。filesystem cleanupやmissing pruningから
  migration / removeを呼ばず、外部Explorerでのrenameを推測追跡しない。

### 16.5 lifecycle、Phase 4/5境界、変更予定path

- collection Grid request / prepareはsurface退出、collection delete、context retire、App exitでcancelしreceiverを
  dropする。通常frameではfinished workerだけを回収しjoinしない。final exitはPhase 2のproducer admission close後、
  prepareをcancelする。新しいremove / migration admissionを先に閉じ、既にactorへenqueue済みの短いcommandだけを
  resultまたはactor terminal ACKまで処理する。FailedRetryable、Busy、Unavailableを成功まで無期限に待たず、未完の
  migration stageをdurable journal writerのlatest revisionまでflushして次bootへ渡した後、collection actorを
  shutdown/joinする。UI frameには待機を置かず、このdrain / flush / joinはfinal exit境界だけで行う。settings restore/resetは
  collection DBとsession snapshotを変更せず、restore modal中の既存入力gateだけを維持する。終了時はlazyな
  crash-recovery journalを必ず先にloadしてcurrent queueへ統合し、最新full snapshotをもう一度非同期writerへ渡してから
  flushする。恒久的なI/O障害ではsource操作を巻き戻さず、stageを未開始のままunsaved errorとしてlogへ残す。
  書けない永続媒体を成功扱いしたり、終了を無期限に止めたりしないという物理的な限界は明示する。
- Phase 4のCtrl+上下、slideshow NextFolder、再生中編集のlatest-nextは
  [`collection-playback-plan.md`](collection-playback-plan.md)のtyped owner / latest prepared resolverへ接続した。originは
  entry ID / source key / collection IDを保持し、indexだけのfallbackや物理DFSへcollectionを流さない。
  Phase 5 Remoteへraw UI sessionを流用せず、同じimmutable prepared modelとactor revisionを渡せるようにする。
- 主な変更候補は`src/collection_store/{model,runtime,db,prepare}.rs`、
  `src/app/top_level_grid_view.rs`、`src/app/viewer_context_registry.rs`、`src/app.rs`、`src/ui_main.rs`、
  `src/grid_item.rs`、`src/app/grid_paint.rs`、`src/context_menu_model.rs`、
  `src/ui_dialogs/{collections,context_menu}.rs`と対応test、context-menu default/settings/help/manual、
  本書、`docs/detached-rework-plan.md` §11である。通常Folder / Smart / Search / Favorite / ReadingHistory /
  Bookshelf / Snapshot / Rating / Remoteの既存surface・設定・順序は変更対象外とする。

### 16.6 Phase 3 直接回帰と完了境界

- prepare pure test: Manual exact、Standardのcustom4行と全sort、known-kind missing、Unresolved tail、
  unsupported / access error、全entry ID集合、available GridItem変換、placeholder非I/O / 非物理操作。
- handler test: toolbar open→actor snapshot→prepare→install、別collection選択、surface exit、reload、revision notice、
  cancel / late result、context / generation / revision mismatch、park / remount、collection delete / runtime Failed。
- context test: root→image fullscreen、folder / book / ZIP / PDF child→最新root復帰、entry ID→source key→selection無し、
  load_folderでscrollがresetされた後も解決済みentryを`scroll_to_selected`がviewport内へ戻すこと、
  mainとdetachedのorigin独立、片方の遅着resultが兄弟へ入らない。既存detached viewport / placement / focus / host
  fingerprintと通常top-level return回帰を維持する。
- context-menu test: reference removeだけがDBを更新しsource bytes不変、enqueue後surface exit / context retireでも
  commit/error ownerが結果を失わず兄弟/新generationへ直接applyしないこと、Conflict後の再読込、source delete成功後
  entryを保持してmissing化、file Exact / folder Treeの子孫invalidate、missingはremove / relink以外の物理操作不可、
  checkedの別surface / generation混在拒否。
- migration test: file Exact、folder / book rename Tree、book Move / reorderのmulti-mapとcycle、全collection一transaction、
  duplicate / DB error rollback、Busy / unavailable / crash後journal再開、generic stage完了後もcollection ACK前はjournal保持、
  final exitは未完retryを待たずdurable stageをflushしてbootへ渡し、already-enqueued commandだけterminalへ収束すること、
  journal save失敗中はstageを開始せず、書込先回復後のbounded retryは最新full snapshotだけを保存してから開始可能になること、
  既存journal atomic置換、journalをまだlazy-loadしていない即時終了でも旧recovery jobを空で消さないこと、恒久I/O failureは
  unsavedをsuccessと記録しないこと、abort / failure no-enqueue、watch fanoutと複数context再prepare。
- focused test、viewer-context audit、UI snapshot / glyph、`scripts/test-full.ps1`、fmt / diff、resident不在確認後の
  `scripts/build-dev.ps1 -PreserveRuntime`までを同一freezeで行う。Phase 3完了時点でもPhase 4 navigation /
  slideshow / playback latest resolverとPhase 5 Remote閲覧、GUI実機確認は未完了として明記する。

### 16.7 Phase 3 実装checkpoint

- `TopLevelGridView`へcollection IDだけをsurface identityとする`CollectionGridSession`を追加した。actor snapshotは
  minimum / wanted revision、filesystem prepareはexact revision、installはviewer context / surface generation /
  collection IDを照合する。cloneしたcontextはimmutable prepared成果だけを引継ぎ、mount後に独立watchを再購読する。
  entry IDを先に、source keyを次に使うviewport anchorを既存`selected` / `scroll_to_selected`へ接続した。
- prepare workerはManual順をexactに保ち、Standardだけ既存4行 / sort正本を使う。available itemは既存GridItemへ、
  missing / unsupported / access errorは物理操作不能なtyped placeholderへ投影する。UIはimmutable aligned bindingだけを
  読み、DB、stat、canonicalize、thumbnail decodeを同期実行しない。dark / light snapshotは3理由、selection、checkを
  production painterで固定した。
- toolbarのcollection名はcollection Gridを開き、「管理…」は管理windowを維持する。physical leaf / folder / book /
  ZIP / PDF / convertibleは既存open経路を使い、戻る時は同じcontextのtyped restoreから最新collectionを取り直す。
  context menuは「コレクションから外す」を別commandとして持ち、commit responseはprocess-global stamped ownerが保持、
  watch fanoutが各contextを収束させる。元sourceの「ごみ箱へ移動」とreference removeは相互に呼び出さない。
- delete開始時にcaptureしたExact / Tree scopeだけでmounted / parked sessionをinvalidateする。rename / book moveの
  durable journalはgeneric store完了後もcollection actor exact ACKまで残り、複数path / swapは全collectionを一つの
  transactionで更新する。各stageはjournal保存成功ACKまで開始せず、失敗時はlatest full snapshotのbounded async retry、
  終了時はlazy recovery load後のfinal persist / flushを行う。重複またはDB errorは全旧参照をrollbackし、Busy / unavailable /
  crashはjournalを次bootへ残す。恒久的な媒体I/O failureだけはstage未開始・unsaved errorとして明示し、成功扱いしない。
- focused checkpointはcollection filter 93/93、collection store 23/23、collection Grid 13/13、
  rename migration 8/8、rename journal 31/31、context menu model 27/27、collection placeholder snapshot 2/2、
  exact owner / lifecycle回帰8/8、core checkがすべてpass。snapshotはagentがdark / light双方を目視した。
  初回fullは新しい「コレクションから外す」行に対する既存preferences context-menu golden 2枚だけが不一致で、
  main 8492 passed / 1 failed / 45 ignored、exit 101だった。actualはRootへの1行追加とOpenWith captureの
  scroll位置変化だけで、独立reviewerが原寸確認した。golden更新後のexact回帰は1/1 passedで、初回ログは
  `target/collection-phase3-final-20260914/test-full.initial.{stdout.log,stderr.log,exit.txt}`へ分離保存した。
- 更新goldenを含む39 pathのtest/build freezeは
  `target/collection-phase3-final-20260914/source-freeze.sha256.txt`、manifest SHA-256は
  `FA12AF99DD4D1912A79235723AB2C6A671052A7956B7A7B253FA24626CD03A40`である。final
  `RUST_TEST_THREADS=1 scripts/test-full.ps1 -SuppressCrashDialogs`はmain 8493 / 0 / 45、UI snapshot 52/52、
  vendor egui / egui-wgpu / eframe 25 / 9 / 15、`[test-full] PASS`、exit 0で、process error modeを
  `0x00008001`へ復元した。stdout / stderr / host / exitログSHA-256は順に
  `A064AAC301FBCF119AB7F406C026EAEB733B5392BFAAA1D9A2A1C3C109AA1B9F`、
  `A154DD0EA8D52993C0F29B3C852DD2C45B62E916700B7DE6344CE28CC86359F6`、
  `2A7FC39AF5B2BEAE63CB4FB4A60F9DD89BC8E5EFC60FCB98DF18BBC58B9D2610`、
  `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`である。
- 同じfreezeでfmt、UI glyph、viewer-context audit、diff checkはexit 0。static stdout / stderr / exitの
  SHA-256は`F25A93473A64B60B2056B15EE45335AB46D7B436FAFDA252B60895C2912760C4`、
  `84DD34CC8E34F450363ECFDF09C72B4FF56CD85D4CC0D1EADE6139DD430121BE`、
  `13BF7B3039C63BF5A50491FA3CFD8EB4E699D1BA1436315AEF9CBE5711530354`。
  resident 0確認後の`build-dev.ps1 -PreserveRuntime`もexit 0で、core SHA-256は
  `7B1928E134AD370FCB5312600404D18B7171C391DF7D5966A48211E2B105E36C`、Remoteは
  `192E2F800704A833C46831C2A42C47F24558B17A19C80F3DC23BEC21CE7D057D`。アプリの起動 / 停止、GUI、
  通常profile、real dataは使用していない。この記録時点ではPhase 4のCtrl+上下 / slideshow / playback latest-nextと
  Phase 5のRemote閲覧は未接続だった。Phase 4は2026-09-15に後続chunkで実装し、Phase 5は未接続である。

## 17. Phase 4 実装checkpoint（2026-09-15）

Ctrl+上下、通常 page / native next・prev、slideshow / NextFolder、video / video-audio / music EOFを、
viewer-context所有のtyped requestと最新`CollectionPreparedSnapshot`のpure resolverへ接続した。entry ID→source key→
head、見開きdisplay-unit anchor、media / container kind、Stop / Loop、有限preflightを一つの順序正本で扱う。
folder / ZIP / PDF / convertibleは着地可能payload成功後だけcommitし、暗号sourceは既存password / conversion ownerへ
exact request / revision watchを移譲する。book / ZIP / PDF child内の固定順、local query / filter、既存native /
ParkedLive landingを維持した。編集noticeは現在再生とroot rowsを変更せず、次requestだけが最新有効順を使う。

独立Sol / xhigh reviewerは非同期current / cancel / drop、同revision watch race、Grid selection、manual rapid input、
same-context / detached nested ZIP、既存EOF landingをaffected pathで再照合し、blocking / should-fixなしで承認した。
focused / full / staticの数値とログhash、build保留理由は
[`collection-playback-plan.md` §11.2](collection-playback-plan.md#112-製品実装独立-completion-review-checkpoint2026-09-15)
を正本とする。このcheckpoint時点で未実装だったPhase 5 Remoteは、後続の§18で実装した。

## 18. Phase 5 Remote read-only実装checkpoint（2026-09-16）

認証済みmIV Remoteへ、名前付き永続collectionのcatalog、root一覧、direct image / video / audio、
container child open / return、latest next / prev / EOFを追加した。protocolは56で、既存aggregate collectionとは
別message / route / browser ownerを使う。PCと同じactor snapshot / immutable prepared model / pure resolverを読み、
Remoteからcreate / edit / sort writeを公開しない。

session / route / revision / deadline / cancel、Remote pathとactual kindの二重検証、64 MiB response budget、
truncated sparse target、display-unit / ordinal、終了時App ACK→server→producer→actorの所有境界と独立review、
focused / full / static / verification build保留証跡は
[`collection-remote-plan.md` §16](collection-remote-plan.md#16-製品実装独立-completion-review-checkpoint2026-09-16)
を正本とする。通常RemoteとPC編集、Folder / ZIP / PDF child内部順、AI / streamingは既存経路を維持する。

## 19. PC toolbarからの参照追加と管理picker撤去（2026-09-16）

> この節は最初のtoolbar実装時点の記録である。現行の管理責務、固定shortcut、表示形式、上部メニューは
> [§21](#21-本棚同型のcollection管理とgrid起点操作2026-09-16)で置き換えた。

- 本棚と同じく、全表示形式で追加先コンボと明示的な`追加` / `開く`を常設した。コンボは追加先の
  選択だけを行い、一覧を開かない。`展開`と展開中の`折りたたみ`では名前shortcutを表示し、左クリックで
  開く、右クリックで現在のGrid選択を追加する。`プルダウン`はshortcut列だけを隠す既存compact表示として
  保持する。`grid_selection_indices()`を共用し、checkedがあればcheckedを優先、無ければselectedを使い、
  追加操作時にpathを捕捉する。
- toolbar targetは設定に永続化し、管理windowの`selected_id`、現在のGrid surfaceと別ownerにした。
  authoritativeなReady catalogだけで存在を検証し、削除済みなら先頭、空ならNoneへ補正する。Starting /
  Unavailableと一時的なsnapshot欠落では保持する。追加完了でも元surface / address / items generation / selected /
  checked / scrollを変えず、明示的な`開く`または名前の左クリックだけがsurfaceを切り替える。
- 追加対象は`GridItem::drag_source_path()`を持つ物理sourceだけとする。仮想page / archive directory / Stack /
  SearchContainer / CollectionPlaceholderを混在させたbatchは全体を保存前に拒否し、親archive・代表画像へ
  丸めない。分類は既存worker、保存はactorの`add_batch`を使い、source bytesは変更しない。
- toolbar click後に対象collectionの最新actor snapshotを非同期取得し、そのrevisionで分類結果をsubmitする。
  managerの遅いsnapshotをexpected revisionに使わない。duplicate / conflict / actor errorは対象名付きtoastへ
  terminal結果を返し、成功後のPC Collection GridとRemote read-only表示は既存watch fanoutで収束させる。
- managerのfile/folder pickerと対応actionを撤去し、管理windowにはimport / export / remove / relinkを残した。
  manager operationが進行中ならtoolbar addは開始しない。toolbar起点のsnapshot / classify / actor submitは
  manager windowを閉じてもcancelせず、明示的な「取り消す」だけがclassify workerを止める。
- handler回帰はchecked優先 / selected fallback、latest revision、duplicate、実folder、source無変更、仮想項目と
  placeholderの全体拒否、import / export / relink owner保持、manager close 3段階detach、Conflict terminalと
  watch/catalog refreshを固定する。さらにeguiのprimary / secondary press-releaseを通して、コンボ選択、明示的な
  追加 / 開く、名前shortcutが相互発火しないことと、プルダウンでも追加 / 開くが残ることを固定する。
  管理windowのdark / light snapshotは旧picker撤去とtoolbar誘導文を固定する。
- focusedはcollection toolbar 5/5、toolbar add 5/5、target reconcile 1/1、collection handler / snapshot
  16/16、core checkがpassした。final `scripts/test-full.ps1 -SuppressCrashDialogs`はmain 8590 / 0 / 45、
  UI snapshot 52/52、IPC 57/57、
  Remote 122/122（1 ignored）、vendor egui / egui-wgpu / eframe 25 / 9 / 15、`[test-full] PASS`、exit 0で、
  process error modeを`0x00008001`へ復元した。fmt、UI glyph、viewer-context audit、diff checkもexit 0である。
- 独立Sol / xhigh reviewerはselection捕捉、latest revision、manager owner / close lifecycle、physical-onlyの
  全体拒否、source不変、actor terminal / watch収束、Remote非変更を再照合し、重要指摘なしで承認した。
  `scripts/build-dev.ps1 -PreserveRuntime`はexit 0で、動作中アプリを停止せず
  `target/dev-runtime/mimageviewer-core.exe`と`mimageviewer-remote.exe`を更新した。アプリは起動していない。

### 実機退行修正: 追加後のsurface復活と動画サムネイル（2026-09-16）

- portable実機で、いったんコレクションを開いた後にアドレス入力で物理フォルダへ移動し、別動画を
  toolbar追加すると、actor watch更新時にコレクションrootが再materializeされる事象を再現した。物理folderの
  visible loadを採用しても`TopLevelGridSurface::Collection`とsession/watchを退役していなかったことが原因で、
  toolbarのAdd intentやactor completionがOpenを発行したものではない。
- root Folder / ZIP / PDF open、物理子のdescendant open、same-folder sort / external reloadを
  `CollectionGridPhysicalLoadOwner`で型付けした。root originはsurface stamp、accepted / wanted revision、installed
  items generation、entry / source / target pathの完全一致を要求する。採用済みPhysicalSourceはroot anchorと
  current pathを要求し、child閲覧中のwanted revision進行とcollection削除を許容する。独立したアドレス・履歴・
  お気に入り・pane navigationはvisible load採用時だけCollectionをFolderへ退役させる。scan失敗、stale async
  candidate、scope拒否、変換取消ではsurfaceと旧itemsを分裂させない。
- コレクションrootの動画がPendingのままだった原因は、aggregate items install後に通常thumbnail poolとvideo
  workerを起動していなかったことだった。exact revisionのprepare workerがfull-path動画sidecarとvideo pinを
  一括準備し、watch再表示とlatest再生root着地の両方へ同じsnapshotを渡す。root installはviewer bundleの
  channel/cache/queueと通常workerを新generationへ作り直し、collection専用video workerだけをsession lifetimeで
  cancelする。別synthetic surfaceが再利用するbundle共有pool tokenはsession dropでcancelしない。
- Collection surfaceをfull-path thumbnail cache対象へ加え、別folderの同basename画像・動画sidecarを混同しない。
  Remote read-onlyもsidecar探索helperを共有するが、64 parent上限、入力順、scan失敗時のShell fallback、protocolを
  変更しない。
- focused回帰はcollection grid 18/18、collection navigation 15/15、aggregate / Remote 13/13、
  mixed folder 7/7、detached collection 2/2、folder candidate mode matrix 1/1、collection toolbar 6/6、
  toolbar add 5/5がpassした。実egui primary Addからproduct dispatch、actor terminal、catalog watch、pollまでを
  一続きに通す回帰でも、物理surface / folder / address / generation / selection / checked / scrollの維持と、
  OpenだけがCollectionへ遷移することを固定した。
- `scripts/test-full.ps1 -SuppressCrashDialogs`はmain / lib 8597 passed、0 failed、45 ignored、UI snapshot
  52/52、IPC 57/57、Remote 122 passed（1 ignored）、vendor egui / egui-wgpu / eframe 25 / 9 / 15で
  `[test-full] PASS`だった。core check、fmt、UI glyph、viewer-context audit、対象pathのdiff checkもpassした。
  独立reviewerはcache read / seed / pruneのfull-path一致、Folder / ZIP / PDF移行時のroot video worker cancel、
  bundle共有poolの継続、detached exact owner、実event sequenceを再確認し、重要指摘なしで承認した。
- `scripts/build-dev.ps1 -PreserveRuntime`と`prepare-portable-smoke.ps1 -TestScript`はアプリを起動・停止せず
  完了した。portableにはbaselineの隔離dataと、同basename・異sidecarを持つ2 folderのfixtureを復元した。
  修正版portableの実機再確認では、cold / 再訪時のcollection動画thumb、collection訪問後に物理folderへ戻って
  Addした場合のsurface / selection / scroll維持、Openだけのcollection遷移、別親にある同basename動画の各sidecar、
  collection内folderから親rootへの復帰、sidecarなし動画の実frame thumbがすべてpassした。

## 20. Collection履歴とroot参照解除（2026-09-16）

- folder履歴の正本を`FolderNavHistoryTarget`へ統合し、物理path、Rating、Smart Folder、Collectionを
  相互排他的なtyped entryとしてnormal / A / Bのback・forward stack、rollback snapshot、dispatchで共有した。
  Collectionはsynthetic pathへ投影せずstable IDとrestore hintを保持し、明示的なCollection Openだけを履歴へ
  記録する。物理loadはvisible adoption成功時だけ元Collectionを記録し、scan失敗、stale result、scope拒否では
  back / forwardを変更しない。Collection-owned child、reload、toolbar Add、管理window selectionは履歴を積まない。
- Back / ForwardでCollectionへ戻る時はIDからauthoritativeな最新catalogへ収束する。Ready catalogで削除済みのIDは
  normal / A / B stackとrollback installから除き、削除済みrootを表示中でもcurrent targetの再捕捉から履歴へ戻さない。
  Starting / Failed / Inert中の一時的なcatalog不在では履歴を消さず、renameはID一致で復元する。
- Collection rootの通常`Delete`、通常の「コレクションから外す」は、checked優先 / selected fallbackの参照を
  collection actorへRemoveとして送る。source file / folderのbytesは変更しない。root bindingが未installまたはstaleなら
  toastを出してfail closedとし、物理削除へfallbackしない。CollectionのPhysicalSource childと通常Folderは従来どおり
  実ファイル操作を使う。明示的なsource操作は「元ファイルをゴミ箱へ移動…」として区別した。
- Collection rootのWindows Shell機能は削除せず、global Inline設定にかかわらず
  「元ファイルのWindowsメニュー」submenuへ配置する。submenu内のProperties、関連付け、Shell Delete等は参照先sourceへ
  従来どおり作用し、collection actorへ誤routeしない。PhysicalSource childと通常Folderはglobal Inline / Submenu設定を
  維持する。definition delete、manager remove、import / export / relink、Remote read-onlyは変更していない。
- 回帰はphysical A→Collection C→physical BのBack / Forward、C1 / C2 identity、rename、Ready delete prune、
  snapshot rollback、failed load、normal / A / B、Rating / Smart restore、root checked / selected / unavailable、
  child physical delete、source bytes不変、root shell submenu、Shell Invoke非actorを固定した。直接`App` fixtureは
  crate-wideの`AppTestEnvForTest + setup_app_for_test`へ統一し、並列testがprocess-global settings DB / data-dir leaseを
  競合しないようにした。
- 検証正本は`target/collection-history-*.log`である。focused collectionは144 passed / 0 failed / 8508 filtered。
  final `scripts/test-full.ps1 -SuppressCrashDialogs`はmain / lib 8607 passed / 0 failed / 45 ignored、UI snapshot
  52/52、IPC 57/57、Remote 122 passed（1 ignored）、vendor egui / egui-wgpu / eframe 25 / 9 / 15、
  `[test-full] PASS`、exit 0で、process error modeを`0x00008001`へ復元した。fmt、UI glyph、
  viewer-context audit、core checkもexit 0で、UI glyphは0件だった。独立Sol / xhigh reviewerはtyped history、
  deleted-ID再混入防止、root delete / Shell owner、fixture lifetimeを限定再確認し、重要指摘なしで承認した。
- mImageViewer processが見えないことを`Get-Process`で確認した後、`scripts/build-dev.ps1 -PreserveRuntime`を実行し、
  exit 0、VCRT PE check 4 runtime / 2 PE passだった。core SHA-256は
  `5E2E0D9C8407B800D4C93574F3AF56898FC51FAF5274721FE8A2E682C873C12E`、Remoteは
  `8ABADDA66BC570D269ABB22439C2A6CA90964A5C2DF2239124CD81B01AEED0AD`。アプリ、通常profile、real dataは
  起動・操作していない。履歴 / Delete key / Windows submenuの実機確認は親の手動検収として未実施である。

## 21. 本棚同型のCollection管理とGrid起点操作（2026-09-16）

- `request_collection_grid_remove`が確認modalの表示場所を管理windowに依存させ、Grid起点Deleteでも
  `show_manager=true`と管理選択の切替を行っていた。operationのoriginを`Manager` / exact `Grid(stamp)`へ型付けし、
  modal、進捗、取消、terminal toastをmanager windowの寿命から分離した。Collection rootのDelete確認を開いても
  managerは非表示のままで、manager選択、追加先、表示surfaceを変更しない。
- 管理windowは本棚managerと同じ入れ物管理へ限定した。現在の追加先を冒頭へ表示し、inlineの新規作成、各行の
  名前変更、追加先指定、固定、削除、開くを同じ列で提供する。entry一覧は置かず、内容はメインGridを正本とする。
  definition削除は参照だけを削除し、source file / folderを変更しない。
- 上部に`コレクション`メニューを`製本`の隣へ追加した。保存済みの旧menu orderへは`製本`直後に補完する。
  メニューの`追加` / `開く`はglobalな追加先を対象とし、import / export、order mode / sort、専用並べ替え画面は
  現在表示中のexact Collection rootだけを対象とする。Grid requestはsurface stampとrevisionを運び、latest/full actor
  snapshotを読んでから専用画面を開く。Standard表示中、stale surface、対象なしは理由付きで無効にする。
- toolbarは本棚と同じく全catalogの追加先comboと明示的な`追加` / `開く`を常設し、固定したstable UUIDだけを
  左Open / 右Addのshortcutとして表示する。表示形式は`展開` / `折りたたみ`の2種類とし、旧`プルダウン` / unknown
  保存値は読込時に`展開`へ正規化する。追加先と固定列はauthoritative Ready catalogだけでorder-preserving dedupe / prune
  し、Starting / Failed / 一時的なcatalog欠落では消さない。renameはUUIDで追従し、削除時の追加先fallbackは1か所で行う。
- import preview、Grid Remove確認、分類・export進捗はmanagerを開かず表示できる。managerを閉じてもGrid / Toolbar起点workerを
  cancelせず、明示取消だけがそのoriginのworkerを止める。classification完了時もexact Grid stampを再検証し、別surfaceへ
  移った後のlate resultをactorへ送らない。actor成功後のGrid / Remote収束は既存revision watchを維持する。
- 本棚はページ画像を本フォルダへコピーする製本機能、Collectionは既存file / folder / book / ZIP / PDFへの参照であり、
  データモデルは統合しない。virtual pageを親pathへ丸めず、未対応対象は理由付きで無効にする。import / export、
  manual order、履歴、再生、Remote read-onlyの既存契約を維持する。参照再リンクはUIから撤去し、登録解除→再追加を案内する。
- 検証は`collections` 63件、collection toolbar 8件、menu layout 13件、旧menu補完1件、追加のpin順2件と
  modal input gate 1件をfocusedで通した。`test-full.ps1 -SuppressCrashDialogs`は本体8613件成功・0失敗・45 ignored、
  vendor egui / egui-wgpu / eframeは25 / 9 / 15件成功し`[test-full] PASS`だった。初回fullで既存navigation testが
  subscribe時のbaseline noticeをrename noticeと誤認するraceを1件検出したため、baselineを明示検証・消費するfixtureへ
  修正し、focusedと再fullで成功を確認した。fmt、UI glyph、viewer-context audit、対象pathのdiff checkもexit 0。
  manager / import preview / manager非表示のGrid Remove確認snapshot 3枚は親が目視受入し、独立reviewerはtyped origin、
  latest/full actor snapshot、manager / global add target / current Gridの分離、stable pin順、modal input gateを確認して
  重要指摘なしとした。`build-dev.ps1 -PreserveRuntime`はresident不在を確認してcore / Remote serviceとVCRT PE検査を
  exit 0で完了した。通常profile、real data、GUIアプリは起動・操作していない。

## 22. 再生navigationの同一root再利用と専用並べ替え画面（2026-09-16）

- linked別窓のnext / prev / EOFでactor prepareが既存rootと完全一致するとき、target / originだけをinstalled binding上で
  解決し、Gridを再installしない。presentation identityはordered entry ID / source key / path / availability / item /
  display metadataと、prepare workerがsidecar path・metadataおよびpin blob SHA-256から作る固定長identityを一つに所有する。
  pin blob payloadはinstalled bindingへ複製せずthumbnail workerへmoveする。filesystem facts、thumbnail source、revision、順序、sourceが
  変わった場合は従来どおり再installし、latestを採用する。これによりmain一覧のgeneration、Autoサムネ比率、scroll、
  thumbnail worker / queue / cacheをviewer cursor移動で初期化しない。
- 手動順は上部`コレクション`メニューの「現在のコレクションを並べ替え…」から専用サムネイルwindowを開く。本棚と
  同じ単一 / Ctrl / Shift複数選択、group drag、挿入marker、edge auto-scroll、左右移動、thumb size、hover previewを
  提供する。actor snapshotのfull manual entry列を正本とし、filter / Standard表示順やEntryId sortから順序を作らない。
  dirty closeは保存完了までwindow ownerを保持し、Conflict / errorでは編集中の順を保持する。Conflictは再試行せず、
  「最新内容を読み直す（変更破棄）」または「変更を破棄して閉じる」で明示的に回復する。
- `選択中の参照を再リンク`はメニュー / dispatch UIから撤去した。legacy saved menu IDは読取互換のためparseできるが
  描画しない。既存entry / missing表示、store migration、import / export、remove→add、source非変更、Remote read-onlyは
  維持する。参照先を変える利用者操作は登録解除→再追加とする。
- focusedはcollection navigation 20 / 20、collection Grid 22 / 22、専用並べ替え・管理 27 / 27、legacy relink
  menu互換 1 / 1が成功した。`scripts/test-full.ps1 -SuppressCrashDialogs`は本体8625件成功・0失敗・45 ignored、
  UI snapshot 52 / 52、IPC 57 / 57、Remote 122件成功（1 ignored）、vendor egui / egui-wgpu / eframe
  25 / 9 / 15で`[test-full] PASS`、exit 0だった。core check、fmt、UI glyph（0件）、viewer-context audit、
  diff checkもexit 0。独立reviewerはcomplete presentation / thumbnail source identity、payload move、actual poll、
  split context、D&D conflict回復を再確認し、重要指摘なしで承認した。
- resident mImageViewer / core / Remote processが見えないことを確認し、`scripts/build-dev.ps1 -PreserveRuntime`を実行した。
  core / RemoteとVCRT PE検査（runtime 4 / PE 2）はexit 0。core SHA-256は
  `B9B6EB86FAEDB00124AB8E0B111A432B08285DA707080EED04DFC608D77B9FE6`、Remoteは
  `8ABADDA66BC570D269ABB22439C2A6CA90964A5C2DF2239124CD81B01AEED0AD`。通常profile / real data / GUIは起動・
  操作していない。専用並べ替えwindowの実動画サムネイル、実ドラッグ、linked別窓動画nextは利用者確認へ引き渡した。
- 2026-09-16、上記確認用ビルドの引き渡し後、利用者から「治りました。コミットお願いします。」との実機確認・
  コミット承認を受領した。個々の操作項目についての詳細な結果は未採取であり、agentによる実機検証とは区別する。

## 23. v4.0.0 出荷前レビュー後の修正計画（2026-09-16）

状態: §23.1〜23.4、§23.5（M-1）、§23.6、A-6、D-4、§23.7 前半（登録上限・import資源上限・preview仮想化）は実装・自動検証・Codex 独立レビュー完了。同節のprepare再利用は未着手。§23.4 / §23.6 / A-6 / D-4 は実機確認待ち。A-6 / D-4 の統合 full gate / build は2026-09-20に通過した。§23.7 前半のfull gate / buildは後続prepare再利用と統合する。§23.8 の移行前 DB 保護だけ先行し、ほかの残件は未着手。実装は Codex、出荷前の ClaudeCode レビュー指摘を修正中。2026-09-17 に仕様判断 2（シャッフル方式）・3（上限 10,000 件）・5（バックアップ 2 段）を利用者が確定。指摘 ID は
[docs/review-v4.0.0/README.md](review-v4.0.0/README.md) と同フォルダの A〜E 報告書を指す。
利用者の判断は [仕様案「利用者の判断（2026-09-16）」](collection-spec-proposal.md#利用者の判断2026-09-16v400-出荷前レビュー後)
が正本。修正ごとに handler-level / 状態遷移テストを付け、着手前に §13 不変条件と review の
「監査したが問題なし」節を読む。

### 23.1 計装を先に入れる（B-6）

`open_collection_grid` / prepare / navigation / import 解析・分類 / export / source migration の各区間に
`perf::event` を差し、修正前後を `--perf-log` の同じ指標で測れるようにする。

#### §23.1 実装記録（2026-09-19）

`cat="collection"` を追加した。`open` は選択と context、`actor_rtt` は manager / Grid /
Remote の catalog または snapshot 応答（enqueue から受信まで）、`prepare` は Grid の
`classify` / `sidecar_scan` / `pin_db` / `identity` 各段階、`install` は UI 適用を計る。
`navigation_begin` / `navigation_decision` / `navigation_root_install` / `navigation_prepare` /
`preflight` は既存 intent sequence と collection ID / revision で関連を追う。再試行は同じ
intent sequence に複数の開始があり、完了が無い開始を成功や滞留と断定しない。
`navigation_decision` は対象を選んだ段階、`navigation_root_install` は root の採用段階を表す。
`import_parse` / `import_classify` / `export_prepare` /
`export` / `migrate` はそれぞれ件数、所要 ms、結果を記録する。`remote_prepare` は Remote
の同じ分類区間を記録する。結果には `ok`、`cancelled`、`stale`、`error`、`disconnected`、
`timeout` 等の区別を入れ、個々の source path は記録しない。Remote の `actor_rtt.reply_ok` は
受信成功で、revision 採用は `remote_exact` に分ける。計測専用の開始時刻以外に
request ownership、worker の配置、deadline、取消、表示順は変更しない。`--perf-log` 無効時は
時計取得と JSON 構築を避ける。

### 23.2 ソート UI（A-1 / A-2、案 C。仕様判断 1）

- `top_level_grid_view` の Collection Root / PhysicalSource 位置、installed presentation の revision / order mode、
  viewer context と surface stamp に束ねた typed order 対象を正本にする。`grid_sort_lock_reason` /
  `details_header_sort_active` / `apply_sort_change_reload` / `main_window_title`（A-5）へ通す。
  loading / stale / failed / deleted はそれぞれの理由で無効にし、独立更新される catalog の定義値を
  表示中の順序正本にしない。新しい `items_are_collection_view` bool は足さない。
- ツールバー Buttons / Dropdown と表示メニューの選択肢に「手動順」「シャッフル」を追加し、Collection root では
  定義の `order_mode` / `standard_sort` を選択表示する。Collection 分岐では `settings.sort_order` を書かず
  `CollectionGridSnapshotAction::SetOrder` を送る。`apply_sort_change_reload` は Collection で早期 return。
- Manual / Shuffle では列ヘッダだけを固定し、ツールバー / 表示メニューは選択可能に保つ。
  Standard の列ヘッダソートは既存どおり表示限定で、列ヘッダ所有中はツールバー / 表示メニューを無効にする。
  Collection root の reader（見開き、seek、Home / End、slideshow 等）は installed items の有効順を
  local filter で可視投影した列を使い、列ヘッダ順は一覧の描画・選択にだけ使う。PhysicalSource 子と
  通常一覧の reader は従来の表示順を保つ。
  `open_collection_grid` と履歴からの復帰、同じ root の order 切替で列ソートを解除し、items と新order公開後に
  details 表示順を再構築する。ZIP / PDF / 自動画像本の `page_order_locked_for_current_view()` は
  列ヘッダと details 並びの共通consumerにも使い、本内ページ順を固定する（§1.244）。
- 上部「コレクション」メニューの「並び順」は残し、同じ installed root 述語から表示を導く。
  このメニューだけは Standard の列ヘッダ所有中も order を選べ、成功採用時に列ソートを解除する。
  loading / stale / failed / deleted で無効のときは理由を出す（A-9）。
- A-6「元の場所へ移動」はこのsort chunkから分離する。既存 `begin_context_jump_to_folder` は開始時にrootを
  破棄するため流用せず、後続の独立physical jump chunkで stamped scan owner を持ち、成功時だけrootを退役、
  失敗・取消・遅着では元rootと履歴を保つ。`ContextMenuViewFlags` / `can_jump_to_folder` の変更もそこで行う。
- 回帰: review README §2.2 の (a)〜(e)。

### 23.3 シャッフル順（仕様判断 2、設計は仕様案「シャッフル順の設計」）

`CollectionOrderMode::Shuffle` + `shuffle_seed` を model / db / prepare / UI / Remote wire へ通す。
有効順は `SHA-256(seed の little-endian 8 bytes || entry ID の UUID raw 16 bytes)` の digest を
辞書順で昇順、同値なら UUID raw bytes 昇順にする。再選択で seed 更新。reducer は変更しない。
DB v1→v2 は全collection / entry / revisionを保持する単一transaction移行とし、その前に既存の世代
バックアップ関数でWAL込みsnapshotを保存する。seed は16桁のhex TEXTでu64全域を保持する。
Remote IPCは56→57、WebはShuffleと表示する。Standardのfacts集合照合は同じ契約のままO(N)へする。

#### §23.2 / §23.3 実装記録（2026-09-19）

- Collection Root の order UI は installed session の identity / items generation / accepted revision と
  immutable prepared snapshot を正本にした。loading / stale / failed / deleted を理由付きで無効にし、
  SetOrder の actor snapshot 採用にも captured revision の exact check を加えた。既存の
  Import / Export の relaxed revision 契約は変えない。Manual / Shuffle は列ヘッダだけ固定し、
  上部 Collection メニューは列ヘッダ所有中も選択可能にした。order 採用時と root open / 履歴復帰時は
  列ヘッダを戻し、新しい items / order の後に details 行順を再構築する。通常一覧の global sort は変更しない。
- Standard root の列ヘッダ順は一覧表示・選択専用とし、reader / 見開き / seek / Home / End /
  slideshow / native video は installed 有効順の local-filter 可視投影を使う。PhysicalSource 子と
  通常一覧の reader は既存の表示順を使う。ZIP / PDF / 画像本の列ヘッダ・details order は
  共通の `page_order_locked_for_current_view()` から固定し、本内ページ順を守る（§1.244）。
- Shuffle は seed と entry ID の固定 SHA-256 順を model / DB / prepare / UI / Remote に通した。
  同モード再選択でも新 seed と revision を発行し、manual position は保持する。DB v1→v2 は
  WAL を含む世代バックアップを移行前に保存してから単一 transaction で全 collection / entry を保持する。
  seed の16桁 hex TEXT は u64 全域を往復し、Remote IPC は57、Webは「シャッフル」を表示する。
  Standard facts の集合照合は同じ契約のまま HashSet で O(N) にした。
- focused collection 168/168、Remote IPC Shuffle wire、Web 123/123、
  `scripts/test-full.ps1 -SuppressCrashDialogs` の `[test-full] PASS` / exit 0、
  core check、viewer-context audit、fmt、UI glyph、diff check が成功した。
  `scripts/build-dev.ps1 -PreserveRuntime` は居残りプロセス不在を確認して成功し、core / Remote を
  `target/dev-runtime` に生成した。証跡は `target/collection-sort-verify-20260919/`。
  Codex独立 reviewer は §23.2/3 と §1.244 の差分・検証を確認し、重大な残件なしと判断した。
  GUI / 実データは操作していない。
- A-6「元の場所へ移動」はこの chunk に含めず、root を成功前に破棄しない独立 physical jump として
  後続で設計・実装する。§23.8 の全件書き出しも未実装で、ここでは schema 移行前の DB 保護だけを先行した。

#### §23.2 / §23.3 追加の利用者報告（2026-09-20）

- 利用者は、再生中に Shuffle へ切り替えるとソート表示が「更新中」のままになり、次の項目まで
  一覧へ反映されないと報告した。コード上、`poll_collection_grid` は fullscreen leaf が開いている間、
  表示中の `items` と index の結び付きを守るため root の再 install を保留する。現在の root の
  ID・revision・items generation・件数が installed snapshot と一致し、更新が保留されている場合は、
  ソート選択の無効理由を「反映待ち」と表示する。説明文は、表示中の項目を閉じると一覧更新が再開し、
  次の項目への移動が成功した場合も latest order を採用することを示す。初回読込は「更新中」、
  削除・読込失敗は各専用理由を優先する。再生中の root install 条件と navigation owner は変えない。
- 利用者は、ソート Dropdown の長い Shuffle 項目に scrollbar が重なると報告した。
  アプリ共通の floating scrollbar 予約幅は 0 で、egui ComboBox は content closure より前に
  ScrollArea を作る。ソート popup の `popup_style` に既定 `menu_style` と scrollbar 最大幅分の
  局所 gutter を設定し、他の一覧・popup の style は変えない。
- headless 回帰は fullscreen root の Shuffle notice→保留表示→close 後の採用と、Popup の
  予約幅・既定メニュー装飾を対象とし、各 1/1 成功。Collection Auto 比率 cache と
  集約した全体検証・確認 build の結果は以下へ記録する。
- Collection root の Auto 比率は UUID 専用 table と App 全体のメモリ値から復元する。
  空 install は同一プロセスの値を使い、初回起動時は prepare worker が 100 ms 上限の
  actor Get 結果を accepted 実 rows install に渡す。確定値の非同期保存は exact Ready root
  の ID・generation・revision・件数に限定する。音声と欠損 placeholder は root の
  sample 可能母数から除外し、通常 folder / PhysicalSource 子の既存経路は変えない。
  管理画面の件数・全件削除・期限整理は folder / Collection の結果を合算し、UUID 側の
  read/write/cleanup は actor だけが扱う。actor 不調でも従来の folder/catalog/tile 処理は
  続け、合算件数は不明、全件削除は部分失敗として表示する。Get と clear の受付・epoch は
  短い lock 内で直列化し、clear 前に発行した遅着結果を採用しない。詳細は
  `auto-thumb-aspect-plan.md` §13。
- focused は Auto 比率関連 38/38、actor 不調時の既存 cache 管理継続 1/1、
  上記 UI 回帰各 1/1 が成功した。`scripts/test-full.ps1 -SuppressCrashDialogs` は
  `[test-full] PASS` / exit 0。core check、`cargo fmt --all --check`、UI glyph、
  `git diff --check` も exit 0。証跡は
  `target/collection-auto-aspect-verify-20260920/test-full.log`（PowerShell transcript は
  native cargo 出力を省略するが、各 gate 名と PASS を記録）に保存した。
  居残り mImageViewer プロセス不在を `Get-Process` で確認してから
  `scripts/build-dev.ps1 -PreserveRuntime` を実行し、exit 0 で core / Remote を
  `target/dev-runtime` に生成した。core SHA-256 は
  `89C5FEAF0EFBC5BC560CF9A5A7A7FB40E647B1B13EAE277B067D2502851E5455`、
  Remote は `60F4EFD348C5AD5A1963BD61B7D8DD79855CA4C707B275C901324C9247793AA9`。
  独立 reviewer は UUID actor の順序・部分失敗と UI 差分に追加 blocking 所見なしと判断した。
  GUI / 実データ操作は行っていない。
  2026-09-20、利用者が確認用ビルドで「治りました」と回答し、追加報告3点の実機確認を完了した。
  これは当該3点の確認であり、§23.4以降やD-4を含むレビュー全体の完了を意味しない。

### 23.4 参照解除の非対称（A-3 / C-11）

`CollectionRootDeleteResolution::Unavailable(reason)` のとき `RemoveFromCollection` を無効項目 +
`disabled_reason` として残す（`MenuNode::Item { enabled, disabled_reason }` の前例）。

実装・自動検証完了（2026-09-20、実機確認待ち）。Grid/Fullscreen の単一選択・複数選択で同じ typed resolution から
参照解除の表示、無効化、理由を導く。Delete キーの既存理由通知も維持する。
`MoveToRecycleBin` は捕捉済みの実ファイル target を持つ別操作であり、参照解除が
Unavailable でも既存の明示削除契約を変更しない。model と menu handler の焦点回帰、
全体 gate、独立コードレビュー、確認ビルドは成功した。検証結果は §23.6 に集約する。

### 23.5 rename migration scope（M-1）

`poll_rename_pending` が読む `rename_target_is_file` を `clear_rename_dialog_state()` より前に確定させるか、
`rename_pending` に scope を同梱する。既存テストは `spawn_rename_key_migration` を直接呼び配線を迂回して
いるので、dialog → poll を通す handler-level 回帰を 1 件足す。

2026-09-20、M-2 と同じ Shell rename の所有境界で実装。`RenamePending` が receiver と
開始時の Exact / Tree を所有し、ダイアログの消去や Empty poll で scope を失わない。
焦点回帰と独立コードレビューは成功し、最終の本体全回帰・確認ビルドも成功した。
検証の正本は [復旧記録の保護](collection-migration-journal-recovery.md)。

### 23.6 一過性状態の typed 化（C-1 / C-2 / C-3 / B-3）

read 経路の入口を `collection_store_client_for_migration` と同じ `Result<Option<_>, CollectionStoreError>` へ揃え、
`Busy` / `Starting` は終端 `Failed` や無言 drop にせず `RequestNeeded` + `request_repaint_after`
で再駆動する。並べ替え保存中も read は編集可否から独立し、actor が受け付ければ進む。
Grid の `Snapshot` / `Preparing` と期限付き再受付は、表示中の root に限って
遅延 poll とし、tail の即時 repaint reasons には入れない。即時 repaint によって遅延予約が
失われ得るため、各 pass で残り時間を再予約する。

実装・自動検証完了（2026-09-20、実機確認待ち）。read client は Manager の編集可否と切り離し、Grid は既存 session の
`RequestNeeded` に次回受付時刻を保持する。Manager の catalog / selected snapshot は
`Idle → RequestNeeded → InFlight` の単一 owner で明示 refresh と新しい需要を保存し、
Busy/Starting 後に同じ revision の要求も落とさない。window を閉じても runtime/catalog の
需要は消さず、選択・runtime の変更時に旧 snapshot 要求を退役する。navigation は既存 pending に
`RequestNeeded` を加え、subscribe 済み watch と source/intent を保ったまま load Busy を待つ。
繰り返し届く同一 slideshow / EOF 通知は既存要求の期限を伸ばさず、別 action/serial は別意図として
扱う。terminal read error は真の末尾として表示しない。新規・関連焦点回帰 158 件、
本体 lib 8,750 件（45 件除外）を含む `scripts/test-full.ps1 -SuppressCrashDialogs`、
`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo run --locked -p viewer_context_audit`、
`cargo fmt --all -- --check`、UI glyph と差分検査は成功した。独立コードレビューに blocking 所見なし。
`scripts/build-dev.ps1 -PreserveRuntime` も成功し、core SHA-256 は
`F89E782A2BB6BC09E048075CA4EF4376887B1E5BB4A8743E0C88FAD0038772BB`。
自動検証ログは `target/section23-full-gate-20260920.txt`、ビルドログは
`target/section23-build-20260920.txt`。GUI 起動・実データ操作は行っていない。

この変更でコード上の一過性 admission 経路を扱うが、利用者が報告した一度だけの
「ぱらどっとが読み込み中で止まった」事象は再現・原因特定されておらず、解決済みとは扱わない。

### 23.6A コレクションから元の場所へ移動（A-6）

コレクション root のセルメニューに「元の場所へ移動」を追加した。Folder 本体も含め、元 source の
親ディレクトリを worker で走査し、元項目の実パスを exact selection として運ぶ。
`JumpToFolderRequest` と既存 `FolderOpenScanPurpose::JumpToPhysicalFolder` が
`CollectionGridPhysicalLoadOwner::Root` を開始から完了まで所有し、surface / context / revision /
entry / source / items generation を開始時と ready 時に再検証する。Collection root の items、選択、
scroll、address、history、cache は走査中に保持し、失敗・断線・取消・置換・stale・scope 拒否では
退役しない。成功採用は `OpenRequestOwner::Navigation` に限り、物理 Folder として Collection→Path
履歴を作る。元項目が走査後の一覧から消えた、または表示設定で除外されたときも親 Folder への移動は
成功とし、既存の exact 不在通知を出して別項目を誤選択しない。

grid メニューの保存対象は index 単独から context と items generation を持つ owner へ替えた。
Collection の再 install は同じ context / generation の古いメニューだけを失効させ、別 viewer の
メニューは保持する。A-6 のメニュー可否は元 source owner の実行前判定から導き、revision 更新待ち
などは理由付きで無効化する。表示後に owner が失効した押下には再選択案内を出す。
参照解除の可否は変更しない。Collection Fullscreen にはこの Grid 専用入口を出さない。
Search / 閲覧履歴の既存 Jump は開始時に source を退役する従来契約を維持する。
Windows の ready / scan error、共通 ready の成功・取消・revision / items / context stale、
元項目不在、履歴、別 quick-folder 履歴、メニュー世代と既存 Search Jump の焦点回帰を通した。
2026-09-20、独立 reviewer は追加 blocking なしで A-6 完成差分を受理した。
`cargo check -p mimageviewer --bin mimageviewer-core`、焦点テスト（location 3、menu model 1、
handler 1、既存 Search Jump 3）、`cargo fmt --all -- --check`、UI glyph、`git diff --check` は通過した。
後続D-4と統合した full gate / `build-dev.ps1 -PreserveRuntime` は後述のとおり通過した。
GUI 起動・通常 profile / 実データ操作は行っていない。

### 23.7 上限と大量件数（仕様判断 3、B-1 / B-4 / D-2）

- 1 コレクション 10,000 件の定数を model に置き、`add_batch` / import の actor 側で typed に拒否する。
  Remote の永続 root entry prefix は同じ定数から導く。catalog 100,000 件と既存 aggregate 100,000 件は別上限とする。
- import 確認画面を `show_rows` で仮想化し、worker の実読込を 32 MiB + 1 byte に制限する。
  pure `parse_collection_text` も 32 MiB と非空 50,000 行を独立に拒否し、資源超過時は preview 全体を採用しない。
  これらの資源上限は 1 コレクション 10,000 件の登録上限とは別である。

2026-09-20、前半を実装。actor transaction は `COUNT(*)` を正本として既存/同batch重複を先に分類し、
入力順で残容量へ追加する。結果は追加・重複・容量拒否の3件数を表示し、追加0件ならrevision / 通知を
進めない。既存の上限超過データは読取・削除・手動並替を維持する。importは資源超過をpreview全体拒否とし、
無効パス時のall-or-none分類、確認前の対象path非アクセスを維持する。長いpathは固定高の可視行だけ省略表示し、
hoverで全文を確認できる。Remote rootは10,000件またはwire予算内のdisplay unit境界prefixを返し、
full eligible / seek / totalは旧上限超過データを含むexact全件のままにする。
焦点検証はstore 24件（容量境界、旧上限超過、SQL rollback、資源境界を含む）、Remote prefix 1件、
worker実読込 1件、長path preview snapshot 1件を通過し、snapshot PNGを目視確認した。
bin check / fmt / UI glyph / viewer context audit / diff checkも通過。独立reviewerは追加blockingなしで
前半差分を受理した。GUIは起動していない。
- navigation の prepare は、actor revision が installed と一致し installed presentation が存在する場合、
  選ばれた target entry（と隣接数件）だけ availability を確認し、全件 stat は revision 前進と明示更新に限る。
  Remote の `persistent_collections.rs` も同じ helper を使う。
  2026-09-20、利用者は速度優先で、外部ツールによる変更の自動反映を保証せず、mIV が認識した
  一覧・並び順を保持する方針を承認した。mIV 内の編集・名前変更への追従と、実際の移動先・Remote
  公開範囲の検証は維持する。再取得契機は後続の実装設計で明記する（本項は未実装）。

### 23.8 バックアップと全件書き出し（仕様判断 5、K-1）

- actor 起動時に `db_backup::rotate_generation_backups(data_dir, "collection.db", ..)` を tags.db と同じ
  `rotate_backups_once` パターンで 1 回回す。settings-family lease には含めない（§3.3）。
- 上部「コレクション」メニューに「すべてのコレクションをテキストで書き出す…」を追加する。フォルダ選択 →
  1 コレクション 1 ファイル（`<name>.txt`、名前衝突は連番）+ `collections-index.txt`（名前・並び順・件数）。
  既存の export serializer と worker を流用し、開始時の immutable catalog / snapshot を固定する。

### 23.9 キー操作（A-4）

`GridAddToCollectionTarget` / `FsAddToCollectionTarget` / `VideoAddToCollectionTarget` を `KeyAction` に追加し、
`ini_name()` / `context()` / `trigger()` / `default_chords()`（空でよい）/ `ALL_ACTIONS` / helper /
`docs/keymap.ini.default` を揃える。リングショートカットへの追加は任意。

### 23.10 文書（E-1〜E-7、E-10 / E-11 / E-21、仕様判断 6）

README v4.0.0 節（E 報告書 §A の下書きを Phase 0 の利用者承認へ）、`manual/collections.html` +
`tut-collections.html` 新設とサイドバー 30 リンク同期、`shortcuts.html` の Delete 説明、`remote.html`、
privacy.html / 製品ページの保存データ列挙、移行ガイドの「仮想フォルダ / プレイリスト → コレクション」、
`version_highlights` 4.0.0 節、`architecture-overview` / `async-architecture` / `virtual-folders` /
`keymap-spec` / `spec.md` の同時更新。

### 23.11 v4.0.x 以降へ送るもの

B-2（revision 前進時の再 install 抑制）、B-5（migration の M×N）、D-1（Remote レーン分離）、D-3、
A-8〜A-18 の P3、M3U 対応、登録順ソート、D&D 追加、件数表示、終了時の自動書き出し、通常フォルダの
セッション限定シャッフル。前提件数（10,000）は known-issues と本書に明記する。

D-4 は 2026-09-19 に利用者が出荷前修正へ戻すことを承認し、2026-09-20 に
§23.2 / §23.3 とは別の後続 chunk として実装・焦点検証・独立レビューを完了した。
request の `target_kind` は探索用に維持し、着地した実媒体の typed `position` を exact Remote 公開列から返す。
protocol v57 → v58。candidate本体と画像group pageの公開可否を同じ列で照合し、対象消失時の
partner誤着地を拒否する。HTTPの画像partner昇格はchecked ordinal補正、Webの画像seekは
`still_image` 位置だけを使う。core焦点 8、IPC 3、HTTP 1、Web 409件が通過し、
`cargo check`、fmt、UI glyph、diff-checkも通過した。A-6と統合した全体gate / 確認用buildも通過した。

2026-09-20の統合checkpoint: `RUST_TEST_THREADS=1`で
`scripts/test-full.ps1 -SuppressCrashDialogs` は `[test-full] PASS`、exit 0。
UI snapshotは53/53、vendor egui / egui-wgpu / eframeは25 / 9 / 15で、process error modeは
`0x00008001`に復元した。Web全体のNodeテストは409/409。
`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --all -- --check`、
`python scripts/check_ui_glyphs.py`（危険glyph 0）、`cargo run --locked -p viewer_context_audit --quiet`、
`git diff --check` はすべてexit 0。mimageviewer / core / remoteのresident不在確認後、
`scripts/build-dev.ps1 -PreserveRuntime` はcore / RemoteとVCRT PE検査（runtime 4 / PE 2）でexit 0。
core SHA-256は`63143304F2CAEF6B5EE2AC8AB921C65BA2F80F8ED1C26443EFE2CAB0855FFBE2`、
Remoteは`9FBF80F94403CDF78E8698A0BD31271B1B874DAEB8D5C7063EA6F780BEEC45E5`。
GUIと通常profile / 実データは起動・操作していない。

M-2 も 2026-09-20 に利用者が修正を承認し、出荷前対象へ戻した。読み取り失敗を
正常な空と区別し、旧記録の保全と新しい保存・移行・終了の所有境界を修正した。
同日、読み取り失敗時に限って名前変更・本棚の移動/並べ替え・削除を物理変更前に停止し、理由を
表示する方針も利用者が承認した。閲覧と通常時の操作を維持し、旧記録の削除・上書きを禁止する。
実装・検証は Sol xhigh、独立設計/完成レビューは別の Sol xhigh が担当する。実機テストは未承認。
独立設計レビューで、読み取り失敗と保存失敗を区別する admission、全物理変更入口の事前判定、
成功済み遅延結果の保持、再読込後の FIFO 統合と保存 ACK、終了時の旧記録保護に合意した。
本棚 Copy も元ページの番号変更を伴うため対象とし、純粋な追加コピーは維持する。
同じ Shell rename の pending 所有境界を触るため、§23.5 の M-1（開始時の Exact / Tree 捕捉）も
このまとまりで実装・検証する。初回読込は既存の一回から増やさず、明示再読込は非同期とする。
実装・独立コードレビュー・自動検証・確認ビルドを完了した。同日、利用者から通常の名前変更への
追従は正常との確認を受けた。読取失敗・再読込は自動検証済みで、実データを壊す試験は行っていない。
処理中の削除対象への generic migration の
新規開始を既存の pending owner で保留し、完全成功した削除結果だけを invalidation する。
本棚の部分削除失敗は別の設計を要するため、残件を保留バックログ §1.258 に記録した。
検証結果・既存の保存失敗に関する範囲外事項は
[復旧記録の保護](collection-migration-journal-recovery.md) を正本とする。
全体 gate は成功し、最後の再読込開始時 repaint 追加後に本体全回帰 8,686 件成功 / 45 件除外、
焦点 recovery 17 件成功で再検証した。無変更の workspace / vendor は直前の全体 gate を再利用。
fmt / UI glyph / diff check と `scripts/build-dev.ps1 -PreserveRuntime` はすべて exit 0。
core SHA-256: `F64B64B514E638D7F23F1AA7CB9B52F7AD07D464A3B5B35B57F553FF189D2F28`。
Remote SHA-256: `60F4EFD348C5AD5A1963BD61B7D8DD79855CA4C707B275C901324C9247793AA9`。
