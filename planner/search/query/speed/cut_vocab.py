"""Cut the mmBERT vocabulary to six Latin-script languages and slice the embedding to match.

Keeps every added/special token, every byte-fallback token, single characters (and their
word-initial form) for Latin, Latin-1 and Latin Extended-A, and every token that appears
on the BPE merge path of a corpus word. Merge paths are replayed exactly, so a corpus word
tokenizes the same after the cut, and any other text splits into smaller kept pieces or
bytes, never into <unk>.

    .venv/bin/python cut_vocab.py --top 50000 --out work/cut-50k
    .venv/bin/python cut_vocab.py --top 3000 --elision --out work/cut-3k

Corpus: the top words of hermitdave/FrequencyWords 2018 `<lang>_50k.txt` (downloaded into
work/corpus), sentences/domain_words.txt and sentences/places.txt. The check runs on
sentences/heldout_tatoeba.tsv (held out) and sentences/queries.tsv (shares domain words).
"""
import argparse
import json
import statistics
import urllib.request

import torch
from tokenizers import Tokenizer

from common import BASE_MODEL, HELDOUT, QUERIES, SPEED, WORK, load_encoder, load_sentences, model_dir

LANGS = ["en", "de", "fr", "it", "es", "nl"]
FREQ_URL = "https://raw.githubusercontent.com/hermitdave/FrequencyWords/master/content/2018/{l}/{l}_50k.txt"
LATIN = [chr(c) for c in range(0x180)] + [chr(c) for c in range(0x2010, 0x2030)] + ["€", "°", "²", "³"]
ELISIONS = {"fr": ["l'", "d'"], "it": ["l'", "d'", "dell'", "all'"]}


def corpus(top, title, elision):
    words = []
    for lang in LANGS:
        path = WORK / "corpus" / f"{lang}_50k.txt"
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            urllib.request.urlretrieve(FREQ_URL.format(l=lang), path)
        top_words = [ln.split(" ")[0] for ln in path.read_text(encoding="utf-8").splitlines()[:top]]
        words += top_words
        if title:
            words += [w[:1].upper() + w[1:] for w in top_words]
        if elision:
            words += [e + w for e in ELISIONS.get(lang, []) for w in top_words if w[:1] in "aeiouhàâéèêîôû"]
    domain = (SPEED / "sentences" / "domain_words.txt").read_text(encoding="utf-8").split()
    words += domain + [w[:1].upper() + w[1:] for w in domain]
    words += (SPEED / "sentences" / "places.txt").read_text(encoding="utf-8").splitlines()
    return words


def bpe_path(piece, vocab, ranks):
    """Replays the HF BPE merge loop (lowest rank first, leftmost on ties); returns every token seen."""
    syms = []
    for ch in piece:
        syms += [ch] if ch in vocab else [f"<0x{b:02X}>" for b in ch.encode()]
    seen = set(syms)
    while len(syms) > 1:
        best = None
        for i in range(len(syms) - 1):
            r = ranks.get((syms[i], syms[i + 1]))
            if r is not None and (best is None or r < best[0]):
                best = (r, i)
        if best is None:
            break
        i = best[1]
        syms[i : i + 2] = [syms[i] + syms[i + 1]]
        seen.add(syms[i])
    return syms, seen


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default=BASE_MODEL)
    ap.add_argument("--out", required=True)
    ap.add_argument("--top", type=int, default=50000, help="words per language")
    ap.add_argument("--no-title", action="store_true", help="skip the Capitalised variant of each word")
    ap.add_argument("--elision", action="store_true", help="add French/Italian l'/d' forms")
    args = ap.parse_args()

    src = model_dir(args.src)
    out = SPEED / args.out if not args.out.startswith("/") else __import__("pathlib").Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    tj = json.loads((src / "tokenizer.json").read_text())
    vocab = tj["model"]["vocab"]
    merges = [m.split(" ") if isinstance(m, str) else m for m in tj["model"]["merges"]]
    ranks = {(a, b): i for i, (a, b) in enumerate(merges)}
    orig_tok = Tokenizer.from_file(str(src / "tokenizer.json"))

    keep = {a["content"] for a in tj["added_tokens"]}
    keep |= {t for t in vocab if len(t) == 6 and t.startswith("<0x")}
    keep |= {t for c in LATIN for t in (c, "▁" + c) if t in vocab}
    keep.add("▁")
    words = corpus(args.top, not args.no_title, args.elision)
    mismatch = 0
    for n, w in enumerate(words):
        tokens = []
        for piece, _ in orig_tok.pre_tokenizer.pre_tokenize_str(orig_tok.normalizer.normalize_str(w)):
            final, seen = bpe_path(piece, vocab, ranks)
            tokens += final
            keep |= seen
        if n % 50 == 0:  # spot-check the replay against the real tokenizer
            mismatch += tokens != orig_tok.encode(w, add_special_tokens=False).tokens
    assert mismatch == 0, f"BPE replay differs from tokenizers on {mismatch} words"

    kept = sorted(vocab[t] for t in keep)
    old2new = {old: new for new, old in enumerate(kept)}
    inv = {i: t for t, i in vocab.items()}
    new_vocab = {inv[old]: new for new, old in enumerate(kept)}
    new_merges = [f"{a} {b}" for a, b in merges if a in new_vocab and b in new_vocab and a + b in new_vocab]
    tj["model"]["vocab"] = new_vocab
    tj["model"]["merges"] = new_merges
    for a in tj["added_tokens"]:
        a["id"] = old2new[a["id"]]
    for st in tj["post_processor"]["special_tokens"].values():
        st["ids"] = [old2new[i] for i in st["ids"]]
    (out / "tokenizer.json").write_text(json.dumps(tj, ensure_ascii=False))
    tc = json.loads((src / "tokenizer_config.json").read_text())
    tc["added_tokens_decoder"] = {str(old2new[int(k)]): v for k, v in tc["added_tokens_decoder"].items()}
    (out / "tokenizer_config.json").write_text(json.dumps(tc, ensure_ascii=False, indent=1))
    (out / "special_tokens_map.json").write_text((src / "special_tokens_map.json").read_text())
    (out / "kept_ids.json").write_text(json.dumps(kept))

    orig = load_encoder(src)
    cut = load_encoder(src)
    emb = orig.embeddings.tok_embeddings
    cut.embeddings.tok_embeddings = torch.nn.Embedding.from_pretrained(
        emb.weight.data[torch.tensor(kept)].clone(), freeze=False, padding_idx=old2new[emb.padding_idx or 0])
    for k in ["pad_token_id", "bos_token_id", "eos_token_id", "cls_token_id", "sep_token_id", "mask_token_id"]:
        if getattr(cut.config, k, None) is not None:
            setattr(cut.config, k, old2new[getattr(cut.config, k)])
    cut.config.vocab_size = len(kept)
    cut.save_pretrained(out)

    # Check: same tokens on held-out text, never <unk>, identical hidden states when tokens agree.
    cut_tok = Tokenizer.from_file(str(out / "tokenizer.json"))
    unk = cut_tok.token_to_id("<unk>")
    report = {"kept_tokens": len(kept), "merges": len(new_merges), "corpus_words": len(words),
              "params_total": sum(p.numel() for p in cut.parameters()),
              "params_embedding": cut.embeddings.tok_embeddings.weight.numel(), "agreement": {}}
    same_ids, max_diff = [], 0.0
    for name, path in [("tatoeba", HELDOUT), ("queries", QUERIES)]:
        per_lang = {}
        for lang, s in load_sentences(path):
            a, b = orig_tok.encode(s), cut_tok.encode(s)
            assert unk not in b.ids, f"<unk> in cut tokenization of {s!r}"
            eq = a.tokens == b.tokens
            n = per_lang.setdefault(lang, [0, 0, 0, 0])
            n[0] += eq
            n[1] += 1
            n[2] += len(a.ids)
            n[3] += len(b.ids)
            if eq and len(same_ids) < 60:
                same_ids.append((a.ids, b.ids))
        report["agreement"][name] = {l: f"{v[0]}/{v[1]} same, tokens {v[2]}->{v[3]}" for l, v in per_lang.items()}
    with torch.no_grad():
        for a_ids, b_ids in same_ids:
            ha = orig(input_ids=torch.tensor([a_ids])).last_hidden_state
            hb = cut(input_ids=torch.tensor([b_ids])).last_hidden_state
            max_diff = max(max_diff, (ha - hb).abs().max().item())
    report["hidden_state_max_abs_diff"] = max_diff
    lens = {}
    for lang, s in load_sentences(QUERIES):
        lens.setdefault(lang, []).append(len(cut_tok.encode(s).ids))
    report["query_token_counts"] = {
        l: {"min": min(v), "median": statistics.median(v), "max": max(v)} for l, v in lens.items()}
    (out / "cut_report.json").write_text(json.dumps(report, indent=1, ensure_ascii=False))
    print(json.dumps(report, indent=1, ensure_ascii=False))


if __name__ == "__main__":
    main()
