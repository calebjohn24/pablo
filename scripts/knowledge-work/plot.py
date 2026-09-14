"""Render measured benchmark rows; requires matplotlib (pilot: 3.11.2)."""
import json, math, sys
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

NAMES = {'pablo':'Pablo · GLM Flash', 'pablo-astra':'Pablo · Astra', 'codex':'Codex · Astra',
         'claude':'Claude Code · Fable 5.1', 'ori':'Ori + Pi · GLM Flash', 'pi':'Pi · GLM Flash'}
def quantile(values):
    return sorted(values)[math.ceil(len(values)*.5)-1] if values else None

def render(source, destination):
    data=json.loads(Path(source).read_text())
    groups={h:[r for r in data['rows'] if r['harness']==h] for h in NAMES}
    groups={h:r for h,r in groups.items() if r}
    names=[NAMES[h] for h in groups]
    facts=[]; walls=[]; memory=[]; cpu=[]; utilization=[]; costs=[]
    for rows in groups.values():
        checks=[c for r in rows for c in (r.get('correctness') or {}).get('checks', [{'value':False}]*r.get('factual_total', 0))]
        facts.append(100*sum(c['value'] for c in checks)/len(checks) if checks else None)
        measured=[r['resources'] for r in rows if r.get('resources')]
        walls.append(quantile([r['wall_ms']/1000 for r in measured]))
        memory.append(max((r['sampled_tree_peak_rss_kib']/1024 for r in measured), default=None))
        cpu.append(quantile([r['cpu_total_s'] for r in measured]))
        utilization.append(quantile([r['cpu_percent_one_core'] for r in measured]))
        reported=[r.get('events',{}).get('reported_cost_usd') if r.get('events',{}).get('reported_cost_usd') is not None else r.get('events',{}).get('estimated_cost_usd') for r in rows]
        costs.append(sum(reported) if all(isinstance(v,(int,float)) for v in reported) else None)
    # One shared row per configuration avoids repeating six long labels in every panel.
    ink, muted, rule, accent = '#172D35', '#52656D', '#DEE6E8', '#187D86'
    plt.rcParams.update({'font.family':'DejaVu Sans','font.size':12,'svg.fonttype':'none'})
    fig = plt.figure(figsize=(18, 9), facecolor='#FFFFFF')
    ax = fig.add_axes([0, 0, 1, 1])
    ax.set(xlim=(0, 18), ylim=(0, 9)); ax.axis('off')
    ax.text(.65, 8.48, 'PABLO  /  BENCHMARKS', fontsize=11, color=accent, weight='bold')
    ax.text(.65, 7.91, 'Knowledge work, measured.', fontsize=29, color=ink, weight='bold')
    ax.text(.65, 7.47, data.get('chart', {}).get('subtitle', 'Six synthetic tasks per configuration · one seed · one attempt per task'), fontsize=13, color=muted)

    columns = [
        ('Accuracy ↑', 'Factual checks', facts, lambda v:f'{v:.0f}%', 100),
        ('Latency ↓', 'p50 · seconds', walls, lambda v:f'{v:.2f}', None),
        ('Cost ↓', '6 tasks · USD', costs, lambda v:f'${v:.4f}', None),
        ('Memory ↓', 'Peak tree · MiB', memory, lambda v:f'{v:.2f}', None),
        ('CPU time ↓', 'p50 · seconds', cpu, lambda v:f'{v:.3f}', None),
        ('CPU use ↓', 'p50 · % of one core', utilization, lambda v:f'{v:.2f}%', None),
    ]
    starts = [4.00, 6.22, 8.44, 10.66, 12.88, 15.10]
    ax.text(.65, 6.83, 'Harness / model', color=ink, fontsize=13, weight='bold')
    for x, (title, subtitle, _, _, _) in zip(starts, columns):
        ax.text(x, 6.96, title, color=ink, fontsize=13, weight='bold')
        ax.text(x, 6.65, 'Higher is better' if title.startswith('Accuracy') else 'Lower is better', color=muted, fontsize=10)
        ax.text(x, 6.38, subtitle, color=muted, fontsize=10)
    ax.plot([.65, 17.35], [6.18, 6.18], color=rule, lw=1)
    row_step = .76
    for i, (h, rows) in enumerate(groups.items()):
        y = 5.88-i*row_step
        if i % 2 == 0:
            ax.add_patch(plt.Rectangle((.45, y-.43), 17.10, .74, color='#F5F8F9', lw=0))
        harness, model = NAMES[h].split(' · ')
        ax.text(.65, y+.025, harness, fontsize=15, color=ink, weight='bold', va='center')
        suffix = ' · rerun' if h in data.get('chart', {}).get('refreshed_harnesses', []) else ''
        ax.text(.65, y-.245, model+suffix, fontsize=11, color=muted, va='center')
        for x, (title, _, values, fmt, limit) in zip(starts, columns):
            v=values[i]
            estimated = title.startswith('Cost') and any(r.get('events',{}).get('reported_cost_usd') is None and r.get('events',{}).get('estimated_cost_usd') is not None for r in rows)
            label = (fmt(v)+('*' if estimated else '')) if v is not None else '—'
            ax.text(x, y+.025, label, fontsize=17, color=ink, va='center')
            scale=limit or max((n for n in values if n is not None), default=1) or 1
            ax.add_patch(plt.Rectangle((x, y-.27), 1.64, .055, color=rule, lw=0))
            if v is not None:
                ax.add_patch(plt.Rectangle((x, y-.27), 1.64*v/scale, .055, color=accent, lw=0))
    ax.plot([.65, 17.35], [.98, .98], color=rule, lw=1)
    ax.text(.65, .69, 'Bars use independent linear scales from zero in each column. ↑ Higher is better; ↓ lower is better.', fontsize=10.5, color=muted)
    ax.text(.65, .43, '*Codex cost: standard token-price estimate. Other costs: client-reported, not necessarily billed amounts.', fontsize=10.5, color=muted)
    ax.text(.65, .17, data.get('chart', {}).get('footer', 'Exploratory pilot · all attempts included · provider routes and reasoning settings differ · local CPU and RAM only'), fontsize=10.5, color=muted)
    dest=Path(destination);dest.parent.mkdir(parents=True,exist_ok=True)
    fig.savefig(str(dest)+'.png',dpi=180)
    fig.savefig(str(dest)+'.svg')
    svg = Path(str(dest)+'.svg')
    svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines())+'\n')
    plt.close(fig)

if __name__=='__main__':render(sys.argv[1],sys.argv[2])
