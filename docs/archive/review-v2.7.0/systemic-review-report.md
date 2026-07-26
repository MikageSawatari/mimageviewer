# v2.7.0 出荷前 横断レビュー結果

作成: 2026-07-23
コード比較基準: `v2.6.0` (`0d504f6d`) .. `a963317b`
レビュー計画: [systemic-review-plan.md](systemic-review-plan.md)

## 結論

コード対象を `0d504f6d..a963317b` に固定し、最新のbookmark detached修正を起点に、
context ownership、非同期request、cache invalidation、入力経路、同型のtop-level viewを
横断して再レビューした。

**レビュー時点の未解決はP2が1件。** slow pathのbookmark openを通常navigationまたはactivationが置き換える場合、
path resolverとbookmark requestの解消が一体化されていない。手動navigation後に古いbookmarkが
遅れて開く、またはactivation後にold pendingが残る可能性があるため、コード出荷判定は保留とする。

それ以外の最新bookmark routing、detached handoff、main thumbnail/cache ownership、metadata
import/export、mipmap、details/rating/music metadataでは追加指摘はない。既レビュー指摘の修正も維持され、
focused test、全library test、全main binary test、check、format、UI文字検査はすべて成功した。

## 指摘一覧

### [P2] bookmark path resolverとopen requestが別々に置換・cancelされる

- 場所: `src/app/startup_ops.rs:25-38`, `src/app/startup_ops.rs:90-176`,
  `src/app.rs:14452-14484`, `src/app.rs:24254-24325`, `src/app.rs:24420-24586`
- 破れている不変条件: 新しいnavigation / activationがopen要求を置き換えた後、古い要求は現在の
  表示を変更せず、request、view state、resolver、repaint待ちを同じownership境界で終了する。
- 根本原因: `PendingBookmarkOpen::{Media, Book}`はmedia/bookの同時保持を防ぐが、
  `StartupOpenPathResolvePending`とは別のApp fieldであり、両者を結ぶrequest identityがない。
  `start_startup_open_path_resolve`は旧resolverだけをdrop/cancelし、対応するbookmark pendingと
  `BookmarkViewState`のdispositionを決めない。通常の`load_folder_with_scan`もview stateは照合するが、
  in-flight resolverと`bookmark_open_pending`を終了しない。
- 影響1: network path等のbookmark resolve中に通常navigationするとresolverは生存する。完了時には
  detached bookmark用のtarget照合が外れてもgeneric Bookmark openへfall throughするため、ユーザーが
  後から選んだ場所を古いbookmark containerで上書きし得る。
- 影響2: bookmark resolve中にactivationが入ると旧resolverはcancelされるが、Book pendingは
  `Resolving`のままtimeoutせず残る。`poll_bookmark_browser`は50ms repaintを継続する。Media pendingは
  30秒後に無関係な失敗toastを出す。
- 同型検索: bookmark A→Bは新resolverのstate-local receiverへ置き換わるため旧resultは適用されない。
  archive変換も二重起動を拒否し、各`ArchiveConvertState`が固有receiverを所有するため、当初疑った
  AのcompletionがBへ混線する経路は成立しない。metadata transfer、details lazy metadata、thumbnail
  worker、top-level search/smart folderにもgenerationまたはstate-local receiverがあり、同型指摘なし。

### 必要な修正

1. bookmark open requestへ単調なrequest IDまたは同等のtyped ownerを持たせ、path resolverまで同じ
   requestに含める。
2. bookmark A→B、activation、通常navigation、cancel、disconnect、timeoutの各遷移で、旧IDに属する
   resolver / pending / view stateだけを原子的に終了する。新しいBを旧Aのcancelで消さない。
3. completionはrequest IDとtarget identityが現在のrequestに一致する場合だけ適用する。
4. Bookの`Resolving`にも終了条件を持たせ、ownerのないpendingがrepaintを継続しないようにする。

### 必須回帰テスト

- slow bookmark resolve → 通常folder navigation → late resultでも現在folderが変わらない。
- slow bookmark resolve → activation → 旧bookmark pending/view stateが残らずactivationだけが作用する。
- bookmark A → bookmark BではBだけが開き、AのcancelがBを消さない。
- Media / Bookそれぞれでcancel、worker disconnect、timeout後にpendingとrepaint理由が残らない。

## 最新bookmark修正の確認

### routing / input parity

- `PendingBookmarkOpen::{Media, Book}`でmedia/book requestを排他化している。
- mouse double-click、Enter、gamepad acceptはすべて`open_bookmark_browser_row`へ合流する。
- PDF、ZIP、画像フォルダ、製本、変換アーカイブは既存のdetached book context seamを使用し、mainの
  bookmark grid bundleを置き換えない。
- full-feature bookを開く前のactive mediaは既存のParkedLive handoffで退避する。

### context / cache ownership

- `ViewerContextBundle`はitems、thumbnail state、request/result channel、queue、cancel token、generation、
  fullscreen/edit cache、bookmark stateをcontext単位で所有する。detachedのDropやresult drainがmainの
  thumbnail workerを直接cancel/drainする経路はない。
- detached openはmain bundleを退避してempty bundleへloadし、active bundleを確定後にmainを戻す。
  close時もmainの現在selection/scrollを優先してbookmark read modelだけを再照合する。
- edit preview closeは、実在rowを削除した場合だけread-only closeのinvalidationを発行する。明示的編集は
  cache missでもinvalidationを発行する。報告されたmain thumbnailの不要なPending化に対する修正方針は
  適切である。
- bookmark read modelの同一判定はstable key、source/title/position、provenance、metadata、marker thumbの
  有無、created time、missingを比較する。marker thumb blobは既存blobを自動更新しない現行生成経路のため、
  有無比較で表示上の変更を取りこぼさない。

## 機能別再レビュー

### metadata import / export

- dialog workerは`MetadataTransferState`固有receiverとcancel tokenを持ち、別jobのcompletionを適用しない。
- exportのatomic sidecar、importの部分commit/cancel仕様、scan depth/size制限、manifest path検証を維持する。
- relative bookmark pageはprovenanceをmetadata、thumbnail、fullscreenまで運び、open済みfile handleのfinal
  pathをcontainment検証した同じhandleから読む。前回のTOCTOU指摘は解消済み。
- 追加指摘なし。

### mipmap / GPU lifecycle

- full mip chain、compare textureの現在組だけの保持、pin縮小texture、panorama seamのexplicit gradient、
  crop sampler分離を維持する。`20627fb4`以降に対象実装の意味を変える差分はない。
- 追加指摘なし。

### details / rating / music metadata / その他

- details best-fitはview kindと列別content revisionをjob keyへ含め、動的State列をall-row scanする。
- filesystem journal phase/rollback保持、rating shared write generation、stable ring target、XMP undoの前回修正を
  維持する。
- 音楽metadata URLは既存HTTP(S) parser/rendererを再利用し、外部browser open前にplayerをpauseする。
- top-level virtual viewは`TopLevelGridView`がsurface、return owner、generationを所有し、smart folder/search
  workerもcancel/generation境界を持つ。bookmark resolverと同型の未相関completionは見つからない。
- 追加指摘なし。

## commit分類

- bookmark / top-level view / detached routing: `8439c0b5`, `1ff18e91`〜`76f180df`,
  `2814e6bb`, `a575b174`, `f8b8ff78`, `ae6dde0d`, `ee8ffe57`, `04b9ee37`,
  `37b687ea`, `e5875e65`, `2d60663a`
- metadata import / exportとpath安全性: `1c773b1d`, `6d457883`, `7f5c1ce2`, `0534955f`
- mipmap / panorama / GPU resource: `d36a6005`, `0d42b62b`, `efb94303`, `20627fb4`
- details / font / UI scale: `9e506d80`, `e89eec16`, `d7331b1b`, `61218736`,
  `c25a26a3`, `1b8028ef`
- rating / ring / undo: `04b9ee37`, `37b687ea`, `eec48ae4`
- その他UI / release: `f4b2c663`, `1a13038a`, `e75c5504`, `a963317b`

## 検証結果

- `cargo test -p mimageviewer --bin mimageviewer-core bookmark -- --test-threads=1`:
  134 passed
- `cargo test -p mimageviewer --lib metadata_transfer -- --test-threads=1`: 17 passed
- `cargo test -p mimageviewer --bin mimageviewer-core details_best_fit -- --test-threads=1`:
  14 passed
- mipmap / panorama / music URL focused tests: 各1 passed
- `cargo test -p mimageviewer --lib -- --test-threads=1`:
  2,314 passed / 17 ignored
- `cargo test -p mimageviewer --bin mimageviewer-core -- --test-threads=1`:
  4,054 passed / 18 ignored
- `cargo check -p mimageviewer --bin mimageviewer-core`: passed
- `cargo fmt --all -- --check`: passed
- `python scripts/check_ui_glyphs.py`: passed（dangerous glyph 0）

## 最終判定

**レビュー時点ではコード出荷保留。** 上記P2をroot causeで修正し、通常navigation / activationを含むrequest lifecycle
testを追加した後、この指摘と同型経路を再レビューする。P2以外のコード領域は本レビュー上readyである。

## 対応追補（2026-07-23）

- bookmark open ごとに process-local の単調増加 request ID を発行し、media / book pending と path
  resolver owner を同じ ID で結び付けた。resolver owner は target identity も保持し、完了時に ID と
  target の両方が現在の request に一致する場合だけ結果を適用する。
- bookmark A → B、activation、通常 folder navigation は置き換え対象の owner だけを cancel する。
  resolver 完了後の page / player 待機中も、別 container への navigation で同じ pending と戻り先を
  終了する。古い A の cancel / completion は新しい B を消さず、表示 folder も変更しない。
- worker disconnect、media timeout、book page timeout に加え、従来無期限だった Book `Resolving` に
  45 秒 timeout を追加した。終了時は request に属する resolver / pending / view state を同じ境界で
  解消し、50ms repaint の理由を残さない。
- slow resolve → navigation、slow resolve → activation、A → B、stale completion、media / book の
  disconnect / timeout、resolver 完了後 navigation を回帰テストへ追加した。

対応後の検証は request lifecycle focused 9件成功、bookmark focused 139件成功、main binary 全テスト
4,065件成功・失敗0件・18件ignored、
`cargo check -p mimageviewer --bin mimageviewer-core` と `cargo fmt --all -- --check` 成功。

**追補判定: 上記P2は解消。本指摘によるコード出荷保留を解除する。**

## 変換アーカイブ追補（2026-07-23）

- bookmark path resolver の完了後に RAR / 7z / LZH のスキャン・確認・パスワード入力・変換へ
  移る場合も、`ArchiveConvertState` の型付き completion policy に同じ request ID と target identity を
  引き継ぐ。直接 RAR と元アーカイブ → キャッシュ ZIP の load は通常 navigation ではなく、同一
  request の内部遷移として適用する。
- 変換 state が request を所有している間は Book `Resolving` の 45 秒 timeout を停止する。通常
  navigation、activation、後続 bookmark、ダイアログ close / cancel は一致する request の変換
  cancel token と receiver を破棄し、遅延 completion は現在表示を変更できない。
- cache hit、direct RAR、new conversion completion、60 秒を超える確認待ち、通常 navigation / activation
  cancel、stale A completion 対 B request を focused test で固定した。

対応後の検証は startup / bookmark archive lifecycle focused 16件成功、ライブラリテスト
2,321件成功・失敗0件・17件ignored、バイナリテスト直列実行4,072件成功・失敗0件・18件ignored、
`cargo check -p mimageviewer --bin mimageviewer-core` と `cargo fmt --all -- --check` 成功。

**追補判定: 変換アーカイブで途切れていた bookmark request ownership は解消。**

## 変換前 scan cancel 追補（2026-07-23）

- `ArchiveConvertState` に scan / password retry / convert 共通の `Arc<AtomicBool>` を持たせ、state の
  Drop、Esc / cancel、通常 navigation、activation、後続 bookmark から同じ token を停止する。
- `spawn_archive_scan` から RAR direct-read 判定と RAR / 7z / LZH / ZIP summary scan へ token を渡し、
  各 entry 列挙境界で `Cancelled` を返す。receiver も state と同時に drop するため、cancel と競合した
  late result は表示状態へ到達しない。
- scan 中 bookmark A → B、通常 navigation、activation、通常 scan ダイアログの Esc、late result、
  cancel 後の後続 scan 成功を focused test で固定した。

対応後の検証は startup open-path lifecycle focused 21件成功、archive convert focused 43件成功、
ライブラリテスト2,323件成功・失敗0件・17件ignored、バイナリテスト直列実行4,082件成功・失敗0件・
18件ignored、`cargo check -p mimageviewer --bin mimageviewer-core` と
`cargo fmt --all -- --check` 成功。

**追補判定: 事前 scan worker が取消後も並行継続する経路は解消。**

## 通常 navigation の archive 置換境界追補（2026-07-23）

- `OpenRequestOwner::Navigation` は container 種別判定より前に visible-open lifecycle を取得する。
  archive A の scan 中に archive B を開いた場合、A の token と receiver を終了してから B の
  `ArchiveConvertState` を作るため、B が `archive_convert.is_some()` で拒否されない。
- 所有権判定は通常 folder load と共通の `claim_open_request_owner` に集約し、Bookmark owner の
  request ID / target 検証と same-folder reload の例外は維持する。
- App レベルで A scan → B open、A token cancel、B state ownership、A late completion の receiver
  drop を固定する。

対応後の検証は startup open-path lifecycle focused 22件成功、same-folder reload 回帰テスト成功、
ライブラリテスト2,324件成功・失敗0件・17件ignored、バイナリテスト直列実行4,086件成功・失敗0件・
18件ignored、`cargo check -p mimageviewer --bin mimageviewer-core` と
`cargo fmt --all -- --check` 成功。

**追補判定: 通常 navigation の archive A → archive B 置換で所有権が途切れる経路は解消。**
