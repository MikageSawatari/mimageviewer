# 動画シークバー近傍のストリップ (サムネイル列 ⇄ 音声波形)

backlog [§1.102](next-release-backlog.md) (YouTube 型のサムネイル列) と
[§1.113](next-release-backlog.md) (音声波形への切替) の**共通の正本**。2 件は 1 つの UI の
モード切替として設計する。着手の経緯とレーン運用は
[briefs/session-lane-b-video-strip.md](briefs/session-lane-b-video-strip.md) と
[next-cycle-work-lanes.md](next-cycle-work-lanes.md)。

作業ブランチ `video-strip` (worktree `C:\home\mimageviewer-video-strip`)。

実装状況 (2026-08-26): Increment 1 (軸・窓・gesture の純ロジック) / Increment 2 (サムネイル抽出
worker) / Increment 3 (App owner・設定・描画・入力・HUD region・tile と hover preview の排他、
**実機確認済み**) / Increment 4 (波形モード・モード切替) / Increment 5 (3 値の表示状態、
フィルムアイコン、再生位置追従、180 秒波形ラスタのスクロール) / Increment 6 (フィルムボタンの
1 クリック巡回、ストリップ内切替の撤去、左右パネルのホバー帯境界修正) / Increment 10
(cursor とセルの左端基準を実機確認) / Increment 11 (pending / ready / failed のセル表示と、
補助 decoder を開けない場合の strip 全体 notice) / Increment 12 (長さ情報のない動画の案内と
操作抑止、波形可視 span 設定、無 blank 再構築、長尺 first-paint 実測) / stage-1 follow-up
(raw GOP が疎すぎる素材の decode 前 unavailable 判定、batch の sampled duplicate skip) / D18
(ホイールによる段階的な範囲変更、現在値表示、設定 UI の段階値化) / D19
(ストリップ固定) / D17 (シークバーと両ストリップ mode の共有 1 枚プレビュー) / D27
(メモリ内の粗い全尺ピーク列) まで実装。
残りは §9 の未確定項目の実機調整、
MPEG-TS など `TimeGrid` 経路の実素材確認。

確定した既定値: 開閉は `Shift+S` (`V` はレーン C の動画パノラマ用に空けた)、最小間隔 15 秒、
抽出幅 320px、波形は 1 画面 3 分、セル 152x104pt。

**未解決**: 特定素材で末尾 4 セルが黒いという利用者報告を再現できていない。HW / SW とも、
独立した復号でも黒くならなかった。輝度・分散・channel range を記録するようにしたので再発時に掴める。

### 第 2 段の一括検証の結果 (2026-08-25、22,811 ファイル / 4 時間 26 分)

`D:\home\18` + `E:\share\18`。生データは `C:\home\miv-batch-runner\sweep-stage2.json`
(BOM 付き。`encoding='utf-8-sig'` で読む)。

| | 件数 |
| --- | ---: |
| 通過 | 16,692 |
| 失敗 | 303 (失敗セル 985) |
| 利用不可 (キーフレームが疎) | 546 |
| skip (壊れて開けない) | 8 |

判定対象に対する通過率 98.2%。失敗セルの理由は `no matching frame` 730 /
`seek failed (av_seek_frame -1)` 180 / `decoder unavailable (decoder not found)` 75。

- 軸別の失敗ファイル: `time_grid/incomplete_index_coverage` 241、`keyframe_index` 48、
  `time_grid/too_few_index_entries` 12、`time_grid/index_unavailable` 2。
- 拡張子別の失敗率: `.mpg` 4.4% / `.mkv` 1.6% / `.avi` 1.5% / `.wmv` 1.4% / `.mp4` 0.4%。
  **古いコンテナに寄っている。**
- **失敗した 303 件はすべて最終的に SW 復号へ落ちている** (HW のまま失敗したものはゼロ)。

**最多の `no matching frame` 283 件は、2 つの別問題に割れる** (2026-08-25 に JSON を分解して確定。
「格子時刻に許容内のフレームが無い形が中心」という当初の見立ては、下の A のほうが正確):

- **A: 末尾セルだけ失敗 — 246 件 (87%)。→ D23 で修正済み。** 原因は復号ではなく、**コンテナが
  申告する尺が実際の中身より長いこと**。実測 (06jademarx04rocker.wmv) で duration 118.019 に対し
  実際に復号できる最後のフレームは 116.866 で、**1.153 秒ぶん中身より長く申告している**。格子は
  その区間へセルを置き、最終セル 118.000 は許容 1 格子 (1.000 秒) をわずかに超えて外れていた。
  軸種別は `time_grid` 239 / `keyframe_index` 90 の**両方**で出るので、TimeGrid 固有ではなく
  軸に依らない境界の問題。
- **B: それ以外 — 37 件**。`.mpg` 16 / `.mp4` 12 が主。最悪例は duration 40 秒に対し
  **keyframe_count 3・index_last_secs 1.0・coverage 2.5%** の mp4 で、索引が先頭 1 秒しか無いため
  D21 の判定どおり `incomplete_index_coverage` → `TimeGrid` へ落ちるが、**索引がほぼ無い素材では
  TimeGrid でも目的時刻のフレームを見つけられない**。D21 の判定自体は正しく働いており、
  TimeGrid 側の限界。診断は `last_frame=none` (1 枚も復号できていない) で見分けられる。
  「索引がほぼ無い素材は事前に unavailable と断る」方向の判断が要る (D21 の最大 GOP 基準は、
  索引が密なまま早期で終わる素材を拾えない)。

**D23 (実素材修正、2026-08-25、利用者判断)**: **stream が本当に終端に達したとき、残ったセルには
最後に復号できたフレームを充てる** (`accepts_final_frame_at_end_of_stream`)。そのセルが表して
いるのは「動画の終わり」であり、動画の最後のフレームを出すのが正しいという判断。空セルにする案・
軸を切り詰める案は却下 (末尾が空くと壊れて見える)。

- 前方向へは伸ばさない。目的時刻より後のフレームは通常経路が拾うので、ここで拾うと別の場面を
  持ってきてしまう。
- **読み取り / 復号エラーで途切れた場合は充てない** (`first_demux_error` / `first_decode_error` が
  無いときだけ)。truncate された読み取りは「動画の終わり」ではない。
- 失敗理由 `NoFrame` は型付きの `NoFrameReason` を持つようになった (終端で落ちたか / 目的時刻 /
  直近に復号できたフレームの PTS / 前後の許容幅、ミリ秒整数)。batch がこれを出すので、
  「フレームを弾いたのか、1 枚も来なかったのか」が後から分かる。
- **効果 (実測)**: 旧 sweep で失敗した 303 件を再検査して **272 件 pass / 19 件 fail**。
  `no matching frame` は **283 件中 267 件が解消**。残りは `last_frame=none` (1 枚も復号できない
  = 上の B) 13 件、`decoder not found` 2 件、`seek failed` 1 件、`sws_scale Input changed` 1 件。
  なお旧 sweep の `seek failed` 18 件は再検査で 17 件が clean になったが、**今回の変更は seek に
  触っていない**ので、これは run 依存 (cold / warm など) の可能性が高い。別途確認すること。
- **未確認の推測**: 「末尾 4 セルが黒い」という旧報告は、この構図で説明が付く可能性がある。
  申告尺が中身より長い素材では末尾の複数セルが同じ最終フレームに解決するので、フェードアウト
  する動画なら同じ黒フレームが並ぶ。**再現できていないので断定しない。** この修正で最終フレームを
  充てるセルは増えるため、同じ見え方が出やすくなる点は意識しておく。


**残りの 2 種** (いずれも実素材で診断してから直すこと):

- `decoder unavailable` (75 セル / 2 件) — コーデックが無い素材。ストリップ以前に再生もできない
  可能性が高い。キーフレームが疎な素材と同様、**ファイル単位で事前に断る**のが筋。セルごとに
  失敗を並べる形にしない。
- `seek failed` (180 セル / 18 件) — 古い AVI / MPG に集中。索引が無い / 壊れている素材。

**着手順 (当初)**: A → B → `decoder unavailable` → `seek failed` → D17 → §5.4。
**A は D23 / D26 で解消。残り 19 件は利用者判断で Fix とした** (2026-08-26、下節)。

### 実素材 sweep の打ち切り (2026-08-26、利用者判断)

**残り 19 件は直さずに Fix とした**。22,811 件中 19 件 (0.08%) で、しかも
「ファイル末尾への seek そのものができない」類で、ストリップ以前の素材側の問題だから。
内訳は B 13 件 / `decoder not found` 2 件 / `seek failed` 1 件 / `sws_scale Input changed` 1 件 /
その他 2 件。一覧は `C:\home\miv-batch-runner\remaining-failures.txt`。

**再開するなら**、B の失敗セル数が多い上位 2 件を batch で個別に診断するところから。
**推測で直さない。** 実素材はここまで 6 件連続で別々の原因を出しており、着手前の見立ては
6 件中 1 件しか当たっていない。`seek_strip_batch` の診断で理由を出してから直す。
自前の簡易復号 (`tools/seek_strip_probe`) はアプリが失敗する素材で 2 回とも通っている。

黒いセルの報告は**未再現のまま**。D25 の 12 秒診断が仕掛けてあるので、次に出たら
`%APPDATA%\mimageviewer\logs\mimageviewer.log` に理由が残る。

§5.4 (波形の絵巻き全長化) は D27 として実装済み。100ms / 60 秒 chunk のメモリ内列を
現在中心優先で埋め、30 分超は未解析区間を区別しながら progressive に描く。
**D17 は実装済み** (2026-08-25 に範囲を両モードへ拡大)。上の「着手順 (当初)」は D17 を
実装する前に書いた並びなので、そこだけ古い。残りは §9 の U2〜U5 の実機調整、
MPEG-TS など `TimeGrid` 経路の実素材確認。

**ブランチの状態 (2026-08-26)**: `video-strip` は master を取り込み済み。衝突は 3 箇所で、
いずれも両側が同じ場所へ独立な物を足しただけだったので両方残した (Cargo.toml の workspace member /
`native_video.rs` の型定義 / `render_core.rs` の per-frame 状態)。計測だけは順序に意味があるので、
`egui_run_t0` を `run()` の直前、`egui_run_ms` を直後に置いてある。lib test 6498 件 green。

### 検証ツール

```powershell
C:\home\miv-batch-runner\seek_strip_batch.exe --json <folder> [<folder>...]
```

`dev-tools` feature の bin (`src/bin/seek_strip_batch.rs`)。**アプリの実ワーカーと軸解決を駆動する。**
永続キャッシュを使わない (`None`) ので利用者データを汚さず、キャッシュヒットが失敗を隠さない。
pass / fail / unavailable / flat / duplicate を分けて数え、失敗があれば非ゼロ終了。
**exe と FFmpeg DLL を worktree 外へ写しておくと、Codex のビルドと並行して回せる。**

## 1. 何を作るか

動画のシークバーから**上へドラッグしたときだけ開く帯 (ストリップ)**。中身は
**サムネイル列**と**音声波形**をモードで切り替える。常時表示のサムネイル付きシークバー
ではない。

```
        ┌──────────────────────────────────────────────┐
        │           サムネイル列または音声波形          │
        │ ┌───┬───┬───┬───┬───┬─│─┬───┬───┬───┬───┬───┐│
        │ │   │   │   │   │   │ │ │   │   │   │   │   ││  ← 等幅セル、中央 | は固定
        │ └───┴───┴───┴───┴───┴─│─┴───┴───┴───┴───┴───┘│
        └──────────────────────────────────────────────┘
    ============================|=======================   ← 既存シーク行 (24pt)
    [再生] [ループ] [音量] ...                             ← 既存コントロール行 (40pt)
```

- 中央の `|` は**動かない**。スクラブすると背後のセル列が左右に流れる。
- 選択位置 = `|` の直下。クリック / 離した時点でその時刻へシークする。

## 2. 確定した決定 (2026-08-23、利用者判断)

| # | 決定 | 補足 |
| --- | --- | --- |
| D1 | **ラッチして開いたまま**にする | ドラッグ中だけの一時表示にしない。表示内容を切り替えられること、波形をじっくり見られること、タッチでも扱えることを優先 |
| D2 | モードは**永続設定**に保存、**既定はサムネイル列** | `Settings.video_seek_strip_mode` |
| D3 | サムネイル列の横軸は**キーフレーム番号軸・等幅** (案 I) | 「前後のサムネを見たい」が要望の主眼。セル幅が伸び縮みして隙間になるより、先のサムネが見えることを取る |
| D4 | 抽出は**キーフレームのみ**。精密抽出はしない | 精密は 1 枚 1 秒級で、11 枚並べると 10 秒以上待たされる。キーフレームは数十 ms。**待ち時間を作らないことが最優先** |
| D5 | 波形モードは**時間一定速度**でよい | モード間でスクラブの進み方が違うことは許容する |
| D6 | v1 は**本体のみ** | mIV Remote と parked-live (入力抑止中の別ウィンドウ) は対象外。detached リワーク領域の allow-list を触らない |
| D7 | 波形は**窓オンデマンドで即応させ、キャッシュを持たない** | 窓だけ解析すれば待ちが出ないと分かったので、永続キャッシュ自体を作らない (§5.4 は必要になってから) |
| D8 | セルは**位置に応じて連続に流れる**。セル単位にスナップしない | `center_index` の小数部をそのまま描画オフセットに使う |
| D9 | ストリップに**時刻表示を足さない** | シークバー側に既にある |
| D10 | ドラッグを離したら、**線形補間した時刻へ精密シーク**する | ドラッグ中はシークしない。離した 1 回だけ本編を動かす |
| D11 | キーフレームは**間隔が一定以上あくように間引く**。しきい値は環境設定 | N 枚おきではない。D18 の段階値から選び、既定 15 秒。大きくすると大雑把なシークがしやすくなる |
| D12 | ストリップの状態は**「なし / サムネイル / 波形」の 3 値ひとつ**。open bool + mode の 2 本立てにしない | `Shift+S` はこの 3 値を巡回する。CLAUDE.md「相互排他の状態を複数の bool で持たない」に沿う |
| D13 | 下部バーのロックアイコンの左に**フィルムのアイコン**を置き、1 クリックごとに **なし → サムネイル → 音声波形 → なし**を巡回する | OFF は非アクティブ表示、サムネイルはアクティブなフィルム、音声波形はフィルム上に**ベクター描画の音符**を重ねる。プルダウンとストリップ左上の切替は置かない |
| D14 | **再生中はストリップが再生位置に追従してスクロールする** | 止まったままだと再生が進むほど内容がずれて違和感が出る。ドラッグ中だけ追従を外し、離すと再び追従する |
| D15 | ドラッグを離したら、その位置へ精密シークして**再生を始める** | §9 U1 をここで確定。一時停止中に離した場合も再生を始める |

D12 は D1 / D2 を置き換える (ラッチ開閉と mode 設定を 1 つの 3 値にまとめた)。
D15 は「離す前の再生状態を保つ」という当初の既定を置き換える。

**D18 (実装済み、2026-08-25)**: **ストリップ上のホイールで範囲を段階的に変える。現在値は右上に表示する**
(利用者要望 2026-08-25)。「大まかに場面を把握したい」用途の方が多い、という判断による。

- サムネイル (最小間隔): 0.1 / 0.2 / 0.5 / 1 / 2 / 5 / 10 / 15 / 30 / 60 秒
- 波形 (1 画面の範囲): 5 / 10 / 15 / 30 秒 / 1 / 2 / 5 / 10 / 15 / 30 / 60 / 120 / 180 分
- ホイールは**永続設定を動かす** (右上表示と一致させ、次回起動でも保つ)。自由入力の
  コントロールは段階リストへ置き換える。上回転は 1 段狭く、下回転は 1 段広くする。旧版由来の
  段階外値は、回した向きにある直近の段へ移る。両端では値を保つ。
- ホイールは native event の時点で背後の項目移動を作らず、strip 側で egui の wheel event と
  scroll delta を消費する。グリッドへ同じ wheel を再転送しない。
- **既定はサムネイル 15 秒 / 波形 3 分** (利用者判断 2026-08-25)。「大まかに場面を把握したい」
  用途が多いので、開いた瞬間から見渡せる側へ寄せる。
- サムネイルは全索引から adopted list だけを作り直し、実 timestamp をキーにした永続 cache を
  保つ (D11)。波形は旧 raster を display-only holdover にして、新しい first paint が届くまで
  表示し続ける。設定変更中に帯を blank にしない。
- **D18 increment では §5.4 を実装しなかったが、D27 で置き換え済み。** 30 分超は中央から
  埋まる粗い全尺ピーク列を progressive に描く。D27 前の窓復号待ち時間は比較基準として §14 に残す。

**D19 (実装済み、2026-08-25)**: **ストリップの右上に鍵アイコンを置き、固定できるようにする** (利用者要望
2026-08-25)。ストリップが見やすいので常時出しておきたい、という用途。

- 固定すると**ストリップの領域を常に確保し、映像はその内側に表示する**。既存の下部バー固定
  (`video_seek_bar_locked` と `bottom_reserved`) と同じ仕組みに乗せる。`bottom_reserved` に
  ストリップ高さを足すだけで、新しいレイアウト構造は要らない。
- **到達できる状態は 3 つ** (利用者判断 2026-08-25)。
  1. 固定なし
  2. 下部 HUD だけ固定 (ストリップは必要なときだけ出す) — **既存の挙動**
  3. 下部 HUD + ストリップ固定 (常時閲覧)

  「HUD 非固定 + ストリップ固定」は作らない。これで組み合わせは増えない。
- **既存の `video_seek_bar_locked` (bool) は変えない。** これは**リリース済み**の設定なので、
  3 値の列挙へ作り変えると利用者の設定 DB にある旧値の移行が要る。代わりに
  `video_seek_strip_locked` (bool、未リリース) を足し、**両方向から**不変条件を守る。

  - ストリップ固定を ON → バー固定も ON にする
  - **バー固定を OFF → ストリップ固定も OFF にする** (利用者判断 2026-08-25)

  片方向だけだと「バー非固定 + ストリップ固定」という到達不能なはずの状態が残る。
  この 2 つを 1 か所の状態遷移として書き、`(false, true)` を表現できないことをテストで固定する。
  移行コードは不要。
- **固定 ON はストリップの表示も含意する** (利用者判断 2026-08-25、当初案を置換)。当初は
  「固定は出ているストリップの領域を確保するだけ」としたが、環境設定で固定にしても何も
  起きないため利用者が不便と判断した。固定 = 常に見えている、という語感に合わせる。
  - 固定 ON で表示状態が「なし」なら `video_seek_strip_last_choice` から復元する。
    既に出ている内容は置き換えない。
  - 固定 OFF は表示状態を変えない (固定を外しただけで見えているストリップを畳まない)。
  - **利用者が自分で閉じたら固定も外れる** (`Toggle` / `DownwardDrag` / `Escape`)。
    そうしないと次の動画でまた開き、閉じた操作が無視されたように見える。
  - **利用者が閉じたのではない close は固定を壊さない**。`Unavailable` (この動画の素材が
    使えない) / `TileModeOpened` / `HudHidden` は表示状態も消さない。消すと「1 本だけ
    素材が合わなかった」ことで以降の全動画の固定が消える。
  - 入口は `Settings::set_video_seek_strip_locked` 1 か所。鍵ボタンと環境設定の
    チェックボックスは両方ここを通る。
- 永続化は既存の `video_seek_bar_locked` と未リリースの `video_seek_strip_locked` の 2 bool を
  保つが、実行時の変更は `VideoBottomLock::{None, BarOnly, BarAndStrip}` だけを通す。
  `(false, true)` は復元時に `None` へ正規化し、移行コードは置かない。
- 領域確保でいう「ストリップが出ている」の正本は presenter に渡った
  `Option<NativeOverlaySeekStrip>` とする。`Some` のときだけ、かつ `BarAndStrip` のときだけ
  `bottom_reserved` に `SEEK_STRIP_HEIGHT` を加える。`Some` / `None` が変わった時点でも
  video visual transform と display-resolution surface の準備状態を更新する。
- 鍵ボタンは 28×28pt。ストリップ右上を基準に、右 7pt・上 4pt の inset で置く
  (`min = right_top + (-35, 4)`)。D18 の現在値は鍵の左へ 6pt 空け、
  `RIGHT_TOP` anchor を `right_top + (-41, 5)` とする。ホイールの範囲変更は鍵上でも有効。
- **鍵 widget はストリップ本体の `interact` より後に登録する。** 鍵はストリップ全面に張った
  `Sense::click_and_drag` と同じ矩形の中にあり、egui は重なった領域では**後から**登録した
  widget にポインタを渡す。先に登録すると本体がポインタを取り、鍵は一度も `clicked()` に
  ならない (実装中に実際に起き、単体の鍵だけを描くテストでは再現しなかった)。本体側は
  press 位置が鍵矩形にあれば seek / drag を開始しないので、どちらか一方だけが反応する。
  これはストリップ全体を描いて両方の位置をクリックするテストで固定してある。

**D17 (実装済み、範囲を 2026-08-25 に拡大)**: **ストリップ上を hover / スクラブしたら、その時刻の
サムネイルを 1 枚、ストリップの上に出す** (利用者要望 2026-08-25)。

- **波形モードとサムネイル列モードの両方で出す** (利用者判断 2026-08-25)。当初は「サムネイル列
  モードでは出さない (セル自体が絵なので重複する)」としていたが、**実機で間隔を粗くすると
  セルだけでは狙った場面に寄せられない**という判断で取り消した。既定 15 秒、最大 60 秒まで
  広げられるので、セルの間を指しているときに 1 枚出る意味がある。
- 見た目と位置はシークバーの hover 1 枚プレビューに合わせる。出す場所はストリップの上。
- ワーカーは既存の `ThumbnailWorker` (最新勝ちの単発) を使う。ストリップ用の窓ワーカーは
  N 枚保持が仕事なので用途が違う。
- ラベル時刻 = ポインタ位置なので、**この経路は「シーク時のズレ許容」設定を使う**
  (§3 の表でストリップ本体が使わないと決めたのとは、性質が違う)。

**下部の並び順 (2026-08-25、利用者判断)**: **現状のまま**とする。上から 映像 / ストリップ /
シーク行 / コントロール行。YouTube 型 (シークバーをストリップの上へ) は検討して**採らない**。

- 上ドラッグでストリップを開く操作の向きが自然 (バーから上へ引くと、その上に出る)。逆順に
  すると引いた向きと出る場所が食い違う。
- シーク行とコントロール行が 1 つの帯のままなので、固定 (`bottom_reserved`)・HWND の
  hit-test region・`HUD_BOTTOM_HEIGHT` がそのまま使える。分断すると全部作り直しになる。
- **YouTube のサムネイル列は時間線形でシークバーと目盛りが一致するが、mIV は D3 で
  キーフレーム番号軸・等幅**。さらに中央 `|` は固定で再生位置は動く。隣接させると「揃っている
  はず」という期待を生み、揃っていないことが目立つ。波形モードだけは時間線形なので一致する。
- 「スクラブしながら絵を見たい」という元の要望は、並び順ではなく **D17 で満たす**。

**プレビューの置き場所は 1 つに集約する (2026-08-25、利用者判断)**: ストリップが出ている間、
**シークバーの hover プレビューは現状「完全に抑止」されている** (`suppress_hover_preview =
video_speed_popup_open || seek_strip_visible`)。位置が `hud_rect.min.y` 基準のままなので、抑止を
外すとストリップ 104pt のうち 90pt を覆ってしまうため。

これを次の形に変える。

- プレビューの基準を **ストリップが出ていれば `strip_rect.min.y`**、出ていなければ従来どおり
  `hud_rect.min.y` にする。どちらも 14pt 空ける。
- `seek_strip_visible` による抑止を外す。速度 popup による抑止は残す (カーソル動線の問題で
  理由が別)。
- **シークバーからでもストリップからでも、同じ 1 か所にプレビューが出る**。シークバーの
  ドラッグ中に絵が見える従来の操作感が、ストリップを開いていても戻る。
- ワーカーは D17 と同じ `ThumbnailWorker` を共有する (最新勝ちの単発)。2 つの面から同時に
  要求が来ることはない (ポインタは 1 つ) ので、owner を分けない。


**D24 (実素材修正、2026-08-26、利用者報告)**: **格子軸 (`TimeGrid`) が「画像間隔」設定を
無視していた。** 2 つの別々の穴があり、片方だけ直しても症状が残る。

- **軸を作る瞬間**: 間隔は `pick_interval(duration, FALLBACK_MAX_CELLS)` で尺だけから決まり、
  `video_seek_strip_min_interval_secs` を見ていなかった。実素材 (172.7 秒 mp4) で
  **1 秒間隔・173 セル**。尺由来の値と設定は**どちらも下限**なので大きい方を採る
  (`time_grid_interval_secs`)。同ファイルで 15 秒・12 セルになる。
- **設定を変えたとき**: `StripAxis::with_minimum_gap` は `KeyframeIndex` を間引き直す一方、
  `TimeGrid` は `self.clone()` で**何もしていなかった**。開き直せば新しい間隔になるが、
  開いたまま変えても一覧が変わらない (利用者報告はこちら)。格子も作り直すようにした。
- そのため `TimeGrid` は **`fallback_interval_secs` (尺由来の下限) を自分で持つ**。持たせないと
  作り直し時に下限が分からず、細かい側へ戻せなくなる (粗いまま固定される)。
- **この動画が格子へ落ちる理由**: 末尾キーフレーム 165.2 秒から終端 172.7 秒までが 7.5 秒で、
  素材内の最大 GOP 5.5 秒を超えるため D21 が索引を不完全と判定する。判定自体は正しい。

**D25 (実素材修正、2026-08-26、利用者報告)**: **共有プレビューの x が、どちらの面から出しても
シークバー上の位置から引き直されていた。** ストリップはキーフレーム番号軸で再生位置中心なので
時間線形の x とは無関係になり、早い時刻を指すと左端へ、遅い時刻なら右へずれた。目的時刻に
**それを出した x を持たせ**、更新は `set_seek_preview_target` だけが行う (2 つの `Option` が
食い違わないようにする)。

**観測 (2026-08-26)**: 「末尾のセルが黒いまま」という報告が 2 度出たが、batch でも利用者の
手元でも再現しない。`maybe_emit_fill_wait` は**全部 ready になったときだけ**出るので、
埋まらなかった側は無言だった。可視セルが **12 秒経っても Pending** なら、セルの状態と worker
status を 1 行だけ出す (`note_seek_strip_visible_pending`)。次に出たら通常ログに残る。

**D26 (実素材修正、2026-08-26、利用者判断)**: **最後のキーフレームは間隔に関わらず必ず採用する。**
間引きは前から順に採るので、末尾のキーフレームが直前の採用より近いと落ち、ストリップが動画の
終わりまで届かず右端が空いていた。実素材 `アカリがやってきたぞっ` は 301 秒だが最終セルが
283.1 秒で、**末尾 18 秒にセルが無かった** (最後のキーフレーム 294.8 秒が直前から 11.7 秒しか
離れておらず 15 秒に満たない)。

- **末尾だけ間隔が詰まるのは承知のうえ**。「間隔が詰まるよりサムネイルが見えない方が影響が
  大きい」という判断 (利用者、2026-08-26)。終わりは利用者が探す目印なので必ず 1 枚置く。
- 間隔が尺より長い素材も、先頭だけでなく末尾を置く (8 秒の素材に 20 秒指定 → 2 枚)。
- 既存テスト 2 本の期待値がこの規則で変わる。弱めたのではなく規則が変わったので更新した。

**B の性質 (2026-08-26 に訂正)**: 当初「索引がほぼ無い素材」と書いたが、**これは 1 件だけを見た
過度の一般化だった**。修正後になお失敗する 13 件を数え直すと、**12 件は `keyframe_index` で
coverage 99〜100%** の健全な索引を持ち、**13 件すべてが既に全フレーム復号へフォールバック済み**
(うち 7 件はハードウェアのまま完走)。つまり「索引が無い」のではなく、**全フレーム復号でも特定の
セルだけフレームが出ない**。ファイルごとの診断が要る。一覧は
`C:\home\miv-batch-runner\remaining-failures.txt`。

**D16 (実装済み、2026-08-24)**: サムネイルのセルは**左端をキーフレーム時刻に合わせる**。
旧実装は `Rect::from_center_size` でセル中央がキーフレーム時刻になっていたが、キーフレームは
場面の変わり目に打たれることが多く、1 セルが表すのは実質 `[kf_i, kf_i+1)` の区間である。
左端合わせにすると、セル内での中央線の位置が「その場面のどこまで進んだか」を表し、狙った
位置へ合わせやすくなる (利用者要望 2026-08-24)。補間の式 (§4.1) は変わらない。描画位置を
半セルずらすことと、ポインタ位置 → セル位置の変換・ヒット判定を同じ基準に揃えることが変更点。

D3 の帰結として、横軸は時間線形ではない。波形モード (時間線形) とは軸の意味が変わる。
**モードを切り替えると同じ x が別の時刻を指す**ことは承知のうえで採用した。両モードとも
中央 `|` の時刻は一致するので、切替時に中央は動かない。

**D17 (Increment 11、2026-08-25)**: サムネイルセルの表示状態を `pending / ready / failed` の
3 値で扱う。`pending` は従来どおり空の枠、セル単位の terminal failure は枠内に
「表示できません」と出す。軸解決後に補助 decoder を開けず列全体が使えない場合だけ、セル列を
「サムネイルを表示できません」という 1 個の notice に置き換える。`ThreadSpawnFailed` は軸失敗、
`Cancelled` は close / 切替 lifecycle として既存経路で処理し、画面へ重ねて出さない。
この判定は軸種別に依存しないため、未実機確認の `TimeGrid` も同じ failure surface を通る。

**Increment 12 の長さ不明素材決定 (旧重複番号 D18、2026-08-25)**: `duration_secs` が finite な正数でない動画は再生可能なまま、
シークバーとシークストリップだけを unavailable とする。動画 info を open ごとに 1 回受け取る地点で
理由を日本語 toast にし、フィルムボタン、上ドラッグ、cycle / toggle / 3 直接状態キーはいずれも
設定状態を変更しない no-op にする。未対応形式とは扱わない。

**D19 (Increment 12、2026-08-25、範囲と既定は D18 で置換)**: 当時の波形可視 span は
30〜1800 秒、既定 60 秒の preference とした。
first paint は可視 span だけ。保持 raster は通常 3 倍だが最大 3600 秒へ制限し、1800 秒設定では
3600 秒 / 2 倍物理幅とする。90 分 decode を避けながら両側 15 分の coverage を残す。

**D20 (実素材修正、2026-08-25)**: `AVIndexEntry.timestamp` は presentation PTS とは限らず、
MP4 では key packet の DTS であることがある。索引時刻を frame PTS と直接比較しない。窓の復号中に
同じ key packet の DTS を索引セルへ対応付け、その packet PTS を presentation target にしてから、
decoded frame は target の nearest-preceding を採る。許容幅は indexed cell では前後それぞれ隣の
raw index entry まで、`TimeGrid` では 1 grid interval とし、隣の場面を越える古い frame は
`NoFrame` にする。1 セルの timestamp 不一致はそのセルだけを failed にし、同じ run の後続セルを
巻き込まない。

**D21 (実素材修正、2026-08-25)**: 索引 completeness の主判定は、**最後の keyframe から動画末尾
までが、素材内で実際に観測した最大 raw GOP 1 個分以内であること**。percentage coverage の
下限と平均 GOP 3 個分 / 5〜30 秒の別規則は重ねない。短尺・長 GOP 素材は完全な索引でも最後の
keyframe が尺の 80% 未満になり得る一方、途中 1/3 で止まった partial index は未索引 tail が最大
GOP を越えるので同じ主判定で拒否できる。境界比較には timestamp 変換の丸め誤差だけを吸収する
guard を使う。`ヨスガノソラ_プロモーション.mp4` (末尾 7.63 秒) と jellyfish 4K HEVC 10-bit
(raw 4 件、最大 GOP 8.34 秒、末尾 6.7 秒、coverage 78%) は `KeyframeIndex`、同じ 4 件が尺の
1/3 で止まる fixture は `incomplete_index_coverage` の `TimeGrid` になる。

**D22 (実素材修正、2026-08-25)**: hmovie.mp4 は index 259 件、duration の 99.5% coverage、
列挙順の逆行 0 件で、index timestamp 非単調という当初仮説には該当しなかった。失敗原因は
AVDISCARD_NONKEY で H.264 の非 key frame を復号しない高速経路が、この素材の参照構造では
decode_slice_header error / NoFrame になったことだった。通常素材では従来の keyframe-only
経路を維持し、その経路が NoFrame / decode / convert failure を返したファイルだけ、成功済み
セルを保持したまま software full-frame decoder を開き、対象 run の 1 つ前の raw index gap から
pre-roll して再試行する。以後そのファイルでは同じ full-frame decoder を再利用する。これは
健全な index を TimeGrid へ落とす axis fallback ではなく、keyframe packet が単独復号できない
ファイル向けの decode strategy fallback である。

**D23 (batch follow-up、2026-08-25)**: サムネイル窓は 30 秒を上限とし、未決着セルを
`WindowTimedOut { timeout_secs }` の typed failure へ確定する。failure reason は settled outcome が
所有し、App は「生成が時間切れです」、batch は `thumbnail window timed out after 30s` として
同じ failure surface から表示する。上限前の `pending` だけが空枠であり、上限後に無言の空枠を残さない。

**D24 (stage-1 follow-up、2026-08-25)**: 完全な index でも最大 raw GOP が 15.0 秒を越える素材は、
`KeyframeIndex / TimeGrid` のどちらにも進まず `Unavailable(KeyframesTooSparse)` とする。判定は index
統計だけで行い、サムネイルと波形のどちらの decoder も開かない。stage-1 再走の unique な正常
keyframe-index 1,164 件では最大 raw GOP の p99 / 最大が 10.43 秒で、15 秒は 44% の余裕を持つ
(maximum adopted gap は p99 12.10 秒 / 最大 12.39 秒)。
唯一の外れ値は 833.43 秒で、従来は 30 秒 timeout になった。初回 hardware window の elapsed / cell
概算 p90 は adopted gap 4 秒以下で 33.5ms、4〜8 秒で 43.3ms、8〜10 秒で 19.8ms、10〜12.5 秒で
23.3ms。4K HEVC は raw 8.34 秒で約 0.2 秒 / cell だった。しきい値は preference で間引いた adopted
gap ではなく raw index gap に固定し、1 cell が高々 1 個の現実的な GOP という契約を守る。
unavailable は既存の duration 不明と同じ open gate / 1 回 toast / disabled film button を使い、
上ドラッグと全 strip KeyAction も設定を書き換えない no-op にする。

**D25 (stage-1 follow-up、2026-08-25)**: batch は `(file size, SHA-256(head 8 KiB + tail 8 KiB))`
が先に検査したファイルと一致するものを `duplicate` として decode せず、一致先 path を記録する。
全ファイル hash や container / codec / GOP 構造による dedupe は行わない。前者は 3.2 TB の全読みを招き、
後者は content-dependent decode defect を隠すためである。`pass / fail / unavailable / skip / duplicate`
は別集計とし、意図的な unavailable と duplicate を defect や pass に混ぜない。

axis resolver は raw index entry を sort する前の列挙順について monotonic / inversion count を
診断へ残す。実際に逆行があれば sort で隠さず、理由 non_monotonic_index_timestamps を伴って
TimeGrid を選ぶ。

## 3. 実現手段 (調査で確定した前提)

| 手段 | 確認結果 |
| --- | --- |
| キーフレーム列挙 | `avformat_index_get_entries_count` / `avformat_index_get_entry` / `avformat_index_get_entry_from_timestamp` が ffi に生成済み。MP4 / MOV / MKV / AVI など索引を持つコンテナは**復号なしで全キーフレーム seek timestamp を即取得できる**。これは presentation PTS とは限らず、MP4 では DTS のことがある (D20) |
| キーフレームの高速復号 | `decoder.skip_frame(Discard::NonKey)` が安全 API にある。窓先頭へ 1 回シークし、非キーフレームを捨てながら前方復号すると、窓内のキーフレームをまとめて取れる |
| サムネイルの永続キャッシュ | `video_tile_thumbs.db` が既に `(path, tile_w, timestamp_ms)` キー・mtime 無効化・batch lookup を持つ。**ストリップ専用の `tile_w` を決めれば同じ表に同居できる** |
| 波形のラスタライズ | `render_timeline_row_image(row_start, row_secs, bins, ...)` が「ある時間窓を 1 本の帯に描く」関数そのもの。ストリップの 1 窓 = 1 行として呼べる |
| 窓だけの波形解析 | `analyze_stereo_timeline` の bin 集計は**窓内ローカル** (peak / rms / band / transient にグローバル正規化が無い)。窓だけ解析しても全尺解析と絵が食い違わない。ただし低域/中域の 1 次フィルタ状態は連続なので**前置き区間 (pre-roll) が要る** |
| 「ズレ許容」設定との関係 | あれは hover 1 枚プレビュー専用の概念 (ラベル時刻 = ポインタ位置、絵がそこからどれだけ離れてよいか)。ストリップは各セルが自分の時刻を持つので**この設定を使わない**。重複しない |

## 4. サムネイル列

### 4.1 軸と座標

状態は実数の **セル位置** `center_index: f64` ひとつ。セルは §4.1.1 で間引いた**採用列**の
要素で、`cell(i)` はその i 番目の時刻を指す。

- セル `i` は `cell(i)` のキーフレームを表示し、幅は一定 `cell_w`。
- ドラッグ開始時の中心 `press_center` と pointer 位置 `press_x` を固定し、各 frame の現在位置
  `pointer_x` から `dx = pointer_x - press_x`、
  `center_index = press_center - dx / cell_w` を求める。frame 間 delta は加算元に使わない。
  **小数部はそのまま描画オフセットに使い** (D8)、セル境界へスナップしない。
  `|` の裏でセルが pointer に追従して連続に流れる。
- 中央 `|` の直下は `center_index` の位置。**選択時刻はセル内で線形補間する** (D10):
  `t = cell(i) + frac * (cell(i+1) - cell(i))` (`i = floor(center_index)`, `frac = fract`)。
- **ドラッグ中は本編をシークしない。** 離した時点で 1 回だけ `t` へ精密シークし、
  一時停止中だった場合も再生を始める (D15)。スクラブ中に本編が飛び回らないので、
  復号も UI も軽い。
- ストリップに時刻ラベルを出さない (D9)。絵と補間時刻が最大 1 GOP ずれるが、**画面に矛盾する
  数字が出ない**ので破綻しない。時刻は直下のシークバーが示す。
- 端では片側のセルが空になる。詰めない (中央 `|` の意味を保つ)。

### 4.1.1 キーフレームの間引き (最小間隔、D11)

**GOP 長は素材で 6 倍以上違う** (§14 の実測: 0.50s / 0.93s / 1.10s / 3.00s / 4.17s)。
キーフレームを 1 枚ずつセルにすると、11 セルが覆う時間が **5.5 秒から 46 秒までばらつく**。
密な素材では 35 分の動画を端から端まで見るのに 1939 セル分ドラッグすることになる。

そこで **採用するキーフレームの間隔が一定以上あくように間引く**。

```rust
StripAxis::KeyframeIndex {
    /// 索引から取れた全キーフレーム seek timestamp (PTS とは限らない)。
    keyframes: Vec<f64>,
    /// 間引き後に採用した添字。セル i が表示するのは keyframes[adopted[i]]。
    adopted: Vec<usize>,
}
```

採用は貪欲に決める。先頭を採り、以後は「直前に採った時刻 + 最小間隔」以上の最初のキーフレームを
採る。1 パスで済み、結果は決定的。

- **N 枚おき (stride) にしない。** GOP が可変の素材 (シーンチェンジで打つもの) では、
  N 枚おきだと採用間隔が不揃いになる。最小間隔なら**局所的な密度に合わせて間引ける**。
- 最小間隔は環境設定または strip 上のホイールで D18 の段階値から変える
  (`Settings.video_seek_strip_min_interval_secs`、**既定 15 秒**、0.1〜60 秒)。
  **大きくすると 1 画面が覆う時間が伸びて大雑把なシークがしやすくなる。**
- GOP が最小間隔より長い素材では、結果的に全キーフレームが採用される。
- 間引いても **抽出はキーフレームのみ** (D4)、セルは等幅 (D3) のまま。変わるのは
  「どのキーフレームを拾うか」だけ。
- 時刻の線形補間 (§4.1) は隣り合う**採用セル間**で行う。
- **設定を変えてもサムネイルの永続キャッシュは無効化されない。** 採用列は常に実 index entry
  timestamp の部分集合で、キャッシュのキーは要求した index timestamp (§4.5) なので、しきい値を上げ下げしても
  既に抽出した絵をそのまま引ける。設定変更時に作り直すのは採用列だけ。

### 4.2 キーフレーム列挙とフォールバック

軸は `StripAxis` として型で持つ。silent fallback にしない。

```rust
enum StripAxis {
    /// コンテナ索引からキーフレーム seek timestamp を全取得できた (案 I)。
    KeyframeIndex { keyframes: Vec<f64>, adopted: Vec<usize> },
    /// 索引が無い / 不完全なコンテナ。等時間グリッドに落とし、絵は直前キーフレーム。
    TimeGrid { interval_secs: f64 },
}

enum StripAxisDecision {
    KeyframeIndex,
    TimeGrid(TimeGridReason),
    Unavailable(StripAxisUnavailableReason),
}
```

**索引は「開いた直後」には埋まっていないコンテナがある** (§14 の実測)。

- MP4 / MOV は `avformat_find_stream_info` の時点で完成している (35 分で 1939 件)。
- **Matroska (MKV / WebM) と ASF (WMV) は 1 件以下**。Cues / index object を遅延パースするため。
  **捨てシークを 1 回撃つと埋まる** (MKV 1→58、WebM 1→19、WMV 0→590、いずれも 0.1〜0.2ms)。
- したがって列挙は「数える → 疎なら `av_seek_frame` を 1 回 → 数え直す」の 2 段にする。
  この捨てシークはストリップを開いた最初の 1 回だけで、以後は不要。
- 2 段目でも、最後の keyframe から末尾までが素材内で観測した最大 raw GOP 1 個分を越えるなら
  `TimeGrid`。通常の末尾 1 GOP は index 軸の最終セルが担う。別の percentage floor は置かない
  (D21)。
- index が完全でも最大 raw GOP が 15.0 秒を越えるなら `Unavailable(KeyframesTooSparse)`。これは
  fallback 軸ではなく「この素材では strip の 1 cell = 高々 1 GOP を安価に作れる」という前提が
  成立しない結果であり、worker は decoder request loop へ入る前に終了する (D24)。
- どちらも**見た目は等幅セル**なので、利用者には区別を見せない。ログと perf event には出す。
- `TimeGrid` の interval はタイルモードと同じ `INTERVAL_CANDIDATES_SECS` から選ぶ
  ([ui_video_tile.rs](../src/ui_video_tile.rs) `pick_interval`)。

### 4.3 抽出ワーカー

`src/video/seek_strip_thumbs.rs` (新規)。既存 2 本のどちらとも用途が違うので独立させる。

| 既存 | 用途 | 合わない理由 |
| --- | --- | --- |
| `thumbnail.rs` `ThumbnailWorker` | hover 1 枚 | 最新勝ちの単発。N 枚を保持しない |
| `tile_thumbnails.rs` `TileThumbnailWorker` | `S` タイル一括 | spawn 時に全 timestamps を固定する。窓が動くたび作り直せない |

新ワーカーの契約:

- 要求は**窓** `(axis, center_index, visible_count, lookahead)`。窓が動いたら最新勝ちで差し替える。
- **既に復号済みのフレームは窓が変わっても捨てない** (プロセス内キャッシュへ入れる)。
- 1 窓の充填は「窓先頭のキーフレームへ 1 回シーク → `skip_frame(NonKey)` で前方復号 →
  届いたキーフレームを順に公開」。1 枚ごとにシークし直さない。
- `KeyframeIndex` の cell timestamp は seek/index domain (MP4 では DTS のことがある)。要求窓内の
  key packet の DTS と PTS を対応付けて presentation target を作り、decoded frame はその target の
  nearest-preceding を採る。許容幅は前後それぞれ隣の raw index entry までに制限する。
- `TimeGrid` は従来どおり直前 frame を採るが、1 grid interval より古い frame は採らない。
  timestamp 不一致で 1 セルが failed になっても cursor を進め、同じ run の後続セルを処理する。
- 復号は本編と別の補助デコーダ (`thumbnail.rs` と同じく HW 優先 / 失敗時 SW フォールバック)。
  本編の D3D lock を奪わない。`LIVE_VIDEO_DECODE_THREADS` には数えない。
- cancel: ストリップを閉じた / 動画が変わった / フルスクリーンを抜けた。

### 4.4 生成範囲

- 見えている枚数 + **進行方向へ 1 画面分**だけ先読みする。全尺の先行生成をしない。
- 窓が確定したら、まず `video_tile_thumbs.db` の `lookup_webp_batch` でキャッシュ分を埋め、
  残りだけ復号要求にする。DB 読みと WebP 復号はワーカー内 (UI スレッドで触らない)。

### 4.5 永続キャッシュ

`video_tile_thumbs.db` を再利用する。`tile_w` にストリップ専用の抽出幅を使い、
`timestamp_ms` にキーフレームの実 PTS を入れる。タイルモード (640px / グリッド PTS) とは
別行になり、混ざらない。

- 既存の mtime 無効化・キャッシュ管理 UI・フォルダ単位の削除がそのまま効く。
- 容量特性: **閲覧した範囲しか作らない**ので、通常は 1 本あたり数百 KB。全編をスクラブし尽くすと
  GOP 5 秒 / 2 時間で 1400 枚前後 (320px WebP で十数 MB)。この性質はキャッシュ管理の説明に書く。

## 5. 波形モード

### 5.1 全尺デコードを待たせない

現行 `ensure_music_analysis` は**全尺 PCM を progressive デコードしてから**タイムラインを作る。
これをストリップの起動条件へ広げると、動画を開くたび長時間デコードが走る (backlog §1.113 の懸念)。

**ストリップは `ensure_music_analysis` を呼ばない。** 代わりに窓だけを解析する。

```
D27 の優先順位:
  1. 粗い列が可視範囲を被覆済み、かつ要求 bin 幅が 100ms 以上 → 粗い列
  2. 可視 span が 30 分以下 → 従来の窓経路
     a. 同じファイルの完成済み TimelineAnalysis があれば窓で切る
     b. 無ければ窓 ± pre-roll だけ音声を復号する
  3. それ以外 → worker-local の粗い列だけを progressive に描く
```

粗い列もストリップを開き 10 分以上の span を使ったときだけ開始し、現在中心から 60 秒ずつ埋める。
先頭からの全尺 decode を起動しないため、見ない残り範囲は動画を閉じれば費用を払わずに終わる。

### 5.2 窓オンデマンド解析

`src/video/seek_strip_wave.rs` (新規) + `audio_decode` に範囲デコードを追加。

- 要求は `(path, window_start, window_end, bin_secs)`。最新勝ち。
- `open_audio_decode` 済みの ictx を窓先頭 − pre-roll へシークし、窓末尾まで復号する。
  ほとんどのコーデックで音声フレームは全てキーフレームなので、シークは安く正確。
- **pre-roll (0.5〜1 秒) を捨てる**。`analyze_stereo_timeline` の低域/中域 1 次フィルタは
  状態を持ち越すため、窓の先頭が 0 状態から始まると立ち上がりが歪む。pre-roll 分の bins は
  返さない。
- `bin_secs` は音楽ビューの 10ms ではなく、**ストリップの画素密度に合わせた粗さ**にする
  (窓幅 / 描画ピクセル数から決める)。長い窓で bins が数十万にならないようにする。
- `beat_grid` は作らない (ストリップでは使わない、全尺が要る処理なので窓では出せない)。

### 5.3 描画とスクロール

**波形は「動かすたびに作り直す」形にしない (D14 の実機フィードバック)。** 窓を作り直すたびに
前のラスタを捨てると、スクラブのたびに波形が一度消えてから出る。利用者が見たいのは
**スクロール**であって再表示ではない。

- 初回はまず**設定された可視 span / 可視幅と同じ物理画素幅**だけを要求して first paint を出す。
  これが表示された直後、同じ中心の保持 span を background で要求する。保持 span は通常 3 倍、
  最大 3600 秒で、物理幅も同じ比率にする。秒 / pixel が一致するので texture 交換で内容は動かない。
- first-paint raster は保持用 upgrade の完了まで表示し続ける。upgrade 要求を Working にしても
  published raster を外さず、失敗時も first paint を残す。初回 → upgrade の間に blank を挟まない。
- 保持用へ交換後、ドラッグ中はその texture の**可視 span 分の部分矩形をずらして描く**だけで、
  再解析も再 upload もしない。
- replacement の先行 margin は可視 span の 25%。60 秒 / 保持 180 秒なら従来どおり中心が
  ±45 秒を外れた時点、1800 秒 / 保持 3600 秒なら ±450 秒を外れた時点で次を要求する。
- 可視 span が 3600 秒以上なら保持 span も可視 span と同じにする。同じ幅の upgrade は要求せず、
  中心が可視 span の 25% を越えて動いたときだけ次の同幅 first paint を要求する。
- replacement 待ち中は、その新 span に可視範囲が収まる限り要求を差し替えない。
  これを境界のヒステリシス latch とし、往復操作で要求が発振しないようにする。
- **新しいラスタが届くまで、古いラスタを描き続ける。** 空白を挟まない。可視範囲が解析済み
  範囲から完全に外れた場合だけ、やむを得ず空白になる。
- preference 変更時は旧 worker と LRU を捨て、旧 raster の Arc と旧可視 span だけを display-only
  holdover にする。新 first paint 到着時に画像と span を同じ frame で交換し、blank / 誤スケールを
  挟まない。
- 再生追従は 100ms 間隔で中心だけを更新する。毎 frame の解析 / upload にはならない。

`render_timeline_row_image` を「窓 = 1 行」として呼ぶ。

- metrics lane (下段の指標帯) はストリップの高さに入らないので出さない。現在の関数は
  `metrics_h` を `max(1)` するので、**metrics 無しを表現できるよう小改修が要る**。
- ラスタはワーカーで作り、RGBA を presenter へ渡す (§6 の経路)。presenter スレッドで
  毎フレーム CPU ラスタしない。
- 窓ラスタは在メモリ LRU (`path` + mtime + 窓 + bin_secs + 画素幅) で持つ。

### 5.4 粗い全尺ピーク列 (D18 で必要になった)

D7 の時点では「窓オンデマンドで即応するので要らない」と判断した。**D18 で範囲の上限が
全尺級 (最大 180 分) になったため、この判断は覆る。** 範囲が全尺へ近づくと、窓ごとの復号は
同じ音声を繰り返し復号することになり、範囲に比例して待たされる。

30 分までは窓オンデマンドで実用範囲 (実測 first paint 3.1 秒)。それを超える段では、粗いピーク列
を作ってそこから任意の範囲を描く。

**D18 increment 時点では未実装だったが、D27 で実装済み。** D27 前の 3 時間 AAC 付き MP4 の
窓復号実測は 60 分 3.120 秒、120 分 6.546 秒、180 分 8.189 秒 (§14)。この旧値を、D27 の
`wave_coarse_chunk` / `wave_coarse_serve` と比較できるよう残す。

**先頭から全尺を舐めない (利用者指摘 2026-08-25)。** 動画は 1GB を超えることがあり、HDD 上の
ファイルを先頭から読み切るのは待ち時間が大きい。**現在位置を起点に、前後へ範囲を広げながら
埋める。**

- 解析済み区間を interval の集合 (coverage) として持ち、描画はその時点の coverage から行う。
  未解析の範囲は「まだ解析していない」と分かる見せ方にし、空白と区別する。
- 埋める順は中央から外側へ。見ている場所が最初に埋まり、そこしか見ないなら残りの費用を払わない。
- 音楽解析の progressive 機構 (`MusicAnalysisMsg::Timeline` の partial) と同じく、
  **埋まりながら見える**ようにする。

**映像ストリームに `AVDISCARD_ALL` を立てる。** 音声だけが要るので、demuxer が映像パケットを
選択対象から外せば、MP4 は sample table で次の音声サンプルへ直接飛べる。読む量が全体の数% に
なり、1GB のファイルでも数十MB で済む (代わりに細かいシークが増えるので HDD ではそこが効く)。
`ffmpeg-the-third` の安全 API に `set_discard` は無いため、索引列挙と同じく `AVStream.discard`
を ffi 経由で立てる。**効果は実測して記録する** (立てた場合と立てない場合の読み出し時間)。

- 以下の D27 より前は mono 5 バイト/bin と保存を想定していたが、D27 で stereo 7 バイト/bin、
  worker のメモリ内だけへ変更した。
- 音楽ビューの「永続 DB はやめて直近 N 曲だけメモリ」方針とは**別物**として扱う。あちらは
  10ms・chroma 込みの重い解析で、こちらは表示専用の粗い列。用途も精度も違う。


**D27 (§5.4 の実装方針、2026-08-26、利用者要望「長い動画で音声の切れ目を探したい」)**:
**粗い全尺ピーク列をメモリに持ち、広い範囲はそこから描く。**

**データ**: `COARSE_BIN_SECS = 0.100`。1 bin は `peak_l / rms_l / peak_r / rms_r / band[0..3]` の
**u8 × 7 = 7 バイト**。計画時の「5 バイト」から増えたのは、`render_timeline_row_image` が
L/R を上下に分けて描き、`band_energy` を色に使うため。mono へ落とすと**粗い列と窓復号で
見た目が変わる**ので、描画側が読む値をそのまま粗く持つ。3 時間で 108,000 bin = 約 756KB。
**永続化しない** (D7 の方針を維持)。動画を閉じたら捨てる。

**被覆**: 全尺を `COARSE_CHUNK_SECS = 60` 秒の chunk に切り、**chunk 単位**で埋める。被覆は
chunk index の bitset (3 時間で 180 個)。区間演算が要らず、合成も判定も添字で済む。

**埋める順**: 現在中心に最も近い未取得 chunk を 1 つ選んで復号し、公開して、また選ぶ。
中心が動けば次の選択から反映される。**先頭から舐めない** (利用者指摘 2026-08-25)。

**chunk-local failure**: 取得に失敗した chunk は coverage とは別の failed bitset に記録し、以後の
選択から外して残りへ進む。構築終了は `covered ∪ failed` が全 chunk を埋めた時点だが、failed は
可視範囲の被覆には数えず、未解析の暗い背景かつ中心線なしで残す。音声 track が無い、または decoder
自体を開けない `Unavailable` だけはファイル単位の性質なので、従来どおり構築全体を終了する。

**前面が常に勝つ**: worker は 1 スレッド・1 decoder のまま。ループの先頭で前面要求を見て、
あれば先に処理する。粗い列の 1 単位は 60 秒ぶんなので、**前面要求が待たされる上限は chunk 1 つ分**。
スレッドも decoder も増やさないので、lock 順序も二重シークも発生しない
(前面の待ちを背景作業で伸ばさない、という原則)。

**どの経路で描くか** (上から順に判定):

| # | 条件 | 経路 |
| --- | --- | --- |
| 1 | 粗い列が可視範囲を被覆済み、かつ `waveform_bin_secs(span, px) >= COARSE_BIN_SECS` | 粗い列から描く (復号なし) |
| 2 | `span <= WINDOW_DECODE_MAX_SPAN_SECS` (1800 秒) | **今までどおり窓復号**。`span >= COARSE_BUILD_MIN_SPAN_SECS` なら並行して粗い列の構築を始める |
| 3 | それ以外 | 粗い列だけで描く。未被覆の範囲は「未解析」として描き、埋まり次第更新する |

- **2 行目があるので、今より遅くなる範囲が無い。** 30 分までは窓復号が実測で実用範囲 (§14)。
- 構築開始は `COARSE_BUILD_MIN_SPAN_SECS = 600` 秒**の span 閾値**で決める。`bin_secs` を
  閾値にすると、同じ段でもウィンドウ幅で構築するかどうかが変わってしまう。既定 180 秒の段では
  構築しない。
- 1 行目に `bin_secs` 条件を付けるのは、粗い列が 100ms より細かい要求には解像度が足りないため。

**未解析の見せ方**: 未被覆の列は**背景を暗くし、中心線を描かない**。無音は中心線が引かれるので、
**線の有無**で「音が無い」と「まだ解析していない」を区別できる。空白にはしない。

**AVDISCARD_ALL**: `AudioRangeDecoder::open` で音声以外の全ストリームに `AVDISCARD_ALL` を立てる。
`ffmpeg-the-third` の安全 API に `set_discard` が無いので `AVStream.discard` を ffi で書く。
`AudioRangeDecoder` の利用者は strip の波形ワーカーだけなので、影響範囲はここに閉じる。
**実測 (2026-08-26)**: 7.0GB / H.264 + AAC の MP4 (コンサート映像) で、60 秒 chunk を 4 つ
復号した合計。**立てると 219 / 220 / 221ms、立てないと 390 / 394 / 404ms** (各 3 回、
page cache が温まった状態)。**1.8 倍速く、1 chunk あたり約 98ms → 約 55ms。**
初回だけは cold cache で ON 345ms / OFF 390ms と差が縮むので、**この数字は I/O ではなく
demux と copy の削減**を測っている。HDD 上の cold read でどれだけ効くかは別途。

**構築速度の実測 (2026-08-26)**: 7.0GB / H.264 + AAC の MP4 で 60 秒 chunk を 10 個。
**1 chunk あたり 177.5ms (復号 56ms + 解析 122ms)**。60 分ぶん (60 chunk) の完全被覆に
**10.6 秒**、3 時間の全尺で **31.9 秒**。

- **重いのは復号ではなく解析側**。粗い 600 bin を作るために音楽ビュー用の解析
  (`analyze_stereo_waveform_cancellable`) をそのまま通しているため。**次に効く手はここ**で、
  帯が使わない指標 (chroma / pitch / beat) を省いた軽い経路を用意すると縮む見込み。
- 60 分の窓復号は 3.1 秒 (§14) なので、**完全被覆までの時間だけを比べると粗い列の方が遅い**。
  それでも採るのは、(1) 中心から順に埋まるので**見たい場所は即座に出る**、(2) 一度作れば
  span 変更もスクロールも**復号なし**で済む、の 2 点。窓復号は動かすたびに毎回払う。
  「切れ目を探す」用途は動かす回数が多いので、2 回目以降で逆転する。

**計測**: `wave_coarse_chunk` (chunk の復号 + 解析時間、chunk index、成功 / 失敗、失敗理由、
被覆数 / 失敗数) と
`wave_coarse_serve` (粗い列から描いた回数、被覆率) を perf へ出す。3 時間素材で「開いてから
可視範囲が埋まるまで」を実測して §14 に追記する。

**実装メモ (2026-08-26)**: worker の foreground latest-wins slot と単一 decoder は維持し、
foreground が無い loop だけが 1 chunk を処理する。現在中心は App から atomic priority hint として
同期するため、既存 raster の保持範囲内で中心だけ動いて新しい foreground request が出ない場合も、
次 chunk の選択へ反映される。chunk ごとの同一時間窓 raster 再公開を presenter が検知できるよう、
RGBA payload に worker-local content revision を含める。これは永続 identity ではない。


**D28 (帯の可読性、2026-08-26、利用者報告)**: **中心線を行境界へ丸めて濃くし、素材の先頭と末尾に
境界線を引く。**

- **中心線がほぼ見えなかった**。原因は明るさだけではない。`center_y +- 0.5` は **2 行に半分ずつ**
  乗るので、どの行も半分の濃さにしかならない。10x6 の検証ラスタで音楽ビューの中心線は
  **255 中 3**。行境界へ丸めて 1 行を満たし、alpha も 28 -> 170 にした (同ラスタで約 97)。
  **無音とは「中心線だけが見えている状態」**なので、ここが見えないと無音と未解析の区別も付かない。
- **音楽ビューは変えない**。帯かどうかは `SeekStripRowStyle` の有無で決まり、`None` の音楽ビューは
  従来の描画のまま (`tests/ui_snapshot.rs` が一致し続ける)。窓復号の経路も `Some` を渡すので、
  粗い列と窓復号で中心線の見え方が変わらない。
- **素材の先頭と末尾に境界線** (alpha 200、レーン全高)。帯は中心固定なので端へ寄ると窓が素材の
  外へはみ出す。そこが未解析と同じ見た目のままだと、**末尾より先へシークして次の動画へ移って
  しまう** (利用者指摘)。境界線は中心線より明るくして、線の役割を取り違えないようにする。
- 検証ラスタの実測は中心線 約 97 / 境界線 約 157 / 未解析 0 / 解析済み背景 (6,8,10)。
  テストは絶対値ではなく**この序列**を固定する。

## 6. presenter との境界

ストリップは native presenter のオーバーレイに描く。既存の**タイルオーバーレイと同じ経路**を使う。

- App → presenter: `NativeOverlaySeekStrip { center: SeekStripCenter, cells, wave_image, ... }`
  を毎フレーム渡す。テクスチャ化は `sync_tile_overlay_textures` と同じ形で presenter 側。
- presenter → App: `NativeOverlayCommand` に追加する。
  - `OpenSeekStrip` / `CloseSeekStrip`
  - `SetSeekStripState { state }`
  - `MoveSeekStrip { center }` / `CommitSeekStrip { center }`
  - `RequestSeekStripWindow { center, visible_count, pixel_width, pixel_height }`

## 7. 入力とジェスチャ

現在シーク行のドラッグは**水平スクラブに使われている**
([render_core.rs](../src/video/native_presenter/render_core.rs) の `native_video_seek_hit`)。
上ドラッグを足すので、同じジェスチャの分岐を**型で決着させる**。bool の積み増しにしない。

```rust
enum SeekRowGesture {
    /// 押下直後。まだどちらとも決まっていない。
    Undecided { origin: Pos2 },
    /// 水平スクラブ (従来動作)。
    Scrub { last_target_secs: Option<f64> },
    /// ストリップを開く上ドラッグ。
    OpenStrip { origin: Pos2 },
}
```

- 押下時に `Undecided`。最初に閾値を超えた移動で決める: **上方向へ 24pt 超 かつ |dy| > |dx|**
  なら `OpenStrip`、そうでなければ `Scrub`。
- 一度決めたら**そのドラッグの間は変えない**。`Scrub` に入った場合の挙動は従来と完全に同じ
  (退行を作らない)。
- ストリップが開いている間のドラッグは、シーク行の上ではなく**ストリップ本体**が受ける。

### 開閉

- 3 値の正本は persisted `Settings.video_seek_strip_state` (`none / thumbnails / waveform`)。
  runtime の session 有無と表示内容の mode を別々の状態にはしない。`Shift+S` の
  `VideoSeekStripCycle` は `none → thumbnails → waveform → none` を巡回する (D12)。
  既定 chord なしの `VideoSeekStripToggle` は表示中なら閉じ、非表示なら上ドラッグと同じ
  `video_seek_strip_last_choice` へ戻す。`VideoSeekStripNone` / `VideoSeekStripThumbnails` /
  `VideoSeekStripWaveform` は指定状態へ直接移る。
- 開く: 上ドラッグ (上記) / `Shift+S` / ロックアイコン左のベクター描画フィルムアイコン。
  上ドラッグは明示保存した `video_seek_strip_last_choice` を復元するので、波形を閉じた利用者は
  次も波形へ戻る。フィルムボタンは `Shift+S` と同じ 3 値を同じ順で 1 クリック巡回し、
  OFF / サムネイル / 音声波形を背景色とベクターアイコンで区別する (D13)。
- 閉じる: ストリップからシーク行へ下ドラッグ / Esc / `Shift+S` で `none` へ到達 / HUD 非表示 /
  動画切替 / フルスクリーン終了 / タイルモード (`S`) 開始。
- **クリックしてもストリップは閉じない** (シークして開いたまま)。続けて別の位置を探せる。
- 再生中は 100ms cadence で再生位置へ追従する。ストリップ本体をドラッグすると追従から外れ、
  離すとその時刻へ seek して再生を始めたうえで追従へ戻る。両 mode が同じ typed drag / commit
  経路を通る (D14 / D15)。
- ストリップ内に場面 / 波形の切替 UI は置かず、帯全体をドラッグ面にする。
- ストリップ上のホイールは上で範囲を 1 段狭く、下で 1 段広くする。現在値は右上の固定小文字で
  表示し、変更した永続設定と一致させる。この wheel は strip が消費し、背後の項目移動へ通さない。
- 左右パネルの端ホバー帯は、ストリップ表示中だけその上端側までに縮める。非表示時の帯は従来どおり
  HUD の 2pt 上までとする。

### タッチ

タッチのドラッグも同じ判定を通す。v2.13.0 のタッチ経路 (`Touch Move` / `Touch End`) から
同じ `SeekRowGesture` へ入れる。慣性スクロールは v1 では付けない。

## 8. 既存機能との境界

| 既存 | ストリップとの関係 |
| --- | --- |
| シークバー hover の 1 枚プレビュー | ストリップ表示中もシークバー / ストリップの両方から同じ 1 枚を出す。表示位置はストリップの 14pt 上で、帯を覆わない |
| `S` のタイル表示 | 全画面の俯瞰グリッド。ストリップはその場のスクラバ。**同時には出さない** (タイルを開いたらストリップを閉じる) |
| `B` の動画ブックマーク | 左パネルの一覧はそのまま。ストリップ側は自分の目盛にブックマーク / チャプターの印を描く |
| 「シーク時のズレ許容」設定 | ストリップ本体の seek / drag 時刻には使わない (§3 の表)。D17 の 1 枚プレビューはラベル時刻がポインタ位置なので使う |
| 動画→音声モード | そちらは全尺 `TimelineAnalysis` を持つ。ストリップは §5.1 の優先順位 1 でそれを再利用する |

## 9. 未確定 (実装中に確定させる)

| # | 論点 | 決め方 |
| --- | --- | --- |
| U2 | `KeyframeIndex` と `TimeGrid` の判定式 | 索引エントリ数と尺から妥当性を見る。実素材 (MP4 / MKV / TS / WMV) で確認 |
| U3 | ストリップの高さ・セル幅・枚数の既定 | 4K / FHD / ウィンドウ内再生で実機調整。11 枚前後を出発点にする |
| U4 | ストリップ用サムネイルの抽出幅 (`tile_w`) | 320px を出発点。容量と見やすさで調整 |
| U5 | 窓の `bin_secs` の決め方 | 窓幅 / 描画画素から導出する式を実測で決める |

## 10. スコープ外

- mIV Remote (D6)。将来出す場合は `/api/video/thumbnail` が 1 枚単位なので、窓をまとめて返す
  API と IPC 版の更新が要る ([web-remote-plan.md](web-remote-plan.md) / 先例は
  [video-upscale-shader-plan.md](video-upscale-shader-plan.md) §10.7)。
- parked-live (入力抑止中の別ウィンドウ) での表示 (D6)。
  [detached-rework-plan.md](detached-rework-plan.md) §2 の allow-list を広げない。
- 360 度動画 (レーン C)。同じ presenter とマウス入力を触るので同時に作らない。
- presenter のスケーリング構造 (§1.47、v3.2.0 で出荷済み)。触らない。

## 11. テストと計測

### 純ロジック (unit test)

- `center_index` ↔ 時刻の相互変換 (補間あり / なし)、端のクランプ。
- 窓の算出 (可視枚数 + 先読み)、窓が動いたときの再利用範囲。
- `StripAxis` の選択 (索引あり / 無し / 不完全)。
- 完全な index の最大 raw GOP が 15.0 秒なら index 軸、境界を越えたら decode 前 unavailable。
- coverage 80% 未満でも末尾が観測最大 GOP 内の complete index と、同じ entry 列が尺の 1/3 で
  止まる truly partial index。
- 最小間隔と GOP が等しい浮動小数点境界、および adopted cell が複数 raw entry をまたぐ可変 GOP。
- サムネイル / 波形の段階値一覧、wheel 上下の対応、両端、旧設定由来の段階外値。
- 空セルの扱い (先頭 / 末尾)。
- `SeekRowGesture` の決定 (上ドラッグ / 水平ドラッグ / 斜め / 閾値未満)。
- 1 回のドラッグ中の pointer 位置列を press-time pointer から変換すると、中心が単調に進み、
  release も最後の表示中心と一致すること。
- persisted 3 値の cycle と、`none` から最後の non-none 選択を戻す規則。
- 設定可視 span の first paint → 同じ中心・同じ秒 / pixel の保持用 upgrade と、Working 中の旧 raster 保持。
- 60 / 600 / 1800 秒の保持 span と要求境界、25% margin、replacement 待ちのヒステリシス。
- 3600 / 7200 / 10800 秒は可視幅を 1 回だけ要求し、同幅 upgrade を作らず、25% 移動後に
  replacement を要求すること。
- 保持中の波形 texture から設定可視 span の部分を切り出す UV / 部分 gap の算術。
- preference 変更時に旧 raster が request coverage にならず、新 first paint まで display holdover になること。
- 粗い 7-byte bin の量子化往復誤差、現在中心に最も近い未試行 chunk の選択、failed chunk の
  選択除外と非被覆扱い、`Unavailable` の全体終了、D27 の順序付き 3 経路、bitset coverage の
  連続区間合成、chunk bin の全尺列への合成。
- 再生位置追従の 100ms rate limit、微小差の抑制、ドラッグ中の detach。
- サムネイルセルの `pending / ready / failed` 判定と、最新要求の index-level failure の反映。
- サムネイル窓の 30 秒境界が pending セルを typed timeout failure へ確定し、UI / batch 用の理由を
  失わないこと。
- 軸解決後の `DecoderUnavailable` だけが strip 全体 notice へ置き換わり、軸失敗 / thread spawn
  failure / cancel は置き換えないこと。
- index DTS → packet PTS の対応付け、nearest-preceding の境界内 / 境界外、1 セルの不一致が同じ
  run の後続セルを stranded にしないこと。
- sampled duplicate detector が同じ size + head/tail を最初の path へ結び、同サイズでも sample が
  異なるファイルは検査対象に残すこと。

### ワーカー

- 最新勝ちで窓が差し替わっても、復号済みフレームを捨てないこと。
- cancel (閉じる / 動画切替 / フルスクリーン終了) で確実に止まること。
- 波形: pre-roll を捨てた bins が、全尺解析の同区間と一致すること (許容誤差内)。
- 実素材の手動診断は ignored test `probe_app_thumbnail_worker_window_from_env` に
  `MIV_STRIP_THUMB_PROBE_PATH` を渡す。任意で `MIV_STRIP_THUMB_PROBE_CENTER_SECS`、
  `MIV_STRIP_THUMB_PROBE_CENTER_SEQUENCE_SECS`、`MIV_STRIP_THUMB_PROBE_REQUEST_DELAY_MS`、
  `MIV_STRIP_THUMB_PROBE_WARMUP_CENTER_SECS`、`MIV_STRIP_THUMB_PROBE_VISIBLE_COUNT`、
  `MIV_STRIP_THUMB_PROBE_MIN_GAP_SECS`、`MIV_STRIP_THUMB_PROBE_HW=0|1` を指定し、各セルの
  typed failure、実 decode path、software retry の trigger、実アプリと同じ方向付き lookahead、
  fallback interval と raw / adopted spacing、および ready pixel の平均輝度・輝度分散・RGB channel
  range・alpha range を出す。axis 判定だけを切り分ける場合は
  `MIV_STRIP_THUMB_PROBE_FORCE_INDEX=1` で列挙済み index を強制できる。
- 全ライブラリの回帰確認は production の SeekStripThumbnailWorker と axis resolver を直接使う
  seek_strip_batch を使う。
  cargo run --profile dev-runtime --features dev-tools --bin seek_strip_batch -- folder...
  で mIV 対応動画拡張子だけを再帰走査する。0% / 25% / 50% / 75% / 100% のセルを中心に production
  window を要求し、特に先頭セルと真の最終セルを同じ center 経路で検証する。--limit N、差分比較用
  --json、SW 固定の --software を持つ。各 ready pixel は BT.709 luma (0..255) の平均・母分散と、
  R/G/B 各 channel の最大 range を測る。輝度分散 <= 1.0 かつ最大 channel range <= 4 のセルは
  `flat` として ready / failed から独立して報告する。この条件は単色に近い出力をレビュー対象へ
  挙げるだけで、不具合とは断定しない。実際の黒フレーム、fade、単色 title card も一致し得る。
  1 行 summary の後、failed / flat file は axis reason、全 cell time / outcome / failure reason / pixel
  統計を詳記する。failed cell または scan error があれば非 0 で終了するが、flat だけでは失敗に
  しない。JSON schema v3 には判定式と閾値、ready / flat cell を含む全 cell time と pixel 統計を
  保存する。open 不可、video stream 無し、duration 不明は decode window を開始せず `skip`、raw GOP
  上限超過は decoder unopened の `unavailable` とする。検査前に size + head/tail 8 KiB sample hash が
  既検査 path と一致すれば `duplicate` と一致先 path を記録する。pass / flat / fail / skip /
  unavailable / duplicate の 6 outcome は summary でも別集計する。

### perf 計装

`--perf-log` に次を出す。`if crate::perf::is_enabled()` の外ガードを付ける。

- `video_strip/open` — 開いた時点の軸種別、キーフレーム数、キャッシュヒット数。
- `video_strip/fill_wait` — **利用者が待った時間**。窓が確定してから可視セルが埋まるまで。
  既存の hover 待ち計装 (`seek-thumb-bench` ブランチ) と同じ観点。
- `video_strip/decode` — 1 窓の復号内訳 (シーク / 復号 / 変換、枚数)。
- `video_strip/wave_window` — 窓の音声復号 + 解析 + ラスタの内訳。
- `video_strip/wave_coarse_chunk` — chunk index、成功時の復号 / 解析時間、失敗時の理由、
  `AVDISCARD_ALL` を立てた非音声 stream 数、被覆済み / 失敗 / 全 chunk 数。
- `video_strip/wave_coarse_serve` — 粗い列から raster を作った時点の全尺被覆率と被覆済み /
  失敗 / 全 chunk 数。

判断基準: **窓が確定してから可視セルが埋まるまでの p90 を 300ms 未満**に置く。超えるなら
先読み量・抽出幅・窓の大きさを調整する。

## 12. `seek-thumb-bench` ブランチの扱い (ブリーフ §6 の決定)

`C:\home\mimageviewer-seek-bench` (ブランチ `seek-thumb-bench`) の 2 コミットは
**master へ入れず、コードとしても取り込まない**。

- `thumbnail.rs` の内訳計装は旧構造 (`bucket_key` / `pending_target_bits`) 前提で、現行の
  実 PTS `BTreeMap` + 要求ごと許容の worker には**そのまま適用できない**。
- 測定の目的 (高速シークを入れる価値があるか) は既に達成し、結論は
  [brief-seek-thumbnail-measurement.md](brief-seek-thumbnail-measurement.md) §7 に残っている。
- 有用なのは**観点**のほう (「利用者が待った時間」を独立に測る)。これは §11 の
  `video_strip/fill_wait` として作り直す。
- ブランチは参照用に残す (削除しない)。

## 13. 出口

- backlog §1.102 / §1.113 から本書へリンクする。
- 実装後に [video-architecture.md](video-architecture.md) を更新する
  (`ThumbnailWorker` 節の隣に、ストリップの 2 ワーカーと presenter 経路を書く)。
- `htdocs/mimageviewer/manual/video.html` を更新する。内部用語を出さない
  (「キーフレーム」「デコード」「ワーカー」等は書かない。「シークバーから上へドラッグすると
  前後の場面が並びます」の水準にする)。

## 14. 実測 (2026-08-23、`tools/seek_strip_probe`)

本体をビルドせずに前提だけ確かめる probe を `tools/seek_strip_probe` に置いた。
`cargo build --release -p seek_strip_probe` の後、FFmpeg DLL を `target/release` へ置いて
`seek_strip_probe <動画> ...` で走る。**HW デコードは使っていない** (= 下限性能)。

| 素材 | 索引 | GOP p50 | 11 枚 | 1 枚目 | 波形 60 秒窓 |
| --- | --- | --- | --- | --- | --- |
| MP4 / H.264 / 35 分 | 1939 件 (即) | 1.10s | 92〜110ms | 20〜24ms | 92ms |
| MKV / H.264 / 4 分 | 1→58 件 (要シーク) | 4.17s | 97〜123ms | 18〜20ms | 88ms |
| MOV / HEVC / iPhone | 37 件 (即) | 0.93s | 168〜188ms | 50〜55ms | 55ms |
| WebM / AV1 | 1→19 件 (要シーク) | 2.03s | 139〜145ms | 15〜18ms | 214ms |
| WMV / WMV3 / 29 分 | 0→590 件 (要シーク) | 3.00s | 19〜21ms | 3ms | 70ms |
| MP4 / H.264 / 密 GOP | 166 件 (即) | 0.50s | 254〜336ms | 23〜37ms | 音声なし |

読み取れたこと。

1. **索引からの列挙は実質ゼロ費用** (0.0〜0.2ms)。ただし Matroska / ASF は捨てシークが要る
   (§4.2)。**この 2 系統を落とすと MKV が全部 `TimeGrid` になる**ので、必ず 2 段で数える。
2. **11 枚は SW デコードでも 300ms 前後に収まる。** §11 の判断基準 (p90 300ms 未満) は、
   HW デコードを足す前の状態で既にほぼ満たしている。密 GOP の 336ms が唯一の超過。
3. **1 枚目は 15〜55ms で出る。** 中央セルを先に出して外側へ埋めれば、体感の待ちはほぼ無い。
4. **波形の 60 秒窓は 55〜215ms** (復号 + 解析)。**永続キャッシュは要らない** (D7 の裏付け)。
5. **窓の充填費用はファイルオープンを含めてはいけない。** 補助デコーダを開き直す実装にすると、
   iPhone MOV では open だけで 576ms かかり、同じ処理が 188ms → 1146ms に化けた。
   デコーダは動画 1 本につき 1 回だけ開く (§4.3)。
6. **GOP 長は素材で 6 倍以上違う** (0.50s 〜 4.17s)。これが §4.1.1 の stride を必要にした根拠。

### Increment 5 review の二段 waveform 再測定 (2026-08-24)

同じ probe で 60 秒 first paint / 180 秒 wide job を続けて測った。比較は同じ素材の同じ中央位置、
SW decode の decode + analyze で、raster 時間は含まない。表は release build 後の最初の pass。

| 素材 | 尺 | 60 秒 | 180 秒 | 倍率 |
| --- | ---: | ---: | ---: | ---: |
| MP4 / H.264 / 音楽 | 3.8 分 | 267.4ms | 587.9ms | 2.20x |
| MKV / H.264 / 映画 | 14.8 分 | 293.0ms | 793.2ms | 2.71x |
| MP4 / H.264 / 画面収録 | 3.6 分 | 535.4ms | 1214.8ms | 2.27x |

first paint は **267〜535ms**、wide upgrade / replacement は **588〜1215ms**。直後の warm pass は
60 秒が 173〜204ms、180 秒が 432〜484ms だった。したがって最大 1.21 秒の wide job は初回表示から
外し、60 秒 raster を出した後だけ background で行う。連続 1 倍再生では約 45 秒に 1 wide job なので、
最初の pass を保守的に使った worker 稼働時間は壁時計比で約 **1.3〜2.7%**、warm pass では
**1.0〜1.1%**。upload は 60 秒 first paint の 1 回、同じ中心の 180 秒 upgrade の 1 回、その後は
replacement ごとに約 45 秒で 1 回である。

### Increment 12 のアプリ経路 first-paint 実測 (2026-08-25)

`SeekStripWaveWorker` に presenter と同じ可視 raster 要求 (1920x94px、完成済み全尺解析なし) を送り、
`dev-runtime` 最適化プロファイルで request から raster publish までを測った。span ごとに新 worker を
作り、decoder / LRU の再利用は含めない。内部の `video_strip/wave_window` 計装値も同時に採取した。

| 素材 | span | first paint | decode | analyze | raster | bin_secs | 最大 bins |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| MP4 / AAC / 1時間51分 | 60s | 158.8ms | 108.3ms | 15.6ms | 1.8ms | 0.032s | 1875 |
| MP4 / AAC / 1時間51分 | 600s | 1117.8ms | 896.2ms | 180.7ms | 1.8ms | 0.313s | 1917 |
| MP4 / AAC / 1時間51分 | 1800s | 3092.8ms | 2566.0ms | 476.2ms | 1.8ms | 0.938s | 1919 |
| WMV / WMA / 1時間3分 | 60s | 651.9ms | 627.2ms | 12.8ms | 1.0ms | 0.032s | 1875 |
| WMV / WMA / 1時間3分 | 600s | 817.9ms | 682.7ms | 122.7ms | 0.8ms | 0.313s | 1917 |
| WMV / WMA / 1時間3分 | 1800s | 2200.4ms | 1788.1ms | 384.8ms | 1.2ms | 0.938s | 1919 |

`bin_secs = ceil_ms(span / 1920px)` により span を 30 倍にしても bins は 1875→1919 の範囲で、
出力密度と raster 費用は一定だった。解析は PCM 全体を 1 回走査するため完全な定数時間ではないが、
1800 秒でも 385〜476ms に収まり、総時間の 16〜18% である。主費用は実音声の decode
(1788〜2566ms、総時間の約 81〜83%) で、利用者 probe の見立てどおり decode が長尺時の床になる。

### D18 の 60 / 120 / 180 分 first-paint 実測 (2026-08-25)

§5.4 を実装しない暫定経路の待ちを、3 時間の H.264 / AAC MP4 (45,752,658 bytes、48kHz AAC)
で実測した。`dev-runtime` 最適化プロファイル、1920x94px、動画中央、span ごとに新 worker とし、
decoder / LRU は再利用していない。素材は測定用の黒映像 + 440Hz 音声なので、実素材の codec、
ストレージ、CPU により値は変わる。

| span | request から表示まで | decode | analyze | raster | bin_secs | 最大 bins |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 60 分 | 3120.0ms | 2218.8ms | 796.7ms | 1.7ms | 1.875s | 1920 |
| 120 分 | 6546.4ms | 4674.6ms | 1610.6ms | 1.6ms | 3.750s | 1920 |
| 180 分 | 8189.1ms | 5492.0ms | 2429.0ms | 1.9ms | 5.625s | 1920 |

したがって D18 increment では上段も利用可能なままにし、3.12 / 6.55 / 8.19 秒程度待って新 raster
へ交換する。待ち中は変更前の raster を display-only holdover として描き続ける。段階的な途中表示と
同じ音声範囲の再利用は §5.4 の責務として残す。

### stage-1 sweep の GOP 上限測定 (2026-08-25)

3 root、1,270 ファイルの production batch 結果は pass 1,247 / fail 1 / skip 22 で、axis は
`KeyframeIndex` 1,167 / `TimeGrid` 81 だった。修正後の unique な正常 keyframe-index 1,164 件では
maximum raw GOP が p50 8.34s / p90 10.00s / p95 10.00s / p99 10.43s、正常最大 10.43s
(maximum adopted gap は p50 9.04s / p90 11.26s / p95 11.72s / p99 12.10s / 最大 12.39s)。
唯一の旧 fail は index 13 件、
coverage 100%、最大 raw GOP 833.43s で、775.8s / 836.9s の cell が 30 秒 timeout になった。
15.0 秒上限は正常 raw 最大に 44% の余裕を残しつつ、この 1 件だけを decode 前に decline する。

hardware D3D11VA で初回窓の elapsed / cell を adopted maximum gap ごとに集計した p90 は、4 秒以下
33.5ms、4〜8 秒 43.3ms、8〜10 秒 19.8ms、10〜12.5 秒 23.3ms (file open と 11-cell run の
共有費用を按分した概算)。別途 4K HEVC / raw 8.34s は約 0.2s / cell。正常群は 1 GOP の仕事量で
収まる一方、833 秒 outlier は数分の前方復号になり、scrubber の間隔としても無意味なので D24 とした。

修正後の同じ 1,270 ファイルは pass 1,245 / fail 0 / unavailable 1 / skip 22 / duplicate 2、
failed cell 0、discovery issue 0。duplicate 2 件は旧 run で合計 620.7ms、10 window / 110 cell を
検査していたため、その分を直接省けた。sample detector が全 1,270 ファイルで読んだのは最大
20,481,101 bytes (19.53 MiB) で、対象 174.28 GiB の 0.011% 未満。旧 outlier の 30.0 秒 timeout も
7.5ms の index-only unavailable 判定に変わった。

未取得: MPEG-TS (索引を持たない代表格) が手元に無く、`TimeGrid` 経路を実素材で確認できていない。
