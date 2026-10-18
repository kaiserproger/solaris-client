"""Explicit local test driver; controls the real application, never a packet bot."""
from __future__ import annotations
import json
import pathlib
import time
import uuid
import urllib.error
import urllib.request

class Client:
    def __init__(self, descriptor: pathlib.Path, evidence: pathlib.Path):
        self.descriptor = json.loads(descriptor.read_text())
        self.endpoint = self.descriptor['endpoint']
        self.token = pathlib.Path(self.descriptor['token_file']).read_text().strip()
        self.evidence = evidence
        evidence.mkdir(parents=True, exist_ok=True)
        self.trace = (evidence / 'mcp-transcript.jsonl').open('a')
        self.sequence = 0
        self.run_id = uuid.uuid4().hex[:12]
        self.session = None
        self.fencing_token = None

    def request(self, method, params=None, authenticated=True):
        self.sequence += 1
        request = {'jsonrpc':'2.0', 'method':method}
        if not method.startswith('notifications/'):
            request['id'] = self.sequence
        if params is not None:
            request['params'] = params
        headers = {'Content-Type':'application/json', 'Accept':'application/json, text/event-stream',
                   'MCP-Protocol-Version':'2025-11-25'}
        if authenticated:
            headers['Authorization'] = 'Bearer ' + self.token
        if self.session:
            headers['Mcp-Session-Id'] = self.session
        started = time.monotonic_ns()
        try:
            with urllib.request.urlopen(urllib.request.Request(self.endpoint, json.dumps(request).encode(), headers), timeout=130) as response:
                body = response.read()
                self.session = response.headers.get('Mcp-Session-Id', self.session)
                result = json.loads(body) if body else None
                status = response.status
        except urllib.error.HTTPError as error:
            status = error.code
            result = {'http_error':status, 'body':error.read().decode(errors='replace')}
        self.trace.write(json.dumps({'request':request, 'response':result, 'http_status':status,
                                    'elapsed_ns':str(time.monotonic_ns()-started)}) + '\n')
        self.trace.flush()
        return result

    def initialize(self):
        result = self.request('initialize', {'protocolVersion':'2025-11-25', 'capabilities':{},
                              'clientInfo':{'name':'solaris-native-evidence-driver', 'version':'0.1.0'}})
        if not result or 'error' in result:
            raise RuntimeError(result)
        self.request('notifications/initialized')
        return result

    def tool(self, name, arguments=None):
        result = self.request('tools/call', {'name':name, 'arguments':arguments or {}})
        if not result or 'error' in result or 'http_error' in result:
            raise RuntimeError(result)
        payload = result['result']
        if payload.get('isError'):
            raise RuntimeError(payload)
        if 'structuredContent' in payload:
            return payload['structuredContent']
        texts = [part['text'] for part in payload.get('content', []) if part.get('type') == 'text']
        return json.loads(texts[0]) if texts else payload

    def observe(self):
        return self.tool('client_observe')

    def acquire(self, controller='native-acceptance', ttl_ms=30000):
        result = self.tool('controller_acquire', {'controller':controller, 'ttl_ms':ttl_ms})
        self.fencing_token = result['fencing_token']
        return result

    def action(self, tool_name, **arguments):
        observation = self.observe()
        arguments.update(action_id=f'proof-{self.run_id}-{self.sequence+1}', instance_id=observation['instance_id'],
                         connection_epoch=observation['connection_epoch'], fencing_token=self.fencing_token)
        result = self.tool(tool_name, arguments)
        if 'action_id' in result:
            result = self.tool('client_wait_action', {'action_id':result['action_id'], 'timeout_ms':30000})
        if result.get('phase') != 'client_applied':
            raise RuntimeError(f'owner did not apply action: {result}')
        return result

    def wait(self, path, value=None, op='eq', timeout_ms=30000):
        observation = self.observe()
        predicate = {'op':op, 'path':path}
        if op != 'exists':
            predicate['value'] = value
        result = self.tool('client_wait', {'instance_id':observation['instance_id'],
                           'connection_epoch':observation['connection_epoch'], 'timeout_ms':timeout_ms,
                           'predicate':predicate})
        if result.get('status') != 'matched':
            raise RuntimeError(f'event predicate did not match: {result}')
        return result

    def release(self):
        if self.fencing_token is not None:
            self.tool('controller_release', {'fencing_token':self.fencing_token})
            self.fencing_token = None

    def close(self):
        self.release()
        self.trace.close()
