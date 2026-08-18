# v3.1.1 公開待ち (2026-08-18 夜に実施予定)

タグ `v3.1.1` は push 済み。**配布物はビルド済みで、以下に退避してある。**

```
dist\v3.1.1\mimageviewer.exe                    413.5 MB  単体exe (launcher)
dist\v3.1.1\mImageViewer_setup.exe              225.0 MB  インストーラ
dist\v3.1.1\mImageViewer_installer_v3.1.1.zip   224.6 MB  Vector 申請用 (setup + readme)
dist\v3.1.1\mImageViewer_portable_v3.1.1.zip    249.9 MB  ポータブル
```

4 点とも署名 + RFC3161 タイムスタンプを検証済み。

## 開発を再開する前に

- **`cargo build --release` / `build-dist.ps1` を回さないこと** (公開まで)。
  `target\release\mimageviewer.exe` は cargo の出力先そのものなので、普通の release build で
  **署名済みの配布物が上書きされる**。上の `dist\v3.1.1\` は退避コピーなので、
  公開時はここからアップロードすれば安全。
- 開発ビルドは `build-dev.ps1` (`target\dev-runtime`) を使う。こちらは release 成果物に触らない。
- タグの指すコミットを rebase / amend しない。打ち直すと force-push が必要
  (`git tag -f v3.1.1 <commit>` → `git push -f origin v3.1.1`)。
- `Cargo.toml` は 3.1.1 のまま。次のリリース準備で上げる。

## タグ位置の検証結果

配布物のビルドは `9d4366c7` 時点。タグは `760638ba` (別セッションの docs コミットを含む) だが、
`git diff --name-only 9d4366c7 v3.1.1` の差分は `docs/` のみで**コードは同一**。打ち直し不要。

## 残っている手順

1. GitHub Releases で `v3.1.1` を作成。body は README.md の `### v3.1.1` セクションをそのまま
   (3,993 バイト、アプリ内更新通知の 8KB 上限内)。Assets に上記 4 点。
2. mikage.to へ 3 点配置 (製品ページの版・最終更新日・ポータブルのリンクはコミット済み)。
3. Vector 申請 (`mImageViewer_installer_v3.1.1.zip`)。
4. 公開後、別マシンで更新通知ダイアログの表示を目視確認。
5. Microsoft Store は毎リリース必須ではない。今回は見送りで可。

**公開が済んだらこのファイルを削除する。**
