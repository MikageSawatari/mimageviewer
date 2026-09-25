# §1.239 / §1.240 — 見開き端の個別配置と端近くの単独表示

2026-09-24。§1.239 を先に実装・独立レビューする。§1.240 は同日の新モデルに改め、実ページの強制単独表示として別実装する。**利用者決定により §1.239 と §1.240 は v4.1.0 の対象。**要件の経緯は [backlog §1.239–1.242](backlog-on-hold.md) にある。

## 1. 実装前の所有境界（2026-09-24 調査時点）

- 全体の `Settings::singleton_spread_placement_enabled` は bool、既定 OFF。本別 `SingletonSpreadPlacementPreference` は `FollowGlobal / Place / Center` と `effective()` を持つ。`settings.db` は JSON 値の `settings_kv` で、Remote は毎要求の `load_remote_reading_settings` を読む（`src/settings.rs:3109`, `src/settings.rs:4563`, `src/settings.rs:6775`, `src/settings_db.rs:16`, `src/settings_db.rs:805`）。
- 本別設定は `spread.db::singleton_spread_placements(path, preference)`。nested exact → root fallback、nested の明示 `FollowGlobal` 行は root fallback を遮る。read-only 旧 DB は表不在を継承として扱う（`src/spread_db.rs:37`, `src/spread_db.rs:74`, `src/spread_db.rs:226`, `src/spread_db.rs:255`）。
- 本体 App は実効 bool を解決し、book load で DB を読む。`ViewerContextBundle` は値を mount/swap/fork/park へ運び、全体変更は継承中の parked context を無効化する（`src/app.rs:21277`, `src/app.rs:21350`, `src/app/viewer_context_registry.rs:987`, `src/app/viewer_context_registry.rs:2216`, `src/app/viewer_context_registry.rs:3223`）。
- 明示 metadata transfer の container state、import refresh、rename/purge、`SpreadDb::clear_all/count` に接続されている（`src/metadata_transfer.rs:456`, `src/metadata_transfer.rs:2517`, `src/metadata_transfer.rs:4776`, `src/app/metadata_import_refresh.rs:544`, `src/rename_key_migration.rs:1046`, `src/spread_db.rs:358`）。自動 sidecar は対象外（`docs/section218-singleton-spread-placement.md`「確認した現行境界」）。
- UI は環境設定 checkbox と本の見開き menu にあり、変更時に live/parked display を無効化する（`src/ui_dialogs/preferences/pages.rs:9128`, `src/ui_dialogs/preferences.rs:2397`, `src/ui_fullscreen.rs:41158`, `src/ui_fullscreen.rs:35554`）。
- `build_image_reading_indices` から `build_spread_display_units_with_predicates` へ実 `GridItem` index 列、mode、ずらし anchor、回転後横長・pairable 判定を渡す。cache token は item 世代、mode、ずらし、横長 epoch、nav 列。composition は unit 形成理由、complete-book proof、末尾表紙補助から placement を決める（`src/ui_fullscreen.rs:12875`, `src/ui_fullscreen.rs:13717`, `src/ui_fullscreen.rs:13772`, `src/ui_fullscreen.rs:14045`, `src/ui_fullscreen.rs:14118`, `src/ui_fullscreen.rs:14357`, `src/ui_fullscreen.rs:16003`）。
- paged/連結、holdover、F12 passive snapshot は composition または capture 済み geometry を使う。snapshot の片側配置は frozen geometry が必須で、active/parked の絵を一致させる（`src/ui_fullscreen.rs:7700`, `src/ui_fullscreen.rs:9710`, `src/ui_fullscreen.rs:17166`, `src/ui_fullscreen.rs:17201`, `docs/multiwindow-scenario-test-plan.md:108`）。
- Remote は `spread.db` を read-only で解決し、live reading settings と共通 builder を使って `PageGroup.singleton_placement` を送る。typed write は UI thread、Web は group の placement で DOM layout を解く（`src/remote_ipc/container.rs:4863`, `src/remote_ipc/container.rs:4897`, `src/remote_ipc/ui.rs:2267`, `crates/remote-ipc/src/lib.rs:1057`, `crates/remote-web/web/app.js:4783`, `crates/remote-web/web/app.js:6085`）。

## 2. §1.239 — 先頭と末尾を独立させる

1. `EndpointPlacementSettings { first: bool, last: bool }` と `EndpointPlacementPreferences { first, last: SingletonSpreadPlacementPreference }` を一つの値として App/context/Remote に渡す。各端だけ `effective(global_end)` で解決する。1 unit が両端なら**先頭優先**という現行規則を維持する。geometry は §1.218 の `Center/Left/Right` と仮想二枠 canvas をそのまま使う（`src/ui_fullscreen.rs:13717`、`docs/section218-singleton-spread-placement.md`「型と判定の唯一の所有者」）。
2. `settings.db` の新キー `singleton_spread_first_enabled` / `singleton_spread_last_enabled` は旧 `singleton_spread_placement_enabled` から両方へ同じ値を写す。**選択する failure-safe 規則は「committed marker が無い間は旧値を両端へ投影」**。専用 migration marker と新二キーを一 transaction で書き、marker がある場合に限り新二キーを正本として読む。旧キー無しは両方 OFF。marker 無しで新キーが 0/1/2 個ある状態はすべて未完了として扱い、特に**片方だけある状態を不完全と診断**する。旧キーから両端へ投影して次回再試行し、未完了中の `save_full` と個別設定書込を抑止する。marker 有りで片キー欠損・不正なら旧値/既定値へ黙って戻さず読込エラーと保存抑止へ送る。書込失敗でも利用者に旧 ON の見た目を保持でき、既存 loader の「migration error を記録して継続」による quarantine 誘発を避けられるためこの規則を選ぶ（`src/settings_db.rs:625`, `src/settings_db.rs:627`, `src/settings_db.rs:853`）。
   新規 `settings.db` の初回 `save_full` は両キーと完了 marker を同じ transaction で書く。保存抑止は既存 DB の未完了移行にだけ適用する。旧 JSON/backup 読込も旧 bool→両端の互換変換を行い、復元した旧 `settings.db` は marker 無しとして再移行する。Remote `load_remote_reading_settings` も同じ marker-gated helper を使い、未完了なら旧キーの live 値を両端へ投影する。移行失敗・不完全状態を起動時設定の古い clone で覆わない。released settings の移行は必須（`src/settings_db.rs:48`, `src/settings_db.rs:805`, `CLAUDE.md:1654`）。
3. `spread.db` は新 table `singleton_spread_endpoint_placements(path PRIMARY KEY, first_preference, last_preference)`、各 0=FollowGlobal / 1=Place / 2=Center と CHECK。旧 table の**全 row** を `(old, old)` へ copy し、専用 completed marker と共に一 transaction で commit する。table の存在だけを移行完了とみなさない。marker 有りの read は新 table のみを使い、表/列欠損はエラー。marker 無しの read は旧 table を両端へ投影し、書込可能 open は次回 transaction を再試行する。途中で新 table のみ作られていた場合は旧 table から再構成して commit し、未完了中の新 endpoint 書込は拒否する。read-only 旧 DB は旧 table の root/nested row を両端へ投影し、旧 table も無ければ両端 `FollowGlobal`（`src/spread_db.rs:65`, `src/spread_db.rs:84`, `src/spread_db.rs:240`）。
   root の両端 FollowGlobal は row 削除、nested は `(0,0)` を保持して root fallback を遮る。片端だけ FollowGlobal の nested row も保持する。旧 `spreads`/final-cover 行は生成しない。移行失敗時に旧 row は不変で retry 可能。App の書込可能 open が移行失敗した場合も既存 DB を read-only で開き直して旧 row を各本へ投影し、本別書込を拒否する。この App は read-only handle を保持するため、その起動中は再移行せず、次の起動時の writable open で再試行する。UI thread の本切替時に同期 DB 移行を追加しない。marker 有りなのに新 row/表が壊れた場合は旧値へ silent fallback しない。schema/query error は `Result` として App と Remote まで伝え、`FollowGlobal` に見せかけない。本別書込失敗は画面上の選択値・描画を変更しない。
4. resolver は最終 presentation が実 1 page、complete canonical book、`UnpairedSlot` のときだけ `first` または `last` を選ぶ。横長・非 pairable、末尾表紙補助で二枚になった unit、collection/検索/切詰めは従来どおり Center。片端変更は page count、anchor、seek、source demand を変えず、その context の placement layout だけ invalidate する（`src/ui_fullscreen.rs:13496`, `src/ui_fullscreen.rs:13684`, `src/ui_fullscreen.rs:13717`, `src/ui_fullscreen.rs:15986`）。
5. paged、縦/横連結、holdover の capture-time placement、F12 の active と passive frozen projection を同じ composition 値から通す。parked sibling のうち変更端を継承する context のみ再計算し、旧 snapshot を現在設定から描き直さない（`src/ui_fullscreen.rs:7725`, `src/ui_fullscreen.rs:9818`, `src/ui_fullscreen.rs:17166`, `src/app/viewer_context_registry.rs:3223`）。detached viewport/述語に入る実装は `docs/detached-rework-plan.md` §2 の構造合意を先に得て §11 に記録する。
6. 本別 menu は「先頭」「末尾」それぞれ `全体設定に従う / 片側へ配置 / 中央表示`。環境設定は先頭/末尾二項目。metadata export/import、refresh、rename/purge、content identity 復元、Remote live snapshot/write、IPC/Web menu/refit を両端同時に更新する。metadata transfer の出力版を v8 に上げ、**入力は released v7 と v8 の manifest・shard header の両方を明示受理**する。同じ bundle 内の manifest/header 版一致を要求し、未知版は拒否。v7 の `singleton_spread_placement` は root と nested の明示 `FollowGlobal(0)` 行も含め、各行を `(old, old)` として v8 endpoint state へ移す。row absent は absent のまま。現 importer は両 header を `FORMAT_VERSION=7` に厳密比較するため、定数更新だけでは旧 bundle を失う（`src/metadata_transfer.rs:27`, `src/metadata_transfer.rs:456`, `src/metadata_transfer.rs:3041`, `src/metadata_transfer.rs:3062`, `src/metadata_transfer.rs:4491`）。IPC protocol と Web asset token は既存 strict 手順で更新する（`src/remote_ipc/ui.rs:2267`, `crates/remote-ipc/src/lib.rs:31`, `docs/web-remote-plan.md:2837`）。利用者文書 `docs/spec.md`、`htdocs/mimageviewer/manual/settings.html:1141`、`htdocs/mimageviewer/manual/fullscreen.html:441` と Remote 読書設定説明を実装時に更新する。

## 3. §1.240 — 表紙あり見開きの端近くの実ページを単独表示（2026-09-24 利用者決定）

### 適用境界と期待 unit

- 「表紙の次のページを単独で表示」と「最終ページを単独で表示」は独立した切替で、LtrCover / RtlCover（表紙あり見開き）でだけ表示構成へ適用する。表紙なし見開き、Single、横長 Split では値を保存・変更できるが構成、白い面、ページ送りへ適用しない。表紙なしで白紙挿入 parity を当てると 2 ページ目が単独にならないため、>>402 の parity oracle は表紙あり見開きに限定する。
- 実ページ以外の navigation unit、GridItem、path、source request は作らない。表紙を除く実列を強制境界で区切って組み直し、各 unit は実 anchor を持つ。1 ページ本では両切替とも効果なし。2 ページ本で同じ 2 ページ目を両切替が指すときは「表紙の次」を優先し、単独 unit を一つだけ作る。単独の形成理由は通常の UnpairedSlot / LandscapeBoundary と区別し、強制対象と強制境界で余った実ページを記録する。単独 unit の実ページは必ず 1 回だけ現れる。
- 以下は完全な portrait・pairable 本で、**末尾表紙補助 OFF** の期待値。列は「表紙の次 / 最終」の OFF=0、ON=1。C1 は通常の表紙 unit、E は通常の端数 singleton（どちらも §1.239 が配置を決め、白は描かない）。nL□ は実ページ n が画面左、同じ可視寸法の白い相方が右、nR□ はその鏡映。括弧は同じ unit 内の実 2 ページ。区切りの「;」はページ送りの 1 step であり、□ 自体は step ではない。

| LTR 表紙あり・実ページ数 | 00 | 10 | 01 | 11 |
| --- | --- | --- | --- | --- |
| 1 | C1 | C1 | C1 | C1 |
| 2 | C1; E2 | C1; 2R□ | C1; 2R□ | C1; 2R□ |
| 3 | C1; (2L,3R) | C1; 2R□; 3L□ | C1; 2L□; 3L□ | C1; 2R□; 3R□ |
| 4 | C1; (2L,3R); E4 | C1; 2R□; (3L,4R) | C1; (2L,3R); 4R□ | C1; 2R□; 3L□; 4L□ |
| 5 | C1; (2L,3R); (4L,5R) | C1; 2R□; (3L,4R); 5L□ | C1; (2L,3R); 4L□; 5L□ | C1; 2R□; (3L,4R); 5R□ |

| RTL 表紙あり・実ページ数 | 00 | 10 | 01 | 11 |
| --- | --- | --- | --- | --- |
| 1 | C1 | C1 | C1 | C1 |
| 2 | C1; E2 | C1; 2L□ | C1; 2L□ | C1; 2L□ |
| 3 | C1; (2R,3L) | C1; 2L□; 3R□ | C1; 2R□; 3R□ | C1; 2L□; 3L□ |
| 4 | C1; (2R,3L); E4 | C1; 2L□; (3R,4L) | C1; (2R,3L); 4L□ | C1; 2L□; 3R□; 4R□ |
| 5 | C1; (2R,3L); (4R,5L) | C1; 2L□; (3R,4L); 5R□ | C1; (2R,3L); 4R□; 5R□ | C1; 2L□; (3R,4L); 5L□ |

- 表紙なし見開きでは 00/10/01/11 がすべて同じ従来列になる。1=E1、2=(1,2)、3=(1,2);E3、4=(1,2);(3,4)、5=(1,2);(3,4);E5。LTR の pair は左→右、RTL は右→左。端数 E の配置は §1.239 の first/last 設定を使う。Single と Split も従来列を保持する。
- **末尾表紙補助 ON なら、末尾の単独ページの空いた側には白ではなく表紙を添える**（2026-09-25 利用者報告により改訂）。当初は「白い相方を持つ強制単独が優先し、表紙を添えない」と設計したが、利用者が両切替 ON・末尾補助 ON で「表紙が添えられず白が添えられる」ことを不具合として報告した（画面は利用者が観測）。改訂後の規則: 最終 unit が実 1 ページで空き側を持つとき、それが強制単独・強制境界の余り・通常の端数 E のいずれでも、通常の最終単独と同じ末尾補助の適格判定（単一の判定を共有する。現行コードでは通常の横長の最終単独にも表紙を添える）を満たせば表紙を添える。適格でなければ強制単独は白、通常 E は §1.239 の配置。表紙の次の強制単独（最終 unit でないもの）は常に白。unit の数・実 anchor・ページ送り・export/capture の出力は末尾補助の従来規則のまま（添えた表紙は移動先にも出力にもならない）。以下は LTR の補助 ON 時の最終 presentation（RTL は左右反転、□ は白、+表紙1 は添えた表紙）。

  | 実ページ数 | 切替 | 最終 presentation |
  | --- | --- | --- |
  | 2 | 10 / 01 / 11 | `2R` + 表紙1（白なし） |
  | 3 | 10 | `2R□; 3L` + 表紙1 |
  | 4 | 01 | `4R` + 表紙1 |
  | 4 | 11 | `2R□; 3L□; 4L` + 表紙1 |
  | 5 | 10 | `5L` + 表紙1 |
  | 2 | 00 | `E2 + 表紙1`（通常補助） |
  | 4 | 00 | `E4 + 表紙1`（通常補助） |

- >>402 の表紙 01・本文 03..34・裏表紙 36、両切替 ON は 01;03;04+05…32+33;34;36。**§1.239 の先頭配置が ON の場合**、画面側は LTR で 01=右、03=右、34=左、36=左、RTL で鏡映する。先頭配置 OFF なら通常の表紙 01 は中央になる。これは「01 の後」と「36 の前」に各 1 枚を挿入した場合の実ページ slot と一致する。1–2 ページの重複境界は上表の畳み規則を優先する。

### 描画、操作、他機能との関係

- 白い相方は実ページの保存回転・表示トリム後の可視寸法を参照する layout-only decoration。§1.218 の二 slot 仮想 canvas、fit、gap を再利用するが、paint owner は実ページ texture と別の、同じ viewport に clip された固定 WHITE の矩形とする。texture-present 分岐の外に描画責務を持ち、実ページが ready で描かれた frame だけ表示する。実 texture が未到着・失敗なら loading/error を示し、白面を偽の成功ページに見せない。draw 経路が白矩形の PaintRecord を発行する。
- paged と縦横連結は同じ decoration geometry を使う。Z zoom では実ページ transform と同じ倍率で gap も拡大する。holdover は捕捉時の実 texture・白 rect・clip・配置入力を一体で保持し、後の設定変更から再計算しない。viewport resize 時は捕捉した入力から縦横比を保って再投影する。F12 active と passive frozen snapshot にも白 decoration を保存し、実描画で一致を検証する。Remote は実 address 1 件と address の無い white decoration を送る。Web は実ページの ready presentation に従属する白い DOM 矩形として描き、白用 fetch・decode・cache・edit target を作らない。通常の §1.218 endpoint singleton の空き側は背景色のまま。
- 強制 singleton と境界で余った singleton は §1.239 の Center/Place より優先して上表の側に置く。強制されない真の端数 E だけに §1.239 を適用し、1 unit が両端なら先頭優先。保存回転後を含む横長ページが強制対象なら例外として片側へ置き、通常の横長は従来どおり中央にする。この横長例外は、通常の横長を中央へ置く実装と literal blank-insertion oracle が一致しないことを明示した仕様である。末尾補助は強制単独でも抑止しない。最終 unit が実 1 ページで補助の適格条件を満たせば、空き側を白ではなく表紙で埋める（上記改訂）。
- 完全な本かどうかの context gate と canonical permutation proof は pairing より前に判定する。cache token に item 世代・実読書列・mode・横長 epoch・spread-shift anchor・両切替の実効値・eligibility を含めるか、同じ条件の gated construction を持つ。検索、collection、切詰め、synthetic の列では builder に強制境界を渡さず、従来 unit 列を保つ。shift anchor は強制境界を越える pair を作らず、境界内の segment に正規化してから組み直す。旧 anchor の index だけを新 grouping へ流用しない。
- 実ページ数・実 index・resume・bookmark・履歴・go-to-page・rating/tag/edit 対象は不変。前後操作、Home/End、slideshow、連結 scroll は実 anchor を持つ再構成後の unit を進む。paged seek は切替の ON/OFF にかかわらず従来の unit fraction（unit 先頭=0、末尾=1、1 unit なら 0）を track と tick に使う。ON 時は実 unit 数が変わる分だけ目盛り間隔が変わる。click は最寄りの unit に着地し、その実 anchor を選ぶ。strip の各 label/thumbnail と page-number cache、prepared seek label は同じ real-page→unit 対応表から実ページを引き、白 slot に番号を付けない。paired unit 内の go-to-page 等が選んだ実ページ identity は保持し、表示番号の分母は実ページ総数のまま。
- **利用者決定:** Ctrl+E export と capture は、強制 singleton なら実ページだけを出力し、白い相方を含めない。白は画面装飾であり保存画像の構成要素ではない。単独実ページの source、crop、補正、回転は既存 single-page output を使う。ペアの出力は従来の実ページ構成を維持する。

### 設定、互換性、検証とリリース

- 全体は after-cover / final の二 bool（既定 OFF）。本別は各々 FollowGlobal / On / Off。spread.db の独立 table は ZIP ごとの exact book key を使い、nested exact → root fallback、nested 明示 FollowGlobal の fallback 遮断を §1.239 と同じ意味にする。旧 read-only spread.db に新 table が無ければ両方 FollowGlobal として読み、書き込まない。table があるのに schema/query が壊れていれば Result error を伝え、FollowGlobal に見せかけない。settings.db live Remote snapshot、context bundle、rename/purge、content identity 復元も接続する。自動 sidecar は増やさない。
- 二つの KeyAction（FsTogglePageAfterCoverAlone / FsToggleLastPageAlone、仮名）は FsImage、既定 chord なし。FollowGlobal で押したときはその端の実効 global 値の反対を明示 On/Off として書き、明示 On/Off では反転する。menu と同じ本別書込・再構成経路を使う。表紙なし/Single/Split 中も値は変更できるが、表紙あり見開きへ戻るまで見た目に適用しない。KeyAction registry、dispatch、help、keymap.ini.default と keymap 文書を更新する。
- 明示 metadata transfer は新出力版（v9 を予定）で二つの本別値を持ち、released v7/v8 と新 v9 の manifest・shard header を入力受理する。機能導入前の bundle に欄が無ければ「行なし」を保ち、旧本の全体値は既定 OFF、存在しない明示 override を作らない。新 bundle の root/nested 明示 FollowGlobal 行は保持する。混在版は拒否する。
- pure builder は両表・表紙なし表の 1–5 ページ、>>402、odd/even、片方/両方 OFF/ON、LTR/RTL、横長/保存回転、shift、§1.239、末尾補助を検査する。各実 index が一度だけ現れ、空 unit と偽 idx が無いこと、白 slot の側と可視寸法を検査する。統合では partial-book gate、seek fraction/tick/click/strip/番号、Ctrl+E/capture の白除外、loading/failed texture、縦横連結、Remote Web DOM、F12 active/parked、holdover を確認する。第1層 I1/I2 は composition DTO だけでなく実 texture paint と draw 発行の白 rect 証拠を比較する。
- 規模は Medium–Large（単独実装で概ね 1–2 週間、§1.239 の受入れ後）。**利用者決定により §1.240 は v4.1.0 に含める。**実装・自動検証・独立レビューを終えてから出荷する。任意位置への本当の白紙挿入と context-owned typed cursor 基盤、§1.242 の本構成編集は後続に延期する。
