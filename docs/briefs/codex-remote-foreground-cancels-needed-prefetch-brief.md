# リモート閲覧: 前景要求が、自分で使う先読みを本体側で打ち切る

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

**直前の増分 (`143fc596`) の続き。** Web 側の同型の不具合は直したが、**本体側に同じ形が
もう一段あった**。症状は同じ (シークバーだけ進み、画面が変わらない)。

## 1. 観測された失敗 (ログで確定)

利用者報告: 画面左上に「ページの編集結果合成は取り消されました」が出た状態で、
シークバーは 2-3 ページ、画面は 1 ページ目のまま。

`page_display` テレメトリ:

    515358ms  applied      dom_committed  要求=[1]    適用=['154']
    516410ms  not_applied  fetch_failed   要求=[2,3]  候補=[] 適用=[]

- 前回の `abort` ではなく **`fetch_failed`**。Web 側の中止経路は塞がっている
- **失敗は全て見開き (`[2,3]`)。単ページ (`[1]`) は常に成功している**
- 表示された文言は [src/remote_ipc/container.rs:830](../../src/remote_ipc/container.rs) の
  `MediaErrorCode::Busy` / 「ページの編集結果合成は取り消されました」。
  `execute_edit_source` が `Ready` 以外 = **cancel トークンで打ち切られた**ときに返る

## 2. 原因 (source inspection で確定)

[src/remote_ipc/container.rs:960](../../src/remote_ipc/container.rs) の `begin_page_render`:

    if priority == PagePriority::Foreground {
        for prefetch in prefetches.drain(..) {
            prefetch.store(true, Ordering::Relaxed);
        }
    } else {
        prefetches.push(Arc::clone(&cancel));
    }

**前景のページ描画が始まると、登録済みの先読み描画を全部 cancel する。**
優先度は URL の `prefetch=1` の有無で決まる
([crates/remote-web/src/http.rs:3178](../../crates/remote-web/src/http.rs))。

見開きは 1 ページずつ別々に要求される。したがって:

1. ページ 1 を表示中に、2 と 3 が先読みで走る
2. 捲る。Web 側の `loadForeground` は、進行中の先読みには相乗りし
   (URL は `prefetch=1` のまま = 本体から見て先読み)、進行中でないページだけ
   新規に前景として要求する (`prefetch` なし)
3. 見開きの片方が先読み中・もう片方が未取得だと、**後者の前景要求が本体へ届いた瞬間に、
   前者の先読み描画が cancel される**
4. cancel された側が `Busy` を返し、`Promise.all` なので見開き全体が失敗する
5. シークバーは既に進んでいるので、画面だけ 1 ページ目のまま残る

単ページで起きないのは、相乗りと前景要求が同時に存在しないため。
**前景が、自分がこれから使う仕事を打ち切っている**のが根本原因。

## 3. やること

**前景要求は、自分自身が使う描画を打ち切らないこと。**

- 現在 `page_prefetch_cancels` は `Arc<AtomicBool>` の列で、**どの先読みが何を描いているか
  分からない**。だから前景は一律に全部消すしかない。ここを直す
- 前景要求は自分の対象ページを知っており、**見開きの相方も `render_context.spread_partner`
  として既に受け取っている**。この情報で「自分が使う描画」を判別できる
- 判別できる状態にしたうえで、**自分の描画対象に当たる先読みは打ち切らない**。
  それ以外の先読みは従来どおり打ち切ってよい

### 3.1 併せて検討すること (報告に書く)

直前の増分で、本体は heavy worker のうち **1 本を前景へ必ず予約する**ようにした
(`remote_page_prefetch_limit = min(heavy_workers - 1, 2)`)。前景が枠を待つことは無い。

そのうえで「前景が来たら先読みを全部打ち切る」が今も必要かを評価すること。必要なら
残してよいが、**その理由を報告に書くこと**。不要なら範囲を狭める方が状態が減る。

## 4. 判断が要る点 (実装前に報告してよい)

本体は `MediaErrorCode::Busy` を返している。これは「一時的なので後で試せる」という
意味であり、Web 側は先読みとサムネイルでは同種の busy を再試行している。しかしページの
経路は再試行せずに失敗させている。

**サーバが再試行可能と宣言している応答を、ページの経路でも尊重すべきか**を判断して
報告すること。これは症状 guard ではなく契約の話だが、**§3 の根本原因の代わりにしない**。
根本原因を直したうえで、なお必要かで判断する。

## 5. やってはいけないこと

- `Busy` の再試行を入れて §3 を直さずに済ませること
- 先読みを打ち切る仕組みごと消して、前景が枠を待つ状態へ戻すこと
- Web 側で相乗りをやめて毎回前景で取り直すこと (先読みの投資を捨てることになる)
- 直前の増分で入れた予算・同時実行数・待機カウントの判断を戻すこと

## 6. テスト

- **修正前に落ちるテストを先に書くこと**: 見開きの片方が先読み中、もう片方が前景要求の
  とき、先読み側が cancel されずに完了すること
- 前景の対象でない先読みは、従来どおり前景開始で打ち切られること
- 単ページの前景要求でも同じ規則が働くこと
- `finish_page_render` の登録解除が、上記の変更後も漏れないこと

## 7. 確認と報告

- `cargo test -p mimageviewer --lib` / `cargo test -p mimageviewer-ipc` /
  `cargo test -p mimageviewer-remote` /
  `node --test --experimental-test-isolation=none crates/remote-web/web/*.test.mjs` /
  `cargo fmt --all -- --check` / `python scripts/check_ui_glyphs.py` を全て実行する
- 先読みの識別をどう表したかを報告に書く
- 決定は `docs/web-remote-plan.md` へ書き戻す
- ビルド (`build-dev.ps1`) とコミットは行わない。`htdocs/` は触らない
