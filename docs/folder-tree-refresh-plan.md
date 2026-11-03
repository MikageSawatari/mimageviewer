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

一覧→ツリーの橋は `poll_current_folder_watch` の生通知には置かず、**`poll_external_rescan` の `accepted_now == true` の直後**に置く。既存のowner/source/generation/projection/subscription検証を通過し、要求元contextがmainであることを明示する。`Different` のdefer/拒否/失敗は通知しない。`Equal` も、一覧のfilter/投影とツリーのdirectory集合は異なるため確認のヒントを送る。対応するツリー親は列挙した `folder` 自身であり、常に `folder.parent()` を更新するという意味ではない。

mainがpark中にそのownerの受理が行われても、捕捉したmain identityとpathだけで通知し、投影中の別contextの `effective_folder()` を取り直さない。pane側は未作成nodeを増やさず、既存cacheを未確認へするだけ。現在需要がある親だけ再取得する。別窓の能動Operation完了や独立readerのopen/closeをこの橋の入口にしない。

`FolderPaneState` はbundle swap対象でなく、`current_folder_watch` 等は `viewer_context_registry.rs` のswap対象である。この区別を維持する。pane同期・監視需要計算・active表示の更新は既存main chrome境界で行い、全contextのmounted serviceへ追加しない。独立readerの受動購読を復活させず、mainの購読を流用/取消しない。detached述語やviewportの変更は本案の範囲外であり、必要になったら実装前にリワーク計画§2/§11の合意へ戻す。

## 5. nodeの状態と単一の採用境界

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
