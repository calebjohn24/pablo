"""POSIX serial process supervisor. Never reads credentials or environment files."""
import json, os, selectors, signal, subprocess, sys, time
from pathlib import Path

def run(spec):
    out = Path(spec['output'])
    out.mkdir(parents=True, exist_ok=True)
    interval = spec.get('sample_ms', 100) / 1000
    start = time.monotonic()
    child = subprocess.Popen(spec['argv'], cwd=spec['cwd'], stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    sel = selectors.DefaultSelector()
    for name, pipe in [('stdout', child.stdout), ('stderr', child.stderr)]:
        os.set_blocking(pipe.fileno(), False)
        sel.register(pipe, selectors.EVENT_READ, name)
    files = {n: (out / (n + '.jsonl')).open('wb') for n in ['stdout', 'stderr']}
    receipts = (out / 'receipts.jsonl').open('w')
    samples, sizes, first = [], {'stdout': 0, 'stderr': 0}, {'stdout': None, 'stderr': None}
    status = usage = None
    reason = None
    cleanup_sent = False
    root_end = None
    next_sample = start
    interrupted = False
    old = {}
    known = {}
    def tree_snapshot():
        ps = subprocess.run(['ps', '-axo', 'pid=,ppid=,pgid=,rss=,lstart='], capture_output=True, text=True, timeout=2, check=True)
        rows = []
        for line in ps.stdout.splitlines():
            fields = line.split()
            if len(fields) >= 9:
                rows.append((*map(int, fields[:4]), ' '.join(fields[4:])))
        members = {child.pid}
        members |= {pid for pid,_,_,_,born in rows if known.get(pid) == born}
        for _ in range(len(rows)):
            extra = {pid for pid,ppid,pgid,_,_ in rows if ppid in members or pgid == child.pid}
            if extra <= members: break
            members |= extra
        for pid,_,_,_,born in rows:
            if pid in members: known[pid] = born
        return rows, members
    def request_stop(*_):
        nonlocal interrupted
        interrupted = True
    for sig in (signal.SIGTERM, signal.SIGINT):
        old[sig] = signal.signal(sig, request_stop)
    def kill_group():
        # Native tools can create their own process groups. Kill observed descendants
        # too, checking process birth identity before following a reparented PID.
        try:
            rows, members = tree_snapshot()
            for pid,_,_,_,_ in reversed(rows):
                if pid in members and pid != child.pid:
                    try: os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError: pass
        except (subprocess.SubprocessError, ValueError):
            pass
        try: os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError: pass
    try:
        while status is None or sel.get_map():
            now = time.monotonic()
            if not cleanup_sent and (interrupted or now - start > spec['timeout_s'] or any(s > 16*1024*1024 for s in sizes.values())):
                reason = 'interrupted' if interrupted else 'timeout' if now-start > spec['timeout_s'] else 'output_limit'
                kill_group()
                cleanup_sent = True
            if status is None and now >= next_sample:
                try:
                    rows, members = tree_snapshot()
                    samples.append({'ms': (now-start)*1000, 'rss_kib': sum(rss for pid,_,_,rss,_ in rows if pid in members), 'processes': sum(pid in members for pid,_,_,_,_ in rows)})
                except (subprocess.SubprocessError, ValueError):
                    samples.append({'ms': (now-start)*1000, 'rss_kib': None, 'processes': None})
                next_sample = time.monotonic() + interval
            for key, _ in sel.select(0.02):
                data = os.read(key.fileobj.fileno(), 65536)
                if not data:
                    sel.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                name = key.data
                ms = (time.monotonic()-start)*1000
                if first[name] is None: first[name] = ms
                sizes[name] += len(data)
                if sizes[name] <= 16*1024*1024:
                    files[name].write(data)
                    receipts.write(json.dumps({'stream': name, 'end_byte': sizes[name], 'ms': ms})+'\n')
            if status is None:
                pid, wait_status, ru = os.wait4(child.pid, os.WNOHANG)
                if pid:
                    status, usage = wait_status, ru
                    child.returncode = os.waitstatus_to_exitcode(status)
                    root_end = time.monotonic()
                    # End the owned process group even if a tool left pipes open.
                    kill_group()
            if root_end and time.monotonic()-root_end > 3:
                reason = reason or 'pipe_cleanup_timeout'
                break
    finally:
        kill_group()
        if status is None:
            _, status, usage = os.wait4(child.pid, 0)
            child.returncode = os.waitstatus_to_exitcode(status)
        for key in list(sel.get_map().values()): key.fileobj.close()
        sel.close()
        for f in files.values(): f.close()
        receipts.close()
        for sig, handler in old.items(): signal.signal(sig, handler)
    wall = time.monotonic()-start
    cpu = usage.ru_utime + usage.ru_stime
    valid = [s['rss_kib'] for s in samples if s['rss_kib'] is not None]
    result = {'exit_code': child.returncode, 'failure': reason, 'wall_ms': wall*1000,
              'first_stdout_ms': first['stdout'], 'first_stderr_ms': first['stderr'],
              'cpu_user_s': usage.ru_utime, 'cpu_system_s': usage.ru_stime,
              'cpu_total_s': cpu, 'cpu_percent_one_core': cpu/wall*100,
              'cpu_percent_machine': cpu/wall*100/(os.cpu_count() or 1),
              'sampled_tree_peak_rss_kib': max(valid) if valid else None,
              'rusage_maxrss_kib': usage.ru_maxrss/(1024 if sys.platform=='darwin' else 1),
              'sample_interval_ms': interval*1000, 'samples': samples,
              'stdout_bytes': sizes['stdout'], 'stderr_bytes': sizes['stderr'],
              'cpu_scope': 'wait4 root and descendants whose resource usage was reaped into it; unjoined descendants can be omitted',
              'memory_scope': 'sampled simultaneous RSS sum of process group and descendants; shared pages may double count, short-lived peaks can be missed'}
    (out/'resources.json').write_text(json.dumps(result, indent=2)+'\n')
    return result

if __name__ == '__main__':
    run(json.loads(Path(sys.argv[1]).read_text()))
