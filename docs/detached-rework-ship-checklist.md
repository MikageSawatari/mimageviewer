# Detached リワーク 出荷前 実機検証チェックリスト (2026-07-07)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md) §7 (出荷ゲート)
ベース: [smoke-matrix](detached-viewer-smoke-matrix-20260630.md) (CUT 前の記述を本書 §2 で補正)

## 0. 準備

- `MIV_DETACHED_WINDOW_DEBUG=1` を設定して起動する
  (PowerShell: `$env:MIV_DETACHED_WINDOW_DEBUG=1; .\target\release\mimageviewer.exe`)。
  findings-11 C3 により、このフラグでログが **256MB × 4 世代** 自動保持される =
  **手動のログ退避は不要**。異常が出たら発生時刻をメモするだけでよい。
- 素材: `H:\home\mimageviewer_detached_smoke_20260630` (smoke-matrix §0 参照)。
- 異常の報告形式: 「ケース ID + 操作 + 見た目 + 発生時刻」。

## 1. smoke-matrix の実施 (連続 2 回グリーンが条件)

[smoke-matrix](detached-viewer-smoke-matrix-20260630.md) §3 を S1 → S3 → S2 の順で実施。
時間がないときは §4 の時間別範囲 (10 分 = A1-A4, B1-B3, C1-C3) で 1 周し、
フル 2 周は時間が取れるときに行う。

- [ ] 1 周目: S1 (A1-A6, B1-B5) / S3 (C1-C6, D1-D5) / S2 (E1-E3) / 動画 (F1-F4)
- [ ] 2 周目: 同上

## 2. smoke-matrix の CUT 後補正 (読み替え)

ピン留めは CUT で撤去済みのため、以下を読み替える:

| 旧ケース | 扱い |
| --- | --- |
| A1 の「ピン切替」表記 | ピンは存在しない。A1 = OFF (毎回別ウィンドウ OFF) の F12 linked 追従確認のみ |
| F5 (ピン留め静止画窓で ↑↓ → 動画) | **廃止**。代替: OFF モードの linked 窓で ↑↓ し動画に到達 → 再生 or 案内が出て固まらない、を確認 |
| F6 (アニメを F12 → ピン) | **廃止**。代替: S3 (ON) でアニメ画像を開き、別窓へ切替えてもアニメ再生が止まらないことを確認 (F4 に統合) |
| 共通マーカーの「linked 復帰 passive_activate_still_committed」 | OFF モードでは linked 窓は 1 枚のみで passive にならない (CUT §1)。linked の passive 復帰ケース自体が存在しない |

## 3. findings 回帰スポットチェック (リワーク中に潰したバグの再発確認)

各 1〜2 分。1 周目と 2 周目の両方で確認する。

| ID | 操作 | OK 条件 | 由来 |
| --- | --- | --- | --- |
| R1 | ON モードで窓 3〜5 枚 → アクティブを速く切替 (10 回+) | 窓が消えない・点滅しない・クリック 1 回で必ず復帰 | findings-10 / 8 |
| R2 | R1 の窓を 1 枚ずつ全部閉じる | 他の窓がフラッシュ/移動/突然表示しない。× を押して隣の窓がアクティブ化しない | findings-12 D1/D2 |
| R3 | 大きめフォルダを開いた直後 (サムネ読み込み中) に PDF を別窓で開く | メイングリッドが動かない (スクロール/リフローなし)・サムネ読み込みが止まらない | findings-12 D3 |
| R4 | PDF 多数フォルダで窓を開いたままサムネ一覧をスクロール | サムネが出続ける (長時間止まらない) | findings-9 B2 / 11 C1 |
| R5 | OFF モードで F12 を 10 回+ 連打 (静止画/PDF) | メインの文字が消えない (font atlas)・窓が同じ位置サイズで開閉 | CUT fix2 / R1 系 |
| R6 | 動画を F12 別窓 → メイン窓クリック (park) → 再生継続を確認 → 動画窓クリックで復帰 → ホイールでファイル切替 → 閉じる | 全手順が滑らか・close 後メインのフォルダ/一覧が無傷 | R2b live-park |
| R7 | ON⇔OFF 設定切替 (窓を開いた状態で) | 全窓自動クローズ + トースト | CUT §6 |
| R8 | セッション終了後に `panic.log` を確認 | 新規 panic なし (Y-32 / OOM 含む) | プラン §7 |

既知・対象外: OFF モード F12 切り離し瞬間の「黒い線」(キャプチャに写らない、
scanout レベル、findings-13 でクローズ済み)。これは NG に数えない。

## 4. ログの見方 (異常があったときだけ)

```powershell
$log = "$env:APPDATA\mimageviewer\logs\mimageviewer.log"
Select-String -Path $log -Pattern 'passive_activate|passive_close|active_context_state|session_|host_lost|clear host|allocate_window_id|font_resync|panic|hwnd_adopted|liveness_clear|rejected_default|deferred_registration_delayed' | Select-Object -Last 250
```

異常の代表シグネチャ (出たら NG として時刻ごと報告):

- `parked_hwnd_liveness_clear` (窓を閉じていないのに出る) = 窓の裏死
- `passive_placement_update_rejected_default` = 既定サイズ再生成
- `hwnd_adopted_watcher` (生きた窓がある状態で出る) = repair 誤採用
- `deferred_registration_delayed` (初回 open 以外で出る) = 直列化スキップ
- `down_window_from_point_mismatch` = stale hwnd 棄却

## 5. 出荷までの残りステップ (プラン §7)

1. [ ] 本チェックリスト (smoke 2 周 + R1-R8 × 2) グリーン
2. [ ] `detached-rework` → ローカル `master` へ merge (チェック通過後すぐでよい)
3. [ ] **2 週間の実機常用で新規 P1 ゼロ** (P2 以下は backlog 化で出荷可。
       常用は master ビルドで行い、`MIV_DETACHED_WINDOW_DEBUG=1` は付けたままを推奨 =
       もし P1 が出ても証拠が自動で残る)
4. [ ] リリース作業 (CLAUDE.md「リリース手順チェックリスト」Phase 0 から)。
       README 更新履歴に detached リワークの要約 (ユーザー向け表現) を含める
