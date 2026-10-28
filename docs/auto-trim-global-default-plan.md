# 自動余白カットの全体既定と本別継承

対象: v4.9.0 公開後の次リリース (当初は v3.8.0 の次。2026-09-11 に延期)  
状態: 2026-09-11 設計・独立レビュー済み。2026-10-28 に現行 master (c6951fafb) と照合して §0 の
改訂を加え、実装に着手 (branch `autotrim-1216`)。§0 と本文が食い違う箇所は §0 が優先する。

## 0. 2026-10-28 改訂 (現行 master との照合)

設計から 1 か月半の間に、同じ「本別の値 + 全体既定 + FollowGlobal」の形をした機能
(末尾表紙 `FinalCoverSpreadPreference`、単ページ端の配置 `SingletonSpreadPlacementPreference`、
表紙の次・最終ページの単独表示 `PageAlonePreference`) が実装・出荷された。本機能はこの前例と
**同じ経路・同じタイミング**に揃え、前例に無い独自の仕組みは足さない (CLAUDE.md「設計の簡素化」)。

### 0.1 前例に揃えるもの

- **保存と fallback**: `spread_db::set_final_cover_spread_preference` と同じく、root key の
  `FollowGlobal` は行を持たず、nested key (fallback がある本) の `FollowGlobal` は root fallback を
  遮る marker 行として保存する。§5 の削除規則はこの形で実装する。
- **解決の時点**: App / `ViewerContextBundle` が持つのは本別 preference (`ViewTrimBookPreference`) だけで、
  解決済み mode を state に持たない。表示側は描画のたびに `preference.resolve(全体設定)` を引く
  (`final_cover_spread_enabled_for_current_book()` と同じ)。全体設定を変えても state を書き換える
  必要がなく、古い解決結果が残る中間状態ができない。
- **全体設定の変更**: Preferences の OK で `install_preferences_settings` 後に新旧の bool を比べ、
  変わったときだけ (1) parked context のうち preference が `FollowGlobal` のものを
  `invalidate_final_cover_spread_display_in_parked_contexts` と同じ範囲で無効化し、
  (2) mounted context が `FollowGlobal` なら、トリム panel のモード変更と同じ
  `reanchor_continuous_reading_after_view_trim_change` + 再描画を行う。余白検出 cache
  (`fs_margin_bbox_cache` / detached の検出結果) は mode 非依存なので消さない。
- **passive な F12 窓の frozen snapshot**: 旧 §7 の「`DetachedImageWindowSnapshot` /
  `DeferredDetachedImageWindowView` に trim dependency generation を stamp する」は**採らない**。
  前例の末尾表紙・単ページ配置も passive 窓の frozen 表示を作り直さず、その窓の context が
  次に mount されたときに通常経路で作り直る。同じ割り切りに揃える (状態の組み合わせを増やさない)。
  passive 窓は全体設定を変えた時点では旧トリムのまま見え、クリック等で active になった時点で
  新しい解決結果になる。
- **Remote の全体設定**: `settings_db::RemoteReadingSettings` (末尾表紙等と同じ live snapshot) に
  bool を足し、Remote の解決もこの値を使う。client から bool を受け取らない。

### 0.1a Remote のキャッシュ (旧 §7 末尾の generation 追加は採らない)

2026-10-28 の調査で、現行の Remote は次のとおりと確認した (コード参照)。

- 本体プロセス側の Remote container は、ページ要求のたびに `remote_view_trim_plan_timed` で
  `view_trim.db` を読み直し、JPEG 化の直前に crop する。crop 結果は cache していない
  (`RemoteCompositeCacheKey` は crop 前の画素、`RemoteAutoTrimCacheKey` は検出結果だけ)。
- remote-web 側の generation は `view_trim.db` / `rotation.db` の `PRAGMA data_version` と
  favorites / sort_order で進む。末尾表紙などの settings_kv の値は generation に入れておらず、
  `crates/remote-web/src/store.rs` のテストが「関係ない設定の書込でページ資源を捨てない」ことを固定している。
- 本体と Remote は排他で使うため、本体側で設定を変える操作は Remote session の切り替わりをまたぐ。
  session が変わると Web 側は `invalidatePageResources(SESSION_INVALIDATED)` でページ資源を捨て、
  HTTP cache も `Vary: X-mIV-Remote-Session` で分かれる。

したがって全体設定は末尾表紙と同じく `RemoteReadingSettings` の live 読取だけで扱い、
`LibrarySnapshot` / generation の比較対象には**足さない** (旧 §7 の該当段落と §10 の
「Remote generation が進む」回帰は取り下げ)。1 ページ要求の中では settings を 1 回だけ読み、
見開きの相手ページの Auto 判定にも同じ値を使う。

### 0.2 書込経路の現状 (設計時から変わった点)

- 本別トリムの保存は `book_composition::service` の `Command::ViewTrimBook` と
  `save_view_trim(ViewTrimWriteBatch)` (本の構成編集・本への追加と同じ worker) を通る。
  §5 の削除規則・旧形式変換・invalid row の保護は `ViewTrimDb` の操作に置き、service・App・
  Remote・画像フォルダ ZIP 化・メタ情報転送の各 caller は同じ操作を呼ぶ。caller ごとに
  `is_removable` 相当を判定しない。
- `ViewTrimWriteBatch.book` は fallback の有無を運ぶ (nested marker の判定に必要)。
- 名前変更の key 移行・画像フォルダ ZIP 化・内容の復元は `state_json` を SQL でそのまま写すので、
  新形式の行もそのまま運ばれる。メタ情報転送は型として読み書きするので、DB と同じ codec を通す (§0.9 の 2)。

### 0.3 保存形式の確定

```json
{"version":2,"preference":"follow_global","apply_mode":"none","book_settings":{...}}
```

- `preference`: `follow_global / none / auto / manual`。
- `apply_mode`: **旧版へ戻したときのための写し**。`None→none / Auto→auto / Manual→book /
  FollowGlobal→none`。新版の reader は `version: 2` の行ではこの値を読まない。旧版は未知 field を
  無視して `apply_mode` と `book_settings` を読むので、明示の自動・手動はダウングレード後も効く。
- `version` が無い object は旧形式 (§3 の表で変換)。`version` が 2 以外・型違い・`preference` 欠落は
  `Invalid`。

### 0.4 invalid / DB error の扱い (まれな失敗の割り切り)

`Invalid` / `Error` は ログを残し、その本は**トリムなしで表示**する。root fallback へ進まず、
旧 margin-fit の移行書込もしない。行は書き換えず、利用者がその本で新しい値を選んだときだけ
上書きする。旧 §2・§4 にある「commit 済みの表示 state を保持する」ための追加の仕組みは作らない
(既定の割り切り方針。データは残るので失われるものはない)。

### 0.5 旧 margin-fit

旧 flag は、行が `Missing` (= FollowGlobal) か旧形式の `none` のときだけその本を `Auto` へ移行する
(現行の「保存値が None なら Auto」と同じ範囲)。新形式の明示値 (`FollowGlobal` marker を含む) は
上書きしない。旧 flag を新しい全体設定へ読み替えることはしない (§2 の方針どおり)。

### 0.6 読取口を 1 つにする

本別トリムを読む箇所は現在 6 か所ある (`App::apply_view_trim_for_key_with_fallback`、
`metadata_import_refresh` の再読込、`book_composition::service` の `ViewTrimBook`、
Remote container の `remote_view_trim_plan_timed`、Remote の get / set handler (`remote_ipc/ui.rs`))。
このうち `metadata_import_refresh` だけが root fallback を見ていない。すべて
`ViewTrimDb::load_book_preference_with_fallback(exact, fallback) -> Result<ViewTrimBookLoad, _>`
(exact → `Missing` のときだけ fallback) と、純関数の resolver を通す。

### 0.7 実装の範囲 (旧 §9 の置き換え)

旧 §9 の「§1.209 作業中の共有 file」は解消済み。並行作業 (ページめくり・一覧表示・全画面比較・
編集反映コピー・依存物更新) と共有する大きな file (`src/app.rs`、`src/ui_fullscreen.rs`) は、
本機能では field の型と読取口の差し替えに留め、構造を書き換えない。

1. `view_trim.rs` / `view_trim_db.rs`: `ViewTrimBookPreference`、versioned JSON、
   `ViewTrimBookLoad`、fallback 付き読取、fallback を知る書込 (`set_book_state(exact, fallback, state)`、
   `ViewTrimWriteBatch.book` に fallback 有無)、旧 margin-fit の判定関数、純 resolver。
2. 設定: `Settings::auto_view_trim_default_enabled` (既定 false)、settings_db の KV、
   `settings_transfer`、Preferences の「閲覧表示」ページ (末尾表紙の隣、検索 anchor `spread/auto-view-trim`)、
   未保存比較、OK 時の無効化 (§0.1)、`RemoteReadingSettings`。
3. 本体: App / `ViewerContextBundle` の `view_trim_apply_mode` を `view_trim_book_preference` に置き換え、
   表示側は resolver 経由で解決。panel は 4 ラジオ。
4. Remote: container の解決、get / set handler、`normalize_remote_view_trim_state`、Web panel の 4 値、
   `PROTOCOL_VERSION` 更新 (本体・service・Web asset)。
5. 文書: display-pipeline、web-remote-plan、マニュアル (fullscreen / tut-reading)、製品ページの該当箇所、
   backlog。

### 0.8 回帰 (§10 への追加・変更)

- §10 の「Remote generation が進む」は取り下げ。代わりに「1 ページ要求の解決と見開き相手の Auto 判定が
  同じ live settings 値を使う」「全体 ON で行なしの本の Remote ページが Auto で crop される」。
- passive F12 snapshot の generation 回帰は取り下げ。代わりに「parked context は FollowGlobal だけ
  layout を無効化し、明示値の context と余白検出 cache は変えない」。
- 旧版への写し: `version: 2` 行を旧 `ViewTrimBookState` の serde で読むと、明示 Auto / Manual が
  `auto` / `book` として読める。
- `metadata_import_refresh` の再読込が root fallback を含め通常の読込と同じ結果になる。

### 0.9 2026-10-28 独立設計レビュー (Codex gpt-6.1-sol xhigh) の結果と決定

判定は ACCEPT with changes。指摘と決定は次のとおり。

1. **[P2] Preferences の OK と Remote の取得が同じ frame で重なると、設定確定が session 境界を
   またがない経路がある** (Remote modal の登録が Preferences 描画より後)。末尾表紙など既存の
   全体設定にも同じ穴がある。→ **本機能では閉じない**。Preferences の確定全体と Remote 取得の排他は
   全体設定すべてに共通する問題で、本機能だけ別経路にすると前例と食い違う。起こるのは、開いておいた
   Preferences の OK と Remote 端末の接続がたまたま同じ frame になったときだけで、結果も「その session で
   取得済みのページが旧トリムのまま見える」(データは失われない、次の session で直る) に留まる。
   別項目としてバックログへ起票する。
2. **[P2] メタ情報転送は `state_json` を型として読み書きしている** (`PortableContainerState.view_trim`、
   export で deserialize・import で serialize)。§0.2 の「そのまま写す」はメタ情報転送には当たらない。
   → DB・portable 形式の両方が**同じ codec** (旧形式と version 2 を読み、version 2 を書く) を通す。
   旧 package の import、version 2 の 4 値と手動値の往復、nested marker、旧版の reader での写しを回帰に足す。
   import は利用者の明示操作で宛先の設定を置き換えるものなので、宛先の行が invalid でも既存どおり置き換える。
   package 側の値が invalid なら既存どおり import を中止して報告する。
   名前変更の key 移行・画像フォルダ ZIP 化・内容の復元は JSON を解釈せずに行ごと写すので、codec は不要
   (bytes が保たれる回帰だけ足す)。
3. **[P2] parked の無効化を末尾表紙と同じ範囲にすると、pan・自由回転まで 0 に戻る**。mounted 側
   (トリム panel と同じ再 anchor) は保持するので差が出る。→ parked 側は layout・連結読みの keep set・
   遷移・slideshow の範囲 cache だけを無効化し、連結読みは panel と同じ再 anchor 状態にする。
   pan と自由回転は保持する (回帰を足す)。
4. **[P3] invalid / error のとき利用者への通知が無い**。→ 既存の通知経路で
   「表示トリムの設定を読み込めないため、トリムなしで表示しています」を出す (再試行や復旧の仕組みは足さない)。

補足 (レビューで確認された点):
- 読取の失敗経路は 1 本にし、`Missing` だけが fallback へ進む (`ViewTrimBookLoad` の中の Error と外側の
  `Result` を併存させない)。
- 未保存の panel 値を転送する batch (`take_pending_view_trim_transfer`) も fallback 付き key で書く。
- `canonical_demand.rs` の見開き側の直接読取も resolver へ置き換える。
- passive F12 窓は、再 active 化で snapshot が取り除かれ保持 context のライブ描画へ戻る
  (`app.rs` の snapshot 除去)。受動描画中は snapshot の bbox だけを読む。

### 0.10 実装と独立レビュー (2026-10-28)

実装は Codex gpt-6.1-sol high (コミット 7c40410bb、b372f6589)。別の Codex gpt-6.1-sol xhigh の独立レビューは
ACCEPT with changes → 修正後 ACCEPT。検収で決めた点:

- **手動設定を選ぶと `book_settings.enabled = true`** (出荷済み動作。本体 panel・Remote Web とも)。実装の途中で
  この暗黙の ON が外れ、検収 (本体) と独立レビュー P2 (Web) で戻した。他の選択肢へ切り替えても手動の値と
  enabled はそのまま残す。
- 手動設定の適用範囲で「本全体」を選んでページ個別行を解除する出荷済み動作は維持。モード切替と全体設定の
  変更ではページ個別行を消さない。
- Remote の JPEG worker 単独の経路で設定が読めない場合は、ログとトリムなし表示だけにする (通知なし)。
  Remote でトリムのパネルを開くと既存の通知が出る。まれな失敗の既定の割り切り。
- Remote Web の「全体設定に従う」の括弧表示は、その選択中だけサーバ応答の現在値を出す。

実機での確認 (2026-10-28、観測者 ClaudeCode (親セッション)。利用者の許可を得て `--data-dir` で隔離したデータと
autotrim-1216 の dev-runtime ビルドを computer-use で操作。白い余白付きの ZIP と PDF):

- 全体 OFF: ZIP の本は余白付きのまま (従来どおり)。
- 全体 ON (環境設定「表示 → 閲覧表示」で ON にして OK): 未設定の ZIP・PDF とも自動余白カットがかかった。
  表示トリムのタブは 4 択で「全体設定に従う (現在: 自動余白カット)」と表示。
- 「トリムなし」: 選ぶと即時に外れ、終了・再起動後も「トリムなし」のまま。
- 手動設定: 「自動検出」で入った値 (上下左右 16〜18%) が即時に効き、「トリムなし」→「手動設定」と戻しても値が残り即時に効いた。
- 未確認: 見開き・連結読み・F12 別窓での追従、Remote、ページ個別の手動値。

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
