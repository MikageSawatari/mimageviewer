# 引き継ぎ (2026-08-11 時点)

`web-remote` ブランチ。次のセッションはここから読む。

## 次にやること (2026-08-11 に利用者と合意)

**表示周りの優先度キュー化から進める。設定導線はそれが安定してから。**

優先度キュー化は独立した小作業ではなく、下の **cutover そのもの**である。本体の heavy
queue を 3 レーン化するだけで拒否をやめると、**昇格と剪定が無いぶん今より悪化する**
(読者が先へ進んだ後の古い先読みが queue に溜まり、前景がその後ろで待つ)。昇格と剪定は
D0 と同じものなので、段階 1 → 2 → 3 を順に進める。

設定導線を後にする理由は、**同時に 2 つ変えると実機確認で切り分けられなくなる**から。
表示側が安定してから着手する。

## いま止まっている論点

**所有権の cutover。** 正本は git 管理下の [../web-remote-plan.md](../web-remote-plan.md)
**§14 系** (§14 / §14.5 / §14.5.1 / §14.6 / §14.6.1 / §14.8 / §14.8.1)。

段階 A (表示グループの outcome 契約) と下の段階 1 は完了。残りは 2 → 3 の順。

1. ~~**契約と状態機械を純粋テストで固定**~~ — **完了 (2026-08-11、`fd466b9d`)**。
   `crates/remote-web/web/page-coordinator.mjs` に `pageResourceKey` と
   `PageDisplayCoordinator`。契約 11 項目は plan §14.9 が正本。dormant (app.js から未使用)
   なので実機の挙動・protocol version・telemetry は変わっていない。
   ブリーフは [codex-remote-display-coordinator-stage1-brief.md](codex-remote-display-coordinator-stage1-brief.md)
2. ~~**本体側の基盤を dormant で追加**~~ — **完了 (2026-08-11、`d6256def`)**。
   `src/remote_ipc/page_jobs.rs` (ページジョブ registry、typed cause、promote/release) と
   `src/remote_ipc/heavy_queue.rs` (3 レーン + レーン別容量 + 昇格 + 剪定)。plan §14.10 が正本。
   request 経路からは未使用で protocol version も据え置き。
   ブリーフは [codex-remote-page-jobs-stage2-brief.md](codex-remote-page-jobs-stage2-brief.md)
3. **段階 3 は 3a / 3b / 3c に分割した** (plan §14.11 が正本)。
   - **3a 完了・実機確認済み (`4e13ae8a`)** — 取消と優先度の所有権を Web / 本体で同時に切替。
     protocol v42。4 つの取消機構が 1 本の lease に置き換わった
   - **3b 完了・実機確認済み (`507af0f1`)** — heavy lane を 3 レーン queue へ差し替え、
     入口拒否を撤去。目視では判断できなかったのでログで測った (plan §14.12 末尾に実測表)
   - **3c 完了・実機確認済み (`68bb2645`)** — 位置の requested / displayed 所有権。
     plan §14.13。`positionRequest` token を廃止し、義務を (requested, displayed) の対から
     導く形にした
4. **混在フォルダのパリティ 完了・実機確認済み (`c3dc2743`)** — protocol v43。plan §12.27。
   本体の `physical_page_order_locked` と seek overlay の分類をリモートから直接呼ぶ形にした
5. **master 取り込み済み (`7de01e38`)** — v2.13.0 を含む 45 件。`master` は `web-remote` に
   完全に含まれる。退避ブランチ `web-remote-premerge` あり

## 段階 3c で入ったもの (正本は plan §14.13)

`viewer-position.mjs` の純粋状態機械が requested / displayed の identity snapshot を持つ。
巻き戻し義務は token ではなく**その対**から決まる (requested が displayed より先にあれば、
終端失敗した側が誰であっても戻す)。これで §14.5.1 の 2 経路が入口別の分岐無しに閉じた。

- 位置を動かす入口は `requestPageGroup` の 1 本 (ページ送り / seek / bookmark jump)。
  `state.pageGroupIndex` への代入は `assignOwnedPageGroupPosition` の 1 か所だけ
- URL は常に requested を写す。`request` で push、実際に巻き戻したときだけ `replaceState`
- `pageGroups` の再構成は `reanchorViewerPageGroups` が唯一の境界。displayed も
  生きている配列の snapshot しか持たない (DOM commit 時にも再解決する)
- owner と `state.pageGroupIndex` が食い違ったら再アンカーで収束させ、型付きの理由を残す

**確認済み (2026-08-12)**: 通常のページ送り / seek / bookmark jump、および読み込み失敗時に
画面・ページ番号・タイトル・URL が揃って前ページへ戻ること。

**表示失敗を実機で再現する手順**: 読み込み中に通信を切るのはタイミングが合わない。
**先に機内モードにしてからページを送る**と fetch が即失敗して同じ経路を通る。Wi-Fi を戻して
リロードし、送ろうとした先ではなく元のページが開けば URL も戻っている。

**まだ未確認 (3a から持ち越し)**:

- **64 MiB 予算を埋めた深い高解像度コンテナで、遠い取得済みページが近い候補と交換される**
- **表示中の切断 / 再接続、セッションの解放 / 再取得**

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
