# Diarization spike: sherpa-onnx vs Gemini labels (Sep 1 Meeting, 93 min, 2026-10-04)

Setup (Python, for evaluation only; production would use the `sherpa-rs` crate, which exposes the same
`Diarize` API):

```bash
python3 -m venv venv && ./venv/bin/pip install sherpa-onnx numpy
# models from the sherpa-onnx GitHub releases (no Hugging Face token needed):
#   speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2   (6 MB, pyannote segmentation 3.0)
#   speaker-recongition-models/3dspeaker_speech_eres2net_base_200k_sv_zh-cn_16k-common.onnx  (39 MB)
ffmpeg -i meeting.m4a -ac 1 -ar 16000 sep1.wav
sqlite3 -separator $'\t' library.sqlite "select start, end, coalesce(speaker,''), text from segments where transcript_id='…' order by start" > gemini.tsv
./venv/bin/python run.py <embedding.onnx> <num_speakers|-1> <threshold> out.json   # diarize + compare to Gemini
./venv/bin/python sep.py <embedding.onnx>     # embed Gemini's own segments: are Speaker 1/2 separable?
./venv/bin/python sir.py out.json             # "sir/po" address-word rate per cluster (ground-truth proxy)
```

## Findings

- **Speed**: 93 min of audio in 7.5–8.5 min on an M5 Pro (11–13× realtime), CPU only, 6–8 threads.
  Fine for a background job, too slow for the foreground. Not yet profiled whether segmentation or
  embedding dominates; int8 segmentation made no measurable difference.
- **Gemini's speaker labels on this transcript are close to random between the two main voices.**
  Proxy: the presenter addresses the boss as "sir"/"po". Gemini Speaker 1 vs 2: 42 % vs 28 % of
  segments contain it. sherpa's two main clusters (count pinned to 4, ERes2Net): 58 % vs 14 %.
  2-means on embeddings of Gemini's own segments: 64 % vs 9 %. Every embedding model tested splits each
  Gemini label roughly in half, and the split is not time-correlated (both clusters present in every
  10-min window), so it's voices, not drift.
- Consequently the raw "agreement with Gemini" numbers (43–49 % with 4 speakers pinned) say more about
  Gemini than about sherpa. There is no human ground truth yet; spot-check candidates are printed by
  `disagree.py` (e.g. 37:26–39:40, 33:24–34:22, 45:09–45:29).
- **Embedding models**: ERes2Net base 200k (zh-cn) gave the cleanest proxy split; CAM++ was clearly worse
  (28 % vs 37 %); wespeaker resnet34_LM and ERes2Net v2 showed the same 2-cluster structure;
  NeMo TitaNet small fails to run under sherpa-onnx 1.13.8 (ONNX broadcast error).
- **Clustering without a speaker count over-splits**: thresholds 0.5 / 0.8 / 0.9 gave 221 / 54 / 61
  speakers. Pinning the count to 4 works. For production either ask for an expected count, or cluster
  turn embeddings ourselves (agglomerative with a tuned distance, merge tiny clusters) instead of
  sherpa's fast clustering.
- `sherpa-rs` 0.6.8 (Oct 2025) wraps this API in Rust with prebuilt or static sherpa-onnx libraries.

## Suggested next step

Prototype in Rust: `diarize` job stage → `turns` table → assign each segment the speaker with the most
overlap → average embedding per speaker for cross-meeting matching in the Identify speakers dialog.
Validate on one meeting with a human listening to the `disagree.py` stretches first.
