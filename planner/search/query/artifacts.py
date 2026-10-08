"""Label contract and input fingerprints for trained model directories."""
import hashlib
import json
from pathlib import Path

from schema import INTENTS, LABELS


def label_contract():
    return {"intents": INTENTS, "labels": LABELS}


def check_labels(directory: Path):
    if json.loads((directory / 'labels.json').read_text()) != label_contract():
        raise ValueError('Model labels do not match the query schema. Retrain and export the model.')


def fingerprints(paths):
    result = {}
    for path in paths:
        path = Path(path)
        with path.open('rb') as stream:
            result[str(path)] = hashlib.file_digest(stream, 'sha256').hexdigest()
    return result


def model_identity(directory: Path):
    paths = [directory / name for name in ('labels.json', 'tokenizer.json', 'model.int8.onnx')]
    return {Path(path).name: digest for path, digest in fingerprints(paths).items()}
