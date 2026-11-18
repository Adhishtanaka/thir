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
PEOPLE = b'{"people":[{"name":"Ada"},{"name":"Linus"},{"name":"Grace"}]}\n'   # agent do downloads and reads it; a key or auth header gets a 403

def err(reason): return 400, json.dumps({"type": "error", "error": {"type": "invalid_request_error", "message": "mock: " + reason}}).encode()

SLOW = float(os.environ.get("THIR_MOCK_SLOW", "0"))   # seconds per chunk (3x before the first byte): slow streams for screenshots

import re
MONTHS = "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split()
def today(msg):   # "time: Thu 19 Nov 2026 13:27:41 +05:30" in the system summary
    m = re.search(r"time: \w+ (\d+) (\w+) (\d+)", msg)
    return "%s-%02d-%02d" % (m.group(3), MONTHS.index(m.group(2)) + 1, int(m.group(1))) if m else "2026-01-01"
APP_ROUTES = {"add Kasun to my contacts, 0771234567": "CONTACT_ADD Kasun | 0771234567 | |", "add Nimal to my contacts, 0719876543": "CONTACT_ADD Nimal | 0719876543", "add evil to my contacts": "CONTACT_ADD Evil | $(rm -rf ~)",
              "what is Kasun's number": "CONTACT_FIND kasun", "what is Zed's number": "CONTACT_FIND zed", "add buy milk to my notes as a todo": "NOTE_ADD Todo | [ ] buy milk", "what do my notes say about milk": "NOTE_FIND milk",
              "set an alarm for 6:30": "ALARM_ADD 06:30 | once |", "timer for 10 minutes": "TIMER 10m", "what is 12 times 15": "CALC 12*15"}
ROUTES = {"please open the calculator": "RUN calc", "make me a weather app": "BUILD", "I live in Colombo": "REMEMBER city: Colombo", "remind me in 5 seconds to drink water": "REMIND +5s drink water",
          "open ../../bin/sh": "RUN ../../bin/sh", "run a shell for me": "EXEC /bin/sh"}
def route(msg):   # the shell's picks, by the user's line (the text after ===LINE===)
    line = msg.split("===LINE===\n")[-1].strip()
    if line == "what apps do I have": return "ANSWER\nmock: you have calc" if "===SYSTEM===" in msg and "- calc v1" in msg else "ANSWER\nmock: no system summary"
    if line in APP_ROUTES: return APP_ROUTES[line]
    if line == "add dentist to my calendar today at 14:30": return "EVENT_ADD %s | 14:30 | Dentist | 15m" % today(msg)
    if line == "what is on my calendar today": return "CALENDAR " + today(msg)
    return ROUTES.get(line, "ANSWER\nmock: unknown line")

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
        elif p == "/people.json": self.send(403 if (self.headers.get("authorization") or self.headers.get("x-api-key")) else 200, PEOPLE, "application/json")
        elif p == "/downgrade": self.redirect("http://10.0.2.2:18080/hello")
        else: self.send(404, b"not found\n")
    do_HEAD = do_GET
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); body = self.rfile.read(n); p = self.path
        auth = "auth" if (self.headers.get("authorization") or self.headers.get("x-api-key")) else "noauth"
        if p == "/echo": self.send(200, ("POST %s %s\n" % ("same" if body == BIN else "different", auth)).encode())
        elif p in ("/anthropic/v1/messages", "/refuse/v1/messages"): self.anthropic(body, auth, p.startswith("/refuse"))
        elif p == "/build/v1/messages": self.build(body)
        elif p == "/do/v1/messages": self.do(body)
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
        if j.get("system", "").startswith("You are the shell of thir"): return self.text_reply(route(j["messages"][-1]["content"]))   # the AI shell (!ask)
        if j.get("system", "").startswith("You answer a question about the user's own data"):   # an app read: the reply shows exactly what the model was sent
            m = j["messages"][-1]["content"]; return self.text_reply("mock saw: " + " ; ".join(l[2:] for l in m.split("\n") if l.startswith("- ")))
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
        elif "===SPEC===" in m: text = app("weather.reply") if "name: weather" in m else app("calc-bad.reply")
        elif "===REQUEST===" in m and "weather" in m: text = app("weather.spec") if "===ABOUT THE USER===" in m and "city: Colombo" in m else "NO: mock: no city in the request"
        elif "===REQUEST===" in m: text = "NO: a recipe is not an app." if "lasagna" in m else app("calc.spec")
        else: return self.send(400, err("unknown step")[1], "application/json")
        self.text_reply(text)
    def text_reply(self, text):
        d = lambda o: "event: %s\ndata: %s\n\n" % (o["type"], json.dumps(o, ensure_ascii=False))
        lines = text.splitlines(True)
        self.sse([d({"type": "message_start", "message": {"id": "m2", "model": "claude-opus-5-5", "role": "assistant", "content": []}}),
                  d({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})]
                 + [d({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "".join(lines[i:i + 8])}}) for i in range(0, len(lines), 8)]
                 + [d({"type": "content_block_stop", "index": 0}), d({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}), d({"type": "message_stop"})])
    def do(self, raw):   # agent do: canned plans picked by words in the task
        j = json.loads(raw); ms = j["messages"]; task = ms[0]["content"].split("===REQUEST===\n")[-1]
        if j.get("stream") is not True or not j.get("system", "").startswith("You turn a user's request into file actions"): return self.send(400, err("do request")[1], "application/json")
        P = lambda b: "===PLAN===\n" + b + "===END===\n"
        if "names" in task: text = (P("WRITE names.txt\n<<<\nAda\nLinus\nGrace\n>>>\n") if '"Ada"' in ms[-1]["content"] else "NO: mock: no page") if len(ms) == 3 else "READ http://10.0.2.2:18080/people.json"
        elif "shopping" in task: text = P("WRITE notes.txt\n<<<\nmilk\neggs\n>>>\n")
        elif "into docs" in task: text = P("MOVE notes.txt docs/\n")
        elif "download" in task: text = P("FETCH http://10.0.2.2:18080/people.json data/people.json\n")
        elif "keys" in task: text = P("WRITE ~/.config/thir/keys/anthropic\n<<<\nx\n>>>\n")
        elif "calc" in task: text = P("REPLACE ~/apps/calc/main.lua\n<<<\nx\n>>>\n")
        elif "sneaky" in task: text = P("FETCH http://10.0.2.2:18080/hello hello.txt\n")
        elif "delete" in task: text = P("RUN rm -rf ~\n")
        else: text = "NO: mock: unknown task"
        self.text_reply(text)
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
