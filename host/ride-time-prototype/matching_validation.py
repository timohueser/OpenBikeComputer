"""Compare geometric review labels with predictions; keep unresolved cases visible."""

from collections import Counter
import json
import math

from enrichment import digest
from matching_review import OUTPUT


def wilson(errors,count):
    if count==0:
        return None
    z=1.959963984540054
    p=errors/count
    centre=(p+z*z/(2*count))/(1+z*z/count)
    radius=z*math.sqrt(p*(1-p)/count+z*z/(4*count*count))/(1+z*z/count)
    return [max(0,centre-radius),min(1,centre+radius)]


def score(phase):
    labels=json.loads((OUTPUT/'review-labels.json').read_text())
    predictions={r['case']:r for r in json.loads((OUTPUT/'review-predictions.json').read_text())}
    catalogue={r['case']:r for r in json.loads((OUTPUT/'review-catalogue.json').read_text())}
    results={}
    rows=[]
    for label in labels:
        if label['phase']!=phase:
            continue
        pred=predictions[label['case']]
        valid=label['judgeable']
        correct=bool(pred['core_way_ids']) and set(pred['core_way_ids'])<=set(label['allowed_way_ids']) if valid else None
        surface={catalogue[label['case']]['labels'][letter]['tags'].get('surface','unknown') for letter in label['allowed_labels']}
        expected=next(iter(surface)) if valid and len(surface)==1 and 'unknown' not in surface else None
        agreement=pred['tags'].get('surface',{})
        asserted=next(iter(agreement)) if len(agreement)==1 and sum(agreement.values())>=1-1e-6 else None
        rows.append(dict(case=label['case'],judgeable=valid,selected_correct=correct,
                         quality_pass=pred['status']=='accepted',core=pred['core'],
                         surface_assertion=asserted,surface_checkable=expected is not None,
                         surface_correct=asserted==expected if expected is not None and asserted is not None else None))
    for name in ['plausible']+[f'core_{n}' for n in (0,5,10,15,20)]+['surface']:
        count=Counter(total=len(rows))
        for r in rows:
            accepted=r['quality_pass'] if name=='plausible' else bool(r['surface_assertion']) if name=='surface' else r['core'].get(name.split('_')[1],False)
            count['accepted']+=accepted
            if not accepted:
                continue
            truth=r['surface_correct'] if name=='surface' else r['selected_correct']
            count['unresolved']+=truth is None
            count['correct']+=truth is True
            count['incorrect']+=truth is False
        assessed=count['correct']+count['incorrect']
        results[name]=dict(count,error_rate=count['incorrect']/assessed if assessed else None,
                          wilson95=wilson(count['incorrect'],assessed))
    return dict(phase=phase,metrics=results,cases=rows,labels_sha256=digest(OUTPUT/'review-labels.json'))


if __name__=='__main__':
    import argparse
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase',choices=('development','validation'))
    args=parser.parse_args()
    result=score(args.phase)
    (OUTPUT/f'{args.phase}-validation.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k!='cases'},indent=2))
