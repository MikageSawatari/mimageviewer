# v3.8.0 類似検索の一時休止

2026-09-10、利用者が明示承認。動画プレビュー等の重要修正を先に配布するため、
類似検索を一時的に画面から隠し、バックグラウンド処理も開始しない版を作る。
全件照合の高速化・監視通知の差分更新化は後続であり、本対応で根本解決したとは扱わない。

## 背景

起動後20分以上類似索引の照合が続き、idle-healthを実施できないとの報告。
初回configureが全体走査を要求し、走査中のwatcher通知はrevisionを進めるため、
現行schedulerは一周後に再び全体照合する。案A/BのDB照合最適化だけで通知反復の問題は解消しない。
今回の休止は利用者が認めた機能制限であり、通常のバグ修正に紛れて機能を削るものではない。

利用者経由でClaudeCodeから引き継いだ同夜の実測（本タスクで再計測はしていない）:

- 完了記録19:09:50→22:04:12→22:09:00、一周の増加は70〜280件、CPU約2コア使用が継続。
- 類似照合は約5,000件/秒、名前索引は約25,000件/秒。similar.db約2.07GBへの小さな
  ランダム読みは毎秒約18,000回。両者の処理内容が異なるため、名前索引の速度を改善後の保証にしない。
- 「画像を確認中610,262/610,262」の分母は総数でなく発見済み件数で、残量を示していない。

後続の優先順は監視通知の差分更新化、一周の照合高速化、残量を誤認させない進捗表示。
本休止にはこれらの修正を混ぜず、実測と再現条件を後続の検証に再利用する。

## 範囲と不変条件

- お気に入り追加・編集の「別バージョン索引」設定を表示しない。
- 右の情報パネルの情報/類似タブ列を表示せず、通常の情報を表示する。
  main/F12で同じ表示方針を使い、旧similar選択状態でも空のパネルにしない。
- 保存済みauto_index_similar=trueでも、類似の起動走査・監視再照合・メモリ表ロード・
  照会・サムネイルprefill等を開始しない。設定OFFに伴う索引削除経路を休止のために流用しない。
- 設定値、similar.db、検索配列ファイルを休止のために削除/reset/false上書きしない。
- 通常のmetadata索引・そのwatcher・検索・サムネイル生成・AI・動画・表紙補助は維持する。
- UIと実行入口の許可条件は共通の方針を正本とし、復帰の判断箇所を明確にする。
- ユーザー向けに存在しない機能入口を新設しない。既存Remote入口の有無も確認する。

## 担当と検収

親は本書と設計判断、Sol/xhighの実装担当は製品コード・関連テストと必要な設計記録、
別のSol/xhighが重要設計と完成差分を独立検収する。公開用README/htdocs/versionはClaudeCode担当。
既存未コミット変更・稼働中アプリ・通常APPDATAデータは保持する。

開始前に入口・所有境界を確認し、未公開機能の既存enabled側テストは可能な限り維持する。
無効版では設定trueでも索引worker/DBロード/削除が発生しないこと、通常metadata索引の継続、
UIの非表示と通常情報表示を回帰確認する。focused→独立完成検収→全体gate・test-script check・
fmt/glyph→確認buildの順とし、sourceが有効な成功証拠は再利用する。
実アプリの起動・入力は別途明示了承が必要。通常確認buildは作成のみ、エージェントは起動しない。

## 実施状況

実装前inventoryを受領。製品capabilityをPausedとし、Appでは類似managerを生成しない方針。
これによりscheduler/executorとDB登録を作らず、configure/memory load/notifier/prefill/query/purgeを休止する。
共有IndexerManagerにはeffective similar flagを渡し、通常metadataの要否は従来どおり判定する。
既存enabled側回帰は明示capability注入で保持し、通常のPaused構成を別途検証する。
初回更新告知（version_highlights）の類似項目も非表示対象。README/htdocsは公開担当のまま。
親と独立Solが構造合意。追加条件はproductのprefill入口で直接拒否すること、
capability/notifierの不整合でsimilar専用supervisorを作らない共通effective resolver、
Pausedのterminal query/Idle/stale=false、同frameの通常情報表示とquery需要なし、
favorite削除でも休止を理由にpurgeしないこと。

独立レビューでAppのcapabilityとOptionの二重管理を指摘し、runtimeの正本をOption単独へ修正。
復帰時のEnabled文言・layoutを維持し、保存済みtrueと既存DB/baseを保持する回帰、
旧Similar選択から同frameでInfoを描くsnapshotも追加した。
実装担当からcapability 8件、paused App 2件、通常metadata連携1件の狭域成功報告を受領。
追加のprefill回帰はprocess-global登録を使わず、Paused時はtarget resolverを一度も呼ばないことを
直接検証する形にし、他のscheduler testsとの並列競合を解消した。
凍結差分の独立完成レビューはblocking findingなしで承認。狭域・snapshot・feature check・
fmt・glyph成功を受領。Infoのみ/no tabsのsnapshotも独立レビューで目視確認済み。
全体gate `scripts/test-full.ps1 -SuppressCrashDialogs` はexit 0 / PASS。
core libは8,084成功/43ignored、UI snapshot 48成功、vendor 25/9/15成功。
通常featureの `scripts/build-dev.ps1` も9/10 23:53 JSTに成功。既存アプリ停止・起動・UI入力は行っていない。

検証記録: `target/next-version-work/similar-feature-pause-v3.8.0-20260910/manifest.json`
（SHA256 `97ECD7A0FD1FCC8EE22DCBB82E077258C53740CDBA30F9502A3523BC3A8D7165`）。
対象source fingerprint: `c681392fd184a9adb240276448687061d4f796770e40eadf484ebb7203049b10`。
対象13ファイルと成果物2件のhashは親も照合し、不一致0。
core: `D435B61317372EB3500FC6E5E123377F8C8CEEE73EC840D3373E1CD6939E88B3`。
Remote: `0C97F18C3471A7542B43CEE7CF6A34C0BACF887AB990355DCDB7A3B49EAFEC27`。

開発側の休止修正・独立検収・非対話gate・確認buildは完了。追加実機は未実施。
公開担当は本対応を反映して更新履歴・マニュアルと配布工程を再開できる。
perf smoke / idle-healthなど既存公開ゲートは未免除。今回のbuildは配布用署名buildではない。
利用者が確認する場合は既存/トレイ常駐mIVを終了してrepo rootで
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常APPDATAを使い実設定/dataを更新し得る。類似タブ・お気に入り類似索引項目の非表示、
通常情報表示、類似索引の起動負荷が発生しないことを確認する。

根本修正はdupe側タスク「引き継ぎ状況を整理」へ利用者の明示依頼で委任済み。
`codex/similar-index-incremental-reconcile` で並行開発し、今回の休止版へ自動統合しない。
