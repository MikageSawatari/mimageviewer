## mimageviewer-vst3-host
##
## C++ で書かれた VST3 プラグインホスト bridge プロセス。
## メインの mimageviewer.exe (Rust) から子プロセスとして起動され、
## stdin/stdout の制御プロトコルと shared memory 経由で音声データを受け渡す。
##
## ビルド前提:
##   - bash scripts/setup-vst3-sdk.sh で vendor/vst3sdk/ に SDK 3.8.0+ を配置
##   - CMake 3.20+ (PDFium / FFmpeg と同じく Visual Studio 同梱で OK)
##   - MSVC 2022 (Visual Studio C++ デスクトップ開発ワークロード)
##
## ビルド方法:
##   cd crates/vst3-host
##   cmake -B build -G "Visual Studio 17 2022" -A x64
##   cmake --build build --config Release
##
## 出力:
##   build/Release/mimageviewer-vst3-host.exe
##
## この exe は最終的に Rust 本体に include_bytes! で埋め込まれ、
## %APPDATA%\mimageviewer\ に展開されて子プロセスとして起動される
## (PDFium / Susie ワーカーと同パターン)。

cmake_minimum_required(VERSION 3.20)
project(mimageviewer-vst3-host CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# VST3 SDK のパス (リポジトリ root から相対)
get_filename_component(VST3_SDK_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/vst3sdk" ABSOLUTE)

if(NOT EXISTS "${VST3_SDK_ROOT}/LICENSE.txt")
    message(FATAL_ERROR
        "VST3 SDK が ${VST3_SDK_ROOT} に見つかりません。\n"
        "リポジトリ root で以下を実行してください:\n"
        "  bash scripts/setup-vst3-sdk.sh")
endif()

# VST3 SDK のヘッダパス
include_directories(
    "${VST3_SDK_ROOT}"
    "${VST3_SDK_ROOT}/pluginterfaces"
    "${VST3_SDK_ROOT}/public.sdk/source"
)

# bridge ソース
add_executable(mimageviewer-vst3-host
    src/main.cpp
    src/host_app.cpp
    src/plugin_loader.cpp
    src/audio_pipe.cpp
    src/protocol.cpp
)

target_include_directories(mimageviewer-vst3-host PRIVATE
    include
)

# VST3 SDK の必要最小限のソースを取り込む
# (公式が提供する base / pluginterfaces / public.sdk から host に必要な分だけ)
target_sources(mimageviewer-vst3-host PRIVATE
    "${VST3_SDK_ROOT}/pluginterfaces/base/coreiids.cpp"
    "${VST3_SDK_ROOT}/pluginterfaces/base/funknown.cpp"
    "${VST3_SDK_ROOT}/pluginterfaces/base/ustring.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/common/commoniids.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/common/commonstringconvert.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/common/memorystream.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/common/threadchecker_win32.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/connectionproxy.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/hostclasses.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/module.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/module_win32.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/plugprovider.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/pluginterfacesupport.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/parameterchanges.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/processdata.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/hosting/eventlist.cpp"
    # VST 関連 iid をまとめて実体化する SDK 公式ファイル (DEF_CLASS_IID 群)
    "${VST3_SDK_ROOT}/public.sdk/source/vst/vstinitiids.cpp"
    "${VST3_SDK_ROOT}/public.sdk/source/vst/utility/stringconvert.cpp"
)

if(WIN32)
    target_compile_definitions(mimageviewer-vst3-host PRIVATE
        UNICODE
        _UNICODE
        WIN32_LEAN_AND_MEAN
        NOMINMAX
        _CRT_SECURE_NO_WARNINGS
        # VST3 SDK は WINDOWS / SMTG_OS_WINDOWS 等を内部で参照
    )
    # Windows の Shell API / Console / 共有メモリ
    target_link_libraries(mimageviewer-vst3-host PRIVATE
        kernel32
        user32
        dwmapi
        ole32
        shell32
        comctl32    # SetWindowSubclass / RemoveWindowSubclass for plugin child HWND hooks
        advapi32
        avrt        # AvSetMmThreadCharacteristicsW (MMCSS)
    )
endif()

# Release ビルドで最適化と CRT 静的リンク (本体の +crt-static と整合)
if(MSVC)
    target_compile_options(mimageviewer-vst3-host PRIVATE
        /utf-8       # ソース・実行両方を UTF-8 として扱う (CP932 環境での文字化け防止)
        /W4
        /permissive-
        /wd4100  # unreferenced formal parameter (VST3 SDK で多発)
        /wd4127  # conditional expression is constant
        /wd4201  # nameless struct/union (VST3 SDK で多発)
    )
    # CRT 静的リンク: 配布時に VC++ 再頒布パッケージ依存を出さないため
    set_property(TARGET mimageviewer-vst3-host PROPERTY
        MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

# 出力ディレクトリは vendor/vst3-host/ にして、Rust 本体の include_bytes! で
# 拾えるようにする (PDFium / Susie ワーカーと同じパターン)。
set(OUTPUT_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/vst3-host")
set_target_properties(mimageviewer-vst3-host PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY         "${OUTPUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY_DEBUG   "${OUTPUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${OUTPUT_DIR}"
)
