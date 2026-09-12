"""Real PTY regression: stable frames, durable tools/history, Markdown and scrolling."""
import os,sys,pty,fcntl,termios,struct,subprocess,select,time,json,threading,http.server,tempfile
from screen import Screen
binary=os.path.abspath(sys.argv[1]); release=threading.Event(); requests=[]
def wire(delta,finish=None): return ('data: '+json.dumps({'choices':[{'index':0,'delta':delta,'finish_reason':finish}]})+'\n\n').encode()
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_POST(self):
        request=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append(request)
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
        user=next(m['content'] for m in request['messages'] if m['role']=='user')
        try:
            if user=='first task' and request['messages'][-1]['role']!='tool':
                self.wfile.write(wire({'tool_calls':[{'index':0,'id':'quick-tool','type':'function','function':{'name':'shell_run','arguments':json.dumps({'command':'printf fixture-tool','cwd':'.'})}}]},'tool_calls'))
            else:
                text=('# Rendered heading\n\n**bold label** with `inline_code`\n\n- bullet item\n> quoted text\n```rust\nlet answer = 42;\n```\n[reference](https://example.test)\n'+''.join(f'history-line-{i:02}\n' for i in range(40))+'first answer complete') if user=='first task' else 'second answer complete'
                self.wfile.write(wire({'content':text}));self.wfile.flush()
                if user=='first task':release.wait(8)
                self.wfile.write(wire({},'stop'))
            self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
        except (BrokenPipeError,ConnectionResetError):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler);thread=threading.Thread(target=server.serve_forever);thread.start()
master,slave=pty.openpty();saved=termios.tcgetattr(slave)
fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,100,0,0))
def control():os.setsid();fcntl.ioctl(0,termios.TIOCSCTTY,0)
child=None
try:
    with tempfile.TemporaryDirectory(prefix='pablo-tui-display-') as cwd:
        wrapper="import subprocess,sys; code=subprocess.call(sys.argv[1:]); print('\\nFIXTURE_EXIT='+str(code),flush=True); sys.stdin.readline(); print('FIXTURE_READY',flush=True); sys.stdin.readline(); sys.exit(code)"
        child=subprocess.Popen([sys.executable,'-c',wrapper,binary,'tui','--no-filesystem'],stdin=slave,stdout=slave,stderr=slave,cwd=cwd,env={'TERM':'xterm-256color','PABLO_FIXTURE_ENDPOINT':f'http://127.0.0.1:{server.server_port}'},preexec_fn=control)
        output=bytearray();display=Screen(40,100)
        def drain(timeout=.05):
            if select.select([master],[],[],timeout)[0]:
                data=os.read(master,65536);output.extend(data);display.feed(data)
                assert len(output)<1024*1024
        def wait_for(predicate):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None and time.monotonic()<end,display.text()[-3500:]
                drain()
        def idle_bytes():
            end=time.monotonic()+.2
            while time.monotonic()<end:drain(.02)
            before=len(output);end=time.monotonic()+.4
            while time.monotonic()<end:drain(.02)
            return len(output)-before
        wait_for(lambda:b'Enter: new independent task' in display.text())
        assert idle_bytes()==0,'idle renderer emits repeated output'
        os.write(master,b'first task\r')
        wait_for(lambda:len(requests)==2 and b'first answer complete' in display.text())
        os.write(master,b'\x1b[1;5H')
        wait_for(lambda:b'You: first task' in display.text() and b'Rendered heading' in display.text())
        text=display.text();assert b'Tool: shell.run' in text and b'running' in text and b'Completed' in text,text
        assert b'**bold label**' not in text and b'inline_code' in text and b'`inline_code`' not in text
        assert '• bullet item'.encode() in text and b'let answer = 42;' in text
        heading=next(i for i,row in enumerate(display.cells) if 'Rendered heading' in ''.join(row));assert 1 in display.styles[heading][0]
        code=next(i for i,row in enumerate(display.cells) if 'let answer = 42;' in ''.join(row));assert 36 in display.styles[code][2]
        release.set();wait_for(lambda:b'pablo | completed' in display.text())
        os.write(master,b'second task\r')
        wait_for(lambda:len(requests)==3 and b'You: second task' in display.text() and b'second answer complete' in display.text() and b'pablo | completed' in display.text())
        assert 'first task' not in json.dumps(requests[-1]) and 'Rendered heading' not in json.dumps(requests[-1])
        os.write(master,b'\x1b[1;5H');wait_for(lambda:b'You: first task' in display.text() and b'Tool: shell.run' in display.text())
        os.write(master,b'\x1b[1;5F');wait_for(lambda:b'You: second task' in display.text())
        before=display.text();os.write(master,b'\x1b[<64;10;10M');wait_for(lambda:display.text()!=before)
        os.write(master,b'\x1b[1;5F');wait_for(lambda:b'second answer complete' in display.text())
        assert idle_bytes()==0,'settled renderer emits repeated output'
        assert output.count(b'\x1b[2J')==1,'streaming or navigation clears the entire screen'
        assert output.count(b'\x1b[?2026h')==output.count(b'\x1b[?2026l')
        os.write(master,b'\x04');wait_for(lambda:b'FIXTURE_EXIT=0' in output)
        assert b'\x1b[?1000l\x1b[?1006l' in output
        os.write(master,b'\n');wait_for(lambda:b'FIXTURE_READY' in output);assert termios.tcgetattr(slave)==saved
        os.write(master,b'\n');deadline=time.monotonic()+5
        while child.poll() is None:
            assert time.monotonic()<deadline,'PTY owner exit timed out'
            if select.select([master],[],[],.05)[0]:
                try:os.read(master,65536)
                except OSError:pass
        child.wait(timeout=1);assert child.returncode==0
        print(json.dumps(dict(idleBytes=0,fullScreenClears=1,toolsPersist=True,historyPersists=True,markdownStyled=True,scrollDuringRun=True,mouseScroll=True,freshTaskContext=True,terminalRestored=True)))
finally:
    release.set()
    os.close(master)
    try:
        if child is not None and child.poll() is None:child.kill();child.wait(timeout=5)
    finally:
        os.close(slave);server.shutdown();thread.join();server.server_close()
