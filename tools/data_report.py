"""Keep one GitHub issue for read-only Live data and VPS attention."""

import argparse
import json
import subprocess
from pathlib import Path

MARKER = '<!-- obc-data-readonly-report -->'
TITLE = 'Live data needs attention'
ROOT = Path(__file__).resolve().parents[1]


def output(argv):
    return subprocess.run(argv, cwd=ROOT, capture_output=True, text=True, check=True).stdout


def summarize(status):
    """A clear report requires complete structured evidence, not a successful exit alone."""
    rows, clear = [], True
    products = status.get('products', [])
    if not products:
        clear = False
        rows.append('- Products: unavailable')
    for product in products:
        rows.append(f"- {product['product']}: {product.get('release') or 'not applied'}")
        layers = product.get('layers')
        if not product.get('release') or not layers:
            clear = False
            rows.append('  - Layer comparison: unavailable')
        for layer in layers or []:
            if layer['state'] != 'ok':
                clear = False
                reason = layer.get('reason') or layer['state']
                rows.append(f"  - {layer['layer']}: {layer['state']} — {reason}")
    for attention in status.get('attention', []):
        clear = False
        rows.append(f"- {attention['kind']} · {attention['about']}: {attention['reason']}")
    check = status.get('check')
    if check is None:
        clear = False
        rows.append('- R2: comparison unavailable')
    elif check.get('drift') or check.get('leftovers'):
        clear = False
        rows.append(f"- R2: {len(check.get('drift', []))} missing/changed, {len(check.get('leftovers', []))} leftovers")
    vps = status.get('vps')
    if not vps or vps.get('unavailable') or not vps.get('host') or len(vps.get('services', [])) != 3:
        clear = False
        rows.append('- VPS: ' + ((vps or {}).get('unavailable') or 'observation unavailable'))
    else:
        for service in vps['services']:
            if not service['ready']:
                clear = False
                rows.append(f"- VPS/{service['service']}: {service.get('reason') or 'readiness unavailable'}")
    body = MARKER + '\n\nRead-only Live check. No bake, publication or cleanup runs here.\n\n'
    body += '\n'.join(rows) + '\n'
    return clear, body


def issue_changes(issues, clear, body):
    matching = [issue for issue in issues if MARKER in (issue.get('body') or '')]
    if len(matching) > 1:
        raise ValueError('More than one managed report issue; resolve the duplicate before updating')
    if not matching:
        return [] if clear else [('create', body)]
    issue = matching[0]
    changes = []
    if issue['body'] != body:
        changes.append(('edit', body))
    opened = issue['state'].upper() == 'OPEN'
    if opened and clear:
        changes.append(('close', None))
    elif not opened and not clear:
        changes.append(('reopen', None))
    return changes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--status', type=Path, help='Read an existing status JSON document')
    parser.add_argument('--update', action='store_true', help='Update the single GitHub report issue')
    args = parser.parse_args()
    try:
        raw = args.status.read_text() if args.status else output(['./tools/obc', 'data', 'status', '--check', '--json'])
        clear, body = summarize(json.loads(raw))
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        # Do not copy stderr from a credential-bearing transport into a public issue.
        clear, body = False, MARKER + '\n\nRead-only check unavailable. Repair the status command or observer setup; no clear result is recorded.\n'
        print(f'Read-only status unavailable: {type(error).__name__}')
    if not args.update:
        print(body)
        return
    issues = json.loads(output(['gh', 'issue', 'list', '--state', 'all', '--search', TITLE, '--limit', '100', '--json', 'number,body,state']))
    matching = [issue for issue in issues if MARKER in (issue.get('body') or '')]
    for action, value in issue_changes(issues, clear, body):
        argv = ['gh', 'issue', action]
        if action == 'create': argv += ['--title', TITLE]
        else: argv += [str(matching[0]['number'])]
        if value is not None:
            subprocess.run(argv + ['--body-file', '-'], cwd=ROOT, input=value, text=True, check=True)
        else:
            subprocess.run(argv, cwd=ROOT, check=True)


if __name__ == '__main__':
    main()
