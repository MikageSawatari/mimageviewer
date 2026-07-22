# v2.7.0 出荷前 横断レビュー計画

作成: 2026-07-23
コード比較基準: `v2.6.0` (`0d504f6d`) .. `a963317b`
正本となる設計: `docs/architecture-overview.md`、`docs/top-level-grid-view.md`、
`docs/detached-rework-plan.md`、`docs/detached-viewer-context-separation-plan.md`、
`docs/display-pipeline.md`、`docs/virtual-folders.md`

## 1. 目的

機能別の差分レビューだけでは、ブックマーク一覧からの open が通常の detached book 経路を
迂回した問題や、別ウィンドウの open / close が main grid の thumbnail を失効させた問題を
検出し切れなかった。本レビューは、機能ごとの縦断確認に加えて、状態 ownership、入口 routing、
非同期 request、context resource、終了処理を横断し、同じ壊れ方が別機能へ残っていないか確認する。

レビュー中はコードを変更しない。対象 HEAD を固定して指摘を集め、修正は別コミットで行う。
修正後は指摘箇所だけでなく、同型経路と本書の不変条件を再監査する。

## 2. 共通不変条件

1. 状態の正本と所有者を型または単一構造で特定できる。相互排他的な mode / request を複数の
   bool、`Option`、pending、`is_none()` sentinel の組合せで表現しない。
2. mouse、Enter、gamepad、ring、context menu、startup / activation など同じ操作意図の入口は、
   side effect を持つ前に同じ router へ合流する。
3. open request は target、origin、presentation、return / close policy、request identity を所有する。
   新しい要求、cancel、timeout、worker disconnect、dialog close、late result のいずれでも、古い要求の
   completion が現在の状態へ作用しない。
4. main、active detached、passive / ParkedLive の items、index、cache、queue、channel、generation、
   cancel token、worker は所有 context だけが変更・drain・cancel・dropする。
5. detached open / close / park / resume は main grid の items、selection、scroll、thumbnail state、
   worker generation を不必要に変更しない。途中失敗時は request・context・window runtime を同じ
   所有境界で rollbackし、半端な mounted stateを残さない。
6. index は現在の一覧内だけで使用し、sort、filter、非同期処理、context交換をまたぐ対象は stable
   identityで保持する。generationだけをownershipの代用にしない。
7. 読み取り専用のopen / closeは、永続値または派生表示を実際に変更した場合を除き mutation-style
   invalidationを発行しない。明示的編集はcache missでも必要な失効を発行する。
8. read modelを「変更なし」として再利用する比較は、表示、操作、identity、provenance、missing状態、
   thumbnail / metadataの意味を変える全フィールドを包含する。
9. close、Esc、Backspace、OSの×、別target open、設定切替、cancel、errorは、同じsession ownerが
   dispositionを決める。fieldの有無から戻り先を推測しない。
10. UI threadで同期I/O、decode、archive scan、worker joinを新設しない。

## 3. 最新ブックマーク修正から追加した監査観点

### 3.1 request相関とlate result

`PendingBookmarkOpen::{Media, Book}` による排他化だけでなく、次を確認する。

- path resolver、archive direct-read probe、変換dialog、PDF / ZIP enumerate、media player readyが、
  開始時のrequest identityとowner contextをcompletionまで保持する。
- 新しいbookmark、通常grid open、startup / activation、dialog cancelが古い処理を置き換えた後、
  古いresultを現在の`bookmark_open_pending`へ適用しない。
- cancel / disconnect / timeout / spawn failureの全経路でrequestとview stateが同じ境界で解決する。
- App-global dialogやworkerのcompletionが、可変なglobal pendingの「現在値」だけを見てopen先を決めない。

### 3.2 context handoffの原子性

- main bundleの退避、active / passive化、window runtime生成、request移送、page / seek待ち開始を
  一つの状態遷移として確認する。
- park失敗、target未検出、archive変換失敗、window close、enumerate失敗時に、mainとdetachedの
  両方へrequestやsessionが残らない。
- book↔media、media↔media、book↔bookの連続openで、直前contextのclose policyと新contextの
  open policyが混線しない。

### 3.3 read model維持とcache invalidation

- bookmark再読込が同一の場合、`start_loading_items`を避ける比較が全visible fieldを含む。
- bookmark名、位置、missing、provenance、source metadata、marker thumbnail、sort orderの変更を
  正しく検出する。
- viewer close時のedit preview削除は、行が実在して削除された場合だけ通常thumbnailを失効させる。
  編集操作による削除はpreview未生成でも派生表示を失効させる。
- invalidation eventはitem keyだけでなく、必要ならcontext / generation / source identityを持ち、
  同じpathを表示する別contextへ意図した範囲でだけ伝播する。

## 4. サムネイル・cache・worker ownership監査

「cache」を一括りにせず、次の層ごとにownerとlifecycleを確認する。

| 層 | 主な状態 | 確認する操作 |
| --- | --- | --- |
| 永続cache | catalog、edit preview DB / WebP | key、mtime / size、save / delete、通知条件 |
| CPU / read model | `items`、`ThumbnailState`、metadata、bookmark rows | install、restore、Ready→Pending / Evicted |
| GPU texture | thumbnail texture、keep set / range、LRU | upload、evict、viewport close / resync |
| 非同期制御 | request queue、result channel、pending / finalize | enqueue、drain、stale判定、result適用先 |
| lifecycle | generation、cancel token、worker pool、bundle Drop | switch、park、resume、close、error |

最低限、open / close前後でmain contextのitems identity、読み込み済み件数、generation、cancel token、
queue / receiver ownership、keep rangeが不必要に後退しないことを確認する。detached側のDropや失効通知で
main側の`Loaded`が`Pending` / `Evicted`へ戻る場合は、実データ変更という根拠が必要である。

## 5. 入口・対象・表示先・終了の監査表

| 軸 | 値 |
| --- | --- |
| 入口 | mouse / double click、Enter、gamepad、ring、context menu、startup / activation |
| 一覧 | 通常、bookmark、rating、tag、history、search、smart folder |
| 対象 | image、folder、PDF、ZIP / direct RAR、converted archive、compiled book、video、audio |
| 表示先 | main fullscreen、linked viewer、independent detached、media window、ParkedLive |
| lifecycle | first open、rapid replacement、park / resume、Esc / Backspace、OS close、cancel / error |

静的レビューでは各入口のcall pathを全件確認する。自動・実機テストは全直積ではなく、各不変条件を
破りやすいpairwiseと連続操作を選ぶ。ただし全book種別のbookmark open、book↔media連続要求、
main thumbnail読み込み中のdetached open / close、gamepad経路は必須とする。

## 6. 実施順

1. commit分類と変更されたownership / async境界の抽出
2. bookmark open routingとreturn / close policy
3. detached session lifecycleとcontext handoff
4. thumbnail / cache / worker ownership
5. input parityとrequest correlation / cancellation
6. bookmark以外のtop-level virtual viewへの同型検索
7. metadata import / exportとpath / transaction境界
8. mipmap / GPU resource lifecycle
9. details表示、rating、music metadata、その他変更
10. focused test、全lib test、`cargo check`、`cargo fmt --check`、必要なportable smoke整理

各章が終わるたびに`systemic-review-report.md`へ結果を記録する。指摘は重要度、破れた不変条件、
証拠、root cause、同型検索範囲、必要な修正、回帰テスト、修正commit / 再レビュー状態を持つ。

## 7. 出荷判定

- P1 / P2が未解決でない。
- typed request / ownerを迂回する同等入口が残っていない。
- main / detached間でcontext resourceを誤ってcancel / drain / invalidateする経路がない。
- request置換・cancel・timeout・late resultの回帰テストがある。
- bookmark、metadata transfer、mipmap、miscの機能別指摘が再レビュー済みである。
- 自動検証が成功し、Windows固有挙動は通常設定を起動せずportableまたはユーザー実機で確認する。
