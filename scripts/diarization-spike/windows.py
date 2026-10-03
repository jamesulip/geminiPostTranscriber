import json, sys
from collections import defaultdict
turns = json.load(open(sys.argv[1])); win = float(sys.argv[2]) * 60
gem = []
for line in open("gemini.tsv"):
    s, e, spk, *_ = line.rstrip("\n").split("\t"); gem.append((float(s), float(e), spk or "?"))
def ov(a0,a1,b0,b1): return max(0.0, min(a1,b1)-max(a0,b0))
end = max(g[1] for g in gem); w0 = 0
print("per window: overlap seconds of Gemini Speaker 1 / Speaker 2 with each sherpa cluster")
while w0 < end:
    conf = defaultdict(float)
    for gs, ge, gl in gem:
        if ge < w0 or gs > w0 + win: continue
        for ts, te, tl in turns:
            if te < gs: continue
            if ts > ge: break
            conf[(gl, tl)] += ov(max(gs,w0), min(ge,w0+win), ts, te)
    row = lambda gl: "  ".join(f"S{tl}:{conf[(gl,tl)]:5.0f}" for tl in range(4))
    print(f"{int(w0//60):3d}-{int((w0+win)//60):3d} min   Sp1 → {row('Speaker 1')}   |   Sp2 → {row('Speaker 2')}")
    w0 += win
