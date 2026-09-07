# 音楽ビデオ: 編集の整合性修正

開始: 2026-09-08、`musicvideo` / `849c5a9d2`。
Status: 対象修正の実装・独立レビュー・全体テスト完了。確認用ビルドを準備中。
実装・自動検証・実機確認の結果は別々に記録する。

正本: [music-video-export-plan.md](music-video-export-plan.md)。
前回監査: [music-video-handoff-20260907.md](music-video-handoff-20260907.md)。

## 1. 対象と担当

ユーザー承認の次工程は、保存・Undo・歌詞・選択状態の整合性修正。
ラボの短い実機操作確認を成立させることを目的とする。
H3 の解析時刻列・較正入力整合、時間軸 zoom/pan、重複キーの選択 UI、
再生 shortcut、編集済み Project の MP4 導線、P4 本体統合は後続工程として残す。

- 主担当 Astra / 高: 要件・設計判断、進行管理、文書の単独編集。
- Sol / xhigh（素材担当）: 主要前提の検証後、保存・選択・素材・読込のコードとテストを編集。
  矛盾は編集前に根拠付きで主担当へ戻す。
- Sol / xhigh（歌詞担当）: 並行段階では新規 `musicvideo-ui/src/lyrics_editing.rs` だけを編集。
  既存 UI への接続は素材担当の該当ファイルを固定し、主担当が編集権を移してから行う。
  Cargo の実行も調整し、同じファイルの同時編集とビルドの重複を避ける。
- 独立 Astra / 高: 設計段階と実装後のレビュー。指示への適合だけでなく設計自体の誤りも対象。
- 旧 brief の ClaudeCode 統括・検収・コミット指定は、この工程では上記体制に移行する。
  AGENTS.md の検証・データ保護・非同期処理・変更範囲の要件は維持する。
- 未追跡 `out.mp4`、別系統の変更、master は変更しない。push しない。

## 2. 修正が必要な不変条件

### 保存とプロジェクト切替

保存の入力 Project と保存先は要求時に固定する。A の保存中に B を開いても、
A の成功・失敗通知で B の保存先や編集状態を変更しない。
session の識別と、その中の編集 revision は分ける。
同じ session の保存中に新しい編集があっても、その保存先は有効であり、新しい編集は保存済みと扱わない。

読込中の旧 Project に対する編集、Undo、素材/LRC 完了の扱いを一つの状態遷移で決める。
読込成功時に、読み込み開始後の編集を無言で失わない。
読込失敗・取消では旧 Project と履歴を保持する。

### 履歴と素材

Project と、その時点の画像・音声内容を同じ履歴の単位として復元する。
パスが同じでも内容が異なる差し替えを区別し、Undo 時に外部ファイルを読み直すだけで代用しない。
PCM・波形・再生対象・画像・解析/較正状態は、復元された資源と一致させる。

差し替え、画像追加、レイヤー削除/復活、読込、LRC のすべての入口を確認する。
遅延完了が Undo 済みの追加を復活させたり、別 session の同じ ID に適用されたりしない。
失敗で有効な素材や編集履歴を失わない。
別のホスト専用 Undo stack を並走させず、既存の連続操作のまとめ方と 128 段上限を保つ。
素材の実体は共有参照で保持し、履歴の枝切り・上限・session 終了後には不要な参照を解放する。
処理中の preview/export が参照する素材を、現在の編集で上書きしない。

### 歌詞

現在の `ReplaceLyricsText` は新しい各行へ旧行番号の時刻を付けるため、
時刻付き A/B の前へ X を挿入すると X に A の時刻、A に B の時刻が移る。
行挿入・削除・本文訂正で、別の歌詞へ既存時刻を無言で付け替えない。
自由な複数行入力、空行による消去区間、タップ、LRC 読書き、Undo/Redo を維持する。
繰り返し歌詞・同じ空行は、新旧全文の比較だけでは編集位置を断定できないことを前提にする。
IME と TextEdit 自身の取り消しを含めて、操作に由来する対応または明示的な曖昧性解決を設計する。

### 選択

一覧クリック・追加・並べ替え・削除・タイムライン選択・Undo/Redo・読込で、
選択レイヤーと key/lyric/drag の所属を一貫させる。
別レイヤーを選んだ後、旧歌詞の inspector が残って別の対象を書き換えない。
選択遷移を共通の入口へ集約し、呼出し側ごとの解除漏れで対処しない。

## 3. 検証と引き渡し

回帰テストは実際の状態遷移・完了受理・入力 handler を通す。
遅延順序はテスト側で制御し、sleep に依存させない。
画像は画素、音声は PCM/波形、履歴は選択と資源参照まで検査する。
資源の枝切り・上限解放、同じパスの内容変更、失敗・取消・別 session への切替も対象とする。

関連 crate のテストから始め、共有機構への影響に応じて全体 gate を実行する。
fmt と UI 文字列を変えた場合の glyph 検査を通す。
実装完了後にラボの release ビルドと `scripts/build-dev.ps1` を実行し、起動はユーザーへ引き渡す。
エージェントは通常版の本体や実設定を起動・変更しない。
実機では素材差し替えと Undo/Redo、歌詞の行挿入、レイヤー切替、保存・再読込を短く確認する。

## 4. 設計決定・結果

### 保存・切替・選択（設計レビュー済み、実装中）

- 保存要求に session identity、要求時の保存内容、保存先を持たせる。
  同じ session で後続編集があっても保存先を受理するが、保存済み baseline は要求時の内容とする。
  dirty 判定に単調 counter の一致だけを使わない。保存した A → 編集 B → Undo A は clean、
  A 保存中 → B → Undo → 別枝 C → A 保存完了は dirty になることを検証する。
  選択状態は保存内容に含めず、非同期の古い要求を判別する世代は Undo で巻き戻さない。
- 読込開始時の旧編集状態を記録し、完了まで変更がなければ新 session へ交換する。
  変更があれば読込結果を保持し、旧編集を残すか読込結果へ切り替えるかを画面で選べるようにする。
  読込中のすべての操作を無効にすることで解決しない。
  開始後の変更検出は JSON の dirty 判定と分離し、素材 revision の変更、Undo/Redo、
  未完了の素材操作も含む編集世代で判定する。同じ path の素材再取得を見落とさない。
- 選択レイヤーとその詳細（なし / key / lyric）を一つの owner へ集約する。
  詳細が別のレイヤー ID を重複保持する構造を避け、レイヤー変更時の遷移を共通化する。
  親は安定した `LayerId` を保持し、一覧 index は導出する。削除後に隣接レイヤーへ詳細を
  付け替えず、並べ替えでは同じレイヤーの詳細を維持する。最初の実装レビューで index 所有の
  残経路が見つかったため、この不変条件に沿って修正する。

H2 の素材保管・メモリ budget と、歌詞の編集範囲 / TextEdit Undo は
この先行塊に含めず、以下の具体案を独立レビューして順に実装する。

### 素材の一時保持と保存形式の境界（設計レビュー済み）

immutable な素材保持は、session 内の Undo/Redo と処理中の preview/PNG 出力に適用する。
既存 `.mivmv.json` は外部素材へのリンク形式を維持し、今回、素材同梱形式へ拡張しない。
保存は元の素材ファイルへ書き込まない。再読込ではその時点の外部素材を読み、欠落通知も維持する。
したがって、外部で削除・変更した素材を session 内で Undo できることと、
保存後の再読込で以前の素材内容を再現できることは別の契約である。
未保存表示は保存する Project の内容差で判断する。同じ path の内容再取得では JSON に差がなくても、
runtime の素材 revision が変わるため Undo の一段として扱う。

### H2 の保管方式（条件付き設計レビュー済み）

履歴専用の decoded cache の予算を 0 とし、過去素材の canonical RGBA / PCM は
worker が作る private な一時 blob で保持する。履歴は内容を識別する manifest/lease と Project を
同じ snapshot に格納する。current・ready・実行中処理が必要な decoded 内容は pin する。
これは総 RAM 使用量の上限ではなく、履歴が decoded 内容を積み上げない契約である。
property だけの変更は同じ manifest を共有し、素材の再解決を行わない。

新素材は blob 保管の成功を確認してから適用可能にする。保管失敗で旧素材を失わない。
cold Undo は blob を worker で復元し、別の素材の画素や PCM を復元先へ流用しない。
復元の実行数を制限して、連続 Undo/Redo で巨大な読込 worker を増殖させない。
完了は session と要求 manifest の一致で受理し、復元中の最新 Project 編集や選択を上書きしない。
資源変更で新しい Assets bundle を作り、既に実行中の preview/PNG の bundle は変更しない。
最後の大きな decoded 内容の解放と blob 削除は worker へ委譲する。

共有機構への変更は `musicvideo-core/src/assets.rs` の資源所有 API に限定する。
フォントと画像の共有参照から独立した Assets bundle を作れるようにし、
lab の preview/PNG が開始時の内容を保持するために使う。
既存の raster/MP4/CLI 呼出し側への型・所有の影響を core テスト、CLI check と全体 gate で確認する。
Project の JSON schema と描画計算、`music-core`、本体の再生・viewport 機構は変更しない。

新素材の要求開始は Project/履歴を変更しない。画像の追加も、読み込み中は未確定の要求として
表示し、成功時にレイヤー・参照・資源を一括で確定する。通常の property 編集は継続できる。
完了は対象の所有を確認して最新 Project へ適用する。

未完了の要求は Undo/Redo の対象ではない。Undo/Redo は最初の操作から通常履歴を移動し、
その境界で未完了素材要求を失効させる。取消だけで Undo/Redo を消費しない。
読み込み中には明示的な取消操作を置く。取消済みの読み込みを Redo が再実行することもない。
例: property A を編集 → 素材読込開始 → Undo は A を戻して読込取消 → Redo は A だけ復元する。
履歴なしの読込取消は Project と履歴の両方を変えない。

cold restore 失敗では logical な Project/履歴と復元用 lease を保持し、未解決状態を表示する。
誤った旧素材を表示・再生せず、再試行または別の Undo/Redo で復帰できるようにする。

独立した store / editor を本体へ複数組み込んでも識別子が衝突しないよう、素材 lease と
DocumentSession の identity は process 全体で一意に発行する。対象レイヤー・画像は ID 値だけで
非同期完了を受理せず、削除して同じ ID を再生成した場合にも異なる lifetime で判別する。
匿名 blob の File clone は seek cursor を共有するため、同じ blob の seek/read 全体を一つの
mutex の所有範囲に置く。独立 store、別 session、同一 blob 並行復元を回帰テストに含める。

接続レビューで確認した状態境界:

- cold restore 中に新しい画像・音声が成功しても、一つの slot だけで全体を Ready としない。
  Project と最新 manifest を確定し、必要な全素材を揃えてから runtime を Ready にする。
- 取消は pending の receiver を即座に捨てず、実行中の処理を drain するまで owner を保持する。
  新しい要求は最新一件として待たせ、取消→開始の反復でも worker 数の制約を維持する。
- cold restore 中・失敗中は旧 preview texture を表示対象から外す。無音 PCM を入れた
  再利用用 stream の存在を、音声の準備完了や timeline 位置の正本と混同しない。
- 復元完了は一件だけでなく、同じ poll に到着済みの結果を drain して最新の採否を決める。
- 終了時も receiver owner ごと cleanup worker へ移す。先に空まで drain してから UI で
  receiver を捨てる方式では、その間の send による大きなデータの UI 解放を防げない。
- LRC 読込は対象歌詞の要求時 baseline を保持する。同じレイヤーの本文・時刻が変わった場合は
  読込結果を Ready として保持し、反映または現在の歌詞を維持する選択を求める。
  他レイヤーや無関係な property の編集では確認を要求しない。
- Project/LRC の完了採否は、その frame の editor/timeline 入力と既存 Ready 選択の処理後に行う。
  完了と入力が同じ frame に届いても、入力を編集世代/pending に反映してから切替を判断する。
  新しい Ready の確認表示は repaint を要求して次の frame に表示する。

### 歌詞の行対応（独立調査後の設計方針）

通常編集は custom TextBuffer で Unicode char 範囲の splice を記録する。
delete と insert の途中状態だけで時刻を失わないよう、同じ pass / IME 編集を一つの操作として解釈する。
本文と行 identity・時刻を分けて持ち、次の境界を固定する。

- 一行内の訂正は、本文全体の消去と再入力を含めてその行の時刻を維持する。
- 行頭の `X\n` 挿入は X を未設定にし、既存行の identity と時刻を後ろへ移す。
- 行内分割は先頭 fragment に元の時刻を残す。完全行削除はその行の時刻と一緒に削除する。
- 時刻付き行の本文を空にしただけなら、空行の消去 cue を維持する。
  空文字だけでは「0 行」と「空行 1 行」を区別できないため、本文だけから決めない。
- 複数の時刻付き行を結合して時刻が競合する場合だけ、保持する時刻を画面で確認する。

egui の native TextEdit Undo/Redo は `replace_with(全文)` となり、通常 splice と区別する。
有界の行対応情報（sidecar）から意味を復元し、候補間で対応が異なる場合・記録外の場合は
明示確認する。全文・cursor の一致だけで過去行を断定せず、時刻を黙って消す fallback も入れない。
sidecar は native TextEdit の意味対応情報であり、別の編集 Undo stack を実装するものではない。

本文 Undo は、存続する行に後から設定した時刻を巻き戻さない。削除行の復活ではその行の
削除時情報を使う。全体 Undo は Project と行 identity を同時復元する。
widget ID と sidecar は session / layer の寿命に所属させ、別 Project の同じ layer ID から
前の歌詞を取り消し復元しない。LRC による外部置換も同じ境界へ接続する。
通常入力・IME で毎回確認を要求せず、実際の API で前提を満たせない場合は実装前に設計へ戻す。

確認待ちでも、存続する行への最新の時刻修正を resolve で上書きしない。
本文の外部置換・全体 Undo・session 切替では待機中の提案を失効または再評価する。
ただし、非同期 Project/LRC 読込の完了は、それより後に入力した確認待ちの草稿を
無確認で破棄しない。Project は旧値のままでも、EditorState の実 pending から導出した
query を host が参照し、既存の Ready 選択へ送る。LRC は対象レイヤーだけを確認する。
取消は本文/identity だけでなく native TextEdit の cursor と undoer も操作前へ戻す。
`TextEditState::clone()` は undoer を Arc 共有するため、必要な pass で独立した undoer コピーを保持する。
全体 Undo は current identity を復元し、残存する native 履歴から到達する既知対応は有界に保持する。
LRC や全体 Undo のたびに widget ID を変えて native 履歴を消す回避は行わない。

sidecar から古い候補を破棄した後も、残った同本文の候補だけで対応を断定しない。
native Undoer の独立コピーから到達可能な本文を観測し、対応を失った本文の印をその寿命に沿って
保持・回収する。長い入力 session の後にすべての本文 Undo が恒久的に確認待ちになる方式は採らない。
native の内容を観測できない場合は安全側の確認に留める。UI には内部の履歴機構名を出さず、
変更前後の歌詞と保持する時刻を比較できる候補・取消を当該歌詞欄に表示する。

pass 中の意味対応と native 保存点の保持は別の寿命として扱う。同じ pass の途中に現れた
本文も Undo/Redo の候補として扱い、実際に到達できる保存点を入力回数の FIFO だけで捨てない。
post-pass の到達情報による回収は編集の確定後に行う。確認待ち/取消では pre-pass の候補・
不確実性・tombstone を保持し、native rollback だけで古い Redo を復活させない。
LRC の外部置換も context-aware な共通入口で直前の native 履歴を観測し、pending rollback を処理する。

## 5. 実装・検証記録

### 保存・選択の先行修正

コミット: `4ba3b7932`。独立 Astra の最終再レビューで追加指摘なし。

- 保存の状態遷移を `musicvideo-ui/src/document.rs` の `DocumentSession` に共有化。
  lab の保存 worker 完了 handler が、要求時の `SaveCheckpoint` を渡して受理する。
- 選択は `Option<LayerId>` を親に、layer を重複保持しない key/lyric 詳細を持つ。
  初回レビューで見つかった index 再解釈の経路を修正し、削除・並べ替えも回帰テストに追加。
- Sol 実行: `cargo test -p musicvideo-ui` 44 passed、`cargo test -p musicvideo_lab` 26 passed。
  `cargo fmt --check` と `python scripts/check_ui_glyphs.py` は exit 0。
  レビューで並べ替え後の同じ描画 pass 内に残る古い index も修正し、UI 44 件を再確認。
  全体の最終検証・実機確認は別途記録する。
- この先行塊には、読込開始後の編集を保護する切替確認と、素材/歌詞の修正は含まれない。

### 素材履歴の共有 API 基盤

lab の接続から分けて、`musicvideo-core/src/assets.rs` と共有 UI の基盤を先行確認した。
コミット: `91326ca8d`。
独立 Astra の最終レビューで追加の修正要求なし。lab の非同期接続と歌詞の承認は別に扱う。
Sol 実行: `cargo test -p musicvideo-core` 72 passed、`cargo test -p musicvideo-ui` 50 passed、
`cargo fmt --check -p musicvideo-core -p musicvideo-ui` は exit 0。
歌詞 module の登録を外した基盤だけの状態で実行した。

- `ResourceManifest` / `ResourceLease` を Project と同じ snapshot に保持する。
  128 段の履歴上限による最終 lease 解放、Project と資源の Undo/Redo を回帰テストに追加。
- lease / DocumentSession は process 全体で採番し、別 store / editor の衝突を防ぐ。
  非同期対象には削除・再生成を区別する lifetime を付ける。
- `Assets` は既存の挿入・参照 API を維持し、必要な画像とフォントを共有した独立 bundle を
  作成できるようにした。fork で scaled/text cache・inflight・待機条件は共有しない。
  既存 repo のフォント利用は読み取りであり、Arc 化後も同じデータを読む。
- この基盤だけでは lab の H2 は解消しない。部分復元中の素材追加、取消後の worker 所有、
  古い完了の drain、preview の切離し、音声なし transport、終了・失敗の処理を接続側で検証する。

### lab の素材・読込接続

コミット: `0c24a453e`。
Sol 実行: `cargo test -p musicvideo_lab -- --nocapture` 44 passed / 0 failed、
`cargo fmt -p musicvideo_lab --check` exit 0。独立レビューの初回指摘を修正した後の再実行である。
独立 Astra の最終再レビューで新たな blocking 指摘なし。

- 匿名 blob の同一 lease 並行復元、同 path 別内容、別 store の revision、元ファイルに
  依存しない画像・PCM 復元を確認。
- host handler で Undo と pending 取消の同時処理、ABA 拒否、復元中の画像/音声追加、
  stale と最新の完了 drain、後発 property 編集の保持、失敗・再試行を確認。
- project/LRC 読込中の対象変更は明示確認まで結果を保持。再生 stream の有無だけで
  音声なし timeline の位置を上書きしない。出力初期化失敗では具体的な通知を維持する。
- receiver owner と大きな decoded 内容の cleanup worker への移譲を追加。
  復元 worker の切断後は再生成して再試行し、再生成失敗は未解決状態として通知する。
- この結果は headless の自動確認であり、Windows 実音出力・実 IME・目視の検収とは分ける。

### 描画 snapshot の追加回帰

コミット: `c80326788`。
`assets::tests::replacing_an_image_in_a_fork_does_not_change_a_pinned_render_snapshot` を追加。
無変調の 2×2 Project と明示 RGBA の画像を使い、`evaluate` / `render_frame` の出力で検査する。
新 bundle で同じ ImageId の画像を交換しても、処理中の旧 bundle の RGBA は完全一致し、
新 bundle の RGBA は異なること、両方に未解決素材がないことを確認した。
Sol の narrow test は 1 passed、core fmt check は exit 0。最終 gate の core は 73 passed。

これは今回定義した fixture の回帰であり、過去報告 `077ad307e7676c3b` の入力・コマンドを
再現した結果ではない。旧ハッシュの再取得は未確認のまま区別する。

### 歌詞の行対応と入力・読込境界

旧 `ReplaceLyricsText` の行番号による時刻再付与を撤去し、Unicode の編集範囲と行 identity に
基づく `LyricsTextBuffer` へ移行した。Project JSON の `LyricLine` schema は変更していない。
共通 snapshot には Project と行 identity を同時に格納する。本文訂正・挿入・削除・空行 cue・
IME・native TextEdit Undo/Redo と共通 Undo の境界を別々に検証した。

行の結合で時刻が競合する場合や、過去本文との対応が確定できない場合は、変更前後と候補を
歌詞欄に表示する。明示確定までは Project を維持し、取消で native cursor/undoer も復元する。
他レイヤー・再生・共通 Undo は続けられる。非同期読込はこの未確定草稿も保護する。

- Sol 実行: `musicvideo-ui` 87 passed、統合後 `musicvideo_lab` 45 passed。
- 実 `draw_editor` / TextEdit のイベント回帰は、先頭貼付、IME 置換、長入力後 Ctrl+Z、
  pending の取消・確定と query、共通 Undo 後の本文 Undo、identity の同時復元、
  layer/session 分離、同本文 LRC の pending 取消と cursor 復元を含む。
- 180 行・320 回の変異入力＋200 idle frame の handler は debug/headless で約 1.58 秒
  （全 draw 込み平均約 3 ms/frame）。通常曲を模した局所的な入力負荷確認であり、
  実ウィンドウ/GPU/音声付きの性能測定や実 IME の検収ではない。
- 最終段階で `reconcile` の非公開化による lab の E0624 を検出。公開 wrapper は観測不能な
  native 履歴を Unknown と扱い、内部/context 付き経路は fresh な観測を保持する形へ修正。
  再度 UI 87 / lab 45 件と fmt を確認してから全体 gate へ進んだ。
- 独立 Astra の最終再レビューで、H1/H2・歌詞 identity/時刻・選択の今回の freeze 範囲に
  未解決の blocking 指摘なし。実機確認、H3 の解析一致、P4 は承認の対象外。

### 最終検証（進行中）

ログ保存先: `target/musicvideo-edit-integrity-verification/`。

| 検証 | 結果 |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、12.29 秒。`cargo-check-mimageviewer-core.log` |
| `cargo check -p mimageviewer --features dev-tools --bin mvexport_cli` | exit 0、11.41 秒。`cargo-check-mvexport-cli.log` |
| `cargo test -p musicvideo-ui` | 87 passed。`cargo-test-musicvideo-ui.log` |
| `cargo test -p musicvideo_lab -- --nocapture` | 45 passed。`cargo-test-musicvideo-lab.log` |
| `cargo fmt --check` | exit 0。`cargo-fmt-check.log` |
| `python scripts/check_ui_glyphs.py` | exit 0、dangerous glyphs なし。`check-ui-glyphs.log` |
| `.\scripts\test-full.ps1` | exit 0 / PASS。本体 lib 7,498 passed / 33 ignored、UI snapshot 45 passed、core 73 / UI 87 / lab 45 passed、vendor egui-wgpu 9 / eframe 15 passed。`test-full.log` |

本体の既存 warning は残るが、上記 check に error はない。歌詞 UI に依存しない両対象を先行確認した。
依存ソースが変わらなければ同じ check を繰り返さず、検証済み HEAD から確認用ビルドへ進む。

## 6. 修正後の短い実機確認

この工程では完成した一本を作る前に、以下を小さい試験用 Project で確認する。
自動テストで判定した内容と、実音・IME・操作感の結果を分けて記録する。
ラボの起動対象は `target/release/musicvideo_lab.exe`。P4 未実装なので、本体 core から
この編集画面へ入る導線はまだない。ビルド完了時の起動手順は検証結果とともに追記する。

1. 音声と画像を開き、見分けられる別画像・別音声へ差し替える。Undo/Redo で表示・音・波形が
   対応して戻ること、連続操作しても途中の素材が遅れて復活しないことを見る。
2. 歌詞 A/B/C に時刻を付け、先頭へ一行追加し、中間行を削除し、本文を日本語 IME で訂正する。
   残った行の時刻、空行の消去区間、本文入力中の Ctrl+Z とフォーカス外の共通 Undo を見る。
3. 歌詞の行・キーフレームを選択してから別レイヤーへ切り替え、並べ替え・削除・Undo を行う。
   選択表示と右ペインの編集対象が一致することを見る。
4. 試験用の別名で保存して開き直す。大きめの Project の読込中に編集できる場合は、切替確認で
   「現在の編集を続ける」を選び、その変更が残ることを見る。非同期の競合自体は handler テストで
   固定し、ユーザーが短い時間窓を繰り返し狙うことを検収条件にしない。

操作感については「どの操作をしたかったか」「どこで手が止まったか」を優先して記録する。
次の UI 改善では、時間軸の拡大・移動、キーの発見と選択、現在時刻と選択キーの関係、
レイヤー一覧と編集ペインの面積配分を、この実用結果に沿って具体化する。

後続 UI 設計で確認する具体的な条件:

- 現 timeline は曲全体を横幅へ当てはめ、左ラベルに 190 px を使う。例えば表示時間軸が
  800 px の 5 分曲では約 0.375 秒/px となり、微調整には拡大表示が必要。
  任意の短い区間へ移動し、編集後に曲全体へ戻れることを受け入れ条件にする。
- 同時刻・近接したキーを個別に選び、対象 property と時刻を確認してから動かせること。
  選択キーの時刻と再生位置が異なる場合も、どちらを書き換える操作かが見えること。
- lab の下部 panel は既に resize 可能だが、timeline 内の行領域は最大 190 px である。
  panel を広げた分が編集対象の一覧に使えるかを実用確認し、pane の単なる大型化で済ませない。

これらは今回の整合性修正へ追加実装する条件ではなく、ユーザー確認後の UI 改善の入力である。
