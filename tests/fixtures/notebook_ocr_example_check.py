"""Actual example/engine/HTTP pipeline; authored pixels/text, dummy keys only."""
import argparse, base64, hashlib, http.server, json, os, pathlib, struct, subprocess, tempfile, threading, time, zlib
from research_example_check import Server

def png():
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b'\0' + b'\x7f' * 6 + b'\0' + b'\x7f' * 6)) + chunk(b'IEND', b'')

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_CONNECT(self):
        self.server.external_attempts += 1
        self.send_error(502, 'external forwarding denied')
    def do_POST(self):
        self.connection.settimeout(3)
        assert self.path == '/v1/chat/completions'
        assert self.headers['Authorization'] == 'Bearer PRIVATE_FIXTURE_KEY'
        size = int(self.headers['Content-Length']); assert 0 < size <= 65536
        body = json.loads(self.rfile.read(size)); self.server.requests.append(body)
        assert body['model'] == 'loopback-notebook-model' and body['max_completion_tokens'] == 4000
        assert len(body['messages']) == 1 and body['messages'][0]['role'] == 'user'
        parts = body['messages'][0]['content']; assert len(parts) == 6
        assert 'untrusted data' in parts[0]['text']
        assert json.loads(parts[1]['text']) == {'protocol':1, 'pages':[9,2]}
        descriptor = parts[:2].copy()
        for offset, page in [(2,9),(4,2)]:
            assert parts[offset] == {'type':'text','text':f'Page {page} PNG follows'}
            encoded = parts[offset+1]['image_url']
            assert encoded['detail'] == 'high' and encoded['url'].startswith('data:image/png;base64,')
            raw = encoded['url'].split(',',1)[1]
            assert base64.b64decode(raw,validate=True) == png()
            descriptor += [parts[offset], {'type':'image_url','image_url':raw}]
        self.server.descriptor = descriptor
        assert 'Mattie' not in json.dumps(parts) and 'Junior' not in json.dumps(parts)
        reply = json.dumps({'pages':[{'page':9,'status':'transcribed','text':'Mattie Walden studies at Hampton.'},{'page':2,'status':'transcribed','text':'Second year in Junior class. café'}]})
        if self.server.mode == 'invalid-output': reply = 'malformed private output'
        if self.server.mode == 'unreadable': reply = json.dumps({'pages':[{'page':9,'status':'transcribed','text':'Mattie'},{'page':2,'status':'unreadable','text':''}]})
        self.server.returned = reply
        if self.server.mode == 'cancel': time.sleep(0.4)
        status = 503 if self.server.mode == 'provider-error' else 200
        raw = json.dumps({'choices':[{'message':{'content':reply}}]}).encode() if status == 200 else b'PRIVATE_PROVIDER_BODY'
        try:
            self.send_response(status); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(raw))); self.end_headers(); self.wfile.write(raw)
        except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):
            assert self.server.mode == 'cancel'

def case(binary, mode):
    with tempfile.TemporaryDirectory(prefix='buddy-notebook-http-') as folder:
        root = pathlib.Path(folder); output = root/'private.json'; image = root/'synthetic.png'; image.write_bytes(png())
        server = Server(('127.0.0.1',0),Handler)
        server.requests,server.failures,server.external_attempts,server.mode = [],[],0,mode
        worker = threading.Thread(target=lambda:server.serve_forever(poll_interval=0.01)); worker.start()
        try:
            args = [str(binary),'--output',str(output),'--image',str(image),'--page','9','--image',str(image),'--page','2','--query','Mattie Walden','--query','Junior class','--base-url',f'http://127.0.0.1:{server.server_port}','--model','loopback-notebook-model']
            if mode != 'no-live': args.append('--live')
            if mode == 'existing-output': output.write_text('preserve original',encoding='utf-8')
            if mode == 'configured-provider':
                pos=args.index('--base-url');del args[pos:pos+2]
            if mode == 'invalid-base': args[args.index('--base-url')+1] += '/v1/responses'
            if mode == 'input-budget': args += ['--max-input-bytes','1']
            if mode == 'output-budget': args += ['--max-page-text-bytes','1']
            if mode == 'cancel': args += ['--cancel-after-ms','150']
            env = {k:os.environ[k] for k in ('SYSTEMROOT','WINDIR','PATH') if k in os.environ}
            env.update(TMP=folder,TEMP=folder,HTTPS_PROXY=f'http://127.0.0.1:{server.server_port}',NO_PROXY='127.0.0.1')
            if mode != 'missing-key': env['OPENAI_API_KEY']='PRIVATE_FIXTURE_KEY'
            if mode in ('configured-provider','explicit-override'): env['OPENAI_BASE_URL']='https://other-provider.invalid'
            result=subprocess.run(args,cwd=root,env=env,capture_output=True,timeout=20)
        finally:
            server.shutdown();server.server_close();worker.join(timeout=5);assert not worker.is_alive()
        assert not server.failures, (mode,server.failures)
        assert server.external_attempts == 0
        assert b'PRIVATE_FIXTURE_KEY' not in result.stdout+result.stderr and b'PRIVATE_PROVIDER_BODY' not in result.stdout+result.stderr
        requests=int(mode in ('output-budget','invalid-output','unreadable','provider-error','cancel','success','explicit-override'))
        assert len(server.requests)==requests,(mode,len(server.requests))
        if mode=='no-live': assert result.returncode != 0 and not output.exists()
        elif mode=='existing-output': assert result.returncode != 0 and output.read_text(encoding='utf-8')=='preserve original'
        else:
            text=output.read_text(encoding='utf-8');record=json.loads(text);assert 'PRIVATE_FIXTURE_KEY' not in text and 'PRIVATE_PROVIDER_BODY' not in text
            assert record['engine_invocations']==requests
            if requests:
                assert record['engine_content_cleared'] is True
                descriptor=json.dumps(server.descriptor,ensure_ascii=False,separators=(',',':'),sort_keys=True).encode()
                assert record['prompt_sha256']==hashlib.sha256(descriptor).hexdigest()
                assert record['prompt_bytes']==len(descriptor)
            if mode in ('success','explicit-override'):
                assert result.returncode==0 and record['status']=='ok'
                pipeline=record['pipeline'];assert pipeline['cache_reopened'] and pipeline['prompt_matches']
                norm=pipeline['normalization'];key=pipeline['expected_key'];assert key['format']=='notebook' and key['extractor_revision'].startswith('notebook-ocr-unverified-v1:')
                assert norm['authority']=='unverified-historical-page-image-ocr'
                image_set=b'notebook-page-image-set-v1\0'+struct.pack('>I',2)
                for page in (9,2): image_set += struct.pack('>IQ',page,len(png()))+png()
                assert key['source_sha256']==hashlib.sha256(image_set).hexdigest()
                for i,entry in enumerate(pipeline['queries']):
                    found=entry['retrieved'];assert found['source']==key and found['passages']
                    p=found['passages'][0];assert p['page']==(9,2)[i]
                    unit=norm['document']['units'][p['unit']]['text'].encode();assert unit[p['range'][0]:p['range'][1]].decode()==p['text']
            else: assert result.returncode != 0 and record['status']=='failed' and record['pipeline'] is None
            if requests and mode not in ('provider-error','cancel'):
                raw=base64.b64decode(record['raw_model_response_base64'],validate=True);assert raw==server.returned.encode() and hashlib.sha256(raw).hexdigest()==record['response_sha256']
        return {'case':mode,'http_requests':requests,'result':'pass'}

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('binary',type=pathlib.Path);args=parser.parse_args()
    for mode in ('no-live','existing-output','missing-key','configured-provider','invalid-base','input-budget','output-budget','invalid-output','unreadable','provider-error','cancel','success','explicit-override'):
        print(json.dumps(case(args.binary.resolve(),mode)),flush=True)
