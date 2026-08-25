# Plan: migrating to transformers 5.x

Status as of 2026-08-24: **not done, not currently worth doing.** This file records what
breaks, why, and the monkey-patch shape required, so the work can be picked up later without
re-deriving it.

Current pinned stack: `transformers==4.51.3`, `vibevoice==0.0.1`, `torch==2.12.1+xpu`.

## Why we are pinned

`vibevoice` declares a hard equality pin, not a floor:

```
vibevoice 0.0.1 -> Requires-Dist: transformers==4.51.3
```

`vibevoice` has exactly **one** release on PyPI (`0.0.1`). There is no newer version coming to
lift the pin, so any migration is our patch to carry.

## What upstream did NOT solve

The VibeVoice GitHub README advertises "Integration with Hugging Face Transformers"
(2026-03-06). That integration is **ASR only**. transformers 5.15.1 ships:

- `VibeVoiceAsrForConditionalGeneration` — speech -> text
- `VibeVoiceAcousticTokenizer*` — the shared audio codec

There is **no TTS generation model upstream**. `models/vibevoice/` does not exist in 4.51.3,
4.57.6, or 5.15.1. The direction that landed is the opposite of what this project needs, so
upgrading transformers does not let us drop the `vibevoice` package.

(Note: `VibeVoiceAcousticTokenizerConfig` references the `microsoft/VibeVoice-1.5B` checkpoint,
because the codec is shared between the ASR and TTS models. Part of our stack is therefore
already maintained upstream; the diffusion head and LM wiring are not.)

## Breakage inventory (verified against the 5.15.1 wheel)

### 1. BLOCKER — `Qwen2TokenizerFast` no longer exists

```python
# vibevoice/modular/modular_vibevoice_text_tokenizer.py:7
from transformers.models.qwen2.tokenization_qwen2_fast import Qwen2TokenizerFast
```

v5 collapsed the slow/fast tokenizer split — **61** `tokenization_*_fast.py` files down to
**2**. `transformers/models/qwen2/tokenization_qwen2_fast.py` is gone; `Qwen2Tokenizer` is now
the Rust-backed implementation. This is an unconditional `ImportError` at import time.

It matters because the processor instantiates the fast class specifically:

```python
# vibevoice/processor/vibevoice_processor.py:100
tokenizer = VibeVoiceTextTokenizerFast.from_pretrained(...)
```

### 2. BLOCKER — the constructor signature changed

An alias is not sufficient. `VibeVoiceTextTokenizerFast.__init__` forwards keyword arguments
that v5 no longer accepts:

| vibevoice passes | v5 `Qwen2Tokenizer.__init__` takes |
|---|---|
| `vocab_file=`  | `vocab=`   |
| `merges_file=` | `merges=`  |
| `tokenizer_file=` | *(no such parameter)* |

Note that `vocab_files_names` still uses the keys `vocab_file` / `merges_file` for **file
resolution** in `from_pretrained`, so the mapping between resolved files and `__init__`
parameters is the part that needs runtime verification — the wrong shim silently yields a
tokenizer built from v5's `{"<|endoftext|>": 0}` default vocab rather than the real one.

**This is the failure mode to watch for: a tokenizer that constructs successfully but encodes
wrongly.** Guard the migration with an assertion on vocab size and on the three special-token
IDs before trusting any audio output.

### 3. Generation / cache API churn

Per the v5 migration guide, affecting `VibeVoiceForConditionalGenerationInference.generate()`:

- Default KV cache class is now **model-defined** rather than always `DynamicCache`.
- Generation parameters are no longer readable from `model.config` — must use
  `model.generation_config`.
- Old generate output aliases removed; only four output classes remain.

vibevoice overrides `generate()` and imports `GenerationMixin`, `LogitsProcessor`,
`LogitsProcessorList`, `StoppingCriteriaList` from `transformers.generation`. Those names all
still exist in 5.15.1, but the behaviour around them changed. Needs runtime testing.

### 4. Non-issues (verified, no action needed)

- `ALL_PARALLEL_STYLES` — removed from `modeling_utils`, but vibevoice **already** shims it:
  ```python
  # modeling_vibevoice.py:29
  if not hasattr(modeling_utils, "ALL_PARALLEL_STYLES") or modeling_utils.ALL_PARALLEL_STYLES is None:
      modeling_utils.ALL_PARALLEL_STYLES = ["tp", "none", "colwise", "rowwise"]
  ```
- `_supports_static_cache`, `_supports_quantized_cache` — gone from v5, but these are plain
  class attributes. Setting an attribute transformers no longer reads is harmless.
- These symbols all still exist in 5.15.1: `Qwen2Config`, `LlamaRMSNorm`,
  `FlashAttentionKwargs`, `GradientCheckpointingLayer`, `PreTrainedModel`, `PretrainedConfig`,
  `ACT2FN`, `FeatureExtractionMixin`, `CausalLMOutput`, `BaseModelOutputWithPast`.
- Methods the tokenizer subclass calls all survive: `add_special_tokens`,
  `convert_tokens_to_ids`, `from_pretrained`, `save_pretrained`, `batch_decode`.

## The monkey-patch shape

Install a shim module into `sys.modules` **before** `vibevoice` is imported, so its
`from transformers.models.qwen2.tokenization_qwen2_fast import Qwen2TokenizerFast` resolves.
Sketch — the kwarg mapping is the part that needs testing against a real checkpoint:

```python
# tf5_shim.py — import this before any vibevoice import
import sys, types
import transformers
from transformers.models.qwen2.tokenization_qwen2 import Qwen2Tokenizer

class Qwen2TokenizerFast(Qwen2Tokenizer):
    """v4-compatible constructor over the v5 consolidated tokenizer."""
    def __init__(self, vocab_file=None, merges_file=None, tokenizer_file=None, **kw):
        # v5 renamed these; tokenizer_file has no v5 equivalent and is dropped.
        if vocab_file is not None:
            kw.setdefault("vocab", vocab_file)
        if merges_file is not None:
            kw.setdefault("merges", merges_file)
        super().__init__(**kw)

_m = types.ModuleType("transformers.models.qwen2.tokenization_qwen2_fast")
_m.Qwen2TokenizerFast = Qwen2TokenizerFast
sys.modules["transformers.models.qwen2.tokenization_qwen2_fast"] = _m
```

Then bypass the metadata pin. Either install with `--no-deps`, or patch the installed
`vibevoice-0.0.1.dist-info/METADATA` to relax `transformers==4.51.3`. Both are fragile against
reinstall — see the "regression trap" note below.

## Verification gate — do not skip

A tokenizer or cache bug here produces **plausible-sounding but wrong audio**, not a crash.
Before accepting the migration:

1. Assert `len(tokenizer)` matches the 4.51.3 value for the same checkpoint.
2. Assert the three special-token IDs (`<|vision_start|>`, `<|vision_end|>`, `<|vision_pad|>`)
   match the 4.51.3 values.
3. Generate the same fixed script under both stacks with the same seed and compare the audio
   — byte-identical is unlikely, but gross divergence in length or content means it is broken.

## Regression trap

Same shape as the `torch 2.13.0+xpu` problem documented in the README: any later
`pip install -r requirements.txt` will snap `transformers` back to `4.51.3` and silently
discard the migration. If this is ever adopted, pin the new version in `requirements.txt` and
make the shim import unconditional in `article2pod.py`.

## Recommendation

Not worth doing today. The `vibevoice` package must be carried regardless, the upstream
integration does not cover TTS, and the failure modes are silent rather than loud. Revisit if:

- VibeVoice **TTS** lands in transformers upstream (then drop the `vibevoice` package entirely
  rather than shimming it), or
- a security fix or a needed feature lands only in 5.x, or
- `vibevoice` publishes a release past `0.0.1`.

The intermediate step — `transformers==4.57.6`, latest 4.x — is a much smaller move that keeps
every symbol vibevoice imports and adds a real XPU benefit (see README, "Optional: transformers
4.57.6").
