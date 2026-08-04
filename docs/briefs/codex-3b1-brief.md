# 段 3b-1 — AI job の手続きと end-to-end backend

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `b626f6c9`

## 0. 立場

**本体が正本。web 側で独自の規則を発明しない。**

以下の「正本」節は私が読んで確認した内容だが、**実際の規則と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで過去に私の誤りが 5 回訂正されている
(製本ページの既定、desktop のカラー化 UI、`remote_session_active()` の呼び出し側分岐、
`.viewer-seek`、PC/remote の競合前提)。

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 目的と範囲

段 3b-0 (`b626f6c9`) で排他 lifecycle と singleton `AiRuntime` は入った。
**3b-1 は job の手続きと、実際の推論を bridge へ接続するところまで。**

Web UI は **3b-2**。今回は UI を作らない。手続きが正しいことを test で固定するのが成果物。

正本計画: [../web-remote-ai-plan.md](../web-remote-ai-plan.md) §5・§6・§7・§8・§13。
本 brief と食い違う場合は計画側を優先し、食い違い自体を報告してほしい。

## 2. 正本 (確認済み — 違えば報告)

### 2.1 AI は「後から差し替わる」。待たせない

[docs/display-pipeline.md](../display-pipeline.md) §2.3:

> AI 待ちの `complete=false` final composite も表示専用の候補である。final AI が到着しても
> 同一 `FinalCompositeKey` の entry は消さず、AI 後の final-effect worker が完成するまで
> カラー化済みの暫定 texture を表示し続ける。完成結果の `insert` が同じ key を上書きするため、
> 表示はキャッシュ欠落を挟まず原子的に差し替わる。

**本体は AI を待たずに非 AI の絵を出し、出来たら原子的に差し替える。**
スマホも同じ意味にする。これが §3.1 の根拠。

### 2.2 AI が要るなら入力は native へ収束する ⚠ 今回の核心

[docs/display-pipeline.md](../display-pipeline.md):

> AI が必要な raster は display-fit が native より小さい場合も大きい場合も native へ収束させ、
> その間の final AI / AI 先読みを保留する。

適用可否の判定も入力寸法で決まる
([src/app.rs](../../src/app.rs) `final_ai_key_for_pixels` → `should_process_rect(w, h, limit)`)。

**したがって remote が表示寸法の画素に推論すると、絵が違うだけでなく判定が食い違う。**
既定 2048x2048 上限で、長辺 6000px の原稿は本体では AI を**適用しない**。
remote が 2048px の decode に推論すると、**本体が掛けない画像にスマホだけ AI が掛かる。**
利用者が繰り返し警戒しているのはこれ:

> 2 箇所にわかれていると他にも設定の動作の違いなどがおこることを懸念しています

### 2.3 現在の remote page 経路は表示寸法で decode している

[src/remote_ipc/container.rs:1913](../../src/remote_ipc/container.rs) は
`thumb_loader::process_load_request(..., target_px, ...)` を通る。`target_px` は web が
query で指定する値 (実測 2048)。上限は `MAX_PAGE_RENDER_PX = PDF_RENDER_MAX_LONG_PX`。

つまり **AI 用の入力は現在の page 経路からは取れない。**
§2.2 の収束規則に合う入力を別に用意するのが 3b-1 の主要作業。

PDF は本体側も display target を持つ (raster は native 長辺が絶対上限、vector は最低 4096、
全経路 8192 上限)。**PDF だけ規則が違うので、本体の実際の式に合わせてほしい。**

### 2.4 3b-0 で置いた bridge

[src/remote_ipc/session.rs:833](../../src/remote_ipc/session.rs) `RemoteAiExecutionBridge`。
`resources_for_remote()` が `AiRuntime` + `ModelManager` を返す (未生成なら呼び出し元 worker 上で
一度だけ生成、Condvar で二重生成を防ぐ)。remote MI-GAN は既にここを通っている。

**final AI も同じ bridge を通す。別 runtime を作らない。**

## 3. 設計判断 (決定済み)

### 3.1 既存 `/api/page` は変えない

AI 有無にかかわらず `/api/page` は**今までどおり即座に非 AI の合成を返す**。
§2.1 の本体規則と同じ意味になり、既存経路への回帰risk も無い。

AI は**別の job** が同じページのより良い版を作る。スマホは job が `Ready` になったら
取りに行って差し替える。3b-2 の UI はこの差し替えを描くだけになる。

### 3.2 結果の取り出し口

計画 §6 の 4 つに加えて `GET /api/ai/jobs/{job_id}/result` を置き、WebP を返す。

`/api/page` に AI 用の query を足さない。足すと 10 分保持・stale 拒否・terminal reason が
page 経路の関心事になり、既存の cache key と混ざる。**job の成果物は job が持つ。**

### 3.3 executor は trait、fake は test 側に置く

計画 §13 の「fake executor で start / state / cancel / disconnect / background recovery を
固定する」は、**出荷経路に fake を置くという意味ではない。**
executor を trait にして、test が決定的な double を挿せる形にしてほしい。

理由: disconnect drain・background 復帰・10 分保持・supersede は GPU 無しで固定できるべきで、
実機でしか試せない状態にすると回帰が入っても気付けない。

### 3.4 cache

AI 結果は `RemoteFinalAiKey` (計画 §5) で別 cache に持つ。
既存 `page_composite_cache` (`RemoteCompositeCacheKey`) には `target_px` が入っているので、
**AI 結果をそこに入れない** — 表示寸法ごとに推論し直すことになる。

推論は native、表示寸法への縮小は AI の**後**。縮小結果を job の result として持つか、
native 結果を持って要求ごとに縮小するかは実装判断でよい。判断と理由を報告してほしい。

## 4. 今回やること

- job registry と 5 本の HTTP (計画 §6 + §3.2 の result)
- 状態遷移 (計画 §6 の 13 状態) と terminal reason (計画 §7)
- disconnect drain への参加 (計画 §4.3 の 1〜7 を job まで通す)
- background 復帰 (計画 §8 の 1〜6)、terminal 10 分保持
- §2.2 の入力収束: 通常画像 / ZIP / nested archive / raster PDF / 製本ページ
- vector PDF は AI を起動しない、size gate、stale result 拒否
- source → (MI-GAN) → final AI → final composite → WebP を bridge へ接続

やらないこと: Web UI、model 選択の `SetAdjustment` 接続 (どちらも 3b-2)。

## 5. 調べて報告してほしいこと

1. §2.2 の native 収束を remote で満たす最小の形。既存 `process_load_request` に
   native を要求する口があるか、無いなら何を足すのが最小か
2. 製本ページの canonical input。本体は `book_page_default_adjust_params()` の例外を持つが、
   **AI 入力側にも同種の例外があるか**
3. nested archive の入力が、本体の final AI 入力と本当に同じ画素に収束するか
4. `RemoteFinalAiKey` に何を入れれば stale result を確実に落とせるか。
   計画 §5 の列挙で足りるか、足りないなら何が要るか
5. §3.4 の縮小タイミング。native 結果保持と都度縮小のどちらが素直か

## 6. 受け入れ条件

- AI 設定が有効なページで、スマホが**本体と同じ判定・同じ model・同じ順序**の結果を得る
- 本体が AI を掛けないサイズの画像に、remote だけ AI が掛からない (§2.2)
- `/api/page` の応答時間が AI 有効時も変わらない (§3.1)
- 切断すると job が `DiscardedByHost` になり、drain 完了まで modal が閉じない
- 切断・background・supersede・取消が **GPU 無しの test で固定されている** (§3.3)
- vector PDF で AI job が起動しない
- UI thread が model load / 推論 / decode / encode / worker join をしない
- `cargo test -p mimageviewer --lib` / `-p mimageviewer-remote` / `-p mimageviewer-ipc` / web が緑
