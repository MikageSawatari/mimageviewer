# v3.3.0 出荷前 機能別・横断レビュー台帳

作成: 2026-08-29
比較基準: `v3.2.0` (`09758b90`) .. `master` (`8a80c336`)
差分規模: 369 commits / 210 files / +68,897 -9,909

レビュー開始後に別レビューの変更が `master` へ入ったため、静的レビュー対象は上記
`8a80c336` に固定する。2026-08-29 時点の作業ツリー HEAD は `97bd8a02`。後続の
`215464e2`（回帰テスト負荷対応）およびレビュー記録 commit は本レビューの評価対象外とし、
後段の build / test は「対象コードと同等だが HEAD は異なる」補助検証として記録する。

## 1. 目的と判定基準

v3.3.0 の変更を機能単位で把握したうえで、次を静的レビューと focused test で確認する。

1. バグ修正が症状 guard / delay / retry / blanket reset ではなく、壊れた不変条件の所有境界を直している。
2. 同じ状態を作る全 producer、全 consumer、open / switch / close / cancel / error / late completion を確認する。
3. main / detached / parked viewer の context 固有 resource が兄弟 context を cancel / drain / invalidate しない。
4. UI thread に同期 I/O、decode、重い CPU 処理、worker join、無制限 GPU upload を新設していない。
5. keyboard / mouse / touch / gamepad の既存操作と入力 ownership を維持する。
6. 新しい通常 keyboard operation は `KeyAction` と操作カスタマイズへ完全に配線され、固定入力には理由がある。
7. 新しい非同期処理は request identity、generation、cancel、stale result rejection、bounded allocation を持つ。
8. 永続形式変更は旧データ移行、破損入力、上限、原子的 commit、失敗時の非破壊動作を備える。

回帰テストそのものの実行負荷・所要時間は、別レビューで対応中のため本レビューの指摘対象外とする。

## 2. 機能インベントリと進捗

| ID | 機能群 | 主なコード / 設計 | 状態 | 結果 |
| --- | --- | --- | --- | --- |
| V1 | 動画シークのサムネイルストリップ | `video/seek_strip*.rs`, `app/native_video.rs`, `video-seek-strip-plan.md` | レビュー完了 | R-06（設定保存）、R-20（unbounded cache）を確認 |
| V2 | 動画シークの音声波形 | `video/seek_strip_wave.rs`, `audio_decode.rs` | レビュー完了 | R-21（mode-switch cancellation）、R-22（preemption）を確認 |
| V3 | 360 度動画・4 投影方式 | `video/spherical_metadata.rs`, `native_presenter/panorama_pipeline.rs`, `panorama.rs` | レビュー完了 | R-19（spherical roll）を確認。投影・input ownership・GPU lifecycle は追加指摘なし |
| D1 | detached viewer R2e context registry | `app/viewer_context_registry.rs`, `app.rs` | レビュー完了 | R-02（gamepad context）、R-12（parked probe）、R-24（close/retire）を確認 |
| D2 | presentation transition / F12 / native video handoff | `app/presentation_transition.rs`, `presentation_observer.rs`, `native_video.rs` | レビュー完了 | R-01（abort identity）、R-15（retire rollback）、R-17（command loss）、R-24 を確認 |
| E1 | 見開きページ分割（本体・Remote） | `page_split.rs`, `displayed_image_transform.rs`, `remote_*` | レビュー完了 | R-04（Remote direction）、R-13（説明）、R-18（ZIP 寸法）、R-23（大量 probe）を確認 |
| E2 | 補正レイヤーの左右ページ編集 | `ui_adjustment_panel.rs`, `local_adjust_db.rs` | レビュー完了 | R-03（page transient）、R-25（Delete keymap）を確認 |
| P1 | mask codec・文書予算・旧形式移行 | `local-adjust-core/mask_codec.rs`, `local_adjust_db.rs`, `sidecar.rs` | レビュー完了 | R-07（UI thread 永続化）、R-26（失敗時 authority split）を確認 |
| P2 | sidecar 非同期 writer / materialize | `sidecar.rs`, `preset-and-adjustment.md` | レビュー完了 | R-11（tray wait）、R-14（UI deep clone）、R-16（既知 import）を確認 |
| F1 | paste / new-folder 後の実生成項目選択 | `post_operation_selection.rs`, `snapshot_ops.rs` | レビュー完了 | R-08 / R-09 を確認 |
| S1 | Susie crash recovery / exhausted workers notice | `susie_loader.rs`, `ui_dialogs/susie_worker_notice.rs` | レビュー完了 | R-10（既知 quarantine key）を確認。pool lifecycle は追加指摘なし |
| W1 | 最大化状態の復元・起動経路 | `startup_ops.rs`, `settings.rs`, vendored eframe | レビュー完了 | 追加指摘なし |
| M1 | content restore / archive / PDF / ZIP / tray / Remote の修正 | 関連差分一式 | レビュー完了 | R-05（content ledger race）を確認 |
| X1 | keyboard / mouse / touch / gamepad / customization 横断 | `keymap.rs`, `touch_input.rs`, `gamepad_input.rs`, native input | レビュー完了 | 新規 8 KeyAction の配線は完備。R-02 / R-04 / R-25 を確認 |
| X2 | UI thread / cancellation / allocation 横断 | UI entry points と worker boundaries | レビュー完了 | R-06 / R-07 / R-11 / R-12 / R-14 / R-16 / R-20 / R-21 / R-23 を確認 |

## 2.5 対応状況 (2026-08-29 時点、ClaudeCode)

台帳の指摘に対する実装の進捗。**この節が残作業の正本。**

| ID | 重要度 | 状態 | commit / 判断 |
| --- | --- | --- | --- |
| R-01 | P1 | ✅ 修正 | `eb275c2a` abort identity を後続要求から隔離 |
| — | P1 | ✅ 修正 | `21b05da2` abort が失敗で終わっても後継を解放 (台帳に無い同型。R-01 修正の取りこぼし) |
| R-02 | P1 | ✅ 修正・実機確認済み | `4a19ed0a` + `9187cb05` (退行 2 件を実機で発見し修正) |
| R-03 | P1 | ✅ 修正 | `b1498ea5` 左右切替で旧ページの未確定操作を持ち越さない |
| R-04 | P1 | ✅ 修正 | `d6f4ab6c` 分割モードが読み方向を決める |
| R-05 | P1 | ✅ 修正 | `a61869da` CAS 期待値を probe 前に取り、clear 後に再 probe して行があれば戻す |
| R-06 | P1 | ✅ 修正 | `3e756ce0` ホイール中はメモリのみ、閉じるときに 1 回だけ書く (save 世代で判定) |
| R-07 | P1 | ❌ **未着手** | 大型 mask の圧縮・DB 保存を UI スレッドで |
| R-14 | P1 | ❌ **未着手** | 巨大 document の UI スレッド deep clone |
| R-15 | P1 | 🚫 **取り下げ** | 反証。effect は FIFO で `RetireOutgoing` が `AbortNative` より先に積まれるため分裂しない。観測されるのは R-17 のハング |
| R-17 | P1 | ✅ 修正 | `64847e3c` (suffix 所有権) + `ce663f82` (裁定 + publication 状態) + `0a1e2aeb` (frame queue) |
| R-20 | P1 | ✅ 修正 | `51b882ad` 復号済みセルに 128 MiB 上限。窓は pin、遠い順に超過分だけ落とす |
| R-24 | P2 | ✅ 修正 | `7a198b07` bundle 破棄前に terminal effects を完遂 |
| R-26 | P1 | ✅ 修正 | `709397b1` 保存失敗を成功として公開しない。**後半 (import の世代比較) は不要と判断** — import の「中央が authoritative」はフォルダ移動からの復旧という目的に対して正しく、sidecar が先行する唯一の道が前半だった |
| R-08/09/10/11/12/13/16/18/19/21/22/23/25 | P2/P3 | ❌ **未着手** | R-19 (360 roll) は実在を確認済み。R-25 は v3.2.0 にも同一コードがあり既存 |

**Codex が追加で見つけ、台帳に無かったもの** (すべて対応済み):

- `Committing::AwaitingRetire` への直接 `TerminalClose` → `02390035`
- 外側 drain の取り残し (3 例目のハング) / frame 延期の追い越し → `0a1e2aeb`
- parked bundle の retire が effect を drain しない → **調査のうえ benign と裁定**。
  `docs/detached-rework-plan.md` §11 に理由と「やってはいけない 2 つ」を記録

**R-06 / R-05 / R-20 の修正に対する Codex レビュー** (2026-08-29):

- **R-05 の修正が不完全** → `30b0ed8a`。編集は store の行を先に commit し、台帳の記録要求は
  channel 経由で worker が後から適用する。行が現れた瞬間の `last_edit_at` はまだ古いので、
  期待値をいつ読んでも CAS は成立する。**私のテストは本番の順序を再現しておらず**、
  台帳を同期更新する自作の interleaving に対して「CAS が止めた」と主張していた。
  UI スレッドで送信前に上げる `RECORD_SEQUENCE` を追加し、観測中に動いたらその回の掃除を
  見送る形に変更。再 probe は `record` を通らない書き手 (サイドカー取り込み等) 用に残す
- **ログ文言 / 設計文書の追随** → 同 commit。「edited while probing」は観測していないことを
  主張していた (store が読めなかった回も同じ経路)。`docs/video-architecture.md` の
  「即時保存する」も更新
- **環境設定ダイアログの stale snapshot** (Medium) → **v3.2.0 以前からある挙動、結果は同一**。
  ダイアログは開いた時点の snapshot を持ち、OK で live へ戻す。2 つのレンジ設定は
  `overwrite_non_preferences_from` の対象外なので、開いている間にホイールで変えた値は
  OK で上書きされる。**修正前も同じ結果** (ホイールが保存 → OK が上書き保存) なので
  R-06 が作った欠陥ではない。世代の doc comment が言い過ぎていた点だけ訂正した。
  一般解は「ダイアログが触っていない項目だけ live を採る」三方向マージで、
  この 2 項目に限らない別件
- **トレイ退避は queued wheel event に対して閉じていない** (Low) → 未対応。トレイ保存が
  native video event の drain より先なので、その間に届いたホイールは hidden session に
  残る。通常終了では保存される。電源断のみ 1 項目を失う
- **`--play-test` は `on_exit` を通らない** (Low) → 未対応。開発用フラグ
- **`backed` だった key は再 probe しない / store が読めない回は候補を残す** (Low) →
  意図した fail-open。編集を失うより dead candidate を 1 回出す方を選ぶ

**分類の訂正**: R-02 の「既存残存」は述語だけを見た分類。述語は v3.2.0 と同一だが、
それが壊れる状態 (`active_detached_context_is_at_rest`) は v3.3.0 の新規。

**凍結ルール**: R-01 / R-17 / R-24 / R-02 は detached リワークの対象。着手前にプラン §2 を
読み、Codex と「症状パッチではなく構造的修正である」ことの合意を取り、
`docs/detached-rework-plan.md` §11 へ記録済み。

## 3. 指摘一覧

以下は固定 target で静的に成立し、独立再検証でも Keep と判定した指摘。severity、同型経路、
必要 test、手動 smoke まで確定済み。

| ID | 重要度 | 機能 | 状態 | 要約 |
| --- | --- | --- | --- | --- |
| R-01 | P1 | F12 / native handoff | 確認済み | abort 中の連打が in-flight request identity を上書きし、transition を永久待機にできる |
| R-02 | P1 | detached / gamepad | 確認済み（既存残存） | at-rest detached viewer が foreground でも gamepad dispatch が main grid へ落ちる |
| R-03 | P1 | 左右ページ編集 | 確認済み | 左右切替で lasso / shape selection 等が残り、別ページへ編集を誤適用できる |
| R-04 | P1 | Remote 見開き分割 | 確認済み | SplitLtr / SplitRtl と reading direction を独立保持し、物理ページ順と操作方向が逆転する |
| R-05 | P1 | content restore | 確認済み | empty-origin cleanup が並行した新編集の ledger flag を消し、復元候補も落とせる |
| R-06 | P1 | シークストリップ | 確認済み | wheel 1 notch ごとに全設定を同期 DB 保存し、通常操作で UI thread を停止させる |
| R-07 | P1 | ローカル補正永続化 | 確認済み | 大型 raster mask の量子化・deflate・JSON・SQLite を UI thread で実行する |
| R-08 | P2 | paste 後選択 | 確認済み | Shell paste の即時失敗後も request が 10 秒残り、無関係な追加項目を選択する |
| R-09 | P2 | paste 後選択 | 確認済み | 複数ファイルの段階的到着中、最初の unchanged refresh で request を早期破棄する |
| R-10 | P3 | Susie 復旧 | 確認済み（既知残存） | archive entry quarantine key が entry 名と長さだけで、別 archive と衝突する |
| R-11 | P2 | tray / sidecar | 確認済み | tray 退避が全 sidecar 書き込み完了を UI thread で待つ |
| R-12 | P2 | detached activate | 確認済み（既存残存） | parked descriptor fallback が UI thread で `parent().is_dir()` を実行する |
| R-13 | P3 | 操作説明 | 確認済み | 設定画面・設計文書のページ構成キー説明が新規 8 / 9 と一致しない |
| R-14 | P1 | sidecar writer | 確認済み | 非同期 enqueue 前後に巨大 local-adjust document を複数回 deep clone する |
| R-15 | P1 | F12 / native handoff | 確認済み | commit 後 retire 前の置換 abort が App と native presentation/generation を分裂させる |
| R-16 | P3 | sidecar import | 確認済み（既知残存） | 初回・外部変更・旧形式移行時の巨大 sidecar read/parse/import が UI thread 上に残る |
| R-17 | P1 | native command queue | 確認済み | transition 待機が drain 済み lossless batch の後半 command を破棄する |
| R-18 | P2 | Remote / ZIP split | 確認済み | catalog 未生成 ZIP の寸法 probe が有効 JPEG を固定 64 KiB で打ち切る |
| R-19 | P2 | 360 度動画 | 確認済み | AVSphericalMapping の roll を parse するが描画 pipeline へ渡さない |
| R-20 | P1 | シークサムネイル | 確認済み | decoded cell cache が無制限で、UI poll が全履歴 BTreeMap を clone する |
| R-21 | P2 | シーク波形 | 確認済み | 波形→サムネイル切替後も不可視の coarse 全尺解析が継続する |
| R-22 | P3 | シーク波形 | 確認済み | 可変 bin が foreground request の preemption 単位を 60 秒から最大 1,800 秒へ伸ばす |
| R-23 | P2 | Remote / 見開き | 確認済み | catalog 未生成の非 Single 応答が最大 10 万画像を直列 header probe する |
| R-24 | P2 | detached close | 確認済み | active context の transition 中に全別窓を閉じると terminal effects を実行せず bundle を retire する |
| R-25 | P2 | 操作カスタマイズ | 確認済み（既存残存） | 補正レイヤーの図形削除だけ raw Delete を消費し、再割当・競合検出を迂回する |
| R-26 | P1 | 編集永続化 | 確認済み | 中央 DB 保存失敗後も mirror を成功公開し、次回 import が新 sidecar を拒否する |

### R-01: abort 中の F12 連打で transition identity が失われる

- 破れた不変条件: native worker が処理中の request は、その request の terminal event
  (`PlacementAborted` / `NativeFailed`) を reducer が受理するまで単一 ownership で保持する。
- 再現条件: native placement 待機中に F12、abort 完了前にもう一度 F12。3 回目の入力が
  `Aborting` の successor を「native 発行済み」と誤認し、`aborted_request_id` を上書きする。
- 根本原因: `presentation_transition.rs:365-405` の `native_prepare_was_issued` が
  `PreparingProgress::Aborting` を successor の発行済み状態として扱う一方、worker は元の
  request の abort だけを完了する。failure event も元 request id なら同様に捨てられる。
- 修正方向: in-flight abort identity と「最新 successor intent」を分離し、元 request の全 terminal
  event まで retarget / 再発行しない。AwaitingNative / Ready / Committing / terminal failure から
  3 回以上連打する reducer + worker boundary test が必要。

### R-02: at-rest detached viewer の gamepad ownership が main へ落ちる

- 破れた不変条件: keyboard / mouse / touch / gamepad は同じ foreground viewer context を操作する。
- 再現条件: always-new / book の独立 detached viewer を active にし、at-rest bundle のまま
  gamepad の page navigation や stick を入力する。
- 根本原因: `app.rs:66666` の gamepad dispatch は `app.rs:66684` の active context mount より前。
  `gamepad_input.rs:524-554` は root projection の `fullscreen_idx` を availability とし、false なら
  `detached_window_manager.rs:901-913` が MainWindow を返す。
- 修正方向: device sampling と context-scoped dispatch を分離し、foreground / bound
  `ViewerContextId` を mount した状態で route する。predicate の緩和だけでは不足。

### R-15: retire 中の rollback が App と native の presentation を分裂させる

- 破れた不変条件: App の `viewer_presentation` / committed generation、transition reducer、native
  presenter/placement は commit/rollback を同じ原子的境界で反映する。
- 再現条件: `NativeCommitted` で target1 を App へ publish した後、`NativeRetired` より前に F12 を
  2 回目入力して request を置換する。native の retire-phase Abort は旧 presentation / generation へ
  rollback するが、reducer は `Stable(current=old)` にするだけで逆向き `ApplyPresentation` を出さない。
- 根本原因: `presentation_transition.rs:537-572,619-635` は commit 時点で Apply effect を出す一方、
  `:207-213,267-295,666-706` の replacement/abort state は published presentation と commit phase を
  保持しない。App は `native_video.rs:1624-1632` で target1 と generation を単調 publish 済み。
- 影響: native は旧側、App の host routing は新側となる。さらに旧 native generation は App の
  committed generation より小さくなり、`native_video.rs:3625-3657,4483-4488,4804-4809` が
  × / Esc の close event を stale として拒否できる。
- 修正方向: Aborting が prepare/committed/retiring phase と published value を型で所有し、rollback
  effect まで一つの transaction にするか、App publish を retire terminal へ移す。AwaitingRetire
  replacement と close-after-rollback の handler test が必要。

### R-17: transition 待機が lossless command batch の suffix を失う

- 破れた不変条件: command receiver が lossless queue から drain した command は、明示的に処理するか
  ownership を queue/actor state へ返す。batch の途中 return で未処理 suffix を捨てない。
- 再現条件: `NativeCommitted` 後に `RetirePlacement(req1)`、直後の F12 置換で
  `AbortPlacement(req1)` を送り、worker が同じ batch `[Retire, Abort]` として drain する。
- 根本原因: `video/mod.rs:1664-1673` の `NativeCommandReceiver::drain` は全件を queue から外すが、
  `:1693-1717` の `wait_for_placement_transition_control` は最初の一致 control で即 return し、
  Vec の未反復 suffix を保存しない。reducer は Aborting なので `NativeRetired` を無視し、消えた
  `NativeAborted` を永久に待つ。suffix の seek / pause / overlay 等も一回限りなら失われる。
- 修正方向: 1 command 単位の受信、または順序付き pending suffix の actor ownership を導入し、
  Retire 対 Abort を typed terminal outcome として解決する。通常 loop へ戻すだけでは、そこが
  placement control を無視するため不十分。

### R-03: 左右ページ切替で page-scoped edit transient が漏れる

- 破れた不変条件: 編集対象 idx を切り替える前に、旧ページの未確定 gesture / selection /
  picker / preview を commit または cancel し、新ページへ持ち越さない。
- 再現条件: 左ページで polygon lasso の途中、または shape を選択したまま補正パネルの左右を切替し、
  release / delete / nudge を行う。新ページの同じ数値 index の shape や mask を編集できる。
- 根本原因: `ui_adjustment_panel.rs:13321-13325` は idx を切り替えて single-view に入るだけで、
  `app.rs:11678` 等の flat transient fields を清算しない。text / erase は専用 switch boundary を
  持つが local-adjust は持たない。
- 修正方向: typed `switch_local_adjust_target_in_spread` に全 entry point を集約し、旧 idx の
  commit/cancel と transient clear を一つの state transition にする。

### R-04: Remote の split mode と reading direction が不整合になる

- 破れた不変条件: `SplitLtr` は LTR、`SplitRtl` は RTL を canonical direction として返し、保存する。
- 再現条件: default SplitRtl + default direction Ltr（または API で矛盾した組合せ）で Remote を開く。
  server の page slice は右→左なのに、web の tap / arrow / swipe / seek は LTR と解釈する。
- 根本原因: `remote_ipc/container.rs:6505-6514` と `remote_ipc/ui.rs:3722-3738` の canonicalization
  が通常 Rtl / Ltr だけを列挙し、split variants を落としている。
- 修正方向: mode→direction を単一の exhaustive owner へ集約し、resolve / persist の双方で使う。

### R-05: empty-origin cleanup が並行した新編集を消す

- 破れた不変条件: store probe 開始後に成立した新しい編集は、古い「空」という観測で ledger と
  restore candidate から除外しない。
- 再現条件: `content_identity.rs:1384-1406` が store 群を probe した直後、UI edit が store row を
  書き ledger を新 timestamp / flag=1 にする。その後 cleanup が ledger を初めて読み、新 timestamp
  自身を CAS の期待値にして flag=0 を成功させる。
- 根本原因: CAS snapshot を store probe **後**に取得しており、コメントで意図した競合防止の
  時系列が逆。candidate filtering も stale な `backed` 集合を使う。
- 修正方向: wall-clock timestamp ではなく monotonic revision / transaction token を probe 前に取得し、
  clear 後にも全 store を再 probe して row があれば re-mark / candidate retain する。異なる SQLite
  store 間の競合 test が必要。

### R-06 / R-07 / R-11 / R-12 / R-14 / R-16: UI thread blocking

- R-06: native strip の MouseWheel は各 notch を `StepSeekStripRange` として送り、
  `app/native_video.rs:7504-7549` が毎回 `settings.save()`。`settings.rs:8007-8071` は全 Settings
  clone / JSON / SQLite transaction、初回は backup rotation / VACUUM も同期実行する。操作中は
  memory state だけ即時更新し、idle/end で最新値を一度保存する必要がある。
- R-07: `undo_ops.rs:473-480` → `app.rs:54029-54042` → `local_adjust_db.rs:103-117` が
  raster mask の q8 化、deflate、base64/JSON、SQLite execute を UI thread で行う。v3.2 からの
  同期保存経路を圧縮形式変更後も残した residual であり、generation/CAS 付き save worker へ
  ownership を移す必要がある。
- R-11: `tray_integration.rs:318` → `app.rs:53823-53825` が process 継続中の tray hide で
  global sidecar writer の Condvar を待つ。真の process exit flush と hide enqueue を分離する。
- R-12: `app.rs:40470-40485` の parked fallback は activate の UI thread で
  `path.parent().is_dir()` を行う。v3.2 にもある残存リスク。parked snapshot を async
  build/commit/abort の結果まで保持する。
- R-14: `sidecar.rs:519-534` の `queue_flush` は UI thread で `self.items.clone()` を行い、
  `SidecarEntry.local_adjust_layers` の全 raster `Vec<f32>` / `Vec<u32>` を deep clone する。
  codec の document budget は保持データだけで最大 1 GiB を許すため、folder switch、編集終了、
  10 分周期、tray/exit の enqueue 自体が数百 MB copy / OOM になり得る。worker 側も
  `sidecar.rs:665-669,690-694,742-747` で snapshot を再度 deep clone する。pending と serialize が
  同じ immutable `Arc` snapshot を共有する COW / move ownership に変え、UI は bounded な
  pointer handoff のみにする。既存 dry-run は時間を表示するだけで上限 assertion がない。
- R-16: `app.rs:59429-59520` の sidecar slow path は `sidecar.rs:198-250` の
  `read_to_string`、全 JSON parse、base64/deflate または legacy 数値配列 decode、その後の DB import
  を folder switch / 初回 edit の UI thread で実行する。733.9 MB 旧形式で 0.8 秒という実測が
  `ui-responsiveness.md` に既知 P3 として記録されているが、v3.3 の形式移行対象に直接当たるため
  先行記録どおり P3 の既知 residual とする。folder generation / CAS を持つ background parse/import が必要。

### R-20: decoded thumbnail cache と UI snapshot が過去全窓に比例する

- `seek_strip_thumbs.rs:830` の `SharedState.cells` は ready / failed を `:1711,1717` で追加するだけで、
  worker lifetime 中の byte/count eviction がない。keyframe axis の adopted list に総セル上限はなく、
  0.1 秒間隔・320×320 RGBA の最悪例では数時間の全域閲覧だけで数十 GiB に達し得る。
- `seek_strip_thumbs.rs:860-869` は mutex 中に cells 全体を clone し、
  `app/native_video.rs:8047-8106` の UI sync が未着中 80 ms、再生中 100 ms 間隔で呼ぶ。画素は Arc
  でも全 map node allocation / Arc refcount / lock hold は累積セル数に比例する。
- visible/lookahead を pin する byte-budget LRU（永続 WebP cache は再利用）と failed count budget、
  window query または revision/delta snapshot が必要。数千窓後も working set / poll cost が一定の
  stress test を追加する。

### R-21 / R-22: waveform background ownership と preemption

- R-21: `app/native_video.rs:7555-7626` は mode 切替先 worker を必要なら作るだけで、切替元
  `wave_worker` を pause / take / cancel しない。10 分以上の span で始まった coarse build は
  `seek_strip_wave.rs:1139-1167,1461-1510` で worker-wide cancel まで全尺を解析し続け、不可視でも
  再生と thumbnail decode の I/O/CPU を奪う。mode が activity owner となる pause/resume または
  cancel/recreate transition が必要。
- R-22: 設計上 background chunk は 60 秒だが、可変 bin は 600 bins/chunk のままなので
  `seek_strip_wave.rs:473-558` で最大 1,800 秒へ伸びる。foreground request は latest id を更新しても、
  実行中 coarse decode/analyze の closure (`:1367-1424`) は worker-wide cancel しか見ず、chunk 完了まで
  preempt できない。chunk の時間量を約 60 秒に固定し、request generation 変化を `Preempted` として
  coverage/failed へ記録せず再queueする。

### R-08 / R-09: post-operation selection request の lifecycle が不完全

- R-08: `app.rs:36614-36624` と `ui_dialogs/context_menu.rs:1607-1615` は paste request を
  Shell invoke より先に arm し、即時 Err で clear しない。10 秒以内の無関係な filesystem 追加を
  paste 結果として扱う。成功後に commit するか、失敗時に typed request を cancel する。
- R-09: `post_operation_selection.rs:126-130` は最初の tranche を Apply 後、同じ集合の refresh で
  request を Drop する。遅れて到着する残りのコピーを失うので、明示的な quiet/completion policy
  まで request を保持し、同じ集合は再適用しない state が必要。

### R-10 / R-13: その他

- R-10: `susie_loader.rs:864-867` の quarantine key は `filename_hint#bytes.len()`。
  ZIP caller は entry 名だけを渡すため、別 archive の同名・同サイズ entry が一方の plugin crash で
  session 中 quarantine される。outer container identity + entry、または stable content fingerprint
  を key に含める。`next-release-backlog.md` §1.141 で既に出荷前に直さない P3 と裁定済みであり、
  本レビューも同じ重要度を維持する。
- R-13: `ui_dialogs/preferences/pages.rs:7515` は数字 1–5 と説明し、`docs/spec.md:1378` は
  0–7、`docs/page-split-plan.md:181` は default key なしと記載するが、新規 split 操作は 8 / 9。
- R-18: `remote_ipc/container.rs:1993-2048` の catalog 無し ZIP 寸法 fallback は、
  `zip_loader.rs:760-773` で各 entry を固定 64 KiB prefix に切って `into_dimensions` を呼ぶ。
  JPEG は SOF より前の APPn が 1 segment だけでも 65,535 byte、複数 ICC/Exif segment も合法なため、
  SOF が境界より後の有効 JPEG を永久に寸法不明扱いし、Remote の横長単独表示 / split を適用しない。
  `ZipFile` reader を header parser へ直接渡して必要量だけ読む（総量上限付き）か、段階的 prefix retry
  にする。catalog 未生成の ZIP 直開き test に late-SOF JPEG fixture が必要。
- R-19: `video/spherical_metadata.rs:124-175` は FFmpeg `AVSphericalMapping` の
  yaw / pitch / roll を読むが、`app.rs:63018-63024` は yaw / pitch だけを viewer pose へ写し、
  `app/native_video.rs:6611-6623`、`panorama.rs:240-244`、
  `video/native_presenter/panorama_pipeline.rs:271-290` の command / pose / uniform に roll または
  source mapping rotation がない。nonzero roll の正規な 360 動画は傾いたままで、ユーザー操作でも
  補正不能。FFmpeg の yaw→pitch→roll 規約を source mapping transform として viewer pose から分離し、
  shader と nonzero-roll orientation test へ配線する。

### R-23: Remote の catalog-free 寸法判定が全画像を直列 probe する

- 破れた不変条件: Remote の response build は、コレクション上限までの件数に対して無制限な
  filesystem probe を直列実行せず、cancel / progress / bounded work を持つ。
- 再現条件: catalog 行が作られない `Auto` 方針の画像を多数含む folder、または直接 Image を含む
  rating / smart-folder 等を Remote から見開きまたは split で開く。collection は最大 100,000 entries
  を許す。既定 Single の favorite/search listing 自体はこの probe を通らない。
- 根本原因: `remote_ipc/container.rs:5107-5175` は catalog miss ごとに
  `page_dims_without_catalog` を呼び、`collections.rs:830-909` は親 folder ごとの catalog open に加えて
  各 miss を `ImageReader::open` / format guess / header read する。結果生成は完了まで返らず、途中 cancel
  も nearby-page lazy policy もない。これは回帰テスト負荷ではなく通常の Remote 応答経路の負荷。
- 修正方向: 寸法を永続 index に非同期補完し、初回応答は近傍 / bounded batch だけで生成する。
  request generation/cancel を持たせ、10 万件でも初期応答 latency と I/O 上限が一定の test が必要。

### R-24: active context の terminal close effects を retire 前に失う

- 破れた不変条件: context を close-and-retire する境界は、mounted owner が生成した terminal effects と
  `close_fullscreen_now` の resume 保存 / pending cancel / native teardown を完了してから bundle を破棄する。
- 再現条件: always-new / book の独立 detached 動画が F11/F12 placement transition 中に、main 側の
  UI 倍率・font・別窓 mode 変更から「全 detached close」を実行する。
- 根本原因: `app.rs:37881-37886` が最初に処理する transition は root projection の owner だけ。
  AtRest active context は `:37910-37912` から mount され、closure 内の `close_fullscreen()` (`:51382-51391`)
  が `TerminalClose` を dispatch して即 return する。その closure は effects を drain せず、
  `viewer_context_registry.rs:3144-3154` が直後に context を retire/drop するため、
  `close_fullscreen_now` の `save_all_video_resume_positions` などへ到達しない。
- 修正方向: close-and-retire seam が mounted owner の terminal effects を同期的に完遂して通常 teardown
  plan へ到達するか、同等の保存・cancel・host cleanup を所有する typed media close plan を実行する。
  AtRest transition の全 phase × mode-change close test が必要。

### R-25: 補正図形の Delete が KeyAction を迂回する

- 破れた不変条件: 通常の keyboard operation は `KeyAction` を唯一の binding owner とし、help、default、
  conflict detection、consumer が同じ action を参照する。
- 根本原因: `ui_fullscreen.rs:19130-19131` だけが `Modifiers::NONE + Delete` を raw consume する一方、
  `keymap.rs:1760-1810` の Local Adjust actions に delete action がない。対称な消しゴム / conceal の
  shape delete は `EraseDeleteShape` / `ConcealDeleteShape` を経由し、固定操作として文書化もされていない。
- 影響: 削除を再割当できず、Delete を別 Local Adjust action へ割り当てても conflict detection が
  この consumer を認識せず、入力順により二重発火または意図しない別操作になる。
- 修正方向: `LaDeleteShape` を追加し、all/name/description/context/default/help/conflict/consumer と parity
  test へ配線する。

### R-26: 編集 DB 保存失敗後も mirror を成功状態として公開する

- 破れた不変条件: durable な正本 DB への commit に失敗した edit を、memory / presence / sidecar /
  content ledger へ「保存済み」と publish しない。複数 store の世代が分裂した場合は明示的に
  retry/reconcile する。
- 根本原因: local-adjust (`app.rs:54039-54062`) だけでなく erase mask save/delete
  (`:53894-53932`)、conceal (`:53983-54022`)、export crop (`:54152-54185`)、comic
  (`:58529-58564`)、adjustment (`:60555-60612`) も central DB の `Result` を無視または caller で捨て、
  mirror を先に publish する。`sidecar.rs:892-975` の import は全 sibling store で中央 DB に行が
  1 件でもあれば内容・世代を比較せず skip する。background repack/VACUUM、lock、readonly、disk-full
  等が失敗条件になる。
- 影響: 既存中央行の更新だけ失敗し sidecar が成功すると、再起動後は古い中央行が authoritative とされ、
  新 sidecar は取り込まれず同期済み扱いになる。通常 load (`app.rs:26443-26447`) も中央 DB を読むため、
  sidecar に bytes は残っても通常 UI から新編集が消える。sidecar 無効時は復旧元自体もない。
- 修正方向: 全 edit store が共有する typed commit generation を設け、DB durable success を publication
  boundary にする。失敗時は memory dirty と retry/error を保持する。sidecar を復旧元にするなら DB と
  sidecar の双方へ monotonic revision を持たせ、新しい側だけを採用する reconciliation が必要。
  既存行 skip の単純解除は古い外部 sidecar で新 DB を上書きするため不可。各 sibling route について
  failure injection → sidecar flush → restart/import の統合 test が必要。

## 4. 検証台帳

| 検証 | 状態 | 結果 |
| --- | --- | --- |
| 差分・commit 分類 | 完了 | 主要 13 機能群へ分類 |
| 設計文書確認 | 完了 | architecture / async / responsiveness / keymap と対象機能の設計・backlog を確認 |
| 静的 code review | 完了 | 機能別 3 レーン + 横断 review 後、全 finding を独立再検証。旧 R-23 候補 1 件を false positive として除外 |
| focused tests | 完了 | Rust: `presentation_transition` 11、`post_operation_selection` 11、`keymap::tests` 131、`page_split` 15、`sidecar::tests` 31、`viewer_context_audit` 32、`mask_codec` 11、`local_adjust_db` 10、`content_identity` 35、`susie_loader` 13、`seek_strip` 117（2 ignored）、`panorama` 74、Remote container 76 / UI 27、gamepad 39、touch 158、maximized 7、startup 61、sidecar import integration 12 が成功。Remote Web Node 228 も成功。ただし本台帳の競合 sequence / lifetime stress / late-SOF / roll は既存 test に無い |
| detached mode-change 単独 test | 完了 | 既存の active/passive/ParkedLive 一括 close 1 件が成功。ただし AtRest owner の presentation transition 中 close は fixture に含まれず R-24 を反証しない |
| viewer context 静的 audit | 完了 | `cargo run -p viewer_context_audit --quiet` 成功（指摘なし） |
| `cargo check` / format / glyph | 完了 | core `cargo check`、`cargo fmt --all -- --check`、UI glyph check、target diff/current `git diff --check` が成功（既存 warning のみ） |
| verification binary | 対象外 | 本ターンは read-only review。修正が必要なら修正後の別工程で作成する |

## 5. 結論と出荷判定

固定対象 `8a80c336` のまま v3.3.0 を出荷することは推奨しない。P0 はないが、P1 には native
transition の永久待機、別ページへの誤編集、編集 store の durable generation 分裂、通常操作の
UI block、巨大 document clone/OOM、サムネイル cache の無制限増加が含まれる。focused test が全て
成功しても、現行 test がこれらの競合 sequence、失敗注入、長時間 lifetime を覆っていないため判定は変わらない。

| 重要度 | 件数 | 出荷前の扱い |
| --- | ---: | --- |
| P0 | 0 | なし |
| P1 | 12 | 修正、同型経路監査、回帰 test、再レビューを必須とする |
| P2 | 10 | 原則修正。延期する場合は個別に利用者判断と既知制限を記録する |
| P3 | 4 | 文書不一致・極端長尺・既知 residual。R-10 / R-16 は先行文書の P3 裁定を維持した（技術的には P2 candidate との再評価意見もあった） |

本レビューで source code は変更していない。`docs/next-release-backlog.md` §1.132 の先行レビューを
手掛かりにはしたが、現行 target の正しさの代用にはしていない。途中で挙がった「AtRest context の
transition が poll されない」という候補は、mounted context から毎 tick `poll_video()` が呼ばれるため
false positive と確認し、指摘一覧から除外した。

## 6. 根本修正のまとまり

個別 guard を追加するより、次の ownership boundary ごとに直す方が同型退行を防げる。

1. **presentation transaction**: R-01 / R-15 / R-17 / R-24。in-flight request、published
   presentation/generation、未処理 command suffix、terminal close を一つの actor/transaction が所有する。
2. **edit durable generation**: R-03 / R-05 / R-07 / R-11 / R-14 / R-26。page transient の切替、
   central DB commit、sidecar mirror、ledger、background serialization を monotonic generation で結ぶ。
3. **seek-strip activity / resource budget**: R-06 / R-20 / R-21 / R-22。mode が worker activity を所有し、
   decoded cache・failed set・UI snapshot・preemption span に固定予算を持たせ、設定保存を coalesce する。
4. **Remote spread metadata**: R-04 / R-13 / R-18 / R-23。mode→direction の正本を共通化し、寸法は
   bounded/cancellable な永続 index から供給する。
5. **foreground input context**: R-02 / R-25。device sampling と context dispatch を分離し、通常の
   keyboard operation は `KeyAction` を唯一の binding owner にする。
6. **request lifecycle / media metadata**: R-08 / R-09 / R-10 / R-12 / R-16 / R-19。completion/error、
   outer identity、async filesystem probe、source spherical transform をそれぞれ型付き owner に置く。

## 7. 監査済みで追加指摘のなかった境界

- page split core の source identity、左右 slice、回転後の横長判定、free rotation 時の無効化、group/step。
- 360 の 4 投影写像、display rotation、partial FOV、pointer/touch ownership、GPU texture lifecycle
  （R-19 の source roll だけ例外）。
- seek worker の fullscreen close / source swap cancellation、nonblocking command send、overlay texture prune、
  decode/SQLite/WebP の worker 配置（R-06/R-20/R-21/R-22 の例外を除く）。
- `ContextTable` の mount/build/fork/retire/promote、binding 逆写像、abort/panic 復元、兄弟 context の
  channel/cancel/cache 非干渉（R-02/R-12/R-24 の seam を除く）。
- v3.3 の新規 8 `KeyAction` は enum/all/name/description/context/default/help/parity/consumer まで配線済み。
  split 8/9、panorama Shift+V、seek cycle Shift+S と未割当 4 direct action を確認した。
- keyboard/mouse/touch/gamepad の reading-direction helper、native strip/panorama の touch ownership、
  固定 Esc/矢印の keymap 外扱い（R-02/R-04/R-25 の例外を除く）。
- mask codec の q8z/u32z、legacy 互換、単一 mask 上限、1 GiB document budget、parse 入口の予算経由。
- startup/maximized の creation-time 適用、`--window-size` 優先、normal rect と maximized flag の分離。
- Susie pool の crash job 再送禁止、slot respawn/backoff、全滅 drain/notice。
- Remote の path traversal/network-path guard、group identity、slice layout/prefetch、collapsed ZIP key。

## 8. 修正後の必須 smoke matrix

| 領域 | シナリオ | 確認する不変条件 |
| --- | --- | --- |
| F11/F12 | native transition の全 phase で 3 回以上連打、failure、×/Esc、同一 batch の Retire→Abort | 固着せず、App/native の presentation と generation が一致し、close できる |
| detached close | AtRest 動画の transition 中に UI倍率/font/別窓 mode を変更 | terminal effects、resume 保存、pending cancel、host teardown が完了してから retire |
| input | main / active detached / ParkedLive を前面にし、物理 gamepad と keyboard/mouse/touch を使用 | 全 device が同じ foreground context を操作する |
| 補正見開き | 左で polygon/shape select/drag 後、mouse/touch で右へ切替し Enter/Delete/矢印/Undo | 旧 page transient が新 page に適用されない。Delete 再割当と conflict 表示も一致 |
| 編集保存 | DB busy/readonly/disk-full を各 edit store に注入し、sidecar flush 後に再起動 | dirty/retry/error が残り、中央 DB・sidecar・ledger の最新 generation が一致する |
| content identity | detector の store probe 中に新規 edit、その後同内容 file を移動/複製 | 新 edit の flag と restore candidate が残る |
| thumbnail strip | 長い all-intra 動画を全域 scrub、再生/seek/zoom、cold settings DB で wheel | memory/poll cost が一定予算内、UI hitch なし、表示/seek が正しい |
| waveform | coarse build 中に thumbnail/none へ切替、超長尺で前景 request | 不可視 worker が資源を使わず、foreground が短い上限で preempt する |
| Remote | SplitLtr/Rtl と矛盾 direction、catalog 無し大量 folder/rating、late-SOF JPEG ZIP | page 順・tap/swipe/seek が一致し、初期応答と I/O が bounded、寸法が取れる |
| panorama | nonzero yaw/pitch/roll、partial FOV、display rotation、各 projection | source transform と viewer pose を分離し、正立して操作できる |
| paste/tray | Shell 即時失敗・slow multi-file・同時外部追加、巨大 sidecar 中 hide→restore | selection request が正しく完了/取消され、tray restore UI を writer が止めない |

native D3D11 / HWND / multi-monitor / DPI / physical touch/gamepad は unit test だけでは完結しない。
本ターンは read-only review なので通常 profile/portable smoke binary は起動・作成しておらず、上表は
P1/P2 修正後に automated regression と実機確認の双方で実施する。
