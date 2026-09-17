#!/usr/bin/env python3
"""Explicit live routing probes; only Pablo privately loads the gateway credential."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
# Expectations are fixed before running either prompt, without benchmark keys.
CASES = [
    ('literal', 'small', 'Reply with exactly OK.'),
    ('legal-extraction', 'small', 'Extract only the effective date from this synthetic contract sentence: "This agreement takes effect on 2026-05-12." Return the date as YYYY-MM-DD.'),
    ('arithmetic', 'small', 'Three invoices total 20, 30 and 40 dollars. Return their sum as a number.'),
    ('invoice-rules', 'medium', 'Reconcile 20 invoices against purchase orders and payment records in three CSV files. Match invoice IDs, apply the explicit 2 percent discount only to payments within 10 days, flag duplicates, and report outstanding balances with a short explanation. All rules are supplied and there are no conflicting records.'),
    ('notice-deadline', 'medium', 'Read a synthetic contract, notice log and payment ledger. Apply explicit service rules to find the effective notice date, count a 10-business-day cure period excluding supplied holidays, subtract credited payments, and decide whether the remaining balance permits termination. Return six findings with source citations.'),
    ('schedule', 'medium', 'Create a one-day agenda for eight meetings using the supplied durations, two rooms and fixed attendee availability. Avoid overlaps, preserve the stated lunch break and list any impossible meetings. The constraints are complete and consistent; use a straightforward scheduling method.'),
    ('conflicting-authorities', 'large', 'Analyze a synthetic 40-document dispute with conflicting witness accounts, three amendments with disputed precedence, and overlapping notice, tolling and waiver exceptions. Construct alternative factual timelines, propagate each interpretation through liability and damages calculations, identify which conclusions change under each assumption, and recommend a defensible strategy with counterarguments and citations.'),
    ('distributed-design', 'large', 'Design a multi-region payment ledger migration with zero downtime under network partitions. Prove the consistency invariants, reconcile exactly-once business effects with at-least-once delivery, evaluate competing rollout strategies, and derive recovery procedures for failures at every migration boundary. Existing data contains conflicting transaction histories and requirements conflict on availability versus consistency.'),
    ('dependent-planning', 'large', 'Develop and justify a five-year hospital capacity plan across twelve sites under uncertain demand and funding. Staffing, training lead times, equipment procurement and transfer capacity impose coupled constraints. Compare alternative scenarios, identify infeasible combinations, quantify sensitivity, and recommend a robust staged plan that addresses contradictory stakeholder goals.'),
]
MODELS = {'small': 'zai/glm-5.3-flash', 'medium': 'spacexai/grok-4.6', 'large': 'openai/gpt-6-astra'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, default=Path(__file__).with_name('jev-router.toml'))
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/pablo')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeats', type=int, default=1)
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error('--repeats must be positive')
    args.output.mkdir(parents=True, exist_ok=False)
    frozen = args.output.resolve() / 'router-config.toml'
    frozen.write_bytes(args.config.read_bytes())
    env = {k: v for k, v in os.environ.items() if not k.startswith(('PABLO_', 'OTEL_'))
           and k not in {'AI_GATEWAY_API_KEY', 'VERCEL_AI_GATEWAY', 'OPENROUTER_API_KEY', 'ANTHROPIC_API_KEY'}}
    results = []
    for repeat in range(args.repeats):
        for case_id, expected, prompt in CASES:
            with tempfile.TemporaryDirectory(prefix='pablo-jev-probe-') as temp:
                workspace = Path(temp).resolve()
                trace = workspace / 'trace.jsonl'
                start = time.monotonic()
                run = subprocess.run([str(args.binary.resolve()), 'run', prompt, '--config', str(frozen),
                                      '--bind', f'workspace={workspace}', '--bind', f'secrets={ROOT}',
                                      '--trace', str(trace), '--json', '--max-model-calls', '1', '--timeout', '45'],
                                     env=env, capture_output=True, text=True, timeout=55)
                task = json.loads(run.stdout) if run.stdout.strip() else {}
                events = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
                calls = [e for e in events if e['type'] == 'model.started']
                terminal = next((e for e in reversed(events) if e['type'] == 'run.finished'), {})
                route = terminal.get('model_route') or {}
                outcome = task.get('outcome', {})
                row = {'case': case_id, 'repeat': repeat + 1, 'expected': expected,
                       'selected_tier': route.get('entry'), 'selected_model': route.get('model'),
                       'correct': route.get('entry') == expected and route.get('model') == MODELS[expected]
                                  and outcome == {'status': 'limit_exceeded', 'limit': 'model_calls'},
                       'classifier_only': len(calls) == 1 and calls[0]['model'] == 'typesafe-ai/jev',
                       'outcome': outcome, 'elapsed_ms': round((time.monotonic() - start) * 1000)}
                results.append(row)
                (args.output / f'{case_id}-{repeat + 1}.trace.jsonl').write_text('\n'.join(json.dumps(e) for e in events) + '\n')
                with (args.output / 'routing.jsonl').open('a') as out:
                    out.write(json.dumps(row) + '\n')
                print(json.dumps(row), flush=True)
    summary = {'cases': len(results), 'correct': sum(r['correct'] for r in results),
               'provider_failures': sum(r['outcome'].get('status') == 'failed' for r in results),
               'wrong_tier': sum(r['selected_tier'] is not None and r['selected_tier'] != r['expected'] for r in results),
               'classifier_only': all(r['classifier_only'] for r in results),
               'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
               'config_sha256': hashlib.sha256(frozen.read_bytes()).hexdigest(), 'results': results}
    (args.output / 'report.json').write_text(json.dumps(summary, indent=2) + '\n')
    raise SystemExit(0 if summary['correct'] == summary['cases'] and summary['classifier_only'] else 1)


if __name__ == '__main__':
    main()
