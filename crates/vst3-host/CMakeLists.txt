## mimageviewer-vst3-host
##
## C++ で書かれた VST3 プラグインホスト bridge プロセス。
## メインの mimageviewer.exe (Rust) から子プロセスとして起動され、
## stdin/stdout の制御プロトコルと shared memory 経由で音声データを受け渡す。
##
## ビルド前提:
##   - bash scripts/setup-vst3-sdk.sh で vendor/vst3sdk/ に SDK 3.8.0+ を配置
##   - CMake 3.20+ (PDFium / FFmpeg と同じく Visual Studio 同梱で OK)
##   - MSVC 2026 (Visual Studio 18 BuildTools, C++ デスクトップ開発ワークロード)
##
## ビルド方法:
##   cd crates/vst3-host
##   cmake -B build -G "Visual Studio 18 2026" -A x64
##   cmake --build build --config Release
##
## 出力:
##   vendor/vst3-host/mimageviewer-vst3-host.exe (リポジトリ root から相対)
##   vendor/vst3-host/vcrt/ に公式 VC runtime 4 DLL を毎回配置
##
## この exe は最終的に Rust 本体に include_bytes! で埋め込まれ、
## %APPDATA%\mimageviewer\ に展開されて子プロセスとして起動される
## (PDFium / Susie ワーカーと同パターン)。

cmake_minimum_required(VERSION 3.20)
project(mimageviewer-vst3-host CXX)

# Canonical build identity, shared with scripts/vst3-host-identity.ps1.
# Each sorted slash-relative name contributes name:lowercase-SHA256(raw-bytes)\n.
# The aggregate UTF-8 (no BOM) text is SHA256 hashed. Generated headers, SDK
# downloads and binaries are excluded. Current source paths are ASCII, so CMake's
# case-sensitive lexicographic sorting is the same as ordinal sorting.
file(GLOB_RECURSE HOST_IDENTITY_FILES CONFIGURE_DEPENDS
    RELATIVE "${CMAKE_CURRENT_SOURCE_DIR}"
    "${CMAKE_CURRENT_SOURCE_DIR}/include/*.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.cpp"
    "${CMAKE_CURRENT_SOURCE_DIR}/src/*.h"
    "${CMAKE_CURRENT_SOURCE_DIR}/tests/*.cpp")
list(APPEND HOST_IDENTITY_FILES "CMakeLists.txt")
list(SORT HOST_IDENTITY_FILES)
set(HOST_IDENTITY_TEXT "")
foreach(HOST_IDENTITY_FILE IN LISTS HOST_IDENTITY_FILES)
    file(SHA256 "${CMAKE_CURRENT_SOURCE_DIR}/${HOST_IDENTITY_FILE}" HOST_FILE_SHA256)
    string(TOLOWER "${HOST_FILE_SHA256}" HOST_FILE_SHA256)
    string(APPEND HOST_IDENTITY_TEXT "${HOST_IDENTITY_FILE}:${HOST_FILE_SHA256}\n")
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        "${CMAKE_CURRENT_SOURCE_DIR}/${HOST_IDENTITY_FILE}")
endforeach()
string(SHA256 HOST_SOURCE_SHA256 "${HOST_IDENTITY_TEXT}")
file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/generated")
file(CONFIGURE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/generated/build_identity.h"
    CONTENT "#pragma once\n#define MIV_VST3_HOST_SOURCE_MARKER \"MIV_VST3_HOST_SOURCE_SHA256:@HOST_SOURCE_SHA256@\"\n"
    @ONLY)

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
# Headless visibility/lifecycle regressions are checked at compile time.
add_library(vst3-gui-visibility-tests OBJECT tests/gui_visibility.cpp)
target_include_directories(vst3-gui-visibility-tests PRIVATE include)

add_executable(mimageviewer-vst3-host
    src/main.cpp
    src/host_app.cpp
    src/plugin_loader.cpp
    src/audio_pipe.cpp
    src/protocol.cpp
    src/vcrt_preload.cpp
)

target_include_directories(mimageviewer-vst3-host PRIVATE
    include
    "${CMAKE_CURRENT_BINARY_DIR}/generated"
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
    "${CMAKE_CURRENT_SOURCE_DIR}/src/sdk/module_win32.cpp"
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
        version     # Read all four CRT DLL file versions before selecting a set
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
# Direct bridge consumers (handler tests and the VST tester) launch this vendor
# exe rather than the Rust-extracted copy. Stage its required official CRT set on
# every build, including when the exe itself is up to date. POST_BUILD would miss
# that case. Keep the DLLs below vcrt/, outside the executable's default search.
set(VCRT_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../vendor/vcrt")
set(VCRT_FILES vcruntime140.dll vcruntime140_1.dll msvcp140.dll msvcp140_1.dll)
set(VCRT_SOURCES "")
foreach(VCRT_FILE IN LISTS VCRT_FILES)
    set(VCRT_SOURCE "${VCRT_SOURCE_DIR}/${VCRT_FILE}")
    if(NOT EXISTS "${VCRT_SOURCE}" OR IS_DIRECTORY "${VCRT_SOURCE}")
        message(FATAL_ERROR
            "Required official VC runtime is missing: ${VCRT_SOURCE}\n"
            "Restore all four official x64 DLLs from the licensed VC/Redist source recorded in\n"
            "vendor/vcrt/provenance.json (see vendor/vcrt/README.md), then configure again.")
    endif()
    list(APPEND VCRT_SOURCES "${VCRT_SOURCE}")
endforeach()
add_custom_target(vst3-host-stage-vcrt ALL
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${OUTPUT_DIR}/vcrt"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different ${VCRT_SOURCES} "${OUTPUT_DIR}/vcrt"
    COMMENT "Stage the official VC runtime set for direct VST3 bridge consumers"
    VERBATIM)
add_dependencies(mimageviewer-vst3-host vst3-host-stage-vcrt)
set_target_properties(mimageviewer-vst3-host PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY         "${OUTPUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY_DEBUG   "${OUTPUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${OUTPUT_DIR}"
)

# Pure UTF-8/Windows-path and CRT-policy test executable. It cannot load plugins
# or construct the product host; release verification may run it separately.
add_executable(vst3-path-tests tests/path_boundaries.cpp)
target_include_directories(vst3-path-tests PRIVATE include)
if(MSVC)
    target_compile_options(vst3-path-tests PRIVATE /utf-8 /W4 /permissive-)
    target_compile_definitions(vst3-path-tests PRIVATE NOMINMAX WIN32_LEAN_AND_MEAN)
    set_property(TARGET vst3-path-tests PROPERTY MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()
