# 実装ブリーフ: 360 度動画の入力と導線 (§1.112 第 3 段)

対象 worktree: `C:\home\mimageviewer-pano` (branch `panorama-projection`)。
**この worktree で他の codex を並行させないこと。**

第 1 段 (判定) と第 2 段 (描画) は完了済み。**この段で利用者が実際に 360 動画を
見られるようになる。** 完了したら実機確認へ回す。

## 0. 先に読む

1. [docs/next-release-backlog.md](../next-release-backlog.md) **§1.112** — 決定事項の正本。
   「入力設計の決定」と「360 ON はファイルをまたいで保持する」を必ず読む。
2. [docs/panorama-360-view-plan.md](../panorama-360-view-plan.md) **§13** — 投影方式。
3. [docs/video-architecture.md](../video-architecture.md) — native presenter の構造。
4. CLAUDE.md の keymap 方針 (新しいキー操作は `KeyAction` + keymap helper 経由)。

## 1. 完成の定義

360 動画をフルスクリーンで開き、
- 上バーの 360 ボタン (または <kbd>V</kbd>) で球面表示に入り、
- 左ドラッグで見回し、ホイールで画角を変え、
- <kbd>Shift</kbd>+<kbd>V</kbd> か上バーのボタンで投影方式を切り替え、
- 上バーのリセットで初期視点へ戻り、
- もう一度 360 ボタン (または <kbd>V</kbd>) で通常表示へ戻る。

## 2. 既に決まっていること (勝手に変えない)

- **姿勢の型は `crate::panorama::PanoPose`**、状態は **`App::panorama_state` を静止画と共有**する。
  型も lifecycle も分けない。静止画と動画で 2 つ持つと、視点の丸めと stale 判定が二重になる。
- **lifecycle は静止画と同じ**: 明示 ON で state を作り、**通常ナビでは視点を保持**、
  360 でない項目では**非アクティブ化するが state は捨てない**、**明示 OFF と
  フルスクリーン退出で破棄**。述語は `state.is_some() && detect(...)` の形に揃える。
- **ホイールは 360 中、修飾キー不問で FOV**。⚠ **FOV が上下限に達しても必ず消費する。**
  未消費に戻すと、限界でもう一度回した瞬間にファイルが切り替わる。
  レターボックス部分も同じ扱い (画面端だけ挙動が変わると予測できない)。
  ファイル移動は ↑/↓ に残るので到達不能にならない。
- **タッチはドラッグ = 見回し、タップ = 既存のまま** (中央 = HUD、左右 = ±5 秒シーク)。
  ⚠ **ownership latch が要る**: しきい値 (12 logical pt / 700 ms) を一度でも超えたら、
  その接触列は最後まで見回し。開始位置へ戻して離してもタップに戻さない。
  見回し確定フレームでは DOWN からの全移動量を反映する (最初の移動を捨てない)。
  2 本目が入ったら pending tap を必ず取り消す。
  ⚠ **ダブルタップを視点リセットに割り当てない** (左右連続タップのシークが化ける)。
- **× の意味を変えない。** 静止画は 360 中に 360 ボタンを隠して × を解除に使うが、
  **動画では踏襲しない**。動画の × は常に閉じる。360 ボタンは ON 中も強調表示で残し、
  同じボタンで OFF にする。
- **投影中は表示スケーラー (Anime4K / Lanczos / NIS / nearest) が走らない** — これは
  第 2 段で構造的にそうなっている。⚠ **利用者の設定値 (`VideoScaleFilter`) は書き換えない。**
  UI では「360 投影中のため一時停止しています」と出すだけにする。

## 3. 作るもの

### 3.1 メタデータを App まで届ける

- `VideoInfo` ([decoder.rs:1510](../../src/video/decoder.rs)) に球面メタデータを足す。
  **`orientation` と同じ場所で埋める** (`orientation_from_stream` を呼んでいる箇所、
  2028 行付近)。第 1 段の [spherical_metadata.rs](../../src/video/spherical_metadata.rs) の
  `spherical_from_stream` / `stereo_layout_from_stream` をそのまま使う。
- `VideoPlayer` に「この動画は 360 か」を返す accessor を足す
  (`native_video_info_for_anime4k` が前例)。判定は `spherical_metadata::detect` に
  **表示寸法** (SAR + 回転適用後) を渡す。生の符号化寸法で判定しない。

### 3.2 判定の入口を静止画と揃える

`App::detect_panorama(fs_idx)` は静止画専用 (XMP + `fs_cache` の 2:1)。
**動画にも答えられる 1 つの入口へ広げる。** 名前と戻り値の形は静止画側に合わせ、
呼び出し側が媒体を意識しないようにする。`is_panorama_mode_active` も同様。

### 3.3 トグルと投影切替

- **`KeyAction::FsPanorama` (V) を動画でも効かせる。** 現在は
  [ui_fullscreen.rs](../../src/ui_fullscreen.rs) で `!fs_music_view_active` の静止画経路に
  いる。動画フルスクリーンでも同じ action で ON/OFF する。
- **`KeyAction::FsPanoramaProjection` (Shift+V) も同様**に動画で効かせる。
  適用は既存の `App::set_panorama_projection` / `cycle_panorama_projection` へ合流させる
  (静止画と同じ経路。画角の丸めと通知を二重に持たない)。
- **上バーのボタン** は native overlay
  ([overlay_draw.rs](../../src/video/native_presenter/overlay_draw.rs) の
  `draw_native_top_bar`) に足す。VST3 と全画面切替の間あたり。
  - 360 対応動画でだけ有効。非対応では**同じスロットに理由付き disabled**
    (隣のボタンを動かさない)。
  - ON 中は投影方式ボタンと視点リセットボタンを隣に出す。
  - アイコンは静止画側の `draw_panorama_icon` /
    `draw_panorama_projection_icon` と同じ絵にする (別々に描かない)。
  - ⚠ **UI 文字列に環境依存グリフを使わない** (CLAUDE.md のグリフ方針、
    `python scripts/check_ui_glyphs.py` が 0 件になること)。

### 3.4 入力

すべて [src/app/native_video.rs](../../src/app/native_video.rs) の
`NativeVideoWindowEvent` ハンドラ (4673 行付近) に入る。

- **`MouseButton`**: 左押下で見回しドラッグ開始、離しで終了。既存の
  `native_video_pointer_down` と同じ作法で持つ。
- **`MouseMove`**: ドラッグ中なら yaw/pitch を更新。感度は静止画の
  `handle_panorama_drag_if_active` と揃える。
- **`MouseWheel`** (4720 行): 360 中は FOV。**上下限でも必ず消費**し、
  `navigate_native_video_fullscreen` へ落とさない。
- **`MouseLeave`**: ドラッグ状態を必ず解除する。
- **`Touch`**: 上の ownership latch のとおり。

姿勢が変わったら **`set_panorama_pose` で presenter へ流す** (第 2 段で作った経路)。
360 OFF では `None` を流して通常表示へ戻す。

### 3.5 排他と休止

- **360 に入るときタイル一覧を抜ける** (同じキャンバスを所有するため)。
  静止画が 360 入場時に衝突モードを畳んでいるのと同じ考え方。
- **音声モード♪ の間は 360 入力を休止**し、映像へ戻ったら視点を復元する。
  見えていない FOV がホイールで動いてはいけない。**state は破棄しない。**

## 4. テスト

純関数と state 遷移を優先する (D3D を要求するテストは書かない)。

- `detect` の入口が静止画と動画の両方に答えること。
- **ホイールが上下限でも消費されること** (未消費だとファイルが切り替わる回帰)。
- ドラッグの ownership latch: しきい値超え後に開始位置へ戻して離してもタップにならない。
- 2 本目の指で pending tap が取り消されること。
- lifecycle: 360 でない動画へ移ると非アクティブだが state が残り、次の 360 動画で
  同じ視点から再開すること。明示 OFF とフルスクリーン退出で破棄されること。
- 音声モード往復で視点が保たれること。

`cargo test -p mimageviewer --lib` が緑。`cargo fmt`。新規コードに clippy 指摘なし。
`python scripts/check_ui_glyphs.py` が 0 件。

## 5. 仕上げ

`.\scripts\build-dev.ps1` を通すところまで。**エージェントは起動しない**
(CLAUDE.md の検証起動ポリシー)。実機確認は利用者が行う。

## 6. 迷ったら

- **症状を消す guard / delay / retry / silent fallback を入れない。**
- **同型の入口を数える**: マウスとタッチ、CPU 経路と D3D11 共有経路、
  通常ウィンドウと別ウィンドウ。片方だけ直さない。
- 構造判断で迷ったら**実装せずに backlog §1.112 へ質問を書いて止める**。
