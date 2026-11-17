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

APPS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "apps")   # canned apps for the app tests
BIN = bytes(range(256))
USER_TEXT = "mock reply %d café"

def err(reason): return 400, json.dumps({"type": "error", "error": {"type": "invalid_request_error", "message": "mock: " + reason}}).encode()

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def send(self, code, body, ctype="text/plain", extra=()):
        self.send_response(code); self.send_header("Content-Type", ctype); self.send_header("Content-Length", str(len(body)))
        for k, v in extra: self.send_header(k, v)
        self.end_headers(); self.wfile.write(body)
    def chunks(self, parts, ext=None):
        self.send_response(200); self.send_header("Content-Type", "text/event-stream" if ext is None else "text/plain"); self.send_header("Transfer-Encoding", "chunked"); self.end_headers()
        for i, p in enumerate(parts):
            self.wfile.write(b"%x%s\r\n" % (len(p), b";ext=1" if ext and i == 1 else b"") + p + b"\r\n"); self.wfile.flush(); time.sleep(0.02 if ext is None else 0)
        self.wfile.write(b"0\r\n\r\n"); self.wfile.flush()
    def redirect(self, to): self.send(302, b"", extra=[("Location", to)])
    def sse(self, events):   # each event is cut in two so lines are split across chunks
        parts = []
        for e in events:
            b = e.encode(); m = len(b) // 2; parts += [b[:m], b[m:]]
        self.chunks(parts)
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
        elif p.startswith("/apps/") and "/" not in p[6:] and not p[6:].startswith(".") and os.path.isfile(os.path.join(APPS, p[6:])): self.send(200, open(os.path.join(APPS, p[6:]), "rb").read())
        elif p == "/downgrade": self.redirect("http://10.0.2.2:18080/hello")
        else: self.send(404, b"not found\n")
    do_HEAD = do_GET
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); body = self.rfile.read(n); p = self.path
        auth = "auth" if (self.headers.get("authorization") or self.headers.get("x-api-key")) else "noauth"
        if p == "/echo": self.send(200, ("POST %s %s\n" % ("same" if body == BIN else "different", auth)).encode())
        elif p in ("/anthropic/v1/messages", "/refuse/v1/messages"): self.anthropic(body, auth, p.startswith("/refuse"))
        elif p == "/build/v1/messages": self.build(body)
        elif p == "/openai/v1/chat/completions": self.openai(body, auth)
        elif p == "/err/v1/chat/completions": self.send(401, b'{"error":{"message":"Incorrect API key provided"}}', "application/json")
        else: self.send(404, b"not found\n")
    def anthropic(self, raw, auth, refuse):
        h = self.headers
        try: j = json.loads(raw)
        except ValueError: return self.send(400, *[err("bad json")[1]], "application/json")
        why = None
        if h.get("anthropic-version") != "2026-10-21": why = "anthropic-version"
        elif h.get("anthropic-beta") != "server-side-fallback-2026-07-01": why = "anthropic-beta"
        elif h.get("content-type") != "application/json": why = "content-type"
        elif auth != "noauth": why = "a key was sent to a plain http provider"
        elif j.get("model") != "claude-opus-5-5": why = "model"
        elif j.get("stream") is not True: why = "stream"
        elif j.get("max_tokens") != 16000: why = "max_tokens"
        elif j.get("output_config") != {"effort": "low"}: why = "output_config"
        elif j.get("fallbacks") != "default": why = "fallbacks"
        elif any(k in j for k in ("temperature", "top_p", "top_k", "thinking")): why = "sampling or thinking field"
        elif not j.get("messages") or j["messages"][-1].get("role") != "user": why = "last role"
        if why: return self.send(400, err(why)[1], "application/json")
        d = lambda o: "event: %s\ndata: %s\n\n" % (o["type"], json.dumps(o, ensure_ascii=False))
        n = len(j["messages"])
        ev = [d({"type": "message_start", "message": {"id": "m1", "model": "claude-opus-5-5", "role": "assistant", "content": []}}),
              d({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}}),
              d({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "c2ln"}}),
              d({"type": "content_block_stop", "index": 0}), d({"type": "ping"}),
              d({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
              d({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": USER_TEXT % n}}),
              d({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "\nsecond line \U0001F43B"}}),
              d({"type": "content_block_stop", "index": 1}),
              d({"type": "message_delta", "delta": {"stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber", "explanation": None}} if refuse else {"stop_reason": "end_turn"}}),
              d({"type": "message_stop"})]
        self.sse(ev)
    def build(self, raw):   # app builds: lenient (no key, no version headers), but stream and the build effort are checked
        j = json.loads(raw)
        if j.get("stream") is not True or j.get("output_config") != {"effort": "medium"}: return self.send(400, err("build request")[1], "application/json")
        m = j["messages"][-1]["content"]; app = lambda f: open(os.path.join(APPS, f)).read()
        if "===ERROR===" in m: text = app("calc.reply")
        elif "===CHANGE===" in m: text = "NO: thir apps cannot play sound." if "sound" in m.split("===SPEC===")[0] else app("calc2.reply")
        elif "===SPEC===" in m: text = app("calc-bad.reply")
        elif "===REQUEST===" in m: text = "NO: a recipe is not an app." if "lasagna" in m else app("calc.spec")
        else: return self.send(400, err("unknown step")[1], "application/json")
        d = lambda o: "event: %s\ndata: %s\n\n" % (o["type"], json.dumps(o, ensure_ascii=False))
        lines = text.splitlines(True)
        self.sse([d({"type": "message_start", "message": {"id": "m2", "model": "claude-opus-5-5", "role": "assistant", "content": []}}),
                  d({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})]
                 + [d({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "".join(lines[i:i + 8])}}) for i in range(0, len(lines), 8)]
                 + [d({"type": "content_block_stop", "index": 0}), d({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}), d({"type": "message_stop"})])
    def openai(self, raw, auth):
        j = json.loads(raw)
        if auth != "noauth" or j.get("model") != "mock-model" or j.get("stream") is not True or j["messages"][0]["role"] != "system": return self.send(400, b'{"error":{"message":"mock: bad request"}}', "application/json")
        n = len([m for m in j["messages"] if m["role"] != "system"])
        c = lambda delta, fin=None: "data: %s\n\n" % json.dumps({"choices": [{"index": 0, "delta": delta, "finish_reason": fin}]})
        self.sse([c({"role": "assistant"}), c({"content": "openai mock reply %d" % n}), c({}, "stop"), "data: [DONE]\n\n"])

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
