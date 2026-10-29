# 動画の上部バーに「…」を付け、デインターレースを再生中に切り替える (§1.105 / §1.660)

- 設計・検収: ClaudeCode Opus 5.5 / high。実装: Codex gpt-6.1-sol / high。独立レビュー: 別の Codex gpt-6.1-sol / xhigh。
- 作業場所: `G:\home\mimageviewer-deinterlace`、ブランチ `video-overflow-1105` (master e1dd9ea93 から)。
- 利用者決定 (2026-10-29):
  - 動画の上部バーの右上に、静止画と同じ「…」(その他の機能) ボタンを出す。最初の項目はデインターレース (自動 / 常に有効 / 無効)。
  - 手動で切り替えた値は **その動画を開いている間だけ** 有効。閉じたら全体設定 `settings.video_deinterlace` に戻る。ファイルごとに記憶しない。全体設定の値も書き換えない。
  - 動画の情報表示 (`format_deinterlace_status`) に、手動で切り替えた状態も出す。

## 1. 反映方法の判断: 開き直しではなく、デコーダが次のフレームから読む値を変える

利用者との合意時点の第一案は「同じ再生位置で開き直す」だった。コードを確かめた結果、次の理由で
**再生中に decoder thread が読むモードの値を差し替え、現在位置へ seek して表示を更新する**方式にする。
CLAUDE.md「設計の簡素化」の観点 (扱う状態の組み合わせを減らす) での比較を残す。

- 開き直しに使える既存経路は fast source swap (`start_native_video_source_swap`, `src/app/native_video.rs`) だけで、
  `from_idx == target_idx` を拒否する。同じ index へ入れ替える版を作ると、`fs_cache` の同一 key の remove/insert、
  `open_fullscreen` の履歴、resume 位置の保存、ノーマライズ状態の初期化、seek strip セッションの終了、
  動画ズーム状態の破棄 (`video_zoom_state = None`)、音声トラック選択、VST、detached / Remote の所有権と
  組み合わさる。いずれも「設定を 1 つ変える」操作には不要な副作用で、利用者の拡大位置や seek strip が消える。
  音声トラック切り替え (§1.251) も開き直しではなく、player への要求で切り替えている。
- decoder thread (`run_video_decode_thread` 相当, `src/video/decoder.rs`) は bwdif を **遅延生成**し、
  seek・SW decoder 再オープンのたびに `deinterlacer = None` で捨てて作り直している。モードが変わったときも
  同じ扱い (捨てて、次のフレームで判定し直す) をすれば、新しい状態の組み合わせは増えない。
  HW (D3D11VA) フレームの CPU bwdif への切り替えも、フレームごとの `should_try_deinterlace` 判定なので追従する。
- 一時停止中も結果が見えるよう、切り替え後に **現在位置への seek を 1 回** 出す (既存の seek 経路。
  再生中は再生を続け、一時停止中は一時停止のまま)。これでキュー済みの古いフレームも捨てられる。

## 2. 状態の持ち主 (1 か所)

- `VideoPlayer` が、その player だけのデインターレース制御を `Arc` で持ち、decoder thread と共有する。
  - 中身: 現在のモード (`VideoDeinterlaceMode` を `AtomicU8` で符号化) と、open 時のモード (不変値)。
  - 「手動で切り替えた」は **保存しない**。`現在のモード != open 時のモード` から導出する。
    (open 時の値 = 開いた時点の全体設定。環境設定で全体設定を後から変えても、開いている動画には効かない現行挙動を変えない。)
  - player が drop されれば値も消える。別の動画への切り替え・閉じる・開き直しは新しい player を
    `settings.video_deinterlace` から作るので、自動的に全体設定へ戻る。App 側に追加の状態を置かない。
- decoder thread は毎フレームこの値を読み (Relaxed/Acquire の load 1 回)、前回と違えば:
  `deinterlacer = None`、`deinterlace_failure_logged = false` (切り替えは再試行の機会にする)、
  状態表示を `Off → Inactive` / `それ以外 → Pending` に戻す。ログを 1 行出す。
  既存の `deinterlace` 引数 (open 時の固定値) を使っている判定 (`should_try_deinterlace` / `bwdif_force_all_frames` /
  GPU 経路の Inactive 遷移 / seek 時の Pending 戻し / 起動時の Inactive 初期化) はすべてこの現在値に置き換える。
  固定値と現在値が混在しないこと。
- `VideoInfo::effective_deinterlace_mode` (open 時の値) を表示に使っている箇所 (`sync_native_video_metadata`) は
  player の現在値に置き換える。info 前の分岐 (`self.settings.video_deinterlace`) も player の値を使う。
  使われなくなる field は残さず削除してよい (表示の持ち主を 1 つにする)。
- Remote の headless / clockless player はこの UI の対象外。変更は入れなくてよいが、引数の型変更で
  通るように直すだけにする (挙動は open 時の全体設定のまま)。

## 3. UI (native presenter の上部バー)

動画 HUD は `src/video/native_presenter/` で描く (`ui_fullscreen.rs` ではない)。

- ボタン: `draw_native_top_bar` で、静止画と同じく **× のすぐ左** に「…」を置く (静止画の `draw_more_icon` と同じ見た目。
  `crate::ui_fullscreen::draw_icons::draw_more_icon` を再利用できるなら再利用する)。上部バー固定の鍵ボタンは 1 つ左へずれる。
  `native_top_bar_lock_button_rect` を使う箇所 (render_core のヒット領域やテスト) も同じ位置を返すよう揃える
  (位置の計算を 1 か所にする)。tooltip は「その他の機能」。
- 出す条件: 通常の動画再生の上部バーだけ。音声のみ native シェル (`audio_only`)、タイル一覧の上部バー、
  移動プレビューの上部バーには出さない。360 度表示・動画拡大中も出してよい (デインターレースは表示モードと独立)。
- ポップアップ: 360 度の投影方式ポップアップ (`draw_native_panorama_projection_popup`) と同じ作り
  (presenter-local の open bool、描いた rect を HUD の領域 `SetWindowRgn` に含める、開いている間は HUD を表示し続ける、
  外側クリックで閉じる)。見出し「その他の機能」、行「デインターレース」と 3 択 (`VideoDeinterlaceMode::label()` の
  「自動 / 常に有効 / 無効」= 環境設定と同じ文言。別の言い回しを作らない)。現在の値に印を付ける。
  補足 1 行: 「この動画を閉じると環境設定の値 (〜) に戻ります」(〜 は open 時のモードの label)。
  ソース切り替え (fast swap) とタイル一覧への切り替えでポップアップを閉じる。Escape の扱いは投影方式ポップアップに揃える。
- 選択すると `NativeOverlayCommand::SetDeinterlaceMode { mode }` → `NativeVideoOutputEvent::SetDeinterlaceMode { mode }` →
  App のハンドラ。経路は音声トラック選択 (`SelectAudioTrack`) と同じにする: source epoch の検査、ParkedLive での扱い、
  音声のみシェルの許可リストでの扱い (音声のみシェルでは出さないので拒否側でよい) を、`SelectAudioTrack` が列挙されている
  match すべてについて判断し揃える。App ハンドラは `fs_cache` の該当 player に `set_deinterlace_mode(mode)` を呼び、
  値が変わったときだけ現在位置へ seek し、`sync_native_video_metadata` と HUD の再描画要求を出す。
- detached (F12) の動画も同じ上部バー・同じイベント経路を通る。detached 専用の分岐や guard を足さない。
  既存の per-context 経路 (音声トラック選択と同じ) に新しい variant を載せるだけにし、そうなっていることをコードで確かめて報告する。

## 4. 情報表示

- `format_deinterlace_status(mode, status, interlace_detected, manual)` のように引数を足し、手動のときは先頭に「手動: 」を付ける
  (例: 「手動: 常時 (適用中)」「手動: オフ」「手動: 自動 - プログレッシブ」)。手動でないときの文字列は今と同じ。
- `NativeOverlayMetadata` に `deinterlace_manual: bool` (または open 時のモード) を足し、ポップアップの現在値・補足文もここから描く。

## 5. キー操作

- 今回は `KeyAction` を足さない。理由: 動画ごとに 1 回切り替える低頻度の操作で、既定キーを新たに奪う理由が弱い。
  必要になれば `KeyAction` 経由で後から足せる。文書 (keymap-spec) の変更も不要。

## 6. 割り切り

- まれな失敗 (bwdif の初期化失敗など) は既存どおり「初期化失敗」の表示とログ。切り替え時に一度だけ再試行の機会がある以外、新しい回復処理は足さない。

## 7. テスト (少なくとも)

- decoder: モード変更の判定を純関数に切り出し、変更時に「deinterlacer を捨てる・失敗 latch を戻す・状態を Pending/Inactive に戻す」ことの単体テスト。
  `should_try_deinterlace` が現在値で動くこと。
- player: `set_deinterlace_mode` が値を変え、同じ値なら「変化なし」を返すこと。open 時の値から `manual` が導出されること。
  別の player (= 次に開く動画) は全体設定から始まること。
- 表示: `format_deinterlace_status` の手動あり/なしの全組み合わせ。
- overlay: 「…」ボタンが動画の上部バーにだけ出て、音声のみ・タイル・移動プレビューでは出ないこと。鍵ボタンの位置と
  ヒット領域が一致すること。ポップアップで項目を押すと `SetDeinterlaceMode` が出ること。ポップアップの rect が HUD 領域に入ること。
  既存の上部バーのレイアウトテスト・スナップショットを更新する (スナップショット PNG は `UPDATE_SNAPSHOTS=1` で更新し、差分を報告する)。
- App: `SetDeinterlaceMode` イベントで現在の player の値が変わり、stale epoch のイベントは無視されること (既存の SelectAudioTrack のテストに倣う)。

## 8. 文書

- `docs/video-architecture.md`: HUD の節に「…」ボタンとデインターレースの再生中切り替え (状態の持ち主、decoder の扱い、seek で更新)、
  開き直しを選ばなかった理由を短く。GPU 経路の記述 (Auto/On は CPU bwdif へ fallback) と矛盾しないこと。
- `docs/spec.md` / `docs/architecture-overview.md` にデインターレースの記述があれば、再生中に切り替えられることを足す。
- マニュアル `htdocs/mimageviewer/manual/video.html`: 上部バーの「…」と、デインターレースをその動画だけ切り替えられること
  (閉じると環境設定の値に戻る) を書く。情報表示の「デインターレース」行に「手動」表示を足す。版番号・実装用語を書かない。
- `docs/next-release-backlog.md`: §1.105 を「実装済み・実機確認待ち」に更新し、デインターレースの手動切り替えを §1.660 として起票 (同じく実装済み・実機確認待ち)。
- UI 文言を変えたら `python scripts/check_ui_glyphs.py`。
