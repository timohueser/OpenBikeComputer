"""Compare CPU thread settings on every held-out request in separate processes."""
import argparse
import hashlib
import json
import resource
import subprocess
import sys
import time
from pathlib import Path


def measure(model, threads):
    import onnxruntime
    from evaluate import load_testset, score
    from runtime import Parser
    rows = load_testset(None)
    started = time.perf_counter()
    parser = Parser(model, threads=threads)
    initialization = (time.perf_counter() - started) * 1000
    predictions, times = [], []
    for row in rows:
        result = parser.parse(row['text'])
        predictions.append(result['request'])
        times.append(result['elapsed'])
    accuracy = score(rows, predictions)
    times.sort()
    return dict(threads=threads, runtime=onnxruntime.__version__, initialization_ms=initialization,
                requests=len(rows), p50_ms=times[len(times)//2], p95_ms=times[int(len(times)*.95)],
                peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss * (1 if sys.platform == 'darwin' else 1024),
                outputs_sha256=hashlib.sha256(json.dumps(predictions, sort_keys=True, ensure_ascii=False).encode()).hexdigest(),
                accuracy=accuracy['acc'], silent_errors=accuracy['silent'], out_of_scope=accuracy['oos_handled'], crashes=accuracy['crashes'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('model', type=Path)
    parser.add_argument('--threads', type=int, nargs='+', default=[1, 2, 4])
    parser.add_argument('--child', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if any(n < 1 for n in args.threads):
        parser.error('Threads must be positive')
    if args.child:
        print(json.dumps(measure(args.model, args.threads[0])))
    else:
        samples = []
        for threads in args.threads:
            result = subprocess.run([sys.executable, __file__, str(args.model), '--threads', str(threads), '--child'],
                                    check=True, text=True, capture_output=True)
            samples.append(json.loads(result.stdout))
        for filename in ['model.int8.onnx', 'tokenizer.json', 'labels.json', 'tokenizer_config.json']:
            with (args.model / filename).open('rb') as stream:
                print(json.dumps({'file': filename, 'sha256': hashlib.file_digest(stream, 'sha256').hexdigest()}))
        print(json.dumps({'decoded_outputs_equal': len({s['outputs_sha256'] for s in samples}) == 1, 'samples': samples}, indent=2))
