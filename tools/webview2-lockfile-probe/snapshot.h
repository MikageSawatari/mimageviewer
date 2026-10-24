#pragma once
#include <bcrypt.h>
#include <algorithm>

struct Stamp {
    ULONGLONG filetime;
    LONGLONG qpc;
};
static ULONGLONG filetimeTicks(const FILETIME& value) {
    ULARGE_INTEGER result{}; result.LowPart=value.dwLowDateTime; result.HighPart=value.dwHighDateTime; return result.QuadPart;
}
static Stamp stampNow() {
    FILETIME ft{}; LARGE_INTEGER qpc{}; ULARGE_INTEGER value{};
    GetSystemTimePreciseAsFileTime(&ft); QueryPerformanceCounter(&qpc);
    value.LowPart = ft.dwLowDateTime; value.HighPart = ft.dwHighDateTime;
    return {value.QuadPart, qpc.QuadPart};
}
static std::string stampFields(const Stamp& stamp) {
    return "\"filetime\":" + quote(std::to_string(stamp.filetime)) + ",\"qpc\":" + quote(std::to_string(stamp.qpc));
}
struct FileHandle {
    HANDLE value = INVALID_HANDLE_VALUE;
    explicit FileHandle(HANDLE handle) : value(handle) {}
    FileHandle(const FileHandle&) = delete;
    ~FileHandle() { if (value != INVALID_HANDLE_VALUE && value != nullptr) CloseHandle(value); }
};
static void writeEvidence(const fs::path& path, const std::string& bytes) {
    checkedPath(path);
    FileHandle file(CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
    DWORD written = 0;
    if (file.value == INVALID_HANDLE_VALUE || !WriteFile(file.value, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr) || written != bytes.size())
        throw std::runtime_error("Cannot write snapshot evidence");
}
struct Snapshot {
    struct Entry { std::string path, json; };
    std::vector<Entry> files;
    std::string enumerationError;
    Stamp started{}, finished{};
    bool complete = true;
    void save(const fs::path& path) const {
        std::string json = "{\"started\":{" + stampFields(started) + "},\"finished\":{" + stampFields(finished)
            + "},\"complete\":" + (complete ? "true" : "false") + ",\"enumeration_error\":"
            + (enumerationError.empty() ? "null" : quote(enumerationError)) + ",\"files\":[";
        bool first = true;
        for (const auto& file : files) { if (!first) json += ","; first = false; json += file.json; }
        writeEvidence(path, json + "]}\n");
    }
};
static bool excludedLock(const fs::path& relative) {
    const auto text = relative.generic_wstring();
    return CompareStringOrdinal(text.c_str(), -1, L"EBWebView/lockfile", -1, TRUE) == CSTR_EQUAL;
}
struct Sha256 {
    BCRYPT_ALG_HANDLE algorithm = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    std::vector<BYTE> object;
    Sha256() {
        if (BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) < 0) throw std::runtime_error("BCrypt SHA256 provider failed");
        DWORD length = 0, returned = 0;
        const auto status = BCryptGetProperty(algorithm, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&length), sizeof(length), &returned, 0);
        if (status >= 0) object.resize(length);
        if (status < 0 || BCryptCreateHash(algorithm, &hash, object.data(), length, nullptr, 0, 0) < 0) {
            BCryptCloseAlgorithmProvider(algorithm, 0); algorithm = nullptr;
            throw std::runtime_error("BCrypt SHA256 hash failed");
        }
    }
    ~Sha256() { if (hash) BCryptDestroyHash(hash); if (algorithm) BCryptCloseAlgorithmProvider(algorithm, 0); }
    void append(BYTE* data, DWORD length) { if (BCryptHashData(hash, data, length, 0) < 0) throw std::runtime_error("BCryptHashData failed"); }
    std::string finish() {
        BYTE digest[32]{};
        if (BCryptFinishHash(hash, digest, sizeof(digest), 0) < 0) throw std::runtime_error("BCryptFinishHash failed");
        std::ostringstream text; text << std::hex << std::uppercase << std::setfill('0');
        for (const BYTE value : digest) text << std::setw(2) << static_cast<unsigned>(value);
        return text.str();
    }
};
static Snapshot captureSnapshot(const fs::path& root, ULONGLONG deadline) {
    Snapshot result; result.started = stampNow();
    try {
        std::vector<fs::path> directories{checkedPath(root)};
        while (!directories.empty()) {
            const auto directory = directories.back(); directories.pop_back();
            for (const auto& item : fs::directory_iterator(directory)) {
                if (GetTickCount64() >= deadline) throw std::runtime_error("snapshot deadline exceeded");
                const auto path = checkedPath(item.path());
                const auto relative = path.lexically_relative(root);
                if (excludedLock(relative)) continue;
                if (item.is_directory()) { directories.push_back(path); continue; }
                const auto began = stampNow();
                std::string error, digest, writeBefore, writeAfter; ULONGLONG bytes = 0, sizeBefore = 0, sizeAfter = 0;
                bool unstable = false;
                try {
                    FileHandle file(CreateFileW(path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                        nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
                    if (file.value == INVALID_HANDLE_VALUE) throw std::runtime_error("CreateFile read error " + std::to_string(GetLastError()));
                    BY_HANDLE_FILE_INFORMATION before{}, after{};
                    if (!GetFileInformationByHandle(file.value, &before)) throw std::runtime_error("file information failed");
                    sizeBefore = (static_cast<ULONGLONG>(before.nFileSizeHigh) << 32) | before.nFileSizeLow;
                    writeBefore = std::to_string(filetimeTicks(before.ftLastWriteTime));
                    Sha256 sha; BYTE buffer[65536]; DWORD count = 0;
                    for (;;) {
                        if (GetTickCount64() >= deadline) throw std::runtime_error("file read deadline exceeded");
                        if (!ReadFile(file.value, buffer, sizeof(buffer), &count, nullptr)) throw std::runtime_error("ReadFile error " + std::to_string(GetLastError()));
                        if (!count) break;
                        bytes += count; sha.append(buffer, count);
                    }
                    digest = sha.finish();
                    if (!GetFileInformationByHandle(file.value, &after)) throw std::runtime_error("file information after read failed");
                    sizeAfter = (static_cast<ULONGLONG>(after.nFileSizeHigh) << 32) | after.nFileSizeLow;
                    writeAfter = std::to_string(filetimeTicks(after.ftLastWriteTime));
                    unstable = sizeBefore != sizeAfter || bytes != sizeBefore || CompareFileTime(&before.ftLastWriteTime, &after.ftLastWriteTime) != 0;
                    if (unstable) error = "file changed during read";
                } catch (const std::exception& problem) { error = problem.what(); }
                if (!error.empty()) result.complete = false;
                const auto ended = stampNow();
                const auto name = relative.generic_u8string();
                result.files.push_back({name, "{\"path\":" + quote(name) + ",\"size\":" + std::to_string(sizeBefore)
                    + ",\"size_after\":" + std::to_string(sizeAfter) + ",\"bytes_read\":" + std::to_string(bytes)
                    + ",\"write_time_before\":" + (writeBefore.empty() ? "null" : quote(writeBefore))
                    + ",\"write_time_after\":" + (writeAfter.empty() ? "null" : quote(writeAfter))
                    + ",\"sha256\":" + (digest.empty() ? "null" : quote(digest)) + ",\"read_error\":" + (error.empty() ? "null" : quote(error))
                    + ",\"metadata_change_observed\":" + (unstable ? "true" : "false") + ",\"read_started\":{" + stampFields(began)
                    + "},\"read_finished\":{" + stampFields(ended) + "}}"});
            }
        }
    } catch (const std::exception& error) { result.complete = false; result.enumerationError = error.what(); }
    result.finished = stampNow();
    std::sort(result.files.begin(), result.files.end(), [](const auto& a, const auto& b) { return a.path < b.path; });
    return result;
}
static void copySnapshotTree(const fs::path& source, const fs::path& destination, ULONGLONG deadline) {
    checkedPath(source); checkedPath(destination);
    if (fs::exists(destination)) throw std::runtime_error("reference destination already exists");
    fs::create_directory(destination);
    std::vector<fs::path> directories{source};
    while (!directories.empty()) {
        const auto directory = directories.back(); directories.pop_back();
        for (const auto& item : fs::directory_iterator(directory)) {
            if (GetTickCount64() >= deadline) throw std::runtime_error("reference copy deadline exceeded");
            const auto path = checkedPath(item.path()); const auto relative = path.lexically_relative(source);
            if (excludedLock(relative)) continue;
            const auto target = checkedPath(destination / relative);
            if (item.is_directory()) { fs::create_directory(target); directories.push_back(path); }
            else if (!CopyFileW(path.c_str(), target.c_str(), TRUE)) throw std::runtime_error("reference CopyFile error " + std::to_string(GetLastError()));
        }
    }
}
