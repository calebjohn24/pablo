"""Standalone measured comparison; no old benchmark files are overwritten."""
import json, sys
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

PROFILES = [
    ('pablo-provider_default', 'Pablo · GLM Flash', 'Provider default'),
    ('pablo-low', 'Pablo · GLM Flash', 'Low effort'),
    ('pablo-astra-provider_default', 'Pablo · Astra', 'Provider default'),
    ('pablo-astra-low', 'Pablo · Astra', 'Low effort'),
    ('pi-low', 'Pi · GLM Flash', 'Low effort'),
]

def render(source, destination):
    report = json.loads(Path(source).read_text())
    summary = report['summary']
    ink, muted, rule = '#172D35', '#52656D', '#DEE6E8'
    accent, baseline = '#187D86', '#7B8E97'
    plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 12, 'svg.fonttype': 'none'})
    fig = plt.figure(figsize=(18, 9), facecolor='white')
    ax = fig.add_axes([0, 0, 1, 1]); ax.set(xlim=(0, 18), ylim=(0, 9)); ax.axis('off')
    ax.text(.65, 8.47, 'PABLO  /  REASONING & LATENCY', fontsize=11, color=accent, weight='bold')
    ax.text(.65, 7.91, 'Reasoning, measured across models.', fontsize=28, color=ink, weight='bold')
    ax.text(.65, 7.47, '6 tasks × 3 seeds × 3 repetitions · 54 attempts per configuration · same OpenRouter account', fontsize=12.5, color=muted)
    metrics = [
        ('Accuracy ↑', 'Factual checks', 'factual_accuracy', 100, lambda v: f'{v:.1f}%', 100),
        ('Latency ↓', 'p50 · seconds', 'wall_p50_ms', .001, lambda v: f'{v:.2f}', None),
        ('Cost ↓', '54 attempts · USD', 'reported_cost_usd', 1, lambda v: f'${v:.4f}', None),
        ('Memory ↓', 'Peak tree · MiB', 'peak_tree_rss_kib', 1/1024, lambda v: f'{v:.2f}', None),
        ('CPU time ↓', 'p50 · seconds', 'cpu_p50_s', 1, lambda v: f'{v:.3f}', None),
        ('CPU use ↓', 'p50 · % of one core', 'cpu_one_core_p50_percent', 1, lambda v: f'{v:.2f}%', None),
    ]
    starts = [4, 6.22, 8.44, 10.66, 12.88, 15.10]
    ax.text(.65, 6.82, 'Harness / model / effort', color=ink, fontsize=13, weight='bold')
    for x, (title, unit, *_rest) in zip(starts, metrics):
        ax.text(x, 6.96, title, color=ink, fontsize=13, weight='bold')
        ax.text(x, 6.65, 'Higher is better' if title.startswith('Accuracy') else 'Lower is better', color=muted, fontsize=10)
        ax.text(x, 6.38, unit, color=muted, fontsize=10)
    ax.plot([.65, 17.35], [6.16, 6.16], color=rule, lw=1)
    for i, (profile, name, effort) in enumerate(PROFILES):
        s = summary[profile]; y = 5.75 - i*.86
        if i%2 == 0:
            ax.add_patch(plt.Rectangle((.45, y-.47), 17.1, .82, color='#F5F8F9', lw=0))
        ax.text(.65, y+.06, name, fontsize=14, color=ink, weight='bold', va='center')
        ax.text(.65, y-.23, effort, fontsize=11, color=muted, va='center')
        color = baseline if effort == 'Provider default' else accent
        for x, (title, unit, key, factor, fmt, ceiling) in zip(starts, metrics):
            value = s.get(key); value = value*factor if value is not None else None
            label = fmt(value) if value is not None else 'Unknown'
            ax.text(x, y+.07, label, fontsize=17, color=ink, va='center')
            if key == 'wall_p50_ms':
                p95 = s.get('wall_p95_ms')
                ax.text(x, y-.20, f'p95 {p95/1000:.2f} s' if p95 is not None else 'p95 unknown', fontsize=10, color=muted, va='center')
            elif key == 'factual_accuracy':
                ax.text(x, y-.20, f'{s["factual_correct"]} / {s["factual_total"]} facts', fontsize=10, color=muted, va='center')
            limit = ceiling or max((summary[p].get(key) or 0)*factor for p, _, _ in PROFILES) or 1
            ax.add_patch(plt.Rectangle((x, y-.36), 1.64, .055, color=rule, lw=0))
            if value is not None:
                ax.add_patch(plt.Rectangle((x, y-.36), 1.64*value/limit, .055, color=color, lw=0))
    ax.plot([.65, 17.35], [1.42, 1.42], color=rule, lw=1)
    completed = sum(r['status']=='completed' for r in report['rows'])
    ax.text(.65, 1.10, f'{completed}/{report["attempts"]} completed. All attempts included; facts are scored independently of latency. Costs are client-reported, not invoices.', fontsize=10.5, color=muted)
    ax.text(.65, .79, 'Bars use independent linear scales from zero. Gray: provider default. Teal: low effort. Latency bars show p50; p95 is labeled.', fontsize=10.5, color=muted)
    ax.text(.65, .48, 'Local CPU and RAM only. Provider routing, caching and native tool/prompt differences remain. Repeated synthetic tasks do not establish general quality.', fontsize=10.5, color=muted)
    ax.text(.65, .17, 'CPU use is averaged over task wall time: faster responses can increase utilization even when CPU time is unchanged.', fontsize=10.5, color=muted)
    dest = Path(destination); dest.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(str(dest)+'.png', dpi=180); fig.savefig(str(dest)+'.svg'); plt.close(fig)
    svg = Path(str(dest)+'.svg'); svg.write_text('\n'.join(s.rstrip() for s in svg.read_text().splitlines())+'\n')

if __name__ == '__main__': render(sys.argv[1], sys.argv[2])
