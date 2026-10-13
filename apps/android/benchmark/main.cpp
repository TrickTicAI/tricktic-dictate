// Private JSONL on stdout; runtime diagnostics on stderr. No network adapter.
#include "whisper.h"
#include "json.hpp"
#include <chrono>
#include <climits>
#include <cmath>
#include <csignal>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <sys/resource.h>
#include <unistd.h>
#include <vector>

using json = nlohmann::ordered_json;
using Clock = std::chrono::steady_clock;
static volatile sig_atomic_t cancelled = 0;
static void cancel(int) { cancelled = 1; }
static bool abort_run(void *) { return cancelled != 0; }
static double ms(Clock::time_point start) {
    return std::chrono::duration<double, std::milli>(Clock::now() - start).count();
}
static long peak_rss() {
    rusage usage{};
    if (getrusage(RUSAGE_SELF, &usage) != 0) return -1;
    return usage.ru_maxrss; // Linux/Android KiB, process lifetime high-water mark.
}
static void emit(const json &record) {
    std::cout << record.dump() << std::endl;
    // Device stdout is a private file; retain each completed attempt before continuing.
    (void) fsync(STDOUT_FILENO);
}
static std::vector<float> read_pcm(const std::string &path) {
    std::ifstream in(path, std::ios::binary | std::ios::ate);
    const auto bytes = in.tellg();
    if (!in || bytes <= 0 || bytes % 2 || bytes / 2 > INT_MAX)
        throw std::runtime_error("PCM must be nonempty complete signed 16-bit frames within whisper's int sample count");
    in.seekg(0);
    std::vector<float> samples(static_cast<size_t>(bytes / 2));
    for (auto &sample : samples) {
        unsigned char b[2];
        if (!in.read(reinterpret_cast<char *>(b), 2)) throw std::runtime_error("Incomplete PCM read");
        int value = b[0] | (b[1] << 8);
        if (value >= 32768) value -= 65536;
        sample = static_cast<float>(value) / 32768.0f;
    }
    return samples;
}
int main(int argc, char **argv) {
    std::signal(SIGINT, cancel);
    std::signal(SIGTERM, cancel);
    try {
        if (argc == 3 && std::string(argv[1]) == "--check-pcm") {
            auto pcm = read_pcm(argv[2]);
            emit({{"event", "pcm_checked"}, {"samples", pcm.size()}, {"first", pcm.front()}, {"last", pcm.back()}});
            return 0;
        }
        if (argc != 3) throw std::runtime_error("Usage: writer-android-bench MODEL PLAN.json | --check-pcm PCM");
        json plan;
        std::ifstream input(argv[2]);
        input >> plan;
        int threads = plan.at("threads").get<int>();
        int repeats = plan.at("repeats").get<int>();
        double minimum = plan.at("minimum_seconds").get<double>();
        if (threads < 1 || repeats < 1 || minimum < 0 || !std::isfinite(minimum) || plan.at("clips").empty())
            throw std::runtime_error("Invalid benchmark plan");
        emit({{"event", "start"}, {"pid", getpid()}, {"backend", "cpu"}, {"system_info", whisper_print_system_info()},
              {"threads", threads}, {"translate", false}, {"decode", "greedy-best-of-1-no-temperature-fallback"},
              {"flash_attn", true}, {"no_context", true}});
        const auto load_start = Clock::now();
        auto context_params = whisper_context_default_params();
        context_params.use_gpu = false;
        context_params.flash_attn = true;
        std::unique_ptr<whisper_context, decltype(&whisper_free)> context(
            whisper_init_from_file_with_params(argv[1], context_params), whisper_free);
        if (!context) throw std::runtime_error("Model load failed");
        emit({{"event", "loaded"}, {"load_ms", ms(load_start)}, {"peak_rss_kib", peak_rss()}});
        const auto workload_start = Clock::now();
        int iteration = 0;
        do {
            for (const auto &clip : plan.at("clips")) {
                if (cancelled) break;
                const auto prepare_start = Clock::now();
                auto pcm = read_pcm(clip.at("pcm").get<std::string>());
                const double prepare_ms = ms(prepare_start);
                auto language = clip.at("language").get<std::string>();
                if (language != "he" && language != "en" && language != "auto")
                    throw std::runtime_error("Only he/en/auto are qualified input modes");
                auto params = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
                params.n_threads = threads;
                params.language = language.c_str();
                params.translate = false;
                params.no_context = true;
                params.no_timestamps = true;
                params.print_realtime = false;
                params.print_progress = false;
                params.print_timestamps = false;
                params.greedy.best_of = 1;
                params.temperature_inc = 0;
                params.abort_callback = abort_run;
                const auto start = Clock::now();
                int result = whisper_full(context.get(), params, pcm.data(), static_cast<int>(pcm.size()));
                const double elapsed = ms(start);
                // Never publish partial decoder state as a successful raw transcript.
                if (cancelled || result != 0) {
                    emit({{"event", cancelled ? "cancelled" : "failed"}, {"clip", clip.at("id")},
                          {"iteration", iteration}, {"inference_ms", elapsed}, {"code", result}});
                    return cancelled ? 130 : 1;
                }
                std::string raw;
                for (int i = 0; i < whisper_full_n_segments(context.get()); ++i)
                    raw += whisper_full_get_segment_text(context.get(), i);
                emit({{"event", "result"}, {"clip", clip.at("id")}, {"iteration", iteration},
                      {"audio_ms", pcm.size() / 16.0}, {"prepare_ms", prepare_ms}, {"inference_ms", elapsed},
                      {"peak_rss_kib", peak_rss()}, {"language", whisper_lang_str(whisper_full_lang_id(context.get()))},
                      {"raw_text", raw}});
            }
            ++iteration;
        } while (!cancelled && (iteration < repeats || ms(workload_start) < minimum * 1000));
        emit({{"event", cancelled ? "cancelled" : "complete"}, {"iterations", iteration},
              {"workload_ms", ms(workload_start)}, {"peak_rss_kib", peak_rss()}});
        return cancelled ? 130 : 0;
    } catch (const std::exception &error) {
        emit({{"event", "error"}, {"message", error.what()}});
        return 1;
    }
}
