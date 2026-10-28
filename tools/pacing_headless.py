#!/usr/bin/env python3
"""Native Wayland/RADV walk trace on a private compositor; never uses DISPLAY/uinput.

Every invocation clones the same stopped server world and drives actual MCP input.
Run under mem-run's measurement lane only after Main grants the window.
"""
import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import selectors
import shutil
import signal
import socket
import statistics
import subprocess
import threading
import tempfile

from mcp_client import Client

ROOT = Path(__file__).resolve().parents[1]
WORK = Path('/run/media/kaiserroman/games/solaris-work')


def launch(args, env, log, marker):
    ready = threading.Event()
    found = []
    proc = subprocess.Popen(args, env=env, cwd=ROOT, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, start_new_session=True)
    def pump():
        with log.open('w') as out:
            for line in proc.stdout:
                out.write(line)
                out.flush()
                if marker in line:
                    found.append(line.split(marker, 1)[1].strip())
                    ready.set()
        ready.set()
    threading.Thread(target=pump, daemon=True).start()
    if not ready.wait(240) or not found:
        stop(proc)
        raise RuntimeError(f'No startup event {marker!r}: {log}')
    return proc, found[0]


def stop(proc):
    if proc is not None and proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(20)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()


def main():
    p = argparse.ArgumentParser(__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--world', type=Path, default=ROOT / '.analysis/server/world')
    p.add_argument('--server', type=Path, default=WORK / 'targets/rc1/release/solaris-v2')
    p.add_argument('--prefix', type=Path, default=WORK / 'wt/shader-show/.analysis/cage-prefix')
    p.add_argument('--steps', type=int, default=24)
    a = p.parse_args()
    out = a.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    compositor = server = client = mcp = None
    runtime_dir = tempfile.TemporaryDirectory(prefix='solaris-pacing-')
    report = {'binary': str(a.binary), 'sha256': hashlib.sha256(a.binary.read_bytes()).hexdigest(),
              'native_wayland': True, 'display_scanout_verified': False,
              'route': {'steps': a.steps, 'ticks_per_step': 10, 'yaw_start': 0, 'yaw_step': 6, 'pitch': 60}}
    try:
        # Unix socket paths are limited to 108 bytes; artifact paths are longer.
        runtime = Path(runtime_dir.name)
        fifo = out / 'ready.fifo'
        os.mkfifo(fifo)
        config = out / 'sway.conf'
        config.write_text('output HEADLESS-1 mode 1920x1080@144Hz\n'
                          'default_border none\nfocus_follows_mouse no\nxwayland disable\n'
                          'for_window [all] fullscreen enable\n'
                          f'exec sh -c \'echo $WAYLAND_DISPLAY > {fifo}\'\n')
        env = {k: v for k, v in os.environ.items() if k not in ('DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY')}
        env.update(XDG_RUNTIME_DIR=str(runtime), WLR_BACKENDS='headless',
                   WLR_RENDER_DRM_DEVICE='/dev/dri/renderD128', WLR_RENDERER='gles2',
                   SWAYSOCK=str(runtime / 'sway.sock'),
                   LD_LIBRARY_PATH=str(a.prefix / 'root/usr/lib/x86_64-linux-gnu'))
        with (out / 'sway.log').open('w') as log:
            compositor = subprocess.Popen([str(a.prefix / 'root/usr/bin/sway'), '--unsupported-gpu', '-c', str(config)],
                                          env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        with open(os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)) as pipe, selectors.DefaultSelector() as selector:
            selector.register(pipe, selectors.EVENT_READ)
            wayland = pipe.readline().strip() if selector.select(60) else ''
        if not wayland:
            raise RuntimeError(f'sway did not publish its private socket: {out / "sway.log"}')
        report['wayland_socket'] = str(runtime / wayland)
        shutil.copytree(a.world, out / 'world')
        (out / 'plugins').mkdir()
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        toml = (ROOT / '.analysis/server/server.toml').read_text()
        import re
        toml = re.sub(r'port = \d+', f'port = {port}', toml)
        toml = toml.replace(str(ROOT / '.analysis/server/world'), str(out / 'world'))
        toml = toml.replace(str(ROOT / '.analysis/server/plugins'), str(out / 'plugins'))
        (out / 'server.toml').write_text(toml)
        server, _ = launch([str(a.server), '--config', str(out / 'server.toml')], env,
                           out / 'server.log', 'Solaris is listening')
        settings = out / 'settings.json'
        settings.write_text(json.dumps({'vsync': True, 'max_fps': 120}))
        env.update(WAYLAND_DISPLAY=wayland, VK_ICD_FILENAMES='/usr/share/vulkan/icd.d/radeon_icd.json',
                   SOLARIS_CLIENT_FRAME_TRACE=str(out / 'frames.tsv'), SOLARIS_CLIENT_SETTINGS=str(settings),
                   SOLARIS_CLIENT_ROOT=str(out / 'client-root'),
                   SOLARIS_CLIENT_DATA_DIR='/home/kaiserroman/solaris/data/vanilla',
                   SOLARIS_CLIENT_RESOURCE_ROOT=str(WORK / 'solaris-client/.analysis/resources'),
                   SOLARIS_CLIENT_ASSET_ROOT='/home/kaiserroman/.local/share/PrismLauncher/assets',
                   SOLARIS_CLIENT_ASSET_INDEX=str(WORK / 'solaris-client/.analysis/evidence/C00/asset-index-current.json'))
        client, descriptor = launch([str(a.binary), 'play', '--profile', 'offline-test', '--server', f'127.0.0.1:{port}',
                                    '--username', 'InputProbe', '--automation', '--mcp-http', '127.0.0.1:0',
                                    '--windowed', '--width', '1920', '--height', '1080',
                                    '--runtime-dir', str(out / 'client-runtime'), '--artifact-dir', str(out / 'artifacts')],
                                   env, out / 'client.log', 'automation descriptor: ')
        mcp = Client(Path(descriptor), out / 'mcp')
        report['initialize'] = mcp.initialize()
        report['schemas'] = mcp.request('tools/list')
        report['resources'] = mcp.request('resources/list')
        report['capabilities'] = mcp.request('resources/read', {'uri': 'solaris-client://capabilities'})
        mcp.tool('minecraft_wait_for_play', {'timeout_seconds': 120})
        mcp.tool('minecraft_wait_ticks', {'ticks': 240})
        mcp.tool('minecraft_look', {'yaw_deg': 0, 'pitch_deg': 60})
        mcp.tool('minecraft_wait_ticks', {'ticks': 40})
        report['start'] = mcp.tool('minecraft_observe')
        def frame():
            return int((out / 'frames.tsv').read_text().splitlines()[-1].split('\t')[1])
        report['first_frame'] = frame()
        report['poses'] = []
        for step in range(a.steps):
            yaw = (step * 6 + 180) % 360 - 180
            mcp.tool('minecraft_look', {'yaw_deg': yaw, 'pitch_deg': 60})
            mcp.tool('minecraft_press_inputs', {'keys': ['forward'], 'ticks': 10})
            report['poses'].append(mcp.tool('minecraft_observe')['player'])
        report['last_frame'] = frame()
        report['end'] = mcp.tool('minecraft_observe')
        report['capabilities_end'] = mcp.request('resources/read', {'uri': 'solaris-client://capabilities'})
        rows = list(csv.DictReader((out / 'frames.tsv').open(), delimiter='\t'))
        rows = [r for r in rows if report['first_frame'] < int(r['frame_id']) <= report['last_frame']]
        values = sorted(float(r['submitted_frame_interval_ms']) for r in rows)
        report['interval_ms'] = {k: values[min(len(values)-1, math.ceil(q*len(values))-1)] for k,q in [('p50', .5), ('p95', .95), ('p99', .99)]}
        report['interval_ms'].update(max=max(values), mean=statistics.fmean(values), n=len(values))
        report['mean_fps'] = 1000 / statistics.fmean(values)
        report['slow_frames'] = [r for r in rows if float(r['submitted_frame_interval_ms']) > 50]
        report['status'] = 'completed'
        print(json.dumps({'out': str(out), 'interval_ms': report['interval_ms'], 'mean_fps': report['mean_fps']}), flush=True)
    except BaseException as error:
        report['error'] = repr(error)
        raise
    finally:
        if mcp:
            mcp.close()
        stop(client)
        stop(server)
        stop(compositor)
        runtime_dir.cleanup()
        (out / 'result.json').write_text(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
