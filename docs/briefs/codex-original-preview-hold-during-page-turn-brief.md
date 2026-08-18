# 元画像ホールドは、ページ送りの入力が押されている間ずっと無効にする

対象: backlog §1.91 の続き (利用者報告 2026-08-18)。「右 Ctrl では戻りだけ遅い。押しっぱなし
なのに一瞬だけ右 Ctrl の効果が出る」。**この 2 つは同じ現象の表と裏**であることが実測で確定した。

## 0. 実測 (2026-08-18 の perf log、新しい 4 属性つき)

`fs/page_turn_decision` に足した `right_ctrl_held` / `original_preview_active` /
`context_blocker` で切り分けた結果:

| 右 Ctrl | chord | ページが進んだ割合 |
| --- | --- | --- |
| なし | Ctrl+Right (戻り) | 58% |
| **あり** | **Ctrl+Right (戻り)** | **17%** (302 回中 252 回が空振り) |
| なし | Ctrl+Left (進み) | 71% |
| あり | Ctrl+Left (進み) | 59% |

| 右 Ctrl | pass_through | materialized |
| --- | --- | --- |
| なし | **79** | 11 |
| あり | **0** | 114 |

- `right_ctrl_held=true` かつ `original_preview_active=true` かつ
  `context_blocker="original_preview"` のフレームが **1,888** 件。
- 右 Ctrl を押している間、ページ送りの pass-through が**一度も成立していない**。

### 0.1 連鎖

1. 押しっぱなしの最中、**シーケンスが切れている合間**に `original_preview_active` が true に戻る
   (§1.91 の除外は `fs_navigation_sequence_blocks_new_target()` だけを見ているので、
   シーケンスとシーケンスの隙間が抜けている)。
2. それが `fs_page_turn_ordinary_context_blocker` の `original_preview` として
   `page_turn_input_held=false` を作り、burst を解除する。
3. pass-through が組めず毎ページ完全生成になる。戻り方向は先読みが 4 ページ分しかない
   (進みは 12) ので、大半の auto-repeat が「まだ用意できていない」に当たって空振りする。

利用者が見ている「一瞬の元画像」は 1 の可視化であり、遅さは 2→3 の結果。**同じ原因**。

## 1. 利用者判断 (2026-08-18、確定)

> ページ送り中は右 Ctrl 無効なので、押しっぱなしの間は無効を維持してもいい。

§1.91 の「判定は typed navigation sequence だけを使う」は、隙間が問題だと分かる前の判断。
今回はそれを**押下レベルという事実**へ置き換える (時間窓ではない)。

## 2. やること

**ページ送りの入力が物理的に押されている間、`original_preview_active` は false を返す。**
既存のシーケンス除外は**残す** (単発タップでキーを離した後もシーケンス完了まで抑止が要る)。

### 2.1 物理レベルの読み取りを blocker から切り離す

今は `fs_page_turn_input_held` が

```
blocker = fs_page_turn_ordinary_context_blocker(...)   // ← original_preview を見る
if blocker.is_some() { return (false, blocker, viewport) }
page_turn_edge_hold_this_frame(...) / permit + chord の OS 直読み
```

の順なので、`original_preview_active` から押下レベルを見ると**循環する**。

押下レベルだけを返す helper を切り出す (blocker には一切触らない):

- `page_turn_edge_hold_this_frame(ctx, viewport)` が答えを持っていればそれ
- 無ければ permit + `FS_PAGE_TURN_COALESCE_ACTIONS` / `FS_FIXED_PAGE_TURN_CHORDS` の
  `key_held_chord_via_os` + `fs_page_turn_chord_is_unambiguous`
- permit が取れなければ false

**フレームごとに memo する** (`(frame_nr, viewport)` の ctx temp data。既存の
`fs_page_turn_decision` / `fs_original_preview_active` と同じパターン)。
`original_preview_active` からも `fs_page_turn_input_held` からも呼ばれるので、
OS 直読みと `KeyAction::all()` の走査を 1 フレームに 2 回やらない。

### 2.2 2 つの呼び出し側を組み替える

- `original_preview_active`: 既存の `fs_navigation_sequence_blocks_new_target()` 早期 return の
  隣に、helper が true なら false を返す早期 return を足す。理由をコメントに書く
  (連続ホールド中はシーケンスの隙間でも「移動中」であり、元画像を出すと表示が明滅し、
  同時にページ送りの burst を解除して戻り方向を止める)。
- `fs_page_turn_input_held`: **helper を先に呼び**、そのあと blocker を見る。blocker があれば
  従来どおり `(false, blocker, viewport)`。無ければ helper の値を返す。
  これで `original_preview` blocker はホールド中に成立しなくなり、burst が組める。

### 2.3 blocker の `original_preview` 項目は残す

ホールド中は上の除外で成立しなくなるが、**マウス / リングからのページ送り**を
右 Ctrl で元画像を見ている最中に行う経路はまだ通る。加工済み pass-through と元画像要求の
矛盾を防ぐ役目は残っている。

## 3. やらないこと

- 時間窓 (debounce / grace / settle ms) を入れない (憲法 §2 規則 5)。
- 先読み窓 (12:4) を変えない。
- `original_preview` を blocker 一覧から外して pass-through を復活させる、はしない
  (元画像を出したまま加工済みピクセルを描くことになり、表示が矛盾する)。
- `FsOriginalPreviewHold` の chord と ページ送り chord を突き合わせるような判定は書かない。
  見るのは「ページ送り入力が押されているか」という既存の typed な事実だけ。

## 4. テスト (mutation を通すこと)

- **T1**: ページ送りキーが押されている + 元画像ホールドの修飾も押されている →
  `original_preview_active` が false。
- **T2**: ページ送りキーが押されていない + シーケンス無し + 修飾は押されている →
  従来どおり true (抑止を広げすぎていない)。
- **T3**: ページ送りキーが押されている間、`fs_page_turn_ordinary_context_blocker` が
  `original_preview` を返さない (= burst が組める)。循環していないことも同時に固定する。
- **T4**: §1.91 の既存テスト (シーケンス進行中は false) が通ったままであること。
- 各テストについて、対応するガードを削除 / 反転して**実際に落ちることを確認**し、
  結果を報告に含める。

## 5. 計装は残す

`fs/page_turn_decision` の 4 属性 (`original_preview_active` / `context_blocker` /
`right_ctrl_held` / `left_ctrl_held`) は**そのまま残す**。修正の検証に使う
(期待: `right_ctrl_held=true` でも `pass_through` が出る、`context_blocker=original_preview` が
ホールド中に出なくなる)。

## 6. ドキュメント

- [docs/next-release-backlog.md](../next-release-backlog.md) §1.91: **エントリ末尾に追記**。
  冒頭の「完了 (2026-08-17)」を書き換えず、続きとして今回の実測・判断・修正を記録する。
- [docs/display-pipeline.md](../display-pipeline.md) §§2.3, 2.5.4: 元画像ホールドの成立条件が
  「シーケンス進行中」から「シーケンス進行中 **または** ページ送り入力ホールド中」へ
  広がったことを反映する。

## 7. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
