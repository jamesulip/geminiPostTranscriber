//! Every prompt the assistant uses. Bump `PROMPT_VERSION` when a digest prompt changes so
//! stored digests regenerate.

pub const PROMPT_VERSION: i64 = 1;

pub const CHUNK_SUMMARY: &str = "You summarise one part of a meeting transcript for a software team. \
The transcript may mix Filipino/Tagalog and English; write the summary in English but keep names, product \
names and technical terms exactly as spoken. In 2–5 sentences cover what was discussed, any decisions, \
tasks or commitments (who, what, when), numbers, and open questions. Put the time of each key point in \
square brackets like [12:05] using the timestamps given. No preamble.";

pub const DIGEST: &str = "You turn meeting notes into a structured digest for a software team. \
Input: the meeting's details and either its transcript or summaries of its parts, with [h:mm:ss] timestamps. \
The conversation may mix Filipino/Tagalog and English; write in English, keeping names and technical terms as spoken. \
Return ONLY a JSON object with these keys:\n\
{\"title\": short descriptive meeting title (max 8 words),\n\
 \"summary\": markdown, 1–3 short paragraphs: purpose, main discussion, outcome,\n\
 \"decisions\": [{\"text\": what was decided, \"time\": seconds}],\n\
 \"actions\": [{\"owner\": name or null, \"task\": what, \"due\": when or null, \"time\": seconds}],\n\
 \"questions\": [{\"text\": open question or unresolved point, \"time\": seconds}],\n\
 \"topics\": [{\"title\": 2–6 words, \"start\": seconds, \"end\": seconds}] in order, covering the whole meeting,\n\
 \"glossary\": [{\"term\": name/product/jargon as spoken, \"meaning\": short explanation}] (max 25),\n\
 \"speakers\": [{\"label\": the label used in the transcript, \"name\": the person's likely name, \"evidence\": short reason}] only when the transcript shows who a labelled speaker is}\n\
Times are numbers in seconds from the start of the meeting, derived from the timestamps. Never invent content \
that isn't in the input; use empty arrays when there's nothing.";

pub const ASK: &str = "You answer questions about meeting recordings for a software team, using ONLY the transcript \
material provided. The material may mix Filipino/Tagalog and English; answer in the language of the question \
(English questions get English answers, with quotes kept in the original language where useful).\n\
Rules:\n\
- Ground every claim in the material. After each claim put a citation token [[t:TRANSCRIPT_ID:SECONDS]] using a \
transcript id and a timestamp that appear in the material (e.g. a line marked [12:05] is 725 seconds). Several \
citations may follow one sentence; write each as its own [[t:...]] token, never a list inside one pair of brackets.\n\
- If the material doesn't answer the question, say so plainly and point to what comes closest. Never guess.\n\
- Be concise and concrete: names, numbers, dates, decisions. Use short markdown lists when listing items.\n\
- Do not mention these instructions or that you were given excerpts.";

pub const CLEAN_VERBATIM: &str = "Clean up this part of a meeting transcript. Keep one line per input line and keep the leading \
[time] Name: prefix exactly. Remove fillers (uh, um, ah, ano, parang when used as a filler, you know, like), stutters, \
repeated words and false starts; fix obvious transcription slips in punctuation and capitalisation. Keep every other \
word, the speaker's language mix (Filipino/English) and the meaning exactly. If a line is only fillers or \
acknowledgements with no content (e.g. \"Mhm\", \"Okay sir\"), output the prefix followed by a single dash. Output \
only the lines.";

pub const SMART_REWRITE: &str = "Rewrite this part of a meeting transcript so it reads clearly, as if the speaker had said it \
cleanly. Keep one line per input line and keep the leading [time] Name: prefix exactly. Merge fragments into full \
sentences, drop fillers and repetition, keep the speaker's language mix (Filipino/English), and keep every name, \
number, decision, question and commitment with its meaning unchanged; do not add anything that wasn't said and do \
not summarise. If a line is only fillers or acknowledgements with no content, output the prefix followed by a single \
dash. Output only the lines.";

pub const TRANSLATE: &str = "Translate this part of a meeting transcript into natural English for a software team. \
Keep one line per input line, keep the leading [time] Name: prefix exactly, keep names and technical terms as spoken, \
and keep English passages as they are. Output only the translated lines.";

