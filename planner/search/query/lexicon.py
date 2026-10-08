"""Word lists of the query box, and the phrase matcher that reads them.

Kind terms are seeded from the iD tagging schema (ISC licence, (c) iD contributors,
https://github.com/openstreetmap/id-tagging-schema); `python lexicon.py build` rewrites
lexicon/kinds.json from lexicon/src/ and lexicon/kinds.yaml. lexicon/src/ is not committed; fetch
presets.json and translations/{en,de,fr,it,es,nl}.json from
https://cdn.jsdelivr.net/npm/@openstreetmap/id-tagging-schema@6.19.2/dist/. All other lists are hand-written in
lexicon/words.yaml. Lookup is a union over all languages: the box does not know the language.

    WORDS[category][value][lang] -> [term]      hand-written lists (words.yaml)
    KIND_TERMS[lang][kind] -> [term]            kind terms (kinds.json; en de fr it, plus es nl)
    TABLES[category] -> Table                   a matcher over all languages
"""

from __future__ import annotations

import functools
import json
import re
import sys
import unicodedata
from collections import defaultdict
from pathlib import Path
from typing import Any

import snowballstemmer
import yaml
from rapidfuzz import process
from rapidfuzz.distance import OSA

import schema
import words

LANGS = ("en", "de", "fr", "it")
HERE = Path(__file__).parent / "lexicon"

_TR = str.maketrans({"’": "'", "‘": "'", "`": "'", "–": "-", "—": "-", "œ": "oe", "æ": "ae",
                     "ø": "o", "ł": "l"})


def fold(s: str) -> str:
    """Lower case without accents: the form every match compares."""
    s = unicodedata.normalize("NFKD", s.casefold().translate(_TR))
    return "".join(c for c in s if not unicodedata.combining(c))


# "l'arrivée" and "dell'Abetone" match as two words; so do the halves of "mountain-bike".
_ELISION = re.compile(r"^(l|d|j|n|s|c|m|t|qu|dell|dall|nell|sull|all|un|mezz|quest|nessun)'(.+)$")


def toks(text: str) -> list[tuple[str, int]]:
    """(folded token, index of its word in words.split(text))."""
    out = []
    ws = words.split(text)
    for wi, (w, _, e) in enumerate(ws):
        if w in "'’" and out and wi and ws[wi - 1][2] == ws[wi][1]:
            out[-1] = (out[-1][0] + "'", out[-1][1])        # "un'" in a word list
            continue
        f = fold(w)
        m = _ELISION.match(f)
        parts = [m.group(1) + "'", m.group(2)] if m else [f]
        if parts[-1].endswith("'s") and len(parts[-1]) > 3:       # "tonight's"
            parts[-1:] = [parts[-1][:-2], "'s"]
        for part in parts:
            subs = [x for x in part.split("-") if x] if len(part) > 1 else [part]
            out.extend((x, wi) for x in subs)
    return out


_STEMMERS = [snowballstemmer.stemmer(n) for n in ("english", "german", "french", "italian")]


@functools.lru_cache(maxsize=65536)
def stems(w: str) -> frozenset[str]:
    return frozenset(s for s in (st.stemWord(w) for st in _STEMMERS) if len(s) >= 3)


def _max_edits(n: int, typos: int | None) -> int:
    """Edits allowed in a word of n letters: none below `typos` letters, 2 from 8 letters."""
    return 0 if typos is None or n < typos else 1 if n < 8 else 2


def word_q(tok: str, w: str, typos: int | None = 4, stem: bool = True) -> int | None:
    """0 same word, 1 same stem, 2+ edits; None if the words differ."""
    if tok == w:
        return 0
    if stem and len(tok) >= 4 and len(w) >= 3 and stems(tok) & stems(w):
        return 1
    k = _max_edits(min(len(tok), len(w)), typos)
    if k and (d := OSA.distance(tok, w, score_cutoff=k)) <= k:
        return 1 + d
    return None


def rank(m: tuple[int, Any, int, int]) -> tuple[int, int, int]:
    return m[3], -m[2], m[0]


class Table:
    """Phrases of one category and their values; matches whole phrases over folded tokens.
    `typos` is the shortest word that may carry a typo (None: no typos)."""

    def __init__(self, pairs, typos: int | None = 4, stem: bool = True):
        self.typos, self.stem = typos, stem
        self.by_first: dict[str, list[tuple[tuple[str, ...], Any]]] = defaultdict(list)
        for term, value in pairs:
            ws = tuple(t for t, _ in toks(str(term)))
            if ws and (ws, value) not in self.by_first[ws[0]]:
                self.by_first[ws[0]].append((ws, value))
        self.firsts = list(self.by_first)
        self.by_stem: dict[str, set[str]] = defaultdict(set)
        for w in self.firsts:
            for s in stems(w):
                self.by_stem[s].add(w)

    def _firsts(self, tok: str) -> list[tuple[str, int]]:
        """First words that `tok` may be, best first. Sorted, so that ties never depend on
        set order (PYTHONHASHSEED)."""
        out = {tok: 0} if tok in self.by_first else {}
        if len(tok) >= 4 and self.stem:
            for s in stems(tok):
                for w in self.by_stem.get(s, ()):
                    out.setdefault(w, 1)
        if k := _max_edits(len(tok), self.typos):
            for w, d, _ in process.extract(tok, self.firsts, scorer=OSA.distance,
                                           score_cutoff=k, limit=None):
                if d <= _max_edits(min(len(w), len(tok)), self.typos):
                    out.setdefault(w, 1 + d)
        # A doubled key on a short word: "Taag", "daay".
        single = re.sub(r"(.)\1+", r"\1", tok)
        if self.typos and len(tok) >= 4 and single != tok and single in self.by_first:
            out.setdefault(single, 2)
        return sorted(out.items(), key=lambda x: (x[1], x[0]))

    def match_at(self, ts: list[str], i: int) -> tuple[int, Any, int, int] | None:
        """(length, value, worst quality, exact words) of the best phrase at ts[i].

        Best is most words matched without typos, then fewest typos, then longest: a fuzzy
        "water taps" must not swallow "water gaps". A typo equally close to two values keeps
        the one with the typed first letter, and else matches nothing: a wrong weekday is a
        silent error, an ignored word is not."""
        found = []
        for w, q in self._firsts(ts[i]):
            for ws, value in self.by_first[w]:
                n = len(ws)
                if i + n > len(ts):
                    continue
                qs = [q]
                for a, b in zip(ts[i + 1:i + n], ws[1:]):
                    r = word_q(a, b, self.typos, self.stem)
                    if r is None:
                        break
                    qs.append(r)
                else:
                    found.append(((n, value, max(qs), sum(x <= 1 for x in qs)), ws[0]))
        if not found:
            return None
        top = max(rank(m) for m, _ in found)
        best = [(m, w) for m, w in found if rank(m) == top]
        if best[0][0][2] >= 2 and len({m[1] for m, _ in best}) > 1:
            best = [(m, w) for m, w in best if w[:1] == ts[i][:1]]
            if len({m[1] for m, _ in best}) != 1:
                return None
        return best[0][0]

    def whole(self, ts: list[str]) -> tuple[Any, int] | None:
        """(value, quality) when the tokens are exactly one phrase."""
        m = self.match_at(ts, 0) if ts else None
        return (m[1], m[2]) if m and m[0] == len(ts) else None


class _Loader(yaml.SafeLoader):
    """YAML without booleans: "on", "no" and "off" are words here."""


_Loader.yaml_implicit_resolvers = {
    k: [r for r in v if r[0] != "tag:yaml.org,2002:bool"]
    for k, v in yaml.SafeLoader.yaml_implicit_resolvers.items()}


def _yaml(name: str) -> dict:
    return yaml.load((HERE / name).read_text(), Loader=_Loader)


def _load_words() -> dict:
    return _yaml("words.yaml")


def _load_kinds() -> dict:
    f = HERE / "kinds.json"
    return json.loads(f.read_text()) if f.exists() else {"terms": {}}


WORDS: dict[str, dict[Any, dict[str, list[str]]]] = _load_words()
_KINDS = _load_kinds()
KIND_TERMS: dict[str, dict[str, list[str]]] = _KINDS["terms"]


def pairs(category: str, langs=None) -> list[tuple[str, Any]]:
    """(term, value) over the given languages of a words.yaml category."""
    return [(t, v) for v, per in WORDS[category].items() for lang, ts in per.items()
            if langs is None or lang in langs for t in ts]


def compound(tok: str, head: str, tail: str) -> tuple[Any, Any] | None:
    """(head value, tail value) of a German compound such as "Wasserlücke" or "Rennradtour":
    a phrase of table `head`, an optional linking "s", "n" or "en", then a word of `tail`."""
    for cut in range(len(tok) - 3, 2, -1):
        t = TABLES[tail].whole([tok[cut:]])
        if not t:
            continue
        for link in ("", "s", "n", "en", "es"):
            stem = tok[:cut - len(link)] if link and tok[:cut].endswith(link) else tok[:cut]
            if (link and stem == tok[:cut]) or len(stem) < 3:
                continue
            h = TABLES[head].whole([stem])
            if h and h[1] <= 1:
                return h[0], t[0]
    return None


def kind_pairs() -> list[tuple[str, str]]:
    return [(t, k) for per in KIND_TERMS.values() for k, ts in per.items() for t in ts]


# Closed lists spell out their inflections and do not stem. A typo in "a" or "la" is another
# word, so their short words match exactly; words from 5 letters may carry a typo.
_CLOSED = {"number", "number_and", "ordinal_suffix", "stop", "nearest", "every", "qualifier",
           "here", "stretch_filler", "length_word", "day_value", "prep", "limit", "threshold",
           "weekday", "weekday_short", "avoid", "route_noun", "trip_noun", "compound_tail",
           "top_of"}
_TYPOS = {"day_value", "qualifier", "threshold", "limit", "weekday", "compound_tail"}
TABLES: dict[str, Table] = {
    c: Table(pairs(c), typos=(5 if c in _TYPOS else None) if c in _CLOSED else 4,
             stem=c not in _CLOSED)
    for c in WORDS}
# Short corrections need a language; exact category words still work in every language.
TABLES["kind"] = Table(kind_pairs(), typos=5)
TABLES["kind_exact"] = Table(kind_pairs(), typos=None)
for lang, kinds in KIND_TERMS.items():
    TABLES[f"kind_{lang}"] = Table((term, kind) for kind, terms in kinds.items() for term in terms)
TABLES["split_verb"] = Table(((t, "split") for ts in WORDS["intent"]["split"].values()
                              for t in ts), typos=5)


@functools.cache
def tokens_of(category: str) -> frozenset[str]:
    """Every folded token that appears in a phrase of the category."""
    return frozenset(t for term, _ in pairs(category) for t, _ in toks(str(term)))


# Tokens that carry no value inside a span.
STOP = tokens_of("stop") | {fold(a) for a in schema.ARTICLES}

# A word that names another kind in one language: Italian "bar" is a café, "market" a
# minimarket. It applies when the sentence votes for that language (lang_of).
SENSE = {"it": {"bar": "cafe", "market": "convenience"}}


def _lang_tokens() -> dict[str, set[str]]:
    per = {lang: set() for lang in LANGS}
    for cat in WORDS.values():
        for by_lang in cat.values():
            for lang, ts in by_lang.items():
                per[lang].update(t for x in ts for t, _ in toks(str(x)))
    for lang in LANGS:
        per[lang].update(t for ts in KIND_TERMS.get(lang, {}).values() for x in ts
                         for t, _ in toks(x))
    # only words of one language vote
    return {lang: {t for t in ts if sum(t in o for o in per.values()) == 1}
            for lang, ts in per.items()}


_LANG_TOKENS = _lang_tokens()


def lang_of(text: str) -> str | None:
    """The language most words of the text belong to alone, or None on a tie."""
    votes = sorted(((sum(t in ts for t, _ in toks(text)), lang)
                    for lang, ts in _LANG_TOKENS.items()), reverse=True)
    return votes[0][1] if votes[0][0] > votes[1][0] else None


# --- build -----------------------------------------------------------------------------------

def build() -> None:
    """Writes lexicon/kinds.json from the iD translations and lexicon/kinds.yaml."""
    spec = _yaml("kinds.yaml")
    assert set(spec) == set(schema.KINDS), set(spec) ^ set(schema.KINDS)
    version = json.loads((HERE / "src/package.json").read_text())["version"]
    tr = {lang: json.loads((HERE / f"src/{lang}.json").read_text())[lang]["presets"]["presets"]
          for lang in (*LANGS, "es", "nl")}
    stop = STOP
    hand: dict[str, str] = {}                            # folded term -> kind, written by hand
    named: dict[str, set[str]] = defaultdict(set)        # folded term -> kinds whose preset name
    claims: dict[str, set[str]] = defaultdict(set)       # folded term -> kinds that list it
    found: dict[tuple[str, str], list[tuple[str, str]]] = defaultdict(list)  # (lang, kind)
    for kind, s in spec.items():
        drop = {fold(d) for d in s.get("drop", [])}
        for lang, ts in s.get("add", {}).items():
            for t in ts:
                f = fold(t)
                assert hand.get(f, kind) == kind, f"{t!r} written for {hand[f]} and {kind}"
                hand[f] = kind
                found[lang, kind].append((t, f))
        for p in s.get("presets", []):
            for lang, presets in tr.items():
                e = presets.get(p)
                if not e:
                    continue
                name = e.get("name", "")
                # es and nl terms are too loose for a union lookup ("vía", "alto"); names only.
                loose = s.get("names_only") or lang not in LANGS
                ts = [name] + ([] if loose else (e.get("terms") or "").split(","))
                for t in (x.strip() for x in ts):
                    f = fold(t)
                    long = len(f.split()) > 3 or re.search(r"\d|[/()]", f)
                    if not f or f in drop or f in stop or long:
                        continue
                    if t == name:
                        named[f].add(kind)
                    claims[f].add(kind)
                    found[lang, kind].append((t.lower() if t != name else t, f))
    out: dict[str, dict[str, list[str]]] = {lang: {} for lang in (*LANGS, "es", "nl")}
    lost = []
    for (lang, kind), ts in sorted(found.items()):
        keep = []
        for t, f in ts:
            owner = hand.get(f) or (next(iter(named[f])) if len(named[f]) == 1 else None)
            if owner is None and len(claims[f]) == 1:
                owner = kind
            if owner == kind and f not in {fold(x) for x in keep}:
                keep.append(t)
            elif owner is None:
                lost.append(f"{lang}:{t} {sorted(claims[f])}")
        out[lang][kind] = keep
    doc = {"source": f"@openstreetmap/id-tagging-schema {version} (ISC licence), "
                     "filtered and extended by lexicon/kinds.yaml",
           "terms": out}
    (HERE / "kinds.json").write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n")
    print(f"kinds.json from id-tagging-schema {version}; dropped {len(set(lost))} contested terms")
    for x in sorted(set(lost)):
        print("  contested:", x)


if __name__ == "__main__" and sys.argv[1:] == ["build"]:
    build()
