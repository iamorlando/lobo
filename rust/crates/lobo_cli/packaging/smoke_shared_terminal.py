#!/usr/bin/env python3
"""Measure four simultaneous real terminal streams from one native session."""
import argparse
import fcntl
import os
import pty
import select
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', required=True)
p.add_argument('--renderer', default='cpu', choices=['cpu','gpu','auto'])
p.add_argument('--frames', type=int, default=180)
a = p.parse_args()
binary = str(Path(a.binary).resolve())
with tempfile.TemporaryDirectory(prefix='lobo-terminals-') as directory:
    socket = Path(directory)/'feed.sock'
    server = subprocess.Popen([binary,'session','--socket',str(socket),'--speed','40',
                               '--aggregation','ticks','--bar-size','10'],stdout=subprocess.DEVNULL)
    children = []
    try:
        deadline = time.monotonic()+5
        while not socket.exists() and time.monotonic()<deadline:time.sleep(.01)
        for view in ['candles','book','flow','simulate']:
            master,slave=pty.openpty()
            fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',25,90,0,0))
            before=termios.tcgetattr(slave)
            started=time.monotonic()
            child=subprocess.Popen([binary,view,'--attach',str(socket),'--renderer',a.renderer,
                                    '--fps','60','--frames',str(a.frames)],stdin=slave,stdout=slave,
                                   stderr=slave,close_fds=True,
                                   env={**os.environ,'TERM':'xterm-256color','COLORTERM':'truecolor'})
            children.append({'view':view,'process':child,'master':master,'slave':slave,
                             'before':before,'started':started,'ended':None,'output':bytearray()})
        deadline=time.monotonic()+20
        while any(c['ended'] is None for c in children) and time.monotonic()<deadline:
            ready=select.select([c['master'] for c in children],[],[],.01)[0]
            for c in children:
                if c['master'] in ready:
                    try:c['output'].extend(os.read(c['master'],65536))
                    except OSError:pass
                if c['ended'] is None and c['process'].poll() is not None:c['ended']=time.monotonic()
        for c in children:
            assert c['ended'] is not None, f"{c['view']} stalled"
            assert c['process'].returncode == 0, c['output'].decode(errors='replace')[-1000:]
            assert termios.tcgetattr(c['slave']) == c['before']
            assert b'shared #' in c['output'] and b'\x1b[38;2;' in c['output']
            assert b'\x1b[?1049l' in c['output']
            elapsed=c['ended']-c['started']
            print(f"{c['view']}: {a.frames} real {a.renderer} terminal frames in {elapsed:.3f}s ({a.frames/elapsed:.1f} fps including startup), {len(c['output'])} ANSI bytes, terminal restored")
    finally:
        for c in children:
            if c['process'].poll() is None:c['process'].kill()
            c['process'].wait(timeout=5)
            os.close(c['master']);os.close(c['slave'])
        if server.poll() is None:server.terminate()
        server.wait(timeout=5)
