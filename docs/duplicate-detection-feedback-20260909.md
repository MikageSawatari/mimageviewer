# v3.7.0 dupe 実機フィードバック対応

現在の状態: 前回のパネル・帯セル比較・履歴・音声診断は全体 gate（本体 7941 件）後に portable-dev へ反映済み。その版で [移動] が無反応との追加報告があり、原因は未特定。入力から表示完了までを追う移動診断と、本候補の常時フォルダ表示を追加した。再起動後の独立レビュー指摘を修正し、P1/P2 残件なし・新規 focused 14件・全体 gate（本体7958件）を確認。今回の診断版も既存データを初期化せず portable-dev に更新済み。今回の実機ログによりmain embedded fullscreenの非同期結果回収漏れを原因と特定。post-renderで同じmain所有のRequired要求を回収する修正を実装し、実App::update回帰5件と独立レビューを通過。今回修正版の全体gate（本体7963件）とportable更新も完了。利用者から移動の動作良好・右パネル固定も問題なしとの実機回答を受領。音声の原因確定、§9.2横断一覧、変更のコミット整理とmasterへの逆統合は別残件。以下は調査・実装経過。

2026-09-09。基準は a12380d64 と既存 dirty 層、v3.7.0 portable。前回のマージ・7929 件 gate・24 files 更新記録は [統合記録](duplicate-detection-merge-v370-20260909.md)。以下はその検証版への利用者回答であり、以前の「音声途切れ解消」を最新の受入結果として扱わない。

## 観測と範囲

- 類似移動で右パネルのロックが失われる現象は直ったように見える、との利用者確認。
- ロック中の Ctrl+上下で前後の本へ移動すると、一時的にパネルがないレイアウトで画像を表示し、後からパネルが現れて画像が横移動する場合がある。パネル予約幅の寿命を含め調査中。
- 長押し比較は動作確認済み。押した瞬間に候補の片側ページへ切り替わる現在の表示を、当面の仕様として利用者が承認。対応する類似ページが隣接するとは限らないため、自動で相手の隣頁を組み合わせる仕様には広げない。
- 音声途切れは残る。初回追加回答は類似タブを開く・本を移動した直後。再回答では同じ mIV での再生中、以前は Firefox/YouTube でも発生し、本移動以外でも起きるように感じる。原因は未確定。検索 wall time 短縮だけで音声への影響解消を断定しない。利用者は mIV の検出ログ追加を明示依頼。
- 「この本と重なる本」の帯セルは移動を行わず、押している間の比較へ変更する。移動は明示の移動ボタンが担当する。
- 類似経由で訪れた本への戻り導線を希望。初期本も含む専用履歴を類似タブの上部へ置く案を検討する。

## 分担と進行

親は設計・文書。Sol / xhigh の実装担当がパネル/帯の根因と既存経路を調査し、別の Sol / xhigh が音声途切れ候補を read-only で調査する。実装前に不変条件・対象差分・必要な検証を確定し、独立担当は設計自体もレビューする。コードの編集者は一人に固定。既存 staged R2 と unstaged R4/C/vendor/検証補助を保持し、旧計測の全面再実行はしない。

旧文書の ClaudeCode による設計/検収指定は、このタスクでは利用者指定の親/実装 Sol/独立 Sol へ移行する。detached §2 の構造修正条件は維持し、対象経路に触れる場合は着手前合意と §11 記録を行う。アプリの自動起動・入力は行わず、ユーザーデータを検証用に変更しない。

## 履歴の利用者承認済み仕様

viewer ごとの一時的な訪問済み本一覧。最初の本を含め、同じ本は重複させず、現在の本を表示する。各本へ最後に見ていたページの安定 identity で戻る。行の明示移動だけがナビゲーションを起こす。

同じ本のページ送りとタブ切替は保持し、通常の別本移動で比較 session を終了、真の閲覧終了で破棄する。再起動後は保存しない。履歴へ戻っても他の訪問済み本を削らない。移動失敗/取消/連続要求で未到達の本を到達済みにせず、別 viewer の履歴へ混入しない。既存 typed navigation owner の完了境界を検証してから構造を決める。利用者がこの仕様で今回実装することを明示承認済み。

## 音声診断の調査境界

既存 audio_diagnostics/audio_out に atomic underrun edge と silence 累積、pump 側 perf-log 集約がある。まずこれを活用し、再生所有の識別・短い複数 edge の集計・出力 callback 遅延・検索イベントとの相関の不足を確認する。callback 内へログ/JSON/追加ロックを持ち込まない。データ不足と OS/driver 側の実出力途切れは同一ではなく、無検出を可聴 glitch なしとは判定しない。

## 実装前調査: パネルと帯

実装担当が source で確認: Ctrl+上下/Required の async reopen は fullscreen_idx を空にするが、typed FsNavigationSequence と lock/holdover は保持する。一方 keep_fullscreen_viewport_alive / render_embedded_fs_nav_holdover / detached backstop の gap 描画は全幅で旧画像だけを描き、通常の metadata panel と予約幅を通らない。target 復帰後だけ通常 media rect と panel が戻ることが画像横移動を作る。親は共通 panel geometry owner を gap surface でも使う構造修正が必要と判断。独立 preflight と具体差分は後続。

帯の draw_page_strip は click から open_page を発火し、明示移動と同じ action を使う。既存 peek は QueryHit 用なので、帯の BookPageMatch が持つ target/mtime/size を同じ preview stamp と入力 lifecycle に接続する型を設計する。長押し状態の別コピーは作らない。
## 独立 preflight 合意と実装順

独立 Sol は P1 なし、構造方向を承認。以下の着手条件を親・実装担当も合意した。まずパネル/帯、次に履歴、音声診断を独立 chunk とし、最後の全体 gate と portable 更新はまとめる。

1. NavigationChrome は capture 済み rect や clamp 済み seek geometry を保持しない。意味入力だけを既存 sequence 内で保持し、現 full_rect/DPI と live panel lock/open から毎回共通 layout を解決する。active/keepalive/embedded/detached backstop の 4 入口を同じ answer にする。gap shell は同じ panel frame/tab/lock、item 不在の案内を使い、旧 item action・同期 I/O・query の origin 更新/withdraw を行わず Book Retained を維持する。
2. 帯と QueryHit は同じ typed preview candidate と既存 press lifecycle を使う。cell click/release は移動しない。明示移動だけが exact target navigation を行う。target 不在は比較を開始しない。
3. 履歴の完成状態だけを SimilarPanelState に置く。pending intent は physical scan では FolderOpenScanPurpose が所有し、成功時に FsNavigationSequence purpose へ move。ZIP/PDF/same-items は直接 sequence へ渡し、panel pending を別設しない。
4. 到達確定は generation と display-unit 全 Live の一致に加え、exact destination SnapshotTarget と実 GridItem の一致が必要。dedup は正規化 container identity、last page は別の SnapshotTarget。origin は移動受付時に捕捉する。同一 items 直行と continuous の bypass も同型化する。通常同本のページ更新も Live 確定時だけ、通常別本の移動成功で session を終了する。失敗/取消で未到達先を登録しない。

利用者の再生条件: 動画を画像とは別ウィンドウで再生。同じウィンドウで画像と動画/音声の同時再生はできない。VST はこのプロファイルで未設定、等倍再生、ノーマライズ ON、オーディオ IF 経由のスピーカー。動画 source/stream と画像 viewer の操作相関を記録し、既存 Normalize event を診断に使用する。


範囲補足: 帯セルの click 移動だけを長押し比較へ変える。単体画像候補の既存行 click 移動を無断で削除しない。類似専用履歴は同じ類似移動入口へ接続し、既存候補行経由でも戻り導線を欠落させない。


## パネル/帯 fixed1 完了

checkpoint: target/feedback-panel-strip-fixed1-20260909/manifest.json。製品 check 成功、deferred_navigation_ 2 件、帯 input 回帰 2 件成功、fmt/diff check 成功。帯 fixture の初回期待 item_key を訂正した対象再実行は成功（製品バグと区別）。独立 Sol が current rect/settings/live panel state での共通 geometry、target-free shell の Retained、strip の共通 preview gesture と click 非移動を承認、新規 P1/P2 なし。

staged R2 は raw diff 100861 bytes / SHA256 7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7f を保持。checkpoint の初回テキスト出力は PowerShell が CRLF 化した 103267 bytes だったため、raw 正本へ訂正。index は変更していない。履歴 chunk に着手。実機確認と最終 gate/portable は後続。


## 履歴実装途中の編集ミスと復旧

初回製品 check で app.rs の作業範囲外約 2000 行の脱落を検出。実装担当の置換操作の誤りであり、製品の実機不具合とは別。破損試行と失敗 log を保存し、panel 承認後の history-before raw へ app.rs だけを完全復元、exact anchor/count assert の小さい置換で履歴 7 hunk（+101/-0）を再適用した。予期しない欠落は解消し、check2 成功。R2 staged raw は 100861 bytes / 7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7f を維持。履歴の回帰テストは後続。


## 履歴終了理由の追加合意

実装前提の矛盾を検出: 既存 ViewerExited は実 close と ZIP/PDF/Required の取消・worker loss・missing を兼用していた。完成履歴を失敗時にも消すため、そのまま履歴 clear に使わない。独立 Sol と親は RequestFailed への内部 failure/cancel 再分類を承認。nav owner/query/panel/pending intent の従来終端は維持し、完成履歴だけを保持する。次の同 origin の all-Live は last-page を更新して継続、別 container の all-Live は履歴を終了する。

close_fullscreen_now は汎用 teardown でもあるため一律 ViewerExited にせず、実 viewer が開いており内部 reopen ではない true close だけで履歴を破棄する。失敗後 no-viewer cleanup が true close に昇格しない回帰を含める。独立 preflight は新規 P1/P2 なし。


履歴狭域: similar_navigation_tests 17/17 成功（physical success/failure、ZIP/PDF、password/cancel/supersede、RequestFailed〜true close）。physical success fixture が実 renderer poll 相当の Display bind を省いていた失敗は、production helper を通して訂正し成功。新 UI 代表 snapshot metadata_panel_similar_results_dark.png を親が目視し、暗色の初期本/現在行/長い ZIP 名と path が読めて重なりがなく、結果との区切りも確認。通常 snapshot 比較・check/fmt と独立レビューは後続。


## 履歴 fixed1 最終承認

checkpoint: target/feedback-history-fixed1-20260909/manifest.json、訂正後 SHA256 953f260960951b1a697a5a5fa87089feddf104212df3e3536a78767028fb9d2f。17 navigation / 2 state / snapshot 比較 / 製品 check / fmt / glyph が成功。独立 Sol は製品差分を承認、新規 P1/P2 なし。

証跡だけの P2: manifest が未収録 fmt-check2.log を clean と記載していた。実際は成功時出力なしで artifact を作っていなかったため、架空参照を除き実行記録と既知 fmt-check1 failure の解消理由へ訂正した。独立 Sol が訂正 SHA と内容を照合し P2 解消、最終承認。source や成功テストの再実行は行っていない。音声診断 chunk へ進行。


## 音声診断 fixed2 承認

6 file の限定実装（app.rs は before/after 同一）。RT callback 時計と atomic、pump 集約、CPAL error、stream/viewer/source/Normalize/VST bind、item/book start/end 相関。diagnostics 9 / audio 39 / stale item 判定 1 成功、製品 check/fmt/diff 成功。既存挙動・検索優先度・Normalize/VST 処理は変更しない。

独立 fixed1 review で成功前 stream ID 公開の P2 を発見。allocation と公開を分離し、build 後予約、play 成功後のみ Release publish へ訂正。失敗時は未接続 ID 0、App の成功 stream として公開しない。fixed2 の非空追加差分は audio_diagnostics/audio の 2 file のみ、diagnostics 9 件と最終 check 成功。独立 Sol は fixed2 を承認、新規 P1/P2 なし。manifest: target/feedback-audio-diag-fixed2-20260909/manifest.json、SHA256 fa56e3fc7c2fe6de9c3052254e8d9a8ecbd89e5631b2125b0d57a4df78d6961f。schema と限界は docs/video-architecture.md へ追記した。音声途切れの原因確定/解消は未完了、次の実機ログによる検証が必要。

## 最終 gate / portable と利用者への引き渡し

- source 基準 HEAD は a12380d6404a125a31fb37037883e16744670923 と今回を含む dirty 層。後続の文書専用コミットはバイナリ内容を変更しない。
- test-full.ps1 -SuppressCrashDialogs は PASS（本体 7941 成功 / 0 失敗 / 43 ignored、vendor egui/egui-wgpu/eframe も PASS）。ErrorMode は 0x00008001 へ復元。
- build-portable.ps1 -KeepRunning 成功、update-portable-dev.ps1 -SkipBuild 成功、Seed なし。アプリの起動・停止は行っていない。
- package 24 files は portable-dev と一致。exe は 93908992 bytes / SHA256 5c95e722635fea5da03c988d3f85a059f0417213e4850bb494947f809dcce10c。zip は 267348503 bytes / SHA256 c89f3e5f003798a7f58c5a269e7482c1a799ee0d09c0cb5b4a85fcb495dc18fe。
- data は 4231 entries / 3895 files、data-remote は 1 entry / 1 file。内容を read/hash/write せず metadata のみで前後一致を確認。update preflight 初稿は PowerShell 配列表記で data-remote を落としていたため保存記録として保持し、両方を含む portable-update-preflight2.json を正本にして更新前に確認した。
- R2 staged raw は 100861 bytes / 7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7f を維持。他の source 変更も未コミットで保持し、master 逆統合/push は行っていない。
- 最終 manifest: target/feedback-final-gate-20260909/final-manifest.json、47598 bytes、SHA256 e972f2d019444a9c1cf1ae5f19c1770ea2adb5413df05e0ac06855ae41139144。親も manifest/検証 exe の hash と gate PASS 記録を照合。

利用者がログ付きで起動する（通常 APPDATA の検証版へ切り替えない）:

```powershell
Start-Process -FilePath C:\home\mimageviewer-dupe\target\portable-dev\mimageviewer.exe -ArgumentList '--perf-log'
```

同じ portable 版を開いている場合は先に終了する。ログは `C:\home\mimageviewer-dupe\target\portable-dev\data\logs\perf_events.jsonl`。起動ごとに rotate されるため、再現後は通常終了して再起動前のログを解析する。

確認する操作:

1. 右パネルを固定し Ctrl+上下で本を移動。読み込み待ちでもパネル領域が消えず、後から画像が横へ移動しないか。
2. 重なる本の帯セルを押して比較し、離すと元へ戻るか。帯セルで移動せず明示移動は働くか。
3. 類似から数冊移動し、最初の本へ最後のページで戻れるか。同じ本が増えず、現在行が分かるか。通常別本へ移動すると履歴が終了するか。
4. 動画を別ウィンドウ、VST なし・等倍・Normalize ON、Audio IF 経由で再生しながら類似を開く/移動する。無操作時も含め、聞こえた途切れの概算時刻と直前操作を記録する。

音声診断は原因特定の材料を追加したもので、途切れ解消とはしていない。通常優先度の単体画像全走査の重なりが候補だが因果未確定。今回、検索精度・結果数・再生機能を減らす回避策や任意 sleep は追加していない。

## 最新実機再報告: 移動ボタンの回帰（2026-09-09）

利用者から更新版で移動ボタンを押しても移動しないとの報告。履歴導入との関係を含め調査・修正対象にする。実装担当 Sol は action から実移動経路、独立 Sol は前回回帰が省いた実操作の前提を分担して確認する。既存修正を保持し、履歴無効化で回避しない。親は稼働中 portable の既存診断ログを限定 read-only で参照した（起動/停止/入力なし、DB/索引の変更なし）。

利用者の画像で対象は「この本と重なる本」の帯の左上にある [移動]（open_page）と確定。履歴行のボタンではない。前回テストは移動 handler と lifecycle を直接呼び、実ボタンの press/release から action dispatch までは検証していなかったため、今回その経路を回帰対象にする。原因は調査中。

追加の切り分けでは、実 small_button の press/release → production dispatcher → 別物理フォルダの controlled scan → exact target Display/SimilarBookVisit が 1/1 PASS。さらに render_fullscreen_viewport の固定パネル・Similar タブを含む安定した外枠で、release 後の RequiredFullscreenTarget scan 受付も 1/1 PASS。前者の初回は fixture の enum 名誤り、後者の初回は --exact の filter 不一致による 0 tests であり、いずれも製品不具合ではない。正本ログは target/similar-move-regression-20260909/actual-button-before-fix2.log と outer-scene2.log。まだ利用者の失敗は再現できておらず、same-items の owner 拒否仮説を今回の別物理本移動の根因とは扱わない。

続報で、同名の本を __new と __new5 に持つ状態で、下の画像候補に明記された __new5 側の [移動] でも HUD が __new のままと判明。帯だけの問題ではない。両方の指定ファイルは Test-Path による存在確認のみで実在を確認（内容読込・変更なし）。full path key/target は両親フォルダを区別しており、同名・stale 表示・旧 Display owner を加えた回帰も PASS で症状未再現。通常ログには相手の load_folder はなく、入力/action/受付の相関がないため停止箇所は未特定。根拠のない挙動変更はせず、操作時だけの相関診断と、本候補の常時フォルダ表示を次の検証用変更とする。checkpoint: target/similar-move-regression-fixed1-20260909。

診断版の追加は opt-in の perf log category `similar_move`。操作ごとの相関 ID と source（本・画像ボタン・画像行・履歴）、型付き移動先の正規化 identity から生成した SHA256 先頭128bitの token を使い、press/release・action・route/受付・失敗/取消/表示完了を追跡する。パス本文やファイル内容を新規記録せず、無効時の hash/時計処理と毎フレームの記録は行わない。これは症状の修正ではなく停止境界を調べる追加観測で、既存の key または target が一致した最初の item を選ぶ挙動は維持する。`presented` はアプリ側の対象画像表示完了であり、ユーザーの HUD 確認と照合する。

音声は今回再現しなかったと利用者が報告。rustc 等とのリソース競合が必要かもしれないという仮説は未確認。解消とは扱わず、利用者が継続使用する。今回の追加作業は移動不具合に限定し、音声の再計測や追加実装は行わない。

## 再起動後の診断レビュー（2026-09-09）

再開時に pause checkpoint の6ファイルと HEAD、R2 staged raw（100861 bytes / 7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7f）の一致を確認。狭域3件・snapshot1件・fmt・glyph の完了記録は再利用し、記録不足だけを理由に再実行しない。

独立 Sol 最終レビューは P1 なし / P2 3件。今回診断だけを対象とし、旧調査・fixed1レビューを繰り返していない。

1. park / bundle Drop / materialization failure / generic sequence release で trace owner を捨てる際の terminal 欠落。既存の取消・状態遷移は維持し、所有者の退役 helper から診断を終了する。
2. source + target だけで press/release を対応させると、同じ候補の別 Response が release を取り違える。押下診断に egui::Id を含め、source・target token・Response ID の三者が一致する操作だけが引き取る。再レビューで、非同期の行更新では同じ auto ID が別 target へ再割当されるため ID だけに照合を置き換えてはいけないと指摘し、同じ修正範囲で補完する。
3. 同フレームの複数クリックで action slot を上書きする際、先行 trace が無終端になる。既存の last-wins を維持し、置換 helper で action_replaced を記録する。

これらは移動不能の原因確定ではなく、診断の完全性の修正。viewer_context_registry の対象は既存 park / Drop の診断補完に限定し、親と独立 Sol が症状パッチではないと合意、detached 計画 §11 に記録した。実装担当は修正と所有境界の回帰を行い、独立再レビュー後に最終 gate と portable build へ進む。
再レビュー追補: scan 完了を local FolderPaneOpenReady へ移した同フレームで別の folder pane 操作が優先されると、pending 側の置換 helper を通らず ready の trace が破棄される P2 も確認。既存の優先順位は維持し、その local owner の退役だけを補完する。繰り返し compile を避けるため、trace 付き purpose の全終了経路をまとめて確認してから最終 gate へ進む。

所有境界 sweep 追補: Display(RenditionFailed) は次の navigation を阻害しないため、新規 page/folder sequence と legacy holdover capture が旧 sequence を上書きする到達可能経路でも navigation_superseded 終端を補完する。previous の読み取り順・受付・非 page 分岐の挙動は維持する。独立 reviewer はそれ以外の pending/ready/context/virtual/password/presentation/holdover の trace owner 境界を一巡済みと報告し、以降はこの修正差分の再確認に限定する。

境界設計の追補: 非blocking な旧 Display は上書きだけでなく direct open / continuous seek / scroll reanchor による別 idx 着地でも終端が必要。親は all-Live 表示 observer への集約を比較提案したが、新 target の decode 失敗・native video/audio 遷移・park が先行すると observer が発火せず不十分と独立 reviewer が確認した。最終境界は共通 fullscreen-nav accept の block 判定後と、そこを通らない continuous seek / reanchor の3 setter。Display target の items_generation が現在世代と一致し、pages が受理先 idx を含む場合と FolderItems は保持し、別 target を受理する場合だけ optional 診断を navigation_superseded で終端する。navigation state/target の選択・受付は変更しない。この3入口と helper/回帰の範囲で親・実装・独立レビューが合意。

修正後結果: 新規 focused 回帰14件 PASS / 0 failed、fmt clean。独立 Sol は generation identity を含む最終 source の trace create/move/take/clone/drop/clear と3受理境界を確認し、P1/P2 残件なし・navigation 挙動変更なしと判断した。最終 source/14件の証跡照合後、今回差分の全体 gate と portable build/update を実施する（この時点では未完了）。

## 今回診断版の全体 gate

独立レビュー P1/P2 残件なし。最終 P2 対応 manifest SHA256 は 1f143c2fae2f80081c0c69b74b98cc62370d27ce4cef4279443e748165acc1c3（target/similar-move-diagnostics-review-p2-20260909）。test-full.ps1 -SuppressCrashDialogs は exit 0 / PASS。本体 7958 passed / 0 failed / 43 ignored（309.87秒）、vendor egui 25 / egui-wgpu 9 / eframe 15 も成功し、ErrorMode は 0x00008001 へ復元。実 stdout/stderr と終了結果は target/similar-move-diagnostics-final-gate-20260909 に保存。最初の wrapper は PowerShell 7 環境で powershell.exe のパスが存在せず script 開始前に失敗したため launch-attempt1 として区別し、実体 pwsh パスで再開した成功記録を正本とする。staged raw とレビュー後 source の一致も維持。portable build/update はこの gate 後に実施する。

## 診断版 portable 更新完了（2026-09-09 21:53 JST）

- build-portable.ps1 -KeepRunning は exit 0 / DONE（core release 23分59秒）。update-portable-dev.ps1 -SkipBuild は exit 0、Seed なし。
- package の data/data-remote は不存在、runtime 24 files は portable-dev と byte/SHA 一致、コピー対象の reparse はなし。対象 portable process 0 を更新前後に確認。他 checkout の稼働 mIV は操作していない。
- 既存 data/data-remote はコピー対象から除外し、両 root directory の存在/path/属性/UTC creation・last-write ticks が前後一致。今回はデータ内部の列挙や内容 hash は行っておらず、全内部ファイルの内容一致を検証したとは扱わない。
- staged raw は100861 bytes / 7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7fを保持。レビュー後 source も維持。runtime source は実機未確認のため未コミットのまま保持する。
- 正本: target/similar-move-diagnostics-final-gate-20260909/manifest.json、SHA256 9d77faa5b3fc21f75cc14ee2eed91f160d1bf975844d7fb9bae37af76e447627。親も manifest と起動先 exe の hash を照合した。旧報告の2a699...は記録追記前の hash。
- 起動先: C:\home\mimageviewer-dupe\target\portable-dev\mimageviewer.exe、SHA256 ca6300dc43ff3852d662a4be535134257cfe505584a53d19670cdca04b1ad15f。

利用者確認:

```powershell
Start-Process -FilePath C:\home\mimageviewer-dupe\target\portable-dev\mimageviewer.exe -ArgumentList '--perf-log'
```

本候補のタイトル下で __new / __new5 を識別できるか確認し、上の本候補と下の画像候補の [移動] を試す。HUDのパスが変わるかと、無反応だった概算時刻を確認する。ログは target/portable-dev/data/logs/perf_events.jsonl の similar_move。今回版は診断版であり、移動無反応を解消したとは報告しない。起動・入力・実機確認は利用者が行う。
## 実機ログによる移動不能の原因特定（2026-09-09、PID 58240）

利用者が診断 portable を起動し、上の本候補・下の画像候補の [移動] を複数回クリックしたログを read-only で分析した。今回は分析のみで source 編集・テスト・ビルド・アプリ操作は行っていない。

確定した観測:

- 起動後7.497〜9.528秒に book_button 9回、10.557〜12.321秒に item_button 9回。全18回で pointer_release.response_clicked=true → action_selected → dispatch → route_admitted physical_scan が成立。
- 全件 key_found=false / target_found=false / identity_mismatch=false / selected_current=false。existing item へ誤って着地する経路ではない。
- 全件同じ target token 229a56b400b0a3d6ff1b78c327063ab1。ソースと同じ型・length-framed UTF8正規化パスのSHA256先頭128bitをパス文字列だけから再計算し、__new5/266707/1.jpg と exact 一致。__new側は4f3d1f442d48ad519708166d140c667fで異なる。上下とも本当に別フォルダが移動先。
- seq1〜17は次クリック時scan_replaced、seq18は12.805秒の Escape → FsClose と同時に scan_cancelled。連打がworker未完了だった証拠ではなく、未回収の結果も置換対象になる。読み込み結果からDisplay sequenceを開始した記録はない。
- 抽出証跡: target/similar-move-analysis-20260909-pid58240/selected-events.jsonl。runtime log元ファイルは変更していない。

原因（親・実装Sol・独立Sol一致、P1）:

ROOT viewport FFFF の main embedded still fullscreen で要求はmainの folder_pane_open_pending に置かれる。しかし app.rs:71392 の描画前pumpは folder_nav/PDF/ZIP/fs lock のみで、71650〜71705 のembedded分岐がearly returnし、通常経路72264のpoll_folder_pane_openと72400のresolveへ到達しない。したがってworkerが完了していても結果を反映できない。71677付近のpending repaintにもこのownerが含まれず、完了待ちの更新継続も欠ける。これはクリック・移動先選択・自己一致・音声負荷の問題ではなく、実際の画面更新経路への非同期結果回収の接続漏れ。

既存検証の欠落:

実embeddedボタンテストは受付を確認して直ちにcancelし、完了回帰は poll_folder_pane_open / resolve_main_folder_open_ready を直接呼んでいた。実 App::update のearly-returnをまたいで完了結果を消費するcoverageがなかった。以前の全体gate成功はこの欠落を否定しない。

修正方針（未実装）:

main embeddedの共通遷移処理へ、同main所有のRequiredFullscreenTarget完了回収・反映を接続する。描画前に回収する案と、描画内close/page-navを優先させて描画後early-return直前で回収する案があり、接続位置は実装前の設計確認対象。通常folder paneの全purposeを一律に前倒しせず、keyboard/gamepad/close等の既存優先順位を維持する。pending中の更新継続も同じowner条件へ接続し、timer/retry/同期待ちの回避策にしない。回帰は実App updateのembedded経路を複数frame通し、受付→worker完了→対象item表示まで検証する。別context・通常folder pane・cancel/priorityとの分離も維持する。
## 原因特定後の修正着手（2026-09-09）

利用者が修正まで明示依頼。実装は Sol / xhigh、独立レビューは別 Sol / xhigh、親は設計と文書を担当する。対象は main embedded の RequiredFullscreenTarget 結果回収漏れ。post-render の fullscreen 入力・close処理後に同 owner だけを回収する案を軸に、通常 Pane/Grid の優先順位、別 context、Empty/完了/cancel、repaint 所有境界を実装前に照合する。回帰は従来の poll/resolve 直呼びではなく、実 App::update の embedded early-return 経路を複数フレーム通して接続を検証する。既存診断・履歴・本path表示、他 dirty/R2 staged を保持する。

接続位置の合意: 親・実装・独立 Sol は post-render を採用。render_fullscreen_viewport 内で close/page-nav の取消・置換を先に処理し、その後かつ embedded early-return の世代/repaint判定前に Required だけ poll/resolve する。pre-render では同フレーム入力より旧完了を先に反映し優先順位が逆転するため採用しない。Empty は既存 poll の repaint と同 owner の pending 条件で追跡する。

回帰設計の具体化: 初稿は actual App::update のfixtureでボタン描画前に失敗し、pump欠落の赤を示せなかった（製品fix未着手）。root・独立レビューは、既存の実Sceneボタン→受付テストを再利用し、新規はproduction移動handlerでtyped pendingを作ってcontrolled結果を渡し、actual App::update複数passで回収・着地する境界に限定することで合意。poll/resolve直呼びに戻さず、不要な画面fixture再構築の調査を広げない。

失敗先行成立: startup初回updateをsettleし実機と同じvideo_in_window設定へ揃えたfixtureで、production handler→controlled pending→actual App::update Empty passは要求保持・repaint。完了結果を送った次passでもpendingが残る期待外れを確認した。これを本命のbug-red記録とし、先行するbutton非描画/初期状態取消のfixture失敗とは区別。ここからpost-render Required-only回収の製品修正へ進む。

## 完了回収修正・独立レビュー完了

app.rs の製品変更は22行追加。post-renderのembedded early-return内からRequiredFullscreenTarget限定helperを呼び、既存poll/resolveに接続した。実App::update回帰5件（Emptyから対象leafへ着地、同frame Esc優先、page移動優先、通常Pane非消費、passive context非消費）が全PASS。既存ROOT Scene button admissionもPASS、fmt clean。独立Solは凍結sourceとR2 staged exactを照合しP1/P2なし。検証を重複実行せず、固定差分の設計・所有境界を確認した。今回修正版の最終全体gateへ進む。

## 完了回収修正版の全体gate

scripts/test-full.ps1 -SuppressCrashDialogs（CARGO_BUILD_JOBS=1）はexit 0 / PASS。本体7963 passed / 0 failed / 43 ignored、vendor egui25 / egui-wgpu9 / eframe15も成功。target/similar-move-embedded-pump-fix-20260909 に証跡保存。native出力はPowerShell transcriptだけでは完全に残らないため、toolから取得した実summaryと終了コードを補完し、ログ保存のためだけの全体テスト再実行は行わない。利用者のmIV終了報告後、親もportable-dev process 0を確認。build-portable.ps1 -KeepRunningへ進む。

## 完了回収修正版のportable更新

build-portable.ps1 -KeepRunningはexit 0 / DONE（core18分57秒）、update-portable-dev.ps1 -SkipBuildはSeedなしで更新。対象process 0、package 24files、package data/data-remote不存在、コピー対象reparseなしを更新前に確認。既存設定・索引はコピー対象外。親も更新先exe SHA256 3b426741d5a36532a6e8ea5cbd78adc115edbd2e4eab07dab4a3c5e7fb6674c6と成功ログを照合した。実機未確認のためruntime変更は未コミットで保持。

利用者確認: portable-dev/mimageviewer.exeを --perf-log で起動し、上の本候補と下の画像候補の[移動]それぞれでHUDが __new から __new5 に変わること、類似履歴から元の本へ戻れること、右パネル固定を確認する。エージェントによるアプリ起動・入力・停止は行っていない。音声途切れの解消は今回の修正では判定していない。

最終記録: target/similar-move-embedded-pump-fix-20260909/manifest.json（10730 bytes、SHA256 588e1edbc09a970bf90a30a99d1e62cc89689d550b428643ddd391e1b06c15a7、all_assertions_pass=true）。親もhashを照合。runtime24files一致、data/data-remote root metadata前後一致、R2 raw100861bytes/7e295682e3d4acd103242b2d1811ce7270bc3f71584dd6dc959db3c1d7110d7f保持。

## 利用者の受入回答と全体残件

利用者から「動作良さそうです。右パネルの固定も問題なさそうです」と回答。今回の移動不能・固定パネル修正の実機確認として記録する。個々の履歴操作や音声再現まで追加確認されたとは解釈しない。右パネルからの別バージョン発見・本の重なり・比較・移動という主要実装、独立レビュー、最終7963件gateとportable更新は完了。設計全体では§9.2横断一覧が未実装（現TopLevelGridSurfaceにもSimilarなし）。音声途切れの原因確定/解消は再現ログ待ち。旧テストの初回DB open競合は後続で原因特定・修正済みであり、未調査として一括復活させない。320→375秒の旧時間差の原因は確定記録なしだが、現構成の失敗とは区別する。未コミット変更の整理・保存、masterへの逆統合と統合後release検証も未実施。今回の状態確認ではsource変更・テスト再実行・コミット/統合を行っていない。

## v3.8.0へのリリース方針（2026-09-10 利用者指定）

§9.2横断一覧は保留。現状の右パネルによる比較を今回の提供範囲とする。別系統でv3.7.1向けの最終バグ修正が進行中で、その完了後、v3.7.1としての公開はスキップし、本機能を含むv3.8.0としてリリースする方針。完了連絡までは他系統の修正・version・release作業へ介入しない。統合時は最終バグ修正とdupe側の未コミット変更を保持して変更を整理し、共通機構への影響と競合を独立レビュー、統合後の全体gate・release向けビルド・必要な実機検証・マニュアル/リリース記述を確認する。既存の成功検証を無条件に繰り返さず、統合差分と正式版の条件に応じた検証を行う。音声途切れは未解決の観測事項として維持し、この方針を解消確認とは扱わない。公開やpushは今回行っていない。

## 索引更新中に別画像の旧結果が残る問題（2026-09-10）

通常版で類似索引を更新中、中央画像を `00表紙.jpg` へ切り替えても、固定した右パネルの「表示中（更新中）」に前の ChatGPT PNG とその候補が残る実機報告を受けた。保存済みログでは新しい `00表紙.jpg` の単体照会が `terminal=not_indexed / active=true / stale=false` として短時間で繰り返し完了しており、重い照会や古い worker の未完了が原因ではなかった。

原因は UI の完成結果保持が表示 slot の `Vec<Option<Ready>>` だったこと。現在の照会が `Preparing` なら、その slot に残る直前の `Ready` を origin の一致確認なしで「更新中」として投影していた。索引走査中に manager が現在 origin の cached `NotIndexed` を意図どおり `Preparing` として返す経路と組み合わさり、別画像の旧カード・候補・操作が現在画像の結果として残った。索引走査中の旧 Complete snapshot 利用と manager の `Running + NotIndexed -> Preparing` 投影は変更しない。

UI の保持所有者を `Ready` 内部の `origin.item_key` で識別する型へ置き換えた。現在の `page_key` と `Ready.origin` が一致した結果だけを保持し、毎 frame の現在 page key 全体で退役させてから key 検索で投影する。同じ origin の `Preparing` だけが「表示中（更新中）」を再利用し、別 origin は直ちに `Preparing` 表示へ移る。単体/見開きの切替、左右 slot の入替、見開き片側だけの変更にも同じ規則を使い、terminal は該当 origin だけを退役させる。同一 origin の結果を保持中も、`Preparing` 表示と同じ 100 ms の再描画を要求する。thumbnail/preview/book client の所有・cache は変更しない。

製品 helper の回帰として、A Ready -> B Preparing で A 非表示、A Ready -> A Preparing で同一 origin 更新表示、`[A,B] -> [A,C]` で右だけ退役、`[A,B] -> [B,A]` で key に追従、全 terminal で同 key だけ退役、更新中の repaint 要求、Ready 所有者の origin 不一致拒否を追加した。focused `similar_panel_tests` は 36 passed / 0 failed / 1 ignored。初回は egui の初期 settling repaint を消費しない test fixture の観測だけが失敗し、実 frame と同じ順で初期 pass 後の delayed repaint を観測するよう訂正した。製品 helper はこの訂正で変更していない。`cargo check`、fmt、glyph、既存の類似パネル states/results snapshot を含む `test-full.ps1 -SuppressCrashDialogs` は成功し、本体最大 suite は 8034 passed / 0 failed / 43 ignored、vendor egui / egui-wgpu / eframe も成功した。証跡は `target/next-version-work/similar-stale-panel-20260910/` に保存する。
