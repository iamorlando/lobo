#!/usr/bin/env python3
"""Verify real shared-session fanout, commands, slow panes and cleanup."""
import argparse
import copy
import json
import os
import socket
import struct
import subprocess
import tempfile
import threading
import time
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', required=True)
a = p.parse_args()
binary = str(Path(a.binary).resolve())


def receive(stream):
    def exact(size):
        data = bytearray()
        while len(data) < size:
            chunk = stream.recv(size - len(data))
            if not chunk:
                raise EOFError()
            data.extend(chunk)
        return data
    size, = struct.unpack('>I', exact(4))
    assert size <= 64 * 1024 * 1024
    return json.loads(exact(size))


class Pane:
    def __init__(self, path):
        self.stream = socket.socket(socket.AF_UNIX)
        self.stream.connect(str(path))
        self.stream.settimeout(5)
        self.frames = {}
        self.acks = {}
        self.lock = threading.Lock()
        self.error = None
        self.id = 0
        self.worker = threading.Thread(target=self.read, daemon=True)
        self.worker.start()

    def read(self):
        depth = []
        try:
            while True:
                reply = receive(self.stream)
                with self.lock:
                    if 'Frame' in reply:
                        frame = reply['Frame']
                        assert frame['protocol'] == 1
                        if frame['reset_depth']:
                            depth = []
                        depth.extend(frame.pop('depth'))
                        depth = depth[-frame['config']['capacity']:]
                        frame['snapshot']['depth'] = copy.deepcopy(depth)
                        self.frames[frame['sequence']] = frame
                        while len(self.frames) > 120:
                            del self.frames[min(self.frames)]
                    else:
                        ack = reply['Ack']
                        self.acks[ack['id']] = ack['result']
        except (EOFError, OSError):
            pass
        except Exception as error:
            self.error = error

    def latest(self):
        with self.lock:
            return self.frames[max(self.frames)] if self.frames else None

    def command(self, text, failure=False):
        self.id += 1
        packet = json.dumps({'id': self.id, 'action': {'Command': text}}).encode()
        self.stream.sendall(struct.pack('>I', len(packet)) + packet)
        wait(lambda: self.id in self.acks)
        result = self.acks[self.id]
        assert ('Err' in result) == failure, (text, result)
        return result

    def close(self):
        self.stream.close()


def wait(condition, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.005)
    raise AssertionError('shared session condition timed out')


with tempfile.TemporaryDirectory(prefix='lobo-shared-') as directory:
    path = Path(directory) / 'feed.sock'
    server = subprocess.Popen([binary, 'session', '--socket', str(path), '--speed', '20',
                               '--scope', 'AAPL,MSFT', '--aggregation', 'ticks', '--bar-size', '5',
                               '--capacity', '32', '--history-seconds', '2'],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    panes = []
    slow = None
    try:
        wait(path.exists)
        assert path.stat().st_mode & 0o777 == 0o600
        panes = [Pane(path) for _ in range(4)]
        wait(lambda: all(p.latest() for p in panes))
        wait(lambda: all(p.latest()['snapshot']['messages'] >= 30 for p in panes))
        # Compare complete reconstructed state at the SAME published sequence.
        def common():
            with_locks = [set(p.frames) for p in panes]
            return set.intersection(*with_locks)
        wait(lambda: len(common()) >= 3)
        for seq in sorted(common())[-3:]:
            expected = panes[0].frames[seq]
            assert all(p.frames[seq] == expected for p in panes), 'panes received different native state'
        panes[0].command('pause')
        wait(lambda: all(p.latest()['config']['paused'] for p in panes))
        frozen = panes[0].latest()['snapshot']['source_clock_ns']
        time.sleep(0.12)
        assert all(p.latest()['snapshot']['source_clock_ns'] == frozen for p in panes)
        panes[1].command('sim buy limit 100 99.98')
        wait(lambda: all(p.latest()['snapshot']['simulation'] for p in panes))
        assert all(p.latest()['snapshot']['simulation']['alternate'] for p in panes)
        assert len({p.latest()['snapshot']['clock_ns'] for p in panes}) == 1
        assert all(p.latest()['snapshot']['orders_ahead'] is not None for p in panes)
        panes[2].command('main')
        wait(lambda: all(p.latest()['snapshot']['simulation'] is None for p in panes))
        panes[3].command('symbol MSFT')
        wait(lambda: all(p.latest()['snapshot']['symbol'] == 'MSFT' for p in panes))
        panes[0].command('aggregation time 1')
        wait(lambda: all(p.latest()['config']['aggregation'] == 'Time' for p in panes))
        panes[0].command('queue buy 99.001', failure=True)
        panes[1].command('add buy 95 42')
        panes[1].command('queue buy 95')
        wait(lambda: all(any(o['quantity'] == 42 for o in p.latest()['snapshot']['queue']) for p in panes))
        panes[1].command('cancel 1 12')
        wait(lambda: all(any(o['quantity'] == 30 for o in p.latest()['snapshot']['queue']) for p in panes))
        panes[0].command('play')
        slow = socket.socket(socket.AF_UNIX)
        slow.connect(str(path))  # Deliberately never read its frames.
        before = panes[0].latest()['sequence']
        time.sleep(0.5)
        after = panes[0].latest()['sequence']
        assert after - before >= 12, (before, after)
        panes[0].command('pause')
        wait(lambda: all(p.latest()['config']['paused'] for p in panes))
        # The executable attach path must use this state instead of opening a demo.
        captures = [subprocess.Popen([binary, view, '--attach', str(path), '--renderer', 'cpu',
                                      '--snapshot-json', '--capture-seconds', '.1'], stdout=subprocess.PIPE)
                    for view in ['candles', 'book', 'flow', 'orders']]
        snapshots = [json.loads(p.communicate(timeout=5)[0]) for p in captures]
        assert all(p.returncode == 0 for p in captures)
        for snapshot in snapshots:
            assert snapshot['session_sequence'] is not None
            snapshot.pop('session_sequence')
        assert all(s == snapshots[0] for s in snapshots), 'CLI attach state differs between chart views'
        assert snapshots[0]['symbol'] == 'MSFT'
        panes[0].command('restart')
        wait(lambda: all(p.latest()['snapshot']['messages'] == 0 for p in panes))
        assert all(not p.latest()['snapshot']['candles'] for p in panes)
        assert all(p.error is None for p in panes)
        panes[0].command('stop')
        assert server.wait(timeout=5) == 0, server.stderr.read()
        assert not path.exists(), 'session socket not cleaned up'
        # A normal file may never be replaced when binding a session socket.
        path.write_text('user data')
        refused = subprocess.run([binary, 'session', '--socket', str(path)], capture_output=True, text=True)
        assert refused.returncode != 0 and path.read_text() == 'user data'
        path.unlink()
        # Ctrl-C / SIGTERM also clean up the socket.
        server = subprocess.Popen([binary, 'session', '--socket', str(path)], stdout=subprocess.DEVNULL)
        wait(path.exists)
        time.sleep(.05)
        server.terminate()
        assert server.wait(timeout=5) == 0
        assert not path.exists()
        print(f'Shared session passed: four identical streams, shared controls/simulation/FIFO, slow-pane isolation, executable attachments, cleanup; {after-before} frames published in 0.5s')
    finally:
        if slow:
            slow.close()
        for pane in panes:
            pane.close()
        if server.poll() is None:
            server.terminate()
            server.wait(timeout=5)
