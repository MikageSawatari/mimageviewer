# 引き継ぎ — 2026-08-17〜18 セッション

このセッションで master へ 12 コミット入れた。**未解決の調査 2 件 (③④) が残っており、
それが次セッションの主題**。実機確認待ちも数件ある。

## 0. まず読むもの

- 本書 §3 (未解決 ③④) — ここから始める
- `docs/next-release-backlog.md` の §1.31 / §4.2 / §1.85 / §1.86 / §1.88〜§1.91
- `docs/briefs/codex-acquire-readiness-gate-brief.md` (= §1.31-B0、計測先行に差し戻した正本)

## 1. 入ったもの (master、すべてフルゲート exit 0)

| commit | 内容 | 状態 |
| --- | --- | --- |
| `a7923080` | §1.85-A no-surface のテクスチャ配送に回帰テスト | 完了 |
| `0b645861` | §1.86 配送を 5 exit から単一 transaction へ | 完了 |
| `4e6e5efe` | §1.31-A message-dispatch 位相と render 位相の分離 | 実機確認一部済 |
| `9373a947` | §1.31-B を計測先行 (B0) へ差し戻し | 判断のみ |
| `4606c318` | §1.88 見開きずらし後のページ送り固着 (P1 退行) | **実機確認済み・完了** |
| `95469d83` | §1.89 元画像プレビュー中の 4〜5 秒停止 | 実機確認待ち |
| `dcf17532` | §4.2 計装 (2 回目) | — |
| `923a9c6d` | レイアウト 2 件 (3.1.1) | ▲/▼ は OK、キーボードは追加修正済 |
| `0940ff1e` | §1.90 アニメ先読み方針 (1 フレーム先読み / アーカイブ内アニメ) | 表示 OK |
| `79323c5e` | §1.91 元画像ホールドをナビ中は無効 + §4.2 計装 (3 回目) | **§1.91 は効いていない (③)** |
| `0e2b5479` | キーボード indent / アニメ進捗の経路差 / ③④ の計装 | 進捗表示 OK |
| (未コミット) | キーボード indent に `item_spacing.x` を加算 | 要ゲート・コミット |

コードを変えた分はすべて **mutation で「テストが実際に退行を捕まえる」ことを確認済み**。

### 特筆すべき成果

- **§1.30 が決定的に再現可能になった**。`RedrawRequested` を inline paint に戻す mutation で
  Windows プロセステストの `SendMessageTimeoutW` が timeout する。
  従来「実機で 2 回再現、原因未確定」だった現象がテストとして固定された
- **ゲートに入っていなかったテストを 2 群発見**。vendored egui-wgpu (6 件) と
  vendored eframe (3 件、うち 1 件は tray 就寝ガード) は workspace `exclude` のため
  `test-full.ps1` から一度も走っていなかった。両方とも無フィルタで組み込み済み
- **§1.90 で Codex が EXIF orientation の罠を発見**。アニメデコーダは orientation を
  適用しないが静止画経路は適用するため、昇格時に画像が回る手前だった

## 2. 実機確認待ち

`.\scripts\build-dev.ps1` 後に
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
⚠ 実利用中の `%APPDATA%\mimageviewer` を使う。起動前にインストール版／常駐版を終了。

- **キーボード図** — Q と A の左端が揃うか (未コミット分を含めてビルドすること)
- **§1.89** — 右 Ctrl 押下中に 4〜5 秒停止が無いか (分析モードでも)
- **§1.90** — アニメ WebP 多数の ZIP でメモリが跳ねないか / アーカイブ内 GIF・APNG がアニメするか
- **§1.31-A 残り** — 動画再生、長時間の tray 常駐

## 3. 未解決 ⚠️ ここが次セッションの主題

### ③ 右 Ctrl 押下中のページずらしが戻り方向だけ遅い

**実測 (§1.91 適用後の最新セッション)**:

| 操作 | 元画像プレビュー | 速度 |
| --- | --- | --- |
| Left | あり (453 件) | 13.2 手/秒 (速い) |
| Right | あり (693 件) | **5.5 手/秒 (遅い)** |
| Left / Right | なし | 14.5〜15.4 手/秒 |

遅い区間は `texture_choice source=nav_holdover` が 724 件で支配的 = 前の表示単位を出し続けている。

**機構 (確定している部分)**:

1. 元画像プレビュー中は `fs_page_turn_ordinary_context_blocker` が `original_preview` を返し、
   パススルー表示 (低解像度の代理) が**正しく**無効になる
2. 通常はそのパススルーが先読み窓の非対称 (`prefetch_forward=12` / `prefetch_back=4`) を
   隠している。プレビューがそれを剥がすので戻り方向 (4 ページ) の差が露出する
3. target が ready にならない間 holdover が出続ける

**§1.91 が効いていない**。`original_preview_active` の先頭に
`fs_navigation_sequence_blocks_new_target()` の除外を入れた (`79323c5e`) のに、
`page_turn_decision` の reason に `original_preview` が **1095 件**出続けている。

**計装が判別に失敗している**: `fs/original_preview_blocker_summary` は
**全 39 行で `original_preview_returns=0`**。summary の `viewport` が `"FFFF"` (root) で、
blocker が走るのは `"5361"` (フルスクリーン)。**カウンタを viewport ごとに溜めて
別 bucket を出している疑い**。

**次にやること (Codex 意見待ち)**: 候補 2 つ。

- (i) 計装の viewport スコープを直す
- (ii) 計装をやめ、`original_preview_active` の呼び出し元をソースで全部洗い、
  「§1.91 の除外を通らない経路」を特定する

**私 (ClaudeCode) は (ii) を推した**。この周辺では観測より読解の方が確実だった。
また `original_preview_active_for_frame` の frame memo
(`(frame_nr, items_generation, idx, viewport)`) が、シーケンス生成**前**に `true` を書いて
フレーム中保持する筋も潰していない。

**利用者判断 (確定済み、変えないこと)**:

- 右 Ctrl は**静止して画像を見ているときに元を確認する用途**である。
  移動中に補正が乗っていることは**仕様として問題ない** (2026-08-17)
- したがって「移動中は元画像ホールドを適用しない」という方針自体は正しい。
  効いていないのが問題

### ④ 音声モードから戻る Z が効かない (backlog §4.2)

**計装 4 回目でようやく範囲が絞れた**。`[fs-key]` が Z を見た直後・
`handle_fs_key_input` 呼出し前のログで **guard 6 つ全て通過**:

```
video_audio_mode=Some(10) fs_music_view_active=true wants_keyboard_input=false
ime_input_active=false music_bookmark_modal_open=false music_normalize_modal_active=false
fs_context_menu_idx=None viewport="5361"
```

一方 `handle_fs_key_input` 冒頭の診断は `egui_z_pressed=false` / `peek_action=false` /
`any_viewport_z_down=false`、outcome は `action_not_consumed`。

| 地点 | egui の Z |
| --- | --- |
| `[fs-key]` ログ時点 | **ある** (`fs_summary` が `Z:down` を報告、source=egui_events) |
| `handle_fs_key_input` 冒頭の診断 | **無い** |

**その間で誰かが egui の Z を消費している。**
この範囲に `consume` を含む行は `consume_context_shortcuts_help_key` の**1 つだけ**。
ただし outcome が `context_shortcuts_help_preempted` ではなく `action_not_consumed` なので
**断定できない**。

**否定された候補** (backlog §4.2 の記述を更新済み):

- (A) フォーカス喪失 → `focused=true` で否定
- guard 6 つ (music view / IME / TextEdit / モーダル / コンテキストメニュー) → 全通過で否定

**次にやること (Codex 意見待ち)**: `handle_fs_key_input` の入口 / help キー判定後 /
診断採取地点の 3 点で egui の Z の有無を出して二分する案を検討中。
`fullscreen_shortcut_event_summary` 自身が消費している可能性も潰していない
(診断はこれを 2 回目に呼んでいる)。

### 3.1 Codex Sol の見解 (2026-08-18、両方とも裏取り済み) ⚠️ ここから始める

#### ③ = frame memo の評価順。ソース読解で確定できる

`page_turn_decision` は**入力処理・ナビシーケンス生成より先に評価される**
(`ui_fullscreen.rs` 12787 → 14925 → 23464)。したがって
**シーケンス生成前に `original_preview=true` が frame memo に入り、そのフレーム中残る**
= 候補 (a) が本命。別の除外漏れ経路 (b) より時間順序が濃厚。
**viewport 集計の修正は後回しでよい。**

→ 次の一手: `original_preview_active_for_frame` の memo が書かれる時点と
`fs_navigation_sequence_blocks_new_target()` が true になる時点の前後関係をソースで確定し、
**評価順に依存しない形**へ直す。時間窓で吸収しない (憲法 5)。

#### ④ = `FsZoomMode` が context 判定より前に Z を消費している。**原因確定**

犯人は `consume_context_shortcuts_help_key` では**なく**、先行する `update_fs_zoom_mode_keys`
(`ui_fullscreen.rs:17402` から呼ばれる)。

```rust
// コンテキスト外でも先に edge を消費し、連結表示では無言抑止せず rising edge に一度だけ
// 理由を表示する。                                    ← 意図的にそうしている
let (z_press_event, z_release_event) =
    self.keymap.take_key_hold_edges(ctx, KeyAction::FsZoomMode);   // 6258: ここで Z を消費
...
if !self.fs_zoom_mode_context_ok(fs_idx) {                          // 6272: Video/Audio 除外はここ
```

音声モードでは item が Video なので `fs_zoom_mode_context_ok` は false になるが、
**その時点で Z は既に消費済み**。だから音楽ビューの Z 分岐 (`consume_action_no_repeat`) は
何も見つけられず、`egui_z_pressed=false` / `action_not_consumed` になる。

`fullscreen_shortcut_event_summary` は `ctx.input` で読むだけなので**消費しない** (無罪)。
**三点二分ログは不要。** これは backlog §4.2 の候補 (B) が当たっていたということ。

→ 次の一手: **consume-then-check の順序を直す**。「コンテキスト外でも先に消費する」のは
連結表示で理由を出すための意図的な設計なので、単に順序を入れ替えると連結表示側の挙動が
変わる。**設計判断が必要** — 消費前に所有権を判定するか、除外コンテキストではエッジを
消費しない (別 consumer へ渡す) か。利用者に方針を確認してから着手する。

## 4. 保留

- **§1.31-B0** (acquire / configure / Present の計測) — ③④ が片付いてから。
  正本 = `docs/briefs/codex-acquire-readiness-gate-brief.md`。
  **acquire だけ先に直すのは誤り**と判明している (`configure` に `INFINITE` がある)
- **preferences ページのスナップショット整備** — 今回のレイアウトバグ 2 件は
  スナップショット 35 件すべてを素通りした。キーボード indent の描画側 1 行と
  ▲/▼ の列予約は現在テストで守れていない
- **§3.3 送り中の AI アップスケールを待たない / 打ち切る** — §1.89 で切り出した別項

## 5. このセッションで踏んだ罠 (同じ轍を踏まないため)

1. **§1.31-A の位相分析で 3 点誤った** — 「入口 3 は既に外側」「`RepaintNow` は resize 専用」
   「A だけで §1.30 が閉じる」。Codex のレビューで訂正
2. **§1.31-B の着手順を誤った** — 「acquire は API が届くから先に直せる」→ `configure` に
   `INFINITE` があり目的を達しない。計測先行へ差し戻し
3. **§4.2 の症状を読み違えた** — 「Z が効かない」を入力経路と読んだが、ログでは Z は
   効いていて直後に勝手に再突入していた
4. **計装の抑制条件を調査対象の信号に依存させた (2 回)** —
   **原則: 抑制条件を、調査している信号そのものに依存させてはいけない。**
   `docs/keymap-spec.md` に明文化済み
5. **Ctrl 左右差を「仕様どおり」と結論した** — 機構 (先読み窓 12:4) は正しかったが、
   `[fs-key]` が左右 Ctrl を区別しないため、方向差として測ったものが実際はプレビュー有無の差
6. **コミット後のリビルド漏れ** — レイアウト修正が入っていないバイナリを渡した。
   **以後は必ず `build-dev.ps1` を回してから実機依頼する**
7. **キーボード indent の値を 2 回間違えた** — `add_space` は item_spacing を足さず、
   直後のキーも先頭項目扱いで leading spacing が付かない。正解は
   **代替キーの幅 + `item_spacing.x`**

## 6. 運用メモ

- 実装は Codex Sol (`codex exec -c model="gpt-5.6-sol" --sandbox workspace-write`) に振る。
  私はブリーフ / レビュー / テスト / 統合
- **検収では mutation を回す** — 修正を戻してテストが落ちるかを確認する。
  今回これで「テストが実は守っていない」を 2 回検出した
- Codex は `.git` を書けないので、コミットは私が pathspec commit で行う
- 実機依頼の前に**必ず `build-dev.ps1`** (§5-6)
