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
    groups={h:[r for r in data['rows'] if r['harness']==h and r['status']=='completed'] for h in NAMES}
    groups={h:r for h,r in groups.items() if r}
    names=[NAMES[h] for h in groups]
    facts=[]; walls=[]; memory=[]; cpu=[]; utilization=[]; costs=[]
    for rows in groups.values():
        checks=[c for r in rows for c in r['correctness']['checks']]
        facts.append(100*sum(c['value'] for c in checks)/len(checks))
        walls.append(quantile([r['resources']['wall_ms']/1000 for r in rows]))
        memory.append(max(r['resources']['sampled_tree_peak_rss_kib']/1024 for r in rows))
        cpu.append(quantile([r['resources']['cpu_total_s'] for r in rows]))
        utilization.append(quantile([r['resources']['cpu_percent_one_core'] for r in rows]))
        reported=[r.get('events',{}).get('reported_cost_usd') if r.get('events',{}).get('reported_cost_usd') is not None else r.get('events',{}).get('estimated_cost_usd') for r in rows]
        costs.append(sum(reported) if all(isinstance(v,(int,float)) for v in reported) else None)
    plt.rcParams.update({'font.family':'DejaVu Sans','font.size':10,'svg.fonttype':'none','axes.spines.top':False,'axes.spines.right':False})
    fig,axs=plt.subplots(3,2,figsize=(15,10),layout='constrained')
    panels=[('Factual accuracy · higher is better',facts,'%',lambda v:f'{v:.0f}%',105),
            ('Completion latency · p50',walls,'seconds',lambda v:f'{v:.2f}s',None),
            ('Peak sampled process-tree memory',memory,'MiB',lambda v:f'{v:.2f}',None),
            ('Local CPU time · p50',cpu,'CPU seconds',lambda v:f'{v:.3f}s',None),
            ('Local CPU utilization · p50',utilization,'% of one core',lambda v:f'{v:.2f}%',None),
            ('Cost · total over six tasks',costs,'USD',lambda v:f'${v:.4f}',None)]
    for ax,(title,values,unit,fmt,limit) in zip(axs.flat,panels):
        ax.barh(names,[v or 0 for v in values],color='#24778c',height=.55)
        ax.invert_yaxis();ax.set_title(title,loc='left',pad=12,weight='medium');ax.set_xlabel(unit)
        maximum=max((v for v in values if v is not None),default=1)
        ax.set_xlim(0,limit or maximum*1.3 or 1)
        ax.grid(axis='x',alpha=.18);ax.set_axisbelow(True)
        for i,v in enumerate(values):ax.text((v or 0)+ax.get_xlim()[1]*.02,i,(fmt(v)+('*' if title.startswith('Cost') and list(groups)[i]=='codex' else '')) if v is not None else 'Not reported',va='center')
    fig.suptitle('Knowledge-work pilot · six synthetic tasks per configuration',fontsize=18,weight='medium')
    fig.supxlabel('One seed and attempt per task · corrected handoff fixture · local resources only\nCost is client-reported, not necessarily subscription billing. *Codex: standard token-price estimate. Provider routing and effort differ.',fontsize=10)
    dest=Path(destination);dest.parent.mkdir(parents=True,exist_ok=True)
    fig.savefig(str(dest)+'.png',dpi=160)
    fig.savefig(str(dest)+'.svg')
    plt.close(fig)

if __name__=='__main__':render(sys.argv[1],sys.argv[2])
