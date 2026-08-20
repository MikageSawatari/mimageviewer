# フルスクリーンの現在ページが Normal レーンに居座る

着手前に [CLAUDE.md](../../CLAUDE.md) の「バグ修正の一般原則」と
[docs/async-architecture.md](../async-architecture.md)、
[docs/scroll-visibility-priority-plan.md](../scroll-visibility-priority-plan.md) を読むこと。

## 1. 症状と実測

利用者報告: **サムネイル一覧が先読みしている最中に新しい PDF を開くと、ホイールでページを
送っても表示が変わらない。**左下に「再レンダリング中」が出ている。単ページモード (連結ではない)。

2026-08-20 の実ログ (`perf_events.jsonl`、再現時):

| 記録 | 値 |
| --- | --- |
| ホイール入力 | フルスクリーンへ届いており、**ページ送りも 0.6 秒以内に成立**していた |
| `fs/load_begin` の優先度 | **Normal 48 件 / Critical 1 件** |
| `pdf/pool_queue_snapshot` | `normal=18`, `in_flight=4`, **in_flight_age_max = 6,441ms** |
| `pdf/pool_recv` の所要 | **p50 907ms / p95 5,273ms / 最大 7,101ms** |

**入力は正常。ページ送りも成立している。描画だけが最大 7 秒待たされている。**

## 2. 原因 (source inspection で確認済み)

優先度の規則自体は正しい ([app.rs:50044](../../src/app.rs:50044)):

```rust
// フルスクリーン現在ページ (ユーザーが待っているもの) は Critical、
// それ以外 (先読み) は Normal。
let pdf_priority = if self.fullscreen_idx == Some(idx) { Critical } else { Normal };
```

**優先度はキューへ入れる瞬間に焼き付く。**先読みで Normal として積まれたページが、その後
「現在ページ」になっても**昇格されない**。`fullscreen_idx` が変わるだけでは pool 内の Job は動かない。

一覧側には**まさにこの問題への対策が既にある** — `promote_to_high_normal`
([pdf_loader.rs:130](../../src/pdf_loader.rs:130)) が Normal レーンの Job を HighNormal へ移す。
しかし**呼び出し箇所は 2 つとも一覧用**で、可視グリッド範囲の `GridItem` を走査している
([app.rs:32221](../../src/app.rs:32221)、[app.rs:33159](../../src/app.rs:33159))。
**フルスクリーンの現在ページには昇格経路が無い。**

一覧側が先読みで Normal レーンを埋めるため、フルスクリーンの現在ページがその後ろに並ぶ。

なお `fs_pdf_render_context_epoch` はフルスクリーンの load に対して**常に 0 を返す**
([app.rs:49726](../../src/app.rs:49726)) ので、これらの Job は epoch prune の対象外。
**epoch まわりの複雑さは無い。**

## 3. やること

**現在ページが変わったフレームで、その perf key が Normal レーンに居れば昇格する。**
一覧側と同じ機構をフルスクリーンにも通す。

- 見開き表示のときは**両ページ**が対象。
- 既に in-flight の Job は昇格できない (PDFium は cancel 不可)。**これは直せないので、
  直せるふりをするガードを足さない。**待ちが残ることは受け入れて記録する。
- **二重 enqueue はしない。**新しく Critical で積み直すと同じページを 2 回描くことになる。

### 決めてほしいこと: 昇格先は HighNormal か Critical か

- **HighNormal**: 既存 `promote_to_high_normal` をそのまま使える。一覧の可視サムネと同じレーン。
- **Critical**: 現在ページは「利用者が待っているもの」で、初回 open は Critical になる。
  意味論としてはこちらが素直。ただし **Critical は予約ワーカー 1 つで処理される**設計なので、
  予約ワーカーの取り合いが変わる。

**理由付きで判断し、報告に明記すること。** Critical にするために予約ワーカーの意味論
(`CRITICAL_RESERVATION_ACTIVE` 周り) を変える必要があると判断したら、**変更せず止めて報告する。**

## 4. 制約

- **時間窓・delay・retry・一括 reset を使わない。**「現在ページが変わった」という事実で判定する。
- 一覧側の昇格の意味論 (`last_promoted_visible_keys` / `promote_retry_pending` の dedup と
  retry latch) を変えない。フルスクリーン側は自分の state を持つ。
- 既存の `pdf/pool_promote_visible` と**区別できる perf event** を出す
  (例: `pdf/pool_promote_fullscreen`)。promoted / already_high / not_found を含める。
  **not_found は「まだ pool に入っていない」を意味するので、一覧側と同じ retry latch が要るか
  判断すること。**

## 5. テスト

- 先読みで Normal に積まれた key が、現在ページになった時点で昇格されること。
- 見開きで両ページが昇格されること。
- 既に HighNormal / Critical の Job を降格させないこと (一覧側テストと同じ性質)。
- 現在ページが変わらないフレームで昇格を走らせないこと (毎フレーム lock を取らない)。
- 一覧側の昇格の挙動が変わっていないこと (既存テストを通す)。
