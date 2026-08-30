# v3.3.0 出荷前 機能別・横断レビュー台帳

作成: 2026-08-29
比較基準: `v3.2.0` (`09758b90`) .. `master` (`8a80c336`)
差分規模: 369 commits / 210 files / +68,897 -9,909

レビュー開始後に別レビューの変更が `master` へ入ったため、静的レビュー対象は上記
`8a80c336` に固定する。2026-08-29 時点の作業ツリー HEAD は `97bd8a02`。後続の
`215464e2`（回帰テスト負荷対応）およびレビュー記録 commit は本レビューの評価対象外とし、
後段の build / test は「対象コードと同等だが HEAD は異なる」補助検証として記録する。

> **最新判定:** 実際に公開した `v3.3.0` タグ (`0d141615`) に対する P1 再レビューは §9。
> §1〜§8 は修正前 target の初回レビュー記録として残す。

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
| R-07 | P1 | ⏸ **今回は見送り (計測して判断)** | 下の「R-07 の判断」参照 |
| R-14 | P1 | ✅ 修正 | `df245720` items を `Arc` 共有 + `make_mut` の COW にし、積む側の複製を無くした |
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

### R-07 の判断: 今回は見送る (2026-08-29)

**実測** (release、`encode_alpha` = q8 量子化 + deflate + base64):

| マスク | v3.2.0 の形式 (素の JSON 数値配列) | v3.3.0 の形式 (`q8z:`) |
| --- | --- | --- |
| 0.8 MP | 11.9 ms / 8 MB | 2.4 ms / 14 KiB |
| 3 MP | 49.4 ms / 28 MB | 8.1 ms / 40 KiB |
| 12 MP | 195 ms / 115 MB | 33 ms / 150 KiB |
| 24 MP | 410 ms / 230 MB | **70.6 ms** / 375 KiB |

マスクは画像原寸 (`local_adjust_image_dims`) なので、24MP の写真ならストローク確定 1 回に
つき 70 ms が UI スレッドに乗る。**ただしこれは v3.2.0 以前からある同期経路で、しかも
v3.3.0 の codec 変更が 5.8 倍速く・600 分の 1 の大きさにした後の値**である。

見送る理由:

- 非同期化は `edit_store_write_succeeded` の**同期的な失敗報告**を壊す。この契約は同じ
  レビューの R-26 で今回直したばかりで、「DB 書き込みが失敗したらサイドカーのミラーも
  書かない」という一貫性判断もそこに乗っている。worker 化すると失敗の通知・サイドカー
  ミラーの適用・同一 key の連続保存の順序付け (世代/CAS) を一度に設計し直すことになる
- 対象はリリース済みで日常的に使われる編集経路。リリース直前に入れる変更としては割に合わない
- 安全な部分的改善が見つからなかった。deflate level を下げる案は実測が不安定で
  (合成マスクの圧縮特性に左右される)、保存サイズを全ユーザーに対して変える。
  未変更レイヤーの再エンコード回避は `Vec<f32>` の同一性判定が要り、判定自体が数 ms かかる

**次にやるなら**: 保存要求を (key, layers, 世代) で worker へ渡し、失敗はトースト遅延表示、
サイドカーミラーは応答時に UI スレッドで適用、同一 key は世代で last-writer-wins。
R-14 で `Arc` 共有にしたので、worker へ渡す時点の複製はもう無い。

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

## 9. v3.3.0 リリースタグ P1 再レビュー (2026-08-29, Codex)

### 9.1 対象と結論

- 固定対象: `v3.3.0` = `0d141615832598ab40562d94c2ae85163eee7cdb`
- 方法: タグ専用 detached worktree で静的レビューと focused test を実施。レビュー中の `master`
  および作業ツリー上の未確定変更は判定に混ぜていない。
- 対象: 初回 P1 12 件。補足として、関連修正 R-24 と R-17 周辺の同型遷移も確認した。
- 結論: **「P1 は適切に解消済み」とは判定できない。元 P1 のうち 7 件は元の P1 原因を解消、
  R-15 の取り下げは妥当、4 件は P1 が残存する。加えて R-17 修正経路に新たな P1 R-27 を確認した。**

従って、リリースタグに残る P1 は **5 件**（R-02 / R-07 / R-14 / R-26 / R-27）。
R-03 / R-05 / R-06 / R-20 には追跡すべき residual があるが、元 finding の P1 不変条件は回復しており、
残件は P2/P3 と評価した。

| ID | 再判定 | 要約 |
| --- | --- | --- |
| R-01 | ✅ 解消 | abort request identity と後継要求の所有権が分離され、成功・失敗 terminal の双方で後継が解放される |
| R-02 | ❌ **P1 残存** | AtRest detached を mount した後も batch 内 action が MainWindow surface を選べ、main 向け操作で detached bundle を更新する |
| R-03 | ⚠️ 元 P1 解消 / residual P2 | ページ間 transient 誤適用は解消。実 gesture 中の page switch は commit/rollback せず handle と snapshot を捨てる |
| R-04 | ✅ 解消 | split mode から reading direction を一意に導出し、解決・永続化の両方が同じ正本を使う |
| R-05 | ⚠️ 元 P1 解消 / residual P2 | sequence と再 probe で通常 race は解消。clear 後の crash window には ledger flag 喪失余地が残る |
| R-06 | ⚠️ 元 P1 解消 / residual P2/P3 | wheel ごとの同期保存は廃止。teardown 時の同期保存と異常終了時の直近値喪失は別残件 |
| R-07 | ❌ **P1 未対応** | 24MP 単一面の 70.6ms 計測後に意図的延期。複数 mask/layer の全量 serialize・圧縮・DB 保存は UI thread 上に残る |
| R-14 | ❌ **P1 残存** | enqueue 時の三重 clone は解消したが、writer が snapshot を保持中の次回 UI mutation で `Arc::make_mut` が文書全体を deep clone する |
| R-15 | 🚫 取り下げ妥当 | reducer の effect は FIFO であり、元 finding の Retire/Abort 逆転は成立しない |
| R-17 | ✅ 元 finding 解消 / **R-27 発見** | command suffix の欠落は解消。ただし同一 detached host を後継が再利用する連続遷移で別の所有権違反がある |
| R-20 | ⚠️ 元 P1 解消 / residual P2 | decoded image は 128MiB に bounded。failed cell 数と全 map `snapshot()` の clone cost は未制限 |
| R-26 | ❌ **P1 残存** | `Some(DB)` の write failure は伝播するが、startup `open().ok()` と `None => Ok(())`、delete failure の publication が authority split を残す |
| R-24 | ✅ 解消（初回 P2） | bundle retire 前に terminal effects を drain する lifecycle へ修正済み |

### 9.2 残存 P1 の根拠と根本修正境界

#### R-02: gamepad の foreground context と action surface が分裂する

- `src/app.rs:66806-66824` は AtRest detached が active なら gamepad batch 全体をその context へ mount する。
  しかし `src/app/gamepad_input.rs:575-583,2248-2275` は foreground 判定により batch 内 action を
  `ActionSurface::MainWindow` と解決できる。
- その後 grid 操作は mount 中の field を使うため、main を対象に選んだ操作が detached bundle を更新する
  (`src/app/gamepad_input.rs:6115-6197`)。device sampling と destination ownership が同じ境界で決まっていない。
- 既存 test (`src/app/tests.rs:63519-63566`) は空 batch かつ foreground HWND 未設定で、実 action の destination
  と状態変化を検証していない。
- 根本修正: foreground / last-touched から batch destination を先に一度決定し、Viewer destination のときだけ
  対象 context を mount する。MainWindow action は root state に dispatch する。main/detached の各 foreground で
  non-empty D-pad、Y、ring action を検証する。

#### R-07: local adjustment の durable save が UI thread を占有する

- `src/app.rs:54137-54165` から `src/local_adjust_db.rs:102-120` へ、文書全体の JSON 化、mask q8 量子化、
  deflate、SQLite write を同期実行する。`app.rs:54160` では sidecar 向け layer clone も同じ操作内にある。
- 70.6ms は 24MP の単一 alpha 面の実測であり、最大 1GiB 文書の複数 layer / primary / add / subtract /
  subject mask を上限化した値ではない。background migration の repack/VACUUM と競合すれば DB busy timeout
  も UI 側の待ち時間になる。
- codec 改善と安全な延期判断は確認できるが、「UI を block しない」という今回の P1 判定基準は満たさない。
- 根本修正: `(key, immutable document snapshot, generation)` を worker が serialize/commit し、応答 generation が
  最新のときだけ UI state と sidecar mirror を publish する。失敗・連続保存・終了時 flush も typed lifecycle に含める。

#### R-14: COW が clone の時点を後ろへ移しただけで、UI deep clone は残る

- `Arc` 化により enqueue / pending / worker 間の重複 clone は解消した。
- 一方 `src/sidecar.rs:196-200` の `Arc::make_mut` は writer が旧 snapshot を保持中なら `BTreeMap` と nested
  mask vector 全体を複製する。10分 periodic flush、edit tool exit 後の即時再編集、folder switch 後の復帰で到達する
  (`src/app.rs:59710-59739`, `src/sidecar.rs:202-217`)。
- 1GiB document budget に対して小規模 fixture の pointer/COW test だけでは UI latency/OOM 不変条件を保証しない。
- 根本修正: entry / layer / mask 単位の immutable `Arc`、persistent map、または generation 付き delta/journal にし、
  UI mutation の複製量を変更部分へ限定する。

#### R-26: DB open/write/delete failure で durable authority が分裂する

- startup は各 edit DB を `open().ok()` で保持する (`src/app.rs:13170-13244`)。その後複数 save route が
  `None => Ok(())` とするため、DB が開けていない状態を durable success と同じに扱う
  (`src/app.rs:53985-54284,58655-58669,60668-60775,60917-60983,61117-61129`)。
- 既存中央 row がある状態で open failure → 新 sidecar publish となると、次回 import は中央 row を authoritative
  として新 sidecar を skip する (`src/sidecar.rs:867-999`)。delete error でも memory/presence を先に更新し、
  dirty/retry を残さないため再起動で旧編集が復活する経路がある。
- 既存 test は `Some(read-only DB) -> Err` の mask save 一経路だけで、`None`、delete、各 sibling store、
  sidecar flush 後の restart/reconciliation を覆わない。
- 根本修正: DB availability を typed state とし、全 edit store の save/delete が durable commit 成功後だけ publish する。
  失敗時は dirty generation と retry/error を保持する。sidecar を復旧 authority にするなら双方の revision 比較が必要。

#### R-27（新規）: retiring host と successor host の alias で Detached 再要求が消える

- `Detached(H) -> Fullscreen` の retire 待ち中に `Detached` を再要求すると、生存中の旧 host `H` が後継の
  `next_host_hwnd` として保存される (`src/app/native_video.rs:1340-1354`,
  `src/app/presentation_transition.rs:433-452`)。
- `NativeRetired` 後、reducer は FIFO で `CloseDetachedSession(H)`、`DestroyHost(H)`、同じ `H` を持つ
  successor `PrepareNative` を生成する (`src/app/presentation_transition.rs:737-812`)。
- executor は先に session/runtime を除去し、`PrepareNative` の typed `host_hwnd` を使わず current host を再解決する
  (`src/app/native_video.rs:1457-1487,1627-1671`, `src/app.rs:37387-37434`)。解決は `None` となり
  `NativeFailed` へ遷移するため、再 Detached 要求が失われる。表示は Fullscreen、window は閉じる一方、
  settings と toast は Detached ON のままになり得る (`src/app.rs:60161-60203`)。
- retire は native pump の非同期境界なので連続 F12、keymap、gamepad から到達可能。既存 test
  (`src/app/presentation_transition.rs:1517-1592`) は outgoing host と successor host を別値に固定し、
  alias、App executor、resolver、最終設定/表示の収束を検証しない。
- 根本修正: 同一 `H` を後継 Detached が使うなら session/host ownership を移譲し、`H` の Close/Destroy を発行しない。
  破棄する設計なら後継を `AwaitingHost` に戻し、新 runtime/viewport 成立後に Prepare する。delay/retry ではなく、
  reducer と detached session/host ownership 境界を修正する。

### 9.3 P2/P3 residual

- **R-03 (P2):** shape / brush / canvas gesture 中の page switch は、すでに memory を変更しているのに
  commit も rollback もせず transient handle と before snapshot を clear する
  (`src/ui_adjustment_panel.rs:10421-10493,10643-10698,13342-13365`)。
- **R-05 (P2):** sequence check 後から CAS/re-probe 完了までの間に process が終了すると、clear 済み ledger
  flag を戻せない crash-consistency window がある (`src/content_identity.rs:1503-1525`)。
- **R-06 (P2/P3):** strip teardown の設定保存は UI thread 上で同期実行される。通常操作の notch ごとの stall は
  解消したが、cold DB の close hitch と strip open 中の異常終了による直近値喪失は残る。
- **R-20 (P2):** decoded image bytes は bounded だが、失敗 cell は最小 128 bytes 換算のため最大約100万件を持て、
  `BTreeMap` overhead を予算に含めない。UI poll の `snapshot()` は map 全体を lock 中に clone する
  (`src/video/seek_strip_thumbs.rs:604-680,959-968,1468-1488`)。

### 9.4 タグ固定検証

| 検証 | 結果 |
| --- | --- |
| target identity / cleanliness | `0d141615832598ab40562d94c2ae85163eee7cdb`、専用 worktree clean、`git diff --check` 成功 |
| presentation transition | 17 tests passed / 0 failed |
| P1 focused filters | gamepad、page transient、Remote direction、content identity、settings generation、sidecar COW、seek budget、DB failure、command batching、terminal close の 24 tests passed / 0 failed |
| format | `cargo fmt -p mimageviewer -- --check` 成功 |

合計 41 test execution は成功した。ただし成功した test は上記の gap を直接覆っていないため、残存 P1 を反証しない。
特に R-02 は空 batch、R-14 は小規模 COW、R-26 は `Some(read-only DB)` の単一路、R-27 は異なる HWND の
reducer state までしか検証していない。本節は read-only 再レビューであり、source code は変更していない。

## 10. §9 の裏取り (2026-08-29, ClaudeCode)

§9 は静的レビューで、§9.4 自身が「成功した 41 test は指摘した gap を覆っていない」と書いている。
そこで 5 件それぞれについて、**実在するか / 何が壊れるか / v3.3.1 に入れるか** を独立に確認した。
結論: **5 件とも実在する。ただし 2 件は「新しい問題」ではない。**

| ID | 実在 | 確認方法 | 深刻度 | v3.3.1 |
| --- | --- | --- | --- | --- |
| R-27 | ✅ | **失敗するテストで再現** | 高 — 複数ウィンドウ操作で表示と設定が食い違う | **入れる** |
| R-02 | ✅ | コード確認 (実装が**自分のコメントの意図を裏切っている**) | 中〜高 — 誤った窓を操作する。データ破壊は無し | **入れる** |
| R-26 | ✅ | コード確認 (無言のデータ損失の連鎖を特定) | 高 — 頻度は低いが編集が黙って消える | **入れる** |
| R-14 | ✅ (残存) | doc comment に既に明記済みの設計上のトレードオフ | 中 — R-07 と同一の構造問題 | R-07 と 1 件として扱う |
| R-07 | ✅ | 既知 | 中 | **既に v3.3.1 予定** (§1.0) |

### 10.1 R-27 — 再現済み

`src/app/presentation_transition.rs` の
`a_successor_does_not_reuse_the_host_the_same_batch_destroys` が現状落ちる (`#[ignore]` 中)。
`Detached(H) -> Fullscreen` の retire 待ちに `Detached` を要求すると、`NativeRetired` の
**同じ effect batch** が `DestroyHost { hwnd: H }` を出しつつ後継を
`ReadyToPrepare { host_hwnd: H }` にする。

production で `ready_host == H` になることも確認した。`native_video.rs` の `ready_host_hwnd` は
`detached_viewer_video_host_ready()` が真なら現在の detached host をそのまま読み、その述語は
`detached_viewer_client_rect_physical().is_some()` — retire 中の窓はまだ生きているので真になる。

既存テストが見逃していた理由も特定した。`OUTGOING_HOST = 0x202` と `CANDIDATE_HOST = 0x404` を
**別値に固定**しており、production で実際に起きる「同じ値」の場合を一度も通していない。

### 10.2 R-02 — 判定が 2 つの情報源に割れている

**2026-08-30 追記**: 実機で確認したところ、**利用者環境ではこの症状は再現しなかった**
(別ウィンドウを開いたままメインを前面にして十字キーを押すと、v3.3.0 でもメイン一覧が
動いて見えた)。以下で確認したのは「配り先と面の判定が別の情報源を使っており、食い違い
得る」ことまでで、`active_detached_context_is_at_rest()` が真になる条件は限られる。
**「誤った窓を操作する」と断定したのは証拠より強かった。** 修正 (`7f064a57`) は 2 つの
判定を 1 つにするもので構造的には正しいが、深刻度は下方修正する。

配り先の判定と surface の判定が**別々の情報源**を使う。

- 配り先: `app.rs` の `gamepad_goes_to_active_context = self.active_detached_context_is_at_rest()`。
  この述語は `active_viewer_context_id()` の residence だけを見ており、**前面ウィンドウを見ていない**。
- surface: `gamepad_input.rs` の `current_input_surface()` →
  `resolve_input_surface(.., foreground_app_hwnd(), ..)` は**前面を見る**。前面がメインなら
  `ActionSurface::MainWindow`。

同じ frame の同じ batch で、前者が「detached bundle を mount」、後者が「対象はメイン」と答え得る。
そして `handle_gamepad_y_tap` のコメントはこう書いてある:

> グリッド面: Y でツリーをトグル。detached viewer が同時表示中でも
> **foreground / last-touched がメインならこちらを操作する。**

ところが `handle_gamepad_direction_for_grid` が読み書きする `self.selected` /
`self.scroll_to_selected` / `self.items` は、mount 中は `ViewerContextBundle` が所有する
detached 側の field である (`viewer_context_registry.rs:704` 以降に `items` / `selected` /
`scroll_offset_y` / `scroll_to_selected` を確認)。**意図した挙動をコードが実現できていない。**

根本修正の境界は §9.2 のとおり: batch の宛先を前面 / last-touched から**先に一度**決め、
Viewer 宛のときだけ context を mount する。

### 10.3 R-26 — 無言のデータ損失の連鎖

損失経路を最後まで辿れた。

1. 起動時に各 edit DB を `open().ok()` で保持する (`app.rs:13170` 以降、adjustment /
   local_adjust / export_crop / mask / conceal / comic の 6 つすべて)。開けなければ `None`。
2. 保存経路は `None => Ok(())` (13 か所)。`edit_store_write_succeeded` は `Err` のときだけ
   失敗を報告するので、**開けていない状態は durable 成功と区別されない**。
3. そのまま `set_page_key_presence(.., true)` が立ち、sidecar ミラーが書かれる。
4. 次回起動で DB が開けると `sidecar::import_to_dbs` が走るが、その doc は
   「中央 DB に既にエントリがあるものは **上書きしない** (中央が authoritative)」。
   → 中央に古い行があると、`None` の間に書いた sidecar は**捨てられる**。

利用者にはエラーもトーストも出ない。到達条件は「以前の行が中央にあり、今回 DB が開けない」で、
別インスタンスの排他ロック・DB 破損・ディスク満杯などで成立する。頻度は低いが、
起きたときの結果が編集の消失なので P1 の格付けは妥当。

### 10.4 R-14 は R-07 と同じ構造問題

`SidecarFile::items` の doc comment に既に書いてあるとおり、`Arc::make_mut` は
**writer が前の snapshot を持っている間だけ 1 回**複製する。v3.3.0 の修正は
「flush・読み直し・worker 取り出しの 3 か所が毎回 map 全体を deep clone」を
「重なったときだけ 1 回」にしたもので、常用経路は 3 回 → 0 回になっている。

残るのは「編集文書という大きな値を UI スレッドが所有して複製する」という一点で、
**これは R-07 (マスクの直列化・圧縮・DB 保存が UI スレッド) と同じ問題**である。
R-07 の根本修正 (`(key, 不変スナップショット, generation)` を worker が処理し、
最新 generation の応答だけを publish する) を入れれば、sidecar ミラー側も同じ
所有権に乗せられる。**2 件を別々に直さず、1 つの作業として扱うほうがよい。**

### 10.5 対応状況 (2026-08-30 時点)

| ID | 状態 |
| --- | --- |
| R-27 | **修正済み** — 後継へ lease を移譲し、retire 後に exact claim を取り直す (`44e012a6` / `e84a8daf`、実装 Codex)。再現テストの `#[ignore]` は外した |
| R-02 | **修正済み** — 配り先と面を `current_input_surface` 1 つから導く (`7f064a57`) |
| R-26 | **修正済み** — `EditStoreOutcome` で「開けていない」を成功と区別する (`b8cb3ce5`) |
| R-14 | R-07 の一部として扱う (格下げ) |
| R-07 | 未着手。§1.0 のとおり v3.3.1 の作業項目 |

**実機確認が残っている**: R-27 は F12 連打まわり、R-02 は別ウィンドウを開いたままメイン一覧を
gamepad で操作する経路。どちらも unit test では完結しない。

### 10.5b v3.3.1 への割り当て (当初)

- **新規に入れる**: R-27 / R-02 / R-26。R-27 と R-02 は detached の所有権境界に触るので、
  CLAUDE.md の凍結ルール (症状パッチ禁止・構造的修正は ClaudeCode と Codex の合意 +
  [detached-rework-plan](detached-rework-plan.md) への記録) に従う。
- **既に予定済み**: R-07 (+ 図形のキー移動 / 塗りの毎フレーム複製)、§1.0b (対応済み)、§1.0c。
- **格付け変更の提案**: R-14 は単独 P1 ではなく R-07 の一部として扱う。

### 10.6 R-27 の設計判断 (ClaudeCode と Codex の合意、2026-08-29)

凍結ルール (CLAUDE.md「Detached viewer リワーク中のルール」) に従い、着手前に
[detached-rework-plan](../detached-rework-plan.md) §2 を読み、Codex に設計判断を当てた。

**ClaudeCode の当初案は誤りだった。** 「破棄予定の host は ready host ではない」ので後継を
`AwaitingHost` へ戻す (案 B) を推したが、Codex が事実で否定した:

- `DestroyHost` は H に `DestroyWindow` していない。現在の `detached_viewer_window_id` から
  ViewportId を再解決して `ViewportCommand::Close` を送るだけで、effect の `hwnd` はログにしか
  使われない。
- egui-winit 0.33.3 の `ViewportCommand::Close` は同期 destroy ではなく `ViewportEvent::Close`
  を積むだけ。
- 案 B には「旧 viewport の close 完了 → 新 window identity の確保 → 新 HWND 登録」という
  因果経路が無い。同じ ViewportId を render し続けるので、**まだ生きている H を再び
  `HostReady` にする**か、`ViewportEvent::Close` を利用者の close と解釈して**後継ごと閉じる**
  かのどちらにもなる。永久待機だけが問題なのではなく、結果が lifecycle ordering 依存になる。

**合意した設計は案 A′ (lease の移譲 + retire 後の再検証)。** native presenter は H 自身では
なく H 配下の `WS_CHILD` で、retire が壊すのは child と render core / DComp / GPU 資源だけ。
親 H には SetParent / style / subclass / DComp target のような再利用不能な状態が残らないので、
**同じ egui parent を後継 presenter の親として再利用できる**。後継の `Switch` も retire と
同じ native pump FIFO に後から入るので、旧 child の Destroy を追い越さない。

ただし要求時に観測した H をそのまま信用はしない。`NativeRetired` の後に現在の host claim を
採り直してから Prepare する (`{window_id, host incarnation, hwnd}` で producer → reducer →
effect → executor を結ぶ)。これは規則 5 の「判断は問うている事柄と同じ時間・所有境界の
事実で行う」に沿う — 要求時の HWND は非同期 retire 境界の向こうでは事実ではない。

**単純な alias guard (同一 H なら Close/Destroy を省くだけ) では Codex は合意しない**と明示
された。失敗・再置換・terminal close で session が残るため。

ブリーフは [r27-fix-brief.md](r27-fix-brief.md)。実装は Codex、検収は ClaudeCode。
完了後に [detached-rework-plan](../detached-rework-plan.md) §11 へ記録する。
