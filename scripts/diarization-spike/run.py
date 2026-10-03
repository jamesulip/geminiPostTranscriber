"""Spike: sherpa-onnx (pyannote segmentation + speaker embeddings) on one meeting, compared to Gemini labels."""
import sys, time, wave, json
from collections import Counter, defaultdict
import numpy as np
import sherpa_onnx

emb_model = sys.argv[1]
num_speakers = int(sys.argv[2]) if len(sys.argv) > 2 else -1  # -1 = cluster by threshold
threshold = float(sys.argv[3]) if len(sys.argv) > 3 else 0.5
out_name = sys.argv[4] if len(sys.argv) > 4 else "turns.json"

cfg = sherpa_onnx.OfflineSpeakerDiarizationConfig(
    segmentation=sherpa_onnx.OfflineSpeakerSegmentationModelConfig(
        pyannote=sherpa_onnx.OfflineSpeakerSegmentationPyannoteModelConfig(model=__import__("os").environ.get("SEG","sherpa-onnx-pyannote-segmentation-3-0/model.onnx")),
        num_threads=int(__import__("os").environ.get("THREADS","8")),
    ),
    embedding=sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=emb_model, num_threads=int(__import__("os").environ.get("THREADS","8"))),
    clustering=sherpa_onnx.FastClusteringConfig(num_clusters=num_speakers, threshold=threshold),
    min_duration_on=0.3,
    min_duration_off=0.5,
)
assert cfg.validate(), "bad config"
sd = sherpa_onnx.OfflineSpeakerDiarization(cfg)

with wave.open("sep1.wav") as w:
    assert w.getframerate() == sd.sample_rate == 16000
    audio = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768
dur = len(audio) / 16000

last = [-1]
def progress(done, total, *_):
    p = int(done / total * 100)
    if p // 10 != last[0] // 10:
        print(f"  {p}%", end="", flush=True); last[0] = p
    return 0

t0 = time.time()
res = sd.process(audio, callback=progress).sort_by_start_time()
elapsed = time.time() - t0
print(f"\n{emb_model}: {dur/60:.1f} min audio in {elapsed:.0f}s ({dur/elapsed:.0f}x realtime), {len(res)} turns, speakers={len({r.speaker for r in res})}")

turns = [(r.start, r.end, r.speaker) for r in res]
json.dump(turns, open(out_name, "w"))

# --- compare with Gemini -----------------------------------------------------
gem = []
for line in open("gemini.tsv"):
    s, e, spk, *_ = line.rstrip("\n").split("\t")
    gem.append((float(s), float(e), spk or "?"))

def overlap(a0, a1, b0, b1):
    return max(0.0, min(a1, b1) - max(a0, b0))

# Overlap time between every Gemini label and every diarizer speaker.
conf = defaultdict(float)
for gs, ge, gl in gem:
    for ts, te, tl in turns:
        if te < gs: continue
        if ts > ge: break
        conf[(gl, tl)] += overlap(gs, ge, ts, te)

glabels = sorted({g[2] for g in gem})
by_time = Counter(); [by_time.update({t[2]: t[1]-t[0]}) for t in turns]
tlabels = [t for t, _ in by_time.most_common(10)]
print("\nOverlap minutes (rows: Gemini, cols: sherpa):")
print("            " + "".join(f"{'S'+str(t):>8}" for t in tlabels))
for gl in glabels:
    print(f"{gl:>11} " + "".join(f"{conf[(gl,tl)]/60:8.1f}" for tl in tlabels))

# Best mapping (greedy) and agreement: fraction of overlapped time where the mapped labels agree.
pairs = sorted(conf.items(), key=lambda kv: -kv[1]); used_g, used_t, mapping = set(), set(), {}
for (gl, tl), v in pairs:
    if gl not in used_g and tl not in used_t:
        mapping[tl] = gl; used_g.add(gl); used_t.add(tl)
total = sum(conf.values()); agree = sum(v for (gl, tl), v in conf.items() if mapping.get(tl) == gl)
print(f"\nmapping sherpa→gemini: {mapping}")
print(f"agreement on overlapped speech: {agree/total*100:.1f}% of {total/60:.1f} min")

# Per Gemini segment: dominant diarizer speaker, and how many segments contain a speaker change.
dominant_agree = 0; multi = 0; covered = 0
for gs, ge, gl in gem:
    share = Counter()
    for ts, te, tl in turns:
        if te < gs: continue
        if ts > ge: break
        share[tl] += overlap(gs, ge, ts, te)
    if not share: continue
    covered += 1
    top, topv = share.most_common(1)[0]
    if mapping.get(top) == gl: dominant_agree += 1
    if sum(share.values()) - topv > 1.0: multi += 1
print(f"segments: {len(gem)}, with diarizer speech: {covered}, dominant speaker matches Gemini: {dominant_agree} ({dominant_agree/covered*100:.0f}%), >1s of a second speaker inside: {multi}")
