# v3.7.0 dupe 実機フィードバック対応

現在の状態: パネル待機中の幅予約、帯セルの長押し比較、類似専用履歴、音声診断を実装し、別 Sol による独立レビューと全体 gate（本体 7941 件）を完了。既存データを保持して portable-dev を更新済み。新しい版の実機確認と音声途切れの原因特定は未完了。以下は調査・実装経過。

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