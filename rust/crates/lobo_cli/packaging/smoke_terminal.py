#!/usr/bin/env python3
"""Exercise the real TUI in a PTY and verify terminal restoration."""
import argparse
import fcntl
import os
import pty
import select
import struct
import subprocess
import termios
import time
from pathlib import Path

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', required=True)
p.add_argument('--renderer', default='cpu', choices=['cpu','gpu','auto'])
p.add_argument('--attach')
a=p.parse_args()
master,slave=pty.openpty()
fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',40,140,0,0))
before=termios.tcgetattr(slave)
command=[a.binary,'--renderer',a.renderer,'--aggregation','ticks','--bar-size','5','--speed','20']
if a.attach: command += ['--attach',a.attach]
child=subprocess.Popen(command,stdin=slave,stdout=slave,stderr=slave,close_fds=True,env={**os.environ,'TERM':'xterm-256color','COLORTERM':'truecolor'})
output=bytearray()
def drain(seconds):
    until=time.monotonic()+seconds
    while time.monotonic()<until:
        if select.select([master],[],[],0.02)[0]:
            try: output.extend(os.read(master,65536))
            except OSError: break
try:
    drain(0.6)
    if child.poll() is not None: raise RuntimeError(output.decode(errors='replace'))
    for keys in [b'?',b'\x1b',b'2',b':aggregation ticks 5\r',b'3',b':sim buy market 100\r',b'm',b'6',b':queue buy 99.99\r',b' ']:
        os.write(master,keys);drain(0.08)
    fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',18,64,0,0))
    import signal
    child.send_signal(signal.SIGWINCH)
    drain(0.15)
    os.write(master,b'q');drain(0.2)
    deadline=time.monotonic()+8
    while child.poll() is None and time.monotonic()<deadline: drain(0.05)
    if child.poll() is None:
        Path('/tmp/lobo-pty-failure.txt').write_bytes(output)
    status=child.wait(timeout=0.1)
    after=termios.tcgetattr(slave)
    assert status==0,output.decode(errors='replace')[-3000:]
    assert b'LOBO' in output and b'OHLC' in output and b'FIFO' in output
    assert b'lobo completions api' in output, 'in-app help omits agent discovery'
    assert b'\x1b[38;2;' in output and b'48;2;' in output, 'chart color data was suppressed'
    assert b'\x1b[?1049h' in output and b'\x1b[?1049l' in output
    assert before==after,'terminal attributes were not restored'
    print(f'PTY passed: {a.renderer}, views/commands/resize, clean exit, terminal attributes restored; {len(output)} output bytes')
finally:
    if child.poll() is None: child.kill();child.wait()
    os.close(master);os.close(slave)
