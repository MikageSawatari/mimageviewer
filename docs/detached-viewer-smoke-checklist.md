# Detached viewer 実機 smoke チェックリスト

作成: 2026-06-29 / ClaudeCode

目的: keep-alive (K0) 導入後の detached image window を、モード × コンテンツ × 操作の
組合せで実機確認する。immediate viewport の破棄/再生成・小窓・閉じ漏れは unit test で
再現できないため、ここを目視 + デバッグログで確認する。

正本設計: [detached-viewer-keepalive-design.md](detached-viewer-keepalive-design.md)。

---

## 0. 準備

- `MIV_DETACHED_WINDOW_DEBUG=1` を立てて起動する (PowerShell: `$env:MIV_DETACHED_WINDOW_DEBUG=1`)。
- ログ: `%APPDATA%\mimageviewer\logs\mimageviewer.log`。
- テスト素材: 通常画像フォルダ複数 + ZIP 複数 + PDF 複数 (`d:\home\scan\comic` 等) +
  動画 + アニメ GIF/WebP を 1 箇所に。

### ログの成功マーカー (どのケースでも共通)

| 観点 | 成功 | 失敗 (要報告) |
| --- | --- | --- |
| ウィンドウ破棄/再生成 | `host_lost_diag` / `clear host` が **folder-nav で出ない** | folder-nav のたびに出る |
| 小窓 | `captured host` の rect が **822x656 等の既定サイズにならない**・HWND 値が変わらない | 822x656 がカスケード |
| window_id churn | folder-nav で `allocate_window_id` が **出ない** (`reuse_active_window_id` は可) | 毎ナビ allocate |
| 空窓の生き残り | close 後に `keepalive_backstop` が **続かない** | close 後も延々 backstop |
| session 整合 | close 操作で `session_closing` → `session_finish` が **出る** | 出ない (= 閉じられない) |

---

## 1. 通常モード (設定「画像を別ウィンドウで開く」= OFF)

> F12 で現在の画像/本を別ウィンドウへ。メイン一覧カーソルに追従 (ピンで切り離し)。

### 1.1 静止画

- [ ] 画像を F12 で別ウィンドウ表示 → ウィンドウが正しいサイズで出る
- [ ] その状態で Ctrl+↓ / Ctrl+↑ (フォルダ移動) → **小窓が出ず**、同じ窓で内容だけ切替
- [ ] 次/前の画像 (→/←/Space) → 同じ窓で滑らかに切替・黒フレームが目立たない
- [ ] Esc / × / グリッド復帰 → **窓が閉じる**・空窓が残らない
- [ ] 別ウィンドウ表示中にメイン一覧で別画像を選択 → 別ウィンドウが追従して切替

### 1.2 ZIP / PDF (本)

- [ ] PDF を別ウィンドウで開く → ページ表示
- [ ] **Ctrl+↓ で次の PDF/ZIP/フォルダへ** → **小窓・ちらつき・ウィンドウ再生成なし**（最重要・回帰元）
- [ ] ページ送り (→/←/Space) → 同じ窓で切替
- [ ] BS / 親へ戻る (ページ一覧/親フォルダ) → 想定どおり (窓は閉じる or 親表示)
- [ ] Esc / × → 窓が閉じる

### 1.3 F11 仮想フルスクリーン (borderless)

- [ ] 別ウィンドウで F11 → 装飾なし・モニタ全面 (仮想フルスクリーン) になる
- [ ] **F11 のまま Ctrl+↓ (フォルダ移動)** → **仮想フルスクリーンのまま**（最大化ウィンドウに化けない・今回修正点）
- [ ] F11 のままページ送り → 仮想フルスクリーン維持
- [ ] F11 をもう一度押す → 元の通常ウィンドウ配置に戻る
- [ ] F11 中に Esc/× → 閉じる

### 1.4 設定切替 (2 モード制)

- [ ] 別ウィンドウを開いたまま「画像/動画を別ウィンドウで開く」を ON/OFF → 開いていた別ウィンドウが閉じ、通知が出る
- [ ] 窓が無い状態で同じ設定を ON/OFF → 余分な通知は出ない
- [ ] OFF に戻した後、F12 の linked 別ウィンドウはメイン一覧に追従する

### 1.5 動画 / アニメ

- [ ] 動画を別ウィンドウで再生 → 音/映像 OK
- [ ] 動画別ウィンドウ中に Ctrl+↓ → 破綻しない (再生継続 or 切替)
- [ ] アニメ GIF / WebP を別ウィンドウ → ループ再生、folder-nav で破綻しない

---

## 2. 常に別ウィンドウモード (設定「画像を別ウィンドウで開く」= ON) ⚠️ 未検証重点

> 画像を開くたびに新しいウィンドウ。古い窓は passive (frozen) として残る。
> このモードの detached 窓では Ctrl+↑↓ / Ctrl+PageUp/PageDown のフォルダ移動は行わない。
> 入力は detached 窓側で消費し、短い案内だけ出す。静止画系の F12 detached 切替も無効。

### 2.1 複数ウィンドウ展開

- [ ] A.zip のページと B.zip のページを **別々のウィンドウで同時表示**できる
- [ ] それぞれ独立して閉じられる (片方閉じても他方が残る)
- [ ] 新しい画像を開くたびに新窓が出て、**古い窓は内容を保ったまま残る** (passive)
- [ ] passive 窓が **既定サイズ (822x656) に縮まない**・勝手に閉じない

### 2.2 ウィンドウ切替

- [ ] passive 窓をクリック (アクティブ化) → その窓が active になり送り / V / Shift+Z の対象に
- [ ] active を切り替えても、他の窓の内容が**別画像へ化けない**
- [ ] active 窓でページ送り → その窓だけ切替、passive は不変
- [ ] active 窓で F12 / Ctrl+↓ / Ctrl+PageDown → 無効 toast。メイン一覧は動かない

### 2.3 メイン操作との干渉

- [ ] メイン一覧でソート変更 → 表示中の窓が誤って閉じない/別画像に化けない
- [ ] メイン一覧でフォルダ移動 → passive 窓が保持される
- [ ] 元コンテナ (ZIP/PDF) が消えた場合のエラー表示と close cleanup が破綻しない

### 2.4 close 整合

- [ ] active 窓を Esc/× → 閉じる・空窓が残らない
- [ ] 全 passive 窓を順に閉じられる
- [ ] ログで `session_closing`/`session_finish` が各 close で出る・backstop が残らない

---

## 3. F12 OFF (別ウィンドウモード無効)

> detached を使わず従来の (メインウィンドウ内) フルスクリーン。

- [ ] 画像/PDF を通常フルスクリーンで開く → 別ウィンドウは出ない
- [ ] Ctrl+↓ folder-nav → 従来どおり (v2.2.0 挙動) 滑らか
- [ ] F12 を押す → 現在の表示が別ウィンドウへ移る (session 開始)
- [ ] 別ウィンドウ表示中に F12 → 別ウィンドウが閉じてメインフルスクリーンへ (session 終了)

---

## 4. モード切替・境界

- [ ] 通常モード ⇄ 常に別ウィンドウ (設定切替) を表示中に変更 → クラッシュ/窓残りなし
- [ ] detached 静止画 → 動画へ移動 (presentation が detached→fullscreen) → 窓が適切に遷移
- [ ] 別ウィンドウで開いたまま アプリ最小化 → 復帰 → 表示維持
- [ ] マルチモニタ: 別ウィンドウを別モニタへドラッグ → folder-nav で位置/サイズ維持

---

## 5. 退行が無いか (keep-alive で触った周辺)

- [ ] 通常フルスクリーン (非 detached) の folder-nav が v2.2.0 同様に動く
- [ ] PDF/ZIP の enumerate 待ち中の holdover (前ページ表示) が出る・黒フラッシュしない
- [ ] スライドショーが detached/非 detached で動く
- [ ] 通常 F12 linked detached では補正/AI アップスケール/消しゴム等が動く
- [ ] ピン / always-new detached では消しゴム等の編集入口が無効化され、全体補正/ポストフィルタ/V/Shift+Z は動く

---

## 報告のしかた

- NG が出たケースは「§番号 + 操作 + 観測 (小窓/ちらつき/閉じない 等)」を記載。
- 可能なら NG 直後のログ (`mimageviewer.log` の該当時刻付近) を添付。
- 成功マーカー (§0) のどれが崩れたかが分かると切り分けが速い。

---

## レビュー記録

### 2026-06-29 Codex レビュー #1

チェックリスト全体の構成は妥当です。通常モード OFF の K0 回帰点、F11 borderless、
always-new の複数窓 / passive 切替 / close 整合、F12 OFF 退行まで、実機で見るべき面が
優先度順に並んでいます。`MIV_DETACHED_WINDOW_DEBUG` の成功マーカーも、今回の小窓 /
閉じ漏れ原因を確認するには有効です。

1 点だけ注意があります。§0 の `session_closing` → `session_finish` は **active detached
session を閉じる操作**の成功マーカーであり、always-new の **passive 窓を個別に閉じる**
操作では必ずしも出ません。passive close は `passive_close ids=...` と viewport `Close`
command、または対象 snapshot の消滅を主マーカーにしてください。したがって §2.4 の
「各 close で session_closing/session_finish」は、active 窓 close では必須、passive 窓 close
では `passive_close` と空窓 / backstop 残りなしを確認、という読み替えが必要です。

実機確認の優先順位はこのままでよいです。まず §1.3 (F11) と §2.1〜2.4 (always-new) を重点確認し、
NG が出た場合は §番号、操作、見た目、該当ログをセットで残してください。
