# article2pod

Convert journal articles into multi-host podcast audio using Claude for script generation and Microsoft VibeVoice for text-to-speech.

## Setup

Requires Python 3.12 and an Intel Arc GPU (tested on Arc B570 / Arc Pro B60, Linux).

```bash
python3.12 -m venv .venv
source .venv/bin/activate          # Linux / macOS
# .venv\Scripts\activate         # Windows

# PyTorch with Intel XPU support — must use the XPU index, not default PyPI.
# Install this FIRST and on its own; mixing indexes lets pip resolve the
# CPU-only PyPI wheel instead.
#
# Pin 2.12.1 — see "Broken torch XPU versions" below. Do NOT take 2.13.0+xpu.
pip install 'torch==2.12.1+xpu' torchvision torchaudio \
    --index-url https://download.pytorch.org/whl/xpu

# VibeVoice and other dependencies
pip install -r requirements.txt
```

Verify the GPU is visible before going further:

```bash
python test_xpu.py
```

After installing vibevoice, apply the patch described below.

## Usage

```bash
source .venv/bin/activate

# Full pipeline: PDF/URL/text -> script -> audio
python article2pod.py "path/to/article.pdf" --output output/episode.wav

# Script only (no TTS) — fast, just generates the dialogue
python article2pod.py "path/to/article.pdf" --script-only

# Options — length is optional; omit it and the article's density sets the pace
python article2pod.py article.txt --hosts 3 --output output/episode.mp3
python article2pod.py article.txt --length "10 minutes"   # explicit steer
```

| Flag | Default | Description |
|------|---------|-------------|
| `--hosts` | 2 | Number of speakers (2-4) |
| `--length` | auto | Optional length steer; by default the article's density sets it |
| `--tone` | conversational and engaging | Script tone |
| `--model` | claude-sonnet-5 | Claude model used for script generation |
| `--output` | output/podcast.wav | Output file (.wav or .mp3) |
| `--script-only` | off | Skip TTS, print script to stdout |
| `--from-script` | none | Synthesize an existing script file, skipping generation |
| `--voices` | alice carter | Preset voices in speaker order — see [Voices](#voices) |
| `--voice-samples` | none | Your own WAV/MP3 files for each speaker, in order |
| `--zero-shot` | off | Let VibeVoice invent voices instead of using clips |
| `--fetch-voices` | — | Download all preset voice clips and exit |
| `--cfg-scale` | 1.3 | VibeVoice classifier-free guidance scale |

`--voices`, `--voice-samples`, and `--zero-shot` are mutually exclusive.

### Reusing a script

Script generation is stochastic — a rerun gives a different episode. When you get one you
like, keep it and synthesize from the file instead of re-rolling:

```bash
# Re-voice a saved script without touching the writing (~11 min, no Claude call)
python article2pod.py --from-script scripts/rotbigs.txt --output output/ep.wav

# Same script, different voices
python article2pod.py --from-script scripts/rotbigs.txt --voices maya frank
```

`--from-script` still runs the speaker-id validation, so a hand-edited file gets the same
guards as a generated one. Saved scripts live in `scripts/` and are tracked in git; the
audio is not, since it is large and regenerable from the script in ~11 minutes.

### What happens when you run it

1. Claude reads the article (PDFs are read natively, preserving tables/figures)
2. Claude researches the topic via PubMed, Scholar Gateway, and web search
3. Claude brainstorms several possible angles, reflects on the weaknesses of each,
   and narrows to the one that makes the most compelling episode
4. A multi-host podcast script is generated and saved to `output/script.txt`
5. VibeVoice synthesizes the audio on the Intel Arc GPU
6. Output is saved as WAV (or converted to MP3 if `--output` ends in `.mp3`)

Script generation takes ~3-4 minutes (Claude researches, brainstorms several angles,
reflects on them, then writes) and TTS runs at roughly 1.25x realtime, so a 15-minute
episode is about **16 minutes of wall time** end to end. Most of that is a single silent
Python process — it is not hung. The TTS half is host-bound, so a faster GPU does not help
(see [Which GPU](#which-gpu--benchmarked)).

## Desktop app

A Dioxus GUI over the same pipeline lives in `gui/`. It shells out to
`.venv/bin/python article2pod.py`, so it runs exactly the code the CLI runs and
inherits the same pinned interpreter.

```bash
cd gui
cargo run            # or: dx serve --platform desktop
```

It works in two stages with a review gate between them: write the script, edit
it, then synthesize. A checkbox skips the gate and runs the CLI's one-shot path
instead. A strip along the bottom shows the live stage, an elapsed clock, and —
once TTS starts — a real step count, so a long run is visibly working rather
than merely silent. Cancel signals the whole process group, which is what stops
the `claude` grandchild along with it.

**On Wayland the app forces `GDK_BACKEND=x11`.** WebKitGTK's Wayland backend
never completes its IPC handshake here, and the failure is silent: the window
opens and renders, but no async task ever runs, so it looks healthy and does
nothing. Set `ARTICLE2POD_KEEP_GDK_BACKEND=1` to opt out once that is fixed
upstream.

The CLI gained three flags for the GUI, all additive and all off by default:
`--progress-json` (JSON-lines events on stdout, human text stays on stderr),
`--script-out PATH` (so each run keeps its own script instead of overwriting
`output/script.txt`), and `--list-voices` (the roster as JSON).

```bash
# Watch the event stream the GUI consumes
python article2pod.py --from-script scripts/rotbigs.txt \
    --output output/ep.wav --progress-json | jq -c .
```

## Voices

VibeVoice clones whichever reference clip you give it. **Without a reference it does not pick
a default voice — it samples a speaker identity from unconditioned diffusion**, producing an
arbitrary voice that can drift mid-episode. A no-flag run therefore uses the preset pair
below rather than zero-shot.

| Name | Gender | File |
|------|--------|------|
| `alice` | female | `en-Alice_woman.wav` |
| `maya` | female | `en-Maya_woman.wav` |
| `frank` | male | `en-Frank_man.wav` |
| `carter` | male | `en-Carter_man.wav` |
| `yasser` | male | `en-Yasser_man.wav` |
| `samuel` | male, Indian accent | `in-Samuel_man.wav` |

Default rosters, alternating gender so adjacent speakers stay distinguishable:

| Hosts | Speaker 1 | Speaker 2 | Speaker 3 | Speaker 4 |
|-------|-----------|-----------|-----------|-----------|
| 2 | alice | carter | | |
| 3 | alice | carter | maya | |
| 4 | alice | carter | maya | yasser |

```bash
# Default: Alice (Speaker 1) + Carter (Speaker 2)
python article2pod.py article.pdf

# Pick your own pairing — order is speaker order
python article2pod.py article.pdf --voices maya frank

# Pre-download all six clips (otherwise the first run fetches what it needs)
python article2pod.py --fetch-voices
```

Only two English female presets exist upstream, which caps how much variety is available
without supplying your own recording via `--voice-samples`.

### Where the clips come from

The pip wheel ships no audio assets and neither does `microsoft/VibeVoice-1.5B`. The upstream
preset clips survive only inside HF Space repos. They are downloaded on first use to
`voices/` (gitignored) from the `Steveeeeeeen/VibeVoice-Large` Space, **pinned to revision
`93ece79b`** — a Space can silently re-record a file in place, which would change the
podcast's voices between runs.

Every run checks `voices/` before touching the network, so once the clips are on disk the
Space going away, getting rate-limited, or renaming files stops mattering. If the download
fails, the error names the file, the URL, the manual placement path, and the `--voice-samples`
/ `--zero-shot` bypasses. A byte-identical mirror lives at `yasserrmd/VibeVoice`.

`en-Alice_woman_bgm.wav` is deliberately excluded — it has background music baked into the
reference clip, which bleeds into every utterance conditioned on it.

### Silent failure modes this guards against

VibeVoice validates none of this itself. `voice_samples[0]` binds to `Speaker 1`, `[1]` to
`Speaker 2`, and the processor slices `voice_samples[:len(speakers)]` with no checks — so
too few clips means the extra speakers silently get invented, drifting voices. Speaker ids
must also be contiguous from 1: the processor normalizes ids only when the lowest is above
zero, so a single stray `Speaker 0:` line shifts *every* voice by one. `article2pod.py`
checks both before the model loads.

## Examples

A sample article, `ROTBIGS.pdf`, ships with the repo:

```bash
python article2pod.py ROTBIGS.pdf --output output/rotbigs.wav
```

`scripts/rotbigs.txt` is a generated script from that PDF kept as a reference — 50 turns,
2,142 words, which synthesized to 11m27s. Re-voice it without a Claude call:

```bash
python article2pod.py --from-script scripts/rotbigs.txt --output output/rotbigs.wav
```

### OPUS trial

```bash
python article2pod.py ~/Books/Landmark\ Trials/OPUS.pdf --output output/opus.wav
```

Produced a 2-host, 11m 45s podcast episode (33.8MB WAV) from the OPUS trial PDF. The script referenced external trials from PubMed and included discussion of all key tables and figures.

## VibeVoice patch for zero-shot TTS

**Only needed for `--zero-shot`.** The default path now passes reference clips (see
[Voices](#voices)), which keeps these fields as real tensors and never reaches the crash.
Apply the patch anyway if you want `--zero-shot` to work.

vibevoice 0.0.1 has a bug in `modeling_vibevoice_inference.py` that crashes when no voice reference samples are provided (zero-shot mode). The `generate()` method assumes `speech_tensors`, `speech_masks`, and `speech_input_mask` are always tensors, but the processor sets them to `None` when no voice samples are given.

The crash occurs at line 468 in the prefill block:

```python
# BEFORE (crashes with AttributeError: 'NoneType' object has no attribute 'to')
prefill_inputs = {
    "speech_tensors": speech_tensors.to(device=device),
    "speech_masks": speech_masks.to(device),
    "speech_input_mask": speech_input_mask.to(device),
}
```

The fix adds None guards:

```python
# AFTER
prefill_inputs = {
    "speech_tensors": speech_tensors.to(device=device) if speech_tensors is not None else None,
    "speech_masks": speech_masks.to(device) if speech_masks is not None else None,
    "speech_input_mask": speech_input_mask.to(device) if speech_input_mask is not None else None,
}
```

This works because the model's `forward()` method already handles `None` for these fields (line 221: `if speech_tensors is not None and speech_masks is not None`). Only `generate()` was missing the check.

### Applying the patch

Edit `.venv/lib/python3.12/site-packages/vibevoice/modular/modeling_vibevoice_inference.py` and make the change shown above around line 468. The patch is lost if vibevoice is reinstalled.

## Broken torch XPU versions

**`torch 2.13.0+xpu` silently returns wrong results on Arc Battlemage GPUs.** Verified
2026-08-24 on Arc B570 and Arc Pro B60. `torch.masked_select`, `torch.nonzero`, and
`torch.unique` return truncated output with no error raised:

```python
x = torch.randn(10000, device="xpu")
torch.masked_select(x, x > 0).numel()   # 324 — should be 4987
```

The trigger is a partial trailing workgroup in the stream-compaction kernel: results are
correct only when `numel` is an exact multiple of the workgroup size (`4096` ok, `4097`
wrong, `8192` ok, `8193` wrong). The mask itself is fine — `(x > 0).sum()` is correct.

This matters for TTS because `generate()` runs masked ops over sequence lengths that are
essentially never exact powers of two. The failure mode is a clean-looking run that
produces garbage audio.

It also makes the bug hard to spot: torch's tensor `__repr__` calls `masked_select`
internally, so printing an XPU tensor shows wrong values or raises
`numel: integer multiplication overflow`.

`torch==2.12.1+xpu` is verified clean. Pin it. Note that installing any package depending
on torch can pull the bad version back in — re-check with `python test_xpu.py` afterwards.

## Optional: transformers 4.57.6

The stack is pinned to `transformers==4.51.3` because `vibevoice 0.0.1` hard-pins it. Moving
to the latest 4.x (`4.57.6`) is feasible — every symbol vibevoice imports still exists there —
and buys one XPU-specific improvement.

`4.51.3` has **zero** XPU handling in the attention dispatch path. XPU falls through to the
generic branch, which only enables native grouped-query attention in SDPA when
`attention_mask is None` — a condition written for CUDA kernels. Since this pipeline passes
`return_attention_mask=True`, every attention call materialises expanded K/V through
`repeat_kv`. `4.57.6` adds an XPU branch that enables it regardless of the mask, gated on
torch >= 2.8:

```python
# 4.57.6 integrations/sdpa_attention.py
if _is_torch_xpu_available:
    return _is_torch_greater_or_equal_than_2_8 and not isinstance(key, torch.fx.Proxy)
```

On a Qwen2-1.5B-class backbone that avoids a several-fold expansion of the K/V activations,
which matters most on the 10 GB B570.

Caveats: the symbol check proves it *imports*, not that it *runs* — the attention dispatch
surface grew from 68 to 220 files between 4.51 and 4.57, and the hard pin may exist because
something broke at runtime. Get a known-good audio run on 4.51.3 first so there is a baseline
to compare against. Installing prints a resolver error against vibevoice's pin but proceeds,
and any later `pip install -r requirements.txt` snaps it back to 4.51.3.

transformers **5.x** is a larger job requiring a monkey-patch — see
[TRANSFORMERS_V5_PLAN.md](TRANSFORMERS_V5_PLAN.md).

## Which GPU — benchmarked

Measured 2026-08-24 on this machine (Arc B570 = `xpu:0`, Arc Pro B60 = `xpu:1`),
torch 2.12.1+xpu, VibeVoice-1.5B bf16/sdpa, identical seeded scripts.

**They are effectively tied. Pick either.**

| script | B570 | B60 | audio out |
|---|---|---|---|
| 4 lines  | 14.18 s | 14.53 s | 18.4 s |
| 24 lines | 79.82 s | 80.85 s | 99.7 s |

Both produce ~1.25x realtime (a 10-minute episode takes ~8 minutes). Peak memory is
**5.47 GiB on both, and does not grow with script length** — so the B570's 10 GB is not a
constraint for this workload.

This is surprising given the raw hardware, where the B60 wins everything:

| microbenchmark | B570 | B60 |
|---|---|---|
| GEMM bf16 4096 | 56.0 TFLOPS | **89.8 TFLOPS** (+60%) |
| Read bandwidth | 373.9 GB/s | **445.6 GB/s** (+19%) |
| Decode matvec (1x1536 @ 1536x8960) | 101.2 us | **61.1 us** (+66%) |
| SDPA 1024 ctx | **0.131 ms** | 0.146 ms |

**The reason 60% more compute buys nothing: the pipeline is host-bound, not GPU-bound.**
A full run measures **102% CPU** — one Python thread pegged for the entire wall-clock
duration. The GPU spends most of its time waiting on the host generation loop, so a faster
GPU has nothing to speed up. Per-step cost stays ~80 ms regardless of which card runs it.

If TTS speed ever needs to improve, the lever is the host loop (`torch.compile` on the
diffusion head, cutting per-step Python overhead), not a bigger GPU.

### Choosing a device

`article2pod.py` uses `device_map="xpu"`, which is always `xpu:0` — the B570. To run on the
B60 without touching the code:

```bash
ZE_AFFINITY_MASK=1 python article2pod.py article.pdf --output output/ep.wav
```

The masked device appears to torch as `xpu:0`, so no code path changes.

Reasons to prefer the **B60** despite the tie: it is headless, while the B570 drives two
DisplayPort outputs and has already given up ~2.8 GB to the display (6.61 GiB free vs 20.5).
Running TTS on the B570 competes with the desktop compositor.

## Key details

- **Model ID**: `microsoft/VibeVoice-1.5B` (not `VibeVoice-TTS-1.5B`)
- **PyTorch XPU**: Install from `https://download.pytorch.org/whl/xpu` — the default PyPI torch package is CPU-only and `torch.xpu.is_available()` will return `False`
- **No Intel PyTorch extension needed**: Intel GPU support was upstreamed into PyTorch at 2.5 and is native in the XPU torch build
- **dtype**: bfloat16 on XPU, bfloat16 on CUDA, float32 on CPU/MPS
