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

### 1.258 本棚削除が途中で失敗した場合の移行待ちメタデータ整合 — コードレビュー、範囲判断待ち (2026-09-20)

- 出典: M-2 独立レビューのコード調査。実機での発生報告・再現ではない。
  `books::delete_book` の `remove_dir_all` は一部を削除してから失敗する可能性があるが、
  現在の結果は途中まで削除できた個々のパスを伝えない。
- 削除処理中の新規 generic migration は待機させるが、削除エラー後には再開するため、
  既に消えたページへ移行待ちのメタデータを再作成する可能性が残る。
  M-2 では完全成功時の削除パスの invalidation と、読めない旧復旧記録の保護を扱う。
- 部分成功のパス報告、または失敗後の再照合を含む削除処理の設計が別途必要。
  今回の M-2 へ拡大せず、障害注入による影響確認と対応範囲の判断を後続へ残す。
  [M-2 の設計・検証記録](collection-migration-journal-recovery.md) を参照。

### 1.252 動画字幕のデコード・表示 — >>429、当面見送り (2026-09-17)

- 出典: >>429。音声トラック切り替えとあわせて、動画字幕のON/OFFが欲しいとの要望。
- 判断: **当面は見送る**。複数音声は §1.251 として字幕から分離して検討する。
- 現状、mIVに動画字幕の列挙・デコード・同期・描画経路は無い。native presenter内の `subtitle` という名前は
  ナビゲーション案内の副テキストであり、メディア字幕ではない。動画エンジンの設計上も字幕はスコープ外。
- 対応する場合は、少なくとも次を一体で設計する必要がある:
  - 埋め込み字幕trackの列挙とOFFを含む選択、seek / source切り替え時のflushと時刻同期。
  - SRT等のtext字幕、ASS/SSAのstyle・font・配置、PGS等のbitmap字幕をどこまで対象にするか。
  - native D3D11動画、F12別ウィンドウ・複数ウィンドウ、回転・360度表示、capture / Remoteにおける合成範囲。
  - 日本語・縦書き・複数行・欠落fontを含む合法的な自作テスト素材と、依存ライブラリを追加する場合の配布・license確認。
- 再検討条件: 字幕形式と表示範囲を限定できる具体的な利用例が複数集まるか、字幕rendererを安全に統合できる
  動画描画基盤の見直しを行うとき。単にON/OFF UIだけを追加して部分対応にはしない。
- 規模 / 優先度: Large / P3 / 保留。

### 1.213 通常動画のナビゲーターを出せるか — 描画経路が静止画と違う (2026-09-11)

- 出典: 利用者要望 (2026-09-11)。v3.7.0 の通常動画ズームを使ってみて、静止画のナビゲーター
  (拡大中に全体と現在位置を示す小窓) が動画にも欲しくなった。「負荷が高くなりそうですかね？」
- **利用者が心配していた「OS 側で拡大する設定があるから重ねられないのでは」は当たらない (確認済み)。**
  ズーム中は OS 任せの設定が自動的に上書きされる:

  ```rust
  fn effective_video_scale_filter(filter: VideoScaleFilter, video_zoom_active: bool) -> VideoScaleFilter {
      if video_zoom_active && filter == VideoScaleFilter::OsDefault {
          VideoScaleFilter::Standard
  ```
  ([render_core.rs:2104](../src/video/native_presenter/render_core.rs:2104))

  サーフェスも表示領域全体を取る (`full_display_region = panorama_active || video_zoom_active`、
  [surface_policy.rs:177](../src/video/native_presenter/surface_policy.rs:177))。
  テスト `video_zoom_uses_the_full_display_region_even_with_os_default` あり。
  **ズーム中は必ず mIV のシェーダが表示領域いっぱいのサーフェスへ解決している。**

- **本当の論点は「どこに描くか」。**
  - **presenter (D3D11) 側**: フレームは既に手元にあるので、いま拡大表示で出している矩形の
    隣にもう 1 枚描くだけ。**毎フレームのコストは小さい見込み**。ただし静止画のナビは egui 実装
    (`fs_navigator_*` 群、[ui_fullscreen.rs:2356](../src/ui_fullscreen.rs:2356) 以降) なので、
    枠・ドラッグ・表示根拠 (Fixed / Hold) を**別実装で作り直す**ことになる
  - **egui overlay 側**: UI はそのまま使えるが、**動画のフレームは egui のテクスチャではない**。
    毎フレーム渡す経路が要る。**安くない**
- **未確認**: presenter 側に描いた場合の実コスト。上は構造からの見込みで、測っていない。
- **関連**: §1.224 (一時表示ナビゲーターの未開始クリックが消える) は静止画側の既知の不具合。
  動画へ広げるなら、その所有境界の整理を先に済ませたほうがよい。
- 規模 / 優先度: Medium / P3。**どちらに描くかを決めるまで見積もらない。**

### 1.128 ★固定で範囲外になった別窓を、閉じず自前の一覧へ切り出す — 仕様提案

- 出典: 2026-08-26、§1.125 の実機確認。複数ウィンドウモードで動画再生中に
  ★固定を押し、**そのファイルが固定範囲外だったため動画窓が閉じた**。
  **§1.125 の設計どおりの動作**であり、不具合ではない。ただし体験としては
  **グリッドのボタンを押したら別の窓が消えた**に見える。
- 利用者の希望: **再生はなるべく継続**し、前後移動も納得できる形で残す。

#### 前提の確認 (2026-08-26、コードで裏取り済み)

「★固定していないときは実 FS 順で前後移動する」という想定は **実装と違う**。

```rust
fn build_nav_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize>
```

`get_nav_indices()` は `current_grid_order()` (= `visible_indices`、Details なら `details_order`)
を渡す ([ui_fullscreen.rs:7621](../src/ui_fullscreen.rs:7621) /
[app.rs:46141](../src/app.rs:46141))。つまり **★固定の有無にかかわらず、
前後移動は常にグリッドの表示順・絞り込み結果を辿る**。実 FS 順を辿るモードは存在しない。

したがって ★固定は「別の並びに切り替える」操作ではなく、
**その時点の並びを凍結する**操作である。

#### 提案 — 別窓を自前の context へ切り出す

現状、複数ウィンドウモードの動画窓は **同じ context を別の窓で描いている**
(実機ログ: `main_fs_idx=Some(121) mounted=true`)。だから親のグリッド操作が直撃する。

**器は既にある**:

- 2026-08-26 に `snapshot` を `ViewerContextBundle` の field にしたので、
  **context ごとに別の並び・別の凍結状態を持てる**
- `fork_mounted_live_media_context` が「再生中メディアを自前の context へ切り出す」既存の仕組み

よって §1.125 の miss 経路を「閉じる」から「**切り出す**」へ変えることで、
窓は自分の items を持ち続け、再生も前後移動も維持される。

#### 詰めるべき点

- 切り出した context の一覧は **固定前の並び** を保持する。それを利用者にどう示すか
  (窓のタイトル / HUD に何か出すか、無言でよいか)
- 親が ★固定を**解除**したとき、切り出した窓を親へ戻すのか、独立のままにするのか
- 切り出した context の寿命 (窓を閉じるまで / 親のフォルダ移動まで)
- 静止画でも同じにするか、メディア限定にするか
- ⚠ detached 述語に触れるので §2 の手続きが必要

- 規模 \\ 優先度: Medium / P3 (不具合ではなく仕様改善)。

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

### 1.247 EPUB を PDF へ変換して読む — 漫画用途を中心に技術検証する (2026-09-16、方針更新 2026-09-17)

- 出典: 利用者からの EPUB 対応要望 → 競合の対応状況調査 → suzunia の実装解析
  (2026-09-16 の相談) → mIV スレ >>434 で用途を確認 (2026-09-17)。
- **方針更新 (2026-09-17)**: 汎用 EPUB ビューアを新設する案は引き続き採らない。一方、利用者の
  主用途が漫画で、文字サイズ・フォント・余白の変更、テキスト選択・コピーはいずれも不要と
  確認できたため、**DRM のない EPUB を固定レイアウトの PDF へ変換し、既存の PDF 経路で読む
  方式を技術検証する**。スパイク結果を見て実装採否を決める。
- 対象範囲:
  - 漫画用途を優先し、まず `rendition:layout=pre-paginated` と画像中心の EPUB で、ページ順・
    右開き・画質・変換時間を確認する。
  - DRM 付き EPUB の解除・変換は扱わない。利用者側で DRM のない EPUB を用意する前提。
  - 変換後はページサイズと組版を固定し、文字サイズ・フォント・余白の変更、テキスト選択・
    コピーは提供しない。
  - 漫画以外のリフロー型 EPUB は、縦書き・ルビ・改ページが実用になるかを追加確認するが、
    初期検証の成立条件にはしない。
- 判断材料として新しく分かったこと:
  1. 国内の現役競合 suzunia が EPUB へ対応した (同梱 CHANGELOG の `### 2026.08.09.`)。
  2. その実装は**リアルタイムに文字を組み直すビューアではなく、固定レイアウトで組版して
     画像で持つ方式**だった (下記)。
  3. ウィンドウをリサイズしてもページ周囲の余白が増えるだけ (**利用者の観測、2026-09-16**)。
  4. 競合の対応状況: NeeView / YACReader / ImageGlass / nomacs / QuickViewer はソース検索で 0 件、
     BandiView は公式の対応形式一覧に無し、OpenComic は未リリースの v1.7.0 のみ。
     SumatraPDF / Komga / Kavita は対応 (いずれもレンダリングエンジンを持つ製品)。
- suzunia の実装 (2026-09-16 の**静的解析**。配布バイナリの import と文字列を読んだだけで、
  **実行していない**):
  - `WebView2Loader.dll` を同梱し、`CreateCoreWebView2EnvironmentWithOptions` を動的ロード。
  - 展開した EPUB を `SetVirtualHostNameToFolderMapping` で WebView2 へ渡す。
  - CSS / JS を注入して組版する。バイナリ内に実物がある:
    `function isV(el) { return !!el && getComputedStyle(el).writingMode.indexOf('vertical') === 0; }`
    で**縦書きを検出して分岐**し、横書き側は `html{width:Wpx}` `body{width:Wpx;padding:Mpx;font-size:…}`
    + `column-gap:(2*M)px;column-fill:auto` でページへ割る。W / H / M (幅・高さ・余白) がパラメータ。
  - `CapturePreview` で画像化し、`EpubLayoutCache` に保存 (README の「ページ数把握の為に最初に
    開く際は時間が掛かります」と整合)。`png-cache hit` というログ文字列もある。
  - **つまり「変換して画像で持つ」方式**。本項の案と設計思想は同じで、違うのは出力先だけ。
- 検証する方式 (2026-09-16 利用者の選択: 画像 ZIP ではなく **PDF**、2026-09-17 検証着手を決定):
  - **EPUB を PDF へ変換してキャッシュし、既存の PDF 経路で読む**。変換は WebView2 の
    `PrintToPdf` (ページサイズ・余白を指定し、ヘッダ / フッタは抑止)。
  - 初期スパイクは出力を PDF に統一する。**画像で構成された EPUB**
    (`rendition:layout=pre-paginated`、spine の実体が画像) も、spine 順・ページ寸法・右開きを
    保って PDF 化できるかを先に確かめる。画像を無圧縮 ZIP へ直接展開する最適化は、方式が
    成立した後の候補とし、初期スパイクでは二経路を持たない。
  - リフロー型は**文字サイズもレイアウトも固定**になる。テキスト選択・コピーは提供しない。
  - キャッシュは変換済みアーカイブキャッシュと同じ扱い (容量上限で古いものから破棄)。
  - `page-progression-direction` から**右開きを自動設定**できる。
  - DRM 付きは変換時に検出して明示的にエラーにする (無言で空の本にしない)。
- 画像 ZIP ではなく PDF にする理由:
  - `CapturePreview` は表示矩形で撮るため、**画面より大きいページを撮れるか (クランプされないか)
    が不明**。PDF ならベクタで出るのでこの問題が消え、mIV 側は PDFium で任意倍率に描ける。
  - ページ分割を Chromium の印刷改ページに任せられる。自前のスクロール制御が要らない。
  - 出力が数 MB で済む。画像 ZIP は 300 ページで数十 MB 以上になる (**測っていない見積もり**)。
  - 着地点が既存の PDF 経路なので、ページ列挙・並列レンダ・ズーム再レンダ・サムネイルの
    **下流作業がゼロ**。
- 実装の当たり所 (2026-09-16 のコード確認):
  - `do_convert` が format で分岐して `expand_*` を呼ぶ構造 (`src/archive_converter.rs:782`)。
    `ArchiveFormat` へ `Epub` を足して `expand_epub` を生やす形に嵌まる。
  - `quick-xml = "0.41"` が既に依存にある (`Cargo.toml:393`)。container.xml / OPF / spine の解析に使う。
  - **容量上限つきキャッシュは既存**: `prune_to_size_limit_locked` (`src/archive_cache.rs:475`)、
    設定 `archive_cache_max_bytes` (`src/settings.rs:4153`)、キャッシュ管理 UI。
  - 変換ダイアログ・進捗・キャンセル・`.part` からの atomic publish・検証も既存。
  - `reserve_cache_zip_path` (`src/archive_cache.rs:542`) が ZIP 前提なので、PDF 出力を置けるように
    する小改修が要る。
  - 拡張子の認識: `is_convertible_archive_path` (`src/folder_tree.rs:173`) は `from_extension` へ
    委譲しているので enum を足せば自動で通る。個別に列挙しているのは
    `src/archive_cache.rs:591`、`src/app.rs:74620` / `74859`、`src/reading_history_db.rs:501`、
    `src/app/grid_paint.rs:1474` (バッジ)、`crates/remote-web/src/store.rs:321`。
  - **WebView2 ワーカーは新規 exe**。`Cargo.toml` に webview2 / wry の依存は無い。Susie 32bit
    ワーカーや PDF ワーカーと同じ子プロセス方式にすれば、GUI 側が COM と Chromium のクラッシュを
    背負わない。別 crate にすれば `windows` crate の版も本体と独立に選べる。変換は一度きりの
    バッチなので、PDF プールのような優先度レーン / epoch / harvest は要らない。
  - 配布への追加は Susie ワーカーと同じ形: `build.rs` の vendor 検査 (`build.rs:493` 付近)、
    launcher の `include_bytes!`、`scripts/build-portable.ps1`、`scripts/sign-files.ps1`、
    `scripts/check-vcrt-pe-dependencies.ps1`、`scripts/bootstrap-vendor.sh`。
- **技術検証 (スパイク 2〜3 日。いずれも未検証)**:
  1. 漫画 EPUB を PDF 化したとき、spine 順・ページ寸法・右開き・見開きが保持され、画像が
     不要に再圧縮・縮小されないか。
  2. `PrintToPdf` が**縦書き** (`writing-mode: vertical-rl`) とルビを実用的に改ページするか。
     これは漫画用途の成立後に確認する副次項目で、失敗しても漫画対応までは否定しない。
  3. `@media print` による見た目の変化と、ヘッダ / フッタの抑止。
  4. WebView2 Runtime が無い環境での失敗の出し方 (Windows 11 は同梱だが、無い環境はあり得る)。
  5. WebView2 のユーザーデータフォルダの置き場 (APPDATA / ポータブルの `data`) と後始末。
  - 画像化 (`CapturePreview`) 方式を採る場合は、加えて**画面より大きいページを撮れるか**を
    確かめる必要がある。
- 未決事項:
  - 組版パラメータ (幅・高さ・余白・文字サイズ) を決め打ちにするか設定にするか。設定にするなら
    キャッシュの鍵へ入れて、変更時は再変換する。初版は決め打ち + 「再変換」で足りる見込み。
  - **対外的な名乗り方**。「EPUB 対応」と書くと DRM 付きの購入書籍と、リフロー時の文字サイズ変更を
    期待される。実装する場合も「DRM のない EPUB を固定レイアウトへ変換して閲覧」と明記する。
- テスト素材: DRM のない漫画 EPUB を優先する。縦書き・ルビの副次確認は青空文庫の EPUB で組める。
- 規模 / 優先度: スパイクが順当なら Medium (1〜2 週間)、画像化方式を採るなら 2〜3 週間。P3
  (まずスパイク、実装採否は結果待ち)。**この数字は測っていない見立て**で、WebView2 側の
  スパイク結果で変わる。

---

## 2. 再現・確認待ち — こちらからは進められない

再現手段が無い、利用者の確認を待っている、実機計測を待っている、原因が未確定のもの。

### 1.111 フルスクリーンで動画へ入る瞬間に前面を失い、押しっぱなしのキーが他アプリへ流れる — 利用者報告

- 出典: 利用者報告 (2026-08-21、v3.1.2 で確認)。
  - 全画面モードで静止画を表示し、**上下キーを押しっぱなしで高速送り**している最中、
    次が動画だと**一瞬ウィンドウが消え、フォーカスが別のウィンドウへ移り**、押しっぱなしの
    上下キーがそのウィンドウへ入力される。
  - **ウィンドウモード / 別ウィンドウモード / 別ウィンドウのフルスクリーンでは起きない。**
  - 併発: ランダムに**カーソルが mIV 上で非表示のまま**になる。次の画像 / 動画へ移ると戻る。
- 原因の見当 (コード確認済み、未確定):
  - フルスクリーン表示では静止画が **egui フルスクリーンビューポート**、動画が
    **別 HWND の native presenter** (D3D11 + DirectComposition)。静止画→動画の遷移で
    前者を `ViewportCommand::Visible(false)` で隠し ([ui_fullscreen.rs:12739](../src/ui_fullscreen.rs:12739))、
    後者を別途 materialize する。
  - **この症状は既に疑われていて計装がある**。同じ場所の直前に
    `log_main_flash_probe("fs_visible_false", …)` ([ui_fullscreen.rs:12735](../src/ui_fullscreen.rs:12735))。
  - [native_video.rs:2842](../src/app/native_video.rs:2842) のコメントが症状そのものを書いている:
    「foreground 奪還まで 80ms 待つと、**その間だけ外部ウィンドウが見えることがある**」。
  - つまり **viewport を隠してから presenter が foreground を取るまで、mIV のどのウィンドウも
    前面を持っていない**。Windows は z-order 上の次のウィンドウへ前面を渡すので、キーリピートが
    そちらへ流れる。報告と一致する。
  - 他モードで起きない説明も付く。ウィンドウ内動画はメインウィンドウの子として扱われ
    (`set_in_window_video_child`、[native_window_host.rs:536](../src/video/native_window_host.rs:536))、
    専用フルスクリーンビューポートと presenter HWND を入れ替える構造になっていない。
  - **カーソル非表示の固着も同根**。`cursor_hidden` はラッチで、解除は (a) egui フルスクリーン
    フレーム内で観測したポインタ操作、(b) HUD が出て `clean` が false になったとき、の 2 つだけ。
    presenter 側は `update_cursor_icon` で別経路の解決をしており、**カーソルの所有者が 2 つある**。
    次の項目へ移ると直るのは、そこで `open_fullscreen` がラッチを落とすため。
- **2026-08-27 追試: 再現しなかった (利用者、v3.3.0 開発版)。** 単一ウィンドウの通常
  フルスクリーン、動画は全画面再生、背後にエディタを置いて ↓ 押しっぱなしで高速送り。
  **動画の再生がすぐ始まり、背後のエディタへキーは 1 つも入らなかった。**
  Windows のキーリピートは初回遅延の後およそ 30ms 間隔なので、1 つも漏れないということは
  前面の空白がほぼ無いことを意味する。
  - main window の cloaking (`native_video_main_cloaked`) は `f2215fd2` (2026-05-13) で
    **v3.1.2 に既に入っている**ので、これが後から効いたわけではない。
  - 未確定なもの: (a) v3.1.2 以降のどこかで解消した (b) より遅く開く動画 (大きい 4K/HEVC、
    低速ドライブ、起動後 1 本目) が必要 (c) 報告者の環境依存。
  - **確かめる手段は既にある**。`MIV_DETACHED_WINDOW_DEBUG=1` で起動すると
    `main_flash_probe stage=fs_visible_false` が窓の状態ごと出る
    ([app.rs](../src/app.rs) `log_main_flash_probe`)。抑制条件は
    `fullscreen_idx.is_some()` を含む広いものなので、この経路なら必ず出る。
    1 回の再現で前面の空白の有無が確定する。
  - **既知の問題ページには載せない** (見せられないものは載せない)。再現を取れたら載せる。
- **着手条件: 複数ウィンドウ / キー入力所有権の整理 (別 worktree、`docs/briefs/modifier-ownership-design.md`) の完了待ち。**
  症状パッチ (遷移前後の追加 `SetForegroundWindow`、遅延、キーリピート抑止) を入れない。
  問われているのは「viewer の遷移中、どのウィンドウが前面と入力とカーソルを所有するか」であり、
  1.100 や過去のキー所有権報告と同型。整理後に、遷移を**所有権の受け渡しが切れない 1 手**として
  設計し直す。
- 回帰確認の観点: 静止画→動画・動画→静止画の双方向、キー押しっぱなし中と単発、
  4 表示モード (フルスクリーン / ウィンドウ / 別ウィンドウ / 別ウィンドウのフルスクリーン)、
  遷移後のカーソル可視状態。
- 規模 / 優先度: Medium / P1 (実害あり)。**整理待ち**。
- **v3.2.0 からは外すと確定 (利用者判断 2026-08-22)。** 着手条件 (キー入力所有権の整理) が
  未達で、症状パッチを入れれば整理時に解きほぐす手間が増えるため。次リリースの筆頭候補として残す。

### 1.0f 別ウィンドウの動画で、gamepad の十字キーによるシークが効かないことがある — 再現待ち (2026-08-30)

> 元の節は「実機確認で見つかった 3 件」。(a) F12 連打で別ウィンドウが閉じる件は v3.3.1 で修正済み
> (`edf1c5ed`、内部 teardown で Close を送らず、terminal になった ViewportId を再利用しない)。
> (c) R-02 の症状は利用者環境で再現せず、[review §10.2](review-v3.3.0/README.md) の記述を弱めて閉じた。
> **残っているのは下の (b) だけ**で、効かない状況の再現待ち。

**(b) 動画を別ウィンドウで開いているとき、gamepad の十字キーでシーク操作ができない。**
利用者報告 (2026-08-30、v3.2.0 / v3.3.0 で同じなので退行ではない)。静止画では動く。

- **当初の見立て「動画面の分岐に入っていない」は否定済み** (`75cdfd9b0`、v3.4.0)。十字キーの
  配り先を `DpadRoute` の 1 値へ集約したうえで計装したところ、mount 済みの detached context が
  動画を `fullscreen_idx` に持つ状態では、判定は正しく `Video` を返していた。
  **この前提で調べると空振りする。**
- **2026-09-04 の実機ログでは、右キーでシークできている**。利用者が「今は効く」と報告し、
  `mimageviewer.log` に裏付けがあった:

  ```
  [gamepad] dpad route: Video(10) surface=Viewer fs_idx=Some(10) is_music=false
            detached_context_at_rest=false session_detached_or_switching=true
  [native-video-key] seq=1 virtual_key=0x27 ... fs_idx=10 presentation=detached
            outcome=action:seek_forward_5s
  ```

  **`session_detached_or_switching=true` かつ `at_rest=false`** — root projection が
  `fullscreen_idx` を持ったまま detached 表示している側では、配り先も下流も通っている。
- **残る疑いは `at_rest=true` の側**。active viewer context が AtRest のとき、batch は root では
  配られず、`gamepad_batch_goes_to_active_context()` (= `at_rest && surface == Viewer`) を通って
  `update_active_viewer_context` が mount した中で配られる。**この経路を通ったときの
  `dpad route:` 行は、まだ 1 度も記録されていない。**
- **次にやること**: 効かない状況を再現し、`[gamepad] dpad route:` を見る。Video 行が出なければ
  batch が mount 済み context へ届いていない (行に並ぶ述語が、どれで落ちたかを示す)。Video 行が
  出ていれば下流で、`[native-video-key]` の `outcome=` が続きを語る。**どちらか分かるまで
  直さない。** 窓が 2 つ以上あるとき / 直前にメインを触ったとき / F12 直後かどうかで
  `at_rest` は変わるので、再現時はその条件も記録する。

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

---

## 3. 見送り / 将来 — 現時点で着手しない判断

### 1.236 ts / mts / 3gp は一覧に出さない — 見送り判断の記録 (2026-09-13)

- 出典: 利用者提案 (2026-09-12)。古い携帯の `.3gp`、ホームビデオの `.mts` が一覧に出ない。
  利用者は `a.mts.mp4` へ改名すると**再生できた**が、サムネイルは出なかったと報告している。
- **方針 (2026-09-13 開発者判断): 見送る。** 利用者へも回答済み。再提案しない。
  理由は「難しいから」ではなく、**継続的に動作を確かめられないから**。とくに AVCHD の
  `.mts` はビデオカメラ実機の素材で、手元に無い。**確かめられない形式を一覧に出すと、
  コンテナ側や Shell 側の挙動まで mIV の不具合として報告が来る**が、その切り分けを
  毎回引き受けられる見込みが無い。
- **2026-09-15 追記: 利用者が拡張子を追加する設定 (動作保証なし) も見送り。** 利用者から
  「サポート外でよいので一覧に出せれば」と提案があった。理由は同じで、一覧に出た時点で挙動が
  mIV の不具合として届き、こちらで確かめられない。加えて、拡張子の正本は定数 1 つ
  ([folder_tree.rs:77](../src/folder_tree.rs:77)) だが、一覧・検索・索引・リモート・類似検索など
  **20 箇所ほどが直接参照している**ので、設定化は 1 行では済まない。EPUB 見送り (2026-08-01) で
  「拡張子を足すだけ」の案も入れなかった判断と同じ形。
- 以下は調べた内容の記録。**判断を覆す材料が出たときに読む**ものであって、
  同じ調査を繰り返さないために残す。

#### 一覧に出すこと自体は 1 行

[folder_tree.rs:77](../src/folder_tree.rs:77) の `SUPPORTED_VIDEO_EXTENSIONS` が唯一の正本で、
一覧 / 検索 / 索引 / リモート / 類似 / メタデータ転送など 20 箇所弱がここを参照する。
現在 7 個 (`mpg mpeg mp4 avi mov mkv wmv`) で、**`webm` も入っていない**。
再生は FFmpeg が中身で判定するので、**拡張子を足すだけで再生は動く** (利用者の改名実験が実証)。

#### 「中途半端」の内訳 — 段によって難易度が違う

| 段 | 状況 |
| --- | --- |
| 再生 | **問題なし。** 利用者が改名で実証済み |
| 一覧のサムネイル | **Shell 次第。** [video_thumb.rs:186](../src/video_thumb.rs:186) の `IShellItemImageFactory` のみで、**FFmpeg による代替経路が無い**。失敗すると 200ms〜6.4s のバックオフ再試行を消費して空になる |
| 再生中のシーク | **動くが精度が落ちる。** mIV は `av_seek_frame` + `AVSEEK_FLAG_BACKWARD` で直前 keyframe へ飛ぶ ([decoder.rs:2906](../src/video/decoder.rs:2906))。手順は MP4 と同じだが、**MPEG-TS は索引を持たないので FFmpeg がバイト位置を二分探索する**。PTS 不連続 (放送録画の継ぎ目、AVCHD の 2GB 分割) があると外れる。尺も推定値になる |
| シークバーのサムネイル列 | **未検証。** 索引が不完全 / 時刻が逆行すると `TimeGrid` へ落ちるが、[video-seek-strip-plan.md:64](video-seek-strip-plan.md:64) に「**索引がほぼ無い素材では TimeGrid でも目的時刻のフレームを見つけられない**」とある。診断は `last_frame=none` |

**3gp は切り離せる。** ISO-BMFF なので索引を持ち、シークもストリップも mp4 と同じ経路。
残る不確実は Shell のサムネイルだけ。**mts / ts は不確実が 2 つ重なる。**

#### 判断を覆すとしたら (今回は実施しない)

1. **エクスプローラーでサムネイルが出るか確かめる。** mIV は Shell へ丸投げなので、
   **エクスプローラーの結果がそのまま mIV の結果**になる。実装せずに半分決まる。
2. **実素材で `seek_strip_batch` を回す。** これは §1.102 (backlog-on-hold) の
   「MPEG-TS など `TimeGrid` 経路を実素材で確認できていない」という**積み残しそのもの**で、
   22,811 件の sweep にも MPEG-TS は含まれていなかった。利用者の `.mts` はその代表格。
   - ⚠ **先に dev bin の拡張子フィルタを広げること。** `is_supported_video`
     ([seek_strip_batch.rs:466](../src/video/seek_strip_batch.rs:466)) が
     `SUPPORTED_VIDEO_EXTENSIONS` を見るので、**今のままでは 0 件で終わる**。
     開発用 bin なので製品の一覧ポリシーに縛る理由は無い。
   - 素材は `samples.ffmpeg.org` の `/HDTV/` (放送キャプチャの `.ts` が多数)、`/MPEG2/`
     (壊れ方で選ばれた検体) からも引ける。**自前生成 (`ffmpeg -c copy` で TS 化) は
     PTS が単調な「きれいな TS」になり、確かめたい失敗をそもそも踏まない**ので、
     生成するなら継ぎ目 (バイト連結) / `-output_ts_offset` / 尺の申告ずれまで作る。
3. 1 と 2 が揃って初めて、**3gp だけ足す**という中間案が検討できる。
   **3gp は ISO-BMFF で索引を持ち、不確実は Shell のサムネイルだけ**なので、
   再検討するならここから。`.mts` / `.ts` は素材の当てが立つまで対象外。

#### 見送る側の論拠 (判断材料として残す)

一覧に出した時点で「サムネイルが出ない」「シークがずれる」「ストリップが空」は
**mIV の不具合として報告される**。実際はコンテナの性質と Shell 側の話でも、切り分けは
毎回こちらに来る。**手元で確認できない形式を出すと、その窓口が開く。**

- 規模 / 優先度: 拡張子追加のみなら Small。**今回は実施しない。**
- 補足: `.ts` 単体なら `samples.ffmpeg.org` から実素材を引ける。入手できないのは
  **AVCHD の `.mts`**。見送りの理由を「素材が手に入らない」だけに置かない。

### 1.235 migemo (ローマ字インクリメンタルサーチ) は採用しない — 見送り判断の記録 (2026-09-13)

- 出典: 利用者要望 (2026-09-12)。
- **方針 (2026-09-13 開発者判断): 採用しない。** 再提案しない。以下は調べた内容の記録で、
  同じ調査を繰り返さないために残す。

#### 実装の選択肢

- **rustmigemo** (oguna) が実質唯一。`src` は MIT (Cargo.toml に `license` フィールドは無い)。
  **crates.io に公開されていない**ので git 依存か vendoring になる。依存は `byteorder` のみ。
- **C/Migemo** (koron) は MIT だが、Rust binding のクレートは見当たらない。
- 辞書 `migemo-compact-dict` は 2.1 MB。**サイズは論点にならない** (Lindera の 13 MB より小さい)。

#### ライセンス

- **根は SKK-JISYO.L で、ヘッダに GPL v2-or-later と明記されている。**
  migemo-compact-dict-latest は「SKK辞書から生成されているため…GPLとして配布」(GPL-3.0)、
  cmigemo も "The built dictionary is subject to the GPL" と書いている。
- **mIV 本体が GPL になるという話ではない** (辞書は実行時に読むデータで、リンクしない)。
  問題は**辞書を配ると GPL の条件が辞書に付いてくる**こと: ライセンス全文の同梱と、
  対応ソース (SKK-JISYO.L + 変換ツール) の提供を**版ごとに続ける**必要がある。
  FFmpeg の LGPL 対応ソースと同種の運用がもう 1 系統増える。
- exe へ `include_bytes!` で焼き込むと、GPLv3 §5 の「媒体の上に置いた別個の著作物
  (aggregate)」という説明が**隣に置く場合より苦しくなる**。ただし**義務の有無は
  焼き込んでも隣でも変わらない**。消えるのは「利用者が自分で設置する」形だけ
  (テキストエディタ Mery が C/Migemo でこの方式を採っている)。

#### 性能 — Ctrl+G と構造が合わない

**rustmigemo の公開 API は正規表現の文字列しか返さない** (`query` / `query_a_word`)。
trie 圧縮された `(kensaku|けんさく|検索|憲[作冊]|…)` の形で、**候補数の上限は無い**。
単語列が要るなら `CompactDictionary::predictive_search` を直接叩くことになる。

**bigram 索引は「語」を要求するので、migemo を安くしている正規表現の圧縮が一切効かない。**
SKK-JISYO.L (okuri-ari を除く 133,438 件) から概算した leaf TermQuery 数
(候補語ごとの bigram AND × 7 フィールド、[fts_index.rs:514](../src/fts_index.rs:514)):

| 入力 | 候補語数 | leaf TermQuery | 現状比 |
| --- | ---: | ---: | ---: |
| `shi` | 14,056 | 191,240 | 9,107x |
| `ken` | 1,350 | 18,410 | 877x |
| `nihon` | 443 | 11,172 | 532x |
| `kensaku` | 34 | 602 | 29x |

現状は 4 文字の英数字 1 語で **21 個**。**打ち始めの 1〜3 文字が最悪で、インクリメンタル
なので必ずそこを通る** (0.3 秒 debounce、[global_search_ui.rs:136](../src/global_search_ui.rs:136))。
**Tantivy の実時間は測っていない**。上は構造から出る倍率。

- **Ctrl+F なら安い。** 最小長の検査が無く ([app.rs:51031](../src/app.rs:51031))、照合は
  `hay_lower.contains` ([search_query.rs:154](../src/search_query.rs:154)) を regex へ
  差し替えるだけ。支配的なのは EXIF / PNG のファイル読み。`regex` は既に依存にある。
- **Ctrl+S は候補展開ではなく regex で。** `name LIKE '%…%'` を N 本 OR にすると 1 行あたりの
  仕事が N 倍になる ([search_index_db.rs:380](../src/search_index_db.rs:380))。rusqlite の
  スカラ関数で行ごとに 1 回 regex にすれば 1 倍だが、**`functions` feature が今は無い**
  ([Cargo.toml:367](../Cargo.toml:367) は `bundled`, `hooks` のみ)。

#### 見送りの理由 (利用者の同意あり)

mIV は画像ビューアでキーボード専従ではなく、日本語が効くのはファイル名とタグが中心。
AI プロンプトはほぼ英語、EXIF はほぼ英数。**得られるものに対して、ライセンス運用と
Ctrl+G の作り替えが釣り合わない。**

### 1.141 Susie のクラッシュ対象 ID が「エントリ名 + 長さ」止まり

- 出典: 2026-08-27 の出荷前レビュー (Codex、機能別評価の Susie 行)。
- 機構: [susie_loader.rs](../src/susie_loader.rs) の `decode_bytes` は、クラッシュした
  入力を二度と同じプラグインへ渡さないための識別子を
  `format!("{filename_hint}#{}", bytes.len())` で作る。ZIP 内画像はパスを持たないため、
  名前だけでは別書庫の同名エントリを巻き添えにする — その対策として長さを足した経緯が
  コメントに残っている。
- 残る穴: **同名かつ同サイズで中身が違う**エントリは、まだ同一視される。
  片方でプラグインが落ちると、もう片方も開かれなくなる。
- 実害の大きさ: 落ちるのは既にクラッシュした後の話で、結果は「開けたはずの 1 枚が
  開かれない」。クラッシュや破損ではない。**同名同サイズは再配布された同一ファイルである
  ことが多く、その場合は巻き添えではなく正しい判断**になる。
- 直し方の候補: バイト列のハッシュを識別子に含める。全体ハッシュは decode 経路 (数 MB) に
  数ミリ秒を足す。先頭 + 末尾の固定長だけを混ぜる案なら実質ゼロだが、**どちらも実際の
  クラッシュを再現できないと検証できない**。
- 2026-08-27 の判断: **出荷前には入れない。** 失敗は限定的で自己修復可能 (再起動で解除)、
  一方で修正は decode のホットパスに触れ、実機で確かめる手段が無い。
- 規模 / 優先度: 小 / **P3**。

### 1.140 超広幅ウィンドウで波形ストリップの鮮鋭さが落ちる (分割描画の検討)

- 出典: 2026-08-27 の出荷前レビュー (Codex P2) をきっかけに判明。
- 経緯: `WaveSpanRequest::pixel_width` は「可視幅そのものが GPU 上限 (8192px) を超えても
  丸めない」を明示的な設計としており、コメントは**「テクスチャ生成側の責務として残す」**と
  書いていた。しかし [render_core.rs](../src/video/native_presenter/render_core.rs) の
  `sync_seek_strip_textures` は分割も縮小もせず `load_texture` を呼ぶだけで、
  **その責務は実装されないまま残っていた**。到達すれば必ずレンダースレッドが
  パニックし、以後どの動画も再生できなくなる (§1.135 と同じ終わり方)。
- 2026-08-27 の対応: 上限を要求側で無条件に効かせた
  (`an_oversized_visible_width_is_capped_at_the_texture_ceiling`)。
  **パニックは消えるが、可視幅が 8192px を超える構成では波形がわずかにぼやける** (伸縮のため)。
  位置は時刻由来の UV で決まるのでずれない。
- 残っていること: 鮮鋭さを取り戻すなら**分割描画**しかない。RGBA を N 枚のテクスチャへ分け、
  `waveform_texture_slice` の UV をタイル境界で割り、N 回 `painter.image` する。
  継ぎ目と、raster revision ごとの N 枚アップロードのコストを見る必要がある。
- 到達条件: 可視ストリップ幅 (物理ピクセル) が 8192 を超える構成。4K 2 面 (7680) では届かず、
  3 面またぎや 8K で届く。**開発機の仮想デスクトップ幅は 6001px なので、ここでは再現しない。**
- 規模 / 優先度: 中 / **P3** (パニックは解消済み。残るのは限られた構成での画質)。

### 2.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

### 3.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 3.4 2x AIアップスケールモデルを追加するか比較する — 専用スレ >>392 (2026-09-14)

- 要望: 4x より処理負荷を抑えられる 2x の AI アップスケールを選びたい。
- 現状:
  - mIV の UI に出る 5 モデルはすべて 4x。高速汎用は `realesr_general_x4v3`、漫画向けは
    `realcugan_4x_conservative` を同梱している。
  - 公式の 2x 候補には `RealESRGAN_x2plus` と `Real-CUGAN 2x` があるが、倍率を設定で変える
    ものではなく、別の学習済みモデルを追加する必要がある。
  - Real-CUGAN は現行の高速汎用モデルより重い系統なので、2x にしても高速汎用 4x より速いとは
    限らない。mIV はモデルを配布物へ同梱するため、採用モデルごとに配布容量も増える。
- 再検討条件:
  - 代表的な写真 / イラスト / 漫画で、現行の高速汎用 4x と 2x 候補を同じ最終表示サイズに揃え、
    画質、初回時間、連続ページ送り時の時間、VRAM / RAM、モデル容量を比較する。
  - 「2x だから軽い」という推測だけでは追加せず、現行モデルに対する明確な用途または速度上の
    利点が確認できた場合に採用を判断する。
  - 採用する場合は倍率をモデル名 / UI に明示し、自動選択、AIキャッシュキー、モデル配布・
    ライセンス表記、旧版との設定互換を確認する。
- 関連: [AI処理のサイズしきい値計画](ai-processing-size-threshold-plan.md#将来の-2x-モデル候補)。
- 規模 / 優先度: 比較 Small〜Medium、採用 Medium / P3。現時点では比較・採否判断待ち。

### 1.227 最大化したF12別窓を開く直後、画像とHUDが一瞬横へ伸びる — 利用者報告 (2026-09-12)

- メイン・別窓ともタイトルバー最大化で画像を開くと、最初だけ横に引伸ばされてから正常fitへ戻る。
  録画17:00:35.527/.561の2フレーム、.594から正常。旧§1.115の連続ちらつきとは区別する。
- ログはwindow_id=123、復元サイズ1180×1140/maximized=true、hidden host1792×1766、次frameまで74.1ms。
  復元サイズのpresent→maximizeによるclient拡大→次のsurface resize/paintまでDWMで旧描画を拡大する
  仮説がHUD全体の変形と整合する。GPU/DWM traceによる厳密な因果確認は未実施。
- 修正には最大化後の物理サイズ・host/surface世代・present成功・可視化の状態管理が必要となり得る。
  maximize/visible順の入替だけでは既存の白露出防止を崩すリスクがある。共通native lifecycleに及ぶ中〜高リスク。
- 利用者は単発のため低優先、リスクが高ければ次版から除外することを希望。親と独立レビューは保留で合意。
  製品コード変更なし。§1.115の旧連続ちらつき非再現・完了判断は維持する。
- [調査記録](section227-maximized-first-frame-stretch.md)。次のリリースには含めない。

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

## 後続リリースへ延期（2026-09-11）

### 1.216 自動余白カットの全体既定と本別の継承設定 — 利用者要望 (2026-09-11)

- **後続版へ延期（2026-09-11 利用者指定）・設計レビュー済み／未実装**。利用者から「自動余白カットは常にONにできないか」と質問。
  現状は本ごとに自動を選択して記憶する方式で、未設定の本にも適用する全体既定がない。
- 承認済み仕様:
  - 環境設定に「自動余白カットを標準で有効にする」を追加。初期値はOFFで従来表示を維持。
  - 本ごとは「全体設定に従う／トリムなし／自動余白カット／手動設定」。明示設定を優先し、
    全体に従う本は全体設定の変更にも追従する。
  - 既存の本別自動・手動設定、ページ個別の手動値を保持。自動検出アルゴリズムは変更しない。
- 移行上の注意: 現行 `ViewTrimBookState::is_removable` はトリムなし＋既定値の行を削除するため、
  過去の明示OFFと未設定は区別できない。新形式では継承と明示OFFを区別し、既存記録から
  復元できない選択を推測して捏造しない。全体既定OFFのままなら既存表示を維持する。
- 完了条件: 本別優先順位と再起動後の保存、全体変更への追従、既存データ移行を回帰検証。
  通常フォルダ・ZIP/PDF・連結読み・別窓・リモートで同じ解決を使い、既存の手動トリムを壊さない。
  独立レビュー、必要な自動検証、確認用ビルドまで実施。実機操作は別途了承された検証枠で行う。
- 進行: 設計・独立レビューまで完了。規模を考慮し今回のリリースから外す。再開時は [設計書](auto-trim-global-default-plan.md) と共有ファイル所有を確認する。


## 今回のリリース後へ延期（2026-09-14 利用者決定）

§1.239と§1.240は同じ次回以降の作業として検討し、現在のリリースには含めない。以下の要件は保持する。

### 1.240 本の先頭・末尾付近へ表示上だけの白紙ページを挿入する — >>402 (2026-09-14)

- 出典: >>402。電子化時に省かれた表紙裏などを補い、見開きの左右関係を実本に合わせたいとの要望。
- 利用者向け仕様:
  - 「先頭ページの次へ白紙を挿入」「最終ページの前へ白紙を挿入」を個別にON/OFFできるようにする。
  - 実ファイルは作らず、閲覧中のページ列にだけ白紙を加える。白紙は見開き構成とページ送りには参加する。
  - 2つの切り替えを操作カスタマイズへ追加し、任意のキーから個別にON/OFFできるようにする。
- 実装前に、仮想白紙上でのシーク表示、ページ番号、読書位置、ブックマーク、編集操作の扱いを
  型付きのページ構成として確定する。白紙を実在する `GridItem` や偽のファイルパスとして扱わず、
  レーティング・タグ・編集など実ファイルを必要とする操作の対象にしない。
- §1.218の単ページ片側配置、末尾表紙補助、横長ページ分割、縦連結読み、F12別ウィンドウ・
  複数ウィンドウ、mIV Remoteで同じページ構成を使い、組み合わせごとの順序を回帰確認する。
- 現在進行中のコレクション機能が落ち着いた後の版で着手する。
- 規模 / 優先度: Medium-Large / P3。ページ列そのものを変えるため、表示だけの描画変更より影響が広い。
- 関連: 将来の §1.242「本の構成」で、この白紙挿入を含むページの並び全体を本ごとに編集できるようにする予定。

### 1.239 見開き端の単ページ配置を先頭・末尾で個別設定できるようにする — >>398 (2026-09-14)

- 出典: >>398。先頭の表紙は中央に保ち、末尾の単ページだけ本来の側へ寄せたいとの要望。
- §1.218で実装した一括設定を、先頭と末尾の2項目へ分ける。環境設定と本ごとの設定の両方で、
  `全体設定に従う / 片側へ配置 / 中央表示` を端ごとに独立して選べるようにする。
- 既存の一括設定がONなら先頭・末尾ともON、OFFなら両方OFFとして移行し、更新だけで見た目を変えない。
  本ごとの既存設定も同じ規則で両端へ引き継ぐ。
- 配置geometry自体は§1.218を再利用する。設定DB、`spread.db`、メタデータ転送、viewer context、
  mIV Remoteの読書設定を両端に分け、片方の変更がもう片方へ影響しないことを確認する。
- 回帰確認: 先頭のみ / 末尾のみ / 両方 / どちらもなし、本別上書き、左右開き、1ページ本、
  末尾表紙補助、縦連結読み、F12別ウィンドウ・複数ウィンドウ、mIV Remote、旧設定からの移行。
- 規模 / 優先度: Medium / P2。描画処理は小さいが、永続化と本体・Remote間の契約を分ける必要がある。

## §1.239 / §1.240 のリリース後に着手（2026-09-15 利用者決定）

§1.239・§1.240 を先にリリースし、その後に着手する。規模が大きいため §1.240 とは別の節として積む。

### 1.242 本の構成 — ZIP / PDF / 本フォルダごとにページの並びを非破壊で編集する (2026-09-15)

- 出典: §1.240 (>>402) の検討から利用者が発展させた構想。スキャンミスで入れ替わったページの補正、
  複数の本を 1 ファイルにまとめた本の左右合わせなど、本ごとに違う崩れを直したい。
  マンガミーヤにも同種の機能 (ページ編集での白紙挿入・削除・切り取り / 貼り付け。書庫は書き換えず
  リスト情報として保存) があった。
- 着手条件: §1.239 と §1.240 のリリース後。
- 利用者向け仕様 (2026-09-15 利用者決定):
  - 構成は ZIP・PDF・本フォルダに個別に作る。入れ子の ZIP は閲覧と同じく**階層ごと**に作る。
    **製本は対象外**。変換して読む RAR 等は**元の書庫側**に付ける。
  - 構成がある本は、通常の表示設定より構成に従う。**見開き設定 (表紙あり / なし、右開き / 左開き) も
    構成に含める**。構成がある本での見開き設定の切り替えや Ctrl+←/→ のずらしは一時操作とし、
    開き直すと構成の状態に戻る。
  - 構成の編集でできること: ページの並べ替え / この本での非表示 (ファイルは消さない) /
    任意の位置への白紙の挿入 (表紙裏、話数の途中、まとめた本の境目など) / 見開きの組の変更 /
    単ページの配置。
  - 編集を始めた時点で、そのときの自動の規則 (表紙の単独表示、横長の単独表示、末尾表紙、
    単ページの片側配置、§1.239・§1.240 の設定など) を当てた並びを初期値にし、以降は手動で編集する。
  - **一覧 (サムネイル) も構成の順**にし、非表示ページは出さない。非表示ページなど元の状態は
    構成の編集画面でだけ見える。
  - 非表示ページをしおり・検索・評価一覧・読書位置などから開こうとしたら、エラーのトーストを出して
    開かない。
  - 構成に無いページ (本フォルダに後から増えたファイル) は末尾に追加する。
  - 一覧での一括書き出し・製本への追加は、構成の順に並んだ一覧で選んだものを対象にし、非表示ページは
    対象にしない。出力順は構成の順が望ましいが、元の順でも許容する。
  - **横長画像の分割は構成に含めない**。閲覧時のモードとして独立させる (mIV Remote の端末ごとの設定
    「縦長画面では見開きを解除する」`crates/remote-web/web/app.js:11536` と同じく、本のデータではなく
    見方の設定として扱う)。
  - 非表示は「削除」と呼ばない。既存の削除は実ファイルをごみ箱へ送る。
- 設計前提 (2026-09-15 のコード調査。未実装・設計レビュー前):
  - **構成は本のアイテム列を作る時点で適用する** ([§1.14 の本の読み順](archive/folders-archives/book-page-order-plan.md)
    を名前順固定から構成順へ広げる形)。一覧・読み順・見開き・シーク・連結読み・F12 別ウィンドウ・
    一括書き出し (`grid_selection_indices` は items の番号順に並べる、`src/ui_fullscreen.rs:42759`) が
    すべて items に従うので、揃えて変わる。非表示ページは items に存在しないので、到達時にトーストを出す
    だけで済む。
    - 読み順のレイヤーだけで適用すると、読み順を独自に作り直している箇所 (単ページの次 / 前、
      スライドショーの先頭戻り、Ctrl+↑↓ の着地、閲覧履歴の位置、F12 の表示列、mIV Remote) と食い違う。
    - `visible_indices` は昇順前提の `binary_search` で使われている (`src/app.rs:112` ほか)。items 自体を
      構成順に作れば昇順のまま保たれる。
    - 末尾表紙・片側配置の適用条件は「読み順の枚数 = items の数」(`spread_complete_page_permutation`、
      `src/ui_fullscreen.rs:15596`)。非表示ページが items に無ければ成立したままになる。
  - **下準備: 本のアイテム列を作る処理の一本化**。ZIP 系 (`finalize_zip_enumerate` /
    `zip_nav_show_current_level` / `zip_nav_handle_ctrl_updown` / `zip_nav_dfs_fullscreen`)、画像だけの
    フォルダ、PDF 列挙に加え、しおりとスナップショットのジャンプ (共通の `resolve_archive_bookmark_target`
    が階層を作り直す、`src/book_bookmarks.rs:1554`、`src/app/snapshot_ops.rs:1775`) と mIV Remote
    (`src/remote_ipc/container.rs` の列挙) が別に列を作っている。Remote の `enumerate_zip`
    (`container.rs:4605`) は本体と同じ並び順の定数で階層を作るが `arrange_grid_items` は通らない。
    これで本体と並びに差が出るかは未確認。
  - 構成の編集画面は items を触らずに元の並びを別に作って構成を重ねる。保存したら本を読み込み直し
    (items の世代を進める)、表示中のページへ `PageIdentity` で戻る。items をその場で並べ替えないので、
    items の番号をキーにしたメモリ上の編集状態はずれない。
  - **白紙は構成の要素として持ち、`GridItem` や偽のパスにしない** (§1.240 と同じ)。構成は「ページ /
    白紙」の並びとし、白紙と組んだページは「1 枚 + 空き側」として描いて `SpreadPair::Single` へ
    解決する (§1.218 の片側配置と同じ扱い)。こうすると編集・書き出し・取り込み・外部ツールは白紙を
    一切見ない。
    - 白紙を見開きの片側の「ページ」として持つと壊れる箇所: 見開きの型が実在ページの番号 2 つを要求
      (`SpreadPair::Double`、`SpreadPageOccurrence.idx`)、編集ツールの入口は見開きなら左ページを選んで
      単ページへ切り替える (`plan_page_edit_pivot`、`src/app.rs:9303`)、見開きの描画は片側のテクスチャが
      無いと両側とも出さない (`src/ui_fullscreen.rs:38470`)、書き出し・取り込み・外部ツールの合成。
    - 白紙は一覧と単ページ表示には出ない。見開きの組とページ送りの単位にだけ効く。
  - 見開きの組み立て (`build_spread_display_units_with_predicates`) は「隣接 + 横長判定 + セッション限りの
    ずらしアンカー 1 個」しか表現できない。構成の組と白紙を入力に取れるようにする。見開き単位の
    キャッシュの token と読み順キャッシュの鍵 (`items_generation`, `reading_flow`) に構成が入っていない。
  - 読書位置 (`book_resume.db`) は items の番号で保存しているので、構成を保存したら識別子で解決し直す。
  - 保存先は本ごとの独立ストア。`rename_key_migration` の `STORES`、削除時の purge、メタ情報エクスポート
    (`metadata_transfer` の版上げ)、内容 ID による復元、F12 のバンドル、mIV Remote に登録する。構成の中の
    ページ識別子は 1 ファイルのリネームでは書き換わらないので、しおりと同様の個別移行
    (`src/book_bookmarks.rs:1479`) か、解決時に欠落を許容する。
  - 変換した書庫のキーはストアによって割れている (ページ単位の編集と `spread.db` はキャッシュ ZIP 側、
    しおり・表紙ピンは元の書庫側)。構成は元の書庫側に付ける前提で、対応付けを設計で確定する。
  - 表示トリムの本単位の見開き左右別設定 (`ViewTrimBookSettings.spread_separate`、`src/view_trim.rs:163`)
    が、組を変えたときにどのページへ掛かるかを設計で確認する。
- 規模 / 優先度: Large / P3 (将来)。本のアイテム列の一本化と mIV Remote の列挙統合が工数の本体。
