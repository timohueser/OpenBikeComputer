"""Synthetic training data for the query-box tagger, from hand-written templates per language.

    .venv/bin/python gen/generate.py --lang en --n 12000 --seed 7 --out data/train/en.jsonl
    .venv/bin/python gen/generate.py --lang de --n 12000 --seed 7 --out data/train/de.jsonl --filter

One output line is {"text", "lang", "intent", "labels", "request"}: one BIO label per
`words.split` word, and the gold request of `schema.py`. 5 % of the lines go to the dev file
(default data/dev/<lang>.jsonl). The same seed gives the same bytes.

`--filter` is the second check: it runs `decode.decode(text, intent, labels)` on each clean
example and drops it when the canonical request differs from the gold. Without the flag, the gold
is checked with `schema.validate` only (always done). The drop statistics go to stdout and to
<out>.filter.json.

Template file (templates/<lang>.yaml)
-------------------------------------
    lang: en
    decimal: "."                        decimal separator of {x}
    numbers: {word: [...], ordw: [...]} number formats, list index = value ({n:word} -> "four")
    weekdays: {<WEEKDAYS id>: {sg: [...], pl: [...]}}      {wd:sg}; its value is "$wd"
    kinds:   {<KINDS id>: {<form>: [...]}}                  forms: pl, sg, verb and any other
    stretches: {<STRETCH_KINDS id>: {<form>: [...]}}
    banks:   {<SLOT>: {<variant>: [entry, ...]}}              phrase banks of the plain slots
    points:  {<role>: {<form>: [entry, ...]}}                 point banks (FROM TO VIA NEAR
                                                              POINT BEFORE AFTER)
    templates: {<intent>: [pattern, ...]}

Surface forms of gendered languages carry a suffix: "Apotheke/f", "Campingplatz/m", "Hotel/n".
A name that starts with a lower-case elided word ("d'Annecy", "l'Alpe d'Huez", "dall'Abetone")
keeps that word in its span, and the gold name drops it.

An entry is a pattern, or [pattern, value]. Without a value, the value comes from the pattern:
WHAT gives the kind or stretch id, a point form gives {"kind": id}, {"name": text} or
{"here": true}, and NAME and IGNORED give the span text. Point forms are name, kind, here, plan,
day and along; plan, day and along entries need a value. Values may use variables: "$n", "$m/1000".

Pattern syntax (templates and entries):
    word            an O word (in a template), or a word of the span (in an entry)
    (a|b|)          one alternative at random; an empty alternative makes the group optional
    ~(m|f|n)        a word by gender: of the nearest kind word in the same phrase, else of the
                    nearest one in the sentence: ~(einen|eine|ein), ~(le plus proche|la plus proche|)
    {art:X}         a French or Italian article of the next word, by its gender and first sound
                    (ARTICLES): {art:a} gives "au", "à la" or "à l'"; an elided form ("l'", "d'",
                    "all'") joins the next word and so its span
    {SLOT}          a phrase from banks[SLOT].default, or a point from points.default
    {SLOT:v}        variant v: a bank variant, or for a point slot "role", "role.forms" or
                    ".forms" with forms joined by "+" ({TO:nach.name+kind}, {NEAR:.name})
    [a|b|SLOT=v]    an inline literal: one of the texts, labelled SLOT with YAML value v;
                    [with a pool|IGNORED] takes the text as its value
    <...>           in an entry: the words of the span; the words outside it are O
    {kind:pl}       a kind word in a form; {kind:pl/gap} limits to a subset of KIND_SUBSETS;
                    the form "any" is pl + sg + the kind terms of lexicon/kinds.json
    {stretch:pl}    a stretch word
    {name}          a sampled place name; {name:town|peak|biz|hut|lake|poi|common|kt|one}
                    (kt: towns with a kind word, "Bad Krozingen"; one: one-word names)
    {a}             the English indefinite article of the next word
    {n} {n:word}    a number variable in a format of `numbers`; see VARS for the variables

A point slot without a form list takes the forms that `FORMS` allows for its intent and slot.
The point role "stop" holds compounds that also name the stop ("Kaffeepause"): in add_point
they set kind "stop". A split with a day and no count splits that day in two.
Words outside every span are O. The request is built from the intent and the span values
(`build`), so a template never writes the request by hand.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import unicodedata
from collections import Counter, defaultdict
from pathlib import Path
from random import Random

import yaml

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import schema  # noqa: E402
import words  # noqa: E402

SHARE = {
    "places": 30, "route": 14, "none": 11, "stretches": 8, "place": 8, "add_point": 7,
    "end_day": 6, "split": 4, "remove_point": 3, "join": 2, "reverse": 2,
}
POINT_SLOTS = {"FROM", "TO", "VIA", "NEAR", "POINT", "BEFORE", "AFTER"}
FORM_WEIGHT = {"name": 10, "kind": 4, "here": 2, "plan": 1, "day": 1, "along": 1}
# Point forms a slot takes when the template does not say. NEAR a kind in `places` is Level 3.
FORMS = {
    ("route", "FROM"): ["name", "here", "kind", "plan"],
    ("route", "TO"): ["name", "kind", "plan", "day"],
    ("route", "VIA"): ["name", "kind"],
    ("place", "NEAR"): ["name", "here"],
    ("end_day", "POINT"): ["name", "kind", "along", "here"],
    ("add_point", "POINT"): ["name", "kind"],
    ("remove_point", "POINT"): ["name", "kind", "along"],
    (None, "NEAR"): ["name"],
    (None, "BEFORE"): ["name", "day", "along"],
    (None, "AFTER"): ["name", "day", "along"],
}
# Mild: rare kinds need exposure too ("Hallenbad", "gelateria").
KIND_WEIGHT = {k: 2 for k in ("water", "campsite", "lodging", "resupply", "pharmacy", "bike_shop",
                              "train_station", "supermarket", "hotel", "bakery", "food")}
KIND_SUBSETS = {
    "gap": ["water", "drinking_water", "resupply", "supermarket", "bakery", "food", "cafe",
            "sleep", "campsite", "lodging", "hotel", "bike_shop", "pharmacy", "atm", "toilets",
            "shower", "train_station", "fuel", "restaurant"],
}
NAME_MIX = {  # category -> weight, per {name:<cat>}
    None: {"towns": 45, "passes": 10, "peaks": 8, "huts": 5, "lakes": 5, "biz": 17, "common": 10},
    "town": {"towns": 80, "common": 20},
    "peak": {"peaks": 50, "passes": 50},
    "biz": {"biz": 100},
    "hut": {"huts": 100},
    "lake": {"lakes": 100},
    "poi": {"peaks": 20, "passes": 25, "huts": 15, "lakes": 10, "biz": 30},
    "common": {"common": 100},
    "kt": {"kt": 100},
    "one": {"one": 100},
}
# A town whose name holds a kind word ("Bad Krozingen", "Pont-à-Mousson", "Acqui Terme").
_KIND_TOWN = re.compile(r"(?:^|[ \-'])(?:Bad|Brunnen|Burg|See|Kirch|Markt|Hütte|Pont|Château|Lac|Chapelle|"
                        r"Ponte|Castel|Lago|Chiesa|Bridge|Castle|Church|Chapel|Pool|Well|Market|Hof|Col|"
                        r"Mont|Porto|Bagni|Terme|Fontaine|Fonte|Fontana|Quelle|Kloster|Abbey|Abbaye|Bains|"
                        r"Spa|Mühle|Moulin|Mulino|Brück|Kapelle|Turm|Torre|Strand|Plage|Camp|Station)")
# Lexicon terms that are verb phrases do not fit the noun form "any".
_VERB_START = {"get", "fill", "go", "eat", "take", "buy", "find", "refill", "top", "do", "have",
               "grab", "charge", "wash", "pump", "catch", "use", "withdraw", "spend", "stay",
               "faire", "prendre", "remplir", "acheter", "trouver", "aller", "manger", "laver",
               "recharger", "retirer", "boire", "se", "sortir", "fare", "prendere", "riempire",
               "comprare", "trovare", "andare", "mangiare", "lavare", "ricaricare", "prelevare",
               "bere", "gonfiare", "mettere", "passare", "où", "dove"}


def _steps(a: int, b: int, s: int = 1) -> list[int]:
    return list(range(a, b + 1, s))


# Number variables: values and weights. n1 = n + 1 and k2 = k1 + gap are derived.
VARS = {
    "n": (_steps(1, 14), [8, 9, 9, 9, 8, 7, 6, 4, 3, 3, 2, 2, 1, 1]),
    "d": (_steps(2, 14), [4, 6, 7, 7, 7, 6, 5, 3, 3, 3, 1, 1, 2]),
    "km": (_steps(5, 250, 5), None),
    "k1": (_steps(5, 400, 5), None),
    "r": (_steps(1, 25), [6, 8, 8, 5, 8, 3, 2, 2, 1, 5] + [1] * 15),
    "m": (_steps(100, 900, 100), None),
    "e": (_steps(100, 2000, 50), None),
    "p": (_steps(4, 20), None),
    "hh": (_steps(2, 8), [8, 7, 6, 5, 3, 2, 2]),
    "ks": (_steps(1, 30), [4, 6, 6, 5, 6, 3, 2, 3, 2, 5] + [1] * 20),
    "min": ([15, 20, 30, 40, 45, 90], None),
    "x": ([v + 0.5 for v in range(0, 12)], None),
}


# Articles by gender and by the next word: v before a vowel (French also h), s for an Italian
# masculine before s + consonant, z, gn, ps, x, y. A form ending in an apostrophe joins that word.
ARTICLES = {
    "fr": {
        "def": {"m": "le", "f": "la", "v": "l'"},
        "a": {"m": "au", "f": "à la", "v": "à l'"},
        "de": {"m": "du", "f": "de la", "v": "de l'"},
        "indef": {"m": "un", "f": "une"},
        "prep_de": {"m": "de", "f": "de", "v": "d'"},
    },
    "it": {
        "def": {"m": "il", "f": "la", "v": "l'", "s": "lo"},
        "defpl": {"m": "i", "f": "le", "mv": "gli", "s": "gli"},
        "indef": {"m": "un", "f": "una", "fv": "un'", "s": "uno"},
        "di": {"m": "del", "f": "della", "v": "dell'", "s": "dello"},
        "a": {"m": "al", "f": "alla", "v": "all'", "s": "allo"},
        "da": {"m": "dal", "f": "dalla", "v": "dall'", "s": "dallo"},
        "in": {"m": "nel", "f": "nella", "v": "nell'", "s": "nello"},
        "su": {"m": "sul", "f": "sulla", "v": "sull'", "s": "sullo"},
    },
}
_ELIDED = re.compile(r"^(?:l|d|dell|dall|all|nell|sull|qu)['’](?=\w)")


def strip_elided(name: str) -> str:
    """The name without a lower-case elided article or preposition: "d'Annecy" -> "Annecy"."""
    m = _ELIDED.match(name)
    return name[m.end():] if m else name


class TemplateError(ValueError):
    pass


# ---------------------------------------------------------------- pattern parser

_SLOT_RE = re.compile(r"^([A-Z_]+)(?::(.*))?$")
_SUB_RE = re.compile(r"^([a-z0-9]+)(?::(.*))?$")


def parse(src: str):
    """Parses a pattern into nodes: ("text", s), ("alt", [seq]), ("gender", [str]),
    ("slot", SLOT, variant), ("sub", name, fmt), ("lit", [texts], SLOT, value), ("span", seq)."""
    pos = 0

    def seq(stop: str) -> list:
        nonlocal pos
        out: list = []
        buf = ""
        while pos < len(src):
            c = src[pos]
            if c in stop:
                break
            if c in "({[<~":
                if buf:
                    out.append(("text", buf))
                    buf = ""
                if c == "~":
                    pos += 1
                    if src[pos:pos + 1] != "(":
                        raise TemplateError(f"~ needs a group: {src!r}")
                    pos += 1
                    opts = alts()
                    if len(opts) != 3 or any(len(o) > 1 or (o and o[0][0] != "text") for o in opts):
                        raise TemplateError(f"~(m|f|n) takes three plain words: {src!r}")
                    out.append(("gender", [o[0][1] if o else "" for o in opts]))
                elif c == "(":
                    pos += 1
                    out.append(("alt", alts()))
                elif c == "<":
                    pos += 1
                    inner = seq(">")
                    expect(">")
                    out.append(("span", inner))
                elif c == "{":
                    end = src.index("}", pos)
                    body = src[pos + 1:end]
                    pos = end + 1
                    m = _SLOT_RE.match(body)
                    if m:
                        out.append(("slot", m.group(1), m.group(2) or ""))
                    else:
                        m = _SUB_RE.match(body)
                        if not m:
                            raise TemplateError(f"bad placeholder {{{body}}} in {src!r}")
                        out.append(("sub", m.group(1), m.group(2) or ""))
                else:
                    end = src.index("]", pos)
                    parts = src[pos + 1:end].split("|")
                    pos = end + 1
                    spec = parts[-1]
                    m = re.match(r"^([A-Z_]+)(?:=(.*))?$", spec)
                    if not m or m.group(1) not in schema.SLOTS or len(parts) < 2:
                        raise TemplateError(f"bad literal [{src[pos:end]}] in {src!r}")
                    val = yaml.safe_load(m.group(2)) if m.group(2) is not None else None
                    out.append(("lit", parts[:-1], m.group(1), val))
            else:
                buf += c
                pos += 1
        if buf:
            out.append(("text", buf))
        return out

    def alts() -> list:
        nonlocal pos
        opts = [seq("|)")]
        while src[pos:pos + 1] == "|":
            pos += 1
            opts.append(seq("|)"))
        expect(")")
        return opts

    def expect(ch: str) -> None:
        nonlocal pos
        if src[pos:pos + 1] != ch:
            raise TemplateError(f"expected {ch!r} at {pos} in {src!r}")
        pos += 1

    nodes = seq("")
    if pos != len(src):
        raise TemplateError(f"unbalanced pattern {src!r}")
    return nodes


def walk(nodes):
    for n in nodes:
        yield n
        if n[0] == "alt":
            for s in n[1]:
                yield from walk(s)
        elif n[0] == "span":
            yield from walk(n[1])


# ---------------------------------------------------------------- language data

def _forms(entry: dict) -> dict[str, list[tuple[str, str | None]]]:
    """{form: [(surface, gender)]} from a kinds/stretches entry."""
    out = {}
    for form, surfaces in entry.items():
        lst = []
        for s in surfaces:
            text, _, g = str(s).partition("/")
            if g and g not in "mfn":
                text, g = str(s), ""
            lst.append((text, g or None))
        out[form] = lst
    return out


class Lang:
    def __init__(self, lang: str):
        root = HERE / "templates"
        self.data = yaml.safe_load((root / f"{lang}.yaml").read_text(encoding="utf-8"))
        if self.data.get("lang") != lang:
            raise TemplateError(f"{lang}.yaml says lang {self.data.get('lang')!r}")
        self.lang = lang
        self.decimal = self.data.get("decimal", ".")
        self.numbers = self.data.get("numbers", {})
        self.weekdays = self.data.get("weekdays", {})
        self.kinds = {k: _forms(v) for k, v in self.data["kinds"].items()}
        self.stretches = {k: _forms(v) for k, v in self.data["stretches"].items()}
        for k in self.kinds:
            if k not in schema.KINDS:
                raise TemplateError(f"unknown kind {k}")
        lex = HERE / "lexicon" / "kinds.json"
        terms = json.loads(lex.read_text(encoding="utf-8"))["terms"].get(lang, {}) if lex.exists() else {}
        for k, forms in self.kinds.items():
            extra = [(t, None) for t in terms.get(k, []) if self._noun(t)]
            seen = {t.casefold() for t, _ in forms.get("pl", []) + forms.get("sg", [])}
            forms["any"] = forms.get("pl", []) + forms.get("sg", []) + [
                x for x in extra if x[0].casefold() not in seen and not seen.add(x[0].casefold())]
        for k in self.stretches:
            if k not in schema.STRETCH_KINDS:
                raise TemplateError(f"unknown stretch {k}")
        self.banks = {slot: {var: [self._entry(e) for e in entries]
                             for var, entries in variants.items()}
                      for slot, variants in self.data["banks"].items()}
        self.points = {role: {form: [self._entry(e) for e in entries]
                              for form, entries in forms.items()}
                       for role, forms in self.data["points"].items()}
        self.templates = {intent: [(t, parse(t)) for t in lst]
                          for intent, lst in self.data["templates"].items()}
        for intent in self.templates:
            if intent not in schema.INTENTS:
                raise TemplateError(f"unknown intent {intent}")
        self.names = self._names()

    def _noun(self, t: str) -> bool:
        w = t.split()
        if not w or len(w) > 3 or len(t) > 30 or any(c.isdigit() for c in t):
            return False
        if self.lang == "de":
            return t[0].isupper()
        return w[0].casefold() not in _VERB_START

    @staticmethod
    def _entry(e):
        if isinstance(e, list):
            pats, value = e
        else:
            pats, value = e, None
        pats = pats if isinstance(pats, list) else [pats]
        return [parse(p) for p in pats], value

    def _names(self) -> dict[str, list[str]]:
        out = {}
        for f in sorted((HERE / "data" / "names").glob("*.json")):
            if f.name.startswith("."):
                continue
            out[f.stem] = json.loads(f.read_text(encoding="utf-8"))
        hand = yaml.safe_load((HERE / "templates" / "names.yaml").read_text(encoding="utf-8"))
        out["common"] = list(hand["common"])
        out["kt"] = [n for n in out["towns"] + out["common"] if _KIND_TOWN.search(n)]
        out["one"] = [n for g in ("towns", "peaks", "passes", "huts", "lakes", "common")
                      for n in out[g] if " " not in n]
        out["biz"] = [parse(p) for p in hand["business"]]
        return out


# ---------------------------------------------------------------- rendering

class Ctx:
    """The state of one rendered template: atoms, spans and the kinds already used."""

    def __init__(self, lang: Lang, rng: Random, intent: str):
        self.L, self.rng, self.intent = lang, rng, intent
        self.atoms: list[dict] = []   # {"text", "span", "gender", "marker"}
        self.spans: list[dict] = []   # {"slot", "value"}
        self.used: set[str] = set()
        self.entry: dict | None = None  # {"id", "slot", "ids", "kinds"} of the entry rendered
        self.n_entries = 0


def _pick(rng: Random, items: list, weights: list | None = None):
    return rng.choices(items, weights=weights, k=1)[0] if weights else rng.choice(items)


def _sample_var(name: str, rng: Random, cap: int | None) -> float:
    vals, w = VARS[name]
    if cap is not None:
        pairs = [(v, (w[i] if w else 1)) for i, v in enumerate(vals) if v < cap]
        vals, w = [p[0] for p in pairs], [p[1] for p in pairs]
    return _pick(rng, vals, w)


def _vars_for(nodes, L: Lang, rng: Random) -> dict:
    """Samples every number variable a pattern uses, within the range of its word formats."""
    caps: dict[str, int] = {}
    names = set()
    for n in walk(nodes):
        if n[0] == "sub" and (n[1] in VARS or n[1] in ("n1", "k2")):
            base = {"n1": "n", "k2": "k1"}.get(n[1], n[1])
            names.add(base)
            if n[2]:
                lst = L.numbers.get(n[2])
                if lst is None:
                    raise TemplateError(f"no number format {n[2]!r}")
                cap = len(lst) - (1 if n[1] == "n1" else 0)
                caps[base] = min(caps.get(base, cap), cap)
    out: dict = {}
    for name in sorted(names):
        out[name] = _sample_var(name, rng, caps.get(name))
    if any(n[0] == "sub" and n[1] == "wd" for n in walk(nodes)):
        out["wd"] = rng.choice(schema.WEEKDAYS)
    if "n" in out:
        out["n1"] = out["n"] + 1
    if "k1" in out:
        out["k2"] = out["k1"] + rng.choice(_steps(10, 120, 5))
    return out


def _fmt_num(L: Lang, v, fmt: str) -> str:
    if fmt:
        return str(L.numbers[fmt][int(v)])
    if isinstance(v, float):
        return f"{v:g}".replace(".", L.decimal)
    return str(v)


def _eval(val, vars_: dict):
    if isinstance(val, dict):
        return {k: _eval(v, vars_) for k, v in val.items()}
    if isinstance(val, list):
        return [_eval(v, vars_) for v in val]
    if isinstance(val, str) and val.startswith("$"):
        m = re.match(r"^\$(\w+)(?:([/*+-])(\d+(?:\.\d+)?))?$", val)
        if not m or m.group(1) not in vars_:
            raise TemplateError(f"bad value {val!r} (vars {sorted(vars_)})")
        x = vars_[m.group(1)]
        if m.group(2):
            y = float(m.group(3))
            x = {"/": x / y, "*": x * y, "+": x + y, "-": x - y}[m.group(2)]
            x = round(x, 3)
        return x
    return val


def _choose_kind(ctx: Ctx, what: str, spec: str) -> tuple[str, dict]:
    """A kind or stretch for {kind:form[/subset]}, distinct within a template."""
    form, _, subset = spec.partition("/")
    table = ctx.L.kinds if what == "kind" else ctx.L.stretches
    ids = [k for k, f in table.items() if form in f and (not subset or k in KIND_SUBSETS[subset])]
    ids = [k for k in ids if k not in ctx.used] or ids
    if not ids:
        raise TemplateError(f"no {what} has form {spec!r}")
    w = [KIND_WEIGHT.get(k, 1) for k in ids] if what == "kind" else None
    k = _pick(ctx.rng, ids, w)
    ctx.used.add(k)
    text, gender = ctx.rng.choice(table[k][form])
    return k, {"text": text, "gender": gender}


def _name(ctx: Ctx, cat: str) -> str:
    mix = NAME_MIX.get(cat or None)
    if mix is None:
        raise TemplateError(f"unknown name category {cat!r}")
    group = _pick(ctx.rng, list(mix), list(mix.values()))
    if group == "biz":
        sub = Ctx(ctx.L, ctx.rng, ctx.intent)
        render(sub, ctx.rng.choice(ctx.L.names["biz"]), None, {})
        return re.sub(r"\s+", " ", "".join(a["text"] for a in sub.atoms)).strip()
    return ctx.rng.choice(ctx.L.names[group])


def render(ctx: Ctx, nodes, span: int | None, vars_: dict) -> None:
    """Appends the atoms of `nodes`. `span` is the span id of the words, or None for O."""
    start = len(ctx.atoms)
    _render(ctx, nodes, span, vars_)
    if ctx.entry is not None:
        for a in ctx.atoms[start:]:
            a.setdefault("entry", ctx.entry["id"])


def _render(ctx: Ctx, nodes, span: int | None, vars_: dict) -> None:
    for n in nodes:
        t = n[0]
        if t == "text":
            ctx.atoms.append({"text": n[1], "span": span})
        elif t == "alt":
            render(ctx, ctx.rng.choice(n[1]), span, vars_)
        elif t == "gender":
            ctx.atoms.append({"text": "", "span": span, "marker": ("gender", n[1])})
        elif t == "span":
            if span is not None or ctx.entry is None:
                raise TemplateError("a span outside an entry, or a nested span")
            sid = new_span(ctx, ctx.entry["slot"], None)
            ctx.entry["ids"].append(sid)
            render(ctx, n[1], sid, vars_)
        elif t == "sub":
            name, fmt = n[1], n[2]
            if name in ("kind", "stretch"):
                k = _choose_kind(ctx, name, fmt)
                if ctx.entry is not None:
                    ctx.entry["kinds"].append(k[0])
                ctx.atoms.append({"text": k[1]["text"], "span": span, "gender": k[1]["gender"],
                                  "kw": True})
            elif name == "wd":
                ctx.atoms.append({"text": ctx.rng.choice(ctx.L.weekdays[vars_["wd"]][fmt or "sg"]),
                                  "span": span})
            elif name == "name":
                ctx.atoms.append({"text": _name(ctx, fmt), "span": span})
            elif name == "a":
                ctx.atoms.append({"text": "", "span": span, "marker": ("a",)})
            elif name == "art":
                if fmt not in ARTICLES.get(ctx.L.lang, {}):
                    raise TemplateError(f"no article {fmt!r} for {ctx.L.lang}")
                ctx.atoms.append({"text": "", "span": span, "marker": ("art", fmt)})
            elif name in vars_:
                ctx.atoms.append({"text": _fmt_num(ctx.L, vars_[name], fmt), "span": span})
            else:
                raise TemplateError(f"unknown placeholder {{{name}}}")
        elif t == "slot":
            if span is not None:
                raise TemplateError("slot inside an entry")
            fill_slot(ctx, n[1], n[2])
        elif t == "lit":
            sid = new_span(ctx, n[2], n[3])
            ctx.atoms.append({"text": ctx.rng.choice(n[1]), "span": sid, "kw": n[2] == "WHAT"})
        else:
            raise TemplateError(f"node {t}")


def new_span(ctx: Ctx, slot: str, value) -> int:
    ctx.spans.append({"slot": slot, "value": value})
    return len(ctx.spans) - 1


def render_entry(ctx: Ctx, slot: str, entry, implicit, form: str | None = None) -> None:
    """Renders one bank entry as one span of `slot`."""
    pats, value = entry
    nodes = ctx.rng.choice(pats)
    vars_ = _vars_for(nodes, ctx.L, ctx.rng)
    ctx.n_entries += 1
    ctx.entry = {"id": ctx.n_entries, "slot": slot, "ids": [], "kinds": []}
    if any(n[0] == "span" for n in walk(nodes)):
        render(ctx, nodes, None, vars_)
    else:
        ctx.entry["ids"].append(new_span(ctx, slot, None))
        render(ctx, nodes, ctx.entry["ids"][0], vars_)
    ids, kinds, ctx.entry = ctx.entry["ids"], ctx.entry["kinds"], None
    if value is not None:
        v = _eval(value, vars_)
    elif implicit == "kind":
        if not kinds:
            raise TemplateError(f"{slot} entry without a kind or a value")
        v = kinds[0] if len(kinds) == 1 else kinds  # "water and campsites" is one WHAT span
    elif implicit == "point":
        if form == "kind":
            v = {"kind": kinds[0]}
        elif form == "here":
            v = {"here": True}
        elif form == "name":
            v = None  # the span text, after noise
        else:
            raise TemplateError(f"{slot} {form} entry needs a value")
    else:
        v = None  # NAME, IGNORED: the span text
    for sid in ids:
        ctx.spans[sid]["value"] = v


def fill_slot(ctx: Ctx, slot: str, variant: str) -> None:
    L = ctx.L
    if slot in POINT_SLOTS:
        role, _, forms = variant.partition(".")
        bank = L.points.get(role or "default")
        if bank is None:
            raise TemplateError(f"no point role {role!r}")
        allowed = forms.split("+") if forms else (
            FORMS.get((ctx.intent, slot)) or FORMS.get((None, slot)) or list(FORM_WEIGHT))
        avail = [f for f in allowed if f in bank]
        if not avail:
            raise TemplateError(f"{slot}:{variant} has no form of {allowed} in role {role!r}")
        form = _pick(ctx.rng, avail, [FORM_WEIGHT[f] for f in avail])
        render_entry(ctx, slot, ctx.rng.choice(bank[form]), "point", form)
        if role == "stop":  # a compound like "Kaffeepause" names the kind and the stop
            ctx.spans[-1]["stop"] = True
        return
    variants = L.banks.get(slot)
    if variants is None or (variant or "default") not in variants:
        raise TemplateError(f"no bank {slot}:{variant or 'default'}")
    entry = ctx.rng.choice(variants[variant or "default"])
    render_entry(ctx, slot, entry, "kind" if slot == "WHAT" else None)


def _gender_near(atoms: list[dict], i: int) -> str | None:
    """The gender of the nearest kind word in the same entry, else in the sentence; a following
    word wins a tie."""
    e = atoms[i].get("entry")
    for same in (True, False):
        cands = [(abs(j - i), j < i, b["gender"]) for j, b in enumerate(atoms)
                 if b.get("gender") and (not same or (e is not None and b.get("entry") == e))]
        if cands:
            return min(cands)[2]
    return None


def _article(lang: str, table: dict, gender: str, word: str) -> str:
    w = unicodedata.normalize("NFD", word.lower())
    vowel = w[:1] in ("aeiouyh" if lang == "fr" else "aeiou")
    impure = lang == "it" and (re.match(r"s[^aeiou]|z|gn|ps|x|y", w) is not None)
    keys = ([gender + "v", "v"] if vowel else []) + (["s"] if impure and gender == "m" else [])
    return next((table[k] for k in keys + [gender] if k in table), table["m"])


def _resolve_markers(ctx: Ctx) -> None:
    """Gender words first: an article depends on the word after it ("alla prossima")."""
    atoms = ctx.atoms
    for i, a in enumerate(atoms):
        m = a.get("marker")
        if m and m[0] == "gender":
            g = _gender_near(atoms, i)
            a["text"] = m[1]["mfn".index(g)] if g else ctx.rng.choice(m[1])
    for i, a in enumerate(atoms):
        m = a.get("marker")
        if not m or m[0] == "gender":
            continue
        j = next((j for j in range(i + 1, len(atoms)) if atoms[j]["text"].strip()), None)
        nxt = atoms[j]["text"].strip() if j is not None else ""
        if m[0] == "a":
            vowel = nxt[:1].lower() in "aeiou" and not nxt.lower().startswith(("uni", "eu", "one"))
            a["text"] = "an" if vowel else "a"
        else:
            g = next((b["gender"] for b in atoms[i + 1:] if b.get("gender")), None) or "m"
            form = _article(ctx.L.lang, ARTICLES[ctx.L.lang][m[1]], g, nxt)
            if form.endswith("'") and j is not None:  # "l'hôtel" is one word, in the span of "hôtel"
                head, _, tail = form.rpartition(" ")
                a["text"] = head + " " if head else ""
                for k in range(i + 1, j):
                    atoms[k]["text"] = ""
                atoms[j]["text"] = tail + atoms[j]["text"].lstrip()
            else:
                a["text"] = form


def assemble(ctx: Ctx) -> tuple[str, list[str], list[int], list[int]]:
    """Text, word labels, the span id of each BIO span in order, and the kind-word indices."""
    _resolve_markers(ctx)
    text = ""
    ranges: dict[int, list[int]] = {}
    kw: list[tuple[int, int]] = []
    for a in ctx.atoms:
        t = re.sub(r"\s+", " ", a["text"])
        if not text or text.endswith(" "):
            t = t.lstrip()
        if not t:
            continue
        if a.get("kw") and t.strip():
            s0 = len(text) + (len(t) - len(t.lstrip()))
            kw.append((s0, s0 + len(t.strip())))
        if a["span"] is not None:
            core = t.strip()
            if core:
                s = len(text) + (len(t) - len(t.lstrip()))
                r = ranges.setdefault(a["span"], [s, s + len(core)])
                r[0], r[1] = min(r[0], s), max(r[1], s + len(core))
        text += t
    text = text.rstrip()
    labels, order, cur = [], [], None
    for w, s, e in words.split(text):
        sid = next((i for i, (a, b) in ranges.items() if a <= s < b), None)
        if sid is None:
            if any(a < e and s < b for a, b in ranges.values()):
                raise TemplateError(f"word {w!r} straddles a span edge in {text!r}")
            labels.append("O")
            cur = None
            continue
        if e > ranges[sid][1]:
            raise TemplateError(f"word {w!r} straddles a span edge in {text!r}")
        slot = ctx.spans[sid]["slot"]
        if sid != cur:
            labels.append(f"B-{slot}")
            order.append(sid)
        else:
            labels.append(f"I-{slot}")
        cur = sid
    missing = set(ranges) - set(order)
    if missing or len(ranges) != len(ctx.spans):
        raise TemplateError(f"empty span in {text!r}")
    kinds = [i for i, (_, s, _) in enumerate(words.split(text)) if any(a <= s < b for a, b in kw)]
    return text, labels, order, kinds


# ---------------------------------------------------------------- the gold request

def span_texts(text: str, labels: list[str]) -> list[str]:
    return [t for _, t in words.spans(text, labels)]


def build(intent: str, spans: list[tuple[dict, str]]) -> dict:
    """The gold request from the span values in order; name values come from the text."""
    r: dict = {"type": intent}
    where: dict = {}
    ignored: list[str] = []
    days: list = []
    for sp, text in spans:
        slot, v = sp["slot"], sp["value"]
        if slot in POINT_SLOTS and v is None:
            v = {"name": strip_elided(text)}
        if slot == "IGNORED":
            ignored.append(text)
        elif intent == "none":
            continue
        elif slot == "WHAT":
            if intent == "stretches":
                r["what"] = v if v in schema.STRETCH_KINDS else f"gap:{v}"
            else:
                r.setdefault("what", []).extend(v if isinstance(v, list) else [v])
        elif slot == "NAME":
            r["name"] = strip_elided(text)
        elif slot in ("FROM", "TO"):
            r[slot.lower()] = v
        elif slot == "VIA":
            r.setdefault("via", []).append(v)
        elif slot == "NEAR":
            if intent == "place":
                r["near"] = v
            else:
                where.setdefault("near", []).append(v)
        elif slot == "POINT":
            r["at" if intent == "end_day" else "point"] = v
            if sp.get("stop") and intent == "add_point":
                r["kind"] = "stop"
        elif slot in ("BEFORE", "AFTER"):
            where[slot.lower()] = v
        elif slot == "DAY":
            if intent in ("end_day", "join"):
                days.append(v["day"])
            else:
                where.update(v)
        elif slot == "SCOPE":
            where["scope"] = v
        elif slot == "ALONG":
            where["along"] = v
        elif slot in ("OPEN", "RADIUS", "BIKE", "GOAL", "DAYS", "PER_DAY", "EVERY", "MIN",
                      "KIND"):
            r[slot.lower()] = v
        else:
            raise TemplateError(f"slot {slot}")
    if days:
        r["day"] = min(days) if intent == "join" else days[0]
    if intent == "split" and "days" not in r and "per_day" not in r:
        r["days"] = 2  # "split day 3" can only mean in two
    if r.get("open") == "bare":
        r["open"] = {"day": where["day"]} if where.get("day") not in (None, "every") else {"now": True}
    if where:
        r["where"] = where
    if ignored:
        r["ignored"] = ignored
    return r


# ---------------------------------------------------------------- noise

KEYS = {
    "en": ["qwertyuiop", "asdfghjkl", "zxcvbnm"],
    "de": ["qwertzuiopü", "asdfghjklöä", "yxcvbnm"],
    "fr": ["azertyuiop", "qsdfghjklm", "wxcvbn"],
    "it": ["qwertyuiop", "asdfghjkl", "zxcvbnm"],
}


def _neighbours(lang: str) -> dict[str, str]:
    rows = KEYS.get(lang, KEYS["en"])
    out: dict[str, str] = {}
    for r, row in enumerate(rows):
        for i, c in enumerate(row):
            near = row[max(0, i - 1):i] + row[i + 1:i + 2]
            for dr in (-1, 1):
                if 0 <= r + dr < len(rows):
                    near += rows[r + dr][max(0, i - 1):i + 1]
            out[c] = near
    return out


def typo(w: str, rng: Random, near: dict[str, str]) -> str:
    op = rng.choice(["swap", "drop", "double", "near"])
    i = rng.randrange(len(w))
    if op == "swap" and len(w) >= 2:
        i = min(i, len(w) - 2)
        if w[i] != w[i + 1]:
            return w[:i] + w[i + 1] + w[i] + w[i + 2:]
    if op == "drop" and len(w) >= 4:
        return w[:i] + w[i + 1:]
    if op == "near" and w[i].lower() in near:
        c = rng.choice(near[w[i].lower()])
        return w[:i] + (c.upper() if w[i].isupper() else c) + w[i + 1:]
    return w[:i] + w[i] + w[i:]


def strip_accents(w: str, german: bool) -> str:
    if german:
        for a, b in (("ä", "ae"), ("ö", "oe"), ("ü", "ue"), ("Ä", "Ae"), ("Ö", "Oe"), ("Ü", "Ue")):
            w = w.replace(a, b)
    d = unicodedata.normalize("NFD", w)
    return unicodedata.normalize("NFC", "".join(c for c in d if unicodedata.category(c) != "Mn"))


def _rejoin(toks: list[str], gaps: list[str]) -> str:
    return "".join(g + t for g, t in zip(gaps, toks))


QUALIFIER_SLOTS = ("DAY", "ALONG", "OPEN", "EVERY", "PER_DAY", "MIN", "RADIUS", "SCOPE")


def noise(text: str, labels: list[str], rng: Random, lang: str,
          kind_words: list[int] = ()) -> tuple[str, list[str], dict]:
    """Typos, lower case, no accents and no punctuation; never splits or merges words.
    A misspelt kind word must stay WHAT, so 20 % of sentences with one get the typo inside it,
    and 8 % of the others get it inside a day or qualifier word."""
    sp = words.split(text)
    toks = [w for w, _, _ in sp]
    gaps = [text[(sp[i - 1][2] if i else 0):s] for i, (_, s, _) in enumerate(sp)]
    applied: dict = {}

    def accept(new_toks: list[str], new_gaps: list[str]) -> bool:
        return [w for w, _, _ in words.split(_rejoin(new_toks, new_gaps))] == new_toks

    near = _neighbours(lang)
    ok = [i for i, w in enumerate(toks) if w.isalpha() and len(w) >= 3]
    kinds = [i for i in ok if i in kind_words]
    quals = [i for i in ok if labels[i] != "O" and labels[i][2:] in QUALIFIER_SLOTS]
    aimed: list[int] = []
    if kinds and rng.random() < 0.20:
        aimed, applied["typo_kind"] = [rng.choice(kinds)], 1
    elif quals and rng.random() < 0.08:
        aimed, applied["typo_qualifier"] = [rng.choice(quals)], 1
    if rng.random() < 0.15:
        rest = [i for i in ok if i not in aimed]
        aimed += rng.sample(rest, min(len(rest), rng.choice([1, 1, 2])))
        applied["typo"] = 1
    for i in aimed:
        new = toks[:i] + [typo(toks[i], rng, near)] + toks[i + 1:]
        if accept(new, gaps):
            toks = new
    if rng.random() < 0.30:
        new = [w.lower() for w in toks]
        if accept(new, gaps):
            toks, applied["lower"] = new, 1
    if rng.random() < 0.15:
        german = lang == "de" and rng.random() < 0.4
        new = [strip_accents(w, german) for w in toks]
        if new != toks and accept(new, gaps):
            toks, applied["accents"] = new, 1
    # "-" and ">" stay: without them "Freiburg - Basel" becomes one name.
    punct = [i for i, (w, lab) in enumerate(zip(toks, labels)) if lab == "O" and w in "?!.,;:"]
    keep = [i for i in range(len(toks)) if i not in punct]
    if punct and keep and rng.random() < 0.5:
        new_toks = [toks[i] for i in keep]
        new_gaps, prev = [], -1
        for i in keep:
            merged = "".join(gaps[prev + 1:i + 1])
            new_gaps.append((" " if merged else "") if new_gaps else "")
            prev = i
        if accept(new_toks, new_gaps):
            toks, gaps = new_toks, new_gaps
            labels = [labels[i] for i in keep]
            applied["punct"] = 1
    return _rejoin(toks, gaps), labels, applied


# ---------------------------------------------------------------- generation

def diff(a, b, path: str = "") -> list[str]:
    if isinstance(a, dict) and isinstance(b, dict):
        out = []
        for k in list(a) + [k for k in b if k not in a]:
            out += diff(a.get(k), b.get(k), f"{path}.{k}" if path else k)
        return out
    return [] if a == b else [path]


class Generator:
    def __init__(self, lang: str, seed: int, use_filter: bool):
        self.L = Lang(lang)
        self.rng = Random(f"{seed}:{lang}")
        self.decode = None
        if use_filter:
            from decode import decode
            self.decode = decode
        self.stats = {"tried": Counter(), "dropped": Counter(), "long": Counter(),
                      "paths": Counter(), "reasons": Counter(), "examples": defaultdict(list),
                      "noise": Counter()}
        self.seen: set[str] = set()

    def one(self, intent: str) -> dict | None:
        L, rng = self.L, self.rng
        src, nodes = rng.choice(L.templates[intent])
        ctx = Ctx(L, rng, intent)
        render(ctx, nodes, None, _vars_for(nodes, L, rng))
        try:
            text, labels, order, kind_words = assemble(ctx)
        except TemplateError as e:
            raise TemplateError(f"{intent}: {src!r}: {e}") from None
        if rng.random() < 0.5 and len(text[:1].upper()) == 1:
            text = text[:1].upper() + text[1:]
        if len(text) > 80:
            self.stats["long"][intent] += 1
            return None
        spans = [ctx.spans[i] for i in order]
        gold = build(intent, list(zip(spans, span_texts(text, labels))))
        try:
            schema.validate(gold)
        except schema.Invalid as e:
            raise TemplateError(f"{intent}: {src!r} -> {text!r}: {gold}: {e}") from None
        self.stats["tried"][intent] += 1
        if self.decode is not None:
            try:
                pred = self.decode(text, intent, labels)
                bad = diff(schema.canonical(pred), schema.canonical(gold))
            except Exception as e:  # noqa: BLE001 - a crash of the second check is a drop reason
                pred, bad = {"error": repr(e)}, [f"error:{type(e).__name__}"]
            if bad:
                self.stats["dropped"][intent] += 1
                for p in bad:
                    self.stats["paths"][f"{intent}:{p}"] += 1
                    self.stats["reasons"][self._reason(p, gold, pred, spans, text, labels)] += 1
                ex = self.stats["examples"][intent]
                if len(ex) < 12:
                    ex.append({"text": text, "spans": words.spans(text, labels),
                               "gold": schema.canonical(gold), "pred": pred})
                return None
        key = text.casefold()
        if key in self.seen and rng.random() < 0.9:
            return None
        self.seen.add(key)
        ntext, nlabels, applied = noise(text, labels, rng, L.lang, kind_words)
        if len(ntext) > 80:  # a doubled letter or "ae" for "ä" can pass the limit
            ntext, nlabels, applied = text, labels, {}
        self.stats["noise"].update(applied.keys())
        request = build(intent, list(zip(spans, span_texts(ntext, nlabels))))
        schema.validate(request)
        return {"text": ntext, "lang": L.lang, "intent": intent, "labels": nlabels,
                "request": request}

    @staticmethod
    def _reason(path: str, gold: dict, pred: dict, spans, text, labels) -> str:
        top = path.split(".")[0]
        slot_of = {"what": "WHAT", "where": None, "open": "OPEN", "radius": "RADIUS",
                   "min": "MIN", "every": "EVERY", "per_day": "PER_DAY", "days": "DAYS",
                   "bike": "BIKE", "goal": "GOAL", "kind": "KIND", "day": "DAY", "at": "POINT",
                   "point": "POINT", "to": "TO", "from": "FROM", "via": "VIA", "name": "NAME",
                   "near": "NEAR", "ignored": "IGNORED"}
        sub = path.split(".")[1] if "." in path else ""
        slot = slot_of.get(top) or {"day": "DAY", "part": "DAY", "scope": "SCOPE", "near": "NEAR",
                                    "along": "ALONG", "before": "BEFORE", "after": "AFTER"}.get(sub)
        if top == "ignored":
            extra = [w for w in pred.get("ignored", []) if w not in gold.get("ignored", [])]
            return f"ignored | extra {extra[:3]!r}" if extra else "ignored | missing"
        if top in ("type", "where") and not sub:
            g = gold.get(top)
            return f"{path} | gold {sorted(g) if isinstance(g, dict) else g} pred {pred.get(top)!r}"[:90]
        texts = [t for s, t in words.spans(text, labels) if s == slot]
        return f"{path} | {slot} {'/'.join(texts)[:40]!r}"

    def run(self, n: int) -> list[dict]:
        total = sum(SHARE.values())
        want = {k: n * v // total for k, v in SHARE.items()}
        for k in list(SHARE)[: n - sum(want.values())]:
            want[k] += 1
        rows = []
        for intent, count in want.items():
            if intent not in self.L.templates:
                raise TemplateError(f"no templates for {intent}")
            got, tries = [], 0
            while len(got) < count and tries < count * 40:
                tries += 1
                r = self.one(intent)
                if r is not None:
                    got.append(r)
            if len(got) < count:
                print(f"warning: {intent}: {len(got)} of {count}", file=sys.stderr)
            rows += got
        self.rng.shuffle(rows)
        return rows


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--lang", required=True)
    ap.add_argument("--n", type=int, default=12000)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--out", required=True)
    ap.add_argument("--dev-out", help="default: data/dev/<name of --out>")
    ap.add_argument("--dev-share", type=float, default=0.05)
    ap.add_argument("--filter", action="store_true", help="drop examples that decode.py reads differently")
    args = ap.parse_args()
    if args.filter and os.environ.get("PYTHONHASHSEED") != "0":
        # Some readings of decode.py follow set order, which changes with the hash seed.
        os.execve(sys.executable, [sys.executable, *sys.argv], {**os.environ, "PYTHONHASHSEED": "0"})

    g = Generator(args.lang, args.seed, args.filter)
    rows = g.run(args.n)
    n_dev = round(len(rows) * args.dev_share)
    out = Path(args.out)
    dev = Path(args.dev_out) if args.dev_out else out.parent.parent / "dev" / out.name
    for path, part in ((out, rows[n_dev:]), (dev, rows[:n_dev])):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in part),
                        encoding="utf-8")
    st = g.stats
    print(f"{args.lang}: {len(rows) - n_dev} train -> {out}, {n_dev} dev -> {dev}")
    print("intent        rows  tried  dropped  too-long")
    counts = Counter(r["intent"] for r in rows)
    for intent in SHARE:
        t, d = st["tried"][intent], st["dropped"][intent]
        pct = f"{100 * d / t:5.1f}%" if t else "   - "
        print(f"{intent:12} {counts[intent]:5} {t:6} {pct:>8} {st['long'][intent]:6}")
    print("noise:", ", ".join(f"{k} {100 * v / len(rows):.1f}%" for k, v in sorted(st["noise"].items())))
    if args.filter:
        print("top mismatch reasons:")
        for reason, c in st["reasons"].most_common(40):
            print(f"  {c:5}  {reason}")
        report = {"dropped": dict(st["dropped"]), "tried": dict(st["tried"]),
                  "paths": dict(st["paths"].most_common()),
                  "reasons": dict(st["reasons"].most_common(200)),
                  "examples": st["examples"]}
        out.with_suffix(".filter.json").write_text(
            json.dumps(report, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
