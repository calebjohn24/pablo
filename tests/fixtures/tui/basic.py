"""T01 composer and native streaming smoke in a real local Unix PTY."""
import os,sys,pty,fcntl,termios,struct,subprocess,select,time,json,threading,http.server,tempfile
binary=os.path.abspath(sys.argv[1])
automatic=len(sys.argv)>2 and sys.argv[2]=="auto"
count=int(sys.argv[2]) if len(sys.argv)>2 and not automatic else 2
assert 2<=count<=40
samples=[]
requests=[]
release=threading.Event()
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def do_POST(self):
        request=json.loads(self.rfile.read(int(self.headers['Content-Length'])));requests.append(request)
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers()
        content='ready-from-provider\x1b]52;c;PRIVATE_CLIPBOARD\x07\n'
        for delta,finish in [({'content':content},None),({},'stop')]:
            if finish and request['messages'][-1]['content']=='cancel task':release.wait(5)
            try:self.wfile.write(('data: '+json.dumps({'choices':[{'index':0,'delta':delta,'finish_reason':finish}]})+'\n\n').encode());self.wfile.flush()
            except (BrokenPipeError,ConnectionResetError):return
        try:self.wfile.write(b'data: [DONE]\n\n');self.wfile.flush()
        except (BrokenPipeError,ConnectionResetError):pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
thread=threading.Thread(target=server.serve_forever);thread.start()
master,slave=pty.openpty();saved=termios.tcgetattr(slave)
fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',24,100,0,0))
def control():os.setsid();fcntl.ioctl(0,termios.TIOCSCTTY,0)
child=None
try:
    with tempfile.TemporaryDirectory(prefix='pablo-tui-basic-') as cwd:
        wrapper="import subprocess,sys; code=subprocess.call(sys.argv[1:]); print('\\nFIXTURE_EXIT='+str(code),flush=True); sys.stdin.readline(); print('FIXTURE_READY',flush=True); sys.stdin.readline(); sys.exit(code)"
        child=subprocess.Popen([sys.executable,'-c',wrapper,binary]+([] if automatic else ['tui','--no-shell','--no-filesystem']),stdin=slave,stdout=slave,stderr=slave,cwd=cwd,env={'TERM':'xterm-256color','PABLO_FIXTURE_ENDPOINT':f'http://127.0.0.1:{server.server_port}'},preexec_fn=control)
        output=bytearray()
        def wait_for(predicate):
            end=time.monotonic()+8
            while not predicate():
                assert child.poll() is None,bytes(output[-2000:])
                assert time.monotonic()<end,bytes(output[-2000:])
                if select.select([master],[],[],0.05)[0]:output.extend(os.read(master,65536))
                assert len(output)<1024*1024
        wait_for(lambda:b'Enter: new independent task' in output)
        started=time.monotonic();os.write(master,b'firt\x1b[Ds\r')
        def screen():return bytes(output).split(b'\x1b[H\x1b[2J')[-1]
        wait_for(lambda:len(requests)==1 and b'completed' in screen() and b'ready-from-provider' in screen() and b'You: first' in screen())
        samples.append((time.monotonic()-started)*1000)
        assert b'\x1b]52;' not in output
        assert requests[0]['messages'][-1]['content']=='first'
        assert b'vercel / zai/glm-5.3-flash' in output
        assert b'Static policy' in output and b'Calls 1 / tools 0' in output
        for index in range(1,count):
            output.clear();started=time.monotonic();os.write(master,f'task-{index}\r'.encode())
            wait_for(lambda:len(requests)==index+1 and b'completed' in screen() and f'You: task-{index}'.encode() in screen())
            samples.append((time.monotonic()-started)*1000)
            assert 'ready-from-provider' not in json.dumps(requests[-1])
            assert requests[-1]['messages'][-1]['content']==f'task-{index}'
        output.clear();os.write(master,b'cancel task\r')
        wait_for(lambda:len(requests)==count+1 and b'ready-from-provider' in screen() and b'You: cancel task' in screen())
        os.write(master,b'\x03');wait_for(lambda:b'cancelled' in screen());release.set()
        os.write(master,b'\x04')
        wait_for(lambda:b'FIXTURE_EXIT=130' in output)
        # A canonical read clears macOS's kernel-owned PENDIN retype state.
        os.write(master,b'\n');wait_for(lambda:b'FIXTURE_READY' in output)
        actual=termios.tcgetattr(slave)
        assert actual==saved,('terminal modes not restored',saved,actual)
        os.write(master,b'\n')
        deadline=time.monotonic()+5
        while child.poll() is None:
            assert time.monotonic()<deadline,'PTY owner exit timed out'
            if select.select([master],[],[],0.05)[0]:
                try:os.read(master,65536)
                except OSError:pass
        child.wait(timeout=1);assert child.returncode==130
        proof={'surface':'tui','tasks':count+1,'cancelledThroughRunToken':True,'freshTaskContext':True,'streamed':True,'oscNeutralized':True,'terminalRestored':True}
        if len(sys.argv)>2 and not automatic:proof['submit_to_completed_screen_ms']=samples
        print(json.dumps(proof))
except BaseException:
    import traceback;traceback.print_exc();raise
finally:
    release.set()
    os.close(master)
    if child is not None and child.poll() is None:child.kill();child.wait(timeout=5)
    os.close(slave);server.shutdown();thread.join();server.server_close()
