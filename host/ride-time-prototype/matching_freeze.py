"""Freeze a matching configuration before the held-aside visual check."""

from datetime import datetime,timezone
import json
from pathlib import Path

from enrichment import digest
from matching_review import OUTPUT
from matching_v2 import CONFIG


if __name__=='__main__':
    labels=json.loads((OUTPUT/'review-labels.json').read_text())
    if len(labels)!=20 or any(r['phase']!='development' for r in labels):
        raise SystemExit('Freeze requires only the 20 development labels; validate on fresh held-aside sections.')
    files=[Path(__file__).with_name(n) for n in ('matching_v2.py','matching_audit.py','enrichment_match.py',
                                                'matching_validation.py','matching_review.py')]
    files.append(OUTPUT/'review-selection.json')
    record=dict(frozen_utc=datetime.now(timezone.utc).isoformat(),
                primary_endpoint_tolerance_m=CONFIG['endpoint_tolerance_m'],
                source_hashes={str(p):digest(p) for p in files},
                development_labels_sha256=digest(OUTPUT/'review-labels.json'),
                note='Frozen after development review and before labeling or inspecting predictions for the validation sections. '
                     'Full-pilot unsupervised coverage was examined during development.')
    with (OUTPUT/'frozen.json').open('x') as f:
        json.dump(record,f,indent=2)
    print('Matcher configuration frozen')
