"""Real endpoint acceptance smoke. Native desktop resize/focus proof is separate."""
from __future__ import annotations
import argparse
import hashlib
import json
import struct
from pathlib import Path
from mcp_client import Client


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--descriptor', type=Path, required=True)
    parser.add_argument('--artifact-dir', type=Path, required=True)
    args = parser.parse_args()
    client = Client(args.descriptor, args.artifact_dir)
    assertions = []
    try:
        denied = client.request('tools/list', {}, authenticated=False)
        assert denied.get('http_error') in (401, 403), denied
        assertions.append({'id':'unauthorized-discovery', 'passed':True})
        initialized = client.initialize()
        assert initialized['result']['protocolVersion'] == '2025-11-25', initialized
        catalog = client.request('tools/list', {})
        names = {entry['name'] for entry in catalog['result']['tools']}
        assert {'client_observe','client_input','client_capture','client_wait','controller_acquire'} <= names, names
        assertions.append({'id':'authenticated-discovery', 'passed':True, 'tools':sorted(names)})
        baseline = client.observe()
        (args.artifact_dir / 'baseline.json').write_text(json.dumps(baseline, indent=2)+'\n')
        lease = client.acquire()
        try:
            client.tool('controller_acquire', {'controller':'second-writer', 'ttl_ms':30000})
            raise AssertionError('second writer admitted')
        except RuntimeError as error:
            assertions.append({'id':'exclusive-controller', 'passed':True, 'rejection':str(error)})
        action = client.action('client_input', intent={'kind':'look', 'yaw':35.0, 'pitch':15.0})
        assertions.append({'id':'ordinary-input-owner-result', 'result':action})
        frame = client.action('client_capture', name='c01-actual-gpu')
        receipt = frame['result']
        assert receipt['gpu_completion'] is True and receipt['offscreen_only'] is False, receipt
        assert receipt['render_target'] == 'actual_window_scene_target', receipt
        assert receipt['yaw'] == 35.0 and receipt['pitch'] == 15.0, receipt
        image = args.artifact_dir / receipt['file_name']
        raw = image.read_bytes()
        assert raw[:8] == b'\x89PNG\r\n\x1a\n', image
        width, height = struct.unpack('>II', raw[16:24])
        assert (width, height) == (receipt['width'], receipt['height']), receipt
        assertions.append({'id':'actual-gpu-capture', 'result':frame, 'path':str(image),
                           'sha256':hashlib.sha256(raw).hexdigest(), 'width':width, 'height':height})
        client.action('client_input', intent={'kind':'release_all'})
        client.release()
        after = client.observe()
        (args.artifact_dir / 'after.json').write_text(json.dumps(after, indent=2)+'\n')
        result = {'status':'partial', 'scope':'C01 authenticated MCP, input, controller and GPU capture; native resize/focus/stdio require their own proof',
                  'instance_id':baseline['instance_id'], 'protocol':'2025-11-25', 'assertions':assertions,
                  'locks':{name:hashlib.sha256((Path(__file__).resolve().parents[1]/name).read_bytes()).hexdigest()
                           for name in ['Cargo.lock','compatibility.lock']}}
    except Exception as error:
        result = {'status':'failed','assertions':assertions,'error':str(error)}
        raise
    finally:
        client.close()
        if 'result' in locals():
            (args.artifact_dir/'mcp-smoke.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps(result))

if __name__ == '__main__':
    main()
