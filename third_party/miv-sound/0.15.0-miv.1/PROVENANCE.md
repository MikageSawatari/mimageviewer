# mIV Sound provenance

Candidate: 4b7a3eba00d3946c591d103eb6adecfba42b6cae3ea3eb3f010958093bf8c0bc
Upstream tag: v0.15.0
Upstream commit: 8c9a0a561d2ee88d2eef6b06bab138bbd639b3c1
Fork commit: c5cd9fa5376f3e9446b032b994000ff492668bc7

Build provenance (from candidate.json):

{
    "identity":  {
                     "commit":  "c5cd9fa5376f3e9446b032b994000ff492668bc7",
                     "upstream_tag":  "v0.15.0",
                     "upstream_commit":  "8c9a0a561d2ee88d2eef6b06bab138bbd639b3c1",
                     "submodules":  [
                                        {
                                            "path":  "external/effetune",
                                            "commit":  "c506bbab8a3dcd75bc5049ebb9b92572b526c557"
                                        },
                                        {
                                            "path":  "third_party/choc",
                                            "commit":  "c0d3a1ba52af7b2a877675495973391808a7a768"
                                        }
                                    ]
                 },
    "preset":  "windows-release",
    "configure_command":  "cmake --preset windows-release \"-DCMAKE_CXX_FLAGS=/DWIN32 /D_WINDOWS /EHsc /wd4458\" -UFETCHCONTENT_FULLY_DISCONNECTED -DFETCHCONTENT_UPDATES_DISCONNECTED=ON || exit /b 2",
    "cmake_presets":  "{\n  \"version\": 6,\n  \"cmakeMinimumRequired\": {\n    \"major\": 3,\n    \"minor\": 24,\n    \"patch\": 0\n  },\n  \"configurePresets\": [\n    {\n      \"name\": \"windows-base\",\n      \"hidden\": true,\n      \"generator\": \"Ninja\",\n      \"binaryDir\": \"${sourceDir}/build/${presetName}\",\n      \"cacheVariables\": {\n        \"CMAKE_EXPORT_COMPILE_COMMANDS\": \"ON\",\n        \"EFFETUNE_BUILD_PLUGIN\": \"ON\",\n        \"EFFETUNE_BUILD_TESTS\": \"ON\",\n        \"EFFETUNE_BUILD_TOOLS\": \"ON\",\n        \"EFFETUNE_AUTOMATION_HOST_GATE_FIXTURE\": \"OFF\"\n      },\n      \"condition\": {\n        \"type\": \"equals\",\n        \"lhs\": \"${hostSystemName}\",\n        \"rhs\": \"Windows\"\n      }\n    },\n    {\n      \"name\": \"windows-debug\",\n      \"inherits\": \"windows-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Debug\"\n      }\n    },\n    {\n      \"name\": \"windows-release\",\n      \"inherits\": \"windows-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Release\"\n      }\n    },\n    {\n      \"name\": \"windows-automation-host-gate\",\n      \"inherits\": \"windows-base\",\n      \"binaryDir\": \"${sourceDir}/build/windows-automation-host-gate\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Release\",\n        \"EFFETUNE_AUTOMATION_HOST_GATE_FIXTURE\": \"ON\"\n      }\n    },\n    {\n      \"name\": \"mac-base\",\n      \"hidden\": true,\n      \"generator\": \"Ninja\",\n      \"binaryDir\": \"${sourceDir}/build/${presetName}\",\n      \"cacheVariables\": {\n        \"CMAKE_OSX_ARCHITECTURES\": \"arm64;x86_64\",\n        \"EFFETUNE_BUILD_PLUGIN\": \"ON\",\n        \"EFFETUNE_BUILD_TESTS\": \"ON\",\n        \"EFFETUNE_BUILD_TOOLS\": \"ON\"\n      },\n      \"condition\": {\n        \"type\": \"equals\",\n        \"lhs\": \"${hostSystemName}\",\n        \"rhs\": \"Darwin\"\n      }\n    },\n    {\n      \"name\": \"mac-debug\",\n      \"inherits\": \"mac-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Debug\"\n      }\n    },\n    {\n      \"name\": \"mac-release\",\n      \"inherits\": \"mac-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Release\"\n      }\n    }\n  ],\n  \"buildPresets\": [\n    { \"name\": \"windows-debug\", \"configurePreset\": \"windows-debug\" },\n    { \"name\": \"windows-release\", \"configurePreset\": \"windows-release\" },\n    {\n      \"name\": \"windows-automation-host-gate\",\n      \"configurePreset\": \"windows-automation-host-gate\"\n    },\n    { \"name\": \"mac-debug\", \"configurePreset\": \"mac-debug\" },\n    { \"name\": \"mac-release\", \"configurePreset\": \"mac-release\" }\n  ],\n  \"testPresets\": [\n    {\n      \"name\": \"windows-debug\",\n      \"configurePreset\": \"windows-debug\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"windows-release\",\n      \"configurePreset\": \"windows-release\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"windows-automation-host-gate\",\n      \"configurePreset\": \"windows-automation-host-gate\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"mac-debug\",\n      \"configurePreset\": \"mac-debug\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"mac-release\",\n      \"configurePreset\": \"mac-release\",\n      \"output\": { \"outputOnFailure\": true }\n    }\n  ]\n}\n",
    "cmake_cache":  "# This is the CMakeCache file.\r\n# For build in directory: g:/home/miv-sound/build/windows-release\r\n# It was generated by CMake: C:/Program Files/CMake/bin/cmake.exe\r\n# You can edit this file to change values found and used by cmake.\r\n# If you do not want to change any of the values, simply exit the editor.\r\n# If you do want to change a value, simply edit, save, and exit the editor.\r\n# The syntax for the file is as follows:\r\n# KEY:TYPE=VALUE\r\n# KEY is the name of a variable in the cache.\r\n# TYPE is a hint to GUIs for the type of VALUE, DO NOT EDIT TYPE!.\r\n# VALUE is the current value for the KEY.\r\n\r\n########################\r\n# EXTERNAL cache entries\r\n########################\r\n\r\n//Build the testing tree.\r\nBUILD_TESTING:BOOL=ON\r\n\r\n//Path to a program.\r\nCMAKE_AR:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/lib.exe\r\n\r\n//Choose the type of build, options are: None Debug Release RelWithDebInfo\r\n// MinSizeRel ...\r\nCMAKE_BUILD_TYPE:STRING=Release\r\n\r\n//CXX compiler\r\nCMAKE_CXX_COMPILER:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/cl.exe\r\n\r\n//Flags used by the CXX compiler during all build types.\r\nCMAKE_CXX_FLAGS:STRING=/DWIN32 /D_WINDOWS /EHsc /wd4458\r\n\r\n//Flags used by the CXX compiler during DEBUG builds.\r\nCMAKE_CXX_FLAGS_DEBUG:STRING=/Zi /Ob0 /Od /RTC1\r\n\r\n//Flags used by the CXX compiler during MINSIZEREL builds.\r\nCMAKE_CXX_FLAGS_MINSIZEREL:STRING=/O1 /Ob1 /DNDEBUG\r\n\r\n//Flags used by the CXX compiler during RELEASE builds.\r\nCMAKE_CXX_FLAGS_RELEASE:STRING=/O2 /Ob2 /DNDEBUG\r\n\r\n//Flags used by the CXX compiler during RELWITHDEBINFO builds.\r\nCMAKE_CXX_FLAGS_RELWITHDEBINFO:STRING=/Zi /O2 /Ob1 /DNDEBUG\r\n\r\n//Libraries linked by default with all C++ applications.\r\nCMAKE_CXX_STANDARD_LIBRARIES:STRING=kernel32.lib user32.lib gdi32.lib winspool.lib shell32.lib ole32.lib oleaut32.lib uuid.lib comdlg32.lib advapi32.lib\r\n\r\n//C compiler\r\nCMAKE_C_COMPILER:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/cl.exe\r\n\r\n//Flags used by the C compiler during all build types.\r\nCMAKE_C_FLAGS:STRING=/DWIN32 /D_WINDOWS\r\n\r\n//Flags used by the C compiler during DEBUG builds.\r\nCMAKE_C_FLAGS_DEBUG:STRING=/Zi /Ob0 /Od /RTC1\r\n\r\n//Flags used by the C compiler during MINSIZEREL builds.\r\nCMAKE_C_FLAGS_MINSIZEREL:STRING=/O1 /Ob1 /DNDEBUG\r\n\r\n//Flags used by the C compiler during RELEASE builds.\r\nCMAKE_C_FLAGS_RELEASE:STRING=/O2 /Ob2 /DNDEBUG\r\n\r\n//Flags used by the C compiler during RELWITHDEBINFO builds.\r\nCMAKE_C_FLAGS_RELWITHDEBINFO:STRING=/Zi /O2 /Ob1 /DNDEBUG\r\n\r\n//Libraries linked by default with all C applications.\r\nCMAKE_C_STANDARD_LIBRARIES:STRING=kernel32.lib user32.lib gdi32.lib winspool.lib shell32.lib ole32.lib oleaut32.lib uuid.lib comdlg32.lib advapi32.lib\r\n\r\n//Flags used by the linker during all build types.\r\nCMAKE_EXE_LINKER_FLAGS:STRING=/machine:x64\r\n\r\n//Flags used by the linker during DEBUG builds.\r\nCMAKE_EXE_LINKER_FLAGS_DEBUG:STRING=/debug /INCREMENTAL\r\n\r\n//Flags used by the linker during MINSIZEREL builds.\r\nCMAKE_EXE_LINKER_FLAGS_MINSIZEREL:STRING=/INCREMENTAL:NO\r\n\r\n//Flags used by the linker during RELEASE builds.\r\nCMAKE_EXE_LINKER_FLAGS_RELEASE:STRING=/INCREMENTAL:NO\r\n\r\n//Flags used by the linker during RELWITHDEBINFO builds.\r\nCMAKE_EXE_LINKER_FLAGS_RELWITHDEBINFO:STRING=/debug /INCREMENTAL\r\n\r\n//Enable/Disable output of build database during the build.\r\nCMAKE_EXPORT_BUILD_DATABASE:BOOL=\r\n\r\n//Generate compile commands\r\nCMAKE_EXPORT_COMPILE_COMMANDS:STRING=ON\r\n\r\n//Value Computed by CMake.\r\nCMAKE_FIND_PACKAGE_REDIRECTS_DIR:STATIC=G:/home/miv-sound/build/windows-release/CMakeFiles/pkgRedirects\r\n\r\n//Install path prefix, prepended onto install directories.\r\nCMAKE_INSTALL_PREFIX:PATH=C:/Program Files (x86)/EffeTunePlugin\r\n\r\n//Path to a program.\r\nCMAKE_LINKER:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/link.exe\r\n\r\n//make program\r\nCMAKE_MAKE_PROGRAM:FILEPATH=C:/PROGRA~2/MICROS~2/18/BUILDT~1/Common7/IDE/COMMON~1/MICROS~1/CMake/Ninja/ninja.exe\r\n\r\n//Flags used by the linker during the creation of modules during\r\n// all build types.\r\nCMAKE_MODULE_LINKER_FLAGS:STRING=/machine:x64\r\n\r\n//Flags used by the linker during the creation of modules during\r\n// DEBUG builds.\r\nCMAKE_MODULE_LINKER_FLAGS_DEBUG:STRING=/debug /INCREMENTAL\r\n\r\n//Flags used by the linker during the creation of modules during\r\n// MINSIZEREL builds.\r\nCMAKE_MODULE_LINKER_FLAGS_MINSIZEREL:STRING=/INCREMENTAL:NO\r\n\r\n//Flags used by the linker during the creation of modules during\r\n// RELEASE builds.\r\nCMAKE_MODULE_LINKER_FLAGS_RELEASE:STRING=/INCREMENTAL:NO\r\n\r\n//Flags used by the linker during the creation of modules during\r\n// RELWITHDEBINFO builds.\r\nCMAKE_MODULE_LINKER_FLAGS_RELWITHDEBINFO:STRING=/debug /INCREMENTAL\r\n\r\n//Path to a program.\r\nCMAKE_MT:FILEPATH=C:/Program Files (x86)/Windows Kits/10/bin/10.0.26100.0/x64/mt.exe\r\n\r\n//macOS deployment target\r\nCMAKE_OSX_DEPLOYMENT_TARGET:STRING=10.13\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_COMPAT_VERSION:STATIC=\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_DESCRIPTION:STATIC=\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_HOMEPAGE_URL:STATIC=\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_NAME:STATIC=EffeTunePlugin\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_SPDX_LICENSE:STATIC=\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_VERSION:STATIC=0.15.0.1\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_VERSION_MAJOR:STATIC=0\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_VERSION_MINOR:STATIC=15\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_VERSION_PATCH:STATIC=0\r\n\r\n//Value Computed by CMake\r\nCMAKE_PROJECT_VERSION_TWEAK:STATIC=1\r\n\r\n//RC compiler\r\nCMAKE_RC_COMPILER:FILEPATH=C:/Program Files (x86)/Windows Kits/10/bin/10.0.26100.0/x64/rc.exe\r\n\r\n//Flags for Windows Resource Compiler during all build types.\r\nCMAKE_RC_FLAGS:STRING=-DWIN32\r\n\r\n//Flags for Windows Resource Compiler during DEBUG builds.\r\nCMAKE_RC_FLAGS_DEBUG:STRING=-D_DEBUG\r\n\r\n//Flags for Windows Resource Compiler during MINSIZEREL builds.\r\nCMAKE_RC_FLAGS_MINSIZEREL:STRING=\r\n\r\n//Flags for Windows Resource Compiler during RELEASE builds.\r\nCMAKE_RC_FLAGS_RELEASE:STRING=\r\n\r\n//Flags for Windows Resource Compiler during RELWITHDEBINFO builds.\r\nCMAKE_RC_FLAGS_RELWITHDEBINFO:STRING=\r\n\r\n//Flags used by the linker during the creation of shared libraries\r\n// during all build types.\r\nCMAKE_SHARED_LINKER_FLAGS:STRING=/machine:x64\r\n\r\n//Flags used by the linker during the creation of shared libraries\r\n// during DEBUG builds.\r\nCMAKE_SHARED_LINKER_FLAGS_DEBUG:STRING=/debug /INCREMENTAL\r\n\r\n//Flags used by the linker during the creation of shared libraries\r\n// during MINSIZEREL builds.\r\nCMAKE_SHARED_LINKER_FLAGS_MINSIZEREL:STRING=/INCREMENTAL:NO\r\n\r\n//Flags used by the linker during the creation of shared libraries\r\n// during RELEASE builds.\r\nCMAKE_SHARED_LINKER_FLAGS_RELEASE:STRING=/INCREMENTAL:NO\r\n\r\n//Flags used by the linker during the creation of shared libraries\r\n// during RELWITHDEBINFO builds.\r\nCMAKE_SHARED_LINKER_FLAGS_RELWITHDEBINFO:STRING=/debug /INCREMENTAL\r\n\r\n//If set, runtime paths are not added when installing shared libraries,\r\n// but are added when building.\r\nCMAKE_SKIP_INSTALL_RPATH:BOOL=NO\r\n\r\n//If set, runtime paths are not added when using shared libraries.\r\nCMAKE_SKIP_RPATH:BOOL=NO\r\n\r\n//Flags used by the archiver during the creation of static libraries\r\n// during all build types.\r\nCMAKE_STATIC_LINKER_FLAGS:STRING=/machine:x64\r\n\r\n//Flags used by the archiver during the creation of static libraries\r\n// during DEBUG builds.\r\nCMAKE_STATIC_LINKER_FLAGS_DEBUG:STRING=\r\n\r\n//Flags used by the archiver during the creation of static libraries\r\n// during MINSIZEREL builds.\r\nCMAKE_STATIC_LINKER_FLAGS_MINSIZEREL:STRING=\r\n\r\n//Flags used by the archiver during the creation of static libraries\r\n// during RELEASE builds.\r\nCMAKE_STATIC_LINKER_FLAGS_RELEASE:STRING=\r\n\r\n//Flags used by the archiver during the creation of static libraries\r\n// during RELWITHDEBINFO builds.\r\nCMAKE_STATIC_LINKER_FLAGS_RELWITHDEBINFO:STRING=\r\n\r\n//If this value is on, makefiles will be generated without the\r\n// .SILENT directive, and all commands will be echoed to the console\r\n// during the make.  This is useful for debugging only. With Visual\r\n// Studio IDE projects all commands are done without /nologo.\r\nCMAKE_VERBOSE_MAKEFILE:BOOL=FALSE\r\n\r\n//Path to the coverage program that CTest uses for performing coverage\r\n// inspection\r\nCOVERAGE_COMMAND:FILEPATH=COVERAGE_COMMAND-NOTFOUND\r\n\r\n//Extra command line flags to pass to the coverage tool\r\nCOVERAGE_EXTRA_FLAGS:STRING=-l\r\n\r\n//How many times to retry timed-out CTest submissions.\r\nCTEST_SUBMIT_RETRY_COUNT:STRING=3\r\n\r\n//How long to wait between timed-out CTest submissions.\r\nCTEST_SUBMIT_RETRY_DELAY:STRING=5\r\n\r\n//Maximum time allowed before CTest will kill the test.\r\nDART_TESTING_TIMEOUT:STRING=1500\r\n\r\n//Build the non-shipping automation registration host-gate fixture\r\nEFFETUNE_AUTOMATION_HOST_GATE_FIXTURE:BOOL=OFF\r\n\r\n//Build the VST3 plug-in\r\nEFFETUNE_BUILD_PLUGIN:BOOL=ON\r\n\r\n//Build EffeTune VST unit tests\r\nEFFETUNE_BUILD_TESTS:BOOL=ON\r\n\r\n//Build EffeTune VST command-line tools\r\nEFFETUNE_BUILD_TOOLS:BOOL=ON\r\n\r\n//Use AVX2/FMA for the Windows x64 resampler\r\nEFFETUNE_ENABLE_AVX2:BOOL=ON\r\n\r\n//Path to a program.\r\nEFFETUNE_NODE_EXECUTABLE:FILEPATH=C:/Program Files/nodejs/node.exe\r\n\r\n//EffeTune source tree (override only for local upstream integration\r\n// checks)\r\nEFFETUNE_UPSTREAM_SOURCE_DIR:PATH=G:/home/miv-sound/external/effetune\r\n\r\n//Expose test-only kernel state inspection\r\nET_DEBUG_STATE:BOOL=OFF\r\n\r\n//Compile registered native test sources with Emscripten Clang\r\n// warnings\r\nET_NATIVE_TEST_WARNING_CHECK:BOOL=OFF\r\n\r\n//Path to a program.\r\nET_NODE_EXECUTABLE:FILEPATH=C:/Program Files/nodejs/node.exe\r\n\r\n//Build the WebAssembly SIMD128 variant\r\nET_SIMD:BOOL=OFF\r\n\r\n//Value Computed by CMake\r\nEffeTunePlugin_BINARY_DIR:STATIC=G:/home/miv-sound/build/windows-release\r\n\r\n//Value Computed by CMake\r\nEffeTunePlugin_IS_TOP_LEVEL:STATIC=ON\r\n\r\n//Value Computed by CMake\r\nEffeTunePlugin_SOURCE_DIR:STATIC=G:/home/miv-sound\r\n\r\n//Directory under which to collect all populated content\r\nFETCHCONTENT_BASE_DIR:PATH=G:/home/miv-sound/build/windows-release/_deps\r\n\r\n//Disables all attempts to download or update content and assumes\r\n// source dirs already exist\r\nFETCHCONTENT_FULLY_DISCONNECTED:BOOL=OFF\r\n\r\n//Enables QUIET option for all content population\r\nFETCHCONTENT_QUIET:BOOL=ON\r\n\r\n//When not empty, overrides where to find pre-populated content\r\n// for vst3sdk\r\nFETCHCONTENT_SOURCE_DIR_VST3SDK:PATH=\r\n\r\n//Enables UPDATE_DISCONNECTED behavior for all content population\r\nFETCHCONTENT_UPDATES_DISCONNECTED:BOOL=ON\r\n\r\n//Enables UPDATE_DISCONNECTED behavior just for population of vst3sdk\r\nFETCHCONTENT_UPDATES_DISCONNECTED_VST3SDK:BOOL=OFF\r\n\r\n//Path to a program.\r\nGITCOMMAND:FILEPATH=C:/Program Files/Git/cmd/git.exe\r\n\r\n//Git command line client\r\nGIT_EXECUTABLE:FILEPATH=C:/Program Files/Git/cmd/git.exe\r\n\r\n//Path to a file.\r\nLIBJACK_INCLUDE_DIR:PATH=LIBJACK_INCLUDE_DIR-NOTFOUND\r\n\r\n//Path to a library.\r\nLIBJACK_libjack_LIBRARY:FILEPATH=LIBJACK_libjack_LIBRARY-NOTFOUND\r\n\r\n//Path to a library.\r\nLIBJACK_libjackserver_LIBRARY:FILEPATH=LIBJACK_libjackserver_LIBRARY-NOTFOUND\r\n\r\n//Command to build the project\r\nMAKECOMMAND:STRING=\"C:\\Program Files\\CMake\\bin\\cmake.exe\" --build . --config \"${CTEST_CONFIGURATION_TYPE}\"\r\n\r\n//Path to the memory checking command, used for memory error detection.\r\nMEMORYCHECK_COMMAND:FILEPATH=MEMORYCHECK_COMMAND-NOTFOUND\r\n\r\n//File that contains suppressions for the memory checker\r\nMEMORYCHECK_SUPPRESSIONS_FILE:FILEPATH=\r\n\r\n//Name of the computer/site where compile is being run\r\nSITE:STRING=DESKTOP4090\r\n\r\n//Here you can define where to find the AAX SDK\r\nSMTG_AAX_SDK_PATH:PATH=\r\n\r\n//Build VST3 Utilities\r\nSMTG_ADD_VST3_UTILITIES:BOOL=ON\r\n\r\n//Create Bundle on Windows for the Plug-ins (New since VST 3.6.10!)\r\nSMTG_CREATE_BUNDLE_FOR_WINDOWS:BOOL=ON\r\n\r\n//Create the moduleinfo.json file\r\nSMTG_CREATE_MODULE_INFO:BOOL=ON\r\n\r\n//Create symbolic link for each Plug-in (you need to have the Administrator\r\n// right on Windows! or change the Local Group Policy to allow\r\n// create symbolic links)\r\nSMTG_CREATE_PLUGIN_LINK:BOOL=OFF\r\n\r\n//Customize output location for binaries\r\nSMTG_CUSTOM_BINARY_LOCATION:PATH=\r\n\r\n//C++ standard version used for plugins: 14, 17, 20, 23\r\nSMTG_CXX_STANDARD:STRING=\r\n\r\n//Enable Address Sanitizer\r\nSMTG_ENABLE_ADDRESS_SANITIZER:BOOL=OFF\r\n\r\n//Enable Target variables Logging\r\nSMTG_ENABLE_TARGET_VARS_LOG:BOOL=OFF\r\n\r\n//Enable VST 3 Hosting Examples\r\nSMTG_ENABLE_VST3_HOSTING_EXAMPLES:BOOL=OFF\r\n\r\n//Enable VST 3 Plug-in Examples\r\nSMTG_ENABLE_VST3_PLUGIN_EXAMPLES:BOOL=OFF\r\n\r\n//Enable VSTGUI Support\r\nSMTG_ENABLE_VSTGUI_SUPPORT:BOOL=OFF\r\n\r\n//Enable Wayland Support (Linux only)\r\nSMTG_ENABLE_WAYLAND_SUPPORT:BOOL=OFF\r\n\r\n//use FOLDERID_UserProgramFilesCommon as VST3 target path\r\nSMTG_PLUGIN_TARGET_USER_PROGRAM_FILES_COMMON:BOOL=ON\r\n\r\n//Rename ASSERT to SMTG_ASSERT\r\nSMTG_RENAME_ASSERT:BOOL=ON\r\n\r\n//Run VST validator on VST3 Plug-ins\r\nSMTG_RUN_VST_VALIDATOR:BOOL=OFF\r\n\r\n//use static CRuntime on Windows (option /MT)\r\nSMTG_USE_STATIC_CRT:BOOL=OFF\r\n\r\n//Create macOS and iOS collection targets\r\nSMTG_VSTSDK_GENERATE_MACOS_IOS_COLLECTION_TARGETS:BOOL=OFF\r\n\r\n//Value Computed by CMake\r\neffetune_dsp_BINARY_DIR:STATIC=G:/home/miv-sound/build/windows-release/external/effetune/dsp\r\n\r\n//Value Computed by CMake\r\neffetune_dsp_IS_TOP_LEVEL:STATIC=OFF\r\n\r\n//Value Computed by CMake\r\neffetune_dsp_SOURCE_DIR:STATIC=G:/home/miv-sound/external/effetune/dsp\r\n\r\n//Value Computed by CMake\r\nvstsdk_BINARY_DIR:STATIC=G:/home/miv-sound/build/windows-release/_deps/vst3sdk-build\r\n\r\n//Value Computed by CMake\r\nvstsdk_IS_TOP_LEVEL:STATIC=OFF\r\n\r\n//Value Computed by CMake\r\nvstsdk_SOURCE_DIR:STATIC=G:/home/miv-sound/build/windows-release/_deps/vst3sdk-src\r\n\r\n\r\n########################\r\n# INTERNAL cache entries\r\n########################\r\n\r\n//ADVANCED property for variable: CMAKE_AR\r\nCMAKE_AR-ADVANCED:INTERNAL=1\r\n//This is the directory where this CMakeCache.txt was created\r\nCMAKE_CACHEFILE_DIR:INTERNAL=g:/home/miv-sound/build/windows-release\r\n//Major version of cmake used to create the current loaded cache\r\nCMAKE_CACHE_MAJOR_VERSION:INTERNAL=4\r\n//Minor version of cmake used to create the current loaded cache\r\nCMAKE_CACHE_MINOR_VERSION:INTERNAL=3\r\n//Patch version of cmake used to create the current loaded cache\r\nCMAKE_CACHE_PATCH_VERSION:INTERNAL=1\r\n//Path to CMake executable.\r\nCMAKE_COMMAND:INTERNAL=C:/Program Files/CMake/bin/cmake.exe\r\n//Path to cpack program executable.\r\nCMAKE_CPACK_COMMAND:INTERNAL=C:/Program Files/CMake/bin/cpack.exe\r\n//ADVANCED property for variable: CMAKE_CTEST_COMMAND\r\nCMAKE_CTEST_COMMAND-ADVANCED:INTERNAL=1\r\n//Path to ctest program executable.\r\nCMAKE_CTEST_COMMAND:INTERNAL=C:/Program Files/CMake/bin/ctest.exe\r\n//ADVANCED property for variable: CMAKE_CXX_COMPILER\r\nCMAKE_CXX_COMPILER-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_CXX_FLAGS\r\nCMAKE_CXX_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_CXX_FLAGS_DEBUG\r\nCMAKE_CXX_FLAGS_DEBUG-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_CXX_FLAGS_MINSIZEREL\r\nCMAKE_CXX_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_CXX_FLAGS_RELEASE\r\nCMAKE_CXX_FLAGS_RELEASE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_CXX_FLAGS_RELWITHDEBINFO\r\nCMAKE_CXX_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_CXX_STANDARD_LIBRARIES\r\nCMAKE_CXX_STANDARD_LIBRARIES-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_C_COMPILER\r\nCMAKE_C_COMPILER-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_C_FLAGS\r\nCMAKE_C_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_C_FLAGS_DEBUG\r\nCMAKE_C_FLAGS_DEBUG-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_C_FLAGS_MINSIZEREL\r\nCMAKE_C_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_C_FLAGS_RELEASE\r\nCMAKE_C_FLAGS_RELEASE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_C_FLAGS_RELWITHDEBINFO\r\nCMAKE_C_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_C_STANDARD_LIBRARIES\r\nCMAKE_C_STANDARD_LIBRARIES-ADVANCED:INTERNAL=1\r\n//Path to cache edit program executable.\r\nCMAKE_EDIT_COMMAND:INTERNAL=C:/Program Files/CMake/bin/cmake-gui.exe\r\n//Executable file format\r\nCMAKE_EXECUTABLE_FORMAT:INTERNAL=Unknown\r\n//ADVANCED property for variable: CMAKE_EXE_LINKER_FLAGS\r\nCMAKE_EXE_LINKER_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_EXE_LINKER_FLAGS_DEBUG\r\nCMAKE_EXE_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_EXE_LINKER_FLAGS_MINSIZEREL\r\nCMAKE_EXE_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_EXE_LINKER_FLAGS_RELEASE\r\nCMAKE_EXE_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_EXE_LINKER_FLAGS_RELWITHDEBINFO\r\nCMAKE_EXE_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_EXPORT_BUILD_DATABASE\r\nCMAKE_EXPORT_BUILD_DATABASE-ADVANCED:INTERNAL=1\r\n//Name of external makefile project generator.\r\nCMAKE_EXTRA_GENERATOR:INTERNAL=\r\n//Name of generator.\r\nCMAKE_GENERATOR:INTERNAL=Ninja\r\n//Generator instance identifier.\r\nCMAKE_GENERATOR_INSTANCE:INTERNAL=\r\n//Name of generator platform.\r\nCMAKE_GENERATOR_PLATFORM:INTERNAL=\r\n//Name of generator toolset.\r\nCMAKE_GENERATOR_TOOLSET:INTERNAL=\r\n//Source directory with the top level CMakeLists.txt file for this\r\n// project\r\nCMAKE_HOME_DIRECTORY:INTERNAL=G:/home/miv-sound\r\n//ADVANCED property for variable: CMAKE_LINKER\r\nCMAKE_LINKER-ADVANCED:INTERNAL=1\r\n//Name of CMakeLists files to read\r\nCMAKE_LIST_FILE_NAME:INTERNAL=CMakeLists.txt\r\n//ADVANCED property for variable: CMAKE_MAKE_PROGRAM\r\nCMAKE_MAKE_PROGRAM-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_MODULE_LINKER_FLAGS\r\nCMAKE_MODULE_LINKER_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_MODULE_LINKER_FLAGS_DEBUG\r\nCMAKE_MODULE_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_MODULE_LINKER_FLAGS_MINSIZEREL\r\nCMAKE_MODULE_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_MODULE_LINKER_FLAGS_RELEASE\r\nCMAKE_MODULE_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_MODULE_LINKER_FLAGS_RELWITHDEBINFO\r\nCMAKE_MODULE_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_MT\r\nCMAKE_MT-ADVANCED:INTERNAL=1\r\n//number of local generators\r\nCMAKE_NUMBER_OF_MAKEFILES:INTERNAL=15\r\n//Platform information initialized\r\nCMAKE_PLATFORM_INFO_INITIALIZED:INTERNAL=1\r\n//noop for ranlib\r\nCMAKE_RANLIB:INTERNAL=:\r\n//ADVANCED property for variable: CMAKE_RC_COMPILER\r\nCMAKE_RC_COMPILER-ADVANCED:INTERNAL=1\r\nCMAKE_RC_COMPILER_WORKS:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_RC_FLAGS\r\nCMAKE_RC_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_RC_FLAGS_DEBUG\r\nCMAKE_RC_FLAGS_DEBUG-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_RC_FLAGS_MINSIZEREL\r\nCMAKE_RC_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_RC_FLAGS_RELEASE\r\nCMAKE_RC_FLAGS_RELEASE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_RC_FLAGS_RELWITHDEBINFO\r\nCMAKE_RC_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1\r\n//Path to CMake installation.\r\nCMAKE_ROOT:INTERNAL=C:/Program Files/CMake/share/cmake-4.3\r\n//ADVANCED property for variable: CMAKE_SHARED_LINKER_FLAGS\r\nCMAKE_SHARED_LINKER_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_SHARED_LINKER_FLAGS_DEBUG\r\nCMAKE_SHARED_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_SHARED_LINKER_FLAGS_MINSIZEREL\r\nCMAKE_SHARED_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_SHARED_LINKER_FLAGS_RELEASE\r\nCMAKE_SHARED_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_SHARED_LINKER_FLAGS_RELWITHDEBINFO\r\nCMAKE_SHARED_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_SKIP_INSTALL_RPATH\r\nCMAKE_SKIP_INSTALL_RPATH-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_SKIP_RPATH\r\nCMAKE_SKIP_RPATH-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_STATIC_LINKER_FLAGS\r\nCMAKE_STATIC_LINKER_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_STATIC_LINKER_FLAGS_DEBUG\r\nCMAKE_STATIC_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_STATIC_LINKER_FLAGS_MINSIZEREL\r\nCMAKE_STATIC_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_STATIC_LINKER_FLAGS_RELEASE\r\nCMAKE_STATIC_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_STATIC_LINKER_FLAGS_RELWITHDEBINFO\r\nCMAKE_STATIC_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CMAKE_VERBOSE_MAKEFILE\r\nCMAKE_VERBOSE_MAKEFILE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: COVERAGE_COMMAND\r\nCOVERAGE_COMMAND-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: COVERAGE_EXTRA_FLAGS\r\nCOVERAGE_EXTRA_FLAGS-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CTEST_SUBMIT_RETRY_COUNT\r\nCTEST_SUBMIT_RETRY_COUNT-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: CTEST_SUBMIT_RETRY_DELAY\r\nCTEST_SUBMIT_RETRY_DELAY-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: DART_TESTING_TIMEOUT\r\nDART_TESTING_TIMEOUT-ADVANCED:INTERNAL=1\r\n//Details about finding Python3\r\nFIND_PACKAGE_MESSAGE_DETAILS_Python3:INTERNAL=[C:/Python313/python.exe][found components: Interpreter ][v3.13.4(3.10)]\r\n//ADVANCED property for variable: GITCOMMAND\r\nGITCOMMAND-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: GIT_EXECUTABLE\r\nGIT_EXECUTABLE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: MAKECOMMAND\r\nMAKECOMMAND-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: MEMORYCHECK_COMMAND\r\nMEMORYCHECK_COMMAND-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: MEMORYCHECK_SUPPRESSIONS_FILE\r\nMEMORYCHECK_SUPPRESSIONS_FILE-ADVANCED:INTERNAL=1\r\n//ADVANCED property for variable: SITE\r\nSITE-ADVANCED:INTERNAL=1\r\n//Test SMTG_USE_STDATOMIC_H\r\nSMTG_USE_STDATOMIC_H:INTERNAL=\r\n//The platform which was detected by SMTG\r\nSMTG_WIN:INTERNAL=TRUE\r\n//Compiler reason failure\r\n_Python3_Compiler_REASON_FAILURE:INTERNAL=\r\n//Development reason failure\r\n_Python3_Development_REASON_FAILURE:INTERNAL=\r\n//Path to a program.\r\n_Python3_EXECUTABLE:INTERNAL=C:/Python313/python.exe\r\n//Path to a program.\r\n_Python3_EXECUTABLE_DEBUG:INTERNAL=_Python3_EXECUTABLE_DEBUG-NOTFOUND\r\n//Python3 Properties\r\n_Python3_INTERPRETER_PROPERTIES:INTERNAL=Python;3;13;4;64;32;\u003cnone\u003e;cp313-win_amd64;;C:/Python313/Lib;C:/Python313/Lib;C:/Python313/Lib/site-packages;C:/Python313/Lib/site-packages\r\n_Python3_INTERPRETER_SIGNATURE:INTERNAL=20537deaa1c9eff5ca863a94b55c64d4\r\n//NumPy reason failure\r\n_Python3_NumPy_REASON_FAILURE:INTERNAL=\r\n\r\n",
    "compiler_configuration":  "set(CMAKE_CXX_COMPILER \"C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/cl.exe\")\r\nset(CMAKE_CXX_COMPILER_ARG1 \"\")\r\nset(CMAKE_CXX_COMPILER_ID \"MSVC\")\r\nset(CMAKE_CXX_COMPILER_VERSION \"19.50.35729.0\")\r\nset(CMAKE_CXX_COMPILER_VERSION_INTERNAL \"\")\r\nset(CMAKE_CXX_COMPILER_WRAPPER \"\")\r\nset(CMAKE_CXX_STANDARD_COMPUTED_DEFAULT \"14\")\r\nset(CMAKE_CXX_EXTENSIONS_COMPUTED_DEFAULT \"OFF\")\r\nset(CMAKE_CXX_STANDARD_LATEST \"23\")\r\nset(CMAKE_CXX_COMPILE_FEATURES \"cxx_std_98;cxx_template_template_parameters;cxx_std_11;cxx_alias_templates;cxx_alignas;cxx_alignof;cxx_attributes;cxx_auto_type;cxx_constexpr;cxx_decltype;cxx_decltype_incomplete_return_types;cxx_default_function_template_args;cxx_defaulted_functions;cxx_defaulted_move_initializers;cxx_delegating_constructors;cxx_deleted_functions;cxx_enum_forward_declarations;cxx_explicit_conversions;cxx_extended_friend_declarations;cxx_extern_templates;cxx_final;cxx_func_identifier;cxx_generalized_initializers;cxx_inheriting_constructors;cxx_inline_namespaces;cxx_lambdas;cxx_local_type_template_args;cxx_long_long_type;cxx_noexcept;cxx_nonstatic_member_init;cxx_nullptr;cxx_override;cxx_range_for;cxx_raw_string_literals;cxx_reference_qualified_functions;cxx_right_angle_brackets;cxx_rvalue_references;cxx_sizeof_member;cxx_static_assert;cxx_strong_enums;cxx_thread_local;cxx_trailing_return_types;cxx_unicode_literals;cxx_uniform_initialization;cxx_unrestricted_unions;cxx_user_literals;cxx_variadic_macros;cxx_variadic_templates;cxx_std_14;cxx_aggregate_default_initializers;cxx_attribute_deprecated;cxx_binary_literals;cxx_contextual_conversions;cxx_decltype_auto;cxx_digit_separators;cxx_generic_lambdas;cxx_lambda_init_captures;cxx_relaxed_constexpr;cxx_return_type_deduction;cxx_variable_templates;cxx_std_17;cxx_std_20;cxx_std_23\")\r\nset(CMAKE_CXX98_COMPILE_FEATURES \"cxx_std_98;cxx_template_template_parameters\")\r\nset(CMAKE_CXX11_COMPILE_FEATURES \"cxx_std_11;cxx_alias_templates;cxx_alignas;cxx_alignof;cxx_attributes;cxx_auto_type;cxx_constexpr;cxx_decltype;cxx_decltype_incomplete_return_types;cxx_default_function_template_args;cxx_defaulted_functions;cxx_defaulted_move_initializers;cxx_delegating_constructors;cxx_deleted_functions;cxx_enum_forward_declarations;cxx_explicit_conversions;cxx_extended_friend_declarations;cxx_extern_templates;cxx_final;cxx_func_identifier;cxx_generalized_initializers;cxx_inheriting_constructors;cxx_inline_namespaces;cxx_lambdas;cxx_local_type_template_args;cxx_long_long_type;cxx_noexcept;cxx_nonstatic_member_init;cxx_nullptr;cxx_override;cxx_range_for;cxx_raw_string_literals;cxx_reference_qualified_functions;cxx_right_angle_brackets;cxx_rvalue_references;cxx_sizeof_member;cxx_static_assert;cxx_strong_enums;cxx_thread_local;cxx_trailing_return_types;cxx_unicode_literals;cxx_uniform_initialization;cxx_unrestricted_unions;cxx_user_literals;cxx_variadic_macros;cxx_variadic_templates\")\r\nset(CMAKE_CXX14_COMPILE_FEATURES \"cxx_std_14;cxx_aggregate_default_initializers;cxx_attribute_deprecated;cxx_binary_literals;cxx_contextual_conversions;cxx_decltype_auto;cxx_digit_separators;cxx_generic_lambdas;cxx_lambda_init_captures;cxx_relaxed_constexpr;cxx_return_type_deduction;cxx_variable_templates\")\r\nset(CMAKE_CXX17_COMPILE_FEATURES \"cxx_std_17\")\r\nset(CMAKE_CXX20_COMPILE_FEATURES \"cxx_std_20\")\r\nset(CMAKE_CXX23_COMPILE_FEATURES \"cxx_std_23\")\r\nset(CMAKE_CXX26_COMPILE_FEATURES \"\")\r\n\r\nset(CMAKE_CXX_PLATFORM_ID \"Windows\")\r\nset(CMAKE_CXX_SIMULATE_ID \"\")\r\nset(CMAKE_CXX_COMPILER_FRONTEND_VARIANT \"MSVC\")\r\nset(CMAKE_CXX_COMPILER_APPLE_SYSROOT \"\")\r\nset(CMAKE_CXX_SIMULATE_VERSION \"\")\r\nset(CMAKE_CXX_COMPILER_ARCHITECTURE_ID \"x64\")\r\n\r\nset(MSVC_CXX_ARCHITECTURE_ID x64)\r\n\r\n\r\nset(CMAKE_AR \"C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/lib.exe\")\r\nset(CMAKE_CXX_COMPILER_AR \"\")\r\nset(CMAKE_RANLIB \":\")\r\nset(CMAKE_CXX_COMPILER_RANLIB \"\")\r\nset(CMAKE_LINKER \"C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/link.exe\")\r\nset(CMAKE_LINKER_LINK \"C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/link.exe\")\r\nset(CMAKE_LINKER_LLD \"C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/Llvm/x64/bin/lld-link.exe\")\r\nset(CMAKE_CXX_COMPILER_LINKER \"C:/PROGRA~2/MICROS~2/18/BUILDT~1/VC/Tools/MSVC/1450~1.357/bin/Hostx64/x64/link.exe\")\r\nset(CMAKE_CXX_COMPILER_LINKER_ID \"MSVC\")\r\nset(CMAKE_CXX_COMPILER_LINKER_VERSION 14.50.35729.0)\r\nset(CMAKE_CXX_COMPILER_LINKER_FRONTEND_VARIANT MSVC)\r\nset(CMAKE_MT \"C:/Program Files (x86)/Windows Kits/10/bin/10.0.26100.0/x64/mt.exe\")\r\nset(CMAKE_TAPI \"\")\r\nset(CMAKE_COMPILER_IS_GNUCXX )\r\nset(CMAKE_CXX_COMPILER_LOADED 1)\r\nset(CMAKE_CXX_COMPILER_WORKS TRUE)\r\nset(CMAKE_CXX_ABI_COMPILED TRUE)\r\n\r\nset(CMAKE_CXX_COMPILER_ENV_VAR \"CXX\")\r\n\r\nset(CMAKE_CXX_COMPILER_ID_RUN 1)\r\nset(CMAKE_CXX_SOURCE_FILE_EXTENSIONS C;M;c++;cc;cpp;cxx;m;mm;mpp;CPP;ixx;cppm;ccm;cxxm;c++m)\r\nset(CMAKE_CXX_IGNORE_EXTENSIONS inl;h;hpp;HPP;H;o;O;obj;OBJ;def;DEF;rc;RC)\r\n\r\nforeach (lang IN ITEMS C OBJC OBJCXX)\r\n  if (CMAKE_${lang}_COMPILER_ID_RUN)\r\n    foreach(extension IN LISTS CMAKE_${lang}_SOURCE_FILE_EXTENSIONS)\r\n      list(REMOVE_ITEM CMAKE_CXX_SOURCE_FILE_EXTENSIONS ${extension})\r\n    endforeach()\r\n  endif()\r\nendforeach()\r\n\r\nset(CMAKE_CXX_LINKER_PREFERENCE 30)\r\nset(CMAKE_CXX_LINKER_PREFERENCE_PROPAGATES 1)\r\nset(CMAKE_CXX_LINKER_DEPFILE_SUPPORTED )\r\nset(CMAKE_LINKER_PUSHPOP_STATE_SUPPORTED )\r\nset(CMAKE_CXX_LINKER_PUSHPOP_STATE_SUPPORTED )\r\n\r\n# Save compiler ABI information.\r\nset(CMAKE_CXX_SIZEOF_DATA_PTR \"8\")\r\nset(CMAKE_CXX_COMPILER_ABI \"\")\r\nset(CMAKE_CXX_BYTE_ORDER \"LITTLE_ENDIAN\")\r\nset(CMAKE_CXX_LIBRARY_ARCHITECTURE \"\")\r\n\r\nif(CMAKE_CXX_SIZEOF_DATA_PTR)\r\n  set(CMAKE_SIZEOF_VOID_P \"${CMAKE_CXX_SIZEOF_DATA_PTR}\")\r\nendif()\r\n\r\nif(CMAKE_CXX_COMPILER_ABI)\r\n  set(CMAKE_INTERNAL_PLATFORM_ABI \"${CMAKE_CXX_COMPILER_ABI}\")\r\nendif()\r\n\r\nif(CMAKE_CXX_LIBRARY_ARCHITECTURE)\r\n  set(CMAKE_LIBRARY_ARCHITECTURE \"\")\r\nendif()\r\n\r\nset(CMAKE_CXX_CL_SHOWINCLUDES_PREFIX \"メモ: インクルード ファイル:  \")\r\nif(CMAKE_CXX_CL_SHOWINCLUDES_PREFIX)\r\n  set(CMAKE_CL_SHOWINCLUDES_PREFIX \"${CMAKE_CXX_CL_SHOWINCLUDES_PREFIX}\")\r\nendif()\r\n\r\n\r\n\r\n\r\n\r\nset(CMAKE_CXX_IMPLICIT_INCLUDE_DIRECTORIES \"\")\r\nset(CMAKE_CXX_IMPLICIT_LINK_LIBRARIES \"\")\r\nset(CMAKE_CXX_IMPLICIT_LINK_DIRECTORIES \"\")\r\nset(CMAKE_CXX_IMPLICIT_LINK_FRAMEWORK_DIRECTORIES \"\")\r\nset(CMAKE_CXX_COMPILER_CLANG_RESOURCE_DIR \"\")\r\n\r\nset(CMAKE_CXX_COMPILER_IMPORT_STD \"\")\r\nset(CMAKE_CXX_COMPILER_IMPORT_STD_ERROR_MESSAGE  \"Experimental `import std` support not enabled when detecting toolchain; it must be set before `CXX` is enabled (usually a `project()` call)\")\r\nset(CMAKE_CXX_STDLIB_MODULES_JSON \"\")\r\n",
    "build_contract":  {
                           "preset":  "windows-release",
                           "configure_command":  "cmake --preset windows-release \"-DCMAKE_CXX_FLAGS=/DWIN32 /D_WINDOWS /EHsc /wd4458\" -UFETCHCONTENT_FULLY_DISCONNECTED -DFETCHCONTENT_UPDATES_DISCONNECTED=ON || exit /b 2",
                           "cmake_presets":  "{\n  \"version\": 6,\n  \"cmakeMinimumRequired\": {\n    \"major\": 3,\n    \"minor\": 24,\n    \"patch\": 0\n  },\n  \"configurePresets\": [\n    {\n      \"name\": \"windows-base\",\n      \"hidden\": true,\n      \"generator\": \"Ninja\",\n      \"binaryDir\": \"${sourceDir}/build/${presetName}\",\n      \"cacheVariables\": {\n        \"CMAKE_EXPORT_COMPILE_COMMANDS\": \"ON\",\n        \"EFFETUNE_BUILD_PLUGIN\": \"ON\",\n        \"EFFETUNE_BUILD_TESTS\": \"ON\",\n        \"EFFETUNE_BUILD_TOOLS\": \"ON\",\n        \"EFFETUNE_AUTOMATION_HOST_GATE_FIXTURE\": \"OFF\"\n      },\n      \"condition\": {\n        \"type\": \"equals\",\n        \"lhs\": \"${hostSystemName}\",\n        \"rhs\": \"Windows\"\n      }\n    },\n    {\n      \"name\": \"windows-debug\",\n      \"inherits\": \"windows-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Debug\"\n      }\n    },\n    {\n      \"name\": \"windows-release\",\n      \"inherits\": \"windows-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Release\"\n      }\n    },\n    {\n      \"name\": \"windows-automation-host-gate\",\n      \"inherits\": \"windows-base\",\n      \"binaryDir\": \"${sourceDir}/build/windows-automation-host-gate\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Release\",\n        \"EFFETUNE_AUTOMATION_HOST_GATE_FIXTURE\": \"ON\"\n      }\n    },\n    {\n      \"name\": \"mac-base\",\n      \"hidden\": true,\n      \"generator\": \"Ninja\",\n      \"binaryDir\": \"${sourceDir}/build/${presetName}\",\n      \"cacheVariables\": {\n        \"CMAKE_OSX_ARCHITECTURES\": \"arm64;x86_64\",\n        \"EFFETUNE_BUILD_PLUGIN\": \"ON\",\n        \"EFFETUNE_BUILD_TESTS\": \"ON\",\n        \"EFFETUNE_BUILD_TOOLS\": \"ON\"\n      },\n      \"condition\": {\n        \"type\": \"equals\",\n        \"lhs\": \"${hostSystemName}\",\n        \"rhs\": \"Darwin\"\n      }\n    },\n    {\n      \"name\": \"mac-debug\",\n      \"inherits\": \"mac-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Debug\"\n      }\n    },\n    {\n      \"name\": \"mac-release\",\n      \"inherits\": \"mac-base\",\n      \"cacheVariables\": {\n        \"CMAKE_BUILD_TYPE\": \"Release\"\n      }\n    }\n  ],\n  \"buildPresets\": [\n    { \"name\": \"windows-debug\", \"configurePreset\": \"windows-debug\" },\n    { \"name\": \"windows-release\", \"configurePreset\": \"windows-release\" },\n    {\n      \"name\": \"windows-automation-host-gate\",\n      \"configurePreset\": \"windows-automation-host-gate\"\n    },\n    { \"name\": \"mac-debug\", \"configurePreset\": \"mac-debug\" },\n    { \"name\": \"mac-release\", \"configurePreset\": \"mac-release\" }\n  ],\n  \"testPresets\": [\n    {\n      \"name\": \"windows-debug\",\n      \"configurePreset\": \"windows-debug\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"windows-release\",\n      \"configurePreset\": \"windows-release\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"windows-automation-host-gate\",\n      \"configurePreset\": \"windows-automation-host-gate\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"mac-debug\",\n      \"configurePreset\": \"mac-debug\",\n      \"output\": { \"outputOnFailure\": true }\n    },\n    {\n      \"name\": \"mac-release\",\n      \"configurePreset\": \"mac-release\",\n      \"output\": { \"outputOnFailure\": true }\n    }\n  ]\n}\n",
                           "cmake_user_presets":  null,
                           "cache_entries":  [
                                                 "BUILD_TESTING:BOOL=ON",
                                                 "CMAKE_AR-ADVANCED:INTERNAL=1",
                                                 "CMAKE_AR:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/lib.exe",
                                                 "CMAKE_BUILD_TYPE:STRING=Release",
                                                 "CMAKE_CACHEFILE_DIR:INTERNAL=g:/home/miv-sound/build/windows-release",
                                                 "CMAKE_CACHE_MAJOR_VERSION:INTERNAL=4",
                                                 "CMAKE_CACHE_MINOR_VERSION:INTERNAL=3",
                                                 "CMAKE_CACHE_PATCH_VERSION:INTERNAL=1",
                                                 "CMAKE_COMMAND:INTERNAL=C:/Program Files/CMake/bin/cmake.exe",
                                                 "CMAKE_CPACK_COMMAND:INTERNAL=C:/Program Files/CMake/bin/cpack.exe",
                                                 "CMAKE_CTEST_COMMAND-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CTEST_COMMAND:INTERNAL=C:/Program Files/CMake/bin/ctest.exe",
                                                 "CMAKE_CXX_COMPILER-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CXX_COMPILER:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/cl.exe",
                                                 "CMAKE_CXX_FLAGS-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CXX_FLAGS:STRING=/DWIN32 /D_WINDOWS /EHsc /wd4458",
                                                 "CMAKE_CXX_FLAGS_DEBUG-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CXX_FLAGS_DEBUG:STRING=/Zi /Ob0 /Od /RTC1",
                                                 "CMAKE_CXX_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CXX_FLAGS_MINSIZEREL:STRING=/O1 /Ob1 /DNDEBUG",
                                                 "CMAKE_CXX_FLAGS_RELEASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CXX_FLAGS_RELEASE:STRING=/O2 /Ob2 /DNDEBUG",
                                                 "CMAKE_CXX_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CXX_FLAGS_RELWITHDEBINFO:STRING=/Zi /O2 /Ob1 /DNDEBUG",
                                                 "CMAKE_CXX_STANDARD_LIBRARIES-ADVANCED:INTERNAL=1",
                                                 "CMAKE_CXX_STANDARD_LIBRARIES:STRING=kernel32.lib user32.lib gdi32.lib winspool.lib shell32.lib ole32.lib oleaut32.lib uuid.lib comdlg32.lib advapi32.lib",
                                                 "CMAKE_C_COMPILER-ADVANCED:INTERNAL=1",
                                                 "CMAKE_C_COMPILER:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/cl.exe",
                                                 "CMAKE_C_FLAGS-ADVANCED:INTERNAL=1",
                                                 "CMAKE_C_FLAGS:STRING=/DWIN32 /D_WINDOWS",
                                                 "CMAKE_C_FLAGS_DEBUG-ADVANCED:INTERNAL=1",
                                                 "CMAKE_C_FLAGS_DEBUG:STRING=/Zi /Ob0 /Od /RTC1",
                                                 "CMAKE_C_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1",
                                                 "CMAKE_C_FLAGS_MINSIZEREL:STRING=/O1 /Ob1 /DNDEBUG",
                                                 "CMAKE_C_FLAGS_RELEASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_C_FLAGS_RELEASE:STRING=/O2 /Ob2 /DNDEBUG",
                                                 "CMAKE_C_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1",
                                                 "CMAKE_C_FLAGS_RELWITHDEBINFO:STRING=/Zi /O2 /Ob1 /DNDEBUG",
                                                 "CMAKE_C_STANDARD_LIBRARIES-ADVANCED:INTERNAL=1",
                                                 "CMAKE_C_STANDARD_LIBRARIES:STRING=kernel32.lib user32.lib gdi32.lib winspool.lib shell32.lib ole32.lib oleaut32.lib uuid.lib comdlg32.lib advapi32.lib",
                                                 "CMAKE_EDIT_COMMAND:INTERNAL=C:/Program Files/CMake/bin/cmake-gui.exe",
                                                 "CMAKE_EXECUTABLE_FORMAT:INTERNAL=Unknown",
                                                 "CMAKE_EXE_LINKER_FLAGS-ADVANCED:INTERNAL=1",
                                                 "CMAKE_EXE_LINKER_FLAGS:STRING=/machine:x64",
                                                 "CMAKE_EXE_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1",
                                                 "CMAKE_EXE_LINKER_FLAGS_DEBUG:STRING=/debug /INCREMENTAL",
                                                 "CMAKE_EXE_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1",
                                                 "CMAKE_EXE_LINKER_FLAGS_MINSIZEREL:STRING=/INCREMENTAL:NO",
                                                 "CMAKE_EXE_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_EXE_LINKER_FLAGS_RELEASE:STRING=/INCREMENTAL:NO",
                                                 "CMAKE_EXE_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1",
                                                 "CMAKE_EXE_LINKER_FLAGS_RELWITHDEBINFO:STRING=/debug /INCREMENTAL",
                                                 "CMAKE_EXPORT_BUILD_DATABASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_EXPORT_BUILD_DATABASE:BOOL=",
                                                 "CMAKE_EXPORT_COMPILE_COMMANDS:STRING=ON",
                                                 "CMAKE_EXTRA_GENERATOR:INTERNAL=",
                                                 "CMAKE_FIND_PACKAGE_REDIRECTS_DIR:STATIC=G:/home/miv-sound/build/windows-release/CMakeFiles/pkgRedirects",
                                                 "CMAKE_GENERATOR:INTERNAL=Ninja",
                                                 "CMAKE_GENERATOR_INSTANCE:INTERNAL=",
                                                 "CMAKE_GENERATOR_PLATFORM:INTERNAL=",
                                                 "CMAKE_GENERATOR_TOOLSET:INTERNAL=",
                                                 "CMAKE_HOME_DIRECTORY:INTERNAL=G:/home/miv-sound",
                                                 "CMAKE_INSTALL_PREFIX:PATH=C:/Program Files (x86)/EffeTunePlugin",
                                                 "CMAKE_LINKER-ADVANCED:INTERNAL=1",
                                                 "CMAKE_LINKER:FILEPATH=C:/Program Files (x86)/Microsoft Visual Studio/18/BuildTools/VC/Tools/MSVC/14.50.35717/bin/Hostx64/x64/link.exe",
                                                 "CMAKE_LIST_FILE_NAME:INTERNAL=CMakeLists.txt",
                                                 "CMAKE_MAKE_PROGRAM-ADVANCED:INTERNAL=1",
                                                 "CMAKE_MAKE_PROGRAM:FILEPATH=C:/PROGRA~2/MICROS~2/18/BUILDT~1/Common7/IDE/COMMON~1/MICROS~1/CMake/Ninja/ninja.exe",
                                                 "CMAKE_MODULE_LINKER_FLAGS-ADVANCED:INTERNAL=1",
                                                 "CMAKE_MODULE_LINKER_FLAGS:STRING=/machine:x64",
                                                 "CMAKE_MODULE_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1",
                                                 "CMAKE_MODULE_LINKER_FLAGS_DEBUG:STRING=/debug /INCREMENTAL",
                                                 "CMAKE_MODULE_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1",
                                                 "CMAKE_MODULE_LINKER_FLAGS_MINSIZEREL:STRING=/INCREMENTAL:NO",
                                                 "CMAKE_MODULE_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_MODULE_LINKER_FLAGS_RELEASE:STRING=/INCREMENTAL:NO",
                                                 "CMAKE_MODULE_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1",
                                                 "CMAKE_MODULE_LINKER_FLAGS_RELWITHDEBINFO:STRING=/debug /INCREMENTAL",
                                                 "CMAKE_MT-ADVANCED:INTERNAL=1",
                                                 "CMAKE_MT:FILEPATH=C:/Program Files (x86)/Windows Kits/10/bin/10.0.26100.0/x64/mt.exe",
                                                 "CMAKE_NUMBER_OF_MAKEFILES:INTERNAL=15",
                                                 "CMAKE_OSX_DEPLOYMENT_TARGET:STRING=10.13",
                                                 "CMAKE_PLATFORM_INFO_INITIALIZED:INTERNAL=1",
                                                 "CMAKE_PROJECT_COMPAT_VERSION:STATIC=",
                                                 "CMAKE_PROJECT_DESCRIPTION:STATIC=",
                                                 "CMAKE_PROJECT_HOMEPAGE_URL:STATIC=",
                                                 "CMAKE_PROJECT_NAME:STATIC=EffeTunePlugin",
                                                 "CMAKE_PROJECT_SPDX_LICENSE:STATIC=",
                                                 "CMAKE_PROJECT_VERSION:STATIC=0.15.0.1",
                                                 "CMAKE_PROJECT_VERSION_MAJOR:STATIC=0",
                                                 "CMAKE_PROJECT_VERSION_MINOR:STATIC=15",
                                                 "CMAKE_PROJECT_VERSION_PATCH:STATIC=0",
                                                 "CMAKE_PROJECT_VERSION_TWEAK:STATIC=1",
                                                 "CMAKE_RANLIB:INTERNAL=:",
                                                 "CMAKE_RC_COMPILER-ADVANCED:INTERNAL=1",
                                                 "CMAKE_RC_COMPILER:FILEPATH=C:/Program Files (x86)/Windows Kits/10/bin/10.0.26100.0/x64/rc.exe",
                                                 "CMAKE_RC_COMPILER_WORKS:INTERNAL=1",
                                                 "CMAKE_RC_FLAGS-ADVANCED:INTERNAL=1",
                                                 "CMAKE_RC_FLAGS:STRING=-DWIN32",
                                                 "CMAKE_RC_FLAGS_DEBUG-ADVANCED:INTERNAL=1",
                                                 "CMAKE_RC_FLAGS_DEBUG:STRING=-D_DEBUG",
                                                 "CMAKE_RC_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1",
                                                 "CMAKE_RC_FLAGS_MINSIZEREL:STRING=",
                                                 "CMAKE_RC_FLAGS_RELEASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_RC_FLAGS_RELEASE:STRING=",
                                                 "CMAKE_RC_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1",
                                                 "CMAKE_RC_FLAGS_RELWITHDEBINFO:STRING=",
                                                 "CMAKE_ROOT:INTERNAL=C:/Program Files/CMake/share/cmake-4.3",
                                                 "CMAKE_SHARED_LINKER_FLAGS-ADVANCED:INTERNAL=1",
                                                 "CMAKE_SHARED_LINKER_FLAGS:STRING=/machine:x64",
                                                 "CMAKE_SHARED_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1",
                                                 "CMAKE_SHARED_LINKER_FLAGS_DEBUG:STRING=/debug /INCREMENTAL",
                                                 "CMAKE_SHARED_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1",
                                                 "CMAKE_SHARED_LINKER_FLAGS_MINSIZEREL:STRING=/INCREMENTAL:NO",
                                                 "CMAKE_SHARED_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_SHARED_LINKER_FLAGS_RELEASE:STRING=/INCREMENTAL:NO",
                                                 "CMAKE_SHARED_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1",
                                                 "CMAKE_SHARED_LINKER_FLAGS_RELWITHDEBINFO:STRING=/debug /INCREMENTAL",
                                                 "CMAKE_SKIP_INSTALL_RPATH-ADVANCED:INTERNAL=1",
                                                 "CMAKE_SKIP_INSTALL_RPATH:BOOL=NO",
                                                 "CMAKE_SKIP_RPATH-ADVANCED:INTERNAL=1",
                                                 "CMAKE_SKIP_RPATH:BOOL=NO",
                                                 "CMAKE_STATIC_LINKER_FLAGS-ADVANCED:INTERNAL=1",
                                                 "CMAKE_STATIC_LINKER_FLAGS:STRING=/machine:x64",
                                                 "CMAKE_STATIC_LINKER_FLAGS_DEBUG-ADVANCED:INTERNAL=1",
                                                 "CMAKE_STATIC_LINKER_FLAGS_DEBUG:STRING=",
                                                 "CMAKE_STATIC_LINKER_FLAGS_MINSIZEREL-ADVANCED:INTERNAL=1",
                                                 "CMAKE_STATIC_LINKER_FLAGS_MINSIZEREL:STRING=",
                                                 "CMAKE_STATIC_LINKER_FLAGS_RELEASE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_STATIC_LINKER_FLAGS_RELEASE:STRING=",
                                                 "CMAKE_STATIC_LINKER_FLAGS_RELWITHDEBINFO-ADVANCED:INTERNAL=1",
                                                 "CMAKE_STATIC_LINKER_FLAGS_RELWITHDEBINFO:STRING=",
                                                 "CMAKE_VERBOSE_MAKEFILE-ADVANCED:INTERNAL=1",
                                                 "CMAKE_VERBOSE_MAKEFILE:BOOL=FALSE",
                                                 "COVERAGE_COMMAND-ADVANCED:INTERNAL=1",
                                                 "COVERAGE_COMMAND:FILEPATH=COVERAGE_COMMAND-NOTFOUND",
                                                 "COVERAGE_EXTRA_FLAGS-ADVANCED:INTERNAL=1",
                                                 "COVERAGE_EXTRA_FLAGS:STRING=-l",
                                                 "CTEST_SUBMIT_RETRY_COUNT-ADVANCED:INTERNAL=1",
                                                 "CTEST_SUBMIT_RETRY_COUNT:STRING=3",
                                                 "CTEST_SUBMIT_RETRY_DELAY-ADVANCED:INTERNAL=1",
                                                 "CTEST_SUBMIT_RETRY_DELAY:STRING=5",
                                                 "DART_TESTING_TIMEOUT-ADVANCED:INTERNAL=1",
                                                 "DART_TESTING_TIMEOUT:STRING=1500",
                                                 "EFFETUNE_AUTOMATION_HOST_GATE_FIXTURE:BOOL=OFF",
                                                 "EFFETUNE_BUILD_PLUGIN:BOOL=ON",
                                                 "EFFETUNE_BUILD_TESTS:BOOL=ON",
                                                 "EFFETUNE_BUILD_TOOLS:BOOL=ON",
                                                 "EFFETUNE_ENABLE_AVX2:BOOL=ON",
                                                 "EFFETUNE_NODE_EXECUTABLE:FILEPATH=C:/Program Files/nodejs/node.exe",
                                                 "EFFETUNE_UPSTREAM_SOURCE_DIR:PATH=G:/home/miv-sound/external/effetune",
                                                 "ET_DEBUG_STATE:BOOL=OFF",
                                                 "ET_NATIVE_TEST_WARNING_CHECK:BOOL=OFF",
                                                 "ET_NODE_EXECUTABLE:FILEPATH=C:/Program Files/nodejs/node.exe",
                                                 "ET_SIMD:BOOL=OFF",
                                                 "EffeTunePlugin_BINARY_DIR:STATIC=G:/home/miv-sound/build/windows-release",
                                                 "EffeTunePlugin_IS_TOP_LEVEL:STATIC=ON",
                                                 "EffeTunePlugin_SOURCE_DIR:STATIC=G:/home/miv-sound",
                                                 "FETCHCONTENT_BASE_DIR:PATH=G:/home/miv-sound/build/windows-release/_deps",
                                                 "FETCHCONTENT_FULLY_DISCONNECTED:BOOL=OFF",
                                                 "FETCHCONTENT_QUIET:BOOL=ON",
                                                 "FETCHCONTENT_SOURCE_DIR_VST3SDK:PATH=",
                                                 "FETCHCONTENT_UPDATES_DISCONNECTED:BOOL=ON",
                                                 "FETCHCONTENT_UPDATES_DISCONNECTED_VST3SDK:BOOL=OFF",
                                                 "FIND_PACKAGE_MESSAGE_DETAILS_Python3:INTERNAL=[C:/Python313/python.exe][found components: Interpreter ][v3.13.4(3.10)]",
                                                 "GITCOMMAND-ADVANCED:INTERNAL=1",
                                                 "GITCOMMAND:FILEPATH=C:/Program Files/Git/cmd/git.exe",
                                                 "GIT_EXECUTABLE-ADVANCED:INTERNAL=1",
                                                 "GIT_EXECUTABLE:FILEPATH=C:/Program Files/Git/cmd/git.exe",
                                                 "LIBJACK_INCLUDE_DIR:PATH=LIBJACK_INCLUDE_DIR-NOTFOUND",
                                                 "LIBJACK_libjack_LIBRARY:FILEPATH=LIBJACK_libjack_LIBRARY-NOTFOUND",
                                                 "LIBJACK_libjackserver_LIBRARY:FILEPATH=LIBJACK_libjackserver_LIBRARY-NOTFOUND",
                                                 "MAKECOMMAND-ADVANCED:INTERNAL=1",
                                                 "MAKECOMMAND:STRING=\"C:\\Program Files\\CMake\\bin\\cmake.exe\" --build . --config \"${CTEST_CONFIGURATION_TYPE}\"",
                                                 "MEMORYCHECK_COMMAND-ADVANCED:INTERNAL=1",
                                                 "MEMORYCHECK_COMMAND:FILEPATH=MEMORYCHECK_COMMAND-NOTFOUND",
                                                 "MEMORYCHECK_SUPPRESSIONS_FILE-ADVANCED:INTERNAL=1",
                                                 "MEMORYCHECK_SUPPRESSIONS_FILE:FILEPATH=",
                                                 "SITE-ADVANCED:INTERNAL=1",
                                                 "SITE:STRING=DESKTOP4090",
                                                 "SMTG_AAX_SDK_PATH:PATH=",
                                                 "SMTG_ADD_VST3_UTILITIES:BOOL=ON",
                                                 "SMTG_CREATE_BUNDLE_FOR_WINDOWS:BOOL=ON",
                                                 "SMTG_CREATE_MODULE_INFO:BOOL=ON",
                                                 "SMTG_CREATE_PLUGIN_LINK:BOOL=OFF",
                                                 "SMTG_CUSTOM_BINARY_LOCATION:PATH=",
                                                 "SMTG_CXX_STANDARD:STRING=",
                                                 "SMTG_ENABLE_ADDRESS_SANITIZER:BOOL=OFF",
                                                 "SMTG_ENABLE_TARGET_VARS_LOG:BOOL=OFF",
                                                 "SMTG_ENABLE_VST3_HOSTING_EXAMPLES:BOOL=OFF",
                                                 "SMTG_ENABLE_VST3_PLUGIN_EXAMPLES:BOOL=OFF",
                                                 "SMTG_ENABLE_VSTGUI_SUPPORT:BOOL=OFF",
                                                 "SMTG_ENABLE_WAYLAND_SUPPORT:BOOL=OFF",
                                                 "SMTG_PLUGIN_TARGET_USER_PROGRAM_FILES_COMMON:BOOL=ON",
                                                 "SMTG_RENAME_ASSERT:BOOL=ON",
                                                 "SMTG_RUN_VST_VALIDATOR:BOOL=OFF",
                                                 "SMTG_USE_STATIC_CRT:BOOL=OFF",
                                                 "SMTG_USE_STDATOMIC_H:INTERNAL=",
                                                 "SMTG_VSTSDK_GENERATE_MACOS_IOS_COLLECTION_TARGETS:BOOL=OFF",
                                                 "SMTG_WIN:INTERNAL=TRUE",
                                                 "_Python3_Compiler_REASON_FAILURE:INTERNAL=",
                                                 "_Python3_Development_REASON_FAILURE:INTERNAL=",
                                                 "_Python3_EXECUTABLE:INTERNAL=C:/Python313/python.exe",
                                                 "_Python3_EXECUTABLE_DEBUG:INTERNAL=_Python3_EXECUTABLE_DEBUG-NOTFOUND",
                                                 "_Python3_INTERPRETER_PROPERTIES:INTERNAL=Python;3;13;4;64;32;\u003cnone\u003e;cp313-win_amd64;;C:/Python313/Lib;C:/Python313/Lib;C:/Python313/Lib/site-packages;C:/Python313/Lib/site-packages",
                                                 "_Python3_INTERPRETER_SIGNATURE:INTERNAL=20537deaa1c9eff5ca863a94b55c64d4",
                                                 "_Python3_NumPy_REASON_FAILURE:INTERNAL=",
                                                 "effetune_dsp_BINARY_DIR:STATIC=G:/home/miv-sound/build/windows-release/external/effetune/dsp",
                                                 "effetune_dsp_IS_TOP_LEVEL:STATIC=OFF",
                                                 "effetune_dsp_SOURCE_DIR:STATIC=G:/home/miv-sound/external/effetune/dsp",
                                                 "vstsdk_BINARY_DIR:STATIC=G:/home/miv-sound/build/windows-release/_deps/vst3sdk-build",
                                                 "vstsdk_IS_TOP_LEVEL:STATIC=OFF",
                                                 "vstsdk_SOURCE_DIR:STATIC=G:/home/miv-sound/build/windows-release/_deps/vst3sdk-src"
                                             ],
                           "registered_tests":  [
                                                    {
                                                        "name":  "effetune_tree_model_embedding_tests",
                                                        "command":  [
                                                                        "C:/Python313/python.exe",
                                                                        "G:/home/miv-sound/external/effetune/dsp/plugins/analyzer/tree_models/embed_models_test.py"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_graph_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_graph_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_parity_runner_help",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune-dsp-parity-runner.exe",
                                                                        "--help"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_pffft_incremental_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pffft_incremental_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_ar_interpolator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_ar_interpolator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_biquad_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_biquad_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_halfband_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_halfband_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_partitioned_convolver_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_partitioned_convolver_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_phase3b_group_b_descriptor_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_phase3b_group_b_descriptor_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_level_meter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_level_meter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_oscilloscope_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_oscilloscope_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_analog_meter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_analog_meter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_chroma_spiral_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_chroma_spiral_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_spectrum_analyzer_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spectrum_analyzer_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_spectrogram_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spectrogram_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_pitch_meter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pitch_meter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_rhythm_analyzer_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_rhythm_analyzer_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_rhythm_analyzer_eval",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_rhythm_analyzer_eval.exe",
                                                                        "--help"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_note_spectrogram_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_note_spectrogram_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_note_model_fixture",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_note_spectrogram_tests.exe",
                                                                        "--model-fixture",
                                                                        "G:/home/miv-sound/external/effetune/dsp/plugins/analyzer/note_spectrogram/fixtures"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_sfz_note_player_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_sfz_note_player_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_heap_tree_model_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_heap_tree_model_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_stereo_meter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_stereo_meter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_basics_topology_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_basics_topology_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_fir_crossover_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_fir_crossover_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_bass_management_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bass_management_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_primitives_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_primitives_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_spectrum_tap_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spectrum_tap_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_dynamics_group_a_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_dynamics_group_a_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_dynamics_group_b_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_dynamics_group_b_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_power_amp_sag_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_power_amp_sag_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_multiband_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_multiband_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_wave3c_saturation_descriptor_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_wave3c_saturation_descriptor_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_wave3c_sub_synth_state_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_wave3c_sub_synth_state_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_bluetooth_sbc_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bluetooth_sbc_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_bluetooth_sbc_conformance_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bluetooth_sbc_conformance_tests.exe",
                                                                        "--self-test"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_digital_error_emulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_digital_error_emulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_dsd64_imd_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_dsd64_imd_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_fm_radio_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_fm_radio_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_g726_adpcm_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_g726_adpcm_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_gsm_full_rate_conformance_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_gsm_full_rate_conformance_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_gsm_full_rate_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_gsm_full_rate_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_mp3_codec_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_mp3_codec_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_md_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_md_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_five_band_dynamic_eq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_five_band_dynamic_eq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_auto_filter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_auto_filter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_auto_pan_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_auto_pan_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_chorus_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_chorus_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_doppler_distortion_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_doppler_distortion_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_frequency_shifter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_frequency_shifter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_phaser_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_phaser_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_pitch_shifter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pitch_shifter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_pitch_shifter_hq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pitch_shifter_hq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_rotary_speaker_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_rotary_speaker_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_am_radio_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_am_radio_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_sw_radio_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_sw_radio_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_cassette_artifacts_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_cassette_artifacts_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_tape_artifacts_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tape_artifacts_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_tv_audio_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tv_audio_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_vinyl_artifacts_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_vinyl_artifacts_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_vinyl_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_vinyl_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_wow_flutter_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_wow_flutter_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_horn_resonator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_horn_resonator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_adaptive_prediction_effect_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_adaptive_prediction_effect_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_modal_resonator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_modal_resonator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_click_remover_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_click_remover_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_clip_restorer_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_clip_restorer_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_hum_remover_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_hum_remover_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_attack_tonal_balance_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_attack_tonal_balance_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_noise_reduction_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_noise_reduction_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_spatial_mapper_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spatial_mapper_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_reverb_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_reverb_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_ir_reverb_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_ir_reverb_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_room_eq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_room_eq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_five_band_fir_peq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_five_band_fir_peq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_group_delay_eq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_group_delay_eq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_group_delay_peq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_group_delay_peq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_tonal_balance_eq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tonal_balance_eq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_bandwidth_extender_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bandwidth_extender_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_bass_extender_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bass_extender_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_multiband_saturation_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_multiband_saturation_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_tube_simulator_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tube_simulator_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_multiband_balance_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_multiband_balance_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_crosstalk_cancellation_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_crosstalk_cancellation_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_phase_select_eq_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_phase_select_eq_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_codegen_tests",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "G:/home/miv-sound/external/effetune/dsp/test/codegen_test.mjs"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_dsp_tube_simulator_standard_v1_projection",
                                                        "command":  [
                                                                        "C:/Program Files/CMake/bin/cmake.exe",
                                                                        "-E",
                                                                        "env",
                                                                        "ET_TUBE_NATIVE_RUNNER=G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune-dsp-parity-runner.exe",
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "--test",
                                                                        "G:/home/miv-sound/external/effetune/dsp/../tests/esm/dsp-tube-simulator-standard-v1-fixture.test.mjs"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_headless_help",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/src/tools/effetune-headless.exe",
                                                                        "--help"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/src/tools"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_vst_unit_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_vst_unit_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst_unit_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "miv_startup_config_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/miv_startup_config_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_startup_config_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_upstream_integration_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_upstream_integration_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_upstream_integration_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  600.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_automation_binding_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_automation_binding_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_automation_binding_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_automation_scheduler_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_automation_scheduler_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_automation_scheduler_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_engine_batch_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_engine_batch_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_engine_batch_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_automation_parameter_bank_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_automation_parameter_bank_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_automation_parameter_bank_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_resampler_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_resampler_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_resampler_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_file_exchange_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_file_exchange_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_file_exchange_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_json_storage_compatibility_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_json_storage_compatibility_tests.exe",
                                                                        "G:/home/miv-sound/tests/fixtures/json-storage"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_json_storage_compatibility_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "miv_webview_permission_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/miv_webview_permission_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_webview_permission_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "miv_processor_startup_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/miv_processor_startup_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_processor_startup_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_webview_test_lifetime_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_webview_test_lifetime_tests.exe",
                                                                        "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_webview_test_lifetime_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_choc_webview_header_contract",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "G:/home/miv-sound/tests/esm/choc-webview-header.test.mjs"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_choc_webview_header_contract"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "miv_sound_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/miv_sound_tests.exe",
                                                                        "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3",
                                                                        "G:/home/miv-sound/tests/fixtures/miv-mixwright-state.json"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_sound_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  300.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_now_playing_metadata_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_now_playing_metadata_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_now_playing_metadata_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_plugin_processing_lifecycle_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_plugin_processing_lifecycle_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_plugin_processing_lifecycle_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  300.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_plugin_view_scaling_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_plugin_view_scaling_tests.exe",
                                                                        "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_plugin_view_scaling_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  300.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_webview_initialisation_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_webview_initialisation_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_webview_initialisation_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_webview_smoke",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_webview_smoke.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_webview_smoke"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  2400.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "miv_original_visualizer_tests",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "--test",
                                                                        "G:/home/miv-sound/tests/esm/miv-original-visualizer.test.mjs"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_original_visualizer_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_json_storage_ui_compatibility",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "G:/home/miv-sound/tests/esm/json-storage-compatibility.test.mjs",
                                                                        "--assets",
                                                                        "G:/home/miv-sound/build/windows-release/webview-assets",
                                                                        "--fixtures",
                                                                        "G:/home/miv-sound/tests/fixtures/json-storage"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_json_storage_ui_compatibility"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_vst_upstream_ui_tests",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "G:/home/miv-sound/tests/esm/vst-upstream-ui.test.mjs",
                                                                        "--assets",
                                                                        "G:/home/miv-sound/build/windows-release/webview-assets"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst_upstream_ui_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_vst_audio_manager_tests",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "--test",
                                                                        "G:/home/miv-sound/tests/esm/vst-audio-manager.test.mjs"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst_audio_manager_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_ui_asset_contract",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "G:/home/miv-sound/tools/verify-ui-assets.mjs",
                                                                        "--assets",
                                                                        "G:/home/miv-sound/build/windows-release/webview-assets",
                                                                        "--source",
                                                                        "G:/home/miv-sound/external/effetune"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_ui_asset_contract"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_vst3_ui_asset_contract",
                                                        "command":  [
                                                                        "C:/Program Files/nodejs/node.exe",
                                                                        "G:/home/miv-sound/tools/verify-ui-assets.mjs",
                                                                        "--assets",
                                                                        "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/Resources/webview",
                                                                        "--source",
                                                                        "G:/home/miv-sound/external/effetune"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst3_ui_asset_contract"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_vst3_validator",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/bin/validator.exe",
                                                                        "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst3_validator"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  120.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    },
                                                    {
                                                        "name":  "effetune_sfz_library_tests",
                                                        "command":  [
                                                                        "G:/home/miv-sound/build/windows-release/tests/effetune_sfz_library_tests.exe"
                                                                    ],
                                                        "properties":  [
                                                                           {
                                                                               "name":  "ENVIRONMENT",
                                                                               "value":  [
                                                                                             "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_sfz_library_tests"
                                                                                         ]
                                                                           },
                                                                           {
                                                                               "name":  "TIMEOUT",
                                                                               "value":  1200.0
                                                                           },
                                                                           {
                                                                               "name":  "WORKING_DIRECTORY",
                                                                               "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                           }
                                                                       ]
                                                    }
                                                ],
                           "selected_tests":  [
                                                  {
                                                      "name":  "effetune_tree_model_embedding_tests",
                                                      "command":  [
                                                                      "C:/Python313/python.exe",
                                                                      "G:/home/miv-sound/external/effetune/dsp/plugins/analyzer/tree_models/embed_models_test.py"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_graph_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_graph_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_parity_runner_help",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune-dsp-parity-runner.exe",
                                                                      "--help"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_pffft_incremental_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pffft_incremental_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_ar_interpolator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_ar_interpolator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_biquad_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_biquad_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_halfband_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_halfband_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_partitioned_convolver_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_partitioned_convolver_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_phase3b_group_b_descriptor_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_phase3b_group_b_descriptor_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_level_meter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_level_meter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_oscilloscope_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_oscilloscope_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_analog_meter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_analog_meter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_chroma_spiral_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_chroma_spiral_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_spectrum_analyzer_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spectrum_analyzer_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_spectrogram_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spectrogram_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_pitch_meter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pitch_meter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_rhythm_analyzer_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_rhythm_analyzer_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_rhythm_analyzer_eval",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_rhythm_analyzer_eval.exe",
                                                                      "--help"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_note_spectrogram_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_note_spectrogram_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_note_model_fixture",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_note_spectrogram_tests.exe",
                                                                      "--model-fixture",
                                                                      "G:/home/miv-sound/external/effetune/dsp/plugins/analyzer/note_spectrogram/fixtures"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_sfz_note_player_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_sfz_note_player_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_heap_tree_model_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_heap_tree_model_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_stereo_meter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_stereo_meter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_basics_topology_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_basics_topology_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_fir_crossover_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_fir_crossover_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_bass_management_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bass_management_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_primitives_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_primitives_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_spectrum_tap_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spectrum_tap_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_dynamics_group_a_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_dynamics_group_a_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_dynamics_group_b_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_dynamics_group_b_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_power_amp_sag_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_power_amp_sag_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_multiband_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_multiband_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_wave3c_saturation_descriptor_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_wave3c_saturation_descriptor_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_wave3c_sub_synth_state_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_wave3c_sub_synth_state_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_bluetooth_sbc_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bluetooth_sbc_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_bluetooth_sbc_conformance_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bluetooth_sbc_conformance_tests.exe",
                                                                      "--self-test"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_digital_error_emulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_digital_error_emulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_dsd64_imd_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_dsd64_imd_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_fm_radio_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_fm_radio_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_g726_adpcm_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_g726_adpcm_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_gsm_full_rate_conformance_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_gsm_full_rate_conformance_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_gsm_full_rate_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_gsm_full_rate_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_mp3_codec_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_mp3_codec_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_md_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_md_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_five_band_dynamic_eq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_five_band_dynamic_eq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_auto_filter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_auto_filter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_auto_pan_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_auto_pan_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_chorus_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_chorus_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_doppler_distortion_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_doppler_distortion_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_frequency_shifter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_frequency_shifter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_phaser_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_phaser_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_pitch_shifter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pitch_shifter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_pitch_shifter_hq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_pitch_shifter_hq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_rotary_speaker_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_rotary_speaker_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_am_radio_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_am_radio_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_sw_radio_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_sw_radio_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_cassette_artifacts_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_cassette_artifacts_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_tape_artifacts_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tape_artifacts_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_tv_audio_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tv_audio_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_vinyl_artifacts_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_vinyl_artifacts_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_vinyl_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_vinyl_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_wow_flutter_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_wow_flutter_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_horn_resonator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_horn_resonator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_adaptive_prediction_effect_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_adaptive_prediction_effect_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_modal_resonator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_modal_resonator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_click_remover_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_click_remover_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_clip_restorer_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_clip_restorer_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_hum_remover_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_hum_remover_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_attack_tonal_balance_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_attack_tonal_balance_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_noise_reduction_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_noise_reduction_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_spatial_mapper_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_spatial_mapper_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_reverb_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_reverb_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_ir_reverb_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_ir_reverb_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_room_eq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_room_eq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_five_band_fir_peq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_five_band_fir_peq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_group_delay_eq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_group_delay_eq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_group_delay_peq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_group_delay_peq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_tonal_balance_eq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tonal_balance_eq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_bandwidth_extender_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bandwidth_extender_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_bass_extender_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_bass_extender_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_multiband_saturation_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_multiband_saturation_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_tube_simulator_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_tube_simulator_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_multiband_balance_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_multiband_balance_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_crosstalk_cancellation_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_crosstalk_cancellation_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_phase_select_eq_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune_dsp_phase_select_eq_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_codegen_tests",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "G:/home/miv-sound/external/effetune/dsp/test/codegen_test.mjs"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_dsp_tube_simulator_standard_v1_projection",
                                                      "command":  [
                                                                      "C:/Program Files/CMake/bin/cmake.exe",
                                                                      "-E",
                                                                      "env",
                                                                      "ET_TUBE_NATIVE_RUNNER=G:/home/miv-sound/build/windows-release/external/effetune/dsp/effetune-dsp-parity-runner.exe",
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "--test",
                                                                      "G:/home/miv-sound/external/effetune/dsp/../tests/esm/dsp-tube-simulator-standard-v1-fixture.test.mjs"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/external/effetune/dsp"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_headless_help",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/src/tools/effetune-headless.exe",
                                                                      "--help"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/src/tools"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_vst_unit_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_vst_unit_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst_unit_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "miv_startup_config_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/miv_startup_config_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_startup_config_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_upstream_integration_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_upstream_integration_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_upstream_integration_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  600.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_automation_binding_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_automation_binding_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_automation_binding_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_automation_scheduler_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_automation_scheduler_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_automation_scheduler_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_engine_batch_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_engine_batch_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_engine_batch_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_automation_parameter_bank_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_automation_parameter_bank_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_automation_parameter_bank_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_resampler_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_resampler_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_resampler_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_file_exchange_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_file_exchange_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_file_exchange_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_json_storage_compatibility_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_json_storage_compatibility_tests.exe",
                                                                      "G:/home/miv-sound/tests/fixtures/json-storage"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_json_storage_compatibility_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "miv_webview_permission_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/miv_webview_permission_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_webview_permission_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "miv_processor_startup_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/miv_processor_startup_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_processor_startup_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_webview_test_lifetime_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_webview_test_lifetime_tests.exe",
                                                                      "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_webview_test_lifetime_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_choc_webview_header_contract",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "G:/home/miv-sound/tests/esm/choc-webview-header.test.mjs"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_choc_webview_header_contract"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "miv_sound_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/miv_sound_tests.exe",
                                                                      "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3",
                                                                      "G:/home/miv-sound/tests/fixtures/miv-mixwright-state.json"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_sound_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  300.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_now_playing_metadata_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_now_playing_metadata_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_now_playing_metadata_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_plugin_processing_lifecycle_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_plugin_processing_lifecycle_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_plugin_processing_lifecycle_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  300.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_plugin_view_scaling_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_plugin_view_scaling_tests.exe",
                                                                      "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/x86_64-win/mIV Sound.vst3"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_plugin_view_scaling_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  300.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_webview_initialisation_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_webview_initialisation_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_webview_initialisation_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_webview_smoke",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_webview_smoke.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_webview_smoke"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  2400.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "miv_original_visualizer_tests",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "--test",
                                                                      "G:/home/miv-sound/tests/esm/miv-original-visualizer.test.mjs"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/miv_original_visualizer_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_json_storage_ui_compatibility",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "G:/home/miv-sound/tests/esm/json-storage-compatibility.test.mjs",
                                                                      "--assets",
                                                                      "G:/home/miv-sound/build/windows-release/webview-assets",
                                                                      "--fixtures",
                                                                      "G:/home/miv-sound/tests/fixtures/json-storage"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_json_storage_ui_compatibility"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_vst_upstream_ui_tests",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "G:/home/miv-sound/tests/esm/vst-upstream-ui.test.mjs",
                                                                      "--assets",
                                                                      "G:/home/miv-sound/build/windows-release/webview-assets"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst_upstream_ui_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_vst_audio_manager_tests",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "--test",
                                                                      "G:/home/miv-sound/tests/esm/vst-audio-manager.test.mjs"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst_audio_manager_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_ui_asset_contract",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "G:/home/miv-sound/tools/verify-ui-assets.mjs",
                                                                      "--assets",
                                                                      "G:/home/miv-sound/build/windows-release/webview-assets",
                                                                      "--source",
                                                                      "G:/home/miv-sound/external/effetune"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_ui_asset_contract"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_vst3_ui_asset_contract",
                                                      "command":  [
                                                                      "C:/Program Files/nodejs/node.exe",
                                                                      "G:/home/miv-sound/tools/verify-ui-assets.mjs",
                                                                      "--assets",
                                                                      "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3/Contents/Resources/webview",
                                                                      "--source",
                                                                      "G:/home/miv-sound/external/effetune"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst3_ui_asset_contract"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_vst3_validator",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/bin/validator.exe",
                                                                      "G:/home/miv-sound/build/windows-release/VST3/Release/mIV Sound.vst3"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_vst3_validator"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  120.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  },
                                                  {
                                                      "name":  "effetune_sfz_library_tests",
                                                      "command":  [
                                                                      "G:/home/miv-sound/build/windows-release/tests/effetune_sfz_library_tests.exe"
                                                                  ],
                                                      "properties":  [
                                                                         {
                                                                             "name":  "ENVIRONMENT",
                                                                             "value":  [
                                                                                           "MIV_SOUND_DATA_DIR=G:/home/miv-sound/build/windows-release/tests/miv-sound-test-data/effetune_sfz_library_tests"
                                                                                       ]
                                                                         },
                                                                         {
                                                                             "name":  "TIMEOUT",
                                                                             "value":  1200.0
                                                                         },
                                                                         {
                                                                             "name":  "WORKING_DIRECTORY",
                                                                             "value":  "G:/home/miv-sound/build/windows-release/tests"
                                                                         }
                                                                     ]
                                                  }
                                              ]
                       },
    "build_script_sha256":  "1ef025ac8ec7cae5c1570647f7652a8444fb49bca9a8e5f6f0bf334cae61d98f",
    "tools":  {
                  "cmake_path":  "C:\\Program Files\\CMake\\bin\\cmake.exe",
                  "cmake":  "cmake version 4.3.1\r\n\r\nCMake suite maintained and supported by Kitware (kitware.com/cmake).",
                  "node":  "v22.16.0",
                  "vst3sdk_pin":  "3cdf9ca5d1f5b1b21e0a86832aa4abe55607bd96",
                  "vst3sdk_submodules":  [
                                             {
                                                 "path":  "base",
                                                 "commit":  "fcf9da0bd27a16f7f03773a3a39822f28f5c8477"
                                             },
                                             {
                                                 "path":  "cmake",
                                                 "commit":  "054c9143cbb8d47fc4694e473f2ee3b4d951a8f5"
                                             },
                                             {
                                                 "path":  "doc",
                                                 "commit":  "8bfca19d3b76a61d093951ba9297047f544caea1",
                                                 "initialized":  false
                                             },
                                             {
                                                 "path":  "pluginterfaces",
                                                 "commit":  "4f547e8e102b47de4a8b8aaf343c73b700786372"
                                             },
                                             {
                                                 "path":  "public.sdk",
                                                 "commit":  "586dc5e6c8012c3e4b01c79389375cbe96bdb1da"
                                             },
                                             {
                                                 "path":  "tutorials",
                                                 "commit":  "33b73dfbb87f3fde3bce8c0a10cae934dc66ad34",
                                                 "initialized":  false
                                             },
                                             {
                                                 "path":  "vstgui4",
                                                 "commit":  "5db272256172557818b6158cf0bb2c4410bddb25",
                                                 "initialized":  false
                                             }
                                         ]
              },
    "upstream_licenses":  [
                              {
                                  "path":  "LICENSE",
                                  "text":  "MIT License\n\nCopyright (c) 2026, Yoshiyuki Kobayashi\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software is\nfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all\ncopies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\nAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\nLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\nOUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\nSOFTWARE.\n",
                                  "sha256":  "771e30f94cdc5216eea41da3bb425150a1c0c8c4453ab77636bfe61d34387462"
                              },
                              {
                                  "path":  "external/effetune/LICENSE",
                                  "text":  "MIT License\n\nCopyright (c) 2025-2026, Yoshiyuki Kobayashi\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software is\nfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all\ncopies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\nAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\nLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\nOUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\nSOFTWARE.\n",
                                  "sha256":  "ed53876b0cceb6819e76675556f3a3146ab9c3fea84a8135ad237adadcf64d19"
                              }
                          ]
}
