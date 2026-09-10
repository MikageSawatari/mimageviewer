# 末尾に表紙を添える見開き表示

## 状態と開発境界

2026-09-10、利用者依頼により調査・設計着手。基準masterは4a60d770c49ecce1792031ae388af9a92eb9e5c9、作業場所はC:/home/mimageviewer-dupe、機能branchはcodex/final-cover-spread。duplicate-detectionの履歴を保持する。v3.8.0の公開には未完成の変更を混ぜず、masterへのmerge/push/公開/version変更は行わない。

親が設計・文書、implement_resume（Sol/xhigh）が唯一のsource/test writer、review_resume（別Sol/xhigh）が独立設計・完成差分レビューを担当。旧ClaudeCode設計検収指定は現AGENTSの開発役割へ移管し、公開責任は変更しない。大きなCargo/GPU処理はmaster親タスクとの所有調整後に行う。アプリ操作は個別suiteの明示了承後のみ、通常profileのagent起動や実データ変更は行わない。

### 実装と検証の区切り

| 段階 | 成果物 | 現在の状態 |
| --- | --- | --- |
| A 共通基盤 | role/occurrence/composition、phase所有demand、global既定ON、本override別table、context・metadata・rename境界 | 実装・狭域テスト13件・fmt・独立レビュー完了 |
| B 本体表示 | paged/連結読みの描画・需要・保持・失敗終端、編集復帰、HUD・各操作のidentity接続、設定UI | 未完了 |
| C Remote | sparse presentation wire、live設定snapshot、typed設定書込、Webのnavと描画分離、protocol互換 | 設計調査完了、未実装 |
| D 最終検証 | 各段階の回帰・snapshot、shared full gate、利用者確認用build、実機確認 | 未実施 |

Aの成功だけでは機能完成・利用者検証可能とは扱わない。各段階のsource変更は実装担当、凍結差分レビューは別担当とし、既存検証を重複実行しない。Cargo枠はAのcheckと狭い回帰に限定して借用し、結果とログを共有して返却する。次段階の重い検証はあらためて所有調整する。

Phase A凍結後、本体表示とRemote実装を分担する予定。Remote担当候補はremote_cover_designで、crates/remote-ipc、crates/remote-web、src/remote_ipcを所有する。settings_dbのlive snapshot APIと共通compositionの公開境界を確定してから編集開始を指示する。それまではsource writerを増やさない。共有settings/spread_db/metadata/renameはmain実装担当の所有を維持する。

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

最終再検証は同じ11 source hashで完了。`cargo test -p mimageviewer --lib final_cover -- --nocapture` は12/12 PASS（final-cover-tests-green-2.*）、通常見開き同値回帰1件は1/1 PASS（spread-role-equivalence.*）、`cargo fmt --check` はexit 0（cargo-fmt-check.*）。独立Solが実ログ・hashを照合しP1/P2なしでPhase Aを最終承認した。これは共通基盤のcheckpointであり、機能全体のfull gate・利用者build・実機確認は未実施。

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
