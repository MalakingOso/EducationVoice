"""Article-to-Podcast: Convert articles into multi-host podcast audio."""

import argparse
import html
import json
import os
import re
import sys
from pathlib import Path

import anyio
from claude_agent_sdk import (
    query, ClaudeAgentOptions, AgentDefinition, ResultMessage,
)
from claude_agent_sdk.types import AssistantMessage, TextBlock, ToolUseBlock
import requests
from bs4 import BeautifulSoup

# Claude model used for script generation.  Sonnet over Opus here on a
# side-by-side read of real output: Opus writes more densely and more
# stiffly for spoken dialogue.  Override per-run with --model.
SCRIPT_MODEL = "claude-sonnet-5"

# Claude model used for the creative-director edit pass that follows script
# generation.  Deliberately the opposite choice from SCRIPT_MODEL's rationale
# above: that finding was about *generating* flowing spoken dialogue, and
# this pass is a different task — critical, discerning judgment on an
# existing draft, which is where Opus's density earns its keep instead of
# hurting.  Override per-run with --edit-model.
EDIT_MODEL = "claude-opus-5"

# Claude model for the researcher, the sub-agent the writer sends into the
# literature.  A sweep is a dozen small tool calls and a report, which is
# retrieval and summary rather than reasoning; the writer reads the report
# with whatever depth its own model has.  Override per-run with
# --research-model.
RESEARCH_MODEL = "claude-sonnet-5"

# The model that answers "what is this article called?".  Deliberately the
# cheapest one: naming a row in a library is not a reasoning problem, and this
# call is pure overhead on every run that makes it.
TITLE_MODEL = "claude-haiku-4-5-20251001"

# ---------------------------------------------------------------------------
# Progress protocol
# ---------------------------------------------------------------------------

# With --progress-json, stdout carries one JSON object per line and nothing
# else, so a caller can parse it a line at a time; every human-readable
# message stays on stderr where it has always been.  Off by default, which is
# what keeps the CLI byte-identical for anyone running it by hand.
_EMIT = False


def emit(event: str, **fields) -> None:
    """Write one progress event to stdout.  No-op unless --progress-json."""
    if not _EMIT:
        return
    print(json.dumps({"event": event, **fields}), flush=True)


def die(message: str, code: int = 1) -> None:
    """Print a fatal message to stderr, emit a matching error event, exit.

    Every fatal path goes through here rather than a bare print+sys.exit.  A
    caller watching the event stream cannot see a SystemExit — without an
    error event it just observes the process vanish, with nothing to show for
    it.  `message` is printed verbatim so stderr stays exactly as it was.
    """
    print(message, file=sys.stderr)
    # The event carries the bare reason; "Error: " is stderr's formatting,
    # not part of the message.
    emit("error", text=message.removeprefix("Error: "))
    sys.exit(code)


class _JsonTqdm:
    """Stand-in for tqdm that reports diffusion steps as progress events.

    VibeVoice builds its bar as `tqdm(range(max_steps), desc=..., leave=...)`
    and then only iterates it and calls `set_description`
    (modeling_vibevoice_inference.py:426-462).  Matching that surface is the
    whole contract — every other method tqdm offers goes unused, so
    implementing them would be dead code.

    `max_steps` is known before the loop starts, which is what makes an
    honest percentage possible here and impossible during script generation.
    """

    def __init__(self, iterable=None, desc=None, total=None, **kwargs):
        self.iterable = [] if iterable is None else iterable
        self.desc = desc
        if total is not None:
            self.total = total
        else:
            try:
                self.total = len(self.iterable)
            except TypeError:
                self.total = None

    def set_description(self, desc=None, refresh=True):
        self.desc = desc

    def __iter__(self):
        # A normal episode runs ~5300 steps.  One event each would be 5300
        # lines of JSON to move a bar that is a few hundred pixels wide, so
        # emit at most ~200 of them.
        every = max(1, (self.total or 200) // 200)
        for i, item in enumerate(self.iterable):
            if i % every == 0:
                emit("progress", stage="tts", step=i, total=self.total)
            yield item


# ---------------------------------------------------------------------------
# Article ingestion
# ---------------------------------------------------------------------------

def ingest_article(source: str) -> tuple[str, bool, str | None]:
    """Read article from URL, file path, or stdin.

    Returns (text_or_path, is_pdf, page_title).  When the source is a PDF file
    we return the *absolute path* so Claude can read it natively — this
    preserves tables, figures, and visual layout that text extraction would
    lose.

    `page_title` is the HTML document's own <title>, and only a URL has one.
    It is free — BeautifulSoup has already parsed the page — and it is the
    first fallback when the model-written title does not come back.
    """
    if source == "-":
        return sys.stdin.read(), False, None

    if source.startswith("http://") or source.startswith("https://"):
        resp = requests.get(source, timeout=30, headers={"User-Agent": "article2pod/1.0"})
        resp.raise_for_status()
        soup = BeautifulSoup(resp.text, "html.parser")
        paragraphs = [p.get_text(strip=True) for p in soup.find_all("p")]
        page_title = clean_title(soup.title.get_text()) if soup.title else None
        return "\n\n".join(p for p in paragraphs if p), False, page_title

    path = Path(source)
    if path.is_file():
        if path.suffix.lower() == ".pdf":
            return str(path.resolve()), True, None
        return path.read_text(encoding="utf-8"), False, None

    return source, False, None


# ---------------------------------------------------------------------------
# Episode titles
# ---------------------------------------------------------------------------

# Long enough for a real journal title with a subtitle, short enough that a
# paragraph of explanation is refused rather than shown as a row label.
MAX_TITLE_CHARS = 250

# How much of the article Haiku is shown.  A title lives in the first page;
# sending 60kB of body text to find it would cost more than the script.
TITLE_EXCERPT_CHARS = 4000

# Turn budgets for the title call, which differ because only one path uses a
# tool.  With the text already in the prompt the answer is a single turn.  A
# PDF has to be Read first, and each Read costs an assistant turn plus a result
# turn — measured against ROTBIGS.pdf, an unbounded "read the PDF" prompt spent
# 6 and a "read only the first page" prompt spent 4.  Eight is that measurement
# plus headroom for a title page that spills.  Overrunning is not a crash: the
# SDK raises and fetch_title degrades to the fallback chain — which is exactly
# what a max_turns of 2 did on every PDF before this was measured.
TEXT_TITLE_TURNS = 2
PDF_TITLE_TURNS = 8

# The SDK reads the CLI's stdout one JSON message at a time and refuses any
# message over this many bytes (its default is 1 MiB).  Claude reads the PDF
# itself, so a Read result carries the whole file inline; a 600 KB paper
# already blows the default.  64 MiB covers anything we'd sensibly hand it.
SDK_MAX_BUFFER_BYTES = 64 * 1024 * 1024

_SPEAKER_LINE = re.compile(r"^Speaker \d+\s*:", re.I)


def clean_title(raw: str | None) -> str | None:
    """Normalise a candidate title, or return None if it is not one.

    Everything that reaches here is untrusted: a model asked for a title
    sometimes answers with a sentence about the title, and an HTML <title> is
    whatever the site's template produced.  A bad answer must be discarded
    rather than shown, because it becomes a filename-shaped label on a row
    that already has a perfectly good fallback.
    """
    if not raw:
        return None

    text = raw.strip()
    # Models routinely wrap the answer, or restate the question first.
    for prefix in ("title:", "Title:", "TITLE:"):
        if text.startswith(prefix):
            text = text[len(prefix):].strip()
    text = text.strip('"').strip("'").strip()
    # Collapse the whitespace an HTML <title> carries from its indentation.
    text = " ".join(text.split())

    if not text:
        return None
    if len(text) > MAX_TITLE_CHARS:
        # An answer this long is prose about the article, not its name.
        return None
    if _SPEAKER_LINE.match(text):
        # The script leaking into the title slot. Never a name.
        return None
    return text


def fetch_title(article: str, is_pdf: bool, model: str = TITLE_MODEL) -> str | None:
    """Ask Haiku for the article's title, or None if it cannot say.

    Deliberately a separate call from generate_script.  The script prompt is
    tuned for spoken voice; adding "and also tell me the title" to it puts the
    prose at risk to obtain a library label, and the two would then have to be
    revised together forever.  Cheapest model, and Read allowed only so a PDF
    can be opened.

    Never raises.  A title is a nicety — the row has three fallbacks behind
    this one — so an API failure here must not take down a run that is
    otherwise about to spend three minutes and real tokens.
    """
    if is_pdf:
        # "Only the first page" is doing real work here, not politeness: left
        # unbounded the model pages through the whole document looking for a
        # better answer than the one already printed on page 1.
        prompt = (
            f"Read only the FIRST page of the PDF at this path — the title is "
            f"printed there — and reply with that title and nothing else. No "
            f"preamble, no quotes, no explanation. Do not read further pages. "
            f"If you cannot tell, reply with the single word UNKNOWN."
            f"\n\n{article}"
        )
        tools = ["Read"]
        max_turns = PDF_TITLE_TURNS
    else:
        prompt = (
            f"Reply with this article's title and nothing else. No preamble, "
            f"no quotes, no explanation. If you cannot tell, reply with the "
            f"single word UNKNOWN.\n\n{article[:TITLE_EXCERPT_CHARS]}"
        )
        tools = []
        max_turns = TEXT_TITLE_TURNS

    answer = ""

    async def _ask():
        nonlocal answer
        async for message in query(
            prompt=prompt,
            options=ClaudeAgentOptions(
                model=model,
                max_turns=max_turns,
                allowed_tools=tools,
                disallowed_tools=["Write", "Edit", "Bash", "NotebookEdit"],
                permission_mode="bypassPermissions",
                max_buffer_size=SDK_MAX_BUFFER_BYTES,
                # Left unset, the SDK omits --setting-sources and the `claude`
                # child falls back to its own default of loading every
                # settings.json (user, project, local) — MCP servers and all.
                # On a machine with a dozen MCP plugins configured globally,
                # that turned a one-turn Haiku call into a 50-second one, all
                # spent starting servers this call allows no tools to reach.
                setting_sources=[],
            ),
        ):
            if isinstance(message, AssistantMessage):
                parts = [
                    block.text for block in message.content
                    if isinstance(block, TextBlock)
                ]
                if parts:
                    answer = "\n".join(parts)

    try:
        anyio.run(_ask)
    except Exception as e:  # noqa: BLE001 — see the docstring
        print(f"Could not read the article's title: {e}", file=sys.stderr)
        return None

    if answer.strip().upper() == "UNKNOWN":
        return None
    return clean_title(answer)


# ---------------------------------------------------------------------------
# Episode descriptions
# ---------------------------------------------------------------------------

# The model that writes the Spotify show-notes blurb.  Same call shape and
# same reasoning as TITLE_MODEL: summarising a script that is already written
# is not a reasoning problem, and this one runs while the user waits on an
# upload, so it is the latency that matters rather than the depth.
DESCRIPTION_MODEL = "claude-haiku-4-5-20251001"

# Roughly what Spotify shows before it collapses the rest behind "more".  A
# blurb that has to be expanded has already lost the person it was written
# for, so paragraphs past this point are dropped rather than shown.
MAX_DESCRIPTION_CHARS = 1200

# How much of the script the model is shown.  The longest this app has
# produced is ~36 kB, so in practice this truncates nothing — it exists so a
# pathological script cannot turn a cheap call into an expensive one.
DESCRIPTION_EXCERPT_CHARS = 80_000

# No tools, and the whole script arrives in the prompt, so the answer is one
# assistant turn.  Two for the same headroom TEXT_TITLE_TURNS carries.
DESCRIPTION_TURNS = 2

# Asks for plain paragraphs, not HTML.  The show-notes panel wants a single
# line of <p> blocks with no <br> and no literal newlines, and a model asked
# for HTML returns markdown fences, stray tags and the occasional invented
# <a href>.  Getting prose and assembling the markup in clean_description
# makes the format hold by construction.
DESCRIPTION_PROMPT = """\
Below is the full script of a podcast episode. Write the show-notes blurb \
that sits under it on Spotify.

Two or three short paragraphs, separated by a blank line. Open with the most \
interesting thing the episode actually lands on: the first sentence is all \
most people ever see, so do not spend it on "in this episode" \
throat-clearing. Then say what ground the conversation covers.

Write it for someone deciding whether to press play, in the register of a \
person describing something they found interesting. No marketing voice, no \
rhetorical questions, no "dive into" or "explore". Do not use em dashes.

Describe only what is in the script. Do not add links, URLs, citations, \
timestamps, hashtags or a sign-off, and do not name the hosts. Reply with the \
blurb and nothing else: no preamble, no heading, no quotes, no markdown, no \
HTML. If the script is too garbled to describe, reply with the single word \
UNKNOWN.

The episode is titled: {title}

---

{script}
"""

_CODE_FENCE = re.compile(r"^```[a-z]*\n|\n```$", re.I)
_HTML_TAG = re.compile(r"<[^>]*>")
_DESCRIPTION_LABEL = re.compile(
    r"^(?:episode\s+)?(?:description|blurb|show ?notes)\s*:\s*", re.I
)


def clean_description(raw: str | None) -> str | None:
    """Turn a model's answer into the one line of HTML Spotify wants.

    Everything reaching here is untrusted in the same way `clean_title`'s
    input is, with a sharper consequence: episode metadata is immutable once
    uploaded, so a bad description cannot be edited afterwards — only deleted
    and re-uploaded.  That is the whole argument for refusing a doubtful
    answer instead of sending it.

    The output contract comes from the save-to-spotify skill's
    episode-description reference: one line, every paragraph in its own
    <p>...</p>, no <br> (it renders as literal text on the desktop app), and
    hyphens rather than em dashes.
    """
    if not raw:
        return None

    text = _CODE_FENCE.sub("", raw.strip()).strip()
    text = _DESCRIPTION_LABEL.sub("", text)

    if not text or text.strip().upper() == "UNKNOWN":
        return None
    if _SPEAKER_LINE.search(text):
        # The script leaking into the blurb slot, the same trade clean_title
        # refuses in the other direction.
        return None

    kept: list[str] = []
    total = 0
    for para in re.split(r"\n\s*\n", text):
        # Tags first, then escape: a stray "<" in the prose survives as &lt;
        # rather than being eaten as the start of a tag.
        para = _HTML_TAG.sub("", para)
        para = " ".join(para.split())
        para = para.replace("—", "-").replace("–", "-")
        if not para:
            continue
        wrapped = f"<p>{html.escape(para, quote=False)}</p>"
        if total + len(wrapped) > MAX_DESCRIPTION_CHARS:
            # Stop at a paragraph boundary.  A blurb cut mid-sentence reads
            # worse than a shorter one, and a first paragraph that already
            # overruns is a runaway answer rather than a description.
            break
        kept.append(wrapped)
        total += len(wrapped)

    return "".join(kept) if kept else None


def fetch_description(
    script: str, title: str, model: str = DESCRIPTION_MODEL
) -> str | None:
    """Ask Haiku for a Spotify blurb for `script`, or None if it cannot write one.

    Never raises, for the reason fetch_title does not: this runs inside a send
    the user is watching, and an episode that arrives without a description is
    a much better outcome than a send that dies before the upload.
    """
    prompt = DESCRIPTION_PROMPT.format(
        title=title or "Untitled",
        script=script[:DESCRIPTION_EXCERPT_CHARS],
    )

    answer = ""

    async def _ask():
        nonlocal answer
        async for message in query(
            prompt=prompt,
            options=ClaudeAgentOptions(
                model=model,
                max_turns=DESCRIPTION_TURNS,
                allowed_tools=[],
                disallowed_tools=["Write", "Edit", "Bash", "NotebookEdit"],
                permission_mode="bypassPermissions",
                max_buffer_size=SDK_MAX_BUFFER_BYTES,
                # See the matching comment on fetch_title's ClaudeAgentOptions
                # -- this is what took a Spotify send from a ~4s description
                # call to a ~50s one.
                setting_sources=[],
            ),
        ):
            if isinstance(message, AssistantMessage):
                parts = [
                    block.text for block in message.content
                    if isinstance(block, TextBlock)
                ]
                if parts:
                    answer = "\n".join(parts)

    try:
        anyio.run(_ask)
    except Exception as e:  # noqa: BLE001 — see the docstring
        print(f"Could not write an episode description: {e}", file=sys.stderr)
        return None

    return clean_description(answer)


# ---------------------------------------------------------------------------
# Script generation via Claude
# ---------------------------------------------------------------------------

# What the researcher may call.  The writer's own lookup set minus the
# library-docs tools, which have nothing to say about a clinical paper, and
# minus Agent, so a researcher never spawns a researcher.
RESEARCHER_TOOLS = [
    "Read",
    "WebSearch",
    "WebFetch",
    "mcp__claude_ai_PubMed__search_articles",
    "mcp__claude_ai_PubMed__get_full_text_article",
    "mcp__claude_ai_PubMed__get_article_metadata",
    "mcp__claude_ai_PubMed__find_related_articles",
]
RESEARCHER_MAX_TURNS = 25

# The researcher's standing orders.  The writer dictates the shape of each
# answer in its request; this is only what holds regardless of the request.
# `{paper_access}` is one of the two sentences below, chosen by whether the
# paper is a file the researcher could open.
RESEARCHER_PROMPT = """\
You are the research desk for a writer turning a medical paper into a \
podcast script for surgical fellows. The writer has read the paper and \
sends you questions about the literature around it; you go and look, and \
you come back with what they asked for.

- Answer in the form the request asks for. If it names none, keep it \
tight: short sections, one line per source.
- Every claim carries its source: first author and year, plus a PMID, DOI \
or URL. A claim you could not verify is listed as unverified rather than \
dropped or dressed up.
- Search wide before you answer: several PubMed queries with different \
terms, author names and related conditions; WebSearch for guideline \
updates and recent developments; the full text when an abstract is not \
enough.
- Work from the request. The writer has already read the paper and tells \
you what it says, so you do not read it again to get started. {paper_access}
- Report and stop. No advice on the episode, no dialogue."""

PAPER_ACCESS_PDF = (
    "If a request is too thin to act on, the paper is at {path}; read the "
    "part you need and no more."
)
PAPER_ACCESS_TEXT = (
    "You have no copy of the paper. If a request is too thin to act on, "
    "say so at the top of your report and answer what you can."
)

SYSTEM_PROMPT = """\
You write podcast scripts from medical articles for surgical fellows. \
Peer-to-peer: assume full command of medical terminology, anatomy, \
pharmacology, and statistics, and do not simplify anything a surgical \
trainee already knows. The audience is never mentioned in the script.

HOW THE HOSTS TALK:
All {num_hosts} speakers are experts and there is no lead. They talk the \
way colleagues talk when nobody else is listening: loose, specific, \
unrehearsed, sure of themselves. A claim is stated and left standing. \
Disagreement is flat and immediate, the opposing thing said outright and \
the conversation moving on. A reaction is to the substance of what was \
just said. A point lands on its own, with nothing announcing it and \
nothing wrapping it up. A turn ends where the thought ends: people trail \
off, hand over mid-sentence, or just stop. Em dashes are welcome; they \
mark the breaks and pick-ups of real speech and help the reader hear the \
flow.

Two things this voice never does, because they are the fingerprint of \
generated dialogue:
- Defining something by what it is not ("X, not Y", "it isn't A, it's \
B", "not just X but Y"). State the claim and stop.
- Hedging into a disagreement ("I'd push back", "to be fair", "that \
said"). Say the opposing thing directly.

RHYTHM:
- The content sets the pace. A turn runs as long as the thought needs: a \
few words when someone is reacting, a full paragraph when a mechanism or \
a study design needs unpacking. Neither is rationed.
- Real conversation is lopsided and uneven. The one shape to avoid is two \
people alternating speeches of similar length.
- Interruptions are real: half-sentence reactions, corrections that land \
before the other host finishes, someone picking up a thread from two \
turns back.
- A turn opens by reacting to what was just said rather than starting a \
new topic cold. Disagreements sit unresolved when they would in life.
- Tone: {tone}.
{length_guidance}

RESEARCH (before anything else):
- Read the paper. Then send the researcher agent on one broad sweep of \
the literature around it: the trials it builds on or contradicts, the \
guideline position, the numbers everyone quotes, where honest experts \
still disagree, what it leaves open. This is not optional. The \
researcher has not read the paper, so your request carries what it \
needs: the question, the design, the population, the headline result, \
and the form you want the answer in.
- Send the researcher again whenever a question would take more than a \
couple of lookups; several requests at once is fine. For one specific \
thing you are unsure of while writing, look it up yourself.
- The research is for you, the writer. It is what lets the hosts sound \
like people who already know this literature. Most of it is never said \
aloud.
- Name a study only when it changes how you read THIS paper: it \
contradicts the result, it is the trial this one aims to displace, it \
explains a design choice, or it is the guideline this would change. Two \
or three named sources in an episode is normal, and a section with no \
external reference is fine.
- A named source is first author plus year, or the trial name. Vague \
attribution ("some studies show", "the literature suggests") never \
appears.

PLAN THE EPISODE (after research, before any dialogue):
Work this through out loud, in a message of its own. It is thinking, not \
output. The listener never sees it, so be blunt and be willing to throw \
things away.

1. BRAINSTORM WIDELY. Put up five to eight genuinely different angles \
the episode could take. Cast a wide net: the argument the paper is \
making; the methodological weak point that undercuts it; the single \
clinical decision that changes on Monday; the history of how the \
current practice got established and why it stuck; the number that \
contradicts what everyone does; the question the paper conspicuously \
fails to answer; the disagreement two honest experts would still have \
after reading it. These should be real alternatives that would produce \
noticeably different episodes, never one idea phrased five ways. Do not \
settle on anything yet.

2. REFLECT ON EACH, HONESTLY. For every angle, say what makes it \
compelling and where it falls apart. Is there enough in the paper to \
sustain it for a whole episode, or does it run dry after four minutes? \
Does it need numbers the paper does not report? Would it require the \
hosts to explain background that is more boring than the payoff? Is it \
interesting to a surgical fellow, or only to a methodologist? Name the \
weaknesses plainly. An angle you talk yourself into is the one that \
produces a flat episode.

3. NARROW. Choose the spine of the episode and say why it beat the \
others. Then name the two or three moments that have to land: the \
opening hook, the point where the argument turns, the thing a listener \
should still remember tomorrow. Note anything from the discarded angles \
worth keeping as a beat along the way.

WRITE:
- Study every table and figure first, then report only what changes a \
decision: the effect big enough to act on, the number that surprises, \
the subgroup where the answer flips, the confidence interval that \
undercuts the headline. A table is never walked row by row. If a \
figure's message is one sentence, say the sentence and move on.
- Every statistic gets a plain-language clinical reading close behind \
it: what the number means for the patient in front of you, or for the \
decision the surgeon makes. Never more than two numbers in a row \
without one.

FORMAT:
- Your final message is the script and nothing else: no plan, no \
headings, no commentary. Do not use the Write or Edit tools.
- Every line: Speaker N: dialogue text, with N from 1 to {num_hosts}.
- No stage directions, sound effects, or [brackets]."""


# Default: no word count, no duration, no turn count.  The episode runs as
# long as the article has substance to fill and stops there.  Word budgets
# were tried and made the writing worse — the model pads a thin section or
# rushes a dense one to hit the number.
LENGTH_BY_DENSITY = """\
- Length: let the article decide. A dense paper with fifty procedure \
estimates and a real methodological weakness earns a long episode; a \
thin one earns a short one. There is no target duration and no word \
count. Cover what is worth covering at the pace it deserves, and stop \
when you are done rather than filling to a quota.
- The target is a calm, unhurried listen, the kind of conversation \
someone can follow on a commute and come away understanding the paper. \
A point is never rushed to save time and never stretched to fill it."""


# The editor is the negative half of the pair.  The writer prompt describes
# what good dialogue sounds like and carries only the two bans proven to
# work at draft stage; every other tell is named here, as a sentence shape
# rather than a word list, because a banned word gets replaced by a synonym
# and a banned shape actually changes the rhythm.
EDITOR_SYSTEM_PROMPT = """\
You're the creative director for this podcast, brought in after a writer \
has already produced a full draft. Nobody hired you to check spelling. \
Your job is the one thing a director does that nobody else does: listen \
to a scene and know, in your gut, whether it plays like two surgeons \
talking or like a script being performed as two surgeons. You have a \
trained ear for exactly where the artifice shows.

THE TELLS, in order of how badly they break the illusion:

1. Defining by negation. A line that says what something ISN'T on the \
way to saying what it is: "X, not Y", "it isn't A, it's B", "not just X \
but Y", "the real question isn't X, it's Y". This construction barely \
occurs in speech; it is a model hedging toward precision instead of \
committing to a claim. Kill the negated half and keep the assertion.

2. Rehearsed disagreement. A hedge or a throat-clear in front of an \
objection: "I'd push back on that", "to be fair", "that said", "where \
I'd differ is". Real disagreement is flat: "That's overselling it." "I \
don't buy it." "The data doesn't support that." Cut the wind-up, keep \
the objection.

3. The wind-up before a point. A phrase whose only job is to announce \
that something important is coming: "here's the thing", "that's the \
part that matters", "what gets me is", "which is exactly why". The point \
lands harder without it. Delete the announcement and start on the point.

4. Hosts reviewing each other. A reaction aimed at how well the other \
host put something rather than at what they said: "good catch", "that's \
a real reframe", "that's the honest way to say it", "that's the whole \
paper in one sentence". Colleagues respond to the substance or they \
don't respond at all. Replace the line with a reaction that carries \
content, or cut it.

ALSO: affirmation openers ("Certainly", "Great question"), AI framing \
("it's important to note", "in summary"), remarks about the topic being \
complex, bullet-list cadence dressed as sentences, turns that end on a \
tidy rhetorical bow, and any word that shows up in generated text and \
never in a conversation between two surgeons. For that last one a \
synonym swap just moves the seam; rewrite the sentence the way this \
person would actually say the thing underneath it.

HOW TO WORK:
Read the whole script once, straight through, the way a listener would, \
before touching anything. Notice every place the illusion breaks. Then \
go back and fix those places. A line that already works doesn't need \
your fingerprints on it, and em dashes are part of the voice, so leave \
them.

Rewrite in full sentences, in voice, never as a word-for-word \
substitution. A mechanical swap is exactly as detectable as what it \
replaced, with a different word in the slot.

YOUR LICENCE:
- Cut freely. A turn that carries nothing can go, or fold into the next \
one. A substantive turn that repeats itself, or performs instead of \
talks, can be trimmed.
- Write freely. New reactions, connective tissue, and rephrasings are \
yours to add so the cut edges join up in voice.
- Cut for the conversation, never for the clock. There is no target \
length. The episode should be a calm, unhurried listen that someone can \
follow on a commute and come away understanding the paper. Remove what \
is dead, redundant, or performed; leave what is working at its length, \
even when it is long.

WHAT STAYS FIXED:
- Every fact, number, citation, study name, and clinical claim stays \
exactly as reported, and you introduce none that are not in the draft. \
You are editing performance, not content.
- The order of the argument. Beats stay where the writer put them.
- Exactly {num_hosts} speakers and the Speaker N: format.

Work through the script in a message of its own first, noting what \
you're fixing and why. That is for the record, not the listener. Your \
final message must contain only the corrected script: Speaker N: lines, \
nothing else."""


# Human wording for the `phase` events the script stage emits, keyed by the
# wire name.  The GUI carries its own copy of these labels; the stderr line
# uses this one.
PHASE_LABELS = {
    "researching": "Researching the literature",
    "writing": "Writing the script",
    "editing": "Editing the script",
}

# A tool-free prose message at least this long is taken to be the writer's
# plan rather than a passing narration.  Plans run several thousand
# characters; narration is a sentence.
PLAN_MIN_CHARS = 1500


def _run_pass(
    system: str,
    user_msg: str,
    model: str,
    allowed_tools: list[str],
    disallowed_tools: list[str],
    max_turns: int,
    phase: str,
    writing_phase: str | None = None,
    effort: str | None = None,
    agents: dict[str, AgentDefinition] | None = None,
) -> str:
    """Run one query() to completion and return its selected raw text.

    `agents`, if given, are sub-agents the model may delegate to through the
    Agent tool (which must be in `allowed_tools` for that to happen).  Their
    own turns arrive in the same stream, tagged with the tool use that
    spawned them, and are skipped here: a researcher's report is a long
    tool-free prose message, which is exactly the shape the plan detection
    below keys on, and its narration is not the writer's.

    `phase` names what the pass is doing when it starts and whenever it is
    calling tools ("researching" for the writer).  `writing_phase`, if given,
    is announced once a substantial tool-free prose message arrives: for the
    writer that is the brainstorm/reflect/narrow plan, and the script is
    what comes next.  The editor has no such turn, so it stays in one phase.

    Shared by the writer and editor passes in generate_script.  Selection is
    by *shape* rather than length: both passes think out loud in a message of
    its own before producing the script (brainstorm/reflect/narrow for the
    writer, "here's what I'm fixing" for the editor), and that reasoning
    prose can easily be longer than the script itself.  Only messages that
    actually contain dialogue lines count, and the last such message wins —
    that is the finished draft, after any revision.
    """
    script_re = re.compile(r"^Speaker \d+:", re.M)
    candidates: list[str] = []
    longest_text = ""
    current_phase: str | None = None

    def set_phase(name: str) -> None:
        nonlocal current_phase
        if name == current_phase:
            return
        current_phase = name
        print(f"{PHASE_LABELS[name]}...", file=sys.stderr)
        emit("phase", stage="script", phase=name)

    set_phase(phase)

    async def _run():
        nonlocal longest_text
        async for message in query(
            prompt=user_msg,
            options=ClaudeAgentOptions(
                system_prompt=system,
                model=model,
                max_turns=max_turns,
                allowed_tools=allowed_tools,
                disallowed_tools=disallowed_tools,
                permission_mode="bypassPermissions",
                effort=effort,
                max_buffer_size=SDK_MAX_BUFFER_BYTES,
                agents=agents,
            ),
        ):
            if isinstance(message, AssistantMessage):
                if message.parent_tool_use_id is not None:
                    continue
                text_parts = [
                    block.text for block in message.content
                    if isinstance(block, TextBlock)
                ]
                uses_tools = any(
                    isinstance(block, ToolUseBlock) for block in message.content
                )
                if uses_tools:
                    # Any tool call means the model is back in the literature,
                    # whatever prose preceded it.
                    set_phase(phase)
                if text_parts:
                    candidate = "\n".join(text_parts)
                    # The plan is the one long, tool-free prose message; the
                    # script follows it.  The length floor is what stops a
                    # one-line "let me search for..." narration (the CLI
                    # streams text and tool blocks as separate messages)
                    # from flipping the phase early.  A wrong flip corrects
                    # itself on the next tool call anyway.
                    if (
                        writing_phase
                        and not uses_tools
                        and not script_re.search(candidate)
                        and len(candidate) >= PLAN_MIN_CHARS
                    ):
                        set_phase(writing_phase)
                    # Emitted as it arrives, not at the end: this is the only
                    # window into an otherwise silent multi-minute call, and
                    # it is where the brainstorm/reflect/narrow reasoning (or
                    # the editor's own fix commentary) shows.
                    emit("message", text=candidate)
                    if len(script_re.findall(candidate)) >= 2:
                        candidates.append(candidate)
                    if len(candidate) > len(longest_text):
                        longest_text = candidate

    anyio.run(_run)

    if candidates:
        return candidates[-1]
    if longest_text:
        # No message looked like dialogue.  Fall back to the longest text so
        # validate_script can report what actually came back.
        warn = (
            "Warning: no message contained Speaker lines; falling back to the "
            "longest assistant reply"
        )
        print(warn, file=sys.stderr)
        emit("warning", text=warn.removeprefix("Warning: "))
        return longest_text
    die("Error: no script was generated")


def generate_script(
    article_text: str,
    num_hosts: int = 2,
    tone: str = "conversational and engaging",
    target_length: str | None = None,
    is_pdf: bool = False,
    model: str = SCRIPT_MODEL,
    edit_model: str = EDIT_MODEL,
    draft_out: Path | None = None,
    research_model: str = RESEARCH_MODEL,
) -> str:
    """Generate a podcast script from article text using Claude Agent SDK.

    Two sequential calls: a writer pass that researches and drafts, then a
    fresh creative-director pass that edits the draft for AI-sounding tells.
    The writer does not do the broad literature sweep itself: it delegates
    that to a researcher sub-agent on `research_model`, which keeps a dozen
    search results out of the writer's context and lets a cheaper model do
    the retrieval.  The writer keeps its own lookup tools for the one
    specific question it hits mid-write.
    Fresh eyes matter here — the model that just wrote a stilted line is poor
    at noticing its own tic, so the editor is a separate model instance with
    no memory of drafting it.  `draft_out`, if given, gets the writer's
    pre-edit output so the two can be diffed while tuning the editor prompt.
    """
    if target_length:
        length_guidance = (
            f"- Length: roughly {target_length}, but do not pad or rush to "
            f"hit it — the article's density matters more than the clock."
        )
    else:
        length_guidance = LENGTH_BY_DENSITY

    system = SYSTEM_PROMPT.format(
        num_hosts=num_hosts,
        tone=tone,
        length_guidance=length_guidance,
    )

    # One sentence, deliberately.  The process (research, plan, write) is
    # specified once, in SYSTEM_PROMPT; this is a reminder in the user turn
    # that it applies, and it must not restate the steps in different words
    # or the two drift apart.
    research_instruction = (
        "Research the literature and plan the episode before you write a "
        "line of dialogue; the script is your final message."
    )

    if is_pdf:
        user_msg = (
            f"Read the PDF at this path, then turn it into a podcast script "
            f"with {num_hosts} hosts. {research_instruction}\n\n{article_text}"
        )
    else:
        user_msg = (
            f"Turn this article into a podcast script with {num_hosts} hosts. "
            f"{research_instruction}\n\n{article_text}"
        )

    print("Generating podcast script with Claude...", file=sys.stderr)
    emit(
        "stage", stage="script", status="start",
        model=model, research_model=research_model,
    )

    paper_access = (
        PAPER_ACCESS_PDF.format(path=article_text) if is_pdf
        else PAPER_ACCESS_TEXT
    )
    researcher = AgentDefinition(
        description=(
            "Searches the medical literature around the paper and reports "
            "back with cited findings, in whatever form the request asks for."
        ),
        prompt=RESEARCHER_PROMPT.format(paper_access=paper_access),
        tools=RESEARCHER_TOOLS,
        disallowedTools=["Write", "Edit", "Bash", "NotebookEdit", "Agent"],
        model=research_model,
        maxTurns=RESEARCHER_MAX_TURNS,
        effort="high",
    )

    # Allow enough turns for: reading PDF (multiple pages) + researcher
    # delegations + a few direct lookups + brainstorm/reflect/narrow +
    # script generation.  A delegation costs one turn however many
    # searches the researcher runs.
    draft_raw = _run_pass(
        system=system,
        user_msg=user_msg,
        model=model,
        agents={"researcher": researcher},
        allowed_tools=[
            "Agent",
            "Read",
            "WebSearch",
            "WebFetch",
            "mcp__claude_ai_PubMed__search_articles",
            "mcp__claude_ai_PubMed__get_full_text_article",
            "mcp__claude_ai_PubMed__get_article_metadata",
            "mcp__claude_ai_PubMed__find_related_articles",
            "mcp__plugin_context7_context7__resolve-library-id",
            "mcp__plugin_context7_context7__query-docs",
        ],
        disallowed_tools=["Write", "Edit", "Bash", "NotebookEdit"],
        max_turns=30,
        phase="researching",
        writing_phase="writing",
        # Highest reasoning depth for the writer pass — this is the research
        # + brainstorm/reflect/narrow + write call, where more thinking has
        # room to pay off. xhigh is a real effort level on Sonnet 5 (not
        # Opus-only, despite what the SDK's own docstring for this field
        # claims — verified against current API docs).
        effort="xhigh",
    )
    draft = validate_script(draft_raw, num_hosts)

    if draft_out:
        draft_out.write_text(draft, encoding="utf-8")
        print(f"Draft (pre-edit) saved: {draft_out}", file=sys.stderr)

    editor_system = EDITOR_SYSTEM_PROMPT.format(num_hosts=num_hosts)
    # Closed-book text pass — no tools needed, everything it needs is already
    # in the draft.
    edited_raw = _run_pass(
        system=editor_system,
        user_msg=draft,
        model=edit_model,
        allowed_tools=[],
        disallowed_tools=["Write", "Edit", "Bash", "NotebookEdit"],
        max_turns=5,
        phase="editing",
    )

    return validate_script(edited_raw, num_hosts)


def validate_script(script: str, num_hosts: int) -> str:
    """Keep only well-formed Speaker N: lines with in-range speaker ids.

    Speaker ids matter more than they look.  VibeVoice binds voice_samples[0]
    to "Speaker 1", [1] to "Speaker 2", and so on — but its processor
    normalizes ids by subtracting one only when the lowest id in the script
    is greater than zero.  A single stray "Speaker 0:" line therefore
    disables that shift for the *whole* script and moves every voice by one,
    and a stray "Speaker 3:" in a two-host script produces an id with no
    voice prompt at all.  Neither raises; both just come out wrong.
    """
    pattern = re.compile(r"^Speaker (\d+):")
    allowed = set(range(1, num_hosts + 1))
    lines = script.strip().splitlines()
    valid = []
    stripped = 0
    out_of_range = 0
    seen: set[int] = set()

    for line in lines:
        line = line.strip()
        if not line:
            continue
        m = pattern.match(line)
        if not m:
            stripped += 1
            continue
        speaker = int(m.group(1))
        if speaker not in allowed:
            out_of_range += 1
            continue
        seen.add(speaker)
        valid.append(line)

    if stripped:
        print(f"Warning: stripped {stripped} non-script lines", file=sys.stderr)
        emit("warning", text=f"stripped {stripped} non-script lines")
    if out_of_range:
        msg = (
            f"dropped {out_of_range} lines with a speaker id outside "
            f"1-{num_hosts}"
        )
        print(f"Warning: {msg}", file=sys.stderr)
        emit("warning", text=msg)

    if seen != allowed:
        missing = sorted(allowed - seen)
        die(
            f"Error: script does not match --hosts {num_hosts}. Expected "
            f"speakers {sorted(allowed)}, found {sorted(seen)}"
            + (f" (missing {missing})" if missing else "")
        )

    return "\n".join(valid)


# ---------------------------------------------------------------------------
# Audio synthesis via VibeVoice
# ---------------------------------------------------------------------------

PROJECT_ROOT = Path(__file__).resolve().parent
VOICE_DIR = PROJECT_ROOT / "voices"

# The VibeVoice preset voice clips are not shipped by the pip wheel and are
# not in the microsoft/VibeVoice-1.5B model repo either — they survive only
# inside HF Space repos.  Two independent mirrors carry byte-identical
# copies, which is what makes them credible as the authentic upstream files.
# The revision is pinned because a Space can silently re-record a file in
# place, which would change the podcast's voices between runs.
VOICE_REPO = "Steveeeeeeen/VibeVoice-Large"
VOICE_REPO_TYPE = "space"
VOICE_REVISION = "93ece79b2871e703764f1936cfb95f28576579b8"
VOICE_REPO_FALLBACK = "yasserrmd/VibeVoice"

# Short name -> {path, gender, accent}.  en-Alice_woman_bgm.wav is
# deliberately excluded: it has background music baked into the reference
# clip, which bleeds into every utterance conditioned on it.
#
# gender and accent are data rather than end-of-line comments because a
# voice picker has to display and filter on them.  `accent` reflects the
# upstream locale prefix on the filename (en-/in-), which is the only accent
# information the clips actually carry — the five en- voices are not
# labelled any more finely than that.
PRESET_VOICES = {
    "alice":  {"path": "voices/en-Alice_woman.wav",  "gender": "female", "accent": "English"},
    "maya":   {"path": "voices/en-Maya_woman.wav",   "gender": "female", "accent": "English"},
    "frank":  {"path": "voices/en-Frank_man.wav",    "gender": "male",   "accent": "English"},
    "carter": {"path": "voices/en-Carter_man.wav",   "gender": "male",   "accent": "English"},
    "yasser": {"path": "voices/en-Yasser_man.wav",   "gender": "male",   "accent": "English"},
    "samuel": {"path": "voices/in-Samuel_man.wav",   "gender": "male",   "accent": "Indian"},
}

# Index 0 is Speaker 1.  Alternates gender so adjacent speakers stay easy to
# tell apart; only two English female presets exist, which is exactly enough
# to reach four hosts.
DEFAULT_ROSTER = {
    2: ["alice", "carter"],
    3: ["alice", "carter", "maya"],
    4: ["alice", "carter", "maya", "yasser"],
}


def fetch_voice(name: str) -> str:
    """Resolve a preset voice name to a local .wav path, downloading once.

    Checks the repo-local copy before touching the network, so once the
    clips are on disk the upstream Space going away, getting rate-limited,
    or renaming files stops mattering.
    """
    key = name.strip().lower()
    if key not in PRESET_VOICES:
        die(
            f"Error: unknown voice '{name}'. Valid names: "
            f"{', '.join(sorted(PRESET_VOICES))}"
        )

    rel = PRESET_VOICES[key]["path"]
    local = PROJECT_ROOT / rel
    if local.is_file():
        return str(local)

    from huggingface_hub import hf_hub_download

    print(f"Downloading voice '{key}' -> {rel}", file=sys.stderr)
    try:
        # local_dir=PROJECT_ROOT reproduces the repo layout at voices/<file>
        # instead of burying the clips in the opaque HF cache, so they are
        # visible, inspectable, and easy to back up.  Metadata lands in
        # .cache/, which is already gitignored.
        return hf_hub_download(
            repo_id=VOICE_REPO,
            repo_type=VOICE_REPO_TYPE,
            revision=VOICE_REVISION,
            filename=rel,
            local_dir=str(PROJECT_ROOT),
        )
    except Exception as e:
        die(
            f"Error: could not download voice '{key}' ({type(e).__name__}: {e})\n"
            f"  Download it manually from:\n"
            f"    https://huggingface.co/spaces/{VOICE_REPO}/blob/"
            f"{VOICE_REVISION}/{rel}\n"
            f"    (mirror: https://huggingface.co/spaces/{VOICE_REPO_FALLBACK}"
            f"/blob/main/{rel})\n"
            f"  and place it at: {local}\n"
            f"  Or bypass presets entirely with --voice-samples PATH ...,\n"
            f"  or run with --zero-shot to let the model invent voices."
        )


def resolve_voices(
    hosts: int,
    voices: list[str] | None,
    voice_samples: list[str] | None,
    zero_shot: bool,
) -> list[str] | None:
    """Produce the ordered voice-clip list that binds to Speaker 1..N.

    Returns None only for --zero-shot, where VibeVoice samples a speaker
    identity from unconditioned diffusion — an arbitrary voice that can
    drift mid-episode.
    """
    if zero_shot:
        print(
            "Zero-shot mode: voices are sampled by the model and may drift "
            "mid-episode.",
            file=sys.stderr,
        )
        return None

    if voice_samples:
        resolved = []
        for p in voice_samples:
            path = Path(p).expanduser().resolve()
            if not path.is_file():
                die(f"Error: voice sample not found: {path}")
            resolved.append(str(path))
    else:
        resolved = [fetch_voice(n) for n in (voices or DEFAULT_ROSTER[hosts])]

    # VibeVoice slices voice_samples[:len(speakers)] with no validation: too
    # few clips means the extra speakers silently get invented, drifting
    # voices.  No error, no warning.  So check it here.
    if len(resolved) < hosts:
        die(
            f"Error: {len(resolved)} voice(s) given for {hosts} hosts. "
            f"VibeVoice does not validate this — it would silently invent a "
            f"drifting voice for every speaker past the last clip."
        )

    if len(resolved) > hosts:
        msg = (
            f"{len(resolved)} voices given for {hosts} hosts, "
            f"using the first {hosts}"
        )
        print(f"Warning: {msg}", file=sys.stderr)
        emit("warning", text=msg)
        resolved = resolved[:hosts]

    for i, path in enumerate(resolved, start=1):
        print(f"  Speaker {i}: {Path(path).name}", file=sys.stderr)

    return resolved


def synthesize_audio(
    script: str,
    voice_samples: list[str] | None = None,
    output_path: str = "output/podcast.wav",
    tts_model: str = "microsoft/VibeVoice-1.5B",
    cfg_scale: float = 1.3,
) -> None:
    """Synthesize multi-speaker audio from a podcast script."""
    import torch
    from vibevoice.processor.vibevoice_processor import VibeVoiceProcessor
    from vibevoice.modular.modeling_vibevoice_inference import VibeVoiceForConditionalGenerationInference

    # Determine device
    if torch.cuda.is_available():
        device, dtype = "cuda", torch.bfloat16
        attn = "flash_attention_2"
        try:
            import flash_attn  # noqa: F401
        except ImportError:
            attn = "sdpa"
    elif hasattr(torch, "xpu") and torch.xpu.is_available():
        device, dtype, attn = "xpu", torch.bfloat16, "sdpa"
    elif hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
        device, dtype, attn = "mps", torch.float32, "sdpa"
    else:
        device, dtype, attn = "cpu", torch.float32, "sdpa"

    print(f"Loading VibeVoice model on {device} ({dtype})...", file=sys.stderr)
    emit("stage", stage="tts", status="start", device=device)

    processor = VibeVoiceProcessor.from_pretrained(tts_model)
    model = VibeVoiceForConditionalGenerationInference.from_pretrained(
        tts_model, torch_dtype=dtype, device_map=device, attn_implementation=attn
    )

    # Build voice samples dict if provided
    voice_kwargs = {}
    if voice_samples:
        voice_kwargs["voice_samples"] = voice_samples

    # Swap VibeVoice's progress bar for one that speaks JSON.  It binds
    # `tqdm` as a module-level name, so rebinding that one attribute reaches
    # the only call site without touching the real tqdm for anything else.
    # Guarded because this reaches into another package's internals: if an
    # upgrade moves or renames the bar, the run has to lose its percentage,
    # never fail.  The GUI falls back to an elapsed-only display.
    if _EMIT:
        mod = sys.modules.get("vibevoice.modular.modeling_vibevoice_inference")
        if mod is not None and hasattr(mod, "tqdm"):
            mod.tqdm = _JsonTqdm
        else:
            emit(
                "warning",
                text="progress shim unavailable; TTS progress is elapsed-only",
            )

    print("Generating audio...", file=sys.stderr)
    inputs = processor(
        text=[script],
        **voice_kwargs,
        padding=True,
        return_tensors="pt",
        return_attention_mask=True,
    )

    outputs = model.generate(
        **inputs,
        cfg_scale=cfg_scale,
        tokenizer=processor.tokenizer,
        # Generation is capped at max_length_times x the input token count.
        # The default of 2 leaves only ~25% headroom on a typical script,
        # and an overrun merely prints a message rather than raising — so a
        # truncated episode looks like a clean run.  3 buys real margin.
        max_length_times=3,
    )

    # Save audio
    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)

    wav_path = out if out.suffix.lower() == ".wav" else out.with_suffix(".wav")
    processor.save_audio(outputs.speech_outputs[0], output_path=str(wav_path))
    print(f"Saved WAV: {wav_path}", file=sys.stderr)
    final = wav_path

    # Convert to MP3 if requested
    if out.suffix.lower() == ".mp3":
        try:
            from pydub import AudioSegment
            audio = AudioSegment.from_wav(str(wav_path))
            audio.export(str(out), format="mp3", bitrate="192k")
            wav_path.unlink()
            print(f"Converted to MP3: {out}", file=sys.stderr)
            final = out
        except Exception as e:
            print(f"MP3 conversion failed ({e}), keeping WAV", file=sys.stderr)
            emit("warning", text=f"MP3 conversion failed ({e}), keeping WAV")

    # Reports the file that actually exists, which is not args.output when
    # the MP3 conversion was asked for and failed.
    emit("stage", stage="tts", status="done", path=str(final))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Convert articles into multi-host podcast audio"
    )
    parser.add_argument(
        "source", nargs="?",
        help="Article URL, file path, or '-' for stdin",
    )
    parser.add_argument("--hosts", type=int, default=2, choices=[2, 3, 4])
    parser.add_argument("--tone", default="conversational and engaging")
    parser.add_argument(
        "--length", default=None,
        help="Optional length steer. By default the article's density sets it.",
    )
    parser.add_argument("--model", default=SCRIPT_MODEL, help="Claude model")
    parser.add_argument(
        "--edit-model", default=EDIT_MODEL,
        help="Claude model for the creative-director edit pass",
    )
    parser.add_argument(
        "--research-model", default=RESEARCH_MODEL,
        help="Claude model for the researcher sub-agent the writer delegates to",
    )

    voice_group = parser.add_mutually_exclusive_group()
    voice_group.add_argument(
        "--voices", nargs="+", default=None, metavar="NAME",
        help=(
            "Preset voices in speaker order. Choices: "
            + ", ".join(sorted(PRESET_VOICES))
            + ". Default: "
            + "; ".join(
                f"{h} hosts = {' '.join(v)}" for h, v in DEFAULT_ROSTER.items()
            )
        ),
    )
    voice_group.add_argument(
        "--voice-samples", nargs="+", default=None,
        help="Your own WAV/MP3 files for each speaker, in order",
    )
    voice_group.add_argument(
        "--zero-shot", action="store_true",
        help="Let VibeVoice invent voices (they may drift mid-episode)",
    )
    parser.add_argument(
        "--fetch-voices", action="store_true",
        help="Download all preset voice clips to voices/ and exit",
    )
    parser.add_argument(
        "--list-voices", action="store_true",
        help=(
            "Print the preset voices and default rosters as JSON, and exit. "
            "Lets a front end read the roster instead of duplicating it."
        ),
    )
    parser.add_argument("--output", default="output/podcast.wav")
    parser.add_argument(
        "--script-only", action="store_true",
        help="Only generate script, skip TTS",
    )
    parser.add_argument(
        "--from-script", metavar="PATH",
        help="Synthesize an existing script file instead of generating one",
    )
    parser.add_argument("--tts-model", default="microsoft/VibeVoice-1.5B")
    parser.add_argument("--cfg-scale", type=float, default=1.3)
    parser.add_argument(
        "--script-out", metavar="PATH", default=None,
        help=(
            "Where to save the generated script. Default: script.txt beside "
            "--output, which every run overwrites."
        ),
    )
    parser.add_argument(
        "--describe", metavar="SCRIPT",
        help=(
            "Write a Spotify show-notes blurb for an existing script and "
            "print it as JSON. One cheap Claude call and nothing else — no "
            "TTS, no run. Used by the GUI when sending an episode."
        ),
    )
    parser.add_argument(
        "--describe-title", default="",
        help="The title --describe should describe its script under.",
    )
    parser.add_argument(
        "--progress-json", action="store_true",
        help=(
            "Emit machine-readable progress as JSON lines on stdout. Human "
            "output stays on stderr. Used by the GUI."
        ),
    )

    args = parser.parse_args()

    global _EMIT
    _EMIT = args.progress_json

    # Answered before anything else so it stays a cheap, side-effect-free
    # query.  Prints JSON unconditionally: that is this flag's entire output,
    # not progress reporting, so --progress-json has no say in it.
    if args.list_voices:
        print(json.dumps(
            {"voices": PRESET_VOICES, "default_roster": DEFAULT_ROSTER},
            indent=2,
        ))
        return

    if args.fetch_voices:
        for name in sorted(PRESET_VOICES):
            print(fetch_voice(name), file=sys.stderr)
        print(f"Voice clips are in {VOICE_DIR}", file=sys.stderr)
        return

    # Answered alongside the other side-effect-free queries and before the
    # source check, because describing a script that already exists is not a
    # run and needs no source.  Prints JSON unconditionally, as --list-voices
    # does: that is this flag's entire output, not progress reporting.
    if args.describe:
        try:
            script_text = Path(args.describe).expanduser().read_text(encoding="utf-8")
        except OSError as e:
            print(json.dumps({"description": None, "error": str(e)}))
            return
        print(json.dumps({
            "description": fetch_description(script_text, args.describe_title),
        }))
        return

    if not args.source and not args.from_script:
        parser.error(
            "a source is required (URL, file path, or '-' for stdin), "
            "or pass --from-script PATH"
        )

    # Resolve voices before the ~3-minute Claude run, so a bad preset name or
    # an unreachable Space fails in two seconds instead of after the script
    # has been written.
    voice_samples = None
    if not args.script_only:
        voice_samples = resolve_voices(
            args.hosts, args.voices, args.voice_samples, args.zero_shot
        )

    if args.from_script:
        # Reuse a script we already like rather than re-rolling a new one.
        # Still validated, because the speaker-id rules matter just as much
        # for a hand-edited file as for a generated one.
        src = Path(args.from_script).expanduser()
        if not src.is_file():
            die(f"Error: script not found: {src}")
        script = validate_script(src.read_text(encoding="utf-8"), args.hosts)
        print(f"Using script: {src}", file=sys.stderr)
        emit("stage", stage="script", status="done", path=str(src))
        synthesize_audio(
            script,
            voice_samples=voice_samples,
            output_path=args.output,
            tts_model=args.tts_model,
            cfg_scale=args.cfg_scale,
        )
        emit("done", output=args.output)
        # stdout belongs to the event stream when --progress-json is on; the
        # path already travelled as the tts stage's `path` field.
        if not args.progress_json:
            print(args.output)
        return

    # Ingest
    emit("stage", stage="ingest", status="start")
    article, is_pdf, page_title = ingest_article(args.source)
    if is_pdf:
        print(f"PDF: {article} (Claude will read natively)", file=sys.stderr)
        detail = "PDF, read natively"
    else:
        print(f"Article: {len(article)} characters", file=sys.stderr)
        detail = f"{len(article)} characters"

    # Only under --progress-json.  The title exists to name a row in the GUI's
    # library, so a plain CLI run must not gain an API call or its latency —
    # with no new flags this program behaves exactly as it did, which is the
    # regression test.  Emitted inside the ingest stage so it is available on
    # a --script-only run too, which is stage 1 of the GUI's gated flow.
    if _EMIT:
        title = fetch_title(article, is_pdf) or page_title
        if title:
            emit("title", text=title)

    emit("stage", stage="ingest", status="done", detail=detail)

    # Save script.  Without --script-out this is script.txt beside the audio,
    # which every run overwrites — fine for a one-off, useless as a history.
    # The pre-edit draft is written alongside it as script.draft.txt, so the
    # writer-only pass can be diffed against the edited one.
    if args.script_out:
        script_path = Path(args.script_out).expanduser()
    else:
        script_path = Path(args.output).parent / "script.txt"
    script_path.parent.mkdir(parents=True, exist_ok=True)
    draft_path = script_path.with_name(script_path.stem + ".draft" + script_path.suffix)

    # Generate script
    script = generate_script(
        article, args.hosts, args.tone, args.length, is_pdf, args.model,
        args.edit_model, draft_path, research_model=args.research_model,
    )

    script_path.write_text(script, encoding="utf-8")
    print(f"Script saved: {script_path}", file=sys.stderr)
    emit("stage", stage="script", status="done", path=str(script_path))

    if args.script_only:
        emit("done", output=str(script_path))
        if not args.progress_json:
            print(script)
        return

    # Synthesize audio
    synthesize_audio(
        script,
        voice_samples=voice_samples,
        output_path=args.output,
        tts_model=args.tts_model,
        cfg_scale=args.cfg_scale,
    )
    emit("done", output=args.output)
    if not args.progress_json:
        print(args.output)


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        # die() and argparse already emitted whatever there was to say.
        raise
    except KeyboardInterrupt:
        emit("error", text="interrupted")
        sys.exit(130)
    except Exception as e:
        # Re-raised so the traceback still reaches stderr exactly as before;
        # the event only makes the failure visible to a machine reader.
        emit("error", text=f"{type(e).__name__}: {e}")
        raise
