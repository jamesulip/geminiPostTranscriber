import json, sys
from collections import Counter
turns = json.load(open(sys.argv[1])); mapping = json.loads(sys.argv[2])
gem = []
for line in open("gemini.tsv"):
    s, e, spk, text = line.rstrip("\n").split("\t"); gem.append((float(s), float(e), spk, text))
def ov(a0,a1,b0,b1): return max(0.0, min(a1,b1)-max(a0,b0))
def clock(t): t=int(t); return f"{t//3600}:{t%3600//60:02d}:{t%60:02d}" if t>=3600 else f"{t//60}:{t%60:02d}"
rows = []
for gs, ge, gl, text in gem:
    share = Counter()
    for ts, te, tl in turns:
        if te < gs: continue
        if ts > ge: break
        share[tl] += ov(gs, ge, ts, te)
    if not share or ge - gs < 6: continue
    top, v = share.most_common(1)[0]
    if mapping[str(top)] != gl and v / (ge - gs) > 0.8:
        rows.append((ge - gs, gs, ge, gl, mapping[str(top)], text))
rows.sort(reverse=True)
print(f"{len(rows)} segments >=6 s where sherpa is >80% sure it's a different speaker than Gemini says. Longest:")
for d, gs, ge, gl, sl, text in rows[:12]:
    print(f"  {clock(gs)}-{clock(ge)}  Gemini: {gl}  sherpa: {sl}   “{text[:90]}”")
