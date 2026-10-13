// Writer's private worker protocol. Shared by the pinned Whisper and NeMo builds.
#pragma once
#include <cstdio>
#include <stdexcept>
#include <string>
#include <utility>

namespace writer {
class Auth {
    std::string bearer_;
    std::string instance_;

    static std::string read_secret() {
        // Fixed-size framing, never an argument, environment variable or file.
        char line[66] = {};
        if (!std::fgets(line, sizeof(line), stdin))
            throw std::runtime_error("writer: missing private launch handshake");
        std::string value(line);
        if (value.size() != 65 || value.back() != '\n' ||
            value.find_first_not_of("0123456789abcdef") != 64)
            throw std::runtime_error("writer: invalid private launch handshake");
        value.pop_back();
        return value;
    }

    static bool equal(const std::string &a, const std::string &b) {
        if (a.size() != b.size()) return false;
        unsigned char diff = 0;
        for (size_t i = 0; i < a.size(); ++i) diff |= a[i] ^ b[i];
        return diff == 0;
    }

public:
    Auth() : bearer_("Bearer " + read_secret()), instance_(read_secret()) {}

    template<class Request, class Response>
    bool reject(const Request &req, Response &res, bool nemo) const {
        res.set_header("Cache-Control", "no-store");
        // Reject even a valid capability if used from a browser context.
        if (req.has_header("Origin") || req.has_header("Sec-Fetch-Site")) {
            res.status = 403;
        } else if (req.get_header_value_count("Authorization") != 1 ||
                   !equal(req.get_header_value("Authorization"), bearer_)) {
            res.status = 401;
        } else if (!req.params.empty() ||
                   !((req.method == "GET" && (req.path == "/health" || (nemo && req.path == "/ready"))) ||
                     (req.method == "POST" && (nemo ? req.path == "/v1/audio/transcriptions" :
                         (req.path == "/inference" || req.path == "/writer/finalize"))))) {
            res.status = 404;
        } else {
            res.set_header("X-Writer-Instance", instance_);
            return false;
        }
        res.set_content("{\"error\":\"worker request denied\"}", "application/json");
        return true;
    }

    template<class Server>
    int bind(Server &server) const {
        // The child keeps the socket throughout startup. No reserve/drop race.
        const int port = server.bind_to_any_port("127.0.0.1");
        if (port > 0) {
            std::fprintf(stderr, "writer: private listener %d\n", port);
            std::fflush(stderr);
        }
        return port;
    }
};
} // namespace writer
