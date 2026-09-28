"""Local mmBERT inference. JSON lines keep the model outside the search process."""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

from artifacts import check_labels
from decode import decode
from schema import INTENTS, LABELS, validate
from words import split


class Parser:
    def __init__(self, directory: Path):
        check_labels(directory)
        self.tokenizer = Tokenizer.from_file(str(directory / 'tokenizer.json'))
        self.tokenizer.enable_truncation(max_length=64)
        options = ort.SessionOptions()
        options.intra_op_num_threads = 1
        self.session = ort.InferenceSession(str(directory / 'model.int8.onnx'), options,
                                           providers=['CPUExecutionProvider'])

    def parse(self, text: str) -> dict:
        if not isinstance(text, str) or not text.strip() or len(text) > 80:
            raise ValueError('Use one sentence of 1 to 80 characters.')
        started = time.perf_counter()
        encoding = self.tokenizer.encode(text)
        ids = np.array([encoding.ids], dtype=np.int64)
        intent_logits, tag_logits = self.session.run(['intent_logits', 'tag_logits'],
            {'input_ids': ids, 'attention_mask': np.ones_like(ids)})
        labels = []
        for _, start, end in split(text):
            token = next((i for i, (a, b) in enumerate(encoding.offsets)
                          if a < b and max(start, a) < min(end, b)), None)
            labels.append(LABELS[int(tag_logits[0, token].argmax())] if token is not None else 'O')
        intent = INTENTS[int(intent_logits[0].argmax())]
        request = decode(text, intent, labels)
        validate(request)
        return {'request': request, 'elapsed': (time.perf_counter() - started) * 1000}


def main():
    args = argparse.ArgumentParser()
    args.add_argument('--model', type=Path, required=True)
    parser = Parser(args.parse_args().model)
    print(json.dumps({'ready': True}), flush=True)
    for line in sys.stdin:
        value = {}
        try:
            value = json.loads(line)
            result = parser.parse(value['text'])
            print(json.dumps({'id': value['id'], 'result': result}), flush=True)
        except Exception as error:
            print(json.dumps({'id': value.get('id'), 'error': str(error)}), flush=True)


if __name__ == '__main__':
    main()
