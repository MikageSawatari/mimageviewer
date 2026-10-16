# egui 0.37 準備 graph

段2専用の独立workspace。rootは引き続きegui 0.33.3 / `vendor/egui*`を使用する。
すべて実ディレクトリで、root manifest / patch / lockは変更しない。

| directory | owner |
| --- | --- |
| `egui` | 0.37.0原本＋既存mIV差分 |
| `egui-winit` | 0.37.0原本＋入力配送・診断・clipboard互換 |
| `egui-ime` | 製品共有policyの新版準備用コピー |
| `egui-wgpu` | 0.37.0原本＋mipmap・delta完結・surface所有・診断（wgpu 30.0.1） |
| `eframe` | 0.37.0原本＋段2c-1の既存scheduler・delta配送・計測等。入力所有・案B・Closeは後続 |

`upstream-provenance.json`は原本archive SHA-256、VCS、取り込んだ全ファイルのSHA-256を持つ。
`import-upstream.py`はローカルregistry archiveと展開sourceをbyte照合して原本だけを配置する。
宛先が既にある場合は拒否する。ネットワーク、junction、registry管理ファイルのコピーは行わない。
原本のままの区切りを再現する場合は、未配置のcheckoutでこのscriptを使う。
このscript自体はpatch適用や検証graph構築をしない。license本文の追加は各PATCHESに記録する。

リポジトリrootのPowerShellで実行する（toolchainはrootのRust 1.99.0固定を継承）。

```powershell
$env:CARGO_NET_OFFLINE = 'true'
$env:MSBUILDDISABLENODEREUSE = '1'
$env:CARGO_BUILD_JOBS = '1'
cargo test --manifest-path vendor-next/Cargo.toml -p egui-winit --lib disabled_producer --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui --lib --features miv-repaint-witness --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-ime --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-wgpu --features winit --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p egui-wgpu --features winit,egui/miv-repaint-witness --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p eframe --features glow,persistence --lib --locked --offline
cargo test --manifest-path vendor-next/Cargo.toml -p eframe --features glow,persistence,miv-test-script-window-witness --lib --locked --offline
cargo fmt --manifest-path vendor-next/Cargo.toml --all --check
```

egui／egui-winit／egui-ime／egui-wgpuのlib testsはwindow/EventLoopを作らない。
eframeの実window試験1件はignoreされており、上記commandでは実行しない。--ignoredを付けない。
egui-wgpuのGPU回帰はoffscreen DX12を使用する。
wgpu固有の旧診断はruntime設定であり、架空の診断featureは追加しない。診断構成では既存の
egui/miv-repaint-witnessを併用し、capture probeのoff/onはGPU回帰内で検査する。
egui/egui-winitの同梱Cargo.lockは原本記録であり、
実行には準備workspaceの`vendor-next/Cargo.lock`を使う。rootのfmt対象には含まれないので、
準備graphのfmtも明示実行する。egui-imeを製品側へ一本化する段3の手順は同crateのPATCHESを参照。
検証結果・未実施・原本と差分の境目は[台帳](../docs/egui-0.37-stage2-verification.md)に記録する。

egui-wgpu原本だけの追加取り込みは、同directoryが未配置の場合に
`python vendor-next/import-upstream.py egui-wgpu`で行う。既存のegui／egui-winit記録は保持する。
段2bの原本／差分適用後handoffは`target/egui037-stage2b-handoff/{original,patched}/`に保存する。
eframeも未配置時に`python vendor-next/import-upstream.py eframe`で追加できる。
段2c-1のhandoffは`target/egui037-stage2c1-handoff/{original,patched}/`。
