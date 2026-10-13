#!/usr/bin/env python3
"""Writer-only loopback HTTPS test proxy. Never logs headers, credentials or bodies."""
import argparse
import http.client
import http.server
from pathlib import Path
import ssl
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('command', choices=['certificate', 'serve'])
p.add_argument('--directory', type=Path, required=True)
p.add_argument('--port', type=int, default=18797)
p.add_argument('--upstream-port', type=int, default=18796)
p.add_argument('--mode', choices=['proxy', 'redirect', 'incompatible'], default='proxy')
a = p.parse_args()
a.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
cert, key = a.directory / 'cert.pem', a.directory / 'key.pem'
if a.command == 'certificate':
    if cert.exists() or key.exists():
        raise SystemExit('Refusing to replace existing certificate material')
    subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-sha256', '-days', '2', '-nodes',
                    '-keyout', str(key), '-out', str(cert), '-subj', '/CN=Writer emulator only',
                    '-addext', 'subjectAltName=IP:10.0.2.2', '-addext', 'basicConstraints=critical,CA:TRUE'],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    key.chmod(0o600)
    print('Created short-lived emulator-only certificate; private key remains in the chosen directory.')
else:
    class Proxy(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'
        def log_message(self, *_): pass
        def forward(self):
            if a.mode != 'proxy':
                self.send_response(302 if a.mode == 'redirect' else 200)
                if a.mode == 'redirect': self.send_header('Location', 'https://example.invalid/')
                body = b'{"api_version":"unsupported"}'
                self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body); return
            upstream = http.client.HTTPConnection('127.0.0.1', a.upstream_port, timeout=30)
            # Stream both fixed and chunked uploads without retaining audio in this proxy.
            def chunks():
                if self.headers.get('Transfer-Encoding', '').lower() == 'chunked':
                    while True:
                        size = int(self.rfile.readline().split(b';')[0], 16)
                        if size == 0:
                            while self.rfile.readline() != b'\r\n': pass
                            return
                        remaining = size
                        while remaining:
                            block = self.rfile.read(min(65536, remaining)); remaining -= len(block); yield block
                        if self.rfile.read(2) != b'\r\n': raise ValueError('Invalid chunk')
                else:
                    remaining = int(self.headers.get('Content-Length', '0'))
                    while remaining:
                        block = self.rfile.read(min(65536, remaining)); remaining -= len(block); yield block
            headers = {k: v for k, v in self.headers.items() if k.lower() not in ['host', 'connection', 'transfer-encoding', 'content-length']}
            has_body = 'Content-Length' in self.headers or 'Transfer-Encoding' in self.headers
            try:
                upstream.request(self.command, self.path, body=chunks() if has_body else None, headers=headers, encode_chunked=has_body)
                response = upstream.getresponse(); self.send_response(response.status)
                for k, v in response.getheaders():
                    if k.lower() not in ['connection', 'transfer-encoding', 'content-length']: self.send_header(k, v)
                self.send_header('Connection', 'close'); self.end_headers(); self.close_connection = True
                while block := response.read(65536): self.wfile.write(block)
            except (ConnectionError, TimeoutError, OSError):
                self.close_connection = True
            finally: upstream.close()
        do_GET = do_POST = do_PUT = do_DELETE = do_OPTIONS = forward
    server = http.server.ThreadingHTTPServer(('127.0.0.1', a.port), Proxy)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); tls.minimum_version = ssl.TLSVersion.TLSv1_2
    tls.load_cert_chain(cert, key); server.socket = tls.wrap_socket(server.socket, server_side=True)
    print(f'Writer test HTTPS on loopback:{a.port}; mode={a.mode}', flush=True)
    try: server.serve_forever()
    except KeyboardInterrupt: pass
    finally: server.server_close()
