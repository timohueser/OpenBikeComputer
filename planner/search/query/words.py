"""The word split that word labels refer to. Spans map back to the text by character offsets, so
a span keeps its own punctuation ("Hotel Krone St. Peter", "Saint-Michel")."""

from __future__ import annotations

import re

# A number ("3", "3,5", "10.5"), a word with inner hyphens or apostrophes ("Saint-Michel",
# "dell'Abetone", "l'arrivée"), or any other single character ("%", ".", "?").
_WORD = re.compile(r"\d+(?:[.,]\d+)?|[^\W\d_]+(?:[-'’][^\W\d_]+)*|\S")


def split(text: str) -> list[tuple[str, int, int]]:
    """(word, start, end) for each word of `text`."""
    return [(m.group(), m.start(), m.end()) for m in _WORD.finditer(text)]


def spans(text: str, labels: list[str]) -> list[tuple[str, str]]:
    """(slot, text) for each BIO span; an I- label without a matching B- starts a span."""
    out: list[tuple[str, int, int]] = []
    prev = "O"
    for (_, s, e), lab in zip(split(text), labels):
        if lab != "O":
            tag, slot = lab.split("-", 1)
            if tag == "I" and prev != "O" and prev.split("-", 1)[1] == slot:
                out[-1] = (slot, out[-1][1], e)
            else:
                out.append((slot, s, e))
        prev = lab
    return [(slot, text[s:e]) for slot, s, e in out]
