# article2pod

Convert journal articles into multi-host podcast audio using Claude for script generation and Microsoft VibeVoice for text-to-speech.

## Setup

Requires Python 3.12 and an Intel Arc GPU (tested on Arc B570).

```bash
python -m venv .venv
.venv/Scripts/activate  # Windows

# PyTorch with Intel XPU support — must use the XPU index, not default PyPI
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/xpu

# VibeVoice and other dependencies
pip install vibevoice pdfplumber beautifulsoup4 requests anyio claude-agent-sdk
```

After installing vibevoice, apply the patch described below.

## Usage

```bash
.venv/Scripts/activate

# Full pipeline: PDF/URL/text -> script -> audio
python article2pod.py "path/to/article.pdf" --output output/episode.wav

# Script only (no TTS) — fast, just generates the dialogue
python article2pod.py "path/to/article.pdf" --script-only

# Options
python article2pod.py article.txt --hosts 3 --length "10 minutes" --output output/episode.mp3
```

| Flag | Default | Description |
|------|---------|-------------|
| `--hosts` | 2 | Number of speakers (2-4) |
| `--length` | 5 minutes | Target podcast length |
| `--tone` | conversational and engaging | Script tone |
| `--output` | output/podcast.wav | Output file (.wav or .mp3) |
| `--script-only` | off | Skip TTS, print script to stdout |
| `--voice-samples` | none | WAV/MP3 files for each speaker (zero-shot if omitted) |
| `--cfg-scale` | 1.3 | VibeVoice classifier-free guidance scale |

### What happens when you run it

1. Claude reads the article (PDFs are read natively, preserving tables/figures)
2. Claude researches the topic via PubMed, Scholar Gateway, and web search
3. A multi-host podcast script is generated and saved to `output/script.txt`
4. VibeVoice synthesizes the audio on the Intel Arc GPU
5. Output is saved as WAV (or converted to MP3 if `--output` ends in `.mp3`)

Script generation takes ~2-3 minutes. TTS takes ~8-12 minutes for a 40-line script on Arc B570.

## Example: OPUS trial

```bash
python article2pod.py "C:\Users\berkl\OneDrive\Books\Boox\Needs Annotation\Landmark Trials\OPUS.pdf" --output output/opus.wav
```

Produced a 2-host, 11m 45s podcast episode (33.8MB WAV) from the OPUS trial PDF. The script referenced external trials from PubMed and included discussion of all key tables and figures.

## VibeVoice patch for zero-shot TTS

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

Edit `.venv/Lib/site-packages/vibevoice/modular/modeling_vibevoice_inference.py` and make the change shown above around line 468. The patch is lost if vibevoice is reinstalled.

## Key details

- **Model ID**: `microsoft/VibeVoice-1.5B` (not `VibeVoice-TTS-1.5B`)
- **PyTorch XPU**: Install from `https://download.pytorch.org/whl/xpu` — the default PyPI torch package is CPU-only and `torch.xpu.is_available()` will return `False`
- **IPEX not needed**: Intel Extension for PyTorch is unnecessary as of PyTorch 2.5+; XPU support is built into the XPU torch build
- **dtype**: bfloat16 on XPU, bfloat16 on CUDA, float32 on CPU/MPS
