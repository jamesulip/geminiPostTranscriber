# Mock OpenAI-compatible /audio/transcriptions: returns one segment per non-silent region
# of the uploaded audio (chunk-relative), so time mapping can be verified end to end.
import json, re, subprocess, tempfile
from email.parser import BytesParser
from email.policy import default
from http.server import BaseHTTPRequestHandler, HTTPServer

import os
EVERY = int(os.environ.get("MOCK_429_EVERY", "0"))  # simulate rate limits on every Nth request
count = 0

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        global count
        count += 1
        if EVERY and count % EVERY == 0:
            self.rfile.read(int(self.headers["Content-Length"]))
            self.send_response(429)
            if os.environ.get("MOCK_GOOGLE_429"):
                # Google style: delay and quota in the body, no Retry-After header.
                self.end_headers()
                self.wfile.write(json.dumps({"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "details": [
                    {"@type": "type.googleapis.com/google.rpc.QuotaFailure", "violations": [{
                        "quotaId": "GenerateContentInputTokensPerModelPerMinute-FreeTier", "quotaValue": "250000"}]},
                    {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "1.2s"}]}}).encode())
            else:
                self.send_header("Retry-After", "1"); self.end_headers()
                self.wfile.write(b'{"error":{"message":"rate limited"}}')
            print("429 sent", flush=True)
            return
        raw = self.rfile.read(int(self.headers["Content-Length"]))
        msg = BytesParser(policy=default).parsebytes(b"Content-Type: " + self.headers["Content-Type"].encode() + b"\r\n\r\n" + raw)
        parts = {p.get_param("name", header="content-disposition"): p for p in msg.iter_parts()}
        data = parts["file"].get_payload(decode=True)
        model = parts["model"].get_content().strip()
        with tempfile.NamedTemporaryFile(suffix=".ogg") as f:
            f.write(data); f.flush()
            log = subprocess.run(["ffmpeg", "-hide_banner", "-i", f.name, "-af", "silencedetect=noise=-35dB:d=0.15",
                                  "-f", "null", "-"], capture_output=True, text=True).stderr
        dur = re.search(r"Duration: (\d+):(\d+):([\d.]+)", log)
        total = int(dur[1]) * 3600 + int(dur[2]) * 60 + float(dur[3])
        starts = [float(x) for x in re.findall(r"silence_start: (-?[\d.]+)", log)]
        ends = [float(x) for x in re.findall(r"silence_end: ([\d.]+)", log)]
        # speech = gaps between silences
        speech, cur = [], 0.0
        for s, e in zip(starts, ends + [total]):
            if s - cur > 0.3: speech.append((cur, s))
            cur = e
        if total - cur > 0.3: speech.append((cur, total))
        # Simulate long-audio failures: chunks longer than MOCK_HOLE_OVER seconds lose their middle third.
        hole_over = float(os.environ.get("MOCK_HOLE_OVER", "0"))
        if hole_over and total > hole_over:
            speech = [(a, b) for a, b in speech if not (total / 3 < a < 2 * total / 3)]
        segs = [{"id": i, "start": round(a, 3), "end": round(b, 3), "text": f"utterance {i} model={model}"}
                for i, (a, b) in enumerate(speech)]
        body = json.dumps({"text": " ".join(s["text"] for s in segs), "segments": segs}).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(body)
        print("chunk", round(total, 2), "segments", [(s["start"], s["end"]) for s in segs], flush=True)

HTTPServer(("127.0.0.1", 8765), H).serve_forever()
