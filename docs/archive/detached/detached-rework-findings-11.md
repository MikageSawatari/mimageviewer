# 検収所見 #11: stale request 掃除が毎フレーム requeue ループ (回帰) + close 挙動の証拠再採取

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
前提: findings-10 (4901ac4f) + サムネ stale 掃除 (fa09cc5a) 適用済み。
実機 (2026-07-07 10:1x) の結果: 切替 OK / PDF サムネ OK / 別窓 PDF 閲覧 OK。
**残 = 窓を閉じるとき別窓が消える・表示される・再表示される**。
ログ = Fable が scratchpad に凍結済み (close_bak / close_cur.log)。

## C1: fa09cc5a の stale 掃除が「正常な in-flight」を stale と誤判定し毎フレーム requeue する

### 証拠 (実機ログ)

```
[queue] cleanup stale requested idx=52 state=Evicted   × 2842 回 (68 秒間 = 毎フレーム)
… 同様の idx が約 55 件、全て 2842 回前後
```

- `.bak` が **68 秒で 16MB** に到達しローテート → セッション前半 (0〜74.9s) が消失。
  **ユーザーが再現した「閉じるとおかしい」区間の証拠がこの spam で破壊された**。
- 意味: 該当 idx は毎フレーム「requested から remove → 再 enqueue (重複ジョブ) →
  requested に insert → 次フレーム再判定」を繰り返している。reload_queue /
  heavy_io_queue に**毎フレーム約 55 件の重複ジョブが流入**していた。

### 機構 (コードで確定)

- `requested.insert(idx, false)` は **enqueue 時点** ([app.rs:20661](../src/app.rs)) に立つ。
  サムネ state は worker が完了するまで `Evicted` のまま (state を Loading に変える機構は
  無い)。つまり **`requested ∋ idx かつ state==Evicted` は「再要求済み・ロード完了待ち」の
  正常な in-flight 状態**であり、PDF pool が混んでいれば数秒続くのが正常。
- fa09cc5a の判定 ([app.rs:20964 付近](../src/app.rs)) はこれを stale とみなして毎フレーム
  remove→再 enqueue する。**ループの終了条件が「worker が先に完了すること」しかない**。
- 既存コードの掃除経路は健全であることを確認済み:
  - worker の canceled 応答 (STALE 等) → `requested.remove` + Evicted 戻し
    ([app.rs:20303-20313](../src/app.rs))
  - prune (keep 外 / prefetch 抑制) → `q.retain` 内で `requested.remove`
    ([app.rs:21114-21128](../src/app.rs)、heavy 側も同様)
  - フォルダロード → `requested.clear()`

### 元の「stale で止まる」観測の再評価

fa09cc5a の動機だった「requested に残ったまま Evicted で再要求されない」ログは、
上記のとおり**正常な in-flight 状態の誤読だった可能性が高い**。当時サムネが実際に
止まっていた原因は findings-9 B2 (defer_texture_uploads の恒久 defer) と findings-10
(churn の UI 飽和) で説明でき、両方とも既に修正済み。

ただし今回の実機で「サムネが進む」のはループが強制 requeue している状態での観測
なので、「掃除なしでも進む」ことの証明にはなっていない。よって:

### 修正要件

1. **fa09cc5a の掃除ロジックを revert する** (テストも含めて)。毎フレームの hot loop で
   requested+Evicted を stale 扱いする判定は原理的に in-flight と区別できない。
2. revert 後の実機で PDF サムネ停止が**再発した場合のみ**、真の leak point を特定して
   そこで直す (= 「queue から要求が消えるのに requested が残る」経路を探す。上記の
   既知経路は全て健全なので、残るとすれば worker が pop 後に応答を送らない経路や、
   応答の gen 不一致 drop 側)。頻度依存の scavenger を hot loop に置く方式は採らない。
3. ログ規律: 毎フレーム無条件で出るログ行を追加しない (F10 の間引き方針と同じ。
   今回の spam が close 問題の証拠を破壊した実害)。

## C3: デバッグフラグ有効時はログ保持を自動で拡大する (手動退避の廃止)

### 背景

バグ再現時の「即ログ退避 (Copy-Item)」運用は手間でミスも起きやすく、実際に本セッション
だけで 2 回、ローテートによって証拠を失っている (findings-10 解析時 / 今回の close 問題)。
現行の 16MB / .bak 1 世代は**運用中の利用者環境**に合わせたサイズであり、デバッグ中の
要件とは別物。デバッグフラグを使っている時点で開発環境なので、ディスクを気にせず
保持を拡大してよい (ユーザー承認済み 2026-07-07)。

### 修正要件

[src/logger.rs](../src/logger.rs) に「デバッグ保持モード」を追加する:

1. **判定は init 時に 1 回だけ** (決定性重視、実行時の動的変更なし)。以下のいずれかの
   環境変数が `1` なら有効:
   - `MIV_DETACHED_WINDOW_DEBUG` (今回の運用フラグ)
   - `MIV_LOG_RETENTION_DEBUG` (汎用 override。他サブシステムのデバッグでも使えるように)
   - 将来デバッグフラグが増えたらこのリストに足す (logger 側の配列 1 箇所)
2. デバッグ保持モード時:
   - `MAX_LOG_BYTES` を **256MB** に拡大 (16 倍。今回の spam ペース 16MB/68s でも
     約 18 分/世代)
   - ローテート世代を **`.bak1`〜`.bak4` の 4 世代**に拡大 (rotate 時に bak3→bak4 …と
     シフト)。通常モードは現行どおり 16MB / `.bak` 1 世代で変更なし
3. 起動時に 1 行、選択されたモードをログに書く
   (例: `logger: debug retention mode (256MB x4 generations) via MIV_DETACHED_WINDOW_DEBUG`)。
   解析側がどのモードのログか判別できるように。
4. `.prev` (起動時コピー) の挙動は変更しない。
5. テスト: ローテートのシフト順 (bak1 が最新、bak4 が最古、5 回目で最古が消える) を
   純関数化またはtempdir で固定。

これにより C2 以降の再現採取は「フラグを付けて起動 → 再現 → そのまま Codex/Fable に
解析依頼」で完結し、手動 Copy-Item は不要になる。

## C2: 窓 close 時に別窓が消える / 突然表示される / 再表示される — 証拠再採取

### 現状わかっていること

- 凍結ログに残っていた close は **75.15s の 1 件のみ** (最後の 1 枚 = passive_windows=0 の
  active_close_finalize で、これは正常動作)。問題の多窓 close 区間は C1 の spam による
  ローテートで消失。
- findings-10 前の旧セッションでは「④閉じると別窓が突然表示」は churn (clear-on-park)
  で説明したが、churn 解消後も close 経路の症状だけ残った = **close には churn とは別の
  独立した機構がある**。
- 観察メモ (未確定、調査の手がかり): close 時の遷移が `from=Active to=Removed`
  (`state_transition_unexpected`) で **Closing を経由していない**。reducer の想定外遷移が
  close 経路にあること自体が、close 時の 1 フレーム挙動 (登録・描画の隙間) を疑わせる。
  ※ 憲法 6 のとおり、証拠が揃うまで修正は入れないこと。

### 手順 (ユーザー + Codex)

1. C1 revert + C3 (デバッグ保持モード) を先に適用してビルド。
2. ユーザー実機: `MIV_DETACHED_WINDOW_DEBUG=1` で ON モード窓 3〜5 枚 → 1 枚ずつ閉じて
   症状再現。C3 により手動退避は不要 (256MB × 4 世代に自動保持)。可能なら時計入り録画。
3. Codex は close 前後 (pending_close / active_close_finalize / passive_close /
   state_transition / deferred_registration / visibility 系) を解析して機構を確定し、
   修正案を Fable 承認に回す。

## 完了条件

- [ ] C1: fa09cc5a revert。コミット `(detached-rework findings-11 C1)`
- [ ] C1 revert 後 full test 緑 (fa09cc5a が足したテストは revert で削除してよい)
- [ ] C3: logger デバッグ保持モード + 世代シフトテスト。コミット
      `(detached-rework findings-11 C3)` (C1 と別コミット。detached コードではないので
      凍結ルール対象外だが、紐付けのためタグは付ける)
- [ ] `.\scripts\build-release.ps1` で検証バイナリ用意
- [ ] C2: 再現ログ採取 → 機構確定の報告 (修正はその後、Fable 承認制)

## 実機確認 (C1/C3 適用後)

1. 起動ログ先頭に debug retention mode の 1 行が出る
2. PDF フォルダで `[queue] cleanup stale requested` がログに出ない
3. PDF サムネ一覧が従来どおり進む (止まらない)
4. C2 の再現手順を実施 (退避不要)
