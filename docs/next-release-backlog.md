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

### 1.22 キー入力を viewport / HWND 単位でルーティングする (案D)

- 出典: 2026-07-29 のキーボード所有権レビュー (Codex Sol 設計レビュー + ClaudeCode)。
  正本は [keyboard-input-ownership-plan.md](keyboard-input-ownership-plan.md)。本項は同計画の
  §6「対象外」として切り出したもの。
- 背景: 現状 Win32 の `KeyEdge` キューは全インストール HWND で共有され、送信元 HWND / viewport を
  持たない ([src/key_input.rs](../src/key_input.rs))。IME 状態も `App` に 1 組しかなく、
  各 viewport の入口から更新される。そのため「root ctx はキーボード不要、child ctx は TextEdit
  編集中、物理キーキューはどちら由来か不明」という組み合わせが成立し得る。
- 対応案: `KeyEdge` に送信元 HWND / viewport を持たせ、root / fullscreen / detached が
  **自分由来の edge だけ**を消費する。IME 状態も `ViewportId -> ImeState` へ分離する。
- 前提: **案A (`KeyboardOwner` / `ShortcutPermit` の単一ゲート) の完了後**に着手する。
  案A で所有権判定が一元化されていれば、本項は「どの viewport の所有権か」を正しくするだけの
  変更に縮む。
- 影響範囲: Windows native 入力、egui viewport、detached / native presenter。HWND 再生成や
  viewport lifecycle への対応が要る。detached リワークとの調整も必要
  ([detached-rework-plan.md](detached-rework-plan.md) §2 / §11)。
- 規模 / 優先度: Large / P2 candidate。
### 1.25 一時停止中に動画フィルタを変えても画面へ反映されない

- 出典: v2.8.1 検討中のユーザー報告 (2026-07-26)。動画を一時停止した状態で色調補正 /
  Creative LUT を操作しても絵が変わらず、再生を再開して初めて反映される。静止させて
  パラメータの効きを見比べたいケース (微調整・前後比較) で機能しない。
- 原因 (コード確認済み): `NativeVideoPresenter::set_video_grade`
  ([src/video/native_presenter/mod.rs:3226](../src/video/native_presenter/mod.rs)) は grade 定数バッファと
  Creative LUT テクスチャを更新するだけで、present を行わない。実際に絵へ乗るのは
  `VideoGradePipeline::draw` が走る次の `present()` = 次のデコード済みフレームを提示する
  ときなので、新しいフレームが来ない一時停止中は画面が更新されない。
- 対応案 (2026-07-26 の領域監査で訂正): **`present_retire.back()` を再 present する形では
  直せない。** 当初案は誤りだったので採用しない。理由:
  - `present_retire` は「最後に表示したフレーム」ではなく、**GPU コピー完了までの廃棄待ち
    キュー**。表示内容の永続的な所有者ではない。
  - GPU decode 経路のフレームは一度 present すると keyed mutex guard が `ReleaseSync(0)` で
    writer key へ戻る。次の `present` は `AcquireSync(1)` を要求するが、producer が
    key=1 へ release するのは reader へ最初に渡すときだけで、held frame は pool slot を
    占有したままなので再 arm されない。つまり **再 present 自体が失敗する**。
  - 正しい方向は、native output context が `Empty / Hidden{frame} / Visible{frame, fence}` の
    単一 typed state を所有し、`present_retire` は廃棄待ち専用として残すこと。
    GPU frame を Visible として再利用するには、present 後に keyed mutex を reader key=1 へ
    再 arm する処理も要る。新フレーム提示時に旧 Visible を retire へ移す。
  - 現在は `present_retire` / `hidden_latest_frame` / `source.queue` の 3 つが「使えそうな
    最近のフレーム」を部分的に表現しており、この分離が本項と 1.26 の共通原因になっている。
- 注意:
  - 再提示は grade 変更を検知したときだけにする。一時停止中に無条件の present ループを
    回すと `docs/idle-health-check.md` の静止時 repaint 検査に引っかかる。
  - hidden presenter 中は表示がないので再提示しない (show 時に最新 grade で初回 present
    されるため症状が出ない)。音声のみの再生も対象外。
  - 通常フルスクリーンと detached の可視 presenter は同じ問題を共有する。detached 側へ
    App-global の bool や geometry state を足す形にはせず、native output context 内に閉じる。
  - ⚠ **presenter スレッドで keyed mutex の `AcquireSync` を呼んではならない** (2026-07-29 に
    実測したデッドロックの制約。1.27 参照)。再 arm が必要なら、`reset_unpresented_shared_output`
    と同じく `released_to_reader` を立てるだけにして、実際の key 回復は producer 側の
    `acquire_shared_output` (`recover_shared_output_keyed_mutex`) に任せる。
- 規模 / 優先度: Medium / P2 candidate。1.26 と同じ構造修正で一緒に解消する。

### 1.26 一時停止中の placement 切替で新ウィンドウの prime が失敗する

- 出典: v2.8.1 開発中の領域監査 (2026-07-26、コード確認済み・実機未確認)。
- 症状: 一時停止や EOF などで `source.queue` が空の状態で placement を作り直すと、
  GPU decode 経路では新 presenter の prime が失敗する。コードは prime の成否に関わらず
  新ウィンドウを表示する ([src/video/mod.rs:3113](../src/video/mod.rs)) ため、次のフレームが
  来るまで黒または未初期化の表示になり得る。一時停止のままなら継続する。
- 原因 (コード確認済み): fallback が `present_retire.back()` を新 presenter へ渡すが
  ([src/video/mod.rs:3079](../src/video/mod.rs))、既に一度 present された GPU frame は
  keyed mutex が writer key に戻っており `AcquireSync(1)` が通らない (詳細は 1.25 の対応案)。
  `hidden_latest_frame` と `source.queue.front()` は未 present なので key=1 のままで、
  この問題はない。CPU frame にも keyed mutex がないので再提示できる。
  つまり **「既に一度 present された GPU frame」だけが踏む**。
- 対応: 1.25 と同じ `FramePresentationState` への集約で一緒に直す。個別の再 arm 分岐を
  fallback 経路にだけ足す形にはしない。
- 規模 / 優先度: 1.25 と同一作業 / P2 candidate。

### 1.27 presenter スレッドが UI スレッドの窓の子 HWND を所有したままブロックする

- 構造修正の設計: [Native video HWND ownership / pump 分離計画](native-video-window-thread-plan.md)。
- 出典: 2026-07-29 のハング解析 (`cdb -pv` で両スレッドのスタックを実測)。
- 症状: 動画をフルスクリーンで開いた直後に窓を閉じる / 切り替えると、アプリ全体が
  完全に固まる。CPU はほぼ 0 で、`panic.log` に `UI THREAD HANG suspected` が 10 秒ごとに
  出続ける。強制終了以外に復帰しない。
- 実測した環:
  1. UI スレッドが動画ビューア窓に `DestroyWindow` を呼ぶ
     (`winit public_window_callback_inner::closure$4` → `NtUserDestroyWindow`)。
  2. その窓の**子** `mIVNativeVideoWindow` は `native-video-presenter` スレッドが所有して
     いる。子の破棄と活性化の移動は所有スレッドへの**同期メッセージ**になるため、
     UI スレッドは `WM_ACTIVATE` → `DefWindowProcW` → `NtUserMessageCall` で待ちに入る。
  3. presenter スレッドはそのとき D3D11 の中で待っている。実測したスタックは
     `AcquireSync` → `NDXGI::CDevice::DXGIAcquireSync` → `CDevice::Flush` → nvwgf2umx →
     `WaitForSingleObject`。
  4. 相互待ちで永久ハング。
- 構造的な問題: **HWND を所有するスレッドがメッセージを回さずにブロックし得る**こと。
  presenter スレッドは GPU 待ちを含む処理を常に行うので、UI スレッド側のどんな窓操作
  (破棄 / 活性化 / フォーカス / IME) でも同じ環が成立する。keyed mutex のタイムアウト値では
  防げない (実測では 10ms 指定で 75 秒以上ブロックした。待っているのは `AcquireSync` 内部の
  `Flush` で、タイムアウトの管轄外)。
- 対応案 (どちらかを選ぶ):
  1. **子 HWND を UI スレッドで作る**。presenter スレッドは GPU 処理だけを持ち、窓を
     所有しない。Win32 の原則 (窓を持つスレッドはメッセージを回す) に沿う。
  2. presenter スレッドで**ブロックし得る呼び出しを一切行わない**ことを保証する。
     現実的には難しく、1 のほうが構造的。
- 暫定の運用制約: presenter スレッドで keyed mutex の `AcquireSync` を呼ばない
  (1.25 / `reset_unpresented_shared_output` のコメント参照)。
- 規模 / 優先度: Medium〜Large / **P1** (ハード ハングのため)。

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
     `BottomRight` の各レーンに実測済み `BadgePlacement` (矩形、galley、優先度) を返す
     `ThumbnailOverlayLayout` 相当の純粋なレイアウト層を設ける。
  2. 第1段階は左上を移行し、ブックマーク時刻 → 編集状態 / pin → タグの順に幅を予約する。
     `UP` も同じレーンへ入れ、残り幅が不足する場合だけタグを実測で省略または非表示にする。
     描画と hover / click 判定は同じ `BadgePlacement` の矩形を使い、再計算をなくす。
  3. 第2段階は左下を移行し、フォルダ名 / ZIP / PDF / 変換アーカイブ、評価、ファイル名
     プレートの予約幅・縦積みを同じレイアウト結果から決める。フォルダ名バッジは形式
     バッジと機械的に同じ 70% へ揃えず、長い名前の可読性を残したコンパクトな専用
     スタイルをスナップショット比較で決める。
  4. 第3段階で右上 / 右下も同じ所有境界へ移し、チェックとスタック枚数の排他条件、
     絞り込み件数の配置を明示する。各段階を独立コミット可能にし、全経路を一度に
     書き換えない。
- 完了条件 / 回帰テスト:
  - ブックマーク時刻と `#` から始まるタグが重ならず、表示矩形とクリック矩形が一致する。
  - `UP` + 編集状態 / pin + タグ、狭いセル、長い CJK タグでもバッジ同士が重ならない。
  - 長いフォルダ名 + 評価、ZIP / PDF / RAR + ファイル名 + 評価で重なりやセル外描画がない。
  - レイアウト純関数で矩形の非交差・優先順位・狭幅時の省略を unit test し、代表的な
    フォルダ / 形式バッジとブックマーク時刻 + タグを `docs/ui-snapshot-policy.md` に従う
    snapshot で確認する。
- 規模 / 優先度: Medium / P2 candidate。現在の重なりだけを座標ずらしで直す症状パッチは
  入れず、まず左上レーンの単一所有化から着手する。

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

### 3.3 final-effect worker 待ちの間、色調補正とシャープを毎フレーム作り直している

- 背景: `ensure_final_composite_texture` は、AI 到着後の再合成待ちの間だけ早期 return 条件
  (`entry.complete || (ai_ready.is_none() && !ai_failed)`) を外れる。その間は毎フレーム
  `apply_adjustments_fast` と `apply_final_smart_sharpen` を UI スレッドで走らせ直している。
  結果は捨てられ、実際に使うのは worker の出力。
- 影響: 色調補正なし・シャープ 0 の既定では `Arc::clone` だけなのでほぼ無害。色調補正か
  スマートシャープを使っているユーザーは、AI 切り替え待ちの数秒間フルサイズの CPU パスを
  毎フレーム払う。大判ページほど重い。
- 方針: 同一 key の final-effect job が既に pending なら、CPU パスへ入る前に既存 texture を
  返して抜ける。2026-07-29 の「AI 切替時の黒フレーム」修正で incomplete entry を保持する
  ようになったため、返すべき texture が常に手元にある。
- 優先度: P2 (体感ヒッチだが既定設定では出ない)。perf-log で
  `fs` / `final_composite_build` の連続発火として観測できるはず。

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
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

## 5. リリース前確認 / 依存更新

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
- **P2: `build-release.ps1` の停止対象が広すぎる**:
  - 現状はリポジトリ配下のプロセス名 `mimageviewer*` を列挙して
    `Stop-Process -Force` する。Cargo のテストハーネスも
    `target\debug\deps\mimageviewer-<hash>.exe` なので、別セッションのテストまで
    強制終了し得る。
  - 対応案 = 停止対象を launcher / core / helper の正確な実行ファイル名と想定配置へ
    allowlist 化する。必要なら test / release の同時実行を検知するリポジトリ単位の
    mutex またはロックも追加し、黙って相手を終了せず明示的に待機または失敗させる。
  - 完了条件 = 実行中のダミー `mimageviewer-<hash>.exe` を停止せず、repo の launcher /
    core と APPDATA に展開された対象 helper だけを従来どおり停止できること。
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
