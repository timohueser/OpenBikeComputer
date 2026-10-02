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
from prediction import request_from_prediction, validate_text


class Parser:
    def __init__(self, directory: Path, *, threads: int = 1):
        check_labels(directory)
        self.tokenizer = Tokenizer.from_file(str(directory / 'tokenizer.json'))
        self.tokenizer.enable_truncation(max_length=64)
        options = ort.SessionOptions()
        options.intra_op_num_threads = threads
        self.session = ort.InferenceSession(str(directory / 'model.int8.onnx'), options,
                                           providers=['CPUExecutionProvider'])

    def parse(self, text: str) -> dict:
        validate_text(text)
        started = time.perf_counter()
        encoding = self.tokenizer.encode(text)
        ids = np.array([encoding.ids], dtype=np.int64)
        intent_logits, tag_logits = self.session.run(['intent_logits', 'tag_logits'],
            {'input_ids': ids, 'attention_mask': np.ones_like(ids)})
        request = request_from_prediction(text, encoding.offsets, intent_logits[0].argmax(),
                                          tag_logits[0].argmax(axis=1))
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
