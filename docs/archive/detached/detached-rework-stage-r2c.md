# Stage R2c 指示書: placement の single source of truth 化 + 入力経路表の文書化

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 位置付け: R2 の最終サブステージ (提案書 BA-6)。実機検証なしで進める
  (headless テストで固定し、実機は次回バッチの最終 matrix で確認)。
- 実装: Codex / 検収: Fable

## 1. 修正 1: placement を `DetachedWindowRuntime.placement` に一本化

### 現状の三重所有 (BA-6)

| 保有者 | 役割 |
| --- | --- |
| `settings.detached_viewer_window_placement` | 次に開く窓の seed + 永続化 |
| `active_detached_viewer_live_placement` | active 窓の runtime 実測 |
| `DetachedImageWindowSnapshot.placement` | passive 窓の保存値 |

三者のズレが既定サイズ混入・引き戻しの温床 (2026-06-28 調査の §3.2/BA-6)。

### あるべき形

- **窓ごとの唯一の真実 = `DetachedWindowRuntime.placement`**。
  - OS からの placement 更新 (描画クロージャの outer/inner rect) は runtime に書く。
  - builder の seed は runtime.placement から。**新規窓 (runtime に placement 未設定)
    のみ** settings から seed する。
  - passive snapshot / active live の placement フィールドは runtime 参照へ移行して
    削除 (完了報告に削除一覧)。
- settings への書き込みは「窓を閉じたとき最後の placement を永続化」のみ。
- **既定サイズ拒否ヒューリスティック
  (`passive_placement_update_rejected_default` / `active_placement_update_rejected_default`)
  は削除せず、書き込み先を runtime に付け替えて温存する** (BA-5 根治 = R2d まで防波堤
  として必要。プラン §4 R2c 参照)。
- 憲法 4 のとおり、新しい保存先・同期経路は作らない (移行のみ)。

### テスト

- park → resume → park のサイクルで placement が runtime に追従し続ける
- 新規窓は settings から seed、既存 runtime がある窓は runtime から seed
- 窓 close で settings に最終 placement が書かれる
- 既定サイズ拒否が runtime への書き込みでも機能する
- 削除フィールドの grep 0 件

## 2. 修正 2 (文書): detached 入力経路の分岐表

R2b の F5/F7 はいずれも「native presenter 経路と egui 経路のどちらが生きているか」
の暗黙知が原因で起きた。これを表として
[../../detached-viewer-implementation-plan.md](../../detached-viewer-implementation-plan.md) に
追記する (新規セクション「入力経路の分岐表」):

- 行 = 窓の状態 (Active 静止画 / Active 動画 / Active 音声モード / Parked /
  ParkedLive / Resuming / Closing / presentation switching)
- 列 = 入力の種類 (左クリック / 右クリック / ホイール / キーボード / HUD ヒット)
- セル = どの経路 (native presenter / egui viewport / 破棄) で処理され、何が起きるか
- 実装と一致していることをコードで確認しながら書く (願望ではなく現状を書く。
  現状が仕様と食い違う箇所を見つけたら表に ⚠ を付けて報告)

## 3. 完了条件

- [ ] placement 保有者が runtime 1 つ (+ settings seed) になり、旧フィールド grep 0 件
- [ ] §1 のテストが存在して緑、既存 detached / parked_live / still_window 全緑
- [ ] 入力経路表が implementation-plan に追記され、⚠ 箇所の報告がある
- [ ] `cargo fmt --check` / `cargo test --bin mimageviewer-core` / `cargo test` 緑
- [ ] コミットに `(detached-rework R2c)` を含める
