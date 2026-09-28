"""Shared paths, sentence sets and the joint tagger module for model export."""
import os
from pathlib import Path

SPEED = Path(__file__).resolve().parent
WORK = SPEED / "work"
os.environ.setdefault("HF_HOME", str(WORK / "hf"))

BASE_MODEL = "jhu-clsp/mmBERT-small"
QUERIES = SPEED / "sentences" / "queries.tsv"
HELDOUT = SPEED / "sentences" / "heldout_tatoeba.tsv"
NUM_INTENTS = 12
NUM_TAGS = 45


def model_dir(name_or_path: str) -> Path:
    """A local directory, or a hub id resolved to its snapshot in WORK/hf."""
    p = Path(name_or_path)
    if p.is_dir():
        return p
    from huggingface_hub import snapshot_download

    return Path(snapshot_download(name_or_path, allow_patterns=["*.json", "*.bin", "*.safetensors"]))


def load_sentences(path: Path = QUERIES, langs=None):
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        lang, text = line.split("\t", 1)
        if langs is None or lang in langs:
            rows.append((lang, text))
    return rows


def load_encoder(path):
    """The ModernBERT encoder of a checkpoint (an MLM checkpoint keeps only its encoder)."""
    from transformers import AutoConfig, AutoModel, AutoModelForMaskedLM

    path = model_dir(str(path))
    cfg = AutoConfig.from_pretrained(path)
    if "ModernBertForMaskedLM" in (cfg.architectures or []):
        return AutoModelForMaskedLM.from_pretrained(path, attn_implementation="sdpa").model.eval()
    return AutoModel.from_pretrained(path, attn_implementation="sdpa").eval()


def joint_tagger(checkpoint, seed: int = 0):
    """Encoder plus the sentence head (token 0) and the token head.

    Heads load from `heads.safetensors` in the checkpoint (keys intent.weight/bias,
    tags.weight/bias; their shapes set the label counts) when it exists; otherwise
    they are random with NUM_INTENTS/NUM_TAGS outputs, which is enough for speed and size.
    """
    import torch
    from torch import nn

    class JointTagger(nn.Module):
        def __init__(self, encoder, intents, tags):
            super().__init__()
            self.encoder = encoder
            hidden = encoder.config.hidden_size
            self.intent = nn.Linear(hidden, intents)
            self.tags = nn.Linear(hidden, tags)

        def forward(self, input_ids, attention_mask):
            h = self.encoder(input_ids=input_ids, attention_mask=attention_mask).last_hidden_state
            return self.intent(h[:, 0]), self.tags(h)

    torch.manual_seed(seed)
    path = model_dir(str(checkpoint))
    heads = {}
    if (path / "heads.safetensors").exists():
        from safetensors.torch import load_file

        heads = load_file(path / "heads.safetensors")
    sizes = [heads[k].shape[0] if k in heads else n for k, n in [("intent.weight", NUM_INTENTS), ("tags.weight", NUM_TAGS)]]
    model = JointTagger(load_encoder(path), *sizes).eval()
    res = model.load_state_dict(heads, strict=False)
    if heads and (res.unexpected_keys or any(k.startswith(("intent.", "tags.")) for k in res.missing_keys)):
        raise ValueError(f"heads.safetensors does not match: {res.unexpected_keys or res.missing_keys}")
    return model
