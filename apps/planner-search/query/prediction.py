"""Map token predictions to the shared word decoder, independent of tensor runtime."""
from decode import decode
from schema import INTENTS, LABELS, validate
from words import split


def validate_text(text):
    if not isinstance(text, str) or not text.strip() or len(text) > 80:
        raise ValueError('Use one sentence of 1 to 80 characters.')


def request_from_prediction(text, offsets, intent, tags):
    labels = []
    for _, start, end in split(text):
        token = next((i for i, (a, b) in enumerate(offsets)
                      if a < b and max(start, a) < min(end, b)), None)
        labels.append(LABELS[int(tags[token])] if token is not None else 'O')
    request = decode(text, INTENTS[int(intent)], labels)
    validate(request)
    return request
