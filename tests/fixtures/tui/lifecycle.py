from screen import Screen
"""T02 real-PTY lifecycle cases with synthetic providers and native trace assertions."""
import fcntl
import http.server
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

BINARY = os.path.abspath(sys.argv[1])
CASE = sys.argv[2]
assert CASE in ('success', 'model_cancel', 'model_eof', 'tool_cancel', 'provider_error', 'resize', 'slow_output', 'sigterm', 'local_child', 'remote_child', 'input_eof')
RELEASE = threading.Event()
REQUESTED = threading.Event()


def wire(delta, finish=None):
    return ('data: ' + json.dumps({'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]}) + '\n\n').encode()


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        REQUESTED.set()
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        try:
            if CASE in ('local_child', 'remote_child'):
                if request['model'] == 'zai/glm-5.3-flash':
                    self.wfile.write(wire({'content': 'local-child-stream'}))
                    self.wfile.flush()
                    RELEASE.wait(12)
                    return
                results = [json.loads(m['content']) for m in request['messages'] if m['role'] == 'tool']
                if not results:
                    args = {'action': 'spawn', 'request': {'input': 'local child task', 'capabilities': {'model_route': ['secondary'], 'tools': []}}} if CASE == 'local_child' else {'action': 'spawn_remote', 'remote_request': {'remote': 'peer', 'parts': [{'text': 'hold'}], 'accepted_output_modes': ['text/plain'], 'stream': True}}
                else:
                    args = {'action': 'wait', 'agent_ids': [results[0]['subagent']['agent']['agent_id']], 'mode': 'all', 'timeout_ms': 5000}
                self.wfile.write(wire({'tool_calls': [{'index': 0, 'id': 'child-case-' + str(len(results)), 'type': 'function', 'function': {'name': 'subagent', 'arguments': json.dumps(args)}}]}, 'tool_calls') + b'data: [DONE]\n\n')
            elif CASE == 'provider_error':
                self.wfile.write(b'data: {invalid json}\n\n')
            elif CASE == 'tool_cancel':
                self.wfile.write(wire({'tool_calls': [{'index': 0, 'id': 'shell-case', 'type': 'function', 'function': {'name': 'shell_run', 'arguments': json.dumps({'command': 'echo $$ > owned.pid; exec sleep 30', 'cwd': '.'})}}]}, 'tool_calls'))
                self.wfile.write(b'data: [DONE]\n\n')
            else:
                text = 'stream-visible\x1b]52;c;INERT\x07\x1b[31m\u009b2J\u202e\n'
                if CASE == 'slow_output':
                    # A stalled consumer must face changing frames, not idle redraws.
                    for index in range(200):
                        self.wfile.write(wire({'content': f'bounded-render-{index:04d}\n' * 80}))
                        self.wfile.flush()
                        if RELEASE.wait(0.03): break
                self.wfile.write(wire({'content': text}))
                self.wfile.flush()
                if CASE in ('model_cancel', 'model_eof', 'resize', 'slow_output', 'sigterm', 'input_eof'):
                    RELEASE.wait(12)
                self.wfile.write(wire({}, 'stop') + b'data: [DONE]\n\n')
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


def session_owner():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


def gone(pid):
    try:
        os.kill(pid, 0)
        return False
    except ProcessLookupError:
        return True


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever)
thread.start()
master, slave = pty.openpty()
saved = termios.tcgetattr(slave)
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
owner = None
pablo_pid = None
peer = None
try:
    with tempfile.TemporaryDirectory(prefix='pablo-tui-lifecycle-') as cwd:
        cwd = Path(cwd).resolve()
        trace = cwd / 'trace.jsonl'
        wrapper = "import subprocess,sys; child=subprocess.Popen(sys.argv[1:]); print('FIXTURE_PID='+str(child.pid),flush=True); code=child.wait(); print('\\nFIXTURE_EXIT='+str(code),flush=True); sys.stdin.readline(); print('FIXTURE_READY',flush=True); sys.stdin.readline(); sys.exit(code)"
        args = [BINARY, 'tui', '--trace', str(trace), '--no-filesystem']
        if CASE != 'tool_cancel':
            args.append('--no-shell')
        if CASE in ('local_child', 'remote_child'):
            root = Path(__file__).resolve().parents[3]
            base = (root / 'docs/project/fixtures/c3-model-routes/three-providers.toml').read_text().replace('max_model_calls=2', 'max_model_calls=20').replace('max_tool_calls=1', 'max_tool_calls=10')
            config = base + '\n[options.children]\nenabled=true\n[options.trace]\ncapture_content=false\npath={base="workspace",path="trace.jsonl"}\n'
            args = [BINARY, 'tui', '--config', str(cwd / 'entry.toml'), '--bind', f'workspace={cwd}', '--fixture-endpoint', f'http://127.0.0.1:{server.server_port}']
            if CASE == 'remote_child':
                peer = subprocess.Popen([sys.executable, str(root / 'tests/fixtures/a2a/wire_server.py')], cwd=cwd, env={}, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                assert select.select([peer.stdout], [], [], 5)[0], 'remote peer readiness'
                rpc = json.loads(peer.stdout.readline())['url']
                config += '\n[options.a2a.remotes.peer]\ncard_url="https://agent.example.test/.well-known/agent-card.json"\nendpoint="https://agent.example.test/rpc"\n'
                args += ['--fixture-a2a-endpoint', f'peer={rpc}']
            (cwd / 'entry.toml').write_text(config)
        owner = subprocess.Popen([sys.executable, '-c', wrapper, *args], stdin=slave, stdout=slave, stderr=slave, cwd=cwd, env={'TERM': 'xterm-256color', 'PABLO_FIXTURE_ENDPOINT': f'http://127.0.0.1:{server.server_port}'}, preexec_fn=session_owner)
        output = bytearray()
        display = Screen()

        def drain(timeout=0.05):
            if select.select([master], [], [], timeout)[0]:
                data = os.read(master, 65536)
                output.extend(data); display.feed(data)
            assert len(output) < 4 * 1024 * 1024, 'fixture output bound'

        def wait_for(predicate, timeout=8):
            deadline = time.monotonic() + timeout
            while not predicate():
                assert owner.poll() is None, bytes(output[-1200:])
                assert time.monotonic() < deadline, bytes(output[-1200:])
                drain()

        def screen():
            return display.text()

        def events():
            if not trace.exists():
                return []
            # A writer can be between writes; only consume complete records.
            return [json.loads(line) for line in trace.read_bytes().split(b'\n')[:-1]]

        def terminal_events():
            return [e for e in events() if e['type'] == 'run.finished' and e.get('agent', {}).get('depth', 0) == 0]

        wait_for(lambda: b'Enter: new independent task' in output and b'FIXTURE_PID=' in output)
        pablo_pid = int(bytes(output).split(b'FIXTURE_PID=')[1].splitlines()[0])
        # Exercise UTF-8 insertion, delete, home/end and bracketed paste in the actual PTY.
        os.write(master, 'ab🦀c'.encode() + b'\x1b[D\x1b[3~\x7f\x1b[H\x1b[CX\x1b[F\x1b[200~\nline\x1b[201~')
        wait_for(lambda: b'> aXb' in screen())
        assert not REQUESTED.is_set(), 'paste must not submit'
        os.write(master, b'\r')
        wait_for(REQUESTED.is_set)
        expected_code = 0
        expected_status = 'completed'
        owned_pid = None
        if CASE in ('local_child', 'remote_child'):
            marker = b'local-child-stream' if CASE == 'local_child' else b'Remote Some(Working)'
            wait_for(lambda: marker in screen())
            os.write(master, b'\x03')
            expected_code, expected_status = 130, 'cancelled'
        elif CASE == 'tool_cancel':
            wait_for(lambda: (cwd / 'owned.pid').exists() and b'Tool shell.run' in screen())
            owned_pid = int((cwd / 'owned.pid').read_text())
            assert not gone(owned_pid)
            os.write(master, b'\x03')
            expected_code, expected_status = 130, 'cancelled'
        elif CASE in ('model_cancel', 'model_eof', 'sigterm', 'input_eof'):
            wait_for(lambda: b'stream-visible' in screen())
            if CASE == 'input_eof':
                # Real read(2) EOF: VMIN=VTIME=0 returns zero with no pending input.
                modes = termios.tcgetattr(slave)
                modes[6][termios.VMIN] = 0
                modes[6][termios.VTIME] = 0
                termios.tcsetattr(slave, termios.TCSANOW, modes)
                expected_code = 130
            elif CASE == 'sigterm':
                os.kill(pablo_pid, signal.SIGTERM)
                expected_code = 143
            else:
                os.write(master, b'\x03' if CASE == 'model_cancel' else b'\x04')
                expected_code = 130
            expected_status = 'cancelled'
        elif CASE == 'resize':
            wait_for(lambda: b'stream-visible' in screen())
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 5, 30, 0, 0))
            display.resize(5,30)
            wait_for(lambda: b'Resize terminal to continue' in screen())
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 140, 0, 0))
            display.resize(40,140)
            wait_for(lambda: b'Ctrl-C: cancel and join' in screen() and b'stream-visible' in screen())
            RELEASE.set()
        elif CASE == 'slow_output':
            # Stop consuming the master until the 500 ms frame-write deadline fires.
            # Poll only the native trace, proving cleanup progresses without screen reads.
            deadline = time.monotonic() + 6
            while not terminal_events():
                assert time.monotonic() < deadline, 'blocked rendering did not cancel native run'
                time.sleep(0.025)
            expected_code, expected_status = 1, 'cancelled'
        elif CASE == 'provider_error':
            expected_code, expected_status = 1, 'failed'
        if CASE not in ('model_eof', 'slow_output', 'sigterm', 'input_eof'):
            wait_for(lambda: expected_status.encode() in screen())
            os.write(master, b'\x04')
        wait_for(lambda: f'FIXTURE_EXIT={expected_code}'.encode() in output)
        RELEASE.set()
        assert b'\x1b]52;' not in output and b'\x1b[31m' not in output
        assert '\u009b'.encode() not in output and '\u202e'.encode() not in output
        assert b'\x1b[?2004l\x1b[?25h\x1b[?1049l' in output, 'cursor/paste/screen restoration missing'
        os.write(master, b'\n')
        wait_for(lambda: b'FIXTURE_READY' in output)
        assert termios.tcgetattr(slave) == saved, 'terminal modes not restored'
        assert gone(pablo_pid), 'Pablo still alive after reported exit'
        if owned_pid is not None:
            assert gone(owned_pid), 'owned shell survived terminal cancellation'
        records = events()
        terminals = terminal_events()
        assert len(terminals) == 1, terminals
        assert terminals[0]['outcome']['status'] == expected_status, terminals[0]
        assert records[-1] == terminals[0], 'events emitted after terminal'
        if CASE in ('local_child', 'remote_child'):
            children = [e for e in records if e.get('agent', {}).get('depth') == 1]
            ends = [e for e in children if e['type'] == 'run.finished']
            assert len(ends) == 1 and ends[0]['outcome']['status'] == 'cancelled', ends
            assert records.index(ends[0]) < records.index(terminals[0]), 'root settled before child'
            if CASE == 'remote_child':
                calls = [json.loads(line)['method'] for line in (cwd / 'calls.jsonl').read_text().splitlines()]
                assert calls.count('CancelTask') == 1, calls
        if owned_pid is not None:
            assert any(e['type'] == 'tool.finished' for e in records)
        os.write(master, b'\n')
        deadline = time.monotonic() + 5
        while owner.poll() is None:
            assert time.monotonic() < deadline, 'PTY owner exit deadline'
            drain()
        assert owner.wait(timeout=1) == expected_code
        print(json.dumps({'childJoined': CASE in ('local_child', 'remote_child'), 'case': CASE, 'nativeTerminal': expected_status, 'exactlyOneTerminal': True, 'terminalRestored': True, 'pabloExited': True, 'ownedShellJoined': owned_pid is not None, 'exitCode': expected_code}))
finally:
    RELEASE.set()
    if pablo_pid is not None and not gone(pablo_pid):
        os.kill(pablo_pid, signal.SIGTERM)
        deadline = time.monotonic() + 5
        while not gone(pablo_pid) and time.monotonic() < deadline:
            if select.select([master], [], [], 0.05)[0]:
                os.read(master, 65536)
        if not gone(pablo_pid):
            os.kill(pablo_pid, signal.SIGKILL)
    os.close(master)
    if owner is not None and owner.poll() is None:
        owner.kill()
        owner.wait(timeout=5)
    os.close(slave)
    if peer is not None:
        peer.terminate()
        try:
            peer.wait(timeout=3)
        except subprocess.TimeoutExpired:
            peer.kill()
            peer.wait(timeout=3)
    server.shutdown()
    thread.join()
    server.server_close()
