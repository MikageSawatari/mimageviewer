# §1.228 起動時 Full の ZIP / PDF open 前照合

## 目的と判断

2026-09-12 の v3.9.1 公開前確認では、起動時の類似索引 Full reconcile が 391 秒かかり、
変更のない PDF 3,580 件の列挙が worker 累積 4,512 秒を占めた。既存実装は filesystem 走査で
`mtime` と `file_size` を得ているが、ZIP / PDF を開いて `page_count` を数えた後に
`container_observation` を呼んでいた。

利用者は、保存済み Complete コンテナについて `mtime` とサイズが同じなら、同じ値のまま内容だけ
変わったケースを更新なしとして扱うことを了承した。これは起動時 `FullReason::Initial` に限って
適用する。2026-09-11 の設計で採用しなかった列挙省略は、この限定条件と明示的な利用者判断により
置き換える。実 corpus での短縮倍率は未測定であり、本番 DB やフォルダを開発用検証に使わない。

## 所有と判定

`load_full_reconcile_inventory` が scan 前の有限 read transaction で得た immutable snapshotだけを
使い、candidate ごとの SQLite read は追加しない。open 前に再利用できるのは次の条件がすべて成立
するときだけである。

- container kind が対象の ZIP / PDF と一致する。
- `scan_state == Complete`。
- 保存済み `page_count` が正の `u32` で、公開済み member 数と一致する。
- filesystem 走査済みの `mtime` / `file_size` が保存値と一致する。
- 全 member が現在の hash version で、inventory 自身も同じ version を表す。

不正値、欠落、不一致はすべて `NeedsOpen` とし、従来の列挙、0ページ/破損/パスワード失敗、
post-open freshness、generation staging、publish へ戻す。`mark_container_seen` は判定前に維持し、
再利用時は保存済み page count を discovered と ZIP/PDF page telemetry、member 数を processed と
unchanged に反映する。

policy は job 開始時に `FullReason` から一度だけ導出する。`Initial` だけが metadata trust を使い、
Reconfigure、Overflow、WatchRecovery、SummaryRepair、Manual と Delta は必ず従来どおりコンテナを
開く。これにより password 設定変更、明示修復、watch gap の再確認を変えず、Delta の scope、
transaction、publication、prune、cancel/error 契約にも変更を入れない。別 process で変わった
credential を起動時に識別する永続 fingerprint は持たないため、既存 Complete は今回了承された
起動時 metadata trust の対象となる。Failed / Building / Missing は常に開く。

同一 config epoch / watch gap で複数の Full intent が合流する場合も、Manual 等の `MustOpen` を
Initial の metadata trust へ弱めない。待機中と中断後の再投入のどちらでも、より厳しい open 方針を
持つ intent を scheduler owner に残す。

## 回帰と完了条件

- Complete で無変更の ZIP / PDF は Initial Full で loader を呼ばず、保存件数と従来同じ report /
  telemetryを生成する。
- kind、state、page count、member count、mtime、size、member hash、hash version のどれかが不一致なら
  open 経路へ戻る。
- Reconfigure/password、Manual、repair 系 Full と Delta は Complete でも open 後判定を使う。
- ZIP / PDF の差替え、ページ追加/削除、破損、パスワード要求、0ページは既存の型付き結果と旧
  Complete 保護を維持する。取消は final publish しない。
- 画像本、ZIP/PDF loader、仮想フォルダ閲覧には変更を加えない。

## 開発検証記録（2026-09-12）

実装・テストと独立レビューは別のSol/xhigh担当。最終レビューはblocking 0で承認。
`target/section228-similar-container-preopen-20260912/`に各実行ログを保存した。
対象回帰（DB事前判定、Initial no-openと署名/世代/journal不変、非Initial must-open、
Manual merge、incremental reconcile、full inventory）、core check、fmt、glyph、diff-checkは成功。
`RUST_TEST_THREADS=1`で`test-full.ps1 -SuppressCrashDialogs`がexit 0、mainは
8300 passed / 0 failed / 45 ignored、workspace/integration/doc/vendorも成功した。

`build-dev.ps1 -PreserveRuntime`はexit 0。ビルド後も対象製品ソースは検証時のhashと一致した。
core SHA-256は`9AD0B1B3874F314E119A76082B12DB8C64744B86F0AE9AC7C203DAB1CDF9AF5A`、
remoteは`A89F6516CC2EB65B39E5BA17E92E867CAB7B83BD29D954E03A3197901A50B53C`。
インストール済みアプリは停止・操作していない。実コーパスでの起動時間は未計測。
