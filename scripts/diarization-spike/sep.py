"""Embed each Gemini segment (>=3 s) and ask: are Speaker 1 and Speaker 2 separable at all?"""
import wave, sys, numpy as np, sherpa_onnx
model = sys.argv[1]
ex = sherpa_onnx.SpeakerEmbeddingExtractor(sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=model, num_threads=8))
with wave.open("sep1.wav") as w:
    audio = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768
segs = []
for line in open("gemini.tsv"):
    s, e, spk, *_ = line.rstrip("\n").split("\t"); s, e = float(s), float(e)
    if e - s >= 3 and spk in ("Speaker 1", "Speaker 2"): segs.append((s, e, spk))
embs, labels = [], []
for s, e, spk in segs:
    st = ex.create_stream(); st.accept_waveform(16000, audio[int(s*16000):int(e*16000)]); st.input_finished()
    v = np.array(ex.compute(st)); embs.append(v / np.linalg.norm(v)); labels.append(spk)
E = np.array(embs); L = np.array(labels)
S = E @ E.T
def mean_sim(a, b):
    m = S[np.ix_(L == a, L == b)]
    if a == b: m = m[~np.eye(len(m), dtype=bool)]
    return m.mean()
print(f"{len(segs)} segments embedded with {model}")
print(f"mean cosine  Sp1~Sp1 {mean_sim('Speaker 1','Speaker 1'):.3f}   Sp2~Sp2 {mean_sim('Speaker 2','Speaker 2'):.3f}   Sp1~Sp2 {mean_sim('Speaker 1','Speaker 2'):.3f}")
# 2-means on the embeddings: does the audio itself have two voices, and do they match Gemini?
rng = np.random.default_rng(0); c = E[rng.choice(len(E), 2, replace=False)]
for _ in range(50):
    a = (E @ c.T).argmax(1); c = np.array([E[a == k].mean(0) for k in range(2)]); c /= np.linalg.norm(c, axis=1, keepdims=True)
for k in range(2):
    n1, n2 = ((a == k) & (L == 'Speaker 1')).sum(), ((a == k) & (L == 'Speaker 2')).sum()
    print(f"cluster {k}: {n1} Gemini-Sp1 segments, {n2} Gemini-Sp2 segments")
print(f"centroid cosine between the two clusters: {float(c[0] @ c[1]):.3f}")
# Silhouette-ish: how confidently does each segment sit in its cluster?
d = E @ c.T; margin = np.abs(d[:, 0] - d[:, 1])
print(f"cluster margin: median {np.median(margin):.3f}, 25% of segments below {np.percentile(margin, 25):.3f}")

# Is the 2-cluster structure about voices, or about time (room/mic conditions drifting)?
import re
starts = np.array([s for s, _, _ in segs]); texts = [t for t in open("gemini.tsv").read().split("\n") if t]
sir_by_seg = {}
for line in texts:
    s, e, spk, text = line.split("\t"); sir_by_seg[float(s)] = bool(re.search(r"\bsir\b|\bpo\b", text, re.I))
print("per 10-min window: share of segments in cluster 0")
for w0 in range(0, int(starts.max()) + 1, 600):
    m = (starts >= w0) & (starts < w0 + 600)
    if m.sum(): print(f"  {w0//60:3d}-{w0//60+10:3d} min: {(a[m]==0).mean()*100:3.0f}%  (n={m.sum()})")
for k in range(2):
    rate = np.mean([sir_by_seg[s] for s, k2 in zip(starts, a) if k2 == k])
    print(f"embedding cluster {k}: 'sir/po' in {rate*100:.0f}% of segments")
