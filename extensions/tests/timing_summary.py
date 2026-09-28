#!/usr/bin/env python3
"""Summarize smoke.py's exported timings without retaining prompts or command output."""
import argparse
from collections import defaultdict
from datetime import datetime
import hashlib
import json
from pathlib import Path
from statistics import median

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('fixtures', nargs='+', type=Path)
args = parser.parse_args()
results = []
for root in args.fixtures:
    measured = json.loads((root / 'transitions.json').read_text())
    phases = defaultdict(list)
    method_work = defaultdict(list)
    report_completions = {}
    seen = set()
    lines = []
    for path in root.glob('*.logs.json'):
        lines.extend(json.loads(path.read_text())['lines'])
    for path in root.glob('yinz.log*'):
        lines.extend(path.read_text().splitlines())
    for line in lines:
        if line in seen or 'swamp_timing ' not in line:
            continue
        seen.add(line)
        try:
            record = json.loads(line.split('swamp_timing ', 1)[1])
        except ValueError:
            continue
        elapsed = record.get('elapsedMs')
        if not isinstance(elapsed, (int, float)):
            continue
        key = (record.get('layer'), record.get('phase'), record.get('method', 'workflow'))
        phases[key].append(elapsed)
        if record.get('layer') == 'swamp' and record.get('phase') == 'method':
            method_work[record['step']].append(elapsed)
        if record.get('layer') == 'swamp' and record.get('phase') == 'workflow_reports':
            report_completions[record['run']] = datetime.fromisoformat(
                line.split()[0].replace('Z', '+00:00')).timestamp()
    measured['fixture'] = str(root)
    measured['binaries'] = {
        name: hashlib.sha256((root / name).read_bytes()).hexdigest()
        for name in ('yinz', 'swamp')
    }
    measured['reportSourceSha256'] = hashlib.sha256((root / 'main/extensions/reports/workflow.ts').read_bytes()).hexdigest()
    measured['phases'] = [dict(layer=k[0], phase=k[1], method=k[2], count=len(v),
        minMs=round(min(v), 3), medianMs=round(median(v), 3), maxMs=round(max(v), 3))
        for k, v in sorted(phases.items())]
    measured['methodWork'] = [dict(step=k, count=len(v),
        minMs=round(min(v), 3), medianMs=round(median(v), 3), maxMs=round(max(v), 3))
        for k, v in sorted(method_work.items())]
    for sample in measured['samples']:
        completed = report_completions.get(sample['previousRun'])
        if completed is not None:
            sample['priorRunReportCompletedAfterQuestionMs'] = round(
                (completed - measured['wallClockOffset'] - sample['clocks']['nextQuestion']) * 1000, 3)
    results.append(measured)
print(json.dumps(results, indent=2))
