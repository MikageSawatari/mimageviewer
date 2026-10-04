// Inert DLL for loader-path tests. No VST factory, editor or product code.
#include <windows.h>

extern "C" __declspec(dllexport) DWORD fixture_module_filename(wchar_t* buffer, DWORD size) {
    HMODULE module = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            reinterpret_cast<LPCWSTR>(&fixture_module_filename), &module)) return 0;
    return GetModuleFileNameW(module, buffer, size);
}
