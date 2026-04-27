"""Demo: VibeVoice-Realtime-0.5B — experimental Spanish TTS on Intel XPU."""

import copy
import sys
import time
from pathlib import Path

import torch

from vibevoice.modular.modeling_vibevoice_streaming_inference import (
    VibeVoiceStreamingForConditionalGenerationInference,
)
from vibevoice.processor.vibevoice_streaming_processor import (
    VibeVoiceStreamingProcessor,
)

MODEL_ID = "microsoft/VibeVoice-Realtime-0.5B"
OUTPUT_DIR = Path("output")
OUTPUT_DIR.mkdir(exist_ok=True)

# Spanish voice preset (pre-downloaded)
VOICE_PRESET = Path(__file__).parent / "voices" / "streaming_model" / "experimental_voices" / "sp-Spk1_man.pt"

SPANISH_TEXT = (
    "MUCHAS COSAS hacen odioso al régimen iraní, pero lo que lo convierte en "
    "especialmente peligroso es su búsqueda de armas nucleares. Su promesa de "
    "no fabricar una bomba quedó desmentida por su determinación de enriquecer "
    "uranio a grado militar. "
    "Eso ha sustentado durante mucho tiempo los intentos del régimen de "
    "intimidar a sus vecinos y amenazar la supervivencia de Israel. "
    "Si la guerra desatada por Estados Unidos e Israel el 28 de febrero ha de "
    "considerarse siquiera un éxito limitado, debe entonces retrasar las "
    "ambiciones nucleares de Irán durante años, e idealmente para siempre. "
    "La mejor manera de que esto ocurra sería que el régimen fuera reemplazado "
    "por una democracia centrada en mejorar la vida de su pueblo y en convivir "
    "en paz con sus vecinos."
)


def pick_device():
    """Select best available device, preferring Intel XPU."""
    if hasattr(torch, "xpu") and torch.xpu.is_available():
        return "xpu", torch.bfloat16, "sdpa"
    if torch.cuda.is_available():
        try:
            import flash_attn  # noqa: F401
            return "cuda", torch.bfloat16, "flash_attention_2"
        except ImportError:
            return "cuda", torch.bfloat16, "sdpa"
    return "cpu", torch.float32, "sdpa"


def main():
    device, dtype, attn = pick_device()
    print(f"Device: {device}  dtype: {dtype}  attn: {attn}", file=sys.stderr)

    # Voice preset
    if not VOICE_PRESET.exists():
        print(f"Error: voice preset not found: {VOICE_PRESET}", file=sys.stderr)
        sys.exit(1)
    print(f"Using voice preset: {VOICE_PRESET.stem}", file=sys.stderr)

    # Load processor and model
    print(f"Loading {MODEL_ID}...", file=sys.stderr)
    processor = VibeVoiceStreamingProcessor.from_pretrained(MODEL_ID)

    if device in ("xpu", "mps"):
        model = VibeVoiceStreamingForConditionalGenerationInference.from_pretrained(
            MODEL_ID,
            torch_dtype=dtype,
            attn_implementation=attn,
            device_map=None,
        )
        model.to(device)
    elif device == "cuda":
        model = VibeVoiceStreamingForConditionalGenerationInference.from_pretrained(
            MODEL_ID,
            torch_dtype=dtype,
            device_map="cuda",
            attn_implementation=attn,
        )
    else:
        model = VibeVoiceStreamingForConditionalGenerationInference.from_pretrained(
            MODEL_ID,
            torch_dtype=dtype,
            device_map="cpu",
            attn_implementation=attn,
        )

    model.eval()
    model.set_ddpm_inference_steps(num_steps=5)

    # Load voice embeddings
    target_device = device if device != "cpu" else "cpu"
    all_prefilled_outputs = torch.load(
        str(VOICE_PRESET), map_location=target_device, weights_only=False
    )

    # Prepare input
    text = SPANISH_TEXT.replace("\u2018", "'").replace("\u201c", '"').replace("\u201d", '"')
    inputs = processor.process_input_with_cached_prompt(
        text=text,
        cached_prompt=all_prefilled_outputs,
        padding=True,
        return_tensors="pt",
        return_attention_mask=True,
    )
    for k, v in inputs.items():
        if torch.is_tensor(v):
            inputs[k] = v.to(target_device)

    # Generate
    print("Generating Spanish audio...", file=sys.stderr)
    start = time.time()
    outputs = model.generate(
        **inputs,
        max_new_tokens=None,
        cfg_scale=1.5,
        tokenizer=processor.tokenizer,
        generation_config={"do_sample": False},
        verbose=True,
        all_prefilled_outputs=copy.deepcopy(all_prefilled_outputs),
    )
    gen_time = time.time() - start

    # Metrics
    if outputs.speech_outputs and outputs.speech_outputs[0] is not None:
        sample_rate = 24_000
        n_samples = outputs.speech_outputs[0].shape[-1]
        duration = n_samples / sample_rate
        rtf = gen_time / duration if duration > 0 else float("inf")
        print(f"Audio duration: {duration:.1f}s", file=sys.stderr)
        print(f"Generation time: {gen_time:.1f}s  (RTF: {rtf:.2f}x)", file=sys.stderr)
    else:
        print("Warning: no audio output generated", file=sys.stderr)
        return

    # Save
    out_path = OUTPUT_DIR / "demo_realtime_es.wav"
    processor.save_audio(outputs.speech_outputs[0], output_path=str(out_path))
    print(f"Saved: {out_path}", file=sys.stderr)


if __name__ == "__main__":
    main()
