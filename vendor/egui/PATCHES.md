# egui 0.33.3 のローカル変更

別バージョン検索の長押し表示を修正する際、ナビゲーターの入力順序を
egui のクリック判定と対応付けるために追加した、読み取り専用の API である。
クリックの判定条件、時刻、入力イベントの生成・消費は変更しない。

## 出典

- crates.io package: `egui-0.33.3.crate`
- package SHA-256: `6a9b567d356674e9a5121ed3fedfb0a7c31e059fe71f6972b691bcd0bfc284e3`
- upstream repository: `https://github.com/emilk/egui`
- `.cargo_vcs_info.json` の commit: `44cdd653e2317d300fb8a6c9c36b03f23991e803`
- upstream package path: `crates/egui`
- 検証日: 2026-09-08

Cargo registry の archive checksum を導入前の root `Cargo.lock` と照合し、
archive 内の106ファイルと展開済み registry source の全 bytes が一致することを確認した。
registry 管理用の `.cargo-ok` と生成された `target/` はコピーしない。
この文書とライセンス本文の追加を除き、原本からの Rust 差分は
`src/input_state/mod.rs` の API と回帰テストだけである。

## API の境界

`PointerState::released_buttons_with_click_counts()` は、当該 pass で egui が判定した
全 button release を順序通りに返す。

```rust
impl Iterator<Item = (PointerButton, Option<(Pos2, u32)>)> + '_
```

click でない release も `None` として返すため、raw release 列と序数が一致する。
返すのは値の射影であり、内部の mutable state や内部イベント型は公開しない。
呼出側は全 button・全 focus 区間へ注釈してから、viewport、位置、操作 owner、focus を
判定する。この API 自体は、特定 widget が click を所有することを保証しない。

通常の `Response::double_clicked()` は pass 全体の判定を参照するため、同一 pass 内で
focus が切り替わる場合の release を特定する用途には情報が不足する。
アプリで別のクリック時間窓を実装したり、egui の入力処理を再実行したりしないための境界である。

## 統合

- root workspace では本 crate を除外し、`[patch.crates-io]` から同じ path を指定する。
- standalone の `vendor/eframe` と `vendor/egui-wgpu` にも `../egui` の patch を置く。
- 直接依存だけを path 化し、推移的な registry egui と型を分裂させない。
- root と両 standalone の dependency graph / lock を確認し、同じ local egui 一実体へ揃える。
- `scripts/test-full.ps1` は既存の検証に加え、本 crate の無 filter lib tests を実行する。

## ライセンスの保持

package の `MIT OR Apache-2.0` 宣言は変更しない。原本 archive はライセンス本文を
含まないため、既存 `vendor/egui-wgpu` の同名本文を変更せず同梱する。

| コピー元 | コピー元の SHA-256 |
| --- | --- |
| `../egui-wgpu/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../egui-wgpu/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |

MIT の本文・著作権表示は、同じ upstream commit の
[LICENSE-MIT](https://raw.githubusercontent.com/emilk/egui/44cdd653e2317d300fb8a6c9c36b03f23991e803/LICENSE-MIT)
でも確認した。上表の hash は既存 repository 内のコピー元の値であり、remote ファイルの
byte hash としては主張しない。About の既存ライセンス本文表示は維持する。

## 検証記録

- 原本との差分、読み取り専用 API、2件の回帰、manifest / gate 接続を独立 Astra がレビュー済み。
- `cargo test --manifest-path vendor/egui/Cargo.toml --lib`: 25成功、0失敗。
  記録: `target/r4-vendor-egui-tests2.log`。初回 offline 実行は未取得依存で開始できず、製品失敗ではない。
- 追加回帰は、非 click を含む全 button の release 順序と、discard 後の第二 pass への非再生を確認する。
- アプリ側の Flat / Panorama ordered 入力は54件成功、focus snapshot と release 後移動の補強も各1件成功。
- root の `cargo fmt --all -- --check` 成功時、本 crate の変更ファイルの bytes が変わらないことを確認済み。

ライセンス2本文の配置と上表の hash 一致を確認済み。原本106ファイルとの差分は引き続き
`src/input_state/mod.rs` だけで、追加は本書とライセンス2本文（合計109ファイル、生成 target を除く）。
root / eframe / egui-wgpu / egui standalone の cargo tree は、すべて同じ local egui 一実体。
既存3 lock の差分は egui の registry source / checksum 2行の除去だけで、version 更新はない。
記録: `target/r4-cargo-tree-{root-egui,eframe-egui,egui-wgpu-egui,egui-standalone}1.log`。
アプリ全体の最終 gate は target/r4-test-full-final4-j1.log で成功（main lib 7693成功、38ignored）。
portable のビルド・24 runtime files の更新照合も成功した。実機確認は利用者の返答待ちである。
段階結果と残作業は [レビュー修正記録](../../docs/duplicate-detection-review-fixes-20260907.md) を参照。
