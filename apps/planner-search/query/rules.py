"""Rules-only baseline: keyword rules tag the sentence, and decode.py turns the tags into a
request. The model and this baseline share the decoder, so a comparison measures the tagging.

Four passes over the folded tokens: day phrases, quantities, single words and phrases from the
word lists, then the request type from keywords. Role prepositions ("to", "nach", "vers", "al")
give the chunk after them a slot; unknown words become a name, a FROM or IGNORED.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

import words
from decode import _DAYS_WORD, Span, _num_at, decode
from lexicon import STOP, TABLES as T, WORDS, compound, tokens_of, toks

_CONNECT = {"of", "von", "vom", "du", "de", "del", "della", "dello", "di", "the", "der", "des",
            "la", "le", "il", "l'", "d'", "am", "a", "al", "alla", "au", "at"}
_AND = {"and", "und", "et", "e"}
_RANGE = {"to", "-", "and", "bis", "und", "a", "au", "et", "e", "al", "fino"}
_QTY_LEAD = (tokens_of("qualifier") | tokens_of("limit") | tokens_of("threshold")
             | tokens_of("every") | {"the", "der", "den", "die", "les", "le", "i", "gli", "il"})
_QUIET = (STOP | tokens_of("qualifier") | tokens_of("nearest") | tokens_of("day_noun")
          | tokens_of("route_noun") | tokens_of("trip_noun"))
# Qualifiers that make a quantity a mark or a stretch on the plan, not a radius.
_MARK = {t for v in ("first", "last", "next", "after", "between", "from_here", "from_start",
                     "from_end") for ts in WORDS["qualifier"][v].values() for x in ts
         for t, _ in toks(x)} - STOP
_WORDS = ("intent", "plan", "open", "now", "weekday", "bike", "goal", "stretch", "gap",
          "scope", "kind", "point_kind", "prep")
# Chunks after a preposition end at these.
_BOUND = {"day", "qty", "intent", "open", "now", "weekday", "bike", "goal", "prep",
          "scope", "point_kind"}


@dataclass
class Item:
    i: int
    j: int
    cat: str
    val: Any = None
    slot: str | None = None
    feats: set = field(default_factory=set)


def parse(text: str) -> dict:
    intent, labels = tag(text)
    return decode(text, intent, labels)


def tag(text: str) -> tuple[str, list[str]]:
    sp = Span(text)
    items = _days(sp) + _quantities(sp)
    items += [Item(i, j, c, v) for i, j, c, v in sp.scan(*_WORDS, strict=True)]
    items += [Item(i, j, c, v) for i, j, c, v in sp.scan("kind")
              if sp.t[i] not in STOP]
    items += _compounds(sp, items)
    items = [it for it in items if not _name_link(sp, it, items)]
    items.sort(key=lambda it: it.i)
    # "with a pool", "near the": a second preposition in a row is an article.
    items = [it for k, it in enumerate(items) if not (
        k and it.cat == "prep" and items[k - 1].cat == "prep" and items[k - 1].j == it.i)]
    intent = _intent(sp, items)
    _slots(sp, items, intent)
    return intent, _labels(sp, items)


# --- pass 1 and 2: day phrases and quantities ---------------------------------------------------

def _days(sp: Span) -> list[Item]:
    out, t, n = [], sp.t, len(sp.t)
    for i, j, _, v in sp.scan("day_value", strict=True):
        out.append(Item(i, j, "day", v))
    i = 0
    while i < n:
        j = None
        noun = _match(sp, "day_noun", i) or _match(sp, "night_noun", i)
        if noun and i + noun < n and _num_at(sp, i + noun):
            j = i + noun + _num_at(sp, i + noun)[0]
            # "days 3 and 4", "Tag 3 und 4"
            if j + 1 < n and t[j] in ("and", "und", "et", "e") and _num_at(sp, j + 1):
                j += 2
        elif (o := _match(sp, "ordinal", i)) and _match(sp, "day_noun", i + o):
            j = i + o + _match(sp, "day_noun", i + o)
        elif _num_at(sp, i) and i + 1 < n and t[i + 1] in tokens_of("ordinal_suffix") \
                and _match(sp, "day_noun", i + 2):
            j = i + 2 + _match(sp, "day_noun", i + 2)
        if j and not any(sp.used[i:j]):
            out.append(Item(i, j, "day"))
            sp.mark(i, j)
            i = j
        else:
            i += 1
    for it in out:
        # "end of day 4", "Ende Tag 4", "day 4 end", "tomorrow evening"
        for start in range(max(0, it.i - 4), it.i):
            m = _match(sp, "part", start)
            if m and not any(sp.used[start:start + m]) and all(
                    x in _CONNECT for x in t[start + m:it.i]):
                sp.mark(start, it.i)
                it.i = start
                break
        if it.j < n and (m := _match(sp, "part", it.j)) and not any(sp.used[it.j:it.j + m]):
            sp.mark(it.j, it.j + m)
            it.j += m
    return out


_GENITIVE = {"de", "du", "des", "del", "della", "dello", "dei", "di", "da", "von", "vom", "of",
             "d'", "dell'"}
_LINK = _GENITIVE | {"la", "le", "sur", "am", "an", "im", "in", "en"}


def _name_link(sp: Span, it: Item, items: list[Item]) -> bool:
    """A preposition inside a name: "col du Galibier", "Frankfurt am Main"."""
    if it.cat != "prep" or it.j - it.i != 1 or sp.t[it.i] not in _LINK or it.i == 0:
        return False
    prev = next((x for x in items if x.i <= it.i - 1 < x.j), None)
    nxt = sp.words[sp.wi[it.j]][0] if it.j < len(sp.t) else ""
    # "col du Galibier" links a kind to a name; "shop in Freiburg" does not.
    ok = prev is None or (prev.cat == "kind" and sp.t[it.i] in _GENITIVE)
    return ok and sp.t[it.i - 1] not in _QUIET and nxt[:1].isupper()


def _compounds(sp: Span, items: list[Item]) -> list[Item]:
    """German compounds: "Wasserlücke" (a gap), "Wasserstopps" (a kind), "Rennradtour",
    "Viertagestour"; and a short weekday next to an open word ("Mo geöffnet")."""
    out = []
    opens = {k for it in items if it.cat == "open" for k in (it.i - 1, it.j)}
    for k, tok in enumerate(sp.t):
        if sp.used[k]:
            continue
        kind, bike = compound(tok, "kind", "compound_tail"), compound(tok, "bike", "compound_tail")
        days = _DAYS_WORD.fullmatch(tok)
        if kind and kind[1] in ("gap", "stop"):
            out.append(Item(k, k + 1, "gapkind" if kind[1] == "gap" else "kind", kind[0]))
        elif bike and bike[1] == "trip":
            out.append(Item(k, k + 1, "bike", bike[0]))
        elif days and T["number"].whole([days.group(1)]):
            out.append(Item(k, k + 1, "qty", "day"))
        elif k in opens and (w := T["weekday_short"].whole([tok])):
            out.append(Item(k, k + 1, "weekday", w[0]))
    return out


def _match(sp: Span, table: str, i: int) -> int:
    """Length of an exact match of `table` at token i, or 0."""
    if i >= len(sp.t) or sp.used[i]:
        return 0
    m = T[table].match_at(sp.t, i)
    return m[0] if m and m[2] <= 1 else 0


def _quantities(sp: Span) -> list[Item]:
    out, t, n = [], sp.t, len(sp.t)
    i = 0
    while i < n:
        num = None if sp.used[i] else _num_at(sp, i)
        if not num or (num[2] and not _match(sp, "unit", i + num[0])):
            i += 1
            continue
        start, j = i, i + num[0]
        unit = None
        if i > 0 and not sp.used[i - 1] and _unit(sp, i - 1) == "km":
            start, unit = i - 1, "km"
        elif j + 1 < n and t[j] == "-" and (m := _match(sp, "unit", j + 1)):      # "7-day"
            unit = _unit(sp, j + 1)
            j += 1 + m
        elif m := _match(sp, "unit", j + _skip_qualifiers(sp, j)):
            j += _skip_qualifiers(sp, j)
            unit = _unit(sp, j)
            j += m
        # "40 to 60 km", "km 40 à 60", "zwischen 40 und 60"
        k = j + 1 if j < n and t[j] in _RANGE else None
        if k is not None and _unit(sp, k) == "km":
            k += 1
        if k is not None and k < n and (num2 := _num_at(sp, k)):
            j = k + num2[0]
            if m := _match(sp, "unit", j):
                unit = unit or _unit(sp, j)
                j += m
        if unit is None:
            out.append(Item(i, i + num[0], "num", num[1]))
            i += num[0]
            continue
        it = Item(start, j, "qty", unit)
        while it.i > 0 and not sp.used[it.i - 1] and t[it.i - 1] in _QTY_LEAD:
            it.i -= 1
        lead = t[it.i:start]
        for cat in ("every", "limit", "threshold", "qualifier"):
            if any(x in tokens_of(cat) for x in lead):
                it.feats.add(cat)
        if set(sp.t[it.i:it.j]) & _MARK:
            it.feats.add("mark")
        # "80 km a day", "100 km pro Tag"
        k = j + 1 if j < n and t[j] in tokens_of("every") | {"a", "an", "al", "au", "/"} else j
        if (m := _match(sp, "day_noun", k)) and k > j:
            it.j = k + m
            it.feats.add("per")
        sp.mark(it.i, it.j)
        out.append(it)
        i = it.j
    return out


def _skip_qualifiers(sp: Span, j: int) -> int:
    """Qualifier words between a number and its unit: "les 20 derniers km"."""
    k = j
    while k < len(sp.t) and sp.t[k] in _MARK:
        k += 1
    return k - j


def _unit(sp: Span, i: int):
    m = T["unit"].match_at(sp.t, i) if i < len(sp.t) else None
    return m[1] if m and m[2] <= 1 else None


# --- pass 3: request type ----------------------------------------------------------------------

def _intent(sp: Span, items: list[Item]) -> str:
    cats = {it.cat for it in items}
    kw = {it.val for it in items if it.cat == "intent"}
    roles = [it.val for it in items if it.cat == "prep"]
    day = "day" in cats
    if "question" in kw:
        return "none"
    if "reverse" in kw:
        return "reverse"
    if "join" in kw and day:
        return "join"
    if "split" in kw:
        return "split"
    if "remove" in kw:
        return "remove_point"
    if "end" in kw and day:
        return "end_day"
    first = items[0] if items else None
    verb = T["intent"].match_at(sp.t, 0) if sp.t else None
    if first and first.cat == "day" and first.i == 0 and verb and verb[1] == "end" and (
            "at" in roles or "to" in roles):
        # "end day 4 at X": the verb, not the end of the day
        first.i += 1
        return "end_day"
    if "add" in kw or (first and first.cat == "prep" and first.val == "via" and first.i == 0):
        return "add_point"
    if "goal" in cats and "to" not in roles and "route" not in kw:
        return "none"         # a goal alone asks to re-route the plan: not in the language
    if "gapkind" in cats or cats & {"stretch", "gap"} and cats & {"kind", "stretch"}:
        return "stretches"
    unknown = _chunks(sp, items)
    if "route" in kw or "bike" in cats or ("to" in roles and unknown) or "via" in roles:
        return "route"
    lead = next((k for k in range(len(sp.t)) if sp.t[k] not in _QUIET), None)
    head = next((it for it in items if it.i == lead), None)
    covered = {k for it in items for k in range(it.i, it.j)}
    if cats <= {"kind", "prep", "num"} and head and head.cat == "kind" and any(
            a >= head.j and all(sp.t[k] in STOP and k not in covered for k in range(head.j, a))
            and sp.words[sp.wi[a]][0][:1].isupper() for a, _ in unknown):
        return "place"        # "Hotel Krone", "Col du Galibier"
    if "kind" in cats:
        return "places"
    if unknown and not cats - {"prep", "scope"}:
        return "place"
    return "none"


def _chunks(sp: Span, items: list[Item]) -> list[tuple[int, int]]:
    """Runs of tokens outside every item, without stop words at their ends."""
    covered = [False] * len(sp.t)
    for it in items:
        for k in range(it.i, it.j):
            covered[k] = True
    out, i = [], 0
    while i < len(sp.t):
        if covered[i] or sp.t[i] in _QUIET:
            i += 1
            continue
        j = i
        while j < len(sp.t) and not covered[j]:
            j += 1
        while j > i and sp.t[j - 1] in _QUIET:
            j -= 1
        out.append((i, max(j, i + 1)))
        i = max(j, i + 1)
    return out


# --- pass 4: slots -----------------------------------------------------------------------------

_ROLE = {
    "route": {"to": "TO", "from": "FROM", "via": "VIA", "after": "TO"},
    "places": {"near": "NEAR", "at": "NEAR", "before": "BEFORE", "after": "AFTER",
               "between": "NEAR", "from": "NEAR", "to": "NEAR"},
    "stretches": {"near": "NEAR", "at": "NEAR", "before": "BEFORE", "after": "AFTER",
                  "between": "NEAR"},
    "place": {"near": "NEAR", "at": "NEAR"},
    "end_day": {"at": "POINT", "to": "POINT", "near": "POINT"},
    "add_point": {"at": "POINT", "to": "POINT", "via": "POINT", "near": "NEAR",
                  "before": "BEFORE", "after": "AFTER"},
    "remove_point": {"at": "POINT", "to": "POINT", "from": "POINT", "near": "POINT"},
}
_SIMPLE = {"day": "DAY", "bike": "BIKE", "goal": "GOAL", "scope": "SCOPE",
           "point_kind": "KIND", "open": "OPEN", "now": "OPEN", "weekday": "OPEN"}


def _slots(sp: Span, items: list[Item], intent: str) -> None:
    roles = _ROLE.get(intent, {})
    for it in items:
        it.slot = _SIMPLE.get(it.cat)
        if it.cat == "scope" and intent in ("route", "place", "join", "reverse", "remove_point",
                                             "end_day"):
            it.slot = None       # "reverse the route": a word for the request type
        if it.cat == "qty":
            it.slot = _qty_slot(it, intent)
        if it.cat == "num" and intent == "split":
            it.slot = "DAYS"      # "split the trip in 3"
        if it.cat == "point_kind" and intent != "add_point":
            it.slot = None
        elif it.cat in ("kind", "stretch", "gap", "gapkind"):
            what = intent == "stretches" or (intent == "places" and it.cat == "kind")
            it.slot = "WHAT" if what else None
    # The chunk after a role preposition: its kind and name words together.
    chunks = _chunks(sp, [x for x in items if x.cat in _BOUND or x.cat == "prep"])
    taken: set[int] = set()
    covered = {k for x in items for k in range(x.i, x.j)}
    point_seen = False
    for it in list(items):
        if it.cat != "prep":
            continue
        role = "with" if it.val == "with" else roles.get(it.val)
        ch = next((c for c in chunks if c[0] >= it.j and c[0] not in taken and all(
            sp.t[k] in _QUIET and k not in covered for k in range(it.j, c[0]))), None)
        if m := _here_at(sp, it.j):
            ch = (it.j, it.j + m)
        plan = next((x for x in items if x.cat == "plan" and x.i >= it.j), None)
        if plan and all(sp.t[k] in STOP for k in range(it.j, plan.i)):
            ch = (plan.i, plan.j)         # "from the starting point", "to the end"
        if ch is None:
            continue
        taken.add(ch[0])
        slot = "IGNORED" if role == "with" else role
        if slot is None:
            continue
        if slot == "POINT" and point_seen:
            slot = "NEAR"
        point_seen |= slot == "POINT"
        for x in items:
            if ch[0] <= x.i < ch[1]:
                x.slot = None
        # "between Basel and Bern": two points
        cut = next((k for k in range(ch[0] + 1, ch[1] - 1) if sp.t[k] in _AND), None)
        if it.val == "between" and cut:
            items += [Item(ch[0], cut, "chunk", slot=slot),
                      Item(cut + 1, ch[1], "chunk", slot=slot)]
            continue
        items.append(Item(it.i if role == "with" else it.j, ch[1], "chunk", slot=slot))
    # Kinds with no preposition name the point of a plan change.
    if intent in ("add_point", "end_day", "remove_point") and not point_seen:
        for it in items:
            if it.cat == "kind" and it.slot is None and not _inside(it, items):
                it.slot = "POINT"
                break
    if intent == "place":
        stop = next((x.i for x in items if x.cat == "prep"), len(sp.t))
        a = next((k for k in range(stop) if sp.t[k] not in _QUIET), None)
        if a is not None:
            for x in items:
                if a <= x.i < stop:
                    x.slot = None
            items.append(Item(a, stop, "chunk", slot="NAME"))
    # Unknown words: a name, the FROM of "Basel to Nice", the point of a change, or ignored.
    for a, b in _chunks(sp, items):
        nxt = next((x for x in sorted(items, key=lambda x: x.i) if x.i >= b), None)
        if intent == "place":
            slot = "NAME"
        elif intent == "route" and nxt and nxt.cat == "prep" and nxt.val == "to" \
                and not any(x.slot == "FROM" for x in items):
            slot = "FROM"
        elif intent in ("add_point", "end_day", "remove_point") and not any(
                x.slot == "POINT" for x in items):
            slot = "POINT"
        else:
            slot = "IGNORED"
        items.append(Item(a, b, "chunk", slot=slot))
    items.sort(key=lambda it: it.i)


def _here_at(sp: Span, i: int) -> int:
    """Length of a here-phrase after a preposition ("near me", "von hier"), or 0."""
    while i < len(sp.t) and sp.t[i] in STOP and not T["here"].match_at(sp.t, i):
        i += 1
    m = T["here"].match_at(sp.t, i) if i < len(sp.t) else None
    return m[0] if m else 0


def _inside(it: Item, items: list[Item]) -> bool:
    return any(x.cat == "chunk" and x.i <= it.i < x.j for x in items)


def _qty_slot(it: Item, intent: str) -> str | None:
    unit, f = it.val, it.feats
    if unit in ("day", "week"):
        return "DAYS" if intent in ("route", "split") else None
    if "every" in f and intent == "add_point":
        return "EVERY"
    if "per" in f or intent == "split":
        return "PER_DAY" if intent in ("route", "split") else None
    if "threshold" in f or unit in ("pct", "elev"):
        return "MIN" if intent == "stretches" else None
    if intent == "places" and "limit" in f and "mark" not in f:
        return "RADIUS"
    if intent in ("end_day", "add_point") and "every" not in f:
        return "POINT"
    return "ALONG" if intent in ("places", "stretches", "add_point") else None


def _labels(sp: Span, items: list[Item]) -> list[str]:
    n = len(words.split(sp.text))
    labels = ["O"] * n
    spans = sorted((it for it in items if it.slot), key=lambda it: it.i)
    # Neighbours of one slot with only stop words between are one span: "without a shop".
    merged: list[list] = []
    for it in spans:
        if merged and merged[-1][2] == it.slot and merged[-1][2] in ("WHAT", "OPEN") and all(
                sp.t[k] in STOP for k in range(merged[-1][1], it.i)):
            merged[-1][1] = max(merged[-1][1], it.j)
        else:
            merged.append([it.i, it.j, it.slot])
    for i, j, slot in merged:
        ws = sorted({sp.wi[k] for k in range(i, j)})
        if any(labels[w] != "O" for w in ws):
            continue
        for w in ws:
            labels[w] = ("B-" if w == ws[0] else "I-") + slot
    return labels
