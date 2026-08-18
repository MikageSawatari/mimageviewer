# backlog §2.6 — ダブルクリックが時々無反応 (計装 + 診断 ZIP の終端保証)

対象: [next-release-backlog.md](../next-release-backlog.md) §2.6 (専用スレ >>246, >>249-250)。
v3.1.2 で対応する、と利用者へ回答済み。

**この版で症状は直らない。手元で再現していないので、報告環境のログを取れるようにする。**
原因を推測して guard を足さないこと (CLAUDE.md「バグ修正の一般原則」)。

報告条件: サムネイル表示、ZIP / RAR、Enter では開ける。1 回だけダブルクリックして
約 10 秒ほかの操作をせず待つと開くことがある。RAR はローカルの直読み対象。

**未再現**。§2.3 の stress folder (サムネイル 6 秒遅延)、判定 cache 上限の 2 倍の RAR、
12 万 entry × 128 本の混在でも再現しなかった。§2.3 でサムネイルは順次出るようになったが、
**それでこの件を解決扱いにしない**。§2.5 の再クリック open で症状が見えなくなっても同じ。

## 1. 先にやる — 診断 ZIP の終端保証

計装を足しても、**ログの末尾が ZIP に入らなければ意味がない**。

現在 perf log は 64KiB `BufWriter` で、App frame から約 1 秒ごとに flush している。
一方 `export_diagnostics_zip` ([diagnostics.rs:14](../../src/diagnostics.rs:14)) は
**直前 flush をせずにログファイルを読む**。そのため通常はボタン直前の約 1 秒分、
UI sleep 中に worker event だけが追加された場合は次の frame までの未 flush 分が落ちる。

- 「ログを zip にする」の**受付を perf event に記録**し、`perf::flush()` と
  `logger::flush()` が完了してから logs を読み込む。この受付イベントが
  **ZIP 内ログの終端 witness** になる (「ここまでは確かに記録された」と言える)。
- 診断 ZIP に含める perf log は現行 `perf_events.jsonl` のみで、rotate 済み `.1`〜`.4` は
  従来どおり除外する。
- UI の「性能ログ ON は次回起動から」「再現後は再起動せず ZIP 化」の案内は維持する。
- テスト: 64KiB 未満の未 flush データが ZIP に含まれること。受付イベントが ZIP 内の
  perf log に存在すること。

## 2. 計装

pointer 操作と archive request の境界を **同一の相関 id** で追えるようにする。
高頻度イベントを常時出さない。**entry ごとのイベントは出さない。**

1. cell の first click / `double_clicked` / `drag_started` と、idx、item key、
   pointer position、current folder、`items_generation`。
2. `grid_open_from_click_allowed` の結果。**拒否時は構造化した block reason**
   (modal / context menu / badge hit / D&D 等) を記録する。**単なる bool や自由文にしない。**
3. activation 要求の accepted / ignored、所有者、item kind、dispatch 完了。
4. RAR inspection の begin / decision-cache hit・miss / end / cancel / error。elapsed、
   走査 entry 数、Direct / Solid / Nested / Encrypted、**folder 判定 worker と明示 open の
   どちらから呼ばれたか**。
5. `pending_direct_nav` の publish / consume、RAR image enumeration の begin / end、
   一覧 install、自動 fullscreen 要求 / paint を同じ相関 id で追えるようにする。
6. auto-aspect の実切替 old / new と、その frame の pointer stream 状態。

### 2.1 相関 id の設計

- 1 つの pointer 操作 (press → release) と、そこから始まる open 要求が同じ id を持つこと。
- **click が成立しなかった場合も id が残る**こと。「入力が成立していない」と
  「accepted 後に待たされている」を**ログだけで分離できる**のが、この計装の目的。
  どちらか一方しか記録できない設計にしない。
- §2.3 で入れた `nav/archive_cache_candidate` と突き合わせられること
  (同じ archive key を使う)。

### 2.2 出す条件

- 性能ログ ON のときだけ出す (`crate::perf::is_enabled()`)。
- **抑制条件を、調査対象の信号そのものに依存させない。** 例えば「open が成立したときだけ
  出す」にすると、成立しなかったケース (= 調べたいケース) が記録されない。この原則で
  過去に 2 回失敗している ([next-release-backlog.md](../next-release-backlog.md) §1.91)。
- 1 操作あたりの件数が有界であること。ダブルクリック連打で線形に増えるのは可。
  frame ごとに出るものを足さない。

## 3. やらないこと

- 症状を消す guard / delay / retry を入れない。**原因が分かっていない。**
- §2.5 の再クリック open で隠れることを理由に閉じない。
- auto-aspect や `Sense::click_and_drag()` の挙動を今回変更しない (候補として記録された
  だけで、確定した原因ではない)。
- entry ごとの RAR イベントを出さない (12 万 entry で爆発する)。
- 時間窓で何かを判定しない。

## 4. 利用者へ渡す手順 (報告に書くこと)

実装後、次の手順を報告に書く。利用者へそのまま案内する:

1. 開発者向け設定で性能ログを ON
2. 再起動
3. 症状を再現
4. **再起動せず**「ログを zip にする」
5. 生成された診断 ZIP を送付

ログにファイル名 / path が含まれる既存の注意書きも案内に含める。

## 5. テスト

1. §1 の flush: 未 flush データが ZIP に入る。受付イベントが ZIP 内 perf log にある。
2. block reason が構造化された値として出ること (自由文でないこと)。
3. click が成立しなかった経路でも相関 id 付きのイベントが残ること。
4. 性能ログ OFF では新イベントが出ないこと。
5. 既存の diagnostics / grid 入力テストが無修正で通ること。**赤くなったら報告して止まる。**

## 6. ドキュメント

- [next-release-backlog.md](../next-release-backlog.md) §2.6 に、入れた計装と
  **ログの読み方** (どのイベントを見れば「入力未成立」と「accepted 後の待機」を
  分離できるか) を追記する。**エントリは閉じない。** 報告待ちのまま残す。

## 7. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
報告には変更ファイル一覧、追加テスト、§4 の手順、出るイベント名の一覧を含める。
