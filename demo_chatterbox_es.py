"""Demo: Chatterbox Multilingual TTS — Spanish news with voice cloning on Intel XPU."""

import os
import sys
from pathlib import Path

import torch
import scipy.io.wavfile as wavfile
import numpy as np
from chatterbox.mtl_tts import ChatterboxMultilingualTTS

# Voice reference for cloning. Override with CHATTERBOX_VOICE_REF=/path/to/sample.m4a
VOICE_REF = os.environ.get(
    "CHATTERBOX_VOICE_REF",
    str(Path.home() / "Programming" / "Sonoro" / "assets" / "spansample.m4a"),
)
OUTPUT_DIR = Path("output")
OUTPUT_DIR.mkdir(exist_ok=True)

SPANISH_TEXT = (
    "MUCHAS COSAS hacen odioso al régimen iraní, pero lo que lo convierte en "
    "especialmente peligroso es su búsqueda de armas nucleares. Su promesa de "
    "no fabricar una bomba quedó desmentida por su determinación de enriquecer "
    "uranio a grado militar."    
    "Eso ha sustentado durante mucho tiempo los intentos del régimen de "
    "intimidar a sus vecinos y amenazar la supervivencia de Israel."
    "Si la guerra desatada por Estados Unidos e Israel el 28 de febrero ha de "
    "considerarse siquiera un éxito limitado, debe entonces retrasar las "
    "ambiciones nucleares de Irán durante años, e idealmente para siempre."
    "La mejor manera de que esto ocurra sería que el régimen fuera reemplazado "
    "por una democracia centrada en mejorar la vida de su pueblo y en convivir "
    "en paz con sus vecinos. Un gobierno así representaría la menor amenaza. "
    "Sin embargo, una guerra aérea difícilmente logrará generar tal renovación. "
    "Incluso podría empeorar la situación."
    "El régimen seguramente ha comprendido que ser una potencia de umbral te "
    "convierte en un objetivo y que, para que un programa nuclear ofrezca alguna "
    "protección, debe llegar hasta el final. Se cree que el nuevo líder supremo, "
    "Mojtaba Jamenei, está más ansioso que su difunto padre y predecesor por "
    "obtener una bomba, y tras la muerte de su familia es probable que desee "
    "venganza. En Irán, esos argumentos pueden eclipsar el hecho de que los "
    "misiles y bombas estadounidenses e israelíes han causado un gran daño a la "
    "economía. Aun sabiendo que cualquier trabajo futuro en un arma nuclear será "
    "respondido con una potencia de fuego extraordinaria, Jamenei podría tolerar "
    "el riesgo."
)


def main():
    # Pick device: prefer XPU (Intel Arc), fall back to CUDA, then CPU
    if torch.xpu.is_available():
        device = "xpu"
    elif torch.cuda.is_available():
        device = "cuda"
    else:
        device = "cpu"

    print(f"Using device: {device}", file=sys.stderr)
    print("Loading Chatterbox Multilingual model...", file=sys.stderr)
    model = ChatterboxMultilingualTTS.from_pretrained(device=device)

    print("Generating Spanish audio with voice clone...", file=sys.stderr)
    wav = model.generate(
        SPANISH_TEXT,
        language_id="es",
        audio_prompt_path=VOICE_REF,
    )

    out_path = OUTPUT_DIR / "demo_spanish.wav"
    # Move to CPU and convert to numpy for saving
    audio_np = wav.squeeze().cpu().numpy()
    # Normalize to int16 range
    audio_np = np.clip(audio_np, -1.0, 1.0)
    audio_int16 = (audio_np * 32767).astype(np.int16)
    wavfile.write(str(out_path), model.sr, audio_int16)
    print(f"Saved: {out_path}", file=sys.stderr)


if __name__ == "__main__":
    main()
