// Socket-level contract fixture: compile against each pinned worker's httplib.
#include "httplib.h"
#include "writer-auth.h"
int main(int argc, char **argv) {
    const bool nemo = argc > 1 && std::string(argv[1]) == "nemo";
    writer::Auth auth;
    httplib::Server server;
    server.set_pre_routing_handler([&](const auto &req, auto &res) {
        return auth.reject(req, res, nemo) ? httplib::Server::HandlerResponse::Handled
                                          : httplib::Server::HandlerResponse::Unhandled;
    });
    server.Get("/health", [](const auto &, auto &res) {
        res.set_content("{\"status\":\"ok\",\"writer_protocol\":1,\"warm\":true}", "application/json");
    });
    server.Get("/ready", [](const auto &, auto &res) { res.set_content("{\"ready\":true}", "application/json"); });
    server.Post(nemo ? "/v1/audio/transcriptions" : "/inference", [](const auto &, auto &res) {
        res.set_content("{\"text\":\"\"}", "application/json");
    });
    server.Post("/writer/finalize", [](const auto &, auto &res) {
        res.set_content("{\"acknowledged\":true}", "application/json");
    });
    return auth.bind(server) > 0 && server.listen_after_bind() ? 0 : 1;
}
