# 段 3b-2 — AI の Web UI

worktree: `C:\home\mimageviewer-web` / branch `web-remote` / 起点 `1b77f750` (v2.11.0 merge 済み)

## 0. 立場

**本体が正本。web 側で独自の規則を発明しない。**

以下の「正本」節は私が読んで確認した内容だが、**実際の規則と違っていたら実際の方を報告してほしい。**
私の要約に合わせて実装しないこと。この段取りで既に私の誤りが 7 回訂正されている
(製本ページの既定 / desktop のカラー化 UI / `remote_session_active()` の分岐 /
`.viewer-seek` / PC-remote 競合前提 / animated 一律拒否 / UI thread での params 検証)。

稼働中の本体 / remote-web は操作しない。`build-dev.ps1`・コミットも実行しない。

## 1. 目的

3b-1b で job の口と中身は入った。**スマホの画面から使えるようにする**のが 3b-2。
これで「利用者が最重要と挙げた AI」が remote で完結する。

## 2. 正本 (確認済み — 違えば報告)

### 2.1 本体は AI を待たずに出し、出来たら原子的に差し替える

[docs/display-pipeline.md](../display-pipeline.md) §2.3:

> AI 待ちの `complete=false` final composite も表示専用の候補である。(中略)
> 完成結果の `insert` が同じ key を上書きするため、表示はキャッシュ欠落を挟まず原子的に差し替わる。

**スマホも同じ意味にする。** `/api/page` の絵を先に出し、job が `Ready` になったら
`/api/ai/jobs/{id}/result?page=N` を**先に decode してから**差し替える。
読み込み中の空白や、AI 前後で 1 フレーム欠ける形にしない。

### 2.2 本体は「AI が有効なら勝手に掛かる」

本体に「AI を実行」ボタンは無い。effective params が AI を要求していれば表示時に走る。
**スマホにも実行ボタンを置かない。** ページを開いた / AI 設定を変えたときに自動で始める。

3b-1b の registry は「現在の page group 一つだけ」を持ち、新しい group が来たら旧 job を
`Superseded` にする。ページ送り連打はこれで吸収される。**新しい抑制規則を足さない。**

### 2.3 job の API (3b-1b、`crates/remote-ipc/src/lib.rs:1017`)

```
POST   /api/ai/jobs                          {request_id, pages:[{address,target_px}]}
GET    /api/ai/jobs/{job_id}
GET    /api/ai/jobs?recoverable=1
GET    /api/ai/jobs/{job_id}/result?page=N
DELETE /api/ai/jobs/{job_id}
```

- `request_id` は client 生成の冪等キー。同じ値の再送は元の job を返す
- `pages` は**画面の左→右順**で 1〜2 件 (見開き)
- 13 状態 / 7 phase / 11 terminal code は `RemoteAiJobState` / `RemoteAiProgressPhase` /
  `RemoteAiTerminalCode` を参照

### 2.4 進捗は推測しない

計画 §6:

> 総合 percent は推測せず、model load / finalize は indeterminate、denoise / upscale は
> `completed_tiles / total_tiles` を返す。

`RemoteAiProgress` は `completed_tiles` / `total_tiles` を `Option` で持つ。
**None のときに percent を捏造しない。** 見開きは `page_index` / `page_count`、
両 stage 使用時は `stage_index` / `stage_count` も出す。

### 2.5 文言は server が持つ

`RemoteAiTerminalDetail.message` は server が作った日本語を持つ。
**同じ文言を JS 側にもう一組作らない。** 標準スコープのラベルを IPC で受けている
のと同じ理由 (段 2a §3.2)。

phase のラベルだけは JS に必要かもしれない。**その場合も、server が持つべきか
JS が持つべきかを判断して報告してほしい。** 私は server 側に寄せる方が
一貫すると思うが、既存の実装がどちらに寄っているかを優先する。

## 3. 設計判断 (決定済み)

### 3.1 model 選択は `SetAdjustment` へ

現在 AI は読み取り専用表示 (`app.js:5110` の「現在の AI 処理」)。
これを**操作可能**にする。書き込みは段 2a / 3a と同じ `RemoteWriteRequest::SetAdjustment`。
専用経路を作らない。

**選択肢を JS にハードコードしない。** 本体が持つモデル一覧・既定・「なし」の扱いを
server から受けて出す。標準スコープのラベルを IPC で受けているのと同じ。
どの型に載せるのが素直かは既存の `adjustment_state` の形に合わせて判断してほしい。

スコープ (標準 / このページ) は段 2a と同じ 2 択。AI も同じスコープに従う。

### 3.2 polling

計画 §6:

- foreground の nonterminal は 500 ms
- foreground 復帰時は即時
- 一時通信失敗は 1 s → 2 s → 5 s 上限の backoff
- **background では polling を止める**。browser timer を keepalive にしない

3b-1b の background lifecycle (`Detached { since }` / 10 分) がこれを前提にしている。
ここを守らないと PC 側の modal が閉じない、または早く閉じすぎる。

### 3.3 終わり方をスマホに見せる

計画 §7。**黙って spinner を回し続ける形にしない。** 特に:

| 起きたこと | terminal code | 見せ方 |
| --- | --- | --- |
| PC 側で切断された | `DiscardedByHost` | 中止された旨。自動再開しない |
| 自分で取り消した | `CancelledByUser` | 通常終了。session は続く |
| 別 client に取られた | `Superseded` | 自動再開しない |
| 画面消灯で期限切れ | `BackgroundExpired` | 期限切れの旨 |
| vector PDF / サイズ上限 / アニメ | 各 code | **エラーに見せない**。AI の対象外である旨 |

最後の行が重要。対象外は失敗ではないので、赤いエラー表示にしない。
本体でも同じページは AI が掛からず普通に表示される。

### 3.4 画面消灯からの復帰

`GET /api/ai/jobs?recoverable=1` で同じ job を復元する。
foreground 復帰時に **まず recoverable を引き、既存 job があれば purge せず引き継ぐ**。
新しい `request_id` で二重に始めない。

## 4. やること

- AI モデル選択 (アップスケール / デノイズ) を `SetAdjustment` へ接続
- ページ表示 / AI 設定変更で job を自動開始 (§2.2)
- phase / タイル進捗 / 見開きページ数の表示 (§2.4)
- 取消ボタン
- `Ready` で decode 済み画像へ原子的に差し替え (§2.1)
- terminal の見せ分け (§3.3)、画面消灯復帰 (§3.4)
- polling の規律 (§3.2)

## 5. 調べて報告してほしいこと

1. §2.5 の phase ラベルを server / JS どちらに置くか。既存の寄せ方を優先
2. §3.1 のモデル一覧をどの型に載せるか。`adjustment_state` の既存の形に合うか
3. 自動開始の trigger を、本体のどの条件と対応させるか。
   本体が prefetch でも AI を走らせるなら、remote も隣接ページを先読みすべきか、
   現在ページだけにすべきか。**本体の実際の規則を見て判断し、根拠と一緒に報告**
4. 見開きで片方だけ terminal (例: 左は Ready、右は vector PDF) になり得るか。
   なるなら、その表示をどうするか

## 6. 受け入れ条件

- スマホで AI モデルを選ぶと保存され、**PC 側にも同じ設定が反映される**
- AI 有効なページを開くと自動で走り、出来たら**ちらつかず**差し替わる
- 進捗が phase とタイル数で出る。**percent を捏造していない**
- 取消が効く
- PC 側で切断すると、スマホが待ち続けず `DiscardedByHost` を表示する
- vector PDF / サイズ上限超えが**エラーに見えない**
- 画面消灯 → 復帰で同じ job に戻り、二重に始まらない
- background で polling が止まっている
- `cargo test -p mimageviewer --lib` / `-p mimageviewer-remote` / `-p mimageviewer-ipc` /
  web テストが緑

## 7. 注意

- `crates/remote-web/web/` の JS/CSS はディスクから毎回読まれるので**再ビルド不要**。
  ただし SPA は自分の script を再取得しないので、**スマホ側の再読み込みが要る**
- `[hidden]` は `styles.css` の global rule で効く。component 側で `display` を
  上書きしない (2026-08-04 の修正)
