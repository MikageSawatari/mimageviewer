# コレクション出荷前レビューの現行コード照合（2026-09-19）

## 対象と観測範囲

利用者依頼により、`bee2dbec2` で確定した仕様と
[実装計画 §23](collection-implementation-plan.md#23-v400-出荷前レビュー後の修正計画2026-09-16)、
[ClaudeCode レビュー](review-v4.0.0/README.md) を現行コードに照合する。
本書はコードからの判断であり、実機での再現・性能実測ではない。
利用者が報告した一過性のコレクション読み込み停止は、再発していないため独立案件として保留する。

## 判断済みの修正と設計補正

- **B-6 計装不足は妥当**。collection の open / prepare / navigation / import / export /
  migration に要求・区間・終端を識別できる計測を追加する。ログ無効時の余分な計算、毎フレームの
  大量ログ、計装のための新しい制御状態を増やさない。§23.1 の実装・関連テスト 162 件・
  cargo check / fmt・独立 Sol レビューは完了。全体 gate と確認ビルドは後続のまとまった変更と集約する。
- **A-1 / A-2 ソート所有の欠落は妥当**。`app.rs` の `page_order_locked_for_current_view` /
  `details_header_sort_active` / `apply_sort_change_reload_with_physical_mode` と `ui_main.rs`
  の選択表示・更新経路が Collection root を扱っていない。
  ただし Manual に既存 `PageOrderFixed` をそのまま返す案では toolbar / menu 全体も無効になり、
  手動順から通常順へ切り替えられない。**列ヘッダの固定と、定義の並び順を選択する権限は分ける**。
  root と PhysicalSource 子を区別し、loading / stale は理由付きで無効化する。
  表示中の revision と一致する定義を表示値の正本とし、global `settings.sort_order` を書き換えない。
  同一 root の order 変更時も列ソートを戻す。install が旧 mode を参照して details_order を再構築したまま
  新 mode を公開しないよう、order と items の採用順序を揃える。
  Standard の列ヘッダは既存計画どおり一時的な表示順であり、再生・export・Remote の有効順を変更しない。
- **A-5 タイトル / A-6 元の場所 / A-4 キー入口の不足は妥当**。同じ root 判定を利用し、
  元ファイル操作、子フォルダ操作、既存 keymap の所有境界を維持して補う。
  「元の場所」への移動は成功時にだけ root を退役し、失敗・取消では元の表示を保持する。
- **シャッフルの仕様は採用可能**。stable seed と entry ID から決定的な順序を作り、手動順は変更しない。
  hash の方式と衝突時の tie-break を固定する。Remote の enum / 表示 / IPC 互換境界も更新する。
  仕様案の「未リリースなので schema migration 不要」は不適切。**既存試用 DB を保持する移行が必要**。
- **A-3 / C-11 参照解除の表示非対称は妥当**。Unavailable の場合も理由付き無効項目を残す。
  A 報告の修正案はこれだけだが C 報告は元ファイル削除も無効にする案であり、両者は同一ではない。
  明示的な元ファイル操作の既存仕様を一括して削らず、対象 identity と操作可能性を個別に検証する。
  現行の明示的な元ファイル削除は context menu が捕捉した `delete_targets` を削除確認へ渡し、
  collection actor の参照解除とは別の対象を所有する。actor binding が更新中であることだけを理由に
  この操作も禁止する案は、そのまま採用する根拠が足りない。まず A 案の表示維持を対象とする。
- **M-1 rename scope は妥当**。dialog 状態消去より前の scope 捕捉と、dialog → poll を通す回帰が必要。
- **C-1 / C-2 / C-3 / B-3 は妥当**。Starting / Busy を終端失敗や再生末尾に潰さない。
  再駆動は既存 typed state に帰属させ、単発の遅延 repaint だけに依存しない。
- **B-1 と 10,000 件上限は採用可能**。actor 側で件数を保証し、解析の入力サイズと描画量も制限する。
  既存データが上限を超えている場合に黙って切り捨てる移行は行わない。
- **バックアップ / 全件 export は採用可能だが追加設計が必要**。
  `tags_db.rs` の世代回転は最初の write 前であり、計画の「起動時」と同一ではない。
  collection の移行・書込前に回復可能な保存を行う時点を明記する。
  名前は Windows のファイル名制約・重複・予約名を考慮して割り当て、既存ファイルを上書きしない。
  catalog と snapshot を別々の要求で集めるだけでは同一時点にならないため、actor 側の一括読み取りで
  immutable な集合を得る。部分失敗と完了を区別し、一覧ファイルは完全な結果だけを示す。
- **文書不足は妥当**。製品仕様・マニュアルは開発側で更新する。README のリリース記事、版番号、
  配布・公開用メタデータは従来どおり公開担当へ引き渡し、未完成機能を完了済みとして公開しない。

## 利用者へ相談中の点

### B-4 / D-2: target だけの再検査は現行仕様を維持しない

`collection_store/prepare.rs` は現在の kind / availability / mtime / size で候補と通常順を作る。
actor revision が変わらなくても、外部での削除・復帰・更新は起こる。
したがって「revision が同じなら target と隣接数件のみ stat」では、復帰した候補や日時・サイズ順の
変化を見落とす。高速化の必要性は妥当だが、提案された fast path をそのまま実装しない。
既存動作を維持する再設計か、外部変更を明示更新で反映する仕様変更か、利用者へ確認中。

機能を変えず先行できる候補は、Standard の fact/entry 集合照合を O(N²) から HashSet による O(N) へ
変えること、動画 0 件なら動画 pin DB を開かないこと、Remote の同じ wire entry の JSON 生成を
予算計算と token 用で共有すること。これらだけで全件 stat の所要時間を保証するものではない。
Remote の再 stat / canonicalize は状態変化と公開範囲の検証も担い、単なる二重処理として除去しない。

### M-2: rename journal の読み出し失敗は延期非推奨

`rename_key_migration.rs::journal_load` は read / parse 失敗を空へ潰し、`journal_save` は空を削除する。
さらに新しい非空 snapshot が古い記録を上書きする可能性もあり、空削除だけの抑止では足りない。
読込失敗を typed に伝播し、旧記録の保全と新しい保存・移行の admission を設計する必要がある。
出荷前のデータ保護修正へ戻すことを利用者へ提案中。

### D-4: Remote の deep link / seek の ordinal 不一致は延期非推奨

`app.js` の prefix 外 deep link は `navigable_media` の ordinal を sparsePosition に保存するが、
seek は `still_image` の ordinal として送る。core は画像だけの射影から nth を選ぶため、混在時にずれる。
10,000 件上限でも応答バイト数による prefix 打ち切りはあり、この問題は消えない。
2026-09-19、利用者が「出荷前に修正する」を選択。v4.0.0 の対象へ戻す。

修正では request の `target_kind` を候補探索用として維持し、着地した `SparseTarget` の媒体から
位置の射影を決める。core の同じ exact prepared / Remote 有効列で `{ kind, ordinal, count }` を算出し、
IPC / HTTP / Web へ型付きで渡す。Web の可視 prefix から位置を再計算しない。
画像 seek は `still_image` の位置だけを使い、動画・音声は各媒体の位置を使う。
HTTP の見開き partner 昇格では画像位置を調整し、既存 token / revision / 公開範囲 / session・route の
失効契約を維持する。混在列の prefix 外リンク、直後の seek、媒体ごとの件数、範囲外拒否、
非公開行の除外、見開き、競合の回帰を対象とする。これは設計調査の結果で、まだ実装・検証前。

## 延期候補の扱い

- **B-2**: revision 前進時の全体再 install は残る。データ損失とは区別し、計測を残して延期可能。
- **B-5**: migration の全 entry × mapping は残る。1 コレクションの件数上限は総件数を制限しないため、
  それだけで性能上安全とは言えない。計装後の規模別検証を根拠に判断する。
- **D-1**: catalog が単一 Home worker を待たせる構造は残る。通常 Home/List への影響を計測せず
  「影響なし」とは扱わない。レーン分離を延期する場合も制限と未検証を残す。
- **D-3**: Remote root の動画 sidecar / rating 差は残る。表示差として延期可能。
  既存 sidecar helper の単純流用では cancel / deadline と公開範囲の契約を満たさないため、導入時に再設計する。
- その他 P3 群や M3U / 登録順 / D&D 追加は、既定の後続版候補を維持する。

実装・検証の完了状態は実装計画 §23 を正本とし、本書の妥当性判断だけで完了に変更しない。

## A-6 の実装境界（ソート修正とは別の変更）

`JumpToFolderRequest` に既存 Collection root の stamp / entry / source / items generation を
まとめた origin を捕捉し、`FolderOpenScanPurpose::JumpToPhysicalFolder` の request とともに運ぶ。
既存 `CollectionGridPhysicalLoadOwner` は source 検証に利用可能だが、ロード採用の owner は
`Navigation` とする。`CollectionGridPhysical` で採用すると collection の子として残ってしまう。

開始時は root の surface / items / current_folder / selection / address / history を保持する。
Windows / 非 Windows の ready 処理で origin がまだ一致することを確認し、scan 成功後の
`load_folder_with_scan_owned` が成功した場合だけ root を退役して選択を適用する。
既存の物理ロード採用が Collection→Path の履歴を扱う。scan 失敗・切断・取消・競合・scope 拒否では
root を保ち、失敗後に snapshot を戻す方式を追加しない。

egui の `context_menu_idx` は行番号しか持たず、開いている間の root 再 install で別 entry になり得る。
items generation に結び付いたメニューの失効を採用境界で扱い、古い番号で新しい項目を操作しない。
新たな App bool / 独立 pending は作らない。検証は pending / error / cancel / supersede /
stale revision・generation・context / 成功履歴 / exact selection / A・B / sibling 非干渉を対象とする。
