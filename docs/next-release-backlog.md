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

### 1.43 Enter でフルスクリーンを開くと全画面ズームモードに入る (Enter 系 KeyHold の所有権が開幕フレームで抜ける)

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
- 対応案: `is_frame_active` でない viewport では Enter 系 KeyHold を **false にする**
  (送信元を確認できないなら押下扱いしない) のが所有権の筋。`GetAsyncKeyState`
  フォールバックは per-HWND ラッチの外に残った旧経路。
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

### 1.46 静止画の拡大品質を上げる (bilinear をやめる) — 実施決定

**2026-08-04 に実施を決定した項目。**「やるか」ではなく「どのアルゴリズムにするか」を
決める段階から始める。動画の拡大は §1.47 へ分離した (難易度が別物のため)。

**現状の問題**: 拡大は**素の bilinear**。[gpu_lanczos.rs](../src/gpu_lanczos.rs) の
`GpuLanczosCache::resolve` が `DownscaleLanczos` 以外を素通しし、テクスチャは
`DISPLAY_IMAGE_TEXTURE_OPTIONS` (LINEAR) で貼られるだけ。2〜4 倍ズームでの劣化が大きい。
AI アップスケール (数秒かかる) と bilinear (即時・低品質) の中間が無く、
「AI は少し重い」利用者の受け皿が存在しない。

**方針**: 設定を増やさず**固定で置き換える**。bilinear を選びたい利用者は想定しない。
「補間なし」を明示したい場合はポストフィルタの「ニアレスト（補間なし）」が既にあり、
`needs_nearest_sampler()` で NEAREST サンプラーに切り替わる ([app.rs](../src/app.rs) 4 箇所)
ので、逃げ道は用意済み。ドット絵・整数倍ズーム用途はこちらで足りる。

**アルゴリズム選定 (着手時の最初の作業)**。Jinc で決め打ちしない。系統別の候補:

| 系統 | 候補 | 位置づけ |
| --- | --- | --- |
| 固定カーネル | **EWA Jinc3**、Catmull-Rom、Mitchell、Lanczos3 | 学習なし。Jinc は円形通過域で斜めエッジの階段が出ない |
| エッジ方向適応 | **FSR 1.0 (EASU)**、NVIDIA Image Scaling | どちらも MIT。4K でも 0.2ms 級。エッジ適応 + シャープ化 |
| 学習済みカーネル / 軽量 NN | **Anime4K**、ravu (RAISR 系)、FSRCNNX | **題材 (漫画・線画・イラスト) との一致度が最も高い**。推論ではなくシェーダなので軽い |

- **Anime4K が最有力候補**。用途が漫画・線画に完全に一致する。Jinc は線の方向を見ないので、
  斜め線の階段は減っても線の再構成まではしない
- ⚠ **エッジ適応系・学習済み系は未実測。** 根拠は一般的評価だけ。
  **Lanczos4 の失敗 (0 度トーンだけ見て「上位互換」と誤判定) を繰り返さないこと。**
  実画像 (漫画スキャン・AI 生成イラスト・写真) で比較シートを作り、利用者判断を仰ぐ
- ライセンスを必ず確認する。mIV は MIT。FSR1 / NIS は MIT だが Anime4K / ravu は要確認

**実装規模 (EWA Jinc の場合の実測見積もり)**: Medium。今回の Lanczos 統合の 4〜6 割。

| 項目 | 規模 | 備考 |
| --- | ---: | --- |
| WGSL シェーダ新規 | ~120 行 | **1 パス**で済む (非分離なので縦横 2 パスが不要)。現行の分離シェーダより単純 |
| J1 の LUT 生成 | ~40 行 | WGSL にベッセル関数が無い。CPU で 1D LUT (1024 要素) を作って渡す (madVR / mpv と同じ) |
| Plan / パイプライン | ~150 行 | `LanczosPlan::new` が `target > source` を `Err` で弾いている。拡大用 plan と fetch 見積もりが要る |
| キャッシュ分岐 | ~30 行 | `resolve` の分岐 1 本。cache / lease / resource の枠組みはそのまま使える |
| VRAM 会計 | ~50 行 | 縮小は出力が必ず小さいので現行会計は安全側。**拡大は出力が元より大きく前提が反転する** |
| テスト | ~150 行 | 分岐 / plan / シェーダ検証 |

**行数より重いリスク (ここが採否の分かれ目)**:

1. **ズーム中の再生成コスト。** 目標サイズは 1px 単位で厳密 (量子化は品質のため見送り済み、
   §4.3.3) なので連続ズーム中は毎フレーム作り直す。**拡大は出力が元の 4〜16 倍になり得る**。
   必ず実測する
2. **ドットバイドットを壊さないこと。** `OriginalOneToOne` は素通しのまま維持する
3. 上限クランプ。拡大の出力サイズは上に開いている (8 倍ズーム × 4000px = 32000px)

**コスト計算の注意**: 拡大はカーネルを `1/scale` へ広げないので支持半径 3.238 固定
(ズーム倍率に依らない)。有効タップ約 33、シェーダ実装で約 49〜81 フェッチ。
縮小の約 196 タップとは別物で、§1.45 の「6.5 倍」を拡大へ適用しない。しかも結果は
`GpuLanczosCache` に乗るので、静止時は毎フレームではなく倍率変更時の 1 回だけ。

- 正本: [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) §4.3.4 (Jinc の測定)
- 規模 / 優先度: Medium / **P2 (実施決定)**。

### 1.47 動画の拡大に高品質リサンプルを入れる — コストではなく構造の問題

§1.46 の動画版。**GPU 性能ではなく動画表示の構造が障害**なので、別案件として扱う。

- native presenter は **swap chain を動画解像度で作り**、フレームを `CopySubresourceRegion`
  で 1:1 コピーし、**`IDCompositionVisual::SetTransform2` で拡大している**
  ([render_core.rs](../src/video/native_presenter/render_core.rs) の
  `compute_video_visual_transform`)。つまり**拡大しているのは mIV ではなく DWM / DComp**。
  mIV のシェーダは通っていない (色補正が identity でないときだけ grade シェーダが走るが、
  それも動画解像度で動く)
- 入れるには ①swap chain をウィンドウ解像度にする ②grade シェーダを常時通す
  ③visual transform を SAR 補正だけにする、の 3 点が要る。keyed mutex / swap chain 世代 /
  MPO cover / DComp commit のタイミングという**最も壊れやすい箇所**に触る
- VSR は [video-architecture.md](video-architecture.md) で**スコープ外と決定済み**なので、
  先例として使えるものが無い
- 毎フレーム走るので実測必須。4K 出力で概算 1〜2ms (RTX 4090) / 3〜6ms (ミドルレンジ)。
  60fps の予算 16.7ms に対し 6〜36%。デコードと競合する。静止画と違いキャッシュが効かない
- 規模 / 優先度: Large / P3。単独リリースで実機検証を厚く取る規模。**§1.46 の後**。

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

### 2.3 サブ展開のオプション (走査階層の上限 / 相対パス表示) — 利用者要望

- 出典: 2026-08-02〜04 の利用者メール (pattier)。「追加を検討します」と回答済み。
- 使い方: UNC 共有に `ID001` / `ID002` … のフォルダが多数あり、各 ID 配下は
  `ID001/thumb/img001/*.bmp` のように深い階層へ大量の画像が入っている。目当ては
  `ID200`〜`ID300` の浅い階層にある少数のファイルだけで、全階層走査はネットワーク越しの
  コストに見合わない。
- **起点フォルダの絞り込みは既にできる**: 通常フォルダ表示でフォルダをチェックしてから
  サブ展開すると、チェックしたフォルダ以下だけを展開する。要望の 1 点目はこれで足りるため
  実装不要 (回答済み)。
- 残る 2 点:
  1. **走査階層の上限指定**: `recursive_snapshot_scan` は既に `max_depth` を引数で持つので
     走査側の変更は小さい。決めるのは UI の置き場所 (サブ展開ボタンの右クリック / 展開前の
     確認 / 環境設定) と、既定は現状維持にすること。走査量そのものが減るので、低速共有での
     待ち時間と `GlobalIoSemaphore` の占有にも効く。
  2. **相対パス表示**: 同名ファイルが並ぶため、どの起点フォルダ配下かを一覧で判別したい。
     詳細表示に「場所」列を足す案と、サムネイルのファイル名プレートを相対パスにする案がある。
     ⚠ `DetailsColumnId` に variant を足す場合は、新しい設定 DB を古いバイナリが読んだときに
     unknown variant を壊れた設定として扱う経路がある (2026-07-19 の quarantine 事故) ので、
     設定側の後方互換を先に確認する。
- 規模 / 優先度: 1 = Small〜Medium / **P2** (要望の主目的である走査負荷はこれだけで解消する)。
  2 = Medium / P3。

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

## 4. 入力カスタマイズ / マウス / ゲームパッド

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

## 5. リリース前確認 / 依存更新

### 5.0 v2.11.0 で見送った依存更新 — 次サイクル冒頭で実施

v2.11.0 (2026-08-04) の Phase 2 で新版を確認したが、**検証時間を優先して見送った**。
次のリリースサイクルの**最初の作業**として片付ける。出荷直前に上げない。

| 依存 | v2.11.0 出荷時 | 確認できた新版 |
| --- | --- | --- |
| PDFium | BUILD=7961 | chromium/7988 |
| FFmpeg | n7.1.5-10-g2aefd64d48 | n7.1.5-12-g1fdbca85aa |

見送りの理由: v2.11.0 は表示パイプラインの修正で PDF レンダリングにも動画デコードにも
触れていない。PDFium を上げれば PDF 表示の再確認が要り、FFmpeg を上げれば LGPL 対応ソースの
差し替えも伴う ([ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md))。

FFmpeg の版確認は **DLL の ProductVersion を正**とする。`vendor/ffmpeg/VERSION` は
ローリング名を掴んで腐ることがある。

### 5.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |
| PDFium | v2.7.0 は `chromium/7947` を維持。`chromium/7961` への更新は次版候補 | 更新後は通常 / パスワード付き PDF の表示とページ数を実機確認 |
| FFmpeg | v2.7.0 は `n7.1.5-2-g998de74adf` を維持。`n7.1.5-9-gb9a218bc1e` への更新は次版候補 | DLL・VERSION・LGPL 対応ソースを同じ commit に揃えて動画 / 音声を実機確認 |

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
