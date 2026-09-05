# 保留・着手待ちバックログ

[next-release-backlog.md](next-release-backlog.md) から分けた、**いま着手できない項目**を置く。
本体のバックログは「次に手を付けられるもの」だけにして、探しやすさを保つ。

節の番号は元のまま残してある。他のドキュメントやコミットからの参照が切れないようにするため。

運用ルール:

- 動かせるようになった項目は、この節ごと [next-release-backlog.md](next-release-backlog.md)
  へ戻す。番号は変えない。
- 完了した項目はここからも削除する。記録はコミット履歴・リリースノート・個別設計メモに任せる。
- **設計の正本を個別の plan へ移した項目も、着手できない残りがあるならここに節を残す**。
  作業候補の一覧は next-release-backlog.md とこのファイルの 2 つだけで見ているので、plan に
  しか無い項目は存在ごと見落とす。ここには現状と残りだけを短く書き、詳細は plan へリンクする。
- 「再現・確認待ち」は、こちらの手が空いても進められないもの。再現手段や利用者の返答が
  揃った時点で本体へ戻す。

---

## 1. 判断待ち — 決めれば着手できる

採否や方針が決まっていないもの。**実装より先に決めることがある**。

### 1.59 360 度ビューに等距離魚眼投影を追加する (提案・採否判断が要る)

- 出典: 利用者メール (pattier、2026-08-06)。「現在の透視投影に加えて等距離魚眼投影があると、
  視野を引いたときに 360 度カメラの絵に近い見え方ができる」。
- 現状: [panorama_wgpu.rs](../src/panorama_wgpu.rs) のシェーダは透視投影固定
  (`tan_half = tan(fov_y * 0.5)` でカメラ方向を作る)。この式は原理的に 180 度へ近づくと発散するため、
  引いた画角そのものが表現できない。
- 見込み: シェーダ側は数行 (半径 → 角度を線形に対応させ、`sin/cos` で方向ベクトルを作る)。
  作業の本体は投影モードの uniform 追加、設定への永続化、切り替え UI / キー割り当て、
  画角上限の再定義 (魚眼なら 180 度超も扱える)、ドキュメント。
- **どの魚眼かを先に確かめること** (2026-08-07 調査): 「魚眼」には複数の写像がある。
  半径 `r` と入射角 `θ` の対応が、透視 `r=f·tan θ` / 立体射影 `r=2f·tan(θ/2)` /
  等距離 `r=f·θ` / 等立体角 `r=2f·sin(θ/2)` と異なるだけで、**シェーダ上はどれも 1 行の差**。
  利用者の言う「引いたときに 360 度カメラっぽい絵」は、周辺の伸びが最も穏やかな
  **立体射影 (いわゆるリトルプラネット)** の可能性が高い。krpano も little planet は
  stereographic を使う。等距離はレンズの物理仕様としての標準表記で、見た目は中央が
  やや膨らむ。**どちらか一方ではなく、方式を選ぶ形にするのが素直** (実装コストがほぼ同じため)。
- 判断が要る点: 投影方式を増やすと画角スライダの意味と上限が方式ごとに変わる。既定は透視のまま、
  切り替えを提供するのか、パノラマ設定に持たせるのかを先に決める。
- ~~**先送り (2026-08-13、利用者判断)**。提案として妥当だが、v3.0.0 では扱わない。
  却下ではないので、要望が重なったら再評価する。~~
- **実装済み (2026-08-27、レーン C、branch `panorama-projection`)**。着手可否を利用者へ再確認し、
  4 方式すべてを入れる / 既定は透視のまま / 切り替えはキー + 360 表示中の上バーのボタン、で確定。
  **仕様の正本は [panorama-360-view-plan.md §13](panorama-360-view-plan.md)**。要点:
  - 視野角の意味を全方式で共通化 (画面上下端の入射角 = `fov_y / 2`)。透視は導入前の式と恒等で、
    既定の見え方は変わらない (格子点での回帰テストあり)。
  - 画角上限は方式ごと。透視 = 約 149° (従来と同値)、非透視 = 約 340°。方式を戻すときに
    `clamp_fov` を通すので、広げた画角のまま透視へ戻しても発散しない。
  - 等距離 / 等立体角は広画角で画面隅が定義域を出るため、そこは不透明の黒 (魚眼のイメージ
    サークル外)。WGSL は seam 勾配の uniform control flow を守るため早期 return せず、最後の
    色選択でだけ判定する。
  - settle overlay の stale 判定キーを `PanoPose` へ型化 (投影方式の比較漏れを構造で防ぐ)。
  - **実機確認の状況** (2026-08-27):
    - 確認済み: 既定 (透視) の見え方が導入前と同じに見えること、4 方式で見え方が変わること、
      上バーの投影一覧が画面内に収まること。
    - **未確認**: 各方式が幾何的に正しいか。利用者が 4 方式を見比べた時点では違いを判別
      できなかった。**これは実装の問題ではなく、普通のパノラマ写真では原理的に判別
      できない**ため (差は周辺と広い画角にしか出ない)。判別用のチャートと手順を
      [panorama-360-view-plan.md §13.7](panorama-360-view-plan.md) に用意したので、
      次に触る人はそれで確認する。特に「水平を向いて画角を最大まで引くと、赤道が
      直線になるのは透視だけ」が最も差が出る。
- 規模 / 優先度: Medium / P3 (実装済み)。
- **次の一手 (§1.112)**: 数式・分岐位置・uniform の持ち方はここで確定した。動画側は
  `panorama.rs` の写像表と WGSL の `projection_theta` をそのまま実行時 HLSL へ移す。

### 1.156 見開きの範囲コピーが左右にまたがって選べない — 仕様未実装

- 出典: 利用者が範囲コピー (フルスクリーン上部バーのカメラアイコンを **Ctrl+クリック**
  → ドラッグで範囲選択) を試して発見 (2026-08-31)。トリムとのずれ 2 件は v3.4.0 で対応済み。
- 残っているのは **左右のページにまたがる選択ができない**こと。
  `capture_region_target_at` がポインタ位置のページを 1 つ選び、その中だけを対象にする。
- 「2 ページ分を合成して 1 枚にする」話になるので、誤動作だった 2 件とは別に判断する。
- 規模 / 優先度: 中 / P3。

### 1.166 切り取りを表示にも反映するか — 方針決定

- 出典: 利用者要望 (2026-09-02)。「隠蔽加工は設定画面の外でも適用済みなのに、切り取りは
  適用されない。表示トリムで設定することになるのか」。
- **現状の設計と理由**: 切り取りは最後段で、表示では範囲外を暗くするだけ。実切り出しは
  capture / export のときだけ ([app.rs:55079](../src/app.rs:55079)、
  [display-pipeline.md](display-pipeline.md) 「crop は通常表示では暗転 overlay だけなので、
  レイアウト基準も描画 UV も変えない」)。**隠蔽加工との違いは「画像の範囲が変わるか」**で、
  隠蔽は画素を書き換えるだけで寸法が変わらない。
- **表示を切り取ると困る具体**: 消しゴム / 補正レイヤー / 隠蔽加工 / 注釈は**切り取り前の
  画像へ記録する**ので、切り取りの外側を見て塗る必要がある。各ツールは自分の手前までの状態を
  表示する原則があり ([ui_fullscreen.rs:15499](../src/ui_fullscreen.rs:15499) のコメント)、
  ツール中は既に枠と暗転を出していない。
- **仕組みは既にある**: 部分矩形だけを表示する経路 = `content_bbox` (正規化 0..1)。表示トリムが
  使っており、フィット倍率と 100% 判定 ([ui_fullscreen.rs:7041](../src/ui_fullscreen.rs:7041))、
  見開き (`harmonize_spread_auto_bboxes`)、連結読み (ユニット寸法変化時の再アンカー) まで
  通っている。切り取りも矩形なので、ソース画素値を基準寸法で割れば載る。
  **各辺 2 割という上限は表示トリム側の方針**であり、仕組みの制約ではない
  ([view_trim.rs:4](../src/view_trim.rs:4) `MAX_VIEW_TRIM_MARGIN`)。
- **候補**: 左パネルで編集を開始したら切り取りを一時解除し、抜けたら戻す。ツール中に枠と暗転を
  出さない仕組みが既にあるので、その所有へ寄せられる。
- **決めること**: [fs_page_content_bbox](../src/ui_fullscreen.rs:10236) の優先順位 (現状は
  「分割が勝ち、無ければ表示トリム」。切り取りを 3 番目にどう入れるか) / 見開きで左右の
  切り取りが違うときの扱い / 反映中は枠と暗転を出さない / 常時反映か設定で選ばせるか /
  一時解除の入口をどのツールに持たせるか。
- 1.165 (b) を入れれば「場所によって切り取られたり切り取られなかったりする」食い違いは
  消えるので、**本項は「さらに表示へ反映するか」の判断**であり、1.165 の前提ではない。
- 利用者へは「検討する」と回答済み (2026-09-02)。
- 規模 / 優先度: Small〜Medium / P3 (方針決定が先)。

---

## 2. 再現・確認待ち — こちらからは進められない

再現手段が無い、利用者の確認を待っている、実機計測を待っている、原因が未確定のもの。

### 1.30 native video Stage 5 の再投入条件 (revert 済み、原因未確定)

- 出典: 2026-08-01。Stage 5 (`726f838d`) を入れたところ UI が完全停止する P1 が再現し、
  `737c5234` で **revert 済み**。v2.9.1 は Stage 5 を含めずに出す。§1.28 のカーソル問題は
  既知の問題として残る。
- 症状: 動画をフルスクリーン再生中に VST エディタを開閉すると UI が完全に固まる。**動画・音声の
  再生は継続し、EOF で次の動画へも進む。UI だけが死ぬ。**終了も右クリックも不能。2 回再現
  (t=178s / t=18s)。
- 停止点 (cdb `-pv` で採取): main スレッドが `SendMessageW` 経由の wndproc の中で
  `eframe run_ui_and_paint` → `egui_wgpu paint_and_update_textures` →
  `wgpu_hal::dx12::Surface::acquire_texture` → `WaitForSingleObjectEx`。
  他スレッドは全て健全 (`vst-owner-dispatch` は recv で idle、pump は sleep、render / demux /
  decode は稼働)。
- **原因は未確定。候補 2 つとも潰れていない**:
  1. **VST owner handoff**: owner / z-order / visibility の変更が同期メッセージを撃つ。
     ただし hidden anchor は published fullscreen host の破棄時のみ通り、C++ は
     `old_owner == new_owner` で早期 return し、owner 付け替え自体は Stage 5 以前から存在する。
  2. **カーソル所有**: 新実装は pump から **8ms ごとに `WindowFromPoint`** を呼ぶ。同 API は
     hit-test のため対象の wndproc へ **`WM_NCHITTEST` を同期送信**し、相手は UI スレッドの窓や
     **別プロセスの VST エディタ**になり得る。pump からの無制限な cross-thread / cross-process
     同期呼び出しであり、**Stage 4 の「pump は時間上限を保証できない処理を持たない」原則に抵触する**。
     「窓を作らないから同期メッセージを撃たない」は成立しない。
- 次回に取るべき証拠 (今回取れていない):
  - `acquire_texture` の wait が**本当に無限か**。wgpu-core 27.0.3 は **1000ms timeout** を渡し、
    wgpu-hal は `WAIT_TIMEOUT` を `Ok(false)` にして先へ進む。スタック 1 枚では
    「デッドロック」と「1 秒待ちを繰り返す飢餓」を区別できない。**wait の `dwMilliseconds` が
    `0x3e8` か `0xffffffff` かを読み、1.2 秒以上あけて複数回 break する**。
  - 停止直前に main が実際に受けた `msg` (どの同期メッセージが再入描画を起こしたか)。
    スタックにある `in_window_resize_subclass_proc` は全メッセージを通る常設 subclass なので、
    **resize が起きた証拠にはならない**。
  - `old_owner != new_owner` / anchor handoff / presenter retirement が実際に発生したかのログ。
- 再投入の条件: **同一 release profile** で 4 分割 A/B を通すこと。
  (a) Stage 4 baseline / (b) cursor のみ / (c) owner のみ / (d) Stage 5 全体。
  今回の A/B は `dev-runtime` 対 `release` で、出荷判断には十分だが**原因帰属としては profile 差が
  残る**。(b) が VST 開閉 soak を通ることを出荷条件にする。
- 診断用の回避策 (`MIV_WGPU_FRAME_LATENCY=2`、`WGPU_DX12_USE_FRAME_LATENCY_WAITABLE_OBJECT=DontWait`)
  は**出荷修正にしない**。症状が消えても wndproc 内 GPU wait と Win32 同期依存は残る。
- VST 側の長期案: editor を transient な presenter HWND へ付け替えるのではなく、**editor lifetime
  全体で安定した専用 owner proxy** を使い、topmost / focus / visibility を別の ordered transaction
  として扱う。owner request の dedupe と一括適用も要る。
- **保留 (2026-08-13、利用者判断)**。revert 後は**一度も再現していない**。原因未確定のまま
  再投入の A/B に工数を割く段階ではない、として棚上げする。再発したら上の「次回に取るべき
  証拠」から再開する (特に `dwMilliseconds` が `0x3e8` か `0xffffffff` かの確認)。
  §1.28 のカーソル問題は既知の問題として残したまま。
- 規模 / 優先度: Large / **P2 (保留)**。カーソル (§1.28) の修正はこれが片付くまで入らない。

### 1.102 動画のシークストリップ — 実素材で確認できていない経路 (素材待ち / 再現待ち)

**機能そのものは完了・出荷済み**。v3.3.0 でサムネイル列と音声波形、v3.5.0 で全体表示。
設計と実測の正本は [video-seek-strip-plan.md](video-seek-strip-plan.md)。ここに残すのは
**こちらからは進められない 2 点**だけ。

- **MPEG-TS など `TimeGrid` 経路を実素材で確認できていない**。索引を持たない代表格の素材が
  手元に無い。22,811 ファイルの sweep で `TimeGrid` 軸自体は通っているが、MPEG-TS は含まれて
  いなかった。素材が手に入ったら `seek_strip_batch.exe --json <folder>` を回す
  (`dev-tools` feature の bin、アプリの実ワーカーと軸解決を駆動する)。
- **「末尾 4 セルが黒い」という利用者報告を再現できていない**。HW / SW とも、独立した復号でも
  黒くならなかった。D25 の 12 秒診断 (輝度・分散・channel range) を仕掛けてあるので、次に出たら
  `%APPDATA%\mimageviewer\logs\mimageviewer.log` に理由が残る。**推測で直さない。**
  なお D23 で「終端に達したセルには最後に復号できたフレームを充てる」ようにしたため、申告尺が
  中身より長い素材では同じ最終フレームが複数セルに並ぶ。フェードアウトする動画なら同じ
  見え方が出やすくなる点は意識しておく (これが原因だと断定はしない)。
- 実素材 sweep は 2026-08-26 に利用者判断で打ち切り済み (残り 19 件 = 22,811 件中 0.08%、
  いずれもファイル末尾へ seek できない類で素材側の問題)。再開するなら同書の
  「実素材 sweep の打ち切り」節から。一覧は `C:\home\miv-batch-runner\remaining-failures.txt`。


### 1.137 F12 で別ウィンドウへ入るときだけ、中身の無いホストが 380ms 先に見える — R2b + retire follow-up 実装済み、実機計測待ち

> ⚠ detached viewer リワーク中の領域。症状パッチ (delay / guard / 追加 repaint) を入れない。

- 出典: 2026-08-28、利用者の観察「F12 を押すと、動画のウィンドウサイズをいちど
  真っ白に表示してから、最大化して、その後再生しているように見える」を
  ログで裏付けたもの。**§1.115 の font atlas resync を撤去した後も残るちらつきの主因候補**。

#### 実測 (build C = font work 撤去後のログ)

```
33.192  [native-video] defer placement switch until detached host is ready: target=DetachedWindow
33.192  [native-video-key] F12 (seq=13)
33.489  [detached-viewer] registered host hwnd=0x2280fde        <- ホストはもう見えている
33.568  [native-video] resume deferred detached placement switch after 376.5ms
33.574  [native-video] window created-hidden: hwnd=0x4982e4e rect=(0,34 3840x2054)
33.701  [native-video] window shown: hwnd=0x4982e4e visible=true
```

**ON 側だけが非対称に遅い**。OFF (→ Fullscreen) には遅延が無く、押下から 7ms で
presenter window を作り、`placement switched ... total=139.0ms` で完了する。

内訳: ホスト viewport の生成・登録に約 297ms、登録を検知して resume するまでに約 79ms。

#### 見立て (未検証)

ホストの viewport は `activate=true` で**先に可視になり**、その時点では動画がまだ attach
されていないので中身が無い。376ms 後に全画面サイズの presenter window が上に乗るので、
「白 → 最大化 → 再生」と見える。直すなら**見せる順序の所有権** (presenter が publish するまで
ホストを見せない) を決める形になる。**遅延を短くする・待ちを入れる等の症状パッチは不可**。


#### 同じ根を持つもう一つの症状: タスクバーの明滅 (2026-08-28 利用者報告)

同じログ (約 115 秒の 1 セッション) でのウィンドウ生成回数:

| 契機 | ホスト生成 |
| --- | --- |
| 起動時の動画フルスクリーン | gen=1 |
| **F12 を 12 回 = 6 往復** | gen=2〜7 (**1 往復につき 1 個**) |
| その後の別のフルスクリーン操作 (F12 ではない) | gen=8〜13 |

native-video window は 15 (`detached-viewer-child` 9 + `fullscreen-borderless` 6)。

**回数が多いのではない** —— F12 1 往復につきホスト 1 個で、無意味な繰り返しは無い。
問題は **F12 OFF でウィンドウを捨てている**こと。そのため 1 往復でタスクバーボタンが
「消える」「現れる」の 2 回変化し、6 往復で 12 回の変化になる。隠して再利用すれば 0 回。

> 調査中に `open_fullscreen` が 155ms で 4 回出ているのを一度異常と見たが、**誤り**。
> `idx=1 → 3 → 5 → 7` で `input_seq` も 17→20 と進んでおり、PDF の見開きページ送り。
> ウィンドウは作られていない (この間のホストは gen=9 の 1 個だけ)。

detached ホストの builder は `.with_taskbar(true)` ([ui_fullscreen.rs](../src/ui_fullscreen.rs))。
つまり **F12 のたびにトップレベルウィンドウを破棄して作り直しており、タスクバーボタンが
消えては現れる**。白いウィンドウと同じ根 (= ウィンドウを再利用せず毎回新規に作る)
なので、別起票にせずここで扱う。

修正の方向も共通で、「**ウィンドウの寿命を F12 のトグルと切り離す**」か、
少なくとも「**見せる順序の所有権を決める**」ことになる。リワークの
`DetachedWindowRuntime` / 状態 enum が扱う領域なので、先にプラン §2 を読むこと。

#### 測定: タスクバー自体が 1 フレームごとに出入りする (2026-08-28)

利用者の「タスクバーが何度も明滅する。**アプリのアイコンではなくタスクバー自体**」という
指摘を受けて、キャプチャの下端 18px をフレームごとに分類した (明るい = タスクバー可視)。
**F12 1 往復で 16 回反転していた**。

| 遷移 | 区間 | 反転回数 |
| --- | --- | --- |
| F12 #1 (→ fullscreen) | 6.233ー6.567s (334ms) | **9** |
| (安定) | 6.567ー9.567s | 0 |
| F12 #2 (→ detached) | 9.600ー9.933s (333ms) | **7** |

ほぼ 1 フレーム (33ms) ごとの交互。利用者には「3 回くらいの点滅」と見えていた。

ログとの対応 (F12 間隔がログ 3.47s / キャプチャ 3.37s で一致):

- **F12 #1**: fullscreen 窓の生成→表示→`placement switched` (138ms) の前後に集中。
- **F12 #2**: **押下からホスト viewport が表示されるまで**の 333ms に集中し、
  動画の子ウィンドウが作られる前に終わっている。

つまり閃光の正体は「DWM が再合成している」という漠然としたものではなく、
**遷移中に「全画面を覆うウィンドウがある / ない」が毎フレーム入れ替わっている**こと。
上の白いホストと同じ 330〜380ms の窓で起きている。

対応する geometry: `fullscreen-borderless` = (0,0 3840x2160) はタスクバーを含む全面、
`detached-viewer-child` = (0,34 3840x2054) は含まない。遷移中は両方が短時間共存し、
z-order / show / raise / destroy のたびにどちらが手前かが入れ替わる。

> 注: タスクバーボタン (アイコン) の出入りではない。動画窓は 2 つとも
> `native_window_owner_for_placement` で owner 付き (= タスクバーに出ない)。
> ボタンを持つのは detached ホスト viewport (`with_taskbar(true)`) だけで、1 往復 2 回。
> 当初私はこのボタン側を数えており、**利用者の指摘で対象を間違えていたことが分かった**。
- 関連: §1.115 (font atlas 側。**別機構**。破棄フレームは 14 → 0 になったがちらつきは残った)。
- 規模 \ 優先度: Medium ～ Large / P2。

#### R2b 実装結果 (2026-08-28)

- `pending_detached_video_host_switch` と `native_video_mode_switch` を廃止し、context-owned
  `PresentationTransitionOwner` (`Stable → Preparing → Ready → Committing → Stable`) へ統合した。
  current / target / request generation / activation intent と candidate/prior HWND を 1 request が
  所有し、F12 再入力、failure、Esc、window close、player end、stale Ready/Commit を reducer で
  解決する。
- native contract は hidden candidate の create/attach/prime を `Ready` までに済ませ、`Commit` で
  初めて publish する。`NativeCommitted` で host `Visible/Focus` を先に発行し、同じ effect batch で
  outgoing を retire する。incoming host の OS visibility poll は retire の前提にしない。
  fixed-ms commit / forced recovery は無い。failure/abort は hidden candidate だけを cleanup する。
- host `Visible/Focus/Destroy` と native `Publish/Destroy` を reducer effect に限定し、遷移中の
  presenter/HUD/VST/focus recovery は同じ permit を読む。実 action は transition id / target / HWND
  付き `[presentation-transition]` ログに出る。既存 `[ui-frame-gap]` / `[atlas-probe]` は変更していない。
- 自動回帰は両方向、abort、F12 再入力、Esc、window close、player end、stale generation を含む。
  出荷判定前に実機 1 往復で **outgoing presenter raise 0 / cover change 各方向 1 /
  content-ready 前 host activation 0** を画面キャプチャとログで照合する。

#### build I: publish 済み outgoing presenter の retire が UI sleep に依存 (2026-08-28 follow-up)

```
18.726  Publish incoming presenter
18.727  placement committed
18.732  host Visible / Focus
19.216  shared output pool exhausted (以後約500msごと)
24.630  [ui-frame-gap] 5899.4ms
24.632  Destroy outgoing presenter        <- window click 直後
24.661  placement retired
```

`Committing::AwaitingHostVisible` が、次の `App::update` による `IsWindowVisible(host)` の level poll まで
`RetirePlacement` を出さなかった。native `PlacementCommitted` / `PlacementRetired` は lossless event bus
から root viewport を wake するが、その間の `HostVisible` は UI-only event である。poll 末尾の
`request_repaint` と visibility / focus / redraw の window event が次 pass を起こす想定だった。
build F の約 70ms 完了も第2 pass は必要で、incidental window event に起こされただけだった。

`DetachedHostDisposition` はこの依存を導入していない。問題の `Fullscreen → DetachedWindow` では
disposition は `None` で、変更差分にも `AwaitingHostVisible` の変更は無い。build I は偶発 wake の
無いスケジュールで既存の依存を露出させた。

candidate は `NativeCommitted` が届く時点で create / attach / current-frame prime / pump publish 済み。
ここを presenter ownership の cutover とし、commit effect を
`ApplyPresentation → Visible → Focus → RetireOutgoing` に変更した。host command は先行させるが、
OS visibility confirmation を retire の前提にはしない。`AwaitingHostVisible` / `HostVisible` poll は
撤去し、timeout / retry / settle window / 追加 repaint は入れていない。

GPU output pool は 16 slot、source queue は最大 8、copy-fence retire queue は最大 4 で、candidate
prime の短い二重生存は設計内。二 presenter の zero-overlap 制約ではなく、5.9秒残った旧 fence owner
が pool exhaustion を起こした。容量 tuning ではなく lifecycle ordering の correctness defect である。

回帰テスト
`fullscreen_to_detached_retires_on_native_commit_without_waiting_for_host_visibility` は commit batch の
順序と `AwaitingRetire` を固定する。retire を incoming host event の後ろへ戻す killing mutation では
commit batch から `RetireOutgoing` が消えて失敗する。

次 run では `shared output pool exhausted waiting for free slot` の連続、同区間の約5.9秒
`[ui-frame-gap]` が無いことを確認する。`Destroy outgoing` は `placement committed` の直後、
同じ UI pass で送った retire command の pump / WM teardown 分だけ後に並ぶ。固定msを受け入れ条件には
しないが、build F と同じ数十ms級で、秒単位や user input 待ちにならないことを timeline で照合する。

R4 に残るのは `show_viewport_deferred`、single render entry、host persistence。今回も F12 OFF は
terminal に host HWND を破棄するため、次の ON では約 300ms の hidden host 作成を待つ。
この待ち時間自体の短縮、F12 を跨ぐ taskbar button/host identity の永続化は本項の R2b close には
含めず、R4 gate C の仕様決定後に扱う。

### 1.168 詳細表示の列ヘッダを右クリックすると、アイテムメニューが出ることがある — 再現待ち

- 出典: 利用者報告 (2026-09-02、§1.143(b) の実機確認中)。詳細表示の列ヘッダを右クリック
  すると、列カスタマイズメニューではなく**アイテムのメニュー**が出て、そのあと左クリック
  すると列メニューが出る。スクリーンショットで「パスをコピー」「代表サムネ固定を解除」
  「ペイント編集を反映して開く」を確認済みなので、フォルダ背景メニューでも列メニューでもなく
  単一アイテムのメニューで確定。右ドラッグはマウスジェスチャに設定。
- **再現していない**。同じバイナリで再度試すと出なくなった。**間欠**である。
- **単純な当たり判定の話ではない**。`render_details_list` をそのまま描く kittest harness
  ([ui_main.rs](../src/ui_main.rs) `details_header_right_click_tests`) を作り、マウス
  ジェスチャ ON / OFF の両方でヘッダ帯を右クリックしたが、`context_menu_idx` は `None` の
  まま。行の右クリックでは従来どおり開く。ヘッダは内側縦スクロールの外に確保されるので、
  行の rect とも背景判定の `body_inner_rect` とも重ならない。
- **次に見る場所**: `context_menu_idx` に値を入れるのはコード全体で 3 か所しかない。
  [ui_main.rs](../src/ui_main.rs) の cell 経路 (`handle_cell_interaction` 内)、右ドラッグ
  短押し経路 (`open_grid_right_drag_short_tap_menu`)、フォルダ背景経路
  (`open_current_folder_context_menu_at`)。この 3 か所には**どれが発火したかを記録する
  `[ctxmenu-probe]` ログを入れてある**ので、再現したら
  `%APPDATA%\mimageviewer\logs\mimageviewer.log` を `ctxmenu-probe` で grep すれば経路と
  クリック座標が一意に決まる。**推測で直しに行かず、まずこのログを取ること。**
- 状態依存の可能性 (未検証): 直前の操作で残った `context_menu_idx` / クリックのペアリング
  状態、sticky popup の開閉、ジェスチャ短押し判定の時間しきい値。
- レーン A の右クリックメニュー刷新より前から在ったかは不明。該当 3 経路の最終更新は
  2026-07-12 / 07-24 / 08-05 でレーン A より前だが、間欠なので断定しない。
- 規模 / 優先度: 不明 (原因未特定) / P3。実害は「メニューをもう一度開き直す」程度。

### 2.6 ZIP / RAR のダブルクリックが時々無反応 — 原因確定・修正済み、利用者の確認待ち

- 報告条件: サムネイル表示、ZIP / RAR、Enter では開ける。最初のダブルクリックでは開かず、
  そのまま待つだけでも開かないが、しばらくして再度ダブルクリックすると開くことがある
  (専用スレ >>257 で訂正)。RAR はローカル上の直読み対象。
- **原因確定 (2026-08-19、報告環境の perf log)**: egui は
  `count = if triple_click { 3 } else if double_click { 2 } else { 1 }` で数え、`is_double()` は
  **`count == 2` の完全一致**。`triple_click` は `max_double_click_delay * 2` を**前々回のクリック**から
  測る (`egui-0.33.3/src/input_state/mod.rs:1213-1222`, `1006-1011`)。つまり **3 回目のクリックは
  triple になり `double_clicked()` が false になる**。1 回目で開かなかった利用者はもう一度
  クリックするので、その 3 回目がちょうどここに落ちていた。
  - 実測 (idx 9、同一セル、同一座標): 離す時刻 165.735 / 166.384 / 166.612。3 回目は前回から
    **228ms** で double 成立圏内だが、前々回から **877ms** で `2 x 500ms` の内側 → triple 判定。
    同 session の成功例は 400ms 間隔の 2 回目。**400ms が成立して 228ms が成立しない**という
    逆転がこれで説明できる。
  - v3.1.2 のダブルクリック時間の OS 追従はこれを**悪化させた** (triple 窓 600ms → 1000ms) が、
    原因ではない。300ms でも 600ms 以内に 3 回クリックすれば同じで、**本項は v3.1.1 以前からあった**。
- **修正 (v3.1.2)**: グリッドは egui の click count を起動判定に使わず、`response.clicked()` で
  click 成立だけを受け取る。同じ `items_generation`・同じセル idx・OS 由来のダブルクリック時間内に
  ある 2 click を自前の単一 pairing state で対にし、item activation 後・セル以外の primary click・
  一覧世代変更で対を切る。「開く → Esc → 単発クリック」で再度開いてしまう追補も同時に閉じた。
  egui 本体は変更していない (triple click は text field の行選択が使うため)。
- **状態: 利用者の再確認待ち**。再発報告が来なければ close する。再発した場合に使える観測手段:
  - `grid/cell_signal` が成功 / 失敗を問わず `time_since_last_click`、`max_double_click_delay`、
    `clicked_by_primary`、`double_clicked_by_primary` を出す。失敗例だけを見ず、同じ session の
    成功例を control として比較する。
  - `grid/activation_request accepted=true` と `grid/activation_dispatch_complete` があれば click は
    成立しているので、以降の dispatch / open 側を疑う。`double_clicked_by_primary=false` かつ
    `first_click=true` なら pointer click として成立していない側を疑う。
  - 依頼手順: 開発者で性能ログ ON → 再起動 → 症状再現 → **再起動せず**「ログを zip にする」→
    診断 ZIP を送付。ログにファイル名 / path が含まれる既存の注意書きも案内する。
- 優先度: P2。原因が確定するまで guard / retry / 閾値再調整の症状修正を入れなかった方針は、
  再発時も維持する。

### 3.2 補正パラメータ変更後に AI アップスケールキャッシュが優先される疑い (再現待ち)

- 背景: 5ch レス 792 の追跡項目。「画像補正パラメータを変更しても AI アップスケールキャッシュが
  優先され、ページを行き来すると変更が効いていないように見える」という報告。
- 現状 (2026-06-18): 通常環境と v1.7.0 ポータブル版の追加テストで再現せず。現在の設計では、
  色調補正や AI 設定の変更は final AI / final composite cache のキー差分または明示クリアで反映される。
  一方、最終段スマートシャープなど post-filter 系は final AI cache を再利用して final composite だけを
  作り直す。さらに AI アップスケール出力にはスマートシャープを適用しない固定仕様なので、
  操作内容によっては「変わらない」ように見える場合がある。
- 方針: 具体的な再現手順が出るまではコード修正しない。再報告時は、変更したパラメータが色調補正 /
  AI ON/OFF / デノイズ / post-filter / スマートシャープのどれかを最初に切り分ける。
- 優先度: P3 monitor / 再現待ち。

### 1.185 同名ファイル処理をすべて OFF にしてもファイルが非表示になる — 内訳待ち (2026-09-05)

- 出典: 専用スレ >>354。「同名ファイルの処理」をすべて OFF にしても、表示されないファイルが
  あるという報告。
- 現在の同名判定は、拡張子を除いた名前の**完全一致**を大文字・小文字を区別せず比較する。
  曖昧一致ではない。4 種の同名除外処理はそれぞれ設定で gate されており、すべて OFF なら
  この経路では非表示にしない。設定確定時には現在地を再読み込みする。
- フォルダバーの `非表示 N件` は同名除外だけではなく、`同名など / 隠し項目 / 対象外` の合計。
  隠し属性、Hidden+System、未対応拡張子、拡張子なし、書庫の扱いが「無視」の RAR / 7z / LZH、
  mIV の作業用・付随ファイルなども一覧から除外され得る。現時点では**隠し項目または対象外の
  可能性が最も高く、同名処理の不具合とはまだ確定していない**。
- 利用者には `非表示 N件` をクリックして内訳を確認してもらい、`同名など` が 0 以外なら
  不具合として調査すると回答済み。
- 再開条件: `同名など / 隠し項目 / 対象外` の内訳と、表示されないファイルの名前・拡張子が
  分かること。すべて OFF で `同名など > 0` なら active backlog へ移し、scan result の分類と
  設定反映を追う。隠し項目 / 対象外なら仕様どおりかを確認する。
- UI 補足: 現在の内訳メニューは原因にかかわらず「同名ファイル設定を開く」を表示するため、
  隠し項目 / 対象外だけのときは案内として紛らわしい。報告が仕様どおりだった場合も、内訳に応じて
  説明や導線を出し分ける改善を検討する。
- 規模 / 優先度: 原因未確定 / P3 (利用者情報待ち)。

---

## 3. 見送り / 将来 — 現時点で着手しない判断

判断とその理由を残す。方針が変わったときに、同じ議論をやり直さないため。

### 1.149 製本フォルダごとに上限ピクセルサイズを設定して自動縮小 — 見送り (2026-09-02)

- 出典: 利用者要望 (2026-08-31)。散らばった画像を製本フォルダへ集め、別ツールで一括縮小して
  から送る運用。集める時点で縮小できると手順が 1 つ減る、という話だった。
- **1.148 (複数選択の一括エクスポート) で用が足りるため見送り**。本フォルダを開いて全選択 →
  `Ctrl+E` → 長辺指定で同じ結果になる。報告者も「エクスポート機能の拡張の方が柔軟に使える
  気もする」と書いており、利用者と合意済み。
- 再判断するときの材料。縮小段は既に共有されていて、`books::write_composited_page` へ渡す
  `ExportScale::Full` を本ごとの上限へ差し替えるだけ。ただし「副産物」ではなく、次の 3 つが残る:
  1. 本ごとの上限をどこに永続化するか (本フォルダ単位の設定を新設することになる)。
  2. 無編集画像の byte-copy fast path と両立しない。上限を効かせるなら
     `page_requires_full_composite` へ上限を渡す必要があり、製本の追加規則そのものが変わる。
  3. 既にあるページへ遡って効かせるか。効かせるなら再エンコードが走るので明示操作にする。
- 規模 / 優先度: Small〜Medium / P3。

### 1.153 ZIP / PDF のページにタグを付けられない — **低優先度 / 将来** (2026-08-31 判断)

**★はページ単位で付くのに、タグはコンテナ単位でしか付かない。** 種別ごとの現状は
[docs/item-kind-capability-matrix.md](item-kind-capability-matrix.md)、影響調査は
[docs/tag-page-support-survey.md](tag-page-support-survey.md) が正本。

**壊れてはいない。** ビューアの右パネルは以前から「タグ対象: この本 (名前)」と対象を
明示しており ([ui_metadata_panel.rs](../src/ui_metadata_panel.rs) `tag_target_note_for_item`)、
黙ってコンテナへ付け替えているわけではない。**機能が無いだけ**なので、利用者要望が
出るまで着手しない判断にした (2026-08-31)。

**ただし 1 つだけ実害がある (下の「先に塞ぐなら」参照)。**

#### 規模

段階分けは survey §7。合計 **18〜26 ファイル / 約 2,000〜3,500 行**。

| 段 | 内容 | 規模 | 単独リリース可 |
| --- | --- | --- | --- |
| 1 | タグ対象を型で分ける (ページはまだ無効) | 6〜8 / 250〜450 | ○ |
| 2 | 種別を additive に保存 (ページはまだ無効) | 8〜11 / 600〜950 | ○ |
| 3 | ローカルでページタグを end-to-end 有効化 | 10〜15 / 800〜1,250 | ○ |
| 4 | リモートでページを表示 | 5〜9 / 350〜700 | ○ |

#### 着手前に決めることが 9 件ある

survey §8 に列挙。特に **変換アーカイブ内ページの identity を source archive と cache ZIP の
どちらにするか**は、決めないと段 3 の停止境界を作れない。

#### 先に塞ぐなら: summary と一覧の母集団が食い違う

**これは仮定ではない。** メタデータ転送はページタグを export/import し、往復テストもある
([metadata_transfer.rs:7033](../src/metadata_transfer.rs:7033))。そのため出荷済みの版でも
`tags.db` にページタグ行が存在し得る。

その行が今、片側からだけ消えている。

- タグ横断一覧は `item_key` を実パス化して `Missing` として黙って落とす
  ([tag_view.rs:318](../src/tag_view.rs:318))
- 一方 summary の `COUNT(*)` は `item_tags` 全行を数える ([tags_db.rs:685](../src/tags_db.rs:685))

結果、**「タグA: 5 件」と出ているのに一覧には 3 件しか出ない**。メタデータ転送を使った
環境に限られるが、原因が利用者から見えない。

塞ぎ方は 2 通りで、どちらも段 1〜4 と独立に数十行で入る。

1. summary 側からページ行を除く (= 一覧に合わせる)
2. 一覧に「表示できない項目 N 件」を出す (= 件数の差を説明する)

段 3 まで行けばページが一覧に出るので、この対処は不要になる。**段 3 をやらないと決めて
いる間だけの措置**である点に注意。

**優先度**: 低。利用者要望が出たら段 1 から。母集団の食い違いだけは、タグまわりを
次に触るときに 1 と 2 のどちらかを入れる。

### 1.160 縦連結で画面外のページはアニメーションしない — 仕様

- アニメの次コマ期限は `fullscreen_page_layout` (= 実際に描いたページ) からしか立てない。
  画面外のページを混ぜると、過ぎた期限で 0ms 起床を繰り返しアイドルが空転する。
- 利用者判断 (2026-09-01): 「GIF を連結で観たいケースはあまりないので仕様でよい」。
- 変えるなら「見えていないページのために起き続けない」を保ったまま行う必要がある。

### 2.4 CSV / TSV からの一括タグ / レーティング付与 — 保留

- 出典: 同じメール往復。こちらから代替案として提案し利用者も歓迎したが、**利用者の実際の
  使い方 (参照は一時的で、タグを付けても後から参照しないことが多い) とは噛み合わない**ため、
  RAR フォルダのサムネイル表示遅延 (v3.1.2 で対応済み) を優先すると回答した。
- 位置づけ: 外部ツールで抽出した結果を mIV へ持ち込む導線としては筋が良い。単独では需要が
  薄いので、タグ運用側の要望が別に出たときに合わせて再判断する。
- 実装するときの前提 (利用者へ明言済み): **明示的な「取り込み」操作のときだけ動く**こと。
  パスの一覧をビューとして開く形 (実体のない仮想フォルダ) は採らない。理由は、他人由来の
  リストで任意パスを参照してしまうこと、UNC パスなら開いた瞬間に外部へ認証情報が飛び得る
  こと、実フォルダ前提の処理 (サムネイルの識別キー、移動 / 削除時の扱い、各種設定の保存先)
  への影響範囲が大きいこと。
- 優先度: P3 / 保留。
