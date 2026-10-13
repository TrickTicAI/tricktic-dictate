#include "writer-scheduler.h"
#include <cassert>
#include <chrono>
#include <future>
#include <thread>
int main() {
    std::mutex mutex;
    writer::Scheduler scheduler(mutex);
    auto preview=scheduler.acquire(true);
    assert(preview && !preview->canceled());
    auto final=std::async(std::launch::async,[&]{auto lease=scheduler.acquire(false);return lease && !lease->canceled();});
    auto deadline=std::chrono::steady_clock::now()+std::chrono::seconds(2);
    while (!preview->canceled() && std::chrono::steady_clock::now()<deadline) std::this_thread::yield();
    assert(preview->canceled());
    assert(!scheduler.acquire(true));
    preview.reset();
    assert(final.wait_for(std::chrono::seconds(2))==std::future_status::ready && final.get());
    auto active_final=scheduler.acquire(false);
    scheduler.cancel_preview();
    assert(!active_final->canceled()); // An old cancel cannot abort durable inference.
    active_final.reset();
    auto next=scheduler.acquire(true);
    assert(next && !next->canceled()); // A canceled generation cannot poison a new request.
    scheduler.cancel_preview();assert(next->canceled());
}
