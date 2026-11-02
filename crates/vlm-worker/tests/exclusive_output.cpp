#include "exclusive_output.h"
#include "image_input.h"
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <fstream>
#include <iostream>
#include <mutex>
#include <thread>
#include <vector>

int wmain(int argc, wchar_t ** argv) {
    if (argc != 2) return 1;
    auto root = std::filesystem::path(argv[1]);
    auto suffix = std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(std::chrono::steady_clock::now().time_since_epoch().count());
    auto path = root / (L"出力-排他-" + suffix + L".jsonl");
    auto png = root / (L"極端な寸法-" + suffix + L".png");
    try {
        std::atomic<int> wins{0};
        std::mutex mutex;
        std::condition_variable start;
        int ready = 0;
        bool go = false;
        std::vector<std::thread> threads;
        for (int i = 0; i < 8; ++i) threads.emplace_back([&] {
            {
                std::unique_lock<std::mutex> lock(mutex);
                ++ready;
                start.notify_all();
                start.wait(lock, [&] { return go; });
            }
            try { ExclusiveOutput out(path); out.write_line("owner"); ++wins; } catch (const std::runtime_error &) {}
        });
        {
            std::unique_lock<std::mutex> lock(mutex);
            start.wait(lock, [&] { return ready == 8; });
            go = true;
        }
        start.notify_all();
        for (auto & thread : threads) thread.join();
        std::ifstream input(path, std::ios::binary);
        std::string content((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
        input.close();
        bool rejected = false;
        try { ExclusiveOutput again(path); } catch (const std::runtime_error &) { rejected = true; }
        // A tiny malformed PNG header must be refused for dimensions before decoding.
        const unsigned char header[] = {137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 'I', 'H', 'D', 'R', 0, 2, 0, 0, 0, 0, 0, 1};
        { std::ofstream fixture(png, std::ios::binary); fixture.write(reinterpret_cast<const char *>(header), sizeof(header)); }
        bool dimensions_rejected = false;
        try { read_selftest_png(png); } catch (const ImageFailure & e) {
            dimensions_rejected = std::string(e.what()).find("dimensions") != std::string::npos;
        }
        std::filesystem::remove(path);
        std::filesystem::remove(png);
        bool ok = wins == 1 && content == "owner\n" && rejected && dimensions_rejected;
        std::cout << "exclusive creation and PNG preflight=" << ok << '\n';
        return ok ? 0 : 1;
    } catch (const std::exception & e) { std::cerr << e.what() << '\n'; return 1; }
}
