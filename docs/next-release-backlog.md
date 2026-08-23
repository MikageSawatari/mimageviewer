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

### 1.31 wndproc の内側で GPU 待ちのある描画をする構造

- 出典: §1.30 の解析 (2026-08-01、Codex Sol の独立レビュー)。**v2.9.1 の範囲外**。
- **進捗 (2026-08-16)**: **§1.31-A は完了、§1.31-B は未着手**。A で vendored eframe に
  per-window / viewport / surface-generation 単位の typed scheduler と about_to_wait 外側
  paint drain を新設し、通常 RedrawRequested、bootstrap、AccessKit、不可視窓の要求済み work が
  message dispatch 中に renderer へ入る経路を除去した。inline 例外は event provenance が
  非ゼロ Resized の InteractiveResize frame と、その親 frame が作る immediate viewport render
  subtree だけ。reducer 11契約、既存 hidden throttle 3件、別 thread の SendMessageTimeoutW が
  RedrawWindow で同期 damage を発生させる Windows process test を gate 化した。
  **A が除去したのは「同期メッセージ自身が GPU 待ちを開始する経路」だけ**であり、外側 paint 中の
  GPU 待ちに後着の同期メッセージが巻き込まれる問題は B のまま残る。
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
- **v3.0.0 の判断 (2026-08-13)**: **構造修正は v3.0.0 をブロックしない**。
  - 根拠: この問題が実際に牙を剥いた唯一の事例は §1.30 で、それは **VST エディタの開閉が
    窓の owner / z-order を動かす**ことが引き金だった。v3.0.0 の Web 配信は、HTTP サーバと
    トランスコードを**UI スレッド外**で回すもので、**窓の所有関係にも同期メッセージにも
    手を出さない**。§1.31 の露出面を新たに増やさない。
  - その §1.30 も revert 後は再現していない (上記のとおり保留)。
- **観測の穴は解消済み (v3.0.0 前)**。`ui_heartbeat_should_stay_active_while_hidden`
  ([app.rs](../src/app.rs)) が keep-alive の述語に **mounted media session** (フルスクリーンの
  動画 / 音声、動画→音声モード、`FsCacheEntry::Video`) を含めるようになり、ネイティブ動画
  フルスクリーン中に main HWND が隠れても watchdog が armed のままになる。以下は当時の記録。
- ~~ただし観測の穴だけは v3.0.0 の前に埋める (Small)~~。§1.30 のとき
  `ui-heartbeat-watchdog` は生きていたのに `panic.log` へ何も残らなかった。
  [app.rs:62646 付近](../src/app.rs:62646) を見ると、main HWND が不可視になった時点で
  watchdog を suspend し、**例外は `viewer_session_is_detached_or_switching()` だけ**。
  ネイティブ動画フルスクリーン中は main HWND が隠れるため、**最も危ない操作の最中に
  watchdog が黙る**。keep-alive の述語にアクティブなメディアセッションを含める。
  これだけ入れておけば、次に野生で固まったときにログが残る。
- message-dispatch / render 位相分離は §1.31-A として完了。nonblocking acquire / Present と
  message service latency の上限制約は §1.31-B として後続する。
- **着手順の確定 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  `§1.85-A → §1.86 → §1.31 本体` の 3 段。根拠と逆順の失敗モードは
  [codex-texture-delivery-test-brief.md](briefs/codex-texture-delivery-test-brief.md) §0。
  §1.31 は frame drop を通常経路にするので、その経路の delta 配送を先に契約化する。
  **§1.86 は障害影響としては P3 のままだが、本項の依存関係上は必須前提**として扱う。
- **前提充足 (2026-08-16)**: §1.86 の単一 delta transaction owner と non-submit outcome の
  finalization 契約が完成し、その上で §1.31-A の位相分離まで完了。通常 frame drop と readiness
  wake を足す §1.31-B は未着手。
- **本体を A / B に分割 (2026-08-16、ClaudeCode / Codex Sol 合意)**:
  - **§1.31-A**: message-dispatch 位相と render 位相の分離。**同期メッセージ自身が GPU 待ちを
    開始する経路**を除去する。vendored eframe だけで完結する (winit / wgpu は vendor しない)。
    ブリーフ = [codex-render-phase-separation-brief.md](briefs/codex-render-phase-separation-brief.md)。
  - **§1.31-B**: presentation と message service latency の上限制約。
    **A を merge しても §1.31 は完了しない**。UI スレッドが外側の paint で GPU を待つ間も
    message pump は止まるため、他スレッドの `SendMessage` は依然ブロックする。A が消すのは
    「同期メッセージが GPU 待ちを**開始させた**」経路だけ。
  - **B の実現可能性は確認済み (2026-08-17、コードで実地確認)**。acquire 側は
    **wgpu の vendor 不要**で組める:
    - `Dx12UseFrameLatencyWaitableObject` の既定は **`Wait`**
      ([wgpu-types instance.rs](https://docs.rs/wgpu-types/27.0.1/))。つまり現状の
      `acquire_texture` は frame-latency waitable object を待ち、wgpu-core の
      `FRAME_TIMEOUT_MS = 1000` で打ち切られても**その結果を捨てて先へ進む**。
    - `DontWait` は doc に「**This is useful if the application wants to wait for the
      waitable object itself**」と明記されており、まさに B の用途。swapchain は
      waitable flag 付きで作られ、handle は取れるが wgpu は待たない。
    - handle への到達経路も public: `wgpu::Surface::as_hal::<Dx12>()` (surface.rs) →
      `wgpu_hal::dx12::Surface::waitable_handle()`。
    - → 設計: 起動時に `DontWait` を設定し、paint 前に `WaitForSingleObject(handle, 0)` で
      非ブロッキング判定。未 signal なら damage を保持したまま `DeferredNotReady` として
      その frame を捨て、signal されたら wake する。**時間窓ではなく OS の事実で判定する**
      ので憲法 5 に適合する。
    - 注意: DX12 専用。backend が DX12 でない場合 `as_hal` は None を返すので、その場合は
      現行挙動へフォールバックする (新しい失敗経路を作らない)。
    - 参考: mIV は既に `MIV_WGPU_FRAME_LATENCY` で `desired_maximum_frame_latency` を
      設定しており ([lib.rs](../src/lib.rs))、native presenter は自前 swapchain で
      `DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT` を既に使っている
      ([render_core.rs](../src/video/native_presenter/render_core.rs))。機構は既知。
  - **Present 側は別問題で、これは vendor 不要では閉じない**。`SurfaceTexture::present()` は
    戻り値が `()` で、HAL の FIFO Present は interval 1 かつ `DXGI_PRESENT_DO_NOT_WAIT` 無し
    (`DO_NOT_WAIT` は blocked 時に `DXGI_ERROR_WAS_STILL_DRAWING` を返す契約)。
    厳密に bounded にするなら wgpu / core / hal の patch か render thread 分離が要る。
  - ~~**B は acquire だけ先に閉じて、Present は A 後の実測を見てから判断する**のが妥当。~~
    **2026-08-17 撤回。acquire も先に測る**。下記参照。
- **B は「計測 (B0) が先」に変更 (2026-08-17、Codex Sol の設計レビューで差し戻し)**。
  正本 = [codex-acquire-readiness-gate-brief.md](briefs/codex-acquire-readiness-gate-brief.md)。
  差し戻しの決め手 2 つ (どちらもコードで実地確認済み):
  - **acquire を閉じても message service latency は上限化されない**。非ゼロ `Resized` は
    message-dispatch 中に `on_window_resized` → `configure_surface` を通り、wgpu-hal DX12 の
    `configure` は `ResizeBuffers` の前に `wait_for_present_queue_idle()` を呼ぶ。その中身は
    **`WaitForSingleObject(event, INFINITE)`** (`wgpu-hal-27.0.4/src/dx12/device.rs`)。
    unconfigure / drop 側にも同じ待ちがある。**§1.31-A が残した inline resize 例外は、
    acquire より手前で無期限 GPU wait に入れる。** 「acquire は最大 1 秒」も強すぎた
    (1000ms は frame-latency wait の timeout であって acquire 全体の上限ではない)。
  - **§1.31-A の後、acquire が実害を起こしている証拠が無い**。§1.30 の実スタックが本当に
    この wait だったかも未確定のまま。`configure` の `INFINITE` / acquire / Present の
    3 候補があり、どれが効いているか不明な状態で 1 つを選んで直さない。
  - 旧 acquire gate 案には他に P0 が 4 件あった (ブリーフ §3 に訂正付きで保存):
    `waitable_handle` は `unsafe` で「swap chain が生きている間だけ有効」→ worker へ raw handle を
    渡すのは未定義動作 (`DuplicateHandle` が要る) / UI と worker を同時 waiter にすると signal の
    取り合いになる / §1.31-A の `surface_generation` は handle generation の正本ではない
    (`RecreateSurface` で増えず、`set_window(None)` は全 surface を clear する) /
    immediate viewport は親の pass 内で自前 surface を acquire するので outer gate では捕まらず、
    gate の実体は `egui-wgpu::Painter` の per-surface seam に要る。
  - **API 到達性の結論は正しかった**: wgpu 27 に `hal` feature は無いが native build では
    `cfg(wgpu_core)` で `wgpu::hal` が re-export され、`Surface::as_hal` も public (`unsafe`)。
    mIV の依存構成で `wgpu/dx12` / `wgpu-hal/dx12` が有効、`wgpu-hal 27.0.4` 一つに解決される。
    **acquire handle へ到達するための wgpu vendoring は不要**。
  - **`NativeEguiOverlay` は別の `wgpu::Instance`** (`src/video/native_presenter/render_core.rs`)
    で既定 `Wait` のまま動く。`src/lib.rs` の設定は届かないので、
    「mIV の acquire 全体を直した」とは言えない。
- **§1.31-A 着手前の 2026-08-16 に訂正した認識**
  (前セッションの読み違い。同じ誤りを繰り返さないため履歴として残す):
  - **外側の paint 位相は現在存在しない**。`check_redraw_requests` は
    [run.rs:198](../vendor/eframe/src/native/run.rs:198) `handle_event_result` の末尾と
    [run.rs:396](../vendor/eframe/src/native/run.rs:396) `new_events` から呼ばれ、
    `WinitAppWrapper` には `about_to_wait` の実装が無い (trait 既定の no-op)。
    不可視窓の direct paint を「既に外側」と読まないこと。
  - **`RepaintNow` は resize 専用ではない**。producer は初回 `resumed` (bootstrap) /
    非ゼロ `Resized` / AccessKit initial tree の 3 つで、`EventResult::RepaintNow(WindowId)` は
    理由を失っている。位相を分けるには reason の型付けが要る。
  - **無限 `WM_PAINT` の懸念は撤回してよい**。winit は `RedrawRequested` を送った後に
    `DefWindowProcW(WM_PAINT)` を呼び、これが update region を validate する。
    eframe が描画せず戻っても再送ループにはならない。
  - **厳密な `MARKER_IN_SIZE_MOVE` 限定は採らない**。winit の非公開状態であり、
    取りに行くと winit の vendor patch か全 HWND subclass が要る。A の inline 例外は
    「非ゼロ `Resized`」とする (現行は全 redraw が inline なので、これでも厳密に狭い)。
- **設計時に確定すべき点 (2026-08-16 に追加で判明したもの)**:
  - **`WM_PAINT` 内の同期 `RedrawRequested` だけでは足りない**。Windows の resize では
    [eframe run.rs:121](../vendor/eframe/src/native/run.rs:121) が `RepaintNow` を受けて
    その場で `run_ui_and_paint` を直接呼んでいる (flicker 対策)。同期 paint の入口は 2 つある。
  - **acquire を短くしても Present 側の上限が未規定**。wgpu-core の acquire timeout は
    `FRAME_TIMEOUT_MS = 1000` 固定 (present.rs)。DX12 backend は frame-latency waitable object の
    `wait(timeout)` が返す `false` を捨てて先へ進む (`unsafe { sc.wait(timeout) }?;` で bool を破棄)。
    Present は FIFO なら interval 1 で `DXGI_PRESENT_DO_NOT_WAIT` 無し。
    **公開 wgpu API の設定だけでは短時間 try-acquire にできない**。lower-layer patch /
    waitable-object の明示 readiness gate / 非ブロッキング Present 戦略のいずれかまで設計に含める。
    なお §1.30 の「wait は本当に無限か」は、コード上は 1000ms 上限側と整合する
    (= デッドロックより飢餓)。ただし実際に観測された `WaitForSingleObjectEx` がこの wait かは
    `latency_waitable_object` の設定次第なので、live evidence の項目は開いたままにする。
  - 通常の frame drop は `Skipped` と**別の outcome** (`DeferredNotReady` 等) にする。
    通常動作なので warning 対象ではなく、damage の保持と readiness wake が要る。
  - `Idle / Painting / Pending` を global scalar にすると複数 viewport の dirty 状態を失う。
    viewport ID / dirty set / 最新 resize / surface 世代を持たせる。
  - drop 後に即 repost すると spin する。waitable signal / event / 明示的 scheduler wake で再 arm する。
  - `WM_PAINT` を即 return する際の `BeginPaint` / `EndPaint` または validation 規則を決めないと
    無限 `WM_PAINT` になる。
  - immediate viewport は UI callback 中に同期再帰する
    ([wgpu_integration.rs](../vendor/eframe/src/native/wgpu_integration.rs) の nested 経路)。
    フレーム全体を単純に `Painting` にすると detached / immediate viewport を塞ぐので、
    state の適用範囲を定義する。
  - dropped frame でも `textures_delta` は配送する (= §1.86 の契約)。screenshot request は
    成功まで保持 / 再投入する。
  - Windows 回帰テストは 2 つの性質を分ける: (a) `WM_PAINT` / 同期 message handler が有限時間で
    返ること (b) presentation 不能中も外側 render step と message-pump の service latency が有限で
    あること。ハング防止に raw `SendMessage` ではなく `SendMessageTimeout` を使い、実 DWM stall では
    なく決定的な readiness gate を止めて再現する。
- 規模 / 優先度: **残るのは §1.31-B の上限制約 = Large / P1 candidate (基盤)**。
  watchdog の穴 (Small / P2) は上記のとおり解消済み。

### 1.35 リネーム移行ジャーナルの部分失敗が再試行されない

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、Codex)。source inspection で確認済み。
- 機構 (仕様判断あり): worker が `panicked` を返したときだけ
  ジョブを `rename_migration_boot_retry` へ戻す。`report.errors` に個別ストアの失敗が
  入った場合はトーストを出すものの、ジョブは完了扱いでジャーナルから消える。一時的な
  DB ロックや I/O エラーだと、編集結果やパス依存設定の一部が旧パスに残ったまま再起動でも
  回復しない。
  - **これは現状のコードコメントが明記している意図的な選択**である
    (「per-store エラーは通常経路と同じ best-effort = 再試行しない」、v2.3.0 角度⑤ 検収時の判断)。
    したがって着手前に「一時エラーと恒久エラーを区別できるか」「再試行の上限をどう置くか」を
    決めること。無条件の再試行は、決定的に失敗するストアで無限ループになる。
  - 移行処理自体は冪等に設計されているので、失敗ストアを保持して再試行する形にはできる。
- 規模 / 優先度: Medium / P2 (仕様判断を先に決める)。もう 1 件あった「UI スレッドでの同期 I/O」は
  v2.10.0 で latest-value worker へ移して close 済み。

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

### 1.37 トレイ常駐再生まわりの所有境界 2 件

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
  v2.9.1 で「トレイ格納中も再生を続ける」経路を新設したことで、いずれも**この版から初めて
  実際に通る**組み合わせになった。
  1. **P3: 再生継続中に video processor cache を落とす**。
     [tray_integration.rs](../src/tray_integration.rs) の `release_gpu_resources` は
     detached 早期 return より前で `release_idle_pools()` を呼び、その中の
     `processor_cache.take()` は**無条件**である (`hw_frames_pool.clear()` と
     `shared_output_pool.retain(in_use)` は refcount 上安全)。
     ⚠ **呼び出し順自体は v0.9.1 (`287eea9f`, 2026-05-19) からで、v2.9.1 で動かしたわけではない**。
     新しいのは「格納中も再生が続く」ことのほうで、その結果この経路が再生中に走るようになった。
     トレイ格納の瞬間に video processor の作り直しが 1 回入るはずで、実害は小さい。
  2. **P3: 外部 hide 検出が tray 所有権に追随していない**。[app.rs](../src/app.rs) の
     `IsWindowVisible` 追従分岐は `viewer_session_is_detached_or_switching()` だけで heartbeat
     suspension を決め、`sync_media_presenter_visibility_for_tray()` /
     `sync_retained_viewport_visibility_for_tray()` を呼ばない (これらは `hide_to_tray` /
     復帰経路にしか無い)。同フレーム後半の `sync_tray_resident_media_wake()` が**値が変化した
     ときだけ**自己修復するので実害は出にくいが、hide の所有者が 2 つある状態である。
- 規模 / 優先度: どちらも Small / P3。3 件目だった「WM_PAINT ブリッジに背圧が無い」は
  v2.10.0 で claim / ack 構造へ変えて close 済み。

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

### 1.39 `fs_page_load_state` が PDF ページで毎フレーム String を作る (`9590b661` の続き)

- 出典: v2.9.1 出荷後の他セッションレビュー (2026-08-01、ClaudeCode)。source inspection で確認済み。
- 機構: `has_retained_pdf_final_ai_for_current_params` → `retained_pdf_page_key_for` →
  `metadata_cache_key(idx)` が String を生成し、HashMap を引く。
  ⚠ **レビュー報告より実コストは小さい**: `effective_params` / `final_ai_key_for_pixels` は
  retained cache に**該当エントリがあるときだけ**走る (無ければ `?` で抜ける)。
  非 PDF は `items.get` + `matches!` で即 return なので無コスト。
  とはいえ PDF フルスクリーンでは `update_prefetch_window` の current、prefetch ターゲット
  全件、連結読み keep 範囲ループ全件、見開きパートナーから毎フレーム走るため、
  key を 1 回作って使い回す形にできるかは見ておく。
- 規模 / 優先度: Small / P3。

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

### 1.47 動画の拡大縮小を mIV のシェーダで行う — 設計確定 / 未実装

- **これが入ると 1.112 (360 度動画) の前提が揃う。**投影を差し込む場所を作るのがこの項目であり、
  正距円筒投影は同じシェーダステージ上のもう 1 枚として載る。設計を変えるときは 1.112 も見る。

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
- **先送り (2026-08-13、利用者判断)**。提案として妥当だが、v3.0.0 では扱わない。
  却下ではないので、要望が重なったら再評価する。
- 規模 / 優先度: Medium / P3 (先送り)。

### 1.62 お気に入り編集を開いている間、進捗が動いていなくても 100ms ごとに repaint する

- 出典: v2.13.0 の idle health 測定中に判明 (2026-08-11)。退行ではなく既存仕様。
- 観測: `favorites_editor.rs:752` が `ctx.request_repaint_after(100ms)` を**無条件で**呼ぶため、
  ダイアログを開いている間ずっと 11〜12 fps で `App::update` が回る。perf log で
  `prev_frame_causes=['src\\ui_dialogs\\favorites_editor.rs:752']` が連続して確認できる。
- 実害: 利用者は**起動後の索引作成が終わるのを確認するためにこのダイアログを開いたままにする**
  運用をしており、索引作成は 5 分ほどかかる。その間ずっと起きている。ノート PC やタブレットの
  電池には無視できない。v2.13.0 でタッチ対応を入れた以上、タブレット運用は増える。
- 現行コードのコメントは「active が空でも notify-rs が動き出した瞬間に拾えるよう常に呼ぶ」と
  理由を書いており、**意図的**である。直すなら意図を保ったまま頻度を落とす:
  - active が空の間は 100ms ではなく 500ms〜1s へ落とす (動き出しの検出遅れは許容範囲)
  - または watcher 側から `ctx.request_repaint()` を呼び、ポーリング自体をやめる (構造的)
- 同型の確認: 他にも「進捗表示のために無条件 `request_repaint_after`」を持つダイアログが
  無いか探すこと。あれば同じ方針で揃える。
- 規模 / 優先度: Small / P2。

### 1.69 変換対象アーカイブのキャッシュパスが未解決のとき、ZipDir タイルだけ黙って失敗し得る

- 出典: §1.66 の修正 (`ff56abea`) の周辺を洗って見つけた。**利用者報告ではなく、
  実際に到達するかも未確認**。今回の不具合ではなく以前からある形。
- 形: `converted_archive_cache_paths` (= 変換キャッシュ ZIP へのパス表) は worker が
  非同期に作るので、表が空の窓がある。この窓での振る舞いが item 種別ごとに違う。
  - `GridItem::ConvertibleArchive`: `?` で **LoadRequest を作らない** → 後のフレームで
    再要求される。安全。
  - フォルダピン経由: `ff56abea` で「表が届いたら依存タイルだけ組み直す」ようにした。
  - **`GridItem::ZipDir`: `unwrap_or(zip_path)` で元アーカイブのパスへ落ちる**。RAR は
    `zip_loader` が `rar_loader` へ振り分けるので読めるが、**7z / ソリッド RAR は ZIP として
    開こうとして失敗**し、そのサムネイルは失敗のまま残る。
- 到達性: コードのコメントによると、`ZipDir` が元 RAR/7z/LZH のパスを持つのは
  **レーティング一覧に復元されたとき**。実際にその状態を作って確認していない。
- 方針: **まず到達するかを確かめる。** 到達しないなら、3 経路で fallback の作法が違うこと
  自体をコメントで明示して閉じる。到達するなら `ff56abea` の依存追跡
  (`pin_archive_dependencies`) を ZipDir にも広げる。**確認前に直さないこと。**
- 規模 / 優先度: Small / P3。

### 1.79 比較表示中の元画像表示 (押している間だけ両側を元画像にする) — 利用者要望

- 出典: 利用者実機確認 2026-08-13。比較 (ワイプ) 表示中に元画像表示 (`FsOriginalPreviewHold`、
  既定 RightCtrl) が効かない、という報告。**期待する挙動は「ワイプはそのままに、ワイプ元と
  比較対象の両方が元画像になる」** (利用者判断)。片側だけ元画像にしたり、比較をやめて
  元画像 1 枚を出すのは求められていない。
- **現状 (source inspection、2026-08-13)**: 比較表示中は**全モードで**元画像表示が効かない。
  `compare_requested` は `compare_view_mode != Off` だけで決まり、
  `CompareFramePrimaryDraw::resolve` は元画像表示を入力に取らない
  ([ui_fullscreen.rs:11708](../src/ui_fullscreen.rs:11708))。今回の Ctrl まわりの変更以前からこう。
- **実装コストは Medium。小さくない。** 比較の合成は加工済みピクセルから作られており、
  「元画像版の対」を新たに用意する必要がある。
  1. **prepared pair は非同期 worker が作る**。identity は
     `(current_idx, pinned_source_idx, output)` なので、**加工済み / 元画像の次元を足す**必要がある。
  2. **比較対象 (pin した側) は、pin した時点の加工済みピクセルの snapshot** (`slot.pixels`) で
     保持している ([ui_fullscreen.rs:22982](../src/ui_fullscreen.rs:22982))。元画像版は存在しないので、
     **pin 時に未加工の snapshot も持つ (メモリ倍) か、押された時点で再デコードする (遅延)** かの
     選択が要る。再デコード対象は現在ページとは限らず、`fs_cache` に無いこともある。
- 方針の案:
  - 元画像版の対は**押されたときに遅延生成**し、準備できるまでは**加工済みの比較を出したまま**にする
    (途中で別の絵に切り替わるちらつきを作らない)。既存の `WaitingForSource` / 「比較表示を準備中」
    の枠組みに乗る。
  - 一度作った元画像版は、対の identity が変わるまで保持して 2 回目以降を即時にする。
- 着手時に読む: 上記 `ensure_compare_prepared_pair` / `ComparePrepareOutput` / `pinned_compare_slot`。
- 関連: 線を消す修飾は左 Ctrl 限定にしてあるので (`88b028e1`)、既定 RightCtrl の元画像表示とは
  物理的に競合しない。この項目を入れても修飾キーの整理は不要。
- **当面の決着 (2026-08-13、利用者判断、`42212e9d`)**: 実装コストが高いので、
  **比較表示中は元画像表示を明示的に無効**にした。有効なままだと画面は変わらないのに
  「比較表示を準備中」トーストが出るなど副作用だけが起きるため、`original_preview_active`
  の入口で断っている。本項目を実装するときはこの早期 return を外す。
- 規模 / 優先度: Medium / P3 (比較表示は加工結果を見比べる機能で、元画像表示が効かないことが
  データを壊すわけではない)。

### 1.95 OS 側でコピー・移動したファイルへ編集内容を引き継ぐ (内容ハッシュ照合) — 利用者要望 ✅ Phase 1 実装済み (2026-08-22)

- 出典: 利用者要望 2026-08-20。エクスプローラー等でファイルを移動・コピーすると、
  補正 / 消しゴム / モザイク / 注釈 / トリミング / ★ / タグが引き継がれない。
- **正本: [edit-content-identity-plan.md](edit-content-identity-plan.md)。Phase 1 (A1-A6) 実装済み・実機確認済み。**
  複数コピー元の選択 (A6) まで完了。残りは同計画の「未着手」節 (アプリ内コピー / ページ移動の
  同じ失効漏れ、復元を通っていないファイルの注釈サムネイル) を参照。
  着手前に必ず読む。以下は要旨だけ。
- 現状の穴: 中央 DB は絶対パスキー。フォルダ側サイドカーは**フォルダごと**の移動しか救えず、
  リネーム移行は**アプリ内**リネームだけ。さらにサイドカーがミラーするのは
  `adjust / mask / conceal / local_adjust / export_crop / comic / tags` のみで、
  **★ / 回転 / 見開き / トリム / 本の続きはフォルダごと移動でも失われる**。
- 方式: 編集の確定点で物理ファイルのハッシュを台帳 (`content_identity.db`) に記録し、
  フォルダ読み込み時に **size → 先頭 64KB → 全体** の 3 段で照合。size はフォルダ走査で
  既に取得済み (`image_metas`) なので、候補が無いフォルダでは **I/O ゼロ**。
  照合と復元は worker (`GlobalIoSemaphore` Low、フォルダ切替でキャンセル)。
- コピー実処理は `rename_key_migration::STORES` 駆動の `copy_store` を新設して共有する
  (`App::copy_book_page_edit_key` は UI スレッド前提で worker から呼べない)。
- **変換アーカイブ (RAR/7z/LZH) を最初から対象にする**。キャッシュ ZIP のパスは元パスの
  純関数なので、再変換を待たずキーを付け替えられる。Source / ConvertedCache の 2 基底 ×
  exact / prefix の **4 面**をコピーする。
- 決定済み: 移動もコピーも既定で確認する / 復元範囲は★・タグ込みで一括 /
  「すべて選ぶ・すべて解除」ボタンを置く / 「このフォルダ以下では聞かない」は作らず
  全体 OFF 設定 1 個 (既定 ON、OFF で照合を完全停止) / リモートは対象外 /
  既存編集の遡りは一括スキャンせず訪問時に少しずつ。
- 規模 / 優先度: Medium / P2 (データ損失ではないが、編集した資産が黙って失われる体験は重い)。

### 1.99 複数ウィンドウモードで RAR を開くとメイングリッドまで書庫一覧へ切り替わる — 専用スレ >>270 ✅ 実装済み (2026-08-22)

> 実装は detached-rework の `21c3dc0d` / `d7e139d0` に入っており、2026-08-22 の master マージで
> 取り込まれた。型付き宛先意図 `OpenRequestOwner::DetachedGridArchive` →
> `ArchiveConvertCompletionPolicy::DetachedGridArchive` を、ダブルクリック / Enter /
> 直読み完了 / 変換完了 / cache hit のすべてが引き継ぐ。visibility 述語・`show_viewport_*`・
> R2e は不変。**実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>270 (2026-08-20)。利用者環境・手元とも再現済み。
- 症状: 複数ウィンドウモードで RAR を開くと独立した画像ウィンドウだけでなく、
  メインウィンドウのグリッドも RAR 内の一覧へ切り替わる。ZIP では発生しない。
- 期待する不変条件: 複数ウィンドウモードから画像 / 本を開く操作は、表示先の独立ウィンドウだけを
  更新し、メイングリッドの場所・一覧 generation・検索 / 絞り込み・選択状態を変更しない。
- 修正方針:
  - RAR の open request が、どの入口からメイングリッドの一覧遷移へ流れているかを確認する。
    RAR だけを後段で戻す guard は入れず、表示要求の所有 context と dispatch 先を正す。
  - 直読み RAR / CBR と変換対象 RAR の両方を確認し、ZIP / PDF を対照経路として open request の
    差を棚卸しする。
  - detached 述語 / viewport 経路へ触れる場合は [detached-rework-plan.md](detached-rework-plan.md)
    §2 に従い、症状パッチでないことを ClaudeCode / Codex の双方で確認し、同書 §11 に記録する。
- 回帰確認: 複数ウィンドウモードで RAR / CBR / ZIP / PDF を順に開いてもメイングリッドが不変で、
  各独立ウィンドウには正しいページが表示される。フル機能ウィンドウの従来の一覧遷移は壊さない。
- **根本原因 (2026-08-21 特定、v3.1.3 では未修正)**: ZIP / PDF はダブルクリック・Enter・
  ゲームパッドのどの入口でも共通の detached 振り分け ([ui_main.rs:13017](../src/ui_main.rs:13017)、
  [app.rs:34249](../src/app.rs:34249)) を先に通るが、**typed open plan が本コンテナとして認識するのは
  PDF / ZIP だけで、`ConvertibleArchive` である RAR / CBR は対象外**
  ([app.rs:37317](../src/app.rs:37317))。RAR は振り分けを素通りして通常ナビゲーションへ落ち、
  所有者が `OpenRequestOwner::Navigation` に固定される ([app.rs:18298](../src/app.rs:18298))。
  RAR は同期的に ZIP として開けず、直読み可否を App-global で probe した**後**に
  メイン一覧へ遷移する ([archive_convert.rs:675](../src/ui_dialogs/archive_convert.rs:675) /
  [854](../src/ui_dialogs/archive_convert.rs:854))。
- **「RAR を ZIP と同じ descriptor に足す」では直らない。** RAR は非ソリッド・入れ子なしだけが
  直読みで、それ以外は変換対象という分岐がある ([virtual-folders.md:116](virtual-folders.md))。
- **正しい修正 (2026-08-21 に具体化)**: 完了時点では detached 窓も bundle もまだ存在しないので、
  「既存 context の所有者」ではなく **「この要求のために新しい detached context を作る」という
  型付きの宛先意図**を非同期完了へ持たせる。**同型の実装が既にある**:
  ブックマーク経路は `OpenRequestOwner::Bookmark(owner)` →
  `ArchiveConvertCompletionPolicy::Bookmark(owner)` ([archive_convert.rs:47](../src/ui_dialogs/archive_convert.rs:47))
  を持ち、直読み完了と変換完了の両方から
  `open_converted_bookmark_in_detached_context` ([app.rs:39993](../src/app.rs:39993)) へ着地して
  `ViewerContextDescriptor::Zip { path: 実体, archive_source_override: 元アーカイブ }` で
  **新しい detached context を作る**。グリッドからの `ConvertibleArchive` 開きにも、
  `DetachedGridItemOpenPlan::FolderCandidate` ([app.rs:1857](../src/app.rs:1857)) と同じ形で
  candidate variant と着地関数を用意する。
  入口は既に共通化されているので、ダブルクリック ([ui_main.rs:13017](../src/ui_main.rs:13017)) と
  Enter ([app.rs:34249](../src/app.rs:34249)) の両方が同じ gate を通る。
  **visibility 述語や `show_viewport_*` builder の変更は不要。**
  変換キャッシュ命中時の同期経路 (`open_archive_via_cache_owned`) も同じ宛先意図を通す。
- **R2e (viewer context registry) には依存しない** (正本:
  [detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5)。
- 規模 / 優先度: Medium / P1。次版修正候補。

### 1.100 複数の画像ウィンドウを開くと、先に開いたウィンドウでマウスジェスチャが効かなくなる — 専用スレ >>270

- 出典: 専用スレ >>270 (2026-08-20)。利用者環境・手元とも再現済み。
- 症状: 複数ウィンドウモードで画像ウィンドウを複数開くと、後から開いたウィンドウでは
  マウスジェスチャを使えるが、先に開いたウィンドウへ戻っても認識されない。
- 期待する不変条件: 右ドラッグの入力状態と発火先は、入力を開始した viewer window / viewport が
  所有する。アクティブウィンドウを切り替えても別ウィンドウの状態に上書きされず、閉じた
  ウィンドウの状態も残らない。
- 修正方針:
  - pointer event の source、ジェスチャ状態機械、実行 context の各所有先を追い、グローバルな
    active target や最後に開いた viewport へ誤って寄っている境界を正す。
  - open / activate / deactivate / close / cancel の lifecycle と、リングショートカット・短い
    右クリックの兄弟経路も同じ owner 規則になっているか確認する。
  - detached 述語 / viewport 経路へ触れる場合は §1.99 と同じく
    [detached-rework-plan.md](detached-rework-plan.md) §2 / §11 の規約に従う。
- 回帰確認: 3 枚以上の独立ウィンドウを開き、前後のウィンドウを交互にアクティブ化して同じ
  ジェスチャが発火する。最新ウィンドウを閉じても先行ウィンドウで継続し、メイングリッドと
  native 動画のジェスチャを壊さない。
- **根本原因 (2026-08-21 特定・訂正、v3.1.3 では未修正)。これは R2 そのもの**:
  - **非アクティブな静止画ウィンドウ上の右ドラッグは、アクティブ化もせずジェスチャ状態機械にも
    届かない完全な no-op である。** 非アクティブな静止画窓は `show_viewport_deferred` で描かれ、
    コールバックが root pass へ報告するのは描画 / focus / close 要求 / placement だけで、
    ポインタ入力を一切拾っていない ([ui_fullscreen.rs:11622](../src/ui_fullscreen.rs:11622)、
    [ui_fullscreen.rs:11175](../src/ui_fullscreen.rs:11175))。アクティブ化を担う OS watcher は
    **`VK_LBUTTON` しかサンプルしない** ([detached_window_manager.rs:326](../src/app/detached_window_manager.rs:326))。
    左クリックでアクティブ化した後ならジェスチャは効く (§4.5 の「アクティブにすれば動く」と一致)
  - `MouseGestureState` が持つ識別情報は表示種別だけで、**window ID / viewport ID / session ID を
    持たない** ([ring_shortcut.rs:53](../src/ring_shortcut.rs:53))。状態は
    `ViewerContextBundle` ではなく **App に 1 個だけ**ある ([app.rs:10005](../src/app.rs:10005))。
    発火時に owner を再解決せず、その時点で mount されている context へ適用する。
    複数の画像ウィンドウはどれも `RightDragContext::ImageFullscreen` なので区別できない
  - `last_input_surface` は `MainWindow` / `Viewer` の二択で全 viewer window を区別しない
    ([detached_window_manager.rs:452](../src/app/detached_window_manager.rs:452))
  - **訂正**: 当初「非アクティブな窓が集める pointer 情報は `any_pressed` / `any_released` だけ」
    「アクティブ化は press ではなく release で起きるので最初の右ドラッグが失われる」と記録したが、
    これが当てはまるのは `show_viewport_immediate` で描く `ParkedLive` の**動画 / 音声窓だけ**
    ([ui_fullscreen.rs:11785](../src/ui_fullscreen.rs:11785)、[app.rs:39217](../src/app.rs:39217))。
    静止画窓はそこへも到達しない
- **利用者決定 (2026-08-21)**: 「ジェスチャを認識し、ジェスチャされた場合は自動でアクティブ化した
  上で、ジェスチャコマンドを実行する」。**両方のウィンドウが見えている以上、ジェスチャが受理される
  のが期待動作**という判断。右押下だけでアクティブ化する案 (1 回目のドラッグを消費する / 単なる
  右クリックで窓が前面化する) は採らない。
- 必要なもの (3 つ、いずれも R2 の守備範囲):
  1. 非アクティブ窓のポインタ列を、その窓の identity 付きで root pass へ届ける
     (`DeferredDetachedImageWindowEvent` は既に window id を持つ)
  2. ジェスチャ状態を **window 所有**にする (= R2b 残件の「散在 state の typed 集約」)
  3. 成立時に「アクティブ化 → コマンド実行」を**型付きの順序**として表す (guard / 遅延ではなく)
- **R2 (`DetachedWindowRuntime` + 状態の集約) の一部として実施する。** ジェスチャ状態を window
  所有にするのは R2 の守備範囲であり、App 側に識別子を足す小修正は憲法 3 に抵触する。
  一方、コマンドはアクティブ化**後**に実行するので、マウントされていない context へ適用する必要は
  無く、**R2e (viewer context registry) の完成には依存しない**
  (正本: [detached-r2e-ownership-design.md](briefs/detached-r2e-ownership-design.md) §5)。
- **状態 (2026-08-21)**: 実装済み・検収合格・**実機確認済み**。静止画 (`8db282e3` + `5b83df3f`)、
  ガイド表示 (`d2c19796`)、動画 ParkedLive (`c71d8c08` + `4c5260a2` + `12f60c97`) のすべてで動作確認済み。
  指示書は [detached-rework-stage-passive-gesture.md](detached-rework-stage-passive-gesture.md)、
  進捗記録は [detached-rework-plan.md](detached-rework-plan.md) §9.5。
  **実機で確認が取れて出荷するまで本項は消さない** (根本原因・利用者決定・訂正履歴の正本)。
- 規模 / 優先度: Medium / P1。

### 1.101 動画の上部 HUD / 下部シークバーを個別に固定表示できるようにする — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> 設定は動画専用 bool `video_top_bar_locked` / `video_seek_bar_locked` の 2 つ。既定は両方 `false`。
> 静止画の `fullscreen_*_locked` は共有せず、余白だけ `fullscreen_fixed_bar_gap_px` を再利用する。
> 固定状態は可視性純関数への**入力**として渡し、描画と HUD HWND の hit-test region は
> `render_once` が算出した `top_bar_drawn_visible` / `bottom_hud_visible` の**同じ snapshot** を読む。
> 上部の固定は下部へ漏らさない。tile grid / navigation preview 中は上下とも抑止する。
> 固定バーは映像へ重ねず、`compute_video_visual_transform` の target からバー領域と余白を除外する。
> compact は除外後の残り領域の右上 1/4 へ合成する。鍵アイコンは静止画のベクター描画を共有する。
> 固定表示は external drag 中も維持し、touch latch 自体は変えない。
> 音声専用設定は増やさず、通常の音楽ビューは従来どおり常時表示。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。動画シークバーを常時表示したい要望。
- 方針: 静止画と同じ考え方で、動画の上部 HUD と下部シークバーをそれぞれ独立して
  固定 / 自動表示へ切り替えられるようにする。動画専用設定とし、既定は現在の自動表示を維持する。
- 実装先は native presenter の HUD overlay
  (`src/video/native_presenter/{render_core.rs,overlay_draw.rs}`) とし、旧 egui 動画 UI へだけ
  設定を足さない。通常の音楽ビューは対象外とし、VST 用 audio-only native shell だけ動画設定を共有する。
- 固定中は映像 target からバー領域を除外し、VST editor、タッチ操作、HUD HWND の hit-test region /
  z-order を確認する。固定解除後は既存の自動表示へ戻ること。
- 規模 / 優先度: Medium / P2。

### 1.102 YouTube 型の精密シーク用サムネイル列 — 専用スレ >>271 / >>277

- **1.113 (音声波形ストリップ) と同じ場所を使う。**ストリップの導線 (シークバーから上へドラッグ) を
  共有し、中身をサムネイル列と波形で切り替える前提で設計する。片方だけ先に作らない。

- 出典: 専用スレ >>271 (2026-08-20)。シークバーへサムネイルを並べたい要望。
- **意図確認済み (専用スレ >>277)**: YouTube でシークバーを上へドラッグしたときに出る、
  動画を見たまま複数の場面を横一列で選べる「精密シーク」相当が希望。常時表示する
  サムネイル付きシークバーではない。
- 既存機能との境界:
  - `S` / 上部 HUD のタイルボタンで、動画全体の場面を複数タイル表示してシークできる。
  - `B` の動画ブックマークは、左パネルへサムネイル・時刻・名前を並べる。
  - シークバー hover では、その位置のプレビューを 1 枚表示する。
- YouTube デスクトップ版の観測 (2026-08-20、実装仕様の公式保証ではなく参考値):
  - シークバーから上へドラッグすると、160x90px 程度のサムネイル列を現在位置中心に表示し、
    列を横へ動かすことで動画全体をたどる。通常幅では約 5〜6 枚、広い画面ほど表示枚数が増える。
  - ストーリーボードはキーフレーム列ではなく固定時間間隔。観測例は 3:33 = 2 秒間隔 / 108 枚、
    8:14 = 5 秒 / 100 枚、20:17 = 10 秒 / 123 枚、2:00:53 = 10 秒 / 727 枚。
  - YouTube は事前生成済み JPEG sprite を配信するため、ローカル動画からその場で作る mIV と
    抽出コストの前提が異なる。見た目だけをそのまま真似て全時間分を先に生成しない。
- mIV での UI 方向:
  - シークバーから上方向へ一定量ドラッグしたときだけ、下部 HUD 上へサムネイル列を開く。
    中央の時刻を選択位置とし、左右ドラッグで列を送る。固定シークバーでも通常時は列を隠す。
  - 初期生成は画面に見える枚数 + 少量の前後だけとし、移動方向へ逐次追加する。動画全体の
    サムネイル生成完了を待ってから表示する構造にはしない。
  - 既存 `tile_thumb_cache`、seek hover cache、タイル抽出 worker のどこまでを共有できるか確認する。
- **抽出方式は未決定 (性能との妥協を追加調査する)**:
  - 表示時刻どおりの任意フレームは、素材によって 1 枚約 1 秒かかる。一方、直前のキーフレームを
    そのまま採る場合は約 50ms。既存 §1.104 の実測でも精密側は最大約 1 秒、キーフレーム近傍は
    約 40〜80ms で、GOP 距離が支配項と確認済み。
  - キーフレームだけを列にすると高速だが、GOP によって時刻間隔が不均一になり、長い GOP では
    内容が粗くなる。最終 UI をキーフレーム列に固定するとはまだ決めない。
  - 比較候補は、(A) 固定間隔を精密復号、(B) キーフレームのみ、(C) キーフレームを先に表示して
    固定間隔の精密画像へ非同期差し替え、(D) 表示範囲の先頭側キーフレームへ 1 回 seek して
    順方向にまとめて復号、の 4 方式。素材 / GOP / HW decode 別に初回表示時間、列を送ったときの
    待ち、CPU / GPU 負荷を測って決める。
  - サムネイル画像が近似時刻でも、クリック後の本編 seek は選択した表示時刻へ行う。近似画像と
    ラベル時刻のズレをどこまで許容するかは §1.104 の設定と共有するかも含めて判断する。
- 規模 / 優先度: Medium〜Large / P3 (抽出方式の計測・設計後に着手)。

### 1.103 ごみ箱へ移す場合に mIV の削除確認を省略できる設定 — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> 省略の述語は `設定 ON && DeleteConfirmKind::RecycleBin`。
> **フォルダと ZIP / PDF / 対応アーカイブも省略対象に含む。** ごみ箱から復元できることを前提に、
> タイル 1 つが一覧に見えていない多数のファイルを持ち、そのすべてを確認なしでごみ箱へ移すことは
> 設定画面の説明で明示する。
> `MayPermanent` は従来どおりキャンセルが初期選択、`FOF_WANTNUKEWARNING` も不変。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。削除確認を省略したい要望。
- 仕様:
  - 設定は既定 OFF。ON の場合も、mIV の事前判定が `DeleteConfirmKind::RecycleBin` のときだけ
    mIV の確認ダイアログを省略する。
  - リムーバブル / ネットワークドライブ、ボリューム設定、容量等から
    `DeleteConfirmKind::MayPermanent` と判定した場合は、従来どおり確認を表示し、初期選択を
    キャンセルにする。
  - Shell 側の最終判断は mIV から確定できないため、`IFileOperation` の
    `FOF_WANTNUKEWARNING` は維持する。Windows が出す完全削除警告までは抑止しない。
  - 複数選択時は 1 件でも `MayPermanent` があれば省略しない。全件 `RecycleBin` なら、フォルダや
    一覧に見えていない内容を持つアーカイブも確認を省略する。
- 回帰確認: 通常のローカルファイル、フォルダ、ZIP / PDF / 対応アーカイブをごみ箱へ移す場合は
  確認が省略され、完全削除候補、混在選択、Shell がごみ箱へ移せないケースでは警告導線が残る。
  設定 OFF は従来動作と同じ。
- 規模 / 優先度: Small / P2。

### 1.105 動画のフルスクリーン上部バーにも「…」オーバーフローを出す — 利用者要望

- 出典: 利用者要望 (2026-08)。静止画のフルスクリーン右上に「…」を追加したので、動画にも
  同じものがあると UI として揃う、という指摘。
- 現状: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `overflow_available` が
  `!state.is_video && !is_music_view && !panorama_mode_active_now && ...` で、
  **画像のときだけ**「…」を出す。動画・音楽・パノラマでは出ない。
- 仕様の論点: 動画の上部バーには既に動画専用ボタン (タイル一覧 / Perf グラフ 等) が並ぶ。
  「…」に何を入れるかを決める必要がある。
- **1.104 とは切り離す (2026-08-20 決定)**。「ズレ許容」をフルスクリーン中から変えられると
  体感を確かめながら調整できるが、動画側の上部バーへ overflow を通す修正量が大きい。
  1.104 の初回リリースには**含めない**。設定は環境設定からのみ変更できる形で出す。
  報告者へもこの件は伝えていないので、実施を約束したものとして扱わない。
- 規模 / 優先度: Medium / P3。

### 1.106 リングショートカット / マウスジェスチャを左クリックで取り消す — 利用者要望

✅ 実装済み (2026-08-22)。グリッドセル / グリッド背景 / active viewer / passive detached
viewer の 4 開始面で、進行中の右ドラッグを左ボタン press から既存 cancel helper へ流す。
active 面では対応する左 release と右ボタンの後続 release を通常 click として再利用しない。
passive detached は通常どおり release で窓を activate するが、選択 / open / viewer action には
再利用しない。固定 mouse chord の理由は `keymap-spec.md` に記録した。

- 出典: 利用者要望 (2026-08)。右ドラッグで開始した後にやめたいとき、取り消す手段がない。
  現状の回避は「リングは中央の円へ戻して離す」「ジェスチャは割り当てのない軌跡にする」。
  提案は**右ドラッグ中に左クリックを押すと取り消し**。
- 実装の見通しは良い。取り消し自体は既にあり ([ui_main.rs](../src/ui_main.rs) の
  `cancel_mouse_ring_flick` / `cancel_mouse_gesture` をダイアログが開いたときに呼んでいる)。
  足すのは発火条件だけ。
- 確認すべき点:
  - 開始点は 3 か所。[ui_main.rs](../src/ui_main.rs) に 2 か所 (グリッドと、もう 1 か所)、
    [ui_fullscreen.rs](../src/ui_fullscreen.rs) に 1 か所 (フルスクリーン / native 動画)。
    **同型の入口をすべて塞ぐ** (片方だけ直すと「一覧では取り消せるがフルスクリーンでは効かない」になる)。
    タッチからの開始経路は無い (右ドラッグのみ。`touch_input.rs` / `touch_correlation.rs` に
    ring / gesture の参照が無いことを確認済み)。
  - 取り消しに使った左クリックが、離した時点で通常のクリック動作 (選択 / 開く) として
    発火しないこと。リング / ジェスチャが active な間は左ボタンの press と release を
    両方消費する必要がある。
  - 取り消し自体は既に多くの場所から呼ばれている (gamepad / native_video / app / fullscreen /
    main)。追加するのは左ボタン押下という発火条件だけで、取り消し処理は既存のものを使う。
- 規模 / 優先度: Small / P2。

### 1.107 Z 照準のカーソル写像を「画面帯」から「実際に描いている画像領域」へ — 利用者要望
 ✅ 実装済み (2026-08-21)

> 実装は [briefs/z-aim-cursor-mapping.md](briefs/z-aim-cursor-mapping.md)。写像と描画は
> `ZAimBasis` を 1 つ共有し、basis は **実際に描画しているスケール**を持つ (単ページは Z 中に
> contain を強制するので contain、見開きは表示モード由来の `fit_scale`)。見開きのカーソル写像も
> 同時に直った (それまで描画位置でない contain 矩形へ写像していた)。
> **実機確認済み (2026-08-21)**: 意図どおりの動作。以下は着手前の記録。

- 出典: 利用者報告 (2026-08)。3 パターンの切り分けまでいただいた。
  1. 上下左右に余白がない画像 → 枠とカーソルのズレは小さい (あっても枠内)
  2. 縦長画像 (左右に余白) → 上下端への枠移動は 1. と同じだが、**左右端へ動かすには
     ウィンドウ端付近までカーソルを運ぶ必要があり、左右方向に大きくずれる**
  3. パノラマ画像 (上下に余白) → 2. と逆で、左右は違和感なく、**上下のズレが大きい**
- 原因: [displayed_image_transform.rs](../src/displayed_image_transform.rs) の
  `z_cursor_image_px` が **`pan_band` (画面側の帯) 内の比率**で画像座標を決めている。
  帯は viewport 全体から上下の HUD 分だけ詰めたもので、**画像の縦横比を見ていない**。
  一方、照準枠を描く `z_aim_frame_rect` は `view_rect` に縦横比を保って収めた
  `content_rect` を基準にしている。**写像と描画で基準が違う**ので、余白が大きい方向ほど
  カーソルと枠が離れる。報告の 3 パターンはこの差でそのまま説明できる。
- 決定 (2026-08-20): **写像の基準を `content_rect ∩ pan_band` にする**。
  - `content_rect` = いま実際に描いている画像領域 (`view_rect` に縦横比を保って収めたもの)。
    `z_aim_frame_rect` が既に使っているものと**同一の値を共有する** (別々に計算すると
    また乖離する。共通 helper へ出す)。
  - `pan_band` との交差を取るのは、上下の HUD ホバー帯へカーソルが入る前に画像の上端・
    下端へ到達できるようにするため (実機 FB 2026-06-21 で入れた既存の意図)。これは維持する。
  - 効果: 縦長画像では横方向が画像領域基準になるので 2. が解消。パノラマでは
    `content_rect` が帯の内側に収まるので交差が `content_rect` そのものになり、3. も解消。
    1. は元から差が小さく、変化もほぼ無い。
  - 縮退: 交差が空、または極端に細い場合 (小さいウィンドウ + 大きい HUD 余白) は
    従来どおり `pan_band` を使う。狙えなくなる状態を作らない。
- **同型の入口**: [ui_fullscreen.rs](../src/ui_fullscreen.rs) の `zip_cursor_image_px`
  (連結表示 / 見開き合成) が**同じ式の複製**になっている。片方だけ直さない。
  可能なら 1 つの helper に寄せる。
- カーソル非表示 ([ef9d8b0b](../src/ui_fullscreen.rs)、Z を押している間は隠す) とは独立。
  この写像を直した後にカーソルを出す方が自然かどうかは、実機で見てから判断する。
  当面は非表示のままでよい。
- 回帰確認:
  - 既存テスト `zip_cursor_image_px_maps_band_to_image_and_clamps` と
    `zip_pan_band_reaches_image_edge_before_top_hover_zone` は前提が変わるので更新する。
    後者が守っている「上部ホバー帯へ入る前に画像上端へ届く」性質は**新しい写像でも維持する**
    ことをテストで明示する。
  - 縦長・パノラマ・余白なしの 3 形状で、カーソル位置と照準枠の対応を確認する。
  - トリム表示中 (`content_bbox` あり) と回転ページでも枠と写像が一致すること。
- 規模 / 優先度: Small〜Medium / P2。

### 1.108 開くときは黒地、切り替えるときは前の画像 — 表示先の占有を routing 境界で判定する

- 出典: 2026-08-20 の利用者報告 (グリッドで PDF をダブルクリックすると、1 ページ目が遅いときに
  無反応に見える) と、それに続く仕様整理。
- **確定した規則 (利用者判断)**: **表示先に既に中身があるか**で分ける。
  - **A (中身が無い)**: 新しいウィンドウが開く / F12 デタッチで新規窓 / 同一ウィンドウ内で
    ファイルを開く → **黒地を挟んでフルスクリーンへ移る**。開く操作の結果が即座に返る
  - **B (中身がある)**: Ctrl+↑↓ / メイン側から既存 detached を切り替え → **前の画像を保持**。
    黒を挟むとちらつく
  - どちらも待ちが 500ms を超えたら中央に「読み込み中」(v3.1.3 で実装済み)
- **却下された導出**: `FsNavigationSequence::previous.is_none()` では分離できない。詳細と
  全経路の分類は [docs/briefs/fs-open-black-vs-holdover.md](briefs/fs-open-black-vs-holdover.md)
  の「第 1 段階の結果」。要点:
  - `previous` は「直前の表示単位を texture として捕捉できたか」であり、表示先の占有ではない
  - **A の production 経路のほとんどに navigation sequence が無い** (`open_fullscreen` を直接通る)
  - **B の多くにも sequence が無い** (Home/End、スタック Shift+↑↓、連結読みのシーク、
    スライドショー、native 動画の前後移動、passive snapshot クリック、remote 復元 等)
  - 反例: グリッド由来・`from_explicit_open == true` でも、linked detached の既存窓を
    更新するなら B
- **正しい判定地点**: teardown 後の `open_fullscreen` ではなく、**表示先の surface / context を
  選ぶ routing 境界**。`ViewerPresentation`、viewer session、detached runtime state、
  mounted bundle の `fullscreen_idx`、native presenter の owner を見る必要がある。
- **リワーク R2 (状態の集約) が所有する領域と重なる。**独立した作業として着手しない。
  `previous.is_none()` を viewport 側へ足す実装は §2 の症状パッチに当たる (Codex 判定)。
  条件を増やす形の部分対応も、単一の所有者を持たない routing 判断へ条件を足すことになるため
  採らない (利用者判断)。
- 現状: 中央の「読み込み中」により**無反応に見える問題は解消済み**。残るのは
  「グリッドが見えたまま待つ (黒地にならない)」という見た目の差のみ。
- 規模 / 優先度: 中〜大 / P3 (実害は見た目。R2 で routing の所有者が決まってから)。

### 1.109 複数ウィンドウ・見開き表示でページ戻りを長押しすると操作不能になる — 専用スレ >>276 ✅ v3.1.3 で出荷済み

> 修正は 2 段。`9b84c265` が「rendition sequence が active というだけで全 upload を止める」decision を
> 廃し、塗り元 (paint source) で 3 状態に分けた (`defer_ui_uploads` は後方互換の計測フィールドへ降格、
> 実判定は `ui_work_admission`)。`c3a65a6b` が、その時「別原因」として残した「detached context が
> 自前の thumbnail 結果を捨てる」を直した。**本項の登録はこの 2 コミットより後** (08-21 00:00 対
> 08-20 22:24 / 23:24) なので、調査の記録として書かれたまま完了印が付いていなかった。
> 実機確認済みで、**v3.1.3 (2026-08-21) の更新履歴に記載して出荷済み**。v3.2.0 の対象ではない。
> 以下は着手前の記録。

- 出典: 専用スレ >>276 (2026-08-20)。v3.1.2 の複数ウィンドウモードで報告。
- 報告条件:
  - 見開き表示でカーソルキーを長押ししてページを戻すと、その後操作不能になる。
  - JPEG ではかなり多く戻ったとき、WebP では数ページで発生しやすい。
  - **手元では、複数の静止画ウィンドウを開いた状態で、静止 WebP 600 枚の見開きを
    長押しで往復すると再現した**。テストデータは
    `C:\tmp\miv-spread-webp-portrait-600-20260820` (1200x1800、連番入り)。
- 状態: **再現・perf log で原因確認済み**。WebP のデコード負荷や UI thread の停止ではなく、
  通過表示用 navigation sequence と full-size upload pacing の循環待ち。
- 実ログの時系列 (`perf_events.jsonl` / `mimageviewer.log`、2026-08-20):
  1. 見開き `[415, 416]` の full-size load を開始し、静止 WebP の decode は両方とも
     約 20〜21ms で完了した。
  2. `idx=416` は `fs/ready` まで進んだが、`idx=415` は `fs_upload_backlog` に残ったまま、
     次のページ戻り入力で `[415, 416]` が navigation target になった。
  3. `idx=415` の thumbnail は `thumbnail_not_loaded` のため pass-through rendition を作れず、
     `materialized_ready=false` / `rendition_ready=false` / `still_awaiting` になった。
  4. 一方、`page_turn_decision_for_inputs` は rendition 対応 sequence が active というだけで
     `defer_ui_uploads=true` にする (`src/ui_fullscreen.rs:8832-8845`)。
     `poll_prefetch` はこの値で早期 return するため (`src/app.rs:63201-63207`)、すでに decode
     済みの `idx=415` を backlog から GPU へ載せる経路まで止まった。
  5. target が完成しないので sequence は退役できず、直前の `[417, 418]` を
     `nav_holdover` で表示したまま循環した。ログでは `UI uploads deferred` が 32768 frames、
     `no stand-in` が 131072 frames まで継続した。
- **根本原因**: rendition が未準備の navigation sequence が、target を materialize するために
  必要な upload 自体を抑止できる状態モデルになっている。見開きの片側だけ upload backlog に
  残るタイミングで自己解除不能になる。複数ウィンドウはこの順序を再現しやすくする明確な条件だが、
  共有 navigation / upload 経路の問題なので単一ウィンドウで絶対に起きないとは仮定しない。
- 期待する不変条件: 長押し中に受理したページ移動が順に処理され、キーを離した後は最後に
  表示したページで必ず settled になる。前方向 / 後方向、素材形式、先読み完了順にかかわらず、
  次の入力を塞ぐ pending / transition 状態が残らない。
- 修正方針:
  - timeout、強制 reset、repaint 追加では直さない。
  - pass-through rendition が未準備でも、対象見開きの完成済み full-size result を
    `fs_upload_backlog` から反映して materialized target を settle できる状態遷移にする。
    `rendition_sequence_active` だけで全 upload を止める現在の decision を見直し、
    rendition の待機と target materialization の所有関係を typed phase で明確にする。
  - 1 フレームあたりの upload pacing と「現在ページ + 見開き相方」の優先順位を確認し、
    片側だけ反映された直後の入力でも完成経路を失わないようにする。
  - navigation sequence、`fs_pending`、`fs_upload_backlog`、viewer context の mount / park / activate
    を同じ context owner と generation で棚卸しし、前後移動・open / switch / close / cancel / error
    の兄弟経路にも同じ循環がないか確認する。
- 回帰確認:
  - JPEG / PNG / 静止 WebP / Animated WebP、同一枚数・同一寸法で比較する。
  - 左綴じ / 右綴じ、左右キー、前進 / 後退、先読み枚数、サムネイルキャッシュ有無を振る。
  - 複数ウィンドウモードを主対象にし、フル機能ウィンドウと F12 detached を対照にする。
  - 少なくとも「target の片側に thumbnail が無い」「もう片側だけ full-size ready」「残り片側の
    full-size result は upload backlog」という実ログの順序を固定し、target が materialized で
    settle して backlog が drain されることを検査する。
  - page-turn request / accepted / materialized / settled、表示 generation、cache source、
    key level / release、active viewer context を同じ perf log で追い、同一 target の
    `still_awaiting` が無期限に継続しないことを確認する。
- 修正時の注意: 複数ウィンドウの入力 / 表示状態所有に触れるため、
  [detached-rework-plan.md](detached-rework-plan.md) §2 の禁止事項に従う。再現した症状だけに
  reset を足さず、前後移動と全素材で共有する state transition の破れを所有境界で直す。
  detached predicate / viewport 経路へ変更が及ぶ場合は ClaudeCode / Codex 双方で構造的修正と
  合意し、`detached-rework-plan.md` §11 に変更範囲と理由を記録する。
- 規模 / 優先度: Medium / P1。

### 1.110 別ウィンドウの題が、表示ページ不定のときに一覧の先頭ファイル名になる

- 出典: 2026-08-20、keep-alive backstop の所有権修正 (`detached-rework-plan.md` §11) の
  フェーズ 1 調査で Codex が指摘。**その修正とは別件なので、同時に直さず切り離した** (憲法 7)。
- 機構: detached の viewport builder は題の index を
  `self.fullscreen_idx.unwrap_or(0)` で決める ([ui_fullscreen.rs:12791](../src/ui_fullscreen.rs:12791))。
  **表示ページが定まっていない正規のギャップ** (列挙待ち、パスワード待ち、フォルダ移動の途中など) では
  `None` になり、**その一覧の先頭ファイル名**が題になる。
- 症状: 一瞬だけ、開いているものと無関係なファイル名がタイトルバーに出る。
  backstop の所有権修正で「別 context の名前が出る」方は消えたが、**こちらは残る**
  (同じ context の中で、たまたま先頭のファイル名が出る)。
- 直す方向: **`unwrap_or(0)` をやめる。** 表示ページが定まっていないことは「先頭ページ」ではない。
  ギャップ中は直前に表示していた題を保つか、アプリ名だけにするかを決める。
  **`Option` を握りつぶさずに、ギャップを型で表す**のが筋。
- 同型の入口: 静止画スナップショット窓の題
  ([app.rs:40534](../src/app.rs:40534) 付近の `detached_image_snapshot_title_for_idx`) も
  `items.get(idx)` が空振りしたとき `"mimageviewer"` に落ちる。こちらは先頭へ落ちないので
  挙動は穏当だが、**ギャップの表し方を揃えるなら一緒に見る**。
- 規模 / 優先度: 小 / P3 (実害は一瞬の見た目。所有権の方が先)。

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

### 1.114 PDF の表示解像度合わせが毎フレーム自分を殺し合ってライブロックする

- 出典: 2026-08-21 の実機確認中に遭遇 (別ウィンドウで PDF の先頭 / 末尾へジャンプ)。
  症状は左下の「PDF 再レンダリング中...」が消えず、そのページの高解像度化が永久に完了しない。
- **今回の detached 修正が原因ではない。** 同じループが `mimageviewer.log.bak`
  (2026-08-20 12:09、detached 作業の着手前) に **2051 回**記録されている。
  別の PDF / 別の日 / 同じ機構。今日のログでは 93 秒間で **15883 回**。
- **機構 (ログで確定)**:
  1. ページを読み込み、保持ページキャッシュへ `long=1179` で格納する
  2. 表示解像度合わせが `target_px=1702` を要求 → キャッシュは 1179 なので
     `reason=request_pdf_rerender/resolution_mismatch` で miss
  3. [`request_pdf_rerender_at_target`](../src/app.rs:51535) が
     **進行中の再レンダを無条件にキャンセルして**新しいリクエストを投げる
     (「既に同じ (idx, target_px) が飛んでいるか」を見ていない)
  4. 呼び出し元の `ensure_pdf_display_resolution`
     ([ui_fullscreen.rs:13897](../src/ui_fullscreen.rs:13897) / [:13906](../src/ui_fullscreen.rs:13906)) は
     **毎フレーム**評価される
- PDF のレンダは数百 ms かかるのに、リクエストは約 6ms ごとに投げ直される。
  **どのリクエストも完了する前に次に殺される。**
  ログの裏付け: `pdf rerender done` が **1 回も出ておらず**、
  spawn された全スレッドが `pdf rerender: cancelled (channel closed)` で終わっている。
- **修正方針**: 「同じ `(idx, target_px)` のリクエストが既に in-flight なら投げ直さない」判定を入れる。
  時間窓ではなく、**進行中リクエストの identity** で判定する。
  in-flight を無条件に潰す現在の実装 ([app.rs:51535](../src/app.rs:51535) 付近) がライブロックの直接原因。
  - 副次的に確認すること: 1 回でも再レンダが完了すれば miss が解消するのか。
    今回は一度も完了していないため、要求解像度 1702 を満たす結果が返るかは**未確認**。
    dedup を入れた後に、完了した結果の long がまだ target を下回るなら別の原因が残っている。
- **⚠ 着手前に調整が要る**: 2026-08-20〜21 に master 側で PDF 周辺が活発に動いている
  (`f2c88357` 同時 open 数の上限 / `c817a0d3` 列挙の相乗り / §2.13〜§2.16)。
  同じ領域を触る別作業と衝突しないか確認してから着手する。
- 規模 / 優先度: Small / P1 (利用者の目に見える固着)。**別の PDF 修正が落ち着いた後に再確認する** (利用者判断 2026-08-21)。

### 1.112 360 度動画 (正距円筒) の 360° ビュー — 利用者要望

- 出典: 利用者要望 (2026-08-21)。静止画でできる 360° ビューを動画でも。報告者は
  insta360 / GoPro の素材を持っている。
- **現状で不可能な理由は「難しい」ではなく「投影を差し込む場所が無い」**。presenter は
  デコード済みフレームを動画解像度の swap chain へ 1:1 コピーし、拡大縮小は DComp が担当する
  ([video-upscale-shader-plan.md](video-upscale-shader-plan.md) §1)。シェーダが走るのは色調 /
  Creative LUT のときだけで、それも動画解像度で走る。
- **着手条件: §1.47 (動画の拡大縮小を mIV のシェーダで行う) が入ること。** §1.47 は
  swap chain を表示矩形サイズにし、シェーダでソース解像度から表示解像度へ解決する構造にする。
  **そのステージができれば、正距円筒投影は同じステージ上のもう 1 枚のシェーダ**になる。
- §1.47 が入った後に残る作業:
  1. **シェーダ移植**: 静止画側は WGSL ([panorama_wgpu.rs](../src/panorama_wgpu.rs))、presenter は
     実行時 HLSL コンパイル ([grade_pipeline.rs](../src/video/native_presenter/grade_pipeline.rs) の
     `D3DCompile`)。投影の数式はそのまま移せる。
  2. **ミップ生成**: 静止画側はフルミップ + `textureSampleGrad` で品質を出している
     ([panorama_wgpu.rs](../src/panorama_wgpu.rs))。毎フレーム 5.7K のミップ生成が現実的かは
     **実測してから決める** (代替は品質を落とした単純フィルタ)。
  3. **入力**: 見回しドラッグと FOV を presenter 側に持たせ、シークバードラッグ / ジェスチャ /
     HUD と共存させる。報告者の「動画には拡大縮小ドラッグの処理がなさそう」は正しい観察で、
     それを作るのが §1.47。
  4. **UI 導線**: 投影方式の選択・視野角・リセットを動画 HUD から出す。§1.105 (動画の「…」) と
     関係する。
- **自動判定は可能**。FFmpeg が `AV_SPHERICAL_EQUIRECTANGULAR` を出す
  ([spherical.h](../vendor/ffmpeg/include/libavutil/spherical.h))。回転メタデータと同じ扱いにできる。
- 規模 / 優先度: Large / P3。**§1.47 待ち**。

### 1.113 動画シークバー近傍のストリップを「サムネイル列 ⇄ 音声波形」で切り替える — 利用者要望

- 出典: 利用者要望 (2026-08-21)。動画視聴中に `Z` で出る音声波形を、シークバー近傍にも出して
  波形を手がかりにシークしたい。サムネイルと併せて場所を探せると便利、という趣旨。
- **§1.102 (YouTube 型のサムネイル列) と同じ場所を使う。** 2 つの UI を競合させず、
  **1 つのストリップのモード切替**にする。§1.102 の「シークバーから上へドラッグしたときだけ開く」
  導線をそのまま共有し、開いた中身をサムネイル列と波形で切り替える。
- データ面は好条件:
  - `TimelineAnalysis` ([crates/music-core/src/analysis.rs:186](../crates/music-core/src/analysis.rs:186)、
    bins + ビートグリッド) が**動画ファイルでも `Z` 波形モードで既に生成されている**。
  - 描画も `draw_music_timeline` のラスタキャッシュがある ([ui_music_timeline.rs](../src/ui_music_timeline.rs))。
  - **タイムライン全体あたりのコストはサムネイル列より安い**。1 回のデコードで全尺分の波形が
    得られるので、§1.102 が悩んでいる抽出方式 (1 枚 1 秒 vs 50ms) の問題が無い。
- コスト面で決めること:
  - **全尺の音声デコードが要る** (背景スレッド・progressive)。長い動画では埋まるまでの時間がある。
    途中経過をどう見せるか決める (progressive partial の既存挙動を流用できるか)。
  - **永続キャッシュを持たない設計** ([app.rs:8238](../src/app.rs:8238)「永続 DB はやめて直近 N 曲だけ
    メモリに載せる」)。開くたびに解析し直す。動画でも同じでよいかを判断する。
  - **ストリップを開いたときだけ解析を起動する** (常時ではない)。現状 `ensure_music_analysis` は
    音楽ビューが有効なときだけ呼ばれる ([app.rs:38518](../src/app.rs:38518)) ので、その条件を広げる形。
    常時起動にすると、動画を開くたび全尺デコードが走る。
- 規模 / 優先度: Medium / P3 (§1.102 と同時に設計する)。

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

### 2.11 動画サムネイル中央の再生アイコンを目立ちにくくする — 専用スレ >>271 ✅ 実装済み (2026-08-22)

> `video_thumbnail_indicator` の 3 択 (`PlayIcon` 既定 / `BottomLeftBadge` / `Hidden`)。
> **既定は現在の中央アイコンのまま** (利用者確定 2026-08-22: 「既定は現在の動画再生アイコンで OK。
> 設定で変更できれば良い」)。対象確認の回答待ちは解消。
> 中央アイコンと左下バッジは `video_thumbnail_indicator_parts` が両方を返す形で**構造的に排他**。
> **音声は対象外** — サムネイルを生成せず音楽アイコンがセルの中身そのものなので、消すと空の箱になる
> (`bottom_left_content` で Video と Audio のアームを分離済み)。
> バッジの色は `FormatBadgeKind` で決める。**ラベル文字列の一致で決めない** (旧実装は未知ラベルが
> すべてアーカイブの橙に落ちるため、動画も新形式も静かに橙になる)。ラベルは `動画` — 生成中の中央
> プレースホルダと同じ語にし、1 セルに `動画` と `VIDEO` が並ぶのを避けた。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 専用スレ >>271 (2026-08-20)。動画サムネイル上のアイコンを小さくするか、
  非表示にしたい要望。
- まず、対象が動画セル中央の `▶` 再生アイコンで合っているか回答を待つ。
- 対象が合っていれば、中央の代表画像を隠しにくい角の小型表示を既定候補とし、非表示設定も
  用意する方向で検討する。既存 §2.2 の隅バッジ配置とは意味が異なる overlay なので、単に
  同じレーンへ入れず、再生可能であることの視認性と hover / 選択表示との重なりを確認する。
- スナップショットは通常動画、代表画像あり / なし、選択 / hover、狭いセル、形式 / 評価 / タグ
  バッジとの組み合わせを含める。
- 規模 / 優先度: Small / P2 (対象確認後に仕様確定)。

### 2.7 RAR の header 全走査が、一覧判定とサムネイル生成で 2 回走る

- 出典: v3.1.2 で入れた RAR 判定の逐次公開 (旧 §2.3) の caller 監査 (2026-08-19)。その brief の
  §1.4 として着手したが、範囲を超えると判断して停止条件どおり切り出した。**v3.1.2 で閉じたのは
  「全件判定を待つ表示遅延」であって、判定そのものの重複ではない。**
- 機構: フォルダ一覧の eligibility 判定が `inspect_for_direct_read_cancelable` で RAR の
  header を全走査した後、thumbnail worker の代表画像選択が
  `enumerate_image_entries_detailed` で**同じ RAR の header をもう一度全部**列挙する。
  30,000 entry 級の RAR が多いフォルダでは、この 2 回目がそのまま体感時間に乗る。
- なぜ今回入れなかったか: 1 回へ統合するには `RarInspection` が entry / 代表情報を保持し、
  thumbnail と open 双方の列挙契約を変える必要がある。逐次公開とは独立した変更で、
  同じ commit に混ぜると切り分けられなくなる。
- 直す方向:
  - `rar_loader` の判定結果が、そのフォルダ generation の間だけ代表画像選択に必要な情報も
    持つようにする。`DECISION_CACHE_CAPACITY` (32、full inspection 保持) を単に増やす修正には
    しない。メモリが増えるだけで、判定回数は減らない。
  - 統合後は **同一 RAR の header 判定回数を計装または test double で 1 回に固定する**
    (逐次公開側の完了条件として書かれていた項目をここへ引き継ぐ)。
- 計測の起点: `C:\tmp\miv-rar-thumbnail-test-100` (30,000 entry の RAR を 30 個複製)。
  v3.1.2 時点の実測は visible 12 件が 0.801〜0.860 秒、候補 #130 が 4.021 秒
  (worker 累積 3,257.5ms)。2 回目の走査を消せばここがさらに下がるはず。
- 規模 / 優先度: 中 / P2。表示は既に出るようになったので体感の急所ではないが、
  同じ I/O を 2 回やっている事実は残っている。

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

### 2.10 グリッド以外の egui ダブルクリック消費箇所に場所条件が無い

- 出典: v3.1.2 のダブルクリック対応の追補として行った `Response::double_clicked()` 全箇所棚卸し
  (2026-08-19)。egui 0.33.3 は
  前回クリック位置を持たないため、同じ context 内の離れた widget / 領域のクリックも時間内なら
  ダブルクリックとして各 consumer へ届き得る。
- 今回は利用者報告のあったグリッドセルだけを修正し、本項の consumer は変更しない。現存する
  8 分岐は、詳細表示の列幅 best-fit、360° navigator の視点移動、通常 navigator の zoom 中心移動、
  分析表示の zoom reset、通常 fullscreen の transform reset、360° canvas の視点 reset、native
  presenter egui overlay の音量 reset と再生速度 reset。
- native HWND が `WM_*BUTTONDBLCLK` から作る `double_click` は Windows 自身の時間・距離判定を
  通るため本項の対象外。着手時は一律のピクセル閾値を足さず、各 consumer の意味単位 (同じ
  handle / navigator / canvas / slider など) と、別 widget クリックで pair を切る owner を決める。
- 優先度: P2。latent 誤操作の棚卸し項目で、実害報告が出た consumer から個別に再現確認する。

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

### 2.12 サムネイルキャッシュのクリアが `-wal` / `-shm` を残す — 実測 1.4 GB

- 出典: 2026-08-20、利用者がキャッシュクリア後に計測しようとして発見。
- **実測** (`%APPDATA%\mimageviewer\cache` 配下、クリア + 再起動後):

  | 種別 | 件数 | サイズ |
  | --- | ---: | ---: |
  | `.db` (本体) | 2 | 0.2 MB |
  | `.db-wal` | 1,410 | **1,394.8 MB** |
  | `.db-shm` | 1,410 | 44.1 MB |

  サンプルした `-wal` はすべて**対応する `.db` が存在しない孤児**だった。
- 機構: カタログは `cache/<hash 先頭 2 桁>/<hash>.db` (`catalog.rs` の `db_path_for`)。
  クリアが**その `.db` だけ**を消していると思われる。SQLite は正常な close で `-wal` / `-shm` を
  自分で削除するので、**閉じずにファイルを消すと残る**。
- 実害:
  1. **容量が解放されない。**「クリアしたのにディスクが空かない」= クリアがほぼ無意味に見える。
  2. 同じフォルダを再訪すると、その path に新しい `.db` が作られる一方、古い `-wal` が残っている。
     SQLite は salt 不一致の WAL を無視するので即座の破損は起きにくいが、**消したはずの物が残った
     状態で新規 DB を作る**のは設計として健全でない。
- 直す方向: **削除前に接続を確実に閉じる**のが構造的に正しい (閉じれば SQLite が自分で消す)。
  閉じられない経路があるなら、その理由を確認してから `-wal` / `-shm` の明示削除を足す。
  **どの経路が `.db` を掴んだまま消しているのかを先に特定する** (症状として `-wal` を消して回らない)。
- 既存の 1.4 GB は、アプリ終了後に `cache` 配下の孤児 `*.db-wal` / `*.db-shm` を削除すれば安全に回収できる。
- 規模 / 優先度: 小〜中 / P2 (実害は容量。利用者が明示的に「クリア」を選んだのに効いていない)。

### 2.13 PDF は 1 ページ描くたびに文書を開き直している

- 出典: 2026-08-20、PDF ワーカーのレーン容量を直した後、並列度より効く可能性がある側として確認。
- **機構**: `core_render_with_count` ([pdf_loader.rs:1303](../src/pdf_loader.rs:1303))、
  `core_analyze_page` ([1276](../src/pdf_loader.rs:1276))、`core_get_info` ([1165](../src/pdf_loader.rs:1165))、
  `core_enumerate` ([409](../src/pdf_loader.rs:409)) が、**要求ごとに** `load_pdf_from_file` を呼ぶ。
  子ワーカーは `Pdfium` インスタンスは持ち越すが ([1000](../src/pdf_loader.rs:1000) 付近)、
  **開いた文書は持ち越さない**。
- 実害の見積り (未実測):
  1. 1 冊を 10 ページ描くと **10 回開き直す**。開くたびに xref / trailer を読み直す。
  2. ワーカーは 5 個あり、**同じ 1 冊の別ページを並列に描くと、同じ文書を同時に 5 回開く**。
     HDD では xref (ファイル末尾) と本文 (途中) の間で seek が往復する。
  3. ページ数が多い本ほど不利。フルスクリーンのページ送りは同じ文書への連続要求なので、
     いちばん効く場所で毎回捨てている。
- **ファイルハンドルは保持される**: `load_pdf_from_file` は `File::open` したハンドルを
  文書の生存期間ずっと持つ (`FPDF_LoadCustomDocument` に reader を渡す実装)。
  ただし **2026-08-20 に実測した結果、掴んだままでも `rename` / `remove_file` /
  シェル経由のゴミ箱移動はすべて成功する** (Rust の `File::open` は `FILE_SHARE_DELETE` を
  含めて開くため)。動画デコーダが共有違反を起こす件 ([app.rs:26870](../src/app.rs:26870) の
  コメント) とは事情が違う。**掴むことを理由にした解放タイマーは要らない。**
- **本当の危険は ABA**: 削除・差し替えの後もワーカーは古い文書を握り続けるので、
  同じパスに別のファイルが来たら古い中身を返す。
- 直す方向: **ワーカープロセスごとに「直前に開いた 1 冊」だけを保持する。**
  キーは path だけでなく **(path, パスワード, mtime, サイズ)** にし、**要求ごとに stat して
  確認する** (stat は開き直しに比べて無視できる。mtime / file_size は enumerate の応答が
  既に運んでいるので、mIV 内の PDF 同一性判定として一貫する)。
  判定は純関数 (前回のキー × 今回のキー → 再利用 / 開き直し) に切り出せば PDFium 無しで
  テストできる。**時間窓は使わない。**
- アイドル時のメモリ (見終わった本を各ワーカーが抱え続ける) が問題になるかは**先に実測する**。
  効くなら時計ではなく既存の決定的な信号 (フォルダ移動時の `bump_render_context_epoch`) に
  載せて解放する。
- 実装上の引っかかり: pdfium-render の `load_pdf_from_file` は `password: Option<&'a str>` と
  `&'a self` が同じ寿命なので、文書を長く持つとパスワード文字列も同じだけ生かす必要がある
  (PDFium 自身はコピーするので API 側の過剰制約)。パスワード付き PDF のために小さな回避が要る。
- 着手順: **in-process フォールバック削除の後**。`core_*` の呼び出し元が 2 系統から 1 系統に
  なり、この分割の手数が半分になる。
- **測る先**: 現在の perf イベントは main プロセス側の queue / dispatch だけで
  ([pdf_loader.rs](../src/pdf_loader.rs) の `crate::perf::event` は全て pool 側)、
  **ワーカー内部の「開く」と「描く」は分離して測れていない**。
  先に子ワーカー側へ計装を入れて、開くコストの実測を取ってから直す。
- 規模 / 優先度: 中 / P2 (ワーカー数の調整より効く可能性がある。先に実測する)。

### 2.14 PDF のフルスクリーンが PDFium 経路を 2 本持っている

- 出典: 2026-08-20、ワーカー起動失敗の調査 (フェーズ 1) で判明。
- **機構**: 同じフルスクリーン表示なのに、初回ページと再レンダリングで別の PDFium を使う。

  | | 初回ページ | 再レンダリング |
  | --- | --- | --- |
  | 入口 | `render_page_for_display` ([app.rs:50276](../src/app.rs:50276)) | `render_page_async` ([pdf_loader.rs:3502](../src/pdf_loader.rs:3502)) |
  | 実行場所 | ワーカープロセス 5 個 | **メインプロセス内の 1 スレッド** |
  | Critical 予約 | 効く | 効かない |
  | レーン分割・昇格 | 効く | 効かない |
  | context epoch の間引き | 効く | 効かない |

  再レンダリングはズームだけでなく、`ensure_pdf_display_resolution` による表示解像度合わせ
  (現ページ + 見開き相方、[ui_fullscreen.rs:13561](../src/ui_fullscreen.rs:13561)) も通る。
  つまり**通常の PDF 閲覧が日常的にこの経路を使う**。
- **`render_page_async` はフォールバックではない**。`worker_count` を見ずに常に
  `get_worker()` を使う専用経路で、呼び出し元は [app.rs:50765](../src/app.rs:50765) の 1 つだけ。
  ワーカー起動失敗の対応 ([briefs/pdf-worker-startup-failure.md](briefs/pdf-worker-startup-failure.md))
  では**意図的に残した**。
- 移すかどうかの論点 (2026-08-20 に整理):

  **ズーム連打では並列化は効かない。** 新しいズーム要求は前の要求をキャンセルする
  (`fs_pending.remove(&idx)` → `cancel.store(true)`、[app.rs:50710](../src/app.rs:50710) 付近) ので
  キューに積み上がらず、無駄になるのは実行中の 1 件だけ。その実行中の PDFium レンダリングは
  **プールでも中断できない** (`CancelWaitPolicy` は結果の扱いであって PDFium を止める話ではない)。
  要求は互いに上書きするので、並列に走らせても欲しいのは最新の 1 件だけ。

  **効く可能性があるのは 2 ケース:**
  1. **AI 先読みのレンダリング中にズームしたとき。** in-process は `priority_rx` を先に drain
     するが、それはキューの順序を変えるだけで、**既に走っている非 priority の先読みが
     終わるまで待つ** (実測で 1.5 秒級)。プールなら先読み = Normal、ズーム = Critical で、
     予約により最低 1 ワーカーが空いているので即座に始まる。
  2. **見開きの左右ページ。** 2 ページとも必要な独立した処理なのに今は 1 スレッドで直列。
     Critical は lane cap の対象外なので、プールなら同時に走れる。

  **払うコスト: ビットマップの IPC 転送。** ワーカーの応答は**無圧縮の生 RGBA**
  ([pdf_loader.rs:1219](../src/pdf_loader.rs:1219))。ズーム再レンダリングは長辺 8192px まで
  行くので、A4 比なら 8192 × 5794 × 4 ≒ **190 MB をパイプに流す**。in-process は mpsc で
  ポインタを渡すだけ。プールが `serialize_us` / `write_us` / `wire_bytes`
  ([pdf_loader.rs:494](../src/pdf_loader.rs:494)) を計測しているのは、このコストが効くから
  (初回ページも 4K ビューポートで約 33 MB 払っている)。

  | | 得るもの | 払うもの |
  | --- | --- | --- |
  | プールへ移す | 先読み実行中の待ち解消、見開きの並列化 | ズーム解像度で最大 190 MB の直列化 + パイプ転送 |

- **有力な第 3 案: in-process にいる 2 つを分ける** (2026-08-20 追記)。
  `render_page_async` を使っているのは性質が正反対の 2 つで、まとめて扱う理由が無い。

  | 用途 | 呼び出し元 | 性質 |
  | --- | --- | --- |
  | ズーム / 表示解像度合わせ | `request_pdf_rerender(.., true)` ([ui_fullscreen.rs:6960](../src/ui_fullscreen.rs:6960)) | **利用者操作に同期。低遅延が要る。最大 8192px と巨大** |
  | AI 先読み用の native 再レンダ | `prefetch_final_ai` → `request_pdf_rerender(idx, 1.0, false)` ([app.rs:52845](../src/app.rs:52845)) | **完全にバックグラウンド。遅延は問題にならない** |

  なお**通常のページ先読みは既にプール側**なので競合しない
  (`ensure_fs_page_load` → `start_fs_load` → `render_page_for_display`、
  [app.rs:56551](../src/app.rs:56551))。in-process にいるのは上の 2 つだけ。

  **AI 用 native 再レンダだけをプールへ移す**と:
  - ズームは in-process のまま = レスポンス劣化の懸念が無い
  - in-process スレッドがズーム専用になり、**先読みに塞がれなくなる** (= 上の「効く 2 ケース」の
    ①がプール移行なしで解消する)
  - IPC 転送コストは AI 側が払うが、そちらは元々バックグラウンド
  - 見開き並列 (②) は解決しない

  **①が主因だと実測で分かったら、この案を先に検討する。**

- **測るもの** (上の表がそのまま測定項目になる):
  1. **先読み実行中のズーム要求の待ち時間** — in-process (先読み完了まで待つ) vs
     プール (予約ワーカーで即開始)。
  2. **ズーム解像度での IPC 転送コスト** — 既存の `WorkerRenderMetrics` がそのまま使える。
     長辺 2048 / 4096 / 8192 で `serialize_us` + `write_us` を取る。
  3. **見開き 2 ページの完了までの時間** — 直列 vs 並列。

  1 と 3 の得が 2 の損を上回るかで決める。§2.13 の計装と同時に入れると安い。
- 規模 / 優先度: 中 / P3 (現状で壊れてはいない。実測して有意差が出たときに動かす)。

### 2.15 PDF: 利用者が待っている open を、背景の一斉 open に埋もれさせない ✅ 実装済み (2026-08-21)

> §2.16 と同じ 1 つの機構で実装した。設計と経緯は
> [briefs/pdf-open-admission.md](briefs/pdf-open-admission.md)。
> **実機確認済み (2026-08-21)**: 「サムネイルがまだ出ていない状態でも PDF ページを開ける」。
> 同じ本のページ送りの速度に変化なし。以下は着手前の記録。

- 出典: 利用者報告 (2026-08-20)。「複数ウィンドウを開こうとしているとき、ダブルクリックで
  画像がなかなか開かない」。3 回クリックし、待ちが 1.5 → 2.2 → 3.7 秒と積み上がった。
- **実測** (利用者実機の perf log、`worker_open_ms` を分離したので分かった):

  | 全 render の `worker_open_ms` | p50 | p90 | p99 | 最大 |
  | --- | ---: | ---: | ---: | ---: |
  | | **0.3 ms** | 763 ms | 5,730 ms | **10,803 ms** |

  1 秒超の open が 75 件あり、**固まって発生**している:

  ```
  t=11.9 open=5744ms  20230103_008.pdf     t=13.8 open= 7311ms 20230103_010.pdf
  t=12.4 open=6252ms  20230103_001.pdf     t=16.5 open=10314ms 20230103_009.pdf
  t=13.1 open=6955ms  20230103_005.pdf     t=17.0 open=10803ms 20230103_006.pdf
  ```

  **別々の PDF を一斉に開いている区間** (フォルダのカバーサムネイル生成)。対象ファイルは
  HDD 上にあり、5 ワーカーが同じディスクを取り合う。
- **重要**: 遅いのは**ファイルではなく状況**。同じ `20221030_001.pdf` は他の時点で
  **0.4 ms** で開いている (数十サンプル)。当初「病的なファイル」と誤診したが、
  利用者の指摘で確認して訂正した。
- **Critical レーンの予約はワーカーを 1 つ確保するだけで、ディスクは確保しない。**
  他の 4 ワーカーが大きな PDF を読んでいる間、予約されたワーカーも同じディスクを待つ。
- 直す方向: **前面の PDF open が待っている間、背景 (カバーサムネイル生成・先読み) の open を
  走らせない。** スクロール中の先読み抑制 (`decide_prefetch_allowed`、
  [prefetch-suppression-during-scroll-plan.md](prefetch-suppression-during-scroll-plan.md)) と同じ形。
  **時間窓ではなく「前面が待っているか」で決める。**
- **設計原則 (利用者、2026-08-21)**: **利用者の直接の操作に応答することが最優先**。
  資源が競合したら、背景仕事より前面を通す。
- 規模 / 優先度: 中 / P2。
- 関連: §2.16 (同時 open 数の上限)。両者は補完関係で、こちらは**優先度**、あちらは**総量**。

### 2.16 PDF: open と render を別の資源として扱う (同時 open 数の上限) ✅ 実装済み (2026-08-21)

> 実装は `MSG_OPEN` を新設して open を独立した IPC 要求にする形になった。当初案の
> 「job 単位で許可枠を数える」は、枠が render まで覆って SSD の throughput を落とすため
> 採らなかった。詳細は [briefs/pdf-open-admission.md](briefs/pdf-open-admission.md) §3.1。
> 以下は着手前の記録。

- 出典: 2026-08-21、§2.15 の議論から。利用者提案「PDFium 並列度 10、同時 open は 3 まで」。
- **実測が支持している**: 7.4 秒かかった要求の内訳は

  ```
  worker_open_ms   = 7371.8   ← 競合を全部吸収している
  worker_page_ms   =   11.5
  worker_render_ms =  154.9   ← 平常時 (100〜170ms) と変わらない
  ```

  **競合の最中でも描画時間は動かない。** つまり:
  - **open はディスク律速** — HDD で同時に走らせるとシークが往復して待ちが跳ねる
  - **render は CPU 律速** — 並列度を上げた分だけ効く

  **性質が正反対のものを、同じ並列度で縛っているのが現状の構造。**
- 直す方向: **render の並列度は保ったまま、同時 open 数だけを上限で絞る。**
  - **親側の dispatch で絞る** (ワーカー内部で待たせない)。親はどのワーカーへどの文書を
    送ったかを知っているので、**「そのワーカーが保持している文書と違う要求 = open が要る」と
    予測できる**。§2.13 で入れた文書キャッシュがこの予測を成立させている。
    ワーカー間の名前付きセマフォのような仕掛けは要らない
  - **ストレージ種別を自動判定しない。** HDD / SSD を実行時に見分けて挙動を変えるのは
    「実行時状態で挙動を変えない」方針に反する。**固定値か設定値**にする
- 規模 / 優先度: 中 / P2 (dispatcher の資源管理に手を入れるので、出荷直前には入れない)。
- 関連: §2.15 (前面優先)、§2.13 (文書キャッシュ)。

### 2.17 フォルダ代表サムネの既定ソートが一覧の既定と食い違う (番号順 vs ファイル名順) — 利用者報告 ✅ 実装済み (2026-08-22)

> `default_folder_thumb_sort` を `FileName` にし、`schema_meta.folder_thumb_sort_default_v2` marker による
> 一度きりの移行を追加。**値と marker は同じ transaction で commit する** (marker だけ残ると次回起動が
> 「移行済み」と判断して番号順のまま取り残されるため)。**移行の失敗は致命的にしない** — ログを残して
> 保存済み設定のままロードし、marker が立たないので次回再試行する (load 全体を失敗させると
> `FailedFallbackDefault` でそのセッションが既定設定 + save 抑止になり、全利用者が 1 回だけ通る移行の
> 代償として重すぎる)。`natural_sort_key` は変更していない。3.2.0 の `version_highlights` で戻し方を告知。
> **実機確認は未実施。** 以下は着手前の記録。

- 出典: 2026-08-22。`00表紙.jpg` / `00表紙2.jpg` の 2 枚だけが入ったフォルダで、一覧の先頭は
  `00表紙.jpg` なのに、フォルダタイルの代表サムネだけ `00表紙2.jpg` になるという報告。
- **どちらも既定値のまま**で起きる (利用者が設定を変えたせいではない)。既定が 2 つに分かれている:

  | 設定 | 既定 | 定義 |
  | --- | --- | --- |
  | 一覧の並び順 `sort_order` | ファイル名順 | [settings.rs:5388](../src/settings.rs:5388) (`SortOrder::default()` = `FileName`)、固定テスト [settings.rs:9564](../src/settings.rs:9564) |
  | 代表画像の選択基準 `folder_thumb_sort` | 番号順（区切り無視） | [settings.rs:5126](../src/settings.rs:5126)。導入コミット `e6793dbb` (2026-04-12) から不変 |

- 機構: 代表画像の自動選定は [thumb_loader.rs:2349](../src/thumb_loader.rs:2349)
  `resolve_folder_thumb_image_inner`。**一覧の `sort_order` は見ず** `folder_thumb_sort` で
  並べて先頭 1 枚を採る。実ログでも確認済み:
  `resolve_folder_thumb_image: ... sort=Numeric depth=3 -> ...\00表紙2.jpg` /
  保存キー `folderthumb:auto-v2:numeric:d3:...`。ピン (`folder_thumb_pins.db`) は無関係。
- 番号順で反転する理由: `natural_sort_key` ([ui_helpers.rs:979](../src/ui_helpers.rs:979)) は
  記号・空白を捨てて数字塊 / 文字塊に割るが、**拡張子も直前の文字塊に溶ける**。
  - `00表紙.jpg` → `[数字0, "表紙jpg"]`
  - `00表紙2.jpg` → `[数字0, "表紙", 数字2, "jpg"]`

  2 要素目が `"表紙" < "表紙jpg"` (前方一致で短い方が小) なので、**数字が付いている方が勝つ**。
  `cover.jpg` vs `cover2.jpg`、`a.jpg` vs `a10.jpg` も同型 (連番同士 `001` / `002` は期待どおり)。
  ファイル名順 (Windows ソートキー) なら `00表紙.jpg` が先頭になる (`LCMapStringEx` の
  ソートキー比較・`StrCmpLogicalW` の双方で確認)。
- **直す方向 (利用者判断済み)**: 既定をファイル名順に合わせる。ただし `default_folder_thumb_sort`
  を変えるだけでは**新規インストールにしか効かない**。[settings_db.rs:1550](../src/settings_db.rs:1550)
  `write_settings_kv` が Settings の全フィールドを毎回書くので、既存利用者の `settings_kv` には
  全員 `folder_thumb_sort = "Numeric"` が入っており、**「保存されている」= 「利用者が選んだ」とは
  判別できない**。そこで更新時に一度だけ移行する:
  1. `schema_meta` に一度きりのフラグ (例 `folder_thumb_sort_default_v2`) を置く。既存の
     `bootstrap_complete` / `migrated_from_json_at` と同じ `INSERT OR IGNORE` の形で足せる。
  2. フラグが無い既存 DB では、起動時に一度だけ `folder_thumb_sort` を `FileName` へ書き換えて
     フラグを立てる。クリーンインストールは bootstrap 時にフラグだけ立て、値は触らない。
  3. 副作用: 意図して「番号順」を選んでいた利用者も 1 回だけ戻る (上記のとおり区別できない)。
     §5.0 経由で `version_highlights` の `must_read` に載せ、戻し方 (環境設定 → フォルダ・ファイル →
     「代表画像の選択基準」) を明示する。
  4. 代表サムネのキャッシュキーにソート順が入る (`...:numeric:...` → `...:filename:...`) ので
     選び直しは自動で走る。旧キーのエントリが残って容量だけ増えないかは実装時に確認する。
  5. マニュアルの既定表記 ([settings.html:470](../htdocs/mimageviewer/manual/settings.html:470)) を
     同時に更新する。
- 実装前に決める枝: 「番号順の natural key から拡張子を除く」案でも
  `[0,"表紙"] < [0,"表紙",2]` となり番号順のまま期待どおりになる。ただし `natural_sort_key` は
  一覧ソート ([app/folder_scan.rs:132](../src/app/folder_scan.rs:132))、ファイル名スタック、
  ZIP ツリー、スマートフォルダが共有しており、**番号順を選んでいる全一覧の並びが変わる**。
  既定の食い違いを消すだけなら上の移行案の方が影響範囲が小さい。
- 規模 / 優先度: 小 (既定値 + 一度きりの移行) / P2。

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

### 3.3 ページ送り中の AI アップスケールを待たない / 打ち切る

- 出典: 元画像プレビュー中の見開き 1 ページずらし停止 (2026-08-17 修正済み) の実 perf log。
  停止中に AI upscale 1.3s × 3 回が
  逐次実行され、5.22s / 4.30s の停止時間の大半を占めた。
- その修正は「元画像を描く frame が加工済み source を readiness として待つ」不整合だけを直した。
  AI コストそのものは変更しておらず、元画像表示以外でも通過先 / stale target の推論待ちが起こり得る。
- 方針: ページ送りで表示 target から外れた AI upscale を待たず、context-owned producer の cancel / 打切りと
  着地点だけの再開を ownership・generation・完了回収まで含めて設計する。時間窓や `Awaiting` の強制解除、
  AI 結果の silent fallback では直さない。
- 規模 / 優先度: 中 / P2 performance。

## 4. 入力カスタマイズ / マウス / ゲームパッド

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
- 補足: マスク編集モードで筆系ツールを選び、キャンバス上にカーソルがある場合だけの
  `Shift+ホイール` は筆半径変更の固定入力として別途実装する。一般の入力経路や上記互換フィールドへ
  接続せず、パネル上・筆以外・動画では消費しないため、本項の横断カスタマイズ再設計を再開したものではない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

### 4.5 非アクティブなウィンドウでは右クリックのリングが効かない — 利用者報告 ✅ 解決済み (§1.100 の修正による)

> 下記のとおり §1.100 と同一原因。§1.100 は実機確認済み (`8db282e3` + `5b83df3f` / `d2c19796` /
> `c71d8c08` + `4c5260a2` + `12f60c97`) なので本項も解決済み。経緯の記録として残す。

- ⚠ **§1.100 と同一症状であることが 2026-08-21 に確定した** (同じ 1 つの原因)。
  本項が持っていた追加事実 2 点 (「**アクティブにすれば動く**」= 非アクティブ窓でのみ効かない /
  「App グローバル状態が塞いでいる」仮説の**反証**) は §1.100 へ移した。
  **修正・仕様判断はすべて §1.100 側で行う。本項は経緯の記録として残す。**

- 出典: 2026-08-20 の利用者報告。複数ウィンドウ表示中、**新しいウィンドウを開くと前のウィンドウで
  リングが効かない**。追加確認で「**アクティブにすれば動く**」= 非アクティブ窓では効かない、と判明。
- **調査で否定された仮説**: `ring_picker` / `mouse_ring_flick` / `mouse_gesture` は App グローバルで
  bundle swap 対象外なので、片方の窓が置いた状態がもう片方を塞ぐ、と考えた。**塞がっているなら
  アクティブにしても効かないはずなので、この説明は成り立たない。**
- 実際の挙動: 非アクティブ窓では、**最初のクリックがウィンドウのアクティブ化に使われている**と
  見られる。mIV は detached ウィンドウに active / passive の概念を持ち、passive 側は egui の
  キーイベントからアクティブ化を判断する作りになっている。
- **これは不具合というより仕様の選択**。利用者の期待は「見えている別ウィンドウに直接ジェスチャしたい」で、
  現行仕様と食い違っている。
- 扱い: detached リワークの **R2 (状態の集約)** が扱う領域。BA-7 (App グローバル状態の per-window 化) に
  隣接する。**症状パッチ (非アクティブ窓でも右クリックを通す guard) を入れない。**
- 併せて記録: 上記のとおり `ring_picker` / `mouse_ring_flick` / `mouse_gesture` /
  `mouse_ring_grid_target_idx` / `mouse_ring_nav` は**現在 App グローバルで per-window 化されていない**
  (`analysis_mode` など 223 個は swap 対象)。今回の症状の原因ではないが、**複数ウィンドウでの
  リング状態の所有者は未整理**であり、R2 で扱う対象。
- 規模 / 優先度: 仕様判断が先 / P3。

## 5. リリース前確認 / 依存更新

> 版ごとに実際に取った測定値 (perf smoke / idle health / bench / 依存確認) は
> [release-verification-records.md](release-verification-records.md) にある。ここには
> **手順そのものの未解決点**だけを置く。

### 5.0 次版の「重要な変更点」に載せるもの

[src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ書く候補。
**既定の挙動が変わったもの**は `must_read` に入れる (更新後初回起動で自動表示される。
ここに載せないと利用者に伝わらない)。載せたらこの節から消す。

| 変更 | 区分 | 入った版 |
| --- | --- | --- |
| (なし) | | |

v3.2.0 ぶんは [src/version_highlights.rs](../src/version_highlights.rs) の `TABLE` へ記載済み
(必読 3 件 = 代表画像の既定 / バケツの塗り 1px / 消しゴムの色調合わせ、新機能 7 件)。

### 5.11 v3.2.0 出荷前確認の記録 (2026-08-23)

**動画アップスケール (`video-upscale-shader`) をマージした後の最終ビルドに対する記録。**
マージ前のビルドに対する旧記録はこれで置き換えた。

配布ビルド: `build-dist.ps1` (全体テスト込み) → 署名の 1 本目で
SimplySign のクラウド鍵セッション切れにより失敗 → 再ログイン後
`build-dist.ps1 -SkipRustTests` で完走 (同一ソースでゲート通過済みのため但し書きに合致)。

| 項目 | 結果 |
| --- | --- |
| Rust 全体テスト | ✅ 失敗 0 件 (`lib` 単体でも 6,200 passed)。**フルゲート中に mIV を操作すると `ui_fullscreen::tests` が 5 件落ちる**ので触らないこと |
| CI (GitHub Actions) | ✅ 緑 (`f053625d`)。**`build.rs` の fxc 呼び出しが非 Windows で問題ないことを実証**。`native_presenter` が `#[cfg(windows)]` なので生成テーブルを `include!` する経路が Linux に無く、`find_fxc` も `Ok(None)` になる |
| コード署名 | ✅ 単体exe / setup.exe / portable の 3 種とも `Valid` + RFC3161。内包 vendor PE (pdfium / onnxruntime / FFmpeg 6 本) も `Valid` |
| CRT 静的リンク | ✅ `VCRUNTIME140.dll` / `MSVCP140.dll` 依存なし |
| PDFium | ✅ 最新 (chromium/8009) |
| FFmpeg | ⏸ 4 コミット新しい版あり (`n7.1.5-12-g1fdbca85aa` → `-16-g9a4bb2c579`)。**見送り** — 同じ 7.1.5 系で必要な修正が特定できておらず、更新すると LGPL 対応ソースの再掲と製品ページの手書き節の更新が伴う |
| idle health: static-foreground | ✅ PASS (完全 sleep、`matching_session_events: 1` で同一 session 確認済み) |
| idle health: static-background | ✅ PASS (同上) |
| idle health: tray-residency | ⏭ 本ビルドでは未実施。マージ前ビルドでは PASS したが**軽い条件のみ** (`evidence_floor` が起動時のままで、サムネイル読込中に閉じた状態では測っていない) |
| idle health: video-pin-background | ⏭ 未実施 (waiver)。動画を代表画像に固定し、かつキャッシュから読み直される状態のフォルダが必要 |
| ポータブル版 smoke | ✅ 実機確認済み |
| 「重要な変更点」表示 | ✅ `--whatsnew-from 3.1.3` で**必読 4 + 新機能 8** を確認 |
| 機能の実機確認 | ✅ 製本の復元抑止 (出ないこと + Explorer コピーでは出ること)、動画アップスケール、復元モーダル / ESC、動画バー固定、設定の移動先 |

**見送り (実施不要と判断)**: 検索ベンチ回帰 (本版で検索未変更)、perf smoke。

**次回への申し送り**: `Assert-MivSignReady` は証明書ストアの存在確認だけなので、
SimplySign のクラウド鍵セッションが切れていても通過する (証明書の公開情報だけが残り
秘密鍵が外れるため)。**ビルド 40 分を消費してから署名で落ちる。**
使い捨ての PE を 1 本実署名してから配布ビルドに入れば数秒で分かる。
事前チェック自体をその形にする改善は §5.12。

### 5.12 署名の事前チェックが、鍵の使えなさを検出できない

- 出典: v3.2.0 の配布ビルド (2026-08-23)。`Assert-MivSignReady` を通過したのに、
  実署名の 1 本目 (`vendor/pdfium/bin/pdfium.dll`) が
  `SignTool Error: No certificates were found that met all the given criteria.` で失敗した。
- 機構: SimplySign のクラウド鍵セッションが切れると、**証明書の公開情報は
  `CurrentUser\My` に残ったまま秘密鍵だけが外れる**。`Assert-MivSignReady` は
  証明書の存在を見るだけなので通過してしまう。
- 実害: `build-dist.ps1` は clean からビルドしてから署名するので、
  **40 分ビルドしてから落ちる**。しかも中途半端な成果物は残らないので、
  丸ごとやり直しになる。
- 直す方向: 事前チェックを「証明書が見えるか」から「**実際に署名できるか**」へ変える。
  使い捨ての小さな PE を 1 本コピーして `Invoke-MivSign` し、成功したら消す。
  数秒で済み、セッション切れならビルド前に分かる。
  - 署名対象を汚さないよう、必ず一時ディレクトリのコピーに対して行う。
  - タイムスタンプ取得でネットワークへ出るので、失敗理由を
    「鍵が使えない」「TS サーバに届かない」で区別できると望ましい。
- 回避策 (現状): 配布ビルドの前に手で 1 本署名してみる。
- 規模 / 優先度: 小 / P2 (リリースのたびに 40 分を失うリスク)。

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
- **quick-xml の非推奨 API 追随** (v3.0.0 で 0.39 → 0.41、advisory 対応)。
  `src/xmp_reader.rs` の 4 箇所が非推奨警告を出す:
  `decode_and_unescape_value` → `decoded_and_normalized_value`、
  `unescape_value` → `normalized_value`。
  **0.41 では非推奨側が新実装へ委譲済み**なので、いま呼んでいる限り挙動は
  `normalized_*` と同じ (= XML 仕様どおり属性値の改行 / タブが空白になる)。
  影響するのは**属性値だけ**で、`xtw:*` (ツイート情報の表示用テキスト) と
  `rdf:resource` / `xmp:Rating` / GPano (いずれも URI か数値) にとどまる。
  **タグ (`dc:subject`) は `Event::Text` 経由**で非推奨メソッドを通らないため無関係。
  次の更新で削除される前に呼び出しを置き換える。

### 5.9 リリース手順: 配布ビルド前に core / remote の存在が要る

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`scripts/test-full.ps1` が
  **テストを 1 件も実行しないままビルドエラーで落ちた**。
- 原因: `--workspace` に launcher package が入り、その build.rs が内包対象の
  `target\release\mimageviewer-core.exe` と `mimageviewer-remote.exe` の存在を検査する。
  remote は v3.0.0 で増えたので、それ以前の成果物しか無い環境で初めて表面化した。
- `build-dist.ps1` は test-full を `cargo clean` の**前**に回すため、直前のビルドが
  残っている通常のリリース機では起きない。新しい clone や `cargo clean` 直後に起きる。
- 当座の回避と前提は CLAUDE.md「開発中のビルド・テスト選択」に記載済み。
- 直す方向: test-full が不足分を先に build するか、launcher を `--workspace` の
  テスト対象から外す (launcher 自身に unit test があるかを確認してから決める)。
- 規模 / 優先度: 小 / P3。

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

### 1.85 vendored egui-wgpu のテクスチャ配送に回帰テストが無い

- 出典: v3.0.0 出荷前のクラッシュ修正 (2026-08-14、`ce6616ef`)。
  `paint_and_update_textures` が surface 無し viewport で早期 return し、
  **`textures_delta.set` を丸ごと捨てていた**問題を直したが、**テストは入れていない**。
- **この境界は過去 3 回、別の症状で表面化している**:
  1. サムネイルが純黒で固着 (v1.8.0 回帰) → `poll_thumbnails` 側で resync 窓中の
     upload を先送りする回避
  2. font atlas の `Y 29..44` パニック → `set_fonts` 最大 5 世代リトライの resync
  3. font atlas の `Y 45..126` パニック (今回) → **リトライ 5 世代を回りきって落ちた**。
     リトライ回数では防げないことの実証
- 欲しいテスト (Codex 案):
  1. `Managed(0)` が初期 32px 高
  2. **surface が無い状態で** 128px 以上の full 置換を submit
  3. surface 復帰後に `y=45..126` の partial を submit
  4. full 置換が共有 renderer に届いており validation error が出ないこと
- 障害: vendored egui-wgpu に既存のテスト基盤が無く、GPU device が要る。
  `egui_kittest` の wgpu 経路 (tests/ui_snapshot.rs) が最も近い足場。
- **回避策を撤去するかの判断もここに含める**: 上記 1 の upload 先送りと 2 の resync は、
  境界が直った今は保険であって正しさの担保ではない。撤去は別レビューで
  (今回は同時に触らないと決めた)。
- **2026-08-16 の確定**: §1.31 の第 1 段として着手する (順序は §1.31 参照)。
  ブリーフ = [codex-texture-delivery-test-brief.md](briefs/codex-texture-delivery-test-brief.md)。
  - 足場は `egui_kittest` **ではなく** `vendor/egui-wgpu` の in-crate headless unit test。
    `RenderState::create` の `compatible_surface` が `Option` で全フィールドが `pub` なので
    surface 無しの device / renderer を作れる。`ui_snapshot` 実行体の既知の間欠 AV を避ける。
  - **上記の欲しいテスト 4 は exit 2 (no-surface) しか到達しない**。`surfaces` が空なら
    `get_current_texture` に届かず、`on_surface_error` はエラーを分類するだけで注入できない。
    `RecreateSurface` / `SkipFrame` の coverage は §1.86 で typed outcome の seam を作ってから。
    → 前半を **§1.85-A** として切り出した。
  - **判定は `Renderer::texture_size` の observable な値にする**。mIV の overflow guard が
    範囲外 partial を wgpu へ渡す前に skip するため、「validation error が出ない」だけでは
    full 置換が落ちていてもテストが通ってしまう。
  - `vendor/egui-wgpu` は workspace `exclude` かつ `autotests = false` なので、
    `scripts/test-full.ps1` に専用実行を足さないと**テストが存在するだけで走らない**。
- **2026-08-16: §1.85-A (前半) 完了**。
  `vendor/egui-wgpu` の DX12 headless in-crate unit test
  `paint_and_update_textures_delivers_set_and_free_without_surface` で、surface 無しのまま
  32px seed → 128px full 置換 → `y=45..126` partial を渡し、主判定の
  `Renderer::texture_size` が 128px を保持することと validation error scope が空であることを固定した。
  別 texture の no-surface `free` も `Renderer::texture()` の消失で検査する。
  vendor manifest に明示的な lib test target を設定し、`scripts/test-full.ps1` の専用
  `--manifest-path` 段から実行される。production 挙動と `src/` 配下の既存回避策は変更していない。
- **残り**: exit 3/4 (`RecreateSurface` / `SkipFrame`) の `free` 配送は §1.86 で
  typed `PaintOutcome` seam を作ってから検査・修正する。本項前半からは手を伸ばさない。
- 規模 / 優先度: 中 / P2。

### 1.86 surface 取得に失敗したフレームで `textures_delta.free` が捨てられる

- 出典: v3.0.0 出荷前の font atlas 調査 (2026-08-14) で Codex が併せて指摘。
  **今回のクラッシュの原因ではない** (`set` は既に適用済みの位置にある) が、同じ関数の同型の穴。
- `paint_and_update_textures` は `set` を surface 参照より前に適用するようになったが、
  **`free` のループは描画の後ろ**にある。`get_current_texture` が失敗して
  `SurfaceErrorAction::RecreateSurface` / `SkipFrame` で戻る経路
  (`vendor/egui-wgpu/src/winit.rs`) では free が実行されない。
- 影響は**テクスチャリーク**であってクラッシュではない。egui は id を再利用しないので
  誤描画にはならず、解放されない GPU テクスチャがプロセス寿命で残るだけ。
  サムネイルを大量に流す使い方だと効いてくる可能性がある。
- 直し方: no-surface 早期 return と同じく、これらの経路でも `free` を流す。
  `set` / `free` の適用を 1 つのヘルパに寄せて、描画の成否と独立にするのが素直。
- **2026-08-16 の確定**: §1.31 の第 2 段として着手する (順序は §1.31 参照)。
  ブリーフ = [codex-delta-delivery-transaction-brief.md](briefs/codex-delta-delivery-transaction-brief.md)。
  - **障害影響は P3 のままだが、§1.31 の依存関係上は必須前提**。§1.31 は frame drop を通常経路に
    するので、捨てる経路で `free` が落ちる構造のままだと本物のリークになる。
  - **「1 つのヘルパ」を単一関数と読まない**。`set` は surface lookup の前、`free` は成功経路では
    `queue.submit` の後でなければならず、同じ時点に置けない。
    `begin_delivery` (set をちょうど一度) / inner が typed `PaintOutcome` を返す /
    `finish_delivery` (outcome ごとに free) の**二段階 transaction owner**にする。
    `RenderStateAbsent` は「配送不能」の明示的な例外 outcome として型に残す。
  - **exit 3/4 へ同じ `free` ループをコピーするだけの修正は、「構造的修正」の合意対象外**
    (2026-08-16、ClaudeCode / Codex Sol)。所有構造 (配送責任が各 return に分散している) を
    直さず症状だけ消すため。
- **2026-08-16: 完了**。
  - `begin_delivery` が render state 有りなら surface lookup 前に `set` をちょうど一度適用し、
    inner paint は `RenderStateAbsent` / `SurfaceAbsent` / `SurfaceRecreated` / `Skipped` /
    `Submitted` の typed `PaintOutcome` へ合流する。no-paint 経路も同じ owner を使う。
  - `finish_delivery` が唯一の `free` 適用箇所。非 submit outcome は labeled inner block の
    encoder / command buffer drop 後、`Submitted` は `queue.submit` 後に finalizer へ到達する。
  - 実 driver error は注入せず、小さい acquire classifier seam で `RecreateSurface` / `SkipFrame`
    を分類し、両 outcome で seed texture が消えることを `Renderer::texture()` で検査した。
    §1.85-A の no-surface test は無修正で維持する。
- 規模 / 優先度: 小 / P3 (障害影響)。**ただし §1.31 の必須前提**。

### 1.84 表示キャッシュに item-context 世代の刻印が無い (latent、ABA 危険)

- 出典: v3.0.0 出荷前の holdover 調査 (2026-08-14) で Codex が併せて指摘。
  **今回の不具合 (`docs/briefs/pdf-page-turn-and-stale-composite-plan.md`) の原因ではない**
  が、同じ調査で見つかった同型の穴。
- フルスクリーン表示チェーンの以下が **`idx` だけ**、または item 文脈を含まない世代だけで
  引いている:
  - `current_edit_result_texture` — `idx` で走査し他の `EditResultKey` 世代を無視 (src/app.rs 53781)
  - `current_comic_composite_texture` — `idx` 直引き。コメントに「世代チェックなし」と明記 (src/app.rs 56470)
  - `current_final_composite_texture` — `key.edit_key.idx` のみ (src/app.rs 55823)
  - `conceal_cache` — `idx` + global conceal 世代のみ、`items_generation` を見ない (src/ui_fullscreen.rs 4297)
  - `EditResultKey` / `FinalCompositeKey` に item-context 世代のフィールドが無い
    (src/app.rs 5550 / 6032)
- **今は `close_fullscreen` が各キャッシュをクリアするので救われているだけ**
  (src/app.rs 49757)。軽量な items 差し替え経路がそのクリアを通らないと ABA になる。
- **意図された正しい形が同じファイルにある**: `ContinuousPageTransition` は
  `items_generation` を自分で持ち、`keep_set_evict` でも照合している (src/app.rs 6434)。
- 直すときの注意 (Codex): 「current 系 helper 3 つに条件を足す」では**不十分**。
  exact-key ヒットが src/app.rs 55292 / 55640 にもあるため、
  **key / read / insert のライフサイクル全体**に item 文脈を通す必要がある。
- 規模 / 優先度: 中 / P2。リリース直前に触る範囲ではない。

### 1.87 セッション最初の拡大で UI スレッドが 0.5 秒止まる (拡大パイプラインの初回作成)

- 出典: v3.0.0 リリース前の perf smoke (2026-08-15)。
- 実測: `gpu/lanczos_regenerate` は同セッション 16 回のうち **1 回目だけ
  `encode_submit_cpu_ms=495.1`**。残り 15 回は 0.2〜7.8ms (中央値 0.5ms)。
  同フレームの `ui/fs_viewport_breakdown` は `central_ms=495.3` / `media_ms=495.2` で、
  495ms 全部がフルスクリーン中央の描画に乗っている。セッション中で 50ms を超えた
  UI スレッド作業はこの 1 件だけ。
- 読み: 初回だけという分布は wgpu の pipeline / shader を最初の 1 回で作っている形。
  利用者から見ると「その日はじめて画像を拡大表示した瞬間に 0.5 秒固まる」。
- **今回入った退行ではない**。既定の高品質拡大は v2.12.0 からで、warm-up の構造は
  そのとき入っている (§1.46)。v3.0.0 のブロッカーとしては扱わない。
- 直すなら: 起動後の暇なフレーム、または一覧描画の最初の 1 回で pipeline を先に作る。
  どのタイミングなら利用者の操作とぶつからないかは計測してから決める
  (起動直後に足すと今度は起動が遅くなる)。
- 規模 / 優先度: 小〜中 / P3。

### 1.94 音声モードの退出が期限切れで detach+attach+seek に落ち、音が途切れる

- 出典: 利用者報告 (2026-08-18)。メインウィンドウへ結合した状態で <kbd>F11</kbd> と <kbd>Z</kbd> を
  連打すると、切り替えのたびに**メインウィンドウのグリッドが一瞬見えるちらつき**と、
  **音の途切れ**が起きる。
- **同日 (2026-08-18) に入れた「別ウィンドウの動画再生中に外部アプリから戻ると Z だけ効かない」の
  修正が原因ではない**。その経路は `source=egui_key` で記録されるが、実ログ中に
  2 回しか無く (別ウィンドウでの成功例)、後述のタイムアウト 7 件のいずれとも時刻が重ならない。
  失敗時の enter はすべて既存の `source=native_key`。ちらつき自体はリリース版 v3.1.1
  (本修正を含まないプロセス) でも観測されている。
- **実測した機構** (1 セッションで 7 回発生、`mimageviewer.log`):

  ```
  180.776 exit: placement changed, re-placing via SwitchPlacement
  180.877 exit ignored reason=exit_pending  deadline_remaining_ms=1099
  181.409 exit ignored reason=exit_pending  deadline_remaining_ms=567
  181.986 exit timed out; falling back to detach+attach+seek
  181.986 fallback exit: re-attached presenter, seek=32.298
  ```

  placement 切替を伴う退出が **`VideoAudioExitPending.deadline` 内に完了せず**、
  detach → attach → **seek** のフォールバックへ落ちる。この seek が**音の途切れ**、presenter の
  付け直しが**ちらつき**。連打すると毎回この経路に入る。
- **構造上の問題**: 退出の完了判定に**時間窓 (`deadline`) を使っている**。憲法 §2 規則 5
  「races を時間窓で吸収しない」に当たる。本来は placement 切替の完了イベントで確定すべきところを
  経過時間で見切っているため、切替が遅い状況では必ずフォールバックする。
- **直す方向**: deadline を伸ばさない。`SwitchPlacement` の完了 (または presenter の再配置完了) を
  typed なイベントとして受け、それで退出を確定させる。フォールバックは「イベントが来ない」ことが
  確定した場合の最後の手段に落とす。着手前に、退出の producer / consumer と
  open / switch / close / cancel / error の各 lifecycle を列挙すること。
- detached / native video のリワーク凍結領域。症状パッチを入れず、原因に対応付けてから直す。
  着手時は [detached-rework-plan.md](detached-rework-plan.md) §2 を読み、
  ClaudeCode / Codex 双方で「症状パッチではない」ことの合意を取る。
- 再現手順: メインウィンドウのフルスクリーンで動画再生 → <kbd>F11</kbd> と <kbd>Z</kbd> を連打。
  判定はログの `timed out; falling back to detach+attach+seek` の有無で機械的にできる。
- 規模 / 優先度: 中 / P2 (音が途切れるので体感は悪いが、連打しない通常操作では出ない)。

### 5.10 入力テストの共有ロックが poison して、失敗 1 件が全滅に見える

- 出典: 動画補正スロットの入力 parity 確認 (2026-08-19) の mutation 確認中。可視性ガードを外して
  1 件だけ落とすつもりが、
  同じフィルタの 5 件すべてが FAILED になった。実際に落ちたのは 1 件で、残り 4 件は
  `fullscreen fixed-key test lock poisoned: PoisonError { .. }` だった。
- 機構: `crate::key_input::TEST_INPUT_LOCK` は入力テストを直列化するためだけの
  `Mutex<()>` だが、12 箇所すべてが `.expect(...)` / `.unwrap()` で取得している。1 件が
  panic するとロックが poison し、以降このロックを使う全テストが**本来の失敗とは無関係な
  理由で**落ちる。**最初の 1 件を読まないと原因が分からない状態**になり、CI ログでも
  mutation 確認でも切り分けの手間が増える。
- 直す方向: 取得を 1 つの helper に集約し、`unwrap_or_else(|e| e.into_inner())` で poison から
  回復する。この Mutex はデータを保護しておらず、panic で壊れる不変条件を持たないので
  回復が正しい。**ただし共有フレーム状態の後始末は別問題**なので、helper 化のときに
  「各 call site が取得直後に `clear_test_frame()` するか、guard の Drop で戻すか」を
  揃えて確認する (現在は `FullscreenFixedKeyTestGuard` だけが Drop で戻している)。
- 対象: `src/key_input.rs` (定義 + 6 箇所)、`src/app/tests.rs` (5 箇所)、`src/keymap.rs` (1 箇所)。
- 規模 / 優先度: Small / P3。製品には影響しないテスト基盤の可読性問題。

### 5.8 検索ベンチのゲートに絶対時間の下限が無い

- 出典: v3.0.0 リリース前確認 (2026-08-14)。`check_bench_regression.py` が 10 件中 7 件を
  劣化と報告したが、**製品側の劣化ではなかった**。
- 何が起きたか: 判定が `+30%` の**比率だけ**で、対象クエリの絶対時間を見ていない。
  実測はこう:

  | クエリ | baseline | 3 回の実測 | 実行間ばらつき |
  | --- | --- | --- | --- |
  | `rare_jp` | 1.03ms | 3.61 / 1.13 / 0.89 | **+304%** |
  | `super_generic` | 0.01ms | 0.02 / 0.01 / 0.01 | +184% |
  | `rare_jp_and` | 0.15ms | 0.31 / 0.42 / 0.23 | +79% |

  **同一バイナリの実行間ばらつきが閾値 (+30%) を大きく超える**。1 回目は 25 分の
  配布ビルド直後で、全クエリが一様に遅い (ディスクキャッシュと CPU の状態)。
  3 回の最良値なら 10 件中 9 件が合格し、3 件は baseline より速かった。
- 唯一残る `rare_jp_and` も 0.15ms → 0.23ms で、**差は 0.08 ミリ秒**。この規模に
  比率の閾値を当てても意味がない。
- ずれている前提: ゲートは「1 回の計測が真の性能」と仮定しているが、ミリ秒未満の
  クエリでは計測ノイズのほうが大きい。
- 直す方向 (組み合わせる):
  1. **絶対時間の下限**を入れる。`+30%` かつ `+N ms` (N=1〜2 程度) の両方を満たしたときだけ
     劣化とする。ミリ秒未満のクエリが騒がなくなる
  2. ベンチ側で **1 クエリを複数回まわして最良値**を JSON に書く (現在も batch 内では
     `best=` を出しているので、外側にも同じ考え方を広げる)
  3. baseline に**測定条件** (日付・機種・同時実行の有無) を残す。現在の
     `vendor/bench_baseline.json` は `num_docs` と `version` しか持たず、
     いつ何の上で取った値か分からない
- 当座の運用: 失敗したら**同じバイナリで 3 回まわして最良値で判断する**。
  ノイズと判断した場合に `--save` で baseline を上書きしないこと (ノイズを基準値にすると
  次から本物の劣化を見逃す)。
- 規模 / 優先度: 小 / P3 (リリースのたびに人が判断すれば回るが、毎回時間を取られる)。

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
