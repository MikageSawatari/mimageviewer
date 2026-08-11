# リモート閲覧: home 画面の再取得をセッション側へ戻す / 効かない音量スライダー

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. home 画面が再取得されない — 発火の入口が間違っている

### 1.1 観測

直前の増分で「世代が変わったら `/api/favorites` と `/api/home` を取り直す」構造を入れたが、
実機では本体の設定変更が反映されない。ログ (`remote-web-log.jsonl`) の 12:48〜12:49:

    12:48:39  /api/session/ping      409   本体で切断
    12:48:41  /api/session/acquire   200   再接続
    12:49:09  /api/session/ping      409
    12:49:11  /api/session/acquire   200   再接続
    12:49:39  /api/session/ping      200

**この間 `/api/home` も `/api/favorites` も 1 度も呼ばれていない。** 再取得が発火していない。

### 1.2 前提 — 方針は既に決まっている。作り直さないこと

**セッションを版の正本にし、再接続で cache を破棄する**、が確定済みの方針である
([briefs/codex-remote-session-epoch-addendum.md](codex-remote-session-epoch-addendum.md)、
計画書 §2.2 と §12.16 冒頭に書き戻し済み)。

直前の増分でこの方針を取り違え、home 画面のデータを世代側へ載せてしまった。本増分はその
差し戻しである。**世代の計算に手を入れる方向へ進まないこと。**

### 1.3 なぜ世代では拾えないか (特定済み。再調査不要)

[crates/remote-web/src/store.rs](../crates/remote-web/src/store.rs) の `refresh_settings_snapshot`:

    let (data_version, favorites, sort_order) = read_stable_settings_snapshot(...)?;
    settings.data_version = data_version;
    if favorites == state.snapshot.favorites && sort_order == state.snapshot.sort_order {
        return Ok(false);
    }

世代が上がるのは **favorites か sort_order が変わったときだけ**。「場所▼に出す項目」や
スマートフォルダの定義を変えても、`data_version` は動くのに favorites / sort_order は
同じなので変更なしとして返る。

`remote_state_generation` は元々「お気に入り / 並び順が変わった」信号だった。そこへ
`/api/home` を載せたので、信号が担っていない範囲まで期待する形になっていた。

### 1.4 直し方 — セッション取得で読み直す

**世代の計算は触らない。home 画面のデータを世代に紐づけるのをやめ、
`/api/session/acquire` が成功したときに読み直す。**

判断の根拠は排他そのものである。**リモートが操作権を持っている間、本体は設定を変更できない**
(計画書 §2.2 の単一 owner + 入力ロック)。したがって本体側の変更は必ず
「切断 → 変更 → 再取得」を通る。**セッション取得は「本体で何か変わったかもしれない」の
完全な信号**であり、近似ではない。

計画書 §2.2 は「操作権がローカルへ戻った瞬間、本体は既存の再読み込み入口を 1 回呼ぶ」と
定めている。その鏡像がこれにあたる。

- **この方針と受け皿は既に存在する。新設しないこと。**
  [app.js](../crates/remote-web/web/app.js) の `applyRemoteSessionId` が、セッション ID が
  変わったときに cache epoch を更新し `pageResourceCache` / `imageInfoCache` /
  `containerImageInfoHints` を破棄している。これが
  [briefs/codex-remote-session-epoch-addendum.md](codex-remote-session-epoch-addendum.md)
  「再接続ボタンを押したときだけ再開し、そのときキャッシュを破棄する」の実装
- 直前の増分で入れた coordinator (`targets` の表) はそのまま使う。**発火の入口を
  `applyRemoteStateGeneration` から `applyRemoteSessionId` へ移す**。
  セッション ID は取得ごとに変わるので、ID の変化が新しい取得と一対一で対応する
- 起動時の取得と二重に走らせないこと。既存の single-flight を活かす
- 取得に失敗したら**既存の一覧を保つ** (直前の増分で入れた不変条件を壊さない)
- **`remote_state_generation` 自体は削除しない。** page cache のキー、viewer の無効化、
  `require_remote_state_generation` による stale 検出という別の用途がある。
  home 画面のデータがそこへ乗るのをやめるだけ
- なぜセッション取得が完全な信号なのか (= 本体は操作権を持たない間しか変更できない) を
  コメントに残す。後から「世代でやるべきでは」と揺り戻さないため

### 1.5 テストの穴

直前の増分のテストは「世代が変わったら再取得する」ことしか見ておらず、**世代が変わるか
どうか**を見ていなかった。だからテストは緑のまま実機で壊れていた。同じ形を作らないこと。

- セッション取得が成功したとき home 画面のデータが取り直されること
- 起動時に二重取得にならないこと
- 取得に失敗したとき前の内容が残ること
- セッションを失っただけ (取得していない) では取り直さないこと

## 2. iPad で効かない音量スライダーが出ている

### 2.1 観測 (利用者による切り分け済み)

- **iPad: 音量スライダーを動かしても効かない**
- **PC Chrome: 効く**

iOS / iPadOS の Safari は `HTMLMediaElement.volume` への代入を無視する (音量はハードウェア
ボタン専用)。実装漏れではなく**プラットフォーム制約**。

### 2.2 やること

**効かない操作部品を出しておくのが問題。** 効かない端末では音量スライダーを出さない。

- 判定は**代入して読み戻す**方法で行う。代入が無視される端末では値が変わらない。
  effective かどうかを判定して記録し、以後スライダーを出さない
- 判定のために利用者に聞こえる音量変化を起こさないこと。効かない端末では何も起きないので
  無害だが、効く端末で不要な上下をさせない形にする
- 端末ごとの UA 判定で分岐しないこと。将来の挙動変更に追随できない。**実際に効くかを
  見る**

### 2.3 スライダーを相対移動へ揃える

効く端末向けの話。音量は素の `<input type=range>` なので押した位置へ飛ぶ。シークバーは
意図して相対移動にしてある (押下位置の絶対値を採らず、押下時の値からの相対移動)。
**同じ操作感へ揃える。** 判定は既存のシークバーと同じ純関数を共有できるか確認し、
できるなら共有する。二重に書かない。

## 3. やってはいけないこと

- 世代の判定に「見る項目」を足していく方向で直すこと (§1.3)
- `state.snapshot` の更新をやめること
- UA 文字列で音量の可否を判定すること
- 音量とシークで移動量の計算を二重に持つこと

## 4. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`crates/remote-ipc` / `crates/remote-web`、
  web テスト一式
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- `cargo check` の警告が増えていないこと
- ビルドとコミットは行わない。`htdocs/` は触らない
