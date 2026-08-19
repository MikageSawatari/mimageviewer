# 静止中に変換アーカイブ判定が毎フレーム再起動する

## 症状 (計測済み、推測ではない)

リリース前ゲート `.\scripts\check-idle-health.ps1 -Scenario static-foreground` が FAIL する。

```
events=11185 frames=2478 update_rate=164.96/s tail_repaint=2479 input=0
tail actions: none: 2479
FAIL: 静止中の update rate が 164.96/s です (上限 10.00/s)
FAIL: CPU one-core ratio 0.4656 exceeds 0.1
FAIL: perf_events.jsonl grew by 3079256 bytes (limit 262144)
top_repaint_causes: src\app.rs:65549 (1240)
```

同じ perf log をセッション全体 (t=7.4〜291.6) で数えると:

| 指標 | 値 |
| --- | --- |
| `nav/archive_cache_peek` の総数 | **10,946** |
| うち `peeked > 0` (実際に候補を調べた) | **2** |
| 起動間隔の中央値 | **12 ms** |

つまり **10,944 回は、ワーカースレッドを spawn して候補ゼロで即終了している**。
その間 `converted_archive_cache_paths_pending` が Some のままなので
[src/app.rs:65549](../../src/app.rs) の `ctx.request_repaint()` が毎フレーム走り、
静止中に 165fps で回り続ける。`tail_repaint` の action が全て `none` なのは、
repaint 要求がフレーム末尾ではなくこの pending 判定から出ているため。

## 原因

今日の `bc4adbdd` (§2.3「Ask the cheap question first…」) で、判定の起動契機が
**items generation ごとに 1 回** から **毎フレーム、先読みゲート経由**
(`maybe_start_converted_archive_cache_paths_refresh`) に変わった。

`start_converted_archive_cache_paths_refresh` (src/app.rs:24236) の早期 return は

```rust
if direct_inputs.is_empty() && (pin_roots.is_empty() || pin_db.is_none()) {
    return;
}
```

だが、**2 つの入力で「もう答えが出た」の扱いが非対称**になっている。

- `direct_inputs`: `converted_archive_cache_paths: HashMap<archive_key, ConvertedArchiveSourceState>`
  に generation 開始時 (`initialize_converted_archive_cache_paths`) で `Pending` として登録され、
  解決すると `Direct` / `CachedZip` / `Unavailable` の終端状態になる。以降 `Pending` でない
  ものは候補から外れる。**記憶がある。**
- `pin_roots`: サムネイル状態 (`Pending | Evicted` かつ `!requested`) から**毎フレーム作り直す
  だけ**。ワーカーが cascade を walk して「変換対象は無い」あるいは「見つけたが既に
  `already_resolved`」と結論しても、**その結論はどこにも残らない**。

そのため一度でも該当タイルがあると、同じ探索を永久に繰り返す。1 回起動するたびに
worker 側で pin DB lookup + cascade + ファイル metadata まで走るので、無駄は spawn だけではない。

セッションの実測では `peeked > 0` が 2 回あり、そのうち 1 回は t=63.5 (フォルダロード直後)。
**解決に成功した後も同じ root が候補に残り続ける**ので、成功後こそ回り続ける。

## 直してほしいこと

**`pin_roots` にも、通常アーカイブと同じ「答えが出た」状態を持たせる。**

判定条件は概ね次の形にしたい (実装の形は任せる)。ある pin root が候補であるのは

1. この generation でまだ cascade を走らせていない、**または**
2. その root の cascade が見つけた archive_key のうち、まだ `Pending` のものがある

のいずれか。両方 false なら候補から外し、全 root が外れたら worker を起動しない。

失効させる契機:

- items generation が進んだとき (既に `converted_archive_cache_paths` を clear している場所と同じ所有者)
- 利用者が代表サムネイルのピンを付け外ししたとき (`refresh_folder_pin_map` の呼び出し元)
- `folder_thumb_depth` (cascade 深さ) 設定が変わったとき

## やってはいけない直し方

- **時間で間引く** (前回起動から N ms 以内なら skip、`request_repaint_after` で遅らせる)。
  症状を隠すだけで、無駄な spawn は残り、ゲートも「遅い spin」として通ってしまう。
- **前フレームと同じ入力なら skip する** といった frame-to-frame の比較キャッシュ。
  同じ理由。答えを記憶するのと、直前と同じかを見るのは別物。
- pending が Some の間の `request_repaint()` を消す。これは非同期完了を拾うために必要で、
  問題は repaint 側ではなく「終わらない仕事を作り続けている」側にある。

## 注意すべき境界

**表示範囲外で後回しにされた候補を「答えが出た」と記録しないこと。** worker の resolve ループは
`worker_desired_indices` に無い idx を `continue` で飛ばす (peeked にも数えない)。スクロールで
scope が縮んだ場合にこれが起きる。この候補の archive_key は `Pending` のまま残るので、
上の条件 2 が満たされ、次のフレームで再度候補になる — これは**正しい再試行**なので潰さないこと。

cascade 自体は desired set に関係なく走るので、「cascade が何も見つけなかった」という結論は
desired set に依存しない。ここは安全に記録できる。

## テスト

`src/app/tests.rs` の同サブシステムのテスト群 (`converted_archive_cache_paths_pending` を扱う
17400〜18000 付近) に追加する。最低限:

1. **静止フレームで再起動しないこと** — 変換対象を含まないピン付きフォルダを用意し、
   1 回目の refresh が Finished まで終わった後、同じ状態でさらに数フレーム回して
   `converted_archive_cache_paths_pending` が None のままであることを固定する。
   今の実装ならここが Some に戻り続ける (= このテストは修正前に落ちる)。
2. **解決後も再起動しないこと** — ピンが変換対象を指し、解決して終端状態になった後、
   同じ状態で回しても再起動しない。
3. **後回しは再試行されること** — desired set から外れて skip された候補が、
   scope に戻ったときに再び候補になる。
4. **失効すること** — generation 変更 / ピン編集で再び候補になる。

テストが修正前に赤くなることを確認してから直すこと (mutation で確かめる)。

## 完了条件

- `cargo test -p mimageviewer --lib` が緑
- `cargo fmt` 済み
- 上記テストが修正前は赤、修正後は緑
- 変更点を `docs/next-release-backlog.md` の §2.3 完了記録に追記
  (この spin は §2.3 が入れたもの、と分かるように)
