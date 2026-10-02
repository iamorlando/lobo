#!/usr/bin/env python3
"""Verify the shipped executable against raw, gzip, HTTP and corrupt ITCH."""
import argparse
import gzip
import http.server
import json
import struct
import subprocess
import tempfile
import threading
from functools import partial
from pathlib import Path

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary',required=True)
a=p.parse_args()
def record(kind,ident,side,quantity,price,ns):
    body=kind+b'\x00\x01\x00\x00'+ns.to_bytes(8,'big')[2:]+ident.to_bytes(8,'big')
    body+=(side+struct.pack('>I',quantity)+b'AAPL    '+struct.pack('>I',price)) if kind==b'A' else struct.pack('>IQ',quantity,42)
    return struct.pack('>H',len(body))+body
wire=record(b'A',1,b'B',500,990000,0)+record(b'A',2,b'S',500,1010000,0)+record(b'E',2,b'',250,0,1000000000)
base=[str(Path(a.binary).resolve()),'candles','--renderer','cpu','--snapshot-json','--capture-seconds','0.15','--start-at','00:00:00','--speed','1000','--bar-size','100']
class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self,*args): pass
with tempfile.TemporaryDirectory() as d:
    root=Path(d)
    (root/'session').write_bytes(wire)
    # Deliberately no .gz suffix: decoding must inspect magic bytes.
    (root/'compressed').write_bytes(gzip.compress(wire))
    (root/'bad').write_bytes(wire[:-1])
    for filename in ['session','compressed']:
        state=json.loads(subprocess.check_output(base+['--file',str(root/filename)],text=True))
        assert state['complete'] and state['messages']==3,state
        assert [bar['volume'] for bar in state['candles']]==[100,100,50],state['candles']
        assert state['bid']==99 and state['ask']==101
    (root/'watermark').write_bytes(wire+record(b'A',3,b'B',10,980000,2000000000))
    timed=json.loads(subprocess.check_output(base[:-2]+['--file',str(root/'watermark'),'--aggregation','time','--bar-size','1'],text=True))
    assert len(timed['candles'])==1 and not timed['candles'][0]['forming'],timed['candles']
    failed=subprocess.run(base+['--file',str(root/'bad')],capture_output=True,text=True)
    assert failed.returncode!=0 and 'Truncated' in failed.stderr,failed
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),partial(Quiet,directory=d))
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    try:
        state=json.loads(subprocess.check_output(base+['--url',f'http://127.0.0.1:{server.server_port}/compressed'],text=True))
        assert state['complete'] and len(state['candles'])==3,state
    finally: server.shutdown();server.server_close();thread.join()
print('Source smoke passed: raw/gzip/magic-byte detection/HTTP/native volume splits/truncated input rejection')
