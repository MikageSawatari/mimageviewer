# 動画 HUD 2 段化リデザイン計画

## 背景

現行の動画 HUD は下端 46px の 1 行構造で、再生ボタン群・シークバー・時間/速度/音量を
すべて横並びで詰め込んでいる ([src/video/native_presenter/mod.rs:5691](../src/video/native_presenter/mod.rs))。
そのため以下の不満が累積している:

- **シークバーの精密操作がしづらい** — 4K/長尺動画でも seek bar の縦幅は 8px、controls 行と
  Y を共有していて操作領域も狭い
- **ボタンが無い機能が多い** — 前/次マーカー (J/K)、前/次ファイル (↑/↓)、Ctrl+S (フレーム保存)
  はキーボードのみ。新規ユーザーには発見性が低い
- **Space キーの慣習衝突** — 動画モードの Space = チェックは、他プレイヤー慣習 (= 再生/停止)
  と異なる。開発者本人も誤押下を報告。

これに対し:

1. シークバーを controls から分離した **2 段 HUD** へ移行
2. **ボタン化** すべき機能を整理し、シークバー幅を温存しつつ操作経路を追加
3. **キー再割り当て** で Space/Enter を業界慣習に合わせる
4. 画像モードとの **挙動差** (EOF トースト等) も併せて統一する

## ゴール

| # | ゴール | 検証方法 |
|---|---|---|
| G1 | シークバーがフル幅 + ヒット領域 24px で精密操作可 | 1280px 窓 / 2560px フルスクリーン両方で目視 |
| G2 | 高頻度キーボード機能 (J/K 前後マーカー、↑/↓ 前後ファイル、Ctrl+S 保存) にマウス経路 | 各機能のボタン存在確認 |
| G3 | 動画モード Space = 再生/停止 に揃える (Enter は再生/停止のまま、画像モード変更なし) | 動画フルスクリーンでキーテスト + 画像モードに regression なし |
| G4 | 動画でも前/次ファイル境界で「最後の項目です」等トースト | 画像と同じトースト文言で発火 |
| G5 | HUD 表示時の合計高さ増加 ≤ 25px (auto-hide 中は影響ゼロ) | 旧 46px → 新 ~66px (24+40) |

## キー再割り当て

### 動画モード

| キー | 旧 | 新 | 備考 |
|---|---|---|---|
| **Space** | チェック | **再生/停止** | YouTube/VLC/MPC/mpv 等の業界標準に統一 |
| Enter | 再生/停止 | **維持 (再生/停止)** | グリッドの Enter (= 動画を開く / 再生開始) と意味系統が連続 |
| Shift+Enter | 外部プレイヤー | 維持 | Enter (再生) のファミリーとして自然な variant |
| J / K | 前後マーカー | 維持 | ボタン経路を追加 |
| B | ブックマーク追加 | 維持 | ボタン化はしない (左パネル + ホバーサムネイルで充足) |
| P | ピン追加 | 維持 | ボタン化はしない (同上) |
| Ctrl+S | 保存 | 維持 | カメラパレット内の💾でマウス経路追加 |
| Ctrl+Shift+←/→ | フレームステップ | 維持 | カメラパレット内に移植 |
| その他 (W/L/M/F/S/F1-F6/Home/End/Ctrl+↑↓/↑↓) | — | 維持 | — |

**動画フルスクリーンから「チェック」操作は削除する**。
- 動画再生中にチェックする頻度は低いと判断 (= 大半は一覧でまとめて操作)
- チェックしたい場合は Esc で一覧へ戻ってから Space で操作 (= 2 キー)
- 削除を選んだ理由: Enter = チェック案 と比べて、Enter (グリッド = 動画を開く / 動画 = 再生)
  と Shift+Enter (外部プレイヤー) の意味系統を壊さない

**Space 再割り当ての同期対象 (Phase 1 実装範囲)** — Codex P1 指摘反映、native arm 単独ではない:

| 場所 | 現状 | 変更 |
|---|---|---|
| [src/app/native_video.rs:4542-4554](../src/app/native_video.rs) | Space = チェックトグル match arm | 再生/停止呼び出し (`handle_native_video_toggle_play_command`) に置換 |
| [src/app/native_video.rs:4389-4392](../src/app/native_video.rs) | (Enter のみ) tile mode で Enter = `play_selected_video_tile` | **Space も同じ tile-aware 挙動にする** (下記参照) |
| [src/ui_fullscreen.rs:2777-2781](../src/ui_fullscreen.rs) | コメント「動画でも Space は画像と同じ選択トグル」 + `key_space` を消費せず後段の画像ハンドラへ流す配線 | コメント書き換え + 動画モード時は Space を **画像ハンドラに流さず** native 側で処理させるよう routing 変更 |
| [src/ui_fullscreen.rs:7073-7081](../src/ui_fullscreen.rs) (`handle_video_input`、旧/フォールバック動画経路) | 同じく「Space は consume せず画像選択に流す」前提のコメント・実装 | 上記と整合させて動画モード Space を native 側 (= 再生/停止) で処理。Codex 第 2 ラウンド P2 指摘 |
| HUD tooltip / 中央ヘルプテキスト (`overlay_draw.rs` `draw_native_center_pause_controls` 等) | 「Enter: 再生」記載 | 「Space / Enter: 再生」に拡張 |
| keymap 関連テスト (routing test 等、`tests/` 下) | Space = チェック前提 | 動画モード分岐を追加 |
| [docs/keymap-spec.md](keymap-spec.md) | 動画モード Space = 選択トグル | 動画モード Space = 再生/停止 に書き換え |
| [docs/spec.md](spec.md) | 動画機能の挙動記述 | Space/Enter のキーバインド変更を反映 (Codex 第 2 ラウンド P3 指摘、`CLAUDE.md` 「コード修正時のドキュメント同時更新」節準拠) |
| [README.md](../README.md) | 動画操作セクション (あれば) | Space で再生/停止できる旨を追加。次リリースの更新履歴セクションにも「動画フルスクリーン Space を再生/停止に変更 (旧: 選択トグル)」を 1 行記載 |
| [htdocs/mimageviewer/manual/](../htdocs/mimageviewer/manual/) (動画関連ページ) | キーバインド表記 | Space を再生/停止として追加記載 |

**`self.checked` HashSet 自体は一覧で使うので残す** (= 削除するのは動画 fullscreen の trigger 経路だけ)。

### tile mode の Space 挙動 (Codex P1 指摘反映)

Enter は tile mode で **キーボードカーソル位置のタイルから再生** (`play_selected_video_tile`、
[src/app/native_video.rs:4389-4392](../src/app/native_video.rs)) する分岐がある。Space を素朴に
`handle_native_video_toggle_play_command` に繋ぐと、タイルグリッド背後の現在動画を再生/停止して
しまい、ユーザー期待 (= 選択タイルから再生) とズレる。

**方針**: tile mode の Space は **Enter と同じ tile-aware 挙動** にする。
- tile mode 中: Space = `play_selected_video_tile` (= Enter と同じ)
- tile mode でない: Space = `handle_native_video_toggle_play_command` (再生/停止)

これで「Space と Enter は文脈依存だが互いに等価」が成立し、tile mode の Enter 慣れしている
ユーザーが Space に乗り換えても同じ操作で動く。

### 画像モード

**画像モードのキーマップは変更しない**。Space = チェック を含めて現状維持。

理由: Enter = チェック (両モード共通) 案を当初検討したが、
- グリッド Enter = 動画を開く / 画像を開く という既存の挙動と動画フルスクリーン Enter で
  意味系統が分裂する
- Shift+Enter (= 外部プレイヤー、動画) との修飾子系統も整合しなくなる
両方を避けるため、両モード共通キーは導入せず動画側だけ慣習に合わせる方針に変更。

## ボタン棚卸し

[src/app/native_video.rs:4345](../src/app/native_video.rs) のキーハンドラ一覧から、ボタン化候補を整理:

| 機能 | キー | 頻度 | 判定 | 配置 |
|---|---|---|---|---|
| 前/次マーカー (chapter/bookmark/pin) | J / K | 高 | ✅ 追加 | 下 HUD |
| 前/次ファイル | ↑ / ↓ | 高 | ✅ 追加 | 下 HUD (連続再生の隣) |
| フレーム保存 (Ctrl+S 相当) | Ctrl+S | 中 | ✅ 追加 | カメラパレット内 |
| フレームステップ ←→ | Ctrl+Shift+←/→ | 中 | ✅ 維持 (パレット内に移植) | カメラパレット内 |
| ブックマーク追加 | B | 中 | ❌ 不要 | 左パネル + ホバーサムネで充足 |
| ピン追加 | P | 中 | ❌ 不要 | 同上 |
| レーティング | F1-F6 | 中 | ❌ 見送り | 画像でもアイコン未提供のため不整合になる |
| 外部プレイヤー | Shift+Enter | 低 | ❌ 不要 | 右クリックメニュー向き |
| DFS フォルダ移動 | Ctrl+↑↓ | 低 | ❌ 不要 | パワーユーザー機能 |
| 先頭/末尾アイテム | Home/End | 低 | ❌ 不要 | 同上 |

## 新レイアウト

```
┌─────────────────────────────────────────────────────────────────────┐
│ filename.mp4                                                        │  Top bar
│ 1920x1080  30fps  H.264  00:05:23           [VST3][F][S][⛶][×]      │  52px (現状維持)
├─────────────────────────────────────────────────────────────────────┤
│ ━━━━━━━━━●━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ │  Seek 行 (24px)
├─────────────────────────────────────────────────────────────────────┤
│ [W][▶] │ [|◀M][M▶|] │ [◀F][📋][💾][F▶] │ [L][⤴][↑][↓]    time │ [⚡][🔇][🚫][vol]│
│  再生     マーカー       キャプチャ常駐パレット   モード+ファイル切替         音量      │  Controls 行 (40px)
└─────────────────────────────────────────────────────────────────────┘
```

ボタングループ (左→右):

1. **再生グループ** (2 個): W (頭出し) / 再生・停止
2. **マーカーグループ** (2 個): 前マーカー [J] / 次マーカー [K]
   - マーカー 0 個の動画では **disabled** 表示 (非表示ではなく)。tooltip「マーカーがありません」
3. **キャプチャパレット (常駐)** (4 個): 前フレーム / コピー / 保存 / 次フレーム
   - 「カメラクリックで開閉」案は状態管理の複雑化を避けて却下、常駐固定
   - 旧フレームステップボタン (`draw_native_frame_step_button`) はパレット内に統合
   - **「保存」は overlay→App の新規イベント追加が必要** (Codex 第 1 ラウンド P2 指摘):
     現在 [src/video/native_presenter/mod.rs:1070](../src/video/native_presenter/mod.rs) の
     `NativeOverlayCommand` には `CopyFrameToClipboard` と `FrameStep` はあるが、保存系コマンドは
     未定義。フェーズ 5 で `SaveFrameToFile` (引数なし、現在フレームを既定キャプチャフォルダへ
     保存) を追加し、App 側で `save_video_frame_to_file` ([src/app/native_video.rs:4584](../src/app/native_video.rs))
     に dispatch するよう配線する。Ctrl+S キー経路 (= `save_video_frame_to_file` を呼ぶ) と挙動を
     完全一致させる

## 新規追加する overlay→App コマンド設計 (Codex 第 2 ラウンド P2 反映)

ボタンから App 側ハンドラへ到達するには `NativeOverlayCommand` enum
([src/video/native_presenter/mod.rs:1070](../src/video/native_presenter/mod.rs)) と
`NativeVideoOutputEvent` ([src/video/mod.rs:290](../src/video/mod.rs)) の双方に variant を追加し、
App 側 dispatch ([src/app/native_video.rs](../src/app/native_video.rs) の event loop) で
分岐を増やす必要がある。**フェーズ 4 / 5 / 6 着手前に enum 設計を固定**しておく:

| ボタン | 新コマンド | フィールド | App 側 dispatch 先 | 該当フェーズ |
|---|---|---|---|---|
| 前/次マーカー | `JumpMarker` | `{ next: bool }` | `jump_native_video_marker(fs_idx, next)` | フェーズ 4 |
| 保存 (カメラパレット) | `SaveFrameToFile` | (なし) | `save_video_frame_to_file(ctx, fs_idx)` | フェーズ 5 |
| 前/次項目 (ファイル切替) | (既存 `WheelNavigate` 改名) | `{ delta: i32 }` | `navigate_native_video_fullscreen(ctx, fs_idx, delta)` | フェーズ 6 |

### 前/次項目: `WheelNavigate` → `NavigateItem` への改名

既存の `WheelNavigate { delta: i32 }` は最終的に `navigate_native_video_fullscreen` を
呼ぶため、機能としてはボタンクリックでもそのまま使える。ただし **名前がホイール由来を
示唆しているため**、ボタン経路と共有する観点では命名が誤誘導になる。

**方針**: フェーズ 6 で `WheelNavigate` を **`NavigateItem`** に改名 (`delta: i32` フィールドは
そのまま)。ホイール経路もボタン経路も同じ variant を使う。改名は機械的な置換 (variant 名と
match arm のみ):

- [src/video/native_presenter/mod.rs:1079](../src/video/native_presenter/mod.rs) (enum 定義)
- [src/video/mod.rs:290 付近](../src/video/mod.rs) (`NativeVideoOutputEvent` の対応 variant)
- [src/app/native_video.rs:621](../src/app/native_video.rs) と
  [:1662](../src/app/native_video.rs) (event dispatch)
- ホイール send 元 (`overlay_draw.rs` 等の wheel handler)

### 前/次マーカー: `JumpMarker { next: bool }` 新設

App 側 `jump_native_video_marker(fs_idx, next: bool)`
([src/app/native_video.rs:4630](../src/app/native_video.rs)) のシグネチャに合わせて `next: bool`
1 フィールドのみ。`AddBookmarkAt` / `SetPinAt` のような `target_secs` は不要 (= 現在位置から
の相対遷移なので App 側で `player.position()` を引く)。

### 保存: `SaveFrameToFile` (引数なし)

App 側 `save_video_frame_to_file(ctx, fs_idx)` はフレーム取得から保存ダイアログ/自動保存まで
すべて App 側で完結するため、overlay からは引数不要のトリガーのみ送る。`CopyFrameToClipboard`
と対称な設計。
4. **モード+ファイル切替グループ** (4 個): ループ / 連続再生 / 前項目 / 次項目
   - 連続再生 = 「末尾で次ファイルへ」なので隣接で意味補強
   - 「左右 = 動画内シーク、上下 = ファイル切替」のキーボード規約に対応
   - 境界 (フォルダ末尾/先頭) では **enabled のまま**、クリックで「最後の項目です」「最初の項目です」トースト
   - **アイコン命名注意 (Codex P3)**: 同じ行に既に `[◀F][F▶]` (frame step) があるため、
     `[↑F][↓F]` のように `F` を流用すると frame/file の意味衝突になる。**ここはシンプルに
     上下矢印 `[↑][↓]` 単独 (フレーム群のような F 添字なし) + tooltip「前の項目 [↑]」
     「次の項目 [↓]」** に寄せる。あるいは stack/list 系の小アイコン (= 複数アイテムを示唆する
     視覚記号) を矢印に組み合わせる案もあるが、まず矢印単独で実装してユーザーフィードバック後に
     必要なら強化する
5. **再生情報・音量グループ** (右側、現状維持): time label / 速度 popup / mute / ノーマライズ / 音量 slider+label+limiter

**ボタン総数**: 左 12 個 + 右 4 個 = 16 個 (現行: 左 7 + 右 4 = 11、+5)。

### グルーピング描画

各グループ間に 1px の縦線セパレータ (アルファ 60〜80 のグレー) を入れる。
YouTube/Vimeo もボタンクラスター間を視覚的に分けている設計を踏襲。

### シークバー (上段) 詳細

- **高さ**: バー本体 4px、ヒット領域 24px (controls 行と分離したので余裕がある)
- **マーカー表示**: chapter/bookmark/pin 表示は現行と同じ (`draw_timeline_marker`)
- **ホバーサムネ**: バーの **真上** に出す (現状は controls 行の上)。anchor 計算をシフト
- **drag 中の挙動**: 現状維持 (`hover_preview_target_secs` 等)

## EOF トースト統一

**現状認識 (Codex P2 指摘反映)**: native 動画フルスクリーンの **↑/↓ キーとマウスホイール**経路には
既にトースト発火が実装されている ([src/app/native_video.rs:4942-4947](../src/app/native_video.rs)
の `navigate_native_video_fullscreen`、`WheelNavigate` イベントもここを通る)。
`show_native_video_overlay_toast` + `FsBoundaryHint::Edge` で「最後の項目です  [Ctrl]+[↓] ツリー順で次へ」
等が出る。

つまりフェーズ 1 でやることは **「新規追加」ではなく「表示されていないケースの再現と修正」**:

1. **再現テスト**: 動画フルスクリーンで ↑↓ 押下 / ホイール上下、それぞれで先頭・末尾到達時に
   トーストが出るかを実機確認。「動画では出ない」とのユーザー報告がどの経路に該当するか特定。
2. **想定される未対応経路** (要確認):
   - tile mode 中の境界 (= タイルカーソルが画面端でさらに方向キーを押した場合)
   - PageUp/PageDown / Home/End 境界
   - ホイール navigation 連射時の coalesce 中
   - `fs_nav_is_locked()` 中の押下 (= `video_tile_swap_pending` / `native_video_fast_swap_pending`
     待ち中、現状は early return で何も出ない)
3. **新規追加 (フェーズ 6 連動)**: 新規 **前/次ファイルボタン**のクリック経路も同じ
   `navigate_native_video_fullscreen` を呼ぶように配線すれば、境界トーストが自動で出る (再実装不要)。
4. **マーカーボタンは別扱い**: マーカー 0 個では disabled (= 「存在しない」と「境界にいる」を視覚的に分離)。

**実装作法**: 重複トーストや別経路だけの差分を作らないよう、新規境界 detect 箇所は必ず
`navigate_native_video_fullscreen` / 既存 `show_native_video_overlay_toast(native_boundary_hint_text(...))`
経由に統一する。

詳細仕様は [docs/fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) を併読。

## 左パネル (jump panel) アイコン順序入れ替え

[src/video/native_presenter/overlay_draw.rs:417-487](../src/video/native_presenter/overlay_draw.rs)
を確認した結果、ボタンは jump panel の **上端を右寄せ** で配置されている (`rect.width() - X` で
右端からのオフセット指定、X=100, 68, 36):

**現状 (左 → 右)**: 一括ブックマーク (X=100) → ピン (X=68) → ブックマーク (X=36)

コードコメント (line 416) には「配置: 既存の Pin / Bookmark の左側に並べる (- 100pt)」とあり、
**一括ブックマークが後から追加されて Pin/Bookmark の左端にぶら下げられた歴史的経緯**が分かる。
意図的な並びではない。

### 並びの判断と方針

論理的な読み順 (左 = 基本、右 = 拡張) で並び替える:

**新順序 (左 → 右)**: ブックマーク → ピン → 一括ブックマーク

- 左端 **ブックマーク** = 基本操作 (B キー、最頻)
- 中央 **ピン** = ブックマークの親戚的位置付け (代表サムネ用、P キー)
- 右端 **一括ブックマーク** = ブックマークの拡張機能 (低頻度バルク操作)

**反対意見の検討 (Codex 提案を含む)**: 「右寄せクラスターなら右端 = 一番押す = ブックマーク」
という空間アクセス論もある (mouse が画面中央から来るとき右端ボタンに最初に到達する)。
ただし button 間距離は 32px (= 約 8mm @96dpi) で、Fitts の法則的にも実体感差は小さい。
読み順の論理性を優先する。

### 実装

[src/video/native_presenter/overlay_draw.rs:417-487](../src/video/native_presenter/overlay_draw.rs)
の 3 つの rect の `rect.width() - X` のオフセット値を以下に入れ替え:

| ボタン | 旧 X | 新 X |
|---|---|---|
| ブックマーク (bm_rect) | 36 | 100 |
| ピン (pin_rect) | 68 | 68 |
| 一括ブックマーク (bulk_rect) | 100 | 36 |

コメント (line 416) も「ブックマーク (- 100pt) / ピン (- 68pt) の右側に並べる (- 36pt)」へ
書き換える。

## 実装フェーズ

各フェーズで 1 コミット = レビューしやすい単位に分割:

| # | 内容 | 依存 |
|---|---|---|
| 1 | 動画モード Space = 再生/停止 (チェック削除) + EOF トースト統一 | なし — UX 既知不具合の解消、HUD 変更前に独立して入れる |
| 2 | 左パネル順序入れ替え (ブックマーク → ピン → 一括ブックマーク) | なし — レイアウト変更と独立 |
| 3 | 2 段化 (純レイアウト変更、ボタン構成は現状維持) | なし — 基盤 |
| 4 | マーカーボタン追加 + 旧フレームステップボタン削除 | フェーズ 3 |
| 5 | カメラパレット 4 ボタン常駐化 (前フレーム / コピー / 保存 / 次フレーム) | フェーズ 4 |
| 6 | 前/次ファイルボタン (ループ群の隣) | フェーズ 3 |

フェーズ 1 + 2 はレイアウト変更と無関係なので先行投入。フェーズ 3 を基盤として 4-6 が積み上がる。

## 実装上の注意

### Y 座標の定数化

現在 `46.0` が複数箇所にハードコードされている ([src/video/native_presenter/mod.rs:5614,5616,5691,5693](../src/video/native_presenter/mod.rs))。
2 段化でこれが `24.0 + 40.0 = 64.0` に変わるが、ハードコード散在は退行リスク。

**フェーズ 3 でまず定数化** (`const HUD_BOTTOM_HEIGHT: f32 = ...`、`const HUD_SEEK_ROW_HEIGHT: f32`、
`const HUD_CONTROLS_ROW_HEIGHT: f32`) → 各箇所を参照に置換 → そのうえで値を変更する流れにする。

### compute_hud_regions の更新

[src/video/native_presenter/mod.rs](../src/video/native_presenter/mod.rs) の `compute_hud_regions` が
返す HUD region は bottom HUD 全体を 1 つの rect として扱っている。2 段化後も
**seek 行 + controls 行を 1 つの rect** として返す (内部的に高さだけ増える)。

VST GUI の z-order との衝突は HUD HWND 自体は同じなので追加対応不要。

### activation zone (50ms cursor polling) は触らない (Codex P2 指摘修正)

`cursor_polling_tick` ([src/video/native_presenter/mod.rs:2490](../src/video/native_presenter/mod.rs))
で「画面下端の帯」に cursor が入ったら HUD を表示する判定があるが、その帯は **HUD の実高さとは
独立した 220pt 固定** (`H-220..H`)。HUD region (= 実 UI rect)・excluded rect・描画位置は HUD 高さに
合わせて 64px に変えるが、**activation zone は別定数として扱い触らない**。

旧版で「HUD 高さに合わせて 64px に拡張」と書いていたが、これは逆に zone を狭める誤誘導に
なるため修正。activation zone を変更したい場合は別タスクとして UX 検証 (= cursor がどこまで
近づいたら HUD を出すか) を独立して行う。

### hover サムネの Y 座標

`hover_preview_target_secs` の表示 anchor (`overlay_draw.rs` 内) は現在 controls 行の Y を
基準にしている。seek 行が上に分離するので、anchor もそれに合わせて上にシフト。
**サムネ表示は seek bar の真上** = seek 行 top - サムネ高さ - margin。

### normalize progress の Y 座標

`draw_native_normalize_progress` は HUD の上に重なって出る。2 段化で HUD 上端が
20px 上がるので、normalize progress の上限位置もそれに合わせる。

### VST3 panel の Y 座標

`native_vst3_panel_rect` も HUD 上に乗る形なので同様に調整。

### ボタン横幅予算 (1280px windowed)

左クラスター 12 個 × (28px ボタン + 8px gap) = 432px
右クラスター time(132) + 速度(43) + mute(28) + norm(28) + 音量(144) + label(60) + limiter(14) + gaps = ~470px
合計 ~902px (gap + side pad 込みで ~950px)

1280px 窓では seek bar 自身は別行なので影響なし、controls 行は 1280 - 950 = 330px が time 周辺余白に。

### 狭幅ウィンドウのフォールバック戦略 (実機フィードバック反映: 左側優先)

ウィンドウを狭めるにつれて controls 行のボタンが詰まるが、**「左にあるボタンほど残す」**
という優先順位で段階的に右側から消す。ユーザーの直感 (= 左 = 再生コア機能、右 = キャプチャ等
の周辺機能) と一致させる。実装は `CompactionTier` enum 5 段階:

| Tier | 状態 | 残るボタン | 最小窓幅 (実測) |
|---|---|---|---|
| Full | 全ボタン + フル右クラスター | `[W][▶] | [L][⤴][↑][↓] | [|◀M][M▶|] | [◀F][📋][💾][F▶]` + 時間/速度/音量 | ~965pt |
| NoCapture | キャプチャパレット (4 ボタン) 非表示 | `[W][▶] | [L][⤴][↑][↓] | [|◀M][M▶|]` + フル右 | ~813pt |
| NoMarkers | + マーカー非表示 | `[W][▶] | [L][⤴][↑][↓]` + フル右 | ~733pt |
| NoFileNav | + 前/次項目非表示 | `[W][▶] | [L][⤴]` + フル右 | ~661pt |
| Minimal | + 右クラスター縮小 | `[W][▶] | [L][⤴]` + 時間/速度/ミュート/ノーマ/音量スライダ(100pt) | ~535pt |

判定: 各 tier 最小幅を左/右クラスターの実数値から計算 (`total_full` / `total_no_capture` /
`total_no_markers` / `total_no_file_nav`) し、`overlay_width_points` が収まる最大 tier を選ぶ。
旧版の「width < 1100pt 単閾値」では最小窓 (640pt) で overlap した実害を受けて、実測ベースに
切り替えた経緯あり (Codex 第 4 ラウンド P2 指摘)。

**優先順位の根拠**:
- ユーザー要求「キャプチャパレットより前/次ファイル移動のほうが使うので、優先度を下げてほしい」
- 左端 = 再生コア (W = 頭出し、▶ = 再生/停止) は常時保持
- キャプチャパレット (4 ボタン全部) は Ctrl+S / Ctrl+Shift+←/→ で完全代替可能
- マーカー (J/K)、前/次項目 (↑/↓) もキーボード代替あり
- 最後に右クラスター (時間 / 速度 / 音量) を縮小して 640pt 最小窓に対応

**camera-only 中間 tier は採用せず**: 旧設計では「キャプチャパレットを camera 単独に縮退」する
中間 tier を持っていたが、ユーザー直感「camera より前/次項目のほうが頻度高い」と反するため、
キャプチャパレットは丸ごと on/off にする方針に変更。

## Codex に意見を求めたい点

特に判断が割れる可能性のあるポイント:

1. **キー再割り当ての影響範囲**
   - 動画モード Space = 再生/停止 (チェック削除)、Enter = 再生/停止 (維持) の方針は妥当か
   - 「動画フルスクリーンからチェック操作を削除」(= チェックしたい場合は Esc で一覧へ) の
     ユーザー体験はどうか。チェック workflow を fullscreen でやるユースケースを過小評価していないか
   - 画像モードに一切変更を入れない判断は妥当か (= 両モード共通キーを諦めて regression リスクをゼロに)

2. **2 段化 HUD のヒット領域 24px**
   - seek bar 自体は 4px、それを含む行 24px をすべて drag 領域として使う設計
   - controls 行と分離したので「seek bar の上下を間違えてクリックする」事故はほぼ起きない想定だが、
     **24px は過剰で 16-18px くらいが適切**という意見もありそう

3. **カメラパレットを常駐 4 ボタンにする判断**
   - 「クリックで開閉」「ホバーで開閉」両案を却下、常駐固定とした
   - 視覚ノイズ評価 (4 ボタンが常に並ぶことの違和感) を独立した視点で確認したい
   - 1 クリックコピーの動線は保たれる (📋 ボタンが即コピー) が、慣れたユーザーにとって
     ボタン位置が変わる影響はないか

4. **前/次ファイルボタンを下 HUD に置く判断**
   - 上バーの空き中央でなく、下 HUD の連続再生隣に配置する
   - 「左右=動画内、上下=ファイル切替」のキーボード規約と対応する論理性は妥当か
   - 上バーがファイル情報だけになることでの上下バー役割分担の明瞭化 vs
     下 HUD ボタン数増加 (12 個) のトレードオフ評価

5. **マーカー無し動画でマーカーボタンを disabled (非表示でなく)**
   - レイアウト揺れを避けるため disabled 表示
   - 「非表示にしてレイアウトを動的に縮める」案との比較で disabled が妥当か

6. **左パネル順序入れ替え (ブックマーク → ピン → 一括ブックマーク)**
   - 頻度順の根拠は妥当か (= ブックマーク追加 > ピン追加 > 一括ブックマーク)

7. **EOF トースト: 未表示経路の特定**
   - 既に `navigate_native_video_fullscreen` 経由 (↑↓ キー / ホイール) ではトーストが出る
     ことが判明。「動画で出ない」とのユーザー報告は別経路 (tile mode / Home/End /
     swap pending 中の no-op / その他) と推定
   - フェーズ 1 着手前に**実機再現**で未表示経路を特定する必要あり

8. **フェーズ分割の順序**
   - フェーズ 1 (キー再割り当て + EOF トースト) をレイアウト変更より先に入れる判断
   - フェーズ 5 (カメラパレット) を 4 (マーカーボタン追加) の後にする依存関係
   - 各フェーズが独立コミットでテスト可能なサイズか

## 関連ドキュメント

- [video-architecture.md](video-architecture.md) — 動画再生サブシステム全体。**native presenter の HUD は
  `src/video/native_presenter/{mod.rs, overlay_draw.rs}` で描画される**
- [keymap-spec.md](keymap-spec.md) — キーマップ仕様。本リデザインで Space/Enter の挙動を変更する箇所を
  実装時に同時更新
- [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) — 画像/動画/検索結果を
  またぐナビゲーション統一仕様。EOF トースト統一はこの方針の延長

## 改訂履歴

- **2026-05-25 初版** — 計画提出
- **2026-05-25 Codex 第 1 ラウンドレビュー反映** — 以下を取り込み:
  - P1: G3 ゴール記述の `Enter = チェック` 残骸を訂正、Space 再割り当ての同期対象 (ui_fullscreen
    コメント / HUD tooltip / 中央ヘルプ / routing test / keymap-spec / manual) を列挙、
    tile mode の Space 挙動を Enter と同じ tile-aware として明記
  - P2: EOF トーストは未実装ではなく既存 (`navigate_native_video_fullscreen` 経由)、
    フェーズ 1 を「追加」ではなく「未表示経路の特定と修正」に reframe、`NativeOverlayCommand` に
    `SaveFrameToFile` 系を追加する配線を明記、activation zone は HUD 高さと独立 (220pt 固定) と
    訂正
  - P3: `[↑F][↓F]` の F 衝突を回避して `[↑][↓]` 単独 + tooltip 方式に変更、狭幅
    ウィンドウのフォールバック戦略 (当初は 6 段階の削減優先順を予定。後の実機
    フィードバックで「左側ボタン優先」の 5 段階 tier 設計に再整理 — 現行版は
    本文「狭幅ウィンドウのフォールバック戦略」セクションを参照)
  - 左パネル順序: コード実機確認の上、`bm_rect` / `pin_rect` / `bulk_rect` の rect オフセットを
    入れ替える具体的な実装手順に書き換え (歴史的経緯のコメントも訂正対象に追加)
- **2026-05-25 Codex 第 2 ラウンドレビュー反映** — 以下を取り込み:
  - P2: overlay→App コマンド設計セクションを新設。`JumpMarker { next: bool }` 追加、
    `WheelNavigate` → `NavigateItem { delta: i32 }` 改名、`SaveFrameToFile` (引数なし) 追加の
    3 件を enum 設計レベルで明記。フェーズ 4 / 5 / 6 着手前に固定する
  - P2: Space 再割り当ての同期対象に `ui_fullscreen.rs:7073-7081` (`handle_video_input`、旧/
    フォールバック動画経路) を追加。これまで `ui_fullscreen.rs:2777-2781` 単独だったが、
    実際にはフォールバック経路にも「Space を画像選択に流す」前提のコメント・実装が残っている
  - P3: `CLAUDE.md` 「コード修正時のドキュメント同時更新」節に従い、ドキュメント更新対象に
    `docs/spec.md` と `README.md` (更新履歴 + 動画操作セクション) を追加
