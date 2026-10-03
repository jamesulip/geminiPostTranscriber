"""Linguistic ground truth proxy: the presenter says "sir" constantly; the boss doesn't."""
import json, sys, re
from collections import Counter, defaultdict
gem = []
for line in open("gemini.tsv"):
    s, e, spk, text = line.rstrip("\n").split("\t"); gem.append((float(s), float(e), spk, text))
def ov(a0,a1,b0,b1): return max(0.0, min(a1,b1)-max(a0,b0))
sir = lambda t: bool(re.search(r"\bsir\b|\bpo\b", t, re.I))
print("Gemini labels: share of segments containing 'sir'/'po' (address term for the boss)")
for spk in ("Speaker 1", "Speaker 2", "Speaker 3", "Speaker 4"):
    segs = [g for g in gem if g[2] == spk and len(g[3].split()) >= 6]
    if segs: print(f"  {spk}: {sum(sir(g[3]) for g in segs)/len(segs)*100:4.0f}% of {len(segs)} segments")
for name in sys.argv[1:]:
    turns = json.load(open(name))
    print(f"{name}: same measure for the dominant sherpa cluster of each segment")
    stats = defaultdict(lambda: [0, 0])
    for gs, ge, gl, text in gem:
        if len(text.split()) < 6: continue
        share = Counter()
        for ts, te, tl in turns:
            if te < gs: continue
            if ts > ge: break
            share[tl] += ov(gs, ge, ts, te)
        if not share: continue
        top, v = share.most_common(1)[0]
        if ge - gs <= 0 or v / (ge - gs) < 0.7: continue
        stats[top][0] += sir(text); stats[top][1] += 1
    for k, (a, n) in sorted(stats.items(), key=lambda kv: -kv[1][1]):
        print(f"  S{k}: {a/n*100:4.0f}% of {n} segments")
