fn main() {
    // vendor アセットのチェックと winresource 埋め込みは **Windows ターゲット**の
    // ビルドでのみ行う。CI (ubuntu) では `cargo check --bin mimageviewer-core
    // --features portable` を回して #[cfg(windows)] 宣言の unguarded 参照を検出する
    // が、そこでは vendor/ が存在しない (portable feature により include_bytes! の
    // vendor 参照もコンパイルされない)。
    // 注意: build.rs 自体は HOST 用にコンパイルされるため、#[cfg(target_os)] は
    // クロスコンパイル時にターゲット判定として機能しない。ターゲット側の
    // CARGO_CFG_TARGET_OS 環境変数で実行時判定する。
    let target_is_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");

    // native presenter のシェーダは **すべてビルド時に FXC で DXBC 化する**。
    //
    // 以前は NIS / grade / resample を `NativeRenderCore::new` の中で `D3DCompile` して
    // いた。この constructor は動画を開くたび、さらに **F12 の placement 切替のたび**に
    // 丸ごと走るため、compile コストがそのまま体感遅延になっていた。実測で NIS だけが
    // **2138ms** (他 6 本は合計 28ms)。1 往復あたり 2 秒以上を毎回払っていたことになる
    // (backlog §1.122)。Anime4K は最初からこの経路なので、残りを揃えただけ。
    let fxc = target_is_windows
        .then(find_fxc)
        .transpose()
        .unwrap_or_else(|error| panic!("{error}"));
    generate_video_nis_hlsl(fxc.as_deref());
    compile_video_presenter_shaders(fxc.as_deref());
    generate_video_anime4k_shaders(fxc.as_deref());

    // 先に vendor ファイルの存在チェックをして、cryptic な include_bytes! エラーに
    // 代わって復旧手順付きの明確なメッセージを出す。
    if target_is_windows {
        check_vendor_files();
    }

    // 絵文字スタンプ (Twemoji SVG) を exe に同梱するためのコード生成 (Inc 4c)。
    generate_emoji_assets();

    // mIV オリジナルの注釈スタンプは Twemoji 更新処理と独立したテーブルへ同梱する。
    generate_annotation_stamp_assets();

    // 同梱 FFmpeg のビルド識別子を焼き込む (バージョン情報ダイアログの LGPL 通知)。
    emit_ffmpeg_build_id();

    // FFmpeg DLL は `target/release/` には自動でコピーしない。
    //
    // 本体 (`mimageviewer-core.exe`) は配布物としては直接使われず、ランチャー
    // (`crates/launcher/`、`mimageviewer.exe` を生成) が `include_bytes!` で
    // core と FFmpeg DLL 6 つ (avcodec / avformat / avutil / avfilter / swscale /
    // swresample) を内包し、起動時に `%APPDATA%\mimageviewer\runtime\<v>\`
    // に展開して core を spawn する。
    //
    // 開発時に直接 `target/release/mimageviewer-core.exe` を実行したいときは、
    // `vendor/ffmpeg/bin/*.dll` を手動で同じディレクトリにコピーすること。
    // (もしくは PowerShell で:
    //   `Copy-Item vendor/ffmpeg/bin/*.dll target/release/`)
    //
    // 経緯: 当初 `include_bytes!` → APPDATA 展開で本体に直接埋め込もうとしたが、
    // `ffmpeg-the-third` は MSVC import library 経由なので Windows ローダが
    // exe ロード時点 (Rust コードが走るより前) に DLL を解決する必要があり間に
    // 合わない。`/DELAYLOAD` も rustc 経由の link.exe で Delay Import Directory が
    // 空のまま生成される問題があり機能せず、最終的にランチャー方式に切り替えた。

    // ↑と同じく Windows ターゲット限定 (cfg は HOST 判定なので実行時ガードも併用)。
    #[cfg(target_os = "windows")]
    if target_is_windows {
        // VS_FIXEDFILEINFO は winresource が CARGO_PKG_VERSION_{MAJOR,MINOR,PATCH}
        // から自動で 0.9.0.0 を埋める。文字列版 (ファイルプロパティに表示される) も
        // 揃えるため CARGO_PKG_VERSION から導出する (以前は "0.1.0.0" 決め打ちで
        // クラッシュダンプ / サポート画面で「v0.1.0.0」と表示されていた)。
        let version_str = format!("{}.0", env!("CARGO_PKG_VERSION"));
        let mut res = winresource::WindowsResource::new();
        res.set("FileDescription", "mImageViewer");
        res.set("ProductName", "mImageViewer");
        res.set("FileVersion", &version_str);
        res.set("ProductVersion", &version_str);
        res.set("LegalCopyright", "Copyright (C) 2026 Mikage Sawatari");
        // 本バイナリは内部実体 `mimageviewer-core.exe` であり、配布する
        // `mimageviewer.exe` は別途 launcher が生成する。OriginalFilename は
        // PE リソースのファイル名識別なので core 側はそれに合わせる。
        res.set("OriginalFilename", "mimageviewer-core.exe");

        // アイコンファイルが存在する場合のみ埋め込む
        if std::path::Path::new("assets/icon.ico").exists() {
            res.set_icon("assets/icon.ico");
        }

        if let Err(e) = res.compile() {
            eprintln!("winresource compile error: {e}");
        }
    }
}

/// Convert the canonical WGSL NIS shader to Shader Model 5 HLSL, then to DXBC.
///
/// `gpu_nis.wgsl` remains the single source for the NIS coefficients and algorithm.
/// Native video consumes only this generated output; it does not carry a second hand port.
/// The HLSL file is FXC's input; the presenter only ever sees the `.cso` (backlog §1.122).
fn generate_video_nis_hlsl(fxc: Option<&std::path::Path>) {
    use naga::ShaderStage;

    const SOURCE_PATH: &str = "src/gpu_nis.wgsl";
    println!("cargo:rerun-if-changed={SOURCE_PATH}");
    let source = std::fs::read_to_string(SOURCE_PATH)
        .unwrap_or_else(|error| panic!("read {SOURCE_PATH}: {error}"));
    let (module, hlsl_entry_names, output) = convert_wgsl_to_hlsl(SOURCE_PATH, &source, None);

    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let output_path = out_dir.join("video_nis.hlsl");
    std::fs::write(&output_path, output)
        .unwrap_or_else(|error| panic!("write {}: {error}", output_path.display()));

    // naga は entry point 名を書き換えることがあるので、WGSL 側の名前ではなく
    // reflection が返した HLSL 側の名前で FXC を呼ぶ (Anime4K と同じ扱い)。
    let (entry_index, _) = module
        .entry_points
        .iter()
        .enumerate()
        .find(|(_, entry)| entry.stage == ShaderStage::Fragment && entry.name == "fs_nis")
        .unwrap_or_else(|| panic!("{SOURCE_PATH} has no fs_nis fragment entry point"));
    if let Some(fxc) = fxc {
        compile_hlsl_with_fxc(
            fxc,
            &output_path,
            &hlsl_entry_names[entry_index],
            "ps_5_0",
            &out_dir.join("video_nis_fs_nis.cso"),
        );
    }
}

/// grade / resample の手書き HLSL をビルド時に DXBC 化する。
///
/// これらは `NativeRenderCore::new` から `D3DCompile` されていたが、同 constructor は
/// placement 切替のたびに走る。`.cso` にしておけば `CreatePixelShader` を呼ぶだけになる
/// (backlog §1.122)。
fn compile_video_presenter_shaders(fxc: Option<&std::path::Path>) {
    const SHADERS: &[(&str, &[(&str, &str)])] = &[
        (
            "src/video/native_presenter/shaders/video_grade.hlsl",
            &[("vs_main", "vs_5_0"), ("ps_main", "ps_5_0")],
        ),
        (
            "src/video/native_presenter/shaders/video_resample.hlsl",
            &[
                ("vs_main", "vs_5_0"),
                ("ps_horizontal", "ps_5_0"),
                ("ps_vertical", "ps_5_0"),
                ("ps_nearest", "ps_5_0"),
            ],
        ),
        (
            "src/video/native_presenter/shaders/video_panorama.hlsl",
            &[
                ("vs_main", "vs_5_0"),
                ("ps_orient", "ps_5_0"),
                ("ps_main", "ps_5_0"),
            ],
        ),
    ];

    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    for (source_path, entries) in SHADERS {
        println!("cargo:rerun-if-changed={source_path}");
        let stem = std::path::Path::new(source_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_else(|| panic!("{source_path} has no file stem"));
        for (entry, target) in *entries {
            let output = out_dir.join(format!("{stem}_{entry}.cso"));
            if let Some(fxc) = fxc {
                compile_hlsl_with_fxc(
                    fxc,
                    std::path::Path::new(source_path),
                    entry,
                    target,
                    &output,
                );
            }
        }
    }
}

fn convert_wgsl_to_hlsl(
    source_path: &str,
    source: &str,
    uniform_register: Option<u32>,
) -> (naga::Module, Vec<String>, String) {
    use naga::back::hlsl;
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    let module = naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("parse {source_path}: {error}"));
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|error| panic!("validate {source_path}: {error}"));
    let mut options = hlsl::Options {
        shader_model: hlsl::ShaderModel::V5_0,
        ..Default::default()
    };
    if let Some(register) = uniform_register {
        for (_, global) in module.global_variables.iter() {
            if global.space != naga::AddressSpace::Uniform {
                continue;
            }
            let binding = global
                .binding
                .clone()
                .unwrap_or_else(|| panic!("{source_path} has an unbound uniform"));
            options.binding_map.insert(
                binding,
                hlsl::BindTarget {
                    register,
                    ..Default::default()
                },
            );
        }
    }
    let pipeline_options = hlsl::PipelineOptions::default();
    let mut output = String::new();
    let reflection = hlsl::Writer::new(&mut output, &options, &pipeline_options)
        .write(&module, &info, None)
        .unwrap_or_else(|error| panic!("convert {source_path} to HLSL: {error}"));
    let entry_point_names = reflection
        .entry_point_names
        .into_iter()
        .map(|name| {
            name.unwrap_or_else(|error| {
                panic!("translate an entry point from {source_path} to HLSL: {error}")
            })
        })
        .collect();
    (module, entry_point_names, output)
}

struct Anime4kBuildVariant {
    rust_variant: &'static str,
    suffix: &'static str,
    source_path: &'static str,
}

const ANIME4K_BUILD_VARIANTS: &[Anime4kBuildVariant] = &[
    Anime4kBuildVariant {
        rust_variant: "Small",
        suffix: "s",
        source_path: "src/gpu_anime4k_s.wgsl",
    },
    Anime4kBuildVariant {
        rust_variant: "Medium",
        suffix: "m",
        source_path: "src/gpu_anime4k_m.wgsl",
    },
    Anime4kBuildVariant {
        rust_variant: "Large",
        suffix: "l",
        source_path: "src/gpu_anime4k_l.wgsl",
    },
    Anime4kBuildVariant {
        rust_variant: "VeryLarge",
        suffix: "vl",
        source_path: "src/gpu_anime4k.wgsl",
    },
    Anime4kBuildVariant {
        rust_variant: "UltraLarge",
        suffix: "ul",
        source_path: "src/gpu_anime4k_ul.wgsl",
    },
];

fn generate_video_anime4k_shaders(fxc: Option<&std::path::Path>) {
    use naga::ShaderStage;
    use std::fmt::Write as _;

    let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut generated_table = String::from(
        "// Generated by build.rs from the generated Anime4K WGSL entry points.\n\
         // Do not edit this file directly.\n\n\
         const VIDEO_ANIME4K_BYTECODE_VARIANTS: &[VideoAnime4kBytecodeVariant] = &[\n",
    );

    for variant in ANIME4K_BUILD_VARIANTS {
        println!("cargo:rerun-if-changed={}", variant.source_path);
        let source = std::fs::read_to_string(variant.source_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", variant.source_path));
        let (module, hlsl_entry_names, hlsl) =
            convert_wgsl_to_hlsl(variant.source_path, &source, Some(0));
        let hlsl_path = out_dir.join(format!("video_anime4k_{}.hlsl", variant.suffix));
        std::fs::write(&hlsl_path, hlsl)
            .unwrap_or_else(|error| panic!("write {}: {error}", hlsl_path.display()));

        let mut convolution_entries = module
            .entry_points
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.stage == ShaderStage::Fragment)
            .filter_map(|(entry_index, entry)| {
                entry
                    .name
                    .strip_prefix("fs_anime4k_")
                    .and_then(|suffix| suffix.parse::<usize>().ok())
                    .map(|index| (index, hlsl_entry_names[entry_index].as_str()))
            })
            .collect::<Vec<_>>();
        convolution_entries.sort_unstable_by_key(|(index, _)| *index);
        for (expected, (actual, _)) in convolution_entries.iter().enumerate() {
            assert_eq!(
                *actual, expected,
                "{} has a missing or duplicate Anime4K pass before {actual}",
                variant.source_path
            );
        }
        let resolve_entry = module
            .entry_points
            .iter()
            .enumerate()
            .find(|(_, entry)| {
                entry.stage == ShaderStage::Fragment && entry.name == "fs_anime4k_resolve"
            })
            .unwrap_or_else(|| panic!("{} has no fs_anime4k_resolve", variant.source_path));
        assert!(
            !convolution_entries.is_empty(),
            "{} has no Anime4K convolution passes",
            variant.source_path
        );

        writeln!(
            generated_table,
            "    VideoAnime4kBytecodeVariant {{\n        variant: Anime4kVariant::{},\n        convolution: &[",
            variant.rust_variant
        )
        .expect("write Anime4K bytecode table");
        for (index, entry) in &convolution_entries {
            let output_name = format!("video_anime4k_{}_{}.cso", variant.suffix, index);
            if let Some(fxc) = fxc {
                compile_hlsl_with_fxc(
                    fxc,
                    &hlsl_path,
                    entry,
                    "ps_5_0",
                    &out_dir.join(&output_name),
                );
            }
            writeln!(
                generated_table,
                "            include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{output_name}\")),"
            )
            .expect("write Anime4K convolution bytecode table");
        }
        let resolve_name = format!("video_anime4k_{}_resolve.cso", variant.suffix);
        if let Some(fxc) = fxc {
            compile_hlsl_with_fxc(
                fxc,
                &hlsl_path,
                &hlsl_entry_names[resolve_entry.0],
                "ps_5_0",
                &out_dir.join(&resolve_name),
            );
        }
        writeln!(
            generated_table,
            "        ],\n        resolve: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{resolve_name}\")),\n    }},"
        )
        .expect("write Anime4K resolve bytecode table");
    }
    generated_table.push_str("];\n");
    std::fs::write(out_dir.join("video_anime4k_bytecode.rs"), generated_table)
        .expect("write video_anime4k_bytecode.rs");
}

fn find_fxc() -> Result<std::path::PathBuf, String> {
    println!("cargo:rerun-if-env-changed=MIV_FXC_PATH");
    if let Some(explicit) = std::env::var_os("MIV_FXC_PATH") {
        let path = std::path::PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "MIV_FXC_PATH points to a missing file: {}\n\
             Recovery: set MIV_FXC_PATH to the x64 fxc.exe installed by the Windows SDK.",
            path.display()
        ));
    }

    let mut candidates = Vec::new();
    if let Some(version_bin) = std::env::var_os("WindowsSdkVerBinPath") {
        candidates.push(std::path::PathBuf::from(version_bin).join("x64/fxc.exe"));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles(x86)") {
        let bin_root = std::path::PathBuf::from(program_files).join("Windows Kits/10/bin");
        if let Ok(entries) = std::fs::read_dir(&bin_root) {
            let mut versioned = entries
                .flatten()
                .map(|entry| entry.path().join("x64/fxc.exe"))
                .collect::<Vec<_>>();
            versioned.sort_unstable_by(|left, right| right.cmp(left));
            candidates.extend(versioned);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("fxc.exe")));
    }
    if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
        return Ok(path);
    }

    Err(
        "Native presenter shader bytecode (NIS / grade / resample / Anime4K) requires the Windows SDK\n\
         FXC compiler, but fxc.exe was not found.\n\
         Recovery: install the Windows 10/11 SDK Desktop C++ tools, or set MIV_FXC_PATH to its x64\\fxc.exe.\n\
         Typical location: C:\\Program Files (x86)\\Windows Kits\\10\\bin\\<sdk-version>\\x64\\fxc.exe"
            .to_string(),
    )
}

fn compile_hlsl_with_fxc(
    fxc: &std::path::Path,
    source: &std::path::Path,
    entry: &str,
    target: &str,
    output: &std::path::Path,
) {
    let result = std::process::Command::new(fxc)
        .args(["/nologo", "/T", target, "/E", entry, "/O3", "/Fo"])
        .arg(output)
        .arg(source)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run FXC at {} for {entry}: {error}\n\
                 Recovery: install the Windows SDK or set MIV_FXC_PATH to its x64\\fxc.exe.",
                fxc.display()
            )
        });
    if !result.status.success() {
        panic!(
            "FXC failed for {} entry {entry} (status {}):\n{}{}\n\
             Recovery: verify the generated WGSL/HLSL and that MIV_FXC_PATH names an x64 Windows SDK fxc.exe.",
            source.display(),
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        );
    }
}

/// 同梱 FFmpeg のビルド識別子を `MIV_FFMPEG_BUILD_ID` として埋め込む。
///
/// `docs/ffmpeg-lgpl-source-distribution.md` の Notice Template は、ソフトウェア情報へ
/// FFmpeg のバージョンと対応ソース URL を出すことを求めている。BtbN はタグではなく
/// commit からビルドするので、この識別子 (`n7.1.5-10-g2aefd64d48` 形式) が
/// DLL の ProductVersion・`vendor/ffmpeg/VERSION`・配布するソース tarball 名の 3 つを
/// 突き合わせる唯一の鍵になる。ここで焼き込むことで、FFmpeg を更新しても UI 側の
/// 文字列を手で直す必要がなくなる (直し忘れが LGPL 通知の齟齬に直結するため)。
fn emit_ffmpeg_build_id() {
    println!("cargo:rerun-if-changed=vendor/ffmpeg/VERSION");
    let id = std::fs::read_to_string("vendor/ffmpeg/VERSION")
        .ok()
        .and_then(|raw| parse_ffmpeg_build_id(raw.trim()))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=MIV_FFMPEG_BUILD_ID={id}");
}

/// BtbN の資産名からビルド識別子を取り出す。
///
/// 入力例: `ffmpeg-n7.1.5-10-g2aefd64d48-win64-lgpl-shared-7.1.zip`
/// 戻り値: `n7.1.5-10-g2aefd64d48`
///
/// 資産名の形式が変わって解釈できない場合は `None` を返し、呼び出し側が `unknown` へ
/// 落とす。ここでビルドを失敗させないのは、非 Windows の CI チェックなど vendor が
/// 揃わない構成でもコンパイル自体は通す必要があるため。
fn parse_ffmpeg_build_id(asset: &str) -> Option<String> {
    let rest = asset.strip_prefix("ffmpeg-")?;
    let end = rest.find("-win64-")?;
    if end == 0 {
        return None;
    }
    Some(rest[..end].to_string())
}

/// `include_bytes!` で埋め込む vendor ファイル (PDFium / ONNX Runtime / Susie 32bit ワーカー /
/// FFmpeg LGPL DLL / AI モデル) が揃っているかをビルド前にチェックし、欠落時は復旧用
/// セットアップ手順を含む明確なエラーメッセージで終了する。
fn check_vendor_files() {
    // (path, setup script / 取得方法)
    let mut required: Vec<(&str, &str)> = vec![
        (
            "vendor/pdfium/bin/pdfium.dll",
            "bash scripts/setup-pdfium.sh",
        ),
        ("vendor/ort/onnxruntime.dll", "bash scripts/setup-ort.sh"),
        (
            "vendor/ort/onnxruntime_providers_shared.dll",
            "bash scripts/setup-ort.sh",
        ),
        (
            "vendor/susie-worker/mimageviewer-susie32.exe",
            "bash scripts/setup-susie-worker.sh",
        ),
        // FFmpeg 7.x LGPL shared build (BtbN ffmpeg-n7.1*-win64-lgpl-shared)
        // バージョンを上げる (8.x 等) と DLL のメジャー番号が変わるため、ここと
        // src/video/ffmpeg_loader.rs の include_bytes! パスを揃えて更新すること。
        (
            "vendor/ffmpeg/bin/avcodec-61.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/avformat-61.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/avutil-59.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/avfilter-10.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/swscale-8.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/bin/swresample-5.dll",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avcodec.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avformat.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avutil.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/avfilter.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/swscale.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        (
            "vendor/ffmpeg/lib/swresample.lib",
            "bash scripts/setup-ffmpeg.sh",
        ),
        // AI モデルは配布スクリプトが無いので、既存インストール済み環境からコピーする旨を案内。
        // ここに並べるのは `src/ai/model_manager.rs` の EMBEDDED_MODELS と
        // `scripts/build-portable.ps1` の $models に一致させること
        // (= 旧 anime_classifier_mobilenetv3.onnx は現在どちらからも外れているので要求しない)。
        (
            "vendor/models/realesrgan_x4plus.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/realesrgan_x4plus_anime_6b.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/realesr_general_x4v3.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/realcugan_4x_conservative.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/4x_NMKD-Siax_200k.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/dejpg_realplksr_otf.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
        (
            "vendor/models/migan.onnx",
            "インストール済み環境の %APPDATA%\\mimageviewer\\models\\ からコピー",
        ),
    ];

    // VST3 host bridge プロセスは **通常ビルド (インストーラ / 単体 exe) のみ**
    // `include_bytes!` で本体 (`mimageviewer-core.exe`) に内包する。CMake で
    // `crates/vst3-host/` をビルドするとここに配置され、初回 VST3 enable 時に
    // `%APPDATA%\mimageviewer\vst3\` へ展開する (PDFium / Susie ワーカーと同パターン)。
    //
    // ポータブルビルド (feature = "portable") では、未署名 exe が一部のセキュリティ
    // ソフトに誤検知され zip ダウンロードがブロックされる事象を避けるため **同梱しない**
    // (`scripts/build-portable.ps1` も copy 対象外、`vst3_supported()` が false を返して
    // VST3 機能を自動無効化する)。よって portable では必須ファイルにしない。
    // 将来 exe をコード署名して再同梱する場合は build-portable.ps1 と合わせて復活させる。
    if std::env::var_os("CARGO_FEATURE_PORTABLE").is_none() {
        required.push((
            "vendor/vst3-host/mimageviewer-vst3-host.exe",
            "cmake -S crates/vst3-host -B crates/vst3-host/build -G \"Visual Studio 18 2026\" -A x64 && cmake --build crates/vst3-host/build --config Release",
        ));
    }

    let missing: Vec<&(&str, &str)> = required
        .iter()
        .filter(|(p, _)| !std::path::Path::new(p).exists())
        .collect();

    if missing.is_empty() {
        // 変更されたら再チェックするよう cargo に通知
        for (p, _) in &required {
            println!("cargo:rerun-if-changed={p}");
        }
        return;
    }

    eprintln!();
    eprintln!("================================================================");
    eprintln!(" vendor ファイルが不足しています (include_bytes! で必要)");
    eprintln!("================================================================");
    for (p, how) in &missing {
        eprintln!("  - {p}");
        eprintln!("      → {how}");
    }
    eprintln!();
    eprintln!("不足分をまとめて取得する場合:");
    eprintln!("  bash scripts/bootstrap-vendor.sh");
    eprintln!();
    eprintln!("(vst3-host exe は SDK が ~490 MB なので bootstrap には含まれない。");
    eprintln!(" 別 worktree やバックアップにビルド済み exe があればコピー、");
    eprintln!(" 無ければ CLAUDE.md「VST3 host bridge 管理」節の cmake 手順を実行)");
    eprintln!();
    eprintln!("詳細は CLAUDE.md の「vendor/ 一括セットアップ」節を参照してください。");
    eprintln!("================================================================");
    eprintln!();
    std::process::exit(1);
}

/// `vendor/twemoji/svg/*.svg` をスキャンし、ファイル名 (= emoji キー) → SVG バイト列の
/// 配列を `$OUT_DIR/emoji_svgs.rs` へ生成する。`src/comic_stamp.rs` が `include!` で
/// 取り込み、`include_bytes!` で各 SVG を exe に埋め込む (絵文字スタンプの同梱、Inc 4c)。
///
/// アセット未配置 (fresh clone で `scripts/setup-twemoji.sh` 未実行) の場合は空配列を
/// 生成し、`cargo:warning` で取得方法を案内する。dev ビルドはブロックしない
/// (= スタンプはアセット導入後に有効化される)。
fn generate_emoji_assets() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = std::path::Path::new(&out_dir).join("emoji_svgs.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let svg_dir = std::path::Path::new(&manifest)
        .join("vendor")
        .join("twemoji")
        .join("svg");
    println!("cargo:rerun-if-changed=vendor/twemoji/svg");

    // (key, abs_path) を収集。キーは Twemoji 命名 (小文字 16 進 + '-') のみ許可。
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&svg_dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|s| s.to_str()) != Some("svg") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                continue;
            }
            // svg_dir は manifest 由来の絶対パスなので verbatim (\\?\) にはならない。
            // include_bytes! は Windows でも forward slash を受けるので '/' に正規化する。
            let abs = path.to_string_lossy().replace('\\', "/");
            entries.push((stem.to_string(), abs));
        }
    }
    entries.sort();

    let mut src = String::new();
    src.push_str("// @generated by build.rs from vendor/twemoji/svg/*.svg — do not edit.\n");
    src.push_str("pub static EMOJI_SVGS: &[(&str, &[u8])] = &[\n");
    for (key, abs) in &entries {
        src.push_str(&format!("    ({key:?}, include_bytes!({abs:?})),\n"));
    }
    src.push_str("];\n");
    std::fs::write(&dest, &src).expect("write emoji_svgs.rs");

    if entries.is_empty() {
        println!(
            "cargo:warning=絵文字スタンプ用アセットが未配置です (vendor/twemoji/svg)。\
             `bash scripts/setup-twemoji.sh` で取得するとスタンプが exe に同梱されます。"
        );
    }
}

fn generate_annotation_stamp_assets() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = std::path::Path::new(&out_dir).join("annotation_stamp_svgs.rs");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let svg_dir = std::path::Path::new(&manifest)
        .join("assets")
        .join("annotation-stamps");
    println!("cargo:rerun-if-changed=assets/annotation-stamps");

    let mut entries: Vec<(String, String)> = Vec::new();
    collect_annotation_stamp_assets(&svg_dir, &mut entries);
    entries.sort();
    write_annotation_stamp_table(&dest, &entries);
}

fn collect_annotation_stamp_assets(svg_dir: &std::path::Path, entries: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(svg_dir) else {
        return;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("svg") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.is_empty()
            || !stem
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            continue;
        }
        let key = format!("miv:{stem}");
        let abs = path.to_string_lossy().replace('\\', "/");
        entries.push((key, abs));
    }
}

fn write_annotation_stamp_table(dest: &std::path::Path, entries: &[(String, String)]) {
    let mut src = String::new();
    src.push_str("// @generated by build.rs from assets/annotation-stamps/*.svg — do not edit.\n");
    src.push_str("pub static ANNOTATION_STAMP_SVGS: &[(&str, &[u8])] = &[\n");
    for (key, abs) in entries {
        src.push_str(&format!("    ({key:?}, include_bytes!({abs:?})),\n"));
    }
    src.push_str("];\n");
    std::fs::write(dest, &src).expect("write annotation_stamp_svgs.rs");
}
