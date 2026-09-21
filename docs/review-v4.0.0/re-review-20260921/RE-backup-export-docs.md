# RE: バックアップ / 一括書き出し (K-1) と利用者向け文書 (E-1〜E-27) の再レビュー

対象: `9a8a403af`（バックアップと一括書き出し） / `01e99b32c`（文書） / `a5a1fc43b`（sitemap）。
コード・設計文書は `git show HEAD:<path>` の**コミット済み内容**を読んだ。htdocs は作業ツリー
（未コミット差分なし）。作業ツリーの未コミット差分（`src/keymap.rs` ほか、Codex の §23.9）には
触れていない。読み取り専用で実施し、アプリは起動していない。本報告の事実はすべて
(a) コードの参照 (b) read-only チェックコマンドの実出力 のいずれか。実行時挙動の観測は含まない。

---

## 1. 元指摘の判定表

| ID | 判定 | 根拠 |
| --- | --- | --- |
| E-1 README v4.0.0 節 | **公開担当へ持ち越し** | `README.md:136` が `### v3.10.0 (2026-09-14)` で最新のまま。v4.0.0 節なし |
| E-2 製品ページの機能紹介 | **解消** | `htdocs/mimageviewer/index.html:1021-1025` に「主な機能」カード「名前付きコレクション」追加。`manual/collections.html` へリンク |
| E-3 専用マニュアルページ | **解消** | `manual/collections.html`（180行、新設）/ `manual/tut-collections.html`（114行、新設）。サイドバー実測 = **30 ページすべてが 30 リンク**（CLAUDE.md Phase 1 手順 6 のループを実行）。`tutorial.html:211-215` にカード追加、`manual/index.html:194` に目次カード追加 |
| E-4 `shortcuts.html` の Delete | **解消** | `manual/shortcuts.html:181` が「コレクション直下では…登録を外すだけで、元ファイルは削除しません」に書き換え。`:190-192` に追加キーの節 |
| E-5 `remote.html` できること表 | **解消** | `manual/remote.html:205` に「コレクション（閲覧のみ）」、`:219-224` に読み取り専用・公開範囲・10,000 件の段落 |
| E-6 `version_highlights` 4.0.0 | **公開担当へ持ち越し** | `git show HEAD:src/version_highlights.rs` の `TABLE` 末尾は依然 `:853` の `"3.10.0"` |
| E-7 バージョン表記 | **公開担当へ持ち越し** | `Cargo.toml:41` / `installer/mimageviewer.iss:5` / `index.html:34,1245,1246,1251` すべて 3.10.0 |
| E-8 Phase 1 手順 4 に JSON-LD | **未解消** | `CLAUDE.md` は未変更。`index.html:34` の `softwareVersion` は今も手順から漏れている |
| E-9 sitemap | **解消** | `python scripts/gen-sitemap-xml.py --check` → `sitemap.xml is up to date (71 URLs)` exit 0。`sitemap.xml:39,189` に 2 新ページ |
| E-10 `privacy.html` | **部分的** | 日本語 `:140` は追加済み。**英語版 `:253-263` は未更新**（→ RE-6） |
| E-11 `index.html` 安心セクション | **解消** | `index.html:1224` の列挙に「コレクションの名前と登録先」追加 |
| E-12 web-remote-plan の protocol 版 | **解消** | `docs/web-remote-plan.md:1874` が「現行版は **v58**」。実装 `crates/remote-ipc/src/lib.rs:31` = 58 で一致。v56〜v58 の内容も 1 行ずつ記載 |
| E-13 `architecture-overview.md` | **解消** | `:78-81` にモジュール 4 行（`collection_store/*` / `ui_dialogs/collections.rs` 系 / `prepare.rs` / `remote_ipc/persistent_collections.rs`）、`:349` に永続化ストア `collection.db` 行 |
| E-14 `async-architecture.md` | **解消** | `:37-38` に「保存コレクション actor」「保存コレクション import / export」の 2 行を §2 の表へ追加。起動時検証→世代バックアップ、単一 read transaction、32 MiB / 50,000 行 / 10,000 件も記載 |
| E-15 `virtual-folders.md` | **部分的** | `:47-55` に Collection root と full-path キーの理由を追記。ただし E-15 が求めた「full-path キー対象 surface（検索 / タグ / 履歴 / ★ / Collection）を 1 箇所に列挙」にはなっておらず、Collection のみの記述 |
| E-16 `spec.md` | **部分的** | §2.1 メニューバー表に「コレクション」行を追加（`spec.md` のメニューバー表）。**`pinned_collections` は依然 0 件**（`git show HEAD:docs/spec.md \| grep pinned_collections` が空。実体は `src/settings.rs:4793`） |
| E-17 `keymap-spec.md` の対象外理由 | **未解消（HEAD）** | HEAD の `docs/keymap-spec.md` で collection に触れるのは `:346` の Delete 分岐 1 行のみ。製本の並べ替え画面に相当する節がない。※ §23.9（KeyAction 追加）が作業ツリーで進行中なので、着地後に再判定が要る |
| E-18 backlog §1.118 | **部分的（想定どおり）** | HEAD `docs/next-release-backlog.md:1206-1210` が「対応中（2026-09-19）」+ 正本リンク + 「v4.0.0公開後に本項を削除する」に整理済み。削除は公開後の作業として残る |
| E-19 `grid.html` | **解消** | `manual/grid.html:84`（メニューバー）と `:85`（ツールバー）の両方に「コレクション」追加 |
| E-20 `books.html` の導線 | **解消** | `manual/books.html:85` に「元ファイルをその場所に残したまま名前付きの一覧へまとめる場合は、コレクションを使います」 |
| E-21 移行ガイド | **解消** | `migrate-zippla.html:262,281,283,320` / `migrate-leeyes.html:255,290`。`.kdk` 直接取り込みをしないこと、入れ子仮想ツリーでないことも明記 |
| E-22 `known-issues.html` | **対応済みだが判定は review と逆** | `known-issues.html:107-112` に「コレクションの利用範囲」節を新設。E-22 は「掲載基準に当たらないので `collections.html` の『できないこと』へ」と判定していた。plan §23.11 は「known-issues と本書に明記する」。→ RE-12（要判断） |
| E-23 `docs/README.md` の領域別案内 | **未解消** | `docs/README.md:36-40` は説明文を改善したが「〜を触るとき」形式の案内行はない。CLAUDE.md の「触る領域 → 読むドキュメント」表にもコレクション行なし（`grep -i コレクション CLAUDE.md` が空） |
| E-24 `tutorial.html` の一般語衝突 | **未解消** | `tutorial.html:189`「増えていくコレクションに印を付けて」/ `:287`「大量のコレクションを快適に扱う」。`:189` の 22 行下 `:211-213` に機能名のカードが並ぶ → RE-7 |
| E-25 `CLAUDE.md` Project Structure | **未解消（P3、既知の広範な古さ）** | `CLAUDE.md` に collection 系の記述 0 件。E-25 の但し書きどおりツリー全体の棚卸し案件 |
| E-26 `settings.html` の優先順 | **解消** | `manual/settings.html:197`「チェック済みを優先し、なければ選択中の」。`tut-collections.html` 本文と `shortcuts.html:180` も同じ順 |
| E-27 禁止語 | **解消（公開文書）** | `git grep -i -E '…' -- 'htdocs/**' 'README.md' 'installer/*.txt'` が 0 件。`collections.html` / `tut-collections.html` にバージョン表記・内部用語（SQLite / actor / revision / worker / snapshot / protocol）も 0 件 |

---

## 2. 新規指摘

### RE-1 / **P1** / バックアップの成否が実運用のログに残らない（`eprintln!`）

- 根拠: `src/collection_store/db.rs:104`（v1 移行前）/ `:128`（通常 v2 rotate の log closure）/ `:135`
  （失敗メッセージ）がいずれも `eprintln!`。`src/main.rs:1` は `#![windows_subsystem = "windows"]`
  なので GUI 起動ではコンソールが無く、`src/lib.rs:868-884` の `AttachConsole` は CLI 経路専用。
  `git grep -c 'logger::log' -- src/collection_store` は 0。
- 対比: `src/tags_db.rs:181` は `crate::logger::log`、`src/settings_db.rs:1070` は `log_diag`。
  どちらも `mimageviewer.log` / diag ログに残る。
- なぜ P1: `docs/spec.md`（名前付きコレクション節）の「通常のバックアップ失敗はログに記録して機能を維持する」と
  `manual/collections.html:161`「通常の起動ではバックアップ作成だけが失敗しても登録を使い続けます」が、
  **公開時点で実装と食い違う**。利用者も開発者も「世代バックアップが毎起動で作られている」と思い込んだまま、
  実際には 1 度も作られていない状態を検出できない。データ保護機能の失敗が無言になる。
- 修正方向: 2 つの closure と `:135` を `crate::logger::log(...)` に替える。成功行
  （`db_backup.rs:75` の `rotate_backups: snapshot -> …`）も同じ経路に載るので、
  「毎起動で作られているか」がログで確認できるようになる。

### RE-2 / **P2** / 世代ローテーションの契機が settings.db / tags.db と違い、誤削除に対して弱い

- 根拠:
  - collection: `src/collection_store/db.rs:117-140` — `CollectionDbStartup::ExistingV2` なら
    **起動のたび無条件に** `rotate_generation_backups`。テスト
    `new_database_does_not_rotate_and_existing_v2_backup_includes_committed_wal`
    （`src/collection_store/tests.rs`）と `actor_ready_on_existing_v2_follows_startup_backup` が
    この挙動を固定している。
  - settings: `src/settings.rs:9394-9406` — `BACKUP_DONE_THIS_SESSION.swap(true)` による
    **プロセス内最初の user save** でのみ rotate。`src/settings.rs:7060` のコメントが正本。
  - tags: `src/tags_db.rs:191-196` `rotate_backups_once` → `mark_backup_needed_for_path`。
    呼び出しは `:272, 304, 348, 432, 468, 482, 507, 542, 612, 628, 683, 938` の**書き込み系入口**のみ。
    `:194` のログ文言も `(continuing with write)`。
- 何が起きるか: 利用者がコレクションを誤って削除（または一括で参照解除）した後、**コレクションを一度も
  編集せずに mIV を 10 回起動するだけで** bak1..bak10 が全部「削除後」の複製に入れ替わる。
  `integrity_check(1)` と全行読み取りは**構造破損しか見ない**ので、論理的な消失はこのガードを素通りする。
  settings / tags は編集が無い起動では世代を消費しないため、同じ事故に耐える。
- 文書との関係: 利用者判断 5 は「起動時に settings.db / tags.db と**同じ** `db_backup::rotate_generation_backups` で
  …回す」。**アルゴリズム（snapshot-first）は確かに同じだが、契機は同じではない。**
  判断を求めた文が既存 2 本を引き合いに出しているので、この差は利用者へ提示したほうがよい。
- 修正方向（どちらか）:
  1. rotate を「actor が最初の mutating command を処理する直前」へ移す（検証と Ready は今のまま open で行う）。
     `process_command` は既に `mutated` を持っているので、`rotate_once` フラグ + 実行前フックで足りる。
     副作用として RE-3 の起動コストも消える。
  2. 現状維持。その場合は spec / 実装計画から「settings.db / tags.db と同じ」という表現を外し、
    「起動ごとに 1 世代消費する」ことを明記する。

### RE-3 / **P2** / 起動時の検証 + `VACUUM INTO` に計装が無く、Ready 遅延を測れない

- 根拠: `git show HEAD:src/collection_store/db.rs | grep perf::` が 0 件。
  `src/collection_store/runtime.rs:847` の `actor_main` も `open_at` を無計装で呼ぶ。
  一方 §23.1 は `cat="collection"` を入れ、`open` / `actor_rtt` / `prepare` / `export_prepare` などを
  計っている（`src/collection_store/prepare.rs:411-438` が実例）。
- 何が抜けるか: 既存 v2 の起動経路は **DB を 3 回通る**（`integrity_check(1)` / `load_catalog` + 全 entry +
  `foreign_key_check` / `VACUUM INTO` の全複製）。10,000 件 × 複数コレクションでどれだけ Ready が
  遅れるかを測る手段が無い。CLAUDE.md「追加した同期処理の区間には perf::event を必ず差し込む
  （悪化を検知できるように）」に該当する。
- 影響範囲の確認（問題なしの部分）: この処理は `collection-store` スレッド上で走り、
  `AdmissionPhase::Starting` の間 UI スレッドは `Err(Starting)` を即座に受け取るだけなので、
  **UI スレッドはブロックされない**。コレクション UI は準備中表示になり、ほかの機能は動く。
  ただし Starting が延びるほど、起動直後のコレクション操作・起動時復元が遅れる。
- 修正方向: `collection` / `open` に `startup`(new/v1/v2) `validate_ms` `backup_ms` `bytes` `outcome` を出す。
  RE-2 の案 1 を採るなら `backup_ms` は最初の編集時に移るので、起動計装は `validate_ms` だけでよい。

### RE-4 / **P2** / 作ったバックアップの**戻し方**が文書化されておらず、素朴な手順は危険

- 根拠: `manual/collections.html:161` は「世代バックアップを作ります」で終わり、復旧手順が無い。
  settings.db と違い、コレクションには復元 UI が無い（`src/keymap.rs` の `MenuCommandId` に復元系なし）。
  `src/collection_store/db.rs:83` が `journal_mode = WAL` を設定するため、異常終了後は
  `collection.db-wal` / `collection.db-shm` が残り得る。`VACUUM INTO` が作る bak は WAL を含まない
  単体ファイルなので、**残った -wal を消さずに bak1 を collection.db へ上書きすると、古い WAL が
  新しい本体へ再生され得る**。settings.db 側が隔離を「main + WAL + SHM の 3 ファイルで 1 世代」として
  扱っている（`src/db_backup.rs` の `quarantine_group_of` とその doc コメント）のは同じ理由。
- なぜ P2: K-1 の元の問題は「利用者が手で組み上げた一覧が 1 ファイルの破損で全損し、復旧導線が無い」こと。
  世代バックアップを作っても、使い方が書かれていなければ復旧導線は半分しか埋まっていない。
- 修正方向: `collections.html` の「テキストへ書き出す・保存データ」節に 1 段落。
  「mIV を終了する → `collection.db` と `collection.db-wal` / `collection.db-shm` を別の場所へ退避する →
  `collection.db.bak1` を `collection.db` という名前でコピーする → mIV を起動する」。
  番号の大きいものほど古い世代であることも添える。

### RE-5 / **P2** / マニュアルが未コミットのキー操作機能を実装済みとして書いている

- 根拠: `manual/collections.html:101-102` と `manual/shortcuts.html:180, 190-192` が
  「操作カスタマイズで『追加先のコレクションへ追加』キーを割り当てられます。既定キーはありません」と書く。
  `git show HEAD:src/keymap.rs | grep AddToCollectionTarget` は**0 件**。
  同じ grep を作業ツリーに掛けると `src/keymap.rs:1562, 1635, 1787, …` に出る（Codex の §23.9、未コミット）。
- 判定: 文書だけが先行している状態。§23.9 が出荷されればこの記述は正しくなるが、外されたり
  文脈（グリッド / 静止画フルスクリーン / 動画・音声の 3 系統、VST 画面中は受け付けない）が変わると
  **公開時点でマニュアルが偽**になる。
- 修正方向: 公開前に、`KeyAction` の 3 種が `ini_name()` / `context()` / `trigger()` /
  `default_chords()` / `ALL_ACTIONS` / helper / `docs/keymap.ini.default` まで揃って着地したことを確認し、
  `collections.html:101-102` と `shortcuts.html:180, 190-192` の記述を実装の文言と突き合わせる。
  着地しない場合は 3 箇所を削る。

### RE-6 / **P2** / `privacy.html` 英語版にコレクションが無い（E-10 の残り半分）

- 根拠: `htdocs/mimageviewer/privacy.html:136-145`（日本語）には `:140` でコレクション行がある。
  英語の "Data stored on your device"（`:255-262`）は
  settings / thumbnail cache / tags・ratings・rotation・history / PDF passwords / remote PIN /
  remote operation records の 6 項目のままで、コレクションが無い。
- 付随: 英語版には**日本語版にある別の 2 項目も無い**（「別バージョンを探すための索引」「外部ツールへ渡す
  一時書き出し画像」）。英語版は collection 以前から遅れている。
- なぜ P2: CLAUDE.md「通信・データ保存に関わる機能を追加したときは 2 か所を突き合わせる」は
  `privacy.html` を名指ししている。日本語だけ直すと、同じ事実について 2 つの記述が食い違う。
- 修正方向: 英語リストへ
  "Collections (list names, ordering, and the locations of the files and folders you add), and their rolling backups"
  を追加。ついでに残る 2 項目も揃える。

### RE-7 / **P3** / `tutorial.html` の一般語「コレクション」が機能名カードの直上に残る

- 根拠: `tutorial.html:189`「増えていくコレクションに印を付けて、あとから探し出す…」（カテゴリ「3. 印を付けて整理する」の説明文）。
  その同じカード一覧の `:211-213` に「コレクションを作って続けて見る・聴く」が並ぶ。`:287` も同種。
- E-24 は既に指摘していたが、今回の文書追加で**同じ画面内に並んだ**ため読み違いが起きやすくなった。
- 修正方向: `:189` →「増えていくライブラリに印を付けて」、`:287` →「大量の画像・動画を快適に扱う」。

### RE-8 / **P3** / 書き出しテキストは「有効順」なので、シャッフル / 通常ソートでは手動順が保存されない

- 根拠: `src/collection_store/export_all.rs:110` → `prepare_collection_export`
  （`src/collection_store/prepare.rs:383-410`）→ `prepare_collection_snapshot_while`
  （同 `:461-546`）の `effective_collection_order`。`manual_position` は出力に入らない。
  `collections-index.txt` には `order_mode` と `standard_sort` は記録されるが（`export_all.rs:120-128`）
  手動位置は記録されない。
- したがって: シャッフル中のコレクションを書き出して取り込み直すと、**そのときのシャッフル順が新しい手動順**になる。
  仕様（利用者判断 5 は「既存 export の流用」）どおりで、`collections.html:157`
  「書き出すのはパスの一覧で、元ファイルやタグ・評価などのバックアップではありません」が
  過大評価を予防してはいる。
- 修正方向（文書のみ）: 同じ節に「手動順そのものは書き出しに含まれません。手動順の保全は
  `collection.db` の世代バックアップが担います」を 1 文。手動順を紙に残したい利用者は
  一時的に手動順へ切り替えてから書き出す、という回避も書けるとよい。

### RE-9 / **P3** / 書き出しテキストの BOM（K-8）は未対応のまま — 判断が要る

- 根拠: `src/collection_store/text.rs:133-146` `serialize_collection_paths` は UTF-8（BOM なし）+ CRLF。
  取り込み側 `:73` は `trim_start_matches('\u{feff}')` で BOM を許容するので mIV 内の往復は成立する。
  `export_all.rs:91`（index の先頭）も BOM 無し。
- 判断材料:
  - **付けるべき側**: CLAUDE.md「Markdown / テキストファイルのエンコーディング（BOM 必須ケース）」は
    「外部ツールに渡すテキスト」を対象にしており、PowerShell 5.1 の `Get-Content` が
    `-Encoding utf8` 無しで CP932 として読むことを明示している。パスに日本語が入るのは常態なので、
    `Get-Content collection.txt` は mojibake になる。書き出しは明示的に「持ち出す」ための機能。
  - **付けなくてよい側**: Win11 の既定メモ帳は UTF-8 を検出する。主な読み手は mIV 自身。
    単一コレクション書き出しの形式を変えることになる（未リリースなので互換の問題は無い）。
- 私の推奨: **付ける**。取り込み側が既に BOM を許容しており、リポジトリの既存方針と同じ向き、
  外す理由が「たいていのツールは平気」でしかない。付けるなら `collections-index.txt` も同時に。

### RE-10 / **P3** / Windows 予約名の列挙に `COM0` / `LPT0` が無い

- 根拠: `src/collection_store/export_all.rs:240-270` の `matches!` は
  `CON / PRN / AUX / NUL / CONIN$ / CONOUT$ / COM1..9 / COM¹²³ / LPT1..9 / LPT¹²³`。
  Microsoft の命名規則は現行版で `COM0` / `LPT0` も予約として挙げている（実 OS 挙動は未確認）。
- 影響: 当たる場合、`write_new_file`（`:206-210`、`create_new(true)`）が失敗し、
  `CollectionAllExportFailure::Failed` で**書き出し全体が中断**する。無言の取りこぼしではないので実害は小さい。
- 修正方向: 2 語を列挙へ追加し、`sanitized_stem("COM0")` のテストを
  既存の `empty_export_and_long_names_have_complete_bounded_outputs`（同ファイル）へ 1 行足す。

### RE-11 / **P3** / index の sort 欄だけ `{:?}`（Debug）で、`order_mode` と綴りが揃っていない

- 根拠: `src/collection_store/export_all.rs:120-128` —
  `snapshot.definition.order_mode.as_str()` と `{:?}` の `standard_sort` が同じ行に並ぶ。
  テストの期待値も `"\tmanual\tFileName\t1\t"`。
- 影響: `collections-index.txt` の列 1 つだけが Rust の enum 名に依存し、enum をリネームすると
  ファイル形式が黙って変わる。CLAUDE.md「One Owner Per Spelling」の観点。
- 修正方向: `SortOrder` にも `as_str()` 相当を置き、両方を同じ綴りの owner から出す。

### RE-12 / **P3** / `known-issues.html` への掲載が review E-22 の判定と逆 — 要判断

- 根拠: `manual/known-issues.html:107-112`「コレクションの利用範囲」。内容は
  10,000 件上限 / 入れ子不可 / ZIP・PDF 内 1 ページ不可 / 再リンク画面なし / Remote 編集不可 /
  外部変更の自動反映なし。
- 食い違い: E-22 は「CLAUDE.md の掲載基準『②不具合だと思う見た目をしている』に当たらない。
  `collections.html` の『できないこと』節が正しい場所」と判定した。
  一方 plan §23.11 は「前提件数（10,000）は known-issues と本書に明記する」と書いている。
  現状は**両方**に載っている（`collections.html:165-171` の「利用範囲」節にも同内容）。
- 判断材料: known-issues は Phase 1 手順 6.5 で「この版で直した項目を削除する」棚卸し対象。
  仕様上の範囲を載せると、直らないので**永久に残る行**になり、棚卸しの意味が薄れる。
  一方「1 コレクション 10,000 件」は利用者が事前に知りたい数字でもある。
- 修正方向（推奨）: `known-issues.html` の節は落とし、`collections.html#limits` への 1 行リンクだけ残す。
  残す判断をするなら、棚卸し対象外の「仕様上の制限」であることが分かる見出しにする。

### RE-13 / **P3** / 未リリース機能なのに「すでに上限を超える古い一覧」を利用者向けに書いている

- 根拠: `manual/collections.html:167`「すでに上限を超える古い一覧は読取・解除・並べ替えを続けられますが、追加はできません」。
  `manual/remote.html:222-223`「以前からある超過分を勝手に削除することはありません」。
- コレクションは v4.0.0 が初出なので、**公開時点で利用者の手元に 10,000 件超の一覧は存在しない**。
  読者は「どういう状況の話だろう」と止まる。設計上の保証としては正しい（`src/collection_store/db.rs:321`
  の `saturating_sub` が既存超過を削らない）が、それは設計文書側で足りる。
- 修正方向: 利用者向けからは削り、「1 コレクションに登録できるのは 10,000 件までです」だけにする。

### RE-14 / **P3** / 実装計画 §23 の状態行が §23.10 の完了を反映していない

- 根拠: `git show HEAD:docs/collection-implementation-plan.md:1104` は
  「§23.8 は…実装・検証中。**ほかの残件は未着手**」だが、`01e99b32c` が §23.10 の大半を実施済み。
- ※ 同ファイルには Codex の未コミット差分があるため、そちらで更新される可能性がある。
  公開前の台帳として読むときは HEAD の状態行を信じないこと。

---

## 3. 確認して問題なし

### 世代バックアップ（K-1 前半）

- **actor スレッド上で走り、UI スレッドを止めない**: `src/collection_store/runtime.rs:109-118` が
  `collection-store` スレッドを立て、`:847` の `actor_main` が `CollectionStoreDb::open_at` を呼ぶ。
  完了するまで `AdmissionPhase::Starting`（`:101`）のままで、`request`（`:700-716`）は
  `Err(Starting)` を即返す。UI は待たない。
- **起動ごとに 1 回**: production の `open_at` 呼び出しは `src/lib.rs:1295` の 1 箇所だけ
  （残りはすべてテスト）。rotate は `CollectionDbStartup::ExistingV2` の分岐内に 1 回だけ置かれている。
- **新規 DB / 空 DB では回さない**: `CollectionDbStartup::New` と `ExistingUnversioned` は
  `initialize_schema` のみ。テスト `new_database_does_not_rotate_and_existing_v2_backup_includes_committed_wal`
  が `bak1` の不在を固定。
- **将来版・破損 DB で既存 chain を押し出さない**: `newer => IncompatibleSchema` で早期 return。
  破損は `validate_integrity`（`db.rs:589-597`）/ `validate_existing_v2`（`:611-618`）/
  `validate_entries_and_foreign_keys`（`:620-636`）で弾く。
  テスト `invalid_or_future_v2_does_not_rotate_known_good_backup` と
  `orphaned_v2_entry_does_not_displace_known_good_backup` が「`bak1` の中身が不変」「`bak2` が生えない」まで固定。
- **v1 移行前 backup は必須、失敗したら移行しない**: `db.rs:93-113` が `.map_err(…)?` で伝播し、
  `migrate_v1_to_v2` はその後。テスト `v1_backup_failure_remains_fail_closed_before_schema_write` が
  `user_version` が 1 のまま残ることまで確認。`corrupt_v1_schema_does_not_rotate_a_known_good_generation` も併設。
- **同じ起動で二重 rotate しない**: v1 経路と v2 経路が `match startup` の排他分岐。
  テスト（`v1_migration_preserves_…` の末尾）が「別起動で初めて bak2 が生え、その中身は user_version=1」を固定。
- **通常 v2 の backup 失敗は機能を維持**: `db.rs:126-140` は `if let Err(…)` でログのみ（→ RE-1 はログ先の話）。
  テスト `existing_v2_backup_snapshot_failure_is_nonfatal_without_rotating_chain` が
  「`bak1` 不変・`bak2` 無し・スナップショットは読める」を確認。
- **WAL を含む**: `VACUUM INTO` を書き込み接続から実行。read-only probe 側も
  「Read-only probing includes committed WAL rows」とコメント済み。
  テストが「WAL に書いた 1 件が bak1 に入る」を確認。
- **chain は snapshot-first**: `src/db_backup.rs:29-78` の共有実装をそのまま使い、変更していない
  （T42 の順序が維持される）。世代は bak1..bak10 に上限があり、`preupgrade-*` のような無制限増殖はしない。
- **ポータブル版**: `src/lib.rs:1295` が `data_dir::get().join("collection.db")`、
  rotate は `path.parent()` を渡すので `<exe_dir>\data` に出る。APPDATA 固定の経路は無い。
- **設定リセットの対象外**: テスト `settings_full_reset_does_not_change_collection_family_or_runtime_snapshot`
  が `collection.db.bak1` の中身まで含めて不変を確認。

### 一括書き出し（K-1 後半）

- **単一 read transaction で同一 revision に固定**: `db.rs:155-181` の `export_all_snapshot` が
  `unchecked_transaction` 内で `load_catalog` → 各 `load_snapshot_with_catalog_cancel` を回す。
  書き込みは同じ actor スレッドなので並走しない。`export_all.rs:100-105` が
  `definitions[position] == snapshot.definition` と `catalog_revision` 一致を毎件再検証し、
  ずれたら**黙って省略せず**失敗させる。テスト
  `one_actor_export_request_freezes_catalog_and_all_member_revisions` が、書き出し要求の後に
  rename を挟んでも bundle が古い名前のままであることを確認。
- **書き込みは worker、進捗と取消あり**: `ui_dialogs/collections.rs` の
  `CollectionDialogOperation::ExportAllSnapshot` → `ExportAllWriting`。後者は
  `WorkerTask::spawn("collection-export-all", …)`。進捗は `(done, total)` の
  `Arc<(AtomicUsize, AtomicUsize)>` で、`draw_collection_operation_status` が 2 段階とも表示する。
  取消は snapshot 段が `AtomicBool`（actor が entry ごと `db.rs:746-748` と collection ごとに確認）、
  書き込み段が `WorkerTask::cancel`。
- **取消時に owner を手放さない**: `show_collection_manager` の cancel 分岐と
  `show_detached_collection_operation_status` の cancel 分岐が、export-all のときだけ
  operation を `Idle` にせず「取り消しています…」に留める。返事が来て初めて未完成フォルダの有無を報告する。
  テスト `cancelled_all_export_actor_reply_cannot_start_a_late_worker` が
  「取消後に届いた actor 応答で worker を起こさない」「フォルダを作らない」を固定。
- **管理画面の選択変更で巻き込まれない**: `CollectionUiState::select_collection` が
  `is_export_all()` のときだけ `cancel_worker` + `Idle` を飛ばす。
  テスト `all_export_keeps_its_owner_across_manager_selection_and_reports_completed_folder` が固定。
- **既存を一切上書きしない**: フォルダは `create_dir`（`create_unique_export_folder`、`collections-export`
  → `-1` … `-9999`）、各ファイルは `OpenOptions::create_new(true)` + `sync_all`、
  index は `MoveFileExW` を `MOVEFILE_REPLACE_EXISTING` 無しで呼ぶ。
  テスト `index_publication_never_overwrites_an_existing_destination` が、既存 index が残り
  pending も消えないことを確認。
- **完成印は最後**: `# complete\r\n` を末尾に持つ `collections-index.txt` が最後に publish される。
  失敗・取消時は index を作らず、`CollectionAllExportFailure::{Cancelled,Failed}` が
  未完成フォルダのパスを添えて返す。外部ファイルを巻き込む recursive cleanup はしない。
  テスト `cancellation_and_failure_leave_explicit_unindexed_partial_folder` が
  「first.txt はある / index は無い」まで確認。UI 側も `Cancelled` を `(false, …)`、
  `Failed` を `(true, …)` と区別して表示する。
- **名前の安全化**: `sanitized_stem` が制御文字と `<>:"/\|?*` を `_` に、UTF-16 100 単位で打ち切り
  （`len_utf16()` を足す前に判定するのでサロゲートを割らない）、末尾の空白とドットを除去、
  最初のドットより前が予約名なら `_` を前置。大小文字衝突は `used.insert(candidate.to_lowercase())` で
  連番回避。`collections-index.txt` 自身を `used` の初期値に入れてあるので、同名コレクションは
  `collections-index-1.txt` になる。空名は `collection` へ。テストが `_CON.txt` / `SAME-1.txt` /
  `collections-index-1.txt` / `あ×200` の長さ上限をすべて確認。
- **取り込みでそのまま読める**: `serialize_collection_paths` の whole-line quote（空白を含む行のみ）と
  `unquote_whole_line`（`text.rs:148-166`）が対になっている。CRLF は取り込み側が `trim_end_matches('\r')`。
  欠損参照も出力に含む（テストが `missing image.png` の出力を確認）。
- **rfd のフォルダ選択**: `src/ui_main.rs:6185` の `rfd::FileDialog::new().pick_folder()` は
  既存の単一 import / export（同 `:6148`, `:6166`）とまったく同じ位置・同じ呼び方。
  戻った後に `start_collection_export_all` が `collection_export_all_available()` を**再確認**するので、
  ダイアログ中に状態が変わっても取りこぼさない。→ 許容範囲。
- **メニューの可用性**: `collection_export_all_available()` = `can_edit() && operation.is_idle()`。
  コレクション直下にいる必要はない（バックアップ用途として正しい）。無効時のホバー文言もある。
  `menu_command_is_available_in_build` からは除外されていない（除外は Relink のみ）。
- **revision を進めない**: `process_command` の `ExportAllSnapshot` 分岐は `mutated` に触れないので、
  書き出しが watch 通知を撒かない。

### 文書

- 新設 2 ページを含め、`collections.html` / `tut-collections.html` に**バージョン表記が 0 件**、
  内部用語（SQLite / actor / revision / worker / snapshot / Tantivy / protocol / thread / IPC）も **0 件**。
- 禁止語（ダウンローダ名・投稿サイト名）は `htdocs/**` `README.md` `installer/*.txt` で 0 件。
- `collections.html` の主要記述はコミット済み実装と一致する（照合したもの）:
  BOM と LF/CRLF の許容（`text.rs:73`、`:79`）/ 相対パスの基準は「テキストファイルのあるフォルダ」
  （`text.rs:69`）/ 行全体の二重引用符（`text.rs:148-166`）/ 空行は読み飛ばす（`text.rs:78-81`）/
  **`#` はコメントではなくパス**（parser に `#` の扱いが無い＝そのとおり）/ URL 不可
  （`path.rs:31` の `validate_external_text_path`）/ 32 MiB・50,000 行（`text.rs:7-8`）/
  無効行が 1 つでもあれば全体を開始しない / 10,000 件は別上限で入力順に残り分だけ追加
  （`model.rs:12`、`db.rs:321` の `saturating_sub`）/ 並べ替え画面はキャンセル無しで閉じる時に保存
  （利用者判断 4）/ 外部変更は自動反映せず明示更新が要る（plan `:1333`, `:1350-1353` の 2026-09-20 決定）/
  Remote は読み取り専用。
- **未実装機能を実装済みと書いていないか**: D&D 追加・M3U・登録順ソート・件数表示・終了時の自動書き出しは
  いずれも新ページに記述が無い（§23.11 の送り分けと一致）。唯一の例外が RE-5（キー操作）。
- `docs/spec.md` のコレクション節にバックアップと一括書き出しの段落が追加され、実装と一致している。
- `index.html:1200-1207`「🌐 通信するのは 3 つの場面だけ」は v4.0.0 でも偽にならない
  （コレクションは通信経路を増やさず、Remote の既存経路を読むだけ）。E 報告書 §E の判定を再確認した。

---

## 4. 公開担当（リリース Phase 0〜1）の作業リスト

開発側では着手せず、ClaudeCode Opus の公開タスクとして残っているもの。上の判定表で
「公開担当へ持ち越し」とした項目 + Phase 0/1 の定型。

**Phase 0（利用者レビューが先）**

1. `README.md` に `### v4.0.0 (YYYY-MM-DD)` 節を新設し、**利用者の承認を得る**（E-1）。
   下書きは E 報告書 §A + 下の §5 の差分メモ。
2. **バイト数を測る**。§A の時点で 6,859 / 8,192 だったので、§5 の追加分を入れると
   `src/update_check.rs:106` の `BODY_CAP = 8 * 1024` を**超える見込み**。
   `awk '/^### v4\.0\.0( |$)/{f=1} /^### v3\.10\.0( |$)/{f=0} f' README.md | wc -c` で確認し、
   超えたら `docs/release-body-4.0.0.md`（BOM なし、8,192 バイト以内）を作って Phase 4 の
   Release body に使う。README はフル版のまま残す。

**Phase 1**

3. `Cargo.toml:41` / `installer/mimageviewer.iss:5` / `installer/readme.txt:2` /
   `installer/readme_portable.txt:2` のバージョン（E-7）。
4. `htdocs/mimageviewer/index.html` の `:1245`（版表記）/ `:1246`（最終更新日）/
   `:1251`（ポータブル zip の href）/ **`:34` の JSON-LD `softwareVersion`**（E-7 + E-8）。
   あわせて CLAUDE.md Phase 1 手順 4 に JSON-LD を追記する（E-8）。
5. `htdocs/mimageviewer/manual/index.html` のマニュアル版表記（Phase 1 手順 5）。
6. `python scripts/gen-changelog-html.py` で `manual/changelog.html` を再生成（手順 4.5）。
   サイドバーは `getting-started.html` から複製されるので、30 リンクで揃っている現状のまま追随する。
7. **`src/version_highlights.rs` の `TABLE` に `"4.0.0"` を追加**（E-6、手順 5.5）。
   これはコード変更なので `cargo test --lib version_highlights::` を通す。
   案は E 報告書 §C。**§C の 4 件に加えて、下の §5 で増えた分を反映する**
   （シャッフルの追加、コレクション一覧での 10,000 件上限、Ctrl+G の索引再構築）。
8. `known-issues.html` の棚卸し（手順 6.5）。既存 3 件は v4.0.0 で直っていないので残す。
   **RE-12 の判断**（コレクションの利用範囲節を残すか `collections.html` へ寄せるか）をここで決める。
9. htdocs の編集をコミットした**後**に `python scripts/gen-sitemap-xml.py`（手順 6.6）。
   現時点では up to date（71 URLs）なので、Phase 1 の編集後にもう一度回す。

**開発側で先に閉じておきたいもの（公開担当へ渡す前）**

- RE-1（バックアップのログ先）、RE-5（キー操作の着地確認）は**公開までに閉じないと文書が偽になる**。
- RE-2 / RE-4 / RE-9 / RE-12 は利用者判断を仰ぐ項目。
- E-15 / E-16（`pinned_collections`）/ E-17 / E-23 / E-24 / E-25 は設計文書・マニュアルの残り。

---

## 5. README v4.0.0 下書きへの差分メモ（E 報告書 §A に対して）

§A は 2026-09-16 の HEAD（`0a7139d27`）時点。その後に入った分を反映する。
**内部用語を使わない / バージョンタグは見出しの 1 回だけ / 特定の投稿サイト名・ダウンローダ名を出さない**
という §A の制約はそのまま。

### A. 既存項目の書き換え

| §A の項目 | 変更内容 |
| --- | --- |
| 1 番目（コレクション導入） | ソートの説明に**「シャッフル」**を足す。ツールバー / メニューだけでなく、**一覧のソート順に「手動順」「シャッフル」が一級の選択肢として並ぶ**ことを書く（§23.2、案 C）。上部メニューに「すべてのコレクションをテキストで書き出す…」が増えたことも触れる |
| 2 番目（元ファイル非変更） | 右クリックの「**元の場所へ移動**」を追加（A-6）。「参照だけを外す」「元ファイルを操作する」「元の場所を開く」の 3 つが別操作であることを整理する |
| 4 番目（並び順） | 「手動順」「通常ソート」の 2 択から **3 択**へ。シャッフルは手動順を保ったまま並び替え、選び直すと並び直ること、一覧にも次が見えることを書く |
| 5 番目（テキスト入出力） | **1 コレクション 10,000 件の上限**、取り込みの 32 MiB / 50,000 行を追記。書き出しに**「すべてのコレクションをテキストで書き出す…」**（親フォルダを選ぶと新しいフォルダを作り、既存を上書きしない）を追記 |
| 6 番目（Remote） | **読み取り専用であること**を明示し、ページ送り・位置指定が**種類ごとの順番**に従うことを 1 文で（D-4、実媒体別の着地位置修正） |

### B. 新規に足す項目

1. **コレクションの内容を自動でバックアップするようになりました。** 起動時に内容を確かめてから、
   データフォルダに最大 10 世代の控えを作ります。控えの作成に失敗しても、そのまま使い続けられます。
   （※ RE-1 / RE-2 の結論によって表現を調整する。RE-2 案 1 を採るなら「起動時に」→「最初に編集したときに」）
2. **横断検索（Ctrl+G）に進み具合が出るようになりました。** 検索中にどこまで進んだかが分かり、
   長い検索でも止まって見えなくなりました。
3. **⚠️ 初回起動時に、横断検索の索引を作り直します。** 生成情報の扱いを見直したため、
   更新後の最初の起動で索引の再構築が走ります。完了するまで検索結果が一部そろわないことがあります。
   （根拠: `a73d18b48` で `INDEX_VERSION` が 10 へ。`src/fts_index.rs:3`）
4. **「重なる本」の検索が終わらないことがある問題を修正しました。** 1 冊 3,000 ページ /
   候補 10,000 ページを上限として打ち切り、打ち切ったことを知らせます。
   （根拠: `02034bf7d` / `c7158fc83` / `cc3d580bb`）
5. **スマートフォルダから本を開いたあとの移動が正しくなりました。**
   （根拠: `6f720e42c` "Fix smart-folder navigation ownership and ordered book traversal"。
   利用者向けの症状の言い方は、公開担当がコミット本文と backlog で確認して決める）

### C. バイト数

§A が 6,859 バイト。上の A（既存 5 項目の加筆）+ B（5 項目の追加）で確実に 8,192 を超える。
**`docs/release-body-4.0.0.md`（短縮版）を作る前提で進めるのが安全**。
短縮版は「コレクション（1 本にまとめる）→ ⚠️ 索引の再構築 → 主な改善 → 主なバグ修正」の順に
前方へ重要項目を寄せ、冒頭に「全項目は README を参照」のリンクを置く。
`wc -c docs/release-body-4.0.0.md` で 8,192 以下を確認する。

### D. 判定の前提

E 報告書 §A は `git log v3.10.0..HEAD` を全件見て「v3.10.0 に含まれるか」を判定済み。
上の B は `v3.10.0..HEAD` のうち §A 作成後（`0a7139d27` 以降）に入った commit だけを対象にしている。
公開担当は Phase 0 で `git log v3.10.0..HEAD --oneline` をもう一度全件見て、
§A + 本メモに漏れがないか突き合わせること。
