# リモート閲覧: 前景が相乗りした先読みを、先読み計画が中止してしまう

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 1. 観測された失敗 (ログで確定)

利用者報告: 1 ページから 2, 3 ページへ移動したのに画面が 1 ページ目のまま。
**シークバーのページ数は更新されていた。** 再現しない。

直前の増分で入れた `page_display` テレメトリが捉えた:

    705195ms  applied      dom_committed  要求=group0 [1]    シーク=group0  適用=['82']
    705951ms  command      next_page
    705956ms  not_applied  abort          要求=group1 [2,3]  シーク=group1  候補=[] 適用=[]

- 捲ってから **5 ms** で中止。通信が失敗したのではなく、**開始時点で中止済み**
- 候補が空。画像が 1 枚も得られていない
- **そのあと何も再試行していない**
- 同時刻の `browser_double_tap` は `suppressed: false` / `recognized_double_tap: false` で
  無関係 (1013px / 2742ms 離れたタップを正しく別物と判定しただけ)
- 同時刻に `remote_session` の遷移は無い。セッション失効経路ではない

## 2. 原因 (source inspection で確定)

[crates/remote-web/web/app.js](../../crates/remote-web/web/app.js) の `PageResourceCache`。

**前景は進行中の先読みに相乗りする** (`loadForeground`):

    const joined = this.active.get(request.cacheKey);
    ...
    if (joined) {
      const resource = await awaitWithAbort(joined.promise, signal);

**先読み計画の作り直しは、計画に無い進行中を中止する** (`schedule`):

    this.pending = unique;
    for (const active of this.active.values()) {
      if (!seen.has(active.key)) active.controller.abort();
    }

`seen` は**先読み対象だけ**で、先読み窓は現在のグループを含まない (前後だけ)。
したがって:

1. ページ 1 を表示中に、2-3 が先読みで `active` に入る
2. 捲ると前景が `loadForeground` で 2-3 の進行中 promise に相乗りする
3. 同じ捲りで計画が作り直され、新しい窓は 4-5 以降。**2-3 は `seen` に無い**
4. `schedule` が 2-3 を中止する。**前景が待っている当のもの**
5. 前景は `AbortError`。候補ゼロで終了し、再試行は無い

**所有の取り違えが根本原因。** `active` の要素は「先読みが所有する、いつでも捨ててよい通信」
として扱われているが、前景が相乗りした瞬間からそうではなくなる。それを表す状態が無い。

再現しないのは、**捲った瞬間にその先読みがまだ飛んでいる場合だけ**成立するため。
1 ページ 0.6〜2.4 秒かかるので、開いた直後に速く捲ると当たる。ログでも 1 ページ目の適用から
756 ms 後に捲っていた。

## 3. やること

**前景が相乗りした通信は、先読み計画から中止できないようにする。**

- 相乗りが起きた時点で所有を移すのか、待っている前景を数えて中止対象から外すのかは
  選んでよい。**「先読みが所有する通信」と「前景が待っている通信」を区別できる状態**に
  すること
- **`schedule` の `seen` に現在のグループを足すだけの修正は採らない。** それはこの再現
  経路だけを塞ぐもので、「前景が相乗りした通信を計画が中止できる」構造は残る。前景が
  相乗りし得る全ての key に対して同じ危険が続く
- 前景が自分の `signal` で中止されるのは従来どおり正しい。塞ぐのは**他人の都合による中止**

## 4. 併せて判断すること (報告に書く)

中止でページが表示されなかったとき、**シークバーは既に進んでいる**。表示と表示位置が
食い違ったまま残り、利用者は操作しても戻せない。

- 表示できなかったときに、表示位置の表示をどう扱うか
- 中止以外の失敗 (`fetch_failed` 等) でも同じ食い違いが起きるか

**この増分で直すかは判断して報告すること。** 根本原因 (§3) と混ぜないこと。
症状 guard として「失敗したら再試行する」を足すのは**しない**。

## 5. やってはいけないこと

- `seen` に現在グループを足すだけで済ませること (§3)
- 失敗時の自動再試行 / delay / 追加 repaint を根本原因の代わりに入れること
- 前景が自分自身の signal で中止される経路を塞ぐこと
- 先読みの同時実行数や予算を、この不具合の回避のために変えること (直前の増分の判断を戻さない)

## 6. テスト

- **修正前に落ちる**テストを先に書くこと: 先読み中の key へ前景が相乗りし、その直後に
  その key を含まない計画で `schedule` を呼んでも、前景が解決すること
- 前景が相乗りしていない先読みは、従来どおり計画から外れたら中止されること
- 前景が自分の signal で中止されたときは、従来どおり中止されること
- 相乗り中に 503 が来たときの既存の扱い (前景は一時的な admission 失敗を継承しない) が
  壊れていないこと

## 7. 確認と報告

- `cargo test -p mimageviewer --lib` / `cargo test -p mimageviewer-ipc` /
  `cargo test -p mimageviewer-remote` /
  `node --test --experimental-test-isolation=none crates/remote-web/web/*.test.mjs` /
  `cargo fmt --all -- --check` / `python scripts/check_ui_glyphs.py` を全て実行する
- 所有をどう表したかを報告に書く
- 決定は `docs/web-remote-plan.md` へ書き戻す
- ビルド (`build-dev.ps1`) とコミットは行わない。`htdocs/` は触らない
