#!/usr/bin/env python3
"""Use the existing legal benchmark scorer/monitor with Pablo's task-pinned router.

Only the Pablo executable reads credentials. Answer keys never enter the prompt.
Task briefs and source documents inform classification and generation, but their
contents never enter this integration's routing log.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from task_context import MAX_CONTEXT_BYTES, task_context

PABLO_ROOT = Path(__file__).resolve().parents[2]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument('--benchmark', type=Path, default=Path.home() / 'Projects/legal-benchmark')
    parser.add_argument('--binary', type=Path, default=PABLO_ROOT / 'target/release/pablo')
    parser.add_argument('--config', type=Path, default=Path(__file__).with_name('jev-router.toml'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--tasks', default='all')
    parser.add_argument('--timeout', type=int, default=300)
    parser.add_argument('--source-context', choices=['full', 'brief'], default='full')
    parser.add_argument('--context-max-bytes', type=int, default=MAX_CONTEXT_BYTES)
    args = parser.parse_args()
    args.benchmark = args.benchmark.resolve()
    args.binary = args.binary.resolve()
    args.config = args.config.resolve()
    args.output = args.output.resolve()
    sys.path.insert(0, str(args.benchmark))
    from legalbench.adapters import Adapter
    from legalbench.runner import run_suite
    from legalbench.report import build_report

    manifest = json.loads((args.benchmark / 'tasks/manifest.json').read_text())
    task_ids = [task['id'] for task in manifest['tasks']] if args.tasks == 'all' else args.tasks.split(',')
    task_lookup = {
        hashlib.sha256((args.benchmark / 'tasks' / task['id'] / 'task.md').read_bytes()).hexdigest(): task['id']
        for task in manifest['tasks']
    }
    # Validate all inputs before spending any provider calls; no silent truncation.
    for task_id in task_ids:
        task_context(args.benchmark / 'tasks' / task_id, include_sources=args.source_context == 'full',
                     max_bytes=args.context_max_bytes)
    source_commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=PABLO_ROOT, text=True).strip()
    binary_sha256 = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    runner_sha256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    context_builder_sha256 = hashlib.sha256(Path(__file__).with_name('task_context.py').read_bytes()).hexdigest()

    @dataclass(frozen=True, kw_only=True)
    class RouterAdapter(Adapter):
        active: dict = field(default_factory=dict, compare=False)

        def child_env(self) -> dict[str, str]:
            return {key: value for key, value in super().child_env().items()
                    if not key.startswith(('PABLO_', 'OTEL_'))
                    and key not in {'AI_GATEWAY_API_KEY', 'VERCEL_AI_GATEWAY', 'OPENROUTER_API_KEY', 'ANTHROPIC_API_KEY'}}

        def command(self, *, workspace, prompt, timeout_s, env_file, binary=None):
            brief = (workspace / 'task.md').read_text()
            context, context_metadata = task_context(workspace, include_sources=args.source_context == 'full',
                                                      max_bytes=args.context_max_bytes)
            self.active.update(task_id=task_lookup[hashlib.sha256(brief.encode()).hexdigest()],
                               trace=workspace.parent.resolve() / 'pablo-trace.jsonl', context=context_metadata)
            return [str(binary or args.binary), 'run', prompt + context,
                    '--config', str(args.config), '--bind', f'workspace={workspace.resolve()}',
                    '--bind', f'secrets={PABLO_ROOT}', '--bind', f'traces={workspace.parent.resolve()}', '--trace', str(self.active['trace']),
                    '--json', '--timeout', str(timeout_s)]

        def normalize(self, raw, receipts):
            result = super().normalize(raw, receipts)
            path = self.active['trace']
            events = [json.loads(line) for line in path.read_text().splitlines()] if path.is_file() else []
            calls = []
            for event in events:
                if event['type'] != 'model.started':
                    continue
                finish = next((e for e in events if e['type'] == 'model.finished' and e['span_id'] == event['span_id']), None)
                route = event.get('model_route') or {}
                calls.append({'model': event['model'], 'provider': event['provider'],
                              'tier': route.get('entry'), 'selection_reason': route.get('selection_reason'),
                              'span_id': event['span_id'], 'started_unix_micros': event['timestamp_unix_micros'],
                              'duration_ms': (finish['timestamp_unix_micros'] - event['timestamp_unix_micros']) / 1000 if finish else None,
                              'status': finish.get('status') if finish else 'unfinished',
                              'usage': finish.get('usage') if finish else None})
            classifier = [call for call in calls if call['model'] == 'typesafe-ai/jev']
            generation = [call for call in calls if call['model'] != 'typesafe-ai/jev']
            identities = {(call['provider'], call['model'], call['tier']) for call in generation}
            terminal = next((e for e in reversed(events) if e['type'] == 'run.finished'), {})
            selected = generation[0] if generation else {
                'tier': (terminal.get('model_route') or {}).get('entry'),
                'model': (terminal.get('model_route') or {}).get('model'),
            }
            routing = {'task_id': self.active['task_id'], 'selected_tier': selected.get('tier'),
                       'selected_model': selected.get('model'), 'classifier_calls': len(classifier),
                       'generation_calls': len(generation), 'model_switched': len(identities) > 1,
                       'pinning_verified': len(classifier) == 1 and len(identities) == 1,
                       'context': self.active['context'],
                       'calls': calls}
            result['reported_model'] = routing['selected_model']
            result['routing'] = routing
            with (args.output / 'routing.jsonl').open('a') as output:
                output.write(json.dumps(routing) + '\n')
            print(f"  routed={routing['selected_tier']} model={routing['selected_model']} "
                  f"calls={len(calls)} pinned={routing['pinning_verified']} completed={result['completed']}", flush=True)
            return result

    adapter = RouterAdapter(name='pablo-jev-vercel', harness='pablo', requested_model='jev-task-router',
                            reasoning='provider_default', provider='vercel')
    # Freeze before the first task so edits cannot silently change a running suite.
    config_text = args.config.read_text()
    with tempfile.TemporaryDirectory(prefix='pablo-jev-config-') as frozen_dir:
        args.config = Path(frozen_dir).resolve() / 'router-config.toml'
        args.config.write_text(config_text)
        # Verify config without loading credentials or sending requests.
        subprocess.run([str(args.binary), 'config', 'validate', '--config', str(args.config),
                        '--bind', f'workspace={args.benchmark}', '--bind', f'secrets={PABLO_ROOT}'],
                       check=True, env=adapter.child_env(), stdout=subprocess.DEVNULL)
        report = run_suite(catalog_dir=args.benchmark / 'tasks', output_dir=args.output,
                           selected_tasks=task_ids, selected_adapters=[adapter], repeats=1,
                           timeout_s=args.timeout, seed=42, env_file=PABLO_ROOT / '.env',
                           binaries={adapter.name: str(args.binary)})
    # Keep partial artifacts inspectable, but honor the benchmark contract that
    # failed attempts contribute zero to the headline score.
    for attempt in report['attempts']:
        if attempt['status'] != 'completed':
            attempt['partial_artifact_score'] = dict(attempt['score'])
            attempt['score'] = {**attempt['score'], 'factual_accuracy': 0.0, 'citation_score': 0.0}
            (args.output / 'attempts' / attempt['attempt'] / 'result.json').write_text(json.dumps(attempt, indent=2) + '\n')
    routes = [attempt['events']['routing'] for attempt in report['attempts']]
    provenance = {
        'source_commit': source_commit,
        'binary_path': str(args.binary), 'binary_sha256': binary_sha256,
        'config_sha256': hashlib.sha256(config_text.encode()).hexdigest(),
        'runner_sha256': runner_sha256,
        'context_builder_sha256': context_builder_sha256,
        'python': sys.version.split()[0],
        'benchmark_files_sha256': {name: hashlib.sha256((args.benchmark / 'legalbench' / name).read_bytes()).hexdigest()
                                   for name in ['adapters.py', 'runner.py', 'scoring.py', 'monitor.py', 'report.py']},
        'prompt_difference': 'Original benchmark prompt plus task.md and bounded source documents; no answer key inline.'
                             if args.source_context == 'full' else 'Original benchmark prompt plus task.md in a JSON context envelope; no source documents or answer key inline.',
        'source_context': args.source_context,
        'context_max_bytes': args.context_max_bytes,
        'limits': {'model_calls': 9, 'generation_allowance': 8, 'tool_calls': 80, 'timeout_s': args.timeout},
        'cost': 'Unknown when Vercel does not report actual cost; no mixed-model estimate from aggregate usage.',
    }
    report['routing_summary'] = {'selected_tiers': dict(Counter(r['selected_tier'] for r in routes)),
                                 'selected_models': dict(Counter(r['selected_model'] for r in routes)),
                                 'classifier_calls': sum(r['classifier_calls'] for r in routes),
                                 'tasks_with_model_switches': sum(r['model_switched'] for r in routes),
                                 'tasks_with_verified_pinning': sum(r['pinning_verified'] for r in routes)}
    report['local_release'] = provenance
    metadata = {k: v for k, v in report.items() if k not in {'attempts', 'summary', 'by_family'}}
    report = build_report(metadata, report['attempts'], args.output)
    (args.output / 'router-config.toml').write_text(config_text)
    columns = ['task_id', 'selected_tier', 'selected_model', 'status', 'factual_accuracy', 'citation_score',
               'wall_seconds', 'classifier_calls', 'generation_calls', 'model_switched', 'pinning_verified']
    rows = []
    for attempt in report['attempts']:
        routing = attempt['events']['routing']
        rows.append({**{k: routing[k] for k in ['task_id', 'selected_tier', 'selected_model', 'classifier_calls',
                                               'generation_calls', 'model_switched', 'pinning_verified']},
                     'status': attempt['status'], 'factual_accuracy': attempt['score']['factual_accuracy'],
                     'citation_score': attempt['score']['citation_score'],
                     'wall_seconds': round(attempt['resources']['wall_ms'] / 1000, 3)})
    with (args.output / 'routing.csv').open('w', newline='') as output:
        writer = csv.DictWriter(output, fieldnames=columns)
        writer.writeheader()
        writer.writerows(rows)
    with (args.output / 'report.md').open('a') as output:
        output.write('\n## Per-task model selection\n\n')
        output.write('| Task | Tier | Exact model | Status | Accuracy | Seconds | Model calls | Pinned |\n')
        output.write('|---|---|---|---|---:|---:|---:|---|\n')
        for row in rows:
            output.write(f"| {row['task_id']} | {row['selected_tier']} | {row['selected_model']} | {row['status']} | "
                         f"{row['factual_accuracy']:.1%} | {row['wall_seconds']:.2f} | "
                         f"{row['classifier_calls'] + row['generation_calls']} | {row['pinning_verified']} |\n")
        output.write(f'\nOne local release run, with {args.source_context} context in the initial input for classification and generation. '
                     'All reasoning settings are provider-default. The nine-call cap includes one Jev evaluation and '
                     'up to eight generation calls. See report.json for binary/config fingerprints and per-call usage.\n')
    print(json.dumps({'summary': report['summary'], 'routing': report['routing_summary'],
                      'report': str(args.output / 'report.md')}, indent=2))


if __name__ == '__main__':
    main()
