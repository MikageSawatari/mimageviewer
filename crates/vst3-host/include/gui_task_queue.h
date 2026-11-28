#pragma once
#include <deque>
#include <functional>
#include <mutex>
#include <utility>
namespace miv {
// Notifications only wake the outer message loop; the queue owns the work.
// A nested modal loop may consume every notification, so every outer dispatch
// return drains the queue even when no further notification arrives.
class GuiTaskQueue {
public:
    void push(std::function<void()> task) {
        std::lock_guard<std::mutex> lock(mutex_);
        tasks_.push_back(std::move(task));
    }
    void drain() {
        for (;;) {
            std::function<void()> task;
            {
                std::lock_guard<std::mutex> lock(mutex_);
                if (tasks_.empty()) return;
                task = std::move(tasks_.front()); tasks_.pop_front();
            }
            task();
        }
    }
    template<class Dispatch> void dispatch_outer(Dispatch&& dispatch) {
        dispatch();
        drain();
    }
private:
    std::mutex mutex_;
    std::deque<std::function<void()>> tasks_;
};
}
