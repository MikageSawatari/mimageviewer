# 写真の GPS 欄から Google マップを開く

メインセッションへの実装依頼。デスクトップ版 mImageViewer で写真を見ているとき、情報パネルの EXIF → GPS にある「Google マップで開く」から、撮影地点を既定ブラウザーで確認できるようにする。対象は既存の EXIF 表示で GPS を取得できる写真。地図表示はブラウザーに任せる。

## 表示と操作

- 表示中の写真に有効な緯度・経度があり、既存の設定で両方が表示対象のとき、GPS 欄に「Google マップで開く」を表示する。GPS がない写真、片方が欠けた写真、不正な座標の写真では表示しない。見開きは情報パネルで現在選択されている側を使う。
- 押したときだけ、`https://www.google.com/maps/search/?api=1&query=緯度%2C経度` を既定ブラウザーで開く。座標は符号付き十進度で渡す。Maps URLs は座標にピンを表示でき、API キーは不要。 [Google Maps URLs の公式仕様](https://developers.google.com/maps/documentation/urls/get-started#search)
- ボタンの補足で、ブラウザーが開き、Google に撮影位置が渡ることを示す。URL に含める写真由来の情報は緯度・経度のみにする。URL や座標は診断ログ・失敗メッセージに出さない。ブラウザー起動に失敗したら、既存の通知の作法に沿って知らせる。

## 実装の入口

- 着手前に `AGENTS.md`、`CLAUDE.md` の該当節と [ドキュメント索引](../README.md) を確認する。EXIF の表示は `draw_exif_panel`、取得済みデータの参照は `get_exif_for_idx` が入口。現在の情報パネルが選んだ EXIF を使い、表示やクリックのためのファイル再読込は避ける。 [情報パネル](../../src/ui_metadata_panel.rs)・[EXIF の読取と表示用フィルター](../../src/exif_reader.rs)
- MCP の `project_exif` と `coordinate` に座標変換の先例がある。実データの表現と検証条件を確かめ、座標の取得・検証を EXIF の責務として共用できるか判断する。緯度の N/S、経度の E/W、度分秒の範囲、有限値、緯度 ±90°・経度 ±180° を確認する。表示言語に依存する文字列や、EXIF に含まれる任意の文字列を URL に連結しない。MCP の GPS 許可は現行どおり適用する。 [MCP の EXIF 投影](../../src/agent_ipc/item_info.rs)
- `ui_helpers::open_url` は URL 全体をログに出す。地図ボタンの起動は既存の `external_links::open_url` に通し、呼び出し側でも URL を含む可能性のあるエラーの原文は記録しない。 [外部リンク処理](../../src/external_links.rs)・[UI 側のラッパー](../../src/ui_helpers.rs)
- 共通パネルで機能する表示経路を確認する。専用の別ウィンドウ経路や表示所有の変更が必要と判明した場合は、先に [detached viewer 計画 §2](../detached-rework-plan.md) の適用範囲を確認する。アプリ内地図、複数地点表示、住所取得、開くサービスの設定は別の開発項目とする。

## 受入確認と引き継ぎ

1. 自動テストでは南緯・西経の符号、軸と方位の不一致、欠損・範囲外の抑止、緯度・経度の順序と URL エンコードを確認する。写真 A から B に切り替えた直後や見開きの左右切替で、直前の写真の座標を開かないことも確認する。ブラウザー起動はテスト用の受け口で観測し、ネットワークに出さない。実用的なら GPS 欄のスナップショットも更新する。
2. マニュアルに操作と送信先を記載する。製品ページとプライバシーポリシーを突き合わせ、押したときに既定ブラウザーが Google に座標を渡すという説明を揃える。 [公開文書の更新ルール](../public-wording-policies.md)
3. リポジトリの規定に従い、関連テスト、`cargo fmt --check`、`python scripts/check_ui_glyphs.py` を通して `.\scripts\build-dev.ps1` で確認用バイナリを作る。公開地点の座標を埋め込んだ使い捨ての写真でピンの位置を確認し、GPS なしの写真ではボタンが出ないことを確かめる。エージェントによるライブ確認は、具体的なシナリオを示して承認を得た使い捨てポータブル環境に限る。 [ビルド・テスト方針](../development-build-and-test.md)・[実機確認ルール](../interactive-release-verification.md)
4. 利用者への引き継ぎでは `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` と上記の確認手順を示す。先に常駐分も含めて mImageViewer を終了する必要があり、確認用ビルドは既定で `%APPDATA%\mimageviewer` の実設定・データを更新し得ることを伝える。
