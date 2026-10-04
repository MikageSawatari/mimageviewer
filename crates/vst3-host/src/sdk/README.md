# Maintained Windows hosting module

`module_win32.cpp` is an MIT-licensed copy of VST3 SDK 3.8.0's
`public.sdk/source/vst/hosting/module_win32.cpp`. The original file's SHA-256 is
`951e0a0b824c8e1b0c43c757203f8f85dceb9b237dac6f2ec1d9cade61178b37`.
The adjacent `LICENSE.txt` preserves Steinberg's copyright and MIT license.

mImageViewer changes use explicit UTF-8 to UTF-16 conversion for package/DLL
paths, discovery, module metadata, snapshots and diagnostics. No process-wide
active-code-page setting is changed. CMake compiles this tracked copy, leaving
the downloaded SDK unchanged. When upgrading the SDK, reconcile upstream edits
with these Windows path boundaries and retain the MIT attribution.
