// Writer's priority/cancellation policy. Independent of Whisper and the OS.
#pragma once
#include <atomic>
#include <cstdint>
#include <mutex>
#include <optional>

namespace writer {
class Scheduler {
    std::mutex & mutex_;
    std::atomic<std::uint64_t> generation_{0};
    std::atomic<unsigned> finals_{0};
public:
    struct Lease {
        std::unique_lock<std::mutex> lock;
        Scheduler * owner;
        std::uint64_t generation;
        bool preview;
        bool canceled() const { return preview && owner->generation_.load() != generation; }
    };
    explicit Scheduler(std::mutex & mutex) : mutex_(mutex) {}
    std::optional<Lease> acquire(bool preview) {
        if (preview) {
            if (finals_.load() != 0) return std::nullopt;
            std::unique_lock<std::mutex> lock(mutex_, std::try_to_lock);
            if (!lock.owns_lock() || finals_.load() != 0) return std::nullopt;
            return Lease{std::move(lock), this, generation_.load(), true};
        }
        finals_.fetch_add(1);
        cancel_preview();
        std::unique_lock<std::mutex> lock(mutex_);
        finals_.fetch_sub(1);
        return Lease{std::move(lock), this, generation_.load(), false};
    }
    // Called before the client waits for its local provider lock, not afterwards.
    void cancel_preview() { generation_.fetch_add(1); }
};
} // namespace writer
