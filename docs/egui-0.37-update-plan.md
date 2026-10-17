> 状態: 設計案 第4.6版 (2026-10-17、段3 の 2 件の P1 (閉じた後の表示継続の owner、discard と immediate の間の delta の年代順) を埋める追補。独立レビュー前)。第4.5版までは独立レビュー ACCEPT。目標は利用者決定により egui 0.37 系。正本の backlog は §1.382。

# 改訂履歴

## 第4.6版 — 2026-10-17

対象は第4.5版とWIPコミット`fe32f2e88`。段3台帳の「再開後の設計担当へ返す論点」全文・補修追記と、Close、bookmark／navigation、discard／immediateのdelta経路を読み取り専用で照合した。第4.5版の型付き退役と既存の独立設計ACCEPTは維持する。本追補の実装・独立レビュー・テスト・製品起動は未実施。

- **P1 Close後の表示継続 — 反映:** teardown前に復帰先と必要な所有値を捕捉し、存続するmainの既存navigation帰還ownerへ型付きで移す。bookmark一覧更新・最後の実フォルダ復帰・auto-openの親一覧復帰を、次の正当なUI消費位置へ渡す。Remote取得、新navigation、新bookmark開始による失効も同じownerで扱う。
- **P1 discard／immediate間のdelta順 — 反映:** 候補(a)を採用する。実際にdiscard再passへ進む直前、生成済みdeltaを既存backend共有renderer ownerへ一度だけ移譲し、可能な場合は既存の無描画resource consumerで完結する。後発freeが旧set／partialを追い越さないようにする。
- **候補(b) TextureManagerへの再預託 — 不採用:** metadataを変えないprepend APIのためのepaint追加patch、年代順合流、output pluginによる再観測の契約が必要になる。今回はepaintと既存plugin観測を変えず、integrationの所有受渡しを限定追加する。

dirty表示採用のP1は既存owner内で補修済みとして区別し、今回の2件と混同しない。全targetのコンパイル・既存filterの成功は段3台帳の条件付き証拠として再利用するが、段3全体のACCEPTや今回の2件の解消証拠にはしない。

追加範囲は段3で完成させ、detached計画§11へ終了継続とdelta所有の変更を追記する。新しいpending bool、第二queue、遅延cleanup、追加retry、時間窓、強制UI、後付けguardは追加しない。

## 第4.5版 — 2026-10-17

対象は第4.4版と、段3途中のWIPコミット`df0037eb7`。`docs/egui-0.37-stage3-verification.md`の「設計担当へ返す論点」全文と、App・egui・eframeの退役経路を読み取り専用で照合した。段0〜段2のACCEPTは維持するが、今回判明した受渡し不足の解消までを既存のACCEPTに含めない。本追補の実装・独立レビュー・テスト・実機確認は未実施。

- **論点1 mode changeのUI依存 — 反映:** 空の`show_viewport_immediate`と非描画によるGC待ちを、承認済み退役の型付きcommandへ置き換える。logic／uiで同じ終了入口を使う。
- **論点2 App owner終了後のClose再要求 — 反映:** modal等の終了判断が確定し、window／context bindingを失う前に、exactなviewport寿命と子孫への退役権限を出力へ一度だけ移譲する。通常の`ViewportCommand::Close`で代用しない。
- **論点3 ViewportIdだけのhook接続 — 反映:** OS Closeの要求受付と、承認済み退役の実行を分離する。後者はAppの終了済みbindingを再検索せず、型付きcommandの対象を使う。
- **論点4 Full内logicからの直接退役 — 反映:** Appは承認済みcommandを発行するだけとし、Context・native資源の退役は最外側のUI／logic処理が戻ったpass外で行う。
- **論点5 GCによるcommand消失・完了通知欠落 — 反映:** 子のGCに依存しない既存root command出力へ保持し、Full／LogicOnlyとpass外hookの全経路で配送する。通常GCと承認済み退役は共通cleanupを使い、AppへのClose完了通知を唯一の完了根拠にしない。

候補(A)の「AppのClosingとbindingをbackend受理まで延命」は不採用とする。候補(B)の「既存viewport command出力への承認済み退役の追加」を採用し、Appの終了ownerを延命せず、出力への権限移譲後はbackendが資源寿命を完結させる。

追加差分は段3の残作業として計上する。段2の完了記録を遡って書き換えず、正式vendor・App接続・回帰を追加レビューする。detached計画§11への追記と構造合意を実装再開の前提とする。

## 第4.4版 — 2026-10-16

対象は第4.3版、HEAD `b14571d8d`。Sol / xhighのレビュー全文と、root Close、context mount、両backendのimmediate viewport実装を読み取り専用で照合した。案Bとscheduler方針変更が構造修正であるとのレビュー合意を維持する。本版の実装・独立レビュー・ビルド・テスト・製品起動は未実施。並行中の段1b-2のgamepad実装は変更・審査していない。

- **P1 root Close — 反映:** トレイ置換、保存待ち、settings-family、ZIP取消等の既存終了判断を、通常入力から独立したroot lifecycle処理へ集約する。最新のviewport Close情報を使い、logic-onlyでもCancelCloseと完了後の終了再開を行う。
- **P2 非描画mount — 反映:** 保存receiptはまずmount不要のowner付き更新へ寄せる。必要な一時mountではpayload交換と表示再採用・reconciliationを分離し、logicから既存UI mount wrapperを呼ばない。別contextのreceiptと未反映ratingを用いる回帰を追加する。
- **P2 immediate子描画 — 反映:** wg​​pu／glowの`render_immediate_viewport`を案Bの対象へ明記する。子自身のmetadataと明示描画要求で描画資格を決め、非描画時も出力を完結させる。

段1b-3、段2c、段3の範囲・受入条件・見積りを更新する。新しいpending bool、表示副作用を抑える後付けguard、追加repaint、強制UI、retryは追加しない。

## 第4.3版 — 2026-10-16

対象は第4.2版、HEAD `9453bd3c9`。利用者が2026-10-16に採択した案Bと、段1b検証台帳のC1〜C5・D3を反映する。先に提示した案A前提の第4.3版追補は未採用であり、本版へ取り込まない。本追補の実装・独立レビュー・ビルド・テスト・製品起動は未実施。

- **案B — 反映:** Windowsの最小化中も、要求済みFullをUI passとして消化する。UI実行とsurface描画の条件を分け、`is_visible`を偽装しない。要求なしのsleep、既存のrepaint間隔・outer phaseを維持する。
- **期限超過 — 維持:** RedrawRequestedの期限超過によるlogic-onlyは残す。これをFullや強制描画へ置き換えない。
- **出力契約 — 反映:** UIを実行して描画しない場合もtexture delta・platform output・viewport commandsを完結させる。Cut／Copy／Pasteを描画分岐から分離し、screenshot等の明示描画要求と実表示ACKの条件を保持する。wgpu／glowを揃える。
- **C1 — 反映:** Remoteの取得・受信・取消・保存完了・ACK・drainはlogicで進める。PC表示継続だけを既存Remote ownerへ残す。
- **C2〜C5 — 反映:** C2・C4・C5とC3の表示用pollは、既存owner・旧順序のままuiに残す。通常の最小化中は進め、期限超過のlogic-only中は次の対象Fullまで保留する。保存・取消・寿命終端はlogicへ分ける。
- **D3 — 反映:** hydration要求IDをframe／input計装から独立させ、既存hydration ownerで一度だけ採番する。

第4.2版の保留RawInput、IME、非表示子Close、texture delta・surfaceの所有契約は維持する。scheduler方針の変更と残る期限超過経路をdetached計画§11の構造合意へ追記する前提とし、段1b・段2c・段3の範囲、回帰、見積りを更新する。

## 第4.2版 — 2026-10-16

対象は第4.1版、HEAD `ea54a4ca0`。現在の設計書、Astra再確認全文、本家・mIVの該当ソースを読み取り専用で照合した。前回のIME中断要求2件の対応を維持する。本版の独立レビュー、実装、ビルド、テスト、製品起動は未実施。

- **Astra再確認 P1 — 反映:** 最小化した子のCloseを、UI復帰待ちの通常入力から分ける。非描画の受付から既存のmodal判断・終了ownerへ一度だけ渡し、拒否もその要求の消費として扱う。承認後は子callbackや親の描画passを待たず対象viewportを退役させる。wgpu／glow、App接続、構造合意・§11記録、回帰・実機手順・見積りへ反映。

タスクバーから最小化した別窓を閉じる操作は通常利用で到達するため、まれな失敗として割り切らない。追加repaint、最小化中の子へのUI pass強制、新しいpending boolや時間窓は使わない。

## 第4.1版 — 2026-10-16

対象は第4版、HEAD `535968858`。Astra再確認全文と本家・mIVの該当ソースを読み取り専用で照合した。前回4件の対応を維持し、追加2件を反映する。本版の独立レビュー、実装、ビルド、テスト、製品起動は未実施。

- **Astra再確認 P1 — 反映:** Memory全体のIME中断要求を既存のviewport別Focusへ移管する。親だけ／子だけが要求する同一Contextの入れ子run_ui回帰を追加し、detached構造合意・§11記録の範囲へ含める。
- **Astra再確認 P2 — 反映:** 同一viewportの未配送PlatformOutputを集約するとき、最新のIME矩形等を採用しながら中断要求を保持する。実Context::run_ui内でdiscardする回帰を段2aへ追加する。

App側の補助フラグ、追加repaint、新しいqueue、retryは追加しない。段2aと段3の見積りを更新する。

## 第4版 — 2026-10-16

対象は第3.1版、HEAD `4e097de68`。現在の設計書、Astra追加レビュー全文、該当するmIV・本家ソースを読み取り専用で照合した。本版の独立レビュー、実装、ビルド、テスト、製品起動は未実施。

- **Astra P1-1 — 反映:** 単一の`pending_raw_input`をそのまま使う方針を訂正。既存の保留領域をviewport ownerへ移し、同じviewportだけが消費する。skipする子はStateから入力を取り出さない。交互実行・子のskip後復帰の回帰と、detached構造変更の合意・§11記録を追加。
- **Astra P1-2 — 反映:** 本家0.37が捨てる`winit::Ime::Disabled`を正規化まで届ける互換処理を追加。段2aの冒頭で、State変換から実TextEditまでの取消回帰を成立させる。
- **Astra P2-3 — 反映:** native経路のIME中断出力を、既存`NativeWindowIntent`・control配送・対象HWND ownerで処理する契約を追加。非対話回帰と実機手順を追加。
- **Astra P2-4 — 反映:** release射影の保証と本家クリック分類を分離。連続クリックの距離条件6 points・時間条件変更を相談事項へ追加し、旧判定や別タイマーを足さずfixtureを更新する方針を明記。

4件とも採用する。P1-1の修正は保留入力の所有境界に限定し、第2版で提案した汎用入力フィルタ・終端配送・一律pruneは復活させない。

## 第3.1版 — 2026-10-16

第3版の独立レビューはACCEPT WITH CHANGES。設計担当が次の2件を直接反映した。

- **第3版レビュー P2-1 — 反映:** 復帰後は保持中のgamepad軸値で操作を続ける現行挙動を保持。新しい中立通過要求は採らず、変更する場合は利用者相談とする。
- **第3版レビュー P2-2 — 反映:** logicでdrainした未配送edge・意味終端を同じ物理取込みownerで保持し、UI配送へ引き継ぐ。複数logicに分かれたpress→release、追跡ON／OFF、multi-passの既存契約を回帰対象へ追加。

本版でもこの2件を維持する。

## 第3版 — 2026-10-16

対象は第2版`b693945bb`。

- **再レビュー P2-1 — 反映:** SW_HIDE、最小化、visible child、RedrawRequested期限超過を区別。全入力共通の受付・終端配送とeguiへの汎用終端API追加を撤回。
- **再レビュー P2-2 — 反映:** gamepadの物理取込みとUI配送を分離し、WH_GETMESSAGE由来pendingを入力源一覧へ追加。
- **再レビュー P2-3 — 反映:** hide時の`pending`／`frame`／`operation_frame`一律pruneを撤回。受信時点のviewport所有とfocus喪失後も有効な離散キーを保持。

「未消費RawInputは保持する」という方針は維持する。ただし「本家の単一保留領域を変更しない」という第3版の実装方針は、第4版で訂正する。

## 第2版 — 2026-10-16

対象は初版`226985787`。

- **P1-1 — 反映:** TexturesDeltaのdrain・引継ぎ・意図的破棄を経路別に定義し、段3の必須移行へ追加。
- **P1-2 — 反映後に訂正:** 汎用の非表示受付・終端機構を設計したが、第3版で撤回。第4版では実在するviewport間の保留所有違反を限定修正する。
- **P2-3 — 反映:** 無描画配送のsubmissionと適用後freeを追加。
- **P2-4 — 反映:** unsmoothed wheelの取得元・換算・修飾・消費順を定義。
- **P2-5 — 反映:** 通常窓・native入力欄のCtrl+Shift+C/X/V保持を推奨。
- **P2-6 — 反映:** 実ウィンドウ試験を自動gateから分離する準備段を追加。
- **P2-7 — 反映:** accessibility先行、両integration targetとlib snapshotの更新を必須化。
- **P3-8 — 反映:** 通常版immediate viewport計測を独立したvendor保持項目へ追加。
- **P3-9 — 反映:** 0.33.3上の準備段を分割。固定票とPNGごとの重複記録を撤去。

# egui 0.37 更新の影響調査と段階設計

## 0. 調査範囲と結論

調査・改訂日: 2026-10-16  
対象: `G:\home\mimageviewer-egui036`、ブランチ`egui-036`  
製品ソースの基準: `05b6a15de26897369a35e5aa00377d174ca3760d`  
今回照合時HEAD: `4e097de68`

リポジトリ、指定された更新履歴要約、Cargo registryの0.33.3／0.37.0、Rust 1.99の既存checkログ、各版の設計書とレビューを読み取り専用で照合した。ファイル変更、依存解決、ビルド、テスト、ネットワーク利用、製品起動は行っていない。

実行時影響はコードからの見立てである。既存文書に記録された他担当の結果と、今回のソース確認を区別する。

目標は利用者決定によりegui系0.37.0。実装開始時に0.37.xのパッチ版へ合わせる場合は、その版で本書の判断を再照合する。

主な結論:

1. Appの責務分離、root Ui、入力イベント、texture delta、wgpu surfaceの移行が必要。
2. `vendor/egui-winit`は削除できない。Hidden／Forwarded配送保証に加え、Disabled取消終端の互換処理が必要。
3. `vendor/eframe`も削除できない。既存差分のほか、本家の保留RawInputをviewport別の所有へ直す。
4. `ui_fonts.rs`の外形・行計測は既にttf-parser。ab_glyphを直接使うフォントプレビューとは分ける。
5. 動画本体はD3D11 native presenter。wgpu更新の主対象はHUD overlay、比較、panorama、静止画縮小。
6. `raw_scroll_delta`は、消費位置で未消費のeventsから作るunsmoothed投影へ移す。
7. TexturesDeltaのdrain・引継ぎ・submission・freeとheadless明示破棄は、段3の必須契約とする。
8. SW_HIDEと最小化は同じ経路ではない。汎用の非表示入力フィルタは追加しない。
9. gamepadは物理状態更新と操作配送を分離する。離散pressの遅延採用を防ぎ、保持軸の復帰後継続は維持する。
10. native経路はIME cursor位置だけでなく、composition中断要求もOSへ届ける。
11. 連続クリックに距離条件が加わる。release射影は本家分類をそのまま返し、操作差は利用者相談とする。
12. snapshotは445枚。accessibility修正後、両integration targetとlibを更新する。
13. f32 fallback警告は既存ログで445件。旧版上の一括修正はせず、更新後に残りを整理する。

### 参照表記

本家の`egui-0.37.0/src/...:line`等は、次のregistry rootからの相対パス。

```text
C:\Users\mikag\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\
```

mIVの行番号は今回照合した製品ソースに対するもの。件数は特記しない限り`src`、`crates`、`tests`、`tools`のRustソースの一致行数で、コメントを含む。ASTによる呼出数ではない。

## 1. 不変条件と簡素化方針

- 必要なwakeに応じて、非表示でもトレイ復帰、非同期完了、Remote、保存終端、メディア制御を進める。常時tickは追加しない。
- 未消費入力は、受信したviewportだけが消費する。rootの保留をchildへ渡さず、skipしたchildの入力を落とさない。
- 正当に受信済みの入力は、所有者と順序を保つ。未消費という理由だけで失効させない。
- viewportへ配送済みの離散キーは、後のfocus喪失だけで取消さない。現在のキーlevelを読む許可とは分ける。
- release、focus、IME終端、復帰後の新規入力、兄弟viewportの入力を保持する。
- IME取消はproducerから正規化・TextEditまで到達させる。nativeの中断要求も対象HWNDへ届ける。
- gamepadの物理drain・release・軸・disconnectを止めない。未配送edgeと意味終端を最新状態で上書きしない。
- gamepadの離散press抑止と、保持軸による復帰後継続を区別する。
- viewportごとの入力、IME、cache、texture、要求世代、workerの所有を維持する。
- popup所有wheelは、終了frameと平滑化残量を含めて背面へ漏らさない。
- texture deltaを所有者不在にしない。uploadはsubmitまで完結させ、使用中textureをsubmit前にfreeしない。
- release射影は本家の分類結果を返す。別の連続クリック判定を作らない。
- フォントfallback、選択設定、リリース済み機能・データを保持する。
- snapshot更新で操作・到達性・所有権の回帰を承認しない。

状態の組合せを減らす案の比較:

| 対象 | 採った案と理由 | 採らなかった案と理由 |
|---|---|---|
| 非表示処理 | serviceをlogicへ分離し、入力処理は既存ownerへ残す | 全入力共通の「通常／終端のみ」配送。必要性がなく状態を増やす |
| 未消費RawInput | 既存保留領域をviewport ownerへ移す。skipするchildはStateからtakeしない | 単一保留領域＋viewport guard、App側の第二queue。誤配送元が残る |
| 受信済みキー | event時点のviewport所有と既存permitを保持 | hide時の一律prune。focus喪失後のdiscrete採用契約を壊す |
| IME取消 | State変換から既存正規化へ終端を届ける | timeout、focus喪失時の即時一律取消。正当な後続確定を失う |
| native IME出力 | 既存intent・control配送・HWND ownerを利用 | rendererから直接Win32呼出、新pending flag、上書き可能snapshotだけへの格納 |
| gamepad | 一つの物理ownerと既存batch配送を接続 | 全入力源への受付層拡張、復帰時のforegroundだけによる選別 |
| クリック | 本家分類と射影保証を分離 | 旧期待値を通すための旧判定、別タイマー |
| texture delta | 描画・無描画を共通の配送・完結処理へ揃える | 借用適用後の無条件clear |
| wheel | 消費位置の未消費eventsから投影 | raw snapshot復元cache。消費済み入力が復活する |
| snapshot | 画面群の共通理由と例外を一台帳で管理 | PNGごとの重複票、固定数のレビュー票 |
| 長い変換・保存 | 既存モーダル境界を維持 | 更新専用の並行・回復経路 |
| 非表示中のUI | 本家のpass省略と既存wakeを使う | UIを回し続ける、窓を再生成して入力を捨てる |

新しいpending bool、時間窓、retry、追加repaint、silent fallbackは設けない。RawInput保留の修正は既存領域の所有移管であり、保留経路を二重化しない。native IMEのintentは一回の命令であり、Appの保持状態へ重ねない。

まれな失敗は割り切り、ログ＋通知［＋作り直し］を基本とする。更新専用の回復機構は作らない。

### 第4.3版: 案Bによる非表示処理の具体化

本節の「本家のpass省略を使う」は、Windowsの最小化によるUI省略には適用しない。利用者採択の案Bにより、要求済みFullは最小化中もUI passとして消化し、surfaceへ描画する条件は別に維持する。RedrawRequested期限超過のlogic-onlyは本家どおり残す。

「worker結果をlogicへ分ける」は、表示継続へ到達するすべてのpollをlogicへ移すという意味にはしない。C1〜C5・D3は§4.2の個別境界に従う。

| 対象 | 採る案と理由 | 採らない案と理由 |
|---|---|---|
| Windows最小化 | 要求済みFullを従来と同じUI進行へ揃える。ローカル表示準備を復元待ちにしない | 最小化中のUI省略に合わせて、各表示継続を細分化する案A。通常操作の進行条件を変え、受渡し状態を増やす |
| UIとsurface描画 | UI実行資格と描画資格を分ける。不可視metadataと実表示ACKを保持する | `is_visible`をtrueへ偽装する。不要な描画や誤った表示確定を招く |
| 期限超過 | logic-onlyを維持し、Remote・保存・取消終端だけを非描画で進める | 期限超過を無条件Fullへ変える。presentがblockし得る経路まで描画することになる |
| C2・C4・C5とC3の表示用poll | 既存receiver・要求ownerのままuiへ残す。受信から表示継続まで同じ入口で進める | logicで全結果を取り出す共通表示待ちqueue、別のdecision cache、入力snapshot |
| C1と共有終端 | Remoteサービス進行、送信済み保存、破棄済み要求の終端をlogicへ分ける | 最小化中にUIが動くことを理由に、期限超過中のRemote・保存・終了までuiへ依存させる |
| D3 | hydration要求ownerに単一の採番を持たせる | frame_counterやinput_seqを要求IDの発行源にする |

C4の進捗だけをlogicへ移す分割は必須としない。完成採用からeditor再開までの既存経路を保つ方を先に採る。

新しいpending bool、時間窓、常時tick、追加repaint、logicからの強制UI呼出しは設けない。最小化中のUI継続はbackendの実行方針であり、Appがlogicからuiを呼ぶ回避策ではない。

第4.4版では、次の簡素化を先に採る。

| 対象 | 採る案と理由 | 採らない案と理由 |
|---|---|---|
| root Close | 通常の終了判断と既存operationの延期・取消・終端を一つのlifecycle入口へ揃える | logic-only専用の終了判断、uiの旧Close処理との二重受付、別のroot close待ちbool |
| 保存receipt | receiptが持つpath／identityを既存App-global writerへ渡し、editor状態はowner付きで直接更新する | receiptのためだけにviewerをmountし、関係のないratingや表示を再採用する |
| 非描画mount | 必要なサービスだけが既存registryのpayload交換を使う。表示境界はUI wrapperへ残す | `in_logic`等のflagで従来mountの副作用を後から抑止する |
| immediate子 | root／deferredと同じ出力・action契約へ揃え、描画資格は各子で判定する | 親の可視性を子の描画資格として流用する、immediateだけ常時描画する |

既存modalの排他とoperation ownerを保ち、終了待ちのために新たな並行操作の組合せを作らない。まれなworker／資源処理失敗は既存のログ・通知・終端へ揃え、終了要求専用の回復機構は設けない。

### 第4.5版: 承認済み退役の受渡しを一つにする

本節の候補(A)／(B)は、最小化中のUI継続について利用者が選んだ「案B」とは別の比較である。最小化中のUI継続、期限超過時のLogicOnly、C1と保存・取消終端のlogic分離は変更しない。

| 候補 | 必要になる寿命・状態 | 判断 |
|---|---|---|
| (A) Appの`Closing`とwindow／context bindingをbackend受理まで存続させる | App終了、native終了、通常GC、承認通知の前後をApp runtimeが扱う。worker／media終了後も同定用のownerを残す必要がある | 不採用。既存終了ownerの寿命を広く延ばし、Close hookと通常GCの双方からの完了を扱う組合せが増える |
| (B) 承認済み退役を既存viewport command出力へ移譲する | Appは既存終了判断を一度行い、対象の資源退役権限を出力へ渡す。backendはpass外で資源寿命を完結させる | 採用。Appの登録解除後に再同定・再承認せず、Full／LogicOnly／OS Close後のcleanupを一つに揃えられる |

単に子へ通常の`Close`を送る案、子を描かず次FullのGCを待つ案、Appから`Context::retire_viewport`を直接呼ぶ案では、今回の不足を解消できない。Fullの`App::logic`はUI pass内であり、子のcommandはeguiのGCで出力前に失われ得る。

採用する(B)では、既存root command列を承認済み退役の搬送先にする。rootを閉じる命令ではなく、子のexactな寿命を対象とする終端出力である。子自身の登録がGCされても搬送先は失われない。

Appのruntime、command、backend資源に同じ「終了待ち」を重複保持しない。権限移譲後のApp runtimeをbackend ACK待ちで残さず、第二の保留queue、pending bool、退役済みID集合による抑止ラッチを追加しない。

古い出力を識別するため、存続中のviewport登録には、その登録寿命を表す不透明な識別子を持たせる。Context登録、ViewportOutput、backend資源、必要な既存App bindingで同じ識別子を参照する。これは生存資源の同定であり、退役履歴の保存ではない。`ViewportId`、egui memoryの永続Id、surfaceのresize generationとは区別する。同じViewportIdを明示的に開き直した場合も、旧寿命への出力で新しい窓を変更しない。

通常の操作での終了・Remote取得・兄弟窓の維持を構造で保証する。まれな失敗は既定のログ＋通知［＋作り直し］で割り切り、退役のretryや回復用状態は追加しない。

### 第4.6版: 終端後の表示とdiscard中のdeltaの所有

状態の組合せを増やす前に、既存ownerへ受渡しを集約する案を比較する。

| 対象 | 採った案と理由 | 採らなかった案と理由 |
|---|---|---|
| Close後の表示継続 | 存続するmainの既存navigation帰還ownerへ、teardown前に型付きで移す。bookmark由来の所有値も同じ帰還要求へ移す | viewer runtimeやClosingの延命。表示復帰のためだけに終了済みcontextを残すと、backend退役・Remote・新navigationとの組合せが増える |
| 帰還要求の表現 | 既存`pending_return_to_parent`のboolを型付き状態へ置き換える。既存のnavigation合流点で消費する | bookmark専用の終端slotを追加し、auto-open用帰還予約と並存させる。二つのownerの優先・取消・上書きが必要になる |
| discard中のdelta | 実再passの直前にintegrationへ先行移譲し、既存renderer ownerで処理する候補(a) | TextureManagerへ戻す候補(b)。epaintの新API、非相殺合流、plugin再観測の契約が増える |
| native callbackの破棄 | 既存の登録寿命置換・cleanupを維持し、先行deltaの受渡しを直す | callback破棄を最外側終了まで遅らせる。資源寿命と表示採用を再び結び付ける |
| 新操作との競合 | 既存navigation／bookmarkのadmissionで古い帰還要求を失効・置換する | UI復帰時の現在path比較や追加generation guardで古い要求を見分ける。新操作を受け付けたownerで失効させる方が単純である |

`BookmarkViewState`へ終端継続variantを追加する案も検討した。今回はbookmark以外のauto-open帰還も同時に修復するため、既存navigation帰還slotを型付き化する方を採る。`BookmarkViewState`が持つtarget／gridは、その帰還要求へ移し、同じCloseを二つのownerに保持しない。

第4.5版の承認済み退役は資源寿命の終端を担う。今回の帰還要求は、終了したviewerに代わって一覧表示を継続する存続ownerの要求である。backendの退役ACK待ち状態として扱わず、表示が保留されても資源cleanupを遅らせない。

deltaのintegration callbackは資源配送だけを担う。App、navigation、UI、platform output、viewport lifecycleを再実行する入口にはしない。既存discardの回数を増やさず、描画・present・repaintを追加しない。

まれな失敗は既定のログ＋通知［＋作り直し］で割り切り、帰還やGPU処理の再試行機構は作らない。

## 2. 前提・更新履歴要約との相違

| 前提・要約 | ソース確認 | 反映 |
|---|---|---|
| egui-winitは削除できる見込み | Hidden／Forwardedとの差に加え、本家はDisabledを捨てる | vendor維持 |
| eframeはhidden repaint修正だけ | scheduler、visible commit、delta、通常版計測、診断がある | 全差分を台帳化 |
| 本家の未消費RawInput保持をそのまま使える | EpiIntegrationに一つだけあり、deferred childも取り出す | viewport所有へ移管 |
| childはUIをskipしても入力が残る | take後に保存せず戻る経路がある | skip時はtakeしない |
| ui_fontsはab_glyph計測 | 外形・行計測はttf-parser。previewがab_glyph | 別々に移行 |
| `CentralPanel::show`廃止 | Contextを受ける形がなくなり、Ui版は残る | 親Uiを移行 |
| `AsId`削除 | 0.37.0では非推奨として残る | 削除済みと書かない |
| Windowはタイトルバーのみ移動 | 既定OnTouchではタッチ時に本文からも移動する | 入力機器別に確認 |
| clipboardは余分な修飾キーなし | X/C/Vは`command_only()` | Ctrl+Shift操作を保持 |
| texture deltaはmap化だけ | 非空drop assertionと明示破棄APIもある | 消費契約を必須移行 |
| 非表示はすべてlogic-only | SW_HIDE、最小化、期限超過で異なる | 条件表で区別 |
| native IME出力はcursor位置だけ | 0.37は中断要求も出す | intent配送を追加 |
| 連続クリックは時間条件だけ | クリック間距離とtripleの時間条件が変わる | 相談・fixture更新 |
| snapshot約350枚 | 追跡PNG445枚 | 工数を修正 |

根拠: `egui-winit-0.37.0/src/lib.rs:656`、`:785`、`:1277`、`:1538`、`eframe-0.37.0/src/native/epi_integration.rs:188`、`:337`、`:384`、`:418`、`src/native/wgpu_integration.rs:700`、`:716`、`egui-0.37.0/src/data/input/raw_input.rs:162`、`src/input_state/mod.rs:1385`。

## 3. 影響件数

| 検索対象 | 一致行／ファイル | 代表箇所 |
|---|---:|---|
| `\bId::new\s*\(` | 571／84 | `crates/egui-ime/src/lib.rs:159` |
| `ui\.id\(\)` | 99／5 | `src/ui_music_panels.rs:706` |
| `\bSidePanel::` | 9／5 | `src/ui_folder_pane.rs:218` |
| `\bTopBottomPanel::` | 23／9 | `src/ui_main.rs:7400` |
| `\bCentralPanel::` | 339／60 | `src/ui_main.rs:17580` |
| `show_viewport_(immediate\|deferred)` | 38／8 | `src/ui_fullscreen.rs:22139` |
| `raw_scroll_delta` | 58／16 | `src/app.rs:58760` |
| `Event::MouseWheel\s*\{` | 103／20 | `src/video/native_presenter/render_core.rs:9751`付近 |
| `impl eframe::App` | 6／6 | `src/app.rs:97613` |
| `fetch_update\(` | 製品・テスト14／12、別途vendor 1件 | `src/log_writer.rs:105` |

```powershell
rg -n '\bId::new\s*\(|ui\.id\(\)|\bAsId\b' src crates tests tools -g '*.rs'
rg -n 'SidePanel::|TopBottomPanel::|CentralPanel::|show_viewport_' src crates tests tools -g '*.rs'
rg -n 'RawInput|raw_scroll_delta|Event::MouseWheel|\.modifiers' src crates tests tools -g '*.rs'
rg -n 'textures_delta|drop_without_applying_deltas|\.run\(|run_ui|run_pass' src crates tests tools vendor -g '*.rs'
rg -n 'sample_gamepad_input|dispatch_gamepad_batch|suppress_pending_actions|take_pending_mouse_nav' src -g '*.rs'
rg -n 'ImeEvent::|WindowEvent::Ime|NativeWindowIntent|ime_cursor_area_intent' src crates vendor/egui-winit -g '*.rs'
rg -n 'double_clicked\(|triple_clicked\(|released_buttons_with_click_counts|click_count' src vendor/egui -g '*.rs'
```

`.run(`の1274一致行／130ファイルにはHarness・workerも含むため、Context移行件数には使わない。FullOutput破棄、IME入力・出力、クリックconsumerは呼出元と所有を追って棚卸しする。

## 4. 項目別設計

### 4.1 文字描画、補正、fallback、絵文字

#### 本家の変更

0.34以降の文字描画交換と0.35以降のshapingを経た0.37は、skrifa、vello_cpu、harfrustを使う。

| 内容 | 本家ソース |
|---|---|
| units-per-em基準の倍率 | `epaint-0.37.0/src/text/font_face.rs:88` |
| outline／color glyph選択 | 同`:96` |
| color glyph描画 | 同`:190` |
| shaping data | 同`:332` |
| メトリクス・offset | 同`:596` |
| hinting／subpixel binning既定有効 | `epaint-0.37.0/src/text/mod.rs:77` |
| 行内配置 | `epaint-0.37.0/src/text/text_layout.rs:1100` |

FontTweakのscale／offsetは残り、hinting target、variation coordinates等が加わる。根拠: `epaint-0.37.0/src/text/font_tweak.rs:6`。

#### 影響箇所と方針

| 箇所 | 内容・方針 |
|---|---|
| `src/ui_fonts.rs:4`、`:7`、`:13` | user／HUD／toolbar familyを維持 |
| 同`:235`、`:420` | fallback順と主フォント登録を保持 |
| 同`:360`、`:373` | 主フォントの視覚中央補正を再検証 |
| 同`:546`、`:567` | fallback補正を新rendererと比較 |
| 同`:652`、`:687`、`:699` | ttf-parserのbbox／行計測。全面交換しない |
| `src/ui_font_catalog.rs:6`、`:78` | ab_glyph previewを本体表示へ近づける |
| `src/video/native_presenter/bottom_hud.rs` | 実galleyの計測・描画共有と失効を確認 |
| `build.rs:700`、`src/comic_stamp.rs` | Twemoji SVGと生成物を保持 |

`miv-user-text`／`miv-hud-text`、選択フォント、face index、利用者の縦位置調整値を保持する。toolbar固定3.5pt、主フォント補正、CJK／数学／emoji補正は個別に判断する。

color glyphはoutline bboxと可視範囲が異なり得る。補正を全部残す／全部捨てる方針にはしない。同じ文字列・font・point size・DPIについて、行高、baseline、galley矩形、描画範囲を比較する。

previewは既存worker準備を維持する。本体と異なるラスタライザによる見本のずれを解消する方式は小設計としてレビューする。`comic-core`の注釈焼き込み用`ab_glyph_rasterizer`は別経路なので変更しない。

#### default_fontsとsystem fallback

egui単体のdefault featureは空だが、eframe defaultには`default_fonts`が残る。現root指定で全default fontが直ちに消えるわけではない。

根拠: `egui-0.37.0/Cargo.toml:62`、`eframe-0.37.0/Cargo.toml:61`、`epaint-0.37.0/src/text/font_definitions.rs:110`。

eframeのsystem font fallbackは既定有効。featureと`NativeOptions::system_font_fallback`を明示し、既存chainを基準にする。環境依存探索を無条件に重ねない。

根拠: `eframe-0.37.0/src/epi.rs:384`、`src/native/epi_integration.rs:260`。

`monochrome_emoji_fonts`は別feature。Twemojiは`vendor/twemoji`由来の注釈用SVG資産であり、文字fallbackや色付き文字対応で代替しない。

**見た目:** 字面、太さ、字幅、合字、折り返し、行高、baseline、emoji色が変わり得る。代表比較を用意し、§5と段4bで採択する。

### 4.2 App／Context、非表示処理と入力所有

#### 本家の呼出契約

App::logicはroot UIの前と、rootのUI省略時に呼ばれる。常時tickではない。logic-onlyではegui passが進まず、Context::inputはviewport情報の一部を除き最後の表示passの値を持つ。

根拠: `eframe-0.37.0/src/epi.rs:152`、`egui-0.37.0/src/context.rs:1174`。

schedulerのHWND可視判定と、UI省略の`ViewportInfo::visible()`を区別する。後者はminimized／occludedの合成であり、IsWindowVisibleそのものではない。

根拠: `eframe-0.37.0/src/native/run.rs:216`、`:243`、`src/native/wgpu_integration.rs:682`、`:702`、`egui-0.37.0/src/data/input/viewport_info.rs:98`。

#### SW_HIDE／最小化／子窓／期限超過の条件表

以下はソースからの見立てと、第4.3版で採択したvendor方針であり、製品起動による確認結果ではない。案Bの変更はWindowsの最小化に適用し、他OSのocclusion等の契約を一括変更しない。

| 条件 | scheduler／pass | logic・UI | 未消費入力・描画 |
|---|---|---|---|
| 非最小化rootを直接SW_HIDE | 隠れた窓の要求済みrepaintをFullで直接実行 | 通常はroot logic＋UI | SW_HIDEだけを理由に保留が累積するとはいえない。描画資格は別判定 |
| Windowsのroot最小化、visible扱いの子なし、明示描画要求なし | 要求済みFullを直接実行。最小化を理由にUIを省略しない | root logic＋UI | rootの入力をrootで消費。最小化したsurfaceの通常描画は省く |
| Windowsのroot最小化、visible扱いの子あり | Fullで親UIも実行 | root logic＋UI | 各viewportが自身の入力を消費。親のsurface描画とvisibleな子の描画を区別 |
| Windowsの最小化deferred childへFull | 子callbackを実行 | 子UIあり。子のためにApp::logicを重複実行しない | 子自身の入力を消費。子surfaceの通常描画は省く |
| screenshot／RequestPaintWhileHiddenあり | Fullでwants_paintが成立 | UI実行可能 | 明示描画要求と既存capture ownerを維持。不可視を理由に要求を捨てない |
| rootのRedrawRequested期限超過 | 可視性・子の可視性と別にLogicOnly | root logicのみ | root ownerに保持。C1・保存・取消終端は進み、表示用pollは次の対象Fullまで保留 |
| childの期限超過等でUIをskip | child callbackなし。App::logicも呼ばれない | 子UIなし | 通常入力は子のStateからtakeせず保持。Windows最小化単独をskip条件にしない |
| 最小化childへCloseRequested | 案Bでは後続Fullで子UIも実行可能。ただしCloseの進行をその到着に依存させない | 既存終了ownerへの非描画受付を維持 | UI復帰待ちにせず配送・消費し、承認後は対象限定で退役 |
| repaint・wake等の要求なし | Wait等でsleep | 呼出なし | 常時UI／logicとは扱わない |

本家の境界は`eframe-0.37.0/src/native/run.rs:216`、`:243`、`:266`、`src/native/wgpu_integration.rs:682`、`:702`、`:717`、`:821`、`:1031`、`:1189`、`egui-winit-0.37.0/src/lib.rs:1507`。案BではFull時のUI実行条件だけにWindows最小化の継続方針を加え、明示的なLogicOnlyをFullへ変更しない。

旧vendorはhiddenと最小化を同じ判定に含め、要求済みrepaintを100 ms以上先へ調整し、OSのRedrawRequestedを待たずouter phaseでUIへ渡す。根拠: `vendor/eframe/src/native/winit_integration.rs:17`、`native/run.rs:139`、`:549`、`:575`、`native/wgpu_integration.rs:804`、`native/epi_integration.rs:279`。

この要求済みrepaintの下限・集約・sleep契約を移植する。常時10 Hzではなく、要求がある場合だけ実行する。bootstrap／resize等の既存入口まで一律100 msへ変えず、トレイmediaの50 ms WM_PAINT bridgeを通常最小化へ拡張しない。

winit Windowsのis_visibleはIsWindowVisible、is_minimizedはIsIconicを読む。WindowsではOccludedは未対応で、WM_SIZEも生成しない。

根拠: `winit-0.30.13/src/platform_impl/windows/window.rs:147`、`:668`、`util.rs:143`、`event_loop.rs:1397`、`winit-0.30.13/src/event.rs:418`。

mIVのトレイ退避は`src/tray_integration.rs:276`、`:323`の直接SW_HIDEである。

Closeの配送・拒否・承認・退役は、以下の「非表示子のCloseとlifecycle要求」に従う。通常入力を保持する契約だけでは、終了要求の進行を保証できない。

##### UI実行とsurface描画の分離

1. `PassMode::Full`、viewportのmetadata、visible descendant、明示描画要求から、UI実行とsurface描画を別々に決める。新しいApp側の保持フラグは作らない。
2. Windowsの最小化中もFullではUIを実行するが、`ViewportInfo::visible()`やbackendの`is_visible`をtrueへ偽装しない。実際のminimized／occluded情報をAppと入力へ渡す。
3. 不可視surfaceの通常tessellation・render・presentは省く。UI内のtexture生成やApp側GPU処理まで「GPU負荷ゼロ」とは扱わない。
4. FullOutputを生成した場合は、描画の有無にかかわらずtexture deltaを§4.9の共通ownerへ渡し、upload／submission／freeまたは既存ownerへの引継ぎを完結させる。無条件clearや、可視化までの無期限保留で代用しない。
5. platform outputとviewport commandsを描画分岐の外で対象ownerへ配送する。IME出力、終了、可視性変更等をsurface描画待ちにしない。
6. Cut／Copy／Pasteのaction処理は描画要否から分離する。現在の本家wgpuでは`:843`以降、glowでは`:889`以降の描画分岐内にある。対象Stateの次回入力へ一度だけ配送する既存規則を保ち、別viewportへ流さず、同じactionを後の描画で再実行しない。
7. Screenshot／PaintWhileHiddenは描画を必要とするactionとして区別し、対応する描画処理まで保持する。全actionを一括drainして、描画しなかった要求まで完了扱いにしない。
8. UI実行だけでは、present成功・初回visible commit・実表示ACKを発行しない。既存のUI境界ACKと実表示ACKも混同しない。
9. immediate childが別surfaceへ描画する場合も、最小化した親surfaceの描画と区別する。登録・GC・寿命処理を親のpresent成功へ結び付けない。
10. wg​​puとglowで同じ方針を使う。glowのUI判定は`eframe-0.37.0/src/native/glow_integration.rs:692`、描画分岐は`:860`。renderer固有の資源適用方法は各既存ownerへ残す。

本家にも、最小化した親のUIをvisible descendantのために実行し、親surfaceは描画しない経路がある。根拠: `eframe-0.37.0/src/native/wgpu_integration.rs:702`、`:821`、`:1189`。案Bはこの責務分離を利用し、UI実行のためにsurface可視性を偽装しない。

RedrawRequested期限超過は本家の250 ms判定を残す。根拠: `eframe-0.37.0/src/native/run.rs:29`、`:233`、`:266`。新しい時間窓は追加せず、最小化前の未到着deadline等によるLogicOnlyも排除されたとは扱わない。

###### immediate子自身の描画資格

案Bの対象は主経路の`show_ui`と描画分岐だけではない。両backendの`render_immediate_viewport`も、同じ出力・action完結契約へ揃える。

| 対象 | 本家の現状 |
|---|---|
| wg​​pu immediate | `eframe-0.37.0/src/native/wgpu_integration.rs:1203`。子metadata更新は`:1249`、UIは`:1272`、tessellation／paintは`:1308`、`:1309` |
| glow immediate | `eframe-0.37.0/src/native/glow_integration.rs:1708`。子metadata更新は`:1759`、UIは`:1781`、tessellationは`:1787`、clear／paint／swapは`:1835`以降 |
| mIV | `src/ui_fullscreen.rs:22625`等にimmediate呼出しがある。現vendorもwgpu `:1340`、`:1365`で描画へ進む |

本家immediate経路の描画は主経路と別であり、主経路だけの修正では最小化immediate子の通常描画が残る。

- 子UIが親Fullから呼ばれた場合は、案Bに従いUIを進める。親がLogicOnlyなら、子UIを別途強制呼出ししない。
- 子の描画資格は、子自身の最新ViewportInfoと子宛てScreenshot／PaintWhileHiddenで決める。親の`is_visible`、親surfaceの描画結果、親の描画要求を流用しない。
- 子UIの出力に含まれる子宛て明示描画要求も、既存action ownerへ一度だけ取り込む。既存保留actionと合わせ、判定・配送・完了のconsumerを一つにする。描画後のcommand処理で同じrequestを再登録しない。
- 子が非描画でもdelta、platform output、viewport commands、Cut／Copy／Pasteを完結させる。deltaはviewport消滅・surfaceなしも共有renderer ownerへ渡し、不可視子のpendingへ放置しない。
- Screenshotは子のsurface・子のcallbackへ配送する。本家wgpu immediateの空capture引数をそのまま残さず、既存のimmediate screenshot移行と共通のaction処理へ接続する。
- glowではtexture適用に必要なGL contextの選択と、surfaceへのclear／paint／swapを区別する。非描画を理由に必要な資源適用を省かず、資源適用のためだけにsurfaceを描画しない。
- 借用・lockを保持してUI callbackへ再入しない。親の登録・子の寿命・surface generation・通常版immediate計測を維持し、表示ACKを偽造しない。

必須回帰は実`render_immediate_viewport`の判断・配送入口に接続し、wgpu／glowの双方で行う。

| 条件 | 期待結果 |
|---|---|
| 親最小化・子visible | 親と子のUIは進む。親surfaceは非描画、子surfaceは描画 |
| 親子とも最小化 | UIは進むが通常tessellation／render／presentまたはclear／paint／swapは行わない。出力は完結 |
| 最小化immediate子へのScreenshot | 子の明示描画とcallbackを一度だけ実行。親は要求がなければ非描画 |
| 親だけへScreenshot | 子の描画資格へ流用せず、子callbackへ誤配送しない |

既存actionとして保持済みの場合と、子UIからrequestが出る場合を検査する。要求配送に必要な既存Fullの列を使い、追加repaint・専用retryでcallbackを成立させない。surface acquire等のまれな失敗は既存のログ・通知・終端へ揃える。

#### root LogicOnlyとvisible childが共存する経路

**単純な「root最小化＋既にvisible扱いのchild＋Full要求」では、親UIも実行される。** この条件だけを保留入力誤配送の根拠にはしない。

一方、期限超過経路は次の構造になっている。

1. 可視rootへのrequest_redrawと同時に、root固有のdeadlineを登録する。
2. rootのRedrawRequestedが未到着でdeadlineを超えると、rootをLogicOnlyで実行する。
3. この選択ではvisible childの有無を検査しない。
4. wgpu側のshow_uiは`mode == Full`を必要とするため、visible childがいてもLogicOnlyをFullへ変更しない。
5. deferred childは独立したwindow eventから更新でき、同じEpiIntegration::updateを使う。

根拠: `eframe-0.37.0/src/native/run.rs:233`、`:266`、`:281`、`:421`、`src/native/wgpu_integration.rs:647`、`:702`、`:773`。

したがって、**rootの期限超過LogicOnly → deferred childのFull → rootのFullという呼出経路はソース上到達可能**であり、相互排他条件はない。Windowsでの発生頻度や、このPCでの再現は未確認である。

mIVは実際にdeferred childを使う。根拠: `src/ui_fullscreen.rs:22139`。同`:22244`付近の子が文字入力を持たなくても、rootの入力がそこで消費されることは許容できない。

また、childのskip時入力喪失は、rootとの共存条件に依存しない。このため、両方を同じviewport所有境界で直す。

#### 影響箇所

| 箇所 | 内容 |
|---|---|
| `src/app.rs:94680`、`:97631` | update・App trait |
| 同`:94969`、`:97661`以降、`:97700`以降 | tray／media、非同期完了、post-render終端 |
| `src/tray_integration.rs:276`、`:323` | hide／restore |
| `src/key_input.rs:2023`、`:2099`、`:3381`、`:3418`、`:3960`以降 | queue・drain・Win32終端 |
| `src/keyboard_input.rs:106`、`:159` | discrete／current level permit |
| `src/app.rs:55249`、`src/ui_fullscreen.rs:32137` | permit消費 |
| `src/gamepad.rs:170` | gilrs producer |
| `src/app.rs:95772` | gamepad sample |
| `src/app/gamepad_input.rs:2110`以降 | sample・配送・抑止・終端 |
| `src/lib.rs:786`、`:801`、`:836` | WH_GETMESSAGE pending |
| `src/ui_fullscreen.rs:22139`、`:22364`、`:24745` | deferred／immediate viewport |
| `src/video/native_presenter/render_core.rs:12505` | 独立Context |
| `vendor/eframe/src/native/epi_integration.rs` | 新版の保留処理を移植時に修正 |
| `vendor/eframe/src/native/wgpu_integration.rs` | viewport入力取得・skip・復帰 |
| `vendor/eframe/src/native/glow_integration.rs` | 共有integrationの同等経路 |
| 各ラボ・`vst3-host-tester` | App実装 |

#### Appの責務分離

| 処理 | 移行先 |
|---|---|
| UI入力・表示継続へ到達しないworker結果、取消・保存終端、Remote完了、トレイ要求 | UI入力を読まないlogic service。C1〜C5は下記の個別境界に従い、一括移動しない |
| 子のOS／programmatic Close要求 | 非描画のlifecycle受付から、対象windowの既存modal判断・終了ownerへ。一度処理した要求を子UIへ再配送しない |
| gamepad物理drain・保持更新 | logicから進めるApp-global単一owner |
| gamepad操作batch・multi-pass配送 | 既存batch ownerとmounted context |
| key／wheel／pointer／focus／IMEのpass処理 | ui／対象viewport |
| panel／dialog／viewport登録・paint | ui。UI実行とbackendのsurface描画資格は区別 |
| 承認済みviewportの退役 | 既存寿命ownerの終了確定からbackend／Contextの対象限定cleanupへ。描画passを前提にしない |
| 表示済み前提のACK等 | 実際の描画側 |
| contextごとの結果採用 | 既存registry／mount owner。表示継続へ到達する採用はuiに残す |
| native動画frame | 既存presenter thread |

旧updateを丸ごとlogic／uiへ改名しない。logicは古いevents・キーlevel・時刻を操作として読まず、UI pass counterをservice進行条件にしない。

post-render終端を前へ移す際は、そのframeのmodal gateを早く外さない。

Close専用の非描画受付は、通常入力の消費や子UIの代行ではない。既存の終了判断・取消・cleanupを呼ぶ入口を揃えるものであり、App側に別のclose待ち状態を追加しない。

##### C1〜C5の共通境界

段1b検証台帳の`service_*`という関数名はlogic接続の証明ではない。表示用pollは、既存要求ownerとreceiverを保持し、受信から表示継続までをuiの旧位置へ残す。

案Bでは通常のWindows最小化中も対象Fullで進む。RedrawRequested期限超過のLogicOnly中だけは表示用pollを呼ばず、次の対象Fullまで結果を既存receiverへ保持する。worker自体の計算・I/Oは既存threadで進む。別の子のFullを、rootや兄弟contextの表示継続を消費する資格にしない。

Fullではlogic→ui、LogicOnlyではlogicだけが呼ばれる。根拠: `eframe-0.37.0/src/native/epi_integration.rs:337`、`:384`。後者のContext入力はUI passの入力更新を伴わない。根拠: `egui-0.37.0/src/context.rs:1174`。

##### root Closeの非描画lifecycle境界

前掲の「key／wheel／pointer／focus／IMEのpass処理」はroot Closeを含めない。root CloseはOS／viewportのlifecycle要求であり、通常キー入力やdialog描画を通過しなくても処理する。

本家`update_logic_only`はlogic実行前にrootのCloseを読み、logic直後にCancelCloseの有無から終了を確定する。したがって、uiでCancelCloseを出す設計では遅い。

根拠: `eframe-0.37.0/src/native/epi_integration.rs:391`、`:401`、`:428`。`Context::run_logic`はviewport情報を更新するため、ここで参照するroot Closeは古いキーeventとは異なる。根拠: `egui-0.37.0/src/context.rs:1174`。

**入口と判断owner**

1. App::logicの非描画lifecycle入口で、最新のroot ViewportInfoのCloseと、既存tray Quit／installer shutdownの要求元を受け取る。rootを明示し、現在mountしているviewerやforegroundから終了対象を推定しない。
2. 既存の`defer_settings_family_root_close`、`defer_composition_editor_root_close`、`defer_image_folder_zip_root_close`、`maybe_intercept_close`の判断をこの入口へ揃える。ui側に同じCloseを再判断するconsumerを残さない。
3. 「通常の×」「明示的なアプリ終了」「operation終端から戻る終了継続」を区別する。明示Quitをトレイ退避へ変えず、延期された通常Closeも勝手に強制Quitへ格上げしない。
4. 受付時のmodal・operation状態は、完了pollで変える前にその呼出し内で取得する。従来のterminal frameのmodal保護を維持し、時間窓や保持boolを追加しない。
5. 既存の判断・取消副作用は従来の順序と優先規則で実行する。終了継続の保持には既存operationの`EditorExitRequest`、`DeferredRootClose`、ZIPの`Exit`等を使い、同じ要求の第二ownerをAppへ追加しない。
6. 延期ownerの終端では、保持した要求を一度だけ取り出して共通の終了判断へ戻す。他の既存barrierを飛び越えて直接終了・hideしない。UI復帰を再開条件にしない。

| 既存判断 | 非描画で維持する契約 |
|---|---|
| トレイ置換 | 設定ON・tray稼働中の通常×はCancelCloseを返し、既存hide ownerへ渡す。明示Quit／installer終了はこの置換を通さない |
| composition保存待ち | editorのdraftは既存終了理由で閉じ、送信済みticketは維持する。receipt終端までCancelCloseでAppを生かし、完了後に一度だけ終了継続を返す |
| settings-family | 実行中worker／既存retry taskの終了barrierを維持する。通常tray-hideとprocess exitの既存区別を保ち、終端収穫をdialog描画から独立させる |
| 画像folder→ZIP | 既存ownerへ取消とRoot／TrayHideの終了意図を渡す。worker・予約・reader等の終端までCancelCloseし、終了時の通常表示採用を新たに実行しない |
| sidecar等のmodal | 現行の通常Close拒否・tray-hide抑止と、明示lifecycle終了の区別を維持する。入力sanitizeに含まれているroot Close判断も、この入口へ分ける |

代表箇所: `src/tray_integration.rs:223`、`src/app/book_composition_editor.rs:495`、`src/ui_dialogs/settings_restore.rs:328`、`:530`、`src/app/image_folder_zip_operation.rs:261`、`:636`、`src/app/sidecar_restore.rs:984`、`:1021`、`:1037`。

root用の入力sanitizeから分けるのはClose判断であり、release・pointer・IME等の既存入力処理をlogicへ移す変更ではない。子Closeの既存非描画ownerも維持する。

**CancelCloseと終了再開**

- 退避・拒否・延期の判断は、同じlogic呼出しのLogicOutputへroot宛てCancelCloseとして反映する。後のuiに委ねない。
- worker結果の収穫と終了判断は非blockingで行う。既存workerの終端待ちをUI threadのjoinや新しい待機loopへ変えない。
- 完了後のClose再要求は、既存終了継続を消費した時に一度だけ発行する。backendがその要求を次回のCloseとして扱う場合も、同じ共通判断を通す。
- 同じ受信Closeを保留snapshotや後続Fullから再受付しない。既存viewport情報の受付・消費境界と延期ownerを使い、frame counterによる重複抑止や「復帰時に一度捨てる」を追加しない。
- tray／hide、保存、取消の処理は後述の非描画context境界を使い、内部のmountから表示再採用へ到達しない。
- 送信済み保存の失敗も既存のreceipt終端として扱う。ログ・通知と既存終了方針を保ち、新しいretry・救済状態を加えない。

通常Fullでも同じlifecycle入口を使う。期限超過時だけの特別な終了判断を作らず、本家のroot Close確定とCancelCloseのhandshakeを維持する。

**必須のUIなし回帰**

App::ui、viewport callback、paintを一度も呼ばず、最新root Closeを含むRawInputで実`update_logic_only`または同じClose確定経路へ接続する。

- 通常×・tray設定ON: CancelCloseがlogic出力へ入り、既存hide効果をfake ownerで一度だけ観測する。backendの`should_close`はfalse。
- tray設定OFF／明示Quit: 不要なCancelCloseやhideを出さず、既存の終了判断を通る。
- composition保存中: Closeを受けても終了せず、ticketを失わない。後続logicでreceipt・resume書込みを収穫し、終了継続を一度だけ発行する。
- settings-family実行中、ZIP取消中: UIなしでbarrier・取消終端を進め、終端前は終了しない。終端後は元のClose／Quit／TrayHideの意味へ戻る。
- modalによる拒否と、保存待ちによる延期を区別する。拒否した通常×をmodal終了後に自動再発火させない。
- 同じframe番号の複数logicと、その後のFullでhide・取消・終了継続を重複しない。保存中に後着した明示Quitの既存優先規則も維持する。
- uiを呼んだ時だけCancelCloseが補われるfixtureは失敗とする。backendがlogic直後に終了判定することまで検査する。

##### C1: Remote

代表: `src/app/update_phases.rs:27`、`src/remote_ipc/ui.rs:1068`。

**logicに置くもの**

- session snapshot、取得・返却sequenceの採用、旧ownerのstream取消、AI barrierの取得・解放。
- AcquiringRemoteでの既存全context終了、editor取消、遅延openの失効。既存の終了ownerを呼び、UIを強制実行して閉じない。
- 取得barrierへ必要な送信済みsave ticket、resume／shift完了・snapshot公開の確認、FinishAcquireと既存abort。
- Remote要求の受信・admission・書込み投入、headless player／streamの進行。
- read／write／bookmark／shift応答、取消、ACK、DrainingRemoteとApp終了時のdrain。

代表: `src/remote_ipc/ui.rs:1160`以降、`:2012`、`:2073`、`:2103`、`:3671`、`src/app.rs:19057`、`src/app/book_composition_editor.rs:513`。

「UI request」という既存型名を、PC描画が必要である根拠にしない。共有serviceの応答receiverは既存routerを単一consumerとしてlogicでdrainする。RemoteのACKに必要なreceiptをuiへ残さず、local一覧の表示採用とは区別する。

**uiに残すものと受渡しowner**

Local返却後のPC一覧再読込・既定起動先表示と、その先の本準備・fullscreen openはuiへ残す。通常最小化中はFullの旧`render_update_begin`相当位置で進め、期限超過中だけ次の対象Fullまで保持する。

受渡しは既存`remote_session_ui`のPC表示継続slotへ集約する。現在の`pending_fullscreen_restore`を、取得・返却identityと再読込対象を持つLocal返却後の一覧表示継続へ置き換える。第二queueや「返却待ち」boolを追加しない。返却sequenceの採用と同じlogic境界で一度だけ登録し、uiで既存再読込入口へ渡す。再取得時は既存の失効経路で取り消し、古いLocal表示を後から実行しない。

この表示継続をRemoteのACK・取得・drain完了条件にしない。返却と再取得がUIなしで完結した場合は、既存coalesced reacquireの契約を維持する。

`docs/web-remote-plan.md` §2.1の単一writer、§2.2の取得時の全ローカル閲覧終了と、返却後の一覧再読込・閉じた閲覧窓の非復元を維持する。RemoteActive／DrainingRemote中にPC閲覧窓を開く仕様は追加しない。

`src/remote_ipc/ui.rs:4534`の返却処理は旧復元要求を消している。`:4587`以降に復元pollは残るが、今回照合した本番コードには新規`Some(PendingFullscreenRestore { ... })`生成元がなく、直接生成は`:6538`のテストfixtureである。slot置換では旧復元専用の`wait_frames`を継承せず、終了drainのfixtureを新しい一覧表示継続の所有・破棄へ合わせる。

###### 第4.5版: ローカル窓の退役完了とFinishAcquire

`service_remote_session`をlogicへ正式接続する条件として、全ローカル終了から空のimmediate UIを呼ぶ経路を除去し、§4.2「承認後の対象限定退役」の出力契約を成立させる。

Appのruntimeやbindingが空になっただけで、同じlogic呼出し中にFinishAcquireしてはならない。承認済み退役をまだ出力へ保持している間は、既存のlocal close readinessを`InProgress`として扱う。

この判断には、既存root command列にある未配送の承認済み退役という事実を使う。Remote側のpending bool、退役待ちqueue、退役済みIDラッチは追加しない。discard再passの間も承認済みcommandを同じ列へ保持するため、同じ外側runの次のlogicで誤って`Closed`と判定しない。

backendは、最外側のFull／LogicOnlyが戻った後、次のApp処理に入る前に承認済み退役を完結する。この順序が成立して初めて、次の既存service機会で、従来のAI・保存・resume／shift・editorのreadinessと併せてFinishAcquireできる。既存のAcquiringRemoteのwakeを利用し、UI復帰待ち、追加repaint、退役のretryに置き換えない。

これにより、RemoteActive／DrainingRemoteにローカル閲覧窓を残さない`docs/web-remote-plan.md` §2.2の契約を維持する。command発行をnative閉鎖完了と同一視せず、backendの退役にClose hookからの再承認やApp runtimeの延命を必要としない。

検査対象は`src/remote_ipc/ui.rs:1214`の全窓終了、`:1233`のclose readiness、`:1259`のFinishAcquireと、`src/app.rs:77836`の既存close-state判定である。

第4.6版では、Remote取得時のlocal表示失効に、型付きnavigation帰還要求も含める。失効は全ローカルviewer終了より前に行い、Remote起点のCloseには「帰還表示を作らない」既存所有権変更causeを渡す。

型付き帰還要求がUI待ちであることをRemote取得・ACK・drainの完了条件にしない。資源退役、保存・resume／shift・editor・AI readinessは従来どおりlogicで進める。Remote取得中の遅いUIや返却後のUIから、旧bookmark／親一覧帰還を再実行しない。

返却後の既存`pending_local_display`による一覧再読込と、Closeの帰還要求は別の操作由来である。旧Close要求は取得時に終端させ、返却slotへコピーしない。表示要求を二重保持するqueueは作らない。

##### C2: play-testと起動worker

代表: `src/app/update_phases.rs:335`、`src/app.rs:31164`。

`poll_play_test`はフォルダ準備・対象採用・open・起動済み状態の確定を含め、全体をuiへ残す。受渡しownerは既存`PlayTestState`とし、表示開始用の第二要求を作らない。通常最小化中もFullで進み、期限超過中は次のFullまでApp側の診断進行を保留する。経過時間の停止・補正や新タイマーは追加しない。

VST／EffeTune／更新確認は、非描画完了・資源終端と実GUI操作を区別する。`render_startup_workers`全体を関数名だけでlogicへ移さない。通常の動画／音声制御やnative presenterのframe進行をplay-testのUI待ちへ巻き込まない。

##### C3: ローカルopen・一覧採用・本準備・composition

代表: `src/app/update_phases.rs`の`render_update_before_frame`、`render_initial_open`、`render_search_and_metadata_results`、`render_update_after_frame`、`src/app/startup_ops.rs:473`、`src/app.rs:24101`付近、`src/app/sidecar_restore.rs:1268`、`src/app/book_composition_runtime.rs:731`。

startup／activation open、mounted document／collection navigation、bookmark open、tag／ratingからのrebuild、composition projection／permission、本準備・表示再開は、既存receiver・typed state・registry／mount ownerのままuiへ残す。liveなsidecar hydrationも、最小化対応のために全phaseを分割しない。結果受信から同passの表示継続までを旧入口で進める。

F13の`service_initial_worker_results`も例外ではない。段1b台帳が追記したcache Cleared→materialization解除→context mount→rating採用→fullscreen再openは表示継続であり、関数名を理由に全体をlogicへ移さない。

通常最小化中はFullで進む。期限超過中は結果を既存要求ownerへ残し、live要求のmodal・continuation・deferred fullscreenを保持する。gateだけ先に解除して別openを受け付けない。

次はui保留から外し、logicで進める。

1. 送信済みcomposition save ticket等の保存receipt、失敗終端、共有resume書込み。既存App-global ticket ownerで一度だけ収穫する。
2. Remote取得・Closeで破棄された表示要求の取消・drain・予約解放。既存取消ownerとdiscarded sidecar等の終端へ合流し、表示continuationを復活させない。
3. C1のACKへ必要な共有service応答。

`poll_composition_editor_operations`は送信済みticket収穫と、editor準備・Reloading継続を分ける。保存receiptは既存Reloading等へ確定し、表示reloadとmodal解除は旧ui終端で行う。Remote取得では既存editor closeが表示ownerを終了し、送信済みticketは共通ownerへ残す。

`poll_book_composition_service`のchanged scopeをlogicで先にdrainして失わない。receipt→publicationの既存順序を維持し、projection／permissionの表示採用はuiへ残す。共有cacheのACK・取消終端から表示再採用へ到達する部分も、同じ原則で切り分ける。

###### owner付き状態更新と非描画mount

C1・C3の「既存owner／mountを使う」は、logicから現行`with_viewer_context`をそのまま呼ぶ許可ではない。

現行入口はcallbackの前後でrating publicationとpage-edit reconciliationを実行する。rating publicationはmembership採用からfullscreen再openへ到達し得る。

根拠: `src/app/viewer_context_registry.rs:5285`、`:5296`、`:5300`、`:5305`、`:5313`、`src/app.rs:73183`付近、`src/app/rating_membership_ops.rs:462`、`:471`。保存receipt・editor終了の現在の呼出しは`src/app/book_composition_editor.rs:486`、`:535`、`:558`。

**先にmountを不要にする**

- 保存receiptのresumeは、receiptが保持するpath・page identity・meterを既存App-global writerへ渡す。現在の`persist_book_resume_with_identity`が扱うwriter、meter、重複記録のmirrorを共通利用し、receiptのためだけにviewerをmountしない。
- この処理は現在投影中のitemsや選択から保存先を引き直さない。contextが終了していても、既に送信した保存のauthoritative receiptを失わない。
- editorのClosed／Reloading／既存error状態への更新は、`CompositionEditorOwner`等のowner identityを確認した狭い状態更新入口へ寄せる。MountedならApp内の当該状態、AtRestなら既存bundle内の状態を更新する。新しいeditor ownerやコピーを作らない。
- 古いreceiptは新しいeditor sessionを変更しない。表示ownerが既に終了していても、保存・resumeの終端は共通ownerで収穫する。

既存のread-only `ContextRef`やAtRest宛てreply配送と同じく、必要なpayloadだけへ届く入口を優先する。代表: `src/app/viewer_context_registry.rs:5204`、`deliver_spread_shift_reply_at_rest`。receiptのresume記録は`src/app/book_resume_meter.rs:184`。

**App上の一時投影が必要なserviceの境界**

残る取消・寿命処理でApp形式のpayloadアクセスが必要な場合は、現行mountを次の二層へ分ける。

| 境界 | 責務 |
|---|---|
| 非描画の状態mount | 既存tableのplan／payload交換／finish、必要なfavorite設定投影、owner付きcallback、元ownerへの復元 |
| UI mount wrapper | 従来のmount／return前後のrating publicationとpage-edit reconciliationを、従来順序で明示して呼ぶ |

非描画入口は仮称`with_viewer_context_state`とする。既存`with_viewer_context`はUI wrapperとして残し、通常Fullの旧呼出位置・rating boundary順序・reconciliationを維持する。

共通化するのは既存registryとpayload交換であり、第二registry、第二bundle、shadow ownerを作らない。元contextへの復元・既存unwind時の復元契約も共有する。`swap_viewer_context_bundle`内のfavorite overlay投影は所有contextに必要な状態交換として維持し、そこへnavigationや表示refreshを追加しない。

非描画入口では、往路・callback・復路のどこでも次を行わない。

- `publish_mounted_context_rating_writes_at_boundary`
- `poll_page_edit_reconciliation`による表示用snapshot採用・再要求
- membership再配置、fullscreen open／再open、UI decision、入力sample、表示ACK

`in_logic`、`suppress_display`等のflagを従来mountへ足して副作用を抑える方式は採らない。状態交換そのものに表示hookを含めず、UI wrapperだけが表示境界を呼ぶ構造にする。非描画callback自身にも表示継続を含めない。

未反映ratingの既存write generation／seen generation、page-editのstamp／pendingはそのownerへ残す。非描画アクセスのためにseenを進めたり、表示更新済みへ偽装したりしない。次の当該UI境界で、現在と同じ表示再採用を行う。新しい「reconcile待ち」boolやqueueは不要である。

C1、save receipt、editor終了、root／子Close、ZIP取消、discarded sidecar等の非描画serviceについて、callback本体だけでなく呼出し前後を監査する。`with_window_viewer_context`等のwrapperも同じ対象とする。build／fork／promote等の表示開始境界を、状態更新の代用品としてlogicから呼ばない。

**別contextを使う必須回帰**

Aを現在投影し、Bに送信済みcomposition save ticketを置く。A・Bには、UIで採用するとmembership・表示位置が変わる未反映rating writeを用意する。

1. UIなしでserviceを実行し、Bのreceipt収穫、正しいpath／identityへのresume記録、Bのeditor状態確定を検査する。
2. A・Bのfullscreen open／再open、page-turn decision、表示用reconciliationの実行が0回であることを検査する。兄弟の選択、表示位置、items generation、window bindingを変更しない。
3. 元の投影owner・favorite設定投影が復元され、ratingの未反映generationとpage-editの未採用状態が保持されることを検査する。
4. 後にUI mountを行うと、従来のboundary順で正しいcontextだけが未反映rating・編集状態を採用する。service側で更新済みと偽装していないことを確認する。
5. A／Bを逆にした場合、同じcontextへのアクセス、入れ子の状態アクセス、B終了後の遅いreceiptも検査する。新editor sessionへ旧receiptを適用しない。
6. 必要な状態mountのcallbackがunwindする既存fixtureでは元ownerへ戻り、復路でも表示hookを実行しない。

テスト専用の「表示禁止flag」で成立させず、製品が使う状態アクセス／mount入口とUI wrapperを検査する。正常なowner付き保存のために兄弟表示を動かすことも、次の正当なUI採用を永久に抑止することも許容しない。

##### C4: 原画完成とeditor再開

代表: `src/ui_erase.rs:3554`、`src/app.rs:75749`、`src/ui_view_trim.rs:1118`。

`poll_erase_inpaint`は既存進捗受信、最終結果受信、世代・path・mask検証、texture／cache採用、`resume_original_material_entry`からeditor再開までをuiへ残す。既存`erase_inpaint_pending`とreceiverをownerとし、完成pixelsの第二slotを作らない。進捗だけのlogic抽出は必須としない。

通常最小化中はUI入力が更新されたFullで採用・再開する。期限超過中はworkerだけが進み、結果・進捗は既存channelへ残る。次の対象Fullで要求資格を検証し、そのpassのpointer状態を使う。logicから古い`pointer.primary_down`を読まない。

明示取消、Remote取得、対象context終了は既存ownerのlogic終端から進める。これを完成結果の表示採用待ちにしない。既存の1-frame採用上限をlogic呼出し回数の予算へ置き換えない。

##### C5: 表示準備とページめくり決定

対象は段1b台帳C5のprefetch／final-effects、panorama source解決・最終合成、open継続から、`fs_page_turn_decision_for_frame`、`original_preview_frame_observation`、`fs_page_turn_physical_input_for_frame`へ到達する経路とする。

表示用結果の受信・cache採用・後続合成投入・decision・preview観測はuiへ残す。受渡しownerは既存の要求receiverとcontext所有cacheであり、logic用decision cacheや共通完成queueは追加しない。

通常最小化中はFullで進み、期限超過中は次の対象Fullまで保留する。復帰先は「窓の復元」ではなく「当該ownerを処理する次のFull」である。対象contextが閉じた場合は既存取消へ渡し、兄弟へ採用しない。

Runtimeの取消・lease解放・Remote取得barrierへ必要な静止終端はlogicで進める。native presenter、既存動画／音声制御、Remote headless playerを表示用poll待ちへ変更しない。

台帳G1のPDF昇格再試行、G2のmaterialize／upload予算、G3の要約計算予算は、この表示用経路と同じui側へ残す。単に`frame_counter`が止まることを理由に新しいservice counterを作らない。G4の外部ツールのUI分割準備・Cancel／launch ACK、G5のactivation入力資格も、表示・入力側の既存条件を維持する。保存・取消・寿命終端へこれらのgateを流用しない。

##### D3: 外部再走査とhydration要求ID

代表: `src/app/update_phases.rs:287`、`src/app.rs:25646`、`:26068`、`:26246`以降、`src/app/sidecar_restore.rs:1350`、`:1503`、`:1515`。

watcher通知collect・既存走査投入・取消・周期保存はlogicへ分け、`poll_external_rescan`の結果受信からEqual／Different採用、一覧install・hydration、cutoff ACKまではuiへ残す。Equalだけをlogicへ先行させる分割は作らない。

通常最小化中はFullで進み、期限超過中は既存`external_rescan_pending`のreceiverへ結果を保持する。採用前に適用済みmtime／signature・cutoff ACKを進めず、dirty・cause・既存restart_requestedを保持する。新しいdebounceやrepaintは加えない。`Some(scan)`からPDFの`pre_scan.is_none()`枝へ到達するという推論は採らない。

`frame_counter + input_seq + 1`は要求IDとして廃止する。同じcounter値からの取消・再開始やIndexOnly置換に対して一意性を保証しない。案Bでも期限超過中の複数logicは残り、UI側へpollを残すこととIDを正しくすることは別である。

採番は既存のApp-globalなhydration要求ownerへ集約する。通常とIndexOnlyが同じ単一sequenceを使い、一要求につき一度だけ払い出し、`Common.request_id`と対応する`begin_book_shift_restore`へ同じ値を渡す。context mountや要求取消で巻き戻さず、番号を再利用しない。

現ownerには独立採番counterがないため、このownerに要求sequenceを持たせる。入力計装を進めて代用せず、無関係なbookmark等の採番ownerを借用しない。これは要求identityの発行源であり、進行状態を表すpending flagや第二ownerではない。番号枯渇をwrapによる再利用で処理せず、永続データ形式は変更しない。

#### 非表示子のCloseとlifecycle要求

本家0.37のwgpu経路では、最小化等でshow_ui=falseの子についてcallbackを呼ばずに戻る。一方、CloseRequestedはviewport.info.eventsへCloseを積み、子と親にrepaintを要求するだけである。Closeはwants_paintではなく、UI省略を解除しない。

根拠: `eframe-0.37.0/src/native/wgpu_integration.rs:682`、`:702`、`:716`、`:720`、`:1031`。

WindowsのWM_CLOSEもCloseRequestedへ変換して戻り、その場ではwindowを破棄しない。根拠: `winit-0.30.13/src/platform_impl/windows/event_loop.rs:1251`。

mIVのpassive deferred子は、子callback内でCloseを読み、Frameイベントとして親へ送り、親がclose_idsへ採用する。

- `src/ui_fullscreen.rs:22184`: 子callbackでCloseを読む。
- 同`:22246`: 親向けFrameイベントを生成。
- 同`:21531`: close_idsへ採用。
- 同`:21756`: modal判断とbatch適用。
- 同`:21876`: 既存のwindow終了処理。
- 同`:22035`付近: deferredイベントのdrainが描画関数内にある。

したがって、最小化した別窓をタスクバーから閉じる通常操作で、復元までCloseが処理されない経路が残ると見立てる。実機再現は今回行っていない。まれな失敗の割切り対象にはしない。

glowにも同じUI省略とClose蓄積がある。根拠: `eframe-0.37.0/src/native/glow_integration.rs:689`、`:692`、`:708`、`:1061`、`:1075`。修正はwgpuだけに限定しない。

**採る案は、Close専用の非描画受付から既存終了ownerへ渡す方式とする。**

親logicで更新済みRawInput.viewportsを読む案も検討した。run_logicが最新のwindow情報を渡すこと自体は確認できるが、snapshotを読むだけでは、元のviewport.info.eventsの消費、子Fullとの競合、拒否後の再配送を一つの所有境界で解決できない。また、親がlogic-onlyのままでは通常の描画後GCにも到達しない。このため、snapshot監視を新たなclose検出器として重ねる案は採らない。

根拠: `egui-0.37.0/src/context.rs:1174`以降、`eframe-0.37.0/src/native/wgpu_integration.rs:808`、`:1330`、`src/native/glow_integration.rs:853`、`:1533`。

##### 受付・判断・消費

1. vendor/eframeに、子Close専用の非描画App hookを追加する。仮称は`App::on_viewport_close_requested`とし、対象ViewportIdを明示して渡す。子の描画callbackではない。
2. hookはbackendのViewports／glutinのborrow・lockを解放した状態で呼ぶ。描画passの中から再入して呼ばず、window eventまたはviewport commandの処理を終えた非描画境界で配送する。
3. OSのCloseRequestedと、既存ViewportCommand::Closeから生じる要求を同じ受付へ揃える。rootの終了・tray契約は従来どおりとし、この子専用変更で置き換えない。
4. mIVは既存のViewportId→window／context所有から対象を決める。rect、現在foreground、現在表示中のwindowから推定しない。
5. passive deferred子は既存のmodal判断と`apply_detached_image_window_event_batch`／`close_detached_image_windows_by_ids`へ合流させる。必要な部分を描画に依存しない終了serviceとして抽出し、同じ判断を複製しない。
6. active／parkedのegui子についても、既存のwindow／context所有と終了・取消経路へ委譲する。active fullscreen、列挙待ち、parked mediaの終了処理をpassive画像のcleanupで代用しない。
7. hookの返値は、その一回の要求の処理結果とする。mIVが管理する子では「既存ownerが終了を確定した」「既存modal等により今回は拒否した」を区別する。新しいpending boolや復帰待ち状態は作らない。
8. mIVが所有しない子には明示的な従来UI経路への委譲を用意し、既定Appの互換性を保つ。これはmIVの管理対象を黙って見逃すfallbackには使わない。
9. mIVが処理したCloseは、承認・拒否のどちらでもその一回で消費する。元のinfo.events、保留RawInputのviewport情報、子callbackへ同じ要求を残して再発火させない。通常のkey／pointer／focus／IME列は変更しない。
10. 拒否後に新しく届いたCloseは別の要求として通常どおり判断する。拒否済みを再試行したり、以後のCloseを抑止するラッチを置いたりしない。

既存のViewportCommand::CloseはCloseイベントを追加する処理であり、承認済みの即時破棄命令ではない。CancelCloseもegui-winit内では別途処理が必要とされている。これらを送るだけで配送・消費・退役が済んだとは扱わない。

根拠: `egui-winit-0.37.0/src/lib.rs:1955`、`:1958`。

passive FrameのOS Close読取りは新受付へ集約し、同じ要求をFrame経由でも処理する二重ownerを残さない。独自barの×、watcher由来Closeも、既存の要求源を保持したまま同じApp側のmodal判断・終了serviceへ揃える。watcherは画面上のclick検出であり、タスクバーから届く全WM_CLOSEの代替ではない。

根拠: `src/app.rs:3247`、`:3524`、`src/app/detached_window_manager.rs:169`、`:265`。

第4.5版では、上記の「終了を確定した」と「backend資源を退役する」の受渡しを次のように具体化する。

- OS Closeのhookを呼ぶ前に、backendは要求対象の登録寿命を捕捉する。hook後に同じViewportIdだけを再検索して、別の寿命へ承認を適用しない。
- mIVの共通終了ownerは、modal等による拒否なら退役commandを発行しない。承認なら、bindingを解除する前に「承認済み退役」を既存command出力へ渡し、その後にApp側の終端処理を完結させる。
- hookの返値には、要求を消費し、承認済み退役を出力へ移譲したことを表す結果を用意する。仮称は`RetirementTransferred`とする。この結果を通常の`Close`要求へ戻したり、Appへ再照会したりしない。
- 段2の`ViewportCloseResponse::Close`を使う互換経路も、hook前に捕捉した登録寿命への承認として、同じ型付き退役処理へ正規化する。ViewportIdだけを使う第二の直接cleanup経路は残さない。
- `DelegateToUi`は非管理viewportの従来契約として維持する。mIVのApp ownerが既に終了したことを理由に、承認済み退役をこの結果へ落とさない。
- App起点の承認済み退役を先に処理し、対象資源が終了した後で、同じ旧寿命のOS Closeを未知の子として再配送しない。対象が存続するmodal拒否では、一回の要求消費後も新規Closeを受け付ける。
- pass外hookから発行された承認済みcommandも、その非描画境界で取り出して処理する。次のFull、子callback、追加repaintを待たない。

現在の段2実装は`vendor/eframe/src/native/viewport_lifecycle.rs:24`以降で、hookの`Close`結果からViewportIdを使って直接退役する。第4.5版では、この受渡しを上記の寿命付き承認へ改める。hookのmodal判断や通常入力の所有契約は変更しない。

##### 承認後の対象限定退役

Appが登録を外しても、親の次のFull passだけを待ってbackendを掃除する設計にはしない。rootもlogic-onlyの場合を含め、承認した子を復元せずに終了できることを契約とする。

- App側は既存終了ownerで対象のClosing／登録解除・worker／media終端を行う。
- backend側は承認結果から、対象window、WindowId対応、egui-winit State、surface、保留入力を通常終了と同じ所有境界で退役させる。
- eguiのviewport登録・親子登録も同じ終了に合わせて取り除き、後の兄弟FullOutputから閉じた子が再作成されないようにする。
- 現在は通常Full後に行われるviewport GCを、対象を明示した共通退役処理へ抽出する。Context側にはそのためのeframe向け非描画退役入口を新設し、既存GCと処理範囲を共有する。UI passを実行してGCを起こす方法は採らない。
- 退役入口はpass外で、Appの終了ownerが確定した対象にだけ使う。rootや存続する兄弟の入力・memory・cache・surfaceを一括resetしない。
- 対象viewportに未配送texture deltaがあれば、§4.9の共有renderer ownerへ引き継いでから退役する。window終了を理由に共有deltaを捨てない。
- 既存終了処理が持つ保存・取消の意味を維持する。まれな失敗用のretryや新しい回復状態は追加しない。

本家のbackend GCは`eframe-0.37.0/src/native/wgpu_integration.rs:1330`、`src/native/glow_integration.rs:1533`。Context登録の通常GCは`egui-0.37.0/src/context.rs:3317`以降。現本家にこの非描画退役入口があるとは扱わず、今回の限定追加としてvendor台帳と段2へ計上した。段2で実装した入口だけではAppからの受渡しが成立しないことが段3で判明したため、以下の型付き出力・寿命同定・配送の追加は段3へ計上する。

###### 承認と権限移譲

採用する方式は、既存viewport command出力への「承認済み退役」の追加である。以下では仮称を`ViewportCommand::RetireApproved`、対象を表す値を`ApprovedViewportRetirement`とする。

このcommandは終了可否を問う要求ではなく、既存終了ownerが確定した資源寿命の終了をbackendへ移譲する終端出力とする。

1. Appは既存modal・取消・保存方針を使って終了を判断する。
2. 終了が確定した時点で、window／context bindingを解除する前に、exactな対象viewportの登録寿命を捕捉する。
3. 対象寿命と、その寿命に所有される子孫への退役権限を、一つの型付きcommandとして既存root command列へ移す。
4. その後はAppの既存終了処理を進め、runtimeとbindingを終えてよい。backend受理待ちのClosingや同定用runtimeを残さない。
5. backendはcommandの対象を使う。終了済みのApp登録、現在のactive context、foreground、rectから対象を推定し直さない。

対象はViewportIdだけでなく、その登録寿命の識別子を含む。Context登録とbackend資源は同じ識別子を持ち、必要な既存window／context bindingにもその対応を保持する。新しい登録寿命は実際の登録作成時に割り当て、明示的な再openでは新しい寿命にする。永続egui Idやsurface resize generationは流用しない。

既にContextの通常GCが済んでいてnative資源だけが残る場合も、既存bindingが持つ旧寿命への承認を出力できる形にする。存在しないviewportを`viewport_for`等で新規登録してから退役する方法は採らない。初回登録前でContext・native資源のどちらも存在しない対象は、App側の既存要求を終端させればよく、架空の登録やwindowを作らない。

子孫への権限は、対象の登録寿命を根とする所有関係に限定する。command作成時のContext上の子孫情報を保持し、pass外で残存native資源を照合する際も、同じ寿命の親子関係だけを辿る。ViewportIdだけが一致する別寿命や、独立した兄弟を巻き込まない。子孫ごとの再承認hookは呼ばない。

同じ終了処理内で子と祖先の承認が重なった場合は、既存command列の正規化で重複をまとめ、各資源寿命に対する実効的な退役は一度にする。二重終了を防ぐ別ラッチや、退役済みID集合は作らない。

rootはこのcommandの退役対象にできない。root command列は搬送先にすぎず、root自身のClose、CancelClose、tray置換、保存待ちの意味は変えない。

###### GC前の保持とFull／LogicOnly配送

子自身の`ViewportOutput.commands`だけに承認を置くと、未使用childのGCによって出力前に失われる。搬送先は既存root command列とし、子の登録が消えても承認を保持する。

- Fullでは、最外側の`run_ui`／`run_pass`が完了した出力へ渡す。Appのlogic／uiから直接Context退役を呼ばない。
- immediate子の入れ子passでは、親passが残る間に退役を実行しない。承認済みcommandを最外側まで保持する。
- discardによる再passでも承認を失わない。承認済みcommandは途中passで搬送先から取り去らず、最外側のrun全体が戻る時に出力へ移す。通常commandの既存集約と区別して扱い、第二queueを作らない。
- `begin_pass`／`end_pass`を直接使うconsumerには、最外側の出力を消費してから次の処理へ進む同じ契約を適用する。
- LogicOnlyでは、`run_logic`の`LogicOutput.viewport_commands`へ渡し、その処理が戻ったpass外で消費する。
- OS Close用のpass外App hookから発行した場合は、既存command列を同じ境界でdrainする。drainのためにUIや`App::logic`をもう一度呼ばない。
- FullOutputのappend、EpiIntegrationの既存出力保留、子の出力集約を通っても承認を落とさない。
- 両backendは、承認済みcommandを一般のwindow command処理より先に取り出す。対象native viewportが既に存在しないことを理由に、commandごと読み飛ばさない。

backendは、最外側の処理から戻った後、次のApp処理へ入る前に、受け取った承認済み退役を完結させる。surface描画の有無、通常backend GC、rootの終了確定による早期returnより前に、この出力を処理する。

現在の実装では`vendor/egui/src/context.rs:3338`のGC後に`:3347`以降でviewport outputを組み、`:3352`でcommandを取り出す。`run_logic`の取り出しは同`:1174`以降である。上記は、この既存搬送経路に追加する契約であり、現コードで成立済みとは扱わない。

###### 古い出力と再作成

承認済み退役は、その出力内の古いbuilder・callback・通常commandより優先する。

- cleanup後、同じ旧寿命のViewportOutputからnative windowを作り直さない。
- 現在のContext登録寿命と一致しない出力を、登録作成・更新の根拠にしない。
- 同じViewportIdを明示的に再openして新しい登録寿命ができた場合、旧出力で新しい窓を更新・終了しない。
- FullOutputを集約する際も、異なる登録寿命のbuilder・callback・commandを一つの寿命の出力として混ぜない。
- 退役対象自身だけでなく、その旧寿命の子孫についても同じ条件を適用する。
- EpiIntegration等が持つ既存出力内の対象情報も処理対象とする。退役済みIDを別に記録して後から抑止する設計にはしない。

同じ承認出力が集約や診断cloneを経ても、資源ownerを実際に取り外せるのは一度だけとする。既に存在しない旧寿命への再適用は、新しい窓の作成やApp hookの再呼出しを起こさない。診断cloneを独立した実行ownerにしない。

###### cleanupの順序

両backendは、承認済み退役と通常GCで対象資源のcleanupを共通化する。次の順序を満たす。

1. 現在までのFull／immediate出力が持つtexture deltaを共有renderer ownerへ移譲する。対象windowの資源と一緒に共有deltaをdropしない。
2. 対象寿命と子孫の集合を、commandに保持した情報、現在のContext登録、native資源の所有関係から確定する。既に一部がGC済みでも、残っている所有者を取りこぼさない。
3. 対象の新規callback配送・古い出力適用を止め、native側のwindow対応、入力配送、保留RawInput、focus／resize参照、surface／glutin資源等を対象限定で退役させる。native資源の親子依存は子から解消する。共有rendererは残す。
4. backend borrow／lockを解放し、pass外でContextの同じ登録寿命を退役させる。callback等のuser-owned値のdropもContext lockの外で行う。
5. IME policy等の対象別状態を同じ集合について終了する。ContextのGCで既に登録が消えていた場合も、`retire_viewport`が返した「今回削除したID」だけに依存してIME cleanupを省略しない。同じViewportIdの新寿命が存続する場合は、その状態を消さない。
6. Context／callback／TextureHandleのdropで追加のtexture deltaが生成された場合も、共有renderer ownerへ移譲する。既存のset適用・submission・free順で完結または引き継ぐ。回収のための追加repaintは使わない。
7. rootも終了する場合は、その後に共有renderer／Contextの既存終了契約を完結させる。root終了を理由に承認済みcommandや未処理deltaを捨てない。

Contextの通常GCが先に一部を除いていても、保持済みの承認とnative資源の寿命から残りを終了できることを必要条件とする。AppへのClose完了callbackが来ることを唯一の完了根拠にしない。

存続する兄弟の入力、IME、memory、cache、texture、surface、worker、世代には触れない。通常GCも承認済み退役も、共有texture managerや全viewportを一括resetしない。

###### Appの終了入口と非管理viewport

| 入口・状態 | 受渡し |
|---|---|
| mode change、Remote取得時の全ローカル終了 | 各対象の既存終了判断を行い、binding解除前に同じ承認済み退役入口へ渡す |
| active detached sessionの終端 | `finish_active_detached_session_close`／`retire_terminal_detached_viewport_identity`の同定情報を失う前に移譲する |
| parked／passiveの終了、contextのclose-and-retire | 各window／context ownerで終了を確定し、その対象寿命へ移譲する。activeの現在値を代用しない |
| barの×、watcher | 既存modal判断・終了serviceへ合流し、承認後は同じ退役出力を使う |
| OS Close、通常のprogrammatic Close | 未承認の要求としてhookへ渡し、拒否なら一回消費、承認なら同じ退役出力へ正規化する |
| parking、hide、最小化、表示切替の途中 | 寿命終了ではない限り発行しない。parked-liveの存続を壊さない |
| mIV非管理viewport | 従来のUI委譲を維持する。管理対象不明を一律承認する処理を足さない |
| native presenter等の独自HWND | 既存native終了owner／control配送を維持する。このcommandでegui非所有HWNDを直接破棄しない |
| root終了 | 既存root lifecycleを維持し、子の承認済み出力とdeltaを完結してから共有資源を終了する |

`close_all_detached_viewers_for_mode_change`末尾の空の`show_viewport_immediate`、それに続く退役目的の`Visible(false)`と「描かないことでGCに消させる」経路は、この受渡しへ置き換える。実際の終了観測も、架空のVisible command発行を完了根拠にしない。

WIPの影響箇所は`src/app.rs:3810`、`:60709`、`:61213`、`:61384`付近、`src/ui_fullscreen.rs:22175`、`src/remote_ipc/ui.rs:1214`。対象の終了helperへContextまたは承認済み出力のsinkを明示して渡し、同定できなくなった後に現在のfullscreen viewportを再計算しない。その他の呼出しは次で棚卸しする。

`rg -n 'retire_terminal_detached_viewport_identity|finish_active_detached_session_close|close_all_detached_viewers_for_mode_change|close_detached_image_windows_by_ids|close_and_retire_context' src`

通常GCは、参照されなくなった通常viewportの資源を従来どおり終了できる。ただし、Appの承認済み退役の代用品にはせず、GCをClose承認や完了通知の生成器にしない。承認済みcommandがある資源については、GCが先でも後でも、同じcleanupで残存資源を一度だけ終了する。

###### 第4.6版: Close後の表示継続の移譲

資源退役の承認だけでは、リリース済みの「閉じたら元の一覧へ戻る」動作は完結しない。終了するviewerが持つ復帰先をteardown前に存続ownerへ移す。通常Closeの表示継続を削除せず、Remote取得や独立した新navigationで失効する従来の意味も維持する。

**所有者と型**

既存`pending_return_to_parent`のboolを、仮称`FullscreenCloseContinuation`という型付き状態へ置き換える。新しいApp fieldや第二slotは作らない。

状態は、帰還要求なし、既存の生存viewerから解決する帰還要求、終端viewerから移譲済みの帰還要求を区別する。終端要求には、次の排他的な効果を持たせる。

| 効果 | 保持する内容 |
|---|---|
| bookmark一覧の更新・位置復元 | 元bookmark openのidentity、一覧を所有するmain context、target、gridのrow keys／opened keyと復元情報 |
| bookmark mediaが対象を離れた後の実フォルダ復帰 | 元bookmark ownerのidentity、最後の利用者視点の実フォルダ、最後のmedia pathと選択intent |
| standalone bookmarkから一覧へ戻る | 元bookmark owner、一覧復元情報、既存Bookmarks navigation |
| auto-open等の親一覧へ戻る | 解決済みの既存`AddressBarNav`、選択・履歴intent、帰還先mainの所有情報 |

上記は効果の設計上の分類であり、別々のqueueを意味しない。一つの帰還要求が必要な所有値を持つ。

帰還先は「現在mountされているcontext」ではなく、一覧を所有していた存続mainとする。既存のmain navigation proof、quick-folderの切替identity、bookmark open request identityを用い、新しい採番ownerは作らない。bookmarkの同一pathを開き直した場合も区別できるよう、必要な既存request identityはOpening／Detachedの所有値と一緒に保持する。

**移譲の時点**

1. 既存modal等の終了可否判断を行う。拒否したCloseは帰還要求を作らない。
2. 終了を確定した後、`close_fullscreen`、context retire、bookmark return state消去、window binding解除より前に、対象contextの帰還情報を捕捉する。
3. bookmark由来なら、元mainの帰還ownerとの対応を確認し、必要なtarget／grid／終端summaryをそのownerへ移す。元mainが既に別navigationへ移っている場合は、新しい帰還要求を作らない。
4. 既存navigation帰還slotへ型付き要求を移譲する。破棄されるviewerのslotに残さない。
5. 第4.5版どおり承認済み退役を移譲し、viewer／worker／media／contextを終端させる。帰還表示のためにClosingやbindingを延命しない。

`ClosedBookmarkSummary`は、終端後にboolへ変換して捨てる値にしない。必要なsummaryはteardown前に取得し、型付き帰還要求の材料として消費する。`close_and_retire_context`のfinish後に読むだけでは、既にbookmark ownerを消している場合がある。

親navigationの解決も破棄前に行う。`effective_folder`／archive source override、Collection・bookmark・履歴・rating・subfolder・Smartの既存優先を再利用する。変換済みアーカイブのキャッシュZIPを利用者の帰還先にしない。resolverが現在行う選択予約の更新は、非描画で返せる型付き値へ抽出し、実際のload・一覧再構築はUI consumerに残す。

**bookmarkの従来動作**

- detached bookは、内部のページ／container移動だけでは元bookmark一覧への対応を失わない。
- detached mediaが元targetに留まった場合は、mainのbookmark一覧を既存の非同期read-model更新へ渡す。
- detached mediaが元targetを離れた場合は、最後の実フォルダへ戻り、最後のmediaを既存の選択規則で可視化する。
- mainで利用者が選択・scrollを変えていた場合、従来どおり現在のmain状態を優先する。帰還要求に保存した古い選択で上書きしない。row keys／opened keyは増減検知と位置復元に利用する。
- standalone bookmarkは、teardownでOpeningが消える前に一覧復帰情報を移す。

`reconcile_closed_bookmark_detached_context`の判断を複製せず、「終端前に帰還効果を作る部分」と「UIで既存一覧更新・navigationへ適用する部分」へ分ける。現在のmainを読んで更新する処理を、そのままlogicから呼ぶ方法は採らない。

**消費する位置**

消費するのは帰還先mainの正当なUI passである。通常経路は、既存`take_pending_return_to_parent_nav`から`fullscreen_close_nav`へ渡す位置と、既存のnavigation合流点を使う。

bookmark refreshも同じ帰還ownerから一度だけ取り出し、既存の`bookmark_view_begin_restore`／`refresh_bookmark_browser`／grid復元へ渡す。実フォルダ・親一覧は既存navigation dispatcherへ渡す。新しいload経路や同期I/Oを作らない。

- Fullのlogicで受け取ったCloseは、その後のUIが既存消費位置へ到達すれば同じFull内で進められる。
- LogicOnlyやpass外Close hookでは、資源終端と帰還要求の移譲まで進め、表示適用は次の帰還先UIまで保留する。
- 子や兄弟のFullではmainの要求を消費しない。
- 既存のUI早期return／native close即時適用位置も、同じ型付きownerを使う。従来同じdispatchで消費する経路を、無条件に次frameへ遅らせない。
- 非描画hookから即時UI適用helperを呼ばない。新しいmode flagや強制UIで入口を共用しない。

帰還要求は既存dispatcherへ渡した時点で一回消費する。読込拒否・通常の失敗は既存のログ・通知で終端し、古い帰還を再配送しない。

**失効と上書き**

失効はUI復帰時の後付け判定ではなく、新しい操作を受理する既存ownerの境界で行う。

- Remote取得は、全ローカルviewerを閉じる前に、既存のlocal open失効と同じ境界で帰還要求を失効させる。Remote目的のCloseから新しい帰還要求を作らない。
- mainの独立した新navigationを受理したら、`retire_replaced_main_list_requests`等の既存admissionで旧帰還要求を置換・失効させる。同じpathへの新navigationも含める。
- 新しいbookmark openを受理したら、旧帰還要求と旧bookmark ownerを同じ受付で置き換える。遅れて届いた旧viewer Closeは新bookmarkの帰還要求を消去・上書きしない。
- 帰還要求自身の消費は、自分を失効させる「別の新navigation」と区別し、既存request ownerの保持指定と同じ方式でhandoffする。
- 単なるselection／scroll変更や同じ一覧のread-model更新は、新navigationとして帰還要求を消さない。
- root終了では既存終了方針に従って帰還表示を終端させ、UIを開き直さない。

終了理由は、通常Close、別navigationによる置換、Remote取得、App終了等の既存causeから型付きで渡す。reason文字列から意味を推定したり、新しいpending boolを足したりしない。

**active／standalone／parked**

| 対象 | 契約 |
|---|---|
| active at-rest | summaryを捨てず、破棄前に存続mainへ移譲する。旧reconciliationと同じ一覧更新・実フォルダ復帰へ戻す |
| standalone fullscreen | bookmark／auto-open帰還を破棄前に解決し、mainの既存帰還ownerへ移す。直接teardownだけで終えない |
| parked／passive | 保存されたcontextのoriginを非描画のowner付きアクセスで読む。元mainの帰還ownerが存続する場合だけ同じ移譲を行う。無関係なmainや新bookmarkへ移動を発行しない |
| parking／resume途中 | Closeではないので帰還要求を作らない。存続viewerのoriginを保持する |
| Remote／新main navigationによる全終了 | 先に既存帰還ownerを失効させ、通常Closeの一覧復帰を発行しない |

主な影響箇所は`src/app/viewport_retirement.rs:34`以降、`src/app.rs:4146`、`:4177`、`:23030`、`:23436`、`:23468`、`:33511`、`:50707`、`:61182`、`:64873`、`:65144`、`:77607`、`:78142`、`:95230`、`src/app/viewer_context_registry.rs:5679`、`src/app/update_phases.rs:1088`以降。既存boolの型変更はcontext bundleの初期化・swap・fixtureも同時に揃える。

棚卸し式:

`rg -n 'pending_return_to_parent|BookmarkViewState|ClosedBookmarkSummary|reconcile_closed_bookmark_detached_context|take_pending_return_to_parent_nav|handle_fullscreen_close_request' src`

dirty集合の消費は、補修済みの既存grid UI consumerへ残す。帰還修正のために非描画Closeへdirty consumerを戻さない。

##### Close以外の要求・通知の棚卸し

| 対象 | 確認した経路・扱い |
|---|---|
| ViewportEvent | 0.37.0の離散variantはCloseだけ。別の同種lifecycle要求は見当たらない。根拠: `egui-0.37.0/src/data/input/viewport_info.rs:6` |
| programmatic Close | ViewportCommand::CloseもClose生成源。上記と同じ受付へ揃える |
| focus・minimize・maximize・配置・DPI・occlusion | window状態／入力情報。終了要求ではない。通常入力・metadataの所有契約を維持し、Close queueへ変換しない |
| focus喪失等による右drag取消 | passive callback内の`capture_right_drag_event`から既存Cancelを生成する経路がある。`src/app.rs:2040`、`src/ui_fullscreen.rs:21622`付近。callback依存の状態終端として回帰監査対象に含めるが、全focus／releaseを汎用終端配送へ移す理由にはしない。Close時は対象の既存gesture ownerも終了することを検査する |
| FirstCallback・初期配置適用・表示ACK | 実際のcallback／表示を知らせる通知。`src/app.rs:1901`、`:2010`付近、`src/ui_fullscreen.rs:21495`。Closeのために偽造しない。初回callback前のCloseも終了できることを検査する |
| 独自barの×・watcher Close | 既存App要求源。最小化中のOS Closeの代替ではない。同じ終了判断へ合流 |
| ActivationTokenDone | backendがFrameへ応答を保持する経路。子callbackだけの読取りではない。wgpu `:987`付近、glow `:1024` |
| native presenterのClose／Destroyed | 既存native pump・WindowEpoch経路。`src/video/native_window_pump.rs:1465`以降。egui子Closeを重ねて配送しない |

今回確認した範囲では、Close以外に追加の離散ViewportEvent要求はない。focus由来のgesture取消等は性質を分けて記録し、既存順序・所有を保つ回帰を行う。具体的な失敗が示されていない状態まで、新しい汎用lifecycle機構へ広げない。

##### 必須回帰

- child最小化→Close→子を復元せず終了。子callbackの呼出回数が増えていないこと。
- rootもlogic-onlyのままで、承認した子のwindow・backend／Context登録が退役すること。
- modal拒否→modal終了→子復元で、同じCloseが再発火しないこと。
- 拒否後の新しいCloseは受理できること。
- 子Full／親logicとの順序が変わっても、同じ要求を二重処理しないこと。
- 初回子callback前のCloseを処理でき、表示ACKを偽造しないこと。
- 兄弟の入力、IME、保留RawInput、texture、surface、寿命を維持すること。
- 承認後の兄弟Full／root Fullで閉じた子を再作成しないこと。
- OS Close、programmatic Close、bar／watcherの既存App入口で同じmodal判断・終了ownerを使うこと。

追加repaintやRequestPaintWhileHiddenで閉じる動作を成立させる試験にはしない。既存終了処理が通常の状態反映として行うrepaintは保持できるが、その到着やFull passを終了完了の必要条件にしない。

この受付・消費・非描画退役はdetachedの構造修正として、着手前の双方合意と§11記録を必要とする。

第4.3版の案Bでは、Windowsの最小化単独では要求済みFullの子callbackを省略しなくなる。上記の本家調査は変更前の事実として保持するが、通常最小化のUI省略をmIVの採用仕様にはしない。

Close専用の非描画受付・消費・退役は維持する。期限超過や初回callback前、親Fullなしでも終了を進める必要があり、子UIが通常動くことだけをCloseの完了保証にはしない。案Bの通常UI継続と、Closeを成立させるための強制UI passは別である。既存Close回帰は後者へ依存せず通す。

#### 保留RawInputの最小修正

本家EpiIntegrationはアプリに一つであり、`pending_raw_input`も一つである。updateはdeferred callback付きでもprepare_raw_inputを呼び、update_logic_onlyも同じ領域を使う。appendはイベントを連結しつつviewport_idを新しい側へ置き換える。

根拠: `eframe-0.37.0/src/native/epi_integration.rs:175`、`:188`、`:337`、`:384`、`:418`、`egui-0.37.0/src/data/input/raw_input.rs:162`、`:168`。

このため、共有保留領域をそのまま残す方針は訂正する。

**推奨実装は、既存保留領域をbackendのViewport ownerへ移す方式とする。**

1. EpiIntegrationのアプリ共通`pending_raw_input`を除く。
2. backendの各Viewportが、自身の未消費RawInputを所有する。既存保留領域の移管であり、App側や別registryへ第二queueを作らない。
3. 入力取得側が、同じViewportの保留分と新規State入力だけを古い順に連結する。最新metadataを使うappendの意味は維持する。
4. Full処理はその入力を一度だけeguiへ渡す。別viewportの保留へアクセスしない。
5. root LogicOnlyはlogic実行後の未消費RawInputを呼出元へ返し、同じroot Viewportへ戻す。
6. childでUIを実行しないときは、show_ui判定をStateのtakeより前へ置き、入力をStateに残す。childのための別保留queueは追加しない。
7. immediate経路も同じViewportの入力取得入口へ揃える。deferred／immediateの更新経路が変わっても、別viewportへ流れないようにする。
8. viewportの明示終了時だけ、そのownerの未消費入力を終端する。hide／minimizeでは消去しない。兄弟やrootの保留へ触れない。

修正対象となる本家境界は`eframe-0.37.0/src/native/wgpu_integration.rs:700`、`:716`、`:773`、`:1251`付近、および`src/native/glow_integration.rs:689`、`:708`。glowを製品rendererへ変更する意図はないが、共有EpiIntegrationの署名とskip契約を同じ形へ合わせる。

所有者の取り違えをguardで黙って捨てる方式にはしない。入力を組み立てる段階から同じViewportの領域しか選べない形にし、テストでviewport_idとevent列を検査する。

ユーザーコード実行中にViewportsのborrow／lockを保持しない。logicからUIを呼んで保留を消費する対策も採らない。

#### raw_input_hookの契約

hookは、**同じviewportの保留分と新規分を連結した後**に従来どおり呼ぶ。root LogicOnly／Fullとdeferred Fullの既存呼出位置を維持し、skipしたchildを保存するためだけの追加hook呼出はしない。

hookを汎用復帰フィルタにしない。古い保留と復帰後の新規入力を一括削除せず、host固有正規化にもContextの古いeventsを使わない。

#### 保留入力の必須回帰

段2cで、窓を作らない実integration／入力取得helperの回帰を置く。

| 入力列 | 期待結果 |
|---|---|
| root LogicOnly(A) → child Full(B) → root Full(C) | childはBのみ。rootはA＋Cを順序どおり一度だけ消費 |
| root LogicOnlyを複数回 → child Fullを挟む | rootの各入力がrootへ残り、childへ混入しない |
| child skip → 他viewport Full → child復帰 | childのrelease、WindowFocused、ModifiersChanged、IME終端が保持される |
| child skipを複数回 | 入力の欠落・重複・逆順がない |
| child復帰時に新規入力も到着 | 終端と新規入力の順序を保持し、新規分を捨てない |
| child終了、兄弟は存続 | 終了対象の所有だけを終端し、兄弟の保留を変えない |
| deferred／immediate入力取得 | 同じViewport所有の入口を使用する |
| hook観測 | 各呼出で正しいviewport_idと同じviewportのevent列だけを受ける |

focus／IMEの回帰は、イベント列の存在だけでなく、復帰pass後のkeys_down・composition・実TextEditの状態も検査する。

これはdetachedに関わる構造変更である。段2c着手前に設計担当と独立レビュー担当が合意し、`docs/detached-rework-plan.md` §11へ範囲・所有不変条件・判断理由を記録する。

#### 入力源ごとの最小契約

| 入力源 | 方針 |
|---|---|
| 通常OSキー／pointer | 非表示直前のbatch、遅着release、capture終端を所有者へ保持 |
| focus／Modifiers／IME | viewportごとの順序・正規化を保持 |
| Win32 edge queue | 既存enqueue／focus／destroy終端とdrain条件を保持 |
| gamepad | 物理更新と操作受付を局所分離 |
| WH_GETMESSAGE | 既存main／fullscreen配送先・drain条件を保持 |
| 合成入力、Remote、トレイ、IPC | 明示制御経路を通常hardware入力と一括して捨てない |

#### Win32 edgeと受信時点の所有権

RoutedKeyEdgeは対象viewportへ既に届いた離散キーの証拠である。後から同じframe内でfocusを失ってもdiscrete入力として採用する。OS直読み・frame最終levelにはfocused permitが必要である。

根拠: `src/keyboard_input.rs:106`、`:159`、`src/app.rs:55249`、`src/ui_fullscreen.rs:32137`。

保持事項:

- hide時に`pending`／`frame`／`operation_frame`を一律pruneしない。
- 受信済みキーを現在のGetAsyncKeyStateでfresh／stale判定しない。
- ReturnKeyStateのenqueue更新、WM_KILLFOCUS・destroy終端を共用する。
- 兄弟viewportの記録をroot hideで変更しない。
- `routed_edge_keeps_only_discrete_input_after_focus_loss`の期待値を維持する。

根拠: `src/key_input.rs:2089`、`:3971`、`src/keyboard_input.rs:809`。

focus喪失の合成releaseは`winit-0.30.13/src/platform_impl/windows/keyboard.rs:100`、egui-winitのfocus処理は`egui-winit-0.37.0/src/lib.rs:453`、`:483`。

PointerGoneは押下解除ではなく、WM_CAPTURECHANGEDもwinitではreleaseを生成しない。回帰対象とするが、この事実だけで汎用終端APIを追加しない。

根拠: `egui-0.37.0/src/input_state/mod.rs:1219`、`winit-0.30.13/src/platform_impl/windows/event_loop.rs:1927`。

#### ゲームパッドの移行

gilrs threadはforegroundに関係なくqueueへ追加しrepaintを要求する。現sampleはpressから操作を作り、配送時にforeground／modalを判定する。

根拠: `src/gamepad.rs:170`、`src/app/gamepad_input.rs:2158`、`:2373`、`:2510`。

UI側だけでdrainすると最小化中のpressを復帰後の候補にし得る。sample全体のlogic移動も、frame番号・pass indexによる再配送制御があるため採らない。根拠: 同`:2110`。

移行契約:

1. 物理drain・押下／軸／disconnect更新を、UI passが進まなくても実行可能な単一ownerへ分ける。
2. 操作batchとmulti-pass配送は既存経路に残し、logicとuiの二重drainを作らない。
3. 操作先不在でもrelease・disconnect・中立更新を保持する。非前面を設定OFFと同一視してthreadを停止しない。
4. logic未実行の短い退避→復帰も、gilrs取込み側で既存OS前面判定を共用して扱う。復帰時のforegroundだけで選別しない。
5. 受付事実はPadイベント／同じ保持ownerに局所化する。全入力共通層、新App pending bool、時間窓を作らない。
6. 正式な操作先・modal判断はmounted contextの既存配送側に残す。producerからApp／egui lockを取得しない。
7. 既存continuous timer抑止と`suppress_pending_actions`を再利用し、受付不可期間の離散press／repeatを復帰後に発火させない。
8. **軸は物理値を更新し続け、復帰後は保持中の軸値で操作を続ける。中立通過を新たに要求しない。** 別アプリで倒したまま復帰したときの継続はリリース済み挙動であり、離散press抑止と分ける。
9. 短いpopup後の軸継続等、既存modal抑止の挙動を一括変更しない。
10. ringの意味終端・Undo構築は既存context所有で行う。root投影で確定したり、disconnectの終端を捨てたりしない。
11. **logicで受けた未配送edgeと意味終端を、配送側が取り出すまで同じ物理ownerで順序保持する。** press→releaseが複数logicに分かれても、最終保持状態や最新batchで置き換えない。UI入口で一度取り出し、multi-pass用batchを固定する。

現SampledPadTransactionはframe番号付き単一batchであり、この未配送保持をそのまま担わせない。操作追跡OFFでdiscard後のpassから新しいreleaseをpollする既存契約も維持する。

根拠: `src/gamepad.rs:375`、`:380`、`:497`、`:653`、`src/app/gamepad_input.rs:73`、`:509`、`:2115`、`:2260`、`:2342`、`:2398`、`:2732`、`:2764`、`:2809`、`:3657`、`:9809`。

段1bでは旧順序の抽出に留め、logic接続と受付事実利用は段3で行う。引継ぎowner整理後に規模を再評価する。

#### WH_GETMESSAGE pending

`src/lib.rs:786`のWM_APPCOMMAND、`:801`のBrowserキーはpending countへ入る。`:836`のhookは現在のUI thread限定であり、他アプリの全キーを拾うglobal hookではない。

消費側は`src/app.rs:55234`、`:59997`、`src/ui_fullscreen.rs:32101`。既存配送先・重複防止drainを保持する。

#### 追加しない機構

- 全入力共通の「通常配送／終端のみ配送」
- eguiへの「clickを作らないrelease」API
- hide境界のRawInputフィルタ
- Win32 queueの一律prune
- 復帰直後の一回捨て、時間窓、常時追加repaint
- App側の第二RawInput保留queue

**リリース済み挙動:** 正当な離散キー・release確定・gamepad軸継続を保持する。保留所有修正は入力の誤配送・欠落を防ぐための構造修正であり、入力を取消す仕様変更にはしない。

#### API移行

- 描画はrun_uiと受け取ったroot Ui。
- headlessはrun_pass、必要箇所だけroot_ui。
- viewport callbackは`&mut Ui`、Contextはui.ctx()。
- Contextのstyle系はglobal_style系へ。Ui局所styleは一括置換しない。
- UiのDerefで解決するforwarding methodを無意味に書き換えない。
- on_exit署名をglow featureと揃え、終了処理本体を保持する。

根拠: `egui-0.37.0/src/context.rs:950`、`:983`、`:1024`、`:2599`、`:4662`、`src/ui.rs:77`、`eframe-0.37.0/src/epi.rs:227`、`src/app.rs:97792`。

### 4.3 Panel統一と計算高さ

影響箇所:

- `src/ui_main.rs:7400`、`:10291`、`:12590`、`:15558`: menu／toolbar／検索
- 同`:20434`: 下部情報
- `src/ui_folder_pane.rs:218`: side panel
- `src/ui_main.rs:17580`、`:19160`: central
- `src/ui_text.rs:3002`、`:3331`: Area固定パネル
- `tests/support/scroll_layout.rs:6`、`:31`、`:65`: geometry

Panel::showへ親Uiを渡し、default_size／min_size等へ移す。CentralPanel::showもUi版へ接続する。

根拠: `egui-0.37.0/src/containers/panel.rs:252`、`:372`、`:425`、`:1377`。

順序・残り領域を一つの親Uiへ揃える。slide、drag-to-close、端double-click collapseは既存操作と比較して明示設定する。根拠: 同`:337`、`:454`、`:741`。separatorは同`:348`。

Area＋Frame＋ScrollAreaの固定HUDを全面交換しない。新版も親available sizeを上限にするため、separator修正だけで独自計算高さは直らない。根拠: `egui-0.37.0/src/containers/scroll_area.rs:842`。

| backlog §1.380 | 扱い |
|---|---|
| clipboard失敗カード | `src/ui_dialogs/clipboard_capture.rs:378`、`:490`。可変高と固定仮想行の問題は残る |
| 長い保存基点名 | 固定本文予算の残件として扱う |
| 注釈右詳細・左一覧 | `src/ui_text.rs:2745`、`:2761`、`:3252`、`:3348`。sink・最低高・本文高を同じpassで検査 |
| 切り取り・表示トリム | 内外枠・固定操作の実矩形を確認 |
| 構成editor行 | §1.242の既存作業へ残す |

Fits／Scrolls、1物理pixel許容、wheelによる末尾到達を維持する。

**見た目・操作:** separator、余白、collapse、panel操作が変わり得る。既存操作保持を初期案とし、変更採用は§5へ送る。

### 4.4 フォーカス、wheel、drag、クリック、Ctrl+Q

#### フォーカス・drag

影響: `crates/egui-ime/src/lib.rs:242`、`:305`、`:329`、`src/tab_focus.rs:23`、`src/app.rs:22538`、`:22543`、`src/ui_music_panels.rs:706`、native HUD seek／音量。

Area／Window／ScrollAreaの非focus化と入力部品のfocus回復を区別する。Ctrl+矢印改善だけでhelperを撤去しない。全域Tab traversal無効の方針を保持する。本家Tab処理の根拠: `egui-0.37.0/src/memory/mod.rs:590`。

dragは重なり、widget外移動・release、disabled復帰、新規pressを検査する。背景Senseを増やしてclickを奪わない。

ScrollAreaはDragScroll::OnTouchが既定。旧mouse drag scrollを保持する箇所はAlwaysを明示する方針。根拠: `egui-0.37.0/src/containers/scroll_area.rs:224`、`:286`。

#### 連続クリック分類とrelease射影

0.33.3は時間間隔で連続クリックを分類する。0.37は直前clickとの位置距離も使い、既定max_click_distは6 pointsである。比較は距離が6 points未満かどうかであり、6 physical pixelsという意味ではない。

tripleも、旧版の「今回から2回前までがdelayの2倍未満」から、double成立に加えて直近2区間それぞれの時間条件を満たす形へ変わる。

根拠: `egui-0.33.3/src/input_state/mod.rs:1213`、`egui-0.37.0/src/input_state/mod.rs:114`、`:1385`。

既存のpress→release間の移動許容と、今回追加されたclick間距離は区別する。広い同じwidget上でも、click位置が離れれば連続clickと分類されなくなり得る。

mIVの影響箇所:

- `src/ui_fullscreen.rs:2506`: adjudicated release列
- 同`:37876`、`:37976`、`:45128`: double click操作
- `src/ui_main.rs:18440`: resize reset
- 同`:28189`付近: 連続clickの分類を利用するreader処理
- `src/video/native_presenter/overlay_draw.rs:7495`、`:7568`: 音量／速度reset
- `src/test_script.rs:5841`: Response判定のfixture
- `vendor/egui/src/input_state/mod.rs:1798`、`:1824`: 旧分類期待値

release射影APIが保証するのは、**本家が当該passで分類した全button releaseを順に返し、非clickもNoneで残すこと**である。旧版と同じcountを作ることではない。

移植時は次を分ける。

- 射影回帰: 全button・非click・focus区間・順序・discard後の非再生を維持。
- 分類回帰: 本家0.37の近接click、離れたclick、6 points境界、時間条件、tripleを検査。
- 製品回帰: double click resetやreader操作が採択した分類で働くことを検査。

旧fixtureの`(20,20)→(30,30)→(40,40)→(50,50)`に対する`1,2,3,3`は固定保証にしない。この位置列では新版分類に合わせた期待値に変更し、別の近接位置fixtureで`1,2,3,3`を含む射影も検査する。順序・非click等の検査を減らさない。

**方針:** 本家0.37分類の採用を推奨するが、リリース済み操作差なので§5で利用者に相談する。旧期待値を通すための旧判定、別タイマー、距離条件の無効化を追加しない。

#### raw_scroll_deltaの代替契約

影響:

- `src/app.rs:58738`、`:58760`: 一覧・列数
- `src/ui_main.rs:1462`: menu消費
- `src/ui_helpers.rs:414`、`:475`: 筆半径
- `src/ui_fullscreen.rs:25754`、`:27352`: 前面owner・残量終端
- runtime_ops、dialog、ラボ、テスト

旧値は単なる生delta合計ではなく、単位換算・修飾軸変換を含む。根拠: `egui-0.33.3/src/input_state/mod.rs:440`。

取得元は**消費位置でInputState.eventsに残るMouseWheel**。raw.events、smooth値、OS現在modifiersは使わない。

| 処理 | 契約 |
|---|---|
| Point | logical pointとして使用 |
| Line | line_scroll_speed × delta |
| Page | 当該passのviewport高さ × delta |
| 横修飾だけ一致 | `(x+y, 0)` |
| 縦修飾だけ一致 | `(0, x+y)` |
| 両方／どちらも不一致 | 軸保持 |
| modifiers | 各イベントの値 |
| phase／source | 保持。境界イベントを余分な移動にしない |

0.37 WheelStateのgesture中modifier latchとmIV離散操作を分ける。根拠: `egui-0.37.0/src/input_state/wheel_state.rs:104`。

混在modifiersは受理先と条件を保った連続区間で処理する。同じ条件では従来集約単位を維持し、notch数だけ一覧移動を増やさない。筆半径等の独自notch換算は保持する。

消費順:

1. 既存viewport ownerが入力を受ける。
2. fullscreen前面ownerをキー・navigatorより前に確定。
3. 必要なsnapshotはowner内に限定。
4. menu／popupがeventsと対応smooth値を消費。
5. App一覧処理はmenu等の後、grid直前で残ったeventsから投影。
6. 受理consumerがeventsとsmooth値を消費。
7. popup終了時だけdiscard_pending_scroll新版相当で内部残量を終端。

consume後にraw snapshotから復活させない。診断snapshotと配送投影を混同しない。

#### Ctrl+Q

本家はend-passでquit_shortcutsからCloseを送る。根拠: `egui-0.37.0/src/memory/mod.rs:337`、`src/context.rs:2935`。

各製品Contextで本家既定を明示無効化し、既存keymap／終了・tray ownerへ揃える。新しい終了操作として採る場合は相談する。

**挙動:** wheel所有・Tab・keymapを保持する。drag、連続クリック、終了操作の差をsnapshot更新だけで受け入れない。

### 4.5 非推奨API削除とIME

本家はIME owner、下線／cursor、候補窓位置を改善する。

根拠: `egui-0.37.0/src/memory/mod.rs:1064`、`src/widgets/text_edit/builder.rs:617`、`:1049`、`:1382`、`egui-winit-0.37.0/src/lib.rs:1291`。

これらはmIVのshortcut gate、取消補完、focus contract、Tab方針、Process filterを一括代替しない。

| 対象 | 方針 |
|---|---|
| 300ms grace／timestamp／旧pending-event API | 撤去済み。復活させない |
| 確定Enter除去／IME専用Tab lock | 撤去済みのまま |
| 全域Tab、dialog Enter／Escape | 保持 |
| viewport別composition owner | 保持 |
| Disabled取消終端 | producerから正規化へ必ず届ける |
| 空Preedit補完 | 実TextEdit取消回帰を維持 |
| focus recovery／次pass claim | 本家との重複を特定してから縮小 |
| Win32 0xE5 press／repeat除外 | 保持 |
| Hidden／Forwarded | 保持 |
| IME中断要求の生成 | Memory共通フラグを既存のviewport別Focusへ移管 |
| 同一viewportのIME出力集約 | 最新の矩形等を採用し、未配送の中断要求は保持 |
| nativeのcomposition中断出力 | 対象HWND ownerへ配送 |

影響: `crates/egui-ime/src/lib.rs:15`、`:78`、`:99`、`:116`、`:242`、`:305`、`:329`、`src/ime_focus.rs:1142`、`src/tab_focus.rs:23`、`src/app.rs:22538`、`:22543`、`src/key_input.rs:3960`以降。

中断要求の生成・集約に関する追加の影響箇所は、`crates/egui-ime/src/lib.rs:360`、`:378`のfocus要求、`src/ui_fullscreen.rs:22364`、`:24745`等のimmediate viewport、`src/video/native_presenter/render_core.rs:13702`、`:14007`の出力consumer。各呼出側へ補助フラグを散布せず、egui内の生成・集約境界を修正する。

#### Disabled取消終端の入力契約

本家egui-winit 0.37はIme::EnabledとDisabledをイベント化せず捨てる。根拠: `egui-winit-0.37.0/src/lib.rs:785`。

一方、WindowsのWM_IME_ENDCOMPOSITIONには、結果文字列が得られず空Preedit／Commitを出さずにDisabledだけを出す経路がある。

根拠: `winit-0.30.13/src/platform_impl/windows/event_loop.rs:1587`、`:1591`、`:1607`。

現vendorはDisabledを配送し、共有正規化が必要な空Preeditを補う。根拠: `vendor/egui-winit/src/lib.rs:393`、`:414`、`:605`、`crates/egui-ime/src/lib.rs:116`。

したがって、helper移植だけでは足りない。

**互換方針:** Stateの実IME変換入口でDisabledを下流へ配送し、既存正規化が取消に必要な空Preeditを挿入する。

0.37にもegui::ImeEvent::Disabledは非推奨として残るため、初期移行ではこの通知を限定的な互換境界として使う。必要な非推奨許容はこの変換箇所と対応matchに理由付きで限定する。crate全体の警告抑制にはしない。

根拠: `egui-0.37.0/src/data/input/ime_event.rs:6`。

Enabledを含む既存通知の扱いも共有policyの同等性fixtureへ含め、警告削減だけを理由に落とさない。新版Preeditのactive_range_chars変換は維持する。

本家TextEditはEnabled／Disabled自体を本文取消として処理しない。取消用の空Preeditが実TextEditまで届くことを検証する。

根拠: `egui-0.37.0/src/widgets/text_edit/builder.rs:1417`。

最小回帰は合成eguiイベントから始めず、**winit::Ime → 実State変換 → egui-ime正規化 → 実TextEdit**を通す。段2aの冒頭で新版準備graphに移行中の共有正規化を接続し、手書きの模倣normalizerは使わない。実windowは作らない。

必須列:

- 非空Preedit → Disabledのみで取消。
- 同一pass／別passの取消。
- focus喪失 → DisabledでUnfocusedCompositionを終端。
- 非空Commit → Disabledで確定文字を保持。
- compositionなしのDisabledで通常選択文字を削除しない。
- 重複Disabled／空Commitで取消を重複適用しない。
- 兄弟State／viewportへ取消が漏れない。
- 直後の通常Enter／Escapeが従来どおり働く。

#### Hidden／Forwardedを残す理由

本家HashSetは配送済みpressの履歴と同等ではない。

根拠: `egui-winit-0.37.0/src/lib.rs:483`、`:656`、`:1258`。

現vendorの保証を移植する。

- 配送済みpressを後続Process repeatでHiddenへ戻さない。
- releaseはallow_ime／logicalに依存せず記録をremoveし、Hiddenだけ遮断。
- focus喪失は当該Stateだけ終端。
- 通常press→Process repeatと逆方向の再切替でもreleaseを保持。

Disabled配送を追加しても、このkey hold履歴を消す契機にしない。IME終端と物理キーreleaseは別契約である。

traceは既定無効、文字秘匿、worker I/Oを保持する。vendor削除は、配送保証・取消終端が本家と同等になり、traceも不要と判断できた時点とする。

#### IME中断要求のviewport別所有

本家のrequested_interrupt_imeはMemory全体に一つのboolであり、各viewportのbegin_passでfalseへ戻る。end_passはその共有値を当該viewportのIMEOutputへ反映する。

根拠: `egui-0.37.0/src/memory/mod.rs:124`、`:799`、`:1079`、`egui-0.37.0/src/context.rs:3173`。

immediate子窓は同じContextで入れ子のrun_uiを実行する。子から戻る際にviewport IDは復元されるが、中断要求の共有boolは復元されない。

根拠: `eframe-0.37.0/src/native/wgpu_integration.rs:1264`、`egui-0.37.0/src/context.rs:3281`、`:3335`、`egui-0.37.0/src/memory/mod.rs:826`。

このため、親の要求が子のbegin_passで消える場合と、子の要求が親のIME出力へ混入する場合がある。mIVにもimmediate経路がある。製品上の具体的な発生操作は未検証であり、今回の判断はソースからの見立てである。

**最小修正は、既存フラグを既存のviewport別Focusへ移す方式とする。**

1. Memory全体のrequested_interrupt_imeを除く。
2. 既存のviewport別Focus entryに同じpass内の要求を所有させる。新しい独立mapやApp側の補助フラグは作らない。
3. interrupt_ime、request_focus、次frameへのfocus要求は、現在viewportのentryだけを更新する。
4. begin_passは開始対象viewportの要求だけをfalseへ戻す。親や兄弟のentryをresetしない。
5. end_passは終了対象viewportの要求だけをIMEOutputへ反映する。
6. 子から親への既存viewport復帰により、親のFocus entryが再び選ばれる。親子間の手動退避・復元処理を重ねない。
7. 非永続のpass状態として扱い、保存設定やmemoryの新しい永続形式を作らない。

既存のviewport別Focusは`egui-0.37.0/src/memory/mod.rs:503`、そのbegin_passは`:569`、Memory側のviewport選択は`:787`。focusからの中断要求は`:921`、`:931`、中断APIは`:1078`。

**最小回帰は同一Contextの実際の入れ子run_uiで行う。**

親子に別ViewportIdと実TextEditを用意し、事前passでfocusを整える。試験対象passの初期focus設定が、意図しない中断要求として混ざらないようにする。親run_ui内で子run_uiを実行し、親子のFullOutputを別々に検査する。

| 試験対象passの要求 | 親のIMEOutput | 子のIMEOutput |
|---|---|---|
| 親のみ。子を呼ぶ前に要求 | true | false |
| 子のみ | false | true |
| 両方 | true | true |
| どちらもなし | false | false |

親子ともSome(IMEOutput)を出していることを確認し、Noneによる見かけの成功を避ける。次の要求なしrunではfalseへ戻ることも検査する。Memoryのviewport IDをテストから手で切り替えるだけでは受入証拠にしない。

各FullOutputは検査後にheadlessの明示破棄を行う。親子のPlatformOutputをappendして試験結果を混ぜない。

この修正はdetachedに関わる構造変更として、着手前の合意と`docs/detached-rework-plan.md` §11記録に含める。App側で要求を出し直す、追加repaintで後から中断する、といった対策は採らない。

#### discard再passのIME出力集約

Context::run_dynは各passのFullOutputをappendする。しかしPlatformOutput::appendは`self.ime = ime.or(self.ime)`であり、後のSome(IMEOutput)が前の値全体を置き換える。TextEditは通常、各passのIMEOutputを中断要求falseで生成する。

根拠: `egui-0.37.0/src/context.rs:1111`、`egui-0.37.0/src/data/output.rs:242`、`egui-0.37.0/src/widgets/text_edit/builder.rs:1056`。

したがって、第1passで中断要求true → discard → 第2passで要求なし・IME出力ありの場合、最終出力の要求がfalseになり得る。nativeのcontrol配送を直しても、生成・集約段階で失われた要求は復元できない。

**同一viewportの未配送出力について、次の集約契約へ修正する。**

- purpose、rect、cursor_rect等は、新しいSome(IMEOutput)の値を採用する。
- should_interrupt_compositionは、集約前と新出力の要求のORを保持する。
- 新出力がNoneなら既存のSomeを保持し、従来のOption集約の意味を維持する。
- 両方NoneならNoneのままとする。
- 集約済み出力をbackendへ一度移譲した後は、次の新規出力へ要求を持ち越さない。
- OSのACK待ち、新しいqueue、retry、Context外のsticky flagは作らない。

| 旧出力 | 新出力 | 集約結果 |
|---|---|---|
| Some・要求true | Some・要求false | 新metadata・要求true |
| Some・要求false | Some・要求true | 新metadata・要求true |
| Some | None | 旧Someを保持 |
| None | Some | 新Some |
| None | None | None |

PlatformOutputはviewport IDを持たないため、呼出側も別viewportのplatform出力を混ぜない。共有texture deltaの集約とは区別する。

**必須回帰は実Context::run_ui内でdiscardさせる。**

1. 同じviewportで実TextEditを描く。
2. 第1passだけ中断を要求し、request_discardする。
3. 第2passでは中断を要求せず、TextEditからIMEOutputを出す。配置を変え、矩形等を第1passと区別できるようにする。
4. 実際に再passしたことを確認する。
5. 最終出力が第2passのIME metadataと中断要求trueを持つことを検査する。
6. 次の新規run_uiでは要求falseとなることを検査する。

PlatformOutput::append単体の補助試験だけでは受け入れない。nativeの複数logical batch試験とも別の回帰とする。出力は検査後にheadless明示破棄し、製品側の無条件clearは追加しない。

回帰名には共通の`ime_interrupt`を含め、段2aと正式graph切替後の両方で選択・実行できるようにする。修正前失敗・修正後成功の証拠は実装時に取得するものであり、今回実行済みとは扱わない。

この2件はmIV固有ではなく、最小再現と修正前失敗・修正後成功の証拠を添えて本家へ報告する価値がある。報告するかは利用者判断とし、今回は送信しない。

#### native経路のIME出力契約

0.37ではMemory::request_focusがIME中断を要求し、IMEOutput::should_interrupt_compositionへ出る。通常egui-winitはset_ime_allowedの切替で処理する。

根拠: `egui-0.37.0/src/memory/mod.rs:921`、`src/context.rs:3176`、`src/data/output.rs:113`、`egui-winit-0.37.0/src/lib.rs:1277`。

以下の配送契約は、上記のviewport別生成とdiscard再pass集約を修正した出力を前提とする。配送側で親子混入や生成済み要求の消失を補修しない。

mIV nativeはegui-winitを通らず、現在はcursor矩形だけをintentへ変換する。

| mIV箇所 | 内容 |
|---|---|
| `src/video/native_presenter/render_core.rs:13702`、`:14007` | IMEOutput→intent |
| `src/video/native_window_host.rs:149`、`:564`、`:618` | intent型・HWND owner・候補窓更新 |
| `src/video/native_window_pump.rs:1697` | WindowEpochに属するhostへの適用 |
| `src/video/native_window.rs:2308`、`:2341` | OS IME入力・終端 |
| `src/video/native_presenter/render_core.rs:9928` | native IME→egui入力 |
| `src/video/mod.rs:5159`等 | render出力のpump配送 |

移行契約:

1. IMEOutputからcursor位置と中断要求の両方を取り出す。
2. 中断を表すNativeWindowIntentを追加し、既存WindowEpochの対象hostへ送る。rendererから直接USER32／IMM32を呼ばない。
3. 対象は既存IME cursor更新・keyboard入力が所有するHWNDとする。GetForegroundWindowでその場の別窓を選ばない。
4. owner thread上でcompositionを中断する。現在のIMM32経路では、対象HIMCへのImmNotifyIMEの取消操作を基本とし、contextを解放する。通常のIME許可・候補窓位置を恒久的に無効化しない。
5. OSから返るPreedit／Commit／Disabledは既存native入力queueと正規化へ戻す。偽の通常キーや成功扱いのCommitを生成しない。
6. intentはlogical outputから一度だけ移譲する。再presentやmulti-passの同じ出力から再発行しない。
7. **中断要求を上書き可能なvisual snapshotだけに載せない。** 現publish_visualは最新値で置換し、try_lock不成立時には保存しないため、pulseを保証できない。
8. 中断intentは既存control queueの命令として配送する。新queue・pending bool・retryを作らず、cursor／HUD矩形等の最新値配送は既存経路へ残す。
9. control命令は同じWindowEpochへ適用し、終了したhostの要求を新HWNDへ付け替えない。
10. channelやIMM32のまれな失敗は既存のログ・通知・fault終端へ揃える。再試行や待機を追加しない。

上書き経路の根拠: `src/video/native_window_pump.rs:339`、`:1604`。既存control送信は同`:207`付近。logical batchのintent集約は`src/video/native_presenter/render_core.rs:3772`。

非対話回帰では、実TextEditのfocus／cursor移動から中断flagが出ること、intent変換、controlへの一度だけの配送、WindowEpoch所有、複数logical pass、visual更新を挟んでも消えないことをfake hostで検査する。OS上の取消結果は段7の実機で確認する。

段3では、段2aの生成・集約回帰が正式graphで使われることと、集約済み要求が通常backend／nativeの正しい配送先へ一度だけ渡ることを接続確認する。

**挙動・見た目:** 日本語確定直後の通常操作、取消、兄弟IME独立性を保持する。下線・cursor・候補窓位置は外観差があり得る。nativeでは新APIの中断要求を正しく実行する。

### 4.6 Modifiersイベント化、keymap、clipboard

RawInput.modifiersはなくなり、ModifiersChangedがlevelを更新する。イベントごとのmodifiersは残る。

根拠: `egui-0.37.0/src/data/input/event.rs:94`、`src/input_state/mod.rs:434`、`egui-winit-0.37.0/src/lib.rs:548`。

| 箇所 | 内容 |
|---|---|
| `src/app.rs:59309` | OS resync |
| `src/key_input.rs:2459`、`:2466` | 合成入力 |
| `src/video/native_presenter/render_core.rs:2716`、`:9780`付近、`:12505` | native queue／独立Context |
| `src/ui_fullscreen.rs:67268`、`src/ui_adjustment_panel.rs:10985` | テスト |
| `src/ui_erase.rs:815`、`src/ui_conceal.rs:742`、`:1341` | stale対策 |
| `src/ui_adjustment_panel.rs:12014`、`:12510` | Space／Ctrl編集 |
| `src/keymap.rs:7468`、`src/keyboard_input.rs:305` | chord／consume |

modifier変化を対象イベントより前へ並べる。frame末尾に最終値だけ追加せず、過去edgeのmodifiersをOS resyncで書き換えない。

FSのOS直読み、focused permit、物理ラッチ、左右Ctrl、Enter／NumpadEnterは更新だけで撤去しない。ModifiersChangedはKeyUp欠落を保証しない。

新しい修飾Key・Windows固有Keyを理由にKeyActionの保存名・既定割当を変更しない。

Ctrl+Shift+C/X/Vは通常窓・nativeとも保持する。

- 現vendor: `vendor/egui-winit/src/lib.rs:2314`
- 本家command_only: `egui-winit-0.37.0/src/lib.rs:1538`
- native: `src/video/native_presenter/render_core.rs:9863`

共通の純粋なclipboard条件へ揃え、I/Oは各integrationに残す。Process、IME、release、空clipboard時の二重Text防止を保持する。Ctrl+AltやInsert／Delete系をついでに拡大・縮小しない。

### 4.7 Id、scope、永続化

- Id::newはId::uniqueを呼ぶ非推奨API。
- AsIdは非推奨として残る。
- AsIdSalt／IdSaltが導入され、Id::withのhash構成も変わる。
- Ui::idはscope_id。
- From<&str>を使う移行にはしない。

根拠: `egui-0.37.0/src/id.rs:8`、`:91`、`:102`、`:121`、`:142`、`src/id_salt.rs:43`。

root identityはunique、局所塩はIdSalt。ui.id()を機械的にunique_id()へ変えず、scopeの意味ならscope_idを使う。生成・lookupを同時移行する。

代表: `crates/egui-ime/src/lib.rs:159`、`src/ui_music_panels.rs:706`、`:1563`、`tests/ui_snapshot.rs:443`、`tests/book_composition_editor_snapshots.rs:405`。件数は§3。

切替直後のId非推奨は短期間許容し、段5で整理する。保留しても旧Id互換を保証しない。

現manifestでeframe persistenceは有効でなく、独自egui memory保存も見当たらない。本家保存はfeature内。根拠: `eframe-0.37.0/src/native/epi_integration.rs:470`。

settings.dbは初期化しない。リリース済みId永続経路が実装時に発見された場合だけ、旧読込・変換・互換試験を設計する。独自窓配置とsession memoryを区別する。

### 4.8 vendorパッチ台帳

分類:

- **(a)** 本家同等で旧差分を外せる。
- **(b)** 保証・責務が残り付け直す。
- **(c)** 旧実装は不要。必要機能は新APIへ移す。
- **(b・新規)** 今回判明した移行上の所有・互換修正。

元src差分はeframe 9ファイル・約2270追加／124削除、egui-wgpu 9ファイル・約2004追加／192削除。READMEだけを台帳にしない。

#### egui

| 項目 | 分類・方針 |
|---|---|
| release／click count射影 | (b) `vendor/egui/src/input_state/mod.rs:1485`。順序・非click・focus・discardを保持。本家分類と分離 |
| 旧分類に依存するfixture | (c) 旧count固定を外し、近接・距離境界・時間条件を別検査 |
| discard_pending_scroll | (b) 同`:595`。新WheelStateへ適応 |
| repaint delay witness | (b) `src/context.rs:1794`とfeature |
| IME中断要求のviewport所有 | (b・新規) Memory共通フラグを既存のviewport別Focusへ移管。親子のbegin／endで消失・混入させない |
| IME中断要求のpass出力集約 | (b・新規) 最新のIME矩形等を採用し、同一viewportの未配送要求をORで保持 |
| 入れ子run_ui・実discard回帰 | (b・新規) §4.5の親だけ／子だけ要求と、実run_ui再passの最終出力を検査 |
| 承認済み子viewportの非描画退役入口 | (b・新規) 既存Context viewport GCを共通化し、pass外で対象登録を退役できるeframe向け入口を追加。兄弟やrootを変更しない |
| テスト・license・standalone | (b) 新構造へ移植 |

新wheel ownerは`egui-0.37.0/src/input_state/mod.rs:228`、`wheel_state.rs:54`。汎用非表示終端APIは追加しない。

IME修正の本家境界は`egui-0.37.0/src/memory/mod.rs:124`、`:503`、`:799`、`:1078`、`src/context.rs:1111`、`:3173`、`src/data/output.rs:242`。既存フラグの所有移管と既存appendの集約契約修正に限定し、App側の補助フラグ、追加repaint、新しいqueue、retryを追加しない。

親子要求の分離はdetached構造合意・§11記録の範囲へ含める。出力集約では別viewportのPlatformOutputを混ぜず、配送後の次frameへ要求を残さない。

非描画退役は、Appの既存終了ownerが承認したviewportの寿命終了に限る。通常のGC境界は`egui-0.37.0/src/context.rs:3317`以降。最小化・hideだけでは呼ばず、UI pass強制、全memory reset、退役済みIDを抑止する別ラッチで代用しない。通常GCとの処理範囲共有、兄弟不変、後続出力での非再作成を回帰化する。

第4.5版の追加台帳。段2の非描画退役入口を置き換える受渡し不足の解消であり、追加実装・回帰は段3へ計上する。

| 項目 | 分類・方針 |
|---|---|
| 承認済み退役の型付きcommand | (b・新規) 通常Close要求と分離する。exactな登録寿命と子孫への権限を、既存root command列で搬送する |
| 生存登録の寿命同定 | (b・新規) Context登録・ViewportOutput・backendが同じ不透明な登録寿命を参照する。同じViewportIdの再openを旧出力から区別する。永続Idやsurface resize generationを流用しない |
| GC前保持と最外側への移譲 | (b・新規) 子のGCで承認を失わず、immediate入れ子・discard再passでは最外側runの完了まで既存列へ保持する。Full／LogicOnly双方へ配送する |
| 出力appendの寿命境界 | (b・新規) 同じViewportIdでも異なる登録寿命のbuilder・callback・commandを混ぜない。承認済み退役を後続出力の上書きで失わない |
| 寿命を限定したContext退役 | (b・追加修正) pass外制約は保持し、承認された旧寿命と子孫だけを除く。新寿命・root・兄弟を除去しない |
| pass外command drain・未配送事実の照会 | (b・新規) Close hookからの出力も既存列から処理できる入口を用意する。Remote readinessは同じ未配送出力を照会し、補助状態を持たない |

主な対象は`vendor/egui/src/viewport.rs`、`src/data/output.rs`、`src/context.rs`と、その既存回帰である。現在の境界は`context.rs:1080`のrun集約、`:1174`のLogicOutput、`:3170`のGC共通処理、`:3338`のGC、`:3347`の出力生成、`:3418`の非描画退役入口。

既存`retire_viewport`のpass外制約を緩めない。Appのlogic／uiから即時破棄を可能にする変更にはせず、producerとpass外consumerを分ける。未知の対象を登録し直してから終了する方法、別の終端queue、退役履歴集合は追加しない。

通常GCで既にContext登録が消えている場合も、承認済み出力は残る。backend／IME cleanupの対象を「Contextが今回削除できたID」だけで決めない。退役後のcallback drop等で生じるdeltaの引継ぎも§4.9へ接続する。

第4.6版の追加台帳:

| 項目 | 分類・方針 |
|---|---|
| 実discard再pass前のtexture移譲 | (b・新規) `run_dyn`の再pass確定後、次begin_pass前に集約deltaをmoveする。最終pass・上限到達では通常出力を維持 |
| Context束縛のintegration配送scope | (b・新規) 現在threadの限定した配送入口。RAIIで設置・復元し、別Contextへ配送しない。deltaの第二queueは持たない |
| output plugin観測 | (b・追加回帰) 従来どおり各passで一度観測した後に移譲。再預託・再観測を導入しない |
| 一度配送・direct consumer | (b・追加回帰) localを空にし、最終出力への再混入を防ぐ。direct begin／endは従来の出力所有を維持 |

主な製品差分は`vendor/egui/src/context.rs`と必要な最小の公開re-exportに限定する。`FullOutput`に第二delta領域を加えず、platform／IME／viewport commandの集約契約を変えない。

epaintは追加vendor化しない。TextureManagerのmetadata、retain count、`take_delta`、一般の`TexturesDelta::append`を変更しない。

#### egui-winit

| 項目 | 分類・方針 |
|---|---|
| Process基本機構 | (a) 本家を土台 |
| Hidden／Forwarded、release／focus終端 | (b) 保持 |
| Disabled取消通知のproducer配送 | (b) 旧版の通知保証を0.37変換へ付け直す |
| State→正規化→実TextEdit回帰 | (b・新規) 段2a冒頭で成立 |
| trace・文字秘匿 | (b) 保持 |
| 配送／State網羅回帰 | (b) 新modifier／clipboardへ接続 |
| Windows clipboard互換 | (b) Ctrl+Shiftを保持 |
| 旧300ms補正 | (c) 撤去済み。戻さない |

汎用非表示受付は追加しない。Disabledをkey hold全消去の契機にしない。

#### eframe

| 項目 | 分類・方針 |
|---|---|
| #7905 hidden repaint基本修正 | (a) 本家採用 |
| dirty／painting、surface generation、outer phase | (b) `native/run.rs:45`、`:118`、`:425` |
| Windows最小化中のUI継続 | **(b・新規)** 利用者採択の案B。要求済みFullでは最小化を理由にUIを省略せず、surface描画資格は別判定。要求なしのsleepと既存100 ms下限を維持 |
| 非描画Fullの出力・action完結 | **(b・新規)** platform output／viewport commandsを配送し、deltaは共通完結ownerへ。Cut／Copy／Pasteを描画分岐から分離。明示描画要求・実表示ACKを保持 |
| 非ゼロresize inline provenance | (b) 同`:1282`、`native/wgpu_integration.rs:978` |
| 初回visible commit／Frame getter | (b) `src/epi.rs:736`、`native/epi_integration.rs:305` |
| startup window observer | (b) `src/startup_window_observer.rs:8` |
| max_texture_side統一 | (b) `native/wgpu_integration.rs:735`、`:1235`。同等性回帰後に撤去判断 |
| 未消費RawInputのviewport所有 | **(b・新規)** EpiIntegration共通保留をViewportへ移管。§4.2 |
| child skip時の入力保持 | **(b・新規)** take前判定。wgpu／glow同等経路 |
| 子Closeの非描画受付・消費 | **(b・新規)** 子callbackを待たずAppの既存modal判断・終了ownerへ渡す。拒否も一回の要求消費として扱う |
| 承認済み子の非描画退役 | **(b・新規)** 通常の対象限定cleanupを共通化し、window・surface・入力・Context登録を親Full待ちにせず終了 |
| 非描画・消滅viewport delta | (b) 同`:828`、`:1313`。共通完結ownerへ |
| immediate child screenshot | (b) callbackへ移行。旧Event方式は(c) |
| 通常版immediate計測 | (b) 同`:1206`、`:1396`。段階別時間・delta bytes・遅延ログ |
| test-script witness／capture・focus診断 | (b) callback-local identity |
| 実ウィンドウ試験隔離 | (b) 段0a |
| manifest／Win32 feature／standalone | (b) 新原本基準 |

RawInput修正は保持のviewport所有に限定する。汎用フィルタ・復帰pruneは追加しない。交互実行、child skip→復帰、終端、新規入力、retireの回帰を付ける。

CloseはUI復帰待ちの通常入力と区別する。wgpuのUI省略・Close生成は`eframe-0.37.0/src/native/wgpu_integration.rs:702`、`:716`、`:1031`、glowの同等経路は`src/native/glow_integration.rs:692`、`:708`、`:1061`。両backendで同じ受付・消費・退役契約を実装する。

App hookはbackend borrow／lockの外で呼ぶ。承認・拒否した要求をinfo.eventsや保留snapshotから再配送せず、新しいCloseは受け付ける。rootの終了・tray契約と、mIV非管理Appの従来UI経路を維持する。

対象限定cleanupの本家境界はwgpu `:1330`、glow `:1533`。承認済みCloseを実際のwindow／surface退役とegui登録終了まで接続し、兄弟の入力・寿命・共有deltaを維持する。追加repaintや子UI pass強制で代用しない。

通常版immediate計測はstartup／test-script診断と別責務として保持し、常時I/Oへ拡大しない。

本家pending deltaは`eframe-0.37.0/src/native/wgpu_integration.rs:795`、`:1290`。保持だけを完結と扱わない。

案Bの本家変更点はwgpuのUI判定`:702`と描画分岐`:821`、glowのUI判定`:692`と描画分岐`:860`である。Cut／Copy／Pasteはwgpu `:843`以降、glow `:889`以降から、描画不要actionの共通処理へ分ける。`is_visible`やRawInputのminimized情報は偽装しない。

screenshot／RequestPaintWhileHiddenは従来どおり描画資格へ反映する。描画しないFullでもplatform outputとviewport commandsを処理し、texture deltaは§4.9へ接続する。実presentなしのvisible commit・表示ACKを発行しない。

RedrawRequested期限超過は`eframe-0.37.0/src/native/run.rs:29`、`:233`、`:266`の本家契約を維持する。Windows最小化のUI継続を理由に、LogicOnlyの保留RawInputやchild skip修正を撤去しない。

案Bはscheduler方針の明示的な変更として、App責務分離とともにdetached計画§11へ追記する。既存の11項目の合意を破棄せず、変更箇所と残る期限超過経路について設計担当・独立レビュー担当が整合を確認する。

第4.4版の追加台帳:

| 項目 | 分類・方針 |
|---|---|
| root lifecycleのlogic-only接続 | (b・新規) 本家のlogic直後のClose確定を維持し、Appの非描画終了判断が同じLogicOutputへCancelCloseを返すことを回帰で固定する。root用の第二Close queueや別hookは追加しない |
| immediate子の描画資格・出力完結 | (b・新規) 両backendの`render_immediate_viewport`で子自身のmetadata・明示描画要求を使う。主経路とaction／delta契約を共通化し、親の可視性を流用しない |

root lifecycleの製品判断はApp側の既存ownerへ置き、vendor側でtray設定・保存状態を解釈しない。非描画context境界もApp側の責務であり、vendorに表示抑止flagを追加しない。

immediate修正は既存のscreenshot callback移行、通常版計測、非描画・消滅viewport deltaと同じ入口へ付け直す。別rendererや別pending delta ownerを増やさない。

第4.5版の追加台帳。以下は段3で判明した受渡し不足の修正として正式vendorへ追加し、段2の既存ACCEPTとは別に検証する。

| 項目 | 分類・方針 |
|---|---|
| 承認済み退役の共通consumer | (b・新規) Full／LogicOnly／pass外hookから同じ型付きcommandを受け、次のApp処理より前に対象資源を退役する |
| Close要求と承認済み退役の分離 | (b・追加修正) OS／通常Closeは既存hookへ、承認済みcommandは再承認せずcleanupへ渡す。hook前の対象寿命を保持する |
| hook結果の受渡し | (b・追加修正) mIVの終了ownerが出力へ移譲した結果を表現する。既存Close結果も寿命付き承認へ正規化し、ViewportIdだけの直接破棄を残さない |
| 古い出力の非再作成 | (b・新規) 対象寿命を退役してから通常outputを適用する。Contextの現登録寿命と一致しないbuilder・callbackからwindowを作成・更新しない |
| 通常GCとのcleanup共通化 | (b・追加修正) 既にGC済みの部分を含め、残存native／Context／IME資源を終了する。Close完了hookの到着を前提にしない |
| 終端時のdelta引継ぎ | (b・追加修正) 共有deltaを先に移譲し、Context／callback drop後の追加deltaも共通renderer ownerへ渡す |
| root終了・早期return | (b・追加修正) 受領した承認済みcommandとdeltaの完結を、root終了・surfaceなし・通常GCによって飛ばさない |

対象は`vendor/eframe/src/native/viewport_lifecycle.rs`、`epi_integration.rs`、`wgpu_integration.rs`、`glow_integration.rs`、App hookの型を持つ`src/epi.rs`と関連回帰である。

wgpuの`remove_viewports_not_in`／`handle_viewport_output`、glowの同等処理、両backendの`render_immediate_viewport`からの集約、EpiIntegrationの既存FullOutput保留を含める。immediateの内側でContextを退役せず、最外側の出力consumerへ揃える。

通常GCと承認済み退役は同じ対象資源cleanupを使うが、意味は混ぜない。GCはAppへの終了要求を作らず、承認済みcommandはOS Closeを作らない。非管理viewportの従来UI委譲、root lifecycle、兄弟資源を維持する。

最小化中のUI継続、期限超過LogicOnly、viewport別RawInput、非描画action、screenshot、deltaの既存差分は撤去しない。今回の修正を、追加repaint・強制UI・次FullのGC待ちで成立させない。

第4.6版の追加台帳:

| 項目 | 分類・方針 |
|---|---|
| 両backendのdiscard delta sink | (b・新規) 最外側native FullへContext束縛scopeを設置し、既存共有renderer ownerへ移譲する |
| 先行batchの無描画完結 | (b・追加接続) wgpuの既存upload submissionとglowの既存resource context処理を再利用。UI・surface描画・presentを追加しない |
| pending deltaの非相殺受入 | (b・追加修正) Full／immediate／retirementの受入を既存非相殺helperへ揃え、後発freeで先行setを除去しない |
| scope解除・Context隔離 | (b・追加回帰) 正常終了・早期return・unwind・複数Context・入れ子を検査する |
| 実immediate置換との一体回帰 | (b・追加回帰) fake adapterでの保持だけを証拠にせず、旧callback dropと実resource consumerまで通す |

主な対象は`vendor/eframe/src/native/wgpu_integration.rs`、`glow_integration.rs`、既存の共有出力／lifecycle helperと関連回帰。Appや`EpiIntegration`へrendererの第二ownerを設けず、native側が既存Full呼出しの外側でscopeを管理する。

第4.5版の登録寿命、承認済み退役、Context／IME cleanupの契約は維持する。今回の順序修正のために旧callbackのcleanupを遅らせたり、旧native資源を新寿命へretagしたりしない。

#### egui-wgpu

| 項目 | 分類・方針 |
|---|---|
| managed mipmap／再生成／sampler | (b) `src/renderer.rs:712`、`:777`、`:1151` |
| 公開MipmapGenerator／texel count | (b) `src/mipmap.rs`、`src/mipmap.wgsl`、`src/lib.rs:35` |
| atlas診断／renderer identity／texture_size | (b) 新delta列へ |
| 無描画delta | (b) `src/winit.rs:523`。drain／submit／free |
| 対象surfaceだけの破棄 | (b) 本家は全surface clear |
| startup GPU／root present observer | (b) `src/startup_diagnostics.rs`、`src/lib.rs:322` |
| screenshot／readback診断 | (b) callbackへ |
| shader空行等 | (c) 再適用しない |
| license／test接続 | (b) 新原本基準 |

本家surface差: `egui-wgpu-0.37.0/src/winit.rs:203`、`:218`、`:229`、`:247`。

既存atlas overflow skip-writeは根治策として拡張しない。診断を保ち、配送回帰でoverflowが生じないことを受け入れる。既存skip撤去は履歴を含め別判断とする。

第4.6版では、discard先行移譲に新しいrendererやdelta queueを追加しない。既存`Painter::apply_textures_delta`とupload submission／freeの契約を再利用する。epaintのAPI変更も不要とする。

必要な追加は、whole＋partialの実適用・submission・後発freeを証明するheadless／offscreen回帰と、必要最小限のtest用観測である。既存consumerで成立する場合、製品側のegui-wgpu差分は増やさない。CPU側のdelta保持・件数・texture sizeだけで、GPUへの配送順を証明したとは扱わない。

#### 取り込み手順

1. 新原本・checksum・VCS・licenseを記録。
2. 論理項目ごとに差分を付け直す。
3. 旧manifestをコピーせず新manifestを基準にする。
4. root／standaloneの実体を統一。
5. PATCHES／READMEを実差分と一致させる。
6. `.cargo-ok`、targetを取り込まない。
7. 他DLL・モデル・SDKを巻き込まず、reparse pointを作らない。

### 4.9 wgpu、texture delta、native presenter

#### 現行経路・Windows依存

実コードに直接hal／windows_058利用は見当たらず、旧共有経路コメントが残る。現nativeはD3D11 OpenSharedResource1を使う。

- `src/video/native_presenter/render_core.rs:8347`、`:8379`: shared resource
- 同`:6708`: 動画Present
- `src/video/native_presenter/overlay_gpu.rs:43`: overlay
- `src/app.rs:17366`付近: 旧説明

未使用直接依存とコメント訂正は旧版上へ前倒しする。未使用windows_062別名を追加しない。hal30のWindows依存は0.62系。根拠: `wgpu-hal-30.0.1/Cargo.toml:518`。D3D11本体のwindows 0.61は全面更新しない。

#### API影響

| 箇所 | 内容 |
|---|---|
| `Cargo.toml:368` | wgpu30 |
| `src/lib.rs:495`、`:526` | surface |
| `overlay_gpu.rs:31` | Instance／DeviceEpoch |
| `render_core.rs:12089` | configure |
| 同`:13840`、`:13957` | delta |
| 同`:13891`以降 | render pass／submit／present |
| `src/compare_wgpu.rs:274`、`:539` | pipeline／callback |
| `src/panorama_wgpu.rs:332`、`:669`、`:506`、`:833` | pipeline／mipmap |
| `src/gpu_lanczos.rs:680`、`:708` | 縮小 |
| `build.rs:140`、`:230`、`Cargo.toml:495` | Naga27→HLSL→DXBC |

本家の必須差分:

- Instance値渡し: `wgpu-30.0.1/src/api/instance.rs:63`
- BGL Option配列／immediate_size: `pipeline_layout.rs:38`、`:44`
- multiview_mask: `render_pipeline.rs:238`、`render_pass.rs:682`
- color space／CurrentSurfaceTexture: `surface.rs:90`、`:148`
- Queue::present: `queue.rs:377`
- WgpuConfiguration.surface: `egui-wgpu-0.37.0/src/lib.rs:325`

configure write lock、submit／present read lock、Surface／DeviceEpoch寿命を維持する。新presentをgate外へ出さない。surface状態だけでdevice-lost latchやretryを増やさない。

#### TexturesDeltaの所有・消費

新版setはIDごとのdelta列、freeはsetで、非空dropはdebug assertionになる。

根拠: `epaint-0.37.0/src/textures.rs:284`、`:315`、`:335`、`egui-0.37.0/src/data/output.rs:72`。

consumerはmut参照または所有権を受け、借用適用しただけの非空出力を返さない。診断cloneに適用権を持たせない。

| 経路 | 完結責任 |
|---|---|
| 通常eframe | 既存pending ownerへ移譲。set drain→描画submit→free drain |
| native overlay | render batchがset／freeを消費 |
| surfaceなし・無描画 | rendererでset適用・upload submit→無描画確定後free |
| renderer未準備・早期終了 | 既存pending／logical batch ownerへ残りを引継ぐ |
| acquire不成立 | uploadをsubmitし、未処理freeを完結または引継ぐ |
| viewport消滅 | 共有renderer存続を考慮しdeltaを同時廃棄しない |
| renderer／Context終了 | 配送先不在をownerが確定し、対象単位で明示終端 |
| 描画不要headless | 検査後drop_without_applying_deltas |
| upload／描画test | 実consumerで適用・drain |

whole→partial、同一ID内の順序を維持する。freeは当該使用のsubmit後。早期return前に完結ownerへ移す。

製品の無条件clearは禁止する。headless APIで出力を捨てて同じ製品Contextを継続しない。

例: `src/ime_focus.rs:795`。testもoutput検査後に明示破棄する。

第4.5版の承認済み退役では、windowとContext登録の退役より先に、既に生成されたFull／immediate出力のdeltaを共有renderer ownerへ移譲する。古いViewportOutputを適用対象から除いても、そのFullOutputの共有deltaを一緒に破棄しない。

Contextの登録・callback・TextureHandleをdropした結果、texture managerに追加deltaが生じる場合も同じownerへ引き継ぐ。通常GCが先に行われた場合の追加分も含め、回収に次のUI passや追加repaintを必要としない。生存する兄弟と共有するtexture managerをclearしない。

wgpu／glowとも、surfaceなしの退役では既存の無描画submission契約を使う。root終了を伴う場合も、子の承認済みcommandと追加deltaを既存のrenderer終了ownerへ渡してから終了する。正常な退役をheadless用の明示破棄で代用しない。

#### 第4.6版: discard再pass前のdelta配送

**確認した不足**

`Context::run_dyn`は各passの出力をローカルFullOutputへ集約し、discard時もそのまま次passへ進む。旧passのdeltaがこのlocalにある間、次passのimmediate処理は別の実consumerからdeltaを配送できる。

本家の境界は`egui-0.37.0/src/context.rs:1080`、`:1113`、`:1124`。output pluginは同`:2949`、texture managerからのtakeは同`:3169`。

WIPでは、同じViewportIdの新寿命を準備する際に旧native callbackを破棄し、そのTextureHandle由来deltaをmanagerから共有rendererへ渡す。`vendor/eframe/src/native/wgpu_integration.rs:2035`付近とglowの`replace_stale_glow_viewport`が該当する。旧set／partialが外側localへ残ると、後発freeが先に処理され得る。

これはコードから確認した所有順の不足であり、製品実行時の再現を確認したとは扱わない。

**候補の比較と採用**

| 候補 | 必要な変更・契約 | 判断 |
|---|---|---|
| (a) 実再pass直前のintegration配送 | eguiの限定した配送入口と、両backendから既存共有renderer ownerへの接続。旧batchを後発freeより先に処理でき、output pluginの観測を繰り返さない | 採用 |
| (b) TextureManagerへの再預託 | epaintのmetadata非変更prepend API、旧local→新manager deltaの非相殺合流、pluginが同じ未配送分を再観測する契約 | 不採用。callbackを省けても変更する所有・観測境界が多い |
| callback破棄を遅延 | old callbackを外側run終了まで存続させる別の寿命管理 | 不採用。cleanupを遅らせ、同ID新寿命との組合せを増やす |
| setを失ったfreeを無視する等 | consumer側の後付けguard | 不採用。配送順の不成立を隠す |

epaintの`TextureManager.delta`はprivateで、取り出しは`take_delta`である。`set`／`free`の再演はmetadata・retain count等も変更するため、再預託APIの代用品にならない。さらに`TexturesDelta::append`は新しいfreeに対応する旧setを除去する。

根拠: `epaint-0.37.0/src/textures.rs:13`、`:44`、`:67`、`:102`、`:315`、`:319`。今回はこれらのAPI・最適化・metadata契約を変更せず、epaintの新vendorも追加しない。

**発火する位置と移譲するもの**

`run_dyn`で、次の条件を全て満たした直後、次の`begin_pass`より前に配送する。

1. 現passの`end_pass`と全output pluginが完了している。
2. 既存どおりdiscard要求がある。
3. `max_passes`に達しておらず、実際に次passを実行する。
4. 対応するintegration配送scopeが設定されている。

配送するのは、集約local FullOutputが所有する未配送`textures_delta`だけとする。所有権をmoveし、local側は空にする。診断cloneやmut借用だけで配送したことにしない。

platform output、shapes、IME出力、viewport commands、承認済み退役は移譲しない。それらは従来の集約と第4.5版の最外側配送を維持する。callback内でApp、UI、plugin、viewport cleanupを呼ばない。

これは既存discardの受渡しを修正するもので、discard回数・再試行条件・repaintを追加する変更ではない。

**年代順とconsumer**

backend側のconsumerは、既存の共有renderer ownerを使い、次を満たす。

1. 共有ownerに既にある、さらに古い未処理batchを先に処理する。
2. 移譲された旧passのbatchを受け、set列を順に適用する。
3. wgpuではupload／mipmap等のsubmissionを完結し、そのbatchのfreeを適用する。surfaceの取得・描画・presentは要求しない。
4. その後に次passへ戻る。次passの旧callback破棄で生じたfreeは、既存manager→renderer経路から後で処理する。
5. 最終FullOutputには移譲済みset／partialを残さず、外側で二重適用しない。

これにより、対象の旧whole／partial→upload submit→後発freeの順序を保つ。既に生成済みのmanager内freeを、callbackが旧localより先にtakeすることもしない。

glowでは対応する順序でtexture uploadとfreeを行う。既存resource context選択を使い、描画・swapを増やさない。wgpuと同じGPU submission APIがあるとは扱わない。

rendererやGL resource contextが一時的に利用できない既存経路では、所有権を既存共有pending ownerへ移す。cleanupや次passを待たせる新しい状態は作らない。その後の合流でも旧set／partialと新freeを相殺せず、既存consumerが利用可能になった時にset適用→submission→freeを完結する。

このため、backend共有pendingへのFull／immediate／retirement由来の受入は、WIPの`append_retirement_delta`と同じ非相殺の受入契約へ統一する。新しいfreeを受けたことだけで先行setを除去しない。現在のhelperを共通用途へ整理し、第二pending ownerを作らない。epaint一般の`TexturesDelta::append`は変更しない。

無描画の既存consumerを再利用し、空batchのためだけのsubmission、GPU待機、新surface、強制UIは追加しない。実discardで非空deltaがある場合には、uploadのsubmissionが通常の最終Fullより早まる。

**callbackの設置・解除・Context一致**

配送入口はApp pluginではなくintegration専用の、UI実行thread上のscope付き入口とする。

- eframeが最外側のnative Fullを実行する前に、対象Contextと既存renderer ownerに結び付けて設置する。
- scopeはそのFullの実行・出力処理を包含し、終了時にRAIIで解除・元の登録を復元する。早期returnやunwindでも他のContextへ登録を残さない。
- immediateの入れ子は同じContextのscopeを利用する。子ごとのpending領域・callback ownerを作らない。
- Contextは内部Arcの同一性で照合する。root ViewportId、threadだけ、現在foregroundでは照合しない。
- 既存native資源が`Rc<RefCell<_>>`であるため、`Send + Sync`を要求するContext内callbackへ無理に格納しない。現在threadのscope付きdispatcherを使い、backendへの参照はWeakとする。dispatcherはdeltaを保留するqueueではない。
- dispatcherの登録borrowを解放してからconsumerを呼ぶ。Context／TextureManager／backendのlock・borrowを保持したまま入らない。
- native Fullのscope中は配送先が存続することを成立条件とする。別Contextや失効したbackendへ誤配送しない。接続不成立をdeltaのclearや黙った成功にしない。
- headless等でscopeがないContextは従来どおりFullOutputで所有権を返す。eframeのnative immediate経路では、対応するscopeがあることを入口の不変条件にする。

新しいTLS登録は、既存の即時viewport rendererと同じthreadで動くが、その「最後に設定されたcallback」を無条件に流用しない。Contextを束縛したscopeと復元規則を設け、複数Contextのテスト間へ漏らさない。

**最終pass・上限・direct begin／end**

| 経路 | 契約 |
|---|---|
| discard後に実再passを行う | 次のbegin_pass前に未配送deltaを先行移譲する |
| discardなしの最終pass | 新callbackを呼ばず、通常FullOutputで返す |
| max_passes到達で再passしない | 新callbackを呼ばず、通常FullOutputで返す |
| 再passのdeltaが空 | 配送・submissionを増やさない |
| direct begin_pass／end_pass | 新callbackを自動実行しない。consumerは返されたoutputを受け入れてから、次の処理・native寿命置換へ進む |
| LogicOnly | UI passもdiscardもないため発火しない。既存退役delta consumerを維持する |
| immediate内のdiscard | 同じscopeへ先行移譲し、外側へ二重配送しない |

direct consumerが出力を保持したまま次のnative処理を進める場合は、そのconsumer側の所有契約違反として直す。direct begin／endを暗黙のretry経路へ変えない。

**output pluginとテスト隔離**

output pluginの呼出回数・順序は変更しない。各passで従来どおりoutputを観測・変更した後、その結果のdeltaを移譲する。移譲のためにpluginを再呼出ししない。移譲済みの旧deltaをTextureManagerへ戻して次passで再観測させない。

製品の`ImeInputPlugin`とpage-turnの`SubmissionPlugin`はそれぞれ`crates/egui-ime/src/lib.rs:111`、`src/ui_fullscreen/page_turn.rs:547`。IME・discard・page-turn receiptの既存観測を維持し、textureの先行移譲を実presentやpage-turn表示ACKとして扱わない。

テストではscope guardを使い、Context AのsinkへContext Bのdeltaが入らないこと、入れ子scope終了後の復元、unwind後の解除、sinkなしContextへの非漏出を検査する。全testで共有するstatic受信queueは使わない。

本家へ報告する価値がある。未改変本家で成立する最小discard／callback-drop再現を切り分ける対象とし、本家での再現確認済みとは扱わない。報告の送信は利用者判断とし、こちらからは送らない。

#### 無描画submission

本家は早期returnにもqueue submission guardを持つ。根拠: `egui-wgpu-0.37.0/src/winit.rs:502`、`:514`。

共通処理で次を満たす。

1. rendererがあればsurface lookupより前にset適用・drain。
2. mipmap commandを含めsubmit。
3. queue writeだけでもsubmissionを完結。GPU待機は追加しない。
4. 通常submit済みなら重複空submitを足さない。
5. freeはsubmit後にdrain。
6. acquire／capture途中終了も同じownerで完結または引継ぐ。
7. 引継ぎ時にsetを二重適用しない。

本家freeはsubmit後。根拠: `egui-wgpu-0.37.0/src/winit.rs:768`以降。

headless GPUでno-surface、acquire不成立相当、whole＋partial、mipmap、free-onlyを通し、submission・readback・解放順を検査する。サイズ更新だけを証拠にしない。

#### 表示・性能

AutoVsync／latency 1の明示設定と環境変数を新SurfaceConfigへ移す。stencil対応だけを理由にbufferを追加しない。callback format／sample数を揃える。

RGBA atlasへ診断を合わせる。根拠: `epaint-0.37.0/src/image.rs:16`。

D3D11のNaga27→HLSL→DXBCは別経路であり、wgpu更新だけで同時更新しない。

**挙動・見た目:** 動画本体方式、アップスケール方式、VSync方針を維持。HUD文字・透明合成・frame latency・mipmapを段4cと実機で確認する。IME出力は§4.5を満たす。

### 4.10 egui_kittest、accessibility、snapshot

visible inputのaccessible nameを検査し、read-onlyを除外する。label／placeholderを認識する。

根拠: `egui_kittest-0.37.0/src/accessibility.rs:14`、`:26`、`:46`、`src/builder.rs:151`。

検査はsettled後・画像生成前なので、accessibility修正を候補画像収集より先に行う。

根拠: `egui_kittest-0.37.0/src/lib.rs:518`、`src/snapshot.rs:739`。

影響:

- `tests/ui_snapshot.rs:28`
- `tests/book_composition_editor_snapshots.rs:132`、`:247`
- `tests/support/ui_harness.rs:14`
- `tests/support/scroll_layout.rs:6`
- libのPNG生成fixture
- `src/ime_focus.rs:1268`等のraw TextEdit allowlist

kittest 0.3→0.5。NodeT／accesskit_nodeは残るが、名前・scope変更でquery先が変わり得る。根拠: `kittest-0.5.0/src/node.rs:9`、`egui_kittest-0.37.0/src/node.rs:21`。

名前は本体描画側へ付け、旧版で前倒しする。builder検査を無効化せず、role／親containerでqueryを限定する。最終pass geometry、DX12|VULKAN、lazy、CPU adapter優先、renderer独立を維持する。

順序:

1. accessibility修正。
2. baseline未更新で両integration＋libの通常比較。
3. 文字ラスタ、文字幅・行高、panel・margin、操作回帰へ分類。
4. 入力・geometry回帰を修正し外観採択。
5. 同じ条件で全対象更新。
6. 更新なしで全対象再比較。
7. 画面群の共通理由・対象集合・例外・確認者・結果を一台帳へ。

445枚を対象漏れ基準にする。contact sheetを索引に、欠け・位置・例外を原寸確認する。PNGごとの重複票や固定レビュー票は要求しない。

lib対象は実PNG生成呼出から列挙し、`--lib snapshot`だけで全件と見なさない。

### 4.11 その他の挙動と複数viewport

| 変更 | 方針 |
|---|---|
| popup sizing pass | 計測passで保存・選択commandを発行しない |
| release即閉じ防止 | 右click、外側終了、wheel終端を検査 |
| menu hover-open | 選択・wheel所有を維持 |
| ScrollArea端fade | 旧外観維持を初期案 |
| Window margin／min size | 小画面の本文・固定操作を再計測 |
| tooltip／container | 入力owner・query混入を検査 |
| repaint-after削減 | frame頻度依存を直し、追加repaintで戻さない |
| rootのみ保存／復元 | 独自配置との二重所有を避ける |
| raw mouse motion | unfocused受信をkeyboard ownerの根拠にしない |
| Screenshot callback | 既存batch／worker I/Oへ |
| set_fonts即時適用 | atlas／galley失効を確認 |

根拠: `egui-0.37.0/src/containers/popup.rs:629`、`:660`、`menu.rs:440`、`scroll_area.rs:1704`、`window.rs:633`、`:696`、`context.rs:133`、`:2505`、`:4557`、`eframe-0.37.0/src/native/wgpu_integration.rs:511`、`:903`。

mIV代表は§4.3のpanel、§4.4の入力、`src/app.rs:97661`以降の完了処理、`src/ui_fullscreen.rs:22139`以降のviewport。captureは`src/test_script/capture.rs:282`、`:375`。callbackでPNG encode／I/Oをしない。

#### detached凍結

App分離、callback Ui化、gamepad、保留RawInputのviewport所有、IME中断要求のviewport別所有と入れ子pass、同一viewportのdiscard再pass出力集約、native IME intent、非表示子Closeの受付・消費・既存終了ownerへの合流・非描画退役、scheduler／delta／surfaceは構造合意対象とする。

着手前に設計担当と独立レビュー担当が症状パッチではないことに合意し、`docs/detached-rework-plan.md` §11へ範囲・理由を記録する。本改訂案自体を合意済みと扱わない。

IME追加分では、親子の要求が消失・混入しないこと、最新の候補窓矩形等を採りながら未配送要求を保持すること、配送後に要求を残さないことを不変条件として記録する。

Close追加分では、対象window／contextの既存modal判断と終了処理を一つのownerに揃えること、承認・拒否した一回の要求を再配送しないこと、子や親のUI復帰を待たず承認済み対象を退役させること、兄弟の入力・資源・寿命を変更しないことを記録する。Context側の非描画退役入口とwgpu／glowのcleanup共通化も合意範囲へ含める。

rect捕捉条件、geometry由来recreate、新detached pending bool、時間窓、兄弟資源の一括resetを追加しない。

第4.3版では、利用者採択の案Bについて、§11のApp責務分離・scheduler項目へ次を追記する前提とする。§11本文は設計担当が更新する。

- Windows最小化中も要求済みFullをUIとして消化し、UI実行とsurface描画を分ける。不可視metadataと実表示ACKを偽装しない。
- 要求なしのsleep、既存repaint下限、outer phase、surface所有を保持する。追加repaintやAppからの強制UI呼出しを使わない。
- RedrawRequested期限超過のLogicOnlyと、root LogicOnly→child Full→root Full、child skipの経路は残る。
- C1・送信済み保存・取消・寿命終端はlogic、C2・C4・C5とC3の表示用pollは既存registry／mount ownerのuiへ残す。
- Closeの非描画受付・消費・退役、viewport別RawInput、IME、deltaの既存構造合意は維持する。

これは合意済み11項目の適用範囲を具体化し、scheduler方針の変更を記録するものである。利用者の案B採択と、設計担当・独立レビュー担当の構造整合確認を区別し、本追補だけで追加範囲の独立レビュー完了を主張しない。

第4.3版レビューで、案Bとscheduler方針変更が症状パッチではなく構造修正であるとの合意を得た。本版はその合意を維持する。

§11へは、追加でroot lifecycleの非描画受付・既存延期ownerへの合流、owner付き状態更新と非描画mount／UI mountの境界、immediate子自身の描画資格を追記する。§11本文は設計担当が更新する。

状態mountで兄弟の表示を再採用しないことと、次の正当なUI境界では従来どおり再採用することを、両方の不変条件として記録する。既存のrating publicationを後付けguardで抑える変更にはしない。追加3件の実装前レビューと、案B自体について成立した合意を区別する。

第4.5版では、段3で判明したApp終了ownerとbackend資源ownerの受渡し不足について、次をdetached計画§11へ追記する前提とする。§11本文は設計担当が更新する。

- 候補(A)のApp Closing／binding延命を採らず、候補(B)の承認済み退役出力への一度の権限移譲を採る理由。
- exactな対象登録寿命と子孫の範囲、同じViewportIdの新寿命と兄弟を変更しない条件。
- 既存root command列でのGC前保持、immediate入れ子・discard再pass・Full／LogicOnly・pass外hookの配送境界。
- App runtime終了後に再同定・再承認せず、pass外consumerがnative／Context／IME資源を終了すること。
- 古い出力による非再作成、通常GCとのcleanup共有、共有deltaとdrop後の追加deltaの引継ぎ。
- mode change、active／parked／passive終了、Remote取得、watcher、barの×、通常OS Closeと非管理viewportの扱い。
- RemoteのFinishAcquireが、App登録解除だけを根拠にnative退役より先行しないこと。
- root lifecycle、最小化中のUI継続、期限超過LogicOnly、native presenterの既存終了ownerを維持すること。

これは、空のUI呼出し、GC待ち、終了後のViewportId再検索という不成立な受渡しを、既存出力ownerへの移譲に置き換える構造修正である。guard・時間窓・retry・追加repaintによる症状パッチではないことについて、設計担当と独立レビュー担当の追加合意を得る。

第4.4版と段0〜段2の既存ACCEPTを取り消す変更ではないが、そのACCEPTを今回追加分の承認として流用しない。限定レビューによる候補(B)推奨も、今回の具体的契約の独立ACCEPTとは区別する。

第4.6版では、次の2件をdetached計画§11へ追記する前提とする。§11本文は設計担当が更新し、実装再開前に追加範囲の構造合意を得る。

1. **終了経路:** 資源退役と一覧帰還を分け、破棄前にbookmark／navigationの所有値を存続mainへ移譲する。既存帰還boolの型付き化、active／standalone／parkedの対応、Remote・新navigation・新bookmarkによる失効、既存UI消費位置の維持を記録する。
2. **delta所有:** 実discard再pass前のintegration配送scope、Context一致、既存共有rendererへの一度の移譲、旧set／partial→submission→後発free、plugin観測不変、両backendの非相殺受入とtest隔離を記録する。

第4.5版の型付き退役・pass外cleanup・旧出力による非再作成を変更するものではない。表示継続のためにruntimeを延命せず、delta順のためにcleanupを遅らせないことを明記する。

既存設計ACCEPTと、今回の追加設計・追加実装のACCEPTを区別する。新callbackの所有受渡しと帰還要求の失効は、重要な非同期・複数context境界として独立レビュー対象にする。

### 4.12 Rust 1.99警告とMSRV

既存ログはf32 fallback 445件／36ファイル、lib全体688警告。今回の実行結果ではない。

代表: `src/ui_main.rs:184`、`src/ui_text.rs:519`、ui_adjustment_panel、ui_fullscreen、native overlay_draw。

| 対象 | 方針 |
|---|---|
| Stroke／PathStroke | 新版のf32型で解消分を再集計 |
| その他float | 必要境界へ局所型注釈 |
| vendor警告 | 新原本で再評価 |
| fetch_update | egui更新では解消しない。MSRV互換を確認 |
| dead code | feature／test／Windowsを区別し、機能削除で減らさない |
| IME互換通知の非推奨 | 必要箇所に理由付きで限定許容。等価取消契約なしに削除しない |

根拠: `epaint-0.33.3/src/stroke.rs:27`、`epaint-0.37.0/src/stroke.rs:26`、`:140`。

Atomic代表: `src/log_writer.rs:105`、`:165`、`src/everything_ipc.rs:226`、`src/ime_focus.rs:225`、`src/similar_index.rs:1431`、vendor witness。

egui／eframe 0.37のMSRVは1.95。根拠: 各`Cargo.toml:14`。workspace 1.88を更新し、開発toolchain 1.99固定と分けて1.95で検証する。

try_updateの最小安定版を未確認のまま採用しない。全crateの警告抑制を行わず、残りは段5で整理する。

### 4.13 Remote／他crate

| 対象 | 波及 |
|---|---|
| egui-ime | 入力・focus・取消・testを移行 |
| 4ラボ／vst3-host-tester | App／Ui／Panel／RawInputを同時切替 |
| remote-web／remote-ipc | egui直接移行・protocol変更不要。非表示応答を確認 |
| launcher | egui非依存。MSRV・埋め込みcore更新 |
| epub-pdf-worker | egui移行なし。companion build |
| comic-core | 独立ラスタライザ維持 |
| local-adjust-core／music-core | egui非依存維持 |
| Susie／LibRaw／ORT／VST bridge | ABI更新を含めない |

root・各crate・ラボmanifestを棚卸しし、旧新版eguiの型を分裂させない。

## 5. 利用者に相談する事項

0.37を目標にする決定は受領済み。以下の外観・操作差は比較可能な結果を用意して確認する。

| 項目 | 推奨 |
|---|---|
| 字面／baseline／hinting | fallback・保存値を保持して比較整理 |
| color emoji | 代表比較を提示 |
| Window本文drag | 必要箇所で従来操作保持 |
| mouse drag scroll | 旧版で使えた画面は保持 |
| Panel collapse／double-click | 既存操作維持を初期案 |
| **一般の連続クリック分類** | **本家0.37の距離6 points未満・新時間条件の採用を推奨。利用者確認前に操作差を確定しない** |
| 端fade | 旧外観維持を初期案 |
| margin／min size | 到達性を保証した比較案 |
| system fallback | 既存明示chain基準 |
| Ctrl+Shift+C/X/V | 通常／nativeとも保持 |
| 受信済み入力・release確定 | 保持。取消仕様へ変えない |
| gamepad保持軸の復帰後継続 | 現行どおり。中立通過を要求しない |

連続クリックでは、同じ広いwidget上でも位置が6 points以上離れる操作や、旧triple条件だけ成立する操作が変わる。音量・速度reset、reader、resize reset、文字選択の代表例を示す。旧判定や別タイマーによる互換化は推奨しない。採用不可の場合は実装前に範囲を再設計する。

受信済みキー取消、release確定抑止、gamepad軸への中立要求は、具体的な操作差を示して相談する。本版は必須化しない。

clipboard保持や保留所有修正のために版選択を再確認しない。Ctrl+Q追加、OS直読み撤去、動画方式変更、パネル本文省略は自動採用しない。

**2026-10-16、利用者は案Bを採択した。** Windows最小化中も要求済みFullでUI処理を続けるため、現行と同様にそのCPU負荷が残る（更新後の実負荷の同値は未測定）。
案Aの「最小化中の表示継続をUI復帰まで保留」は採用せず、相談事項から外す。期限超過中の表示用pollは次の対象Fullまで保留する。

## 6. 段階設計

### 6.1 共通条件

各段で通常製品ビルドを通し、自動検証・残る検証を記録してコミット可能にする。旧baseline不一致を成功と記録しない。

依存切替の型結合部分は原子的に扱う。レビューは領域分割し、固定票を要求しない。原子範囲が大きすぎる場合は旧版上の挙動不変抽出を追加する。

規模は原本、lockfile、PNG、純移動を分けた推定。ファイル数は段間重複を含む。

反復はtest-focusedまたは明示filter、無filter main libはtest-lib-full、最終gateは実ウィンドウを分離したtest-fullを使う。

### 段0a: 実ウィンドウ試験を自動gateから分離

最初に実施する。未修正のtest-fullを非対話gateとして先に実行しない。

`scripts/test-full.ps1:85`は、子processでrun_nativeを呼ぶ非ignored試験を含む。根拠: `vendor/eframe/src/native/run.rs:782`、`:809`、`:828`。

live試験は削除・条件緩和せず、ignored等で既定対象から分離し、承認付き入口を用意する。pure scheduler試験は自動継続する。

AGENTS.mdに従い、起動・foreground切替・入力の前に具体suite、時間、desktop使用、使い捨てdataを示して明示承認を得る。test名・短時間・子processは例外でない。

エージェントは正規portable-smoke成果物のみ起動する。standalone vendor窓試験を残す場合は利用者実行の隔離試験とする。

**受入:** core check、vendor test compile、pure scheduler、runnerの静的／fake検査。窓は起動しない。

**規模:** 3～6ファイル、80～220行。

### 段0b: 基準固定

source、toolchain、feature、fixture、警告、PNG、既知失敗を記録する。detached構造合意にRawInput所有、IME中断要求のviewport別所有・入れ子pass・discard再pass集約、native IME配送、非表示子Closeの受付・消費・既存終了ownerへの合流・非描画退役を含める。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo fmt --check
```

有効な旧版検証台帳を再利用する。既知失敗を上限緩和で隠さない。snapshot生成箇所とlib filter集合を列挙する。

実施記録: [旧版基準台帳](egui-0.37-baseline.md)と[detached §11下書き（合意待ち）](detached-rework-plan.md#2026-10-16-egui-037-更新に伴う構造修正-合意待ち)（2026-10-16）。

**規模:** 文書・台帳3～5ファイル、100～250行。

### 段1a: 0.33.3上の小さい準備

未使用wgpu-hal／windows_058除去、旧コメント訂正、accessibility名付与を行う。可視文字追加を伴うものは分ける。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo fmt --check
python scripts/check_ui_glyphs.py
```

焦点試験・旧PNG通常比較。名前だけなら画像不変を確認する。適切な区切りでtest-lib-fullを行う。

実施記録: [段1aの検証台帳](egui-0.37-stage1a-verification.md)（2026-10-16）。

**規模:** 12～30ファイル、180～500行。

### 段1b: 0.33.3上の挙動不変抽出

- service／input／render／post-renderを旧順序のまま抽出。
- gamepadの物理owner、未配送edge・意味終端の引継ぎowner、UI batchを整理。
- foreground判定、wheel投影、clipboard、output完結入口を抽出。
- native IME出力変換・intent配送の既存入口を整理。
- Win32 queue・mouse-nav pendingのdrain条件を維持。

新しい入力除外・IME中断動作はまだ導入しない。

```powershell
cargo test -p mimageviewer --lib tray
cargo test -p mimageviewer --lib startup
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib sidecar_restore
cargo test -p mimageviewer --lib gamepad
.\scripts\test-lib-full.ps1
```

core check、旧snapshot、focus喪失後のrouted edge、gamepad終端を含める。

当初見積りは14～30ファイル、700～1500行。第4.3版では完了済み1b-1の純移動と、残るowner境界の整理を分けて再見積りする。service、gamepad、描画出力を分けてコミットする。

#### 段1b-1: 旧順序の責務抽出 — 証拠を再利用

実装・独立レビュー後の監査追従と検証は[段1b検証台帳](egui-0.37-stage1b-verification.md)を正とする。成功記録を全logic-readyの証明へ読み替えない。

C1〜C5・D3、F13の表示継続、G1〜G5のframe依存、L1のcallback fact留保を、次段の接続条件として維持する。今回の追補を理由に完了済み検証を繰り返さない。

#### 段1b-2: gamepad owner／batch — 並行実装の範囲を維持

物理drain、押下／軸／disconnect更新、受付事実、未配送edge・意味終端、UI batchの既存ownerを整理する。案Bでも期限超過LogicOnlyは残るため、物理更新とUI配送の分離は縮小しない。

0.33.3では旧呼出位置・受付条件・保持軸継続を維持する。最小化対応の製品接続を前倒しせず、C1〜C5やbackend変更を混ぜない。本追補は、進行中の実装へ別の所有モデルを追加する指示ではない。

**受入:** 通常／test-scriptのcore check、gamepad焦点試験、focus／tray／keyboard回帰。同じframe番号の複数取込み、press→release引継ぎ、disconnect／ring／Undo、保持軸復帰を検査する。完了・検証結果は実装担当の台帳へ記録し、ここで先取りしない。

**規模:** 4～8ファイル、250～500行。純移動を除く。実装実績で再評価する。

#### 段1b-3a: C1 Remoteと共有完了の境界

Remoteのサービス進行とPC表示継続を分ける。0.33.3 driverでは旧位置で両方を従来順に呼び、最小化時の表示延期を導入しない。

- 取得・全context終了・AI／保存barrier、要求受信、headless動画、read／write／bookmark／shift応答、取消・drainを非描画serviceとして抽出する。
- 共有応答receiverは既存routerの単一drainを維持する。
- PC表示は既存Remote表示継続slotをLocal返却後の一覧継続へ置換し、ui側へ残す。旧fullscreen復元用のwait_frames、第二queue、新pending boolは持ち込まない。
- 返却sequenceと表示継続の登録を一度だけ採用し、再取得で古いLocal表示を失効させる。
- App終了drainは表示継続の消費を待たない。

**受入:** 通常core check、Remote／session／reading write／composition editor焦点試験。表示継続を呼ばずserviceだけを複数回呼び、取得・保存ACK・取消・drainが進むことをfake session／workerで検査する。返却→再取得→遅いUIで旧Local表示が出ないこと、通常driverの旧順序も検査する。

**規模:** 4～8ファイル、350～650行。純移動を除く。

#### 段1b-3b: 保存・取消終端とD3

C2・C4・C5とC3のlive表示用pollはuiに残す分類を固定し、通常最小化用の細分化を行わない。C4の進捗だけをlogicへ移す追加分割も必須としない。

- compositionの送信済みsave ticket収穫と、editor表示準備・Reloading・publication／permissionを分ける。receipt→publicationの順序とmodal gateを維持する。
- Remote取得・Closeで破棄された要求の取消・global終端は、既存ownerから非描画で進められるようにする。
- F13のcache Clearedからmount・rating採用・再openへ進む表示継続を、関数名だけでlogicへ移さない。必要な共有ACK・取消終端と表示採用を区別する。
- watcher／周期保存と、走査結果採用・hydration・cutoff ACKを分ける。
- 通常／IndexOnly hydrationのIDを既存ownerの単一採番へ集約する。
- G1〜G3の表示準備・upload予算と、G4のUI分割準備・Cancel／ACK、G5のactivation入力資格は既存UI側へ残す。新しいservice counterへ一括置換しない。
- root Closeの受付・トレイ置換・保存待ち・settings-family・ZIP取消・終了継続を、通常入力から独立したlifecycle入口へ整理する。
- 保存receiptとeditor終了はmount不要のowner付き更新を優先する。残るserviceの一時mountは、payload交換とUI再採用・reconciliationを分離する。

0.33.3では旧driverの呼出順・Closeの優先規則・modal frameの保護を維持する。0.37のlogic-only接続は段3で行う。

**受入:**

- startup／activation／collection／bookmark／sidecar_restore／compositionの既存回帰を維持する。
- 保存receiptをpublicationより先に採用し、Remote取得時は未保存draft取消と送信済みsave終端を区別する。
- 表示用結果を既存receiverへ保持後、同じownerで一度だけ継続する。
- Close／Remote取得時は元ownerの取消・終端だけを進め、兄弟へ表示継続しない。
- watcherのcutoff ACKは採用後だけ。通知、cause、restart要求を失わない。
- frame_counter・input_seqを固定して通常／IndexOnly要求の開始・取消・再開始を行い、別要求は別ID、同じ要求のCommonとbook restoreは同じIDとなる。mountで採番が戻らない。
- C4・C5の既存pointer／decision／frame予算回帰を弱体化しない。
- root lifecycle serviceだけを呼び、trayへの置換、保存中の延期、receipt後の一度だけの終了継続、settings-family終端、ZIP取消終端を検査する。UIでCancelCloseを補わない。
- sidecar等のmodal拒否と、保存待ちによる延期を区別する。通常Closeと明示Quitの意味を保持する。
- 別contextのsave receiptと未反映ratingを同時に用意し、receipt・resume記録は進むが、往路・callback・復路で表示再採用が起きない。
- UI mount wrapperは従来のrating boundary・reconciliation順序を維持し、未反映更新を次のUIで正しく採用する。
- 元owner・favorite設定投影の復元、入れ子、同contextアクセス、既存unwind復元、終了済みcontextへの遅いreceiptを検査する。

追加境界回帰は共通filter `egui037_continuation`で選択可能にする。実ownerの未受信保持・単一消費・終端・採番を検査し、source grepだけを証拠にしない。

root CloseのApp回帰には`root_lifecycle`、状態アクセス／mount回帰には`context_state`も含め、個別に選択できるようにする。0.37 backendがlogic直後にCloseを確定する接続回帰は段2c・段3へ残す。

レビュー可能な単位で、既存保存・取消／D3、root lifecycle、非描画状態アクセス／mountを分けてコミットする。root lifecycleとmount整理で同じ共有ファイルを触る場合は一人のwriterで順に進める。段1b-2へ追加しない。

**規模:** 8～16ファイル、650～1250行。純移動を除く。第4.3版の250～500行に、root lifecycle約180～350行、owner付き状態更新・非描画mount約220～400行を追加した。App内の共通化・非対話回帰を含み、段2cのvendor回帰・段3の正式接続は重複計上しない。

#### 段1b-3c: 入力投影・clipboard・native出力

foreground、wheel消費位置、clipboard条件、native IME出力変換・intent配送、texture output完結入口を既存owner・旧順序で抽出する。新IME中断、新版delta、案Bのbackend条件変更は段2・段3へ残す。

**受入:** 通常core check、関係する入力／native／output焦点試験、旧snapshot通常比較。Win32 queue・WH_GETMESSAGEの既存drain条件を維持する。

**規模:** 6～12ファイル、300～650行。純移動を除く。

#### 段1b残作業の再見積りと共通受入

| 内訳 | ファイル数 | 概算変更行 |
|---|---:|---:|
| 1b-2 gamepad | 4～8 | 250～500 |
| 1b-3a Remote | 4～8 | 350～650 |
| 1b-3b 保存・取消終端／D3／root lifecycle／非描画状態アクセス | 8～16 | 650～1250 |
| 1b-3c 入力・出力 | 6～12 | 300～650 |
| **残作業合計** | **重複を除き18～38程度** | **1550～3050** |

完了済み1b-1の純移動、段2のvendor、段3の正式接続、PNG更新は含めない。小段ごとにビルド可能な状態でコミットする。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check -p mimageviewer --bin mimageviewer-core --features test-script
cargo fmt --check
.\scripts\test-focused.ps1
cargo test -p mimageviewer --lib egui037_continuation
```

追加filterは実装後の選択件数を記録し、0件を成功としない。段1b全体の終わりでtest-lib-fullと旧snapshot通常比較を行う。同source・同条件の有効な成功台帳を再利用し、無filter main libを直接cargo testへ置き換えない。

受信・採用・取消・ACK・mount ownerの対応を台帳へ記録する。「全serviceの入力依存0件」という名称ベースの完了条件にはしない。実ウィンドウ試験は自動gateへ混ぜない。

第4.4版の増分は1b-3bへ集約する。段1b-2は並行中のgamepadの範囲・受入条件を維持し、ここでは完了や検証結果を先取りしない。

`root_lifecycle`と`context_state`の個別filterも成立件数を記録する。非描画serviceが旧UI mount wrapperへ戻る変更と、UI wrapperから表示再採用を落とす変更の双方を拒否できる回帰を置く。

### 段2: 新版vendor再構築

準備用の実ディレクトリ・standalone graphで新版を検証し、root patchは旧vendorのまま維持する。新版egui-imeの共有正規化も準備graphへ接続する。正式置換は段3。reparse pointは使わない。

| 小段 | 範囲 | 推定規模 |
|---|---|---|
| 2a | egui既存API、IME中断要求のviewport所有・pass出力集約、承認済みviewportの非描画退役入口、クリックfixture、egui-winit IME配送・Disabled互換・trace・clipboard、共有IME fixture | 11～20ファイル、900～1850行 |
| 2b | egui-wgpu mipmap、delta完結、surface、診断 | 10～16ファイル、1100～2100行 |
| 2c | eframe scheduler、案Bの最小化UI継続と非描画出力・action完結、immediate子自身の描画資格、root lifecycleのClose確定接続回帰、viewport入力所有、skip保持、子Close受付・消費・非描画退役、visible commit、delta、計測・診断 | 17～30ファイル、2050～4150行 |

原本置換は別計上する。

第4.1版では段2aに約200～400行を追加した。主な増分はegui内の限定修正と、実入れ子run_ui・実discardのheadless回帰である。App側の新しい入力状態や配送queueは含めない。

第4.2版では、通常GCと共有する非描画退役入口・回帰として段2aへ約100～200行、wgpu／glowのClose受付・消費・対象限定退役と回帰として段2cへ約250～500行を追加した。App側の終了owner接続は段3へ別計上する。

第4.3版では案Bの判断条件・共通action処理・コメントに約40～120行、非対話回帰に約150～350行を見込み、段2cを丸めて約200～500行増とした。既存scheduler／delta修正は重複計上しない。案Bの方針変更と残る期限超過経路を§11へ追記する前提とする。

第4.4版では、両backendのimmediate描画資格・action接続と、root logic-only Close確定の非対話回帰として段2cへ約150～300行を追加した。App側のroot policy・非描画mount実装は段1b、正式接続は段3へ計上する。

#### 段2aの順序・受入条件

1. **冒頭でState変換から実TextEditまでのDisabled取消回帰を置く。**
2. 非空Preedit→Disabledのみ、focus喪失後、Commit後、重複通知、通常選択文字、兄弟Stateを検査。
3. Hidden／Forwardedの網羅回帰を移植。
4. Ctrl+Shift clipboard、Modifiers、trace秘匿を検査。
5. release射影の順序・非click・discardを保持し、分類fixtureを本家0.37へ合わせる。
6. **IME中断要求を既存のviewport別Focusへ移管し、同一Contextの親だけ／子だけ要求する実入れ子run_uiで、それぞれのIMEOutputを検査する。**
7. **PlatformOutputの同一viewport集約を修正し、実Context::run_uiの第1passだけで要求・discardした場合に、第2passのIME矩形等と中断要求trueが最終出力へ残ることを検査する。**
8. 親子ともIMEOutputが存在すること、次の新規runでは要求が残らないことを確認する。Memoryのviewport ID手動切替やappend単体試験だけで代用しない。
9. 生成・集約回帰の名前に共通の`ime_interrupt`を含め、準備graphと正式graphで選択できるようにする。修正前失敗・修正後成功の証拠を実装時に取得する。
10. IME中断要求の所有修正について、着手前の構造合意とdetached計画§11記録があることを確認する。
11. 承認済みviewportの非描画退役入口を通常GCと共通化し、対象登録の終了、兄弟不変、後続root／兄弟出力からの非再作成を検査する。hide／minimizeだけでは退役しない。

本家クリック分類の製品採用は§5の判断を要する。vendor fixtureで本家の事実を検証することと、製品操作差の受入れを混同しない。

段2aはDisabled入力互換、IME中断要求の生成・集約、既存パッチ移植をレビュー可能なコミットへ分ける。各回帰は実windowを作らず、FullOutput検査後のheadless明示破棄を行う。App補助フラグ、追加repaint、新しいqueue、retryは追加しない。

非描画退役の抽出はIME修正と別コミットに分け、Close lifecycleの構造合意・§11記録に含める。

#### 段2bの受入条件

whole／partial／free、no-surface、submission、mipmap、headless readback、対象surfaceだけの破棄、通常／診断featureを検査する。

#### 段2cの受入条件

- RawInput保留がViewport所有となり、共通EpiIntegrationから別viewportが取り出せない。
- root LogicOnly(A)→child Full(B)→root Full(C)でA／B／Cが正しく分離・順序保持される。
- child skip→他viewport更新→child復帰でrelease・focus・ModifiersChanged・IME終端・新規入力が残る。
- skipを繰り返しても欠落・重複がない。
- hookへ正しいviewportの入力だけが渡る。
- immediate／deferred、wgpu／共有glow経路の入力取得・寿命処理が一致する。
- viewport終了が兄弟の保留を消さない。
- scheduler、outer phase、SW_HIDE、最小化、visible child、期限超過をpure testで確認。
- 子を最小化→Close要求→復元せず終了する。子callbackやUI passを実行して成功させない。
- rootもlogic-onlyの場合に、承認済み子のbackend／Context登録を退役できる。
- modalで拒否したCloseが、modal終了・子復元・後続Fullで再発火しない。
- 拒否後の新しいCloseは通常どおり受け付ける。
- 子Fullと親処理の順序が変わっても二重処理せず、初回callback前のCloseも扱える。
- OS Closeとprogrammatic Closeが同じ受付を通る。rootの終了・tray契約と既定Appの従来委譲を維持する。
- 承認後の兄弟Full／root Fullで対象を再作成せず、兄弟の入力・IME・texture・surface・寿命を維持する。
- 拒否した要求だけを消費し、通常のkey／pointer／focus／IME列を消さない。
- 未配送deltaを共有rendererへ引き継いでから対象を退役する。
- wg​​pu／glowのClose配送・消費・退役が同じ契約を満たす。
- §11の構造合意・記録がある。

各段でstandalone非対話試験と旧root core checkを維持する。live compileは可能だが実行は段7。全差分を台帳化する。

Close回帰はbackendの実際の受付・消費・退役helperとfake App／resource ownerを接続して行い、単に「close予定になった」だけで成功としない。追加repaintや強制UI passを必要とせず登録・資源が退役することを非対話で検査し、実HWNDの結果は段7で確認する。

案Bについて、次を追加する。

1. Windows最小化rootの要求済みFullでlogic・uiが実行され、最小化deferred childのFullで子callbackが実行される。
2. UI実行時にもminimized／visible情報は真の値のままで、不可視surfaceの通常render／presentは呼ばれない。visible childの描画は維持する。
3. 同じ最小化条件でも明示的LogicOnlyではui・子callbackを実行しない。deadlineは本家250 msとし、独自の延期・retryを足さない。
4. generic repaintの100 ms下限、遅い要求を早めないこと、要求なしのsleep、outer phaseとbootstrap／resizeの既存例外を維持する。
5. 非描画Fullのtexture deltaが共通ownerへ一度だけ渡り、submission／freeまたは既存ownerへの引継ぎまで成立する。無条件clear、二重適用、可視化待ちの放置がない。
6. platform output・IME出力・viewport commandsが描画なしでも対象ownerへ届く。present／visible commit／実表示ACKは発行されない。
7. Cut／Copy／Pasteのactionが描画なしでも対象Stateの次回入力へ一度だけ渡り、後続描画で重複しない。通常／nativeのclipboard条件は維持する。
8. Screenshot／PaintWhileHiddenを描画不要actionのdrainで失わず、明示描画と既存capture完了契約を維持する。
9. wg​​pu／glowに同じ判定と出力・action回帰を置く。実backendが使う判断・配送helperを検査し、未接続の純粋関数だけで成功としない。
10. child skipのfixtureは期限超過等を明示し、Windows最小化単独をskipの期待値にしない。
11. 案Bの通常Full回帰とは別に、CloseはUI／callbackを実行しない試験でも受付・消費・退役できる。
12. §11へscheduler方針変更、UI／surface分離、残る期限超過経路が追記され、構造整合を確認している。
13. immediateの実入口で、親最小化・子visible、親子とも最小化、最小化子へのScreenshotを検査する。子のUI／paint／callbackと親のpaintを別々に数える。
14. immediateの非描画でもdelta・platform output・commands・clipboard actionを完結し、当該子UIからのScreenshotと既存保留Screenshotが重複・欠落しない。
15. root Closeを含むRawInputで`update_logic_only`へ接続し、ui呼出し0のままCancelCloseで終了を止める場合と、CancelCloseなしで終了する場合を検査する。完了後の再Closeは一度だけ通常経路へ配送する。

準備graphの回帰名に`minimized_full`を含め、正式graphでも選択できるようにする。scheduler／App呼出し・出力適用はfake window／painterと実integration入口で検査する。surfaceを使う検査は既存offscreen契約を利用し、desktop操作を自動gateへ混ぜない。

root Close確定回帰はfake Appのlogicだけからcommandを返し、uiを呼ぶと失敗するfixtureとする。tray・保存・settings-family・ZIPの本体policyは段1b／段3で検査し、vendorに製品設定を持ち込まない。

immediate修正とroot Close確定回帰は独立したコミットへ分ける。glowでも必要なGL資源適用とsurface描画を区別する。§11の追加範囲にroot lifecycle・非描画mount・immediate子描画を記録する。

### 段3: 依存と必須契約の原子的切替

範囲:

- egui系0.37、wgpu30、ラボ、egui-ime、kittest。
- App／Ui／Panel／Context／on_exit。
- logic service、Modifiers、wheel。
- viewport所有RawInputとIME producer互換の正式接続。
- IME中断要求のviewport別生成とdiscard再pass出力集約の正式接続。
- 子Closeの非描画受付、既存modal判断・終了ownerへの合流、承認後の対象限定退役。
- gamepad物理更新・受付事実・未配送edge保持・UI batch。
- native GPU／surface／present、IME中断intent・control配送。
- 全delta消費・引継ぎ・headless明示破棄。
- clipboard、screenshot callback、全test compile、追加accessibility。

必須契約を段4へ残さない。汎用非表示機構は作らない。

```powershell
cargo check -p mimageviewer --bin mimageviewer-core
cargo check --workspace
cargo test -p egui --lib ime_interrupt
cargo test -p egui-ime
cargo test -p egui-winit --lib
cargo test -p mimageviewer --lib --no-run
cargo test -p mimageviewer --test ui_snapshot --no-run
cargo test -p mimageviewer --test book_composition_editor_snapshots --no-run
cargo fmt --check
```

`ime_interrupt`は段2aで追加する生成・集約回帰の共通filterとし、正式graphでの実選択件数を記録する。0件選択を成功と扱わない。

実接続点の必須回帰:

- deltaの描画・無描画・早期終了・headless。
- root／child交互実行とchild skip後の終端保持。
- 最小化したpassive deferred子のCloseが、既存modal判断・終了ownerへ届き、復元せず終了する。
- active／parkedの対象contextへの終了委譲を維持し、passive画像のcleanupで代用しない。
- modalで拒否したCloseが後から再発火せず、新しいCloseは受け付ける。
- OS Close、programmatic Close、bar／watcherのApp入口を同じ終了判断へ揃え、子FrameのOS Close読取りによる二重採用を残さない。
- rootもlogic-onlyの場合に承認済み対象を退役でき、兄弟の入力・資源・寿命を維持する。
- 対象Close時に既存gesture／media／worker ownerを終了し、共有deltaを失わず、後続root／兄弟Fullで子を再作成しない。
- Disabled取消がproducerから実TextEditまで到達。
- 同一Contextの親子で中断要求が消失・混入せず、正式graphで修正済みeguiが使われる。
- 実run_uiのdiscard再passで最新のIME矩形等と中断要求が保持され、次の新規runへ要求が残らない。
- nativeのfocus／cursor移動から中断出力、intent、control、対象WindowEpochへ一度だけ配送。
- 集約済み中断要求が通常backend／nativeの正しい配送先へ一度だけ渡る。別viewportのPlatformOutputを混ぜない。
- native中断がvisual上書き・複数logical passで消失／重複せず、終了したhostから新hostへ漏れない。
- focus移動前の受信済み離散キー。
- gamepadの同じframe番号中の複数logic、短い退避復帰、press→release引継ぎ、disconnect／ring／Undo。
- gamepad保持軸の復帰後継続。既存`src/app/gamepad_input.rs:9697`、`:9763`、`:9809`の期待値を維持。
- WH_GETMESSAGE既存drain。

CloseのApp側接続では、`src/ui_fullscreen.rs:21756`、`:21876`の既存modal判断・終了処理を描画非依存のserviceとして再利用する。`src/app.rs:3524`のwatcher Closeやbarの×も同じ判断へ合流させる。新しいclose pending bool、復帰待ちqueue、時間窓は追加しない。

graphの旧egui混入・型分裂・古いpathを確認する。compileだけでは受け入れない。

#### 段3の再見積り

段1b・段2完了後の接続差分。原本・lockfile・PNGを除き、内訳は重複計上しない。

| 内訳 | 概算変更行 |
|---|---:|
| App／Ui／Panel／一般API／ラボ | 1200～2300 |
| event／wheel／native GPU接続。delta・IME出力を除く | 650～1300 |
| delta消費／引継ぎ／headless | 450～1000 |
| gamepad引継ぎ・受付・UI接続・必須回帰 | 200～450 |
| viewport入力所有・IME producerの本体接続回帰 | 100～250 |
| native IME出力・control配送・必須回帰。生成・集約済み出力の接続確認を含む | 250～550 |
| 子CloseのApp終了owner接続・入口統一・接続回帰 | 200～400 |
| 案BとC1〜C5・D3の正式接続・追加接続回帰。immediateの親子条件を含む | 100～250 |
| root lifecycle・非描画状態アクセスの正式接続回帰 | 150～300 |
| **合計** | **3300～6800** |

**112～200ファイル程度。**

第3.1版のgamepad引継ぎを再評価し、Astra指摘の接続・回帰を追加した。段2a／2cのvendor実装は上表へ重ねて計上しない。

第4.1版では、IME生成・集約からbackend配送までの接続確認として約50～100行を追加した。egui内のフラグ所有・append修正と最小回帰は段2aへ計上し、ここでは重複計上しない。

第4.2版では、子Closeの既存終了ownerへの接続、OS／bar／watcher入口の統一、modal拒否後の非再発火と兄弟不変の接続回帰として約200～400行を追加した。非描画退役のvendor実装・単体回帰は段2a／2cへ計上する。

第4.3版では、案Bの正式接続、最小化Fullでの表示継続、期限超過中のRemote・保存・取消終端、次の対象Fullへの引継ぎの追加接続回帰として約100～250行を追加した。Remote owner整理・共有終端抽出・D3採番は段1b、案Bのvendor判断・action配送と最小回帰は段2cへ計上し、上表へ重複計上しない。

C2・C4・C5とC3の表示用pollをuiへ残すため、全完成結果のlogic受信、共通表示待ちqueue、C4進捗のみの必須分割は見積りに含めない。一方、期限超過が残るのでC1と保存・取消終端を一括削除して減額することもしない。

第4.4版では、実logic-onlyのroot Closeと、別contextのreceipt・未反映ratingを使う正式接続回帰として約150～300行を追加した。root policy・owner付き状態更新・非描画mountの共通化は段1b-3b、immediate描画資格とvendor最小回帰は段2cへ計上する。immediateのApp側代表接続は既存案B行の範囲で見込む。

段1b後に実差分で再評価する。gamepadやnative配送の抽出が不足する場合は、旧版上の挙動不変抽出を追加し、原子切替へ未整理の大改修を押し込まない。

#### 第4.5版: 段3停止点からの追加範囲と再見積り

`df0037eb7`は段3途中のWIPであり、通常core check成功をClose接続・全test target・正式vendor回帰の成立証拠にしない。段3台帳の未実施条件はそのまま残す。

段0〜段2の完了済み差分を遡って未完了へ戻さず、今回の追加vendor修正と製品接続を段3内で完成させる。段4へ退役契約やC1の正式logic接続を先送りしない。

追加範囲:

1. eguiの承認済み退役command、登録寿命の同定、root command列での保持、Full／LogicOnly／pass外drain、入れ子・discard・appendの契約を実装する。
2. eframeの両backendで、承認済み退役consumer、通常GCとのcleanup共有、旧出力の非再作成、root終了時の出力完結を接続する。
3. Appの既存終了ownerで、binding解除前に権限を移譲する。`finish_active_detached_session_close`等のhelperへ必要なContextまたは出力sinkを明示して渡す。
4. mode changeの空immediate UIと退役目的のVisible(false)を除き、logic／uiから同じ終了入口へ揃える。
5. active／parked／passive、watcher、barの×、OS／programmatic Close、Remote起点を棚卸しし、未承認要求と承認済み退役を混ぜない。
6. Remoteのclose readinessを未配送の承認済み出力と接続し、C1をlogicへ正式接続する。AcquiringRemoteの既存wake・保存・AI・取消・ACK契約を維持する。
7. 以下の追加回帰を正式graphで実施し、段3台帳へ選択件数・結果・対象sourceを記録する。

見積りは、上の第4.4版までの段3見積りに対する増分である。通常Closeの既存接続・回帰、段2の既存vendor移植、原本・lockfile・PNGを重複計上しない。

| 第4.5版の追加内訳 | 概算変更行 |
|---|---:|
| egui／eframeの承認済み出力・寿命同定・配送・cleanup修正 | 450～850 |
| App終了入口・mode change・Remote readinessの追加接続 | 150～300 |
| 今回追加するvendor／App接続回帰 | 450～850 |
| **追加合計** | **1050～2000** |
| **段3全体の改訂見積り** | **4350～8800** |

追加差分の対象は約14～24ファイルを見込む。多数は既存の段3対象と重なるため、旧見積りの112～200ファイルへ単純加算しない。段3全体は概ね115～215ファイルを目安に、再開後の実差分で更新する。

この数字は段3全体の設計見積りと今回の追加量であり、WIPからの残り全変更行数を測定した値ではない。既存WIPの機械的test移行、ラボ、gamepad等の未完量は段3台帳から別途見積もる。既に実装した行数を新規残作業として数え直さない。

作業は、vendorの受渡し契約とheadless回帰、App終了入口とRemote正式接続、残る段3全target移行・検証の順に分ける。各チェックポイントでcore checkを維持し、機能未接続の状態を段3完了として扱わない。正式な独立レビューは今回の所有・非同期・複数窓境界を対象に行う。

#### 第4.6版: 段3再停止点からの追加範囲と見積り

`fe32f2e88`の正式graphで得たコンパイル・回帰結果は、段3台帳のsource・feature・条件に従って再利用する。今回の追加2件を未解決のまま段3完了とせず、段4／snapshot更新へ送らない。

追加作業:

1. Close前の純粋な帰還効果生成と、存続mainの既存帰還ownerへの型付き移譲を実装する。
2. `pending_return_to_parent`の型変更、context bundleの保存・swap・初期化、既存UI消費位置を揃える。
3. active／standalone／parked、通常CloseとRemote／置換causeを接続し、navigation／bookmark admissionで失効・上書きを完結する。
4. eguiの実再pass前配送入口とContext束縛scopeを追加する。
5. eframe両backendを既存共有rendererへ接続し、pending deltaの非相殺受入を共通化する。
6. plugin・scope隔離・実callback drop・実resource consumerを含む回帰を追加する。
7. 第4.5版の退役、Remote、dirty保持と既存段3受入を再確認し、独立実装レビューの残るP1を閉じる。

| 第4.6版の追加内訳 | 概算変更行 |
|---|---:|
| Close帰還ownerの型付き化・移譲・失効・UI接続 | 300～550 |
| egui配送scope・両backend接続・受入共通化 | 250～450 |
| App／plugin／scope／両backend／実resource回帰 | 500～900 |
| **追加合計** | **1050～1900** |
| **段3全体の改訂見積り** | **5400～10700** |

今回の追加は概ね14～24ファイルを見込む。既存段3対象との重複が多いため、ファイル数は旧見積りへ単純加算せず実diffで更新する。原本・lockfile・PNGを除く設計見積りであり、WIPからの残り全行数を測定した値ではない。第4.5版で実装済みの行を残作業として再計上しない。

Close帰還とdelta所有を別のレビュー単位に分け、それぞれcore checkと限定回帰を維持する。両方の接続後に段3の残る受入を完了する。epaint vendor追加、TextureManager再預託API、第二renderer ownerは見積りに含めない。

台帳のdirty P1補修は維持し、再実装しない。launcherのrelease入力3ファイル不足、C2〜C5／D3の製品Full接続証拠、C4の新pointer、C5予算、D3 cutoff、fullscreen混在wheel、最終lib全件等の既存残件は別に残る。今回の限定回帰成功で代替しない。

#### 第4.5版: 追加必須回帰と受入条件

既存の段3受入1〜16と、IME・gamepad・入力・deltaの必須回帰は維持する。次を追加する。

1. **Full内logicからの退役:** 実`EpiIntegration::update`でApp::logicから承認済み退役を発行する。UI pass内の直接破棄を起こさず、最外側から戻ったpass外で対象native／Context／IME資源が終了する。
2. **LogicOnlyだけの退役:** UIと子callbackを一度も呼ばず、実`update_logic_only`の出力から対象を終了する。次のFull、追加repaint、通常GCを完了条件にしない。
3. **GC前の保持:** 承認後に対象childを使わず、実`Context::end_pass`のGCを通す。子のViewportOutputがなくなっても承認がroot出力に残り、native／IMEの残存資源が終了する。
4. **immediate入れ子:** 子callbackから承認を発行しても、親passが残る間にContext退役を行わない。最外側が戻った後に対象と所有子孫だけを終了する。
5. **実discard再pass:** 第1 passで承認し、discardでlogicを再実行する。承認が保持され、App終了ownerの再実行や二重退役がなく、Remoteが途中passでFinishAcquireしない。
6. **exactな子孫と兄弟:** 対象・子孫・独立兄弟を置き、Contextとnativeで一部のGC状況を変える。承認した寿命の残存資源だけが終了し、兄弟の入力・IME・pending RawInput・surface・texture・workerを維持する。
7. **旧出力と再open:** 退役前に取得したbuilder／callback／commandを含む出力を、退役後のroot／兄弟出力と集約・処理する。旧窓を再作成しない。同じViewportIdを明示的に新寿命で開いた場合も、旧出力が新窓を更新・終了しない。
8. **通常Closeとの分離:** modal拒否は一回消費され、復元後に再発火しない。新規Closeは受理する。承認済みcommandをOS Closeへ変換せず、終了済みApp bindingを再検索しない。非管理viewportは従来の委譲へ進む。
9. **終了入口:** mode change、active、parked、passive、watcher、barの×、OS／programmatic Closeの代表を同じApp終了ownerへ通す。mode change／Remoteから空immediate UIへ到達しない。parkingだけでは退役しない。
10. **Remote取得:** Full、LogicOnly、discard再passそれぞれで取得を開始する。App runtime解除後でも未配送承認がある間はAcquiringRemoteを維持し、backend退役後の既存service機会で、保存・resume／shift・editor・AI readinessと併せてFinishAcquireする。UI復帰を必要としない。
11. **deltaとdrop後の追加分:** whole／partial／freeを含む共有deltaと、Context／callback dropで生じる追加freeを使う。対象終了でdeltaを失わず、submit前freeを行わず、兄弟textureを維持する。surfaceなしとroot同時終了も検査する。
12. **通常GC・重複・順序:** 承認済み退役と通常GCの順序を入れ替え、祖先と子の承認が重なる場合も各資源を一度だけ終了する。Close hookからの完了通知がなくても残存資源を終了する。
13. **root終了:** 子の退役とroot Closeが同じ処理で起きても、rootのtray置換・保存待ち・CancelCloseを維持する。終了確定時はcommandとdeltaを消費してから共有ownerを終了する。

vendor回帰は同じ共通シナリオをwgpu／glowの両adapterへ適用する。状態・配送はheadless／fake backendを主証拠とし、deltaのsubmission／freeは既存のoffscreen検証を使う。実ウィンドウ起動を通常の自動gateへ混ぜない。

追加回帰の共通filterを`viewport_retirement`とし、正式manifest／featureに合わせて次を実行する。

```powershell
cargo test -p egui --lib viewport_retirement
cargo test -p eframe --lib viewport_retirement --no-default-features --features default_fonts,wgpu
cargo test -p eframe --lib viewport_retirement --no-default-features --features default_fonts,glow
cargo test -p mimageviewer --lib viewport_retirement
```

Appの代表回帰は`viewport_retirement`で選択できる名前へ揃える。選択0件を成功と扱わない。GPU前提不成立は未実施として台帳へ残し、状態回帰の成功と区別する。

追加差分後に、既存段3のcore／test-script feature／workspace check、全test target compile、IME・continuation・root lifecycle・context state・両backend minimized_full、focused、fmtのgateを完了する。main lib全件は`.\scripts\test-lib-full.ps1`を使い、段3台帳の最後の1回を実施・分類する。最終`test-full.ps1`と確認用buildの段階は既存計画を維持する。

`df0037eb7`で未実施のgateを、今回の追加回帰で代替しない。snapshot PNGはこの受渡し修正のために更新しない。

#### 第4.6版: 追加必須回帰と受入条件

**Close帰還**

1. active at-restのbookmark book／mediaを非描画Closeする。viewerとnative資源は終端するが、帰還要求は存続mainに残る。次のmain UIで一覧更新・復元を一度だけ行う。
2. detached mediaが元targetから別media／folderへ移動した後にCloseし、最後の実フォルダとmedia選択へ戻る。bookの内部移動では従来どおりbookmark一覧を維持する。
3. standalone bookmarkとauto-open対象をCloseし、teardown後にも一覧／親navigationと選択intentが残る。ZIP／PDF、通常画像folder、変換アーカイブの代表を使い、キャッシュfolderへ帰還しない。
4. mainのselection／scroll変更は維持し、row増減時の位置復元も既存期待値を保つ。
5. parked／passiveのoriginが存続する場合と、新navigation・新bookmarkで置換済みの場合を検査する。無関係な兄弟・mainを変更しない。
6. Close→UIなし→Remote取得→返却→遅いUIで旧帰還を再発火しない。取得・ACK・drainは帰還UIを待たない。
7. Close→同pathへの新navigation、新bookmark開始→旧Close到着を検査し、新しいownerを旧要求が消去・上書きしない。
8. 実EpiIntegrationのLogicOnlyからCloseを処理し、items・bookmark read-model・dirty集合を表示採用せず、終端と型付き移譲だけ進むことを検査する。
9. 通常UI・早期return・既存native即時Closeのconsumerで一回だけ消費する。`native_immediate_close_consumes_bookmark_return_in_same_dispatch`等の既存期待値を弱めない。

**delta配送**

1. 実`Context::run_ui`の第1 passでwhole＋partialを生成し、discardする。次passで同IDの新寿命immediateを開き、旧native callbackが持つ最後のTextureHandleを実際にdropする。
2. 第1 passのset／partialがintegration sinkへ移り、実immediate consumerの後発freeより先に処理されることを検査する。fake adapterがbatchを保持しただけの試験を主証拠にしない。
3. wgpuの実resource consumerで旧textureの内容をoffscreen readbackし、whole／partial適用とsubmission後にfreeされることを検査する。glowは実adapterとresource consumerの順序を検査し、実GL環境の証拠とfake painterの証拠を区別する。
4. 共有pendingが既にある場合、resource利用不能で既存pendingに残る場合、後発freeとの合流を検査する。先行set／partialを相殺せず、利用可能なconsumerでset→submission→freeを完結する。
5. 入れ子immediateのdiscard、複数回の実discard、空deltaを検査し、最終outputへ移譲済みdeltaが戻らないことを確認する。
6. discardなし、max_passes到達、direct begin／endでは新sinkが呼ばれず、通常outputが必要なdeltaを所有することを検査する。
7. output pluginを実際に組み込み、各passの観測が従来どおり一度であること、pluginによる変更後のbatchが配送されること、IME／page-turn receipt／実表示ACKを変えないことを検査する。
8. Context A／B、入れ子scope、正常終了・早期return・unwindで、誤配送・登録残留・二重配送がないことを検査する。
9. 第4.5版のGC・承認済み退役・root終了と組み合わせ、drop後のdeltaを失わず、新寿命・兄弟資源を維持する。

追加回帰には`close_return`と`discard_texture_delivery`の共通filterを付け、正式graphで実選択件数を記録する。代表コマンド:

```powershell
cargo test -p mimageviewer --lib close_return --offline
cargo test -p mimageviewer --lib bookmark_return --offline
cargo test -p mimageviewer --lib viewport_retirement --offline
cargo test -p mimageviewer --lib native_immediate_close_consumes_bookmark_return_in_same_dispatch --offline

cargo test --manifest-path vendor/egui/Cargo.toml --lib discard_texture_delivery --offline

cargo test --manifest-path vendor/eframe/Cargo.toml --lib discard_texture_delivery --no-default-features --features default_fonts,wgpu --offline
cargo test --manifest-path vendor/eframe/Cargo.toml --lib discard_texture_delivery --no-default-features --features default_fonts,glow --offline

cargo test --manifest-path vendor/egui-wgpu/Cargo.toml --lib discard_texture_delivery --features winit --offline
```

0件選択、GPU／GL前提不成立を成功として記録しない。OS windowを必要とする試験は自動gateへ混ぜず、既存の承認付き検証へ分離する。

追加差分後にcore／test-script check、影響した全test target compile、既存IME・page-turn・退役・Remote・dirty保持の関連filter、fmtを確認する。成功済み証拠はsourceと条件が有効な範囲で再利用する。

最終main lib全件は既存計画どおり`.\scripts\test-lib-full.ps1`の最後の1回で実施・分類する。`test-focused.ps1`がbroad/shared変更として未選択を返した場合は0件成功と扱わない。launcherを含むworkspace gateの前提不足も未達のまま記録する。

今回の修正でsnapshot PNGを更新したり、既存Closeの帰還先・選択・scroll・同dispatch消費の期待値を弱めたりしない。

#### 第4.3版追加範囲・受入条件

段1bの抽出を、案Bの正式backendへ接続する。「logic service」は§4.2の個別境界を意味し、全`service_*`の一括移動は行わない。

- C1 Remote、送信済み保存、共有ACK、取消・寿命終端、watcher等をlogicへ接続する。
- C2・C4・C5とC3のlive表示用poll、D3の走査採用を既存owner・旧順序のuiへ残す。
- Windows最小化中もFullで表示継続を進める。surface描画の有無をAppのpoll許可へ流用しない。
- RedrawRequested期限超過中はlogicだけで必要な終端を進め、表示用結果を既存receiverへ保持する。別viewportのFullでは消費しない。
- 旧outer tailは既存のui本体の早期returnでも実行する。logicからUI tailを強制実行しない。
- frame_counterは表示・入力・予算の既存ownerへ残し、D3の採番だけを分離する。F13、G1〜G5の留保を無視したlogic接続をしない。
- root Closeは通常キー入力から分離したlifecycle入口へ接続し、logic直後のbackend終了判定へCancelCloseを間に合わせる。
- C1・保存・取消・終了処理はowner付き状態更新または非描画mountを使う。旧UI mount wrapperをserviceの前後へ残さない。
- immediate子は両backendの実入口まで正式接続し、子自身のmetadata・描画要求でsurface描画を決める。

正式graphの必須接続回帰:

1. 最小化root FullでC2〜C5・D3の代表結果が旧順序で採用され、復元を必要としない。UI側の要求は一度だけ継続し、surface描画なしでも失われない。
2. 同じframe番号でLogicOnlyを複数回実行しても、Remote取得・受信・保存ACK・取消・drainが進む。C2〜C5の表示用receiverとUI decisionは消費・更新しない。
3. LogicOnly→対象Fullで保留結果を一度だけ採用する。discard再pass・後続Fullで同じ結果やACKを再採用しない。
4. Remote取得中の送信済みcomposition save、resume／shiftの完了をUIなしで収穫し、barrierを早く通さない。PC表示継続はRemote完了条件に含めない。
5. Remote返却→再取得→遅いFullで旧Local一覧継続やfullscreenが割り込まない。
6. C3のlive表示保留中にClose／Remote取得を行い、元ownerの取消・global終端だけが進む。兄弟へ表示採用しない。
7. C4は新しいFullのpointer状態でeditorを再開する。期限超過前のprimary_downをlogicから再利用しない。
8. C5のページめくり・preview・PDF昇格／upload予算はUI側で保ち、Runtimeの取消・lease終端を表示待ちへ巻き込まない。
9. D3は採用前のcutoff ACK／mtime／signature更新を行わず、cause・restartを保持する。通常／IndexOnly要求のIDはframeを進めず再開始しても異なる。
10. 非描画Fullのdelta・platform output・commands・clipboard action、明示screenshot、実表示ACKを実接続点で検査する。
11. 期限超過root LogicOnly→child Full→root Full、child skip後の終端保持と、非描画Close退役の既存回帰を維持する。
12. root CloseはUIを一度も実行せず、通常×のtray置換、保存中の終了延期、receipt後の一度だけの終了継続、settings-family・ZIP取消終端を満たす。CancelCloseの有無からbackendが終了を確定するところまで接続する。
13. rootのmodal拒否は後から再発火せず、延期要求は既存ownerの終端で再開する。通常Close／明示Quitの優先規則を維持する。
14. A投影中にBのsave receiptと未反映ratingを処理し、receipt・resume書込みだけが進む。往復mountを含めfullscreen再open、UI decision、兄弟表示再採用は0回で、未反映generationを保持する。
15. 次の正当なUI mountで従来のrating・page-edit採用が行われる。非描画化のために表示更新を永久に止めていない。
16. 親最小化・immediate子visibleでは子だけ描画し、親子最小化では通常描画せずUIと出力は進む。最小化immediate子のScreenshotは子の描画・callbackを一度だけ完了する。

```powershell
cargo test -p mimageviewer --lib egui037_continuation
.\scripts\test-focused.ps1
```

段2cの`minimized_full`回帰も正式vendor graphで実行する。manifest／featureは段2c台帳の成立したコマンドを用い、0件選択を成功としない。vendorの判断・出力回帰と、上記App owner接続回帰を区別する。

追加repaint、新pending bool、時間窓、logicからの強制UIで成立させない。実Windowsの最小化・復元・負荷とRemote操作は段7へ残す。

第4.4版では`root_lifecycle`、`context_state`の個別filterも実行して選択件数を記録する。headless回帰では本体lifecycle serviceと実backendのClose確定境界を接続し、実ShowWindow・desktop操作だけをfake effect ownerへ置き換える。UIを呼んでから検査する試験では代用しない。

### 段4a: 入力・非表示・popupの等価経路確認

```powershell
cargo test -p mimageviewer --lib ime
cargo test -p mimageviewer --lib focus
cargo test -p mimageviewer --lib keyboard
cargo test -p mimageviewer --lib keymap
cargo test -p mimageviewer --lib gamepad
cargo test -p mimageviewer --lib specified_scale
cargo test -p mimageviewer --lib fullscreen_overflow
cargo test -p mimageviewer --lib navigator
```

root／FS／native／兄弟contextのclose、cancel、focus loss、repeat、復帰新規入力へ広げる。IME入力・出力、pointer capture、クリック分類、wheel単位・混在修飾・消費・残量、通常／native clipboardを確認する。

filter選択件数を記録し、PNG外観差と機能回帰を分ける。

**規模:** 10～22ファイル、400～1000行。

### 段4b: フォント・layout採択

```powershell
cargo test -p mimageviewer --lib ui_fonts
cargo test -p mimageviewer --lib ui_font_catalog
cargo test -p mimageviewer --lib calculated_
cargo test -p mimageviewer --lib startup_dialog
python scripts/check_ui_glyphs.py
```

baseline、fallback、toolbar、HUD、preview、margin、separator、小画面を比較。Fits／Scrolls・末尾到達・固定操作を保持し、§1.380残件と今回退行を分ける。

**規模:** 8～18ファイル、400～1000行。previewが大きければ小段化。

### 段4c: GPU・native HUD整合

```powershell
cargo test -p mimageviewer --lib compare
cargo test -p mimageviewer --lib panorama
cargo test -p mimageviewer --lib gpu_lanczos
cargo test -p mimageviewer --lib native_presenter
cargo test -p mimageviewer --lib bottom_hud
```

delta、mipmap、callback、透明合成、font／DPI／galley、surface寿命、submit gateを検査する。native IME回帰は段3成立済みを再利用し、実Windowは分離したまま。

**規模:** 8～16ファイル、300～800行。

### 段5: 全snapshot更新と警告整理

accessibility通過後に通常比較し、外観採択後に更新する。

対象は両integrationと列挙済み全lib snapshot。

```powershell
$env:UPDATE_SNAPSHOTS = '1'
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 台帳に列挙した全lib filterを同じ更新環境で実行
```

更新専用の子PowerShellを使い、各非ゼロ終了を伝播する。終了後、環境変数なしの別processで同じ全対象を比較する。

```powershell
cargo test -p mimageviewer --test ui_snapshot
cargo test -p mimageviewer --test book_composition_editor_snapshots
# 同じ全lib filterを更新なしで再比較
```

実装担当が候補・分類を用意し、設計担当が外観採択・目視結果を記録する。独立レビューは共通理由・例外・回帰証拠を確認する。一台帳で変更445枚全体をカバーし、固定票・PNGごとの重複票は作らない。

Id非推奨・残るRust警告を整理する。IME互換通知を警告削減だけで削除しない。

**規模:** UI／test 10～30ファイル、250～800行、PNG最大445枚。

### 段6: 非対話全体gateと確認用build

段0a完了後のrunnerを使う。

```powershell
cargo fmt --check
python scripts/check_ui_glyphs.py
.\scripts\test-lib-full.ps1
.\scripts\test-full.ps1
.\scripts\build-dev.ps1 -PreserveRuntime
```

通常feature、必要なtest-script、vendor非対話区分、MSRV、core／Remote／EPUBを確認する。

source、command、feature/profile、fixture、exit、件数、ログを記録し、同条件の成功は再利用する。0件選択やfocused runnerの広域変更exitを成功にしない。

既知失敗を除外・閾値緩和で隠さない。自動gate成功とlive未実施を別報告する。check/buildは最低10分、広域testは最低15分以上を確保し、timeout後のBrokenPipeを製品失敗と誤分類しない。

**規模:** 文書・検証補修3～8ファイル、100～300行。

### 段7: 承認付きdesktop検証・利用者実機受入れ

具体suite、時間、PC使用、使い捨てdataを提示し、明示承認後に実施する。改訂依頼を起動承認と扱わない。

エージェントは正規portable-smokeのみ。standalone vendor窓試験は利用者実行の隔離手順。native・DPI等は段6の通常buildを利用者が起動する。

UI確認は追加IME・クリック比較を含め25～40分程度。ReleaseChecks／scheduler live試験は別枠。未実施・環境不成立をPASSにしない。

各段の自動検証済みcheckpointを残し、Windows native変更の最終受入れ・コミット時期はAGENTS.mdの実機確認規則に従う。

## 7. 実機確認手順

通常版・トレイ常駐版を先に終了する。single-instance mutexを共有する。

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

既定で実利用中の`%APPDATA%\mimageviewer`を開き、設定・データを更新し得る。隔離確認を明示的に選ぶ場合は同成果物へ`--data-dir .\target\dev-runtime\data`を渡す。エージェントは通常profileを起動しない。

### 7.1 フォント・ダイアログ

1. 既定、BIZ UDPGothic 9pt、Meiryo 10pt、Meiryo UI 10ptで一覧・詳細・検索・metadata・toolbar。
2. 日本語、Latin、数字、CJK、数学英字、記号、emojiを比較。
3. baseline、折り返し、欠け、豆腐化を確認。
4. 動画／音楽HUDの時間、x1、Norm、dBを確認。
5. 小窓・UI倍率200%で長文dialogの本文・固定ボタン・×へ到達。
6. 採択したWindow drag、scroll、Panel操作を確認。

### 7.2 日本語入力・取消・中断

Google日本語入力とMS-IMEで共通の少数シナリオを行う。

1. 注釈本文をEnter確定し、確定だけでは改行しない。
2. 直後の通常Enterで改行一つ。
3. 検索欄は確定直後の通常Enterで検索。
4. 変換中Escape／候補Tab／Spaceがアプリへ漏れない。
5. Escape取消後に未確定文字が残らず、直後の通常Escapeが働く。
6. composition中に別窓へfocusを移し、重複・消失・キー残留がない。
7. 長い欄・複数行末尾で候補窓がcursorへ追従。
8. main／F12／native入力の代表を確認。
9. **native入力欄で変換中にcursor位置をclick移動し、古いcompositionの更新・確定が新位置へ残らない。**
10. **nativeで別入力欄へfocus移動し、前の欄の未確定文字が新しい欄へ混入しない。**
11. 中断後に新しく日本語を入力・確定し、IME許可や候補窓位置が戻る。
12. native窓を閉じて別動画を開き、旧窓の中断要求が新しい入力を取消さない。

OSがどのIME列を出したかと、画面結果を区別して記録する。未発生のDisabled-only経路はheadless回帰で担保し、実機発生を確認済みと書かない。

IME中断要求の親子分離とdiscard再pass保持は、段2a・段3の非対話回帰を主証拠とする。実機では上記のmain／F12／native代表経路で、正当な中断が働き、兄弟への不要な取消がないことを確認する。利用者にdiscardの発生や厳密なpass順序を手操作で再現させる追加マトリクスは要求しない。

### 7.3 shortcut・wheel・clipboard・クリック

1. main／F12でCtrl／Shift／Altを押して離し、通常へ戻る。
2. Ctrl+矢印、Space pan、筆切替。
3. Enter／NumpadEnter、右Ctrl、Tabの代表割当。
4. popup内wheel／Ctrl+wheelと終了で背面が動かない。
5. popup終了後の新wheelは働く。
6. slider／seekを外で離し、dragが残らない。
7. 通常／nativeでCtrl+C/X/VとCtrl+Shift+C/X/V。
8. IME終了直後も二重文字が入らない。
9. **音量・速度reset、reader、resize resetを近い位置のdouble clickで操作。**
10. **同じ広い部品上でclick位置を離した場合の差を、採択した0.37分類と比較。**
11. **文字のdouble／triple click選択を確認。**

6 pointsの厳密な境界・時間条件はfake入力回帰で検査し、利用者にpixel精度の操作を要求しない。

### 7.4 非表示・複数窓・Remote・gamepad

1. 非同期処理中にトレイへ隠し、完了後復帰。
2. 別にrootを最小化し、完了処理と復帰後新規入力を確認。
3. visible F12を残してrootをhide／minimizeし、子の入力・処理を確認。
4. childを最小化・復帰し、押下・focus・IMEの残留がない。
5. root最小化中、別アプリでgamepadボタンを操作して復帰。過去press／repeatが発火しない。
6. **軸を倒したまま復帰すると現行どおり継続し、中立・release後の新操作も働く。**
7. ring終了・disconnect後の対象contextとUndo。
8. composition中focus移動、drag外側releaseと復帰。
9. 同じPDF／画像の兄弟reader片方を閉じても他方を保持。
10. root非表示中のRemote応答・完了。
11. idle／wakeは承認済みReleaseChecks。
12. **別窓を2つ用意し、一方をpassive状態でタスクバーから最小化する。そのwindowのタスクバーメニューから「ウィンドウを閉じる」を選び、復元せず終了することを確認する。もう一方の表示・入力は維持する。**
13. **対象子で既存のCloseを拒否するmodal条件を作り、最小化したまま同じClose操作を行う。modalを終え、子を復元しても勝手に閉じないこと、その後の新しいCloseでは終了できることを確認する。**
14. rootも最小化した代表ケースで子をタスクバーから閉じ、root・子の復元を必要とせず対象だけが終了することを確認する。rootを戻した後も子が再作成されないことを確認する。

root LogicOnly→child Full→root Full、短い退避、同一batch順序、期限超過は非対話回帰を主証拠とする。実PCへ大量の任意入力を送らない。「前後入力をすべて捨てる」を期待結果にしない。

modal拒否・初回callback前・兄弟Fullとの順序網羅は非対話回帰を主証拠とする。実機のmodal条件は既存機能で再現できる代表一例に絞り、試験のための保存失敗や設定破損を作らない。エージェント実行の場合は追加したCloseシナリオも承認suiteへ明示する。

案Bの実機確認を追加する。既存の非描画Close確認と混同しない。

15. 本を開く準備中、または原画を待つ編集開始中にWindowsのrootを最小化する。処理後に復元し、最小化中のFullで表示準備が進んだことを既存診断と画面結果で照合する。新しい要求・タイマーを試験のために製品へ追加しない。
16. 最小化したままRemoteで取得・一覧表示・ページ送り・保存を一つ行い、PC復元なしで応答することを確認する。返却後は従来の一覧へ戻り、閉じた閲覧窓を自動復元しない。
17. 原画待ちの表示トリム再開でmouse buttonを離してから復元し、古いdragが残らないことを確認する。
18. 最小化中のidle／処理中の負荷を旧版条件と比較する。UI処理がある間の負荷と、要求なしのidleを分ける。UI継続自体を新しい退行と扱わず、更新による負荷増の有無を記録する。

厳密な100 ms境界、250 ms期限超過、pass交互順序、非描画Fullの出力完結は非対話回帰を主証拠とする。全C3入口・画像形式の手操作マトリクスを増やさない。既存ReleaseChecksへ最小化が含まれない場合は、この追加範囲を別の承認済み検証として記録し、未実施をPASSにしない。

第4.4版の実機代表確認:

19. 通常×をtrayへ置換する設定でrootを閉じ、従来どおり退避・復帰できることを確認する。送信済み保存を伴う代表例では、保存を失わず既存の終了方針へ戻ることを確認する。
20. 別contextに保存完了が届く代表操作で、現在表示中の兄弟のページや選択が勝手に再採用されないこと、そのcontextを通常表示した時は最新rating・編集結果が反映されることを確認する。
21. 既存immediate子を使う代表経路で、親最小化・子visibleの表示を確認する。親子とも最小化した場合の通常描画抑止と、最小化子のScreenshotはheadless／offscreen回帰を主証拠とする。

厳密な期限超過中のClose、receiptとratingの競合順序、UI呼出し0回は非対話回帰で検査する。利用者へdeadline発生の手動再現を求めず、保存失敗や設定破損も作らない。エージェント実行は既存の明示承認・使い捨てportableの規則に従う。

第4.5版の実機代表確認:

22. active／passiveの別窓を用意し、一方を通常のbarの×またはタスクバーのCloseで終了する。もう一方の表示・入力を維持し、rootや兄弟を操作した後にも閉じた窓が再作成されないことを確認する。parkedの終了は既存操作で作れる代表一例に絞る。
23. ローカル閲覧窓を残した状態でrootを最小化し、Remoteから操作権を取得する。PCを復元せずローカル閲覧窓が終了し、取得後の一覧表示・ページ送り・保存が応答することを確認する。返却後は既存の一覧へ戻り、閉じた窓を復元しない。
24. 別窓を閉じた直後に同じ本を明示的に開き直し、新しい窓が維持されることを確認する。別窓を残したroot終了も代表一例で確認し、通常×のtray置換と明示終了の既存差を維持する。

Full／LogicOnly、GC前後、discard再pass、古い出力、同じViewportIdの新寿命、deltaの厳密な順序は非対話回帰を主証拠とする。期限超過やGC競合の手動再現を利用者へ求めない。実機手順のために追加repaintや製品内の試験用タイマーを追加しない。

エージェントによる実行は、既存の明示承認・使い捨てportable・実データ保護の規則に従う。今回の読み取り調査では製品を起動しておらず、上記を確認済みとは扱わない。

第4.6版の実機代表確認:

25. bookmark一覧から別窓の本・mediaを開き、mainのselection／scrollを変えてから閉じる。一覧の更新と現在位置の維持を確認する。mediaでは別ファイル／folderへ移動してから閉じ、最後の実フォルダと選択へ戻ることも確認する。
26. auto-open対象のZIP／PDFまたは画像folderをstandaloneで開いて閉じ、従来の親一覧へ戻ることを確認する。変換アーカイブではキャッシュfolderへ移らないことを確認する。
27. 別窓を閉じた後、mainで別navigationまたは新bookmarkを開始し、旧帰還で巻き戻されないことを確認する。Remote取得・返却後も閉じたviewerや旧bookmark帰還を復活させない。

厳密なLogicOnly・discard・immediate置換・upload submission順は非対話回帰を主証拠とする。利用者へdiscard競合の手動再現を求めない。実機試験のためのタイマー・追加repaint・設定破損を作らない。エージェント実行は既存の明示承認・使い捨てportableの規則に従う。

### 7.5 動画HUD・GPU

1. 再生、HUD文字、時間、速度、音量、track。
2. 820px程度から狭幅へ縮小し、重なり・menu・seek／音量drag。
3. pause／resume／seek／track変更／動画→音声→動画。
4. F11／F12往復、focus、透明合成。
5. 比較wipe／diff、panorama、縮小のmipmap・黒表示・境界。
6. fullscreenから一覧へ戻り、黒サムネ・atlas破損なし。

### 7.6 マルチモニターDPI

1. 100%と150%／200%間でmain／F12／動画窓を移動。
2. 字面・baseline、候補窓、HUD hit rect、slider／seek位置。
3. 最大化・復元・F11往復で操作が画面外へ失われない。
4. 子終了後もrootの表示・入力・撮影が継続。

## 8. 完了条件と文書更新

完了条件:

- 新版graphとvendor全差分を説明できる。
- egui-winit保持理由・削除条件・Disabled取消互換を記録している。
- 未消費RawInputを同じviewportだけが消費し、root／child交互実行とchild skip後復帰の回帰が通る。
- RawInput所有修正について構造合意とdetached計画§11記録がある。
- IME中断要求のviewport別所有と入れ子passについても構造合意とdetached計画§11記録がある。
- 子Closeの非描画受付・消費・既存終了ownerへの合流・backend／Context退役について構造合意とdetached計画§11記録がある。
- 最小化した子へのCloseが、復元や子callback実行を必要とせず既存終了ownerへ届く。
- modalで拒否した要求はその一回で消費され、modal終了・復元後に再発火しない。新しいCloseは受け付ける。
- rootもlogic-onlyの場合に承認済み子を退役でき、後のroot／兄弟Fullで再作成しない。
- 子終了で兄弟の入力・IME・保留RawInput・texture・surface・寿命を変更しない。
- wg​​pu／glowで同じClose配送・消費・退役契約を満たし、rootの終了・tray契約を維持する。
- 同一Contextの親だけ／子だけが中断要求を出す実入れ子run_uiで、要求の消失・混入がない。
- 実run_uiのdiscard再passで、最新のIME矩形等を採用しながら未配送中断要求を保持する。
- 集約済み出力を配送した後、次の新規runへ中断要求が残らない。
- serviceがUI省略時にも必要なwakeで進み、古いContext入力を操作として読まない。
- SW_HIDE／最小化／visible child／期限超過を区別する。
- 受信済み離散キー、Win32終端、mouse-nav pendingの既存契約を保持する。
- gamepad未配送edge・意味終端が複数logicで失われず、保持軸の復帰後継続を維持する。
- IME取消がproducerから実TextEditまで到達する。
- native IME中断要求が一度だけ正しいHWNDへ届き、出力上書き・旧hostからの漏れがない。
- 通常backend／nativeとも、生成・集約済みの正しいviewportのIME出力だけを配送する。
- release射影の順序保証と本家分類が分離され、連続クリックの操作差が採択されている。
- texture deltaのdrain／引継ぎ／明示終了・submission・freeが全経路で成立する。
- 汎用入力フィルタ、一律prune、復帰時一回捨て、旧クリックタイマーを追加していない。
- Closeを処理するための追加repaint、最小化子へのUI pass強制、新しいclose pending bool・復帰待ちqueue・時間窓を追加していない。
- IME中断要求を補修するApp側の補助フラグ、追加repaint、新しいqueue、retryを追加していない。
- Ctrl+Shift clipboardを通常／nativeで保持する。
- accessibility通過後、全snapshotを更新・通常比較・目視確認する。
- 非対話gate、確認用build、必要な承認付きdesktop試験、利用者実機確認が完了する。
- 未実施・環境不成立を確認済みと書かない。
- リリース済み機能・設定・データを黙って縮小・初期化しない。

更新対象はdisplay-pipeline、video-architecture、coding-conventions、keymap-spec、ime-process-keys-plan、ui-snapshot-policy、development-build-and-test、interactive-release-verification、detached-rework-plan §11、vendor台帳、backlog §1.382。

§1.380は個別解決の証拠があるものだけ更新する。採択した外観・操作差はマニュアルと製品ページへ反映し、版更新だけを理由に未確認の改善・互換性を告知しない。

第4.3版の追加完了条件:

- 利用者採択の案Bが正式vendorへ接続され、Windows最小化の要求済みFullでroot UI／deferred子callbackが進む。
- UI実行とsurface描画が分離され、minimized／visible情報、実表示ACKを偽装しない。
- 要求なしのsleep、既存100 ms下限、outer phase、bootstrap／resizeの既存契約を保持する。
- 非描画Fullでもtexture delta・platform output・viewport commandsを完結し、Cut／Copy／Pasteを描画待ちにしない。screenshot等の明示描画要求は失わない。
- wg​​pu／glowが同じ方針を満たし、他OSのocclusion等を一括変更していない。
- RedrawRequested期限超過のLogicOnlyが残り、その間もC1・送信済み保存・取消・寿命終端が進む。
- C2・C4・C5とC3の表示用pollは既存owner・旧順序のuiへ残り、期限超過中の結果は次の対象Fullで一度だけ採用する。
- F13の表示継続とG1〜G5のframe依存を、関数名だけでlogic-readyと扱っていない。
- D3の通常／IndexOnly要求IDが独立採番となり、取消・再開始やcontext mountで再利用されない。cutoff ACKは採用後に行う。
- 案Bを理由にviewport別RawInput、child skip保持、非描画Close、gamepad・IME・delta・surfaceの既存回帰を撤去していない。
- 案Aの通常最小化中の表示保留仕様、共通表示待ちqueue、新pending bool、時間窓、追加repaint、logicからの強制UIを導入していない。
- scheduler方針変更と残る期限超過経路、C1〜C5の責務境界について、設計担当・独立レビュー担当の整合確認とdetached計画§11記録がある。

段1b検証台帳へ入口・receiver・採用・取消・ACK・mount ownerの対応を追記する。`docs/web-remote-plan.md`では単一writer、取得時の全ローカル閲覧終了、返却後の非復元を保持する。案Bの採択を負荷同値・実機確認・追加構造レビューの完了証拠へ読み替えない。

第4.4版の追加完了条件:

- root Closeの判断が通常キー入力・dialog描画から分離され、logic直後のbackend終了確定へCancelCloseを返せる。
- UIなしでtray置換、保存待ち、settings-family、ZIP取消、完了後の一度だけの終了継続が成立する。
- modal拒否とoperation延期、通常×と明示Quitを区別し、旧ui入口との二重受付を残していない。
- 保存receiptのresume記録とeditor状態更新はmount不要のowner付き更新を優先し、元context終了後もauthoritative receiptを失わない。
- 必要な非描画mountはpayload交換・owner更新・復元だけを行い、往路・callback・復路から表示再採用へ到達しない。
- 未反映rating・page-editのgeneration／stampを非描画アクセスで消費せず、次の正当なUI mountで従来どおり採用する。
- 別contextのreceiptと未反映ratingを使う回帰で、保存は進むがfullscreen再open・UI decision・兄弟表示再採用が起きない。
- 両backendのimmediate実入口が、子自身のmetadataと明示描画要求を使う。
- 親最小化・子visible、親子最小化、最小化immediate子Screenshotの回帰が成立し、非描画でも出力を完結する。
- 後付けの表示抑止guard、logic-mode flag、第二registry、第二Close queue、新pending bool、追加repaint・retryを追加していない。
- 案Bについて成立した構造合意を維持し、追加のroot lifecycle・非描画mount・immediate子描画の範囲と理由を§11へ記録している。

段1b台帳のO6・F6はroot lifecycleへの接続へ更新する。C1／C3／F13の留保には、callback本体だけでなくmount前後の表示副作用を含める。段1b-2のgamepadのみの並行実装へ追加3件を混ぜず、検証証拠とファイル所有を分ける。

第4.5版の追加完了条件:

- 候補(B)の具体的な権限移譲・配送・cleanup契約について、設計担当と独立レビュー担当の構造合意があり、detached計画§11へ範囲と理由を記録している。
- Appの終了判断後、binding解除前に、exactな対象登録寿命と子孫への承認済み退役を一度だけ出力へ移譲する。
- App runtimeやClosingをbackend ACK待ちで延命せず、終了後のViewportId再検索・再承認を必要としない。
- 承認済み出力が子のGC前に既存root command列へ保持され、Full／LogicOnly／pass外hookの全経路からpass外consumerへ届く。
- immediate入れ子とdiscard再passで承認が消失・二重適用されず、親passの途中にContext退役を行わない。
- backendが次のApp処理へ入る前に承認済み退役を完結し、surface描画、次Full、追加repaint、Close完了hookを必要条件にしない。
- 旧出力で退役済み寿命を再作成せず、同じViewportIdの新寿命を旧command・builder・callbackから区別する。
- Contextの通常GCが先でも、native／IMEを含む残存資源が終了する。通常GCと承認済み退役のcleanupが共通化されている。
- wg​​pu／glowで、共有deltaの先行移譲、対象限定のnative／Context／IME cleanup、drop後の追加delta引継ぎ、root終了時の完結が成立する。
- root lifecycle、modal拒否後の新規Close、非管理viewportの委譲、兄弟資源、parked-liveの存続を維持する。
- mode changeの空immediate UIと退役目的のVisible(false)が撤去され、logic／uiから同じ承認済み退役入口を使う。
- mode change、active／parked／passive終了、Remote、watcher、barの×、OS／programmatic Closeの対応が台帳にある。native presenterの独自HWNDは既存終了ownerを維持する。
- C1がlogicへ正式接続され、未配送の退役がある間にFinishAcquireしない。RemoteActive／DrainingRemoteにローカル閲覧窓を残さない。
- 新しいpending bool、復帰待ちqueue、時間窓、強制UI、retry、退役済みIDの別ラッチを追加していない。
- 第4.5版の追加回帰と、段3台帳の既存未達gateが正式graphで完了し、今回の追加差分について独立レビューを受けている。

更新対象に、段3検証台帳、vendorのPATCHES／差分台帳、App終了入口とbackend退役の対応表を加える。段0〜段2のACCEPTと`df0037eb7`のcore check成功は履歴として保持し、今回の受渡し契約・実行時挙動の検証完了へ読み替えない。

第4.6版の追加完了条件:

- Close前に帰還先と所有値を存続mainの既存navigation帰還ownerへ型付きで移し、teardown後もbookmark一覧更新・最後の実フォルダ復帰・auto-open親一覧復帰が成立する。
- 帰還boolの型付き化を既存slot・context bundle・消費位置へ一貫して接続し、第二の帰還slot／queueを作っていない。
- active／standalone／parkedのoriginを保持し、無関係なmain・新bookmark・兄弟を古いCloseで変更しない。
- Remote取得、新navigation、新bookmarkのadmissionで旧帰還を失効・置換し、帰還UIをRemoteの完了条件にしていない。
- 既存UI・早期return・native即時Closeの帰還動作、選択・scroll・帰還先を保持している。
- 実discard再passの直前だけ未配送deltaを移譲し、旧whole／partial→submission→後発freeを両backendの所有境界で保証する。
- 最終pass・上限到達・direct begin／end・LogicOnlyの出力所有を変更せず、移譲済みdeltaを二重配送していない。
- integration配送scopeのContext一致、設置・解除・復元・Weak参照・test隔離が回帰で確認されている。
- output pluginの観測回数・順序を保ち、texture先行配送を実表示ACKへ変換していない。
- epaintの新vendor、metadata再演、再預託API、第二renderer ownerを追加していない。
- dirty採用の既存補修と第4.5版の型付き退役・pass外cleanupを維持している。
- 新pending bool、第二queue、遅延cleanup、追加retry、時間窓、強制UI、後付けguardを追加していない。
- 終了継続とdelta所有の追加範囲について構造合意・detached計画§11記録があり、独立実装レビューの残る2件のP1を解消している。

更新対象に、段3検証台帳、navigation／bookmark帰還ownerの対応、vendor PATCHES、delta配送の所有図を含める。本家報告候補は未改変本家での再現状況と分けて記録し、利用者の判断前に送信しない。

第4.5版の設計ACCEPT、`fe32f2e88`のコンパイル・既存filter成功、今回の追加契約の受入証拠を区別する。今回の読み取り調査を実行時確認やテスト成功として記録しない。
