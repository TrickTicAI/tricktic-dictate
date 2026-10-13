#pragma once

#include <memory>
#include <stdexcept>
#include <string>
#include <sys/stat.h>
#include <tuple>

// Model weights only. Decoding state/audio/results belong to a single request.
// Identity changes (including replacement at the same path) invalidate residency.
template <typename T, void (*Free)(T *)> class ResidentModel {
    using Identity = std::tuple<std::string, dev_t, ino_t, off_t, time_t, long, time_t, long>;
    Identity identity_{};
    std::unique_ptr<T, decltype(Free)> model_{nullptr, Free};
public:
    void clear() { model_.reset(); identity_ = {}; }
    template <typename Load> T * get(const std::string & path, Load load) {
        struct stat info{};
        if (stat(path.c_str(), &info) != 0 || !S_ISREG(info.st_mode)) {
            clear();
            throw std::runtime_error("Phone model file is unavailable");
        }
        Identity identity{path, info.st_dev, info.st_ino, info.st_size,
            info.st_mtim.tv_sec, info.st_mtim.tv_nsec, info.st_ctim.tv_sec, info.st_ctim.tv_nsec};
        if (!model_ || identity != identity_) {
            clear(); // Release old weights before allocating another model.
            model_.reset(load(path.c_str()));
            if (!model_) throw std::runtime_error("Phone model could not load; check available memory and reinstall model");
            identity_ = std::move(identity);
        }
        return model_.get();
    }
};
