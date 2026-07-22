# v2.7.0 メタ情報インポート・エクスポートセッション向け 第3回レビューフィードバック

作成: 2026-07-22
比較基準: `v2.6.0` (`0d504f6d`) .. `61218736`
今回の主な再確認コミット: `7f5c1ce2`

## 結論

前回指摘した「欠落 leaf の手前にある reparse ancestor」は import 時と bookmark materialize 時に
再検証されるようになりました。ただし検証済み path を通常の `PathBuf` / `GridItem::Image` に戻した後、
実際の file I/O までの間に path 構造を差し替えられる **TOCTOU の P1 が1件**残っています。

## [P1] containment 検証と実際の file I/O が同じ安全境界にない

- 場所: `src/book_bookmarks.rs:890-923`, `src/bookmark_browser.rs:493-514`,
  `src/app.rs:23663-23673`
- シナリオ:
  1. `resolve_relative_page_path` が container 内への canonical containment を確認する。
  2. 関数は canonical target ではなく lexical な `candidate` を `Existing(PathBuf)` として返す。
  3. bookmark browser はそれを通常の `GridItem::Image` として保持する。
  4. 検証後、thumbnail/metadata/fullscreen loader が open する前に ancestor を root 外 junction/symlinkへ
     差し替えると、loader は root 外ファイルを開ける。
- 同じ短い race は materialize 内でも、`book_grid_item` の検証後から `source_meta(path)` までに存在する。
- 既存の `relative_page_materialization_rechecks_containment_after_path_swap` は materialize **前**の差し替え、
  startup resolver テストは open resolver **前**の差し替えしか検証していない。
- 破れている不変条件: 信頼しない sidecar 由来の relative page は、検証時だけでなく metadata read、
  thumbnail decode、fullscreen open の実 I/O 境界でも canonical container 内に留まる。

### 必要な修正

1. relative-page 由来であることと trust root を loader まで失わない型/descriptor にする。
2. metadata、thumbnail、fullscreen の各 open 直前に containment を再検証するか、検証した file handleを
   そのまま利用して check/use を結び付ける。
3. 検証失敗は missing/invalid とし、外部 path の metadata/read/decode を開始しない。
4. `jump_to_current_book_bookmark` の「既に items にある idx」は無条件に安全とみなさない。
5. Windows junction/reparse point と Unix symlink の両方で同じ ownership boundary を使う。

単に `Existing` へ canonical path を格納するだけでは、その canonical path 自体の ancestor を後から
差し替えられるため、最終 I/O 境界の保証なしには不十分です。

### 必須回帰テスト

- materialize で安全な `GridItem` を作った後、thumbnail open 前に ancestor を root 外へ差し替え、
  root 外の decoder/read が呼ばれない。
- 同じ条件で metadata `source_meta` と fullscreen open が root 外を読まない。
- 現在一覧に既にある bookmark item を `jump_to_current_book_bookmark` する経路でも拒否する。
- container 内の通常画像、container 内を指す許可対象 reparse path、通常の missing page は維持する。
- 差し替えを再現できない環境でも、loader boundary の純粋テストでは provenance/revalidation を検証する。

## 解消を確認した前回指摘

- 欠落 leaf では最も近い既存 ancestor まで遡り、canonical container 配下か確認する。
- import preview/apply と bookmark materialize/startup resolver が共通 helper を利用する。
- root 外 reparse ancestor は leaf の有無に関係なく拒否する。
- manifest 検証、SQLite bind chunk、bounded recursive scan の前回修正も維持されている。

## 検証結果

- `cargo test --lib metadata_transfer::tests`: 17 passed
- `cargo test --lib relative_page`: 7 passed
- `cargo test -p mimageviewer --bin mimageviewer-core bookmark_open_resolver_rechecks_relative_page_containment`: 1 passed
- `cargo fmt --all -- --check`: passed
- `cargo test --workspace`: passed（失敗0）

## 完了条件

実 I/O boundary まで provenance/containment を保持し、上記3種類の I/O 回帰テストを追加してください。
修正報告では import validation、materialize、metadata、thumbnail、fullscreen の各境界で何を保証するかを
表にしてください。
