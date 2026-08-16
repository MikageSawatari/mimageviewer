# §1.31-A — message-dispatch 位相と render 位相を分離する

対象: [next-release-backlog.md](../next-release-backlog.md) §1.31 の**前半**。
前提 = §1.85-A (`a7923080`) と §1.86 (`0b645861`) が master へ merge 済み (`96840fab`)。

**§1.31-B (acquire / Present の待ちに上限を持たせる) には手を出さないこと。**

## 0. 位置付け — A は §1.31 を完了させない

§1.31 は「wndproc の内側で GPU 待ちのある描画をする構造」。これを 2 つに分ける。

- **§1.31-A (本ブリーフ)**: message-dispatch 位相と render 位相の分離。
  **同期メッセージ自身が GPU 待ちを開始する経路**を除去する。
- **§1.31-B (別途)**: presentation と message service latency の上限制約。

**A を merge しても §1.31 は未完である。** 完了報告でそう書かないこと。

理由: UI スレッドが**外側**の paint で GPU を待っている間も message pump は止まる。
その最中に他スレッドが `SendMessage` すれば、sender は GPU 待ちの終了まで待たされる。
A が消すのは「同期メッセージが GPU 待ちを**開始させた**」経路であって、
「既に進行中の GPU 待ちに巻き込まれる」ではない。後者は B の仕事。

さらに §1.30 の採取時に main が受けていた `msg` は未確認で、backlog 自身が
resize の証拠は無いと書いている。**「§1.30 のスタックを閉じた」と言えるのは
§6.1 の Windows gate test が通ってから。**

## 1. 現状 — 同期 paint の入口は 3 つ。外側の位相は存在しない ⚠️

`vendor/eframe/src/native/run.rs` の `run_ui_and_paint` 呼び出しは 3 箇所:

| # | 場所 | 実際の位相 |
| --- | --- | --- |
| 1 | [run.rs:411](../../vendor/eframe/src/native/run.rs:411) `WindowEvent::RedrawRequested` | winit の `WM_PAINT` から同期 dispatch。**wndproc の内側** |
| 2 | [run.rs:129](../../vendor/eframe/src/native/run.rs:129) Windows 限定 `RepaintNow` の再 paint | `handle_event_result` 経由。**wndproc の内側** |
| 3 | [run.rs:233](../../vendor/eframe/src/native/run.rs:233) 不可視 / 最小化窓の direct paint | `check_redraw_requests` 経由。**外側とは限らない (下記)** |

**入口 3 を「既に外側の境界」と誤読しないこと。** `check_redraw_requests` は
[run.rs:198](../../vendor/eframe/src/native/run.rs:198) `handle_event_result` の末尾と
[run.rs:396](../../vendor/eframe/src/native/run.rs:396) `new_events` から呼ばれる。
つまり `WindowEvent` の同期 callback 中にも入口 3 へ到達できる。

さらに `WinitAppWrapper` には **`about_to_wait` の実装が無い** (trait 既定の no-op)。
`EframeWinitApplication` はそれを素通しするだけ。
**外側の paint 位相は現在どこにも存在しない。A はそれを新設する作業である。**

### 1.1 `RepaintNow` は resize 専用ではない ⚠️

producer は 3 つある:

- [wgpu_integration.rs:432](../../vendor/eframe/src/native/wgpu_integration.rs:432) — 初回 `resumed` (bootstrap)
- [wgpu_integration.rs:905](../../vendor/eframe/src/native/wgpu_integration.rs:905) — 非ゼロ `Resized`
- [winit_integration.rs:173](../../vendor/eframe/src/native/winit_integration.rs:173) — AccessKit initial tree

`EventResult::RepaintNow(WindowId)` は**理由を失っている**。位相を分けるには
reason の型付けが要る (§2 の 5)。

### 1.2 無限 `WM_PAINT` は起きない (確認済み)

winit ([event_loop.rs:1276-1298](https://docs.rs/winit/0.30.13/)) は `RedrawRequested` を
送った後に `DefWindowProcW(WM_PAINT)` を呼ぶ。これが update region を validate する。
callback 中に `request_redraw()` された場合だけ `RDW_INTERNALPAINT` で再 arm する。

したがって **eframe が描画せずに戻っても無限 `WM_PAINT` にはならない**。
backlog / 過去の設計メモにある「`BeginPaint` / `EndPaint` を決めないと無限になる」は、
この winit 実装に対しては**撤回してよい**。

ただし validation が保証するのは OS の update region 消費だけ。eframe 側には別途
scheduler の liveness 契約が要る (§2)。

## 2. やること

1. `WindowEvent::RedrawRequested` は **per-window の dirty を記録して戻る**。paint しない。
2. **入口 3 の不可視 direct paint を `handle_event_result` から除去する。**
3. `check_redraw_requests` を 2 つに割る。
   - OS redraw の scheduling (どこから呼ばれてもよい)
   - **本当の外側 (`about_to_wait`) だけで行う paint drain**
4. `handle_event_result` から paint drain を**再帰的に呼ばない**。結果は state reducer へ戻す。
5. `RepaintNow` に**原因を持たせ**、bootstrap / AccessKit / resize を分離する。
6. dirty state は **WindowId / viewport / surface generation ごと**に持つ。
   App-global の scalar な render state にしない。

## 3. resize 例外 — 不変条件の言語化 (これが凍結ルール上の要) ⚠️

winit は modal size/move loop 中に外側ループを pump しない
(`WM_ENTERSIZEMOVE` / `WM_EXITSIZEMOVE` は private marker を上下させるだけ)。
したがって resize の paint を外側へ送ると**ドラッグ中に内容が固まる**。

例外を残すが、**「VST の引き金は resize ではないから残す」という理由では症状に合わせた
carve-out であり、凍結ルール上の合意対象外**。次の不変条件として書き、コードのコメントにも残すこと:

> message-dispatch 位相から renderer へ入れるのは、当該 window の**非ゼロ client resize に
> 応答する `InteractiveResize` frame に限る**。判定は **event provenance のみ**を使い、
> 時刻・geometry・focus・VST・detached 述語から推測しない。それ以外の redraw、bootstrap、
> AccessKit、不可視窓の進行は、すべて外側の render 位相へ送る。

**厳密な `MARKER_IN_SIZE_MOVE` 限定は採らない。** winit の非公開状態であり、
取りに行くと winit の vendor patch (3 つ目の vendored crate) か全 HWND subclass が要る。
後者は multi-window lifecycle を増やすので不可。**本ブリーフの例外は「非ゼロ `Resized`」**
であり、これは現行より厳密に狭い (今は全 redraw が inline)。狭め方の追い込みは測定してから別途。

### 3.1 immediate viewport は例外範囲に含まれる

現行の immediate viewport は root の UI pass 中に子 viewport も同期描画する
([wgpu_integration.rs](../../vendor/eframe/src/native/wgpu_integration.rs) の nested 経路)。
resize 中に root だけを paint することは現構造ではできない。したがって

> inline resize frame が作る immediate viewport の render subtree も例外範囲に入る

と明記すること。ここが残る露出面であり、**B の上限が要る箇所**である。

## 4. 触ってよいファイル

- `vendor/eframe/src/native/run.rs`
- `vendor/eframe/src/native/winit_integration.rs` (`EventResult` の reason 型付け)
- `vendor/eframe/src/native/wgpu_integration.rs` (`RepaintNow` producer の分類)
- `vendor/eframe/Cargo.toml` (test target / dev-dependency が要る場合)
- `scripts/test-full.ps1` (vendored eframe のテスト段。egui-wgpu の段と同じ形)
- `docs/next-release-backlog.md` / `docs/detached-rework-plan.md`

**`vendor/egui-wgpu/` と `src/` には触れない。** winit / wgpu を vendor しない。

## 5. 落とすと壊れるもの (重要度順)

1. **初回表示**。root window は白フラッシュ防止のため最初 invisible で、
   `resumed → RepaintNow` の初回 paint 後に visible になる。これを通常の
   invisible 100ms throttle に入れると起動が遅れるか bootstrap が詰まる。
   **bootstrap は外側で即 drain し、app 要求の hidden throttle と分ける。**
2. **不可視窓の進行と tray の就寝**。hidden direct paint は `Visible(true)` 等の
   viewport command を消化するため必要。一方で**要求の無い tray idle に heartbeat を
   作ってはいけない**。既存の throttle unit test 3 件を維持し、dirty の無い window を
   outer drain の対象に入れない。
3. **immediate viewport の親子関係**。immediate child の単独 `RedrawRequested` は
   自身を描けず親へ `RepaintNext` を返す。**global な `Painting` bool で遮断すると
   detached / fullscreen が壊れる。** scheduler claim は per-window とし、
   immediate recursion は親 frame の render subtree として許可して、
   別の scheduler 再入とは区別する。
4. **`EventResult` の意味**。`RepaintNow` / `RepaintNext` / `RepaintAt` を単なる時刻へ
   潰さない。bootstrap・resize・通常 coalescing が混ざる。
5. **screenshot**。A は UI pass を外側へ遅らせるだけなので通常は問題ない。
   ただし B で non-submit outcome へ落ちる場合に備え、`actions_requested` からの
   除去タイミングを壊さないこと (現在は painter 呼び出し前に除去している)。
6. **texture delta**。§1.86 の `begin_delivery` / `finish_delivery` を必ず通す。
7. **surface generation / resize**。dirty は最新 size と surface generation を持つ。
   古い readiness wake で再生成後の surface を paint しない。
8. **native fullscreen / detached**。hidden main root / active immediate detached /
   passive deferred viewport / native fullscreen 中の egui root / tray 格納・復帰を
   **別々に**確認する。
9. **`windows_next_repaint_times`**。scheduled repaint と OS damage dirty は別概念。
   future repaint を早めない規則、同一 WindowId の coalescing、close 済み window の
   dirty 除去を明示する。

## 6. 検証設計 (実装前に読むこと)

### 6.1 Windows process test — A の主証明 (最重要)

**性質**: presentation が止まっていても wndproc は有限時間で返る。

- 最小の eframe test window を**子プロセス**で起動する。
- test 専用の painter gate を acquire の直前で閉じる。
- **別スレッド**から `SendMessageTimeoutW` で test message を送る
  (同一スレッド / 同一キューでは timeout が無視される)。
- test subclass はその message の中で `RedrawWindow(.., RDW_INVALIDATE | RDW_UPDATENOW)` を
  呼び、同期 `WM_PAINT` を発生させる。**`WM_PAINT` を直接送らない**
  (Microsoft が application からの直接送信を非推奨としている)。
- 検査する順序:
  ```text
  SendMessageTimeout return   <   outer paint gate enter
  ```
- flag は `SMTO_BLOCK | SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT`、明示 HWND、timeout 500ms。
  **`SMTO_NOTIMEOUTIFNOTHUNG` は使わない。**
- 成否にかかわらず gate を解放する。親プロセス側にも総時間 watchdog を置く。

### 6.2 unit test — 純 scheduler reducer

vendored eframe に painter 非依存の純 scheduler reducer を切り出し、最低限:

- `RedrawRequested` は dirty 化だけで paint しない
- 同一 window の damage が coalesce される
- 複数 window の dirty が独立している
- **外側位相だけが drain する**
- paint 中の再要求が次 epoch へ残る
- hidden は**要求済みの作業だけ**を 100ms throttle する
- **idle hidden window に heartbeat を作らない**
- close した window の dirty / readiness wake を破棄する
- immediate child が親の repaint へ昇格する
- bootstrap が hidden throttle を迂回する
- **resize reason だけ inline を許可する**

### 6.3 使えるが主証明にはならないもの

- **アプリ内蔵テストスクリプト** ([test-script-runner-plan.md](../test-script-runner-plan.md))。
  同書 §11 が「OS / winit / 配送・focus・wake は検証できない」と明記している。
  **用途は機能回帰**: メイングリッドの通常操作、静止画フルスクリーンの連続ページ送り、
  immediate / deferred viewport の生成・閉鎖、F12 切替後もフレーム進行と表示内容が一致、
  coalescing 後も入力数と表示決定が一致。既存 `page-turn-smoke` は高頻度 repaint 時の
  実用的な回帰になるが、§6.1 の代替にはしない。
- **idle health** (`scripts/check-idle-health.ps1`)。必須シナリオ: static foreground /
  static background / tray 常駐の完全 sleep / hidden main + 一時停止した native fullscreen /
  active detached・passive deferred の静止。active media は別閾値。
- **perf smoke**。通常の hitch 集計に加えて、入力 → outer paint latency、
  dirty → 提示完了 latency、**resize reason 以外で wndproc 内 paint が 0 件**、
  通常時の 1 イベントループ分の遅延増加を見る。

### 6.4 perf event

最低限これらを相関可能に出す: message id / wndproc depth / damage recorded /
outer paint begin・end / viewport・WindowId・surface generation / acquire begin・end /
present begin・end / readiness arm・signal・stale / dirty age。

## 7. 凍結ルール対応 (必須)

[detached-rework-plan.md](../detached-rework-plan.md) §2 (憲法) の対象。着手前に読むこと。

**ClaudeCode / Codex Sol の合意 (2026-08-16)**: §3 の不変条件で実装されるなら、
これは detached の症状パッチではなく**全 viewport 共通の event-loop 位相 ownership の
構造修正**である。§3 の不変条件を満たさない carve-out (症状に合わせた条件分岐) は
合意対象外なので、その形になりそうなら手を止めて報告すること。

完了時に §11 (リワーク外からの変更記録) へ追記する。

憲法から特に効くもの: **時間窓 (debounce / grace / settle ms) で競合を吸収しない** (5)。
readiness は事実 (OS 状態・世代・イベント) で判定する。hidden の 100ms throttle は
既存の busy-loop 防止であって新設の時間窓ではない — 流用して新しい競合を吸収しない。

## 8. やらないこと

- **§1.31-B に手を出さない**。acquire / Present の待ちに上限を入れる作業は別。
  本ブリーフは待ちを**どこで起こすか**を変えるだけで、待ちの**長さ**は変えない。
- winit / wgpu を vendor しない。厳密な `MARKER_IN_SIZE_MOVE` を取りに行かない (§3)。
- `src/` と `vendor/egui-wgpu/` に触れない。
- App-global な render state を作らない (§5 の 3)。
- 既存の detached テスト (src/app/tests.rs) と throttle unit test を弱体化しない。

## 9. 完了条件

1. `WindowEvent::RedrawRequested` が paint せず dirty 記録のみで戻る。
2. 外側 (`about_to_wait`) の paint drain 位相が存在し、そこが唯一の通常 paint 経路である。
3. `handle_event_result` が paint drain を再帰的に呼ばない。
4. `RepaintNow` が reason 付きで、bootstrap / AccessKit / resize が分離されている。
5. §3 の不変条件がコードのコメントに書かれ、inline paint が `InteractiveResize` に限られる。
6. dirty が WindowId / viewport / surface generation ごとに持たれている。
7. §6.2 の unit test が全部ある。
8. §6.1 の Windows process test があり、通る。
9. `scripts/test-full.ps1` から vendored eframe のテストが**無フィルタで**実行される。
10. `cargo fmt --check` が通り、`.\scripts\test-full.ps1` が exit 0。
11. §11 への記録と backlog の更新。
12. **完了報告に「§1.31 完了」と書かない** (§0)。「A = 同期メッセージが GPU 待ちを
    開始する経路の除去」と書くこと。
