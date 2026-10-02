#!/usr/bin/env python3
"""Run real shared chart panes on an isolated tmux server."""
import argparse
import os
import re
import shlex
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--binary', required=True)
p.add_argument('--renderer', default='cpu', choices=['cpu', 'gpu', 'auto'])
p.add_argument('--output', type=Path)
a = p.parse_args()
binary = str(Path(a.binary).resolve())
tmux = shutil.which('tmux')
if not tmux:
    raise SystemExit('tmux is required for this multipane verification')
root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='lobo-mux-') as directory:
    directory = Path(directory)
    label = f'lobo-verify-{os.getpid()}'
    socket = directory / 'feed.sock'
    wrapper = directory / 'tmux'
    wrapper.write_text(f'#!/bin/sh\nexec {shlex.quote(tmux)} -L {shlex.quote(label)} -f /dev/null "$@"\n')
    wrapper.chmod(0o755)
    env = {**os.environ, 'PATH': f'{directory}:{os.environ["PATH"]}',
           'LOBO_BIN': binary, 'LOBO_SOCKET': str(socket), 'LOBO_TMUX_SESSION': 'verify',
           'LOBO_RENDERER': a.renderer, 'LOBO_DETACHED': '1', 'TERM': 'xterm-256color'}
    server = subprocess.Popen([binary, 'session', '--socket', str(socket), '--speed', '40',
                               '--aggregation', 'ticks', '--bar-size', '10', '--capacity', '64'],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    def run(*args):
        return subprocess.check_output([str(wrapper), *args], text=True)
    try:
        deadline = time.monotonic() + 5
        while not socket.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        subprocess.run(['bash', str(root / 'cli/examples/tmux-dashboard.sh')], env=env, check=True)
        time.sleep(1.5)
        panes = run('list-panes', '-t', '=verify', '-F', '#{pane_id} #{pane_left} #{pane_top} #{pane_width} #{pane_height}').splitlines()
        panes.sort(key=lambda p: (int(p.split()[2]), int(p.split()[1])))
        assert len(panes) == 4
        ids = [p.split()[0] for p in panes]
        # A simulation entered in one actual TUI must be visible in another pane.
        run('send-keys', '-t', ids[0], ':sim buy market 100', 'Enter', '2')
        time.sleep(.15)
        result = subprocess.run([binary, 'ctl', '--attach', str(socket), '--execute', 'pause'], capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
        time.sleep(.2)
        captures = [run('capture-pane', '-p', '-t', pane) for pane in ids]
        for capture in captures:
            assert 'LOBO' in capture and 'shared #' in capture and 'PAUSED' in capture, capture
            assert ('Metal' in capture or 'Vulkan' in capture or 'Dx12' in capture) if a.renderer == 'gpu' else True
        clocks = [re.search(r'PAUSED\s+[^\s]+\s+(\d\d:\d\d:\d\d)', text)[1] for text in captures]
        assert len(set(clocks)) == 1, clocks
        assert 'OHLC' in captures[0] and 'DEPTH' in captures[1]
        assert 'ORDER IN / OUT' in captures[2] and 'SIMULATED FILLS' in captures[3]
        assert '100' in captures[3], 'simulation did not reach the simulation pane'
        colored = [run('capture-pane', '-e', '-p', '-t', pane) for pane in ids]
        assert all('38;2;' in text and '48;2;' in text for text in colored), 'tmux lost chart color data'
        if a.output:
            a.output.mkdir(parents=True, exist_ok=True)
            for index, pane in enumerate(ids):
                (a.output / f'pane-{index}.ansi').write_text(colored[index])
            (a.output / 'panes.txt').write_text('\n'.join(panes))
        # Quit one pane; the remaining charts and feed must remain alive.
        run('send-keys', '-t', ids[2], 'q')
        time.sleep(.15)
        assert len(run('list-panes', '-t', '=verify').splitlines()) == 3
        assert server.poll() is None
        print(f'tmux passed: four {a.renderer} panes, shared clock {clocks[0]}, simulation propagated between panes, closing one pane leaves the others running')
    finally:
        subprocess.run([str(wrapper), 'kill-server'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if server.poll() is None:
            server.terminate()
        server.wait(timeout=5)
