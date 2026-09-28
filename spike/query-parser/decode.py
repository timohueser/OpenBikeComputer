"""Tagger output (intent + BIO word labels) -> a request of language v0 (schema.py).

Each slot has one parser over a Span. A parser marks the tokens it reads; the unread words of a
span that are not stop words go to `ignored`, and so does every span the request type has no
field for. A required field that does not parse gives `none`. Nothing is guessed.
"""

from __future__ import annotations

import re
from typing import Any

import schema
import words
from rapidfuzz.distance import OSA

from lexicon import SENSE, STOP, TABLES as T, compound, fold, lang_of, rank, tokens_of, toks

_DIGITS = re.compile(r"\d+(?:[.,]\d+)?")
# "a", "ein", "un" are 1 only in front of a unit: "an hour", but not "80 km a day".
_WEAK = {"a", "an", "ein", "eine", "einen", "einem", "einer", "un", "une", "una", "un'"}
_IT_TENS = {"vent": 20, "trent": 30, "quarant": 40, "cinquant": 50, "sessant": 60, "settant": 70,
            "ottant": 80, "novant": 90}
_IT_UNITS = {"uno": 1, "due": 2, "tre": 3, "quattro": 4, "cinque": 5, "sei": 6, "sette": 7,
             "otto": 8, "nove": 9}
_LEAD = STOP | tokens_of("prep") | tokens_of("nearest")
# A span of only these carries nothing: a stray "un", "the", "ein".
_EMPTY = STOP | tokens_of("nearest")
_ORD_SUFFIX = tokens_of("ordinal_suffix")


class Span:
    """The folded tokens of one span; parsers mark what they read."""

    def __init__(self, text: str, lang: str | None = None):
        self.text, self.lang = text, lang
        self.words = words.split(text)
        tt = toks(text)
        self.t = [t for t, _ in tt]
        self.wi = [w for _, w in tt]
        self.used = [False] * len(self.t)

    def mark(self, i: int, j: int) -> None:
        for k in range(i, j):
            self.used[k] = True

    def scan(self, *tables: str, strict: bool = False) -> list[tuple[int, int, str, Any]]:
        """Best matches over unread tokens, left to right; earlier tables win ties. `strict`
        takes no typos, for filler words that must not eat a value ("hours" is not "jours")."""
        out, i = [], 0
        while i < len(self.t):
            best, name_ = None, None
            if not self.used[i]:
                for name in tables:
                    m = T[name].match_at(self.t, i)
                    if m and not any(self.used[i:i + m[0]]) and not (strict and m[2] > 1) and (
                            best is None or rank(m) > rank(best)):
                        best, name_ = m, name
            if best:
                out.append((i, i + best[0], name_, best[1]))
                self.mark(i, i + best[0])
                i += best[0]
            else:
                i += 1
        return out

    def find(self, table: str) -> list[Any]:
        return [v for _, _, _, v in self.scan(table)]

    def kinds(self) -> list[str]:
        """Kind values, with the sentence language's own sense of a word ("bar" in Italian)."""
        sense = SENSE.get(self.lang, {})
        return [sense.get(" ".join(self.t[i:j]), v) for i, j, _, v in self.scan("kind")]

    def drop(self, extra: set[str] = frozenset()) -> None:
        """Marks stop words (and `extra`) as read."""
        for k, t in enumerate(self.t):
            if t in STOP or t in extra:
                self.used[k] = True

    def leftover(self, extra: set[str] = frozenset()) -> list[str]:
        """The unread words, as runs of the original text."""
        self.drop(extra)
        bad = sorted({self.wi[k] for k, u in enumerate(self.used) if not u})
        runs: list[list[int]] = []
        for w in bad:
            if runs and runs[-1][-1] == w - 1:
                runs[-1].append(w)
            else:
                runs.append([w])
        return [self.text[self.words[r[0]][1]:self.words[r[-1]][2]] for r in runs]


# --- numbers and quantities -------------------------------------------------------------------

def _compound(tok: str) -> float | None:
    """German "fünfundzwanzig" and Italian "ventidue"; other languages write the parts apart."""
    if "und" in tok[1:-1]:
        a, _, b = tok.partition("und")
        va, vb = T["number"].whole([a]), T["number"].whole([b])
        if va and vb and va[0] < 10 <= vb[0]:
            return float(va[0] + vb[0])
    for pre, tens in _IT_TENS.items():
        rest = tok[len(pre):].lstrip("aie") if tok.startswith(pre) else ""
        if rest in _IT_UNITS:
            return float(tens + _IT_UNITS[rest])
    return None


def _num_at(sp: Span, i: int) -> tuple[int, float, bool] | None:
    """(length, value, weak) of one number at token i."""
    tok = sp.t[i]
    if _DIGITS.fullmatch(tok):
        return 1, float(tok.replace(",", ".")), False
    m = T["number"].match_at(sp.t, i)
    if m:
        return m[0], float(m[1]), m[0] == 1 and tok in _WEAK
    v = _compound(tok)
    return (1, v, False) if v is not None else None


def numbers(sp: Span) -> list[tuple[int, int, float, bool]]:
    """(start, end, value, weak) of each number over unread tokens, in reading order.

    Number words add up ("twenty five", "vingt et un", "one and a half"); digits never add to
    each other, so "40 and 60" stays two numbers."""
    out, i = [], 0
    while i < len(sp.t):
        m = None if sp.used[i] else _num_at(sp, i)
        if not m:
            i += 1
            continue
        n, v, weak = m
        j, digit = i + n, _DIGITS.fullmatch(sp.t[i]) is not None
        while j < len(sp.t) and not sp.used[j]:
            k = j + 1 if sp.t[j] in ("and", "und", "et", "e") else j
            nxt = _num_at(sp, k) if k < len(sp.t) else None
            if not nxt:
                break
            if nxt[1] == 0.5 and (sp.t[k - 1] in ("and", "und", "et", "e") or k == j):
                v, weak = v + 0.5, False
            elif nxt[1] == 100 and not digit and v < 10:
                v *= 100
            elif not digit and not _DIGITS.fullmatch(sp.t[k]) and v >= 20 and v % 10 == 0 \
                    and nxt[1] < 10:
                v += nxt[1]
            else:
                break
            j = k + nxt[0]
            weak = False
        out.append((i, j, v, weak))
        i = j
    return out


def quantities(sp: Span) -> list[dict]:
    """Numbers with the unit after them, or before them ("km 40"). Marks what it reads.

    Each is {"value", "unit" (km mi h min m elev pct day week, or None), "pre"}."""
    out = []
    for i, j, v, weak in numbers(sp):
        if any(sp.used[i:j]):
            continue
        # A km post puts its unit first ("km 40", "du km 40 au km 60").
        pre = i > 0 and not sp.used[i - 1] and T["unit"].whole(sp.t[i - 1:i]) == ("km", 0)
        if pre:
            sp.mark(i - 1, i)
        # The unit may follow a qualifier that is already read ("les 20 derniers km") or a
        # hyphen ("7-day", "5-tägige").
        while not pre and j < len(sp.t) and (sp.used[j] or sp.t[j] == "-" and j + 1 < len(sp.t)
                                            and T["unit"].match_at(sp.t, j + 1)) \
                and sp.t[j] not in _ORD_SUFFIX:
            j += 1
        u = None if pre or j >= len(sp.t) or sp.used[j] else T["unit"].match_at(sp.t, j)
        if pre:
            unit = "km"
        elif u:
            sp.mark(j, j + u[0])
            unit = u[1]
            # "1 h 30" and "1h30"
            if unit == "h" and j + u[0] < len(sp.t) and _DIGITS.fullmatch(sp.t[j + u[0]]):
                k = j + u[0]
                v += float(sp.t[k]) / 60
                sp.mark(k, k + 1)
        else:
            unit = None
        if weak and unit is None:
            continue
        sp.mark(i, j)
        # "4." in "4. Tag" and "4th"
        if j < len(sp.t) and sp.t[j] in _ORD_SUFFIX and not sp.used[j]:
            sp.mark(j, j + 1)
        out.append({"value": v, "unit": unit, "pre": pre})
    return out


def _to(q: dict, dims: tuple[str, ...]) -> dict | None:
    """A schema Quantity in one of `dims` (km, h, m, %). "m" is elevation where `dims` allows
    it, else a distance."""
    v, u = q["value"], q["unit"]
    conv = {"km": ("km", 1), "mi": ("km", 1.609344), "m": ("km", 0.001), "h": ("h", 1),
            "min": ("h", 1 / 60), "elev": ("m", 1), "pct": ("%", 1)}
    if "m" in dims:
        conv["m"] = ("m", 1)
    if u not in conv or conv[u][0] not in dims:
        return None
    unit, f = conv[u]
    return {"value": round(v * f, 6), "unit": unit}


def quantity(sp: Span, dims: tuple[str, ...], *fillers: str) -> dict | None:
    """The first quantity of the span in one of `dims`; "every hour" is 1 h. Filler words are
    read after the numbers and units, so a typo match cannot eat them ("hours" is not "jours")."""
    found = next((r for q in quantities(sp) if q["unit"] and (r := _to(q, dims))), None)
    if found is None:
        found = next((r for u in sp.find("unit") if (r := _to({"value": 1, "unit": u}, dims))),
                     None)
    sp.scan(*fillers)
    return found


# --- slot parsers ------------------------------------------------------------------------------

# "80 km a day", "80 km daily", "km/Tag", "in 80-km-Etappen aufteilen"
_PER_DAY = ("every", "day_noun", "day_value", "split_verb")
_DAY_VALUES = {"today": ("today", None), "tomorrow": ("tomorrow", None),
               "tonight": ("today", "end"), "thismorning": ("today", "start"),
               "todaymiddle": ("today", "middle"),
               "every": ("every", None), "everynight": ("every", "end"),
               "everymorning": ("every", "start")}


def parse_day(sp: Span) -> tuple[Any, str | None, list[int]]:
    """(day, part, further day numbers) of a day phrase: "end of day 4", "tomorrow",
    "every night", "4. Tag", "troisième jour"."""
    day, part, ints = None, None, []
    every = noun = night = morgen = False
    # Numbers first, so that a typo match cannot take them: "five" is not "fine".
    for q in quantities(sp):
        if q["unit"] in (None, "day") and q["value"] >= 1 and q["value"] == int(q["value"]):
            ints.append(int(q["value"]))
    for i, j, name, v in sp.scan("day_value", "part", "ordinal", "every", "day_noun",
                                 "night_noun"):
        if name == "day_value":
            morgen |= sp.t[i:j] == ["morgen"]
            d, p = _DAY_VALUES[v]
            day = day or d
            part = part or p
        elif name == "part":
            part = part or v
        elif name == "ordinal":
            ints.append(int(v))
        elif name == "every":
            every = True
        elif name == "day_noun":
            noun = True
        else:
            night = True
    if night:
        part = part or "end"
    if day is None and every and (noun or night):
        day = "every"
    if morgen and day == "tomorrow" and ints:
        day, part = None, part or "start"      # "Morgen von Tag 4" is its morning
    if day is None and ints:
        day = ints.pop(0)
    return day, part, ints


def parse_point(text: str, lang: str | None = None) -> tuple[dict | None, list[str]]:
    """One point parser for FROM, TO, VIA, NEAR, POINT, BEFORE and AFTER."""
    sp = Span(text, lang)
    if not sp.t:
        return None, [text]
    # "the top of the Belchen", "in cima al Mottarone": the name, never a summit kind
    for k in range(len(sp.t)):
        if (m := T["top_of"].match_at(sp.t, k)) and all(t in _LEAD for t in sp.t[:k]):
            rest = sp.t[k + m[0]:]
            if not rest:
                return None, []
            return parse_point(text[sp.words[sp.wi[k + m[0]]][1]:], lang)
        if sp.t[k] not in _LEAD:
            break
    k = 0
    while True:
        # "in meiner Nähe", "the starting point": test before a leading word is stripped
        core = sp.t[k:]
        if T["here"].whole(core) or (T["scope"].whole(core) or [None])[0] == "here":
            return {"here": True}, []
        if p := T["plan"].whole(core):
            return {"plan": p[0]}, []
        if k == len(sp.t) - 1 or sp.t[k] not in _LEAD:
            break
        k += 1
    for probe in (_day_point, _along_point):
        if (r := probe(text)) is not None:
            return r, []
    # Determiners around a kind: "the nearest pharmacy", "la pharmacie la plus proche".
    lo, hi = k, len(sp.t)
    while lo < hi and (m := T["nearest"].match_at(sp.t, lo)) and lo + m[0] < hi:
        lo += m[0]
    for cut in range(lo + 1, hi):
        m = T["nearest"].match_at(sp.t, cut)
        if m and cut + m[0] == hi:
            hi = cut
            break
    if lo < hi:
        # A bare word stays a name when it only resembles a kind ("Bern" is not "Berg").
        table = "kind" if (lo > 0 or hi < len(sp.t)) else "kind_exact"
        if kd := T[table].whole(sp.t[lo:hi]):
            return {"kind": SENSE.get(lang, {}).get(" ".join(sp.t[lo:hi]), kd[0])}, []
    name = name_of(text)
    return ({"name": name}, []) if name else (None, [text])


def _day_point(text: str) -> dict | None:
    if not Span(text).scan("day_value", "day_noun", "night_noun", strict=True):
        return None      # a bare number is not a day
    sp = Span(text)
    day, part, extra = parse_day(sp)
    if day is None or extra or sp.leftover():
        return None
    return {"day": day, "part": part} if part else {"day": day}


def _along_point(text: str) -> dict | None:
    sp = Span(text)
    a = parse_along(sp)
    return {"along": a} if a and "at" in a and not sp.leftover() else None


def name_of(text: str) -> str | None:
    """The name as typed, without leading lower-case prepositions, articles or "nearest"."""
    ws = words.split(text)
    k = 0
    while k < len(ws) - 1 and ws[k][0].islower() and fold(ws[k][0]) in _LEAD:
        k += 1
    if k == len(ws):
        return None
    start = ws[k][1]
    m = re.match(r"(l|d|dell|dall|all|nell|sull|qu)['’](?=\w)", text[start:])
    if m and text[start].islower():
        start += m.end()
    name = text[start:].strip().rstrip("?!,;:").strip()
    return name or None


def parse_along(sp: Span) -> dict | None:
    """An Along from qualifiers, numbers and units: "first 20 km", "km 40 to 60", "next 2 h"."""
    quals = set(sp.find("qualifier"))
    qs = [q for q in quantities(sp) if q["unit"] in (None, "km", "mi", "m", "h", "min")]
    if not qs and quals:
        # "the next hour", "die nächste Stunde": one unit
        qs = [{"value": 1, "unit": u, "pre": False} for u in sp.find("unit")][:1]
    units = [q["unit"] for q in qs if q["unit"]]
    if not qs or not units:
        return None
    # "40 to 60 km" and "km 40 to 60": one unit for the whole range.
    fill = units[0] if qs[0]["pre"] else units[-1]
    vals = [_to({**q, "unit": q["unit"] or fill}, ("km", "h")) for q in qs[:2]]
    if None in vals or len({v["unit"] for v in vals}) != 1:
        return None
    if "between" in quals and len(vals) == 1:
        quals = quals - {"between"} | {"in"}     # Italian "tra 4 ore" is a mark
    if quals & {"next", "from_here"}:
        ref = "here"
    elif quals & {"last", "from_end"}:
        ref = "end"
    elif quals & {"first", "from_start"} or ("after" in quals and not qs[0]["pre"]):
        ref = "start"
    elif quals & {"in", "within"} and not qs[0]["pre"]:
        ref = "here"
    else:
        ref = "km"
    if len(vals) == 2:
        return {"ref": ref, "from": vals[0], "to": vals[1]}
    if "from" in quals and "to" not in quals:
        return {"ref": ref, "from": vals[0]}
    if quals & {"first", "last", "next", "within", "to"}:
        return {"ref": ref, "to": vals[0]}
    return {"ref": ref, "at": vals[0]}


def parse_open(sp: Span) -> dict | str | None:
    """{"weekday"}, {"now"}, {"day"}, or "bare" for a lone "open"."""
    wk, now, opened = sp.find("weekday"), sp.find("now"), sp.find("open")
    wk = wk or sp.find("weekday_short")
    if wk:
        return {"weekday": wk[0]}
    if now:
        return {"now": True}
    day, _, _ = parse_day(sp)
    if day is not None:
        return {"day": day}
    return "bare" if opened else None


_DAYS_WORD = re.compile(r"(.+?)(tages|tage|tagig\w*|wochig\w*)(tour|reise|fahrt|trip|runde)?")


def parse_days(sp: Span) -> int | None:
    """A day count: "in 7 days", "a week", "fünf Tage", "5-tägige", "Viertagestour"."""
    sp.scan("trip_noun", "split_verb", strict=True)
    for q in quantities(sp):
        n = q["value"] * (7 if q["unit"] == "week" else 1)
        if q["unit"] in (None, "day", "week") and n >= 1 and n == int(n):
            return int(n)
    for k, tok in enumerate(sp.t):
        m = None if sp.used[k] else _DAYS_WORD.fullmatch(tok)
        if m and (n := T["number"].whole([m.group(1)])) and n[0] >= 1:
            sp.mark(k, k + 1)
            return int(n[0]) * (7 if m.group(2).startswith("woch") else 1)
    return None


def first(table: str):
    """A parser for a slot that is one word-list value: KIND, SCOPE."""
    return lambda sp: next(iter(sp.find(table)), None)


def _compounds(sp: Span, head: str, tail: str) -> list[Any]:
    """Head values of the unread German compounds with that tail: "Wasserstopps" -> water."""
    out = []
    for k, tok in enumerate(sp.t):
        c = None if sp.used[k] else compound(tok, head, "compound_tail")
        if c and c[1] == tail:
            sp.mark(k, k + 1)
            out.append(c[0])
    return out


def parse_what(sp: Span) -> list[str]:
    """Up to all kinds of a list: "water and campsites", "Trinkwasser, Betten und Cafés"."""
    return list(dict.fromkeys(sp.kinds() + _compounds(sp, "kind", "stop")))


_STRETCH_GOAL = {"climb": "least_climbing", "steep": "least_climbing",
                 "unpaved": "least_unpaved", "unknown_surface": "least_unpaved",
                 "pushing": "least_unpaved"}


def parse_stretch(sp: Span) -> str | None:
    found = sp.scan("stretch", "gap", "kind")
    gaps = _compounds(sp, "kind", "gap")
    sp.scan("stretch_filler", strict=True)
    for name in ("stretch", "kind"):
        vals = [v for _, _, n, v in found if n == name]
        if vals:
            return vals[0] if name == "stretch" else f"gap:{vals[0]}"
    return f"gap:{gaps[0]}" if gaps else None


def parse_goal(sp: Span) -> str | None:
    """A goal phrase, or "avoid" plus a stretch kind: "Anstiege vermeiden", "ohne Feldwege"."""
    goal = next(iter(sp.find("goal")), None)
    if goal is None and sp.scan("avoid", strict=True):
        goal = next((_STRETCH_GOAL[v] for v in sp.find("stretch") if v in _STRETCH_GOAL), None)
    sp.scan("route_noun", strict=True)
    return goal


def parse_bike(sp: Span) -> str | None:
    """A bike phrase, or a German compound: "Rennradtour", "Gravelstrecke", "MTB-Strecke"."""
    bike = next(iter(sp.find("bike") + _compounds(sp, "bike", "trip")), None)
    sp.scan("trip_noun", "route_noun", "compound_tail")
    return bike


# --- assembly ----------------------------------------------------------------------------------

class _Req:
    """The spans of one sentence, and what the request took from them."""

    def __init__(self, text: str, labels: list[str]):
        self.lang = lang_of(text)
        self.spans = [(slot, s) for slot, s in words.spans(text, labels) if not _empty(s)]
        self.taken = [False] * len(self.spans)
        self.ign: list[tuple[int, str]] = []

    def take(self, slot: str) -> list[tuple[int, str]]:
        out = [(i, s) for i, (sl, s) in enumerate(self.spans) if sl == slot and not self.taken[i]]
        for i, _ in out:
            self.taken[i] = True
        return out

    def ignore(self, i: int, texts: list[str]) -> None:
        self.ign.extend((i, t) for t in texts)

    def one(self, slot: str, parse, *args):
        """The value of the first span of `slot` that parses; the rest is ignored."""
        val = None
        for i, text in self.take(slot):
            if val is not None:
                self.ignore(i, [text])
                continue
            sp = Span(text, self.lang)
            val = parse(sp, *args)
            self.ignore(i, sp.leftover() if val is not None else [text])
        return val

    def point(self, slot: str, n: int = 1, kinds: bool = True) -> list[dict]:
        """Up to n points of the slot's spans. A name, a mark or "here" beats a kind: of
        "top" and "Belchen", the Belchen is meant."""
        found = []
        for i, text in self.take(slot):
            p, rest = parse_point(text, self.lang)
            if p and "kind" in p and not kinds:
                # "fountains in Essen": a capitalised bare word is a town, a kind is level 3
                bare = text.strip()[:1].isupper() and Span(text).t[0] not in _LEAD
                p, rest = ({"name": name_of(text)}, []) if bare else (None, [text])
            self.ignore(i, rest)
            if p:
                found.append((i, text, p))
        keep = sorted(found, key=lambda f: "kind" in f[2])[:n]
        for f in found:
            if f not in keep:
                self.ignore(f[0], [f[1]])
        return [p for f in found if f in keep for p in [f[2]]]

    def where(self) -> dict:
        w: dict[str, Any] = {}
        days = self.take("DAY")
        for i, text in days:
            sp = Span(text)
            day, part, extra = parse_day(sp)
            rest = sp.leftover()
            if day is not None and "day" in w and w["day"] != day:
                rest = [text]
            elif day is not None:
                w["day"] = day
            if part and "part" not in w:
                w["part"] = part
            self.ignore(i, rest + [str(x) for x in extra])
        if "part" in w and "day" not in w:
            del w["part"]
            for i, text in days:
                self.ignore(i, [text])
        if (s := self.one("SCOPE", first("scope"))) is not None:
            w["scope"] = s
        # "near a supermarket" nests a second search (level 3); "before the next pass" does not.
        if near := self.point("NEAR", 2, kinds=False):
            w["near"] = near
        if (a := self.one("ALONG", parse_along)) is not None:
            w["along"] = a
        for slot in ("BEFORE", "AFTER"):
            if p := self.point(slot):
                w[slot.lower()] = p[0]
        return w

    def done(self, req: dict) -> dict:
        for i, (_, text) in enumerate(self.spans):
            if not self.taken[i]:
                self.ign.append((i, text))
        req["ignored"] = [t for _, t in sorted(self.ign, key=lambda x: x[0])]
        return req


def _empty(text: str) -> bool:
    """A span of stop words only, not a here-word ("hier", "me")."""
    t = Span(text).t
    return all(x in _EMPTY for x in t) and not (
        t and (T["here"].whole(t) or (T["scope"].whole(t) or [None])[0] == "here"))


def decode(text: str, intent: str, labels: list[str]) -> dict:
    r = _Req(text, labels)
    build = _BUILD.get(intent)
    req = build(r) if build else None
    if req is None:
        return {"type": "none", "ignored": []}
    req = r.done(req)
    try:
        schema.validate(req)
    except schema.Invalid:
        return {"type": "none", "ignored": []}
    return req


def _places(r: _Req):
    what: list[str] = []
    for i, text in r.take("WHAT"):
        sp = Span(text, r.lang)
        new = [k for k in parse_what(sp) if k not in what]
        room = 3 - len(what)
        what += new[:room]
        # a fourth kind, or a span with none, shows as not understood
        r.ignore(i, sp.leftover() if new and len(new) <= room else [text])
    if not what:
        return None
    req: dict[str, Any] = {"type": "places", "what": what}
    w = r.where()
    if w:
        req["where"] = w
    op = r.one("OPEN", parse_open)
    if op == "bare":
        op = {"day": w["day"]} if w.get("day") not in (None, "every") else {"now": True}
    if op:
        req["open"] = op
    if (rad := r.one("RADIUS", quantity, ("km",), "limit")) is not None:
        req["radius"] = rad
    return req


def _place(r: _Req):
    names = r.take("NAME")
    if not names:
        return None
    name = name_of(names[0][1])
    for i, text in names[1:]:
        r.ignore(i, [text])
    if not name:
        return None
    req = {"type": "place", "name": name}
    if near := r.point("NEAR"):
        req["near"] = near[0]
    return req


def _route(r: _Req):
    to = r.point("TO")
    if not to:
        return None
    req: dict[str, Any] = {"type": "route", "to": to[0]}
    if fr := r.point("FROM"):
        req["from"] = fr[0]
    if via := r.point("VIA", 2):
        req["via"] = via
    if (b := r.one("BIKE", parse_bike)) is not None:
        req["bike"] = b
    if (g := r.one("GOAL", parse_goal)) is not None:
        req["goal"] = g
    if (d := r.one("DAYS", parse_days)) is not None:
        req["days"] = d
    if (pd := r.one("PER_DAY", quantity, ("km", "h"), *_PER_DAY)) is not None:
        req["per_day"] = pd
    return req


def _stretches(r: _Req):
    spans = r.take("WHAT")
    sp = Span(" ".join(t for _, t in spans))     # "gaps" and "water" may come apart
    what = parse_stretch(sp) if spans else None
    if what is None:
        return None
    r.ignore(spans[0][0], sp.leftover())
    req: dict[str, Any] = {"type": "stretches", "what": what}
    if w := r.where():
        req["where"] = w
    if (m := r.one("MIN", _min)) is not None:
        req["min"] = m
    return req


def _min(sp: Span):
    sp.scan("threshold")
    for k, tok in enumerate(sp.t):     # "ovr", "uber": short words, but only here
        if not sp.used[k] and len(tok) == 4 and min(OSA.distance(tok, w)
                                                     for w in ("over", "uber")) <= 1:
            sp.mark(k, k + 1)
    long = bool(sp.scan("length_word", strict=True))
    return quantity(sp, ("km", "%") if long else ("m", "km", "%"))


def _end_day(r: _Req):
    days = r.take("DAY")
    day = None
    for i, text in days:
        sp = Span(text)
        d, _, extra = parse_day(sp)
        if day is None and d is not None:
            day = d
            r.ignore(i, sp.leftover() + [str(x) for x in extra])
        else:
            r.ignore(i, [text])
    at = r.point("POINT") or _fallback_point(r)
    if day is None or not at:
        return None
    return {"type": "end_day", "day": day, "at": at[0]}


def _fallback_point(r: _Req) -> list[dict]:
    """A mark or a kind where the tagger gave no POINT: "end day 3 at km 120"."""
    for i, text in r.take("ALONG"):
        if (p := _along_point(text)) is not None:
            return [p]
        r.ignore(i, [text])
    what = r.one("WHAT", parse_what)
    return [{"kind": what[0]}] if what else []


def _add_point(r: _Req):
    kind = r.one("KIND", first("point_kind"))
    pts = r.take("POINT")
    point = None
    for i, text in pts:
        if point is not None:
            r.ignore(i, [text])
        elif split := _split_point(text):
            point, kind = split[0], kind or split[1]
        else:
            point, rest = parse_point(text, r.lang)
            r.ignore(i, rest)
    if point is None:
        fb = _fallback_point(r)
        point = fb[0] if fb else None
    if point is None:
        return None
    req: dict[str, Any] = {"type": "add_point", "point": point}
    if kind:
        req["kind"] = kind
    if (e := r.one("EVERY", quantity, ("km", "h"), "every")) is not None:
        req["every"] = e
    if w := r.where():
        req["where"] = w
    return req


def _split_point(text: str) -> tuple[dict, str] | None:
    """A kind and a point kind in one span, either order: "coffee stops", "Bäcker-Halt",
    "arrêts boulangerie", "soste pranzo", "Mittagspause"."""
    t = Span(text).t
    lo = 0
    while lo < len(t) - 1 and t[lo] in _LEAD:
        lo += 1
    for cut in range(lo + 1, len(t)):
        for a, b in ((t[lo:cut], t[cut:]), (t[cut:], t[lo:cut])):
            kd, pk = T["kind"].whole(a), T["point_kind"].whole(_strip_lead(b))
            if pk and kd and pk[1] <= 1:
                return {"kind": kd[0]}, pk[0]
    c = compound(t[lo], "kind", "compound_tail") if len(t) - lo == 1 else None
    return ({"kind": c[0]}, "stop") if c and c[1] == "stop" else None


def _strip_lead(t: list[str]) -> list[str]:
    """Without the articles of "arrêts pour la boulangerie"."""
    k = 0
    while k < len(t) - 1 and t[k] in _LEAD:
        k += 1
    return t[k:]


def _remove_point(r: _Req):
    for i, (slot, text) in enumerate(r.spans):
        if slot == "POINT" and (split := _split_point(text)):     # "remove the coffee stop"
            r.taken[i] = True
            return {"type": "remove_point", "point": split[0]}
    p = r.point("POINT") or _fallback_point(r)
    return {"type": "remove_point", "point": p[0]} if p else None


def _split(r: _Req):
    req: dict[str, Any] = {"type": "split"}
    days = r.one("DAYS", parse_days)
    per_day = r.one("PER_DAY", quantity, ("km", "h"), *_PER_DAY)
    w = r.where()
    if days is not None:
        req["days"] = days
    elif per_day is not None:
        req["per_day"] = per_day
    elif isinstance(w.get("day"), int):
        req["days"] = 2      # "split day 3" can only mean in two
    else:
        return None
    if w:
        req["where"] = w
    return req


def _join(r: _Req):
    nums: list[tuple[int, int]] = []
    for i, text in r.take("DAY"):
        sp = Span(text)
        day, _, extra = parse_day(sp)
        got = ([day] if isinstance(day, int) else []) + extra
        r.ignore(i, sp.leftover() if got else [text])
        nums += [(i, n) for n in got]
    if not nums:
        return None
    # "merge day 4 into day 3" and "join day 3 and 4" both join 3 and 4
    day = min(n for _, n in nums)
    r.ign += [(i, str(n)) for i, n in nums if n not in (day, day + 1)]
    return {"type": "join", "day": day}


def _reroute(r: _Req):
    req: dict[str, Any] = {"type": "reroute", "where": r.where() or {"scope": "route"}}
    if (g := r.one("GOAL", parse_goal)) is not None:
        req["goal"] = g
    if (b := r.one("BIKE", parse_bike)) is not None:
        req["bike"] = b
    return req if "goal" in req or "bike" in req else None


_BUILD = {
    "places": _places, "place": _place, "route": _route, "stretches": _stretches,
    "end_day": _end_day, "add_point": _add_point, "remove_point": _remove_point,
    "split": _split, "join": _join, "reverse": lambda r: {"type": "reverse"},
    "reroute": _reroute,
}
