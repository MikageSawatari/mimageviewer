# §1.218 見開き端の単ページ配置 — 実装設計

## 状態

2026-09-13 に構造合意し、同日製品実装と focused 検証まで完了した設計・検証記録。
2026-09-16 の追加仕様では、端にある横長ページも中央へ置き、本当に相方 slot が不足した単ページだけを
片側へ置くよう形成理由の所有境界を補強した。仕様正本は
[next-release-backlog.md §1.218](next-release-backlog.md#1218-見開きの先頭末尾にある単ページを本来の側へ配置する--384-386-2026-09-11)。
親担当と独立レビュー担当が確認した見開き構成・描画・保存・Remote の所有境界を正本として残す。

## 確認した現行境界

- `SpreadDisplayUnitsCache` が、現在の item 世代、読書順、見開き mode、ずらし位置、
  横長判定 epoch に対応する canonical な表示単位列と、その各 unit の形成理由を所有する。同じ token は、現在の列が
  Folder surface の全 page を一意に含むことを示す complete-page-permutation も memoize する。
- `resolve_spread_display_composition` は canonical unit の位置から
  `SpreadDisplayComposition` を作り、末尾表紙補助を加えた後の実 presentation を返す。
  通常表示、連結読み、Remote は既にこの resolver を共有している。
- `SpreadPair`、navigation anchor、`SpreadPageOccurrence`、seek/history/read-position は
  ページの topology と identity の所有者である。表示位置だけを表す値をここへ混ぜると、
  ページ数、要求、PDF target、履歴へ偽の相方を波及させる。
- `FullscreenPageLayout` はその frame に実際に描いた `DisplayedImageTransform` を保持し、
  edit、crop、ルーペ、capture、navigator、PDF page hit が同じ transform を読む。
- `ContinuousReadingUnitSpec` は同じ composition の occurrence を持つが、現行の単ページは
  `continuous_unit_layout` で可視 union へ縮めて中央へ置く。`Width` の一部で 2 ページ幅を
  fit 計算に使う既存処理だけでは、空き側を含む片側配置にはならない。
- `FsDisplayUnitHoldover` は capture 時の texture、回転、canonical 寸法、content bbox を保持し、
  folder navigation、presentation switch、final-effect reload、detached backstop で再描画する。
  current items から geometry を再導出してはならない。
- 本別の末尾表紙設定は `ViewerContextBundle` と `spread.db` の独立 table が所有し、
  nested book fallback、rename/purge、明示 metadata transfer、metadata import refresh、Remote の
  read/write に接続済みである。新設定もこの lifecycle を踏襲できる。
- 自動 sidecar (`mimageviewer.dat`) は spread 系設定を保存していない。今回も対象を増やさず、
  明示的なメタ情報エクスポート・インポートだけを container state の移送経路とする。

## 型と判定の唯一の所有者

### 表示配置

`ui_fullscreen.rs` に次の値型を置く。

```rust
enum SingletonSpreadPlacement {
    Center,
    Left,
    Right,
}
```

`SpreadDisplayComposition` が、この配置と role 付き presentation を一体で所有する。
placement は navigation demand ではなく layout projection であり、`SpreadPair`、page index、
occurrence、anchor、page count は変えない。Remote 向け group と連結読みも、この field を
コピーするだけで caller ごとの端判定を行わない。

`SpreadDisplayUnit` はページ列と `Paired / Singleton(cause)` を同じ値に保持する。singleton の
`cause` は `UnpairedSlot / LandscapeBoundary / NonPairableBoundary` のいずれかで、unit builder だけが
決める。現在ページまたは本来の相方候補が横長なら `LandscapeBoundary`、現在ページまたは相方候補が
ペア化不能なら `NonPairableBoundary`、cover 位相、ずらし segment 端、本末尾で相方 slot 自体が無い場合だけ
`UnpairedSlot` になる。cover 先頭も先に回転後横長を評価し、横長なら cover 位相より
`LandscapeBoundary` を優先する。形成理由は unit と同じ cache token に属し、寸法確定や保存回転の変更で
横長判定 epoch が進むと unit と一緒に再構築する。

判定は `resolve_spread_display_composition` 内で、末尾表紙補助を解決した後に一度だけ行う。

1. 設定が無効、`SpreadMode::is_spread()` が偽、または complete-book proof が偽なら `Center`。
   検索、stack、部分的な visible 列、synthetic surface の先頭・末尾を本の端と誤認しない。
2. 最終 presentation が実 1 page でなければ `Center`。したがって末尾表紙補助で実 2 page に
   なった末尾へ重ねて適用しない。
3. canonical unit の形成理由が `Singleton(UnpairedSlot)` でなければ `Center`。横長ページと、その横長を
   相方候補に持った縦長ページ、ペア化不能 item の境界は、先頭・末尾でも中央のままにする。
4. canonical `unit_pos == 0` なら先頭規則を優先する。既存 unit phase に従い、
   `Ltr=Left / LtrCover=Right / Rtl=Right / RtlCover=Left`。
5. 先頭でなく `unit_pos + 1 == units.len()` なら末尾規則。LTR は `Left`、RTL は `Right`。
6. それ以外は `Center`。

先頭側は `Right iff spread_mode.is_rtl() XOR spread_mode.has_cover()` と同値で、末尾側は
`Right iff spread_mode.is_rtl()`。この順序により、1 unit だけの本は常に先頭規則になり、
cover有無で自然な側が変わる。ただしこの規則を使うのは `UnpairedSlot` だけである。横長・回転後横長、
横長の相方候補によって単独になった縦長、非ペア対象の境界は、端 unit でも中央のままにする。
端判定に `idx == 0` や `items.len() - 1` を使わず、filter、自然順、
見開きずらし、ZIP 内 book、Remote でも同じ canonical unit position を使う。

`Single`、`SplitLtr`、`SplitRtl` は見開き presentation ではないため常に `Center`。
通常の single-page 表示と、見開き設定中に端 unit が 1 page になる場合を区別する。

現 `final_cover_spread_complete_book_eligible` の context gate と cached permutation proof は
singleton placementでも必要になるため、両機能共通の complete-book eligibility へ一般化する。
proofの計算回数やtoken invalidationは変えない。Remote は既にserverが持つ
`complete_book_eligible`を同resolverへ渡し、collection / truncated / synthetic列は `Center` にする。

### 設定

- 全体設定: `Settings::singleton_spread_placement_enabled: bool`。serde の旧設定 fallback と
  `Default` は `false`。
- 本別設定:

  ```rust
  enum SingletonSpreadPlacementPreference {
      FollowGlobal,
      Place,
      Center,
  }
  ```

  `effective(global_enabled)` だけが bool を解決する。
- `ViewerContextBundle` に本別 preference を置き、mount / swap / fork / park / retire を既存
  final-cover preference と同じ transaction で運ぶ。各 viewer context は独立する。
- 環境設定変更時は `FollowGlobal` の mounted / AtRest context だけ placement 依存 layout を
  invalidate する。明示 `Place` / `Center` の sibling は触らない。
- 本のメニューは `全体設定に従う / 配置する / 中央表示`。既存の末尾表紙補助の 3 状態と
  別の行、別の保存値にする。新しい keyboard action は作らない。

## 仮想キャンバスと fit

片側配置は、相方の画像を推測しない。表示する 1 page 自身の回転後 canonical layout size と
表示トリム後 content bbox を左右へ同じ寸法で 2 回並べた仮想見開きを layout 上だけ作る。

```text
virtual visible width = page visible width * 2 + quantized spread gap
virtual height        = page visible height
```

相方の `GridItem`、occurrence、texture、thumbnail、PDF render、AI request、cache keep demand は
作らない。空き slot は既存の fullscreen margin background のままになる。異なる相方寸法を
推定すると、隣ページの decode/回転/trim 到着で endpoint の倍率が変わるため採用しない。

Rust 側は既存 `spread_layout_geometry(page, page, bbox, bbox, ...)` と物理 pixel 量子化規則を
使う純粋な singleton-slot geometry helper を設ける。仮想見開きを viewport 中央へ置き、
`Left` / `Right` が選んだ一方の page rect だけを返す。実 raster は既存
`resolve_fs_transform_in_layout_rect` でその rect に contain し、`FullscreenPageLayoutKind` は
`Single`、登録 occurrence も 1 件のままにする。

fit の意味は実 2 page 見開きと揃える。

- `Page` と paged の `MarginFit`: gap を除く仮想 2 page 幅と仮想高さを viewport 内へ contain。
- `Width`: gap を除く仮想 2 page 幅を viewport 幅へ合わせる。
- `Height`: 仮想高さを viewport 高さへ合わせる。横にはみ出す場合も実見開きと同じ。
- `Original`: page は viewport の `pixels_per_point` に対応する 100% physical scale。仮想
  canvas は同倍率の 2 page と gap を持ち、viewport 中央を基準にする。
- 拡大しない／縮小しないの scale limit、保存回転、表示トリム、自動余白、通常 zoom/pan、
  Z zoom は、単ページ独自の再計算を足さず仮想 canvas の fit/center と選択 slot を入力にして
  既存 transform resolver を通す。Z zoom は実見開きの combined-canvas solve と同じ基準で
  解き、source request は 1 page のままにする。

paint は全 `image_rect` で clip する。選択 slot の rect から得た実
`DisplayedImageTransform` を edit hit、crop、loupe、capture、PDF target、navigator が読むため、
overlay や入力側で Left/Right を再計算しない。比較・パノラマ・音楽など、自身の typed mode が
通常の book composition を置換する画面は既存の専用 layout を維持する。そこから通常見開き
表示へ戻った frame で composition の placement が再び適用される。

Center の composition は既存 single painter をそのまま通し、既定 OFF の geometry と丸めを
bit-for-bit で維持する。

## 各表示経路への伝達

### 通常 paged / F12 / detached

通常 paged の `SpreadPair::Single` branch は composition の placement を singleton-slot helper
へ渡す。F12 / detached は同じ mounted `ViewerContextBundle` と同じ painter を使うため、window
種別の分岐を追加しない。viewport ごとの `pixels_per_point` と `image_rect` を引き続き使用する。

`FsDisplayUnitHoldover` は capture-time `SingletonSpreadPlacement` を保持する。1 page holdover の
4 draw 経路は current item、現在設定、現在の unit 列から再判定せず、capture 済み placement と
寸法から同じ slot geometry を再構成する。2 page holdover では placement は `Center`。

設定変更時は live `FullscreenPageLayout`、continuous transform、final-effect source-reload
holdover を final-cover 設定と同じ境界で invalidate する。navigation / presentation-switch の
previous holdover は「変更前に実際に見えていた unit」なので capture-time placement を保ち、
次の live unit を提示した時点で自然に退役させる。設定変更後の current navigation demand は
ページ topology が不変なので増減させない。

### 縦連結 / 横連結

`ContinuousReadingUnitSpec::composition` に placement をコピーし、通常 `pages` と split `half`
constructor は `Center` にする。`ContinuousReadingUnitSize` / `continuous_unit_layout` は、片側配置の
1 page unit に限り空き slot を含む仮想 canvas の cross-axis extent と page origin を返す。
page list と source set は 1 件のままにする。

- 縦連結では仮想 canvas の幅を viewport 中央へ置き、その Left/Right origin へ実 page を置く。
- 横連結では仮想 canvas の幅を unit の scroll-axis extent にも使う。空き slot を含む unit 間隔を
  実見開きと同じにしつつ、navigation step は 1 unit のままにする。
- 現行 `Width` の centered singleton 用 2-page fit 特例は既存挙動として保持する。片側配置は
  fit scale だけでなく `continuous_unit_layout` の origin / drawn extent まで typed placement を
  反映し、最後に可視 union だけへ縮めて中央へ戻さない。

連結読みでも `VerticalReadingPage` は実 page 1 件だけを持ち、描画後の transform が edit hit と
navigator の正本になる。keep-set、VRAM、transition、prefetch は source index 1 件のまま。

### Remote

`RemotePageGroupSpec` と IPC `PageGroup` に serde default `Center` の placement を追加する。
`anchor/pages/presentation/slice` は変更せず、通常 singleton group も page address は 1 件のまま。
old payload は field 不在を `Center` として読めるようにし、protocol version と asset token は
現行の strict client/server 更新手順に従う。

server は complete canonical unit から local と同じ composition placement を address group へ
写す。画像 folder、ZIP、PDF は共通 builder を使う。truncated / collection / non-book など端を
証明できない列は `Center`。ページ request、prefetch、decode lease、render context の
`display_slot` は 1 page のままで、偽の spread partner を作らない。

Web の `viewerSlicedSpreadLayout` は placement を受け、Rust と同じ self-duplicated virtual canvas
寸法と fit を解く。DOM は `<img>` 1 個だけを維持し、page layer の幅と image の X origin
(`flex-start` + typed offset、または同等の一意 helper) で Left/Right を表す。`load`、decode-ahead
reuse、refit、resize、adjustment preview replacement のすべてが保存した placement を同じ helper
へ渡す。`Original` の初期 scroll は仮想 canvas 中央、`Page` / `Width` は上記 fit と同じ。

本別 preference は final-cover と並ぶ別 typed write request とし、exact/fallback book keyへ
UI-thread ownerが保存する。refresh は現在 anchor/slice/historyを保持したまま group placement だけ
更新する。Remote の live reading settings snapshot に global bool を追加し、起動時の古い Settings
clone で判定しない。

## 保存・移送・削除

`spread.db` に独立 table を追加する。

```sql
CREATE TABLE singleton_spread_placements (
    path TEXT PRIMARY KEY,
    preference INTEGER NOT NULL CHECK (preference BETWEEN 0 AND 2)
)
```

`0=FollowGlobal / 1=Place / 2=Center`。root の `FollowGlobal` は row を削除し、nested exact key の
`FollowGlobal` は row 0 を保存して root fallback を遮る。table 不在の old read-only DB は
`FollowGlobal`。既存 `spreads` や `final_cover_spreads` row を materialize / update しない。

次を final-cover table と同じ単位で接続する。

- `SpreadDb::clear_all` と path 単位の重複を除いた `count`
- `rename_key_migration::STORES` の optional legacy store、move/rename/delete/hard purge
- metadata transfer の `PortableContainerState`、export、validation、destination family delete、
  import、nested key、round trip
- `metadata_import_refresh::ContainerStateResult` と exact/fallback refresh
- local load/persist、content identity の Edit trigger
- Remote read-only missing-table fallback と typed write

自動 sidecar の schema と import familyは変更しない。既存 DB rows、読書位置、タグ、評価、編集、
末尾表紙補助は走査も書換えもしない。

## 変更対象の見込み

共通・local:

- `src/settings.rs`, `src/settings_db.rs`
- `src/spread_db.rs`, `src/rename_key_migration.rs`, `src/metadata_transfer.rs`
- `src/app.rs`, `src/app/viewer_context_registry.rs`, `src/app/metadata_import_refresh.rs`
- `src/ui_fullscreen.rs`
- `src/ui_dialogs/preferences.rs`, `src/ui_dialogs/preferences/pages.rs`,
  `src/ui_dialogs/preferences/search_index.rs`

Remote:

- `crates/remote-ipc/src/lib.rs`
- `src/remote_ipc/container.rs`, `src/remote_ipc/ui.rs`
- compiler が示す payload snapshot / constructor 接続 (`src/remote_ipc/collections.rs`,
  `src/remote_ipc/session.rs`, `src/remote_ipc/pipe.rs`)
- `crates/remote-web/web/command-core.mjs`, `crates/remote-web/web/app.js`,
  `crates/remote-web/web/styles.css`
- 対応する Rust / Node / UI snapshot tests

文書:

- `docs/display-pipeline.md`, `docs/architecture-overview.md`
- `docs/detached-rework-plan.md` §11（detached 専用分岐を足さず共通 composition を運ぶ変更記録）
- `docs/final-cover-spread-plan.md`（独立機能であることと共有 lifecycle の追補）
- Remote の wire / UI 正本、および環境設定マニュアルの該当箇所

keymap は変更しない。自動 sidecar restore の source も変更対象にしない。

## 回帰計画

### 判定と topology

- `Ltr / LtrCover / Rtl / RtlCover` × first / last、奇数 / 偶数、1 page first-priority。
  side表を4 modeごとに固定し、cover phaseをreading directionだけへ潰さない。
- cover / non-cover mode、末尾表紙補助 ON/OFF。補助後 2 page は `Center` / placement 不適用。
- middle / endpoint の横長 singleton と、横長の相方候補により単独になった縦長は `Center`。
  片側になるのは `UnpairedSlot` の first / last だけ。
- setting OFF、book Center、Single / split mode は現行同値。
- filter / reorder / shift anchor を含む canonical `unit_pos` で判定し、raw idx 端を使わない。
  partial visible/search/stack/syntheticはcomplete-book proofが偽で全てCenter。
- navigation pages、anchor、occurrence、page count、seek group、bookmark/history/read-positionが
  before/after で同一。fake item/request が 0。

### geometry と描画

- `Page / Width / Height / MarginFit / Original`、gap、1.0/1.5/2.0 ppp、回転、trim、PDF
  canonical sizeについて、仮想幅、scale、slot rect、clipを pure test。
- default OFF / Center の transform と physical-pixel rounding が既存 golden と exact 同値。
- actual painter / snapshot で4 modeのfirst/last side表、空き側 background、1 pageだけ描画を確認。
- zoom/pan、Z zoom、navigator、edit/crop/loupe/capture/PDF hitが同じ stored transform を使用。
- capture-time Left/Right holdoverを4経路で再描画し、folder replace / resize後も中央へ戻らない。
- landscape singleton のpainted transformとcapture-time holdoverは `Center` を維持する。
- preference変更でlive/final-effect geometryを更新し、previous navigation holdoverはcapture表示を維持。

### continuous / context

- 縦・横連結で同じ endpoint placement。cross-axis originとvirtual extentを確認。
- page/source/keep/prefetch/transition数は1のまま。middle singleton center。
- main / linked F12 / 複数 detached context で一致し、別book preferenceとscroll/layout ownerが混ざらない。
- global変更はFollowGlobalのmounted/AtRestだけinvalidateし、明示override siblingは不変。

### 保存 / Remote

- global default false、3-state resolver、nested explicit FollowGlobal、old read-only schema。
- `spread.db` の既存mode/flow/direction/final-cover row不変、count/clear。
- rename/move/delete/hard-purge、metadata export/import/delete、nested ZIP、metadata refresh。
- Remote image-folder / ZIP / PDF server group placement、collection/truncated center、serde old fallback。
- Remote の endpoint landscape / 回転後 landscape は本体と同じ `Center`、portrait の相方不足だけ片側。
- Web `Page / Width / Original` layout、resize/refit、single decode reuse。DOM image/request/page numberは1。
- Remote per-book write後のanchor/slice/history保持、live global更新、protocol/frame budget。

## 規模と実装順

correctness上の新しい非同期 workerやI/Oは不要だが、local paged、continuous、holdover、DB lifecycle、
Remote protocol/Webを横断するため規模は Medium。暫定 bool、caller別端判定、blank image、fake item、
描画後の位置補正では実装しない。

実装順は、(1) 型・純判定・DB/設定 lifecycle、(2) local paged/holdover/continuous、
(3) context/global invalidationと設定UI、(4) Remote wire/server/Web、(5) 文書・snapshot、
(6) focused → full gate → verification build とする。各段階で topology不変を先に固定し、
構造レビュー合意後にだけ製品編集へ進む。

## 実装結果（2026-09-13）

- `SpreadDisplayComposition` が `Center / Left / Right` を所有し、complete-book proof と canonical
  unit position、cover 位相から一度だけ解決する。`Single` / split、部分列、synthetic、末尾表紙を
  添えて実 2 page になった composition は `Center` のまま。
- paged、Z、縦・横連結、capture-time holdover、Remote `PageGroup` / Web DOM が同じ typed placement
  を受け取る。仮想 2-slot canvas と gap は layout にだけ存在し、page / occurrence / request / DOM
  image は 1 件のまま。zoom/pan clamp、回転・trim の source↔screen 写像も同じ transform を使う。
- 全体設定と本別 3 状態を `spread.db` の独立 table に保存し、ViewerContextBundle、AtRest、rename / purge、
  metadata transfer / refresh、Remote read / write まで接続した。既定 OFF と旧 DB / 旧 wire の
  `Center` fallback を維持する。
- 項目追加で高さが増えた fullscreen の表示モード popup は、利用可能高へ clamp した ScrollArea が
  見出しと全行の paint / interaction rect を所有する。576 / 720 pt でもスクロール後に本別 3 状態を
  選択できる。

### 2026-09-16 追加修正

- `SpreadDisplayUnit` にページ列と形成理由を一体で保持し、builder が
  `Paired / UnpairedSlot / LandscapeBoundary / NonPairableBoundary` を一度だけ確定する。
  cover 先頭も回転後横長なら `LandscapeBoundary` となる。
- 共通 composition は末尾表紙補助を従来どおり先に解決し、実1pageかつ
  `Singleton(UnpairedSlot)` のfirst / lastだけを片側へ置く。横長境界と非ペア境界は端でも`Center`。
- paged / F12 / 複数window / 縦横連結 / holdover / Remote はtyped placementをそのまま使い、
  caller別の縦横再判定、wire変更、detached predicateやviewport変更を追加しない。
- unit pages、anchor、occurrence、navigation demand、seek、history、bookmark、page count、edit target、
  末尾表紙補助、設定値の意味は変更しない。

focused 検証では singleton 関連 Rust 19 件、Z の左右・active/inactive・90°回転+trim、popup の
576 / 720 pt 実 egui 操作、DB / metadata / context / rename 境界、Remote IPC 55 件を確認した。
Remote Web は command-core 129 件と app-runtime 106 件が成功し、actual `ImageViewer.loadGroup` で
Left / Right の DOM image 1 件、仮想幅 `2 * page + gap`、各 fit と adjustment replacement 後の
再 fit を確認した。UI snapshot は新しい全体設定の文言・配置を画像で確認済み。全体 gate と
verification build の証拠は、この文書の最終検証節へ追記する。

## 最終検証（2026-09-13）

- 最初の full gate は標準出力の保存が不完全で、当初の失敗件数を確定できなかった。この結果は
  最終証拠へ再利用していない。完全ログを保存した次の実行で、既存 Preferences snapshot の
  scrollbar thumb 1 pixel 差、content identity restore の一定 DB open 回数期待、旧 read-only
  `spread.db` の `count()` 互換性をそれぞれ切り分けた。
- snapshot は同じ設定ページへ singleton 行を追加したことで全 content 高が増えた正当な差分で、
  可視本文・背景は不変だった。DB open は候補 1 / 100 件とも31回で一定（22 store copy、
  1 origin batch、singleton placement を含む8 runtime reads）。旧 schema は optional table が
  0個または片方だけでも既存 row を数え、現行3 tableでは同じ pathを一度だけ数えるようにした。
- 修正後の狭域は `spread_db::tests` 9 / 9、batch restore 1 / 1、Preferences snapshot 1 / 1、
  `cargo check -p mimageviewer --bin mimageviewer-core` が成功した。
- `RUST_TEST_THREADS=1`、`CARGO_BUILD_JOBS=1`、`-SuppressCrashDialogs` の最終
  `scripts/test-full.ps1` は main 8323 / 0（45 ignored）と workspace / integration / doc /
  vendor 全体が成功した。完全 stdout / stderr は
  `target/section218-singleton-spread-placement-20260913/test-full-final.log`（SHA-256
  `436B2DF6506FC9CE03B0AAF68C2E3831133DE567BF2ABA7B013107F230126C32`）へ保存した。
- 新しい parked-context invalidation API は viewer context audit が A4 で検出した。独立検収後、
  exact fingerprintだけを既存 final-cover API の隣へ登録し、監査規則・閾値・CLIは変更していない。
  audit unit 35 / 35 と repository audit 0 violations、`cargo fmt --all -- --check`、UI glyph、
  `git diff --check` が成功した。
- 同じ source freeze で `scripts/build-dev.ps1 -PreserveRuntime` が exit 0。成果物は
  `target/dev-runtime/mimageviewer-core.exe`（SHA-256
  `35167AA7B8C518617C5DB3DA03FEDAF3472084443AF3148862586F3E9B7616C5`、
  2026-09-12 18:15:32 UTC）と `mimageviewer-remote.exe`（SHA-256
  `A03CE402A7137613E063867C9F8338BEDC5C338FE8897325007BB35110935C64`、
  2026-09-12 18:16:07 UTC）。agent はアプリを起動・停止していない。
- 2026-09-13、後続§1.212を含む確認buildで利用者が本体・Remoteの見開き配置を確認し、
  両方「大丈夫そう」と報告した。利用者確認済みとする。

### 2026-09-16 追加修正の検証

- 横長singletonの形成理由を固定する追加回帰9件と、既存の `spread_display` 3件、
  `spread_seek` 8件、`singleton_placement` 1件、`remote_spread` 3件、`final_cover` 30件が成功した。
  `cargo check -p mimageviewer --bin mimageviewer-core` も成功した。
- `scripts/test-full.ps1 -SuppressCrashDialogs` は main 8585 / 0（45 ignored）を含む workspace、
  integration、UI snapshot 52件、IPC 57件、Remote 122件（1 ignored）、doc、vendor 全体で成功した。
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、
  `cargo run --locked -p viewer_context_audit --quiet`、対象pathの `git diff --check` はすべて成功した。
- 同じsourceで `scripts/build-dev.ps1 -PreserveRuntime` が exit 0。成果物は
  `target/dev-runtime/mimageviewer-core.exe`（SHA-256
  `93463AF2AE96DED25AE6A21E96ADC565CFCA3E077FF2536974BA57CE9A2DA0F9`）と
  `mimageviewer-remote.exe`（SHA-256
  `8ABADDA66BC570D269ABB22439C2A6CA90964A5C2DF2239124CD81B01AEED0AD`）。agent はresidentを停止せず、
  アプリも起動していない。

## 2026-09-23 detached passive と ViewerContextBundle 横断監査

### 実機症状の経路と修正境界

active detached window を pause すると、AtRest の `ViewerContextBundle` を一時 mount し、
`build_active_detached_image_window_snapshot` で表示 snapshot を作った後に同じ bundle を deposit する。
`singleton_spread_placement_preference` 自体はこの transaction で失われていなかった。一方、従来の
paged frozen producer は `SpreadPair::Double` だけを再解決し、canonical display unit が 1 page の
endpoint singleton では空を返していた。そのため passive snapshot は direct-single の既定 `Center`
へ落ち、再 activate / remount すると同じ bundle の preference から canonical `Left / Right` が戻って
いた。観測された「非アクティブ時だけ中央、再アクティブ時に片側」はこの経路と一致する。

修正後は、snapshot owner がまず「最後に実際に描いた presentation」を typed に選ぶ。navigation / source
reload の `FsHoldover` が live page の後に overlay された状態なら、その旧 resource unit を clone し、
current `items[idx]` を取り直さない。holdover が無ければ current live unit を capture する。この選択は
`Ready` を `Presenting` に進めず、source-reload holdover も clear しない read-only query である。選ばれた
resource unit へ、直近の `fullscreen_page_layout` が持つ exact `ResolvedDisplayPlacement` を合成し、
`CapturedPagedDisplayUnit { unit, presentation }` を paged snapshot bake の唯一の入力にする。

1 page + `Left / Right` は `ResolvedDisplayPlacement::SingletonSpread`、Z / ZipPla は
`ResolvedDisplayPlacement::Z { active, factor, zoom_pan, singleton_side, singleton_gap }`、2 page は同じ
capture-time page 群と resolved placement を live と同じ `DisplayedImageTransform` へ渡す。active Z の
見開き gap は live と同じく zoom 倍し、fit scale は zoom 前の gap から求める。holdover と Z は1 pageでも
frozen pageへbakeする一方、通常 live の `Center` は空の frozen listを返して従来のdirect-single pathを
保つ。paged producer は egui `Context` ではなく確定済み `pixels_per_point` を受けるので、
`build_active_detached_image_window_snapshot(None)` でも同じ結果になる。passive 側で setting / preference /
endpoint / 縦横を読み直す分岐、新しい lifecycle state は追加していない。

### 245 field の ordinary lifecycle 監査

`ViewerContextBundle` の named field は **245**。ordinary mount / deposit / remount の transaction 自体は
`swap_viewer_context_bundle` の **244 `swap_field!` + `viewer_session.swap_with_mounted` 1 件**で全 field を
交換する。従って **ordinary swap coverage は M / D / R とも245 / 245**であり、そこに field copy の欠落は
無い。ただし active → park は単なるswapではない。snapshot前にforeground-only interactionを終了し、
snapshot後にbackground workをcancel / drainしてからdepositする。この semantic terminal で意図的に変わる
fieldまで「同じ値のままP/R」とは数えない。下表の「swap ✓」はordinary transactionの網羅性、右列は
park terminal と passive可視投影を分けて示す。

| # / 件数 | 意味グループと field（ordinary M/D/R swap ✓） | park terminal / passive の扱い |
|---|---|---|
| 1–12 / 12 | `address`, `current_folder`, `favorite_view_context`, `navigation_scope`, `archive_source_override`, `zip_nav`, `stack_mode_requested`, `stack_view`, `stack_showing_flat`, `stack_active_rule`, `stack_script_error`, `stack_toggle_select_path` | bundle 保持。title / reopen identity に必要な範囲だけ snapshot 化 |
| 13–28 / 16 | `items`, `items_generation`, `visible_indices`, `facet_name_cache`, `facet_name_cache_generation`, `facet_name_cache_pending`, `facet_name_cache_failed_generation`, `thumbnails`, `image_metas`, `video_thumb_overrides`, `auto_aspect`, `selected`, `grid_click_selection_anchor`, `scroll_offset_y`, `scroll_to_selected`, `pending_grid_scroll` | bundle 保持。passive still は固定 snapshot なので一覧 UI を再投影しない |
| 29–47 / 19 | `requested`, `idle_upgrade_cache_bypass_ineligible`, `keep_range`, `keep_set`, `still_seek_thumbnail_pages`, `still_seek_thumbnail_pages_shared`, `thumbnail_eviction_generation`, `details_thumb_suppression_applied`, `details_hover_thumb_idx`, `details_hover_thumb_viewport_open`, `texture_backlog`, `details_order`, `details_order_revision`, `details_cell_content_revisions`, `details_tag_prewarm_indices`, `details_lazy_meta`, `details_meta_pending`, `details_lazy_visible_revision`, `details_image_dims_state` | `texture_backlog` はsnapshot完成後にclear。残りはbundle保持し、worker / details projection は passive still では駆動しない |
| 48–56 / 9 | `metadata_cache`, `exif_cache`, `xmp_cache`, `xmp_panorama_info`, `metadata_pending`, `tags_cache`, `tag_prewarm_pending`, `tag_prewarm_queued`, `pending_finalize` | bundle 保持。passive still DTO の対象外 |
| 57–76 / 20 | `tx`, `rx`, `cancel_token`, `reload_queue`, `heavy_io_queue`, `scroll_hint`, `visible_end_shared`, `keep_start_shared`, `keep_end_shared`, `last_vis_range`, `vis_settle_at`, `vis_first_logged`, `vis_all_logged`, `folder_nav_pending`, `folder_pane_open_pending`, `pending_folder_nav_steps`, `pending_folder_nav_mode`, `search_filter`, `search_filter_origin_folder`, `checked` | worker基盤はbundle所有。park後に`folder_nav_pending` / `folder_pane_open_pending`をcancelしてtake、stepを0、modeをGridへ戻す。passive stillは固定表示 |
| 77–92 / 16 | `rotation_cache`, `page_dims_cache`, `spread_display_units_cache`, `rating_cache`, `rating_filter_suppressed_at`, `rating_session_write_seen_generation`, `metadata_import_refresh_index`, `current_folder_rating_cache`, `current_folder_last_mtime`, `current_folder_signature`, `folder_pin_map`, `converted_archive_cache_paths`, `converted_archive_pin_root_states`, `converted_archive_cache_paths_pending`, `current_color_cache_map`, `current_color_catalog` | bundle 保持。回転・寸法・canonical unit の capture 結果は passive DTO に bake |
| 93–110 / 18 | `vst3_deferred_media_open`, `fullscreen_idx`, `fullscreen_page_slice`, `fs_secondary_press`, `viewer_session`, `native_video_in_window_active`, `video_audio_mode`, `video_audio_vst`, `video_audio_mode_runtime`, `video_seek_strip_runtime`, `video_seek_strip_wave_holdover`, `video_seek_strip_next_session_id`, `panorama_state`, `video_zoom_state`, `native_video_mouse_seek_holds`, `panorama_intent`, `fs_info_panel`, `similar_panel` | `viewer_session`の内包4値は専用swap。park前に`fs_secondary_press`、pointer gesture、`fs_info_panel.hover_active`を終了し、panorama live state / video zoomを既存intentへ畳む。`fullscreen_idx/page_slice`とintentは保持し、still passiveはcapture済みidxだけ表示 |
| 111–126 / 16 | `pano_toast_shown_for_current_fs`, `analysis_mode`, `analysis_hover_color`, `analysis_pinned_color`, `analysis_grayscale`, `analysis_mosaic_grid`, `analysis_filter_mag`, `analysis_guide_drag`, `view_trim_mode`, `view_trim_apply_mode`, `view_trim_page_apply_root_idx`, `view_trim_page_spread_separate`, `view_trim_book_settings`, `view_trim_page_overrides`, `view_trim_dirty_page_overrides`, `view_trim_save_pending` | park前にanalysis live modeとview-trim edit modeを終了し、dirty trimを保存する。book/page trim設定は保持し、capture結果をbbox / UV / clipへbake |
| 127–154 / 28 | `fs_cache`, `fs_lanczos_cache`, `fullscreen_navigator_interaction`, `fullscreen_page_layout`, `fs_margin_bbox_cache`, `input_generation`, `fs_pending`, `fullscreen_pdf_promotion`, `fs_pdf_display_target`, `fs_early_dims`, `fs_upload_backlog`, `top_level_grid_view`, `snapshot`, `global_search_subfolder_restore`, `favsearch_subfolder_restore`, `items_are_global_search_view`, `items_are_tag_view`, `items_are_reading_history_view`, `items_are_bookmark_view`, `items_are_rating_view`, `items_are_subfolder_expansion_view`, `items_are_smart_folder_view`, `items_are_drive_list`, `reading_history_return_from`, `bookmark_view_state`, `bookmark_open_pending`, `fs_open_intent_from_grid`, `video_presentation_transition` | painted `fullscreen_page_layout`をsnapshotへ先にcapture。その後`fs_pending`をcancel/drainし、`texture_backlog`（#29–47）等のbackground queueをclearする。cache/layout/current frameはbundleに残る |
| 155–176 / 22 | `fs_zoom`, `fs_pan`, `fs_zoom_active`, `fs_zoom_aiming`, `fs_zoom_factor`, `fs_zoom_pdf_rerender_idx`, `fs_zoom_pdf_rerender_zoom`, `fs_pan_drag_start`, `fs_vertical_scroll`, `fs_seek_drag_active`, `fs_seek_gesture`, `fs_seek_overlay_visible`, `fs_vertical_cache_keep_set`, `continuous_page_transitions`, `fs_free_rotation`, `fs_rotation_drag_start`, `analysis_zoom`, `analysis_pan`, `analysis_pan_drag_start`, `analysis_overlay_cache`, `analysis_hist_cache`, `analysis_sv_cache` | zoom / pan / Z active/aiming/factor、scroll、free rotationは保持し、exact resolved placement / geometryへbake。park後にseek drag/gesture/overlayを終了し、continuous transitionをclearする |
| 177–205 / 29 | `spread_mode`, `final_cover_spread_preference`, `singleton_spread_placement_preference`, `spread_shift_anchor_idx`, `reading_flow`, `reading_direction`, `slideshow_playing`, `slideshow_next_at`, `slideshow_anchor_idx`, `continuous_reading_scroll_transition`, `slideshow_scroll_range_cache`, `pdf_password_request`, `pdf_current_password`, `pdf_password_pending_save`, `pdf_enumerate_pending`, `zip_enumerate_pending`, `fs_nav_after_pdf_enumerate`, `pending_auto_fs_open`, `pending_return_to_parent`, `pdf_placeholder_count`, `viewer_navigation_caches`, `fs_nav_locked_gen`, `fs_nav_dropped_block_signature`, `fs_nav_dropped_block_count`, `fs_load_skip_signature`, `fs_holdover_tex`, `fs_boundary_hint`, `virtual_folder_writeback`, `pdf_prefetch_grace_until` | spread/layout/preferences/directionは保持するがpassiveから再読しない。snapshot後にslideshow/scroll transition、enumerate/deferred-open、nav lock/signature、`fs_holdover_tex`を意図的terminalにする。holdover resource/projectionはclear前にDTOへcapture済み |
| 206–240 / 35 | `thumb_pixels`, `thumb_edit_preview_layers`, `thumb_edit_preview_keys`, `thumb_adjust_tex`, `passthrough_rendition_cache`, `adjustment_page_params`, `local_adjust_page_layers`, `local_adjust_pages`, `local_adjust_selected_layers`, `local_adjust_generation`, `local_adjust_cache`, `local_adjust_pending`, `export_crop_page_settings`, `export_crop_pages`, `mask_pages`, `comic_pages`, `conceal_pages`, `erase_mask_generation`, `conceal_mask_generation`, `edit_result_cache`, `final_ai_cache`, `final_ai_pending`, `final_ai_failed`, `final_composite_cache`, `final_effect_pending`, `adjustment_cache`, `erase_result_cache`, `erase_preview_cache`, `erase_base_cache`, `conceal_base_cache`, `conceal_cache`, `comic_cache`, `comic_bake_pending`, `erase_inpaint_pending`, `ai_classify_cache` | capture時に選ばれたrendition/post-filterをtextureと一体でbake。park後は`final_ai_pending`, `local_adjust_pending`, `comic_bake_pending`, `erase_inpaint_pending`をcancel/clearし、確定cache/edit設定は保持。編集foreground UIはpause前に終了 |
| 241–245 / 5 | `normalize_ui_states`, `normalize_auto_scan_suppressed`, `music_bookmarks`, `music_bookmarks_loaded_for`, `last_loop_pos` | bundle 保持。still passive DTO の対象外 |

既定値へ落ちても発見しにくい属性として、`fullscreen_page_slice`、`fs_zoom` / `fs_pan`、
`fs_vertical_scroll`、`fs_free_rotation`、`panorama_state` / `panorama_intent`、analysis一式、
`spread_mode` / 両 spread preference / `spread_shift_anchor_idx`、`reading_flow` /
`reading_direction`、`fullscreen_page_layout` を優先照合した。いずれも ordinary transaction のswap対象
であり、park terminal対象か否かを上表で分離した。必要なstill passiveの見た目はterminal前にsnapshotへ
bakeされる。監査で見つかった欠落はbundle fieldのcopyではなく、passive paged capture/bake境界の同じ
構造族だった。具体的には (1) canonical endpoint singletonのtyped side、(2) Z / ZipPlaのpainted
`ResolvedDisplayPlacement`、(3) navigation/source-reload中のlast-painted holdover resource / post-filter
provenance の3点で、いずれも今回の単一typed capture入力へ統合した。

### fork と foreground terminal の区別

- `split_current_context_preserving_main_grid` は245 fieldを全 destructureし、
  `duplicate_for_parked!` / `move_to_parked!` / `keep_in_main!` のいずれかへ明示分類する。field追加時は
  non-exhaustive destructureでコンパイルエラーになる。これは ordinary mount / deposit ではなく、
  main grid と新しい `LiveMedia` ownerを分ける fork である。media/fullscreen transient、render cache、
  zoom / pan / free rotation、analysis、slideshow、navigation holdover、music stateは parkedへmoveし、
  item identity/cacheの必要部分はduplicateする。grid worker、details/tag、edit/view-trim、spread preference、
  direction/layout等はmedia parked windowが駆動しないためmainに残す。`viewer_session` はmedia owner側へmove、
  physical mouse holdはowner変更をterminalとしてclearする。いずれも意図した fork policy であり、
  ordinary lifecycleの属性欠落ではない。
- materialized still fork はその exhaustive splitの後、`split_materialized_physical_context_for_detached_scope`
  が view-trim一式、`spread_mode`、両spread preference（今回のsingletonを含む）、shift anchor、
  reading flow/direction、thumbnail/edit/adjustment stateを明示copyする。新しい物理scopeに合わせて
  navigation scope / surface / selectionを作り、snapshot restore、rating suppression、details order、
  navigation cacheは意図的にresetする。表示属性のcopy漏れは無かった。
- `reset_detached_pause_foreground_modes` は passive still を作る前に panoramaのlive state、analysis mode、
  pointer gesture、panel hover、edit mode等のforeground-only interactionを終了する。panorama **intent**、
  book preference、zoom/scroll/layout等を落とす処理ではなく、ordinary bundle swapの欠落とは区別する。
- `pause_mounted_background_work_keep_current_frame` はsnapshot完成後にslideshow/seek/navigation holdover、
  enumerate/deferred-open、各pending workerを停止する。従ってこれらは245-field swapには含まれていても
  park後に値が同一であるべき属性ではない。今回のlast-painted holdoverはこのterminalより先にtexture、
  occurrence、post-filter、resolved placementをsnapshotへ移し、capture自体ではphaseを進めない。

### passive snapshot DTO の可視属性対応

| 可視属性 | snapshot / deferred view の所有方法 | 結果 |
|---|---|---|
| texture / rendition / post-filter / provenance | snapshot ownerがlast-painted `Holdover` / `Live`をtyped選択。holdoverなら旧resource + capture-time post-filterを使い、index再利用後のcurrent itemを再取得しない | **今回補完**。capture後のterminal clearより先に保持 |
| 保存回転 / 自由回転 | snapshotの `rotation` / `free_rotation`、frozen pageの `rotation` / `free_rotation`。continuous/spreadの自由回転は仕様どおり0 | 保持 |
| fit / scale limit / zoom / pan / Z | `image_rect_norm` または frozen `paint_rect_norm` に、capture-time exact `ResolvedDisplayPlacement`とlive同値transformからbake | **Z projectionを今回補完**。active/aiming、single/doubleを保持 |
| trim / content bbox / source region | directは `image_content_bbox`、frozenはcapture bboxをtransformへ渡し、texture visible-source UV + `uv_rect` + `clip_rect_norm`でpaint | 保持 |
| spread pages / gap / typed singleton side | capture-time `FsDisplayUnitHoldover` の1/2 page、exact placement、量子化gapをpaged producerが消費。active Z doubleだけgapをzoom倍 | **今回補完**。Left/Rightは1件のfrozen page、通常live Centerはdirect path |
| continuous layout / scroll | capture時のvisible page rectとgap alignmentをfrozen listへbake | 保持（`Some(ctx)` capture） |
| page / occurrence / request identity | snapshotはtexture/geometryだけを凍結し、canonical unit自体の1/2 page数を増やさない | singletonは1件のまま |
| underlay | `DetachedImageWindowUnderlay` にsolid/checkerをcapture。frozen pageも同じquadへ描画 | 保持 |
| detached client size / DPI | placementの `w/h` とcapture時 `pixels_per_point`をbake入力に使用 | 保持。DTOへ新規placement ownerは作らない |
| runtime window placement / activation | registry runtime/hostの既存ownerが保持し、deferred viewは既存runtime placementを投影 | 既存契約のまま。今回変更なし |
| margin color | App-global `fullscreen_image_margin_color` をpassive deferred描画時に投影 | 意図したglobal値。bundle fieldではない |
| title / location / reopen identity | snapshotのtitle/location/reopen descriptor/sync stamp | 保持 |

横断監査の結論は、ordinary transactionの**245 / 245 field swap coverage合格**（park semantic terminalは
別表）、materialized still forkの必要表示属性copy合格、LiveMedia forkの差分は全て明示policyである。
passive可視投影では、同じcapture/bake ownership境界にあった **singleton side / Z resolved placement /
last-painted holdover provenanceの3点**を今回補完した。他に同じ構造修正へ含める未保持属性は無い。
