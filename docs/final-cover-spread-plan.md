# 末尾に表紙を添える見開き表示

## 状態と開発境界

2026-09-10、利用者依頼により調査・設計着手。基準masterは4a60d770c49ecce1792031ae388af9a92eb9e5c9、作業場所はC:/home/mimageviewer-dupe、機能branchはcodex/final-cover-spread。duplicate-detectionの履歴を保持する。v3.8.0の公開には未完成の変更を混ぜず、masterへのmerge/push/公開/version変更は行わない。

親が設計・文書、implement_resume（Sol/xhigh）が本体source/test、remote_cover_design（Sol/xhigh）がRemote source/test、review_resume（別Sol/xhigh）が独立設計・完成差分レビューを担当。Phase Aは単独writerで完了し、次段から下記のファイル所有で分担する。旧ClaudeCode設計検収指定は現AGENTSの開発役割へ移管し、公開責任は変更しない。大きなCargo/GPU処理はmaster親タスクとの所有調整後に行う。アプリ操作は個別suiteの明示了承後のみ、通常profileのagent起動や実データ変更は行わない。

### 実装と検証の区切り

| 段階 | 成果物 | 現在の状態 |
| --- | --- | --- |
| A 共通基盤 | role/occurrence/composition、phase所有demand、global既定ON、本override別table、context・metadata・rename境界 | 修正・独立検収・再fullgate完了 |
| B 本体表示 | paged/連結読みの描画・需要・保持・失敗終端、編集復帰、HUD・各操作のidentity接続、設定UI | B2r6でprevious capture・layout寿命・target identity修正、独立検収・再fullgate完了 |
| C Remote | sparse presentation wire、live設定snapshot、typed設定書込、Webのnavと描画分離、protocol互換 | source凍結・独立レビュー・Node388件・IPC55件・Rust狭域とRemote check完了 |
| D 最終検証 | 各段階の回帰・snapshot、shared full gate、利用者確認用build、実機確認 | 再fullgate・snapshot・core/Remote確認build完了。利用者が本体/Remote/連結/F12/ずらしの動作良好を報告。master統合後の検証は別途 |

利用者から確認buildで「本体・Remoteとも動作良好、連結読みも問題なし、F12でも動作、ずらしも問題なさそう」との実機結果を受領した。個別の全体/本別設定や全形式の全組合せを網羅した報告とは区別する。機能側の自動検証・独立レビューとこの実機確認に基づきmaster統合へ進める状態と判断する。確認時のmasterは3b0f9e800で、本機能の分岐後にnative click smoke関連4commitがある。読み取りmerge-treeでは競合markerなしだが、master作業ツリーには別作業の未コミット変更があるため、その所有担当と区切りを調整する。既存変更のstash/取り込み/破棄は行わず、実mergeと統合後検証はmaster担当で実施する。

Aの成功だけでは機能完成・利用者検証可能とは扱わない。各段階のsource変更は実装担当、凍結差分レビューは別担当とし、既存検証を重複実行しない。Cargo枠はAのcheckと狭い回帰に限定して借用し、結果とログを共有して返却する。次段階の重い検証はあらためて所有調整する。

Phase A commit 93a41f82eの後、本体表示とRemote実装の分担開始を承認した。Remote担当はcrates/remote-ipc、crates/remote-web、src/remote_ipc、src/settings_db.rsを専有する。本体担当はapp/app配下/ui_fullscreen/座標変換/local UIを所有する。共有settings/spread_db/metadata/renameは本体担当の所有を維持するが、凍結APIの変更は先に担当間で調整する。docsとcommitは親が担当する。formatterも各自の所有ファイルに限定し、全体formatで相手の編集中ファイルを書き換えない。Cargo枠は返却済みのため次の検証前に再調整する。

## 利用者が確定した要件

- 表紙あり見開きで、先頭と末尾がともに単ページの場合、末尾ページに先頭の表紙を添えて描画する。
- 先頭の単ページ表示は維持する。
- 読書位置・ページ数・シーク列は変えない。添えた表紙を新しいシーク・ページ移動対象にしない。
- ファイルの複製・並べ替えや編集データの改変で実現しない。
- 「移動対象外」を保存・編集・補正等の全操作対象外へ自動的に広げない。

## 設計前提（検収中）

SpreadDisplayUnitの先頭[0]と末尾[last]をnavigationの正本として保持し、別の役割付き描画構成が末尾anchorとFrontCoverSupplementを表す。resolve_spread_pairだけを非連続Doubleへ置き換えない。描画・保持・読み込み・AI/カラー化・PDF promotionの需要集合はこの共通構成から導出する。個々のconsumerが「末尾なら0を足す」を独自実装しない。

FullscreenPageLayoutは実描画ページと座標変換を保持するため、元画像のidentityは表紙のまま、操作の目的に応じて末尾の読書anchorと区別する。編集/ルーペのhitは実画像を識別しつつ、シーク・ページ移動への逆引きでは表紙へ飛ばない契約が必要。現コードの全consumerを調べてからAPIを確定する。

非同期需要・holdover・park/activate/dropはviewer context内で閉じる。表紙の読み込み失敗/寸法未知/処理中/回転変更/設定変更/本の切替でも、旧contextへ結果を戻さず、表示と需要のidentityを一致させる。通常ページ送りのall-Live/色忠実rendition契約を弱めない。

## 現在の仕様（追加回答・設計検収を反映）

| 項目 | 採用する仕様 | 現状態 |
| --- | --- | --- |
| 初期対応範囲 | 本体main/F12・連結読み・Remote、通常画像/ZIP/PDF | 確定 |
| 有効化と保存 | 全体設定は既定ON、本ごとは全体に従う/ON/OFF。既存spreadとは別table | 確定 |
| 左右配置 | LTRは末尾/表紙、RTLは表紙/末尾。読書anchorは末尾 | 確定 |
| 横長・回転・一時ずらし | 既存Cover resolverが先頭・末尾の両方をSingleと判断する場合 | 確定 |
| 本の境界 | 単本の完全canonical読書列、両端distinct。合成・切り詰め列は対象外 | 確定 |
| 保存・補正・編集・類似 | 実描画需要は全slot、current操作はanchor、pointer操作は実画像。既存操作の意味を維持 | consumer接続は実装・検証対象 |

## 検証方針

実装前に上の前提をコード・反例で確認し、親と独立reviewが設計を検収する。回帰は両方向・枚数/単独条件・ページ移動/End/Home/seek/resumeとpage count不変、左右geometry/hitと編集identity、main/F12/sibling context、非連続表紙のload/keep/AI/PDF需要・完了/取消/失敗、設定永続化を対象にする。表示変更はsnapshotを含める。既存機能の弱体化で回避しない。

関連正本: backlog-on-hold.md §4.3、display-pipeline.md、virtual-folders.md、preset-and-adjustment.md、async-architecture.md、detached-rework-plan.md §2/§11。最終shared gate/確認buildは資源所有を調整して実施する。実機suiteと未検証事項は自動検証と区別する。

## 前提調査の追補（製品未実装）

現SpreadDisplayUnitsCache（items_generation/spread/shift/landscape_epoch/nav）を正本とし、role構成はそのunitから導出する。既存display-pipelineの固定cacheなし記述は旧状態なので採用しない。typed compositionの候補はanchorとNavigation/FrontCoverSupplementのpage slot列。単本の通常読書順を基本案とし、両端distinct・cover mode・先頭末尾各Single・対象image/ZIP/PDFという条件を検討中。左右は通常読み方向、LTRは末尾/表紙、RTLは表紙/末尾。

現PageEditSpreadPivotがpair.0へ復帰するため、補助表紙では元navigation anchorの明示保存が必要。編集targetは実idx、復帰は末尾とする。external toolのspread_reading_orderがidx昇順を使う点は非連続配置と矛盾するため、実表示/reading-role順へ合わせる必要がある。HUD/page number/seek label/history/current anchorはnav unitを参照し、保存/export/holdover/debug/実画像操作はvisible構成を参照する。

初期の「nav roleがLiveならsupplement未準備でもnavigation terminalを進める」案は撤回。既存の両slot atomic/色忠実契約を維持し、実paint全slot Liveを要求、target identity比較だけNavigation roleへ投影する案へ更新した。supplement失敗/取消もpresentation需要の失敗終端へ届く必要があり、nav target.pagesだけで待機解除を判定しない。独立検収中。

元タスク親が有効化と初期範囲を利用者へ確認し、下記の追加回答を受領。前述のpaged-only案は採用しない。

## 利用者追加回答（2026-09-10、こちらが優先）

環境設定で全体ON/OFF、既定ON。本ごとは見開きモードと同様に記憶する。親案は本overrideを「全体に従う/ON/OFF」とし、共通resolverで実効値を決める。連結読みとRemoteも初版から対応し、モードで見え方が変わり設定の記憶が混乱することを避ける。paged-only除外案は撤回し、全flowで設定と両端の組み合わせを共有する。今夜のv3.8.0へ完成を必須化せず、別branchで開発する。

初版対象は本体main/F12・連結読み・Remote、画像/ZIP/PDF。全体→本overrideの保存/取消/既存DB移行・oldschema readonly、continuousの重複表示/keep/VRAM/anchor逆引き、Remote group/address/protocol/client renderを独立設計検収へ追加。シーク/移動位置・page countを増やさない条件は全mode共通。raw source page0を操作対象から全面除外しない。

Remoteのbounded read-only設計調査はremote_cover_design（Sol/xhigh）を追加し、writerは引き続きimplement_resume一人、独立reviewは別review_resume。製品実装前にlocal/Remoteの契約を合わせる。

## 全mode共通化の設計候補

phaseの単なるpages列をFsNavigationDisplayDemandへ置換し、private constructorがrole付きpresentation slot列からnavigation投影を導出する。外から2集合を独立更新できない。全phase/rebindが同demandを所有し、readiness/failure/全Liveは全presentation、位置/履歴/シークはNavigationのみ参照。通常見開きは両slotがNavigationなので従来と同値。現在約62参照のphase接続を変更する見込みで、描画だけの局所修正ではない。

continuousはunit-positionごとのcompositionを需要正本にする。同source idx0が先頭Navigationと末尾Supplementで重複するため、cache/VRAM trimで除外unitのidxをkeep集合から直接removeする現方式は不可。残存positionsからkeepを導出するかrefcountで保持し、visible/prepareの需要を失わない。layout/VerticalReadingPage/navigatorにもoccurrence roleと読書anchorを保持し、逆引きはnav roleに限定する。

設定案はglobal bool既定true＋同spread.db内の別table(path, preference)。spreadsへcover-only新規rowを作って既存mode/flow/direction継承を固定しない。absence=exact未指定から既存nested fallback、0=FollowGlobal（明示的に親fallback遮断）、1=On、2=Off。rootのFollowGlobalはrow削除、nestedは0保存の案。設定値はcontext bundle所有、実効resolver共通。rename/content-identity/import/export/metadata cleanup/copy/restoreとRemote旧readonly missing tableも接続する。DB操作はテスト用一時領域で検証し、利用者profileを移行実験に使わない。

## 設計検収とPhase A着手

独立Solは最新typed demand案を条件付き承認。親は次の不変条件を必須として共有純型・設定resolver/別table・互換回帰のPhase A実装を承認した。製品writerは一人、Remote wire詳細は別担当の調査後に確定し、未完成状態をmasterや利用者用buildへ渡さない。Cargoは元タスクとの排他枠確認後。

- navigation pages/anchorとpresentation roleをphaseで一体所有し、全role readiness/failure・全Live、nav投影だけtarget位置と照合する。
- continuousの同sourceをunit anchor+role occurrenceで区別し、keepは残存unit需要のunion/refcount、idx単独のremoveは不可。
- 編集targetは実画像、復帰anchorは末尾。既存の左右操作を維持する。
- 別tableでcover-only保存がmode/flow/direction継承を実体化・削除しない。nested FollowGlobal、oldschema readonly、metadata transfer/rename/cleanup/context/global変更を接続する。
- 対象は単本・完全canonical readingorder。既存Cover resolverの読書順を使い、独自natural sortは作らない。両端Single判定には既存shift/保存回転/横長判定を共有する。
- tag等は既存操作の意味を保持する。current操作はanchor、pointer操作はactual、book-levelは同container。暗黙の両側更新を追加しない。Similarはvisible情報を維持しprimary/historyは末尾、external MainPageOnlyはanchor、Both/Mergedは明示のscreen/read順。

旧ClaudeCodeとの構造合意条件は現AGENTSの親＋独立reviewへ移管済み。この変更は新機能の表示・需要identityを正す構造修正で、detached症状パッチではない。実装で触れる正確な範囲をdetached計画§11にも記録する。

## Remote設計一次合意

wire PageGroup.anchor/pages/sliceはnavigation正本として保持し、role付きpresentation_slotsを追加する。通常/旧payloadでslotsが無い場合は既存pagesをNavigationとして解決する。WebのnavigationEntriesとpresentationSlotsを分け、seek/URL/position/progress/page count/current操作はnav、layout/display coordinator/decoded unit/AI/左右調整はpresentationを使う。render contextはpage idxからgroupを逆引きせずgroup+slotから明示的にimage requestへ渡す。decode-ahead/unit identityにrole/slot address/contextを含め、全slot atomic ready/failureを保つ。近傍prefetchはnavigation位置で決め、visible presentation需要は別途protectする。

Remote global設定は起動時の古いSettingsだけを参照せず、request時のlive small snapshotから取得する。別override tableはread-only旧schemaでmissingならFollowGlobal、Remote側でmigrationしない。設定変更はSetSpreadへ混ぜずtyped commandでexact/fallback book keyへ保存。必要なIPC PROTOCOL_VERSIONの更新は機能branch内で行い、リリースversion/changelogには触れない。旧JSは新slotsを無視しnav表示を継続、新JSは旧slots不在に互換、既存asset token更新へ従う。

collection等の合成列はlocalと同じeligibilityを使う。canonical単本完全列・元container keyを確認できる場合にのみ補助を解決し、mixed/syntheticを一冊とみなさない。新しいsession-only本overrideやcollection独自のglobal-only意味は作らない。既存collectionの表示mode/session機能は維持する。

Remote wire追補: 通常groupのaddress列を倍増させないため、presentationはOption slotsとし、全Navigationでpages同値ならfieldを省略、新clientがpagesから再構成する。内部coreは常にrole付き構成。補助がある末尾groupのみfull screen-order slotsを渡す。応答budgetでentriesがtruncateされた場合に部分末尾を本の末尾と誤認しないよう、complete canonical列の確認を必須とする。localの完全列eligibilityと同じ意味にそろえる。

Remote budget根拠の補足: ContainerEntryBudgetはentry実測にaddress_bytes×2+32を加え既存group pages/anchorを概算済み（40MiB cap、IPC hard64MiB、24MiB envelope reserve）。末尾1groupのOption presentationで追加する2address+role/fieldはO(1)なので全面予算式変更は行わず、既存long entries/page groupsのIPC64MiB回帰へsupplement条件を加える。『entryだけを数えている』という初期調査表現は採用しない。

Remote実装用の共有境界は既存RemotePageGroupSpecへOption presentationを追加し、build_remote_spread_page_groups_with_compositionを公開する。privateなSpreadDisplayUnitとcomposition resolverを本体側に維持し、新builderもそのresolverへ渡す。indicesは従来Navigation投影、presentationは差分時のみ全screen順、splitは従来どおり。旧builderは補助OFFの互換wrapperとし、Remote側はroleをaddressへ変換するだけで判定を複製しない。

live snapshotはRemote担当がsrc/settings_db.rsを専有し、RemoteReadingSettingsにdefault_spread_mode/default_reading_direction/final_cover_spread_enabled/spread_page_gap_pxを同snapshotで取得するAPIを実装する。欠如keyには既存fallbackを使うが、DB読込失敗を起動時設定へ黙って置換せず既存Remote errorへ返す。ContainerEngineのspread_payloadごとに1回取得する。CollectionEngineの既存live設定取得と合成列の表示は維持する。共有API凍結を確認するまでRemote実装は開始しない。

## Phase A検証記録（2026-09-10、途中）

ログはtarget/final-cover-spread-phase-a-20260910/logs。初回cargo checkはexit 0、12分32秒。誤って空の専用targetを選択し依存compileが発生したため、以降は既存dupe targetを再利用する。

cargo test -p mimageviewer --lib final_coverの初回はtest-only compileでexit 101、テストは未実行。新demand APIへの旧fixture追随漏れ・重複method等を修正対象として扱い、製品テストの失敗件数へ換算しない。独立レビューで旧schemaのcount、context隔離回帰、metadata import refreshのnested fallback継承を追加確認し、修正と回帰をまとめてから再検証する。Cargo/rustc停止確認後に元タスクへ枠を返却し、次枠確定まではCargoを起動しない。まだPhase A合格・最終凍結とは扱わない。

修正後の静的凍結は11 source。patchはtarget/final-cover-spread-phase-a-20260910/phase-a-static.patch、SHA256は978752435daed7c534df0dd07e23d8a0ad99479ca55ad09c557c7974e3194307。独立Solがsource hash一致とdiff-check cleanを確認し、3件の指摘解消・追加P1/P2なしとして静的承認した。最終narrow testとfmt-checkは次Cargo枠待ち。renderer/Remote production接続はまだ未完了。

Phase Bは共通composition取得→phaseの全slot需要/ready/failure/observer→paged layout/trace→continuous occurrence/keep→編集復帰と各操作→設定UI/invalidationの順に接続する。Remoteは共有builderを使う別所有へ分ける。Phase Aの再検証前に新source変更を混ぜない。

Phase B前提の独立確認で、page_order_lockedをeligibilityへ要求すると普通の全画像folderを誤除外すると判明した。この条件は採用しない。TopLevelGridSurface::Folder、全itemがpage-data、現在の読書順が全itemの一意完全列、非synthetic/search、stack_viewとstack_mode_requestedの両方なしを確認する。通常folderの手動Cover modeも対象とし、独自sortは追加しない。stackのasync準備中も除外する。localのcurrent fullscreen anchorはNavigation role内であることを確認して全occurrenceへtyped rebindし、Remote向けのunit anchor/APIは変えない。この境界は独立Solが承認し、普通folder包含とstack準備中除外の回帰を本体担当へ依頼済み。

最終再検証は同じ11 source hashで完了。`cargo test -p mimageviewer --lib final_cover -- --nocapture` は12/12 PASS（final-cover-tests-green-2.*）、通常見開き同値回帰1件は1/1 PASS（spread-role-equivalence.*）、`cargo fmt --check` はexit 0（cargo-fmt-check.*）。独立Solが実ログ・hashを照合しP1/P2なしでPhase Aを最終承認した。これは共通基盤のcheckpointであり、機能全体のfull gate・利用者build・実機確認は未実施。

Phase B1はui_fullscreen.rsのeligibility、全phaseのrole付きdemand bind/rebind、全presentation readiness、occurrence順とNavigation投影を確認するobserver、paged render/capture/visible-pairを接続した段階。Cargo未実行で、完成判定ではない。レビュー用snapshotはtarget/final-cover-spread-phase-b1-20260910、base93a41f82e、source SHA256 2095ffec7936929f65bb17beea1fdfdc18afd09330be88783fce8f58487fa9f8、patch SHA256 47c5e0a4b663f050f50bce70092a000a0de2b9c7da2443d31943c1d3fb7d6128。current sourceは後続continuous実装で変わるため、B1の独立レビューはこのsnapshotを対象にし、最終時には最新差分との整合を別途確認する。

Remoteはwire/server/live snapshotを先に実装し独立レビューへ渡した。Web表示は後続実装中。サーバー対象ファイルを凍結したまま別のWebファイルへ進み、レビューと実装の対象差分を分離する。これらのRust変更もまだCargo未検証。

Remoteの小SQL保存は既存SetSpreadと同じ単一UI drain/FIFOとApp所有の既存DB handleを利用する。web-remote-planの既存write契約を独立レビューで確認し、cold open/走査を含まない1 statementの追加として採用した。このvariantだけ別workerへ分けて保存順序を二重所有にしない。遅延を示す実測が出た場合は既存計測logを根拠に全spread writeの共通writerを検討する。

B1の性能確認では、完全列proofのall-items走査とHashSet構築が各composition取得で繰り返される点を指摘した。既存SpreadDisplayUnitsCacheのitems generation/nav tokenに従う派生情報へ寄せ、view/search/stackなど軽い条件だけを都度評価する境界を本体担当が検討中。補助OFFや非Coverにも毎frameの新しいO(N)検証を追加しない。

B1独立レビューはP1×1/P2×2を指摘し、本体担当が修正中。(1) continuous分岐前にpaged navのdemandを作るとstill-unitの実paintと分裂するため、continuousは既存still nav/unit ownerから同一compositionを需要と描画へ渡す。(2) 既存nav Arc/exact identityとitems/order identityで完全列proofを保持し、landscape/rotation epochではproofを再走査しない。既存beforeにも全unit builderはあるため、その処理全体が新規の負荷であるとは扱わない。(3) deferral/decision probeの旧navigation-only pagesを全presentationへそろえる。修正後deltaの独立再確認が必要。

Remote server/wireは混合ZIP levelのP2を修正し、独立レビューが該当条件と回帰を確認してP1/P2なしで静的承認。Webでは旧payloadのpresentation不在/nullだけを互換fallbackとし、明示invalid配列は状態commit前にrejectして既存load/errorへ戻す契約へ修正中。Rust検証とWeb最終レビューはまだ未完了。

Webの明示invalid修正後snapshotはtarget/codex-final-cover-remote/web.patch、SHA256 68f3041cade4708fc68c779a9ff5b59d2b7ea998145f45a83c98cbbc4263fda5。同ディレクトリの9 suiteログは合計387/387 PASS（app-runtime 104、command-core 128、document-double-tap 7、local-settings 11、page-coordinator 27、page-timings 6、pwa 42、video-stream 50、viewer-position 12）。Nodeのtest dispatcherはsandbox EPERMのため各test moduleを直接実行した。実アプリ操作はしていない。独立Webレビューへ渡し、Rust側の実行検証は引き続き待ち。

RemoteのRust検証予定はprotocol crate、live settings snapshot、ZIP mixed/nested、sparse presentation/complete条件、frame境界を含む。frame境界は2件に分ける。container_accepts_one_hundred_thousand_short_entries_and_truncates_the_nextで10万件の完全短path payloadに補助2slotを含めて64MiB未満を確認し、10万1件のcount truncationも確認する。container_long_entries_and_page_groups_stay_below_the_ipc_frame_limitは400文字pathのbyte truncation後のpayloadを確認するもので、truncatedのため補助なしとなる。最終compileはcore binとmimageviewer-remoteを対象に含める。core source編集中はCargoを起動せず、統合した凍結差分で一担当が実行する。

Web snapshot独立レビューはP1なし、P2×2と回帰不足1件を指摘。(1) presentation内の同一address重複を拒否する。(2) prefetchをasync処理前にgroup identityで一意化し、方向別HUDも重複させず、容量に必要なunique presentation resourcesを含める。(3) invalid応答時の旧state維持を、normalizerと無関係なsnapshot自己比較ではなく実applyContainerDataとstate/position/history ownerを通す回帰で確認する。Remote担当へ一括修正を依頼し、修正deltaと関連Node結果を再確認する。387件greenだけでこの区切りを最終承認したとは扱わない。

Web修正後の最終snapshotは同web.patch、SHA256 da5ad2ba8a1f20168ff770eccd1191c86876eed748ab927f79b61cba489e8501。旧snapshotはweb-pre-review-68F3041C.patchとして保存した。3指摘を修正し、実applyContainerDataとViewerPositionOwnerを通す回帰を含む9 suite計388/388 PASS（app-runtimeのみ105へ増加）。独立Solがhash・コードdelta・最終ログを確認してP1/P2なしでWebを承認した。app-runtime-review.log/pwa-review.logは修正前のreview-red各1failで、最終9ログとは区別する。Remote sourceを凍結し、Rustの統合検証を待つ。

B2統合freezeはtarget/final-cover-spread-phase-b2-20260910。本体13fileのpatch SHA256 9c634f517e20078eb38f23ef2ddf5f73227c57ba4baadd186c4ebcd88c394af6、Remoteを含む24 source fileのpatch SHA256 e63e07c941062abce12def68ecbf84f487a748e1a27f169fe8acc3f98972382d。integrated-source-hashes.tsvに対象hashを保存し、Remote11fileも既承認manifestと全件一致を確認した。この同じsourceを本体独立レビューと狭域Cargoへ渡し、実行中は編集しない。次のCargo枠は元タスクから貸与済みで、jobs1/既存target/非UIの開始を連絡した。fullgate・確認buildは別調整であり、まだ実施していない。

初回統合検証でIPCは55/55 PASS。本体libはtest/API追随漏れによるcompile exit101でテスト未実行となり、実装担当がclosure・既存unit helper名・test environment型の3箇所を修正した。旧B2 freezeは失効し、再freezeはtarget/final-cover-spread-phase-b2r1-20260910、body patch SHA256 68b9bfaf2beef3d44ebe646fc222cc2a2d9586caf68d0bd8791716a8057d5310、統合patch SHA256 7db996f547e8f54261dcd7c283bb9d49c0b3909ffbe710eb3538da085d184f7c。Remote11file hashは不変。独立レビューは既存確認を保持してこの修正deltaを追加確認する。以降は広いcover文字列filterを避け、final_coverと必要な個別回帰、Remote live settings・frame境界を実行する。snapshot追加と実行、fullgate、確認buildは依然未実施。

再試行b2r1もtest fixtureの可変receiverとitems_generation参照が重なるE0502でcompile exit101。世代値を呼出し前に取得するtest-only修正後のfreezeはtarget/final-cover-spread-phase-b2r2-20260910、body patch SHA256 bc294a8d577a4c471c21fd66f1736eee79b04146978fdd7705315fb4e4ab8ad9、統合patch SHA256 4d35c2656de0f1ac38c37d3bf1214fb130fe887566632d5c547e3134706eb155。Remote11fileは引き続き不変。2回のcompile-redログも保存し、テストの実行失敗と区別する。

b2r2の`cargo test -p mimageviewer --lib final_cover`は26/26 PASS、8082 filtered、exit0（compile2分58秒、test2.21秒）。最終logは同freeze/logs/cargo-test-lib-final-cover.log、SHA256 04cc9ba5593585f502f5eb8702f2c77052b50424b3bb65b3de9a5bb786f41d93。親が実ログを確認した。masterのportable/test-script準備buildへCargo枠を返すため、次commandは起動せずcargo/rustc停止を担当が確認。残り個別回帰・core/Remote check・fmt/glyphは再貸与後に実行する。sourceは独立レビュー対象として凍結を維持する。

B2r2独立レビューでP1×1/P2×1を検出し、修正へ戻った。(1) 連結読みの通常Double第2ページをcurrentとして開くとphaseだけrequested anchorへrebindされ、actual unit/paintはcanonical先頭anchorのままになりexact all-Live観測が終端しない。current unitのtyped rebindをactual drawまで通し、通常Double第2ページの回帰を追加する。(2) external BothPagesが画面順になり既存RTLの読み順を逆転していた。既存external-tool-launch-planの契約を親も照合し、BothPagesはreading order、Mergedはscreen orderへ分離する。補助付きBothPagesは[last, front]であり、source idxの数値sortで[front, last]へ戻してはいけない。Cargo枠返却中にこの2件のソース・回帰を一括修正し、新freezeのdeltaを独立確認する。

snapshotは既存goldenを作り直さず3枚を追加する予定。末尾補助のLTR/RTLは異なる模様の両textureを用い、productionのcomposition・draw_fs_spread・ページ番号算出/overlayを通して位置とN/Nを確認する。設定は既存preferences snapshotでは新checkboxが画面外のため、checkboxと説明の表示だけを最小helperに分けてfocused dark snapshotで確認する。保存/invalidationのownerは移動しない。連結読みの重複occurrenceは既存production layoutを通す回帰で検証し、画面全体snapshotの追加は行わない。source準備は承認済み、headless実行とPNG確認は未実施。

B2r3は上記2修正・回帰と3snapshot fixtureを追加して凍結。target/final-cover-spread-phase-b2r3-20260910、body patch SHA256 a5ed94f007cf5c49588b77d5eb12e932bf2db476f3670d9766c8e60699332425、統合patch SHA256 31c0926fa338a0970ca27b04a76df6fc6bf459fbbe36e6942e71a1e4842b2d80。Remote11fileは既承認hashのまま。本体差分の独立再確認と狭域Cargoを開始し、sourceは再び凍結した。親から再貸与された枠はjobs1/既存target/非UIに限定し、GPU snapshot・fullgate・確認buildは別枠とする。親の実機確認が先行する場合はcommand終了区切りで返却する。

B2r3狭域検証完了。`final_cover`27件（P1の通常Double第2ページ回帰を含む）とfilter外8コマンド各1件がPASS、計35件。追加8件はexternal policy・display occurrence・continuous mixed owner・source keep・通常spread同値・Remote live settings・container上限2件。core binとmimageviewer-remoteのcargo check、cargo fmt --all -- --check、check_ui_glyphs.pyもexit0（危険文字0）。同freezeのMANIFEST.txt SHA256 372b341d4132ffc1ee3fd8e2b1a1a3aafe3a57fa6495fc0df40fb71cfa569f62に実行記録を保存。24 source hashは不変、Remote11fileも既承認hash一致。cargo/rustc停止確認後に親へ枠を返却した。独立レビューはsnapshot fixture/helper・4文書も含めP1/P2なしで静的承認。snapshot生成・PNG視認、shared fullgate、利用者build、実機は引き続き未実施。

追加snapshot3枚の生成とPNG視認を完了。実装担当と親がLTR/RTLの左右配置、両方4/4のページ番号、設定の文字・配置を確認した。UPDATE_SNAPSHOTS解除後の通常比較も2exact各1件PASS。sourceはB2r3から不変、新規PNGはtests/snapshots/final_cover_spread_ltr_last_page.png、final_cover_spread_rtl_last_page.png、preferences_final_cover_spread_setting.pngのみ。MANIFEST.txt最終SHA256 8ac25dd1a023da0cd9f187142e515d211dddf32d5c6c5d50f217cceb36a762e4b。cargo/rustc停止確認後にGPU/Cargo枠を返却した。この段階を機能branchの限定commitとして保存し、masterへの統合は行わない。shared fullgate・利用者build・実機は別段階で残す。

機能branchの限定commitはd70692837e0686d780e6a7f89c3bcf2b7b957372。直後の独立PNG確認でRTLのページ番号欠落が疑われたためfullgate開始前に一時停止したが、保存済み原本の同座標crop `(660,376)-(710,410)` はLTR/RTLともRGBA SHA256 a64cb827041fff379e313ab759206910f4f464a81c2f24bd15694a389a0d9d2e、白文字31px・bbox `(673,385)-(698,393)` で完全一致した。親の原寸再表示と独立reviewerのread-only画素照合でも両方4/4を確認し、指摘を撤回した。inline画像の目視判定による誤報であり、製品・fixture・PNGには変更を加えない。3snapshot承認と通常比較PASSは有効。訂正後、親から貸与された枠でshared fullgateへ進む。

fullgateは`test-full.ps1 -SuppressCrashDialogs`、CARGO_BUILD_JOBS=1、既存targetで実行する。初回launcherがPowerShell7のPSHOME下にpowershell.exeを探して起動前に失敗したため、WindowsPowerShellの既知絶対pathへ修正した。初回は製品テスト未実行としてtarget/final-cover-spread-full-gate-20260910/launcher-attempt1.txtに分離し、正常起動後はlogs/test-full.stdout.log・test-full.stderr.logへ保存する。crash suppression active・workspace cargo test開始・dupeのcargo/rustc実processを担当が確認してから開始済みとした。通常APPDATA索引processを停止しない。

fullgate成功後は親の条件付き承認により`build-dev.ps1`を使う。停止対象のdupe固有target/dev-runtime/core・remoteにresidentがないことをpreflightで確認し、存在する場合は停止せず報告する。master/通常APPDATAのprocessには触れない。新たなportableは作らず、通常featureのcore+Remote成果物を渡す。buildとagentによる起動は別であり、成果物は起動しない。利用者が起動するときは通常APPDATAprofileを使うことと、既存mIV終了が必要なことを明記する。

初回fullgateはexit101。本体は8061 passed / 7 failed / 43 ignored、実行324秒。後続workspace target/doctestは継続したがvendor3crateは本体失敗により未開始。cargo/rustc停止確認後に枠を返却し、buildは保留した。失敗は次の7件で、まだ一律にfixture追随と判断しない。

| 失敗test | 観測 | 原因確認の担当 |
| --- | --- | --- |
| batch_restore_database_opens_are_constant_for_one_and_hundred_candidates | inventory期待値追随。新storeでDB open count 29→30、候補数に依存しない契約は維持 | 実装担当 |
| migrates_folder_prefix_keys | 製品の旧schema互換漏れ。新tableなしでrename/copy/probeがerror | 新descriptorだけtyped optional-table契約。共通store境界の他descriptor/実在tableのSQL・破損・権限errorは従来どおり。旧/新tableを実経路で回帰 |
| copy_path_covers_all_unique_stores_for_image_zip_and_pdf_faces | inventory期待値追随。descriptor22→23、unique21→22 | 実装担当 |
| every_page_anchor_is_indexed_and_every_page_has_an_entry | 新設定の検索索引への実装漏れ | spread/final-cover anchorを既存検索indexへ追加 |
| preferences_viewer_notice_visibility_snapshot | 新設定増加でscrollbarの長さが10px変化、可視本文は同じ | 差の証拠を保持して既存golden1枚だけ更新 |
| navigation_topology_post_poll_rebind_replaces_same_frame_page_turn_decision_cache | fixture/API追随漏れ。test-only旧pair helperが本番の同frame decision cache更新を迂回 | 独立確認済み。本番composition helperをテストで通し、PassThrough/Deferred/true期待は保持 |
| spread_shift_capture_keeps_previous_pairing_for_both_directions_and_repeat | 製品P1。shift変更後のcanonical再計算が直前paintのpairをprevious captureへ保存しない | 独立確認済み。有効な描画済みlayoutのscreen-order role occurrenceを正本にし、layoutなし時のみcanonical。LTR/RTL・初回/反復の既存期待と補助captureを保持 |

製品不具合・fixture期待値・環境のどれかを根拠とともに分類し、focused回帰と独立delta確認後、未実施vendorも含む全gateを再実行する。既存のreadiness・旧schema互換・navigation identity・shift時の実表示保存を弱めて成功させない。

B2r4の4source修正を凍結してfocused開始。初回は追加test fixtureのE0502で実行前に停止し、screen_pagesを呼出し前に取得するtest-only修正後のpatch SHA256は28aa19b05aa3d5a8ff12b27e74bdcdff5e61eb9c16e8bdb1eecc962e2a4b41ee。validation-r1へ分けて再開した。独立reviewはoptional-table・inventory・検索index・post-poll修正を確認したが、painted layoutの寿命に追加P2を検出。layoutにはitems_generationがなく、同idxでのitems交換やclose→reopen後に古いrole/pairを再利用できる。新しい並列generation fieldを足さず、既存items世代変更と真のsession closeでlayoutを自context内で退役させる。同世代shift・park/一時非activeは保持し、世代交換/reopenでcanonicalへ戻る回帰と兄弟context保持を追加して再検収する。

B2r4-r1のfocusedはrestore件数・rename suite・設定検索suite・post-poll exactがPASS。spread-shiftはprevious pairが直った後、target actual `[2,1]` / expected `[1,2]` で失敗した。独立調査でPhase A以前はbegin/bind/rebindがsortしたcanonical navigation集合をidentityに使っていたと確認し、fixture追随ではなくPhase Aの製品P2と分類。typed demand constructorでNavigation投影だけcanonical sortし、presentationはscreen exactのまま保持する。observerもNavigation投影の照合だけcanonical化し、全presentationのrole/順序照合は弱めない。shift traceはtargetのpresentation順から作り、canonical navigation順を流用しない。外部BothPagesの読書順とこの内部identityのsortは別契約である。

B2r5は中央lifecycleのlayout退役とcanonical Navigation投影を加えて6sourceを凍結し、7exactすべて1/1 PASS（shift、補助painted capture、RTL navigation/presentation、generation、true close、context隔離、post-poll）。既存preferences snapshotも完全修飾exactで更新・通常比較ともrunning1 / PASS、fmt全体check exit0。golden差はscrollbar内22px、bbox `(544,76)-(551,79)` のみで本文pixel不変。親も新PNGを確認した。source/goldenを保存した最終freezeはtarget/final-cover-spread-phase-b2r6-20260910、MANIFEST SHA256 8acb48578b3a3596d586c9b18d5d2e6ff2716eb4ceaf601ce69b3499fd07f6b2、patch SHA256 30e6a3ec3750377d8177f629196360b7f908828975fb66b2f91cd4cf0c61286d。cargo/rustc停止確認後に枠返却、独立delta検収待ち。初回B2r4-r0のartifactはin-place更新され原本を保存できていなかったため、HISTORYにその制約・当時patch SHA・残存compile-red logを明記し、後から保存済みと扱わない。

B2r6の最終独立reviewはP1/P2なしで承認。中央setterはitems世代が変わる場合だけ自owner layoutを退役し、registry swapでcontext ownershipを保持する。真closeはmounted ownerだけ、park/mountやsource不変のcancel/errorへclearを追加しないことを確認した。Navigationのcanonical sortと全presentationのscreen exact照合、7exact・snapshot・fmt・6source/golden hashの一致も独立照合済み。限定commit後に未実施vendorを含む全gateを再実行する。

## 最終自動検証と確認用成果物（2026-09-10）

修正commitは`bc8aa8c210f3bc7b0b83a5bb1a17688e4216419d`。再fullgateは同commitのclean treeで`test-full.ps1 -SuppressCrashDialogs`を実行しexit0。本体8074 passed / 0 failed / 43 ignored、UI snapshot48、IPC55、Remote Web Rust118（1 ignored）、vendor egui25・egui-wgpu9・eframe15はいずれも失敗0。workspace残りtarget/doctestも成功し、末尾`[test-full] PASS`とerror mode復元を親が実ログで照合した。記録はtarget/final-cover-spread-full-gate-r2-20260910/MANIFEST.txt（SHA256 e87f6e79758c2dcd0a37c6c819461236ab7e0c34002a5831bea6ce8b8b2b8253）。先のNode388件・独立レビュー・focused・追加3snapshotの結果も有効で、コードを変更せず再利用した。

確認buildは同commitで`build-dev.ps1`、jobs1、通常feature（portable/test-scriptなし）、exit0。core10分41秒、Remote1分6秒。直前・直後ともdupeの出力pathにresident0を確認し、master側9processと通常APPDATA側4processの同identityを保持、成果物は起動していない。記録はtarget/final-cover-spread-build-dev-20260910/MANIFEST.txt（SHA256 81a42fac65f8f9c3e37131656da098132fa0aa7d9b6f77a6ef90281644083fc8）。

| 成果物 | bytes | SHA256 |
| --- | ---: | --- |
| target/dev-runtime/mimageviewer-core.exe | 309761024 | 299002ffc3e5f32c60115b7975a28f4954f0cccc45ff6c7f7f7d843bde2a0636 |
| target/dev-runtime/mimageviewer-remote.exe | 12197888 | 03ddde6fd4faee48f24d16ac24b9c9c9ef632114f71041e0aa780cf82ece7918 |

利用者が既存mIV（トレイ常駐含む）を終了してから、C:/home/mimageviewer-dupeで`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`を実行する。single-instance mutexを共有し、通常`%APPDATA%\mimageviewer`profileを使うため、実設定・データを更新し得る。agentは起動しない。実機確認は、両端が単ページになる表紙あり見開きの末尾でLTR/RTL・N/N、全体/本別設定、連結読み・F12・Remote、見開きずらしと本切替/close-reopenを確認する。masterへの統合・公開・version変更は行っておらず、実機結果と統合は後続判断である。

## 後続の受入確認

| 境界 | 必須の確認 |
| --- | --- |
| 共通構成 | LTR/RTL、両端Single、1ページ本、末尾Double、横長/保存回転/ずらし、OFF、非Cover、完全列条件。navigation列とページ数を保存 |
| 本体の需要 | paged/連結読み、補助を含むatomic ready/failure/cancel、同じ表紙の別occurrence、保持対象のunion、park/activate/dropと本切替の隔離 |
| 既存操作 | N/N表示、End/Home/seek/resume/history、pointer側編集と末尾への復帰、external Main/Both/Merged、保存/補正/AI/PDF需要 |
| 設定 | global既定ON、exact overrideとnested FollowGlobal、旧read-only schema非移行、cover-onlyで既存spreads不変、rename/import/clear、別context隔離 |
| Remote server/wire | folder/ZIP/PDFの完全性、truncated/collectionで補助なし、live global反映、旧payload fallback、typed書込、protocol更新、補助あり長path応答が64MiB以内 |
| Remote Web | 最初と末尾に現れる表紙のslot context区別、左右viewtrim/調整/AI、末尾からの近傍prefetch、補助も含むall-ready/all-fail、設定reloadでanchor保存 |
| 最終成果物 | 既存snapshot更新とPNG確認、排他調整したshared gateと確認build、実機結果は別記。通常profileのagent起動禁止 |

上表は予定であり、実施済みを示さない。各検証の実行者・対象差分・コマンド・結果・ログは区切りごとに追記し、他担当が同じgateを無条件に再実行しない。
