# 次リリース検討バックログ

このファイルは、まだ着手していない作業候補だけを置く恒久バックログ。
完了した項目はコミット履歴・リリースノート・個別設計メモに任せ、このファイルからは削除する。

運用ルール:

- 着手前に `docs/README.md` から該当領域の設計ドキュメントを読む。
- 着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから削除する。
- 判断保留・見送りの理由は、次に再判断する人が困らない最小限だけ残す。
- 依存ライブラリ更新は `CLAUDE.md` のリリース手順チェックリスト Phase 2 と整合させる。

---

## 1. 優先候補

> **v2.13.0 の出荷範囲 (利用者判断 2026-08-08)**。タッチ対応 (§4.7) の完了後、
> 次の 10 件を入れてからリリースする。ここに無い項目は次版へ回す。
>
> - **バグ**: §1.61 (見開きダブり、P1 相当) / §1.54 (ツリーペイン幅) / §1.57 (色ピッカー) /
>   §1.60 (ワイプ境界) / §1.56 (ルーペ端) / §1.58 (ページ送りの引っかかり) /
>   §3.3 (idx キャッシュの世代刻印)
> - **待たされていた要望**: §1.55 (ナビゲータ 2 件。本文に「タッチ後に着手」と明記済み) /
>   §4.8 (名前の変更・更新コマンド) / §4.9 (削除確認の矢印キー)
>
> 着手順の申し送り:
>
> - §1.61 は**着手前に「詳細表示だけの問題か」を確定する**。数百枚のフォルダなら
>   サムネイル表示でも再現する見込みで、影響範囲が変わる。再現データは
>   `C:\tmp\miv-spread-test\` に生成済み
> - §1.60 と §1.55-2 は同じ比較描画経路を触るので**まとめて着手する**
> - §1.59 (360 度魚眼) は**今回対象外**。採否と写像の選択は次版で再判断する

> **v2.10.0 で修正済み・未リリース (2026-08-02)**。出荷時にこのブロックごと、`対応済み` と
> 付いた項目本文も削除する (§1.29 を消し忘れた再発防止)。
>
> 全部入り (項目本文を既に削除したものを含む):
>
> - §1.22 — 別ウィンドウ入力中にキーがそちらへ行き続ける (本文削除済み)。
> - §1.32 **(A) / (B)** — 変換中 ESC の focus loss と未確定文字残留。
> - §1.33 — 本として扱わないフォルダで既定の見開きが適用される (本文削除済み)。
>   既定値のフォールバックだけ止め、明示保存された値は尊重する仕様で実装した。
> - §1.34 — パスワード入力中のキー識別情報が診断ログに残る。
> - §1.35-**1** — リネームジャーナルの UI スレッド同期 I/O。**2 (部分失敗の再試行) は残る**。
> - §1.36 — ゲームパッドの文字入力ゲート (下記のとおり 3 面へ訂正して実装)。
> - §1.37-**1** — トレイ WM_PAINT の背圧。**2 / 3 (processor cache / hide 所有者) は残る**。
> - §1.38 **の「変更中...」表示のみ** — モーダル span の構造 (§1.31 と合流) は未対応で残る。
> - §1.39-**1** — `update_prefetch_window` の双子の非対称。**2 の PDF 毎フレームコストは残る**。
> - §1.40 — 閲覧履歴 (動画・音声を含める / 自動進行由来は記録しない / 改名)。
> - §1.41 — 静止画縮小表示のシャープ優先 / モアレ抑制切替。
>
> **known-issues.html から消えるのは 3 見出し**: 入力・キー操作の 2 件と見開き。
> 残るのは MPEG シーク (§1.13)。VST カーソル (§1.28) は v2.10.0 で対応済み。

### 1.7 detached 中の発火面解決の残り (? / トースト / スタック) — BA 報告

- 残り (P3、発火面の window_id 粒度が必要):
  1. **? キーのショートカット一覧**: detached 中にグリッドから開くと FS 用一覧が
     出る。修正には発火元の面情報を consume サイトから通す必要がある。
  2. **トーストの面粒度**: 発火面のビューアを閉じた後に完了したバッチのトーストが
     不可視のまま消える / active detached + main fullscreen 同時表示時に Viewer 面が
     両ビューアに出る (2 値 enum の粒度の限界)。許容中。
  3. **複数ウィンドウ×スタックの理想形**: スタックをクリックするとメイン一覧が
     フラット読書ビューに切り替わる (フル機能と同じ挙動)。理想は「メインは集約のまま、
     窓側だけフラット文脈を持つ」だが、窓が自前のフラット items 文脈 (bundle 化された
     stack 状態) を持つ設計が必要 (複数ウィンドウの bundle 化と同系統)。現行でも
     Shift+↓↑ ジャンプは動作する。
- 優先度: P3。

### 1.9 parked 窓のリソース制御 (サムネ pipeline 停止 / VRAM 合算予算) — 角度⑥送り

- 出典: v2.3.0 角度⑥レビュー (Sol/Terra 一致、docs/archive/review-v2.3.0/sol-angle-reviews.md)。
  いずれも bounded な効率問題でデータ喪失は無し。park/再活性ライフサイクルの構造に
  踏み込むため、リリース直前パッチではなく detached リワーク後続で設計対応する。
  1. **P2: park してもサムネ pipeline が止まらない**: `pause_background_work_keep_current_frame`
     は fullscreen/AI 系のみ cancel し、bundle の `cancel_token` / `reload_queue` /
     `heavy_io_queue` に触れない。park 時点で積まれていたデコードが走り切り、結果
     `ColorImage` が誰も poll しない rx に溜まる (窓の再活性化 / close まで保持)。
     対応案 = park 時に queue を drain (worker pool は殺さない)。ただし Requested 状態の
     サムネが再活性化時に再要求される仕組みの確認が必要 (state が Requested のまま
     queue から消えると復帰後にロードされない恐れ)。
  2. **P2: サムネ VRAM 上限が文脈単位**: `update_keep_range_and_requests` の予算は
     mounted 文脈にしか効かず、parked bundle N 個がそれぞれ上限近くまで保持し得る
     (動画サムネは eviction 対象外なのでフォルダサイズ分)。対応 = 全 bundle 合算の
     予算会計 + cross-bundle eviction (リワークの資源予算ステージ)。
  3. P3: `display_px_shared` が App-global のため、detached 文脈のデコード解像度が
     メイングリッドの表示密度に引きずられる (適用ミスは無し、品質/CPU の無駄のみ)。
- 関連: v2.3.0 で対応済みの境界 = bundle Drop の解放 (角度①でクリーン確認)、
  文脈別 channel/cancel/世代 (P2-9)。

### 1.10 終了時の削除 worker 未調整 — 角度⑤送り (P2、実害小)

- 出典: v2.3.0 角度⑤ Sol P2。数百件削除の実行中に終了すると、実行中の
  IFileOperation チャンクは完走するが後続チャンクは開始されず、部分削除の最終報告と
  完了後クリーンアップ (resume purge 等) が走らない。一覧は次回起動の走査で実態に
  収束するため破壊は無し。対応案 = 終了時に cancel を立てて現行チャンクの完了を
  短時間待つ + 次回起動時の注意トースト。v2.2.0 出荷時からの既存挙動。

### 1.12 detached 静止画窓から音声/動画へのフォルダ内ナビ不可 (メディア昇格導線)

- 出典: v2.3.0 実機検証 §5 #33 (2026-07-10)。別窓 (静止画) で ↑↓ ナビ中、音声/動画の
  アイテムへは移動できない (静止画窓はメディア再生セッションを持てない設計境界)。
  音楽ビュー→画像の方向は移動できるため非対称。ユーザー許容済み (「一旦これでもよさそう」)。
- 対応案: detached リワークの後続ステージで「静止画窓がメディアに到達したらメディア
  セッションへ昇格 (または既存メディア窓へ委譲)」の導線を設計。凍結ルール下では
  症状パッチを入れない。

### 1.13 duration 不明/不正 MPEG-PS のシークバー不能

- 背景: `sample.mpg` で実機確認済み。duration が不明または不正な MPEG-PS は
  シークバーで移動できない。VLC は同じファイルをバイトシーク + PTS 再スキャン方式で
  シークできる。
- 対応案: decode EOF で実尺を学習して `VideoInfo` / HUD の duration へ反映するか、
  duration を信頼できない場合にバイト位置シークへフォールバックする。
- 裁定: v2.2.0 以前から同挙動で、ユーザー裁定により次版送り。優先度 P3。
  final-report 追補 5 の P3 参照。

### 1.17 UI レイアウト崩れ (文字はみ出し / 見切れ) の自動検出 QA → その後に英語対応を再検討

- 背景: 英語対応 (UI ローカライズ) を検討したが、AlternativeTo は「英語 UI 必須」
  (FAQ: DB 登録アプリは英語対応が必須) のため、日本語のみの現状では掲載できない。
  一方、多機能ゆえに英語 UI を実機で全画面目視 QA するのは負担が大きく、しかも日本語でも
  文字はみ出し / 見切れのレイアウトバグが繰り返し発生している。そこで **まず言語非依存の
  「レイアウト崩れ自動検出」QA ツールに投資**し、日本語バグに即効かつ将来の英語化の
  「目視 2 倍」問題を先に解消する。英語対応の可否はその後に再判断する。
  (2026-07-13 相談。ユーザー方針: QA 投資を先行 → その後に英語対応を再検討。今は別作業中のため別途着手)
- 現状 (確認済み):
  - egui は文字配置時に galley サイズが取れ、painter は clip rect を持つため
    「galley 幅 > 割り当て幅」で見切れを機械判定できる。
  - UI スナップショット基盤 `tests/ui_snapshot.rs` (egui_kittest) が既にある (回帰検出向け)。
  - 文字列は現状ハードコード (i18n 未 externalize)。見切れ計装は現行日本語のままでも動く
    ので、i18n とは分離して先行できる。
- 方針 (QA ツール先行 → 英語再検討):
  - QA ツール (言語非依存・日本語バグにも効く):
    1. **見切れ計装**: 描画時に galley 幅 vs clip/available 幅を比較し、超過した文字列 +
       ウィジェット位置をログ列挙する。全画面を人が見なくても崩れ候補の一覧が出る。
    2. **疑似ローカライズ (幅ストレス)**: UI 文字列を ~1.4x 長い擬似文字列に置換したビルドで
       幅不足のウィジェットを炙り出す (英語が長くなる前提の先行テスト。日本語のままでも
       将来はみ出しやすい箇所を潰せる)。
    3. 既存 egui_kittest スナップショットを併用 (変化検出。最終は差分画像を目視)。
    4. (任意) スクショ自動キャプチャ + ビジョン判定で「切れ / 重なり」を triage。
    - 限界: 得意なのは見切れ / はみ出し / 行あふれ。位置ズレ・不格好な折り返し・パネルの
      重なりは取りこぼしがあり、多少の目視が残る。
  - 英語対応 (QA ツール整備後に可否判断):
    - 前工程 = 文字列の externalize (i18n 化)。
    - AI 一括翻訳 + **用語集** (見開き=Two-page spread 等を先に固定) + 各文字列の文脈付与で
      用語ゆらぎ / 品詞ズレを抑える。
    - **変数整合チェック**: `%s` / `{}` / 改行 の個数・種類が元と英訳で一致するか機械照合。
    - **逆翻訳 QA**: 重要文字列 (削除 / 上書き / 警告 / エラーダイアログ) に限定して英→日へ
      訳し戻し、元日本語と比較する。英語力が無くても意味ズレを検知できる (日本語同士の比較)。
    - 実機確認は見切れ計装 + 疑似ロカライズで大半を機械化し、目視は残差のみ。
    - "English is machine-assisted; feedback welcome" と明示し、OSS の Issue / PR で継続改善。
- AlternativeTo 連携: 英語 UI が入れば掲載可。申請素材 (Tagline / Description の v2.3.0 版
  = 音楽再生 + VST3 入り / Website / Source / License Free+OSS(MIT) / 画像 = icon.png + ss_*.png
  含む ss_music.png / Alternative to = NeeView・MangaMeeya・Leeyes・ZipPla・Honeyview・
  ImageGlass・IrfanView・XnView MP・nomacs・FastStone) は準備済み。GitHub Topics / 英語 README
  サマリ / egui Showcase / awesome-egui PR は UI 言語非依存の枠なので実施済み。
- 規模 / リスク: QA 計装 = Medium (描画経路への hook + 幅バジェット定義)。英語化 = Large
  (externalize + 翻訳 + QA 工程)。QA ツールは日本語バグにも効くため単独で投資回収がある。
- 優先度: P2 candidate (QA ツール)。英語対応は QA ツール整備後に再判断 (現時点は保留)。

### 1.21 自動選定した親コンテナ代表サムネイルに編集プレビューを反映

- 出典: v2.5.0 リリース前実機確認 (2026-07-17)。ZIP 内画像を編集しても、親階層の
  ZIP セルで自動選定された代表サムネイルには編集結果が反映されない。
- v2.6.0 で手動ピン経路は対応済み。cascade 解決後の固定 leaf (直接 Image / ZipEntry /
  PdfPage、ネスト ZipDir、変換アーカイブ) は canonical page key を親要求へ渡し、編集
  preview を catalog より優先する。固定 leaf の個別色調補正も注釈前の下地へ適用し、
  保存 / 無効化通知を親セルへ伝播する。
- 残件は **自動代表選定だけ**。worker 内で leaf を選定した後に page key を組み立て、
  対応する編集 preview があれば raw decode / catalog より優先する。自動選定結果と親
  キャッシュの無効化条件も同時に設計する。
- 不変条件:
  - 現行の編集 preview 内容 (erase / local_adjust / conceal / crop / comic 注釈) だけを
    自動選定 leaf へ反映する。自動代表へどの色調補正を適用するかは別途決め、手動固定済みの
    個別色調補正規則を変えない。
  - UI スレッドへ ZIP/PDF 列挙や SQLite 単件照会を追加しない。
- 規模 / 優先度: Medium〜Large / P3。

### 1.28 対応済み: presenter の上に別の窓が乗るとカーソル auto-hide が解除されない

- **対応 (2026-08-02、v2.10.0 に含まれる)**: commit `e1940617`
  「Ask which window has the pointer, not where the pointer is」。幾何判定の
  `cursor_within_client` を、pump が所有する typed router (`NativeCursorOwnershipEdge` /
  `CursorOwnership` イベント) と auto-hide reducer へ置き換えた。下記の対応案どおりの
  構造的修正。detached-rework-plan.md の 2026-08-02 行にも記録あり。
- **v2.10.0 リリース時に、手順 Phase 1 の 6.5「既知の問題ページから、この版で直した項目を
  削除する」が漏れていた。** v2.11.0 で known-issues.html から削除する。


- 出典: v2.9.1 リリース前の §7.2 実機確認 (2026-08-01)、シナリオ 5 (VST GUI) の最中。
- 症状: フルスクリーンで動画を**再生中**に VST エディタを開こうとしたところ、マウスカーソルが
  非表示のまま戻らなくなった。**mIV の動画ウィンドウの上でだけ**継続し、別モニタへ移すと
  正常。動画ウィンドウ上へ戻すとまた消える。**クリックしても復帰しない** (ホイールは未試行)。
  動画を閉じて再生し直すと解消した。
- 壊れている前提: [native_window_host.rs](../src/video/native_window_host.rs) の
  `observe()` が作る `cursor_within_client` は `GetCursorPos` → `ScreenToClient(presenter)` →
  `GetClientRect` の **純粋な幾何判定**である。ところが
  [render_core.rs](../src/video/native_presenter/render_core.rs) の
  `cursor_within_focus_window()` はこれを「カーソルの入力を実際に受けている窓が presenter か」
  という**所有権の判定として**使い、true の間だけ `SetCursor` intent を出す。
  presenter の client 矩形の内側に別の top-level 窓 (VST エディタ) が乗ると、この 2 つの問いの
  答えがずれる。
- 成立する環:
  1. エディタ窓は presenter の client 矩形の内側 → `cursor_within_client = true`。
  2. 再生中なのでフレームが出続け、presenter は毎フレーム `SetCursor(Hidden)` を適用する。
  3. mouse move / button はエディタ窓へ行き、`push_native_event` に届かない →
     `cursor_last_activity` が更新されず `cursor_should_hide` が true に固定される。
  4. `WM_SETCURSOR` は `LRESULT(1)` を返すだけで復帰させない (2026-06-06 に zero-delta move
     での誤復帰を潰すため意図的に外した経路)。`restore_cursor_for_mouse_activity` は HUD の
     wheel / button ハンドラからしか呼ばれないので、エディタ上のクリックでは発火しない。
  → 3 つある復帰経路がすべて同時に閉じる。実機の「クリックでも戻らない」がこれを裏付ける。
- 症状パッチにしないこと: `WM_SETCURSOR` での復帰と、タイマーによる強制復帰はどちらも
  根本原因に対応せず、前者は 2026-06-06 の修正を再発させる。auto-hide 状態は現在
  **producer が 3 つ (presenter frame intent / HUD wndproc / `push_native_event`) あって
  所有者がいない**。reducer が単一の owner となり、placement / VST owner 切替の遷移で
  明示的にリセットされる形へ集約する。`cursor_within_client` は幾何ではなく
  「presenter または HUD が実際にそのカーソルの入力先か」を答える述語に置き換える
  (判定不能時のフォールバックが現在 `_ => true` = 隠す側に倒れている点も含めて直す)。
- 関連: [native-video-window-thread-plan.md](native-video-window-thread-plan.md) の Stage 5
  (VST owner handoff と focus 境界) と Stage 6 (placement 切替の lifecycle hardening)。
  VST 固有ではなく、presenter の上に別の窓が乗る全ケースで成立する。
- 観測の欠落: 発生時点のログ (`mimageviewer.log` / perf log) に**カーソル関連の計装が無く**、
  今回の発生は事後確認できなかった。§7.3 の health detection に `cursor_hidden` /
  `cursor_within_client` / `cursor_last_activity` / 直近の placement 遷移を含める。
- 規模 / 優先度: Medium / **P2**。データ喪失は無いが、再生を止めるまで操作感が壊れる。

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
- 規模 / 優先度: Large / **P2**。カーソル (§1.28) の修正はこれが片付くまで入らない。

### 1.31 wndproc の内側で GPU 待ちのある描画をする構造

- 出典: §1.30 の解析 (2026-08-01、Codex Sol の独立レビュー)。**v2.9.1 の範囲外**。
- 何が問題か: winit は `WM_PAINT` 内で `RedrawRequested` を**同期 dispatch** し、eframe はそこで
  `run_ui_and_paint` を直接呼ぶ。さらに Windows の resize では flicker 防止のため意図的に同期 paint
  する。結果として **wndproc の内側で swapchain acquire と Present を行う**。Microsoft も
  `Present` が message-pump thread を待ち得ると明記している。
  main surface は `AutoVsync` / maximum frame latency **1** なので、前フレームが retire しない限り
  次の acquire が待たされる。
- 効果: 窓の owner / z-order / visibility を動かす操作、DWM 合成の切替 (Independent Flip / MPO)、
  device-loss 周辺のいずれかが絡むと、**同期メッセージ 1 通で UI が任意時間停止し得る**。
  Stage 4 で閉じた側 (presenter スレッドが窓を持ったままブロックする) に対し、こちらは
  「UI スレッドが自分の窓のメッセージ処理中に GPU で止まる」。**同じ上位の破綻の別の面**で、
  Stage 4 の pump 分離では届かない。
- Stage 4 以降の運用制約 (維持すること): pump thread では GPU API を呼ばない。held frame の
  再提示にも re-arm 用 `AcquireSync` を追加せず、reader key の維持と producer 側回復を使う
  ([video-architecture.md](video-architecture.md) の `FramePresentationState` 節)。
- 方向 (Codex 案):
  - wndproc は damage / resize / redraw 要求を記録して**即 return** する。
  - 実描画は外側の event-loop 境界へ post して coalesce する。
  - `Idle / Painting / Pending` の単一 typed render state を置き、**再入 acquire を禁止**する。
  - surface acquire は短時間・nonblocking にし、間に合わなければその frame を捨てる。
  - msg / paint depth / wait handle / timeout / Present 前後 / cloak / visibility を 1 つの trace に残す。
  - 同期 `SendMessage` 中に presentation availability を故意に止め、**wndproc が有限時間で返る**
    Windows 回帰テストを追加する。
  - 必要なら eframe / winit / wgpu を vendor patch し、upstream へ最小再現を出す。
- 併せて直す観測の穴: 今回 `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も書かれ
  なかった。**active native fullscreen 中は main HWND が隠れていても watchdog を suspend しない**
  ようにする (危険な操作の最中こそ黙る、という現状は逆)。
- 規模 / 優先度: Large / **P1 candidate (基盤)**。次版で Web 配信とコンテナ詰め替えを載せる前に
  方針を決めておく。

### 1.32 対応済み: IME 変換中の ESC が期待どおりに働かない 2 件

- 出典: v2.9.1 リリース前の実機確認 (2026-08-01)。**どちらも v2.9.0 から同じ挙動**で、v2.9.1 の
  退行ではない。`829ba729` で入れた IME helper は単一行欄を揃えたもので、下記 2 件は範囲外だった。
- **対応 (2026-08-01)**: App / native presenter の両 egui Context に同じ input plugin を登録し、
  viewport ごとの composition state を単一 ownership で追跡する。composing 中の Esc press は
  `Memory::begin_pass` より前に RawInput から除去し、release は残す。これにより未確定文字の
  選択を保ち、Commit の無い composing `Disabled` の直前へ空の `Preedit("")` を 1 回だけ補える。
  非 composing Esc と 300ms grace 中だけの Esc は除去しない。
- **(A) の確認**: helper 対象外の raw `TextEdit::multiline` を使う回帰テストで、変換中 Esc の
  pass 後も focus が残り、遅延 `Disabled` で未確定文字だけが消えることを固定した。複数行 helper の
  追加変更は不要だった。
- **(B) の確認**: composing / 非 composing / Commit / `Disabled` / 1 回だけの空 preedit /
  viewport 分離を plugin 境界の unit test で固定した。

### 1.34 パスワード入力中のキー識別情報が診断ログに残る

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、Codex)。**本セッションで source
  inspection により確認済み**。
- 壊れている前提: [ime_focus.rs](../src/ime_focus.rs) の `diagnostic_key_identity` は
  **`modifiers.is_none()` のときだけ** 文字キーを `Char` へ匿名化する。修飾キーを伴う入力は
  `DiagnosticKeyIdentity::Named(key)` になり、`key=` / `physical_key=` / `modifiers=` が
  そのままログへ出る。つまり「文字は伏せる」という意図が、Shift や AltGr を使った瞬間に破れる。
- 機密欄が同じ経路に乗っている: PDF ([pdf_password.rs](../src/ui_dialogs/pdf_password.rs)) と
  書庫 ([archive_convert.rs](../src/ui_dialogs/archive_convert.rs)) のパスワード欄は
  どちらも `ime_focus::add_singleline` で描いており、helper 管理欄なので診断対象になる。
  `.password(true)` は**画面の伏せ字だけ**で、診断経路には効かない。
- 露出の広さ: `log_key_diagnostic` の routine 記録は 1MB/プロセスで打ち切られるが、
  **anomaly 記録は無制限**。`diagnostic_is_anomalous` の条件に「対象欄が focus を失った」が
  含まれるため、パスワードダイアログのように focus が動く場面はむしろ anomaly 側へ倒れやすい。
  ログは診断 ZIP に同梱されるため、利用者が送付した時点で外部へ出る。
- 直し方: 匿名化の条件を「修飾キーの有無」から切り離す。文字キーは修飾の有無によらず
  `Char` とし、機密欄では key 識別情報を一切残さない (欄そのものを診断対象外にするか、
  `Char` すら出さない段階を設ける)。**修飾キー自体の記録も、機密欄では落とす**
  (`modifiers=SHIFT` だけでも大文字・記号の位置が漏れる)。
- 併せて見直す (N-8): 診断ログ全体のノイズ量。anomaly 記録が無制限であること、v2.9.1 で
  health watchdog のログ (`append_panic_log_entry`、10 秒レート制限) が加わったことを含め、
  利用者からログを受け取る運用に対して総量が妥当かを一度判断する。
- 完了条件 / 回帰テスト:
  - `Shift` / `AltGr` 併用の文字キーで `key=` / `physical_key=` に文字が出ない unit test。
  - パスワード欄が focus を失う (= anomaly になる) 経路でも文字が出ないこと。
- 規模 / 優先度: Small / **P1**。データ喪失ではないが情報漏えい経路であり、
  **通常のバックログ扱いにせず v2.9.2 などの早いパッチで出す**ことを推奨する。

### 1.35 リネーム移行ジャーナルの永続化 2 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、Codex)。両方とも source inspection で確認済み。
  1. **対応済み (v2.10.0): UI スレッドでの同期 I/O (P2)**: [app.rs](../src/app.rs) の
     `persist_rename_migration_journal` / `ensure_rename_migration_journal_loaded` は
     `rename_key_migration::journal_save` / `journal_load` を同期で呼び、その中身は
     `std::fs::read` / `write` / `remove_file` / `rename`。呼び出し元は
     `poll_rename_migration_pending` などの UI スレッド経路である。小さな JSON なので通常は
     短いが、ウイルス対策・低速ディスク・プロファイル同期の影響下では UI が止まる。
     CLAUDE.md の「UI スレッドで同期 I/O を行わない」方針にも反する。
     対応 = App が in-flight + queue + boot-retry の完全 snapshot を組み立てる所有境界は維持し、
     書き出しだけを単一 latest-value worker へ移した。worker は revision 順に直列保存し、I/O 中の
     中間 snapshot は最新値へ coalesce する。起動時 load は回復 entry を新規保存で消さない lazy guard を
     優先して同期 1 回を維持し、通常 update の保存経路は enqueue のみ。終了時は最新 revision まで flush する。
  2. **部分失敗が再試行されない (P2、仕様判断あり)**: worker が `panicked` を返したときだけ
     ジョブを `rename_migration_boot_retry` へ戻す。`report.errors` に個別ストアの失敗が
     入った場合はトーストを出すものの、ジョブは完了扱いでジャーナルから消える。一時的な
     DB ロックや I/O エラーだと、編集結果やパス依存設定の一部が旧パスに残ったまま再起動でも
     回復しない。
     - **これは現状のコードコメントが明記している意図的な選択**である
       (「per-store エラーは通常経路と同じ best-effort = 再試行しない」、v2.3.0 角度⑤ 検収時の判断)。
       したがって着手前に「一時エラーと恒久エラーを区別できるか」「再試行の上限をどう置くか」を
       決めること。無条件の再試行は、決定的に失敗するストアで無限ループになる。
     - 移行処理自体は冪等に設計されているので、失敗ストアを保持して再試行する形にはできる。
- 規模 / 優先度: 1 = Small〜Medium / P2、2 = Medium / P2 (仕様判断を先に決める)。

### 1.36 ゲームパッドの文字入力ゲートは 3 面の union が必要 (2026-08-01 訂正)

- 前段の誤った前提: viewer 宛なら fullscreen / detached viewport、そうでなければ root viewport
  という排他選択で十分と考えた。しかし gilrs は App が直接 poll するグローバル入力で、
  OS の keyboard routing を通らない。正しい不変条件は **mIV 内のどこかで文字入力中なら止める**。
- 文字入力面は 3 つある。
  1. App root viewport (一覧・ダイアログ)。
  2. App fullscreen / detached viewport (静止画パネル)。
  3. native video / music presenter overlay。これは `egui::Context::default()` で独自 Context を持ち、
     App の `Context::data` からは viewport id を変えても見えない。実機ログでも detached 動画の
     ブックマーク名入力は overlay 側 ROOT (`FFFF`) として観測され、App detached viewport (`39E5`) と別だった。
- keyboard は presenter の `NativeOverlayInputRouting` が `wants_keyboard_input` / `text_input_active` を見て
  App 転送を止めるが、ゲームパッドは presenter を経由しないため以前からこの保護を素通りしていた。
- 対応: root と対象 viewer viewport の IME state を常に union し、さらに presenter が既に公開する
  `NativeOverlayInputRouting::wants_keyboard_input` を既存 output event bus の latest-value snapshot として
  App へ一方向 publish する。3 条件は `gamepad_text_input_active` だけが所有し、呼び出し側へ分散させない。
  presenter Context の統合、presenter→App 直接参照、新しい逆向き channel は行わない。
- 回帰テスト: root composing、viewer composing、presenter `wants_keyboard_input`、全て inactive の 4 状態を
  1 つの predicate test で固定し、snapshot publish が semantic event として App へ漏れないことも固定する。
- 規模 / 優先度: Small / P2。前段差分を本訂正で置換。

### 1.37 トレイ常駐再生まわりの所有境界 3 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  v2.9.1 で「トレイ格納中も再生を続ける」経路を新設したことで、いずれも**この版から初めて
  実際に通る**組み合わせになった。
  1. **対応済み (v2.10.0) / P2: WM_PAINT ブリッジに背圧が無い**。[tray.rs](../src/tray.rs) の常駐ループは 50ms ごとに
     `PostMessageW(WM_PAINT)` を投げるが、**未処理の wake を数えていない**。posted WM_PAINT は
     無効領域由来のものと違って合体されないため、隠れている間の `App::update` が 50ms を
     超えると投函が消費を上回る。戻り値は `let _ =` で捨てているので、スレッドの posted message
     queue が溢れても検出できない。
     対応 = `resident_media_wake_pending` を既存共有 atomic 群へ追加し、tray thread が false→true を
     claim できた 1 件だけ投函、`App::update` 入口で ack する構造にした。可視化・wake 不要化・
     `PostMessageW` 失敗では pending を reset し、失敗ログは状態変化時だけ出す。
  2. **P3: 再生継続中に video processor cache を落とす**。
     [tray_integration.rs](../src/tray_integration.rs) の `release_gpu_resources` は
     detached 早期 return より前で `release_idle_pools()` を呼び、その中の
     `processor_cache.take()` は**無条件**である (`hw_frames_pool.clear()` と
     `shared_output_pool.retain(in_use)` は refcount 上安全)。
     ⚠ **呼び出し順自体は v0.9.1 (`287eea9f`, 2026-05-19) からで、v2.9.1 で動かしたわけではない**。
     新しいのは「格納中も再生が続く」ことのほうで、その結果この経路が再生中に走るようになった。
     トレイ格納の瞬間に video processor の作り直しが 1 回入るはずで、実害は小さい。
  3. **P3: 外部 hide 検出が tray 所有権に追随していない**。[app.rs](../src/app.rs) の
     `IsWindowVisible` 追従分岐は `viewer_session_is_detached_or_switching()` だけで heartbeat
     suspension を決め、`sync_media_presenter_visibility_for_tray()` /
     `sync_retained_viewport_visibility_for_tray()` を呼ばない (これらは `hide_to_tray` /
     復帰経路にしか無い)。同フレーム後半の `sync_tray_resident_media_wake()` が**値が変化した
     ときだけ**自己修復するので実害は出にくいが、hide の所有者が 2 つある状態である。
- 規模 / 優先度: 1 = Small / P2、2 と 3 = Small / P3。

### 1.38 ネイティブ名前ダイアログが `App::update` の内側でモーダルを回す

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 何が問題か: [rename_item.rs](../src/ui_dialogs/rename_item.rs) の
  `show_rename_dialog_window` は `loop` の中で `native_name_dialog::prompt_name`
  (`DialogBoxIndirectParamW` = 独自のモーダルメッセージループ) を呼び、バリデーション失敗の
  たびに再表示する。`App::update` は `WM_PAINT` → `RedrawRequested` の同期 dispatch から
  呼ばれるので、**wndproc の内側で無制限長のモーダルが回る**。
  §1.31「UI スレッドが自分のメッセージ処理中に任意時間止まる」構造の一種なので、
  **§1.31 の設計を決めるときに一緒に見る**のが筋が良い。
  なお rename 自体は `rename_item_async` で worker 化されており、ここは**ダイアログの
  モーダル span だけ**の問題である。
- **副次的な退行は対応済み (2026-08-01)**: 既存 `rename_pending` を左下の進捗 overlay に接続し、
  シェル rename 実行中の「変更中...」表示を復帰した。モーダル構造は変更していない。
- 規模 / 優先度: Medium / P2 (§1.31 に合流)。残るのはモーダル span の構造だけ。

### 1.39 見開き / 先読みの残り 1 件 (`9590b661` の続き)

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  1. **対応済み (2026-08-01)**: paged 側も `current_loading` で sibling pending を cancel した後、
     return 前に `ensure_fs_page_load(current_idx)` で current producer を再確認する形へ統一した。
     current が `NeedsLoad` の状態遷移テストで、cancel だけで終わらず `LoadPending` になることを固定した。
  2. **P3: `fs_page_load_state` が PDF ページで毎フレーム String を作る**。
     `has_retained_pdf_final_ai_for_current_params` → `retained_pdf_page_key_for` →
     `metadata_cache_key(idx)` が String を生成し、HashMap を引く。
     ⚠ **レビュー報告より実コストは小さい**: `effective_params` / `final_ai_key_for_pixels` は
     retained cache に**該当エントリがあるときだけ**走る (無ければ `?` で抜ける)。
     非 PDF は `items.get` + `matches!` で即 return なので無コスト。
     とはいえ PDF フルスクリーンでは `update_prefetch_window` の current、prefetch ターゲット
     全件、連結読み keep 範囲ループ全件、見開きパートナーから毎フレーム走るため、
     key を 1 回作って使い回す形にできるかは見ておく。
- 規模 / 優先度: 残る 2 は Small / P3。

### 1.41 対応済み: 静止画縮小表示のシャープ優先 / モアレ抑制切替 (v2.10.0 履歴)

> v2.11.0 では通常静止画を表示サイズへ Lanczos3 で直接縮小する方式へ置き換え、
> この節の ON/OFF と LOD bias は削除した。現在の仕様は
> [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) §4.3.1 / §4.3.3 を参照。

- **対応 (2026-08-02)**: 画像補正パネルのフィルタへ既定 ON のモアレ抑制チェックと
  0.0〜1.5 の強度を追加。OFF は mip chain を保持したまま level 0 固定で読み、主表示、
  wipe/diff 比較、360度パノラマの各 shader を同じ明示 uniform フラグで切り替える。
  既定 ON / 0.0 は従来の bias 0.0 sampling を維持し、切替で texture 再 upload や
  cache invalidation を行わない。

- 出典: 5ch 専用スレ #153 (2026-08-02)。ZipPla と mImageViewer の同一ページ比較で、
  mIV 側が少しぼやけて見えるという報告。提示画像では右側 (mIV) の細線・文字・トーンが
  左側 (ZipPla) より低周波寄りに均されており、v2.7.0 で導入した GPU mipmap 縮小の影響が
  出ている可能性が高い。元画像 / 表示倍率 / 補正設定は未確認のため、mipmap だけが原因とは
  断定しない。
- 方針:
  1. 設定 UI は複雑にしない。画像補正パネルの「フィルタ」内を次の形へ整理する。
     - `□ 縮小表示のモアレを抑制する`
     - `より強く抑制: 0.0〜1.5`
  2. 既定は **モアレ抑制 ON / より強く抑制 0.0**。0.0 は「抑制なし」ではなく、
     **GPU 標準の mipmap 縮小**を意味する。既存の `image_mipmap_lod_bias = 0.0` と同じ
     画質を維持し、v2.7.0 以降の既定挙動を急に変えない。
  3. チェック OFF は、mip chain を作っていても表示時に **mip level 0 固定**で読む
     シャープ優先モードにする。従来の bilinear 1-mip 縮小に近く、線は立ちやすいが
     トーンのモアレ / ちらつきは戻りやすい。
  4. チェック ON かつ値 > 0.0 は、現行の `image_mipmap_lod_bias` と同じく標準より粗い
     mip level へ寄せる。値を上げるほどモアレは減るが、細部は柔らかくなる。
- 実装メモ:
  - managed texture は `DISPLAY_IMAGE_TEXTURE_OPTIONS` のまま完全 mip chain を生成してよい。
    OFF 時は sampler の `lod_min_clamp` / `lod_max_clamp` または shader 側の
    `textureSampleLevel(..., 0.0)` 相当で level 0 固定にする。texture 再生成なしで切り替え
    られる経路を優先する。
  - 比較 callback / 360 度パノラマは独自 shader / sampler を持つため、同じ設定が効くように
    別途適用する。`PostFilter::Nearest`、動画、サムネイル、animated frame は既存どおり対象外。
  - 既存 `image_mipmap_lod_bias` は「より強く抑制」の値として維持し、チェック ON/OFF 用の
    bool 設定を追加するのが自然。旧設定しか無い環境では ON として読む。
- 完了条件 / 回帰テスト:
  - 既定設定 (ON / 0.0) で v2.9.1 までの縮小表示と同等になる。
  - OFF で level 0 固定になり、線のシャープさが戻る一方、モアレ抑制が弱まることを手動確認する。
  - ON の値変更で texture 再 upload / cache invalidation が起きず、表示だけライブに変わる。
  - 比較表示と 360 度パノラマでも同じ設定が適用される。
- 規模 / 優先度: Small〜Medium / P2。

### 1.40 対応済み: 閲覧履歴 — 動画・音声を対象に含め、自動進行由来は記録しない

- **対応 (2026-08-02)**: 動画・音声をファイル単位で追加し、ブックマークと同じ
  すべて / 動画 / 音声 / 本 (+ 本の内訳) の絞り込みを実装。HistoryTrigger を通常 open、
  native source swap、ZIP/PDF 列挙待ち、変換待ちまで必須引数で運び、スライドショー、
  NextFolder、動画 / 動画音声モード / 音楽の EOF を AutoAdvance として記録対象外にした。
  DB はメディア位置 / 尺の専用列を ALTER TABLE で追加し、未知 kind は行ごと読み飛ばす。
  resume の既存条件と永続識別子 reading_history / ReadingHistory は変更していない。

- 出典: 2026-08-01 のユーザー相談。「動画はブックマークには残るのに履歴には残らない」という
  非対称の是非から。本項の調査は source inspection で確認済み。
- 現状:
  - 履歴の種別は [reading_history_db.rs](../src/reading_history_db.rs) の `ReadingHistoryKind` =
    `folder` / `zip` / `pdf` / `archive` の 4 つ。記録判定 `record_reading_history` は
    `is_readable_page_idx` (= `Image` / `ZipImage` / `PdfPage`) を通らないと即 return するため、
    動画・音声は**構造的に入らない**。[reading-history-plan.md](reading-history-plan.md) §2.1 に
    「動画は対象外」と明記された意図的な設計であり、退行ではない。
  - ブックマークは [bookmark_browser.rs](../src/bookmark_browser.rs) が動画・音声・本を横断し、
    `MediaFilter` (すべて / 動画 / 音声 / 本) + `BookKindFilter` + ソートを持つ。
  - 履歴ビューは**絞り込みバーごと無効**。[ui_main.rs](../src/ui_main.rs) の facet filter bar が
    `items_are_reading_history_view` で早期 return するため、種別で絞る手段が今は無い。
  - 位置の記憶は媒体ごとに既にある (本 = `book_resume.db`、動画・音声 =
    `settings.video_resume_positions`)。欠けているのは**動画・音声の時系列一覧だけ**である。
- **resume の挙動は既に望ましい形なので変更しない**: `save_video_resume_position` は EOF /
  3 秒未満 / 末尾 5 秒以内でエントリを削除し、それ以外を保存する。既定は「一覧から開く = 続きから」
  「移動 = 続きから」。したがって「完走したら次回は先頭から、途中で止めたらその位置から」は
  **現行どおり**で追加作業は無い。履歴はこれに依存せず、完走した動画も履歴には残す
  (位置の記憶と、見たという事実は別物)。
- 方針:
  1. 動画・音声を履歴の対象に加える。記録単位はファイル (本はコンテナ)。
  2. 名称を **閲覧履歴 → 閲覧履歴** へ改名する。音声を含めた時点で「読書」が成立しない。
     ブックマークが 1 ビュー + 種別絞り込みで統合されている以上、履歴を媒体別ビューに
     分割せず同じ形に揃える。
  3. 種別絞り込みをブックマークと同型にする (すべて / 動画 / 音声 / 本 + 本の内訳)。ラベルと
     並びを合わせて学習コストを 0 にする。前提として履歴ビューで絞り込みバーを有効化する。
     並びは最終閲覧↓固定のままでよい。
  4. **自動進行由来は記録しない**。記録するのはユーザーが選んだ遷移だけにする。
     - 記録する: 一覧からのオープン、↑↓ / ホイールのファイル移動、Ctrl+↑↓、履歴 /
       ブックマークからのオープン。
     - 記録しない: 連続再生 (`VideoContinuousMode`) の EOF 遷移、スライドショーの自動送り、
       スライドショーの `SlideshowEndAction::NextFolder`。
     - 理由: 履歴は探し直すための足跡であり、自分で選んでいないものは足跡にならない。加えて
       動画・音声は記録単位がファイルなので、フォルダを 1 つ流すだけで保持上限 1000 件を
       自動生成が食い潰し、**本来残したい本が押し出されて機能自体が壊れる**。
     - 画像側にも同じ穴が既にある: `try_start_slideshow_next_folder` で渡ったフォルダが 1 件ずつ
       積まれる。同じ方針で塞ぐ (= 現状からの挙動変更になる)。同一フォルダ内の自動送りは
       同一キーの upsert なので元から件数は増えない。
     - 「流れてきた曲を後から探したい」需要が確認できたら、設定 (既定 OFF) で後付けできる。
       需要が読めない段階で設定を足さない。
- 実装上の注意:
  - **DB はリリース済み**なので、動画の位置 / 尺を持つなら `ALTER TABLE ADD COLUMN` の
    マイグレーションが要る。本の `last_page` / `page_count` に秒数を流用すると意味が混ざるので
    別列にする。
  - **旧版へのダウングレード**: `ReadingHistoryKind::from_str` は未知 kind を `unwrap_or(Folder)` に
    落とすため、新版が書いた動画行を旧版が読むと**動画ファイルをフォルダとして表示・オープン
    する**。未知 kind は行ごと読み飛ばす形へ変える。
  - 記録タイミングは本と揃えて「開いた瞬間」でよい。自動進行を除外すれば、誤って開いたものが
    残る実害は小さい。
  - 完走した動画は resume 側のエントリが消えているので、履歴の位置表示は履歴自身の列から出す
    (resume の有無に依存させない)。
  - 改名の波及先: `StartupFolderMode::ReadingHistory`、環境設定「履歴と復元」、アドレスバー
    「場所▼」、ファイルメニュー、`address` 文字列、[reading-history-plan.md](reading-history-plan.md)
    §2.1、`htdocs/mimageviewer/manual/`。設定の enum 名を変えるなら旧値の読み替えを用意する。
- 完了条件 / 回帰テスト:
  - 動画・音声を手動で開くと履歴に載り、種別絞り込みで動画 / 音声 / 本を分けられる。
  - 連続再生の EOF 遷移で履歴が増えない。スライドショーの自動送りと NextFolder でも増えない。
  - 手動のファイル移動 (↑↓ / ホイール / Ctrl+↑↓) では従来どおり載る。
  - 完走した動画も履歴に残り、次回オープンは先頭からになる (resume 側の既存挙動が
    変わっていないことを固定する)。
  - 新版が書いた動画行を旧版が読んでも、フォルダとして開かない。
- 規模 / 優先度: Medium / P2。

### 1.42 右クリックメニューの表示が遅い (シェル拡張を UI スレッドで同期ロード)

- 出典: 2026-08-02 の v2.10.0 実機確認中の利用者報告。「右クリックメニューを開くのが
  かなり遅い」ため、連続リネームのテストが現実的に行えなかった。
- 壊れている前提: [native_context_menu.rs](../src/native_context_menu.rs) の
  `query_shell_context_menu` は `IContextMenu::QueryContextMenu` を**メニュー構築時に
  同期実行**する ([native_context_menu.rs:288](../src/native_context_menu.rs) と
  [872](../src/native_context_menu.rs))。同 module に worker は無く、**UI スレッドで走る**。
  `QueryContextMenu` はサードパーティのシェル拡張 (ウイルス対策 / クラウドストレージ /
  書庫ソフト等) を列挙してロードするため、環境によっては数百 ms〜秒単位かかる。
- 「Windows のせい」は半分正しい。遅いのは拡張側だが、**それを UI スレッドで待っているのは
  mIV 側**であり、その間アプリ全体が固まる。CLAUDE.md「UI スレッドでの同期 I/O は即
  worker 化する」の対象。
- 未計測: 実際に何 ms かかっているか、どの拡張が支配的かは未測定。**着手前に計測すること**
  (`perf::event` を `query_shell_context_menu` の前後に入れる)。環境差が大きいので、
  遅い環境のログが取れると判断が早い。
- 対応案 (計測してから選ぶ):
  1. **シェル部分を遅延構築**する。mIV 自身のメニューを即座に出し、シェル項目は
     サブメニューを開いた時点で初めて `QueryContextMenu` する。エクスプローラー互換の
     見た目からは離れるが、体感は最も改善する。
  2. mIV のメニューを先に表示し、シェル項目だけ**非同期に差し込む**。項目が後から
     増えるので、開いた直後にクリックすると位置がずれる問題を設計で潰す必要がある。
  3. 直近で使った拡張セットを**キャッシュ**する。COM オブジェクトの寿命と、
     拡張の追加 / 削除の検知が要る。
- ⚠ **症状パッチにしないこと**: タイムアウトで打ち切って一部の拡張を落とす、は
  「どの拡張が出るか環境ごとに変わる」という新しい非決定性を持ち込むので採らない。
- 完了条件 / 回帰テスト:
  - 右クリックからメニュー表示までの UI スレッド占有時間を計測し、改善を数値で示す。
  - シェル項目の実行 (`InvokeCommand`) が従来どおり動く。
  - 拡張が 1 つも無い環境でも従来と同じメニューが出る。
- 規模 / 優先度: Medium / P2。実害は「操作のたびに待たされる」で、データ喪失は無い。

### 1.43 対応済み: Enter でフルスクリーンを開くと全画面ズームモードに入る (Enter 系 KeyHold の所有権が開幕フレームで抜ける)

- 出典: 2026-08-04 の利用者報告と同日の調査。「画像フォルダで Enter で画像を開くと最初から
  Z のズームモードになっている。Z を 1 回押すと解除できる。ダブルクリックでは起きない」。
- 再現条件: **Enter / テンキー Enter を KeyHold アクションに割り当てている**こと。報告環境の
  keymap override は `FsZoomMode = Z, NumpadEnter`。既定 (Z のみ) では成立しないので、
  データ保存先が別のポータブル版では再現しない。**バージョン差ではなく設定差**であり、
  同じ `%APPDATA%` を使うインストール版なら旧バージョンでも再現するとみられる。
- 壊れている前提:
  1. `key_held_via_os` ([keymap.rs:6939](../src/keymap.rs)) は Enter / NumpadEnter を
     「送信元 HWND 由来の物理ラッチ」で判定するが、それは `is_frame_active(viewport)` が
     true のときだけで、false のときは `GetAsyncKeyState(VK_RETURN)` にフォールバックする。
     `KeyName::Enter` と `KeyName::NumpadEnter` は `to_vk()` が同じ 0x0D
     ([keymap.rs:636](../src/keymap.rs)) なので、この経路は本体 / テンキーの区別も
     どのウィンドウのキーかの区別も失う。
  2. `frame_active_viewports` は subclass 登録済み HWND を持つ viewport の集合
     ([key_input.rs:333](../src/key_input.rs))。フルスクリーンの viewport は
     `install_key_input_subclass_for_viewport_rect` が outer_rect と可視ウィンドウの
     rect 一致で HWND を見つけてから登録する ([ui_fullscreen.rs:6955](../src/ui_fullscreen.rs)、
     呼び出しは [8247](../src/ui_fullscreen.rs)) ため、**新規作成直後の数フレームは false**。
  3. その間 `update_fs_zoom_mode_keys` ([ui_fullscreen.rs:3070](../src/ui_fullscreen.rs)) が
     「まだ押されたままの Enter」を押下エッジとして拾って `fs_zoom_aiming = true` にし、
     離した瞬間の離しエッジで `fs_zoom_active = true` を確定させる。
- 影響範囲: FsZoomMode 固有ではない。**Enter / NumpadEnter を割り当てた全ての KeyHold
  アクション** (`*SpacePan` 等) と、**新規 viewport の開幕フレーム全般**が同じ穴を通る。
  713d36bf 以前は frame-active gate 自体が無く常に `GetAsyncKeyState` だったので、
  713d36bf は同じ穴の frame-active 側だけを塞いだ状態になっている。
- 対応 (2026-08-05): `key_input::routed_return_key_held` は対象 viewport が frame-active の
  ときだけ main / numpad 別の物理ラッチを返し、未登録の開幕フレームは `None` にする。
  `key_held_via_os` は送信元と物理種別を復元できない Enter / NumpadEnter の `None` を
  **false** とし、`GetAsyncKeyState(VK_RETURN)` へフォールバックしない。その他の KeyHold は
  既存の VK ベース判定を維持する。既定 keymap、登録後のラッチ、本体 / テンキー Enter の分離、
  既定 Z のズーム操作は変更していない。
- 追加修正 (2026-08-05、実機確認で判明): `FsZoomMode = Z, NumpadEnter` でテンキー Enter を
  押すとズームに入らず**表示が閉じた**。`take_key_hold_edges` は Win32 edge を消費するが、
  同じ物理押下から egui が生成した双子イベントを claim していなかった。egui は本体 /
  テンキーの Enter をどちらも `Key::Enter` へ畳むため、残った event を後続の `FsClose`
  (既定 Enter) が egui 経路で拾っていた。`consume_chord_inner` の Win32 経路は同じ claim を
  既にしており ("Claim both at this ownership boundary")、KeyHold 側だけが抜けていた。
  照合用の `to_egui` (NumpadEnter は取り違え防止で `None`) とは別に、claim 用の
  「畳まれた先」を返す `egui_twin_key_for_claim` を追加して塞いだ。
- 追加修正 2 (2026-08-05、実機再現とログで確定): 双子 claim だけでは閉じるのが止まらなかった。
  `[fs-key] source=root` / `source=fullscreen` のログが示すとおり、**同じ押下の egui イベントは
  main と fullscreen の両 viewport へ届く**一方、Win32 edge は届いた側 (fullscreen) にしか無い。
  main 側は claim の対象外なので `Key::Enter` が残り、`FsClose` が egui フォールバックで拾っていた。
  frame-active な viewport は Win32 キューが正本なので、**畳まれ先のスロット
  (`Enter` / `Num0`-`Num9`) は egui フォールバックへ落とさない**ようにした
  (`consume_chord_inner` / press count / pressed probe の 3 経路)。
  なお本来は frame-active なら全スロットで egui フォールバックを止めるのが筋だが、出荷直前の
  範囲としては曖昧なスロットに限定した。全面適用は次サイクルで判断する。
- 実機確認済み (2026-08-06): Enter 開閉でズームに入らない / NumpadEnter でズームに入り表示が
  閉じない / 通常 Enter で閉じられる / 既定 Z / グリッドの Enter 起動、いずれも確認。
- 残件 (P3): 検収で追加した `shared_virtual_keys_stay_limited_to_the_known_pairs` が、
  **`Backslash` / `IntlYen` も `0xDC` を共有している**ことを検出した。Enter ペアと違って
  extended bit ではなく scan code でしか分かれず、対応する per-HWND ラッチが無いため、
  今回は routed 必須の対象へ含めていない。この 2 つを KeyHold へ割り当てると
  `GetAsyncKeyState(0xDC)` がどちらの物理キーか判別できず取り違える。実害は Enter より
  小さい (フルスクリーンを開く操作がこのキーではないので「開いた瞬間に押されている」
  状況が無い)。ラッチを scan code 対応へ広げるかは次サイクルで判断する。
- ⚠ 症状パッチにしないこと: 「FS 入場時に `fs_zoom_z_was_down` を現在の押下状態で
  初期化する」「開幕 N ms はズームモードを無効にする」は、この 1 アクションの見え方を
  消すだけで、他の KeyHold と他 viewport の同型を残す。
- 完了条件 / 回帰テスト:
  - `FsZoomMode` に NumpadEnter を割り当てた状態で Enter 開閉を繰り返してもズームモードに
    入らない。
  - subclass 登録前後で Enter hold の判定が一致する (viewport 単位の unit test)。
  - 本体 Enter とテンキー Enter の分離が保たれる (既存
    `key_hold_slot_matching_distinguishes_both_enter_directions` の hold 経路版)。
- 当面の回避: 操作カスタマイズで `FsZoomMode` から NumpadEnter を外す。
- 規模 / 優先度: Small / P2。データ喪失は無いが、Enter で開くたびに表示が壊れる。

### 1.45 縮小に EWA (Jinc) 経路を検討する

- 出所: 2026-08-04 の利用者経由の意見 (MPC-BE / MPC Video Renderer の Jinc2m 評価) と、
  それを受けた再測定。正本は
  [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) §4.3.4。
- 測定結果: EWA Jinc3 は網点の残留が**角度にほぼ依存しない** (0/15/30/45 度で 11.0〜11.7)。
  現行の分離 Lanczos3 blur1.00 は同条件で 13.5→48.8 と角度で 3.6 倍悪化する。しかも
  0 度の細線コントラストは同等 (123.4 vs 123.5) を保つ。**分離 blur1.00 に対しては全角度で優位。**
- ただし: ①不規則トーン (実スキャン相当) では分離 blur1.30 に負ける (36.38 vs 30.23) ので
  「なめらかさ」設定は残る ②斜めディテールは分離 blur1.00 比 8〜10% 落ちる (円形通過域が
  対角の角を落とす選択的バイアス) ③**コストが約 6.5 倍** (0.41 縮小で約 196 タップ vs 約 30)。
- 着手時に見るべき点:
  - リサイズ中の実測。結果はキャッシュされるので静止時は一度きりだが、リサイズ中は
    毎フレーム走る。ここが採否の分かれ目
  - 現行の分離経路を置き換えるのか、拡大側にだけ入れるのか。MPC-VR 自身は
    「拡大は Jinc2m、縮小は Lanczos」を推奨しており、拡大側の方が素性は良い
  - 経路を 2 本持つ判断は §3.3 の「縮小経路は 1 本化する」と衝突する。増やすなら理由を残す
- 規模 / 優先度: Medium / P3。現行が壊れているのではなく、より良い候補があるという位置づけ。

### 1.46 静止画の拡大品質を上げる (bilinear をやめる) — 実装済み

**2026-08-05 実装。** 比較結果の正本は
[upscale-algorithm-selection.md](upscale-algorithm-selection.md)。動画は表示構造が異なるため
§1.47 の別案件とする。

- 既定の `PostFilter::None` は、静止画を拡大すると既存の GPU Lanczos3 経路で
  表示サイズへリサンプルする。整数倍 / 非整数倍を区別しない
- 物理等倍 (`OriginalOneToOne`) はリサンプルせず、ドットバイドットを維持する
- `PostFilter::Nearest` は、拡大では従来どおり NEAREST で直接描画する。縮小は
  モアレを戻さないため、従来どおり Lanczos3 を通す
- `PostFilter::UpscaleSharp`（シャープ拡大）は、拡大だけ公式 NVIDIA Image Scaling SDK 由来の
  WGSL shader を使う。縮小は `DownscaleLanczos`、物理等倍は元 texture のまま
- `PostFilter::UpscaleAnime`（アニメ塗り拡大）は、拡大だけ x2 VL の多段 WGSL shader を使う。
  可視 source 領域の長辺上限は 2048px / 4096px / 制限なし（既定 4096px）で、境界値は
  処理し、超過時は同じ領域・目標寸法の標準拡大へ戻す。縮小・物理等倍は他の選択式拡大と同じ
- CRT / レトロ / セピア等の効果付きポストフィルタは、拡大では従来どおり LINEAR で描画する。
  標準の高品質拡大と効果の同時指定には対応しない
- 拡大ではカーネルを広げず、支持半径は Lanczos3 本来の 3.0 とする。縮小用の
  「なめらかさ」設定は拡大へ適用しない
- 初回実装の `source × scale` 目標は大判写真で画面外まで生成して上限へ当たったため修正。
  拡大は表示 trim と画面の可視範囲の積だけを表示解像度へ生成し、パン先読みは持たない。
  可視 source UV は拡大 entry だけの cache identity とし、縮小の目標計算・シェーダ・cache は
  変更しない。`gpu/lanczos_regenerate` には生成元 source pixel 領域も記録する
- 拡大出力は一辺 8192px、総画素 4096×4096 相当を上限とし、超過時は表示を止めず
  従来の LINEAR 描画へフォールバックする。フォールバックと生成時間は perf event へ記録する
- 拡大出力キャッシュは 4096×4096 相当の 2 枚分を上限に LRU 解放する。既存の縮小キャッシュ
  上限は変更しない

NIS の「シャープ」と Anime4K の「アニメ塗り」は選択式として実装済み。どちらも標準
Lanczos3 と同じ可視領域・出力上限・cache ownership を使い、branch key で結果を分離する。
これで §1.46 の静止画拡大候補は完了とし、動画は §1.47 の別構造として扱う。

### 1.47 動画の拡大縮小を mIV のシェーダで行う — 設計確定 / 未実装

**正本は [video-upscale-shader-plan.md](video-upscale-shader-plan.md)。** 本項は要約と着手判断のみ。

§1.46 の動画版。**GPU 性能ではなく動画表示の構造が障害**だったので別案件として扱ってきたが、
2026-08-07 に構造・測定方式・UI まで設計を確定した。

- 現状: native presenter は **swap chain を動画解像度で作り**、`CopySubresourceRegion` で 1:1
  コピーし、**`IDCompositionVisual::SetTransform2` で拡大縮小している**。つまり拡大しているのは
  mIV ではなく DWM / DComp で、mIV のシェーダは通っていない (色補正が identity でないときだけ
  grade シェーダが走るが、それも動画解像度で動く)
- 採る構造: **swap chain を「映像の表示矩形の物理ピクセルサイズ」にし、シェーダでソース解像度
  から表示解像度へ直接解決する**。`compute_video_visual_transform` はサーフェスサイズを引数に
  取る作りなので**無改造で正しい**。リサイズ中は差し替えず既存サーフェスを DComp に伸ばさせ、
  静止後に 1 回だけ差し替える (= 全画面再生では差し替えが一度も起きない)
  - 当初案の「swap chain をウィンドウ解像度にする」は黒帯まで描くので上位互換の表示矩形サイズを
    採る。「動画解像度 × 2 の整数倍」も却下 (mIV の Anime4K / NIS は任意倍率へ直接解決できるため、
    整数倍に縛ると 1〜2 倍の中間倍率で情報を捨てる)
- **Phase A (標準 Lanczos3 / シャープ NIS / ニアレスト + 縮小)** と
  **Phase B (Anime4K)** に分ける。Phase A は 4K 出力で +0.4〜0.9ms と実質タダで全解像度に使え、
  **4K 動画をウィンドウで見たときの縮小モアレも同時に直る**。重いのは Anime4K だけ
- Phase B は変種 (S/M/L/VL/UL) を扱うため、**現行の VL 専用ハードコードを表駆動へ一般化**する
  必要がある ([gpu_anime4k.rs](../src/gpu_anime4k.rs) と
  [convert_anime4k_glsl_to_wgsl.py](../scripts/convert_anime4k_glsl_to_wgsl.py))。一般化すれば
  静止画側にも同じ選択肢が生える
- モデル選択は **利用者が Anime4K を選んだ瞬間にモデル × ソースサイズを実測して表を作り、
  以後は表引きで決める**。GPU 性能は 4090 とノート iGPU で 10 倍以上違い、解像度やフレーム
  レートからの固定しきい値では当てられないため。測定結果は GPU + ドライバ版をキーに永続化し、
  回復手段は再測定ボタン。**再生中はモデルを変えない** (自動昇降格はハンチングし、切替のたびに
  リソース再構築が走るため)
- 最重要の不変条件: **切り替えの瞬間にシェーダのコンパイルもテクスチャ確保も一切しない**。
  現行 grade pipeline はレンダースレッドで同期 `D3DCompile` しており、同じ作りで Anime4K UL
  (25 本 + 中間 24 枚) をやると設定を触った瞬間に数百 ms〜秒単位で固まる
- UI は動画左パネル →「画像補正」→「フィルタ」タブに置く (Creative LUT の隣)。制限で実行され
  ない場合は静止画と同じ `processing_size_outside_note` の書式で選択肢直下に出す
- VSR は [video-architecture.md](video-architecture.md) で**スコープ外と決定済み**なので、
  先例として使えるものが無い
- 性能の数字は**すべて静止画実測からの外挿**であり、これを根拠に既定値やしきい値を決めない。
  測定機構自体が「1080p VL が現実的か」を利用者ごとに答えるためのものである
- 規模 / 優先度: **Phase A = Large / P3 (約 2 週間、測定を待たずに着手可)**、
  **Phase B = Large / P3 (約 2 週間、Phase A の後)**。いずれも単独リリースで実機検証を厚く取る
  規模。detached リワーク ([detached-rework-plan.md](detached-rework-plan.md)) と presenter を
  共有するので、着手時期はリワークの進捗と調整する

### 1.48 対応済み: 5ch #171 画像フォルダの見開き案内と固定バー / 設定画面 / 操作カスタマイズの小修正

- 出典: mImageViewer 専用スレ #171 (2026-08-05)。
- 画像フォルダが見開きにならない件:
  - v2.10.0 直前の `802d31b4` で「本として扱わないフォルダには `default_spread_mode`
    をフォールバックしない」仕様へ変更済み。`start_loading_items` が読み込み対象の
    `source_path` / `items` から `page_order_locked_for_items` を先に判定し、本でなければ
    `SpreadRestoreDefaults::NON_BOOK` を渡す。
  - 現行の本判定は、製本 / 読書履歴 / ZIP・PDF・直読み RAR・変換キャッシュ /
    `auto_fullscreen_image_folders_enabled()` かつ物理フォルダで中身が通常画像だけ、のいずれか。
    つまり「画像のみのフォルダは、PDF/ZIP のように本として扱う」が OFF の通常画像フォルダは
    単ページ既定になる。明示保存済みの見開き設定は引き続き優先される。
  - 対応 (2026-08-05): **見開きにならない件は仕様据え置き**。利用者確認済み。
    本扱い OFF の通常画像フォルダは単ページ既定とし、動画混在で隣動画を誤再生しない現行仕様を保つ。
- 固定した上部情報バー / 下部シークバーの境界線:
  - 対応 (2026-08-05): 下部バーの 1px 境界線をバー内側へ収め、画像側へはみ出さないようにした。
    上下共通の「固定バーと画像の間隔」を追加し、0〜100px、既定 0px とした。隙間は
    `fullscreen_rect_excluding_fixed_bars` の既存除外計算に加え、ホバー表示と操作矩形は変えていない。
- 環境設定の右ペインスクロール:
  - 対応 (2026-08-05): `show_preferences` の false → true 遷移ごとに右ペインの
    `right_panel_scroll_generation` を進め、同じページを開き直しても毎回先頭から表示する。
- 操作カスタマイズの「場所」フィルタ:
  - 対応 (2026-08-05): コマンド一覧の絞り込み用 context と、割り当て編集セッション用 context を
    別フィールドへ分離した。Key target の編集 context は action から導出し、キー設定後も利用者が
    選んだ一覧の「場所」フィルタを維持する回帰テストを追加した。
- 規模 / 優先度: Small〜Medium / P2。いずれもデータ破壊は無いが、v2.11.0 直後の
  利用者報告なので次版候補。

### 1.49 対応済み: 一覧に出ていないファイルの存在を知らせる (フォルダ削除の安全性を含む)

- 対応 (2026-08-05): 利用者判断どおり **(A) と (B) の両方**を実装した。
  フォルダを含む削除確認では定型の注意文を常に表示し、通常フォルダ一覧では
  `folder_scan` の既存走査結果と同名除外の差分だけから「非表示 N 件」を表示する。
  サブ展開 / スマートフォルダ / 検索結果 / スタック表示は後追い範囲として表示しない。
  件数取得専用の I/O は追加していない。
- 置き場所を変更 (2026-08-05、実機確認): 当初案の絞り込みバーから**フォルダバー右端**へ移した。
  一覧から落ちるのは利用者が足した絞り込み条件ではなく**フォルダの既定動作**なので、
  サブ展開 / スタック / `(2/2)` と同じ列が定位置。フォルダバー右クリックで ON/OFF できる
  (`show_address_bar_omitted_entries`、既定 true)。
- 主数字の定義を修正 (2026-08-05、実機確認): 当初は「対象外拡張子」を主数字から外したが、
  利用者の検証フォルダ (BMP/MAG/PI × 2 + HED/TXT = 8 ファイル、一覧 2 件) で **6 件のうち
  2 件しか主数字に乗らない**ことが判明。除外の動機だった `Thumbs.db` / `desktop.ini` は
  Hidden 属性を持つので隠し項目側へ入り、対象外を外しても静かにならない。ノイズ源を
  **ファイル名で `system` バケットへ分離** (`Thumbs.db` / `ehthumbs.db` / `desktop.ini` /
  `.DS_Store` / `._*`) し、それ以外は種類を問わず主数字へ数える形へ変更した。
- 出典: 利用者メール (pattier、2026-08-04) の「同名で拡張子違いのファイルが 1 つしか出ない」
  報告から派生。報告そのものは仕様どおりで、環境設定 →「同名ファイル」の
  **「同名の画像が複数拡張子で存在する場合、優先度で選択」** (`skip_duplicate_images`、
  既定 ON) を OFF にすれば全部出る。問題は**設定を知らないと、一覧に出ていないファイルが
  ある事実そのものに気づけない**こと。
- 危険な帰結 (これが本題): フォルダ削除は中身を全部消すのに、一覧には代表 1 件しか
  出ていない。「7 個のうち 1 個しか見えていない」と知らないままフォルダごと削除できる。
- 一覧から silent に落ちる経路:
  1. **正規化 = 代表 1 件へ畳む**: `skip_duplicate_images` / `skip_image_if_video_exists` /
     同名フォルダ優先 / `skip_archive_if_zip_exists` ([app/folder_scan.rs](../src/app/folder_scan.rs))
  2. **完全に出ない**: `show_hidden_files` OFF の隠しファイル、AppleDouble `._*`、
     mIV が扱わない拡張子 (`.txt` / `.json` / `.db` など)
  - フィルタ / 検索 / スタックは既にチップ・「X/Y 件」・枚数バッジで見えているので対象外。
    サブ展開の走査時フィルタ (§2.3-2) も条件をフォルダバーに出すので silent ではない。
- 採用方針:
  - **(A) 削除確認ダイアログで明示する** — 安全性の本体。削除対象にフォルダを含むときは
    「フォルダの中には一覧に表示していないファイルも含まれます」を常に出す。件数まで出す
    場合は UI スレッドで数えず worker にする (CLAUDE.md「UI スレッドでの同期 I/O は即 worker 化」)。
    **定型文だけなら実装コストはほぼゼロで目的の大半を満たす**ので、まずこれを入れる。
  - **(B) 一覧に常設の「非表示 N 件」チップ** — 絞り込みバー (非表示ならアドレスバー) に置き、
    クリックで内訳 (正規化 3 / 隠し 2 / 対象外 5) と「同名ファイル設定を開く」導線を出す。
    件数は folder_scan が畳んだ差分をそのまま返すだけで**追加 I/O ゼロ**。スクロール位置に
    依存せず常に見え、既存のフィルタチップ / 「X/Y 件」と同じ語彙に乗る。
  - **(C) 一覧末尾に「他 N 件非表示」セル (利用者案) — 採らない**。仮想スクロールで末尾へ
    行かないと見えず、大量フォルダでは事実上気づけない。加えてソート / 選択 / チェック /
    キーボードナビ / 詳細表示の全経路に「実アイテムでないセル」を通す必要があり、得られる
    情報量に対して影響範囲が大きい。
- 主数字: 当初案は「対象外の拡張子」を数えないことでノイズを避ける方針だったが、上記のとおり
  ノイズ源は隠し項目側へ入るため効かない。最終形は system バケット分離 + それ以外は全部数える。
- 対象範囲: まず通常フォルダ一覧。サブ展開 / スマートフォルダ / 検索結果は後追い。
- 規模 / 優先度: (A) Small / **P2**、(B) Medium / P2。(A) を先に単独で出せる。

### 1.53 対応済み: ナビゲータの縮小画像がカラー化 / 補正の前の絵になっている

- 出典: v2.12.0 出荷前の実機確認 (2026-08-06)。「Alt のナビゲータが、カラー化がきく前の状態で
  表示されている」。カラー化に限らず、補正・AI・注釈・隠蔽・消しゴムも反映されていなかった。
  **見た目の問題なので v2.12.0 は見送り** (利用者判断 2026-08-06)。
- 原因: `draw_fs_navigator` が `self.thumbnails[page_idx]` = **一覧用サムネイル**をそのまま
  貼っていた。表示中の絵は `resolve_fs_processed_texture` が返す別物。
- 対応 (v2.13.0、2026-08-06):
  - 単ページ / 見開きの描画経路が**そのフレームで既に解決済み**の
    `FullscreenPaintResource` をナビゲータへ渡す。新しいテクスチャ生成は行わない。
    ナビゲータからは producer (`resolve_fs_processed_texture`) を呼ばないので、
    見開きの相方ページの合成を余計に走らせない。
  - 使うのは `source_texture()` (拡大前の full-image)。§1.46 の GPU 拡大出力
    (`paint_texture_id()`) は**可視 source 領域だけ**なので、掴むとズーム中に
    切り取られた絵がナビゲータに出る。
  - 加工済み composite が未準備のページだけ従来のサムネイルへフォールバックし、
    黒や点滅を挟まない。
  - nav / colorize holdover を重ねたフレームは、ナビゲータへ渡す idx → resource も
    その display unit 全体へ置き換える。ページ遷移中に本文とナビゲータが新旧ページで
    食い違わないよう、同一 idx のケースを回帰テストで固定した。
- 対象外:
  - パノラマのナビゲータ (`draw_panorama_navigator`) は equirect の全体図という別用途なので
    従来どおり。

### 1.50 対応済み: フルスクリーンのナビゲータ (縮小画像 + 現在表示範囲)

- 出典: 利用者メール (pattier、2026-08-04)。フォトレタッチソフトのナビウィンドウ相当。
  大きな画像で拡大位置をすばやく動かす用途。
- 対応 (2026-08-05):
  - 単ページ / 見開きの拡大中に、サムネイルカタログの縮小画像と現在表示範囲の枠を表示する。
    左ドラッグは範囲指定ズーム、右ドラッグはパン、ダブルクリックは中心移動、ホイールは
    ナビゲータのサイズ変更。ヘッダーの四隅ボタンで配置を選べる。
  - `FsNavigatorToggle` (既定 `Alt+N`) を追加し、表示状態・配置・サイズを `Settings` へ保存する。
    ルーペはカーソル周辺の確認、ナビゲータは全体位置の確認という別用途なので同時表示を許可する。
  - `fullscreen_page_layout` の描画済み `DisplayedImageTransform` を縮小して使い、
    表示範囲は `visible_source_uv_rect`、位置変換は既存の source↔screen API を正本にする。
    UV から `fs_pan` を求める逆変換は単ページ / 見開き / 回転 / 表示トリムの往復テストで固定した。
  - 初期実装では全体可視時に自動で隠していたが、§1.55 で常時表示へ変更した。
  - 第2段階として 360 度パノラマへ対応した。equirect の元画像をサムネイルカタログから全体図として
    表示し、現在の viewport 外周を既存の `ndc_to_equirect_uv` で投影した折れ線を重ねる。
    yaw の継ぎ目は隣接 U の差が 0.5 を超える箇所で線分を分け、全体図を横断する誤線を防ぐ。
    左ドラッグは選択矩形の高さから `fov_y` と中心 yaw / pitch を設定し、右ドラッグは yaw / pitch の
    パン、ダブルクリックは中心移動にする。`FOV_MAX` でも全球は見えないため初期実装から
    常時表示し、§1.55 以降は平面も同じく全体可視時に表示する。
- 対象外:
  - 連結読みは全体図が極端に細長くなり、位置把握に役立たないため表示しない。

### 1.51 対応済み: 5ch #174 一覧ツールバーのファイル名文字列フィルター

- 出典: mImageViewer 専用スレ #174-175 (2026-08-05)。ZipPla 右上のように、文字列入力で
  表示中一覧をファイル名ベースに絞り込みたい。スマートフィルタは種類などのボタン指定で、
  Ctrl+F は都度開く現在地フィルタなので、常設の軽量ファイル名フィルターとして扱う。
- UI 方針:
  - 既存の絞り込みツールバー末尾に小さい単一行入力欄を追加する。絞り込みツールバーは
    横幅を使いがちなので、外側ラベルは置かず、入力欄内のプレースホルダーを `ファイル名` にする。
  - 幅は 160px 固定とし、狭い画面では既存の折り返しレイアウトに任せる。
  - 入力欄の右端または近くにクリアボタンを置く。ツールチップは
    「表示中の一覧をファイル名で絞り込みます」程度。
  - 既存の種類 / 拡張子 / 場所 / ★ / タグ / 日付 / サイズ / 状態 / 画像色などの facet 条件とは
    **AND** で合成する。
- 性能方針:
  - mIV では 50 万件級の一覧表示があり得るため、キー入力ごとに UI スレッドで全件同期スキャンしない。
  - Ctrl+F (`run_metadata_search`) は名前だけでなくメタデータも見る汎用検索なので、これをそのまま
    流用せず、ファイル名だけの軽量経路を作る。
  - 初回使用時だけ basename の小文字化済み文字列を worker で構築し、items 世代が変わったら
    破棄する。準備中は全件を通し、完成時に表示集合を再構築する。
  - 入力変更から 150ms 後、正規化済み文字列への部分一致を既存の `visible_indices` eager パスへ
    AND 条件として加える。別の結果集合は持たず、`details_order` も同じ正本から作る。
  - IME 未確定中は debounce を進めず、確定後の入力だけを適用する。
- 既存機能との差:
  - Ctrl+F は現グリッドを名前 / メタ情報で絞る on-demand 検索で、検索バーを開いて実行する操作。
  - スマートフォルダの `名前に含む` は保存条件で、横断ビュー作成向け。
  - 本件は現在表示中の一覧に常時かける「ファイル名だけの軽量フィルター」。
- 規模 / 優先度: Medium / **P2**。UI 追加自体は小さいが、50 万件級、仮想スクロール、
  詳細表示、既存 facet との合成、IME 中入力停止に注意。

### 1.52 対応済み: 見開きの高さ合わせがズーム倍率で入り切りして、レイアウトが跳ねる

- 症状: 解像度の違う 2 ページを見開きで開き、100%原寸から**ズームすると、
  低解像度ページが突然拡大されて高さが揃う**。縮小でも同じ。倍率を戻すと元に戻る。
  v2.12.0 の実機確認中に利用者が発見 (2026-08-05)。
- 原因: [src/ui_fullscreen.rs:5455](../src/ui_fullscreen.rs) が、**そのフレームの物理倍率が
  整数に近いかどうか**で高さ合わせの有無を切り替えている。

  ```
  倍率 1.00 → 高さ合わせなし (原寸で並ぶ)
  倍率 1.01 → 高さ合わせあり (低解像度ページが跳ね上がる)
  倍率 2.00 → 高さ合わせなし
  ```

  意図は「整数倍率のとき両ページともドットバイドットにする」こと。`b38610d5`
  (v2.11.0「Make 100% mean one image pixel per screen pixel」) で入った。**新規退行ではない。**
- **この分岐は左右の高さが違うときにしか何もしない。** 高さが同じなら倍率係数が両方 1.0 に
  なり、合わせありと合わせなしが一致する。つまり観測可能な効果は
  「解像度違いの見開きでレイアウトが不連続に飛ぶこと」だけ。
- 対応 (利用者合意・実装済み、2026-08-05): **判定をそのときの倍率ではなくフィット方式へ移した。**
  - `FullscreenFitMode::Original` (100%原寸) → 高さ合わせしない。**どの倍率でもしない**
  - それ以外のフィット方式 → 常に高さ合わせする

  これで跳ねが消え、100%原寸で原寸のまま並ぶ現在の挙動も残る。高さが同じ見開きは
  分岐が元から no-op なので影響なし。代償は、フィット方式で倍率がたまたま整数になったとき
  低い方のページが 1:1 でなくなること (もともとその倍率でしか成立していない偶然)。
- ⚠ 事前記述の訂正: `preserve_page_sizes` が Z ズーム確定を走らせる別の条件も兼ねている、
  という読みは誤りだった。実際は次の 2 段解決だった。
  1. 高さ合わせ geometry で Z ズームを解決して候補倍率を出す
  2. 候補倍率が整数近傍なら固有寸法 geometry へ切り替え、同じ `total_scale` を保ったまま
     composite とパンを再解決する

  2 回目も geometry 切替に伴う同じ関心で、分離すべき別状態ではなかった。フィット方式なら
  geometry を解決前に確定できるため、`resolve_spread_zip_zoom` は 1 回だけ呼び、
  `forced_total_scale` の往復も削除した。倍率結果で geometry を再選択しないため、旧コメントが
  警告していた両 geometry 間のフィードバックループも構造的に消えた。
- 通常表示と Z ズームが共用する見開き描画、連結読み、detached frozen snapshot の 3 箇所にある
  同型判定は、`fit_mode` だけを受け取る共有 helper に集約した。
- 回帰テスト: 解像度違いの見開きで全倍率を通じて `Original` は固有寸法、それ以外は高さ合わせを
  維持すること、同じ高さの見開きは全フィット方式・全倍率で不変なこと、連結読みも同じ契約に
  従うこと、非 `Original` の Z ズーム倍率・パンが従来の高さ合わせ composite と一致することを固定した。

### 1.54 フォルダツリーペインの幅が読み込み中に伸び続ける (DVD ドライブで顕著) — 利用者報告

- 出典: 利用者報告 2026-08-07 (動画添付)。「ディスクの入っていない DVD ドライブを開こうと
  するとツリーの境目が横にびよーんと動く。SSD など通常のドライブでも一瞬動く」。
- 観測 (添付動画をフレーム分解して確認): ドライブ切替直後から**対象行のスピナーが出ている
  間ずっと、ペイン幅が 1 フレームごとに単調増加**し、`width_range` の上限で頭打ちになる。
  **元の幅には戻らない**。通常ドライブで「一瞬だけ少量動く」のは、走査が速くスピナーが
  数フレームしか出ないため。
- 根本原因 (3 段の合成。症状は「幅」だが、起点は 1 行のレイアウト算術):
  1. [src/ui_folder_pane.rs:301](../src/ui_folder_pane.rs) の行レイアウトで、末尾ウィジェット用の
     予約幅が `trailing_width = 18.0` の固定値。実際に消費するのは
     `item_spacing.x (2.0) + Spinner 幅 (= spacing.interact_size.y = 既定 18.0) = 20.0` なので、
     `row.loading` の行だけ**毎フレーム 2pt はみ出す**。`row.error` の `!` は文字幅が小さく
     予約内に収まるため、はみ出すのはスピナーの行だけ。
  2. ツリー本体の `ScrollArea::vertical().auto_shrink([false, false])` は横方向が
     (スクロール無効, auto_shrink false) の組み合わせで、egui 0.33 `scroll_area.rs` の
     `(false, false) => inner_size.max(content_size)` に落ちる = **内容に合わせて横に広がる**。
  3. `egui::SidePanel` は `PanelState` へ、要求した `panel_rect` ではなく **frame の content rect**
     (`let rect = inner_response.response.rect; … PanelState { rect }.store(...)`) を保存する。
     次フレームの幅がその値になるので、1 の 2pt が毎フレーム積算される**フィードバックループ**
     になる。区切り線と中央パネルの開始位置も content rect 基準 (`side_x(rect)` /
     `allocate_left_panel`) なので、同じフレームのうちに境界が動いて見える。
  - 加速要因: `Spinner::paint_at` が毎フレーム `ctx.request_repaint()` を呼ぶため、走査待ちの
    間は最高フレームレートで加算され続ける。ディスク未挿入の光学ドライブは Windows の列挙が
    数秒〜十数秒返らないので上限まで到達する。
  - 副作用: 増えた幅は `render_folder_pane` の write-back で
    `settings.folder_tree_pane_width_ratio` に入る = **設定として保存され、再起動しても太いまま**。
- 修正方針 (症状パッチではなく、はみ出しを作らない):
  1. 予約幅を実測から作る (`ui.spacing().item_spacing.x + ui.spacing().interact_size.y`)、または
     末尾を `Layout::right_to_left` の子 Ui で先に確保する。マジックナンバー 18.0 をやめる。
  2. 「幅が内容から決まる」構造自体への保険として、ペイン本体は available_width を超えて
     確保しないことを不変条件にし、**loading 行を出したまま複数フレーム回してもペイン幅が
     変わらない**ことを egui_kittest で固定する (状態遷移テスト)。
  3. 同型の「予約幅を持つ行に `ui.spinner()` を置く」箇所が他に無いか確認する
     (現状は本件のみ。他の spinner はダイアログ内で自動サイズなので影響しない)。
- 別件として扱うもの: ディスク未挿入の光学ドライブで列挙が長時間返らないこと自体は Windows の
  挙動。上記修正後は「スピナーが長く出る」だけになる。
- 優先度: P2 (見た目の破綻 + 設定の汚染)。規模: 小。
- 実装記録 (2026-08-09): 行末予約幅を現在の item spacing と interact size から算出し、
  ScrollArea を載せるペイン本体は親 UI で available size を先に厳密確保する構造へ変更した。
  loading 行を 8 フレーム描いても SidePanel 幅が変化しない egui_kittest 回帰テストを追加。
  同型検索では、予約幅を自前計算した行に spinner を置く箇所は本件以外になかった。

### 1.55 ナビゲータの残り 2 件 (常に表示する / 比較表示中も出す) — 利用者要望

- 出典: 利用者メール (pattier、2026-08-06) と利用者判断 (2026-08-07)。v2.12.0 で出したナビゲータ
  (§1.50) への追加要望。**タッチパネル対応の作業が落ち着いてから着手する**。
1. **画像全体が見えているときも表示する**: 現在は
   [ui_fullscreen.rs:475](../src/ui_fullscreen.rs:475) で「全ページが完全に見えているなら枠が
   全面になって情報がないので隠す」としている。実際にはナビゲータ上のドラッグ範囲指定ズームが
   等倍表示のときこそ使いたい操作なので、**隠さない**方針に変える。
   - 表示範囲が画像より広いときは、**枠をナビゲータ全域に置き、画像側を内側へ縮めて描く**
     (利用者決定)。「今は画像全体より広く見えている」ことが読み取れる形にする。
   - 画像の縮小には**下限を設ける** (例: キャンバスの 40%)。倍率へ完全連動させると、ズームアウトの
     たびに絵が縮んで用をなさなくなる。フィット丁度では画像＝枠＝全域で連続する。
2. **比較表示 (X / C) 中も出す**: 現在は [ui_fullscreen.rs:14693](../src/ui_fullscreen.rs:14693) の
   `matches!(self.compare_view_mode, CompareViewMode::Off)` で明示的に切っている。
   - 出す絵は「**いま画面に出ているのと同じ合成結果の全体像**」(利用者決定)。
   - 実装は新しいシェーダ不要。[`compare_shader_shape`](../src/ui_fullscreen.rs:20297) は
     コールバックへ両画像の全体 RGBA を渡し、`draw_rect` へ画像全体を写す作りなので、
     **`image_rect` にナビゲータのパネル矩形、`zoom_pan` に `None` (フィット) を渡して再度呼ぶ**
     だけで同じ合成が全体像として得られる。`Wipe` / `Diff` は同経路、`PinnedNormal` だけは
     準備済みテクスチャを直接フィット描画する。
   - 注意: 1 フレームに合成パスが 2 回走る。`CompareShaderCallback` が `pair.key` で GPU 側の
     テクスチャを再利用するかを確認する。Shape はナビゲータのクリップ矩形へ乗せる。
- 規模 / 優先度: 各 Small〜Medium / P2。
- 実装記録 (2026-08-09、1): 全体可視時の自動非表示を廃止した。現在の実表示がフィット基準より
  小さいときは、表示範囲枠を全体枠に保ったまま縮小画像を内側へ連続的に縮め、下限を 40% にした。
  フィット丁度では画像 = 枠 = 全体となる連続性と、下限を unit test で固定した。
- 実装記録 (2026-08-09、2): 比較中も単一の比較キャンバスを `FullscreenPageLayout` へ登録し、
  ナビゲータの画像矩形へ `zoom_pan=None` で同じ Wipe / Diff callback を再描画する。
  PinnedNormal は準備済み pinned texture を直接描画する。GPU 側の
  `CompareGpuResources::ensure_pair` は同じ `pair.key` / 寸法なら既存の2 textureとmip chainを
  再利用するため、2回目の pass に texture upload / mipmap 再生成はない。
- 実装追記 (2026-08-09、3): 上記の初期実装は texture と一緒に uniform / bind group まで共有し、
  1フレーム内で本文の `prepare` が書いた部分UVを、後続するナビゲータの `prepare` の全域UVで
  上書きしていた。`egui-wgpu` は全callbackの `prepare` 後に全callbackを `paint` するため、本文も
  ナビゲータ用uniformで描画される退行になった。`CompareShaderSlot::{Main, Navigator}` を追加し、
  texture / mip chainは従来どおり1組だけ共有しつつ、uniform buffer / bind groupだけをslotごとに
  分離した。一般則として、**1フレームに同じGPU resourceを使うcallbackを複数出す場合、prepareで
  callbackごとに変わるGPU状態は共有せず、paintまで生存するper-callback slotへ分ける**。

### 1.56 ルーペが画像の端で像を引き延ばす — 利用者報告

- 出典: 利用者メール (pattier、v2.12.0 で確認)。ルーペ ON でカーソルを画像の外周へ寄せると、
  拡大像が引き延ばされる。
- 原因: [ui_fullscreen.rs:21538 付近](../src/ui_fullscreen.rs:21538) で、サンプルする uv 矩形の
  min / max を **それぞれ独立に `clamp(0.0, 1.0)`** している一方、描画先の `loupe_rect` は
  `LOUPE_SIZE` の正方形で固定。端では uv 側だけが小さく / 非正方形になり、それを正方形へ
  引き伸ばすため像が歪む。
- 方針: **(c) に決定** (利用者判断 2026-08-07)。倍率を一定に保ったまま**ルーペの中身がパンし、
  画像の外側にあたる部分は背景色で埋める**。位置も倍率も正しく、動きとして何が起きているかが
  読み取れる。
  - 採らない案とその理由: (a) uv 窓をクランプせずスライドさせる = 歪まないが、端でルーペ中心と
    カーソル位置がずれ、画素検分では嘘になる。(b) 描画先をレターボックスに縮める = 実装は最小だが、
    ルーペの大きさが端で変わる。
- 規模 / 優先度: Small / P2。
- 実装記録 (2026-08-09): 要求 UV 窓を画像範囲でクリップすると同時に、同じ比率で画像の
  描画先部分矩形も切り出す純関数を追加した。ルーペ全体は既存の黒背景で先に塗るため、画像端でも
  カーソル位置と倍率を保ち、画像外だけが黒く残る。中央・左端・右上隅・表示より小さい画像で、
  source px あたりの描画倍率と背景範囲を固定する回帰テストを追加した。
- 追加修正 (2026-08-09、実機確認で判明): 引き延ばしを直したことで、**カーソルが画像の外へ出た
  瞬間にルーペごと消える**別の打ち切りが目立つようになった。原因は
  `fullscreen_page_layout.hit_test(cursor)` が対象ページを引けないと描画を打ち切っていたこと
  (元からの挙動)。`FullscreenPageLayout::hit_test_or_nearest_in_window` を足し、**拡大窓
  (画面上 `LOUPE_SIZE / LOUPE_ZOOM` = 100pt 四方) と重なるページのうち最も近いもの**を対象に
  する。拡大するものが窓に残っている間だけ表示を保ち、離れれば従来どおり消える。窓 0 なら
  `hit_test` と同値。単ページ・見開きの谷間・窓外の 3 条件を純関数テストで固定した。

### 1.57 テキスト注釈の色ピッカーが、パネル外を最初にクリックすると閉じる — 利用者報告

- 出典: 利用者メール (pattier、v2.12.0)。開発側でも再現確認済み。色見本をクリックして開く
  グラデーションのポップアップが、**詳細設定パネルの矩形の外側**でクリックすると反応せずに閉じる
  (枠内で押してからドラッグすれば操作できる)。
- 原因: [ui_text.rs:2383](../src/ui_text.rs:2383) で、キャンバス入力を無視するかどうかを
  `panel_rect.contains(p) || detail_rect.contains(p)` という **手で列挙したパネル矩形との包含判定**で
  決めている。egui の色ピッカーは独立した浮動 Area で、パネル外へはみ出して開くため、はみ出した
  部分のクリックが「キャンバスのクリック」と判定されてキャンバス側処理が走る。
- 方針: 矩形の列挙をやめ、**ポインタ直下に `Order::Middle` / `Foreground` の浮動レイヤがあるかを
  egui に問い合わせる**判定へ寄せる。同型の前例が既にあり、CLAUDE.md「ダイアログ」節の
  `App::process_scroll` が同じ考え方で背面グリッドの wheel を止めている。矩形の列挙のままだと、
  次にポップアップを増やしたときに同じバグが再発する。
- 規模 / 優先度: Small / P2 (症状は操作不能なので体感は大きい)。
- 実装記録 (2026-08-09): キャンバス入力の抑止判定をパネル矩形の列挙から、ポインタ直下の
  Middle / Foreground 浮動レイヤ照会へ置き換えた。パネル外の浮動 Area を表示した次フレームの
  クリックはキャンバスへ渡さず、浮動レイヤが無いクリックは従来どおり通す状態遷移テストを追加した。

### 1.58 ページ送り (キー押しっぱなし) が単ページ表示だけ引っかかる — perf log 実測あり

- 出典: 利用者メール (pattier) → 開発側で `--perf-log` 取得 (2026-08-07、5600×3500 JPEG 12 枚、
  `C:\tmp\miv-speed-test`)。連結読みは滑らかで単ページだけ引っかかる、という報告の裏取り。
- **仮説の訂正**: 当初疑った「current が表示待ちのとき他の pending を全キャンセルする経路」は
  **発生していなかった** (`fs/cancel_check` 132 件すべて `cancelled=false`、`thread_exit` は全件
  `static_ok`)。原因は別。
- 実測 (p50): キーリピート間隔 **34ms** に対し、実際に表示できたページ間隔 (ready→ready) は
  **84ms**。デコード 103ms (ワーカー、並列なので問題なし)、**アップロード 21.3ms**、
  **`final_composite_build` 21.0ms** (中身は `edit_result` upload)。`fs_viewport_breakdown` の
  `media_ms` は定常 22〜26ms、初回オープンのみ 466ms。低解像度サムネイルで描いたフレームが
  163 回中 47 回。
- 分かったこと:
  1. **キー 1 回ごとに 1 ページを完全に実体化しようとしている**。UI スレッドだけで
     21 + 21 ≒ 42ms/枚かかり、34ms 間隔の入力に構造的に追いつかない。連結読みが速いのは、
     スクロール中は**実体化済みのページを見せるだけ**でこのコストがキーごとに出ないから。
  2. **同じ画像を何度も読み直している**。12 枚のフォルダで `load_begin` が **132 回**
     (1 枚あたり 7〜15 回)、デコード合計 **25.6 秒** (1 回ずつなら約 1.2 秒)。
     同一 idx が 200ms 以内に Normal → Critical で二重に走る例もある。
  3. 次ページの最終合成は一度も先読みできていない (`final_effect_prefetch_blocked` は全件
     `reason=not_in_keep_set`)。
- 対策 (効く順):
  1. **キーリピートの合流**: 次のキーが来ている間は途中ページを実体化しない。通過ページは
     アップロード済みのカタログサムネイルで 1 フレーム描く。**ページ表示自体は飛ばさない**
     (利用者要件: 一瞬でも見える形にする)。UI スレッド 42ms → 1〜2ms になるので、現状より
     多くのページが実際に描画されるようになる。回帰テストは「押しっぱなし中に各 idx の
     `paint` が最低 1 回出る」で固定する。サムネイル未生成のページは従来どおりデコード待ち。
  2. **再デコードの抑止**: keep 判定が滑る条件と、`loaded_texels` が途中で 0 に落ちる箇所を追う。
  3. **UI スレッドの 42ms を削る**: 表示解像度へ縮めてからアップロードする / `edit_result` の
     アップロードを 1 フレーム 1 枚のバックログへ回して 1 フレームに 2 回分を乗せない。
  4. **最終合成の先読み**: 次ページ分だけでも作れば、切り替えはテクスチャ差し替えで済む。
- 規模 / 優先度: 1 = Medium / **P2**、2〜4 = Medium / P3。
- 対策1 実装記録 (2026-08-09): 通常の単ページ表示で、描画準備より前に同じ viewport の
  Win32 frame edge queue を読み取り、**そのフレームに未消費の前後ページ入力が残る**場合だけ
  現在 idx を通過ページと判定する。時間閾値・キー押下開始時刻・前フレームからの pending 状態は
  判定に使わない。同一 egui frame の再 pass は frame / items generation / idx 付きの一時 cache で
  同じ判定を再生する。カタログサムネイルが Loaded の通過ページでは processed resolver、
  `fs_upload_backlog` の upload、完成済み final-effect の upload を保留し、サムネイルを 1 frame
  paint する。未消費入力が無い最初の frame は即 `Materialize` に戻り、保留結果を通常経路で
  upload / 最終合成する。サムネイル未生成時は常に既存の materialize / decode 待ちを使う。
  見開き、連結読み、ホイール、クリック、seek drag、ゲームパッド、スライドショーは対象外。
  `fs.page_turn_ready` (`mode=pass_through|materialized`) と
  `python scripts/analyze_perf.py <jsonl> page-turn` を追加した。対策2へ進む判断は、この対策1を
  同じ実機条件で再計測し、通過/実体化枚数と ready→ready を確認してから行う。
- 戻り方向の残件調査 (2026-08-09、Codex): ソース上は通常単ページ predicate、固定の
  Left / Right / Up / Down、カスタム `FsPagePrev` / `FsPageNext`、曖昧 chord 除外のいずれにも
  前進 / 後退の非対称条件がなく、既存 `page_turn_ready mode=materialized` だけでは
  `ordinary context`、pending 0、曖昧 chord、catalog thumbnail 未準備のどこで落ちたか確定
  できなかった。このため判定条件は緩めず、時間閾値も追加していない。
  `fs.page_turn_decision` に判定理由、通常 context blocker、Win32 frame queue の全 key-down 数、
  候補 / eligible ページ送り edge 数・repeat 数・chord を追加した。さらに read-only の
  `raw_input_hook` から `fs.page_turn_winit_input` に egui-winit 翻訳直後の `RawInput` 数を残し、
  `fs.page_turn_egui_input` に root / embedded / fullscreen / detached の egui 処理後
  `Event::Key` 数・repeat 数・chord を追加した。
  `analyze_perf.py page-turn` はページ入力のあった frame だけを相関し、false 理由と
  Win32 pending/repeat/matching 対 winit/egui の各 press/repeat の署名を集計する。これで次の
  実機ログから「Win32 到着前」「Win32 から egui-winit 翻訳まで」「RawInput から egui 処理後」
  「全段にあるが通常 context / 曖昧性で却下」を区別する。既存の `fs_upload_backlog` /
  final-effect upload 抑制は対策1と同じ
  materialization 判定へ既に接続済みのため、原因確定までは挙動を変更しない。
- 見開き拡張 (2026-08-09): 上記の実機診断では 423 件すべてが
  `ordinary_blocker=spread_mode` だったため、この blocker を撤去して対策1を paged の
  見開きにも適用した。現在 idx を含む `SpreadDisplayUnit` を通常描画と同じ resolver で求め、
  unit 内の全ページが `ThumbnailState::Loaded` の場合だけ通過扱いにする。片方でも未生成、
  または unit を解決できなければ `Materialize` に倒す。通過中の `draw_fs_spread` は左右とも
  processed resolver を呼ばず、カタログサムネイルで unit 全体を 1 frame 描く。入力判定は
  従来どおり同一 frame の未消費ページ送り edge だけで、時間ガードと連結読みの対象範囲は
  変更していない。単ページだけを対象としていた上の初回実装記録を、この追記で拡張した。
- **残した挙動 (利用者判断 2026-08-10「支障なし、このままでよい」)**: フォルダ末尾でキーを
  押し続けると、**最後のページがサムネイル画質のまま留まる**。ページは進まないがキーリピートは
  届き続けるので「次のページ送りが保留されている」が成立し続けるため。キーを離せば実体化する。
  直すなら判定に「実際にページが変わったか」を足すことになるが、そのための状態を 1 つ増やす
  価値はないと判断した。再検討するときは、行き止まりを「通過中」と区別する形にすること。

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
- 規模 / 優先度: Medium / P3 (提案段階。採否未定)。

### 1.60 ワイプ比較でズームすると境界線と掴み位置がずれる — 利用者報告

- 出典: 利用者報告 (2026-08-07、v2.12.0)。`Shift+C` のワイプ比較中にズームすると挙動が乱れ、
  ワイプの境界 (掴むカーソル) がずれる。
- 原因: **描画と入力で別の矩形を基準にしている**。
  - 描画: [ui_fullscreen.rs:21112 付近](../src/ui_fullscreen.rs:21112) の `draw_compare_wipe_line` は
    `draw_rect` (= `compare_image_draw_rect` が返す、fit とズーム・パンを反映した実表示画像矩形)
    を基準にする。シェーダの合成境界も同じ `draw_rect` 内で決まる。
  - 入力: [`handle_compare_wipe_drag`](../src/ui_fullscreen.rs:14628) は `image_rect`
    (= ビューポート全体) を基準に `line_x` と `new_fraction` を計算している。
  - 等倍かつ画像が画面幅いっぱいのときだけ両者が一致するので今まで表面化しなかった。ズームすると
    `draw_rect` だけが動いてずれ、掴むと `image_rect` 基準で再計算されるため境界が跳ぶ。
    縦横比の違いでレターボックスが出ている場合はズームなしでもずれる。
- 方針: **合成境界・線の描画・掴み判定を同じ矩形から導く**。`compare_image_draw_rect` は
  `image_rect` / `pair.target_size` / `zoom_pan` / fit 上限から決まる純粋な計算なので、入力
  ハンドラ側でも同じ値を求めて使う。前フレームの `draw_rect` を保存する方式はズーム中に 1 フレーム
  遅れるので採らない。掴み判定の範囲も `image_rect` ではなく `draw_rect` に合わせる。
- 関連: §1.55-2 (比較中のナビゲータ表示) と同じ描画経路を触るので、着手はまとめてよい。
- 規模 / 優先度: Small / P2。
- 実装記録 (2026-08-09): fraction を「実表示 `draw_rect` の左端 0 / 右端 1」とする純関数へ
  集約し、白線、CPU fallback の clip、掴み判定、ドラッグ更新を同じ往復変換へ通した。
  等倍・ズーム + パン・レターボックスの3条件で、描画線 x から入力 fraction が往復一致する
  unit test を追加した。前フレームの矩形は保存せず、入力フレームの
  `compare_image_draw_rect` を使う。
- 実装追記 (2026-08-09): GPU callback が画面外へ出た `draw_rect` をそのまま viewport に
  指定し、egui-wgpu のクランプ後の矩形へ合成画像全体を押し込んでいた残件を修正した。
  callback rect を `draw_rect ∩ image_rect` へ制限し、切り落とした範囲を UV 窓として
  uniform へ渡す。テクスチャ採取と Wipe 判定は UV 窓で復元した合成画像座標を使うため、
  白線は引き続き `draw_rect` 基準のまま一致する。ナビゲータは `zoom_pan=None` で
  `visible == draw_rect`、UV 窓 `(0,0)-(1,1)` のため表示を変えない。
- 実機ログによる残件の真因 (2026-08-09): 上記の `draw_rect` / `visible` / 部分 `uv_window` は
  正しかったが、§1.55-2で同一フレームに追加されたナビゲータcallbackが、同じ `pair.key` の
  単一uniform bufferへ全域 `uv_window=(0,0)-(1,1)` を後書きしていた。`egui-wgpu` は全callbackの
  `prepare`を先に回してから`paint`するため、本文のpaint時点には部分UVが失われ、合成画像全体を
  クリップ済みcallback矩形へ引き伸ばしていた。本文 / ナビゲータをtyped slotに分け、重いtexture /
  mip chainは共有したままuniform buffer / bind groupだけを分離した。1フレームに複数callbackを
  出すときは、prepareで変わるGPU状態をpaintまで共有してはならない。
- 画面端クランプ追記 (2026-08-09): drag 中の pointer x は、シェーダ callback と同じ
  `compare_shader_visible_region(draw_rect, viewport)` が返す `visible` の左右端へ先にクランプし、
  その x を full `draw_rect` の fraction へ戻す。ズーム / パンで画像が画面外へ広がっても境界は
  可視端で止まり、保存した fraction を `compare_wipe_screen_x(draw_rect, fraction)` へ通すため、
  白線・clip・掴み位置は引き続き同じ full-image 座標を共有する。等倍と 2x zoom の両方で、
  viewport 外の pointer が可視左 / 右端の fraction で頭打ちになる unit test を追加した。
- ページ移動中の stale prepared pair 修正 (2026-08-09): 本文の
  `draw_compare_prepared_mode` もナビゲータと同じ `compare_prepared_pair_matches(fs_idx)` を通す。
  準備済み pair の `current_idx` が現在ページと違う間は古い合成を描かず、既存の通常表示 fallback
  と「比較表示を準備中」を維持する。別ページの pair を本文が描画済みとして扱わない回帰 test を
  追加した。
- 新規準備時の縮退 pair 修正 (2026-08-10、実機確認待ち): `prepare_capture_pixel_work` は
  replacement load (`fs_pending`) 中でも在住中の旧 `fs_cache` から complete な final composite を
  作れたため、確定前の source を worker が snapshot し、その寸法を正当な完了結果として
  `ComparePreparedPair` へ publish できた。従来の完了側検査は `w × h × 4` の byte 数しか見ず、
  実機ログの縮退寸法も通していた。`ComparePreparationState` を
  `Unprepared / WaitingForSource / Preparing / Ready` の排他的 enum にし、replacement load と
  final composite 未完了は `WaitingForSource` のまま通常ページを描く。source が canonical 寸法と
  同じ縦横倍率であること、worker 結果が crop / rotation / 見開き union から事前計算した寸法と
  一致することを確認し、worker 開始後にいずれかの source replacement が始まった競合でも
  完了結果を withheld してからだけ `Ready` を公開する。literal な縮退寸法の分岐は置いていない。
  source reload の generation 境界でも同じ状態を失効させ、旧 worker 完了を同一 idx へ戻さない。
  `[compare-geometry]` の一時計測は実機確認まで残す。
- 上記見立ての訂正と真因 (2026-08-10): 利用者実機では `ComparePreparationState` 導入後も症状が
  残り、利用者の切り分けと `[compare-geometry]` ログから、4×4 の縮退 pair 説は誤りと確定した。
  真因は縦横比が異なる場合の pinned 整列バッファで、current 自身の寸法を `target_size` とする
  キャンバスへ pinned を等比縮小・中央配置した際、未書き込み余白が透明 RGBA のままだったこと。
  WGPU は alpha blending、CPU fallback の Wipe も current を先に描くため、その透明余白から
  current が見えて `[current | pinned | current]` になっていた。`target_size` の決め方は維持し、
  比較準備 worker が pinned の範囲外をフルスクリーン既定背景と同じ不透明な黒で埋めるよう修正。
  PinnedNormal / Wipe は黒い余白を表示し、Diff は黒背景と current の差を余白にも表示する。
  異なる縦横比の余白画素と、同じ縦横比では従来の Lanczos 出力が変わらないことを単体テストで固定した。
- 3 回目の真因 (2026-08-10): 不透明な黒余白へ直した後の実機スクリーンショットで、比較矩形の
  外側に通常の現在ページが別レイヤーとして残ることを確認した。`draw_compare_prepared_mode` が
  `false` を返す準備中 fallback は、単ページでは `draw_fs_image`、見開きでは `draw_fs_spread` で
  通常ページを描いた直後、Wipe の raw pinned texture を狭い矩形へ重ねる二重描画だった。
  PinnedNormal も準備中に raw pinned texture を先行表示し、通常ページだけを出す契約に反していた。
  さらに primary の後段にある colorize / folder-nav の display-unit holdover は比較描画の成否を
  見ず、状態が残っていれば通常ページ unit を同じ frame へ追加できた。
  `CompareFramePrimaryDraw::{OrdinaryOnly, PreparedCompareOnly}` を primary 描画の単一 decision とし、
  current に一致する `Ready` のときだけ比較を描く。それ以外は通常ページだけを描き、比較を描いた
  フレームでは後段の nav / colorize holdover も重ねない。単ページ・見開きの両経路を同じ decision
  に通し、準備中 / 準備完了 / 比較 OFF の排他を単体テストで固定した。
- 見開き比較OOMの計測と対策 (2026-08-10、実機確認待ち): 実機の最大canvas
  `8320x7296` はRGBA 1枚が242,810,880 bytes。修正前の`ComparePreparedPair`は
  pinned/currentの`Vec<u8>` 2枚とpinned/current/diffの`ColorImage` 3枚を重複保持していたため、
  定常CPU量は1,214,054,400 bytesだった。Windows callbackのpinned/current完全mip 2枚は
  647,495,328 bytesで、旧GPU組は新規確保前にdrop済みだった。mipは大縮小時のモアレ抑制に
  実使用されているため撤去しない。
  - 準備payloadをtyped化し、WindowsのWipe/DiffはRGBA 2枚 (同寸法なら485,621,760 bytes)、
    PinnedNormalはpinned 1枚、CPU fallbackのWipeは2枚、DiffはDiff時だけ差分1枚を保持する。
    `TextureHandle`へは`Arc<ColorImage>`を直接渡し、追加のdeep cloneも作らない。
  - ページ移動で旧receiverを捨てて次の途中cancel不能workerを並走できた経路を、
    `Preparing` / `Draining`の単一ownerへ直列化した。旧CPU pairとcallback GPU pairを次worker開始前に
    解放し、`[compare-memory]`へtarget、CPU/GPU合計bytes、buffer/texture数、mip有無、解放順を出す。
  - 上記だけでは問題canvasのGPU量647,495,328 bytesが減らないため、見開き中だけ現在ページ1枚を
    比較対象にする。問題の左右同寸法ケースでは`4160x7296`となり、CPU 242,810,880 bytes、
    完全mip付きGPU 323,747,432 bytesへ半減する。固定サイズ上限、空きメモリ依存分岐、OOMの
    握りつぶしは入れていない。

### 1.61 見開きでページが 1 枚ダブる (横長ページの寸法をキャッシュ在住に頼っている) — 利用者報告

- 出典: 利用者報告 2026-08-07 (Windows 10 / v2.12.0 ポータブル版)。縦横混在フォルダを
  詳細表示から開いて見開きで読み進めると、途中で 1 ページが 2 回表示される。
  例 1 (01 = 横長、02〜11 = 縦長、表紙なし見開き): `1 / 3,2 / 5,4 / 7,6 / **8,7** / 10,9`。
  戻ると `… 8,7 / 一瞬 6,5 → 5,4 / 3,2 / 1`。
  **サムネイル表示から開くと再現しない**。先読み (後方) の枚数で発生位置が動く
  (2-3 枚 → 5 でダブる / 4-5 枚 → 7 / 6-7 枚 → 9)。
- **修正前の原因は特定済み (報告の 3 つの数値すべてと一致)**:
  1. 見開きの組み方は毎回 `build_spread_display_units_with_predicates`
     ([src/ui_fullscreen.rs:5960](../src/ui_fullscreen.rs)) が **nav の先頭から歩き直して**
     決める。横長ページは単独ユニットになるので、そこから後ろの偶奇が決まる。
  2. 修正前の判定 `is_landscape` ([src/ui_fullscreen.rs:5838](../src/ui_fullscreen.rs)) は
     **フルスクリーンのテクスチャキャッシュ `fs_cache` かグリッドの `thumbnails` にしか
     寸法を聞かない**。どちらにも無ければ「**不明 = 縦長**」と返す (関数の doc にもそう書いてある)。
  3. `fs_cache` は先読み設定ぶんの窓なので、読み進めると横長ページ (例 1 の idx 0) が
     窓から落ちる。落ちた瞬間に `is_landscape(0)` が true → false へ変わり、
     **単独だったページが対になって以降の偶奇が 1 つずれる** → 直前に出したページが
     もう一度出る。戻ると窓に戻ってきて偶奇が戻る = 報告の「一瞬だけ 6,5」。
  - 発生位置が先読み (後方) 枚数 N と一致する: N=2/3 → idx 0 が落ちるのが 1 ユニット早く、
    N=6/7 → 1 ユニット遅い。報告の 5 / 7 / 9 とそのまま合う。
- **詳細表示が効く理由**: `update_keep_range_and_requests`
  ([src/app.rs:30057](../src/app.rs)) は詳細表示のとき `keep_range = (0,0)` にして
  **グリッドのサムネイルを全部 Evicted にし、要求キューも drain する** (詳細表示は独自の
  プレビュー経路を持つため意図的な抑止)。`ThumbnailState::Evicted` は `source_dims` を
  持たないので、詳細表示では 2 番目の寸法ソースが最初から消えている。
  サムネイル表示では 11 枚全部が keep 範囲に入るので寸法が常に分かり、再現しない。
- **影響範囲は詳細表示に限定されない (2026-08-09 ソース確認で確定)**:
  `evict_grid_thumbnail` は表示モードを問わず `Loaded` を `Evicted` にするため、通常の
  サムネイル表示でも横長ページが keep 範囲から外れれば同じ既知 → 未知の後退が起きる。
  11 枚の報告データでは全件が keep 範囲に残るためサムネイル表示で再現しなかっただけで、
  枚数が多い通常表示も同じ根因の対象。
- **実装記録 (2026-08-09、Codex 実装 / ClaudeCode レビュー・利用者実機確認待ち)**:
  - `src/page_dims.rs` に generation 付き `PageDimsCache` を追加した。`ViewerContextBundle` と
    App の同名 field で `items` / `thumbnails` / `fs_cache` と一緒に所有・swap し、generation
    空間を共有しない main / detached 間で同じ世代番号の別 item 寸法を混同しない。
  - 記録は `poll_thumbnails` の `Loaded` 遷移時と、フルスクリーン frame 冒頭で live な
    `fs_cache` を 1 回 harvest する 2 箇所だけ。テクスチャ退去では消さず、idx 空間変更時の
    正規フック `invalidate_idx_state_and_queues` だけで clear する。
  - cache 自身も `items_generation` を刻印し、不一致なら `None` を返す fail-closed とした。
    掃除漏れがあっても別 items の同じ idx へ寸法を誤適用せず、従来どおり未知へ倒す。
  - `is_landscape` は `fs_cache` → `ThumbnailState::Loaded` → `PageDimsCache` の順で読む。
    一度も寸法が分からないページの `false` fallback は維持し、禁止すべき既知 → 未知の後退だけを
    解消した。回転は従来どおり判定へ反映せず、見開きアルゴリズムと先読み範囲も変更していない。
  - 純関数の先頭横長 / 後退形、fs cache + thumbnail 退去後も境界不変の App 状態遷移、
    generation fail-closed、同世代番号の main / detached 分離を回帰テストへ追加した。
  - カタログ DB からの寸法先読みは今回行わない。未知ページの初期精度を将来改善する場合も、
    UI スレッド同期 I/O を追加せず worker 境界で検討する。
- **再現用テストデータ (生成済み)**: `C:\tmp\miv-spread-test\`
  - `case1-yoko-first-folder\` と `case1-yoko-first.zip` (01 = 横長 1800x1100、02〜11 = 縦長 900x1400)
  - `case2-yoko-second-folder\` と `case2-yoko-second.zip` (01 = 縦長、02 = 横長、03〜11 = 縦長)
  - `gen_spread_testdata.py` (生成スクリプト。枚数や横長の位置を変えて作り直せる)、`README.txt` (手順)
  - 各ページに番号が大きく描いてあるので、同じ番号が 2 画面続けて出れば再現。
  - 同ディレクトリの `01_portrait_even_8` 〜 `04_landscape_multi_12` は 2026-07 の別件の見開き
    テストデータ (今回のものとは別)。
- 優先度: P1 相当 (閲覧の基本動作が壊れて見える。ただしデータ破壊はなし)。規模: 小〜中。

## 2. 一覧 / サムネイル / フォルダ走査

### 2.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

### 2.2 サムネイルバッジのレーン配置共通化

- 背景 / 現象:
  1. ZIP / PDF / RAR 等の形式バッジだけを従来比 70% に縮小したため、従来サイズを
     維持している緑色のフォルダ名バッジが相対的に大きく見える。フォルダ名は可変長で
     セル幅の 80% まで使うため、面積差も目立つ。
  2. ブックマーク一覧では、通常セルの左上バッジ列 (編集状態 / pin / タグ) と
     ブックマーク時刻を別々に同じ左上座標から描画している。後から描く時刻がタグの
     先頭を覆い、見た目では隠れた領域がタグのクリック判定として残る可能性もある。
  3. 動画の `UP` バッジも左上を独自使用しており、タグや編集状態バッジと同じ衝突を
     起こし得る。右上のチェック / スタック枚数、左下のコンテナ形式 / フォルダ名 /
     評価、右下の絞り込み件数も、現在は個別の座標計算または相互の高さ推定で配置している。
- 原則:
  - 共通化の単位は「全バッジを同じ見た目で描く巨大関数」ではなく、セル四隅の使用領域を
    割り当てる純粋なレイアウトとする。形式、フォルダ名、時刻、評価など意味の異なる
    バッジは、共通の計測 / padding / 配置部品を使いつつ個別スタイルを維持する。
  - 中央の再生アイコンや代替アイコンなど、隅のバッジではないコンテンツ overlay は
    この仕組みに含めない。
- 実装案:
  1. セルの `inner` と表示対象を受け、`TopLeft` / `TopRight` / `BottomLeft` /
     `BottomRight` の各レーンに実測済み `BadgePlacement` (矩形、文字 / style、優先度) を返す
     `ThumbnailOverlayLayout` 相当の純粋なレイアウト層を設ける。
  2. **v2.9.1 完了**: 第1段階は左上を移行し、ブックマーク時刻 → 編集状態 / pin → タグの順に幅を予約する。
     `UP` も同じレーンへ入れ、残り幅が不足する場合だけタグを実測で省略または非表示にする。
     描画と hover / click 判定は同じ `BadgePlacement` の矩形を使い、再計算をなくす。
  3. **v2.9.1 完了**: 第2段階は左下を移行し、フォルダ名 / ZIP / PDF / 変換アーカイブ、評価、ファイル名
     プレートの予約幅・縦積みを同じレイアウト結果から決める。フォルダ名バッジは形式
     バッジと機械的に同じ 70% へ揃えず、長い名前の可読性を残したコンパクトな専用
     スタイルをスナップショット比較で決める。
  4. **未着手**: 第3段階で右上 / 右下も同じ所有境界へ移し、チェックとスタック枚数の排他条件、
     絞り込み件数の配置を明示する。各段階を独立コミット可能にし、全経路を一度に
     書き換えない。
- v2.9.1 実装記録 (2026-07-31):
  - `src/thumb_overlay_layout.rs` に painter 非依存の純粋レイアウトを新設。セルごとに 1 回だけ
    構築した左上 / 左下の配置結果を、通常描画、タグ hit-test、`UP`、ブックマーク時刻へ渡す。
  - 左下は形式 / フォルダ名とファイル名プレートの実測矩形を下段へ置き、その行の上へ評価を
    4px gap で積む。旧 `lower_left_container_badge_height` の高さ推定は撤去した。
  - フォルダ名は旧フォントの 85%、8.5pt 下限・13.5pt 上限、横 padding 0.30em・縦
    padding 0.12em の専用 style とした。長い CJK 名の可読性を残しつつ、70% 形式バッジとの
    面積差を抑える値を snapshot で確認した。
  - 純関数 unit test 5 件と、フォルダ / ZIP / PDF / RAR、ブックマーク時刻 + タグの snapshot を
    追加・更新して目視確認済み。第3段階の右上 / 右下は未着手のまま残す。
  - 左下の item 種別 → コンテナバッジ / ファイル名プレートの対応は `bottom_left_content`
    として painter 非依存に切り出した。旧 `cell_has_lower_left_container_badge` の
    unit test 4 件 (フォルダは Loaded まではファイル名、アーカイブは常に形式バッジ) を
    同じ規則の検証として移設してある。ここは「フォルダ名と ★ が重なる」報告の退行ガード。
- **第3段階と一緒に片付ける残件 (v2.9.1 では未対応)**: 左上レーンは cursor を右へ送るだけで、
  **セル幅でクランプしていない**。時刻 + `UP` + 編集バッジ 6 個が同時に立つ狭いセルでは
  レーンが右へはみ出す (タグだけは実測で省略される)。第2段階以前からある挙動だが、
  時刻を同じレーンへ入れたぶん到達しやすくなった。直すには「溢れたときどのバッジを落とすか」の
  方針が要るので、右上 / 右下を同じ所有境界へ移す第3段階と合わせて決める。
- 完了条件 / 回帰テスト:
  - ブックマーク時刻と `#` から始まるタグが重ならず、表示矩形とクリック矩形が一致する。
  - `UP` + 編集状態 / pin + タグ、狭いセル、長い CJK タグでもバッジ同士が重ならない。
  - 長いフォルダ名 + 評価、ZIP / PDF / RAR + ファイル名 + 評価で重なりやセル外描画がない。
  - レイアウト純関数で矩形の非交差・優先順位・狭幅時の省略を unit test し、代表的な
    フォルダ / 形式バッジとブックマーク時刻 + タグを `docs/ui-snapshot-policy.md` に従う
    snapshot で確認する。
- 規模 / 優先度: Medium / P2 candidate。現在の重なりだけを座標ずらしで直す症状パッチは
  入れず、まず左上レーンの単一所有化から着手する。

### 2.3 サブ展開のオプション (走査階層 / 走査時フィルタ / 相対パス表示) — 利用者要望

- 出典: 2026-08-02〜04 の利用者メール (pattier)。「追加を検討します」と回答済み。
- 使い方: UNC 共有に `ID001` / `ID002` … のフォルダが多数あり、各 ID 配下は
  `ID001/thumb/img001/*.bmp` のように深い階層へ大量の画像が入っている。目当ては
  `ID200`〜`ID300` の浅い階層にある少数のファイルだけで、全階層走査はネットワーク越しの
  コストに見合わない。
- **起点フォルダの絞り込みは既にできる**: 通常フォルダ表示でフォルダをチェックしてから
  サブ展開すると、チェックしたフォルダ以下だけを展開する。要望の 1 点目はこれで足りるため
  実装不要 (回答済み)。
- 対応状況:
  1. [x] **走査階層の上限指定**: `サブ展開` を押すと確認ダイアログを開き、
     「起点のみ / 1 / 2 / 3 / 5 / 10 階層 / 無制限」を選んで実行できるようにした。選択値は
     スカラー設定として保存して次回の初期値にし、ボタンのツールチップにも反映する。サブ展開中は
     同じボタンでダイアログを出さず即時解除する。既定の「無制限」は従来どおり実効上限 40。
  2. [x] **走査時フィルタ**: 実行前モーダルで既存の絞り込みと同じ種類 / サイズ / 更新日の
     語彙を選び、条件外の項目を snapshot へ入れない。サイズ / 更新日はファイルだけへ適用する。
     サイズは従来の 4 区分に加え、数値 + KB / MB / GB で最小 / 最大を片側または両側指定できる。
     条件は保存して次回モーダルへ復元し、展開後のフォルダバーにも表示する。条件なしは従来互換。
     ディレクトリ走査の I/O 自体は減らないが、後段のソート・一覧構築・メタデータ準備を減らす。
  3. [x] **相対パス表示**: 場所 facet、選択情報、ツールチップ、詳細表示の「場所」列を、
     サブ展開ボタンを押した実フォルダからの相対パスで表示する。複数のチェック済みフォルダを
     起点にしても基準は押下位置のままにし、直下は `(直下)`、root 外はフルパスへ戻す。
     詳細表示の場所列は任意表示・ソート可能で既定非表示。詳細表示では表を隠す行テキスト
     ツールチップを止め、列と下部情報バーへ集約する (プレビュー画像 viewport は維持)。
     `DetailsColumnId::Place` / `DetailsSortKey::Place` は `PageCount` と同じ保存用 clone の stash で
     メイン列と専用下部バーの列順・幅・ソートキーから退避し、旧版 quarantine を防ぐ。
- 規模 / 優先度: 1 / 2 / 3 とも対応済み。

### 2.4 CSV / TSV からの一括タグ / レーティング付与 — 保留

- 出典: 同じメール往復。こちらから代替案として提案し利用者も歓迎したが、**利用者の実際の
  使い方 (参照は一時的で、タグを付けても後から参照しないことが多い) とは噛み合わない**ため、
  §2.3 を優先すると回答した。
- 位置づけ: 外部ツールで抽出した結果を mIV へ持ち込む導線としては筋が良い。単独では需要が
  薄いので、タグ運用側の要望が別に出たときに合わせて再判断する。
- 実装するときの前提 (利用者へ明言済み): **明示的な「取り込み」操作のときだけ動く**こと。
  パスの一覧をビューとして開く形 (実体のない仮想フォルダ) は採らない。理由は、他人由来の
  リストで任意パスを参照してしまうこと、UNC パスなら開いた瞬間に外部へ認証情報が飛び得る
  こと、実フォルダ前提の処理 (サムネイルの識別キー、移動 / 削除時の扱い、各種設定の保存先)
  への影響範囲が大きいこと。
- 優先度: P3 / 保留。

## 3. 補正 / AI

### 3.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

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

### 3.3 フルスクリーンの idx キャッシュに一覧世代の刻印が無い (未再現・構造の穴は確認済み)

- 報告 (2026-08-08): PDF を読んでいるとき、2 ページ目に直前まで開いていた別の PDF の
  ページ (おそらく同じ 2 ページ目) が表示された。再現せず。リモート閲覧ではなく本体側。
- 調査で分かったこと:
  - `fs_cache` / `fs_early_dims` / `fs_upload_backlog` / `fs_pending` は **index だけをキー**にし、
    どの item 一覧のものかを記録していない。
  - 完了適用 ([src/app.rs](../src/app.rs) `poll_fs` 系、`for (key, mut result, load_seq) in completed`) も
    index だけで行う。一緒に運ぶ `load_seq` は perf ログの対応付け用で、照合には使っていない。
  - これらを一括で捨てるのは `close_fullscreen` / `invalidate_idx_state_and_queues` /
    `enter_drive_list` の 3 か所だけ。`invalidate_idx_state_and_queues` の呼び出し元は
    `remove_items_batch` と `replace_search_view_items` に限られる。
  - **フォルダ・コンテナ読み込みが必ず通る `install_new_items` は `items_generation` を
    進めるだけで、これらに触れていない。**
  - 読み出し側 (`fs_cache.get(&idx)` は 57 か所) にも世代照合は無い。
- つまり「index N のテクスチャが現在の一覧のものである」ことは、item を差し替える場所が
  全部クリアを覚えていることに依存していて、構造では保証されていない。フルスクリーンを
  閉じずに一覧が入れ替わる経路、または差し替え後に旧要求の完了が着地する経路があれば、
  index N に前のコンテナのページが残る。報告の症状と形が一致する。
- **どの経路が実際に漏れているかは未特定**。穴が開いていることまでしか確認していない。
- 方針: 経路を探して個別にクリアを足すのではなく、これらの entry に `items_generation` を
  刻み、読み出し・適用時に照合する。世代違いは現在の一覧のものではないと確定できるので、
  記録したうえで捨てる。欠けている識別子を足す修正であって、症状隠しではない。再現しなくても
  穴は塞がり、再発時は記録が残る。
- 実装記録 (2026-08-09): `fs_cache` / `fs_early_dims` / `fs_pending` を生 `HashMap` から
  `ItemsGenerationMap` へ、`fs_upload_backlog` を `ItemsGenerationVec` へ置換した。各 entry は
  bundle-local な `items_generation` を保持し、lookup / iteration / 世代更新と
  `fs_pending` 完了の early-dims / upload-backlog / final-cache 着地を fail-closed で照合する。
  不一致は `[fs-generation] stale entry discarded cache=... idx=... expected_generation=...
  actual_generation=...` と通常ログへ残して破棄し、pending は同時に cancel する。
  `install_new_items` 相当の差し替えと旧完了拒否を状態遷移テストで固定した。
- 退行修正 (2026-08-10、実機確認待ち): 世代更新時の即時 purge により、それまで stale entry が
  偶然埋めていた main embedded の 1 presentation frame が黒として露出した。active detached は
  `update_active_detached_viewer_context` 内で `poll_folder_nav` / apply → PDF/ZIP enumerate →
  `poll_fs_nav_lock` → `render_fullscreen_viewport` の順だった一方、main embedded は先に
  `render_fullscreen_viewport` で shape を構築し、early-return 内で folder result を apply/purge して
  いた。main も遷移結果の apply・enumerate・既存 holdover の解放判定を描画前へ移し、描画後の
  回収は同フレーム中に初めて embedded へ入った場合だけの backstop にした。世代刻印・即時 purge・
  既存 holdover は維持している。`FolderNavMode::Fullscreen` の `load_folder_nav_target` / reopen は
  画像フォルダ・ZIP・PDF の共通経路なので、PDF 固有の例外は追加していない。
- 優先度: P2。急ぎではないが、別の本のページが黙って表示され得る点で放置しない。

## 4. 入力カスタマイズ / マウス / ゲームパッド

### 4.7 タブレット PC のタッチ操作対応 (仕様確定 / 未実装)

> **v2.13.0 予定** (ナビゲータのカラー化反映 §1.53 と同時)。

- 出典: 利用者からの「タブレット PC のタッチ操作に対応しているか」という質問 (2026-08-06)。
- 正本: [docs/touch-support-plan.md](touch-support-plan.md)。調査 = ClaudeCode + Codex Sol。
- 現状の要点: winit が `WM_TOUCH` を握って `DefWindowProc` へ渡さないため、egui 側では
  マウス合成が起きず、効いているのは egui-winit の「先頭接点 = 左マウス」翻訳だけ。
  タップ / ドラッグ / 長押しメニューは動くが、**一覧のスクロール・ピンチズーム・
  リングショートカットは不可**。指を離すと `PointerGone` でホバーが消えるため、
  **既定設定ではフルスクリーンをタッチだけで閉じられない (詰み)**。
- 設計案: 3 領域タップ (左/中央/右) + 中央タップでクローム表示・固定 + anchor-fraction 方式の
  一覧スクロール + ピンチズーム。マウス挙動は変えず、タッチ由来入力にだけ適用する。
  フリック / 長押しリング / 慣性 / ピンチ回転は入れない。
- **仕様確定 (2026-08-06、未実装)**: 動画も対応必須 / 左右パネル (AI アップスケール・カラー化・
  ブックマーク・レーティング・タグ) もカバー / 選択済みセル再タップ open も入れる /
  **タッチ ON-OFF 設定は作らない** (マウス無影響で、タッチ時だけ別動作。egui-winit のイベント列
  シグネチャで入力源を相関し、肯定証拠だけで新経路へ入る fail-closed 設計) / **ペンはタッチ扱い**
  (代償はペンでのファイル D&D 不可のみ) / 診断用の強制無効 `MIV_DISABLE_TOUCH_GESTURES` は持つ /
  タップ領域は**中央矩形案** / 動画のシークは **±5 秒** / AI アップスケールとカラー化は静止画のみ対象。
- UI は「**初回オーバーレイヘルプ 1 回 + 中央タップでクローム + 端からのスワイプでパネル**」。
  常時表示の affordance は置かない。初回ヘルプは中央タップを一度使うまで再表示 (詰み防止の安全網)。
- 工数目安: **総計 22〜34 人日** (Phase 1 = 入力源分離 + 両 backend 成立確認 7〜10 /
  Phase 2 = 一覧・再タップ open・静止画パネル 7〜11 / Phase 3 = 動画 native 完成 8〜13)。
- **着手前の最大リスク**: `WM_POINTER` が presenter / HUD の実 HWND に期待どおり配送されるかが
  **未確認**。崩れると動画側の設計を引き直すので、**Phase 1 の出荷ゲートにする** (plan §6-1)。

### 4.2 音声モードから戻る Z が 1 回だけ効かなかった (未再現・報告者の追加情報待ち)

- 出典: 利用者メール (pattier)、v2.10.0 で確認。
- 経緯: v2.9.1 で毎回再現していた「音声モード中の Z で動画表示へ戻れない」は、`713d36bf`
  (キーエッジに送信元 HWND / viewport を持たせた) で解消し、v2.10.0 では戻れる。ただし
  利用者の試用中に **1 回だけ**戻れない状態が起き、その後は再現していない。
- 確定している事実 (利用者による切り分け): ツールチップに `(Z)` が出る = 割り当ては生きて
  いる。「動画表示に戻る」ボタンの表示条件は `video_audio_mode == Some(fs_idx)` なので、
  ボタンが見えて効いた時点で **キー側 exit の判定条件も成立していた**。状態ではなく入力
  経路の問題として扱う。
- 候補 (再現時に Z 以外のキーも効かないかで割れる):
  - (A) フルスクリーン viewport がフォーカスを持っていない → `handle_fs_key_input` 冒頭の
    `has_focus` 判定で全ショートカットが早期 return する。Z 以外も効かない。
  - (B) 同一フレームで Z が別の consumer に取られた → Z は画像フルスクリーンの `FsZoomMode`
    の既定でもあり、音楽ビューでの取り合いは過去にも起きて明示的に除外した経緯がある
    (`ui_fullscreen.rs` の fs_zoom ラッチ除外コメント)。Z だけ効かない。
- 依頼済み: 再現したら (a) mIV ウィンドウの余白を 1 回クリックして復帰するか (b) そのとき
  Z 以外のキー ([Space] など) も効かなかったか (c) 直前に別ウィンドウ / タスクバーを
  クリックしていないか。
- 優先度: P3 / 再現待ち。**報告が来るまで着手しない** (ユーザー判断 2026-08-02)。

### 4.1 Shift / Alt + ホイールのカスタマイズ再設計

- 背景: v1.7.0 のリングショートカット / マウスボタン実装中に、Shift / Alt + ホイールのペアバインドを
  追加候補にしたが、実機確認で動画まわりの退行リスクが高いと判断した。
- 方針:
  - v1.7.0 では公開 UI / 入力経路から外し、通常ホイール、Ctrl+ホイール、中ボタンドラッグの既存挙動を維持する。
  - 将来再開する場合は、グリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計する。
  - native video overlay の consumed wheel、modifier 転送、動画タイルの Ctrl+ホイール、編集パネル / スクロールパネルとの
    優先順位を先に決める。
- 実装メモ: `ring_shortcuts.shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用フィールドとして残すが、
  現行 UI / 入力経路からは参照しない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

### 4.8 名前の変更 / 更新コマンドの追加 (既定は割り当てなし、F2 / F3 / F5 はカスタマイズで) — 利用者要望

- 出典: 利用者報告 2026-08-07。「★のレート付けは数字キーに変更して、F2 キーは Windows と
  同じリネームの動作にしてほしい」。
- **現状の割り当て (2026-08-07 時点、`keymap.rs::default_chords` を正とする)**

  ファンクションキー:

  | キー | サムネイル一覧 | 画像フルスクリーン | 動画 / 音楽フルスクリーン |
  | --- | --- | --- | --- |
  | F1〜F5 | ★1〜★5 (アイテム) | 同左 | 同左 |
  | F6 | レーティング解除 (アイテム) | 同左 | 同左 |
  | Shift+F1〜F6 | ★1〜★5 / 解除 (コンテナ) | 同左 | 同左 |
  | F7 / F8 | 消しゴムマスク 1 / 2 を適用 | 同左 (現在ページ) | — |
  | F9 / F10 | 隠蔽マスク 1 / 2 を適用 | 同左 (現在ページ) | — |
  | Shift+F7〜F10 | 消しゴム / 隠蔽マスクを削除 | 同左 | — |
  | F11 | ウィンドウ最大化 / 復元 | ウィンドウ ⇔ 全画面 | 同左 |
  | F12 | 別ウィンドウモード切替 (Global) | 同左 | 同左 |

  最上段の数字キー (テンキーは別スロットだが既定は同じ操作を併記):

  | キー | サムネイル一覧 | 画像フルスクリーン | 動画 / 音楽フルスクリーン |
  | --- | --- | --- | --- |
  | 1〜5 | **空き** | 見開き 5 種 (単ページ / 左開き / 左開き表紙単独 / 右開き / 右開き表紙単独) | **空き** |
  | 6 | **空き** | ページ単位 / 縦連結 / 横連結 の切替 | **空き** |
  | 7 | **空き** | 横方向の読み進み方向の切替 | **空き** |
  | 8 / 9 | **空き** | **空き** | **空き** |
  | 0 | **空き** | ズーム / フィット方式の切替 | **空き** |
  | Ctrl+1〜0 | 補正プリセットスロット 1〜10 | 同左 | 動画補正スロット 1〜10 |
  | Alt+1〜0 | サムネイル列数 1〜10 | **空き** | **空き** |
  | Ctrl+Alt+1〜0 | 空き | スロットを標準設定へ読み込む | 空き |

  ほかに隠蔽加工モード中のみ `1`〜`4` = 隠蔽プリセット (別コンテキストなので競合しない)、
  `Alt+-` = サムネイル一覧 ⇔ 詳細一覧。
- **Windows (エクスプローラー) の標準**: F1 ヘルプ (Windows 11 では Web 検索が開く) /
  **F2 名前の変更** / F3 検索 / F4 アドレスバーの一覧表示 / **F5 最新の情報に更新** /
  F6 ペイン間のフォーカス移動 / F10 メニューバー / F11 全画面 / Alt+F4 閉じる。
  F12 はエクスプローラーでは未使用 (mIV の別ウィンドウ割り当てはぶつからない)。
  → **F11 は一致、F1〜F6 は 5 個ぶつかっている**。
- **足りないコマンド (= 操作カスタマイズで割り当てたくてもできない)**:
  - 名前の変更: `KeyAction` が無い。右クリックメニュー専用で、実体は
    `App::request_rename_dialog` ([src/ui_dialogs/rename_item.rs](../src/ui_dialogs/rename_item.rs))。
    単一選択かつフォルダ背景でないときだけ出る。ダイアログ自体は Win32 なので、
    ダイアログ内のキー操作は Windows 標準のまま。
  - 再読み込み: `KeyAction` が無い。実体は `App::reload_current_folder_preserving_override`
    ([src/app.rs:16118](../src/app.rs)) と、フォルダツリーペイン内の `↻` ボタン。
  - アドレスバーへフォーカス / ペイン間フォーカス移動も `KeyAction` は無い
    (ツリーペインは `F` で表示トグル、`Esc` でツリーへフォーカス)。
- **構造上の制約 (レーティングを数字へ移すときの肝)**: レーティングは
  `KeyContext::Rating` の 1 グループで、`GRID_ACTIVE_SCOPES` / `FS_IMAGE_ACTIVE_SCOPES` /
  `FS_VIDEO_ACTIVE_SCOPES` の**3 面すべてに同時に載る** ([src/keymap.rs:5532](../src/keymap.rs))。
  つまり 1 つの chord が 3 面すべてで空いていなければならず、**今すぐ空いている数字は 8 と 9 だけ**。
- **方針 (2026-08-07 利用者判断・確定)**: **既定の割り当ては一切変えない**。
  上段数字 (見開き / 読み流し / 読み方向 / フィット)、テンキーの別名、F1〜F6 のレーティングは
  現状維持。**不足しているコマンドを 3 つ追加し、既定は `none`** にして、使いたい人が
  操作カスタマイズで F2 / F3 / F5 へ割り当てる形にする。F4 (アドレスバー一覧) / F6 (ペイン移動) は
  Windows でも使用頻度が低いので入れない。
  - 既定を変えないので **非互換なし** = `version_highlights` への「重要な変更点」追記も不要。
    マニュアルとリリースノートで「エクスプローラーと同じ F2 / F3 / F5 で使いたい場合の手順」を案内する。
- **なぜレーティングを数字へ移す案を採らなかったか (再検討時の判断材料)**:
  - Rating は `KeyContext::Rating` の 1 グループで一覧 / 画像 FS / 動画 FS の **3 面すべてに
    同時に載る** ([src/keymap.rs:5532](../src/keymap.rs))。chord を面ごとに変えられないため、
    「一覧とテンキーだけレーティング、画像 FS は見開きのまま」という**部分適用ができない**。
    テンキーをレーティングにするなら、画像 FS の `Numpad0`〜`Numpad5` (見開き / フィットの別名) を
    **外すことが必須**で、テンキーで見開きを操作している利用者の操作が変わる。
  - 上段数字をレーティングにする案は、見開き変更を `Alt+数字` などへ追いやることになり、
    見開きを頻繁に変える多数派に対して改悪になる。循環キー化も同じ理由で却下。
  - テンキー**専用**にすると 2 つの穴が開く: (1) テンキーの無いキーボード (ノート PC / TKL) で
    既定のレーティング操作が消える。(2) **NumLock OFF ではテンキー数字が効かない**
    (`KeyName::matches_win32` はテンキーを `VK_NUMPAD0..9` で照合するが、NumLock OFF では
    `VK_END` / `VK_DOWN` などになる)。今はテンキーが上段の別名でしかないので表面化していない。
  - 参考: 同じ理由で **`Shift+テンキー` は既定に使えない** (NumLock ON で Shift を押すと Windows が
    一時的に NumLock OFF 扱いにするため `VK_NUMPAD*` が来ない)。将来テンキーへ何かを割り当てる
    ときも同じ制約がかかる。
- **追加する 3 コマンド (すべて既定 `none`)**:
  - **名前の変更** (`GridRename`, `Grid` 文脈): `App::request_rename_dialog` を呼ぶ。
    単一選択のみ有効 (複数チェック時は無効かトースト)。ダイアログは Win32 なので、
    ダイアログ内のキー操作は Windows 標準のまま得られる。フルスクリーン側 (`FsCommon`) は
    §1.38 の構造課題に触るので初版では入れない。
  - **現在地の絞り込み検索の 2 本目の chord 枠**: 新規アクションは不要。既存
    `GlobalLocalSearch` (既定 `Ctrl+F`) に利用者が F3 を足すだけで済む = **実装作業なし**。
    マニュアルの案内対象にだけ含める。
  - **更新** (`GridReload`, `Grid` 文脈): **現在メニューにも UI にも「現在のフォルダを
    再読み込み」は無い** (あるのはフォルダツリーペインの `↻` だけ) ので、これが初の導線になる。
    **メニューにも項目を置く** (2026-08-07 利用者判断)。キーを割り当てない利用者にも届くようにする。
- **`GridReload` の適用範囲 (2026-08-07 利用者判断: 仮想ビューでも再スキャンする)**:
  現在のビュー種別は `TopLevelGridSurface` ([src/app/top_level_grid_view.rs](../src/app/top_level_grid_view.rs))
  が既に 1 つの enum で持っているので、**この enum を router にして 1 か所で分岐する**
  (述語を各所に足さない)。各ビューには**冪等な再入場関数が既にある**ため、原則として配線作業になる。

  | ビュー (`TopLevelGridSurface`) | F5 の意味 | 呼ぶ既存経路 |
  | --- | --- | --- |
  | `Folder` (実フォルダ / ZIP / PDF / 変換アーカイブ) | 再読み込み | `reload_current_folder_preserving_override` |
  | `SmartFolder` | **再スキャン** | `open_smart_folder(id, refresh = true)` (既存の「更新」ボタンと同じ) |
  | `SubfolderExpansion` | **再スキャン** | `start_subfolder_expansion_scan_roots` + `SubfolderExpansionSnapshot.roots` |
  | `Search(Global)` (Ctrl+G) | クエリ再実行 | `spawn_global_search` |
  | `Search(Favorite)` (Ctrl+S) | クエリ再実行 | favsearch の spawn |
  | `Search(Tag)` (タグビュー) | 再クエリ | `open_tag_view_with_query` |
  | `Rating { stars }` | 再構築 | `enter_rating_view(stars)` |
  | `ReadingHistory` | 再構築 | `enter_reading_history` |
  | `Bookmarks` | 再構築 | `enter_bookmark_view` |
  | `DriveList` | 再構築 | `enter_drive_list(origin)` |
  | `Snapshot` (★固定) | **何もしない** | 凍結が機能の目的なので更新しない |

  加えてフォルダツリーペインが表示中なら `folder_pane.reload_for_active` も同時に走らせる
  (ペインの `↻` と同じ)。Ctrl+F の現在地フィルタは `Folder` 上のフィルタなので、
  再読み込み後に `execute_search` を再適用する順序だけ守れば別扱い不要。
- **`GridReload` で気をつける点 (配線以外の実作業はここ)**:
  1. **Ctrl+G は「クエリ再実行」であって「索引の作り直し」ではない**。索引が古いままなら結果は
     変わらないので、期待値のズレをマニュアルに書く (索引の更新は別系統)。
  2. **二重起動の防止**: スマートフォルダ / サブ展開は進捗ダイアログ + cancel を持つ非同期走査。
     走行中の F5 は無視するか、cancel してから再開するかを 1 つに決める (無視が安全)。
  3. **再スキャン後の選択・スクロール・チェックの扱い**: `Folder` の再読み込みは既に
     パス追跡で選択を復元する。スマートフォルダ / サブ展開は items が総入れ替えになるので、
     どこまで復元するかを決める (初版は「先頭へ」で割り切るのも可)。
  4. 実フォルダは外部変更を検知して自動再読み込みする仕組み (`check_external_folder_changes`) が
     既にあるため、F5 は「今すぐ」の明示操作という位置づけになる。
- **利用者側の手順 (F2 を名前の変更にしたい場合)**: F2 / F3 / F5 は ★2 / ★3 / ★5 が
  既定で持っているため、**先に該当レーティングの割り当てを外してから**新コマンドへ割り当てる。
  片方でも customized になっていれば競合は操作カスタマイズの「競合している割り当て」一覧に出る
  (`binding_conflicts` は既定同士では警告を出さないので、**両方に F2 を残した状態で出荷しては
  いけない** = 新コマンドの既定を `none` にする理由でもある)。
  - 改善余地: 競合一覧から「もう一方の割り当てを外す」導線が今は無く、各コマンドの編集画面へ
    飛んで手で消す必要がある。小さな UX 改善として別途検討してよい。
- **着手前の確認**: 素の数字キー / テンキーが keymap 以外の固定入力 (一覧の先頭文字ジャンプ等) に
  使われていないこと。現状は見当たらないが、`consume_key` の直接呼び出しを一度洗う。
- 優先度: P2 (要望)。規模: 小 (新規アクション 2 本 + メニュー項目 + ドキュメント。既定変更なし)。
  `KeyAction` を足すときは `ini_name` / `context` / `trigger` / `default_chords` / `ALL_ACTIONS` /
  呼び出し側 helper / `docs/keymap.ini.default` / `docs/keymap-spec.md` を揃える。
- **実装記録 (2026-08-10)**: `GridRename` / `GridReload` を Grid 文脈・既定 `none` で追加し、
  既存の既定 chord は変更していない。更新メニューをファイルメニューへ追加した。
  `reload_top_level_grid` の `TopLevelGridSurface` match 1 か所から各再入場経路へ配線し、フォルダ
  ツリーペインと Ctrl+F の再適用も同じ入口で扱う。スマートフォルダ / サブ展開の走査中は無視し、
  通常フォルダは読込後選択 hint で同じ項目へ戻し、スマートフォルダ / サブ展開は先頭へ戻す。
  F2 / F3 / F5 の任意設定手順と、Ctrl+G の更新が索引再構築ではないことをマニュアルへ記載した。

### 4.9 削除確認ダイアログを矢印キーで選べるようにする — 利用者要望

- 出典: 利用者報告 2026-08-07。「Delete キーで表示される削除確認のウィンドウも、標準
  エクスプローラーと同じように矢印キーで選択可能にしてほしい」。
- 現状: [src/ui_dialogs/context_menu.rs](../src/ui_dialogs/context_menu.rs) の
  `show_delete_confirm_modal` は `egui::Modal` +「削除[Y]」「キャンセル[N]」の 2 ボタン。
  キー入力は `consume_delete_confirm_action` の **Y / N / Esc だけ**で、矢印による選択も
  Enter による決定も無い。Tab は `egui_focus_policy` がアプリ全体で traversal を止めている
  ため、キーボードでフォーカスを動かす手段が無い状態。
- 実現可能性: **容易**。既存構造がそのまま使える。
  - 判定は純関数 `resolve_delete_confirm_action` に分離済みなので、選択位置を引数に足せば
    純関数の単体テストで固定できる。
  - 背面へのキー漏れは `show_delete_confirm` が `common_modal_dialog_open` に入っているため
    既存ゲートで足りる。矢印 / Enter も同じ `consume_delete_confirm_action` の中で consume する。
  - Enter は CLAUDE.md の IME 定型どおり `dialog_enter_pressed` を使う (変換確定の Enter を
    奪わない)。矢印はモーダル中だけの固定入力なので keymap 対象外 =
    [docs/keymap-spec.md](keymap-spec.md) に固定理由を追記する。
  - 選択中ボタンの見た目は `response.request_focus()` で egui のフォーカス枠を出せる。
    Tab 抑止ポリシーは focus 移動 API 自体を止めていないので影響しない。
- 決めてもらう点:
  1. 初期フォーカス。エクスプローラーのごみ箱確認は「はい」が既定。mIV の確認は
     `DeleteConfirmKind::RecycleBin` と `MayPermanent` の 2 種類があるので、**ごみ箱行きは
     「削除」、完全削除の可能性がある場合は「キャンセル」**に置く案を推奨。
  2. 左右だけ受けるか、上下も同義で受けるか (ボタンは横並び。上下も受けると迷いにくい)。
  3. 同型の 2 ボタン確認モーダル (回転情報リセット / TensorRT パック削除 / 編集用追加ファイル
     削除 / 音量ノーマライズ測定値削除など) へ広げるか。広げるなら個別にキー処理を書き足さず、
     確認モーダル共通 helper に集約する。
- 採らない案: Win32 の TaskDialog / MessageBox へ寄せると Windows 標準のキー操作は無料で
  付くが、削除対象リストのスクロール表示ができず、`App::update` の内側でモーダルを回す構造
  (§1.38) を増やすことになる。
- 関連: §4.8 の「F2 名前の変更」。名前の変更ダイアログはすでに Win32 なので、そちらの
  ダイアログ内キー操作は元から Windows 標準どおり。
- 優先度: P2 (要望)。規模: 小。
- **実装記録 (2026-08-10)**: 削除確認だけに選択位置を追加した。ごみ箱へ移す確認は「削除」、
  完全削除の可能性がある確認は「キャンセル」を初期選択にし、左右と上下の矢印を同義で受ける。
  Enter は `dialog_enter_pressed` の結果で選択中ボタンを実行し、矢印 / Enter は Y / N / Esc と同じ
  `consume_delete_confirm_action` で消費する。選択中ボタンは `request_focus()` で表示し、他の
  2 ボタン確認モーダルには広げていない。初期選択、方向移動、Enter、IME guard、背面への入力漏れを
  純関数 / input consumption テストで固定した。

## 5. リリース前確認 / 依存更新

### 5.0 対応済み: v2.11.0 で見送った依存更新 (2026-08-04、v2.12.0 サイクル冒頭で実施)

v2.11.0 (2026-08-04) の Phase 2 で新版を確認したが検証時間を優先して見送っていた分を、
v2.12.0 サイクルの最初の作業として取り込んだ。

| 依存 | v2.11.0 出荷時 | v2.12.0 で採用 |
| --- | --- | --- |
| PDFium | BUILD=7961 (MAJOR=152) | **BUILD=7988 (MAJOR=153)** |
| FFmpeg | n7.1.5-10-g2aefd64d48 | **n7.1.5-12-g1fdbca85aa** |

同時に実施したこと:

- `vendor/ffmpeg/VERSION` と 6 DLL の `ProductVersion` の一致を確認
  (`n7.1.5-12-g1fdbca85aa-20260803`)。版確認は **DLL の ProductVersion を正**とする
- 対応ソース `htdocs/mimageviewer/ffmpeg-n7.1.5-12-g1fdbca85aa-source.tar.gz` を取得。
  展開ディレクトリ名の commit hash と `RELEASE`=7.1.5 を検証し `.sha256` を併置
- `docs/ffmpeg-lgpl-current-report.txt` を再生成。`--enable-lib*` の増減は無かったので
  [ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md) の
  外部ライブラリ表は据え置き
- **副産物: 製品ページの FFmpeg 節が 2 版ぶん腐っていたのを修正した。**
  `htdocs/mimageviewer/index.html` は `n7.1.5-2-g998de74adf` のまま止まっており、しかも
  commit 厳密でない release tarball を指していた。アプリ内表記は `build.rs` が
  `vendor/ffmpeg/VERSION` から焼き込むので追随していたが、サイト側は手書きで追随しない。
  再発防止として CLAUDE.md のリリース手順に更新項目を明記した

**旧 vendor の退避先**: `C:\home\mimageviewer_vendor_backup\rollback-v2.11.0\`
(v2.11.0 出荷時の `pdfium` / `ffmpeg` をそのまま複製)。実機確認で問題が出たらここへ戻す。
`setup-pdfium.sh` は常に最新を取るのでバージョン指定の再取得手段が無い。

**実機確認済み (2026-08-05)**: PDFium MAJOR 152 → 153 で、通常 PDF の表示 / ページ数 /
ページ送り、パスワード付き PDF の解錠と保存済みパスワードの復元、FFmpeg 更新後の
動画 / 音声再生をいずれも確認した。rollback は不要。

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |
| PDFium | v2.12.0 サイクルで `chromium/7988` へ更新済み (§5.0) | 更新後は通常 / パスワード付き PDF の表示とページ数を実機確認 |
| FFmpeg | v2.12.0 サイクルで `n7.1.5-12-g1fdbca85aa` へ更新済み (§5.0) | DLL・VERSION・LGPL 対応ソース・製品ページの FFmpeg 節を同じ commit に揃えて動画 / 音声を実機確認 |

### 5.2 Rust クレート

- 互換範囲の一括更新とは分け、次のメジャー更新 / rc 脱出は個別判断する:
  - `ort`
  - `pdfium-render`
  - `ffmpeg-the-third`
  - `image`
  - `zip`
  - `sevenz-rust2`
  - `delharc`
  - `unrar`
  - `turbojpeg`
- 更新後に確認するもの:
  - `cargo test`
  - 検索 bench 回帰
  - perf smoke
  - `dumpbin /dependents` で不要な VC runtime DLL が復活していないこと

### 5.3 リリース / テストスクリプトの並行実行耐性とクリーン環境再現性

- 出典: v2.8.0 リリース前確認 (2026-07-26)。別セッションで
  `scripts/build-release.ps1` を実行中に通常並列の `scripts/test-full.ps1` を実行すると、
  `mimageviewer` のライブラリテストが assertion / panic を出さず `0xffffffff` で終了した。
  リリース処理の完了後、同じコミットを競合プロセスなしで再実行すると正式ゲートは
  `[test-full] PASS` で完走したため、製品テストの並列不具合ではなくリリースツール間の
  干渉と判定。v2.8.0 の出荷は止めず、次版でツールを堅牢化する。
- **P2: `build-release.ps1` の停止対象が広すぎる (v2.9.1 対応済み)**:
  - 原因はプロセス名の前方一致 `mimageviewer*`。Cargo のテストハーネスは
    `target\debug\deps\mimageviewer-<hash>.exe` で、**リポジトリ配下にあるため path 判定も
    通過**し、別セッションの `cargo test` ごと `Stop-Process -Force` していた。
  - 停止対象を `mimageviewer` / `mimageviewer-core` / `mimageviewer-vst3-host` /
    `mimageviewer-susie32` の完全名 allowlist に変更した。`build-portable.ps1` も同じ
    欠陥を持っていたので揃えた (`build-dev.ps1` は元から完全名指定で影響なし)。
  - path を読めないプロセス (昇格 / 保護) は従来どおり停止するが、allowlist を通った
    ものだけになったのでテストハーネスは対象外。
  - リポジトリ単位の test / release 相互排他ロックは**入れていない**。名前の取り違えが
    原因だったので、まず allowlist だけで様子を見る。再発したらロックを検討する。
- **P2: クリーン環境の正式テストゲートが release core に依存する**:
  - `scripts/test-full.ps1` の workspace test には launcher が含まれるが、launcher の
    build script は既存の `target\release\mimageviewer-core.exe` を要求する。このため
    release core がないクリーン checkout ではテストゲートを開始できず、既存成果物が
    ある開発環境でだけ成功する。
  - 対応案 = launcher のテストを正式ゲート内で別段に分けて必要な core を明示的に用意
    するか、launcher のテストビルドを埋め込み用 release core から分離する。
  - 完了条件 = `target\release\mimageviewer-core.exe` が存在しないクリーン環境から
    `scripts/test-full.ps1` が完走し、launcher のテストも省略されないこと。
- 規模 / 優先度: Small〜Medium / P2。いずれも製品 runtime の品質問題ではなく、
  同一 worktree で複数セッションを使うリリース運用とクリーン再現性の改善。

### 5.4 idle health の video-pin evidence 窓が狭い

- 出典: v2.9.1 リリース前確認 (2026-08-01)。`-TargetKey` 方式に変えた直後の実測で FAIL。
  原因は**ゲートの窓**で、製品側でもセットアップ手順の誤りでもない。
- 何が起きたか: 対象キーのサムネイル処理は **t≈177-182 に 519 件**あり、そのままアプリが
  就寝して測定区間へ入った。ところが evidence 窓は「Enter プロンプト表示〜測定終了」
  (t≈199.7-219.8) なので、**20 秒前に完了していた keep 範囲入りが窓の外**になり
  `matched=0` で FAIL した。`-NoLaunch` で 3 シナリオを連続実行すると必ずこうなる。
- ずれている前提: ゲートは「evidence 窓の**中で** keep 範囲へ入ったこと」を要求するが、
  シナリオが必要とするのは「測定中にタイルが keep 範囲に**ある**こと」。先に入って
  居残っているタイルも等しく正しいセットアップ。
- 直す方向 (どちらか):
  1. 窓をセッション全体へ広げ、**最後の `nav.load_folder_begin` より後**に対象キーの
     work があることを条件にする (= 今表示している場所のタイルだと言える)。
  2. 窓は現状のまま、「セッション内に対象キーの work はあるが evidence 窓の外」を
     FAIL ではなく warning にし、手順書で「連続実行時は開き直す」を明示する。
- 1 のほうが構造的 (操作者の手順に依存しない)。着手時は
  [idle-health-check.md](idle-health-check.md) の §3 も揃えること。
- 規模 / 優先度: Small / P2。**毎リリース必須のチェックが手順どおりでも落ちる**状態なので、
  次版で片付ける。
- **v2.10.0 (2026-08-02) は 3 シナリオとも PASS**。waiver 不要。今回窓に当たらなかったのは、
  **Enter を押す直前に対象フォルダを開き直した**ため。証拠は
  `matched=170 (enqueue=23, idle_upgrade_enqueue=2, idle_upgrade_ineligible=2, ready=143)` で、
  アイドル高画質化パスが対象タイルを実際に評価したうえで測定区間は完全 sleep
  (perf event 0 件 / perf log 増加 0 バイト / CPU 1.2%)。
  つまり**現状でも手順を「準備 → 即 Enter」に揃えれば通る**。ただしそれは操作者の手際に
  依存する状態であり、上記の対応方針 1 (窓をセッション全体へ広げる) は引き続き有効。
  なお `-TargetKey` が checklist から抜けていた件は `31b45449` で別途修正済み。
- v2.9.1 の waiver 根拠 (2026-08-01 更新)。`static-foreground` / `static-background` の PASS に加え、
  この版の変更が `video-pin-background` が見ている経路 (アイドル高画質化 / 動画ピンのタイル保持)
  に触れていないこと。対象の変更は次のとおり:
  - バッジレイアウト、rename 移行、ネイティブ名前ダイアログ
  - **トレイ常駐中の再生継続** — hidden 中の `App::update` を 50ms で起こす経路を新設した。
    アイドル高画質化とは別の wake 源だが、**静止時の消費は未実測**。次版で
    `tray-resident` シナリオを足すこと (今回は前 2 シナリオが常駐なしの静止を見ている)。
  - **スマートフォルダ セッション** — parked grid が keep 範囲分のサムネイルを保持する。
  - **見開きの表示可否判定 (`FsPageLoadState`)** — 再入ループを止めた側なので、アイドル時の
    work はむしろ減る (`9590b661`)。
  - **入力所有権 (raw key permit / IME の viewport 分離)** と **native window health** —
    後者は native video window が生きている間だけ 1 秒に 1 回 pump へ ping を送る。
    無再生時は送らないが、**動画を開いたまま放置したときのアイドル影響は未実測**。

### 5.6 v2.12.0 リリース前確認の記録 (2026-08-06)

§5.5 の「記録が残らない」を繰り返さないための実測記録。

- **依存**: PDFium `chromium/7988` / FFmpeg `n7.1.5-12-g1fdbca85aa` とも最新。DLL の
  ProductVersion `n7.1.5-12-g1fdbca85aa-20260803` が `vendor/ffmpeg/VERSION` と一致。
  PDFium MAJOR 152 → 153 の実機確認 (通常 / パスワード付き PDF、動画 / 音声) 済み。
- **CI**: master / main とも success (run 31021319941 / 31021332945)。ubuntu の
  `cargo check` を含む。今サイクルの 32 commit が初めて CI を通った。
- **idle health**: `static-foreground` / `static-background` とも PASS。CPU one-core ratio
  0.0073 / 0.0083、測定窓は perf event 0 件 (完全 sleep)。`video-pin-background` は
  条件を満たすフォルダを用意しなかったため未実施。アイドル高画質化経路は今サイクルで
  触っていないので waiver とする。
- **perf smoke**: frame 7968、16ms 超のギャップ 1376 件。**件数ではなく直前の
  `ui.tail_repaint.action` で判定**した結果:
  - 16-100ms 帯 1323 件のうち **1259 件が `none`** (repaint 未要求 = 入力待ちで就寝)。
    p50 85ms は Ctrl+↓ 連打時のキー間隔そのもの
  - 100ms 超 53 件のうち 42 件が `none`、7 件が `request_repaint_after_idle_upgrade`
    (予定どおりの起床)。最大の 33.7s / 30.3s / 20.1s / 19.6s は idle health の測定窓
  - **`request_repaint` が立っていた 100ms 超は 3 件のみ**: 起動直後 356ms、
    PDF を含むフォルダ読み込み 585ms + 106ms (`load_folder_end=55ms` +
    thumb decode + `pdf/pool_promote_visible`)。今サイクルの新経路とは無関係
  - ⚠ 同一フォルダでの v2.11.0 基準値は未取得なので「悪化していない」とは言えない。
    次サイクルで PDF フォルダの folder-load ギャップを基準値化するか判断する
- **検索 bench**: 全文索引に触れていないため未実施 (ファイル名フィルターは一覧の絞り込み)。

### 5.5 v2.9.1 の perf smoke が最終 tree で未取得

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。本セッションで確認済み。
- 何が起きたか: perf smoke の記録は 2026-07-31 の tree のものしか無く (`17231956` は
  CLAUDE.md の判定基準を書き足した commit で、計測そのものではない)、**8/1 に入った構造変更の
  後に取り直した記録が見当たらない**。対象の変更は入力所有権 (`56db11bc` / `596bb65d`)、
  `FsPageLoadState` (`9590b661`)、health watchdog (`2c11e4cd`)。
- ずれている扱い: idle-health 側は §5.4 の waiver を「未実測部分を明示する」形に書き直したのに、
  perf smoke 側は同じ扱いになっていない。CLAUDE.md Phase 2 の 9.6 は「UI 周り / I/O 経路に
  変更を入れたリリースで実施」なので `FsPageLoadState` はまさに対象。
- 対応 (どちらか):
  1. v2.9.1 の tree で perf smoke を取り直す。**PDF フルスクリーンでの計測を含める**
     (§1.39-2 の毎フレーム経路がそこにしか出ない)。
  2. 取らない判断を §5.4 と同じ形で明文化する (何が未実測かを書く)。
- 規模 / 優先度: Small / P2 (プロセス)。次版のリリース前確認までに片付ける。

---

## 6. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| サブフォルダ展開 / フラット仮想ビュー | `docs/subfolder-expansion-view-plan.md`, `docs/ui-responsiveness.md`, `docs/async-architecture.md`, `docs/details-view-and-filter-plan.md`, `docs/virtual-folders.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md`, `docs/operation-customize-share-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| 詳細表示 / スマートフィルタ | `docs/details-view-and-filter-plan.md`, `CLAUDE.md` の UI / スクロール節 |
| タグ / フルスクリーン右パネル / 動画 overlay | `docs/tag-catalog-redesign-plan.md`, `docs/display-pipeline.md`, `docs/video-architecture.md`, `docs/detached-viewer-implementation-plan.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
