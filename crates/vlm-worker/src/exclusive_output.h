#pragma once
#include <windows.h>
#include <algorithm>
#include <filesystem>
#include <limits>
#include <stdexcept>
#include <string>

class ExclusiveOutput {
    HANDLE handle_;
public:
    explicit ExclusiveOutput(const std::filesystem::path & path) : handle_(CreateFileW(path.c_str(), GENERIC_WRITE,
            FILE_SHARE_READ, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr)) {
        if (handle_ == INVALID_HANDLE_VALUE)
            throw std::runtime_error("Cannot exclusively create output (Win32 error " + std::to_string(GetLastError()) + ")");
    }
    ExclusiveOutput(const ExclusiveOutput &) = delete;
    ExclusiveOutput & operator=(const ExclusiveOutput &) = delete;
    ~ExclusiveOutput() { CloseHandle(handle_); }
    void write_line(const std::string & line) {
        const std::string bytes = line + '\n';
        size_t offset = 0;
        while (offset < bytes.size()) {
            auto size = static_cast<DWORD>((std::min)(bytes.size() - offset, size_t((std::numeric_limits<DWORD>::max)())));
            DWORD written = 0;
            if (!WriteFile(handle_, bytes.data() + offset, size, &written, nullptr) || !written)
                throw std::runtime_error("Cannot write output");
            offset += written;
        }
    }
};
