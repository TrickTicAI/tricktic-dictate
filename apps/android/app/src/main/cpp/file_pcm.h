#pragma once
#include <array>
#include <algorithm>
#include <cerrno>
#include <cstdlib>
#include <climits>
#include <cstdint>
#include <fstream>
#include <functional>
#include <limits>
#include <stdexcept>
#include <string>
#include <vector>
#include <unistd.h>
#include <sys/mman.h>

// Whisper needs a contiguous float pointer, but preparation need not own a
// contiguous heap copy. The unlinked backing inode is reclaimed on process death.
class FilePcm {
    int fd_ = -1;
    void * mapping_ = MAP_FAILED;
    size_t bytes_ = 0;
    void release() noexcept {
        if (mapping_ != MAP_FAILED) munmap(mapping_, bytes_);
        if (fd_ >= 0) close(fd_);
        mapping_ = MAP_FAILED; fd_ = -1;
    }
    static uint32_t little(const unsigned char * p) {
        return p[0] | (uint32_t(p[1]) << 8) | (uint32_t(p[2]) << 16) | (uint32_t(p[3]) << 24);
    }
public:
    explicit FilePcm(const std::string & path, const std::function<bool()> & cancelled) {
        try {
            std::ifstream wav(path, std::ios::binary | std::ios::ate);
            const int64_t bytes = static_cast<int64_t>(wav.tellg());
            // Upstream accepts a signed int sample count. This is an API
            // limit, not a duration policy.
            if (!wav || bytes <= 44 || (bytes - 44) % 2 || (bytes - 44) / 2 > INT_MAX)
                throw std::runtime_error("Saved PCM exceeds the native sample-count limit; full audio retained");
            std::array<unsigned char, 44> header{};
            wav.seekg(0); wav.read(reinterpret_cast<char *>(header.data()), header.size());
            const auto * h = header.data();
            if (!wav || std::string(reinterpret_cast<const char *>(h), 4) != "RIFF" ||
                std::string(reinterpret_cast<const char *>(h + 8), 8) != "WAVEfmt " || little(h + 16) != 16 ||
                h[20] != 1 || h[21] != 0 || h[22] != 1 || h[23] != 0 || little(h + 24) != 16000 ||
                little(h + 28) != 32000 || h[32] != 2 || h[33] != 0 || h[34] != 16 || h[35] != 0 ||
                std::string(reinterpret_cast<const char *>(h + 36), 4) != "data" ||
                little(h + 40) != bytes - 44 || little(h + 4) != bytes - 8)
                throw std::runtime_error("Expected complete mono 16 kHz PCM16 saved WAV");
            const auto samples = static_cast<size_t>((bytes - 44) / 2);
            if (samples > std::numeric_limits<size_t>::max() / sizeof(float))
                throw std::runtime_error("Saved PCM exceeds native address space; full audio retained");
            bytes_ = samples * sizeof(float);
            if (cancelled()) throw std::runtime_error("Phone preparation cancelled; saved audio retained");
            // Same app-private filesystem as input; never replace or write input.
            std::string pattern = path + ".writer-pcm-XXXXXX";
            std::vector<char> name(pattern.begin(), pattern.end()); name.push_back(0);
            fd_ = mkstemp(name.data());
            if (fd_ < 0) throw std::runtime_error("Cannot create PCM backing file; free storage and retry saved audio");
            if (unlink(name.data()) != 0) throw std::runtime_error("Cannot unlink temporary PCM backing file");
            std::array<unsigned char, 8192> input{};
            std::array<float, 4096> output{};
            size_t offset = 0;
            while (offset < samples) {
                if (cancelled()) throw std::runtime_error("Phone preparation cancelled; saved audio retained");
                const auto count = std::min(output.size(), samples - offset);
                if (!wav.read(reinterpret_cast<char *>(input.data()), count * 2))
                    throw std::runtime_error("Saved audio read failed; full audio retained");
                for (size_t i = 0; i < count; ++i) {
                    int sample = input[i * 2] | (input[i * 2 + 1] << 8);
                    if (sample >= 32768) sample -= 65536;
                    output[i] = sample / 32768.0f;
                }
                const auto * data = reinterpret_cast<const char *>(output.data());
                size_t remaining = count * sizeof(float);
                while (remaining) {
                    const auto written = write(fd_, data, remaining);
                    if (written < 0 && errno == EINTR) continue;
                    if (written <= 0) throw std::runtime_error("PCM preparation ran out of storage; retry saved audio after freeing space");
                    remaining -= static_cast<size_t>(written); data += written;
                }
                offset += count;
            }
            if (cancelled()) throw std::runtime_error("Phone preparation cancelled; saved audio retained");
            mapping_ = mmap(nullptr, bytes_, PROT_READ, MAP_SHARED, fd_, 0);
            if (mapping_ == MAP_FAILED) throw std::runtime_error("PCM mapping exceeds available native address space; retry saved audio");
            // Let the kernel reclaim consumed backing pages under pressure.
            madvise(mapping_, bytes_, MADV_SEQUENTIAL);
        } catch (...) { release(); throw; }
    }
    ~FilePcm() { release(); }
    FilePcm(const FilePcm &) = delete;
    FilePcm & operator=(const FilePcm &) = delete;
    const float * data() const { return static_cast<const float *>(mapping_); }
    size_t size() const { return bytes_ / sizeof(float); }
};
