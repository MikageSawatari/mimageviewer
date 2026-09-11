# 自動余白カットの全体既定と本別継承

対象: v3.8.0 公開後の次リリース  
状態: 設計のみ。製品コード、DB、Remote protocol、UI は未変更・未検証。

## 1. 目的と範囲

環境設定の「表示 > 閲覧表示」に、既定 OFF の次の項目を追加する。

> 自動余白カットを標準で有効にする

各本の表示トリムは次の 4 値を保存する。

1. 全体設定に従う
2. トリムなし
3. 自動余白カット
4. 手動設定

未設定の本は全体設定に従う。明示した本別モード、本全体の手動 margin、ページ個別の
手動 margin は、全体設定の切替や別モードの一時選択で書き換えたり削除したりしない。
表示トリムは従来どおり表示専用であり、補正、AI、export crop、保存画像の画素は変えない。

本設計は local folder、ZIP / 対応アーカイブ、PDF、単ページ、見開き、連結読み、F12 の
viewer context、末尾表紙補助、mIV Remote の解決規則をそろえる。余白検出アルゴリズム、
20% 上限、回転時の無効化、見開き上下の調停は変更しない。

## 2. 現状確認と破るべきでない契約

- `src/view_trim.rs` の `ViewTrimApplyMode` は `None / Auto / Book / Page` で、`Page` は
  enabled なページ行から導出される表示上の mode である。保存 selection と表示結果が同じ
  enum に混在している。
- `ViewTrimBookState::is_removable` は `None + ViewTrimBookSettings::default()` を削除対象にし、
  `src/view_trim_db.rs` の `set_book_state` / `apply_write_batch` は実際に本行を DELETE する。
- 本体は `src/app.rs::apply_view_trim_for_key_with_fallback` で exact 本キー、次にネスト ZIP の
  root fallback を読み、`src/ui_view_trim.rs` でページ行を重ねる。Auto は
  `fs_margin_bbox_cache` に保存した元 raster の検出結果を使う。
- 連結読みも各 idx の同じ resolver と `content_bbox` を使う。F12 は mode、book settings、
  page overrides、検出 cache、描画 geometry を `ViewerContextBundle` に保持している。
- Remote は `src/remote_ipc/container.rs::remote_view_trim_plan_timed` で同じ DB key、legacy
  margin-fit、ページ行を読み、JPEG 化の直前に trim する。Remote Web の現在の UI / JSON は
  `none / auto / book` の 3 値だけを受理する。
- 旧 `FullscreenFitMode::MarginFit` / `Settings::margin_fit_enabled` は互換入口であり、本を開いた
  ときに Page fit + book Auto へ移行する。この値を新しい全体既定として再利用しない。
- `cached_margin_bbox` は source raster の identity (`load_seq` と Arc pointer) に属する検出 cache
  であり、適用 mode の cache ではない。全体既定を切り替えるだけで再検出してはならない。

## 3. 過去データの判別限界

既存版では、保存 mode が `None` で book settings も既定値なら本行を削除する。そのため、現在
DB に行が無い本について、過去に一度も設定していないのか、利用者が明示的に「トリムなし」を
選んだのかを復元できない。

次版では **行なしを「全体設定に従う」** と解釈する。新しい全体既定は OFF なので、更新直後の
表示は変わらない。利用者が全体既定を ON にした後は、過去の明示 OFF が削除済みだった本も
Auto になる可能性がある。推測による移行表や path ledger を新設して両者を作り分けない。

既存行が残っている場合は row presence を証拠に変換する。

| 旧 JSON の `apply_mode` | 次版の本別 preference |
| --- | --- |
| `auto` | 自動余白カット |
| `book` | 手動設定 |
| `none` | トリムなし |
| 不正な `page` | 現行の正規化と同じくトリムなし |

`none` 行に非既定の手動値が残っている場合、その値も保持する。ページ個別行は別 table のため、
本行の移行、全体設定の変更、Auto / None / FollowGlobal の選択では触らない。

## 4. 型と解決規則

保存 selection と表示用 mode を別型にする。

```rust
enum ViewTrimBookPreference {
    FollowGlobal,
    None,
    Auto,
    Manual,
}

enum ViewTrimBookLoad {
    Missing,
    Legacy(LegacyViewTrimBookState),
    Current(ViewTrimBookState),
    Invalid(ViewTrimStateError),
    Error(ViewTrimDbError),
}

enum ViewTrimApplyMode {
    None,
    Auto,
    Book,
    Page,
}
```

`ViewTrimApplyMode` は既存描画側の解決済み結果としてそのまま再利用する。保存 state、App / bundle の selection、
Remote 編集 UI は `ViewTrimBookPreference` を持つ。`Inherit` を `ViewTrimApplyMode` に足して
consumer ごとに fallback させたり、`None` と row absence を bool で補ったりしない。

`ViewTrimBookLoad` は概念上の DB 読取結果である。現在の `get_book_state -> Option` のように row missing、
SQL error、invalid JSON を同じ `None` に畳まない。`Missing` だけが container fallback と
`FollowGlobal` へ進める。`Invalid / Error` は resolver の terminal failure とし、親 row への fallback、
既定値の保存、移行 write を行わない。すでに commit 済みの表示 state があれば保持し、初回読取なら
no-trim の安全な表示を一時的に選べるが、それを persisted `None` として扱ったり DB へ書いたりしない。

共通 resolver の入力は次である。

```text
stored preference provenance (missing / legacy row / current row / invalid / DB error)
global_auto_trim_default
legacy margin-fit state
optional enabled page override
```

解決順は次の一箇所に固定する。

1. current schema の明示 preference を採用する。
2. legacy row は §3 の表で current preference へ変換する。
3. invalid row / SQL error はその key の terminal failure とし、fallback や write をしない。
4. exact row が `Missing` の場合だけ既存の container fallback を見る。
5. exact な `FollowGlobal` は実値として fallback を止め、環境設定へ進む。
6. fallback にも行が無ければ `FollowGlobal` とする。
7. `FollowGlobal` は global bool により resolved `Auto` / `None`、他の 3 値は対応する resolved
   mode にする。
8. resolved `Manual` のときだけ enabled なページ行を `Page` に昇格する。

これにより同じ本・ページ・global snapshot は、本体と Remote で必ず同じ effective mode になる。
book settings と page override は resolver の出力に従って参照するだけで、非選択中も保存値を保持する。

## 5. 保存、fallback、旧 margin-fit

`view_trim_books.state_json` は table schema を変えず、version 付きの current JSON へ更新する。
DB reader は current と旧 `apply_mode` shape を明示的に分けて読む。新規 write は current shape
だけを出し、metadata transfer / restore も同じ decoder / encoder を通す。

reader は最初に JSON object と `version` field を最小限だけ厳密に読む。`version` が無い既知の
旧 shape だけを `Legacy`、対応する exact version だけを `Current` とする。未知 version、version の
型違い、current schema の必須 `preference` 欠落は `Invalid` であり、Serde の `default` や旧 enum の
`None` へ落とさない。SQL prepare / row read の失敗は `Error` として別に返す。invalid/error row は
metadata transfer、rename、restore、通常 UI 保存のいずれも暗黙修復・上書きしない。利用者がその本へ
新しい明示値を保存する操作だけが置換を許し、その場合も既存の設定保存エラー報告を維持する。

削除規則は `ViewTrimBookState::is_removable` 単独から、exact / fallback を知る DB operation へ移す。

- root 本の `FollowGlobal + 既定 book settings` は行を削除できる。
- ネスト本で `FollowGlobal` を明示した場合は、root fallback を遮断する marker row を残す。
- `None / Auto / Manual` は明示値なので、book settings が既定でも行を残す。
- `FollowGlobal` でも保持すべき手動 book settings があれば行を残す。
- page rows は「このページの個別設定を解除」という明示操作だけが削除する。

旧 margin-fit は global default より先に処理する。legacy flag と旧 row / row absence の組み合わせは
現行どおり current book の Auto へ移行し、Page fit へ戻して旧 flag を保存する。current schema の
明示 `None` を legacy flag が上書きしてはならない。Remote の read-only 表示は同じ resolver で
legacy Auto を見せ、書込が起きたときだけ既存の保存・legacy flag cleanup 境界を使う。

## 6. UI

### 6.1 環境設定

`src/ui_dialogs/preferences/pages.rs::page_spread_mode` の「表示 > 閲覧表示」に、末尾表紙の全体設定と
同じ階層でチェックボックスを追加する。ラベルは「自動余白カットを標準で有効にする」、既定 OFF。
説明は「本ごとの設定が優先される」ことと、回転ページでは従来どおり適用しないことを示す。
snapshot と設定検索 anchor を追加する。

### 6.2 本体の表示トリム panel

現在の 3 ラジオを次の 4 ラジオへ置き換える。

```text
全体設定に従う（現在: 自動余白カット / トリムなし）
トリムなし
自動余白カット
手動設定
```

括弧内は resolver の結果を表示するだけで保存値にしない。手動設定の「本全体 / このページ」、
左右連動、スライダー、自動検出、リセット、個別設定解除の意味は維持する。別 mode を選んでも
手動値を消さず、Manual に戻したとき exact page row を再利用する。

### 6.3 Remote Web

Remote panel も 4 値を表示し、サーバが返した persisted preference を選択状態にする。JS は
global fallback を独自計算せず、サーバが返す effective label / mode を表示するだけにする。
opaque JSON の受理値が変わるため Remote IPC protocol version を更新し、本体、Remote service、
embedded Web asset の同梱版をそろえる。

## 7. 設定変更と表示 cache

環境設定の OK では変更前後の global bool を比較し、**effective mode が変わる FollowGlobal context
だけ**を各 owner 内で無効化する。

- mounted viewer は `fullscreen_page_layout` と trim を焼き込んだ表示 snapshot / transform を
  退役し、paged を次 frame で再構築する。
- active continuous reading は current unit を既存
  `ContinuousReadingScrollTransition::AwaitingReanchor` へ載せ、旧寸法の scroll range を捨てる。
- AtRest / F12 / multi-window bundle は mount せず、その bundle の layout、frozen/deferred
  content bbox、continuous range を無効化する。明示 `None / Auto / Manual` の兄弟 context は触らない。
- passive F12 には bundle 外にも `DetachedImageWindowSnapshot` と
  `DeferredDetachedImageWindowView` が trim 済み content bbox / frozen page を保持する。各 snapshot は
  exact window → `ViewerContextId` と source/items generation に加え、trim dependency を
  `FollowGlobal(settings generation) / Explicit` として stamp する。global 変更では FollowGlobal のものだけを
  stale にし、次回 materialize 時に保持 raster と margin detection cache から作り直す。旧 settings generation
  の deferred completion は exact owner へも install しない。passive window を検査のため mount したり、
  別 context の snapshot を一括破棄したりしない。
- 末尾表紙補助は source idx 0 の別 occurrence でも同じ preference / page row を使い、表示 slot の
  Left / Right semantics だけを現在の composition から渡す。navigation 列や最終表紙設定は変えない。
- `fs_margin_bbox_cache` と detached の raw raster に対する検出結果は mode 非依存なので保持する。
  画像 source / load identity が変わったときだけ既存規則で失効する。
- 同じ表示変更で補正、AI、export crop、thumbnail raster cache を消さない。

Preferences の既存 commit 内で新 bool を App state に install して保存し、表示の無効化は install 後の
side effect として行う。再描画が旧 bool を再利用する中間状態を作らず、保存失敗の扱いは既存の設定保存
policy に従う。UI thread で余白検出、PDF decode、ZIP read を同期実行せず、最初の再描画は既存 raster /
worker 経路を使う。

Remote は server-owned settings DB snapshot から global bool を読み、client が送る bool を信用しない。
`RemoteReadingSettings` には global bool と旧 margin-fit の移行判定に必要な flags を含め、1 request の
resolver、Auto partner、legacy migration 判断が同じ stable snapshot を使う。1 page 内で設定を重複 read
しない。

`crates/remote-web/src/store.rs` の `LibrarySnapshot` / `read_stable_settings_snapshot` もこの値を所有する。
現状の Remote generation は favorites / sort 等の snapshot 比較で進み、settings DB の data version だけを
見れば新 bool の変化を必ず検出できる契約ではない。generation 比較対象へ global bool と必要な legacy
flags を明示的に加え、値が変わったら page cache / viewer group を進める。同じ Remote generation から
異なる trim 画素を返さず、snapshot 採取途中に settings generation が動いた場合は既存 stable-read retry
境界で一式を読み直す。

## 8. 表示経路の適用表

| 経路 | 正本と追加確認 |
| --- | --- |
| 通常 folder | `spread_container_key` の exact row、page key、global snapshot を共通 resolver へ渡す |
| ZIP / 対応アーカイブ | exact nested key → root fallback → global。exact FollowGlobal marker は root fallback を止める |
| PDF | folder / ZIP と同じ resolved mode を PDF display target の content bbox 計算前に使う |
| 見開き | 両 idx を個別解決し、Auto の上下調停と手動 spread-side semantics を維持する |
| 連結読み | visible / keep unit の各 idx を同じ resolver で解決し、変更時は current unit を再 anchor する |
| F12 / multi-window | preference、book settings、page rows は ViewerContextBundle owner。bundle 外の passive snapshot も window→context と trim dependency generation を持ち、FollowGlobal だけ再materializeする |
| 末尾表紙補助 | source page identity は同じまま occurrence の表示 side だけを使い、補助 page を別設定にしない |
| Remote | DB と server-owned live global setting を同じ resolver へ渡し、JPEG crop と UI state を一致させる |

## 9. 実装 ownership と順序

現在 `src/app.rs`、`src/app/native_video.rs`、`src/ui_fullscreen.rs`、
`src/app/viewer_context_registry.rs`、`src/remote_ipc/ui.rs` は別の §1.209 作業中である。所有返却と
source freeze 後に一つの coherent chunk として接続する。

| 段階 | 主な path | 内容 |
| --- | --- | --- |
| A: 型と互換 reader | `src/view_trim.rs`, `src/view_trim_db.rs` | preference / resolved mode、versioned JSON、row presence / fallback、削除規則、legacy decoder |
| B: 設定 | `src/settings.rs`, `src/settings_db.rs`, `src/ui_dialogs/preferences.rs`, `src/ui_dialogs/preferences/pages.rs` | bool の default / save / live Remote snapshot、UI、適用時 invalidation trigger、snapshot fixture |
| C: 本体 resolver | `src/app.rs`, `src/ui_view_trim.rs`, `src/ui_fullscreen.rs` | load / persist / legacy migration、4 値 panel、初回表示、paged / spread / continuous / PDF |
| D: context ownership | `src/app/viewer_context_registry.rs`, `src/app/snapshot_ops.rs` と関連 tests | bundle swap / park / restore / retire、passive detached snapshot の owner / generation、FollowGlobal context だけの invalidation、F12 frozen display |
| E: metadata lifecycle | `src/app/metadata_import_refresh.rs`, `src/metadata_transfer.rs`, `src/rename_key_migration.rs` | versioned state の import/export/rename、book/page値の完全保持 |
| F: Remote | `src/remote_ipc/{mod,container,ui}.rs`, `src/settings_db.rs`, `crates/remote-ipc/src/lib.rs`, `crates/remote-web/src/store.rs`, `crates/remote-web/web/{app.js,app-runtime.test.mjs}` | 同じ resolver、global + legacy flags の stable snapshot、4 値 UI、generation/cache、protocol update |
| G: docs | `docs/display-pipeline.md`, `docs/web-remote-plan.md`, `docs/detached-rework-plan.md` §11、必要な利用者向け文書 | 実装後の正本と検証記録。release 文書・version 更新は release owner |

実装は A の純型 / DB fixture を先に検収し、その source を B〜F へ接続する。App に一時 bool や
`Option` を足して consumer を段階的に分裂させない。C〜F は共有 file の所有権を確保してから行い、
compile 不能な片側だけの commit を作らない。

## 10. 必須回帰

### 型・DB・移行

- default global OFF + row absence は None、global ON + row absence は Auto。
- 明示 None は global ON でも None、明示 Auto は global OFF でも Auto、Manual は global に依存しない。
- Manual の enabled page row だけ Page になり、None / Auto / FollowGlobal→Auto 中は行を参照しない。
- exact missing は legacy root fallback を使い、exact FollowGlobal は fallback を遮断する。
- old `auto / book / none / page` JSON、current exact version JSON、unknown version、必須 field 欠落、
  malformed JSON、SQL error を区別し、旧 None row と row absence を混同しない。
- invalid/error exact row は root fallback、default write、legacy migration を起こさず、commit 済み表示 state
  と DB bytes を保持する。初回の安全表示も persisted None には昇格しない。
- root FollowGlobal default だけ削除でき、nested FollowGlobal marker、明示 None、非既定 manual values は残る。
- global toggle、mode toggle、metadata transfer、rename、restore の前後で book margin と page rows の
  bytes / typed valuesが変わらない。
- legacy MarginFit は current book Auto + Page fit へ一度だけ移行し、current explicit None を上書きしない。

### 本体表示と context

- local image、ZIP nested fallback、PDF の初回 frame が同じ effective modeを使う。
- single / LTR spread / RTL spread / rotation / split / continuous で既存 bbox、side、調停が維持される。
- global toggle は FollowGlobal の mounted / AtRest / F12 contextだけを再layoutし、明示 override の兄弟、
  source raster、Auto detection cache、page DB を変えない。
- continuous は設定変更後に current unit を保って再anchorし、旧寸法の hit-test / scroll rangeを使わない。
- F12 の mount→deposit→remount、close、context retire、snapshot restore で preference と page overrides が
  owner を越えない。
- `DetachedImageWindowSnapshot` / `DeferredDetachedImageWindowView` は FollowGlobal のときだけ global change で
  stale になり、exact window→context/source/settings generation で再materializeする。旧 generation の
  completion は拒否し、明示 override の sibling snapshot と margin detection cache は維持する。
- final-cover supplement は同じ表紙 page の trim identity を再利用し、表示側だけ正しく変換する。

### UI・Remote

- Preferences の default OFF、検索結果、適用 / 取消 / reopen、global checkbox snapshot。
- 本体 4 ラジオの selected / effective 表示、Manual 値保持、個別行を削除する明示操作。
- Remote 4 値 normalize / render / write、古い JSON、protocol mismatch、same-generation page cache。
- desktop で global setting または resolver に必要な legacy margin-fit flag を変えると、favorites / sort が
  不変でも `LibrarySnapshot` 比較で Remote generation が進み、次の group / page は一式の live snapshot を使う。
- Remote の local / ZIP / PDF / spread partner / final-cover occurrence が本体 pure resolver と同値。

## 11. 検証計画

狭域は `view_trim` / `view_trim_db`、preferences snapshot、viewer-context ownership、Remote container / IPC /
Web runtime の順に行う。次に core と Remote の `cargo check`、`cargo fmt --all -- --check`、UI glyph
check、`scripts/test-full.ps1 -SuppressCrashDialogs` を同じ source hash で一度行う。ユーザーが表示を
確認できる変更なので、成功後に通常 `build-dev.ps1` と使い捨て `prepare-portable-smoke.ps1` を既存の
process 保護手順で準備する。agent による UI 起動・入力は別途 concrete suite の承認があるまで行わない。

実機候補は global OFF / ON、4 book preferences、ZIP / PDF、見開き左右、連結読み、F12、Remote の
代表組み合わせである。過去の削除済み明示 OFF を自動判別できない点も確認案内に含める。

## 12. 概算

共有 file の所有返却後、実装と狭域回帰に 2〜3 開発日、Remote / multi-context / snapshot の統合と
独立 review 修正に 1〜2 開発日、full gate・build・実機確認に 0.5〜1 開発日を見込む。合計は
**3.5〜6 開発日**。旧 JSON と nested fallback を保たず単純に enum を増やすだけなら短くなるが、
保存済み設定の消失、Remote の表示差、F12 の stale layoutを生むため採らない。
