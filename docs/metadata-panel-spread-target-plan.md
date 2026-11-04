# §1.712 情報パネルの見開き対象

## 原因と不変条件

右パネルの情報タブは、見開きでも各セクションが直接`fullscreen_idx`を読んでいた。
表示中の相方を選ぶ入口がなく、相方のAI生成情報・EXIF・ツイート・外部メタデータを確認できなかった。
情報の切替は表示内容の選択であり、読書位置・ページ送り・画像の配置・補正の対象を変更しない。
タグと類似は従来どおり表示中の両ページを扱う。本の★・本のAI説明は本のまま扱う。
キーボードによる★も従来の対象を維持し、情報タブのページ★だけを選択対象へ向ける。

## 所有と選択規則

- 既存`SimilarPanelState`は情報／類似のタブと表示状態を一つのviewer contextに所有する。
  この共通パネルowner内の`MetadataInfoTarget`へ、既存items世代・現在のanchor key・見開きmodeと
  ペアのpage key・読む順先頭・選択側を保持する。新しいApp fieldや第二のcontext交換経路は作らない。
- 画面上の左右と既定側は`resolve_visible_spread_edit_pages`と`AdjustSpreadTarget`の純粋な
  側／index解決を再利用する。LTRは左、RTLは右が読む順先頭。末尾に添える補助表紙があるときは
  Navigation側が先頭で、左右ボタンでは補助表紙も選べる。
- `open_fullscreen`は情報の明示選択を解除する。ページ送り・再openの既定は新しい組の読む順先頭。
  連結読みのseekはadmission／保留判定後の実anchor変更時、スクロールのreanchorは
  admission後の実表示単位変更（同indexのslice変更も含む）時に、既存の退役処理と同じ境界で
  所有contextの情報ownerを退役する。Info非表示／類似タブのまま往復しても旧選択を復活させない。
  同一表示単位へのno-op、拒否／未受理／保留では選択を保持する。表示時の追加照合には頼らない。
  同じ組で情報／類似のタブやパネルを閉じ開きしても選択は保持する。
  items世代、anchor key、pair key、modeまたは読む順先頭が変わると選択を初期化する。
  単ページ・表紙単独・横長単独では切替を出さず、単ページへ移ると左右の選択を捨てる。
- 隠れたパネルでもSingle→Spread・方向・shiftの往復で選択が復活しないよう、
  mode採用（通常・構成管理本・popup）、方向採用、shiftの実変更／解除、編集用Single入場で
  情報ownerだけを退役する。no-opのmode／shift解除では維持する。
  局所編集も共通Single入場を通り、通常／局所editorの復帰は旧情報選択を復活しない。
  source／設定hydration・snapshot復元・構成投影の再採用は既存openとitems世代で照合し、
  別の世代counterやreset購読経路を作らない。
- 末尾補助表紙と表紙直後／末尾の単ページ規則の実変更も、既存の本別／全体設定の採用境界で
  情報ownerを退役する。全体設定のparked配布は既存のFollowGlobal／対象側条件を共有し、
  補助表紙の情報選択退役はactive／parkedとも`spread_mode.has_cover()`を必要とする。
  FollowGlobalでも表紙なしLtr／Rtlのcontextは選択を保持し、本別On／Offも全体変更から保護する。
  情報選択の対象条件だけを修正し、既存geometry／cache失効は変更しない。単ページの「置き方」だけは
  ペアのページ集合を変えないので、この追加退役の対象にはしない。
- タブ先頭へ編集パネルと同じ`egui::Button::selectable`の「左ページ」「右ページ」と
  「対象: 左ページ／右ページ」を置く。クリックした同じ描画passで対象indexを確定し、
  ★・パレット・画像AI説明・ツイート・AI生成prompt・EXIF・外部メタデータをそのindexから解決する。

## 単純化の検討

`adjust_spread_target`の共有は、情報を見る操作が補正パネルや保存スロット適用の対象を変えるため不採用。
情報と編集の独立した利用目的を残し、左右判定のコードだけを共有する。
別途の左右bool、読み込み待ち対象、復帰待ち状態は足さない。一つの表示単位に選択を束縛し、
組が変わったら既定へ戻すことで古い選択の追従・復帰の組み合わせをなくす。
設定保存やmodal化は不要な表示選択である。

## 読み込みとライフサイクル

情報側の初回対象／クリックの変更時だけ、`ensure_metadata_load`から既存`start_metadata_load`へ選択indexを渡す。
既存`MetadataLoadPending`へ要求keyを持たせ、情報側のensureは同じkeyのin-flightを再起動しない。
明示openのstartは従来どおり同じkeyのin-flightも取り消して読み直す。
別keyは既存のcancel/channelで置き換える。結果は要求keyのcacheへ採用されるため、
旧ページの内容を選択中のページへ流用しない。結果send後は`request_repaint_of(ROOT)`で
既存root pollを起床する。保存したegui Contextが後で子viewportを描画していても宛先を変えない。
新しいworker、同期ファイル読取り、画像decode、GPU upload、DB読取りは追加しない。
パレットは既存の取得処理へindexを渡し、画像AI説明は既存の共有cache／visit／workerを使う。
本のAI説明は従来のcontainer ownerを維持する。
contextのmount／park／fork／dropとメタデータworkerの取消は既存ownerが担う。
detached述語・viewport routing／生成／identity／geometryは変更しない。
popup callback内の1行は表示mode採用で同contextの情報選択だけを退役する。
判断と範囲は[detached計画§11](detached-rework-plan.md#11-リワーク外からの変更記録)にも記録する。

## 検証

明示回転の実変更 (`apply_rotation` の既存 cache 採用) でも、同じ context の情報選択を既定へ戻す。
同じ回転値の再採用では保持する。自動の寸法確定や一般の `landscape_epoch` では解除しない。
無関係な先読みページの寸法判明によって、利用者が選んだ左右を変えないためである。
隠れた情報タブでの R/L 往復と、同値の no-op を回帰で検査する。

`metadata_info_target_tests`で左右／RTL・全ページ情報の対象・補正と読書位置の不変・
組／方向／単ページ／再openの初期化・末尾補助表紙・ボタンクリックを検査する。
独立レビューREJECTのP2追補では、LTR／RTL×縦／横連結×seek／reanchorについて、
閉じたInfo／類似タブのまま別の組へ移動して戻り、旧選択が復活しないことを検査する。
同値／未受理の保持、slice変更の退役、別contextの選択保持も検査する。
P3追補はactiveの表紙なしLtr／Rtlと、parkedの4見開きmode×FollowGlobal／On／Offの12条件で、
affected／unaffectedの情報選択と既存geometry失効を独立に確認する。
既存のglobal補助表紙／page-alone多窓fixtureで、影響するparked ownerだけが退役し、
overrideと対象外ownerが保持されることも検査する。page-aloneの既存設定保存試験は、
保存失敗とno-opの選択保持、成功と隠れた往復での選択破棄を追加する。
snapshotは純粋な共通selectorと情報タブの固定fixtureを1枚追加し、生成画像の目視後に通常比較する。
実行結果は下記と開発検証台帳に記録する。製品起動・build-dev・コミットは利用者指示により未実施。

初回`UPDATE_SNAPSHOTS=1 cargo test --offline -p mimageviewer --lib metadata_info_target`は
新規fixtureの`open_fullscreen`へ`HistoryTrigger`を渡し忘れたE0061の1件でexit 101。
テストは未実行であり製品のテスト失敗には数えない。引数を訂正し、セルフレビューと独立レビューで
確認したensure限定dedup・ROOT起床・隠れたパネルの採用境界・items世代照合をまとめてから再実行した。

最終snapshotは`metadata_info_target_selector.png`の1枚、340×300、16,264 bytes。
生成と更新指定なしの通常比較は各7/7成功。実装担当と独立review担当が目視し、
左右selector・対象・ページ★・Prompt全文の欠けがないことを確認した。
関連試験と全体gateの結果は[開発検証台帳](next-release-backlog.md) §1.710〜§1.712に集約する。
今回関連はfeature付き133件、default-featureの全件内でも130件がすべて成功。
一方、全体gateは75失敗・21timeoutで未通過。代表の単独対照3件は成功したが、
元の全件結果や原因未確定を上書きせず、今回差分に対するcode-review受入と区別する。

再compile後の初回7件は6成功／1失敗（exit 101、`target/metadata-info-first-seven.log`）。
末尾補助表紙fixtureが`current_folder=None`のため、既存の
`spread_complete_book_context_eligible`が本全体の表示と認めず、Doubleの期待に失敗した。
通常のtemp folderを設定して既存fixtureの前提に揃える。製品の補助表紙の不具合ではない。
同じ区切りで合意した補助表紙／page-aloneの採用境界とcross-context回帰を加え、
snapshotの下端でPromptが切れていた高さ200を300へ直してから、生成と通常比較を行う。

### 独立レビューREJECTのP2／P3追補（2026-11-05）

完成差分の独立gpt-6.1-sol／xhighレビューは追加指摘なし・コード受入可。
指定どおり`TEMP`／`TMP=G:\home\tmp-test-env`で関連12件を実行し、
コンパイルは成功した。selector snapshotの通常比較1件は成功したが、残る11件は
共通fixtureの一時ディレクトリ作成時にOS 5（アクセス拒否）で停止し、製品経路は未実行。
指定TEMPはこのセッションの書込許可範囲外にあり、別TEMPやfixture変更による回避は行わない。
同じfixtureを使う追加filterは未実行。回帰成功は未確認で、書込可能な環境での再実行が必要。
実行コマンド・時間・ログは[backlog §1.712の追補](next-release-backlog.md)へ記録した。

前回の全件75失敗・21timeoutについては、設計担当が旧TEMPの約77万件の残骸に起因すると確認し、
空のTEMPで同じ1519件の全成功を確認した、と今回の利用者指示で共有された。
上の「原因未確定」は当時の記録として残す。今回の全件ゲートは設計担当が別途実行する。
