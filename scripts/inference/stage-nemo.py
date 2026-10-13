#!/usr/bin/env python3
"""Stage pinned NeMo sources with Writer's private speech-worker protocol.
Initialize ggml and third_party/cpp-httplib in --source first. This stages their
exact gitlinks; builds and artifact publication belong to the runtime package gate.
"""
import argparse
import hashlib
import io
from pathlib import Path
import shutil
import subprocess
import tarfile

REVISION = "4f9676226f667d14608487df744f375db87127f8"
ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    archive = subprocess.check_output(["git", "-C", str(args.source), "archive", REVISION])
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(args.output, filter="data")
    # Archive the exact gitlinks, never an arbitrary checked-out submodule head.
    submodules = {}
    for name in ["ggml", "third_party/cpp-httplib"]:
        revision = subprocess.check_output(
            ["git", "-C", str(args.source), "rev-parse", f"{REVISION}:{name}"],
            text=True).strip()
        archive = subprocess.check_output(
            ["git", "-C", str(args.source / name), "archive", revision])
        destination = args.output / name
        destination.mkdir(parents=True, exist_ok=True)
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(destination, filter="data")
        submodules[name] = revision
    source = args.output / "server/http/http_server.cpp"
    text = source.read_text()
    original = hashlib.sha256(source.read_bytes()).hexdigest()

    def replace(old, new):
        nonlocal text
        if text.count(old) != 1:
            raise ValueError("Pinned NeMo patch context changed: " + old[:60])
        text = text.replace(old, new)

    replace('#include <httplib.h>', '#include <httplib.h>\n#include "writer-auth.h"')
    replace('    EngineRegistry& models;', '    writer::Auth writer_auth;\n    EngineRegistry& models;')
    begin = text.index('        if (this->config.access_log) {')
    end = text.index('        server->Options(', begin)
    # Replace the upstream credential/query exemptions and disable request logging.
    text = text[:begin] + '''        server->set_pre_routing_handler(
            [this](const httplib::Request& request, httplib::Response& response) {
                return writer_auth.reject(request, response, true)
                    ? httplib::Server::HandlerResponse::Handled
                    : httplib::Server::HandlerResponse::Unhandled;
            });
''' + text[end:]
    replace('    return impl_->server->listen(impl_->config.address, impl_->config.port);',
            '    return bind() && listen_after_bind();')
    replace('    return impl_->server->bind_to_port(impl_->config.address, impl_->config.port);',
            '    return impl_->writer_auth.bind(*impl_->server) > 0;')
    # Upstream realtime upgrades may precede pre-routing. Writer uses file ASR only.
    replace('        server->WebSocket("/v1/realtime", realtime);',
            '        // Realtime browser upgrades are not part of the Writer protocol.')
    replace('        server->WebSocket("/v1/audio/transcriptions/realtime", std::move(realtime));',
            '        (void)realtime;')
    source.write_text(text)
    # The CLI accepts ephemeral binding for the private packaged worker.
    cli = args.output / "app/serve.cpp"
    cli_text = cli.read_text()
    old = 'parse_int(value(i, arg), arg, 1, 65535)'
    if cli_text.count(old) != 1:
        raise ValueError("Pinned NeMo port parser context changed")
    cli_text = cli_text.replace(old, 'parse_int(value(i, arg), arg, 0, 65535)')
    cli.write_text(cli_text)
    header = source.parent / "writer-auth.h"
    shutil.copyfile(ROOT / "runtime/whisper/writer-auth.h", header)
    (args.output / "writer-source.txt").write_text(
        f"upstream={REVISION}\noriginal_server_sha256={original}\n"
        f"patched_server_sha256={hashlib.sha256(source.read_bytes()).hexdigest()}\n"
        f"writer_auth_sha256={hashlib.sha256(header.read_bytes()).hexdigest()}\n"
        f"patched_cli_sha256={hashlib.sha256(cli.read_bytes()).hexdigest()}\n"
        + "".join(f"{name}={revision}\n" for name, revision in submodules.items())
    )
    print(args.output)


if __name__ == "__main__":
    main()
