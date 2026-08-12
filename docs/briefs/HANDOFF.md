# 引き継ぎ (2026-08-12 時点)

`web-remote` ブランチ。次のセッションはここから読む。

## いまの状態

**表示所有権の cutover と設定導線は完了。** 直近は実機で出た不具合の修正とマニュアル整備。

| コミット | 内容 | 実機 |
|---|---|---|
| `fd466b9d` `d6256def` | cutover 段階 1 / 2 (dormant) | 不要 |
| `4e13ae8a` `507af0f1` | 段階 3a / 3b (protocol v42、heavy queue 差し替え) | ✅ |
| `c3dc2743` | 混在フォルダのパリティ (v43) | ✅ |
| `68bb2645` | 段階 3c 位置の所有権 | ✅ |
| `733faa85` `6d078969` | PIN を本体が所有 / ダイアログ即時反映 (v44) | ✅ |
| `628d8347` | `tailscale serve` の代行 (v45) | ✅ |
| `06b33254` | tailnet 前提条件の検出と案内 (v46) | ✅ |
| `151e37a1` | **master 取り込み** (v2.13.0 以降 21 件) | ✅ |
| `5d05fff8` | 先読みを枚数設定へ。バイトの門を撤去 | ⏳ |
| `6cf7d843` `423ef3d6` | 保存済みトリムの基準を正準ラスタへ | ⏳ |
| `9ede23e4` | マニュアルにリファレンス追加 + Tailscale 説明 | — |

## 次にやること

1. **デコード先読み** — 実装済み、**実機で測るのが次**。
   表示単位で前後 2 単位ぶんデコード済みの `<img>` を保持し、表示時はその要素を使い回す。
   **効果は仕様で保証されていない** (画面外の `<img>` のデコード結果を保持するかは
   ブラウザ次第) ので測定が主目的。ログの見方は下の「デコード先読みの効果を測る」。
   効かなければ canvas 方式へは進まず、**高画質の選択肢に「ページ送りが遅くなります」の
   注記を足す**方針 (利用者判断)
2. **remote-web 側の 503** (`admission_busy`)。入口で断る形が残っている。2026-08-12 の
   3 分半で 10 件。内訳が出ており、**先読みレーンの上限 2 で断る形が 3 件**
   (`ipc_prefetch_in_flight: 2 / limit: 2`)、**heavy レーンの上限 4 で断る形が 1 件**
   (`ipc_heavy_in_flight: 4 / limit: 4`、先読みは 0)。全体の上限 6 には届いていない
3. **ズーム中の先読みが 8192px を要求する件**。1 ページ 6 MB の主因
4. **診断ログのローテーションが無い**。実測 59 MB
5. **一覧の `/api/thumb` が 404 を返すことがある** (2026-08-12、フォルダを開いた直後に 10 件)。
   本体の返答は `ipc_status: "miv_not_found"` で、**すべて `source_kind: "file"`**。
   2 秒後の 200 は `source_kind: "pdf"` なので、別種の項目で起きている。未調査
6. リリース前: 製品ページにリモート閲覧の記載、README 更新履歴

## 実機で未確認 (3a から持ち越し)

- 64 MiB 予算を埋めた深いコンテナでの遠近交換 — **枚数設定へ移行したので条件が変わった**。
  HUD の先読みドットが `ready` から `missing` へ戻る様子で見る
- 表示中の切断 / 再接続、セッションの解放 / 再取得。クライアント側は機内モードの手順で
  確認済み。**本体側の後始末はログで見る** (接続が切れた connection のジョブが破棄されるか)

## 保存済みトリムの基準について (2026-08-12、本体側と合意)

いまリモートが使っている**正準ラスタ寸法はつなぎ**である。本体側も同じ誤りを持っていたことが
確認され (4096 幅で作った矩形を 8192 幅へ当てると左上拡大になる)、**本体は `CropSettings` に
基準サイズを持たせる形へ直す**。

- 本体が基準サイズを保存するようになったら、**リモートもそれを読む形へ移す**。
  ページ固有の正準寸法を推定する必要がなくなり、vector ページも同じ規則で扱える
- vector ページに正準寸法を与える案は、本体のページ枠まわりが落ち着いてから両側で揃える
- 基準サイズを持たない**既存の保存値をどう解釈するか**は本体の決定に従う。
  リモートが独自に決めない

## 直近で学んだこと

- **カタログの `source_*` を編集座標の基準に使わない。** master が PDF で意味を変え
  (raster px → 1/1000 ポイント)、青一色になった。基準は**ページ固有で要求解像度に依存しない
  値**でなければならない。不変条件「保存済みトリムは要求解像度が変わっても同じ割合を切り出す」
  をテストに置いた
- **本体の挙動を先に確認してから合わせる。** vector PDF を描画失敗にしたが、本体は失敗しない
  (ズーム依存のラスタへ適用する) ので、リモートだけ読めなくなるところだった
- `fetch_ms` は `waitForDisplay` の待ち時間で、体感は `tap_to_display_ms`。取り違えて
  「取得済みなのに待たされている」と誤読した
- **高画質のページ送りが待たされる原因はデコードだけである** (2026-08-12 の端末ログ、
  8192 で先読みが間に合った 17 表示グループ)。`tap_to_display_ms` 中央値 287ms に対し
  `decode_ms` 中央値 271ms で、**差は 16〜30ms**。17/17 がスピナーのしきい値 225ms を
  超えていた。先読み外しは 2464〜3814ms、標準 4096 の `decode_ms` は 94ms。
  **先読み自体は効いている**ので、残りはデコードを前倒しできるかどうかに尽きる

## 作業の進め方 (この一連で確立したもの)

- 実装は Codex (`codex exec -c model="gpt-5.6-sol" -c service_tier="default"
  --sandbox workspace-write`)、ブリーフ / レビュー / テスト / 統合は ClaudeCode、
  実機確認は利用者。**fast は使わない**
- 同じタスクの 2 周目以降は `codex exec resume --last` (新セッションを開かない)。
  resume に `-C` / `--cd` は**無い**ので worktree の中から打つ
- **レビューでは「テストが本物か」を必ず確かめる**。修正を潰して該当テストが実際に落ちる
  ことを見る。この一連で 3 回、これで本物の欠陥を捕まえた
- **release ビルドでしか出ない差に注意**。`debug_assert!` は式ごと評価されないので、
  副作用を入れると release で消える (3b で実害)。`dev-runtime` は release を継承する
- ビルドは `.\scripts\build-dev.ps1`。**`*>&1` や `| Select-Object` を付けて呼ばない**
  (cargo の stderr が terminating error 化して落ちる)
- web の新モジュールは `build.rs` が `web/` を再帰走査して配るので登録不要
  (`*.test.mjs` は自動除外)

### 検証コマンド

```
cd crates/remote-web/web && node --test
cp vendor/ffmpeg/bin/*.dll target/debug/deps/
cargo test -p mimageviewer --lib
cargo test -p mimageviewer-remote
.\scripts\build-dev.ps1
```

### 旧記述 (分割前)

3. ~~**B+C+D0 を一体で cutover**~~ — Web の group lease、requested/displayed の単一 owner、
   ページ別 GET への同一 `DisplayRequestId`、本体のページジョブと昇格・明示 release、
   補正プレビューを要素数 1 の consumer として統合、`loadForeground` の他 active 取消走査と
   `begin_page_render` のアドレス近似を**同時に**撤去
4. 後段 — `prefetch=1` 撤去、lane/fairness、telemetry 拡張

**§2 と §3 には「リモートの優先度キュー化」を畳み込む。** 本体の PDF プールが参照実装
([`src/pdf_loader.rs`](../../src/pdf_loader.rs) の `JobQueue` 3 レーン + dispatcher +
`promote_to_high_normal` + context epoch)。リモートの heavy は `mpsc::sync_channel` の素の
FIFO なので、並べ替えられず「入口で断る」しかなく、それが 503 の正体。**昇格と剪定が
無いまま拒否をやめると悪化する**ので、D0 と同時にやる。

Codex の admission 評価 (2026-08-11) の提案順序も同じ結論だった。最有力は本体の
`try_acquire_prefetch` の `queued > 0` 判定で、queue の中身 (foreground / prefetch /
thumbnail) を区別できていない。

## 2026-08-11 に入ったもの

| コミット | 内容 |
|---|---|
| `15c5a9ab` `12b89d0b` | 表示結果を applied/superseded/failed の 3 値に。終端失敗で位置を戻す |
| `23b6bd40` `7a34143e` | 先読みをバイト予算方式へ。画質設定に実測処理時間 |
| `367f3816` | コンテナ更新に単一 owner。HUD に先読みドット |
| `3b37ba30` | 起動時 TDZ の修正 + 回帰テスト |
| `55061d78` | 消えたメソッド呼び出しの除去 (見開き切替が効かない本体) |
| `ad8a6778` | 弾かれた先読みを最後尾へ回さない。503 判定を状態コードへ集約 |
| `d716f243` | × で本を閉じられない (履歴遡上が no-op) |
| `58e3562e` | 拡大を viewport の `maximum-scale` / `user-scalable` で止める |
| `a04a8d5e` | 二度打ち抑止をやめ観測専用に。除外表を撤去 |
| `7649e763` | 指を離してコマンドが出ない理由を記録 |
| `60105c5d` | tap の許容量を swipe の半分 (26px) へ。連打の抜けが解消 |
| `fd466b9d` | cutover 段階 1。需要と優先度の純粋な状態機械 + 完全なページ資源キー (dormant) |
| `d6256def` | cutover 段階 2。本体のページジョブ registry + 3 レーン heavy queue (dormant) |

## 段階 3 に持ち越した宿題 (段階 2 で判明)

- **1 worker の環境で実行不能な先読みに誰が応答するか。** 予約が `workers - 1` なので
  worker 1 本では先読みが pop されない。今は入口が即 503 を返すが、入口を外すと client の
  timeout まで queue に残る
- **prefetch の `start` に display request ID が無い。** 段階 1 の coordinator は計画由来の
  start に `requestId` を載せない。registry は必須にしているので、wire に載せる identity を
  段階 3 で決める
- 昇格した `Work` は `PageRequest.priority` が Prefetch のまま。pop 時に registry の実効優先度で
  解決するか、render 前に書き換える
- 剪定・拒否・shutdown で捨てる `Work` には typed な応答を返す (`SessionOperation` を drop
  しても client には返らない)
- registry の cancel token と `SessionOperation::cancel_flag()` は別物。render から見える
  取消源を 1 つにするか、明示的な合成点を作る
- `QueueMetrics` と新 queue の二重計上を避けつつ、既存ログの `queued` / `active` /
  `queue=heavy` を保つ

実機確認済み: 見開き切替、× で閉じる、拡大しない、連打でタップが抜けない、ドラッグ誤爆なし。

## 進め方の約束

- 実装は Codex (`codex exec -c model="gpt-5.6-sol" -c service_tier="default"`)、
  ブリーフ / レビュー / テスト / 統合は ClaudeCode、実機確認は利用者
- **速度は fast を使わない** (`service_tier="default"` を毎回明示)
- ビルドは `.\scripts\build-dev.ps1`。**実行中の core と remote サービスを停止する**ので、
  利用者には毎回「起動し直してください」と伝える
- 検証コマンド:
  `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe -ArgumentList '--data-dir','.\target\dev-runtime\data' -WorkingDirectory (Get-Location).Path`
- **原因がログから見えない不具合は、直す前に無言の早期 return へ型付きの理由を足す。**
  2026-08-11 は推測で直した 2 件を外し、観測を先に入れた 1 件だけ一発で当たった
- module scope の const は起動ブロック (`if (!RUNTIME_TEST_MODE)`) より前で初期化する。
  `pwa.test.mjs` に回帰テストあり

## 他に残っている作業

- **リモートの設定導線** (最優先候補)。有効化ダイアログは `PIN: 未設定` /
  `tailscale serve` の状態を表示するだけで、**どちらも設定する手段が無い**。マニュアルの
  「準備する」もこれが決まらないと書けない
- §14.5.1 の 2 経路 (非位置再描画が位置要求を追い越す / bookmark jump) は cutover で塞ぐ
- URL / history が失敗ページを指したまま残る残存不整合 (§14.5)
- 集約系ビューの 38ms `load_into_settings()` (優先度低)
