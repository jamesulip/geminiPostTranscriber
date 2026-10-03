"""Apply sherpa turns to the CURRENT version of a transcript: each segment gets the speaker with the
most overlapping diarizer time. Names are carried over from a previous labeling (old_segments.tsv)
by majority vote: for each sherpa cluster, the old display name it overlapped most."""
import json, sqlite3, sys
from collections import Counter, defaultdict
DB = "/Users/james-tiger/Library/Application Support/ph.tigergraphics.transcriber/library.sqlite"
tid, turns_path, dry = sys.argv[1], sys.argv[2], "--apply" not in sys.argv
turns = json.load(open(turns_path))
def ov(a0,a1,b0,b1): return max(0.0, min(a1,b1)-max(a0,b0))
def dominant(gs, ge):
    share = Counter()
    for ts, te, tl in turns:
        if te < gs: continue
        if ts > ge: break
        share[tl] += ov(gs, ge, ts, te)
    if not share: return None
    top, v = share.most_common(1)[0]
    return top if v >= 0.3 * max(0.1, ge - gs) or v >= 1.0 else None

# Old labeling with names (exported before re-transcribing).
old = [l.rstrip("\n").split("\t") for l in open("old_segments.tsv")]
old_names = dict(l.rstrip("\n").split("\t") for l in open("old_speakers.tsv"))
votes = defaultdict(Counter)
for s, e, spk, *_ in old:
    s, e = float(s), float(e)
    d = dominant(s, e)
    if d is not None and spk: votes[d][old_names.get(spk, spk)] += e - s
# Greedy one-to-one: biggest overlaps first; a name can only go to one cluster.
pairs = sorted(((v, c, n) for c, cnt in votes.items() for n, v in cnt.items()), reverse=True)
cluster_name, used = {}, set()
for v, c, n in pairs:
    if c in cluster_name or n in used or n.startswith("Speaker "): continue
    cluster_name[c] = n; used.add(n)

con = sqlite3.connect(DB, timeout=10)
segs = con.execute("select id, start, end from segments where transcript_id=? order by idx", (tid,)).fetchall()
assign, unassigned = {}, 0
for sid, s, e in segs:
    d = dominant(s, e)
    if d is None: unassigned += 1
    else: assign[sid] = d
# Label clusters in order of first appearance: "Speaker N" unless a name carried over.
order = []
for sid, s, e in segs:
    c = assign.get(sid)
    if c is not None and c not in order: order.append(c)
label = {c: f"Speaker {i+1}" for i, c in enumerate(order)}
talk = Counter()
for sid, s, e in segs:
    if sid in assign: talk[assign[sid]] += e - s
print(f"{len(segs)} segments, {len(segs)-unassigned} assigned, {unassigned} left unlabeled")
for c in order:
    print(f"  cluster {c} → {label[c]:10s} name={cluster_name.get(c, '-'):10s} talk={talk[c]/60:.1f} min")
if dry:
    print("dry run; pass --apply to write"); sys.exit()
with con:
    con.execute("BEGIN IMMEDIATE")
    con.execute("update segments set speaker=NULL where transcript_id=?", (tid,))
    for sid, c in assign.items():
        con.execute("update segments set speaker=? where id=?", (label[c], sid))
    con.execute("delete from speakers where transcript_id=?", (tid,))
    for c in order:
        con.execute("insert into speakers (transcript_id, label, display_name) values (?,?,?)", (tid, label[c], cluster_name.get(c, label[c])))
print("applied")
