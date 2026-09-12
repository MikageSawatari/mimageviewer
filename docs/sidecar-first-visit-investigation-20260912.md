# サイドカー復元ダイアログの初回訪問調査（2026-09-12）

## 対象と結論

v3.9.0 で `G:\home\comfyui\202609-21\_ランウェイのモデル` を初めて開いたときだけ、
「サイドカーから設定を復元中」ダイアログが一時表示された事象を読み取り専用で調査した。

この訪問で約 0.9 秒を占めた主因は、サイドカーの存在確認そのものではなく、初訪問時に
300 画像を対象に行った旧 XMP タグ seed の完了待ちだった。2026-09-12 の利用者了承に基づき、
この自動救済経路は撤去した。あわせてダイアログの見出しを実phaseから導出し、中央DBへの
importを行う `Running` だけを「サイドカーから設定を復元中」と表示するよう変更した。

## ログ証拠

通常プロファイルの `%APPDATA%\mimageviewer\logs\mimageviewer.log` に次の時系列がある。

- 初回: `load_folder` 3059.779s、restore開始 3059.782s、入力解放 3060.704s
  （restore ownerの存続時間は約922ms）。
- 3060.682s に
  `[TAG] legacy seed complete: candidates=300 read=300 imported_items=0 inserted_tags=0 marked_empty=300 skipped_decided=0 read_errors=0 db_errors=0`
  が記録されている。seed完了から入力解放までは約22msだった。
- 同じフォルダの再訪は restore開始 3061.895s、入力解放 3061.920s（約25ms）。
- 再々訪は restore開始 3066.890s、入力解放 3066.906s（約16ms）。
- 初回の `content_identity: detection ready` は入力解放と同じ 3060.704s に出ており、
  約0.9秒の待機を代表サムネイル選定やcontent identity検出の時間とは分類できない。

同日、`D:\home\photo\2022\2022-02-20` の別sessionでも同じ境界を確認した。restore開始
3698.384s、legacy seed完了3703.899s、入力解放3703.913sで、待機は約5.529秒だった。
156候補のうち152件を読み、4件は決定済み、importされたタグは0件、152件を空として決定した。
必要部分のログは
`target/section226-legacy-xmp-retirement-20260912/photo-2022-02-20.log`
（SHA-256 `AAA606441DBCF5BDCAA1D8BE8842897638D44159875DC2ED173327DFFCA04F15`）へ保存した。

v3.8.0ではlegacy seedはフォルダ表示後のバックグラウンド処理だったが、commit `9c9df532c` で
通常のsidecar restoreがquiescenceを通るようになり、seedの完走も入力解放前の条件になった。
したがってこの5.529秒は、以前からあった処理をダイアログで見せただけではなく、v3.9.0で
操作待ちへ変わった退行である。今回の自動seed owner撤去は、この待機原因そのものを除去する。

旧タグseedは未決定itemのXMPを読み、タグが無いitemも `tag_item_state` で決定済みにする。
再訪時は決定済みkeyをbulk queryで先に除外するため、同じ300ファイルを再読込しない。

## コード上の境界

- `src/app/sidecar_restore.rs` の `begin_sidecar_restore` は、通常フォルダで編集またはタグの
  sidecar backupが有効なら、`mimageviewer.dat` の存在を調べる前にApp-global restore stateを
  `Quiescing` で開始する。
- `Quiescing` は保存中のsidecar/metadata writerとDB releaseを従来どおり待つ。廃止した
  `tag_legacy_seed_pending` は存在せず、初訪問の画像XMP走査を待たない。rating readerのcancelと
  ほかの保存保護barrierは維持する。
- 待機が100ms以内に終わればダイアログは描画しない。100msを超えるとphase別の見出しと説明を
  描画する。`Quiescing` は「保存中の設定を確定中」、`Checking` は
  「保存されている設定を確認中」、`Running` だけが「サイドカーから設定を復元中」となる。
  この100msは表示の点滅を抑える猶予であり、restore state、入力gate、worker pollを遅らせない。
- seed完了後のworkerはsidecar writerのstrict idleを確認し、`mimageviewer.dat` のmetadata/readと
  source再検証、要求familyの中央DB marker読取を行う。sidecarが無ければmarkerの有無を調べ、
  古いmarkerだけがあれば削除する。sidecarがありmarkerと一致しなければimportへ進む。
- フォルダを訪問済みというだけでrestore coordinatorを省略するcacheは無い。再訪が短い主な
  理由は、今回のログでは旧タグseedの300件が決定済みになったことにある。sidecarが存在する
  場合は、中央DB markerが同期済みになることも再訪を短くする。

## 確認できなかった点

該当sessionは成功時のprobe outcomeを通常ログへ記録せず、該当時刻のperf logも無かったため、
実際にsidecar importまたはmissing-marker clearを行ったかは確定できない。調査環境では現在
`G:` の対象フォルダを参照できず、`mimageviewer.dat` の有無も直接確認していない。

今回の証拠から確定できるのは、初回の約0.9秒が300件の旧タグseed完了に支配され、seed後の
sidecar flush/probeから入力解放までが約22msだったこと、固定見出しが実処理phaseを誤認させた
ことである。自動seedとその待機ownerを型ごと撤去し、100msの表示猶予や保存barrierは維持した。

## 旧XMPタグ自動seedの廃止（利用者了承・実装済み）

利用者は2026-09-12に、v1.0の旧XMP `#タグ`を今後自動救済しないことを了承した。
日常利用してきたデータは既に移行済みとの判断であり、旧データ救済という機能の削除も明示的な
仕様変更として承認済みである。

現行の利用者向け手動取り込み／取り込み後削除は2026-08-30のcommit `e63600147`で既に削除され、
`src/tag_legacy_xmp_worker.rs`も存在しない。このため手動機能の復活は行わない。

### 撤去した範囲

次の自動seed入口と、そのowner・待機・結果適用だけを一貫して撤去する。

- 通常の一覧／フォルダloadで `prewarm_grid_tags` から現在の実Image／Videoを渡す入口。
- prepared subfolder／aggregate loadが保持する `legacy_paths` と直接spawnする入口。
- 明示メタ情報importの終端refreshがcontextごとの `legacy_seed_paths` を収集し、cache反映後に
  再spawnする入口。
- `App`／`ViewerContextBundle`の `tag_legacy_seed_pending` owner、poll・cancel・repaint、
  metadata-transfer／sidecar-restoreのquiescence待機。
- `tag_legacy_seed_worker`と、自動seedだけが使うbounded `dc:subject` reader／判定helper。

入口を止めるだけのdead code化ではなく、context mount／退役、metadata import、sidecar restoreに
残るpending ownerを型ごと整理する。これにより初訪問で旧タグ移行を目的としたファイルごとの
XMP読取を行わず、sidecar確認が旧タグseed完了を待つこともなくなる。

### 維持する境界

- `tags.db`の既存タグと `tag_item_state` は削除・再構築しない。`tag_item_state`は通常編集、
  sidecar import、metadata importでも使う現役の決定台帳であり、空にしたタグが
  `mimageviewer.dat`から復活するのを防ぐ。
- `tag_item_state(source='xmp_legacy')`の既存行もそのまま保持する。自動seed廃止時にDB全件走査や
  source書換えは行わない。
- `mimageviewer.dat`のタグbackup／復元と、`tag_sidecar_backup_enabled`の現行契約を維持する。
- 起動時に旧TantivyのSTORED tagsを一度だけtags.dbへ移すmigrationは今回据え置く。完了meta後は
  即returnするため、今回観測したフォルダ初訪問I/Oの原因ではない。
- XMP rating、説明、生成情報等の一般XMP metadata読取／書込を維持する。外部JSON／TXT sidecarも
  mIV旧XMPタグとは別系統である。
- 通常のタグ付与／削除／全clearは引き続きtags.dbを正本とし、設定ON時だけ
  `mimageviewer.dat`へmirrorする。現行 `TagJobKind::ClearMiv` はXMP削除ではなくtags.db上の
  mIVタグclearなので残す。

### 受入条件

- 通常load、prepared subfolder load、metadata import終端refreshのいずれもlegacy seed workerを
  生成せず、初訪問で旧タグ移行を目的としたXMPファイル読取を行わない。
- sidecar restore／metadata transferは存在しないlegacy seed pendingを待たず、他のwriter・DB
  barrierは従来どおり維持する。
- 旧XMPにだけ存在する未移行 `#タグ`はtags.dbへ現れず、元ファイルも変更されない。
- 既存tags.dbタグ、空の決定state、sidecarからの復元、通常タグ操作とUndo／Redo、ratingを含む
  一般XMP処理が変わらない。
- contextのmount／退役とprocess exitにlegacy ownerが残らず、関連する通常／prepared／
  metadata-import／sidecar-quiescence回帰を更新する。

製品差分はこの条件を満たす形で実装した。旧XMPだけにある未移行タグを読む救済は、自動・手動とも
存在しない。既存DB行の移行や削除は行わず、一般XMP metadataとratingの現役経路は維持する。
