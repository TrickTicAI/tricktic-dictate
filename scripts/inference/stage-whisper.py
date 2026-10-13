#!/usr/bin/env python3
"""Stage a pinned upstream source plus Writer scheduling/warmup extensions.
Never modifies the supplied upstream checkout or installed runtime binaries.
"""
import argparse
import importlib.util
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import re
import tarfile

REVISION = '371b5a7561823ab2bb32142d2751e35e7534727b'
ROOT = Path(__file__).resolve().parents[2]


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--httplib',type=Path,required=True,help='httplib.h pinned by runtime/whisper/httplib-source.json')
    args=p.parse_args()
    pin=json.loads((ROOT/'runtime/whisper/httplib-source.json').read_text())
    httplib=args.httplib.read_bytes()
    if hashlib.sha256(httplib).hexdigest()!=pin['sha256']: p.error('cpp-httplib header differs from its pin')
    args.output.mkdir(parents=True,exist_ok=False)
    archive=subprocess.check_output(['git','-C',str(args.source),'archive',REVISION])
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar: tar.extractall(args.output,filter='data')
    source=args.output/'examples/server/server.cpp'
    text=source.read_text()
    original=hashlib.sha256(source.read_bytes()).hexdigest()
    def replace(old,new,count=1):
        nonlocal text
        if text.count(old)!=count: raise ValueError('Pinned upstream patch context changed: '+old[:60])
        text=text.replace(old,new)
    replace('#include "whisper.h"','#include "whisper.h"\n#include "writer-scheduler.h"')
    replace('    std::mutex whisper_mutex;','    std::mutex whisper_mutex;\n    writer::Scheduler writer_scheduler(whisper_mutex);')
    replace('''        // acquire whisper model mutex lock
        std::lock_guard<std::mutex> lock(whisper_mutex);''','''        const bool writer_preview = req.has_file("writer_preview") && req.get_file_value("writer_preview").content == "true";
        auto writer_lease = writer_scheduler.acquire(writer_preview);
        if (!writer_lease) {
            res.status = 409;
            res.set_content("{\\"error\\":\\"preview superseded by final inference\\"}", "application/json");
            return;
        }''')
    replace('''            // tell whisper to abort if the HTTP connection closed
            wparams.abort_callback = [](void *user_data) {
                // user_data is a pointer to our Request
                auto req_ptr = static_cast<const httplib::Request*>(user_data);
                return req_ptr->is_connection_closed();
            };
            wparams.abort_callback_user_data = (void*)&req;''','''            struct writer_abort_state { const Request * request; const writer::Scheduler::Lease * lease; };
            writer_abort_state writer_abort{&req, &*writer_lease};
            wparams.abort_callback = [](void * data) {
                const auto * state = static_cast<writer_abort_state *>(data);
                return state->lease->canceled() || state->request->is_connection_closed();
            };
            wparams.abort_callback_user_data = &writer_abort;''')
    replace('''                // handle failure or early abort
                if (req.is_connection_closed())''','''                // A canceled preview never publishes a partial successful transcript.
                if (writer_lease->canceled()) {
                    res.status = 409;
                    res.set_content("{\\"error\\":\\"preview canceled for final inference\\"}", "application/json");
                    return;
                }
                if (req.is_connection_closed())''')
    replace('''        // return results to user
        if (params.response_format''','''        if (writer_lease->canceled()) {
            res.status = 409;
            res.set_content("{\\"error\\":\\"preview canceled for final inference\\"}", "application/json");
            return;
        }
        // return results to user
        if (params.response_format''')
    # Health is exposed only after an actual same-model synthetic warmup completes.
    replace('''    state.store(SERVER_STATE_READY);


    svr->set_default_headers''','''    {
        std::vector<float> silence(WHISPER_SAMPLE_RATE, 0.0f);
        auto warm = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
        warm.n_threads = params.n_threads;
        warm.audio_ctx = std::min(768, whisper_n_audio_ctx(ctx));
        warm.language = params.language.c_str();
        warm.no_context = true;
        warm.no_timestamps = true;
        warm.print_realtime = warm.print_progress = warm.print_timestamps = warm.print_special = false;
        warm.temperature_inc = 0.0f;
        if (whisper_full(ctx, warm, silence.data(), silence.size()) != 0) {
            fprintf(stderr, "writer: model warmup failed\\n");
            whisper_free(ctx);
            return 4;
        }
        whisper_reset_timings(ctx);
        fprintf(stderr, "writer: inference warmup complete; priority protocol 1\\n");
    }
    state.store(SERVER_STATE_READY);

    svr->Post(sparams.request_path + "/writer/finalize", [&](const Request & req, Response & res) {
        if (!req.body.empty()) { res.status = 400; return; }
        writer_scheduler.cancel_preview();
        res.set_content("{\\"acknowledged\\":true}", "application/json");
    });

    svr->set_default_headers''')
    replace('const std::string health_response = "{\\"status\\":\\"ok\\"}";', 'const std::string health_response = "{\\"status\\":\\"ok\\",\\"writer_protocol\\":1,\\"warm\\":true}";')
    # Preserve parser rejection status (e.g. 414) and never reflect request
    # targets, which may contain accidentally supplied credentials.
    begin = text.index('    svr->set_error_handler(')
    end = text.index('    });', begin) + len('    });')
    text = text[:begin] + '''    svr->set_error_handler([](const Request &, Response &res) {
        if (res.body.empty()) res.set_content("Worker request failed", "text/plain");
    });''' + text[end:]
    # Models are owned by Writer's supervisor. No secondary HTTP path can change
    # weights behind its identity/capability checks or bypass warmup.
    begin=text.index('    svr->Post(sparams.request_path + "/load",')
    end=text.index('    svr->Get(sparams.request_path + "/health",',begin)
    text=text[:begin]+text[end:]
    replace('#include "writer-scheduler.h"', '#include "writer-scheduler.h"\n#include "writer-auth.h"')
    replace('    std::mutex whisper_mutex;', '    writer::Auth writer_auth;\n    std::mutex whisper_mutex;')
    replace('    svr->set_default_headers', """    svr->set_pre_routing_handler([&](const Request &req, Response &res) {
        return writer_auth.reject(req, res, false) ? httplib::Server::HandlerResponse::Handled
                                                   : httplib::Server::HandlerResponse::Unhandled;
    });
    svr->set_default_headers""")
    replace('    if (!svr->bind_to_port(sparams.hostname, sparams.port))',
            '    sparams.port = writer_auth.bind(*svr);\n    if (sparams.port <= 0)')
    replace('''    svr->set_default_headers({{"Server", "whisper.cpp"},
                             {"Access-Control-Allow-Origin", "*"},
                             {"Access-Control-Allow-Headers", "content-type, authorization"}});''',
            '    svr->set_default_headers({{"Server", "whisper.cpp"}});')
    # No browser CORS or static asset service on a private worker.
    replace('    svr->set_base_dir(sparams.public_path);', '    // Writer workers do not serve static files.')
    # MW-SEC-26 W-01: a current cpp-httplib with explicit pre-authentication
    # bounds. Headers are parsed before Writer's auth check runs, so their size,
    # count, idle time, thread growth and queue are limited here. This is a
    # bounded burst/recovery policy, not immunity to continuous local flooding.
    replace('#include "httplib.h"', '''#include "writer-http-limits.h"
#include "httplib.h"''')
    replace('''    svr->set_read_timeout(sparams.read_timeout);
    svr->set_write_timeout(sparams.write_timeout);''', '''    // Writer uploads over loopback, so a 10 s stall is a stuck or hostile peer.
    svr->set_read_timeout(WRITER_HTTP_READ_TIMEOUT_SECONDS);
    svr->set_write_timeout(WRITER_HTTP_WRITE_TIMEOUT_SECONDS);
    svr->set_keep_alive_max_count(WRITER_HTTP_KEEP_ALIVE_MAX_COUNT);
    svr->set_keep_alive_timeout(WRITER_HTTP_KEEP_ALIVE_TIMEOUT_SECONDS);
    svr->set_payload_max_length(size_t(1) << 32); // the WAV format's own limit
    svr->new_task_queue = [] { return new httplib::ThreadPool(
        WRITER_HTTP_BASE_THREADS, WRITER_HTTP_MAX_THREADS, WRITER_HTTP_MAX_QUEUED_REQUESTS); };
    fprintf(stderr, "writer: pre-auth limits line=%d header=%d count=%d read=%d threads=%d queue=%d\\n",
        CPPHTTPLIB_MAX_LINE_LENGTH, CPPHTTPLIB_HEADER_MAX_LENGTH, CPPHTTPLIB_HEADER_MAX_COUNT,
        WRITER_HTTP_READ_TIMEOUT_SECONDS, WRITER_HTTP_MAX_THREADS, WRITER_HTTP_MAX_QUEUED_REQUESTS);''')
    # cpp-httplib 0.2x+ splits multipart text fields from files.
    replace('void get_req_parameters(const Request & req, whisper_params & params)', '''static bool writer_has_part(const Request & req, const std::string & key) {
    return req.form.has_field(key) || req.form.has_file(key);
}
static std::string writer_part(const Request & req, const std::string & key) {
    return req.form.has_field(key) ? req.form.get_field(key) : req.form.get_file(key).content;
}

void get_req_parameters(const Request & req, whisper_params & params)''')
    replace('req.has_file("file")', 'req.form.has_file("file")')
    replace('req.get_file_value("file")', 'req.form.get_file("file")')
    text = re.sub(r'req\.has_file\(("\w+")\)', r'writer_has_part(req, \1)', text)
    text = re.sub(r'req\.get_file_value\(("\w+")\)\.content', r'writer_part(req, \1)', text)
    if re.search(r'req\.(has_file|get_file_value)\(', text):
        raise ValueError('Unmigrated cpp-httplib multipart call')
    source.write_text(text)
    (source.parent/'httplib.h').write_bytes(httplib)
    spec=importlib.util.spec_from_file_location('writer_checkpoints',ROOT/'scripts/inference/encoder-checkpoints.py')
    checkpoints=importlib.util.module_from_spec(spec);spec.loader.exec_module(checkpoints)
    checkpoints.patch(args.output)
    spec=importlib.util.spec_from_file_location('writer_model_guards',ROOT/'scripts/inference/harden-whisper-model.py')
    guards=importlib.util.module_from_spec(spec);spec.loader.exec_module(guards)
    guards.patch(args.output)
    shutil.copyfile(ROOT/'runtime/whisper/writer-scheduler.h',source.parent/'writer-scheduler.h')
    shutil.copyfile(ROOT/'runtime/whisper/writer-auth.h',source.parent/'writer-auth.h')
    shutil.copyfile(ROOT/'runtime/whisper/writer-http-limits.h',source.parent/'writer-http-limits.h')
    shutil.copyfile(ROOT/'runtime/whisper/writer.manifest',source.parent/'writer.manifest')
    (args.output/'writer-source.txt').write_text(f'upstream={REVISION}\nhttplib={pin["revision"]}\noriginal_server_sha256={original}\npatched_server_sha256={hashlib.sha256(source.read_bytes()).hexdigest()}\n')
    with (args.output/'writer-source.txt').open('a') as receipt:
        for name in ['src/whisper.cpp','ggml/src/ggml-backend.cpp','examples/server/writer-scheduler.h','examples/server/writer-auth.h','examples/server/writer-http-limits.h','examples/server/writer.manifest','examples/server/httplib.h']:
            receipt.write(name+'='+hashlib.sha256((args.output/name).read_bytes()).hexdigest()+'\n')
    print(args.output)


if __name__=='__main__': main()
