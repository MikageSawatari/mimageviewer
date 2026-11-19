# §1.686 外部変更に対するフォルダツリーの再照合

設計案 (2026-11-02、Codex)。v5 向け、ClaudeCode の採否・実装引き継ぎ用。
調査基点は `tree-1686` / `3a193e50df91faed2969dd752ed780270a2ae3b0`。
§1〜9は初回提案の調査記録。**現在の実装範囲は§9.5の(a)+(c)であり、(b)の監視機構は実装しない。** 実装・検証は§10へ記録する。製品起動とコミットは実装担当では行わない。

## 1. 原因候補との照合と設計上の追加条件

§1.686 の主原因候補に食い違いはない。次はソースで確認した事実であり、報告者の実行環境で原因を実証したという意味ではない。

| 確認点 | 基点のコードと結果 |
| --- | --- |
| 再利用の判定 | `folder_pane.rs::sync_to_active` → `ensure_scans_for_expanded` → `ensure_scan(Populate)`。`loaded` かつ `resolved_options == listing_options` なら終了する (285–342、588–635)。外部変更の鮮度条件はない |
| 折りたたみ・ドライブ・再表示 | `collapse_cursor` は展開集合だけを変え、`select_drive` も `Populate`。`set_folder_tree_pane_visible` / `sync_folder_pane_state` も既存 node を使う。非表示時は結果の poll も止まり、pending は直ちには捨てない |
| 一覧との非接続 | `app.rs::poll_current_folder_watch` → `check_folder_changes_with_cause` → rescan worker → `poll_external_rescan`。受理後に `folder_pane.nodes` を再確認する通知はない |
| 明示更新 | ツリーの ↻ と、ツリー表示時の `top_level_grid_view.rs::reload_top_level_grid` は `reload_for_active` に接続済み。これは全 node / 展開を reset する別の操作。F5 の既定は★5であり、今回キー割り当ては変えない |
| 現在の成功処理の限界 | `poll_pending` は親の `children` を交換し cursor を修復するが、消えた子孫の node / 展開 / pending は整理しない。`expanded_materialized_paths` は他ドライブや折りたたまれた祖先の配下も返す |
| 「成功」の追加条件 | `scan_real_subfolders_inner` は entry / file_type エラーを読み飛ばし、取消も break 後に `Ok(Vec)` を返す。`classify_dir_entry` は分類失敗を非 directory へ倒し得る。このまま `Ok` を消失の証明にしてはいけない |

報告者の「一覧からは消えるが左ツリーに残る」は、一覧とツリーの別キャッシュ、および鮮度を見ない再利用で説明できるコード欠陥候補である。特定版での退行、F5 を手動ツリー更新として実行したこと、↻ でも残ることは確認していない。報告手順・観測の正本はバックログ §1.686 に残す。

関連する前提: [ツリーソート設計](folder-tree-sort-plan.md)、[非同期設計の実Folder観測](async-architecture.md#実folderの自動観測1375-r10)、[UI 応答性 §4](ui-responsiveness.md#4-新機能追加時のチェックリスト)、CLAUDE.md「設計の簡素化」「まれな失敗」。

## 2. 先に検討した簡素化

1. **手動再読み込みを自動でも呼ぶ案は不採用**。既存 `reload_for_active` は全 node・手動/自動展開・明示collapseを消し、cursor と scroll を現在地へ戻す。日常の外部変更のたびにこれを行うと既存操作を劣化させる。明示更新としての挙動は保持する。
2. **`RefreshPreservingChildren` の worker と受理境界を再利用する案を採用**。通知の由来ごとに子を直接追加・削除しない。すべて「この親の直下を確認する」需要へ集約し、完全な走査結果の採用時だけ cache を確定する。一覧の並びや投影をツリーへコピーしない。
3. **自動更新中のモーダル化・paneを閉じ直す案は不採用**。短い受動走査のたびに通常の移動・展開・閲覧を止める必要はない。旧表示の保持、親単位の直列化、private Receiver の破棄で扱う組み合わせを限定できる。
4. **複数の更新機械は作らない**。options変更、外部通知、再展開・再表示は同じ node 所有の状態へ合流させる。watch は変化のヒントだけ、scan は結果だけを生成し、UI側の一か所だけが採用する。filesystem rename の追跡・rollback・変更journal・全ツリーtransaction・一覧とツリーの同時commitは不要。

## 3. 候補比較と推奨

`E` は選択ドライブの表示ツリーから到達できる展開parent数、`N(p)` は parent p の直下項目数。ここで「表示ツリー」はスクロール外も含む展開済みの行集合であり、ディスク上の全子孫ではない。

| 候補 | 費用と UI thread | 単独で取りこぼす場面 |
| --- | --- | --- |
| (a) 一覧で受理した実Folder観測をツリーへ渡す | 既存watch / rescanを利用。受理時にpath keyを渡すだけ。対象親だけ worker で `O(N(p))` 再列挙。一覧の Equal も通知対象 | 現在の一覧と違う展開枝、検索・ZIP/PDF表示中、一覧でdeferされた間、未監視の親。項目の上書き通知ではツリーに不要な確認も起こり得る |
| (b) 必要な枝だけ非再帰監視 | 選択rootと到達可能な展開parentだけ `O(E+1)` 購読。watch作成・登録・解除・最後のdropはworker。変化したparentだけ走査。UIは小さい通知の受理と描画のみ | 閉じた枝、非表示pane、他ドライブ、登録前の変更、OS通知欠落。多数展開時の購読資源は増える |
| (c) 再展開・再表示・ドライブ再選択/切替・main再表示時の再照合 | 入口の遷移で一度だけ需要作成。旧childrenを保ち、workerで必要parentだけ再取得。毎frameには出さない | 展開したまま静止した別枝の変更。単独では報告者が一覧だけを使う間の自動反映を満たせない |

**推奨は (a)+(b)+(c) を一つの再照合経路に接続する案。** (a)+(c) だけでは、現在地と別の展開枝を開いたまま外部変更したとき古い表示が残る。(b) だけでは閉じたcacheの再利用と登録前の隙間を閉じられない。(a) は報告された親一覧と確実に接続する小さな補強であり、(b) と同じpathを通知してもnode側で合流する。

ドライブ全体の再帰監視、全nodeの周期検査、毎frameの走査、一定時間経過だけでのcache全消去は採用しない。通知が正常に届く静止時は周期scanも周期repaintも発生させない。OS通知欠落までの完全保証はせず、(c)と既存の明示更新を再取得入口とする。

## 4. 所有と producer / consumer

| owner | 所有するもの / producer | consumer と権限 |
| --- | --- | --- |
| `App::folder_pane` (main chrome) | 正規化pathで引くnode cache、表示drive、active表示、操作cursor、3種の展開集合、親ごとの走査状態、pane監視session | ツリー描画/操作が読む。結果採用・子孫整理はこのownerだけ。viewerのitemsや他窓のcacheへ書かない |
| nodeの走査状態 | 要求path、現在の `FolderPaneListingOptions`、適用方針、cancel、**要求ごとにprivateなReceiver**。既存 `folder-pane-scan` workerが直下の結果を生成 | paneの採用処理。取消はtokenとReceiver dropで行い、旧channelを新node/同pathの新要求へ再利用しない |
| pane監視session (新設、pane内所有) | workerが非再帰watchを所有。callbackは登録時のparent keyと登録ticketを付けたdirtyヒントだけ送る | paneが現在session・登録ticket・需要集合を照合して走査要求へ変換。callbackからnodeを変更しない |
| 一覧の `CurrentFolderWatch` / `ExternalRescanPending` | **ViewerContextBundle所有**。mainのwatch epoch、dirty/cutoff、観測record、generation、source、optionsを既存worker/受理処理が扱う | `poll_external_rescan` の既存owner検証と `accepted_now` が受理を決める。paneへ渡すのは受理後の実親pathだけ。watch / itemsの所有をpaneへ移さない |
| `folder_pane_open_pending` | ツリーの行を開く能動操作。名前は似ているがnode列挙とは別のviewer-context所有owner | 既存navigation採用処理。自動refreshはこれを作成・取消・再開しない |
| main の Shell 転送の完了 (§1.464) | 整理先とツリーへのドロップが共有する `FileOrganizeRequest::Running` の確定要求 | `poll_file_organize` が先のフォルダと各元の親へ `observe_accepted_folder(path)` を渡す。path 自身の直下を再確認し、viewer context の状態は変更しない |

一覧→ツリーの橋は `poll_current_folder_watch` の生通知には置かず、**`poll_external_rescan` の `accepted_now == true` の直後**に置く。既存のowner/source/generation/projection/subscription検証を通過し、要求元contextがmainであることを明示する。`Different` のdefer/拒否/失敗は通知しない。`Equal` も、一覧のfilter/投影とツリーのdirectory集合は異なるため確認のヒントを送る。対応するツリー親は列挙した `folder` 自身であり、常に `folder.parent()` を更新するという意味ではない。

mainがpark中にそのownerの受理が行われても、捕捉したmain identityとpathだけで通知し、投影中の別contextの `effective_folder()` を取り直さない。pane側は未作成nodeを増やさず、既存cacheを未確認へするだけ。現在需要がある親だけ再取得する。別窓の能動Operation完了や独立readerのopen/closeをこの橋の入口にしない。

`FolderPaneState` はbundle swap対象でなく、`current_folder_watch` 等は `viewer_context_registry.rs` のswap対象である。この区別を維持する。pane同期・監視需要計算・active表示の更新は既存main chrome境界で行い、全contextのmounted serviceへ追加しない。独立readerの受動購読を復活させず、mainの購読を流用/取消しない。detached述語やviewportの変更は本案の範囲外であり、必要になったら実装前にリワーク計画§2/§11の合意へ戻す。

## 5. nodeの状態と単一の採用境界

§1.464 の転送完了も既存の `observe_accepted_folder` を使う。新しい node や走査 owner を作らず、現在需要がある親だけを再取得する。通常一覧の `Notified` は従来どおり現在地が元の親・先のときだけ一度要求する。

`loaded` / `loading` / `resolved_options` と別のpending配列に、さらに `dirty: bool` を足す方式は避ける。今回触るnode列挙の所有だけを次のtyped stateへ集約する (名称は実装時に調整可)。

- 成功cacheは `CachedListing` とし、表示順children、成功時options、**非表示も含む実directory直下membership**を持つ。未取得はcacheなし。表示用rowsのloading/loadedは以下の状態とcacheから導出する。
- 走査状態は `Unverified` / `Queued(request)` / `Running(private pending, FollowUp)` / `Settled(attempted options, outcome)` の排他enum。`FollowUp` は `None` / `Recheck(latest request)` の排他enum。失敗した試行と最後の成功cacheを混同しない。
- `Queued` への重複要求は最新options/適用方針へ合流する。`Running` 中の新たな外部dirtyは `Recheck` 一件に合流し、同pathのworkerを次々spawnしない。**Recheckがある旧完了は成功でも採用せず**、終端後に最新要求を一回だけ実行する。継続する実変更は収束まで次の観測を必要とするが、失敗自体をretry理由にしない。
- options変更やscope離脱・明示reloadは既存同様cancelし、Receiverをdropする。新要求は別Receiver。path一致だけで受理しない。scanの相関のために重複する数値世代を追加しない。監視の登録ticketはcallbackの寿命用であり、scan世代の代用品ではない。
- `Settled(Err)` は同options・同表示機会の毎frameで再投入しない。新しい実変更通知、次の再展開/再表示、options変更、明示更新だけが次の取得入口になる。

### 5.1 需要と監視範囲

1. 選択rootと、rootから表示childrenを辿って到達できる展開parentを需要集合とする。**空childrenでも展開中なら含める**。別drive、collapse祖先の配下、成功で消失した枝は含めない。既存 `expanded_materialized_paths` の集合unionをそのまま監視対象にしない。
2. 現在地への初回祖先展開は保つ。未走査の祖先は従来同様に並行して先行取得できるが、既知の親の成功membershipに含まれないchildで祖先revealを止める。`sync_to_active` が毎frame、消失確定済みのactive祖先を再生成/再走査しない。active pathの記録自体は保持する。
3. 需要へ新規参加した親は、既存cacheが同optionsでも未確認として一度 `RefreshPreservingChildren` (cacheなしは `Populate`) を要求する。監視登録はworkerで行い、**登録成功後に初回確認scan**を出す。登録失敗時も一度の確認scanは行い、以後はエラーと手動入口で閉じる。
4. 需要を外れたnodeのcacheと展開意図は保持し、走査はcancel/Receiver drop、購読はworkerへ解除依頼。鮮度は `Unverified` にする。再参加時は必ず確認する。collapse・drive往復・pane再表示・mainの再表示/復帰はこの入口を共用する。毎frameの `sync_to_active` は遷移がなければ何もしない。
5. **以前emptyと分かった非監視のcollapsed leaf**は、cacheがあっても未確認なので `has_children_or_unknown` をtrueとして再展開可能にする。現在の「loaded + emptyならarrowなし」をそのまま再利用しない。未知を示す既存arrowを使い、新操作・新キーは追加しない。展開して完全成功のemptyなら通常どおり子なし表示になる。

監視sessionはmainのpane表示寿命に属し、非表示/mainを表示しない期間・drive切替では旧sessionを退役させる。再表示は新しいprivate通知channelと登録ticketを使う。rootと到達parentはすべてNonRecursive。junction/symlinkを自動で再帰追跡せず、現在と同じ字句path identity / 深度上限を保つ。

watchの作成/登録/解除/dropは専用管理workerに閉じる。UIは最新の需要snapshotを非blockingで送る。command channel満杯時は、同sessionの未送信需要を最新一件へ置き換えて後続passで送る (UIで待たない)。**未送信需要/Queued scanが残る間はownerが各passで短い `request_repaint_after` を再要求**し、callbackや次の利用者入力を待たない。workerは差分だけ適用し、登録完了も送信後にrootを起こす。追加登録には新ticketを使い、解除後や同path再登録前のcallbackは無効。古いsnapshotを一時処理しても、現在ticketに一致しない登録完了・dirtyは採用しない。session終了は通常commandの満杯に依存せず、取消tokenとchannel切断でworkerが解除し、UIはjoinしない。

停止・再開の呼出し責任もmain rootへ固定する。`src/app/logic_service.rs::service_app_logic` のtray/lifecycle checkpoint後に、paneの**退役と非blocking送信だけ**のserviceを置く。ここでは設定の非表示、既存 `window_visible == false`、ROOTのminimized、終了要求を確認してsessionを即時退役させ、activeの再解決・viewer mount・結果の表示採用はしない。logic-only passでも通るため、描画が省略されたトレイ状態で旧購読が残らない。

fullscreen/native等でmain chromeを描かない可視UI passは、`App::ui` 外側のbegin/endで扱う。pane session ownerにそのpassの表示leaseを渡し、`render_folder_pane` が実際にmain chromeを描く境界でだけ更新する。`update_frame` の早期returnにかかわらず到達する `App::ui` 終端で、未更新leaseを退役する。leaseは同じpane session enum内の状態で表し、別の「表示した」boolをAppへ並置しない。表示されたrootの境界だけでmain activeから需要を作成し、dirty回収→結果採用を行う。非mainを一時mountしているserviceからleaseを更新しない。復帰は新leaseで一度再照合する。新しいdetached/fullscreen述語を作らず、既存main chromeの実描画有無とROOTの既存可視状態を使う。

callbackのbounded通知はparent単位で合流し、Accessだけの通知は除く。それ以外のcreate/remove/rename/metadataはdirectory種別をUIでstatせず確認のヒントとする。renameの旧新pathから直接nodeを書き換えない。自己削除/rename通知は既存cache内の字句上の親も確認対象とし、子自身のread失敗だけで消失を決めない。両親が開いていれば別親移動は双方で観測し、閉じた移動先は再展開時に取得する。

**同pathの実体入れ替えも再確認する。** 旧 `p/child` の移動直後に別directoryが同名で入ると、親のchildrenに差がなくても旧childのchildren/watchは古くなる。Create/Remove/Name系のヒントには、捕捉済みparentとeventのaffected pathをboundedで保持し、正規化したresident node keyへメモリ内で照合する。一致したnodeとmaterialize済み子孫はcacheを消さず未確認へし、旧走査をcancel/drop、現在需要の購読をfresh ticketで再登録→確認する。collapsed子孫は次の展開で取得する。eventは実消失の証明やrename先への状態移送には使わない。affected pathを特定できない構造変更ヒントは、そのparent直下の既存directory枝を同じ再確認へ合流する。単なるファイル上書きでは全子孫を再登録しない。生通知が失われた入れ替えは検出保証せず、通常の再表示/手動再取得で閉じる。

UIは監視dirtyを回収してからscan完了をpollする。通知合流の短いdebounceはI/O頻度だけを抑え、stale判定の根拠にしない。deadlineは到達まで各passで `request_repaint_after` を再要求する。scanは送信後に `request_repaint`、購読が安定したidle中はtimerなし。overflowは現需要parentを一度未確認にしてログへ記録し、通知履歴の再構成や常時の補完巡回はしない。

### 5.2 完全成功・整理・失敗

scan workerは `read_dir` 完走、各entry/file_typeおよびdirectory分類成功を満たした結果だけを成功にする。分類には既存 `try_classify_dir_entry` を使う。取消は部分成功にせず取消終端とし、送信直前もcancelを見る。metadataでdirectoryか判断できない場合も、非directoryへ倒して消失扱いにしない。

実directory membershipはhiddenフィルタ**前**に作る。表示childrenの差だけでは、隠れた枝と実際に消えた枝を区別できない。属性取得失敗は隠れたという根拠にせず、既存同様表示側へ倒す。日付取得失敗の既存mtime=0のソート方針は維持でき、directoryの不在証明とは分ける。internal-app-entryの除外は維持する。共通fs_entry helperの全consumerの挙動は変更しない。

採用する順序は、(1) 現owner/private pending・要求options・後続Recheckなし・現在需要を検証、(2) 完全成功cacheを親単位で交換、(3) 新membershipから実消失が確認された直下childの子孫だけnode/展開/走査/購読を退役、(4) cursorとscrollを修復、(5) 新しい到達需要を計算、で固定する。同frameで親と子の完了があっても親の整理を先に反映するか、各採用時に到達資格を再検査し、子の遅着で復活しないようにする。

パスの子孫判定は既存正規化とpath component境界で行い、単純な文字列prefix (aaaとaaa2) で消さない。rename/moveは旧pathの消失と新pathの追加として扱い、ファイルID等でcursor/展開/メタ情報を勝手に新pathへ移さない。いったん成功で消失した同pathが再作成された場合、旧子孫cacheの復活ではなく新placeholderから取得する。

hidden等で表示対象から外れただけの実在childは、cache/展開意図を保持しつつ現在需要を外し、再表示時に確認する。実消失と同じ破棄をしない。親そのものが読めない、選択driveが一時到達不能、worker生成失敗、channel切断では、旧childrenを残して既存rowのエラー表示とログを使う。読めないことを空directory成功やdrive全消去へ変換しない。

**まれな失敗は割り切り (ログ + 既存のエラー表示/通知 + 手動再取得)。** watch登録/継続エラーも同じ表示機会でretry loopにしない。監視errorはscan成功で黙って消さず、既存の警告表現へ投影する。回復用の多段状態、指数backoff、再登録巡回、journal replay、保存データの削除/再構築は作らない。既知の読み取り失敗を繰り返させる自動循環も作らない。新しい実通知や再表示での通常再照合と、失敗による自動retryは区別する。

## 6. lifecycle

| 遷移 / producer | node / scan / watch の扱い | 一覧・他窓への影響 |
| --- | --- | --- |
| open: 初回pane表示、設定復元で表示 | mainのactiveから既存祖先展開を作り、現需要へ登録→確認scan。旧cacheがあれば先に描画 | 既存の表示時focus規則だけ。自動Openなし |
| switch: 現在地変更 | 既存のauto展開/明示collapse解除規則を維持し、追加/離脱需要だけ処理。手動展開の別枝も到達可能なら維持 | 現在地の決定は既存navigationだけ |
| switch: drive同一再選択/別drive/戻る | 明示のcursor/root/scroll規則を維持。同一drive再選択も確認入口。旧driveのcacheは保持、購読/走査を退役。戻ったら再確認 | 一覧を開き直さない |
| expand / collapse (mouse・既存key・自動祖先) | 参加/離脱で同じ需要遷移。隠れた子孫の展開意図は保持。成功で到達した新parentも登録→確認 | 同一処理へroutingし片方だけforceにしない |
| hide / main非表示・復帰 | cache/cursor/展開を残し旧sessionとscanをcancel/drop。復帰時は新sessionで確認。単なるfocus往復の毎frameイベントにしない | 既存トグルによる「移動先cursorを開いて閉じる」は能動操作として維持 |
| manual reload (↻ / GridReload) | 従来どおりreset。先にsession/pendingを退役し、再構築後に登録→走査。自動refreshと型で区別 | GridReload本来の一覧更新・sidecar方針は不変 |
| options変更 (tree sort / hidden) | 旧scanをcancel/drop、新optionsで既存preserve-children経路。watch自体は同じpathなら再作成不要。非表示枝は次の参加で取得 | 一覧sort/詳細sortと混ぜず、現在フォルダを開かない |
| dirty during scan / old completion | 同pathのFollowUp一件に合流。旧完了は非採用。scope離脱済みReceiver/登録ticketは到達不能 | generationや他contextのqueueを進めない |
| parent success proves removal | 消えたpath配下だけ退役。activeの再同期も既知の不在を尊重。cursor修復は一度 | active locationや他窓の表示を変更しない |
| error / cancel / App close | errorは旧cache+通知で終端。cancel/closeはtoken+private Receiver破棄、OS handleはworkerでdrop、UIでjoinなし | main一覧watchや兄弟viewerを一括cancelしない |
| viewer fork / park / mount / independent close | paneはmain chrome ownerに据え置く。能動openと受動列挙を別ownerのまま維持 | main受理だけ橋渡し。独立reader受動購読の停止とlinked動作を保持 |

## 7. 現在地・操作cursor・展開・scroll

- **現在地**: `active_path` はmainの採用済み現在地を表示するだけ。消失して行がなくなってもツリーrefreshは `load_folder` / `FolderPaneCommand::Open` / 履歴変更を起こさない。既存一覧のエラー・復帰判断へ委ねる。
- **操作cursor**: path identityで保持。sort変更や取得開始・失敗でactiveへ戻さない。成功でcursorを含む実消失枝が確定した場合、既存 `repair_cursor_after_successful_refresh` と同じ可視active、なければ選択rootへ戻し、既存scroll-to-cursorを一度だけ要求する。hiddenで行が消える場合も可視cursorへの修復だけ行い、cache/展開破棄とは分ける。
- **展開**: 同pathの追加・並べ替えは `user_expanded` / `auto_expanded` / `user_collapsed` を保持。折りたたみ中の子孫を勝手に再展開しない。実消失が確定した子孫だけ各集合から除去する。明示reload/drive操作/active変更に元からある規則は変えない。
- **scroll**: 同じ `folder_pane_tree_scroll` のIDを保ち、refresh開始/失敗時は位置を変えない。自動成功時は交換直前の最上段可視pathと行内offsetをanchorにして、残存時は同位置へ復元する。anchor消失時は近い残存行/可視祖先へ寄せ、末尾のclampを許す。明示cursor移動や上記cursor修復のscroll要求を優先する。実際の行allocation高と縦item spacingから記録したpitchを用いるメモリ内計算でありFS照会しない。指定label高24ptはButton/textの最小寸法で拡張され得るため、pitchには使わない。drive切替や表示時の既存focus移動は自動refreshによるscroll保持とは別の能動規則のままにする。

## 8. テスト計画と受入条件

今回は以下を**計画しただけ**で未実行。実装担当が検証を所有し、独立reviewは同じsource/条件の有効な結果を再利用する。実際のOS通知の順序をunitのsleep頼みにせず、fake watch登録/dirtyとprivate scan senderを注入して順序を決定的に検査する。

| 検査 | 構成と期待結果 |
| --- | --- |
| 追加・削除・改名・移動 | 同一親内rename/移動相当 (旧名→新名)、別親A→B、同名再作成。完全成功で両親childrenが整い、旧子孫を再利用しない。親scanが一度も不在を観測しない同path入れ替えでも、affected nodeの旧watch/走査は退役し、新実体のchildrenへ収束する。a と b それぞれ単独の入口、重複通知も検査 |
| 報告経路 | tempの `data/aaa` を両cacheへ入れ、main一覧が `data` の削除観測を受理→pane確認。ツリーを触らずaaaが消える。実FolderのEqual受理も確認需要を作る |
| 別の展開枝 | 現在地Aと展開B。Bの変更だけでBが更新。スクロール外の展開parent、expanded emptyへの最初の子追加も含む |
| 折りたたみ・empty leaf | loadedの同optionsでもcollapse→変更→expandで再取得。以前emptyのcollapsed leafをmouseで再展開可能。親collapseの子孫watch/scan停止、再参加後の確認を検査 |
| drive / pane / main再表示 | D→C→D、Dの同一再選択、pane hide→show、main非表示→復帰。通知のない間の変更を確認し、旧sessionの登録完了/dirty/scanを拒否。logic-onlyのtray/minimize、native/fullscreen early returnでpaneを描かないUI passも旧session退役。command満杯/Queuedが最後の入力passに残っても、自分のwakeだけで最新需要へ進む。毎frame syncだけでは走査増加ゼロ |
| 走査の遅着と通常の連続変更 | 結果送信済み→dirty→poll、running中dirty複数回、options A→B→C、cancel直前送信、親削除結果と子結果の到着順両方。同path新nodeを旧結果で上書きせず、FollowUpは一件、成功まで旧表示保持 |
| 消失と現在地 | cursor自身/祖先が消える、activeだけ消える、cursorとactiveが別。完全成功時だけcursor修復し、Open/履歴/一覧generation変更ゼロ。次frameのactive同期でも消えた枝が復活しない |
| 隠し属性・sort | tree全6sortとtie-break、hidden ON/OFF、Hidden+System、internal entry、junction/symlink。実在hidden枝の展開cacheは破棄せず、可視除外と実消失を区別。一覧sort/詳細表示とは独立 |
| 失敗・取消 | read_dir失敗、途中entry/file_type/分類失敗、spawn失敗、channel切断、途中cancelは空成功にしない。古いchildren/cursor/scroll維持。失敗後の同条件syncを反復して自動retryゼロ。watch登録失敗・overflowも一段の通知/確認で閉じる |
| スクロール・描画 | headlessの実paneで追加/削除/降順変更のanchor保持、anchor消失のclamp、cursor scroll優先、幅180pt、loading/error/unknown empty arrow、light/dark。変更する描画についてsnapshotを追加/更新 |
| 一覧の回帰 | `section375_tests` 等のEqualでitems/thumbnail要求を交換しないこと、Differentのdefer/restart/dirty cutoff、明示更新だけのsidecar取込を保持。tree通知から一覧へ戻る循環ゼロ |
| 多窓 | registry fixtureでmain/独立readerが同pathの場合も検査。fork/open/close/read-only mountはpaneを失効させず、pane確認は兄弟のitems/cache/texture/queue/channel/generation/cancel/watchを変えない。mainのpark中受理と別context投影中の非main受理を区別 |

### UI threadで同期I/Oしないことの確認

1. 変更する入口 (main update/chrome、expand/drive/show/options、一覧受理) からcall graphを追い、`read_dir` / metadata / exists / canonicalize / watcher作成・watch/unwatch/drop / join / blocking recvへUIから到達しないことを独立reviewで確認する。既存 `check_folder_changes_with_cause` の親metadataなど残存同期I/Oは今回追加したものと区別し、この既存経路をtree更新のために呼ばない。
2. scanとwatch backendをfakeにし、呼ばれたthread IDを記録する。登録/read/解除をworker側で意図的に停止してもheadless UIの別入力・描画passが進み、cancel/hide/drive変更が即座に戻ることをbarrierで検査する。製品UIを起動しない。UI内にsleep/try_lock待機を入れない。
3. `folder_pane / scan_subfolders` の既存計測へenqueue/採用/破棄理由、watch登録/解除/dirty件数とscope、UI採用所要時間を必要最小限追加する。perf有効時だけextrasを構築。重い列挙・比較・sortと旧大payloadのdropはworker側へ寄せる。UI採用が大量siblings/削除subtreeで重い場合は、一parentの可視cache交換を確定点に保ち、後始末をframe予算つきで進める。整理未完のnodeを需要へ戻さない。
4. 必要親を数百展開した合成fixtureと、大量直下directory・遅いI/Oのfixtureで、無変更frameに新規scan/watch登録がないことを数える。開始要求/結果採用は一frameの件数予算を設け、一度にE個のthread生成・全snapshot cloneをしない。購読数はroot+到達展開parent数、scanはdirty/参加parentだけであることを確認し、上限で枝を黙って更新対象から外さない。
5. 実装後のリリース前にのみ、承認済みdisposable portableで報告経路とidle/perfを検証する。具体suite/所要時間/desktop入力/一時data範囲を示して事前承認を得る。normal profileをagentが起動しない。実機のhitch/idle結果はheadlessの結果とは分けて記録する。

実装時の順序はnode状態の焦点テスト (`cargo test -p mimageviewer --lib folder_pane`)、一覧橋/registry/手動更新の焦点テスト、UI snapshot、通常core check、fmt/UI glyph検査、共有非同期境界を触る最終 `scripts/test-full.ps1` とする。本体lib全件を追加する場合は `scripts/test-lib-full.ps1` を使い、成功台帳を再利用する。実行command/対象source/features/件数/exit/logを記録する。実装完了時はgreenの上で `scripts/build-dev.ps1` による利用者用buildが必要だが、この設計だけの作業には不要。

## 9. 実装範囲・引き継ぎ・未決事項

- 主な変更予定: `folder_pane.rs` (node状態/共通需要/受理/整理、完全scan)、`ui_folder_pane.rs` (表示寿命/empty arrow/scroll anchor)、`app/logic_service.rs` (root非表示退役と送信進行)、`app.rs` の外側 `App::ui` 表示leaseと `poll_external_rescan` のmain受理通知。監視backendは小さい専用moduleへ分離可。`reload_top_level_grid` の手動reset契約は保持する。
- 実装に合わせ `folder-tree-sort-plan.md` の既存ownership記述、`async-architecture.md` のworker/channel/cancel一覧、必要な仕様/マニュアルを更新する。ソート・隠し属性・手動更新・詳細一覧・ZIP/PDFを実Folderと誤認しないことを回帰する。内部D&D、★/タグのpath追従、キー割り当て、一覧自動更新の仕様拡張は含めない。
- 段階化するなら、(1) 完全scanと単一node owner/成功整理/再参加の基盤、(2) main受理通知と限定watch接続、(3) UI保持と横断検証、を一つの採択設計の内部checkpointとする。(1)だけで外部変更対応が完成したとは扱わない。担当fileの一writer、独立review、検証担当をClaudeCodeが割り当てる。
- **利用者への追加質問は現時点では不要。** フォルダを勝手に開かず、既存機能を減らさず、限定監視と非同期再照合で要件を満たす推奨である。rename先へのcursor自動追従は今回要求していないので採用しない。
- 設計担当の採否が必要な項目は、この (a)+(b)+(c) の範囲と、nodeの排他状態への集約。実装調整事項はwatch worker/module配置、frame当たりの受付/採用予算、既存retirement経路との接続。実測前に性能改善や最大反映時間を約束しない。資源制限のため監視対象を減らす、通常操作を待たせる等の仕様変更が必要と判明したら、実装せず費用と選択肢を設計担当/利用者へ戻す。

## 9.5 設計担当の採否 (2026-11-02、ClaudeCode)

- **採用: (a) 一覧で受理した実フォルダ観測のツリー親への通知 + (c) 再展開・再表示・ドライブ再選択/切替・main 再表示での再照合。(b) 展開枝の非再帰監視は見送る。** CLAUDE.md「設計の簡素化」による。(b) は監視 worker・登録 ticket・pane 表示 lease・command channel 満杯時の合流など、状態と lifecycle を大きく増やす。報告 (>>707・710) の症状は「一覧の親は更新されるのにツリーに残る」「折りたたみ・再展開・ドライブ切替でも残る」で、(a)+(c) で閉じる。即時の自動反映を保証しない範囲は、別の展開枝、検索・ZIP/PDF表示中、一覧更新defer中、同pathの実体入れ替え、現在folder自身の消失も含む。現在folder自身の消失では一覧のmetadata失敗が走査前に戻るため通知がなく、親の再参加時の再照合で拾う。これらは再展開・再表示・drive選択・既存の ↻ で取り直す（割り切り）。(b) は利用者の報告や要望が出たときに、本書 §5.1 の監視節を土台に別途検討する。
- 採用する部分: §1 の追加条件 (部分列挙・取消・分類失敗を成功扱いしない、hidden 除外と実消失の区別、成功で消えた枝を現在地同期や子の遅着で復活させない)、§2 の簡素化 (既存 `RefreshPreservingChildren` の受理境界への集約、手動 reload の契約維持)、§5 の排他 enum への集約と FollowUp 一件への合流、§5.1 の 1・2・4・5 (需要集合・祖先 reveal・需要離脱時の未確認化・以前 empty の collapsed leaf の再展開)、§5.2、§6 から (b) 固有の行を除いたもの、§7、§8 から監視固有の検査を除いたもの。§4 の一覧→ツリーの橋は `poll_external_rescan` の `accepted_now == true` の直後とする。
- 見送る部分: §4 の「pane 監視 session」、§5.1 の 3 のうち監視登録、§5.1 の監視 worker / command channel / lease / logic service 退役 / callback 通知の段落、同 path の実体入れ替えを watch の affected path で検出する段落 (再表示と手動再取得で閉じる)、§8 の watch 登録・overflow・lease の検査。
- (c) の再照合の入口は、需要へ新規参加した親を「同 options の cache があっても一度確認する」で実装する (§5.1 の 3 の監視を除いた部分)。毎 frame の `sync_to_active` は遷移がなければ何もしない。
- main表示中断の通知は残す。既存main chrome実描画とROOT可視状態からpane走査だけを取消・未確認化し、復帰時に再参加する（監視leaseは作らない）。同一drive実クリックは明示再照合とする。現在地変更時だけ最初のmembership矛盾親を一度再確認する。非監視のexpanded emptyにもcollapse arrowを残す。Populate/RefreshPreservingChildrenのrequest方針は成功cache有無から導く。

## 10. 設計レビュー・検証記録

本案はソース調査による提案。製品の実行観測はバックログに記録された報告者の結果だけであり、この作業で再現したものではない。独立レビューの結果は完了後に本節へ追記する。

### 実装前照合・独立設計レビュー追補 (2026-11-02)

§9.5を優先し、監視session/ticket/command channel/leaseは実装しない。採用範囲の補足:

- 現在地変更時、祖先列の最初の既知membership矛盾親を一度だけ再確認する。同じ現在地の毎frame同期では再要求せず、確定消失枝を復活させない。
- mainの描画中断は既存main chromeの実描画有無とROOT可視状態から通知し、pane走査だけ取消・未確認化する。復帰は需要へ再参加する。同一driveの実クリックも明示再確認にする。
- expanded emptyにもcollapse arrowを残す。collapse後は未確認として再展開できる。
- 行間隔は24ptではなく描画した行allocation高と縦item spacingから記録する実pitchを使う。
- Populate/RefreshPreservingChildrenはrequestに持たず、成功cache有無から導く。明示reloadのみ先にcacheを消し、失敗は残存cacheを保持する。
- (b)見送りの非自動範囲には、別の展開枝、検索・ZIP/PDF表示中、一覧更新defer中、同path実体入れ替え、現在folder自身の消失も含む。一覧のmetadata失敗は受理に到達しないため、親を再参加時に再照合して消失を取得する。

コード照合: `poll_external_rescan`の既存owner/source/generation/projection検査と`accepted_now`を利用できる。paneはbundle swap対象外。main表示通知は`update_frame`のmain bars描画到達を外側へ返すだけにし、detached/fullscreen述語・viewport経路を変更しない。scan workerは`try_classify_dir_entry`を使い、hidden除外前membershipを返せる。上記以外に採用設計との矛盾なし。

### 実装 (tree-1686 / ac412bb76上の未コミット差分)

- `FolderPaneNode`が成功cacheと排他的scan stateを一つずつ所有する。適用方針をrequestから削除。Running中の通知はFollowUp一件、旧完了は非採用、options/需要離脱/表示中断/reloadは即時cancelとprivate Receiver退役。旧cache/受信payloadのdropは既存rayonへ渡す。
- 完全な実directory membershipをhidden投影前にworkerで作る。entry/file_type/分類失敗と取消は成功にせず、失敗は旧cacheと既存row警告・logで終端する。成功した親から実消失子孫だけをcomponent境界で整理し、子の遅着と同じ現在地の同期による復活を止める。
- 一覧橋は捕捉したmain identityとfolderを使い、Equal/Differentのacceptedだけ通知する。pane側はresident親だけ未確認化し、一覧/他窓への書戻し・新しい監視はない。
- main表示は`update_frame`の既存chrome描画到達boolを外側で受け取る。ROOT不可視/最小化は既存logic service checkpointでpaneだけ中断する。実pane描画で復帰し、同一drive実選択とexpanded emptyのcollapse/reexpandも確認入口にした。新しいdetached/fullscreen述語やviewport処理は追加していない。
- 自動成功時は同じScrollArea IDと実pitch（実allocation高+縦item spacing）のpath anchorを保持し、cursorの能動scroll要求を優先する。↻/GridReloadの全resetは維持する。
- 開始/採用は各pump 4親まで。Queued/Running残存中は既存80ms repaint pollingを毎pass継続し、idleは新しいscan/repaintなし。worker送信wakeの新設はせず、既存paneの進行方式を再利用した。

### 独立コードレビュー

`gpt-6.1-sol / xhigh`、別context・read-only、ビルド/テスト重複実行なし。
自動展開枝自身のcollapseを先行取得需要が無視するP2を指摘。自身のis_expandedと到達性の両方で需要を判定するよう修正し、empty/rootの再展開arrowも確認した。修正後のcoherent差分はコード観点ACCEPT。最終検証台帳・snapshotの確認は別途記録する。

### 検証環境と途中の失敗

- default features、Windows MSVC、`MSBUILDDISABLENODEREUSE=1`。製品起動/コミットなし。
- 初回・2回目checkはTurboJPEGのCMake/MSBuild（parallel24/4）がexit1で止まり、製品型検査へ未到達。同じbundled sourceを既存target内で`cmake --build ... --target install --config Debug --parallel 1 -- /nr:false`として成功させた。
- 以後はprocessだけ`TURBOJPEG_SOURCE=explicit`、`TURBOJPEG_LIB_DIR=target/section1686/turbo-explicit`（同じstatic libraryをturbojpeg.libとして実コピー）、`TURBOJPEG_INCLUDE_DIR=target/debug/build/turbojpeg-sys-e58481dd5d2e733f/out/include`、通常include補充の`CXXFLAGS`、`CARGO_BUILD_JOBS=4`を使用する。依存source、Cargo設定、build scriptは変更していない。再現用環境は`target/section1686/env.ps1`。
- 3回目checkでegui ScrollArea Stateのprivate offset参照（製品1か所・test fixture1か所）を検出し、公開`unclamped_offset()`と実ScrollAreaへ渡す初期anchorに修正した。`check.log` / `check2.log` / `check3.log`を成功根拠にしない。最終結果は後記。
- live UI、実機idle/perfは未実施。headless/fake-input/snapshotと区別し、製品は起動しない。
- 初回焦点検査は44成功/3失敗。現在地testのSuspended直接代入とreload後root未展開を正規の状態遷移/fake backendへ直した。2回目は46成功/1失敗。実行行矩形の差は29ptで、指定label高24pt+spacing3ptの仮定が誤っていたことを確認した。Button/text最小寸法による実allocation高を返し、描画したgeometryからpitchを記録するよう製品を修正した。テストもviewport上端の実anchorを選び、未clip座標を比較する。初回/2回目を成功証拠にしない（`folder-pane.log` / `folder-pane2.log`）。

### 最終検証台帳 (2026-11-02)

対象は本worktreeの未コミット差分、default features、test/dev profile。製品起動・コミットなし。上記process環境を継続し、以後はcmdによるraw log転送と明示exitでPowerShellのnative stderr変換を避ける。

| コマンド・範囲 | 結果 | 証跡 (`target/section1686/`) |
| --- | --- | --- |
| `cargo test -p mimageviewer --lib folder_pane -- --test-threads=2` | **47成功 / 失敗0**、実行18.36秒、exit0。実paneの同driveクリック、expanded empty再展開、実座標の深いanchor/逆順/cursor優先、報告経路とmain復帰A Equal/別枝B、受理資格、hidden、aaa/aaa2、私有応答、worker隔離を含む | `folder-pane4.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/section375/) \| test(/external_rescan/) \| test(/viewer_context_registry/) \| test(/folder_tree/) \| test(/reload/)' --no-fail-fast --retries 0 --no-tests fail` | **321成功 / 失敗0**、38.427秒、exit0。既存一覧/registry/手動更新/ソート・巡回を回帰 | `related.log` |
| `UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot folder_pane_refresh -- --test-threads=2` | **2成功 / 失敗0**、exit0。light/dark新PNGを実装担当・独立reviewerが目視確認 | `snapshot-generate2.log`、`tests/snapshots/folder_pane_refresh_{light,dark}.png` |
| `cargo test --test ui_snapshot -- --test-threads=4`（更新指定なし） | **220成功 / 失敗0**、75.24秒、exit0 | `snapshot-all.log` |
| `cargo check -p mimageviewer --lib --tests` | exit0、47.24秒 | `check-final.log` |
| `cargo fmt --check`、UI glyph / site cards、`git diff --check` | exit0。危険文字0、49cards・上限超過0 | `fmt-final.log`、`glyph.log`、`site-cards.log` |
| `scripts/test-lib-full.ps1`（1回、source `42c758de…`） | **15,503成功 / 2失敗 / 71skip**、814.548秒、exit100。14slow / 2leakyを含む。成功扱いにしない | `lib-full.log`、`target/test-lib-full/runs/20261102T114744341Z-5cbad50b/junit.xml` |
| 上記失敗2件を各単独3回、retryなし・test-threads1 | **6実行すべて成功**、各exit0。main chromeは約0.52秒、起動復元は約0.90〜0.95秒 | `isolated-before.json`、`isolated-before-{0,1}-{1,2,3}.log` |
| fixture修正後 `nextest` の `folder_pane \| main_chrome::tests` | **55成功 / 失敗0**、2.552秒、exit0。a11y検査も有効 | `fixture-final.log` |
| 修正後main chromeを各単独3回 | **3実行すべて成功**、各exit0 | `isolated-after.json`、`isolated-after-{1,2,3}.log` |
| `scripts/test-full.ps1 -SuppressCrashDialogs`、`NEXTEST_TEST_THREADS=8` | **未完了 / exit101**。workspaceのno-run compileでlauncher入力exe不足。workspaceのテスト実行・doc・vendor後続へ未到達 | `full.log` |
| fixture補正後 `cargo fmt --check` / `cargo check -p mimageviewer --lib --tests` | **各exit0**、check 46.81秒 | `fmt-post-fixture.log` / `check-post-fixture.log` |
| `scripts/build-dev.ps1 -PreserveRuntime`（通常features / dev-runtime） | **exit0**。core 19分38秒、remote 2分05秒、EPUB worker 2分15秒。VCRT runtime4 / PE3成功。製品起動・プロセス停止なし | `build-dev.log` / `artifacts.json` / `target/vcrt-pe-reports/dev-runtime.json` |

`folder-pane3.log`は追加testの廃止API `Context::style()` 参照でcompile失敗。既存`global_style()`へ直して上記47件で確認した。成功件数には含めない。

初回snapshot生成は常時repaintするspinnerに対するHarness.runの静止待ちで2件失敗した。新規2件だけ、既存helperのanimated_steps=Some(12)へ変更して上記2件と全220件で確認した（製品描画と既存fixtureは不変）。

全libの失敗は `main_chrome_folder_tree_clicks_follow_common_modal_ownership` のHarness.run 4pass超過（23passなら静止）と、`section1335_tray_explicit_book_list_hide_restore_keeps_normally_opened_and_backspace_lists` のlisting identity準備待ち上限超過。両方とも単独3回では再現しなかった。前者は入力/resizeの試験でありFS列挙完了の検査ではないため、既知cacheへfixtureを隔離し、全BlockerのassertionとHarness.runを維持した。旧blank-arrow用のaccessibility検査無効化も撤去した。test_treeは実drive一覧を保持し、1.5秒後の実drive再列挙によるtemp root変更を避ける。headerだけの同drive操作testは選択肢を明示し、同期入口を呼ばない。製品の同期I/O・待機・表示機能と、起動復元の待ち上限は変更しない。独立reviewerが単独6結果とこのtest隔離を確認してACCEPT。

追加full gateは `target/release/mimageviewer-core.exe`、`mimageviewer-remote.exe`、`mimageviewer-epub-pdf.exe` の未配置で `crates/launcher/build.rs` が停止した。今回launcher/配布/ビルドlogicは変更しておらず、偽の入力や別sourceのexeを流用しない。この結果を全体gate成功と扱わず、release入力が揃った環境でのworkspace/doc/vendor gateを未検証として残す。必要な通常core確認用buildは別途`build-dev.ps1`で行う。

### 利用者確認用成果物・引き継ぎ

通常featuresの `target/dev-runtime/mimageviewer-core.exe` を作成済み（339,803,136 bytes、SHA-256 `1992F32CA9E16EEA30A190E8C097E40656ED334E5680972A64B886B187C3F462`）。同protocolのremoteとEPUB PDF workerも同じdirectoryに配置済み。3件の日時/size/hashは`target/section1686/artifacts.json`。エージェントは起動していない。実装担当はコミットせず、`tree-1686`の差分を設計担当へ渡す。

このworktreeで利用者が起動するコマンド:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

先にインストール版/tray常駐版を終了する（single-instance mutexを共有）。このfast buildは通常の `%APPDATA%\mimageviewer` を使い、実設定・data/cacheを更新し得る。手動確認は、一覧の親を開いたまま外部でaaaを移動/削除したときツリーからも消えること、空枝のcollapse→外部子作成→reexpand、一覧Aを変えず別の展開Bを変更したmain復帰、既に展開中のdataへ外部でnewを追加してアドレス入力で開いた場合の祖先表示を対象とする。live UI/idle/perfは未実施であり、headless成功と読み替えない。

コード観点の独立reviewはACCEPT。独立reviewerも焦点/単独/snapshot/check/buildログと3exeの実hash一致、利用者向け手順・検証制限の記載を確認した。全体gate未完了と実機未確認を残し、release入力が揃った環境でのfull gateと最終受入・コミットは設計担当に引き継ぐ。

## 11. §1.694 ツリーを開いたときの現在地表示 (2026-11-03)

以下の初回実装・検証は`655e79211`までの記録。後続の独立レビューで見つかったP2 2件の対応は本節末尾に追記する。初回の確認用binaryには後続対応を含まない。

### 観測・再現と旧版比較

利用者は§1.686/§1.683を含むv5試用ビルドで「最初にツリーを開いたとき現在地が表示されず、開いた後の場所変更では追従する」と報告した。新しい隔離data-dirでの確認だが、開いた入口と、枝の未展開・画面外・選択表示なしのどれかは未確認。

開発側のheadless Appテストは実際の`set_folder_tree_pane_visible(true)`と既定Fのキーイベント（Tはタグ操作）から開き、`render_folder_pane`のeguiフレームと実scan workerの完了を進めた。起動時非表示→表示、起動時表示、閉じた間の別フォルダ移動→再表示、移動なしの再表示では、枝・行、active/cursor表示、viewport内の行全体の包含が成功した。利用者の初回表示と同じ症状を再現したとは扱わない。

別途、再表示の最初の描画後に再参加scanを完了させ、現在地より前へ多数の兄弟を挿入すると、**(b) 行とactive/cursor表示はあるが、行がviewport外に残る**ことを再現した。修正前の`target/tree-open-repro-ready.log`は9件中8成功/1失敗（行y=4825〜4851、viewport y=39〜358）。使い捨てsubtree内は製品scanner/workerで実走査し、その上の祖先だけ固定listingへ置き換え、ホストdriveの内容・権限を試験条件にしない。

`4d421d1c5^`の旧`sync_to_active`は祖先を無条件にauto-expandし、現行`reveal_active`は成功membership/表示childrenの矛盾で止まる。未取得祖先の先行取得と、新しい場所/再参加時の確認を通じた展開は保たれており、今回のcoldテストで(a)/(c)の退行は確認できなかった。一方、旧版も最初の行描画でscroll要求を消す。§1.686は再表示時のscanとpath anchor復元を追加したため、残存cacheに対する能動scrollと新listing採用が重なる経路が生じた。再現した(b)はその経路の退行であり、祖先展開の停止条件を撤去する修正にはしない。

### 原因と不変条件

`set_folder_tree_pane_visible`→`sync_to_active`→`set_focus_tree_at_active`で作った一回要求を、`render_folder_pane_tree`が最初のcursor行描画だけで消費する。しかしeguiの既定`scroll_to_me`は数値offsetへのanimationを後続フレームへ持ち越す。その途中でscanが行を増減すると、古い絶対目標と新しいpath anchor復元が競合する。同じ親の新しい兄弟がviewport上端anchorと現在地の間に入る場合も、上端anchorの保持だけでは現在地の可視性を保証できない。

不変条件は「開く/能動cursor操作の表示要求が完了するまで、要求したcursorの行を新しいscan/layoutへ追従させる。明示的なwheel/dragは利用者の操作を優先する。要求完了後の自動更新は従来の上端path anchorを保持する」。追加repaint、毎フレームの強制scroll、scan待ちの遅延、全tree resetは使わない。

### 修正

- `scroll_to_cursor: bool`を単一の`FolderPaneCursorScroll::{Idle, Requested, FollowingRefresh}`へ置換した。開く・active同期・cursor移動・Enter後の同期・drive選択・reload・展開/折りたたみは同じ要求へ合流する。scan中にcursor行を描いても、`FollowingRefresh`が最終listingまで要求を所有する。未変化のフレームではscrollを再発行せず、描画path列または実pitchが変わった時だけ現在のcursor行へ再解決する。
- 能動scrollは`ScrollArea::animated(false)`で同じlayout内に確定する。通常のwheel/dragのnative physicsは維持する。native animationの絶対targetを別ownerで追跡・取消する機構を作る案を検討したが、現在地への一回要求を即時確定すればその寿命を除けるため採らなかった。再走査中の行挿入は上の単一状態が扱う。`ScrollAnimation::none()`だけでは次passまでtargetが残るため代用しない。
- rendererは既存要求をtakeして処理し、同じ描画中の行click/arrowが作った新要求を旧要求の完了で上書きしない。追従中のtree内wheel/scrollbar/content drag開始は利用者へviewportの所有を渡す。tree外のgrid wheelでは追従を取り消さない。要求完了後は既存の上端path anchor復元へ戻す。
- `reveal_active`のmembership/hidden投影による停止、需要の所有、完全成功による消失確定、他contextとの境界、§1.686の外部更新経路は変更しない。

独立設計レビュー（Sol/xhigh、別context/read-only）は、即時確定と単一要求による最終layoutまでの追従を構造的修正として支持した。最終コードレビューと検証結果は以下に記録する。

### 回帰・独立レビュー

App側へ11件追加した。4つの開き方と既定F、root/同じ親へのscan中の兄弟挿入、tree内wheelでの追従取消・tree外wheelとdisabled treeでの追従保持、同じ描画中の新しい行Click要求の優先を検査する。最後の検査は実際のlabel response IDへのAccessKit Clickで製品の`resp.clicked()`を通し、pointer hit testing/drag配送の検査とは分ける。scan中の未変化フレームとscan完了後90フレームではscroll要求の発行数が増えないことも確認する。既存のstateテストは同じassertionをenumへ適応した。

Sol/xhighの独立コードレビューは**ACCEPT、重要な未解決指摘なし**。指摘1件（disabled treeの入力でも追従を取り消していた）を`ui.is_enabled()`で修正し、modal gate下のwheel回帰を追加した。reviewerはコードと修正前/修正後ログを読み、ビルド・テストを重複実行していない。

途中の追加testにはwheel eventの必須`phase/source`不足、行全体rectの中心をクリックしたfixture、disabled wheelを消費するdialog本体を描かず直ちにreenableしてsmooth tailを流したfixtureの不備があった。compile失敗/58件中56成功2失敗/57成功1失敗を成功根拠にはしない。Clickは上記handler経路へ限定し、disabled入力はgate保持中にscan/quiet framesを完了させてからreenableする。最終`target/tree-open-folder-final.log`は**58成功/失敗0、14.404秒、exit0**。focused runnerも39/17/18/15件の4群で全成功（計89実行、重複を含む）。

### 検証環境

`tree-open-1694` / `498a7e901`上の未コミット差分、Windows MSVC、default features、test/dev profile。`MSBUILDDISABLENODEREUSE=1`。初回FFmpeg binding生成はlibclangの`STATUS_PRIVILEGED_INSTRUCTION`、TurboJPEGは通常CMake buildのexit1で停止した。Visual Studio 18付属libclangを指定し、同梱TurboJPEG sourceを本worktreeの`target/tree-open-native/source`へ実コピー、SIMD有効・Release `/MT`・単並列で静的buildしてexplicit libraryへ配置した。依存source・Cargo設定・製品build scriptは変更していない。以後の再現用process環境は`target/tree-open-env.ps1`（LIBCLANG/TURBOJPEG/CXXFLAGS、最終`CARGO_BUILD_JOBS=4`）。初期PowerShell直接redirectのnative stderr変換を避け、最終Cargoログはcmdのraw redirectと明示exitで取得した。

### 最終検証台帳

全lib gateは利用者指定により設計担当へ引き継ぎ、WebView2を使う試験と製品起動は行わない。下記は同じ製品差分に対するdefault featuresの非対話検証。

| コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast` | **58成功/失敗0、14.404秒、exit0** | `tree-open-folder-final.log` |
| `scripts/test-focused.ps1 -Path src/folder_pane.rs,src/folder_pane/refresh_tests.rs,src/ui_folder_pane.rs,src/app/folder_pane_refresh_tests.rs` | **39/17/18/15件の4群すべて成功、exit0**。89実行は重複を含む | `tree-open-focused.log`、`test-focused/runs/20261102T2329*`のJUnit/command/log |
| `cargo nextest run -p mimageviewer --lib -E 'test(/main_chrome::tests/)' --no-fail-fast` | **8成功/失敗0、0.982秒、exit0** | `tree-open-main-chrome.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit0、7分47秒**。既存warningあり | `tree-open-check.log` |
| `cargo test --test ui_snapshot -- --test-threads=4`（画像更新なし） | **未完了、exit2173**。226件の途中で実行体が異常終了、assertion/画像差の失敗報告なし。成功扱いにしない | `tree-open-snapshot.log` |
| 同じsnapshot実行体をCDB下、`--test-threads=4 --nocapture`で一度診断 | **226成功/失敗0、309.92秒、子process native exit0**。通常実行の2173は未再現・原因未特定。通常gateの失敗を置換しない | `tree-open-snapshot-debug.log`のexit event、`tree-open-cdb-summary-3.log`のlibtest summary |
| `cargo test --test ui_snapshot folder_pane -- --test-threads=4`（通常、画像更新なし） | **4成功/失敗0、2.47秒、exit0**。`folder_pane_refresh_*`/`folder_pane_location_*` | `tree-open-snapshot-folder.log` |
| 引数なし`cargo fmt`、`cargo fmt --check`、UI glyph、`git diff --check` | **各exit0**、危険文字0件。snapshot基準画像の差分なし | `tree-open-fmt.log`、`tree-open-fmt-check.log`、`tree-open-glyph.log`、`tree-open-diff-check.log` |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | **exit0**。通常features/dev-runtime、core17分46秒・remote1分28秒・EPUB worker1分11秒。runtime4/PE3検査成功、起動・既存process停止なし | `tree-open-build-dev.log`、`tree-open-build-progress-{9,10}.log`、`vcrt-pe-reports/dev-runtime.json` |

snapshotの最後のokは4並列の完了順であり、原因テストを表さない。停止付近の`external_tool_*`はlabel等の純描画で外部tool起動・明示exitへ接続せず、対象sourceに2173の明示終了は見つからなかった。現在のharnessはwgpu30系・DX12/Vulkanであり、旧[WGL AV調査](ui-snapshot-gpu-investigation.md)と同一原因とは扱わない。同じ実行体/4threadsをCDB下で一度診断し、例外/exit eventを採取したが同じ異常終了は出ず全226件が成功した。CDB自体のexit0だけでなく、子processのexit event code0とlibtest summaryを確認した。製品、snapshot基準画像、harnessのbackend/並列性は変更していない。CDB初回はrelative executable pathを解決できず試験前に停止し、絶対pathへ直してから診断した。通常全件の2173は未解決の検証制限として残す。

### 成果物・引き継ぎ

`target/dev-runtime/mimageviewer-core.exe`（339,381,760 bytes、SHA-256 `D5CA4815E54FEBEB933B80B24F93FC265022682176A07C1293C2695FDA607C37`）と、同版のremote/EPUB workerを作成した。3ファイルの日時・size・hashは`target/tree-open-artifacts.json`。portable featureは使っていない。製品起動・コミット・全lib gateは実施していない。全lib検証をこのworktreeで行う際は、先に`. .\target\tree-open-env.ps1`で検証用のnative依存環境を読み込む。

利用者確認用（エージェントは起動しない）:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

先にインストール版/tray常駐版を終了する（single-instance mutexを共有）。この高速buildは通常の`%APPDATA%\mimageviewer`を使い、実設定・data/cacheを更新し得る。新しい隔離設定で確認する場合は`--data-dir .\target\dev-runtime\data`を引数へ渡す。確認対象は、兄弟の多い深い現在地から初めてツリーを開いた時の行表示（利用者報告との一致は未確認）と、閉じた間の場所変更/兄弟追加後の再表示で現在地が見えること。通常snapshot全件の異常終了は上記の未解決制限として残し、設計担当が全lib検証・実機確認・最終受入・コミットを行う。

### `655e79211`への独立レビュー追補: P2 2件 (2026-11-03)

追加レビューの指摘はどちらも正しい。製品コードの変更前に実App入口・描画・worker完了を通す8件を追加し、`target/tree-p2-repro.log`で8失敗/成功0（exit100）を確認した。

- **P2-1 / (b):** `animated(false)`の即時scrollはoffsetだけを書き換え、以前のdrag releaseの速度や既存のanimation targetを消さない。実pointer drag→release→通常慣性でoffsetが進むことを確認した後、↑でrevealしても縦速度が約2443pt/s残った。旧animated経路はtarget処理時に速度を消していたため、これは初回修正の退行。不変条件は「通常の慣性は維持し、新しい能動revealが適用された境界で以前の縦運動を終える」。vendor eguiの`State::stop_vertical_scroll_motion`で縦velocity/targetだけを消し、rendererが実際に`scroll_to_me`した`ScrollArea`の出力stateへ一度適用してstoreする。offset・横運動・現在のpointer gesture・通常の同期は変更しない。deltaが0でも適用される。
- **P2-2 / (a)(c)、表示範囲への要求欠落:** 現在地Noneで開くと`set_focus_tree`がfocusと既定root cursorを設定し、後日確定した現在地を`sync_to_active`のfocus guardが抑止する。同一driveでは現在地行にcursorが付かず、別driveでは`reveal_active`のdrive guardで枝も現れなかった。ZIP/PDF/検索から実フォルダへの移行でもcursor不一致を再現した。`4d421d1c5^`にも同じfocus/cursor guardとselected driveの同期条件があり、§1.686や初回修正で導入した退行ではない。利用者報告の原因候補だが、報告時の初期現在地が未確定だったことは未確認。

P2-2の不変条件は「未確定時の代替位置と利用者が選んだ位置を区別し、受動位置は最初/次の現在地変更へdriveとcursorを一緒に追従させる。利用者の明示位置はツリー操作中には奪わない」。`selected_drive`と`cursor_path`を`FolderPaneSelection`へまとめ、focusは入力配送だけを担う。状態は受動`FollowActive`、ツリー操作の`Explicit`、gridへfocusを戻した後も次の現在地変更までは明示位置を保つ`ExplicitUntilActiveChange`。最後の状態はFでのEnter相当確定権限を保持し、次のactive変更で受動追従へ戻る。開く/Esc/reloadは現在地へ再解決し、cursor/drive/展開/折畳操作は明示状態、受動scanのcursor修復とdrive消失時のpair修復は由来を保つ。drive列挙を待たずactive rootへ解決でき、未確定時の既定rootをFで勝手に開かない。

仮想フォルダの前提も照合した。現行`active_filesystem_folder`はZIP/PDFを親フォルダの`Some`へ写し、検索結果のsynthetic pathも`Some`を返す。これらの既存mappingを変更せず、各状態から実フォルダへの追従を検査する。drive列挙前はstate試験でdrive到着→代替root→別driveの現在地確定を検査する。非同期順序の組合せを待機・retryで管理する案は採らず、位置の由来を単一ownerで表すことでfocusとの曖昧な組合せを除く。

追加回帰はApp 9件（Noneからの現在地確定をmenu/F×同一/別driveの4経路、ZIP/PDF/検索から実フォルダの3経路、実drag慣性中の↑/↓、明示矢印→active変更→grid focus→F確定）、state 6件（drive到着、明示cursor/drive保持、移動しない展開/折畳、drive消失、受動scan修復）、vendor 1件（offset/横運動を保った縦速度/target停止）。App試験は兄弟の多いsubtreeで(a)行・祖先、(c)active/cursor、(b)viewport内の行全体を確認し、慣性試験は通常慣性の継続とreveal後30フレームの可視性をIdle/scan中の両方で検査する。実subtree外のlistingはsyntheticに限定する。

独立Sol/xhighの追補設計/コードレビューを実施した。最初の2状態案は、gridへfocusを戻すと明示位置の由来まで捨て、FのEnter相当確定を失うP2でREJECT。上の第三状態を加え、focus反復・同一activeの同期・F target保持のstate回帰と実App入力回帰を追加した最終差分は**ACCEPT、重要な未解決指摘なし**。reviewerはread-onlyでコードと再現ログを確認し、build/testを重複実行していない。

初回修正後のfilterは72件中71成功/1失敗（`tree-p2-fixed.log`）。既存の祖先再確認テストが、fixture上で明示位置なのにcursor未設定のままactiveだけを注入していた。実openと同じ`set_focus_tree_at_active`で初期化するよう補正し、消失枝が同一active同期で復活しないassertionは維持した。この途中結果を最終成功の根拠にはしない。

第三状態を含む次のfilterは73件中72成功/1失敗（`tree-p2-folder-final.log`）。新規App試験が現在地を親へ移したことで、既存のauto-expanded branch入替により保持cursorの枝が折り畳まれ、viewport検査まで進めなかった。製品の展開仕様は変更せず、現在地を同じ親の別子へ移すfixtureへ補正し、明示cursorの可視性・保持・F確定を検査する。これも最終成功として扱わない。core checkとsnapshot成功後の変更は、この`cfg(test)` fixtureと文書だけであり、製品差分は同一。

追補の最終検証は`tree-open-1694` / `655e79211`上の差分、Windows MSVC/default features、`MSBUILDDISABLENODEREUSE=1`と上記`tree-open-env.ps1`のnative依存環境で実施した。

| コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast` | **73成功/失敗0、14.313秒、exit0** | `tree-p2-folder-verified.log` |
| `scripts/test-focused.ps1 -Path src/folder_pane.rs,src/folder_pane/refresh_tests.rs,src/ui_folder_pane.rs,src/app/folder_pane_refresh_tests.rs` | **45/23/18/24件の4群すべて成功、exit0**。110実行は重複を含む | `tree-p2-focused.log`、`test-focused/runs/20261103T0107*`のJUnit/command/log |
| `cargo test --manifest-path vendor/egui/Cargo.toml --lib stop_vertical_scroll_motion -- --nocapture` | **1成功/失敗0、exit0**。standalone vendor graph | `tree-p2-egui.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit0、4分41秒（build lock待ちを含む）** | `tree-p2-check.log` |
| `cargo test --test ui_snapshot folder_pane -- --test-threads=4` | **4成功/失敗0、2.37秒、exit0**。画像更新なし | `tree-p2-snapshot.log` |
| 引数なし`cargo fmt`、`cargo fmt --check`、`git diff --check` | **各exit0**、snapshot基準画像の差分なし | `tree-p2-fmt-check.log`、作業終了時のdiff検査 |

今回の指定により`build-dev.ps1`、全snapshot、全lib gate、WebView2試験、製品起動、コミットは実施しない。確認用binaryの再作成と最終受入は設計担当へ引き継ぐ。初回全snapshotのexit2173に関する制限は上記の歴史的記録として残す。

### サブPC Cのfocus差 (2026-11-03)

同じ入力batchのgrid release→Fで、先にFがtree focusを設定し、後段gridが古いreleaseを受理して`set_focus_grid`する。`655e79211^`からある経路で、起動引数固有の退行ではない。サブPC Cでは現在地の枝・行・viewportは正常で薄青だったが、frame内の入力順序の実機ログは無いため、この再現とCの原因一致は未確定。

最初の入力ordinal案`90a6da40e`は独立レビューでREJECT。toolbar close→F openで再びfocusを失うことと、native Enter/テンキーを無条件に`AfterInputBatch`へ置くと後のgrid releaseまで古いと退けるP2 2件があった。ツリー全操作の順序付けへ広げなければ塞げないため、`517d0b5ce`でrevertし、利用者決定の単純な形へ改める。

**設計の簡素化:** Fとtoolbarの開閉はその場で適用せず、Appのtypedな最終表示要求1つへ合成し、一覧・toolbarのclick処理後、既存の`finish_folder_pane_presentation`でtakeして適用する。次のツリー描画前の1点に確定し、表示/focus変更に伴うrepaintは適用時に1回だけ。main chromeを描かなかったframeも、先にpaneをsuspendして受理要求を適用し、scanを始めず要求を次frameへ持ち越さない。既存main projectionの終端を使い、detachedの述語・viewport経路は変更しない。

複数要求は**受理済み要求数の偶奇**で表示を反転する。toolbarも実設定でなくpending込みの投影表示を反転する。偶数回でも最終openなら現在地へfocusし直し、途中closeの移動は実行しない。最終closeの方針は最後に受理したhandler由来（F/Yはcursor commit、toolbarは単純close）とし、rawの最後とは主張しない。元から非表示で最終closeなら何もしない。keymapのSinglePerFrame消費・割り当て・APIは変えず、raw Fが2個でも1個しか受理しなければ要求は1個。

入口はF、toolbar「ツリー」、main grid向けgamepad Yを共通toggle要求へ寄せる。gamepad Aは既存のOpen開始を維持し、visibilityだけ「追加commitなしの明示close」として合流させる。設定の起動時読込み/復元/transferは設定採用でありframe内の開閉commandではないため、このqueueへ入れない。行click・矢印・Enter・drive選択・枝の開閉は既存処理のまま。

承認済みの制限は**同じ描画1回分に入った場合だけ**、F→grid clickでも最後の開閉がtree focusを保つこと、非表示からのF→矢印は開く前の一覧を動かすこと。間が空く通常操作は従来どおり。入力ごとのordinal案は全ツリー操作のrouterが必要になるため採らず、入力を1frame1件へ制限する案も、重いframeでの遅延・nativeキー状態とのずれ・全機能への影響が大きいため採らない。「下寄り」は末尾offset clampという前回判断を維持し、変更しない。

`Align::Center`は行中心をviewport中心へ合わせるoffsetを要求するが、eguiは最後に`content_size - inner_rect.size()`を上限としてclampする。末尾に近いtargetを中央へ置くための下側余白が足りない場合は下寄りになる。viewport内表示は成立しており、余白を増やす変更はしない。

修正前の実App・egui回帰5件は2成功/3失敗（`target/tree-simple-repro.log`）。F単独/toolbar単独は成功し、grid release→Fと承認済み制限のF→releaseはfocus検査で失敗、toolbar close→Fは不要な中間Open開始で失敗した。修正後は同じ実click・key入力からscan完了まで進め、枝/active/cursor/viewportを検査する。追加で複数受理の偶奇・最終close方針、F→矢印、非描画frameの要求消費、gamepad AのOpen非重複を検査する。

独立Sol/xhighレビューは今回の単純化差分を**ACCEPT**とし、要求範囲内の未解決指摘なし。レビュー側は編集・build/test・製品起動を行わず、最終検証は実装側が担当する。

最初の修正後filterは82件中81成功/1失敗（`tree-simple-filter-fixture.log`）。残った1件はparity試験がscan backendの設定をcache seedと誤認し、走査前にvisible行を要求したfixtureの誤り。適用直後のcursorは、状態を変更しないEnterの戻り値`Open(active)`で直接確認するよう補正した。描画/viewportは実Appフレームを進める別の回帰で引き続き検査する。製品コードの追加変更はない。

最終検証（`MSBUILDDISABLENODEREUSE=1`、既存native環境`target/tree-open-env.ps1`を使用）:

| 検査 | 結果 | 記録（`target/`） |
|---|---|---|
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast` | **82成功/失敗0、17.738秒、exit0**。新規9回帰を含む | `tree-simple-filter.log` |
| `scripts/test-focused.ps1 -Path @('src/ui_folder_pane.rs','src/ui_main.rs','src/app/folder_pane_refresh_tests.rs','src/app/gamepad_input.rs')` | **18/384/33/110件成功、計545実行（重複あり）、exit0** | `tree-simple-focused.log`、`test-focused/runs/20261103T042501967Z-bc7a971b` / `20261103T043003418Z-f9a99c8e` / `20261103T043424180Z-38df1453` / `20261103T043652894Z-795f37db` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit0** | `tree-simple-check.log` |
| `cargo test --test ui_snapshot folder_pane` | **4成功/失敗0、2.74秒、exit0**。画像更新なし | `tree-simple-snapshot.log` |
| 引数なし`cargo fmt`、`cargo fmt --check`、`git diff --check` | **各exit0** | `tree-simple-fmt-check.log`、終了時diff検査 |

`app.rs`はfocusedのbroad/shared対象なので、今回は指定された全lib省略に従い上記4pathを明示し、rootのF配線は実handler回帰で検査した。fixture補正は`cfg(test)`の試験だけで、製品sourceが同じcore check/snapshot結果を再利用する。独立レビューは補正後もACCEPTを維持。指定により全lib・全snapshot・build-dev・WebView2試験・製品起動・コミットは行わない。実機Cとの原因一致と最終受入は設計担当へ引き継ぐ。

### 単純な形のnavigation裁定への接続（2026-11-03、`e196c3123`追補）

別担当Sol/xhighの独立レビューはP2 1件/P3 1件でREJECT。単純なframe末尾適用の方針は維持する。P2はF-closeが予約した`HideAndNavigate`が、toolbarのお気に入りC等を既存裁定が採用した後にcursor BのOpenを新しく開始して覆す退行。P3はEnterが既に開始したBのOpenを同じbatchのF-closeが再開始し、先のworkerを取り消す既存経路。共通の原因は、visibility適用がnavigation裁定の外でOpenを開始していたこと。

不変条件は「表示状態はframe末尾の1点で適用する。cursor確定Openは既存main navigationの所有境界で一度だけ裁定し、同じframeの別navigationや既存pane Openを覆さない」。単純化としてcommit候補を既存裁定へ合流し、新たなbool/pending/入力順序番号や全入力routerを追加しない。検索drillのように候補を返さずその場で移動する経路も、既存main surface切替の要求退役境界へ接続する。独立readerに対する既存main-owner条件は変更しない。

`post_render_main_navigation`の入口で`HideAndNavigate`を`Hide`へ変更し、**poll前**のpane Openが無い場合だけcursor候補を取得する。mouse ring/saved groupを統合したinput、folder結果、favorite/address/context等の既存優先候補、明示pane候補、grid候補を優先し、他に移動が無い場合だけ既存pane worker開始経路へ合流する。末尾ではOpenを開始せず、main chrome未描画で裁定が無いframeもvisibilityだけを確定する。直接移動する検索drillや書庫開始は共通の`retire_replaced_main_list_requests_with_startup_intent`の既存main-owner確認後、既存`explicit_request=true`の場合にcommit部分を退役させる。非明示refresh/継続とShow/Hide要求は保つ。旧即時toggle helperは唯一の呼出し元が不要となるため撤去した。

修正前の実handler回帰は**2件中2失敗**（`target/tree-nav-repro.log`）。F-close＋toolbarお気に入りはCの採用後にBのpendingが残り、Enter＋Fは既にreadyへ移ったEnterのworkerとは異なるcancel Arcを持つworkerが新規開始された。追加回帰はこの2件と、候補を返さない検索drill（container/subfolder両方）、裁定無しのframeでvisibilityだけ閉じる場合、書庫/rating ZipDirの直接受理、非明示refreshの退役境界でcursor確定を保持する場合の計6件。既存frame helperも製品と同じ裁定→末尾適用の順へ合わせた。

最初はsurface切替境界だけへ退役を接続し、filter86件・focused552実行・core check・関連snapshot4件が成功した（`tree-nav-*-pass1.log`）。兄弟入口の追加照合で書庫/rating ZipDirの直接開始を取りこぼすと分かり、共通の明示request受理へ接続を移した。既存`explicit_request`で非明示refreshを区別できるため、新たな世代snapshotや別routerは不要。surface側への重複接続は残さない。

追補の独立Sol/xhighレビューは**ACCEPT**。poll前のOpen保護・既存優先順位・直接surface切替の所有境界を確認し、重要な未解決指摘なし。レビュー側は編集・build/test・製品起動を行わず、最終検証は実装側が担当する。

最終版の検証（`MSBUILDDISABLENODEREUSE=1`、`target/tree-open-env.ps1`の既存native環境）:

| 検査 | 結果 | 記録（`target/`） |
|---|---|---|
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast` | **88成功/失敗0、18.155秒、exit0**。新規6回帰を含む | `tree-nav-filter-final.log`（注記補正前も`tree-nav-filter.log`で88成功） |
| `cargo nextest run -p mimageviewer --lib -E 'test(/ratingnav649::/)' --no-fail-fast` | **17成功/失敗0、10.105秒、exit0**。既存main裁定を使う一覧順navigationも維持 | `tree-nav-arbitration.log` |
| `scripts/test-focused.ps1 -Path @('src/ui_folder_pane.rs','src/ui_main.rs','src/app/folder_pane_refresh_tests.rs','src/app/update_phases.rs','src/app/gamepad_input.rs')` | **18/384/39/3/110件成功、計554実行（重複あり）、exit0** | `tree-nav-focused.log`、各runの台帳pathは同logに記録 |
| `scripts/test-focused.ps1 -Path @('src/app/folder_pane_refresh_tests.rs')` | **39成功/失敗0、18.592秒、exit0**。試験の注記補正後の再確認 | `tree-nav-focused-app-final.log`、`test-focused/runs/20261103T051055906Z-c0b34290` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit0** | `tree-nav-check.log` |
| `cargo test --test ui_snapshot folder_pane` | **4成功/失敗0、1.83秒、exit0**。画像更新なし | `tree-nav-snapshot.log` |
| 引数なし`cargo fmt`、`cargo fmt --check`、`git diff --check` | **各exit0** | `tree-nav-fmt-check.log`、終了時diff検査 |

追加の一覧順filterは最初にfile名で指定して0件だったため、実module名`ratingnav649::`へ補正した（`tree-nav-arbitration-empty-filter.log`）。製品の失敗ではない。全lib・全snapshot・build-dev・製品起動・コミットは指定により省略する。

終了時、追加archive試験で非同期payloadの戻り値を意図的に消費しないことを`let _ =`とcommentで明示し、新規unused-must-use warningを除いた。受理の成否は既存のrequest所有assertで引き続き確認する。試験だけの注記補正で製品source/挙動は同一のためcore/snapshot/他群は上記結果を再利用し、影響するApp回帰群とfolder pane filterを再確認して成功した。既存lib test warning数153へ戻り、追加したwarningは残っていない。

### pendingと異なるcursorのF確定（2026-11-03、`42676f3dd`追補）

別担当の独立レビューP2は妥当。`post_render_main_navigation`がpendingの有無だけでclose候補を除外し、同じOpenの重複と、新しく選んだ別の宛先を混同していた。現在地AからBのOpenがpendingの間にcursorをCへ移してFで確定しても、要求はHideへ退役したうえでCが捨てられ、古いBが残る。これは前追補の一律pending保護による退行。

不変条件を「poll前にpendingの宛先とcursor確定の宛先を比較し、同じ先だけ重複として保護する」とする。異なる先は既存のpane navigation候補へ残し、既存のcancel/startまたはready置換経路で新しい先へ移る。`folder_tree::path_eq`の既存path同一性を使い、新しいbool/pending/順序番号は追加しない。高優先navigationの裁定・明示要求の退役・末尾でのvisibility適用は維持する。

回帰は実B行クリックでOpenを開始し、既存receiverを遅延返信fixtureへ差し替えてBをpendingに保持してから、eguiのArrowDownでCへ移しFで確定する。Bの取消token、新しいCのpending、完了後の現在地Cを検査する。最初の試験は行全体の中央をクリックしてBのOpen開始前に失敗したため、既存のlabel idからresponse rectとviewportの交差を取得してクリック位置を補正した（`target/tree-pending-repro-fixture.log`）。このsetup失敗を製品の再現とは扱わない。

補正した修正前回帰は**1件中1失敗**。BのOpen開始とCへのcursor移動は通り、F後の「Bの取消tokenが立つ」検査で失敗した（`target/tree-pending-repro.log`）。修正は裁定入口のfilterを、pending有無からpending.pathとtargetのpath同一性比較へ置換するだけ。比較はpoll前に保ち、同先のEnter＋Fは既存worker/readyを維持し、異先のCは既存`replace_ready_folder_pane_open`へ渡す。

追補の独立Sol/xhighレビューは**ACCEPT**。poll前の同一宛先保護と異先のpending/ready置換、他nav優先・context所有・末尾visibilityの維持を確認し、重要な未解決指摘なし。レビュー側の編集・build/test・製品起動は行わず、検証は実装側が担当する。

最終検証（`MSBUILDDISABLENODEREUSE=1`、`target/tree-open-env.ps1`の既存native環境）:

| 検査 | 結果 | 記録（`target/`） |
|---|---|---|
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast` | **89成功/失敗0、19.994秒、exit0**。異先Cへの確定と既存の同先Enter＋Fを含む | `tree-pending-filter.log` |
| `scripts/test-focused.ps1 -Path @('src/app/folder_pane_refresh_tests.rs','src/app/update_phases.rs')` | **40/3件成功、計43実行（重複あり）、exit0** | `tree-pending-focused.log`、`test-focused/runs/20261103T052930687Z-f5b0208b` / `20261103T053349735Z-52587005` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit0** | `tree-pending-check.log` |
| `cargo test --test ui_snapshot folder_pane` | **4成功/失敗0、2.11秒、exit0**。画像更新なし | `tree-pending-snapshot.log` |
| 引数なし`cargo fmt`、`cargo fmt --check`、`git diff --check` | **各exit0** | `tree-pending-fmt-check.log`、終了時diff検査 |

新規warningなし（既存lib test warning153件）。指定により全lib・全snapshot・build-dev・製品起動・コミットは行わない。

## 12. §1.769 フォルダ名のダブルクリックで開閉する (2026-11-11 設計、ClaudeCode)

5ch >>764 の要望。ツリー行のフォルダ名 (label) のダブルクリックを、既存の ▶ / ▼ と同じ開閉処理 (`handle_tree_key(Left/Right)`) につなぐ。単クリックでそのフォルダへ移る動作 (`resp.clicked()` → pane navigation) は変えない。新しい列挙経路・新しい scan 要求は作らない。

### 12.1 egui のクリック回数と、守ること

- egui 0.37 は 2 回目のボタンを離した frame で、同じ label の `clicked()` と `double_clicked()` を両方立てる (`Click::count`、0.3 秒以内かつ 6pt 以内)。3 回目は `triple_clicked()`、それ以降の速い連打も count 3 のまま。行の高さは 24pt なので、別の行どうしで count 2 になるのは行境界の ±6pt に限られる。
- 守ること: (1) 1 回目のクリックは従来どおり即時に移動する (移動を 0.3 秒遅らせて単クリック確定を待つ案は、リリース済みの単クリックを遅くするので採らない)。(2) 同じ行の 2 回目・3 回目のクリックでは移動を出さない。読み込み中の同じフォルダに 2 回目の移動を出すと `start_folder_open_scan` が pending を取り消して scan をやり直すため。(3) 開閉はダブルクリック 1 回につき 1 回だけ。

### 12.2 開閉の向きは 1 回目のクリック時点の表示で決める

2 回目のクリック時点の表示を反転するだけ (状態を持たない案) では、現在地の祖先として自動で開いている枝をダブルクリックしたとき結果が速さで変わる。1 回目のクリックでその枝へ移る移動が 2 回目より先に終わると、`sync_to_active` の現在地変更で `auto_expanded` / `user_collapsed` が消え、その枝は閉じて見える (既存の挙動、現在地自身は自動展開しない)。2 回目で反転すると開き直ってしまい、閉じたかった利用者の意図と逆になる。移動が後に終わる場合は閉じたまま。

そこで、単クリックとして扱ったクリックの時点で「どの行を、開いていたか」を 1 件だけ記録し、同じ行のダブルクリックでは「1 回目の時点の逆」を目標にして、今の表示と違うときだけ既存の開閉を 1 回呼ぶ (目標どおりなら何もしない)。

- 記録: `FolderPaneState` に一時的な 1 件 (行の path、1 回目の時点の `row.expanded`、egui の時刻、開閉済みか)。保存しない。
- 単クリック扱い (count 1、または count 2/3 でも時間内の記録が無い): 従来の処理 (focus、cursor、現在地と違えば移動) をして記録を置き換える。
- **count 2 / 3 は、当たった行ではなく記録した行 (1 回目の行) に結びつける** (2026-11-12 改訂、サブ PC の実機確認 依頼 40 で判明)。1 回目のクリックは既存の作り (v5.1.0 から) で `set_cursor` が cursor の reveal を要求し、次のフレームで `scroll_to_me(Align::Center)` が行を中央へ寄せる (`animated(false)` なので一瞬)。人の手のダブルクリックの間 (0.1〜0.2 秒) に必ず動くので、egui が count 2 と判定する同じ画面位置の 2 回目は、中央へ来た別の行に当たる。また 1 回目の移動が完了すると、前の現在地の祖先の自動展開が閉じて、クリックした行より上の行が減ることがある (既存の作り)。どちらも画面上の行がずれるだけで、利用者の意図は 1 回目の行のダブルクリックなので、2 回目は当たった行を見ずに記録の行へ適用する。中央へ寄せる既存の動き自体は変えない (変えるかは利用者に別途確認する、案 A)。
  - これにより、行境界の ±6pt で「A をクリック→すぐ B をクリック」は egui 上 A のダブルクリックなので A の開閉になり、B へは移らない (r1 指摘 2 で入れた「B へ移って B を開閉」は廃止)。行の中央どうし (24pt 離れる) なら egui は count 1 なので、従来どおり B へ移る。
  - 記録の行が今の表示行に無い (親が閉じた等) 場合は、移動も開閉もせず、cursor も動かさない。
- 記録と一致する count 2 / 3 で、記録がまだ開閉済みでないとき: 移動しない。focus と cursor は記録の行。目標が「開く」で、記録の行が今閉じていて開ける (▶ と同じ条件 `row.expanded || row.has_children_or_unknown`。現状は常に開ける) なら `Right`、目標が「閉じる」で今開いていれば `Left`。今の開閉状態は当たった行ではなく記録の行のもの (`FolderPaneState` 内で求める)。それ以外は何もしない。記録を開閉済みにする。count 3 も含めるのは、時間内の別の count 1 の直後など、egui の count 3 が最初の一致になる並びを取りこぼさないため (独立レビュー r1 指摘 2 の名残)。
- 記録と一致する count 2 / 3 で、記録が開閉済みのとき: 移動も開閉もしない。
- 一致の条件 (2026-11-12 改訂で path の一致は外した): egui の count が 2 / 3、かつ記録の時刻 (直前の一致したクリックの時刻) から egui の `max_double_click_delay` の 2 倍以内。一致したクリックごとに記録の時刻を更新し、1 回目の時点の展開状態は保つ (更新しないと、0.25 秒間隔の 4 連打目で記録が期限切れになり単クリック扱いで移動を出し直す。独立レビュー r1 指摘 1)。▶ / ▼ をクリックしたら記録を消す (▶ と label の境界をまたいだ count 2 を label のダブルクリックと取り違えない)。
- 判定と記録は `folder_pane.rs` の純粋な関数に置き、UI 側は count・行の状態・時刻を渡して結果 (移動するか、どちらへ開閉するか) を受け取る。開閉は ▶ / ▼ と同じ helper を通す (開閉の書き手を 1 つにする)。

### 12.3 確かめる組み合わせ (handler-level test)

既存の `navigation_tree_frame` 形式で実際の pointer event を流す。

1. 閉じた枝 (現在地以外) のダブルクリック: 移動は 1 回 (1 回目の pending の取消 token が立たず、2 回目で新しい pending が作られない)、枝は開く。
2. ユーザーが開いた枝のダブルクリック: 移動 1 回、閉じる。
3. 現在地の祖先 (自動で開いている枝) のダブルクリック: 2 回のクリックの間に移動が完了する場合と、両方のクリックの後に完了する場合の両方で、最後は閉じている。
4. 現在地の行のダブルクリック: 移動なし、開閉 1 回。
5. 子の無いフォルダ (確定済みで空): §1.686 以降は空の枝にも常に ▶ / ▼ が出て開閉できる (`has_children_or_unknown` は常に true、外部で子が増えたときに開き直して確かめるため)。ダブルクリックも ▶ と同じく開閉 1 回 (子の行は出ない)、移動 1 回。空の枝だけ特別扱いしない。
6. 3 回の連打: 移動 1 回、開閉 1 回。
7. 0.3 秒を超えた 2 回のクリック: 従来の単クリック 2 回 (開閉なし)。
8. 行境界 ±6pt で別の行を続けてクリック: (2026-11-12 改訂) egui が count 2 なら 1 行目のダブルクリック (1 行目の開閉、2 行目へは移らない)。行の中央どうし (6pt 超) なら 2 行目へ普通に移る。
9. ▶ / ▼ のクリック、矢印キー・Enter の従来の動作 (既存のテストを含めて退行がない)。
10. (2026-11-12 追加) **ツリーがスクロールする高さ**で、中央にない行をダブルクリック: 1 回目の後に行が中央へ動き、同じ画面位置の 2 回目が別の行に当たることを実 egui の count 2 と行の位置で確認する。移動は 1 回目の行へ 1 回だけ (pending の取消・置換なし)、1 回目の行が 1 回だけ開閉し、2 回目に当たった行は移動も開閉もしない。3 回の連打も同じ。
11. (2026-11-12 追加) 1 回目の移動が 2 回目の前に完了し、前の現在地の祖先の自動展開が閉じて行が上へずれる場合: 1 回目の行が開閉し、ずれて当たった行へは移らない。

まれな失敗は既定の割り切り方針による (この変更には書き込み・永続化がない)。★固定中はペイン全体が disabled のまま。

### 12.4 状態を増やさない案の検討

- 単クリックの移動をダブルクリック判定まで遅らせる: 単クリックが毎回遅くなるリリース済み挙動の退行なので採らない。
- 移動完了で現在地の展開状態を保つように既存の展開規則を変える: 範囲外で、他の移動 (Ctrl+↑↓ など) の見え方も変わるので採らない。
- 上の 1 件の記録は、egui が count 2 以上と判定したときだけ参照し、利用者に見える状態や保存データにはならない。組み合わせは「単クリック扱い / 一致した count 2 / 一致した count 3」の 3 通りに限る。

### 実装記録 (2026-11-12、`bdd537a48` 上の未コミット差分)

| 項目 | 実装・確認方法 |
| --- | --- |
| §12.1 | label の `clicked()` 内で triple → double → single の優先順で回数を渡す。単クリックは従来の即時 navigation、一致した 2/3 回目は navigation を出さない |
| §12.2 | `FolderPaneState::handle_label_click` が path・初回 expanded・egui time・処理済みの一時記録 1 件を所有する。path は `folder_tree::path_eq`、時刻は実際の `max_double_click_delay` の 2 倍以内で照合。初回表示の逆を目標とし、現在表示と異なる時だけ Left/Right を返す |
| 開閉の共通化 | ▶ / ▼ と label の開閉を `apply_folder_pane_branch_click` へ合流。既存の focus/cursor と `handle_tree_key` を使う。矢印 click は label 記録を消す。空の枝にも同じ「開けるか」の条件を渡す |
| §12.3 | 純粋判定 5 件と実 pointer handler 10 件を追加。祖先への移動の前後完了を別試験とし、pending receiver を一時保持して cancel token / `Arc::ptr_eq`、最終現在地・枝の状態・子行を検査する。移動完了待ちは egui time を進めずに既存 navigation 裁定・採用を poll する |
| §12.4・範囲 | 記録は永続化しない。新しい列挙経路・scan 要求・repaint・delay・retry・UI 同期 I/O は追加しない。▶ / ▼・矢印キー・Enter の処理と disabled gate は維持 |
| 文書 | `docs/spec.md` と `manual/tut-navigation.html` を更新。公開ページを検索した範囲では、同じフォルダツリーのマウス操作を記述した他ページは見つからず、追加変更なし。backlog §1.769 の状態行は変更しない |

検証は Windows MSVC / default features。初回の TurboJPEG 自動 CMake build は exit 1 で停止し、Rust test build は exit 101 (試験開始前)。同じ crate 同梱ソースを本 worktree の `target/769-native/source` へ実コピーし、SIMD 有効・Release `/MT`・直列で静的 build した。`turbojpeg.lib` / `turbojpeg-static.lib` とヘッダを `target/769-native/` に配置し、検証プロセスだけ explicit library と通常形式の include path を使う (`target/769-env.ps1`)。製品の依存設定・build script は変更しない。準備途中の LibRaw library/header 配置不足による exit 101 は成功根拠に含めない。

途中の test compile は `InputState::options` が非公開のため exit 101。間隔の取得を公開の `Context::options` へ補正し、その後の core check は成功した。最初に実行できた追加 filter は **15件中12成功/3失敗、exit 100** (`769-added-tests-pass.log`)。祖先 2 件は arbitration fixture が意図的に subtree を drive として選び自動 reveal を抑止していた setup 不備、行境界 1 件は距離が egui の strict `< 6pt` の上限ちょうどになった setup 不備。祖先は既存の synthetic-ancestor/real-subtree backend を実 drive root で使い、境界は行内 0.1pt の点で strict 上限内に補正した。製品コードによる対処や仕様変更は行っていない。

次の追加 filter は **13成功/2失敗、exit 100** (`769-added-tests-final.log`)。実 drive の長い祖先経路を使ったことで、小さい setup viewport と pointer 用 viewport の間で cursor reveal の hit target が変わる fixture 不備が残った。クリック前に B の cursor reveal を pointer 用の 1600×600 viewport で消化させ、2 回目が B の実 response の `double_clicked()` に届いたことも検査するよう補正した。最終追加 filter は **15成功/失敗0、3.441秒、exit 0**。製品コードの追加変更はない。

| 検証コマンド | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/label_click/) \| test(/folder_pane_label/)' --no-fail-fast` | **15成功/失敗0、3.441秒、exit 0** | `769-added-tests-stable.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast` | **104成功/失敗0、24.803秒、exit 0** | `769-folder-filter.log` |
| `scripts/test-focused.ps1 -Path @('src/folder_pane.rs','src/ui_folder_pane.rs','src/app/folder_pane_refresh_tests.rs')` | **50 / 18 / 50 件すべて成功、計118実行 (重複あり)、exit 0** | `769-focused.log`、`test-focused/runs/20261111T155821843Z-9838ebb3` / `20261111T155831717Z-172044d5` / `20261111T155841206Z-15d1f67f` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit 0、4分07秒**。既存 warning あり | `769-check.log` |
| `cargo test --test ui_snapshot folder_pane` | **4成功/失敗0、1.37秒、exit 0**。基準画像更新なし | `769-snapshot.log` |
| 引数なし `cargo fmt` / `cargo fmt --check` / `git diff --check` | **各 exit 0** | `769-fmt.log` / `769-fmt-check.log` / `769-diff-check.log` |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 初回 **exit 101**、並列数 1 の再実行は **exit 0**。通常 features / dev-runtime で core・remote・EPUB worker・MCP・music-video editor の5本を生成。runtime 4本 / PE 5本の検査成功。起動なし | `769-build-dev.log` / `769-build-dev-serial.log`、`vcrt-pe-reports/dev-runtime.json` |

UI 文字列は変更していないため UI glyph 検査は対象外。全 lib gate は設計担当へ引き継ぎ、ここでは実行しない。製品バイナリ起動・実機操作・コミットは行わない。修正済み §12 からの設計変更はない。

確認用 build の初回は依存 `inventory` の rustc 起動が `0xc0000142 (STATUS_DLL_INIT_FAILED)` で失敗し、wrapper は exit 101。製品ソースの compile error ではない。source / feature / profile は変えず、同じ native 環境に `CARGO_BUILD_JOBS=1` を指定して一度再実行し成功した。core の Cargo build は依存込み41分50秒、remote 3分20秒。成果物は `target/dev-runtime/mimageviewer-core.exe` と上記 companion 4本。通常 `%APPDATA%/mimageviewer` profile を既定とする非 portable build であり、エージェントによる起動・既存 process の停止は行っていない。

### 独立レビュー r1 対応 (2026-11-12、`2f82b8b74` 上の未コミット差分)

設計担当が受理した P2 2件と、同 commit で改訂された §12.2 に対応する。

| 指摘 | 原因・修正 |
| --- | --- |
| P2-1：長い連打で同じフォルダの移動を再発行 | 記録時刻が最初のクリックのままで、0 / 0.25 / 0.50 / 0.75 秒の4回目を `Single` としていた。一致した count 2 / 3 のたびに既存の `record.time` を更新する。初回の `expanded` は保持する |
| P2-2：A→Bダブルクリックの B が開閉しない | egui の count は 1(A)→2(B)→3(B)。B の最初の count 2 は path 不一致で単クリック扱いとなるため、次の count 3 が B の開閉を担当する必要がある。回数の `count == 2` 制限を除き、一致したクリックのうち `!record.toggled` の最初の1回だけ既存の目標状態判定を行う。目標どおりで開閉不要の場合も処理済みにする |

変更は判定 owner の時刻更新と既存 `toggled` の利用だけ。UI renderer、共通の開閉 helper、path 同一性、開ける条件、単クリックの即時移動、scan / repaint / I/O / 永続化は変更しない。追加状態・新しい列挙経路はない。公開マニュアルの操作仕様はそのまま成立し、UI 文字列の変更もない。

製品コードを直す前に、新規の純粋判定2件と実 pointer handler 2件、および既存テストの count 列の補正を先に用意し、新規4件だけを選択して実行した。**4件実行 / 0成功 / 4失敗、exit 100、1.488秒** (`target/769-r1-red.log`)。純粋判定は 0.75秒で `Single` が返ることと、B の count 3 が `Repeated(None)` になることで失敗。handler は egui の count 列を確認した後、長い連打の4回目で最初の cancel token が立つことと、B の count 3 後も枝が閉じていることで失敗した。compile / fixture の準備失敗を再現根拠にしていない。この結果を確認してから製品コードを修正した。

| テスト | 検査内容 |
| --- | --- |
| `label_click_r1_long_sequence_keeps_navigation_suppressed` | 0 / 0.25 / 0.50 / 0.75 / 1.0秒、count 1→2→3→3→3。初回が開/閉の双方で、開閉は1回、以降は `Repeated(None)` を維持 |
| `label_click_r1_boundary_count_three_performs_the_second_rows_toggle` | A の count 1、B の count 2（単クリック扱い）、B の count 3（初回状態の逆を目標に開閉）、以後 count 3 は抑止。現在表示が既に目標どおりの場合も検査 |
| `folder_pane_label_r1_long_sequence_preserves_the_first_pending_open` | 実 PointerButton と RawInput.time で上記5連打と、最初からの経過が 2×delay を超えることを確認。各回で取消 token が立たず pending の `Arc::ptr_eq` が成立し、枝が1回だけ開く。元 receiver を戻して完了後の現在地・子行も検査 |
| `folder_pane_label_r1_boundary_then_double_click_opens_the_second_row_once` | 行境界の6pt未満の点で A→B→B をクリックし、実 egui count 1→2→3 を確認。A から B への移動を採用し、B の2回目は pending を置換せず開閉する。完了後の現在地と子行も検査 |

既存の `label_click_double_uses_first_display_and_toggles_only_once`、`label_click_triple_and_further_clicks_do_not_navigate_or_toggle`、`label_click_path_mismatch_or_timeout_starts_a_new_single`、`label_click_arrow_clear_breaks_the_label_sequence` の count 列も、先行クリックを踏まえた count 3 に合わせた。

全 cargo 処理（fmt を含む）は利用者指定の `powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\mikag\AppData\Local\miv-locks\run-heavy.ps1 -Owner "769-impl" -WorkDir G:\home\mimageviewer-769 -Command "<コマンド>"` 経由で順に実行した。同時に複数の重い処理は起動していない。共有の全 lib gate 待ちの後、red / green ともロック領域への書き込み不可の警告が出たため、指示どおり BelowNormal / `CARGO_BUILD_JOBS=6` で継続した。前回の native 環境 `target/769-env.ps1` は継承し、親プロセスでも並列数を6に指定した。feature / profile は既定、既存 warning あり。

| 検証コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| 修正前：`cargo nextest run -p mimageviewer --lib -E 'test(/label_click_r1/) \| test(/folder_pane_label_r1/)' --no-fail-fast` | **0成功 / 4失敗、exit 100** | `769-r1-red.log` |
| 修正後：同じ新規4件の filter | **4成功 / 失敗0、0.654秒、exit 0** | `769-r1-new-tests.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit 0、18.41秒** | `769-r1-check.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/label_click/) \| test(/folder_pane/)' --no-fail-fast` | **108成功 / 失敗0、10.949秒、exit 0** | `769-r1-regression.log` |
| `scripts/test-focused.ps1 -Path @('src/folder_pane.rs','src/app/folder_pane_refresh_tests.rs')` | **52 / 52件成功、計104実行、exit 0** | `769-r1-focused.log`、`test-focused/runs/20261111T181625877Z-bb563bd8` / `20261111T181629881Z-4ce92ecb` |
| 引数なし `cargo fmt` / `cargo fmt --check` / `git diff --check` | **各 exit 0** | `769-r1-fmt.log` / `769-r1-fmt-check.log`、最終diff検査 |

今回の `build-dev.ps1` と本体 lib 全件は利用者指定により設計担当へ引き継ぎ、実行しない。製品起動・実機操作・コミットも行わない。改訂された §12.2 からの設計変更はない。

### 12.5 再開時の設計: 2 回目は「押した瞬間」に、1 回目の位置と時刻で判定する (2026-11-13、ClaudeCode)

v5.2.0 の公開後に再開 (統括の指示 2026-11-13)。サブ PC の依頼 40 (サブ PC 担当の観測、画面操作ツールでのダブルクリック) で、今いる行以外では開閉しなかった。原因は 2 つ (設計担当がコードと依頼 40 のログで確認、backlog §1.769 にも記録)。

1. 1 回目のクリックで、カーソル行を中央へ寄せる動きが入り、同じ画面位置の 2 回目が別の行に当たった。**この動き自体は v5.2.0 で「マウスのクリックでは中央へ寄せない」(§13) に変わったので、マウスの 1 回目でツリーは動かなくなった。** ただし、移動の完了で前の現在地の祖先の自動展開が閉じ、行がずれることは残る。
2. 1 回目の移動でフォルダを開くと、サイドカー復元 (`src/app/sidecar_restore.rs`) が始まる。復元中は `input_update_begin` → `consume_input_during_sidecar_restore` が、毎フレームの入力から押下を捨て、egui のマウスの状態を初期化する (`input.pointer = PointerState::default()`)。依頼 40 のログでは、復元は移動の開始から約 25 ms 後に始まり、約 0.1 秒続いた。2 回目の押下がこの間に来ると消える。この後に来ても、egui はクリックの履歴を失っているので count 1 と判定する。

#### 方針 (設計レビュー r1〜r3 の指摘を反映、2026-11-14)

Windows のダブルクリック (`WM_LBUTTONDBLCLK`) は、**2 回目のボタンを押した瞬間**に、1 回目のクリックとの時間と位置で決まり、当たった行には左右されない。mIV もこれに合わせる。**ツリーの行の名前へのマウスの主ボタン操作は、1 回目のクリックも含めて、1 つの持ち主がフレームの入力の列を順に処理して「何が起きたか」を決める。** 決めた結果の適用 (focus・cursor・reveal 取消・移動の要求) は、今までの描画側の適用点で行う。描画中の `clicked()` (egui がフレーム全体で集約した値) では決めない。

**入力の列。** グリッドの主ボタン操作 (`src/ui_main.rs` の `GridPrimaryInput` / `GridPressWitnesses`) と同じ土台を使う: 生の `input.events` (押下・離し・`PointerMoved`・`WindowFocused(false)`・`Touch` の Cancel) を順に読み、各押下・離しを `ctx.pointer_button_targets()` の witness (前のフレームの部品への当たり・受付可否) と、離しを `input.pointer.released_buttons_with_click_counts()` (egui がクリックとして成立させたか。途中の移動超過・長押しも egui の判定どおり) と対応付ける。対応が取れなければその列は打ち切り。これらはサイドカー復元が入力を書き換える前に読む。

**持ち主 (`FolderPaneState` の型付きの 1 件、§12.2 の記録を置き換える。保存しない。)**

- `Idle`
- `FirstPress { path, pos }`: マウスの主ボタンで行の名前を押した (まだ離していない)。
- `AwaitingPress { path, first_expanded, pos, time, toggled }`: 1 回目のクリック (またはジェスチャ内の離し) の後で、次の押下を待っている。`pos` は 1 回目のクリックの位置、`time` は直前の離しの時刻。
- `PressAccepted { path, first_expanded, pos, toggled }`: ジェスチャの続きとして受け取った押下の離し (または取消) を待っている。
- 適用待ちの結果: 「1 回クリックが確定した行」と「開閉した行」を、列の順に並べた短い一覧。描画側の適用点で取り出して空にする。

前のフレームに描いたツリーの領域、行の名前と ▶ / ▼ の部品 id・矩形と path の対応は、描画のたびに作り直す一時的な表 (表示用の cache) として持つ。ツリーが描かれなかったフレームの後は使わない。

**処理する場所。** ROOT の入力処理の最初 (`input_update_begin`) で、サイドカー復元の入力破棄より前、かつフルスクリーン等の早期 return より前に 1 回呼ぶ。ROOT 以外の viewport では、ROOT の持ち主・表・結果を変更しない。ここでは focus・cursor・reveal・移動の要求を直接変えない (ダブルクリックの開閉だけは、表示状態の変更としてここで行う)。

**入力元。** タッチから egui-winit が作る主ボタンの押下・離しは、この持ち主の対象外にする (グリッドと同じ既存のタッチの相関で見分ける)。タッチのタップは今までどおり描画中の `clicked()` で 1 回クリックとして扱い、ダブルタップでの開閉はしない。タッチの Cancel、マウスとタッチの混在、主ボタン以外の押下は、進行中のジェスチャを打ち切る。

**列の処理 (順に):**

- **押下** (位置 p、witness の当たり c と受付可否):
  - p が前のフレームの ▶ / ▼ の矩形の内側なら `Idle` にする (矢印は描画中の今までの処理に任せる)。サイドカー復元中はツリーが disabled で当たり c が取れないので、矢印の判定は矩形で行う。
  - `AwaitingPress` で、`|p - pos| <= max_click_dist` (6pt) かつ `今 - time <= max_double_click_delay` (2 倍の許容はしない)、かつ「続きを受け取ってよい条件」を満たすなら、受け取る。まだ開閉していなければ、`first_expanded` の逆を目標に `path` の行を開閉する (今の状態が目標と違うときだけ)。結果の一覧に「開閉した行」を足し、`toggled` にして `PressAccepted` にする。c が別の行でも構わない (当たった行は見ない。§12.2 の案 B)。
  - そうでなく、c が行の名前で、受付可で、「新しいクリックを始めてよい条件」を満たすなら、`FirstPress { path: c の行, pos: p }` にする。
  - それ以外は `Idle` にする。
- **離し**:
  - `PressAccepted` なら `AwaitingPress { time: 今, .. }` に戻す。この離しはジェスチャの続きで、移動もカーソルの変更もしない。
  - `FirstPress` で、egui がその離しをクリックとして成立させ (`Some(click)`)、離しの当たりが同じ行で、**離しの時点でも**「新しいクリックを始めてよい条件」を満たす (押下と離しの間に前の移動が完了して復元が始まった等なら取り消す) なら、**1 回クリックが確定**: 結果の一覧に「1 回クリックが確定した行」を足し、`AwaitingPress { path, first_expanded: その行の今の表示, pos, time: 今, toggled: false }` にする。
  - それ以外 (ドラッグ・別の部品で離した等) は `Idle` にする。
- フォーカス喪失・タッチの Cancel は `Idle` にする。

列を順に処理するので、1 フレームに「1 回目の離し→2 回目の押下→2 回目の離し」や 2 回分のクリックがまとめて届いても、ふつうのフレームと同じ結果になる。▶ / ▼ や別の場所の押下が間に入れば、その時点で打ち切られる。

**描画側の適用点。** 今まで行の名前の `clicked()` を処理していた位置 (キー処理の後) で、結果の一覧を順に取り出して適用する。「1 回クリックが確定した行」には、従来の 1 回クリック (focus・cursor、§13 の reveal 取消、現在地と違えば同じ pane navigation の要求) を行う。「開閉した行」は既に開閉済みなので何もしない (focus・cursor も変えない)。行の名前の `clicked()` による処理は、マウスの主ボタン以外 (タッチのタップ・キーボード・AccessKit の活性化) に限って残す。これで、同じフレームのキー処理との順序は今までと変わらない。

**新しいクリックを始めてよい条件**: ツリーが表示中で disabled でない (★固定ではない)。今の表示形態でメイン画面を描く (前のフレームも描いた)。モーダル・入力停止が何も無い (サイドカー復元を含む)。当たり c が受付可 (egui の層・モーダルの判定を通っている)。

**続きを受け取ってよい条件**: ツリーが表示中 (サイドカー復元による一時的な disabled は除く。★固定は不可)。今の表示形態でメイン画面を描く (前のフレームも描いた)。サイドカー復元**以外**の停止理由が無い。押下位置が前のフレームのツリーの領域の内側。押下の当たり c が、ツリーの行の名前、当たり無し (ツリーの背景、または復元中で disabled のツリー)、またはサイドカー復元のモーダルに属する層・部品のいずれか (ポップアップ・メニュー・浮動ウィンドウ・ほかの部品なら打ち切り)。サイドカー復元のモーダルは、作成時の id との単純な一致ではなく、そのモーダルに属する層・部品として見分ける。

**停止理由の判定。** 今の `modal_dialog_block_reason` は最初に一致した理由の名前を返す診断用なので、許可の判定には使わない。同じ判定の表を共有し、サイドカー復元の項目だけを除いた「ほかの停止理由があるか」を返す関数を作る (表は 1 か所に置き、2 つの関数が同じ表を読む)。

**開閉の書き手。** `FolderPaneState` に「path と目標の展開状態だけを受け取り、展開集合と列挙要求だけを変える」処理を切り出し、▶ / ▼・キー (← →)・ダブルクリックが共有する。フォーカス・カーソル・reveal は、▶ / ▼ とキーの呼び出し側に今までどおり残す。

**打ち切り。** 上の列の処理に加えて、ツリーでのキー操作、ツリーのフォーカス喪失、ペインの非表示・★固定、ドライブ切替・再読み込みで `Idle` にする。1 回目のクリックで始まった移動の完了 (行のずれ・サイドカー復元) では打ち切らない。

- 復元中の開閉を許す根拠: 復元が止めるのは、復元対象の閲覧 context への操作と移動。ツリーの開閉はフォルダペイン自身の表示状態 (`FolderPaneState`) で、展開で始まる列挙はペイン専用の worker・channel・cancel・cache を使い、閲覧 context や DB の書き手に接続しない (設計レビュー r1 で確認)。復元中に新しいクリック (移動を伴う) は始めず、確定もしない。
- 行の境目の ±6pt で「A をクリック→すぐ B」は、A のダブルクリックになる。

**実装時の確認点 (設計レビュー r4 = ACCEPT の P3)。** 適用待ちの結果は、その ROOT の入力 pass 限りにする。キー処理などで表示形態が変わり描画側の適用点を通らなかったら、未適用の結果は捨て、次のフレームやフルスクリーン終了後へ持ち越さない (取り出しは 1 回だけ)。キーによる打ち切りは、入力開始の列の処理 (続きの押下の判定) より前に反映する。後段のキー処理で初めて `Idle` にすると、入力開始で既に開閉してしまう。「1 回目のクリック済み → 次の列が ↓ キー・押下・離し」と「キー処理の後に 1 回クリックの結果を適用したら、その新しいジェスチャが残る」を確かめる。

#### 採らなかった案

- 案 C (1 回クリックでの移動を 0.3 秒待つ): 確実だが、リリース済みの 1 回クリックの即時の移動が遅くなる。利用者の判断が要るので、上の方針で足りる限り採らない。
- 案 D (egui の回数に頼らず自前で判定するが、復元中の押下は捨てたまま): 速いダブルクリックほど 2 回目が復元中に入るので不確実。
- サイドカー復元の入力破棄に、ツリーの領域だけ例外を作る: 復元中にツリーの移動まで通ってしまう。上の方針は、破棄の前に「記録と一致する押下」だけを開閉として受け取り、破棄そのものは変えない。

まれな失敗は既定の割り切り方針による (書き込み・永続化なし)。

#### 確かめること (§12.3 に加えて。設計レビュー r1 の提案を含む)

- 実際のサイドカー復元を通す handler テスト: サイドカーを有効にした専用 fixture で、最初の実クリックから通常の移動・復元の開始まで通す。描画ハーネスに `input_update_begin` を含め、入力開始・描画・移動の採用・復元の poll を実際の順序で回す。`SidecarCheckingRelay` で worker の受信だけを保留し、固定時刻で次を流す: 復元中の押下と離し、復元をまたぐ押下と離し (押下は復元中・離しは復元後)、復元の後の押下と離し (egui の履歴が消えた状態)。いずれも開閉 1 回・移動 1 回。取消 token・復元 request・フォーカス・カーソル・reveal・キーボード所有が変わらず、ほかの入力の破棄も変わらない。
- 移動の完了で行がずれた後の 2 回目 (当たった行が別) でも、1 回目の行が開閉する (既存の祖先が閉じるシナリオを使う)。
- 1 フレームに 2 回分のクリック (押下・離し・押下・離し) が届く場合: 移動 1 回・開閉 1 回。
- 同じフレームの並び (r2 の指摘): 「前のフレームで 1 回目の押下 → このフレームで 1 回目の離し・2 回目の押下・2 回目の離し」は移動 1 回・開閉 1 回。「▶ の押下・離し → 6pt 未満の名前の押下・離し」は矢印の開閉と 1 回クリックだけ (名前のダブルクリックにしない)。「A の続きの押下・離し → 離れた B の押下・離し」は A の開閉と B への 1 回クリック (B を抑止しない)。
- サイドカー復元と、ほかの停止理由 (例: 設定ダイアログ) が同時にあるときは、続きを受け取らない。
- r3 の指摘: 押下→6pt 超の移動→元の位置へ戻って離す (egui がクリックにしない) は 1 回クリックにしない。押下の後・離しの前に前の移動が完了して復元が始まったら、その離しで移動を出さない。復元中 (猶予の 100 ms 中・モーダル描画後の両方) に名前から 6pt 未満で ▶ / ▼ を押したら打ち切る。タッチのタップは今までどおり 1 回クリックで、ダブルタップは開閉しない。同じフレームに「↓ キー → B の名前のクリック」が届いたら、今までどおりキー処理の後に B へ確定する。
- 押下で受け取り、離しが 1 回目の位置から 6pt を超える (例: 1 回目 x=0、2 回目の押下 x=4、離し x=8) 場合と、長く押してから離す場合: 移動を出し直さない。
- 判定しないとき: 6pt を超える位置、間隔が delay を超える (既存の間隔超過テストと同じく 2 回の移動)、名前から ▶ / ▼ へまたぐ押下 (矢印の開閉だけ)、ほかのモーダル・ポップアップ・浮動ウィンドウが重なる位置、ツリー非表示、間にキー操作や別の場所のクリック。
- 今いる行のダブルクリック、▶ / ▼、キー操作、1 回クリックの即時の移動、§13 (クリックで中央へ寄せない) が変わらない。
- 途中で残した req40 のテストは、§13 で「1 回目で中央へ寄る」前提が成り立たないので、中央へ寄らない状態での続きの確認と、移動完了による行のずれの確認に分けて作り直す。

#### 実装前提の照合で停止 (2026-11-14、`e1dfa0008`)

製品コードの変更前に、上の「▶ / ▼ の処理は描画中の従来処理へ任せる」と、同一 ROOT pass の「▶ の押下・離し → 6pt 未満の名前の押下・離し」で矢印の開閉と名前の単クリックの両方を要求する条件が両立しないことを確認した。実装依頼の「設計の前提がコードと食い違う場合は実装を止めて報告する」に従い、設計を勝手に拡張せず停止する。

- `vendor/egui/src/context.rs` は最終ポインター位置から `viewport.hits` を1回作る。`interaction.rs` は同じ `hits.click` を各押下へ使い、フレームのクリック先を単一の `clicked: Option<Id>` に集約する。最後が名前なら、矢印の `Response::clicked()` は成立しない。
- `pointer_button_targets()` は各 edge の前フレームの当たりを別々に記録する観測 API であり、集約された `Response` を変えない。名前の持ち主だけを入力列へ移しても、矢印の処理は復元できない。
- 独立の `gpt-6.1-sol` / `xhigh` にこの前提だけを照合してもらい、同じ矛盾と、実装の通常の裁量では解消できないことを確認した。設計の再 ACCEPT や実装の完了レビューを意味しない。
- 設計担当へ戻す論点: 矢印の主マウス操作も順序付き入力・適用結果へ含めるか。その場合の開閉の実行時点、focus・cursor・reveal取消、キー処理との順序、従来の `clicked()` との二重処理防止を明記する必要がある。

製品コード、`docs/spec.md`、`docs/keymap-spec.md`、マニュアル、backlog は変更していない。テストの差分は既存ハーネスへの `input_update_begin` 呼出しと、前提検証1件だけ。実装途中の草稿・テスト準備は `target/769-gesture-*.txt`、`target/769-input-method.txt`、`target/769-handler-tests-draft.rs` に保持する。本体 lib 全件・`build-dev`・`ui_snapshot`・製品起動・コミットは実施していない。

前提検証: `folder_pane_label_ordered_arrow_then_nearby_label_is_single` を、製品コードが HEAD のまま実行した。1 pass に矢印の押下・離し、名前の押下・離しを4ptの距離で渡し、4 edge の witness が矢印・矢印・名前・名前、全て受付可、両者の id が別、矢印の矩形内であることを先に検査する。これらは通過し、「矢印クリックで枝が開く」の検査で失敗した。**1件実行 / 0成功 / 1失敗、exit100、実行0.706秒**。ログ: `target/769-arrow-design-premise.log`。コマンドは指定の `run-heavy.ps1 -Owner "769-impl"` 経由で `cargo nextest run -p mimageviewer --lib -E 'test(=app::folder_pane_refresh_tests::folder_pane_label_ordered_arrow_then_nearby_label_is_single)' --no-fail-fast --retries 0 --no-tests fail`。Windows MSVC / default features / test profile、コンパイル2分59秒、TurboJPEG CMake失敗なし。

先行した実サイドカー復元テスト3件は2回とも Checking worker の受信前の Quiescing で止まった。これはハーネスの成立未確認であり、依頼40の不具合を修正前に再現した証拠には扱わない (`target/769-red.log`、最終試行3件失敗)。設計前提の矛盾を確認したため、そのハーネスの修正と製品実装は継続していない。

停止時の整形検査: 指定の `run-heavy.ps1` 経由の引数なし `cargo fmt`、`cargo fmt --check` は各 exit0 (`target/769-premise-fmt.log`、`target/769-premise-fmt-check.log`)。`git diff --check` も exit0。変更は上記テストファイルと本節の2ファイルのみ。実装停止のため、関連 filter 全体・`test-focused.ps1`・core の `cargo check` は未実施であり、実装や回帰全体の成功を示すものではない。

#### 設計の改訂 r4・r5: ▶ / ▼ も同じ持ち主で扱い、開閉はすべて適用点で行う (2026-11-14、ClaudeCode)

上の停止報告は妥当 (egui はフレームのクリック先を 1 つに集約するので、名前だけを入力の列へ移すと、同じ pass の ▶ / ▼ のクリックが失われる)。§12.5 の方針を次のとおり改める。以下が、これまでの「矢印は描画中の今までの処理に任せる」「タッチは対象外」「ダブルクリックの開閉は入力開始で行う」を置き換える (r5 は r4 への設計レビューの P2 2 件の反映)。

- **ツリーの行 (名前と ▶ / ▼) へのポインタの主ボタン操作は、マウスもタッチも、すべて入力開始の持ち主が列の順に認識する。入力開始では状態を変えず、認識した結果を列の順に一覧に積むだけにする。** 結果は 3 種類: 「1 回クリック (path)」「▶ / ▼ のクリック (path)」「ダブルクリックの開閉 (path、目標の展開状態)」。ダブルクリックの目標は `first_expanded` の逆で、判定時点では開閉しない。
- **描画側の適用点で、結果を列の順に 1 つずつ適用する。** 「1 回クリック」は従来の 1 回クリック (focus・cursor、§13 の reveal 取消、現在地と違えば移動の要求)。「▶ / ▼ のクリック」は今までの矢印の処理 (focus・cursor、適用時点の行の状態で開いていれば閉じる・閉じていて開けるなら開く、§13 の reveal 取消)。「ダブルクリックの開閉」は、適用時点の状態が目標と違うときだけ目標へ揃える (focus・cursor・reveal は変えない)。開閉の書き手は共通の「path と目標の展開状態」処理。適用はキー処理の後で、今までと同じ順序になる。これで、同じ pass に「▶ のクリック → 名前のダブルクリック」が届いても、▶ の開閉の後に、名前のダブルクリックの目標へ揃う (r5 の指摘 1)。
- サイドカー復元中も、描画側の適用点は通る (ツリーは disabled で描かれる)。復元中に積まれ得る結果は「ダブルクリックの開閉」だけで、適用はペインの表示状態だけを変える。表示形態が変わり適用点を通らなかった pass の結果は捨てる。
- **キーボード・AccessKit の活性化は部品ごとに見分ける (r5 の指摘 2)。** egui の `Response::clicked()` は、ポインタのクリック (`clicked_by(Primary)`) とキー (Space / Enter)・AccessKit の活性化 (`FAKE_PRIMARY_CLICKED`) の和。描画側は各部品について `clicked() && !clicked_by(Primary)` (ポインタ以外の活性化) のときだけ従来の処理をする。ポインタのクリック (`clicked_by(Primary)`) は持ち主の結果で扱うので、描画側では処理しない。同じ部品にポインタのクリックとポインタ以外の活性化が同じ pass に重なったときは、持ち主の結果だけを使う。pass 全体の有無では判定しない。
- 持ち主の状態に `ArrowPress { path }` を足す。押下の当たりが ▶ / ▼ の部品で、受付可で、「新しいクリックを始めてよい条件」を満たすなら `ArrowPress` (名前のジェスチャはここで打ち切り)。サイドカー復元中などで当たりが取れない場合は、矩形で ▶ / ▼ と分かれば `Idle` (今までどおり、復元中に矢印は効かない)。離し: `ArrowPress` で、egui がクリックとして成立させ (`Some(click)`)、離しの当たりが同じ矢印で、離しの時点でも「新しいクリックを始めてよい条件」を満たすなら、「▶ / ▼ のクリック」を積んで `Idle`。それ以外は `Idle`。
- タッチ: 1 回のタップは名前なら「1 回クリック」、▶ / ▼ なら「▶ / ▼ のクリック」として同じ一覧に積む。ただしタッチではダブルクリックのジェスチャを始めない・続けない (タップの離しの後は `Idle`)。タッチの Cancel・マウスとタッチの混在は打ち切り。入力元の判定には既存のタッチの相関 (`src/touch_correlation.rs`) を使うが、グリッドのジェスチャの駆動 (`drive_egui_grid_touch_input` の scope や初回の command) を奪わない形で共用する。ツリーの通常の入力では pointer の状態を消費・初期化せず、ScrollArea のドラッグは今までどおり (設計レビュー r5 の P3)。
- 結果と、部品ごとの重複の判定は、ROOT のその pass 限り。取消・ドラッグ・受付不可になった入力で、ポインタ由来の `clicked()` を従来の経路へ戻さない (`clicked_by(Primary)` は描画側で常に処理しない)。早期 return の pass の結果は次の pass へ持ち越さない (設計レビュー r5 の P3)。
- これにより、同じ pass の「▶ のクリック → 6pt 未満の名前のクリック」は、▶ の開閉と名前への 1 回クリックの両方になる (v5.2.0 では egui の集約で片方だけになり得た)。

#### r4・r5 の実装記録 (2026-11-14、`4e856f29b` 上の未コミット差分)

- `FolderPaneState` の旧 count 記録を、名前・矢印・マウス・タッチの型付きジェスチャへ置換。ROOT の入力開始で、変更前のイベント列・edge ごとの witness・egui の release 判定を対応付け、Single / Arrow / Toggle の結果を積む。ここでは展開・focus・cursor・reveal・移動を変えない。部品表・結果・部品別の pointer 活性化記録は ROOT の pass で区切り、未描画の表や未適用結果を次へ持ち越さない。
- 描画側ではキー処理後に結果を列順で適用する。`set_branch_expanded(path, target)` は展開集合とペインの列挙要求だけを変更し、矢印・← / →・ダブルクリックで共有する。ダブルクリックは初回表示の逆を目標とし、移動・focus・cursor・reveal を再発行しない。Single / Arrow は従来の操作と §13 の reveal 取消を行う。部品ごとの非 pointer 活性化は残し、同じ pass の pointer との二重適用だけを抑止する。
- サイドカー以外の停止理由は、既存の modal 診断と同じ表を読む。サイドカーの例外はその modal に属する実 widget の layer で区別し、単純な widget id 一致には依存しない。入力の破棄そのものは変更していない。独立実装レビューの P2（認識後にモーダルが始まった場合）に従い、適用点でも Single / Arrow は全停止理由、Toggle はサイドカー以外の停止理由を再確認して拒否・退役する。
- タッチの出どころは既存の相関処理を、専用の観測状態で共用する。グリッドの scope・初回 command・入力列・pointer 状態は消費しない。名前のタップは Single、矢印のタップは Arrow にし、タッチでダブルクリックを開始・継続しない。別々の pass の相関と、観測後もグリッドの初回 command が届くことを回帰で確認する。
- 通常の `set_focus_grid` は未確定 press も含めてジェスチャを終了する。既に入力列の前に打ち切ったキーボード Enter の後段だけは、別の focus 解放入口を使い、同じ pass の新しい名前クリックを失わない。固定キーボードの操作は入力列より前に打ち切りを反映する。ゲームパッドの step 経路は今回変更していない。独立レビューの静的所見（名前 single → PadDown → 時間内のマウス press では旧ジェスチャを継続し得る）は、物理キーボード以外を追加の打切り条件とするかの設計担当への引継ぎとする。

**製品変更前の実復元回帰:** `folder_pane_label_sidecar_second_press_during_restore` / `across_restore` / `after_restore` の3件を、製品が HEAD のまま実行し、全件が実移動の採用・実 Checking worker の relay 受信保留を通った後、「2回目の押下で初回行が開く」の製品 assert で失敗した。**3件実行 / 0成功 / 3失敗、exit100、実行0.839秒、コンパイル2分27秒**。証跡は `target/769-resume-red.log` と `target/769-resume-red-tests-source.rs`。指定の `run-heavy.ps1 -Owner "769-impl"` 経由の nextest、Windows MSVC / default features / test profile。前回・再開直後の Quiescing timeout は根拠に含めない。実 root の worker / writer 結果受信 checkpoint の不足を補完して fixture が成立したが、busy だった個別 writer は特定していない。

TurboJPEG の初回 core check は CMake / MSBuild の6並列 install がエラー本文を出さず exit1 となり、Cargo は exit101（`target/769-r5-check-initial.log`）。同じ source / Debug / CRT 設定の verbose 再現も「0警告・0エラー」のまま失敗した（`target/769-r5-cmake-diagnostic.log`）。同じ native 出力への直列 install は成功、2分48秒（`target/769-r5-cmake-diagnostic-serial.log`）。6並列の失敗と直列の成功を確認したところまでで、それ以上の原因は未特定。依存ソースや Cargo 設定は変更せず、以降の core check は `-j 1` とする。旧 `target/769-env.ps1` の作成・流用はしていない。

本体 lib 全件・`build-dev`・`ui_snapshot` は指定どおり設計担当の所有。製品起動・実機操作・コミットは行わず、backlog は変更しない。以下に最終の自動検証結果を追記する。

初回の関連 filter は **244件中180成功 / 64失敗、exit100**（`target/769-r5-related.log`）。headless の既存ハーネスは、起動索引の `NotStarted`（`overlay_active() == true`）を終端しないままツリーを直接描いていたため、新しい「今もメイン画面を描く」条件が正しく入力を拒否した。起動後の画面を対象とする fixture の初期化だけで `NotStarted` を `Unavailable` へ進め、製品の入力許可は弱めない。直接 tree を描く UI unit にも入力開始・表示完了の checkpoint を加えた。

結果の先行適用で子行が挿入されても AccessKit の宛先が別行へ移らないよう、行全体の `scope_id` を正規化した path に固定した。egui 0.37 の通常の `push_id` は親の auto 順序を `unique_id` に混ぜるため、この用途では使わない。子 cache を開いて温めてから閉じ、次行 B の id を取得して、同 pass の矢印展開・AccessKit B 活性化で B の画面位置が動いても id と移動先が B のままである回帰を追加した。これは入力結果の順序を変えず部品 identity の生成境界を正す実装として、独立レビューとも照合した。

混在取消は、同じ入力列の mouse / touch の混在だけでなく、未 release の FirstPress / ArrowPress に異なる入力元の press が来た場合、PressAccepted 中に touch press が来た場合も既存の typed state から判定する。新しい保持フラグは足さず Idle へ退役し、確定済み結果の prefix は維持する。両方向・名前 / 矢印の組合せを跨 pass の純粋回帰でも確認する。

2回目の関連 filter は **247件中198成功 / 49失敗、exit100**（`target/769-r5-related-2.log`）。多くは入力前の label 可視前提で止まった。最小1件の診断（`target/769-r5-geometry-diagnostic.log`）では B の読取矩形が `[[48,98],[178,122]]`、最新 viewport が `[[8,143.4],[178,598]]`、offset は0だった。vendor の `end_pass` は `prev_pass` / `this_pass` を swap し、`run_ui` 終了後の `read_response` は古い `this_pass` の同じ id を先に読むため、stable id 化で以前の toolbar 無し画面の矩形が優先されていた。レイアウトを推測で変えず、cfg(test) で実描画中の `Response.rect` を記録し、frame 後の幾何検査・クリック座標はその最新矩形を読むように補正した。id・witness・入力列は実物のまま。

独立レビューの追加指摘に従い、`widget == None` をツリー背景の証明にしない。クリック部品を持たない hover-only Area / DRAG-only Window 本文も None になり得るため、実描画の tree layer を cache に記録し、続きの押下全体を `layer_id_at(pos)` が None / tree / sidecar modal の場合に限る。上位 layer と witness None の前提を先に確認する回帰2件と、overlay 退役後の独立 single の継続確認を追加した。

3回目の関連 filter は **249件中240成功 / 9失敗、exit100**（`target/769-r5-related-3.log`）。残った試験の前提を照合し、製品の受付条件や入力破棄は緩めず、次を補正した。取消 token を比較する試験だけは最初の移動の receiver を保留し、正常採用との競走を除く。非表示から再表示した後と req40 の祖先展開は、Suspended の `has_pending == false` を完了とせず、実描画で resume / scan / poll を通して待つ（egui 時刻は固定し、続きが delay 内であることも検査）。実復元中の入力破棄は既存の release edge 保持契約を検査し、Text / Wheel / press が残らず pointer と scroll が初期化されることを確認する。連続タッチは実 backend と同じ End → primary release → PointerGone の列にした。

`docs/spec.md` と `docs/keymap-spec.md` を初回行への結び付き・押下時の確定と描画側適用・タッチの対象範囲に合わせた。マニュアル `htdocs/mimageviewer/manual/tut-navigation.html` の「ダブルクリックで枝を開閉、1回目で移動」の一文はこの実装とも一致するため変更しない。

**最終検証:** Windows MSVC / default features / test・dev profile。すべて指定の `run-heavy.ps1 -Owner "769-impl"` 経由。外部 lock 領域への書込み不可の警告に従い BelowNormal / `CARGO_BUILD_JOBS=6` で実行し、native の直列再確認後の core check だけは `-j 1` を明示した。

| コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/) \| test(/label_click/) \| test(/sidecar_restore/)' --no-fail-fast --retries 0 --no-tests fail` | **249成功 / 失敗0、21.022秒、exit0**。修正前に失敗した実復元3件、改訂後の矢印→近傍名前の前提検証、req40置換、§13の既存回帰を含む | `769-r5-related-final.log` |
| `scripts/test-focused.ps1 -Path ...`（変更したテスト保有Rust 6ファイル） | **66 / 18 / 4 / 57 / 110 / 46件の6群すべて成功、計301実行、exit0**。関連filterとの重複を含む | `769-r5-focused-final.log`、`769-r5-focused.ps1`、`test-focused/runs/20261113T171450879Z-b2b2a889` から `20261113T171534946Z-f3f852f5` までの各実行（各directoryはログ参照） |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 1` | **exit0、1分25秒** | `769-r5-check-final.log` |
| 引数なし `cargo fmt` / `cargo fmt --check` | **各exit0** | `769-r5-fmt-final.log` / `769-r5-fmt-check-final.log` |
| `git diff --check` / 検証前後のRust SHA-256照合 | **各成功** | `769-r5-diff-check-final.log` / `769-r5-verified-source-hashes.json` |

focused の指定順は `src/folder_pane.rs`、`src/ui_folder_pane.rs`、`src/app/update_phases.rs`、`src/app/sidecar_restore.rs`、`src/app/folder_pane_refresh_tests.rs`、`src/touch_correlation.rs`。共有の `src/app.rs` は focused script が全libへ送る broad path のため、全libを実行しない今回の分担では指定しない。`src/ui_dialogs/sidecar_restore.rs` の modal ID 共用には独立の同moduleテストがなく、実modalを描く復元handler回帰で確認した。共有停止表・表示形態判定は関連handler / core checkでも確認し、全lib・build-dev・ui_snapshotは依頼どおり設計担当へ引き継ぐ。

独立 Sol / xhigh の実装静的レビューは、モーダル再確認・layerの境界・stable widget ID・最終fixture補正を含め ACCEPT（自動テストは上記を実装担当が実行）。HEADは `4e856f29b` のまま、変更は未コミット。製品起動・実機入力・backlog変更は行っていない。

#### 独立コードレビュー P2・P3 の修正（`6f5791d1e` 上の未コミット差分）

P2 の原因は、同じ pass の名前クリック結果を順に適用する際、現在地への Single が先行する移動候補を上書きしないことだった。現在地 A で B → A の順に単クリックすると cursor は A に戻るが、B の `Some(B)` が後段の移動裁定へ残る。名前の各 Single で `Some(path)` / `None` の両方を代入して候補を置換する。同じ形の非 pointer 活性化（キー / AccessKit）も揃え、Arrow / Toggle は直前の Single の移動候補を消さない既定のままにする。

製品ファイルが `6f5791d1e` のまま、次の2件を新しい実 handler 回帰として実行した。ポインター版は 6pt より離れた B / A の押下・離し4 edgeを同じ ROOT pass に渡し、4 witness が B / B / A / A で受付可、release 2件がともにクリック成立、cursor が A であることを先に確認する。AccessKit 版はポインター B のクリックと A の Click 活性化を同じ pass に渡す。**両方とも上記前提を通過し、移動候補 `Some(B) != None` の製品 assert で失敗**した。各1件実行 / 0成功 / 1失敗、exit100、実行0.613秒 / 0.653秒。ログは `target/769-review-p2-red.log` / `target/769-review-accesskit-red.log`、その時点のテストソースは `target/769-review-red-tests-source.rs`。最初の実行は先行の全件テストのロックを待ち、迂回しなかった。

P3 は読み取り条件の共通化に限定する。detached の述語・状態遷移を変えず、描画側の後始末や描画前後の保持位置も維持する。触れた範囲は `docs/detached-rework-plan.md` §11 に追記する。

| 既存の描画条件 | 共通の読み取りと呼び出し対応 | 維持する描画処理 |
| --- | --- | --- |
| native backdrop | 既存 `native_video_fullscreen_active_for_main_backdrop()` を入力と描画の両方から利用（元から共通） | backdrop 側の subclass / chrome / cloak / navigation / return を変更しない |
| embedded still | 既存 `fullscreen_embedded_still_active()` を入力と描画の両方から利用（元から共通） | 描画直前 active の capture と、描画後 active との OR を変更しない |
| in-window かつ deferred reopen 待ち | 描画側の式 `native_video_in_window_active && fs_nav_deferred_reopen_wait_active()` を `main_embedded_fullscreen_reopen_pending(&self)` へ移す。入力、描画前 poll、描画前 pending capture、`render_fullscreen_viewport_with_root_input` の holdover 描画の計4箇所が利用 | 各呼び出しの位置、pending の描画前 capture と後段 gate を維持 |
| still viewport enter suppression | `still_fullscreen_viewport_enter_suppressed_now(&self)` が fullscreen_idx / in-window / 期限の真偽だけを読む。入力と既存の mut 版 `still_fullscreen_viewport_enter_suppressed` が利用 | `until=None` の即 return と、invalid context / expired それぞれの deadline・PresentationSwitch holdover 破棄を元の分岐・位置のまま維持 |

新しい条件・状態・時間窓はない。上のコード対応により、共通化前と同じ入力を同じ時点で描画条件へ渡すことを確認する。読み取りだけの ROOT 入力は deadline / holdover を消さず、既存の mut 版だけが元の描画 checkpoint で後始末を行う。

修正後の検証は Windows MSVC / default features / test・dev profile。Cargo のビルド・テスト・整形はすべて `run-heavy.ps1 -Owner "codex-769" -WorkDir G:\home\mimageviewer-769` 経由。先行全件テストの終了後は、lock 領域への書込み不可の警告に従い BelowNormal / `CARGO_BUILD_JOBS=6` で実行した。core check は前回の native 検証と同じく `-j 1` を明示した。

| コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/) \| test(/label_click/) \| test(/sidecar_restore/)' --no-fail-fast --retries 0 --no-tests fail` | **251成功 / 失敗0、22.655秒、exit0**。修正前に失敗した新2件を含む | `769-review-related.log` |
| `scripts/test-focused.ps1 -Path src/ui_folder_pane.rs,src/app/folder_pane_refresh_tests.rs,src/ui_fullscreen.rs`（配列で指定） | **18 / 112 / 1,359件の3群すべて成功、計1,489実行、exit0**。1.391 / 19.601 / 152.298秒。関連filterとの重複を含む | `769-review-focused.log`、`769-review-focused.ps1`、ログに記録した `test-focused/runs/*` の各結果 |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 1` | **exit0、54.90秒** | `769-review-check.log` |
| 引数なし `cargo fmt` / `cargo fmt --check` | **各exit0** | `769-review-fmt.log` / `769-review-fmt-check.log` |
| `git diff --check` / 検証前後のRust SHA-256照合 | **各成功** | `769-review-diff-check.log` / `769-review-source-hashes.json` |

focused の `src/app.rs` は前回同様 broad path（全lib指定になる）のため除外し、共有判定は上のコード対応・独立静的レビューと変更した呼び出し元モジュールの回帰・core checkで確認した。独立 Sol / xhigh の P2・P3 フォローアップ静的レビューは **ACCEPT**、追加findingなし。HEADは `6f5791d1e` のまま未コミット。本体全lib・ui_snapshot・build-dev・製品起動は指定どおり実施せず、backlogにも触れていない。

#### 全libで見つかった8件の調査と設計境界での停止（`b01467f52`）

利用者の8件を、製品・テストが HEAD のまま指定の `run-heavy.ps1 -Owner "codex-769"` 経由で再実行した。**8件実行 / 0成功 / 8失敗、1.004秒、exit100**。失敗箇所は提供ログと一致する。証跡は `target/769-harness-red.log`。実行は次の nextest filter に限定し、全libは実行していない。

`cargo nextest run -p mimageviewer --lib -E 'test(=app::folder_pane_refresh_tests::folder_pane_label_arrow_and_keyboard_keep_existing_behavior) | test(=app::main_chrome::tests::main_chrome_folder_tree_clicks_follow_common_modal_ownership) | test(/search_nav_pane_frame_/)' --no-fail-fast --retries 0 --no-tests fail`

本番の描画入口は `App::ui → update_frame → input_update_begin → render_main_bars → render_folder_pane → render_folder_pane_tree`。`input_update_begin`（`src/app.rs:100977`）は起動overlay・native backdrop・fullscreen等の早期 return より前にあり、`App::ui` の `finish_folder_pane_presentation`（同102268）は早期 return の後も通る。child viewport は eframe が個別callbackへ送り `App::ui` を呼ばず、tree 自体にも ROOT guard がある。**入力開始を通らずツリーを描く本番入口は見つからなかった。**

ただし、**本番でメニュー外のツリークリックを取りこぼす別の不一致を source 上で確認したため、依頼の停止条件に従って修正を停止する。** 独立 Sol / xhigh の読み取り専用監査も同じ結論だった。

- `main_chrome_pointer_blocked()` は既存の共有停止表を `include_menus=false` で読み、グリッドメニュー外のポインター操作を許す。`main_chrome_modal_classification_keeps_menu_tools_and_hidden_scans_distinct` も GridMenu を common-modal ではあるが pointer chrome の blocker ではないと明示している。
- 一方、ツリーの入力開始（`src/ui_folder_pane.rs:300`）と Single / Arrow の適用時判定は `modal_dialog_block_reason()`（`include_menus=true`）、継続判定も同じ true 側を使う。共有表でこの引数が変えるのは `grid_context_menu` の1項目である。
- 本番の順序は入力開始（`src/app.rs:100977`）→キー（101151）→メニュー描画・退役（`show_context_menu`、101600）→ツールバー（101614）→main bars / tree（101658）。メニュー外の press / release は、入力開始ではまだ menu owner があるため FirstPress / ArrowPress を作らず、その後 `show_context_menu` の外側クリック処理が `context_menu_idx=None` にする（`src/ui_dialogs/context_menu.rs:1216–1227`）。描画時に chrome が許可状態へ戻っても適用結果がなく、Primary の旧 `clicked()` は意図的に除外されているので、従来は届いたクリックが失われる。
- 稀な native メニュー失敗だけの問題ではない。ブックマークのグリッドメニューは専用の egui Window を使い、読書履歴ビューも通常から native メニューを飛ばして egui メニューを使う（同ファイルの `items_are_bookmark_view` / `!items_are_reading_history_view` の分岐）。どちらもメニュー外クリックで退役する通常経路がある。実機起動・操作はしておらず、これは本番コードの順序と条件による確認である。

設計担当へ戻す論点は、§12.5 の新規開始・継続・適用時の停止理由を、既存の pointer chrome と同じ「メニュー外は操作可」にどう揃えるか。メニューが重なる位置の witness / layer による拒否と、メニューが存在するだけで全ツリーを拒否する判定を区別する必要がある。これはハーネスだけの修正では解消しないため、今回その場で製品条件を変更しない。

8件について確認できた範囲は次のとおり。クリックの期待は書き換えていない。

| テスト（`app::` 以下） | 確認できた失敗・前提 |
| --- | --- |
| `main_chrome::tests::main_chrome_folder_tree_clicks_follow_common_modal_ownership` | callback に入力開始・表示完了がない。最初の Blocker::None で要求0件のまま失敗し、GridMenu ケースには未到達。入力開始を補っても上記の本番不一致が残る |
| `search_nav_pane_frame_tests::search_nav_pane_frame_metadata_enter_opens_tree_target` | 共通 `main_frame` に入力開始がなく、実矢印クリックによる tree focus 取得（168行）で失敗 |
| `search_nav_pane_frame_tests::search_nav_pane_frame_everything_enter_opens_tree_target` | 同じ共通frame・同じ失敗箇所 |
| `search_nav_pane_frame_tests::search_nav_pane_frame_favorite_search_enter_opens_tree_target` | 同じ共通frame・同じ失敗箇所 |
| `search_nav_pane_frame_tests::search_nav_pane_frame_tag_enter_opens_tree_target` | 同じ共通frame・同じ失敗箇所 |
| `search_nav_pane_frame_tests::search_nav_pane_frame_plain_enter_opens_tree_target` | 同じ共通frame・同じ失敗箇所 |
| `search_nav_pane_frame_tests::search_nav_pane_frame_enter_preserves_modifiers_keypad_and_coalescing` | 同じ共通frame・同じ失敗箇所。native Enter を投入する前なので native-key 経路の失敗を示さない |
| `folder_pane_refresh_tests::folder_pane_label_arrow_and_keyboard_keep_existing_behavior` | 共通frameは既に入力開始を通る。2607行の Enter 後 pending unwrap が None。別原因として、Enter が開始した実scanが描画中に完了して同じ pass の後段裁定で採用される競走があるが、今回はその実測確定まで進めていない。入力開始不足と断定しない |

前の7件の fixture は、描画だけを直接呼ぶ一方 `IndexerInit::NotStarted` を終端していない点も新しい表示受付と合わない。search の矢印座標も古い `read_response(id).rect` を読むため、再開時は既存 cfg(test) の実描画矩形との照合が必要（この6件の直接原因が幾何かどうかは未観測）。検索 editor の実描画・focus・クリックによる blur、pane の Enter が grid/native twin に漏れないという元の検査意図は維持する。投入済み native test frame を消す `input_begin_key_frame` の単純追加はしない。

リポジトリ全体で実tree / main bars の全呼び出しを列挙し、入力 owner を通らないテスト用描画を次の5系統として確認した。今回は通っているものは変更しない。

| ハーネス | 入力開始なしの描画と現在の用途 |
| --- | --- |
| `src/app/main_chrome.rs:337` 上記のmodal ownership試験callback | 生の名前クリック・resizeを送る。入力開始と表示完了がない（失敗対象） |
| `src/app/search_nav_pane_frame_tests.rs:240` `main_frame` | 生の矢印クリック・キー・native twinを送る。root keyboardはあるが入力開始がない（6失敗の共通frame） |
| `src/ui_folder_pane.rs:1290` `actual_folder_pane_deep_scroll_anchor_keeps_real_coordinate_and_cursor_scroll_wins` | 実treeを描くが生イベントなし。scroll / cursor / listingを直接設定する描画試験 |
| `src/app/tests.rs:27377` `rating_1328_large_folder_return` 内 `draw` | 実paneを描くが `RawInput.events` なし。履歴・お気に入り復元の4テストが共用 |
| `src/app/folder_pane_refresh_tests.rs:3830` `folder_pane_main_return_rechecks_other_branch_even_when_list_a_is_equal` 末尾の再表示closure | 実paneを描くが生イベントなし。復帰時のscan再開だけを検査 |

`ui_folder_pane` の empty-branch 実UI試験は完全な `input_update_begin` ではないが、専用 `begin_folder_pane_label_input → tree → finish` を既に通るため owner 欠落ではない。`folder_pane_refresh_tests` の他9描画siteもすべて `input_update_begin` が先行する。header-only / WidthTest / loading-width / snapshot fixture は header・splitter・純粋行描画だけで、名前・矢印の状態ownerを使うframeとは別に扱った。

停止により製品・テストの修正はしていない。変更はこの調査記録のみ。8件は上記のとおり失敗したままで、関連群全体・focused・core check・fmt の成功を今回の結果として主張しない。全lib・ui_snapshot・build-dev・製品起動・コミット・backlog変更は行っていない。

### 設計の改訂 r6: 停止理由はメインの枠のポインター判定に揃える (2026-11-14、ClaudeCode)

上の停止記録の指摘を受けて、§12.5 と r4・r5 の「停止理由」を次のとおり改める。r6 はそれより前の記述に優先する。

- 従来のツリーは `gate_main_chrome` で `main_chrome_pointer_blocked()` (共有の停止表を `include_menus=false` で読む) によってだけ止まり、グリッドのメニューが開いていてもメニュー外のクリックは届いていた。新しい入力の持ち主もこれと同じ判定を使う。新しいクリックの条件・継続の条件・適用時の確認はすべて、メニューを含まない停止表で評価する。継続の条件は「サイドカー復元の項目とメニューを除いた停止理由が無い」(`include_menus=false`、`include_sidecar_restore=false`) にする。`modal_dialog_block_reason()` (`include_menus=true`) はツリーの入力判定に使わない。
- メニューがツリーに重なる位置の押下は、停止表ではなく押下の witness で区別する。capture がメニューの部品なら、ツリーの名前・矢印の押下ではないので `FirstPress` / `ArrowPress` を作らず、継続中なら打ち切る (capture の条件は r4・r5 のまま)。メニュー外の押下は、そのあと `show_context_menu` がメニューを閉じても、従来どおり 1 回のクリックとして描画側の適用点で適用する。
- 停止表そのもの (項目と順序) は変えない。読み方の引数を揃えるだけなので、新しい状態や時間窓は増えない。メニューの有無で状態の組み合わせを足さず、従来のツリーと同じ判定にまとめる。
- 確かめること: グリッドのメニュー (egui メニュー、ブックマーク表示の専用 Window、読書履歴表示) が開いた状態で、メニュー外のツリーの名前・矢印をクリックすると従来どおり移動・開閉する。メニューに重なる位置の押下はツリーへ届かない。サイドカー復元中にメニューが開いている場合は、復元中の新しいクリックを受け付けない既定のまま。`main_chrome_folder_tree_clicks_follow_common_modal_ownership` の GridMenu ケースを本番の入力開始を通して通す。

#### r6 の実装と全libで見つかった8件への対応（`762936a9c` 上の未コミット差分）

**Enter の原因の確定。** `6f5791d1e` と `b01467f52` 以降で異なる Rust 4ファイル（`app.rs`、`ui_fullscreen.rs`、`ui_folder_pane.rs`、`app/folder_pane_refresh_tests.rs`）を一時的にすべて `6f5791d1e` の内容へ戻し、他の Rust source は両commitで同一であることを照合して、対象の arrow / keyboard 試験1件だけを実行した。**0成功 / 1失敗、0.702秒、exit100**。この環境では `6f5791d1e` でも Enter 後 pending の unwrap が None で失敗した（旧2493行、現在の2607行に対応）。ログは `target/769-r6-enter-6f.log`。HEAD・branchは動かさず、比較後は保存した現行4ファイルを復元した。

現行へ戻して一時診断を加えた実測（`target/769-r6-red-and-enter-diagnostic-2.log`）では、Enter pass は次の順だった。

1. 入力前: tree focusあり、cursor=B、pendingなし。
2. キー処理後: Enter消費済み、tree focus解放済み、実 pending=B。
3. 描画した全ラベル: `clicked=false`、`clicked_by(Primary)=false`、Enter=false、AccessKit Click要求=false。描画の移動候補は None。
4. 移動裁定前: pending=B。裁定後: 開始tokenあり・cancel=false、pendingなし・現在地=B。

従って実scanの成功応答が同じpassの裁定で採用される競走であり、P2 の候補None代入や `FAKE_PRIMARY_CLICKED` の重複ではない。vendor の fake flag は `get_response` がその時点のキー・focus・AccessKit要求から都度生成し、begin-passに保存されないことも照合した。**製品の Enter・非pointer活性化・P2 の候補更新は変更しない。** 試験frame本体を共有したまま裁定直前の hook を追加し、対象 Enter frame だけ、実handlerが作った目的=PaneNavigation・path=B・focus解放を検査して受信を一時保留する。裁定後も同じpath/token・非取消を検査し、実receiverへ戻して既存の実完了を通す。pending先Bという元の期待は維持し、scan速度に左右されない観測境界にした。

最初の診断試行は、診断用の `data_mut` 内で同じ Context を読むロック入れ子を実行前に発見し、コンパイル中に Ctrl+C で取り下げた（`target/769-r6-red-and-enter-diagnostic.log`）。pass番号をロック外で読む形へ直した **`-2.log` だけを有効な診断・失敗証拠とする**。一時診断は製品・テストからすべて撤去済み。

**r6 の実装。** 新規入力の開始、Single / Arrow の適用時確認は既存 `main_chrome_pointer_blocked()` を使う。継続と Toggle の適用時確認は、`main_chrome_pointer_blocked_except_sidecar_restore()` が同じ表を `(include_menus=false, include_sidecar_restore=false)` で読む。計5箇所を揃え、ツリー入力から `modal_dialog_block_reason()` を使わない。停止表の項目・順序、keyboard側の診断と停止、witness / layer によるメニュー重なりの拒否は変更しない。

**7件のハーネス。** main_chrome の既存試験と search_nav の共通 `main_frame` は、fixture開始時の NotStarted だけを終端し、ROOT入力開始をキー・ツールバー・main barsより先に通す。main_chrome に表示完了も加え、search は既存の表示完了を維持する。search の矢印座標は cfg(test) で記録する実描画矩形に揃えた。実search editorのfocus・クリックでのblur・native twin / 修飾 / keypad / coalescing、gridの選択本が開かないこと、PaneNavigationの採用先の期待は書き換えていない。通過していた入力なしの3系統の描画ハーネスは変更しない。

**修正前のr6回帰。** 実 `show_context_menu` を ROOT入力・キーの後、ツールバー・main barsの前に通すfixtureを追加。Grid / Bookmark専用Window / ReadingHistory × 名前・矢印の外側クリック6件、メニュー外の続きのpress、メニューに重なるcaptureの拒否、認識後にmenu ownerが始まるSingle / Arrow / Toggleの適用を検査する。さらに実CheckingRelayで復元の受信を保留した既存fixtureで、実メニュー併存時の新規クリック拒否と初回行への継続Toggle許可を別々に検査する。メニューのlive ownerと実Window、領域・capture・クリック成立を前提として確認し、メニューのcommandを実行しないよう overlapのrelease前にcaptureが実背景move widgetまたは非CLICK部品であることを検査する。全fixtureのデータはTempDir、nativeメニューはmain_hwnd無しで既存Fallbackへ送る。

ハーネス修正と新11件を加えたが、製品のr6停止条件を変える前に上記の診断と合わせて19件を実行した。**8成功 / 11失敗、4.974秒、exit100、コンパイル5分39秒**（`target/769-r6-red-and-enter-diagnostic-2.log`、当時のsourceは `target/769-r6-red-source/`）。searchの6件、メニュー重なり拒否、新クリックの復元中拒否は成功。失敗は、旧main_chromeが最初のNoneを通ってGridMenuで失敗、Enterの上記競走、および新r6回帰のうち正の9件すべてが実メニュー等の前提を通過後の移動・展開の製品assertで失敗した。fixture成立の失敗は含まない。

**最終検証:** Windows MSVC / default features / test・dev profile。すべて指定の `run-heavy.ps1 -Owner "codex-769"` 経由。比較用の実行は先行全件テストのlockを待ち、その後はlock領域への書込み不可の警告に従い BelowNormal / `CARGO_BUILD_JOBS=6` で実行した。core checkのみ既存native検証に合わせ `-j 1` を明示した。

| コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| 元の8件 + 新r6回帰11件の明示filter | **19成功 / 失敗0、18.007秒、exit0**。コンパイル9分24秒。上の修正前失敗11件を含む | `769-r6-green.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/) \| test(/main_chrome/) \| test(/search_nav/) \| test(/sidecar_restore/)' --no-fail-fast --retries 0 --no-tests fail` | **306成功 / 失敗0、169.094秒、exit0**（既存search_nav入力経路列挙等のslow 6件も成功）。P2のpointer B→現在地A / pointer→AccessKit現在地A回帰を含む | `769-r6-related.log` |
| `scripts/test-focused.ps1 -Path ...`（変更したテスト保有Rust 4ファイルを配列指定） | **18 / 114 / 17 / 6件の4群すべて成功、計155実行、exit0**。2.133 / 28.554 / 15.612 / 12.342秒。上のfilterとの重複を含む | `769-r6-focused.log`、`769-r6-focused.ps1`、ログ内の `test-focused/runs/*` の各結果 |
| `cargo check -p mimageviewer --bin mimageviewer-core -j 1` | **exit0、1分09秒** | `769-r6-check.log` |
| 引数なし `cargo fmt` / `cargo fmt --check` | **各exit0** | `769-r6-fmt.log` / `769-r6-fmt-check.log` |
| `git diff --check` / 検証前後のRust SHA-256照合 | **各成功** | `769-r6-diff-check.log` / `769-r6-source-hashes.json` |

focused の指定順は `src/ui_folder_pane.rs`、`src/app/folder_pane_refresh_tests.rs`、`src/app/main_chrome.rs`、`src/app/search_nav_pane_frame_tests.rs`。`src/app.rs` はscriptのbroad判定で全lib指定になるため、従来と同じ分担で除外した。共有表helperの引数修正は上記の実入力回帰・停止理由の既存回帰・core checkで確認した。独立 Sol / xhigh のr6実装静的レビューは **ACCEPT**、追加findingなし。HEADは `762936a9c` のまま未コミット。全lib・ui_snapshot・build-dev・製品起動は行わず、backlogにも触れていない。

## 13. マウスでクリックした行を中央へ寄せない (2026-11-12 設計、ClaudeCode)

利用者決定 (2026-11-12 夜、§1.769 の実機確認 依頼 40 から派生)。ツリーで行の名前、または ▶ / ▼ をマウスでクリックしたときは、その行を中央へ寄せない。押した行は既に見えているので、Windows のエクスプローラのナビゲーション ウィンドウと同じく、スクロール位置をそのままにする。

- 今の作り (v5.1.0 から): 行のクリックは `set_cursor` を呼び、`set_cursor` は `cursor_scroll = Requested` を立てる。▶ / ▼ の `handle_tree_key(Left/Right)` (`collapse_cursor` / `expand_cursor`) も `Requested` を立てる。次のフレームの `render_folder_pane_tree` が cursor 行で `scroll_to_me(Align::Center)` を呼び、行を中央へ寄せる (`animated(false)` なので一瞬)。
- 変えること: マウスの行クリック (名前・▶ / ▼ の Primary) を処理したフレームでは、そのクリックで立った reveal 要求と、クリックで置き換わる前のカーソルに対する未消化の要求を取り下げる。クリック以後の行では旧要求による reveal を行わず、描画末尾でも旧要求の復元・`FollowingRefresh` の決定を行わず `Idle` を保つ。同じフレームでクリックより前に既に適用された reveal は戻さない。フレーム内のローカル変数で扱い、新しい状態・フィールドは足さない。cursor・focus・開閉・移動の要求は今までどおり。
- 変えないこと: キー操作 (↑ ↓ ← → Enter、ゲームパッド)、ツリーを開いたときの現在地表示 (§11)、右側の一覧や Ctrl+↑↓・履歴など「クリック以外で現在地が変わる」ときの中央寄せ、ドライブ選択・↻ の再読み込み。これらは今の中央寄せのまま (「見える範囲に入れるだけ」にするのは次の版以降に検討)。
- クリック後の移動の完了: クリックで cursor は `Explicit` になり、`sync_to_active` は reveal を要求しない。移動の完了で前の現在地の祖先の自動展開が閉じて行が増減しても、既存の path anchor 復元 (`FolderPaneScrollMemory`) のままとし、追加の reveal はしない。
- まれな失敗は既定の割り切り方針による (書き込み・永続化なし)。

テスト (handler-level、実 pointer event): ツリーがスクロールする高さで、中央にない行の名前をクリックしても、次のフレーム以降でスクロール位置が変わらない (移動の完了後も、行の増減が無い場合は同じ)。▶ / ▼ のクリックでも同じ。キー操作 (↓ など) では今までどおり cursor 行が中央へ寄る。既存の cursor reveal のテスト (§10・§11) が通ること。

### 実装記録 (2026-11-12、`fix-tree-click-no-center`)

- §13 の前提を実コードで確認した。クリックの `set_cursor` は `Explicit` を設定し、通常の `PaneNavigation` の worker 完了・採用は選択の由来やツリーのフォーカスを変更しない。`sync_to_active` は `selection.follows_active()` が偽なら reveal を要求しない。完了経路の追加変更はない。
- `render_folder_pane_tree` の名前・有効な開閉矢印のクリック処理後、実ポインターの `clicked_by(Primary)` に限り、その処理が立てた `cursor_scroll` を `Idle` へ戻す。描画前に取り出した既存要求の処理、cursor/focus/開閉/移動、キー・ゲームパッド・drive・↻・現在地同期・path anchor は従来どおり。新しい状態・フィールドは追加しない。
- `Response::clicked()` は AccessKit Click や widget のキー確定も含むため、全クリックを無条件に取り消す形は採らなかった。実ポインターだけを区別することで、§11 の既存 AccessKit Click による要求所有テストを変更せず維持する。
- App の実 pointer press/release を通す回帰を4件追加。80兄弟のツリーで、非ゼロ offset・viewport 中央から100pt以上離れた可視行を使用する。名前クリックの移動を実 worker 応答の受信境界で保留し、保留中・完了後の offset と画面Y座標、行集合の不変、追加 reveal なしを検査する。▶/▼ は子行の増減と非移動も確認し、↓ は中央寄せの継続を検査する。
- **製品コード変更前**の `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane_pointer_/)' --no-fail-fast --retries 0 --no-tests fail` は **1成功 / 3失敗、exit100**。名前・展開・折畳の3件が offset 変化で失敗し、↓ の既存動作は成功した。名前/展開は `974.6875 → 800.6875`、折畳は `1003.6875 → 800.6875`。証跡: `target/tree-click-red.log`。
- `docs/spec.md` と `docs/keymap-spec.md` に操作差を各1行追記。マニュアルのツリー説明には中央寄せの記述がないため変更しない。

検証は Windows MSVC / default features / test・dev profile、`target/769-env.ps1` の native 環境で実施。Cargo のビルド・テスト・整形はすべて `run-heavy.ps1 -Owner tree-click-impl` 経由。先行する全libのロック待ちを継続した後、ロック領域への書き込み不可の警告に従い低優先度・`CARGO_BUILD_JOBS=6` で実行した。

| コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast --retries 0 --no-tests fail` | **94成功 / 失敗0、13.039秒、exit0**。新規4件と既存 cursor reveal 回帰を含む | `tree-click-related.log` |
| `scripts/test-focused.ps1 -Path src/ui_folder_pane.rs,src/app/folder_pane_refresh_tests.rs` | **18件 / 45件の2群すべて成功、各exit0**。実行1.322秒 / 12.965秒、計63実行は上の94件との重複を含む | `tree-click-focused.log`、`test-focused/runs/20261112T135330438Z-ca53eede` / `20261112T135335196Z-10f5baf0` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit0、1分25秒** | `tree-click-check.log` |
| 引数なし `cargo fmt` | **exit0** | `tree-click-fmt.log` |
| `cargo fmt --check`、`git diff --check` | **各exit0** | `tree-click-fmt-check.log`、終了時diff検査 |

対象は `15646a9d3` 上の未コミット差分。検証済み Rust 2ファイルの SHA-256 は `target/tree-click-source-hashes.json` に保存した。新規4件のうちマウス3件は修正前失敗→修正後成功、↓ の継続確認は両方成功。移動完了後の追加 reveal は実 App handler / worker 完了の回帰でも発生しなかった。

指定により、本体lib全件・`build-dev.ps1`・`ui_snapshot`・製品起動・コミットは実施しない。全lib・確認用build・snapshot と最終受入は設計担当へ引き継ぐ。`docs/next-release-backlog.md` は変更していない。

### 独立レビュー P2: 未消化の旧要求の復活 (2026-11-12)

独立レビュー (`xhigh`) が `0bba3577f` を REVISE とし、設計担当が P2 を採用した。初回実装はフィールドだけを `Idle` にするため、描画前に `mem::take` したローカルの `Requested` が残っていた。旧カーソル行が列挙待ちで未表示なら `cursor_drawn == false` のまま末尾が旧要求を復元し、次フレームでクリック先が中央へ移動する。初回記録の「描画前に取り出した既存要求の処理は従来どおり」では、この失効済み要求を新カーソルへ持ち越す経路を残していた。

このため§13の「変えること」を補足し、クリック以前に実行済みの reveal は維持しつつ、クリック以後の旧要求の適用と末尾の要求復元・`FollowingRefresh` 化を止める。ペインの永続状態は追加せず、当該描画のローカル変数だけで扱う。

- `render_folder_pane_tree` のローカル `pointer_row_clicked` に実 Primary の行クリックを記録し、その場でローカル `cursor_scroll` も `Idle` にする。後続行は旧要求を適用せず、末尾はクリック済みならフィールドを `Idle` に確定し、旧要求の復元・`FollowingRefresh` の決定を通さない。クリック前に既に発行した reveal、非ポインター操作・AccessKit の要求所有は維持する。
- 実 pointer press/release の回帰を名前・▶・▼の3件追加。旧カーソルの親を展開し、その初回実走査を既存 backend の gate で保留する。旧カーソル行が未表示、走査 pending、`Requested` が2フレーム持ち越されることを確認してクリックし、後続フレームと保留走査の完了後も offset・行Y座標・reveal回数を保持することを検査する。
- **製品コード変更前**の `cargo nextest run -p mimageviewer --lib -E 'test(/retires_waiting_cursor_request/)' --no-fail-fast --retries 0 --no-tests fail` は **0成功 / 3失敗、exit100**。3件とも後続フレームの offset 検査で失敗した（名前/展開 `974.6875 → 800.6875`、折畳 `1003.6875 → 800.6875`）。証跡は `target/tree-click-p2-red.log`。前提確認の失敗やコンパイルエラーではない。

検証対象は `0bba3577f` 上の未コミット差分、Windows MSVC / default features / test・dev profile。`target/769-env.ps1` を使用し、全 Cargo コマンドを `run-heavy.ps1 -Owner tree-click-impl` 経由で実行した。修正前・修正後とも先行する全件テストのロックを待ち、書き込み不可の警告後は指定の低優先度・6並列で続行した。

| コマンド・範囲 | 結果 | 証跡 (`target/`) |
| --- | --- | --- |
| `cargo nextest run -p mimageviewer --lib -E 'test(/folder_pane/)' --no-fail-fast --retries 0 --no-tests fail` | **97成功 / 失敗0、13.194秒、exit0**。新規3件と既存 cursor reveal / AccessKit 回帰を含む | `tree-click-p2-related.log` |
| `scripts/test-focused.ps1 -Path src/ui_folder_pane.rs,src/app/folder_pane_refresh_tests.rs` | **18件 / 48件の2群すべて成功、各exit0**。1.621秒 / 13.389秒、計66実行は上との重複を含む | `tree-click-p2-focused.log`、`test-focused/runs/20261112T144047347Z-4262ba62` / `20261112T144052760Z-ca06d733` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | **exit0、20.91秒** | `tree-click-p2-check.log` |
| 引数なし `cargo fmt`、`cargo fmt --check`、`git diff --check` | **各exit0** | `tree-click-p2-fmt.log` / `tree-click-p2-fmt-check.log`、終了時diff検査 |

検証済み Rust 2ファイルの SHA-256 は `target/tree-click-p2-source-hashes.json` に保存し、検証後に同一であることを確認した。新規3件はすべて修正前失敗→修正後成功。本体lib全件・`build-dev.ps1`・`ui_snapshot`・製品起動・コミットは指定により実施せず、backlog は変更していない。独立再レビュー・全体検証と最終受入は設計担当へ引き継ぐ。
