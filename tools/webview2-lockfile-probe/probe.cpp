// Standalone experiment. No mIV, CHOC WebView, or product profile is instantiated.
#include <windows.h>
#include <shellapi.h>
#include <shlwapi.h>
#include <RestartManager.h>
#include <wbemidl.h>
#include <wrl.h>
#include <wrl/event.h>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <map>
#include <sstream>
#include <iomanip>
#include <stdexcept>
#include <vector>
#include <memory>
#include "choc_webview2_declarations.h"

namespace fs = std::filesystem;
using Microsoft::WRL::ComPtr;
using Microsoft::WRL::Callback;
static constexpr wchar_t homeURI[] = L"https://effetune-mixwright.localhost/";

static std::string utf8(const std::wstring& value) {
    if (value.empty()) return {};
    const int size = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    std::string result(size, '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), result.data(), size, nullptr, nullptr);
    return result;
}
static std::string quote(const std::string& s) {
    std::ostringstream out; out << '"';
    for (const unsigned char c : s) {
        if (c == '"' || c == '\\') out << '\\' << c;
        else if (c < 32) out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << static_cast<unsigned>(c);
        else out << c;
    }
    out << '"'; return out.str();
}
static std::string hrhex(HRESULT hr) {
    std::ostringstream out; out << "0x" << std::hex << std::uppercase << std::setw(8) << std::setfill('0') << static_cast<uint32_t>(hr);
    return out.str();
}
static fs::path exePath() {
    wchar_t buffer[32768];
    const auto size = GetModuleFileNameW(nullptr, buffer, 32768);
    if (!size || size == 32768) throw std::runtime_error("GetModuleFileNameW failed");
    return fs::path(std::wstring(buffer, size));
}
static std::vector<BYTE> loadPage() {
    // Load the fixed asset before starting the UI/message pump. Requests never
    // map a URI to a filesystem path or perform disk I/O in the callback.
    std::ifstream input(exePath().parent_path() / L"page.html", std::ios::binary | std::ios::ate);
    if (!input) throw std::runtime_error("Cannot read probe page.html");
    const auto size = input.tellg();
    if (size <= 0 || size > 1024 * 1024) throw std::runtime_error("Invalid probe page size");
    std::vector<BYTE> bytes(static_cast<size_t>(size));
    input.seekg(0);
    if (!input.read(reinterpret_cast<char*>(bytes.data()), static_cast<std::streamsize>(bytes.size())))
        throw std::runtime_error("Cannot load probe page.html");
    return bytes;
}
struct Resource {
    int status = 404;
    const wchar_t* reason = L"Not Found";
    const wchar_t* headers = nullptr;
    ComPtr<IStream> stream;
};
static Resource resourceForURI(const std::wstring& uri, const std::vector<BYTE>& page) {
    Resource result;
    // Only one preloaded asset is served. Unknown paths, encoded paths and
    // lookalike origins receive 404; no request is used as an on-disk path.
    if (uri.substr(0, uri.find_first_of(L"?#")) == std::wstring(homeURI) + L"page.html") {
        result.status = 200; result.reason = L"OK";
        result.headers = L"Content-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *";
        result.stream.Attach(SHCreateMemStream(page.data(), static_cast<UINT>(page.size())));
        if (!result.stream) throw std::runtime_error("SHCreateMemStream failed");
    }
    return result;
}
static fs::path checkedPath(const fs::path& path) {
    const auto root = exePath().parent_path().parent_path();
    if (root.filename() != L"webview2-lockfile-probe" || root.parent_path().filename() != L"target")
        throw std::runtime_error("Executable must remain in target/webview2-lockfile-probe/bin");
    const auto full = fs::absolute(path).lexically_normal();
    auto prefix = root.wstring() + L"\\";
    const auto name = full.wstring();
    if (name.size() <= prefix.size() || CompareStringOrdinal(name.data(), static_cast<int>(prefix.size()), prefix.data(), static_cast<int>(prefix.size()), TRUE) != CSTR_EQUAL)
        throw std::runtime_error("Path outside disposable probe root");
    const auto relative = name.substr(prefix.size());
    if (relative.rfind(L"run-", 0) != 0 || relative.find(L'\\') == std::wstring::npos)
        throw std::runtime_error("Path must be inside a run-* directory");
    fs::path cursor;
    for (const auto& component : full) {
        cursor /= component;
        const DWORD attrs = GetFileAttributesW(cursor.c_str());
        if (attrs != INVALID_FILE_ATTRIBUTES && (attrs & FILE_ATTRIBUTE_REPARSE_POINT))
            throw std::runtime_error("Reparse points are forbidden");
    }
    return full;
}
#include "snapshot.h"

struct Log {
    fs::path path;
    ULONGLONG start = GetTickCount64();
    bool preserveShutdown = false;
    mutable bool ioFailed = false;
    void emit(const std::string& kind, const std::string& fields = {}) const {
        try { write(kind, fields); }
        catch (...) {
            if (!preserveShutdown) throw;
            // Diagnostic failure must never bypass the host's Close/wait owner.
            // The host exits unsuccessfully; missing evidence is also rejected
            // by the runner. No retry, replacement log or suppressed dialog.
            ioFailed = true;
        }
    }
    void write(const std::string& kind, const std::string& fields) const {
        std::string line = "{\"kind\":" + quote(kind) + ",\"elapsed_ms\":" + std::to_string(GetTickCount64() - start)
            + ",\"host_pid\":" + std::to_string(GetCurrentProcessId()) + (fields.empty() ? "" : "," + fields) + "}";
        if (!path.empty()) {
            // Change the diagnostic file only, not standard handles or WebView2
            // process-launch inheritance. nullptr security attributes => noninheritable.
            HANDLE file = CreateFileW(path.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
            if (file == INVALID_HANDLE_VALUE) throw std::runtime_error("Cannot open event log: " + std::to_string(GetLastError()));
            DWORD flags = 0;
            const BOOL info = GetHandleInformation(file, &flags);
            const DWORD infoError = info ? 0 : GetLastError();
            line.pop_back();
            line += ",\"log_handle_flags\":" + std::to_string(flags) + ",\"log_handle_info_error\":" + std::to_string(infoError)
                + ",\"log_handle_inheritable\":" + ((flags & HANDLE_FLAG_INHERIT) ? "true" : "false") + "}\n";
            DWORD written = 0;
            const BOOL ok = WriteFile(file, line.data(), static_cast<DWORD>(line.size()), &written, nullptr);
            CloseHandle(file);
            if (!ok || written != line.size()) throw std::runtime_error("Cannot write event log");
        }
        else std::cout << line << std::endl;
    }
    void hr(const std::string& stage, HRESULT result) const {
        emit("hresult", "\"stage\":" + quote(stage) + ",\"hresult\":" + quote(hrhex(result)) + ",\"failed\":" + (FAILED(result) ? "true" : "false"));
    }
    void standardHandles(const std::string& phase) const {
        const std::pair<const char*, DWORD> handles[] = {{"stdout", STD_OUTPUT_HANDLE}, {"stderr", STD_ERROR_HANDLE}, {"stdin", STD_INPUT_HANDLE}};
        for (const auto& entry : handles) {
            HANDLE handle = GetStdHandle(entry.second);
            DWORD flags = 0;
            const BOOL ok = GetHandleInformation(handle, &flags);
            const DWORD error = ok ? 0 : GetLastError();
            emit("standard_handle", "\"phase\":" + quote(phase) + ",\"name\":" + quote(entry.first)
                + ",\"value\":" + std::to_string(reinterpret_cast<uintptr_t>(handle)) + ",\"info_error\":" + std::to_string(error)
                + ",\"flags\":" + std::to_string(flags) + ",\"inheritable\":" + ((flags & HANDLE_FLAG_INHERIT) ? "true" : "false")
                + ",\"file_type\":" + std::to_string(GetFileType(handle)));
        }
    }
};
struct Args {
    std::map<std::wstring, std::wstring> values;
    Args(int argc, wchar_t** argv) {
        for (int i = 2; i < argc; i += 2) {
            if (i + 1 >= argc || std::wstring(argv[i]).rfind(L"--", 0) != 0) throw std::runtime_error("Expected --key value");
            if (!values.emplace(argv[i], argv[i + 1]).second) throw std::runtime_error("Duplicate argument");
        }
    }
    std::wstring get(const wchar_t* key, const std::wstring& fallback = {}) const {
        auto it = values.find(key); return it == values.end() ? fallback : it->second;
    }
    fs::path path(const wchar_t* key) const {
        const auto value = get(key); if (value.empty()) throw std::runtime_error("Required path argument missing");
        return checkedPath(value);
    }
    DWORD timeout(DWORD fallback) const {
        const auto value = std::stoul(get(L"--timeout-ms", std::to_wstring(fallback)));
        if (value < 100 || value > 300000) throw std::runtime_error("Timeout outside 100..300000 ms");
        return static_cast<DWORD>(value);
    }
};
static HANDLE openLock(const fs::path& udf, bool create) {
    return CreateFileW((udf / L"EBWebView" / L"lockfile").c_str(), GENERIC_READ | GENERIC_WRITE,
        0, nullptr, create ? CREATE_NEW : OPEN_EXISTING,
        create ? FILE_ATTRIBUTE_NORMAL | FILE_FLAG_DELETE_ON_CLOSE : FILE_ATTRIBUTE_NORMAL, nullptr);
}
static int lockMain(const Args& args) {
    const auto udf = args.path(L"--udf");
    Log log; if (!args.get(L"--log").empty()) log.path = args.path(L"--log");
    const bool create = args.get(L"--create-new", L"0") == L"1";
    // Hold handles across CreateFile to eliminate PID reuse and process-exit races.
    std::vector<HANDLE> processes;
    std::wstringstream ids(args.get(L"--pids")); std::wstring id;
    while (std::getline(ids, id, L',')) if (!id.empty()) {
        HANDLE p = OpenProcess(SYNCHRONIZE, FALSE, std::stoul(id));
        if (p) processes.push_back(p);
    }
    auto alive = [&]() { unsigned count = 0; for (const auto p : processes) if (WaitForSingleObject(p, 0) == WAIT_TIMEOUT) ++count; return count; };
    const auto before = alive();
    HANDLE handle = openLock(udf, create);
    const DWORD error = handle == INVALID_HANDLE_VALUE ? GetLastError() : ERROR_SUCCESS;
    const auto after = alive();
    log.emit("lock_attempt", "\"opened\":" + std::string(error == 0 ? "true" : "false") + ",\"win32_error\":" + std::to_string(error)
        + ",\"create_new\":" + (create ? "true" : "false") + ",\"alive_before\":" + std::to_string(before) + ",\"alive_after\":" + std::to_string(after));
    if (!log.path.empty()) log.standardHandles("lock_helper");
    for (const auto p : processes) CloseHandle(p);
    if (error) return 2;
    if (!args.get(L"--release").empty()) {
        const auto release = args.path(L"--release");
        const auto deadline = GetTickCount64() + args.timeout(180000);
        while (!fs::exists(release) && GetTickCount64() < deadline) Sleep(50);
        if (!fs::exists(release)) { CloseHandle(handle); log.emit("holder_timeout"); return 3; }
    }
    CloseHandle(handle);
    log.emit("lock_closed", "\"lockfile_exists\":" + std::string(fs::exists(udf / L"EBWebView" / L"lockfile") ? "true" : "false"));
    return 0;
}

struct Host {
    Log log;
    ComPtr<ICoreWebView2Environment> env;
    ComPtr<ICoreWebView2Controller> controller;
    ComPtr<ICoreWebView2> view;
    std::vector<BYTE> page;
    fs::path stop;
    fs::path udf;
    ComPtr<IWbemServices> inventory;
    HWND window = nullptr;
    std::wstring mode, token;
    ULONGLONG deadline = 0, nextHeartbeat = 0;
    bool pageResult = false, failed = false;
    bool closing = false, environmentPending = false, controllerPending = false;
    bool webviewAttempted = false;
    bool shutdownCloseFailed = false;
    std::map<DWORD, HANDLE> shutdownProcesses;
    DWORD shutdownTimeout = 30000;
    void callbackError(const std::exception& error) noexcept {
        failed = true;
        try { log.emit("host_error", "\"error\":" + quote(error.what())); }
        catch (...) { log.ioFailed = true; }
        PostQuitMessage(10);
    }
    template<typename F> HRESULT callback(F&& body) noexcept {
        try { return body(); }
        catch (const std::exception& error) { callbackError(error); return E_FAIL; }
        catch (...) { failed = true; log.ioFailed = true; PostQuitMessage(10); return E_FAIL; }
    }
    // SDK WMI declarations only. No injected Chromium switches or error-mode
    // changes: shutdown is Close + release + an exact-UDF natural-exit wait.
    void prepareInventory() {
        ComPtr<IWbemLocator> locator;
        HRESULT hr = CoCreateInstance(CLSID_WbemLocator, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&locator));
        if (FAILED(hr)) throw std::runtime_error("WMI locator: " + hrhex(hr));
        BSTR name = SysAllocString(L"ROOT\\CIMV2");
        hr = locator->ConnectServer(name, nullptr, nullptr, nullptr, WBEM_FLAG_CONNECT_USE_MAX_WAIT, nullptr, nullptr, &inventory);
        SysFreeString(name);
        if (FAILED(hr)) throw std::runtime_error("WMI connection: " + hrhex(hr));
        hr = CoSetProxyBlanket(inventory.Get(), RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, nullptr,
            RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, nullptr, EOAC_NONE);
        if (FAILED(hr)) throw std::runtime_error("WMI security: " + hrhex(hr));
    }
    bool matchesUdf(const std::wstring& command) const {
        int count = 0; auto** argv = CommandLineToArgvW(command.c_str(), &count);
        if (!argv) throw std::runtime_error("Cannot parse process command line");
        bool match = false; unsigned values = 0;
        const auto normalize = [](const fs::path& path) {
            auto result = fs::absolute(path).lexically_normal().make_preferred().wstring();
            while (result.size() > 3 && (result.back() == L'\\' || result.back() == L'/')) result.pop_back();
            return result;
        };
        const auto expected = normalize(udf / L"EBWebView");
        for (int i = 1; i < count; ++i) {
            std::wstring value = argv[i];
            if (CompareStringOrdinal(value.c_str(), -1, L"--user-data-dir", -1, TRUE) == CSTR_EQUAL && i + 1 < count) value = argv[++i];
            else if (value.size() >= 16 && CompareStringOrdinal(value.data(), 16, L"--user-data-dir=", 16, TRUE) == CSTR_EQUAL) value.erase(0, 16);
            else continue;
            // Canonical absolute path, case-insensitive; no prefix match.
            ++values;
            const auto actual = normalize(value);
            match = CompareStringOrdinal(actual.c_str(), -1, expected.c_str(), -1, TRUE) == CSTR_EQUAL;
        }
        LocalFree(argv); return values == 1 && match;
    }
    unsigned matchingProcesses(ULONGLONG until) {
        ComPtr<IEnumWbemClassObject> rows;
        BSTR language = SysAllocString(L"WQL");
        BSTR query = SysAllocString(L"SELECT ProcessId,CommandLine FROM Win32_Process WHERE Name='msedgewebview2.exe'");
        const HRESULT hr = inventory->ExecQuery(language, query, WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, nullptr, &rows);
        SysFreeString(language); SysFreeString(query);
        if (FAILED(hr)) throw std::runtime_error("WMI query: " + hrhex(hr));
        unsigned matches = 0;
        for (;;) {
            if (GetTickCount64() >= until) throw std::runtime_error("Process inventory deadline");
            ComPtr<IWbemClassObject> row; ULONG returned = 0;
            const HRESULT next = rows->Next(100, 1, &row, &returned);
            if (FAILED(next)) throw std::runtime_error("WMI enumeration: " + hrhex(next));
            if (next == WBEM_S_FALSE && !returned) break;
            if (!returned) continue;
            VARIANT cmd{}, pid{};
            const HRESULT cmdHr = row->Get(L"CommandLine", 0, &cmd, nullptr, nullptr);
            const HRESULT pidHr = row->Get(L"ProcessId", 0, &pid, nullptr, nullptr);
            if (FAILED(cmdHr) || cmd.vt != VT_BSTR || FAILED(pidHr) || pid.vt != VT_I4) {
                VariantClear(&cmd); VariantClear(&pid);
                throw std::runtime_error("Unreadable WebView2 process command line/PID; quiet is unknown");
            }
            const bool match = matchesUdf(cmd.bstrVal);
            if (match) {
                ++matches;
                if (shutdownProcesses.find(pid.ulVal) == shutdownProcesses.end()) {
                    HANDLE process = OpenProcess(SYNCHRONIZE, FALSE, pid.ulVal);
                    if (!process && GetLastError() != ERROR_INVALID_PARAMETER) {
                        VariantClear(&cmd); VariantClear(&pid);
                        throw std::runtime_error("Cannot retain matching shutdown process");
                    }
                    if (process) shutdownProcesses.emplace(pid.ulVal, process);
                }
                log.emit("shutdown_process", "\"pid\":" + std::to_string(pid.ulVal) + ",\"command_line\":" + quote(utf8(cmd.bstrVal)));
            }
            VariantClear(&cmd); VariantClear(&pid);
        }
        return matches;
    }
    int shutdown(int code) {
        const auto began = GetTickCount64();
        closing = true;
        if (window) KillTimer(window, 1);
        log.emit("host_closing", stampFields(stampNow()));
        if (controller && !check("controller_close", controller->Close())) shutdownCloseFailed = true;
        view.Reset(); controller.Reset(); env.Reset();
        const auto until = GetTickCount64() + shutdownTimeout;
        ULONGLONG quietSince = 0;
        bool complete = !webviewAttempted;
        while (!complete && GetTickCount64() < until) {
            // Complete pending creation callbacks on the STA. A late controller
            // is closed immediately and never navigated, even after timeout.
            MSG msg{};
            while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
                if (msg.message != WM_QUIT) { TranslateMessage(&msg); DispatchMessageW(&msg); }
            }
            try {
                const auto count = matchingProcesses(until);
                bool retainedAlive = false;
                // Close terminated handles on each poll, keeping only real OS
                // lifetimes, not just WMI row disappearance.
                for (auto it = shutdownProcesses.begin(); it != shutdownProcesses.end();) {
                    const DWORD state = WaitForSingleObject(it->second, 0);
                    if (state == WAIT_FAILED) throw std::runtime_error("Shutdown process wait failed");
                    if (state == WAIT_OBJECT_0) { CloseHandle(it->second); it = shutdownProcesses.erase(it); }
                    else { retainedAlive = true; ++it; }
                }
                log.emit("shutdown_poll", "\"matching_processes\":" + std::to_string(count)
                    + ",\"environment_pending\":" + (environmentPending ? "true" : "false")
                    + ",\"controller_pending\":" + (controllerPending ? "true" : "false"));
                if (!count && !retainedAlive && !environmentPending && !controllerPending) {
                    if (!quietSince) quietSince = GetTickCount64();
                    complete = GetTickCount64() - quietSince >= 500;
                } else quietSince = 0;
            } catch (const std::exception& error) {
                quietSince = 0;
                log.emit("shutdown_inventory_error", "\"error\":" + quote(error.what()));
            }
            if (!complete) MsgWaitForMultipleObjects(0, nullptr, FALSE, 50, QS_ALLINPUT);
        }
        if (!complete || shutdownCloseFailed) code = 6;
        const bool success = complete && !shutdownCloseFailed;
        log.emit("shutdown_complete", "\"complete\":" + std::string(success ? "true" : "false")
            + ",\"processes_observed_exited\":" + (complete ? "true" : "false")
            + ",\"controller_close_succeeded\":" + (shutdownCloseFailed ? "false" : "true")
            + ",\"wait_elapsed_ms\":" + std::to_string(GetTickCount64() - began)
            + ",\"timed_out\":" + (complete ? "false" : "true") + ",\"timeout_ms\":" + std::to_string(shutdownTimeout));
        if (window) { DestroyWindow(window); window = nullptr; }
        for (auto process : shutdownProcesses) CloseHandle(process.second);
        shutdownProcesses.clear();
        if (log.ioFailed) code = 10;
        log.emit("host_exit", "\"exit_code\":" + std::to_string(code) + ",\"shutdown_complete\":" + (success ? "true" : "false"));
        return log.ioFailed ? 10 : code;
    }
    bool check(const std::string& stage, HRESULT hr) {
        log.hr(stage, hr); if (FAILED(hr)) failed = true; return SUCCEEDED(hr);
    }
    HRESULT onResourceRequested(ICoreWebView2WebResourceRequestedEventArgs* args) {
        if (closing) return S_OK; // environment may already have been released
        try {
            ComPtr<ICoreWebView2WebResourceRequest> request;
            if (!check("resource_request", args->get_Request(&request))) return E_FAIL;
            LPWSTR raw = nullptr;
            if (!check("resource_uri", request->get_Uri(&raw))) return E_FAIL;
            const std::wstring uri = raw ? raw : L"";
            CoTaskMemFree(raw);
            const auto resource = resourceForURI(uri, page);
            ComPtr<ICoreWebView2WebResourceResponse> response;
            if (!check("resource_response_create", env->CreateWebResourceResponse(resource.stream.Get(), resource.status,
                resource.reason, resource.headers, &response))) return E_FAIL;
            if (!check("resource_response_set", args->put_Response(response.Get()))) return E_FAIL;
            log.emit("resource_response", "\"uri\":" + quote(utf8(uri)) + ",\"status\":" + std::to_string(resource.status));
            return S_OK;
        } catch (const std::exception& error) {
            failed = true;
            log.emit("resource_error", "\"error\":" + quote(error.what()));
            return E_FAIL;
        }
    }
    static LRESULT CALLBACK wndProc(HWND hwnd, UINT msg, WPARAM w, LPARAM l) {
        auto* host = reinterpret_cast<Host*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
        try {
        if (msg == WM_NCCREATE) {
            host = static_cast<Host*>(reinterpret_cast<CREATESTRUCTW*>(l)->lpCreateParams);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(host));
        }
        if (host && msg == WM_TIMER) {
            const auto now = GetTickCount64();
            if (now >= host->nextHeartbeat) { host->log.emit("heartbeat"); host->nextHeartbeat = now + 500; }
            if (fs::exists(host->stop)) { host->log.emit("close_requested"); PostQuitMessage(0); }
            else if (host->failed || host->log.ioFailed) PostQuitMessage(2);
            else if (!host->pageResult && now >= host->deadline) { host->log.emit("startup_timeout"); PostQuitMessage(3); }
        }
        if (msg == WM_CLOSE) { PostQuitMessage(4); return 0; }
        return DefWindowProcW(hwnd, msg, w, l);
        } catch (const std::exception& error) {
            if (host) host->callbackError(error);
            else PostQuitMessage(10);
            return 0;
        } catch (...) {
            if (host) { host->failed = true; host->log.ioFailed = true; }
            PostQuitMessage(10); return 0;
        }
    }
    HRESULT onController(HRESULT result, ICoreWebView2Controller* created) {
        controllerPending = false;
        // Retain even an error callback's nonnull controller for mandatory Close.
        if (created) controller = created;
        if (closing) {
            if (controller && !check("late_controller_close", controller->Close())) shutdownCloseFailed = true;
            controller.Reset(); return S_OK;
        }
        if (!check("controller_callback", result) || !created) { failed = true; return S_OK; }
        controller = created;
        if (!check("get_view", controller->get_CoreWebView2(&view))) return S_OK;
        UINT32 pid = 0;
        if (check("browser_pid", view->get_BrowserProcessId(&pid))) log.emit("browser_pid", "\"pid\":" + std::to_string(pid));
        log.standardHandles("controller_ready");
        RECT bounds{0, 0, 640, 360};
        if (!check("bounds", controller->put_Bounds(bounds)) || !check("visible", controller->put_IsVisible(TRUE))) return S_OK;
        // Same origin transport as upstream CHOC fetchResource/customSchemeURI.
        // Use only interfaces declared by CHOC, with no extension QueryInterface.
        const auto filter = std::wstring(homeURI) + L"*";
        if (!check("resource_filter", view->AddWebResourceRequestedFilter(filter.c_str(), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL))) return S_OK;
        EventRegistrationToken resourceRegistration{};
        const auto resources = Callback<ICoreWebView2WebResourceRequestedEventHandler>([this](ICoreWebView2*, ICoreWebView2WebResourceRequestedEventArgs* args) -> HRESULT {
            return callback([&]() { return onResourceRequested(args); });
        });
        if (!check("resource_handler", view->add_WebResourceRequested(resources.Get(), &resourceRegistration))) return S_OK;
        EventRegistrationToken registration{};
        const auto handler = Callback<ICoreWebView2WebMessageReceivedEventHandler>([this](ICoreWebView2*, ICoreWebView2WebMessageReceivedEventArgs* args) -> HRESULT {
            return callback([&]() -> HRESULT {
            if (closing) return S_OK;
            LPWSTR raw = nullptr;
            if (check("message", args->TryGetWebMessageAsString(&raw)) && raw) {
                const auto text = utf8(raw); CoTaskMemFree(raw);
                log.emit("page_message", "\"data\":" + text + ",\"message_received\":{" + stampFields(stampNow()) + "}");
                if (text.find("\"kind\":\"storage_result\"") != std::string::npos) {
                    pageResult = true;
                    if (text.find("\"pass\":false") != std::string::npos) failed = true;
                }
            }
            return S_OK;
            });
        });
        if (!check("message_handler", view->add_WebMessageReceived(handler.Get(), &registration))) return S_OK;
        const auto url = std::wstring(homeURI) + L"page.html?mode=" + mode + L"&token=" + token;
        check("navigate", view->Navigate(url.c_str()));
        return S_OK;
    }
    int start(const Args& args) {
        udf = args.path(L"--udf"); log.path = args.path(L"--log"); stop = args.path(L"--stop");
        shutdownTimeout = static_cast<DWORD>(std::stoul(args.get(L"--shutdown-timeout-ms", L"30000")));
        if (shutdownTimeout < 500 || shutdownTimeout > 120000) throw std::runtime_error("Invalid shutdown timeout");
        fs::create_directories(udf);
        mode = args.get(L"--mode", L"read"); token = args.get(L"--token");
        if ((mode != L"read" && mode != L"write" && mode != L"churn") || token.empty() || token.find_first_not_of(L"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-") != std::wstring::npos)
            throw std::runtime_error("Invalid page mode/token");
        page = loadPage();
        // Environment overrides precede registry/API UDF settings. Pin this child only,
        // so inherited machine/user WebView settings cannot redirect to a real profile.
        SetEnvironmentVariableW(L"WEBVIEW2_USER_DATA_FOLDER", udf.c_str());
        SetEnvironmentVariableW(L"WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", L"");
        SetEnvironmentVariableW(L"WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", L"--no-first-run");
        log.emit("host_start", "\"udf\":" + quote(utf8(udf.wstring())));
        log.standardHandles("before_webview2");
        if (!check("com_init", CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED))) return 2;
        prepareInventory(); // fail before creating WebView2 if inventory is unavailable
        WNDCLASSW wc{}; wc.lpfnWndProc = wndProc; wc.hInstance = GetModuleHandleW(nullptr); wc.lpszClassName = L"WebView2LockfileProbe";
        if (!RegisterClassW(&wc)) throw std::runtime_error("RegisterClassW failed");
        window = CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, wc.lpszClassName, L"Disposable WebView2 lockfile probe", WS_OVERLAPPEDWINDOW,
            0, 0, 660, 400, nullptr, nullptr, wc.hInstance, this);
        if (!window) throw std::runtime_error("CreateWindowExW failed");
        // Display without activating or sending input; double-rAF page ack proves rendering advanced.
        ShowWindow(window, SW_SHOWNOACTIVATE);
        deadline = GetTickCount64() + args.timeout(20000);
        SetTimer(window, 1, 50, nullptr);
        HMODULE loader = LoadLibraryExW((exePath().parent_path() / L"WebView2Loader.dll").c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!loader) { log.hr("load_loader", HRESULT_FROM_WIN32(GetLastError())); return 2; }
        const auto create = reinterpret_cast<decltype(&CreateCoreWebView2EnvironmentWithOptions)>(GetProcAddress(loader, "CreateCoreWebView2EnvironmentWithOptions"));
        if (!create) throw std::runtime_error("CreateCoreWebView2EnvironmentWithOptions missing");
        const auto handler = Callback<ICoreWebView2CreateCoreWebView2EnvironmentCompletedHandler>([this](HRESULT result, ICoreWebView2Environment* created) -> HRESULT {
            environmentPending = false;
            return callback([&]() -> HRESULT {
            if (closing) { log.hr("late_environment_callback", result); return S_OK; }
            if (!check("environment_callback", result) || !created) { failed = true; return S_OK; }
            env = created;
            LPWSTR version = nullptr;
            if (check("runtime_version", env->get_BrowserVersionString(&version)) && version) {
                log.emit("runtime_version", "\"version\":" + quote(utf8(version))); CoTaskMemFree(version);
            }
            const auto completion = Callback<ICoreWebView2CreateCoreWebView2ControllerCompletedHandler>([this](HRESULT hr, ICoreWebView2Controller* c) -> HRESULT {
                return callback([&]() { return onController(hr, c); });
            });
            log.emit("stage_begin", "\"stage\":\"create_controller\"");
            controllerPending = true;
            if (!check("create_controller", env->CreateCoreWebView2Controller(window, completion.Get()))) controllerPending = false;
            return S_OK;
            });
        });
        log.emit("stage_begin", "\"stage\":\"create_environment\"");
        environmentPending = true; webviewAttempted = true;
        if (!check("create_environment", create(nullptr, udf.c_str(), nullptr, handler.Get()))) environmentPending = false;
        MSG msg{}; BOOL got;
        while ((got = GetMessageW(&msg, nullptr, 0, 0)) > 0) { TranslateMessage(&msg); DispatchMessageW(&msg); }
        return got == -1 ? 5 : static_cast<int>(msg.wParam);
    }
    int run(const Args& args) {
        log.preserveShutdown = true;
        int code = 10;
        try { code = start(args); }
        catch (const std::exception& error) { callbackError(error); }
        // One exit owner for normal close, failed startup, failed page, timeout,
        // and C++ exceptions. The Host remains alive through process teardown.
        return shutdown(code);
    }
};
// Read-only Restart Manager inventory. Never call RmShutdown/RmRestart.
static std::string fileUsers(const fs::path& path) {
    DWORD session = 0; WCHAR key[CCH_RM_SESSION_KEY + 1]{};
    DWORD error = RmStartSession(&session, 0, key);
    if (error) return "\"rm_error\":" + std::to_string(error) + ",\"rm_users\":[]";
    const auto filename = path.wstring(); LPCWSTR names[] = {filename.c_str()};
    error = RmRegisterResources(session, 1, names, 0, nullptr, 0, nullptr);
    std::vector<RM_PROCESS_INFO> rows;
    UINT needed = 0, count = 0; DWORD reasons = 0;
    if (!error) {
        error = RmGetList(session, &needed, &count, nullptr, &reasons);
        if (error == ERROR_MORE_DATA && needed <= 4096) {
            rows.resize(needed); count = needed;
            error = RmGetList(session, &needed, &count, rows.data(), &reasons);
        }
    }
    RmEndSession(session);
    std::string users = "[";
    if (!error) for (UINT i = 0; i < count; ++i) {
        const auto& p = rows[i];
        ULARGE_INTEGER created{}; created.LowPart = p.Process.ProcessStartTime.dwLowDateTime; created.HighPart = p.Process.ProcessStartTime.dwHighDateTime;
        if (i) users += ",";
        users += "{\"pid\":" + std::to_string(p.Process.dwProcessId) + ",\"creation_filetime\":" + quote(std::to_string(created.QuadPart))
            + ",\"app_name\":" + quote(utf8(p.strAppName)) + ",\"application_type\":" + std::to_string(p.ApplicationType) + "}";
    }
    users += "]";
    return "\"rm_error\":" + std::to_string(error) + ",\"rm_users\":" + users;
}
static int fileProbeMain(const Args& args) {
    Log log;
    const auto path = args.path(L"--path");
    // Register before our own read/copy so this probe cannot report its own reader.
    const auto users = fileUsers(path);
    HANDLE file = CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    DWORD readError = file == INVALID_HANDLE_VALUE ? GetLastError() : 0;
    ULONGLONG bytes = 0;
    if (file != INVALID_HANDLE_VALUE) {
        BYTE buffer[65536]; DWORD read = 0;
        for (;;) {
            if (!ReadFile(file, buffer, sizeof(buffer), &read, nullptr)) { readError = GetLastError(); break; }
            bytes += read; if (!read) break;
        }
        CloseHandle(file);
    }
    bool copied = false; DWORD copyError = 0;
    if (!args.get(L"--copy-to").empty()) {
        const auto destination = args.path(L"--copy-to");
        copied = CopyFileW(path.c_str(), destination.c_str(), TRUE) != FALSE;
        if (!copied) copyError = GetLastError();
    }
    log.emit("file_probe", "\"path\":" + quote(utf8(path.wstring())) + ",\"read_error\":" + std::to_string(readError)
        + ",\"bytes_read\":" + std::to_string(bytes) + ",\"copy_attempted\":" + (args.get(L"--copy-to").empty() ? "false" : "true")
        + ",\"copied\":" + (copied ? "true" : "false") + ",\"copy_error\":" + std::to_string(copyError) + "," + users);
    return 0; // Observation errors are data, not a silent success verdict.
}
static int lockWatchMain(const Args& args) {
    const auto udf = args.path(L"--udf"); Log log; log.path = args.path(L"--log");
    HANDLE parent = OpenProcess(SYNCHRONIZE, FALSE, std::stoul(args.get(L"--parent-pid")));
    if (!parent) throw std::runtime_error("Cannot retain watcher parent handle");
    std::vector<HANDLE> browsers;
    std::wstringstream ids(args.get(L"--pids")); std::wstring id;
    while (std::getline(ids, id, L',')) if (!id.empty()) {
        HANDLE p = OpenProcess(SYNCHRONIZE, FALSE, std::stoul(id));
        if (!p) { for (auto h : browsers) CloseHandle(h); CloseHandle(parent); throw std::runtime_error("Cannot retain watcher browser handle"); }
        browsers.push_back(p);
    }
    if (browsers.empty()) { CloseHandle(parent); throw std::runtime_error("No browsers for watcher"); }
    const auto alive = [&]() { unsigned count = 0; for (auto p : browsers) if (WaitForSingleObject(p, 0) == WAIT_TIMEOUT) ++count; return count; };
    log.emit("watch_ready", "\"browser_count\":" + std::to_string(browsers.size()));
    const auto deadline = GetTickCount64() + args.timeout(32000); bool complete = false;
    const bool capture = !args.get(L"--capture-prefix").empty();
    const auto prefix = capture ? args.path(L"--capture-prefix") : fs::path{};
    const auto quiet = capture ? args.path(L"--quiet") : fs::path{};
    const auto release = capture ? args.path(L"--release") : fs::path{};
    const auto abort = capture ? args.path(L"--abort") : fs::path{};
    const auto copy = capture ? args.path(L"--copy-to-udf") : fs::path{};
    std::unique_ptr<FileHandle> held;
    bool copied = false; bool aborted = false; bool acquisitionComplete = true; bool referenceComplete = true;
    LARGE_INTEGER frequency{}; QueryPerformanceFrequency(&frequency);
    do {
        if (capture && fs::exists(abort)) { aborted = true; complete = false; log.emit("watch_abort_requested"); break; }
        const bool parentBefore = WaitForSingleObject(parent, 0) == WAIT_TIMEOUT;
        const auto before = alive();
        HANDLE file = INVALID_HANDLE_VALUE; DWORD error = 0;
        bool created = false;
        const bool attempting = !held;
        if (attempting) {
            file = openLock(udf, false); error = file == INVALID_HANDLE_VALUE ? GetLastError() : 0;
            if (error == ERROR_FILE_NOT_FOUND) { file = openLock(udf, true); error = file == INVALID_HANDLE_VALUE ? GetLastError() : 0; created = true; }
        }
        const auto acquiredAt = stampNow();
        const auto after = alive();
        const bool parentAfter = WaitForSingleObject(parent, 0) == WAIT_TIMEOUT;
        const bool opened = attempting && error == 0;
        if (file != INVALID_HANDLE_VALUE) {
            if (capture) held = std::make_unique<FileHandle>(file);
            else CloseHandle(file);
        }
        std::unique_ptr<Snapshot> acquisitionSnapshot;
        if (capture && opened) acquisitionSnapshot = std::make_unique<Snapshot>(captureSnapshot(udf, deadline));
        log.emit("watch_sample", "\"opened\":" + std::string(opened ? "true" : "false") + ",\"win32_error\":" + std::to_string(error)
            + ",\"create_new\":" + (created ? "true" : "false") + ",\"alive_before\":" + std::to_string(before)
            + ",\"alive_after\":" + std::to_string(after) + ",\"parent_alive_before\":" + (parentBefore ? "true" : "false")
            + ",\"parent_alive_after\":" + (parentAfter ? "true" : "false") + ",\"operation\":" + quote(attempting ? "acquire" : "retained")
            + ",\"lock_held\":" + (held ? "true" : "false") + ",\"sampled_at\":{" + stampFields(acquiredAt) + "}");
        if (capture && opened) {
            log.emit("lock_acquired", stampFields(acquiredAt) + ",\"qpc_frequency\":" + quote(std::to_string(frequency.QuadPart))
                + ",\"create_new\":" + (created ? "true" : "false")
                + ",\"browser_alive_before\":" + std::to_string(before) + ",\"browser_alive_after\":" + std::to_string(after)
                + ",\"parent_alive_before\":" + (parentBefore ? "true" : "false") + ",\"parent_alive_after\":" + (parentAfter ? "true" : "false")
                + ",\"snapshot_started\":{" + stampFields(acquisitionSnapshot->started) + "}"
                + ",\"publication_note\":\"snapshot read completed before this log record was published\"");
            // First capture is made by the acquiring worker before waiting on
            // any script/CIM handshake. It is an interval, never an atomic image.
            acquisitionSnapshot->save(prefix.wstring() + L"-acquired.json");
            acquisitionComplete = acquisitionSnapshot->complete;
            log.emit("acquisition_snapshot_done", "\"complete\":" + std::string(acquisitionSnapshot->complete ? "true" : "false"));
        }
        if (capture && held && !copied && fs::exists(quiet)) {
            log.emit("quiet_signal_received", stampFields(stampNow()));
            const auto snapshot = captureSnapshot(udf, deadline); snapshot.save(prefix.wstring() + L"-quiet.json");
            referenceComplete = snapshot.complete;
            try {
                copySnapshotTree(udf, copy, deadline);
                const auto sourceAfter = captureSnapshot(udf, deadline); sourceAfter.save(prefix.wstring() + L"-after-copy.json");
                const auto destination = captureSnapshot(copy, deadline); destination.save(prefix.wstring() + L"-copy.json");
                referenceComplete = referenceComplete && sourceAfter.complete && destination.complete;
                log.emit("reference_copy_done", stampFields(stampNow()) + ",\"captures_complete\":" + (referenceComplete ? "true" : "false"));
            } catch (const std::exception& problem) {
                referenceComplete = false;
                log.emit("reference_copy_error", "\"error\":" + quote(problem.what()));
            }
            copied = true;
        }
        // Capture mode measures browser lifetime while its host remains alive
        // waiting for shutdown. Parent exit is validated by the runner later.
        complete = (capture || !parentAfter) && after == 0 && (held || opened);
        if (!capture && complete) break;
        if (capture && copied && fs::exists(release)) break;
        Sleep(5); // Measurement cadence in this diagnostic worker, not a UI wait.
    } while (GetTickCount64() < deadline);
    const bool protocolComplete = !aborted && (!capture || (copied && fs::exists(release)));
    const bool referenceObserved = protocolComplete && referenceComplete;
    const bool observationComplete = protocolComplete && acquisitionComplete && referenceComplete;
    held.reset();
    if (capture) log.emit("held_lock_released", stampFields(stampNow()) + ",\"observation_complete\":" + (observationComplete ? "true" : "false")
        + ",\"lockfile_exists_after_watcher_release\":" + (fs::exists(udf / L"EBWebView" / L"lockfile") ? "true" : "false"));
    for (auto p : browsers) CloseHandle(p); CloseHandle(parent);
    log.emit("watch_end", "\"complete\":" + std::string(complete ? "true" : "false") + ",\"observation_complete\":" + (observationComplete ? "true" : "false")
        + ",\"aborted\":" + (aborted ? "true" : "false")
        + ",\"protocol_complete\":" + (protocolComplete ? "true" : "false") + ",\"acquisition_capture_complete\":" + (acquisitionComplete ? "true" : "false")
        + ",\"reference_captures_complete\":" + (referenceComplete ? "true" : "false")
        + ",\"reference_observation_complete\":" + (referenceObserved ? "true" : "false"));
    return complete && observationComplete ? 0 : 3;
}
static int snapshotMain(const Args& args) {
    const auto snapshot = captureSnapshot(args.path(L"--udf"), GetTickCount64() + args.timeout(30000));
    snapshot.save(args.path(L"--manifest"));
    Log log; log.emit("snapshot_done", "\"complete\":" + std::string(snapshot.complete ? "true" : "false"));
    return snapshot.complete ? 0 : 2;
}
static int resourceCheckMain() {
    const auto page = loadPage();
    const std::pair<const wchar_t*, int> cases[] = {
        {L"https://effetune-mixwright.localhost/page.html", 200},
        {L"https://effetune-mixwright.localhost/page.html?mode=write&token=fixture", 200},
        {L"https://effetune-mixwright.localhost/page.html#fixture", 200},
        {L"https://effetune-mixwright.localhost/", 404},
        {L"https://effetune-mixwright.localhost/missing.html", 404},
        {L"https://effetune-mixwright.localhost/../page.html", 404},
        {L"https://effetune-mixwright.localhost/%2e%2e/page.html", 404},
        {L"https://effetune-mixwright.localhost.evil/page.html", 404},
        {L"file:///page.html", 404}
    };
    Log log; bool passed = true;
    for (const auto& test : cases) {
        auto resource = resourceForURI(test.first, page);
        bool matches = resource.status == test.second;
        if (test.second == 200) {
            std::vector<BYTE> actual(page.size()); ULONG read = 0;
            const auto hr = resource.stream->Read(actual.data(), static_cast<ULONG>(actual.size()), &read);
            BYTE extra = 0; ULONG extraRead = 1;
            const auto eof = resource.stream->Read(&extra, 1, &extraRead);
            matches = matches && hr == S_OK && read == page.size() && actual == page && eof == S_FALSE && extraRead == 0
                && std::wstring(resource.headers).find(L"Content-Type: text/html; charset=utf-8") != std::wstring::npos
                && std::wstring(resource.headers).find(L"Cache-Control: no-store") != std::wstring::npos;
        } else matches = matches && !resource.stream && resource.headers == nullptr;
        log.emit("resource_check", "\"uri\":" + quote(utf8(test.first)) + ",\"status\":" + std::to_string(resource.status)
            + ",\"pass\":" + (matches ? "true" : "false"));
        passed = passed && matches;
    }
    return passed ? 0 : 2;
}
int wmain(int argc, wchar_t** argv) {
    try {
        if (argc < 2) throw std::runtime_error("Usage: probe.exe host|lock --udf <target/.../run-*/udf> ... | resource-check (no default profile)");
        const Args args(argc, argv);
        if (std::wstring(argv[1]) == L"lock") return lockMain(args);
        if (std::wstring(argv[1]) == L"resource-check") return resourceCheckMain();
        if (std::wstring(argv[1]) == L"file-probe") return fileProbeMain(args);
        if (std::wstring(argv[1]) == L"lock-watch") return lockWatchMain(args);
        if (std::wstring(argv[1]) == L"snapshot") return snapshotMain(args);
        if (std::wstring(argv[1]) == L"process-match-check") {
            Host host; host.udf = args.path(L"--udf");
            const auto path = (host.udf / L"EBWebView").wstring();
            const std::vector<std::pair<std::wstring, bool>> cases{
                {L"msedgewebview2.exe --user-data-dir=\"" + path + L"\"", true},
                {L"msedgewebview2.exe --type=renderer --user-data-dir \"" + path + L"\"", true},
                {L"msedgewebview2.exe --USER-DATA-DIR=\"" + path + L"/\"", true},
                {L"msedgewebview2.exe --user-data-dir=\"" + path + L"-other\"", false},
                {L"msedgewebview2.exe --user-data-dir=\"" + path + L"\\Default\"", false},
                {L"msedgewebview2.exe --database=\"" + path + L"\\Crashpad\"", false},
                {L"msedgewebview2.exe --user-data-dir=\"" + path + L"\" --user-data-dir=\"" + path + L"\"", false}
            };
            bool passed = true;
            for (const auto& test : cases) {
                const bool pass = host.matchesUdf(test.first) == test.second;
                passed = passed && pass;
                host.log.emit("process_match_check", "\"command_line\":" + quote(utf8(test.first)) + ",\"pass\":" + (pass ? "true" : "false"));
            }
            return passed ? 0 : 2;
        }
        if (std::wstring(argv[1]) == L"callback-check") {
            // Fault injection into the same C++ callback/teardown boundary;
            // no window, COM environment, controller or Runtime is created.
            Host host; host.log.preserveShutdown = true; host.log.path = args.path(L"--log");
            const auto hr = host.callback([]() -> HRESULT { throw std::runtime_error("SYNTHETIC callback exception"); });
            const auto code = host.shutdown(10);
            Log result;
            result.emit("callback_check", "\"hresult\":" + quote(hrhex(hr)) + ",\"failed\":" + (host.failed ? "true" : "false")
                + ",\"log_io_failed\":" + (host.log.ioFailed ? "true" : "false") + ",\"shutdown_returned\":true,\"exit_code\":" + std::to_string(code));
            return code;
        }
        if (std::wstring(argv[1]) == L"host") {
            // Keep callback target alive until the OS tears down this host process.
            auto* host = new Host;
            return host->run(args);
        }
        throw std::runtime_error("Unknown mode");
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 10; }
}
