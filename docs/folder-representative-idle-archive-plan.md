# フォルダ代表の ZIP/PDF idle 作成計画（§1.398）

作成: 2026-10-18。状態: **設計案・独立レビュー前・未実装**。
調査基点: `c84bea5dd`（§1.399 / §1.397 とマニュアル補足の反映後）。
要望の正本は `05a724de9:docs/next-release-backlog.md` §1.398。

この段階では製品コード・設定・DB を変更しない。以下の数値は測定済みの値ではなく、
実装・性能検証に使う初期上限案である。独立レビューと設計担当の受入後に実装する。
バックログの項目は実装・検証が終わるまで残す。

関連正本:
[代表選定](folder-representative-plan.md)、
[要求寿命](thumbnail-request-lifecycle-plan.md)、
[非同期処理](async-architecture.md)、
[UI 応答性 §4](ui-responsiveness.md#4-新機能追加時のチェックリスト)、
[仮想フォルダ](virtual-folders.md)、[catalog](catalog-design.md)、
[PDF 所有](pdf-render-epoch-ownership-plan.md)。
本書は通常選定を置き換えず、「通常選定に渡せるキャッシュを idle に補充する」部分だけを追加する。

## 1. 現状と変更する境界

行番号は調査基点での位置。実装時は関数名でも追跡する。

| 対象 | 現在の実装・根拠 |
| --- | --- |
| 要求フォルダ自身の固定代表 | `App::make_load_request` / `apply_folder_thumb_pin` が自動選定より先に解決する。表示契約は `display-pipeline.md` §1.3.2。固定代表に失敗しても、idle が勝手に別画像へ固定し直してはいけない |
| 各階層の自動選定 | `src/thumb_loader.rs:4571` `resolve_folder_thumb_image_inner`。まず直下画像を `folder_thumb_sort.sanitized_for_folder_thumb()` 順に選ぶ。なければ、子実フォルダと ZIP/CBZ・paged document を一つのソート列にして順に調べる。子の固定代表を確認してから再帰する。`remaining_depth == 0` なら直下画像だけ |
| 書庫の利用 | 同関数 `:4790` 付近の子 pin / 再帰と `:4830` 付近の catalog lookup、`:4374` `load_cached_pinned_folder_thumb`。画像以外の子 pin と ZIP/PDF は、source stamp に合う **完全一致の既存 WebP 行だけ**を利用する。active pin があれば対応する `#pin:` 行が必要で、無印の先頭画像行では代用しない |
| 通常選定で開かない理由 | フォルダを一つ描くための再帰探索が、配下の全書庫 open・展開・PDF render に増幅しないようにするため。NAS の open / central directory 読み取りもここでは行わない。通常の ZIP タイル生成とは別の契約 |
| catalog / proof | `src/catalog.rs:37` `db_path_for`、`:81–125` proof 型、`:972` `open_for_folder_selection`、`:1069` `load_stamp_snapshot`、`:2032` `init_schema`、`:2131` revision trigger。選定で参照した catalog の instance/revision（不存在も記録）、参照した pin store の revision、勝者の source stamp / exact row key を保存する |
| proof の限界 | `src/thumb_loader.rs:4459` `select_folder_thumb_image_with_policy` は **勝者があるときだけ** proof を作る。`:4259` `folder_cached_row_usable_in_cache_dir_with_pins` は catalog・pin revision と勝者の metadata を照合する。全訪問ディレクトリの filesystem inventory を証明するものではない。深い階層の追加・削除を root の mtime で検知できるわけではない |
| 代表なし | `process_load_request` `src/thumb_loader.rs:2896–2945` は解決なしを画像なしの終端へ返す。`App::poll_thumbnails` `src/app.rs:53426` 付近が `ThumbnailState::Failed` にする。通常 enqueue（`:54415` 付近）は Pending/Evicted だけを拾うので、その一覧では止まる。永続的な「画像なし」の証明ではなく、再訪では再選定する。取消は別終端で、Loaded を保持し、その他を Evicted にする |
| 既存 idle upgrade | `src/app.rs:54949` `enqueue_idle_upgrades`。設定 ON、非 drive 一覧、sidecar restore 中でないこと、scroll と一般入力それぞれ 500ms idle、`requested.blocks_idle_upgrade()` が false、reload/heavy queue が空、が条件。keep 内の **Loaded** の画質・解像度不足を調べる。代表なしの作成ではない。`:55202` は時刻条件の再確認用 wake |
| 通常先読み | `src/app/prefetch_policy.rs:52` `decide_prefetch_allowed`。scroll 100ms 未満は止め、visible 未完も止めるが、scroll 無し 3 秒で backstop が通る。`src/app.rs:54357` 以降が通常 / archive 判定の blocker を分けて使う。PDF grace、keep、source admission など他の条件も残る。idle 作成に 3 秒 backstop を流用すると「通常要求が残っていても開始」になってしまう |
| thumbnail queue | `src/app.rs:52740` `spawn_thumbnail_workers`。通常 reload と重 I/O の queue、Condvar 待機、`:52832` priority pop。重 I/O worker は全体 thread 数により 1 または 2 本。`src/thumb_loader.rs:23–65` の priority は visible → prefetch → index を持たない catalog 作業 |
| PDF pool | `src/pdf_loader.rs:1142` `JobPriority`、`:3188` `JobQueue`、`:4569` `try_pop_dispatch_job`、`:4781` `run_dispatcher`。Critical → HighNormal → Normal、非 Critical lane 予約と文書 open admission がある。`:5855` `render_page` は cancel / validity / wait policy を受ける。AbortOnCancel で要求側が戻っても、送信済み IPC は終了まで走る。現状は open 成功後に本要求を送るので、その間の idle 取消 checkpoint も必要 |
| 書庫 read | `src/zip_loader.rs:241` `PositionedFileReader` / `:420` `CancellableReader`、`:541` directory cache、`:2462` cancellable first-image read。read/seek・entry・64KiB read loop に取消点がある。一方、通常 `read_first_image_bytes` / `first_visible_image_entry` の Option は「なし」と読取エラーを分離しない。再帰 reader も読めない entry を飛ばす。これを否定結果の根拠にはできない |

`ActivityGate`（`src/activity_gate.rs:53`）は indexer 用で、quiet は既定 1000ms、
未入力起動時は即 idle、待機 API は sleep を使う。これをそのまま新 worker の待機ループにしない。
また `GlobalIoSemaphore` の説明だけから「全 thumbnail / PDF read が既に semaphore に従う」とは
仮定しない。基点の thumbnail worker で取得しているのは音声関連で、`pdf_loader.rs` 自体には
取得呼出がない。今回の優先保証は下記の queue admission と dispatcher に実装し、Low permit
だけで通常要求との競合を解決したとは扱わない。

## 2. 対象範囲と作業量

### 2.1 対象

初期実装は、**表示中の通常実フォルダ一覧の grid** にある `GridItem::Folder` だけ。
各 context の表示順で、visible → 上下 1 行（既存 keep set 内）の順、同距離は前進側優先とし、
その範囲から最大 8 フォルダを保持する。raw index の大小で表示順を代用しない。
通常要求が終端し、画像がないフォルダだけを候補にする。ただし Failed だけでは理由を判断せず、
worker の通常選定をもう一度通して対象を確定する。

除外は drive 一覧、ZIP 内仮想 folder、PDF 内、検索・Smart・Collection・★等の集約 root、
詳細一覧、非表示 / parked context、fullscreen、スライドショー / 再生による継続的な表示需要中。
これらの通常 thumbnail / pin / 先読みを変更しない。main と detached の「役割名」で除外せず、
通常の実フォルダ grid を現に表示している owner なら同じ資格で扱う。
全ライブラリ、履歴、favorite 登録先、非表示の兄弟 folder へ対象を広げない。

### 2.2 探索と候補順

- 深さは既存 `folder_thumb_depth` をそのまま上限にする。0 なら ZIP/PDF 作成は行わない。
  子実フォルダの 1 段と archive 候補の扱いは既存 resolver と同じ。新しい独自 depth 設定は作らない。
- **通常 resolver と共有する候補列挙 / 順序 helper** を使う。各階層の直下画像優先、子 folder と
  archive の混合ソート、child pin cascade、file type visibility を二重実装しない。
  通常 resolver が画像・有効 WebP を選べるなら新規作成せず、その選定を使う。
- 普通の自動選定では cache miss を飛ばす仕様を維持する。idle の対象 root に代表がないときだけ、
  その順で到達する未キャッシュ ZIP/CBZ・**実 PDF** を順次補充する。1 候補成功ごとに通常選定を
  再実行し、勝者ができた root は終了する。後続を先に生成して順序を変更しない。
- 先頭画像は既存 ZIP タイルと同じ entry 順、PDF は物理 page 0。ZIP 内の directory と nested ZIP
  も既存 first-image の順序に含めるが、下記上限に達したら後ろを勝手に選ばず保留する。
  フォルダ候補の `folder_thumb_sort` を ZIP 内ページ順へ新たに適用しない。
- 要求 root 自身に手動固定がある場合は idle の自動補充をしない。子の固定は解決順・exact key を
  尊重し、作成済み行だけ読む。未作成の固定ページ / 動画 / 別 folder pin を背景で生成する機能は
  今回追加しない。必要な固定行がないことは「画像なし」ではない。
- RAR/7z/LZH、EPUB の変換、既存変換 ZIP の新規生成は対象外。通常選定で既存の有効キャッシュを
  利用できる契約は維持する。設定で除外した形式を開かない。

### 2.3 初期上限案

| 単位 | 上限案 | 到達時 |
| --- | --- | --- |
| 現在の表示範囲 | 8 root | 範囲外は探索しない。scroll で対象窓が変わればその範囲から選ぶ |
| root の候補計画 1 件 | 訪問実ディレクトリ 32、列挙 entry 合計 2048、archive 候補と参照 pin source 合計 32 | 不完全な計画として当該一覧世代では保留。完全な否定 proof は作らない |
| 全 context 合計 | queued / 実行中の idle 作業 **1 件** | 空きは完了通知で知らせ、worker を増やさない |
| archive 開始量 | 一つの一覧世代で最大 8 コンテナ（context ごと） | 同じ範囲の idle 再到達で予算を補充しない。更新・再訪で次へ進める |
| ZIP | source 256MiB 以下、read 累計 32MiB、検査 entry 10000、nested 最大 2 段、nested 展開合計 32MiB | `OutsideIdleBudget`。未探索部分を「画像なし」にしない |
| ZIP 画像 | payload 32MiB、デコード前寸法 16M pixel 以下、通常の decoder 制限も適用 | `OutsideIdleBudget`。通常 open では既存処理を維持 |
| PDF | source 256MiB 以下、1 文書・page 0 だけ、既存 thumbnail サイズ | native open / render は次節の非中断区間として扱う |

計画の候補行は完全なディレクトリ列挙を終えてから sort する。2048 を超えた列挙 prefix を
「先頭 2048 件」として選定しない。ディレクトリの reparse / symlink は共有分類・再帰規則を使い、
cycle / 同一実体の再訪を避ける。安全に同定できない先は保留し、対象 root の外へ無制限に広げない。
既存の明示 pin を読む経路だけは、その既存 cascade 深さと source 検証を維持する。

1 root ずつ計画し、1 archive 単位で dispatcher へ戻る。root 間は round-robin、root 内は候補順。
途中成功 / 否定済み候補は残すので、再訪時は既に終えた archive を開き直さず、残りへ進める。
表示窓の変更だけで同じ一覧の archive 開始予算 8 をリセットしない。入力で取消された開始も 1 件に数える。

### 2.4 負荷の見積りと限界

キャッシュのない最大 8 root で、metadata 調査は合計最大 256 directory / 16384 entry。
1 root の完全な否定検証は最大 32 directory と 32 archive / pin source の stat、catalog revision 読取。
NAS の 1 往復が仮に 10ms なら 64 stat で約 0.64 秒、50ms なら約 3.2 秒に DB 待ちが加わる。
これは worker 上の見積りであり、UI 停止時間の許容値ではない。
ZIP payload read は一覧世代あたり最大 8 × 32MiB = 256MiB、同時に 1 候補だけ。
PDFium 内部の I/O 量は同じ read 予算では制御できない。
保持する候補計画は最大 8 × 2048 entry で、path 長に比例する数 MiB〜十数 MiB 程度を見込む。
decode は一度に 1 枚、RGBA 16M pixel なら画素だけで約 64MiB。parser / codec / PDFium の内部メモリは別で測る。

先頭ページだけでも PDF の xref / 文書 open、ZIP の central directory parse は重い。
ZIP の read 予算は directory parse に使う reader から適用し、parse 後の entry 数チェックだけで
上限を保証したとはしない。OS に発行済みの NAS read / open は途中停止できない。
PDF の file size 上限も open 時間や native 内部 allocation の上限を保証しない。
並列度・開始件数・対象範囲は制限するが、「入力後 N ms 以内に disk が完全 idle」とは約束しない。
この性質のため ON/OFF を独立に用意する案を §10 に置く。

## 3. idle の定義、queue、取消

### 3.1 共通 admission

既存 quality upgrade を先に処理し、その後に次の全条件が揃ったときだけ投入する。

1. 機能設定 ON、§2 の grid と範囲が現在も存在し、cache 保存が有効。
2. 全 window の最後の実入力（key / pointer / wheel / touch / gamepad / navigation 指示）から
   1000ms、対象 grid の scroll 変化から 1000ms 経過。起動・一覧 install も計時起点にする。
   pointer hover だけが既存入力扱いから漏れていれば同じ境界で拾う。単なる repaint / receipt は入力ではない。
3. 全 context の通常 / upgrade request が空（保存終端・CPU 画像採用/破棄まで含む）。
   通常先読み、foreground / neighbor PDF、背景 metadata catch-up、変換判定・一覧 prepare、
   modal 操作等の高い需要も残っていない。動画/音声等の継続需要中も新規投入しない。
4. reload / heavy の通常・先読み work、PDF の Critical / HighNormal / Normal queued / 実行中が空。
   idle 自身はこの blocker に数えない。indexer は Low の競合相手として扱う。
5. 全体 1 件の idle 枠と当該一覧の予算があり、対象 idx に他種要求がない。

3 秒 backstop による通常先読み再開は通常需要として優先し、idle の例外にはしない。
`thumb_idle_upgrade` の ON/OFF は本機能の代わりに使わない。

共通の `IdleArchiveAdmission`（仮称）は **仕事を実行する新しい thread / queue ではない**。
既存 owner と dispatcher の需要遷移を受ける小さな process 内仲裁器とする。
通常 `ThumbnailRequests::begin` / 正式終端、PDF queue / dispatcher の enqueue / finish、
一覧準備等の既存開始 / 終了から RAII の需要 lease を更新する。非表示 context の保存待ちも落とさない。
入力と需要増加の境界は admission を閉じ、現在 idle lease の cancel を立てる。
需要減少・実処理終了は通知を出す。UI から queue 全体を毎 frame 走査・lock して監視しない。
PDF pool が未初期化なら、監視のために起動しない。

投入と通常需要増加は仲裁器の同じ短い critical section で順序を付ける。
先に idle が通っても、直後の通常需要が取消を publish し、worker pop の再検査で止まる。
UI は atomic / 小さなメモリ更新と wake だけで、I/O 完了や lease 返却を待たない。
既存通常需要に idle 枠取得を要求しない。

### 3.2 既存実行基盤への接続

`heavy_io_queue` の work source に `FolderIdleArchive` の typed payload を追加する案とする。
payload は `PreparePlan` / `BuildCandidate` のどちらかで、相互排他的な bool 群にしない。
`request_worker_priority_key` で visible → prefetch → 既存 catalog 作業 → idle の順にし、
pop 時に admission / scope / cancel を再検査する。1 回に root 全体の全 archive を処理しない。
独立した idle thread、try_lock + sleep の retry、周期 polling は設けない。

PDF は既存 `JobQueue` / `run_dispatcher` に最下位 `Idle` lane を追加する。
Critical → HighNormal → Normal → Idle の順、Idle の queued + 実処理は全体 1 件、
既存の非 Critical / Normal cap と open admission の対象にも数える。
通常 lane が復帰すれば queued Idle を取消し、通常 lane の予約を侵食しない。
`PdfRenderValidity` は要求元 context の ticket を継承し、Unscoped に逃がさない。
cancel policy は背景用の既存 `AbortOnCancel`。HarvestOnCancel の適用範囲を広げない。
既存 visible / fullscreen promotion は Idle を昇格させない。通常の表示需要が発生した場合は
Idle の取消と通常要求の受付に戻し、同じ key だけを根拠に仕事の種類を変えない。

filesystem / catalog / ZIP 作業は既存 `GlobalIoSemaphore::acquire_cancellable(Low, cancel)` を使う。
PDF dispatch を待つ間はこの filesystem permit を保持しない（入れ子取得や将来の deadlock を避ける）。
native PDF の優先度は PDF lane が所有し、Low permit だけに依存しない。

ZIP の budget / cancel は要求固有とし、共有 directory template に保持させない。
idle の cache miss は既存 reader / parser を使う **要求内の private handle** で構築し、
通常要求が待つ共有 `ArchiveTemplateSlot::Loading` を作らない。既に Ready の template を読む場合も
要求 clone にだけ制限を付ける。不完全 / budget 超過を共有 Failed slot として公開しない。
nested payload も要求内上限を満たすものだけ扱う。この簡素化で、通常要求が idle 固有の budget error を
受け取ったり、取消・上限を抜けるための新しい retry loop を持つ組合せをなくす。

### 3.3 止め方と実処理の終端

| 境界 | 入力 / 通常需要 / scope 失効後の扱い |
| --- | --- |
| 未投入 / queued | 新規投入を停止。取得前取消は既存 queue lock 下で取り除き、元 request ID の取消終端を一度だけ返す |
| directory / proof 検証 | directory 開始前、entry ごと、catalog query 前後、次候補前で中止。途中 inventory を確定否定にしない |
| ZIP | open 前、reader の read / seek、entry、nested 展開 chunk、decode 前後、save 前。既存 cancellable reader を共有し、idle だけに必要な budget / 完了性の返値を加える |
| PDF queued | dispatcher の pop / epoch maintenance で除去。Idle も stale prune 対象に加える |
| PDF open 中 | 発行済み IPC は完了を待つ。open 応答後、render IPC を送る前に cancel / ticket を再検査し、取消後の page render を始めない |
| PDF render 中 | native IPC は中断できない。要求側は AbortOnCancel で戻れるが、dispatcher は応答を受け終える。worker process を kill / restart しない |
| cache commit | save 前の fresh stamp / key / pin / cache admission 検証が成立したものだけ保存。開始済み SQLite transaction は commit/rollback で終える。UI への表示採用資格とは分ける |

**全体 1 件の枠は要求側の early return だけでは解放しない。** Idle work の lease は、
thumbnail worker と、送信済み PDF `Job` が必要な間だけ共有する。最後の実処理・save が終わるまで
生存し、dispatcher の job 終了 / queued 破棄も同じ lease の drop へ合流する。
その完了通知で次の admission を評価する。取消のあとに新しい idle job を重ねない。
context の request 終端と process の実処理枠は用途が異なり、同じ pending bool を二重に持たない。

idle 到達時刻だけが未充足なら既存 frame-tail の deadline wake に統合する。
早い別 repaint が入ったときも残り deadline を再予約する。候補完了・需要終端・範囲変更は event wake。
候補なし / 予算終了 / 否定再利用後は wake を止め、常時 repaint を残さない。

## 4. 成功結果と context の所有

### 4.1 source cache を作り、通常選定へ戻す

1. worker が root の現在の通常選定を実行する。既存勝者があれば新規生成なしで通常経路へ戻す。
2. 未キャッシュ候補を一つ読み、通常タイルと同じ policy / source stamp / exact key で WebP を保存する。
   ZIP/PDF のタイル行は **そのファイルの実親 directory の catalog** の `zipthumb:` / `pdfthumb:`。
   root folder の catalog と取り違えない。drive/share root の key 例外も既存 helper を使う。
   page cache だけ作って親タイル行を欠く実装にはしない。
3. 既存 trigger が同 transaction 内で catalog revision を進める。idle 完了は source row の
   cache-map 更新に必要な確定データと元 owner / items generation / idx / request ID を返す。
4. 元 request を正式終端させてから、**同じ root idx だけ**を既存通常要求へ再投入する。
   既存 `make_load_request` と resolver が direct image / pin / 新キャッシュを再評価し、
   AutoSelected WebP と既存 `FolderSelectionProof` を保存する。勝者画像を idle が UI に直接貼らない。
5. 通常画像の採用・保存終端・GPU upload は既存 `ThumbnailRequests` の契約と 8 textures/frame を使う。

idle 作業も同 idx の単一予約を使えるよう `ThumbnailRequestKind::IdleArchive` を追加する案とする。
Idle の terminal receipt は画像なしの **作業結果**（計画 / cache saved / 確定なし / 保留 / 取消）で、
従来の Failed と混ぜない。通常要求が戻れば Idle を取消し、その idx の予約終端後に通常要求を始める。
全 idx の通常処理まで idle の終端待ちにしない。
idle は通常進捗や `blocks_idle_upgrade` の blocker には含めず、残っている実作業は全体 lease で扱う。
この変更は lifecycle の projection を列挙してテストする（単に件数から差し引かない）。

### 4.2 表示更新と多窓

per-context の `IdleArchiveSession`（仮称）は候補計画・cursor・当該一覧の予算・終端結果を一つに持つ。
in-flight の request ID / 画像寿命は既存 `ThumbnailRequests` が正本であり、別の requested set は作らない。
`ViewerContextBundle`（`src/app/viewer_context_registry.rs:1458` 以降）の items、queue、cancel、
PDF epoch と一緒に移譲する。fork は有効な確定値だけを受け取り、新しい owner と要求空間を作る。

採用条件は owner、items generation、root identity、idx、request ID、候補計画の設定 fingerprint が一致。
旧通知で新しい Failed/Loaded、予約、cursor を上書きしない。
scroll で候補範囲外へ出た場合はその idle 要求を取消す。keep 内でも対象窓外なら次 archive を始めない。
hide / park / close / list 交換 / retire / drop は **その context の idle** を取消し、通常要求の既存寿命は維持する。
mount / unmount という読取り操作を global cache mutation として通知しない。

共有 catalog の確定 write は他 context の将来の通常 lookup からも見えるが、idle 完了で兄弟 context を
reload / Failed解除 / epoch bump / texture clear しない。同じ path を表示する兄弟も、その通常 reload / 再訪時に拾う。
表示中の変更は元 context の対象 idx に限定し、一覧全体の再読込、scroll・選択・facet の再構築をしない。
既存の明示 cache maintenance が対象 owner に届ける失効は別のまま維持する。

detached 述語・viewport・HWND・park 発火規則は変更しない。
bundle の移譲 / fork / drop の追従は、実装前レビューで構造的修正の合意を取り、
`detached-rework-plan.md` §11 に理由と範囲を記録する。

### 4.3 背景で開かない操作

ZIP の暗号化 entry / directory、password PDF は `NeedsForeground` とし、画像なしとは区別する。
保存パスワードを取得・試行せず、password=None の非対話 read だけにする。dialog / 変換 / password 保存を
呼ぶ App open 経路には接続しない。完全な暗号化判定が得られない read error は読取保留のままにする。
child pin の未作成行、EPUB、変換が必要な形式も同様に idle では生成しない。
通常操作で開く / 固定を変える / cache が生成されると、次の再評価が有効になる。

## 5. 見つからなかった結果の記憶

### 5.1 記録する命題を分ける

| 結果 | 意味・記憶 |
| --- | --- |
| `CacheSaved` | 有効な既存タイル行を保存した。通常 resolver に戻す |
| `NoImage` | 対象 ZIP の安全性 / policy 判定を含む全候補をエラーなしに調べ、画像がなかった。PDF は正常な文書応答で page count 0 が確認できた場合だけ。source identity に対する確定否定として永続化可 |
| `NeedsForeground` | 暗号化 / password、対象外形式・未作成の固定行等。idle で処理できない証拠で、画像がない証拠ではない。理由と source/pin/policy に結び付けて再利用可 |
| `OutsideIdleBudget` | 上限を超えた。未探索部分あり。当該一覧では保留し、否定 proof にしない |
| 取消 / 不完全な列挙 / 読取失敗 | 確定結果なし。永続 NoImage にしない。失敗は同一覧世代内で再投入を止め、再訪・明示更新・関連変異で再評価。自動 retry timer は作らない。取消は次の idle episode で予算内なら再開可 |

ZIP の `Ok(None)` を得ただけで NoImage としない。idle 用 read は、既存順序 helper と cancellable
reader を使い、完了性を型で返す。entry open / read / decode 失敗を黙って飛ばして全体の否定を作らない。
先頭候補の decode 失敗で後ろの画像へ変える仕様も追加しない。
PDF render の汎用 error を空文書へ変換しない。page 0 のない正常な文書を扱うには、既存 worker 応答に
正常な page count / empty を返せる最小の typed outcome を設け、Critical な全ページ列挙を先行させない。

### 5.2 保存先・形式・リリース判定

新しい global DB は作らず、**既存 per-directory catalog DB** を使う。
`CatalogDb::db_path_for(cache_dir, directory)` の `{cache_dir}/{xx}/{hash}.db` に、worker が書く時だけ
次の追加表を作る（名称は実装時に統一する）。

| 表案 | 所有 directory と key / 値 |
| --- | --- |
| `folder_idle_archive_results` | archive の実親 catalog。key は版、drive を保持した source identity、対象 tile exact key、visibility / 読み方 fingerprint。値は fresh source stamp、`NoImage` または `NeedsForeground` と根拠。成功は既存 thumbnails 行を正本にする |
| `folder_idle_exhaustions` | 調査 root 自身の catalog。key は版、drive を保持した root identity、sort、configured depth、visibility、idle 対象仕様の fingerprint。値は完成した inventory certificate と参照した候補結果。`NoImage`（全探索完了）と `IdleUnavailable`（foreground 必須等を含む）を区別する |

stamp は少なくとも full-resolution modified time・size・取得できる file/directory identity を用いる。
既存 tile の秒単位 stamp を新しい否定の判定に十分とみなさない。identity が確かめられないものは再利用不可。
catalog の drive-stripped 配置は維持し、追加行の scope は KeepDrive とするので、別 drive の同名 folder の
否定を相互適用しない。持ち運び後の否定 miss は許す。手動 pin store、設定、source directory には書かない。

**リリース済みの DB への追加移行が必要。** 確認根拠は次の通り。

- `CHANGELOG.md:124` v4.1.0（2026-09-25）が、直下画像優先と既存 ZIP/PDF cache の利用を出荷事項として記載。
- `git tag --list v4.1.0 v4.7.1` で両タグを確認。
  `git show v4.1.0:src/catalog.rs` と `git show v4.7.1:src/catalog.rs` に
  `selection_proof` / `folder_selection_revision` が存在。最新記録の v4.7.1 は 2026-10-16。
- 基点 `CATALOG_VERSION` は `"2"`。`init_schema` はこの値が変わると thumbnails を削除するため、
  **この追加のために version を上げて全削除してはいけない**。

既存 DB・既存 thumbnails / proof / pin 行を保ったまま、transaction 内の
`CREATE TABLE IF NOT EXISTS` + 追加表自身の format version で移行する。
追加表がない旧 DB の読取りは未探索とし、追加列なしの旧 thumbnails も従来通り読む。
未知の追加表 version は読み飛ばし、既存画像を壊さず、この機能の記録を勝手に全削除しない。
新表への変更は `folder_selection_revision` を進めない（画像選定の結果を変える mutation ではない）。
画像行の insert/update/delete は既存 trigger に任せる。

旧版は追加表を無視し、既存画像を読める。旧版で archive 行を書き換えれば既存 trigger が revision を進める。
初期 revision がない旧 catalog は既存 additive 初期化を使う。
schema が加わっても cache manager の folder / 期限 / 全件削除対象から外れない。
`CatalogAccess` の admission epoch / SQL lease / retirement に従い、削除中の旧 idle job が DB を再作成しない。
`delete_missing` による新表 prune は **完全な物理 inventory を得た worker だけ**が行い、
別 scope や不完全な一覧による否定行削除をしない。

### 5.3 完全な否定 certificate

root に「これ以上 idle 作成対象がない」と保存できるのは、制限内の全探索が完了し、
未キャッシュだけの候補、取消、I/O エラー、budget で止めた枝が一つも残っていない場合だけ。
直接画像や有効 cache が見つかれば成功経路へ戻す。
NoImage と foreground 必須の混在なら IdleUnavailable であり、NoImage へ丸めない。

certificate は既存の catalog / pin revision 証明を再利用し、**否定用だけ**次の証拠を加える。

1. 訪問した **全 directory** の実 identity と full-resolution directory stamp。
   列挙前後が一致し、全 entry をエラーなしに列挙したこと。root だけの mtime は不可。
2. 全 archive 候補と参照 pin source の path / kind / fresh file stamp。
   directory mtime が変わらない既存 archive の内容変更も候補自身で検出する。
3. visited catalog の Present(instance, revision) / Absent、pin store の Observed revision。
   source candidate と catalog revision は可能な範囲で同一 read snapshot から採取する。
4. sort / depth / type policy / pin 解決 / idle budget 仕様版と、確定した候補結果への参照。
   不完全な証拠、Unverifiable、unknown format は再利用不可。

保存先 root catalog と必要な追加表は **最後の証拠採取より先**に初期化する。
候補結果の保存も済ませ、その後に catalog revision / Absent を取り直す。
root catalog が Absent の証拠を採取してから同じ DB へ certificate を作ると即失効するため禁止する。
新しい否定表の write 自体は画像 revision を進めない。既存の通常選定 proof は変更せず、
否定 certificate の準備と混ぜない。

directory tree は filesystem transaction で固定できない。保存直前に上記の metadata / revision を再照合し、
不一致なら完成否定を発行しない。変更直後の観測が OS / NAS にまだ見えない場合まで強い整合性は保証しない。
同一 stamp を外部ツールで偽装・復元した変更の検知は保証外とし、明示更新を使えるようにする。

### 5.4 一覧内と再訪の再利用・無効化

同じ一覧世代・同じ設定では session 内の計画 / 完成否定 / 保留を使い、idle のたびに列挙しない。
複数 archive の進行には完成した候補列と cursor を使い、1 件ごとに全 tree を再走査しない。
次候補開始前と root 否定の保存前に、その計画の証拠を worker で検証する。
変更があれば計画を捨てて新しい通常選定へ戻す。古い順序の cursor を新しい列へ適用しない。

再訪・再起動後も、idle の PreparePlan で certificate を一度検証する。
全 visited directory の stamp と全既存候補の stamp、catalog / pin revision を worker で調べ、
全部一致なら read_dir / archive open / PDF render を省略する。
これは「無 I/O」ではなく、制限内の metadata 検証だけへ軽量化するもの。
深い階層の追加/削除はその階層の directory stamp、archive 更新は当該 file stamp によって失効する。
検証できない先は有効な否定として使わず、この一覧では保留する。

| 変化・操作 | 再探索 |
| --- | --- |
| 訪問階層内の file / folder 追加・削除・改名、候補内容変更 | 再訪 / reload 時の全証拠検証で失効。表示中の一覧は既存の開いた時点の snapshot 方針を維持し、deep watcher や周期全走査を追加しない |
| ZIP/PDF / pin WebP の生成・更新・削除、catalog 消失・再作成 | 既存 instance/revision / Absent の差で失効。既存成功行を先に見る。否定行が成功を隠さない |
| 固定代表の set/remove、cascade 先の変化、metadata import | 既存 pin revision と owner への変異通知により関係する計画を破棄。新しい手動固定を最優先する |
| sort / depth / 対象形式の変更 | fingerprint が変わり別探索。既存設定変更の reload 経路を使い、専用 live rebuild を足さない |
| 明示「最新の情報に更新」 | owner の計画・予算を更新し、範囲内 root の否定 / NeedsForeground 記録を worker で bypass・失効して再調査。mtime が同じでもやり直せる。root certificate だけ外して候補 NoImage が残る実装は不可 |
| cache 管理による明示削除 | 既存 CatalogAccess epoch 失効と対象 owner 通知を使用。新表も削除に含む。read-only な窓操作をこの通知へ接続しない |
| 単なる入力・idle 再到達・repaint | 否定を失効しない。取消だけは未完なので次の idle で予算内の再開が可能 |

再訪時に計画上限へ達する root は complete certificate を持てないため、bounded metadata 調査が再び必要。
ただし候補単位の確定結果は再利用する。範囲外・budget・エラーを偽の NoImage にして、無反復に見せない。
追加表の行数は現行 stamp / policy のものに置換し、同じ source の歴代 stamp を蓄積しない。
旧設定の行は通常の cache 保守で整理し、新しい常駐 GC は作らない。

## 6. 状態の組合せを減らす検討

| 案 | 判断・理由 |
| --- | --- |
| 調査をモーダルにして入力・窓切替を止める | 不採用。操作のない時間を使う機能で、通常閲覧を止めると目的を失う |
| 設定変更時は既存 reload で作り直す | 採用。旧計画を失効させ、新しい fingerprint で開始。専用 rollback / live rebuild を設けない |
| 全ライブラリの durable job queue / 再開 journal | 不採用。表示範囲だけでよく、scope・取消・起動復元の状態が不要になる。永続化は確定した観測結果だけ |
| 1 root 内の archive を並列化 | 不採用。候補順と早期成功が崩れ、NAS 負荷・取消後完了の組合せが増える |
| 既存 worker + 全体 1 lease + 1 archive ごとに戻る | 採用。実行基盤と優先順位を一元化し、長い独自 worker loop を持たない |
| idle から直接 folder texture を生成・差し替える | 不採用。pin・proof・GPU・複数窓の別採用経路ができる。source cache を保存して通常選定へ合流する |
| Failed を永続 NoImage とみなす | 不採用。既存型は cache miss / decode error / 本当の画像なしを識別しない |
| 永続否定を候補単位だけにする | 不採用。同じ空 tree の再列挙を再訪のたびに続ける。bounded な完全 certificate を併用する |
| root mtime だけ / 全 tree watcher で否定を保つ | ともに不採用。前者は deep 変更を見落とし、後者は対象範囲外にも監視が広がる。再訪時に全 visited 証拠を worker 検証する |
| 中断位置を永続化して完全復元する | 不採用。取消・不完全探索は再開計画を memory に限り、再訪は確定候補結果から作り直す |

まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。

## 7. 実装段階と完了条件

各段は別の coherent diff とし、途中の段階で通常選定を ZIP/PDF 自動 open に変えない。
各段の証拠を記録し、独立レビューは同じ source の有効な gate を再利用する。

| 段階 | 変更範囲・成果 | 必須の回帰 |
| --- | --- | --- |
| A. 候補 / 結果の共通化 | `thumb_loader` と小さな候補 module。通常 cache-only resolver の選定順を保ち、idle 用 complete / partial / canceled を型で区別。bounded plan を pure / fake-I/O で検証 | direct image 優先、混合ソート全種、depth 0/1/設定値、child pin / active archive pin、visibility、drive key、nested entry 順、未キャッシュ≠NoImage、上限 prefix の誤採用なし、同じ ZIP の通常 read が idle の budget/cancel に汚染されない。既存 resolver テストを弱めない |
| B. 確定結果と certificate | `catalog` の追加移行 / reader / writer、proof 検証、明示更新 bypass。CatalogAccess を通す | 空 folder / 空 ZIP / 非画像 ZIP / 0 page 正常応答、取消・読取不全の非保存、同 idle / 再訪 / 再起動の無反復、root mtime 不変で deep 追加削除、archive 内容更新、pin/set/remove/import、cache insert/delete/recreate、policy 変更、別 drive scope、migration / downgrade |
| C. 優先・取消・所有 | `thumbnail_requests` / App queue / PDF dispatcher の Idle、admission lease と需要通知、session の bundle 移譲 | 通常 begin と idle pop の両順序、入力後の新規投入ゼロ、queued prune、ZIP read cancel、PDF open→render 間 cancel、送信済み render 後まで実枠保持、旧 ID の late completion、同 idx 全 kind 競合、別 idx 通常は続行、park/fork/mount/retire/drop と兄弟不変、進捗・idle blocker・repaint 投影 |
| D. 製品配線・設定・性能 | 通常 resolver への成功接続、独立 ON/OFF、owner 単位の明示更新、perf、関連 docs / manual | 代表なし→成功表示、処理途中の direct image 追加・手動 pin 変更、Loaded 非破壊、cache 保存 OFF、予算終了の静止収束、範囲出入り、設定 OFF 中に I/O 開始なし、通常先読み・idle upgrade 優先、実フォルダと仮想項目の対象差、多窓分離 |

移行 / 互換テストは、(1) 出荷版相当の旧 catalog から追加後も画像 bytes / pin 行 / proof を保持、
(2) 追加表なし read-only、(3) 新版が書いた DB を旧版相当 SQL が読める、(4) 旧版相当 insert/update/delete
後に新否定が無効、(5) 不明な表版 / 破損 certificate は採用しない、(6) 削除 admission 後の late save が
DB を再作成しない、を含む。実インストールの DB を使わず fixture DB で行う。

成功・空・失敗の worker / PDF dispatcher fake を使い、外部 GUI / 実データを必要としない回帰を先に作る。
代表的な変異（取消後も次候補を開始、partial を NoImage 化、root mtime だけ検証、pin revision 無視、
PDF lease の早期解放、owner 検証省略）を一つずつ入れると該当テストが落ちることを実装時に確認する。

検証コマンドは各段で `scripts/test-focused.ps1` または明示 filter、
`cargo check -p mimageviewer --bin mimageviewer-core`、`cargo fmt --check`。
UI 文言追加時は `python scripts/check_ui_glyphs.py`。統合時は `scripts/test-lib-full.ps1` を 1 回。
scope / dispatcher の共有変更に必要な追加 gate は実装 handoff で明示し、有効な結果を重複実行しない。
設定 UI の snapshot が増減する場合は差分の理由を報告し、設計担当確認前に PNG を更新しない。
実機で NAS 応答性を測る操作は、独立レビュー後の計画された承認済み検証でのみ行う。
この設計段階ではテスト・バイナリ build・live 実行は不要。

## 8. perf event と性能受入

`perf::is_enabled()` の内側でのみ診断値・path key を作る。入力 seq は診断相関であり妥当性判定ではない。
既存 thumb / PDF event に job ID、owner、items generation、root / candidate key を対応させる。

| event 案 | 主な値 |
| --- | --- |
| `folder_idle_admission` | allow / blocked、理由、入力/scroll idle ms、通常需要数、予算残、lease owner（変化時のみ） |
| `folder_idle_plan` | prepare / certificate_hit / invalidated / partial、dirs/entries/candidates、証拠長、metadata/catalog/total ms |
| `folder_idle_candidate` | cache_hit / begin / end、format、source bytes、ZIP read/expanded bytes、entries、open/decode/render/save ms、outcome |
| `folder_idle_cancel` | input / normal demand / out_of_scope / context_end、queued / open / render / save、要求側終端時刻 |
| `folder_idle_lease_end` | 実処理終端、cancel からの ms、同時 idle 実処理数、PDF IPC の残留時間 |
| `folder_idle_publish` | cache commit / resolver requeue / accepted / stale、request ID、更新 idx 数 |

通常の `pool_dispatch` / `pool_open_request` / UI input-to-present / frame gap と照合する。
perf ON/OFF 双方で定常 idle が静止することも確認する。

測定データは disposable な local SSD と NAS に、少数画像 ZIP、nested ZIP、空/非画像書庫、
大きい central directory、上限超過 ZIP、password ZIP/PDF、小/大 PDF、32 directory 近辺の tree を用意する。
warm/cold cache、機能 OFF/ON、通常 scroll / page open 再開、多窓競合を比較する。
source サイズだけから所要時間を推定して合格にしない。

機械的合格条件は、同時実処理 <=1、予算超過開始ゼロ、需要復帰以後の未開始 idle の dispatch ゼロ、
取消済み open 後の追加 render ゼロ、完了した空 tree の同一覧内再列挙ゼロ・再訪時 archive open ゼロ、
兄弟への表示 mutation ゼロ。UI 入力処理にこの機能由来の同期 I/O / 待機を残さない。
通常操作の遅延が OFF 比で増える場合は、native 非中断区間 / queue 待ち / disk 競合を分けて示し、
数値・対象範囲を設計担当へ戻す。上限を増やして一括生成量だけ改善する調整はしない。

## 9. 規模と実装上の留意点

バックログの Medium 表記より、所有・否定永続化・PDF 実終端を含む分だけ大きい **Medium〜Large** と見積もる。
目安は実装 1000〜1800 行、回帰 60〜100 ケース、関連文書更新、開発 6〜10 日相当
（独立レビュー・修正を含む、NAS の承認済み実測待ちは別）。測定前の概算である。

主な予定変更先は `thumb_loader.rs`、候補 / session の小 module、`catalog.rs`、
`app.rs` / `app/thumbnail_requests.rs` / `app/viewer_context_registry.rs`、`pdf_loader.rs`、
`zip_loader.rs`、設定定義 / DB read-write / preferences UI、関連 tests と docs。
公開 manual は実装受入後に対象・ON/OFF・更新操作だけを記載し、版名・worker・proof 等を説明しない。
新しいキー操作は不要。既存明示更新を使用し、keymap の割当・操作意味を変更しない。

特にレビューする境界は、(a) 完全否定を発行できる条件、(b) 全窓の通常需要の計数漏れ、
(c) PDF Abort と実 lease 終了の分離、(d) cache 削除 admission、(e) bundle の単一所有。
通常動作で繰り返す競合は構造で解決し、失敗 retry / delay / extra repaint を追加しない。

## 10. 受入前に決める利用方針

以下は前提の食い違いではなく、この案の製品方針として選べる点。独立レビュー時は推奨を仮定する。

| 論点 | 選択肢 | 推奨 |
| --- | --- | --- |
| 有効化と既定 | 独立 ON/OFF を設けて既定 OFF / 同じ設定で既定 ON / 既存 quality upgrade 設定へ統合 | **独立 ON/OFF・既定 OFF**。NAS を含め、通常は発生しなかった archive open を自動で始めるため。quality upgrade と意味が違う。初期実測後に既定 ON を再検討できる |
| 範囲 | 通常実フォルダ grid の visible + 1 行 / keep 全体 / 集約一覧にも開放 | **visible + 1 行、最大 8 root**。表示された実フォルダに限定して有効性と負荷を確かめる。集約一覧は後続の独立 scope とする |
| 上限後の継続 | 一覧世代ごと 8 archive で止める / idle 中に予算を自動補充する | **8 件で停止**。未完は否定にせず、再訪・明示更新で既存確定結果の次へ進める。時間経過だけで全探索に変わる仕様を避ける |

推奨から変更する場合は、§2 の負荷・§3 の admission・§7/8 の回帰/性能条件を合わせて更新してから実装する。
