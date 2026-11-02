# §1.686 外部変更に対するフォルダツリーの再照合

設計案 (2026-11-02、Codex)。v5 向け、ClaudeCode の採否・実装引き継ぎ用。
調査基点は `tree-1686` / `3a193e50df91faed2969dd752ed780270a2ae3b0`。
**設計だけを作成した。製品コードの変更、製品起動、再現試験、ビルド、コミットは行っていない。**

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
- **scroll**: 同じ `folder_pane_tree_scroll` のIDを保ち、refresh開始/失敗時は位置を変えない。自動成功時は交換直前の最上段可視pathと行内offsetをanchorにして、残存時は同位置へ復元する。anchor消失時は近い残存行/可視祖先へ寄せ、末尾のclampを許す。明示cursor移動や上記cursor修復のscroll要求を優先する。既存row高24ptを用いるメモリ内計算でありFS照会しない。drive切替や表示時の既存focus移動は自動refreshによるscroll保持とは別の能動規則のままにする。

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

- **採用: (a) 一覧で受理した実フォルダ観測のツリー親への通知 + (c) 再展開・再表示・ドライブ再選択/切替・main 再表示での再照合。(b) 展開枝の非再帰監視は見送る。** CLAUDE.md「設計の簡素化」による。(b) は監視 worker・登録 ticket・pane 表示 lease・logic service での退役・command channel 満杯時の合流など、状態と lifecycle を大きく増やす。報告 (>>707・710) の症状は「一覧の親は更新されるのにツリーに残る」「折りたたみ・再展開・ドライブ切替でも残る」で、(a)+(c) で閉じる。(a)+(c) で残るのは「一覧に出していない別の展開枝を開いたまま外部で変えた」場合だけで、これは再展開・再表示・既存の ↻ で取り直せる (割り切り)。(b) は利用者の報告や要望が出たときに、本書 §5.1 の監視節を土台に別途検討する。
- 採用する部分: §1 の追加条件 (部分列挙・取消・分類失敗を成功扱いしない、hidden 除外と実消失の区別、成功で消えた枝を現在地同期や子の遅着で復活させない)、§2 の簡素化 (既存 `RefreshPreservingChildren` の受理境界への集約、手動 reload の契約維持)、§5 の排他 enum への集約と FollowUp 一件への合流、§5.1 の 1・2・4・5 (需要集合・祖先 reveal・需要離脱時の未確認化・以前 empty の collapsed leaf の再展開)、§5.2、§6 から (b) 固有の行を除いたもの、§7、§8 から監視固有の検査を除いたもの。§4 の一覧→ツリーの橋は `poll_external_rescan` の `accepted_now == true` の直後とする。
- 見送る部分: §4 の「pane 監視 session」、§5.1 の 3 のうち監視登録、§5.1 の監視 worker / command channel / lease / logic service 退役 / callback 通知の段落、同 path の実体入れ替えを watch の affected path で検出する段落 (再表示と手動再取得で閉じる)、§8 の watch 登録・overflow・lease の検査。
- (c) の再照合の入口は、需要へ新規参加した親を「同 options の cache があっても一度確認する」で実装する (§5.1 の 3 の監視を除いた部分)。毎 frame の `sync_to_active` は遷移がなければ何もしない。

## 10. 設計レビュー・検証記録

本案はソース調査による提案。製品の実行観測はバックログに記録された報告者の結果だけであり、この作業で再現したものではない。独立レビューの結果は完了後に本節へ追記する。
