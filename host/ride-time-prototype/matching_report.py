"""Report matching coverage separately from reviewed correctness."""

import argparse
import base64
import json
from pathlib import Path
import shutil
import sys

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

from enrichment import digest
from enrichment_report import table,figure
from matching_review import OUTPUT,PILOT

ROOT=Path(__file__).resolve().parents[2]


def report(publish):
    summary=json.loads((OUTPUT/'summary.json').read_text())
    old=json.loads((PILOT/'summary.json').read_text())
    development=json.loads((OUTPUT/'development-validation.json').read_text())
    validation=json.loads((OUTPUT/'validation-validation.json').read_text())
    freeze=json.loads((OUTPUT/'frozen.json').read_text())
    for filename,expected in freeze['source_hashes'].items():
        if digest(Path(filename))!=expected:
            raise SystemExit(f'Frozen matcher input changed: {filename}')
    if validation['labels_sha256']!=digest(OUTPUT/'review-labels.json'):
        raise SystemExit('Review labels changed after validation scoring')
    names=['MTB','other cycling']
    chart_labels=['Original acceptance','Plausible connected path','Path agreement','Known surface agreement']
    chart=[]
    rows=[]
    for name in names:
        group=summary['groups'][name]
        totals=group['totals']; raw=totals['raw_km']
        surface=sum(group['tag_km']['surface'].values())
        values=[100*old['groups'][name]['status_km']['accepted']/raw,
                100*totals['plausible_path_km']/raw,100*totals['core_20_km']/raw,100*surface/raw]
        chart.append(values)
        rows.append([name,*[f'{v:.1f}%' for v in values]])
    fig,ax=plt.subplots(figsize=(10,5))
    for j,name in enumerate(names):
        ax.barh(np.arange(4)+(j-.5)*.34,chart[j],height=.34,label=name,color=('#305e4c','#b8583f')[j])
    ax.set(yticks=np.arange(4),yticklabels=chart_labels,xlim=(0,100),xlabel='Share of original recorded distance (%)',
           title='Finding a path and trusting its attributes are different results')
    ax.legend(loc='lower right')
    figure(fig,OUTPUT,'matching-v2-coverage')
    metrics=validation['metrics']
    labels={'plausible':'Plausible path','core_20':'Path agreement, 20 m','surface':'Full surface agreement'}
    review_rows=[]
    for key,label in labels.items():
        m=metrics[key]
        ci=m['wilson95']
        review_rows.append([label,m['accepted'],m['correct'],m['incorrect'],m['unresolved'],
                           f'{100*ci[0]:.1f}–{100*ci[1]:.1f}%' if ci else 'Not measurable'])
    text=['# Map matching: revised acceptance and visual checks\n',
        '**Status: experimental host prototype.** This is a development study of matching rules. '
        'It does not establish true surface conditions, production matching accuracy or an improvement in ETA accuracy.\n',
        '## Main results\n',
        table(['Activity',*chart_labels],rows),
        '![Coverage with distinct matching and attribute checks](matching-v2-coverage.svg)\n',
        '**The original figure mainly measured rejection by a way-identity heuristic.** A plausible path means the best connected '
        'sequence passes offset, distance and recording-quality checks. It does not mean competing paths have been ruled out. '
        'Path agreement adds the alternative-path check described below. Surface agreement is independent: competing paths can '
        'share asphalt even when their geometry is unresolved. These measures are not interchangeable.\n',
        f"On the 20 held-aside sections, the selected path-agreement rule accepts {metrics['core_20']['accepted']}: "
        f"{metrics['core_20']['correct']} agree with identifiable reviewed paths, {metrics['core_20']['incorrect']} disagree, "
        f"and {metrics['core_20']['unresolved']} cannot be judged from the overlay. This is encouraging, but the sample is "
        "too small and the labels too limited to certify production accuracy.\n",
        '## What changed\n',
        '1. Candidate projections at the same graph node are one state, even when several OSM ways meet there. '
        'Unconnected crossings stay separate.\n'
        '2. The confidence check considers complete transitions between samples, with the cost of the surrounding sequence. '
        'It no longer tests the two endpoint way identifiers in isolation.\n'
        '3. Alternative positions on one unbranched chain represent the same path. Chains stop at branches; closed rings '
        'are not collapsed. This separates along-road position uncertainty from road identity.\n'
        '4. For other routes, up to 20 m at each endpoint may differ, but the central edges must agree and at least half '
        'of the longer path must overlap. Parallel roads and internal diversions do not become equivalent merely because they are close.\n'
        '5. Every considered transition contributes its complete attribute composition. Missing tags stay unknown. '
        'Attributes are never inferred from the first and last samples alone.\n',
        '### Attribute agreement\n',
        'For each value, retain the smallest distance fraction across the considered paths. If the alternatives contain '
        '70% asphalt / 30% gravel and 50% asphalt / 50% gravel, the retained composition is 50% asphalt / 30% gravel; '
        '20% remains unresolved. If all alternatives are asphalt, the surface can be retained even without exact path agreement. '
        'A zero-length alternative supplies unknown composition.\n\n'
        'The coverage table allocates these common fractions to original GPS chord distance, as in the first pilot. '
        'This is a lower bound across the considered compositions, not an identification of which exact metres carry each value. '
        'The visual surface check below assesses only a single known value supported at 100%, not partially agreed mixtures.\n',
        '## Fixed visual review\n',
        'Forty recording-screen-passing intervals were selected by stable hash before examining revised predictions. '
        'Twenty development sections came from previously inspected riders. Twenty validation sections each came from a '
        'different rider whose tracks had not been visually inspected in the earlier pilot or ambiguity diagnosis. '
        'The cohorts contain disjoint riders. No interval was selected for match success or an attractive error result.\n\n'
        'The assistant labeled GPS/OSM overlays that did not show matcher selections or scores. Clearly identifiable paths '
        'received an allowed set of OSM ways. Unclear sections remained unjudgeable. These are visual judgments from the same '
        'recording and map, not independent field observations or a second human reviewer. Historical map errors remain possible.\n\n'
        'Development checks compared endpoint tolerances of 0, 5, 10, 15 and 20 m. The recorded configuration selects 20 m: '
        'it retained the most reviewed paths without a clear error in that small development set. '
        'Matcher source and settings were frozen before validation labels were recorded or validation predictions inspected. '
        'Unsupervised coverage across the full 80-ride pilot was available during development.\n',
        '### Development sections\n',
        table(['Endpoint tolerance','Accepted','Correct among judgeable','Incorrect','Unjudgeable'],[
            [f'{n} m',*[development['metrics'][f'core_{n}'][key] for key in ('accepted','correct','incorrect','unresolved')]]
            for n in (0,5,10,15,20)]),
        '### Validation sections: 20 different riders\n',
        table(['Rule','Accepted / 20','Correct','Incorrect','Unjudgeable','Error interval, 95% Wilson'],review_rows),
        'Correctness of a path means its selected central way IDs fall within the reviewed allowed set. '
        'Surface correctness means agreement with the OSM surface on the visually identified path; it does not verify '
        'the physical material. Unjudgeable accepted sections are shown separately and excluded from the error-rate denominator. '
        'The Wilson intervals describe sampling uncertainty conditional on these imperfect labels. The small review cannot '
        'certify a low production error rate.\n',
        '## Remaining limitations\n',
        '- The search retains at most eight candidate states per sample and one shortest path for each endpoint pair. '
        'It does not enumerate all internal detours between identical endpoints. Agreement is conditional on that search space.\n'
        '- The cost margin of three is still an uncalibrated heuristic. Small visual checks do not turn it into a probability.\n'
        '- Sparse samples, missing paths and changed geometry can make an incorrect path appear to be the best available one.\n'
        '- These are 2012–2015 rides matched to a 2026 OSM snapshot in central Jutland. The pilot does not represent worldwide '
        'coverage or mountainous trail conditions.\n'
        '- Matching uses future coordinates within continuous recording blocks. Device learning and ETA evaluation need '
        'causal observation matching and a separately known planned route.\n',
        '## Recommendation\n',
        'Use the revised report to separate road-finding failures from uncertainty in accepted attributes. '
        'Keep ambiguous and missing attributes explicit. Before device integration, obtain recent dense rides with known '
        'paths and increase the blinded review sample. Keep the ETA model unchanged until enriched observations have a '
        'credible validation protocol.\n',
        '## Artifacts and reproduction\n',
        'The [prototype README](src:host/ride-time-prototype/README.md) gives the commands. '
        'The portable local report includes the ten private review sheets. Public documentation contains only aggregate '
        'results. [Original enrichment audit](/docs/software/ride-time-enrichment/).\n\n'
        'Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), ODbL 1.0; '
        'extract by Geofabrik. Original ride data: [FitRec](https://cseweb.ucsd.edu/~jmcauley/datasets/fitrec.html). '
        'No ride coordinates were sent to a matching service.\n']
    markdown='\n'.join(text)
    (OUTPUT/'report.md').write_text(markdown)
    sys.path.insert(0,str(ROOT/'docs'))
    from build_docs import render_blocks
    rendered,_=render_blocks(markdown)
    rendered=rendered.replace('href="/docs/software/ride-time-enrichment/"','href="../ride-time-enrichment/report.html"')
    encoded=base64.b64encode((OUTPUT/'matching-v2-coverage.svg').read_bytes()).decode()
    rendered=rendered.replace('src="matching-v2-coverage.svg"',f'src="data:image/svg+xml;base64,{encoded}"')
    rendered+='<h2>Private review sheets</h2><p>These overlays contain source ride geometry. Do not redistribute them.</p>'
    for sheet in sorted(OUTPUT.glob('review-sheet-*.png')):
        encoded=base64.b64encode(sheet.read_bytes()).decode()
        rendered+=f'<h3>{sheet.stem}</h3><img alt="Blind geometry review sheet" src="data:image/png;base64,{encoded}">'
    (OUTPUT/'report.html').write_text('<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
        '<title>Revised map matching</title><style>body{max-width:1100px;margin:40px auto;padding:0 20px;font:17px/1.6 system-ui;color:#243d32;background:#faf8f2}'
        'h2{margin-top:2em}img{max-width:100%;height:auto}table{display:block;overflow:auto;border-collapse:collapse;font-size:14px}'
        'th,td{padding:7px 10px;border-bottom:1px solid #d6c8b5;text-align:left}a{color:#236e58}</style><body>'+rendered+'</body></html>')
    if publish:
        assets=ROOT/'docs/assets/research/ride-time'
        shutil.copyfile(OUTPUT/'matching-v2-coverage.svg',assets/'matching-v2-coverage.svg')
        public=markdown.replace('](matching-v2-coverage.svg)','](/assets/research/ride-time/matching-v2-coverage.svg)')
        (ROOT/'docs/content/software/ride-time-matching.md').write_text('---\ncopy: ai\n---\n\n'+public)
        aggregate=dict(summary,old_groups=old['groups'],review={
            'development':development['metrics'],'validation':validation['metrics']},freeze=freeze,osm=old['osm'])
        (ROOT/'host/ride-time-prototype/results/matching-v2.json').write_text(json.dumps(aggregate,indent=2)+'\n')
        private_labels=json.loads((OUTPUT/'review-labels.json').read_text())
        public_labels=[{k:v for k,v in r.items() if k!='allowed_way_ids'} for r in private_labels]
        (ROOT/'host/ride-time-prototype/results/matching-review-labels.json').write_text(json.dumps(public_labels,indent=2)+'\n')
    print(f'Report: {OUTPUT/"report.html"}')


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--publish-docs',action='store_true')
    report(parser.parse_args().publish_docs)
