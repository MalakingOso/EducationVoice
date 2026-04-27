"""Article-to-Podcast: Convert articles into multi-host podcast audio."""

import argparse
import os
import re
import sys
from pathlib import Path

import anyio
from claude_agent_sdk import query, ClaudeAgentOptions, ResultMessage
from claude_agent_sdk.types import AssistantMessage, TextBlock
import requests
from bs4 import BeautifulSoup

# ---------------------------------------------------------------------------
# Article ingestion
# ---------------------------------------------------------------------------

def ingest_article(source: str) -> tuple[str, bool]:
    """Read article from URL, file path, or stdin.

    Returns (text_or_path, is_pdf).  When the source is a PDF file we return
    the *absolute path* so Claude can read it natively — this preserves
    tables, figures, and visual layout that text extraction would lose.
    """
    if source == "-":
        return sys.stdin.read(), False

    if source.startswith("http://") or source.startswith("https://"):
        resp = requests.get(source, timeout=30, headers={"User-Agent": "article2pod/1.0"})
        resp.raise_for_status()
        soup = BeautifulSoup(resp.text, "html.parser")
        paragraphs = [p.get_text(strip=True) for p in soup.find_all("p")]
        return "\n\n".join(p for p in paragraphs if p), False

    path = Path(source)
    if path.is_file():
        if path.suffix.lower() == ".pdf":
            return str(path.resolve()), True
        return path.read_text(encoding="utf-8"), False

    return source, False


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
- Speaker 1 is the lead host. Others ask questions, add insights, \
push back occasionally.
- Target approximately {target_length}. Tone: {tone}.
- No stage directions, sound effects, or [brackets].

RESEARCH (do this BEFORE writing):
- Conduct a thorough background review — multiple PubMed queries \
(different keywords, author names, related conditions), Scholar Gateway \
for methods/drugs/devices/protocols, WebSearch for guideline updates \
and recent developments. This is not optional.
- Name every source in the script: first author + year (e.g., "the \
Smith 2023 trial in JAMA") or trial name (e.g., "the RECOVERY trial"). \
No vague "some studies" or "the literature suggests" — if you found it, \
cite it.
- Target at least 5-10 external sources beyond the article itself.
- Distribute external sources across the full script. Every major \
section (background, methods, primary results, secondary outcomes, \
complications, clinical implications) should reference at least one \
outside study or guideline where relevant. Do not save all external \
context for a single "literature comparison" block near the end.
- Weave research naturally into dialogue — hosts reference related \
studies and clinical context to give listeners the full picture.

CONTENT:
- Review ALL tables and figures. Describe data, trends, and findings \
so listeners understand the visual information without seeing it.
- Never drop more than two statistics in a row without a plain-language \
clinical interpretation. After citing a number, say what it means for the \
patient or the surgeon's decision — e.g., "So roughly one in six women \
avoided incontinence thanks to the sling" or "That's a one-in-fifteen \
chance of a trocar going through the bladder — not trivial for a \
prophylactic procedure." Stats without interpretation are noise.
- Natural conversation: "Wait, so you're saying...", "Right, exactly", \
genuine pushback and questions between hosts."""


def generate_script(
    article_text: str,
    num_hosts: int = 2,
    tone: str = "conversational and engaging",
    target_length: str = "15 minutes",
    is_pdf: bool = False,
) -> str:
    """Generate a podcast script from article text using Claude Agent SDK."""
    system = SYSTEM_PROMPT.format(
        num_hosts=num_hosts, tone=tone, target_length=target_length
    )

    research_instruction = (
        "First, research the topic using PubMed, Scholar Gateway, and web "
        "search to find related studies and background context. Then write "
        "the podcast script, making sure you review and discuss ALL important "
        "tables and figures."
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

    # Allow enough turns for: reading PDF (multiple pages) + research lookups + script generation
    max_turns = 25

    result_text = ""

    async def _generate():
        nonlocal result_text
        async for message in query(
            prompt=user_msg,
            options=ClaudeAgentOptions(
                system_prompt=system,
                model="claude-opus-4-6",
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
                    # Keep the longest assistant text — the script is the
                    # longest text output; earlier messages are tool-use
                    # reasoning or short status lines.
                    if len(candidate) > len(result_text):
                        result_text = candidate

    anyio.run(_generate)

    if not result_text:
        print("Error: no script was generated", file=sys.stderr)
        sys.exit(1)

    return validate_script(result_text)


def validate_script(script: str) -> str:
    """Strip any lines that aren't properly formatted Speaker N: lines."""
    pattern = re.compile(r"^Speaker \d+:")
    lines = script.strip().splitlines()
    valid = []
    stripped = 0
    for line in lines:
        line = line.strip()
        if not line:
            continue
        if pattern.match(line):
            valid.append(line)
        else:
            stripped += 1
    if stripped:
        print(f"Warning: stripped {stripped} non-script lines", file=sys.stderr)
    return "\n".join(valid)


# ---------------------------------------------------------------------------
# Audio synthesis via VibeVoice
# ---------------------------------------------------------------------------

DEFAULT_SPEAKERS = {
    2: ["Alice", "Frank"],
    3: ["Alice", "Frank", "Maya"],
    4: ["Alice", "Frank", "Maya", "Carter"],
}


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

    processor = VibeVoiceProcessor.from_pretrained(tts_model)
    model = VibeVoiceForConditionalGenerationInference.from_pretrained(
        tts_model, torch_dtype=dtype, device_map=device, attn_implementation=attn
    )

    # Build voice samples dict if provided
    voice_kwargs = {}
    if voice_samples:
        voice_kwargs["voice_samples"] = voice_samples

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
    )

    # Save audio
    out = Path(output_path)
    out.parent.mkdir(parents=True, exist_ok=True)

    wav_path = out if out.suffix == ".wav" else out.with_suffix(".wav")
    processor.save_audio(outputs.speech_outputs[0], output_path=str(wav_path))
    print(f"Saved WAV: {wav_path}", file=sys.stderr)

    # Convert to MP3 if requested
    if out.suffix == ".mp3":
        try:
            from pydub import AudioSegment
            audio = AudioSegment.from_wav(str(wav_path))
            audio.export(str(out), format="mp3", bitrate="192k")
            wav_path.unlink()
            print(f"Converted to MP3: {out}", file=sys.stderr)
        except Exception as e:
            print(f"MP3 conversion failed ({e}), keeping WAV", file=sys.stderr)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Convert articles into multi-host podcast audio"
    )
    parser.add_argument("source", help="Article URL, file path, or '-' for stdin")
    parser.add_argument("--hosts", type=int, default=2, choices=[2, 3, 4])
    parser.add_argument("--tone", default="conversational and engaging")
    parser.add_argument("--length", default="5 minutes")
    parser.add_argument(
        "--voice-samples", nargs="+", default=None,
        help="WAV/MP3 files for each speaker, in order",
    )
    parser.add_argument("--output", default="output/podcast.wav")
    parser.add_argument(
        "--script-only", action="store_true",
        help="Only generate script, skip TTS",
    )
    parser.add_argument("--tts-model", default="microsoft/VibeVoice-1.5B")
    parser.add_argument("--cfg-scale", type=float, default=1.3)

    args = parser.parse_args()

    # Ingest
    article, is_pdf = ingest_article(args.source)
    if is_pdf:
        print(f"PDF: {article} (Claude will read natively)", file=sys.stderr)
    else:
        print(f"Article: {len(article)} characters", file=sys.stderr)

    # Generate script
    script = generate_script(article, args.hosts, args.tone, args.length, is_pdf)

    # Save script
    out_dir = Path(args.output).parent
    out_dir.mkdir(parents=True, exist_ok=True)
    script_path = out_dir / "script.txt"
    script_path.write_text(script, encoding="utf-8")
    print(f"Script saved: {script_path}", file=sys.stderr)

    if args.script_only:
        print(script)
        return

    # Synthesize audio
    synthesize_audio(
        script,
        voice_samples=args.voice_samples,
        output_path=args.output,
        tts_model=args.tts_model,
        cfg_scale=args.cfg_scale,
    )
    print(args.output)


if __name__ == "__main__":
    main()
