#!/usr/bin/env python3
"""Mock web server for test/smoke.sh: plain HTTP on 127.0.0.1:18080, HTTPS on :18443 (a throwaway CA made with openssl).
The guest reaches it as 10.0.2.2 (QEMU's slirp maps that to the Mac's loopback). Usage: mock.py WORKDIR"""
import json, os, ssl, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

W = sys.argv[1]
def sh(*a): subprocess.run(a, cwd=W, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
sh("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "ca.key", "-out", "ca.pem", "-days", "2", "-subj", "/CN=thir-test-ca",
   "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign")
sh("openssl", "req", "-newkey", "rsa:2048", "-nodes", "-keyout", "srv.key", "-out", "srv.csr", "-subj", "/CN=10.0.2.2")
open(os.path.join(W, "ext"), "w").write("subjectAltName=IP:10.0.2.2\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\nkeyUsage=digitalSignature,keyEncipherment\n")
sh("openssl", "x509", "-req", "-in", "srv.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial", "-days", "2", "-out", "srv.pem", "-extfile", "ext")

BIN = bytes(range(256))
SLOW = float(os.environ.get("THIR_MOCK_SLOW", "0"))   # seconds per chunk (3x before the first byte): slow streams for screenshots

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def send(self, code, body, ctype="text/plain", extra=()):
        self.send_response(code); self.send_header("Content-Type", ctype); self.send_header("Content-Length", str(len(body)))
        for k, v in extra: self.send_header(k, v)
        self.end_headers(); self.wfile.write(body)
    def chunks(self, parts, ext=None):
        time.sleep(SLOW * 3)
        self.send_response(200); self.send_header("Content-Type", "text/event-stream" if ext is None else "text/plain"); self.send_header("Transfer-Encoding", "chunked"); self.end_headers()
        for i, p in enumerate(parts):
            self.wfile.write(b"%x%s\r\n" % (len(p), b";ext=1" if ext and i == 1 else b"") + p + b"\r\n"); self.wfile.flush(); time.sleep(max(SLOW, 0.02) if ext is None else 0)
        self.wfile.write(b"0\r\n\r\n"); self.wfile.flush()
    def redirect(self, to): self.send(302, b"", extra=[("Location", to)])
    def do_GET(self):
        p = self.path
        if p == "/hello": self.send(200, b"hello from mock\n")
        elif p == "/chunked": self.chunks([b"chunk-one\n", b"chunk-", b"two\n"], ext=True)
        elif p == "/redirect": self.redirect("/hello")
        elif p == "/loop": self.redirect("/loop")
        elif p == "/bin": self.send(200, BIN, "application/octet-stream")
        elif p == "/ca.pem": self.send(200, open(os.path.join(W, "ca.pem"), "rb").read())
        elif p == "/slow":
            self.send_response(200); self.send_header("Content-Length", "5"); self.end_headers(); self.wfile.flush(); time.sleep(5); self.wfile.write(b"late\n")
        elif p == "/downgrade": self.redirect("http://10.0.2.2:18080/hello")
        else: self.send(404, b"not found\n")
    do_HEAD = do_GET
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); body = self.rfile.read(n)
        auth = "auth" if (self.headers.get("authorization") or self.headers.get("x-api-key")) else "noauth"
        if self.path == "/echo": self.send(200, ("POST %s %s\n" % ("same" if body == BIN else "different", auth)).encode())
        else: self.send(404, b"not found\n")

class S(ThreadingHTTPServer):
    daemon_threads = True
    def handle_error(self, *a): pass

plain = S(("127.0.0.1", 18080), H)
tls = S(("127.0.0.1", 18443), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain(os.path.join(W, "srv.pem"), os.path.join(W, "srv.key"))
tls.socket = ctx.wrap_socket(tls.socket, server_side=True, do_handshake_on_connect=False)
threading.Thread(target=tls.serve_forever, daemon=True).start()
print("mock ready", flush=True)
plain.serve_forever()
