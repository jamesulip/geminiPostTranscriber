# Minimal fake of the Gemini Files + Batch APIs used by the app, plus generateContent that blocks
# every model except gemini-3.5-flash, and the Interactions API (gemini-3.5-transcribe).
# Path prefixes change that: /flash-blocks blocks every generateContent model, /always-block also
# fails every interaction.
import json, re
from http.server import BaseHTTPRequestHandler, HTTPServer
files, batches, polls = {}, {}, {}
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send_json(self, obj, code=200, headers=None):
        b = json.dumps(obj).encode(); self.send_response(code)
        for k, v in (headers or {}).items(): self.send_header(k, v)
        self.send_header("Content-Type", "application/json"); self.end_headers(); self.wfile.write(b)
    def body(self): return self.rfile.read(int(self.headers.get("Content-Length", 0)))
    def route(self):
        for p in ("/always-block", "/flash-blocks"):
            if self.path.startswith(p): return p[1:], self.path[len(p):]
        return "", self.path
    def do_DELETE(self): self.send_json({})
    def do_POST(self):
        mode, path = self.route()
        if self.headers.get("x-goog-api-key") != "test-key" and not path.startswith("/resumable"): return self.send_json({"error": {"message": "bad key"}}, 401)
        if path == "/upload/v1beta/files":
            meta = json.loads(self.body()); fid = f"files/f{len(files)+1}"
            files[fid] = {"meta": meta}
            return self.send_json({}, headers={"x-goog-upload-url": f"http://{self.headers['Host']}/resumable/{fid}"})
        if path.startswith("/resumable/"):
            fid = path[len("/resumable/"):]; files[fid]["data"] = self.body()
            return self.send_json({"file": {"name": fid, "uri": f"https://x/{fid}", "state": "ACTIVE", "mimeType": files[fid]["meta"]["file"]["mime_type"]}})
        m = re.match(r"/v1beta/models/([^:]+):batchGenerateContent", path)
        if m:
            req = json.loads(self.body()); fid = req["batch"]["input_config"]["file_name"]
            bid = f"batches/b{len(batches)+1}"; batches[bid] = {"input": fid, "model": m.group(1)}; polls[bid] = 0
            return self.send_json({"name": bid, "metadata": {"name": bid, "state": "JOB_STATE_PENDING"}})
        m = re.match(r"/v1beta/models/([^:]+):generateContent", path)
        if m:
            req = json.loads(self.body()); assert "inline_data" in req["contents"][0]["parts"][0]
            if mode or m.group(1) != "gemini-3.5-flash":
                return self.send_json({"promptFeedback": {"blockReason": "OTHER"},
                                       "usageMetadata": {"promptTokenCount": 1500, "thoughtsTokenCount": 400}})
            text = json.dumps([{"start": 1.0, "end": 3.0, "speaker": "Speaker 1", "text": f"text from {m.group(1)}"}])
            return self.send_json({"candidates": [{"content": {"parts": [{"text": text}]}, "finishReason": "STOP"}],
                                   "usageMetadata": {"promptTokenCount": 1500, "candidatesTokenCount": 100}})
        if path == "/v1beta/interactions":
            req = json.loads(self.body()); assert req["input"][0]["uri"].startswith("https://x/files/")
            if mode == "always-block":
                return self.send_json({"id": "interactions/i1", "status": "failed", "error": {"message": "internal"}})
            words = [{"type": "word_info", "text": w, "speaker": "spk_1", "start_offset": f"{1 + i}.0s", "end_offset": f"{1.5 + i}s"}
                     for i, w in enumerate(["text", "from", req["model"]])]
            return self.send_json({"id": "interactions/i1", "status": "completed", "usage": {"total_input_tokens": 1200},
                                   "steps": [{"type": "model_output", "content": [{"type": "text", "text": "x", "annotations": words}]}]})
        m = re.match(r"/v1beta/(batches/[^:]+):cancel", path)
        if m: batches[m.group(1)]["cancelled"] = True; return self.send_json({})
        self.send_json({"error": {"message": "unknown"}}, 404)
    def do_GET(self):
        m = re.match(r"/v1beta/(batches/\w+)$", self.path)
        if m:
            bid = m.group(1); polls[bid] += 1
            if polls[bid] < 2: return self.send_json({"name": bid, "metadata": {"state": "JOB_STATE_RUNNING"}})
            # Produce one result line per request line: echo a segment mentioning the key.
            lines = files[batches[bid]["input"]]["data"].decode().strip().split("\n")
            out = []
            for l in lines:
                j = json.loads(l); assert "inline_data" in j["request"]["contents"][0]["parts"][0]
                text = json.dumps([{"start": 1.0, "end": 3.0, "speaker": "Speaker 1", "text": f"text of {j['key']}"}])
                out.append(json.dumps({"key": j["key"], "response": {"candidates": [{"content": {"parts": [{"text": text}]}}],
                                       "usageMetadata": {"promptTokenCount": 1500, "candidatesTokenCount": 100}}}))
            files["files/results"] = {"data": "\n".join(out).encode()}
            return self.send_json({"name": bid, "done": True, "metadata": {"state": "BATCH_STATE_SUCCEEDED", "output": {"responsesFile": "files/results"}}})
        m = re.match(r"/download/v1beta/(files/\w+):download\?alt=media", self.path)
        if m:
            d = files[m.group(1)]["data"]; self.send_response(200); self.end_headers(); return self.wfile.write(d)
        self.send_json({"error": {"message": "unknown"}}, 404)
HTTPServer(("127.0.0.1", 8766), H).serve_forever()
