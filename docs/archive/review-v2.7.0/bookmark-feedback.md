# v2.7.0 ブックマーク拡張セッション向け 第3回レビューフィードバック

作成: 2026-07-22
比較基準: `v2.6.0` (`0d504f6d`) .. `61218736`
今回の主な再確認コミット: `ae6dde0d`

## 結論

前回の ZIP ルート移動、detached 画像レーティング、クラッシュ回復 journal の追加は確認できました。
ただし journal の状態判定と rollback 完了判定に **P1 が2件**、同じ rating ownership の
横断確認で **P2 が1件**残っています。v2.7.0 出荷前に修正してください。

## [P1] ファイルをまだ変更していない journal まで起動時に適用する

- 場所: `src/books.rs:90-138`, `src/books.rs:413-422`,
  `src/book_bookmarks.rs:555-592`
- シナリオ: 本の改名・並べ替え・ページ移動で、write-ahead journal を保存した直後、最初の
  filesystem 操作より前にプロセスが終了する。次回起動時の recovery は journal の全 mapping を
  無条件に `migrate_paths_with_journal` へ渡すため、ファイルは旧パスのままなのにブックマークだけが
  新パスへ移る。
- 破れている不変条件: recovery は filesystem で実際に確定したページ identity にだけ
  ブックマークを追従させる。準備だけで終了した操作は no-op でなければならない。
- 根本原因: journal が mapping だけを持ち、`Prepared` / filesystem 適用中 / filesystem commit 済みを
  区別できない。現行の swap/cycle テストは filesystem を変えずに journal を回収し、むしろ誤った
  動作を正解として固定している。

### 必要な修正

1. journal に operation phase と、回復に必要な filesystem progress/identity を永続化する。
2. 最初の filesystem 変更前に crash した `Prepared` はブックマークを移行せず破棄できるようにする。
3. filesystem commit 後だけ DB mapping を適用する。phase 更新と filesystem 操作の間にある crash 窓も
   回復可能にする。
4. swap/cycle は old/new の存在確認だけでは前後を判定できないため、一時名を含む操作計画または
   冪等な step journal で回復する。

### 必須回帰テスト

- journal 保存後・filesystem 未変更で再起動してもブックマークは旧パスのままで journal が解決する。
- 1件移動後・DB 未反映では新パスへ追従する。
- 2ページ swap / 3ページ cycle の各 crash point から、ファイルとブックマークが同じ最終 identity に収束する。
- recovery を2回実行しても結果が変わらない。

## [P1] rollback の一部失敗を成功扱いし、回復 journal を捨てる

- 場所: `src/books.rs:125-138`, `src/books.rs:636-661`,
  `src/books.rs:1521-1548`, `src/books.rs:1730-1744`
- シナリオ: 複数ページ移動や並べ替えの途中で本処理が失敗し、その後の file rename/copy/delete の
  rollback も一部失敗する。`rollback_completed_transfers`、`rollback_moved_page`、
  `rollback_temp_moves`、`rollback_reorder_pass2` は個々の I/O error を捨てるため、呼び出し側は完全に
  元へ戻ったと判断する。`PreparedBookmarkMigration::Drop` が journal を削除し、stale bookmark と
  orphan temp/final file を残す。
- 破れている不変条件: 通常エラーでも、journal を破棄できるのは filesystem rollback が全件成功したと
  証明できた場合だけである。
- 根本原因: rollback helper が `Result` と失敗対象を返さず、journal ownership と rollback 成否が
  同じ境界で決定されていない。

### 必要な修正

1. 全 rollback helper を fallible にし、rename/copy/delete の失敗を集約して呼び出し側へ返す。
2. 全 step の復元成功を確認した場合だけ prepared journal を discard する。
3. 1件でも復元できなければ journal と診断情報を保持し、次回 recovery が現 filesystem 状態から
   安全に収束できるようにする。
4. 上の phase-aware recovery と一体で設計する。現行の無条件 mapping 適用のまま journal だけ残すのは
   安全な修正にならない。

### 必須回帰テスト

- transfer rollback の rename/copy/delete を fault injection で失敗させ、journal が残る。
- reorder pass1/pass2 の rollback 失敗でも journal と一時ファイル情報が残る。
- rollback 全成功時だけ journal が消え、ブックマークは旧 identity のままになる。
- rollback 失敗後の再起動 recovery が file/bookmark を同じ identity へ収束させる。

## [P2] detached の「現在の本/フォルダ」レーティングだけ共有世代へ記録されない

- 場所: `src/app.rs:39436-39600`, `src/app.rs:40345-40384`
- シナリオ: detached viewer で Shift+F1〜F6 により現在の画像フォルダ/ZIP/PDFへ星を付ける。
  通常画像の `set_rating` と path 指定の `set_folder_rating_by_path` は
  `record_rating_session_write` を呼ぶが、`set_current_folder_rating_internal` は呼ばない。
  main 側の一覧キャッシュや古い hydrate 結果が detached の最終書き込みを認識できない。
- 破れている不変条件: rating のユーザー書き込み世代は、画像かコンテナか、main か detached かに
  関係なく path identity ごとに共有される。

### 必要な修正とテスト

- rating DB write の共通 boundary で必ず `record_rating_session_write` と他 context の cache invalidationを行う。
- main に親フォルダ一覧を表示したまま、detached で子フォルダ/ZIP/PDFへ Shift+F1〜F6 を実行し、
  close 後に main の星と DB が一致するテストを追加する。
- 同じ path の古い hydrate result を pending にしたケースと、0クリアのケースも含める。

## 解消を確認した前回指摘

- ZIP 子階層からルート直下ページへ戻る空 prefix と、stale prefix で現在階層を維持する処理は修正済み。
- detached で通常画像の rating 変更/0クリアを行った際の shared generation と main cache 更新は修正済み。
- filesystem 操作前に最終 old→new mapping を保存し、DB commit 後に削除する journal の骨格は追加済み。
  今回の指摘はその recovery/rollback phase の完全性に関するもの。

## 検証結果

- `cargo test --lib book_bookmark`: 17 passed
- `cargo fmt --all -- --check`: passed
- `cargo test --workspace`: passed（失敗0）

## 完了条件

上記3件を修正し、各 crash/rollback point の fault-injection テストを追加してください。修正報告では、
「journal phase」「filesystem の正本判定」「journal を破棄できる条件」「main/detached rating の共有境界」を
対応表にしてください。
