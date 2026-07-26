# Stage CUT 指示書: 連動時の任意ピン留めを撤去し、別ウィンドウを 2 モード制に単純化する

正本プラン: [../../detached-rework-plan.md](../../detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 位置付け: ゲート C のスコープ決定 (ユーザー決定 2026-07-07)。リワーク後も
  バグが「連動 ⇔ 独立の境界」(bundle なし連動 passive の復帰・passive 上のピン・
  park 遷移順序 = findings-4 F8 系) に 3 連続で集中したため、**境界そのものを
  設計から削除**する。ピンは未リリースなのでデグレではない。
- 実装: Codex / 検収: Fable / 実機 smoke: カット完了後にまとめて

## 1. 確定仕様

### 1.1 モードは既存設定 1 つに一本化

設定「**画像/動画を別ウィンドウで開く**」(preferences) がモードスイッチ:

| モード | 窓の構成 | 復帰経路 |
| --- | --- | --- |
| **ON** (毎回別ウィンドウ) | 全ての detached 窓が生まれつき**独立** (自前 bundle)。メイン連動なし | paused_bundle 復帰のみ (安定済み経路) |
| **OFF** | **連動窓は常に最大 1 枚**で、メイン選択に追従。**passive には決してならない** | 復帰という状態遷移が存在しない |

- メディア live-park (`ParkedLive`) は**両モードで維持** (最大 1 枚、bundle 復帰)。
- OFF モードで独立系の窓 (ParkedLive、その復帰後のメディア窓) がアクティブに
  なる局面では、**連動窓は park せず閉じる** (`close_legacy_detached` 系の既存経路)。
  「連動窓 = 生きて追従しているか、存在しないかの 2 値」にする。

### 1.2 設定切替時の挙動 (ユーザー決定)

- 切替した瞬間に開いている detached 窓 (active / passive / ParkedLive すべて) を
  **自動で閉じる**。メディアは再生停止。
- 確認ダイアログは出さない。トーストで通知する (例:
  「別ウィンドウの表示モードを変更したため、開いていた別ウィンドウを閉じました」。
  窓が無ければトーストも出さない)。
- 理由 (コメントに残す): 旧モードの窓を生かすと混在状態 (ON なのに連動窓が居る等)
  が生まれ、カットで消した複雑さが戻るため。

### 1.3 ピン留め (detached 窓の) を全削除

- タイトルバーのピンボタン・ピンアイコン描画・tooltip・passive 上のピン操作
- 連動 → 独立のその場変換 (pin promote)
- **注意**: 「ピン留め」という語は他機能 (フォルダ代表サムネのピン
  `folder_thumb_pins`、グリッドのタグピン `GridTogglePinnedTag*`) でも使われて
  いる。**削除は detached 窓のピンに限定**すること。

## 2. 削除対象 (完了条件の grep リスト)

コードで確認しながら列挙・削除する。少なくとも:

| 対象 | 備考 |
| --- | --- |
| detached タイトルバーのピンボタン UI + `draw_pin_icon` (draw_icons.rs) | 他機能のピン描画と混同しない |
| `passive_toggle_pin` 経路 | |
| pin promote 一式 (`promote_active_still_to_independent` / `pending_pin_promotion` / `pin_promote_to_independent` 遷移 reason) | **`clone_current_viewer_context_grid_fields_into` は live-park (F6) が使うので残す** |
| `detached_viewer_pin_active` (bundle) / runtime の `pinned` / snapshot の `pinned` | 「独立か連動か」は `linked` 1 軸に集約 |
| **連動窓が passive になる全経路** (`park_legacy_active_to_passive` 等) | 列挙して全て「閉じる」に置換。findings-4 F8-v2 (Parked→Closing 順序バグ) は経路ごと消滅する |
| bundle なし連動 passive の descriptor 復帰経路 | 独立窓側で使う descriptor 機構 (reopen 等) は残す。連動専用部分のみ削除 |
| implementation-plan §3 表⑦ の属性保持マトリクス | 2 モード制の簡潔な表に書き換え |

## 3. ドキュメント同時更新

- [../../detached-viewer-implementation-plan.md](../../detached-viewer-implementation-plan.md):
  §3.0 の状態モデルと表⑦、入力経路表 (§3.0.1) から pin / 連動 passive を除去
- マニュアル: `htdocs/mimageviewer/manual/` で detached のピン留めに言及する箇所
  (fullscreen.html の該当節ほか、`grep -rn "ピン留め" htdocs/mimageviewer/` で全数
  確認し、**detached 文脈のものだけ** 2 モード制の記述へ書き換え)。製品ページ
  (index.html) も同様に確認
- 設定ページの説明文 (preferences pages.rs) を 2 モード制の説明に更新

## 4. テスト

- 削除する既存 pin / 連動 passive テストの一覧を完了報告に (憲法 8 の明示例外)
- 新規:
  - 設定切替で全 detached 窓が close される (active / passive / ParkedLive 各 1 の
    状態から)。窓ゼロのときはトーストも出ない
  - OFF: 独立メディア窓のアクティブ化で連動窓が close される (passive 化しない)
  - OFF: 連動窓が追従を続ける (従来挙動の回帰)
  - ON: 独立窓 2 枚の park → resume (bundle 経路、既存テストの維持で可)
  - 「linked な snapshot が passive リストに存在しない」不変条件を遷移テストで固定
- 既存 detached / parked_live / deferred / placement テスト + full `cargo test` 緑

## 5. 完了条件

- [ ] §2 の grep リスト 0 件 (完了報告にコマンドと結果)
- [ ] §4 のテスト緑 + full test 緑 + `cargo fmt --check`
- [ ] §3 のドキュメント更新済み (マニュアルの grep 全数確認結果を報告)
- [ ] コミットに `(detached-rework CUT)` を含める (機能削除とドキュメントは
      コミットを分けてよい)
- [ ] `.\scripts\build-release.ps1` で実機バイナリ準備

## 6. カット後の実機 smoke (ユーザー、次回まとめて)

1. OFF: F12 で連動窓 → メイン選択に追従 → 動画を開いて再生 → メインから別画像を
   開く → 動画は live-park で残り、新しい連動窓が開く → 動画窓クリックで復帰 →
   **連動窓が閉じる** (passive にならない)
2. ON: 画像 3 枚 → 独立窓 3 枚 → 相互クリックで park/resume が安定 (窓が消えない・
   瞬かない)
3. 設定を ON⇔OFF 切替 (窓を開いたまま) → 全窓が閉じてトースト → 切替後の
   モードで正常に開き直せる
4. ピンボタンがどこにも表示されない
