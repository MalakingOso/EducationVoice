"""Article-to-Podcast: Convert articles into multi-host podcast audio."""

import argparse
import json
import os
import re
import sys
from pathlib import Path

import anyio
from claude_agent_sdk import query, ClaudeAgentOptions, ResultMessage
from claude_agent_sdk.types import AssistantMessage, TextBlock
import requests
from bs4 import BeautifulSoup

# Claude model used for script generation.  Sonnet over Opus here on a
# side-by-side read of real output: Opus writes more densely and more
# stiffly for spoken dialogue.  Override per-run with --model.
SCRIPT_MODEL = "claude-sonnet-5"

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
# Script generation via Claude
# ---------------------------------------------------------------------------

SYSTEM_PROMPT = """\
You write podcast scripts from articles — real conversation between \
hosts, not a lecture. Audience: surgical fellows. Peer-to-peer level. \
Assume full command of medical terminology, anatomy, pharmacology, and \
statistics. Do not simplify \
jargon any surgical trainee would know. \
Do not reference the audience level in the script.

VOICE — hard rules:
- Never open with affirmations ("Certainly", "Absolutely", "Of course", \
"Great question", "Sure", "Happy to help").
- Never use AI framing: "It's important to note", "It's worth noting", \
"In conclusion", "In summary", "In essence", "Let's explore", \
"Let's unpack", "Let's delve into".
- No meta-commentary ("This is a complex topic", "There are several \
factors to consider").
- No bullet-list cadence — write in natural conversational prose.
- Sound like a direct, knowledgeable colleague: answer first, explain \
second, skip the performance.
- BANNED — antithesis by negation. Do not define anything by what it is \
not. No "X, not Y" ("the signal is the approach class, not the \
platform"). No "it isn't A, it's B". No "not just X but Y". No "A is \
not the same as B". No "what I'd say is". No "the real question isn't \
X, it's Y". This construction is the single strongest tell of \
AI-written dialogue and it appears nowhere in real speech at this \
density. State the positive claim and stop. If you catch yourself \
reaching for a contrast, delete the negated half and keep only the \
assertion — "the approach class is what matters" says the same thing.
- Do not end turns on a rhetorical flourish or a summarizing kicker. \
Real people trail off, hand over mid-thought, or just stop.
- BANNED — hedged-disagreement stock phrases. Never write "push back", \
"I'd push back on that", "I'd challenge that", "playing devil's \
advocate", "to be fair", "that said", "fair point", "I hear you, but", \
"where I'd differ". Disagree the way a colleague actually does: say the \
opposing thing flatly. "That's overselling it." "The bleeding data \
doesn't support that." "I don't buy it." No throat-clearing before the \
objection.

Discouraged vocabulary — overused in AI-generated text, sounds formulaic. \
Default to concrete, specific alternatives. Use a word from this list \
only when it is genuinely the best fit and no natural substitute exists: \
a journey of, a multitude of, a plethora of, a testament to, actionable \
insights, adept, adoption rate, aforementioned, agile, ai-powered, \
aligns, ample opportunities, amplify, arduous, as such, at length, \
at the end of the day, augment, bandwidth, based on the information \
provided, best practices, blockchain-enabled, brand awareness, broadly \
speaking, burgeoning, cannot be overstated, capacity building, \
captivating, change management, cloud-based, cognizant, collaborative \
environment, commendable, competitive landscape, complexity, \
conceptualize, considerable, continuous improvement, core, cost \
optimization, craft, critical, customer-centric, data-driven, \
decision-makers, deep understanding, deliverables, delve, delved, \
delving, demonstrates significant, deployment plan, digital realm, \
digital transformation, disruptive innovation, domain expertise, \
downtime, drive, driven approach, driving innovation, dynamic, dynamic \
environment, efficiency, embark, embark on a journey, embarked, emerging \
technologies, enable, encountered hurdles, enhance, enhancing, \
enlightening, enriches, entails, entrenched, epicenter, essentially, \
esteemed, ethical considerations, ever-evolving, excels, exciting, \
exemplary, expertise, explore, facilitate, flourishing, folks, foray, \
foster innovation, fostering, fresh perspectives, from inception to \
execution, fundamental, fundamentally, future-proof, game changer, \
generally speaking, given that, glean, going forward, golden ticket, \
governance framework, granular detail, granular level, granularly, \
grasp, growing recognition, hinder, holistically, impactful, \
implementation strategy, implications, important to consider, in a sea \
of, in brief, in detail, in effect, in general, in light of, in other \
words, in particular, in practice, in terms of, in the dynamic world of, \
in the realm of, in theory, in today's rapidly evolving market, \
in today's world, industry best practices, influencers, innovative, \
insights into, invaluable, issue resolution, it's important to \
remember, iteration, kaleidoscope, key, key takeaways, knowledge \
transfer, kpis, latency, linchpin, low-level, manifold, market \
penetration, market share, market trends, maximize, milestone, \
mission-critical, moving forward, mvp, namely, navigating the landscape, \
navigating the complexities of, nevertheless, new heights, \
next-generation, notable, numerous, offer a comprehensive, offerings, \
on the ascent to, on the contrary, on the cutting edge, on the other \
hand, operational efficiency, operational excellence, pain point, \
paradigm shift, particularly in areas, performance optimization, \
pervasive, pivotal, plethora, preemptively, primary, problem solving, \
process optimization, profitability, profound, promote, pronged, quality \
assurance, quality control, rapidly evolving, reaching new heights, \
realm, recognize, regulatory compliance, relentless, remarkable, \
resonate, resource allocation, resource optimization, revenue growth, \
risk mitigation, roadmap, robust, roi, root cause analysis, scalable, \
scrum, seamless, secondary, shed light, shedding light on, showcasing, \
significant, significantly contributes, simply put, sla, solution \
development, specifically speaking, sprint, stakeholders, \
state-of-the-art, strategic alignment, streamline, strive, strong \
presence, subject matter experts, substantial, substantially, \
sustainability, synergistically, synergy, systemic, tailor, tapestry, \
tco, tertiary, that being said, the future of, the linchpin of, \
the next frontier, the power of, the road ahead, thereby, therefore, \
therein, thereof, thought leaders, thought leadership, \
thought-provoking, thrive, thriving, throughput, time optimization, \
to clarify, to demonstrate, to elucidate, to emphasize, to enrich, \
to exemplify, to furnish, to highlight, to illustrate, to maximize, \
to provide, to reiterate, to shed light on, to showcase, to summarize, \
to thrive, to underscore, to unleash, to unlock, touchpoint, \
transformation, transformative, transforming the way, treasure trove, \
ultimately, uncharted waters, undeniable, underscores, understanding of \
your unique, undoubtedly, unleash, unlock, unparalleled, uptime, \
user engagement, user experience, user feedback, user interface, \
utilize, utmost, valuable, value proposition, value-added, various, \
vast, vibrant, vital, well-crafted, whilst, whilst it is true, \
widely recognized, with a keen eye on, with regards to.

FORMAT:
- Output ONLY the script — no preamble, commentary, or files. Do not \
use the Write or Edit tools.
- Every line: Speaker N: dialogue text (N from 1 to {num_hosts})
- Speaker 1 is the lead host. Others ask questions, add insights, and \
disagree when they have reason to.
{length_guidance}
- Tone: {tone}.
- No stage directions, sound effects, or [brackets].

RHYTHM:
- Let the content set the pace. A turn runs as long as the thought \
needs and stops when it is finished — a few words when someone is \
reacting, a full paragraph when a mechanism or a study design genuinely \
needs unpacking. Do not ration either one.
- The only thing to avoid is the shape where two people alternate \
speeches of similar length. Real conversation is lopsided and uneven.
- Interrupt for real: half-sentence reactions, corrections that land \
before the other host finishes, someone picking up a thread from two \
turns back.
- Open a turn by reacting to what was just said rather than starting a \
new topic cold. Let disagreements sit unresolved when they would in \
life.

RESEARCH (do this BEFORE writing):
- Conduct a thorough background review — multiple PubMed queries \
(different keywords, author names, related conditions), Scholar Gateway \
for methods/drugs/devices/protocols, WebSearch for guideline updates \
and recent developments. This is not optional.
- The research is for you, the writer — not for the listener. It is what \
lets the hosts sound like people who already know this literature. Most \
of it should never be said aloud; it shows up as confidence about which \
numbers matter and which do not.
- Name a study only when it changes how you read THIS paper: it \
contradicts the result, it is the trial this one aims to displace, it \
explains a design choice, or it is the guideline this would change. Two \
or three named sources in an episode is normal. More than four is a \
bibliography, not a conversation.
- When you do name a source, use first author + year (e.g., "the Smith \
2023 trial in JAMA") or the trial name (e.g., "the RECOVERY trial"). \
Never "some studies show" or "the literature suggests".
- Do not distribute citations for coverage. A section with no external \
reference is fine.

PLAN THE EPISODE (after research, before you write any dialogue):
Work this through out loud, in a message of its own. It is thinking, not \
output — the listener never sees it, so be blunt and be willing to \
throw things away.

1. BRAINSTORM WIDELY. Put up five to eight genuinely different angles \
the episode could take. Cast a wide net: the argument the paper is \
making; the methodological weak point that undercuts it; the single \
clinical decision that changes on Monday; the history of how the \
current practice got established and why it stuck; the number that \
contradicts what everyone does; the question the paper conspicuously \
fails to answer; the disagreement two honest experts would still have \
after reading it. These should be real alternatives that would produce \
noticeably different episodes — not one idea phrased five ways. Do not \
settle on anything yet.

2. REFLECT ON EACH, HONESTLY. For every angle, say what makes it \
compelling and where it falls apart. Is there enough in the paper to \
sustain it for a whole episode, or does it run dry after four minutes? \
Does it need numbers the paper does not report? Would it require the \
hosts to explain background that is more boring than the payoff? Is it \
interesting to a surgical fellow, or only to a methodologist? Name the \
weaknesses plainly — an angle you talk yourself into is the one that \
produces a flat episode.

3. NARROW. Choose the spine of the episode and say why it beat the \
others. Then name the two or three moments that have to land: the \
opening hook, the point where the argument turns, the thing a listener \
should still remember tomorrow. Note anything from the discarded angles \
worth keeping as a beat along the way.

4. WRITE THE SCRIPT. Your final message must contain the script and \
nothing else — no plan, no headings, no commentary, just Speaker lines.

CONTENT:
- Study every table and figure before you write — but report only what \
changes a decision: the effect big enough to act on, the number that \
surprises, the subgroup where the answer flips, the confidence interval \
that undercuts the headline. Do not walk a table row by row. If a \
figure's message is one sentence, say the sentence and move on.
- Never drop more than two statistics in a row without a plain-language \
clinical interpretation. After citing a number, say what it means for the \
patient or the surgeon's decision — e.g., "So roughly one in six women \
avoided incontinence thanks to the sling" or "That's a one-in-fifteen \
chance of a trocar going through the bladder — not trivial for a \
prophylactic procedure." Stats without interpretation are noise.
- Natural conversation: "Wait, so you're saying...", "Right, exactly", \
genuine pushback and questions between hosts."""


# Default: no word count, no duration, no turn count.  The episode runs as
# long as the article has substance to fill and stops there.  Word budgets
# were tried and made the writing worse — the model pads a thin section or
# rushes a dense one to hit the number.
LENGTH_BY_DENSITY = """\
- Length: let the article decide. A dense paper with fifty procedure \
estimates and a real methodological weakness earns a long episode; a \
thin one earns a short one. There is no target duration and no word \
count. Cover what is worth covering, at the pace it deserves, and stop \
when you are done rather than filling to a quota.
- Aim for a calm, unhurried listen — the kind of conversation someone \
can follow on a commute and come away actually understanding the paper. \
Never rush a point to save time and never stretch one to fill time."""


def generate_script(
    article_text: str,
    num_hosts: int = 2,
    tone: str = "conversational and engaging",
    target_length: str | None = None,
    is_pdf: bool = False,
    model: str = SCRIPT_MODEL,
) -> str:
    """Generate a podcast script from article text using Claude Agent SDK."""
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

    research_instruction = (
        "First, research the topic using PubMed, Scholar Gateway, and web "
        "search to find related studies and background context. Then write "
        "the podcast script. Read every table and figure, but discuss only "
        "what changes a clinical decision.\n\n"
        "Before writing, brainstorm several different angles the episode "
        "could take, reflect out loud on what works and what does not "
        "about each, and narrow to the one you think makes the most "
        "compelling piece. Then write the script as your final message."
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
    emit("stage", stage="script", status="start", model=model)

    # Allow enough turns for: reading PDF (multiple pages) + research
    # lookups + brainstorm/reflect/narrow + script generation
    max_turns = 30

    # Candidate scripts, in the order Claude produced them.  We select by
    # *shape* rather than length: the model now brainstorms and reflects out
    # loud before writing, and that planning prose can easily be longer than
    # the script itself.  Only messages that actually contain dialogue lines
    # count, and the last such message wins — that is the finished draft,
    # after any revision.
    script_re = re.compile(r"^Speaker \d+:", re.M)
    candidates: list[str] = []
    longest_text = ""

    async def _generate():
        nonlocal longest_text
        async for message in query(
            prompt=user_msg,
            options=ClaudeAgentOptions(
                system_prompt=system,
                model=model,
                max_turns=max_turns,
                allowed_tools=[
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
                permission_mode="bypassPermissions",
            ),
        ):
            if isinstance(message, AssistantMessage):
                # Extract text blocks from the last assistant message
                text_parts = [
                    block.text for block in message.content
                    if isinstance(block, TextBlock)
                ]
                if text_parts:
                    candidate = "\n".join(text_parts)
                    # Emitted as it arrives, not at the end: this is the only
                    # window into an otherwise silent 3-4 minute call, and it
                    # is where the brainstorm/reflect/narrow reasoning shows.
                    emit("message", text=candidate)
                    if len(script_re.findall(candidate)) >= 2:
                        candidates.append(candidate)
                    if len(candidate) > len(longest_text):
                        longest_text = candidate

    anyio.run(_generate)

    if candidates:
        result_text = candidates[-1]
    elif longest_text:
        # No message looked like dialogue.  Fall back to the longest text so
        # validate_script can report what actually came back.
        warn = (
            "Warning: no message contained Speaker lines; falling back to the "
            "longest assistant reply"
        )
        print(warn, file=sys.stderr)
        emit("warning", text=warn.removeprefix("Warning: "))
        result_text = longest_text
    else:
        die("Error: no script was generated")

    return validate_script(result_text, num_hosts)


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

    # Generate script
    script = generate_script(
        article, args.hosts, args.tone, args.length, is_pdf, args.model
    )

    # Save script.  Without --script-out this is script.txt beside the audio,
    # which every run overwrites — fine for a one-off, useless as a history.
    if args.script_out:
        script_path = Path(args.script_out).expanduser()
    else:
        script_path = Path(args.output).parent / "script.txt"
    script_path.parent.mkdir(parents=True, exist_ok=True)
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
