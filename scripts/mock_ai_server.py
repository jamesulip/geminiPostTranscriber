#!/usr/bin/env python3
"""Offline stand-in for the AI providers, for `cargo test ai_end_to_end -- --ignored`.

Serves, on one port:
  OpenAI-compatible   POST /v1/chat/completions (JSON + SSE)   POST /v1/embeddings
  Gemini              POST /v1beta/models/<m>:generateContent, :streamGenerateContent, /v1beta/<models/m>:batchEmbedContents,
                      POST /v1beta/cachedContents
  Anthropic           POST /v1/messages (JSON + SSE)

Answers are deterministic: a digest request returns a fixed JSON digest; a question echoes the first
citation token it can build from the prompt (so citation validation is exercised); embeddings are
hash-seeded unit vectors, so identical texts match exactly and similar ones don't.

  python3 scripts/mock_ai_server.py            # port 8767
  MOCK_429_EVERY=3 python3 scripts/mock_ai_server.py   # every 3rd request is rate limited once
"""
import hashlib, json, math, os, re, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ.get("PORT", "8767"))
RATE_EVERY = int(os.environ.get("MOCK_429_EVERY", "0"))
DIMS = 768
count = 0
# Gemini cachedContents: name -> the system prompt + content stored in it.
CACHES = {}

DIGEST = {
    "title": "Sprint planning for the mobile app",
    "summary": "The team planned the sprint.\n\nLogin and payments were discussed.",
    "decisions": [{"text": "Ship the login flow first", "time": 120}],
    "actions": [{"owner": "Juan", "task": "Write the API spec", "due": "Friday", "time": 300}],
    "questions": [{"text": "Which payment provider?", "time": 420}],
    "topics": [{"title": "Login flow", "start": 0, "end": 300}, {"title": "Payments", "start": 300, "end": 600}],
    "glossary": [{"term": "GCash", "meaning": "Philippine e-wallet"}],
    "speakers": [{"label": "Speaker 1", "name": "Juan", "evidence": "addressed as Juan"}],
}


def vector(text: str):
    # Stable pseudo-embedding: words hash into buckets, so shared words → similar vectors.
    v = [0.0] * DIMS
    for w in re.findall(r"\w+", text.lower()):
        h = int(hashlib.sha256(w.encode()).hexdigest(), 16)
        v[h % DIMS] += 1.0
        v[(h >> 16) % DIMS] += 0.5
    n = math.sqrt(sum(x * x for x in v)) or 1.0
    return [x / n for x in v]


def answer_for(prompt: str) -> str:
    text = _answer_for(prompt)
    if os.environ.get("MOCK_VERBOSE"):
        sys.stderr.write(f"  -> {text[:110]!r}\n")
    return text


def _answer_for(prompt: str) -> str:
    if '"title"' in prompt and "glossary" in prompt:
        return json.dumps(DIGEST)
    if "summarise one part" in prompt.lower() or "part " in prompt.lower()[:40]:
        m = re.search(r"\[(\d+:\d\d(?::\d\d)?)\]", prompt)
        return f"They discussed the login flow and payments [{m.group(1) if m else '0:00'}]. Juan will write the spec."
    ids = re.findall(r"- ([0-9A-Za-z_-]+) → ", prompt)
    times = re.findall(r"\[(\d+):(\d\d)(?::(\d\d))?\]", prompt)
    if ids and times:
        h, m, s = times[-1]
        secs = (int(h) * 3600 + int(m) * 60 + int(s)) if s else (int(h) * 60 + int(m))
        return f"They decided to ship the login flow first [[t:{ids[0]}:{secs}]]. Bogus [[t:{ids[0]}:999999]] gets dropped."
    return "OK"


class H(BaseHTTPRequestHandler):
    def log_message(self, fmt, *a):
        if os.environ.get("MOCK_VERBOSE"):
            sys.stderr.write(f"{time.strftime('%H:%M:%S')} {fmt % a}\n")

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _sse(self, events):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for e in events:
            self.wfile.write(f"data: {e if isinstance(e, str) else json.dumps(e)}\n\n".encode())
            self.wfile.flush()

    def do_POST(self):
        global count
        count += 1
        n = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        path = self.path.split("?")[0]
        self.log_message("POST %s stream=%s", self.path, body.get("stream"))
        if RATE_EVERY and count % RATE_EVERY == 0:
            self.send_response(429)
            self.send_header("Retry-After", "1")
            self.end_headers()
            self.wfile.write(b'{"error":"slow down"}')
            return
        if path.endswith("/chat/completions"):
            prompt = "\n".join(m["content"] for m in body["messages"])
            text = answer_for(prompt)
            usage = {"prompt_tokens": len(prompt) // 4, "completion_tokens": len(text) // 4, "prompt_tokens_details": {"cached_tokens": 0}}
            if body.get("stream"):
                parts = [text[i : i + 7] for i in range(0, len(text), 7)]
                ev = [{"choices": [{"delta": {"content": p}}]} for p in parts]
                ev.append({"choices": [], "usage": usage})
                ev.append("[DONE]")
                return self._sse(ev)
            return self._json(200, {"choices": [{"message": {"content": text}}], "usage": usage})
        if path.endswith("/embeddings"):
            inp = body["input"]
            inp = inp if isinstance(inp, list) else [inp]
            return self._json(200, {"data": [{"index": i, "embedding": vector(t)} for i, t in enumerate(inp)], "usage": {"prompt_tokens": sum(len(t) // 4 for t in inp)}})
        if path.endswith(":batchEmbedContents"):
            return self._json(200, {"embeddings": [{"values": vector(r["content"]["parts"][0]["text"])} for r in body["requests"]]})
        if path.endswith("/v1beta/cachedContents"):
            name = f"cachedContents/mock-{len(CACHES) + 1}"
            CACHES[name] = body.get("systemInstruction", {}).get("parts", [{}])[0].get("text", "") + "\n" + "\n".join(p["text"] for c in body["contents"] for p in c["parts"])
            return self._json(200, {"name": name, "expireTime": "2099-01-01T00:00:00Z", "usageMetadata": {"totalTokenCount": 12345}})
        if ":generateContent" in path or ":streamGenerateContent" in path:
            prompt = (body.get("systemInstruction", {}).get("parts", [{}])[0].get("text", "") + "\n" + "\n".join(p["text"] for c in body["contents"] for p in c["parts"]))
            if body.get("cachedContent"):
                prompt = CACHES.get(body["cachedContent"], "") + "\n" + prompt
            text = answer_for(prompt)
            cached = 12345 if body.get("cachedContent") else 0
            meta = {"promptTokenCount": len(prompt) // 4 + cached, "candidatesTokenCount": len(text) // 4, "cachedContentTokenCount": cached}
            if ":streamGenerateContent" in path:
                parts = [text[i : i + 9] for i in range(0, len(text), 9)]
                ev = [{"candidates": [{"content": {"parts": [{"text": p}]}}]} for p in parts]
                ev[-1]["candidates"][0]["finishReason"] = "STOP"
                ev[-1]["usageMetadata"] = meta
                return self._sse(ev)
            return self._json(200, {"candidates": [{"content": {"parts": [{"text": text}]}, "finishReason": "STOP"}], "usageMetadata": meta})
        if path.endswith("/v1/messages"):
            if self.headers.get("x-api-key") is None or self.headers.get("anthropic-version") is None:
                return self._json(400, {"error": {"message": "missing anthropic headers"}})
            sys_text = body.get("system", "")
            if isinstance(sys_text, list):
                sys_text = "\n".join(b["text"] for b in sys_text)
            prompt = sys_text + "\n" + "\n".join(m["content"] for m in body["messages"])
            text = answer_for(prompt)
            usage_start = {"input_tokens": len(prompt) // 4, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 1}
            if body.get("stream"):
                ev = [{"type": "message_start", "message": {"usage": usage_start}}]
                ev += [{"type": "content_block_delta", "delta": {"type": "text_delta", "text": text[i : i + 8]}} for i in range(0, len(text), 8)]
                ev.append({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": len(text) // 4}})
                ev.append({"type": "message_stop"})
                return self._sse(ev)
            return self._json(200, {"content": [{"type": "text", "text": text}], "stop_reason": "end_turn", "usage": {**usage_start, "output_tokens": len(text) // 4}})
        self._json(404, {"error": f"no mock for {path}"})


if __name__ == "__main__":
    print(f"mock AI server on http://127.0.0.1:{PORT}", file=sys.stderr)
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
