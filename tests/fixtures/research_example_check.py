"""Exercise the actual development example over loopback; never uses real credentials."""
import argparse, base64, hashlib, http.server, json, os, pathlib, subprocess, tempfile, threading

class Server(http.server.ThreadingHTTPServer):
    daemon_threads = False
    block_on_close = True
    def handle_error(self, request, client_address):
        self.failures.append("owned HTTP fixture failed")

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def do_CONNECT(self):
        self.server.external_attempts += 1
        self.send_error(502, "owned fixture denies external forwarding")
    def do_POST(self):
        self.connection.settimeout(3)
        assert self.path == "/v1/responses"
        assert self.headers.get("Authorization") == "Bearer PRIVATE_FIXTURE_KEY"
        size = int(self.headers["Content-Length"])
        assert 0 < size <= 1048576
        body = self.rfile.read(size)
        request = json.loads(body)
        with self.server.lock:
            self.server.requests.append((body, request))
        sources = [{"url":"https://example.invalid/public-source", "title":"Authored development fixture", "excerpt":"Provider-attributed unverified summary."}]
        response = {"status":"completed", "PRIVATE_SUCCESS_BODY":"authored retained-response marker", "output":[
            {"type":"web_search_call","status":"completed","action":{"type":"search","queries":["authored public SI query"],"sources":[{"type":"url","url":sources[0]["url"]}]}},
            {"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":json.dumps({"sources":sources})}]}]}
        raw = json.dumps(response).encode()
        self.server.raw_response = raw
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


def case(binary, mode):
    with tempfile.TemporaryDirectory(prefix="buddy-web-example-") as folder:
        root = pathlib.Path(folder)
        output = root / "private-evidence.json"
        server = Server(("127.0.0.1", 0), Handler)
        server.requests, server.failures = [], []
        server.external_attempts = 0
        server.lock = threading.Lock()
        worker = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.01))
        worker.start()
        try:
            args = [str(binary), "--output", str(output), "--endpoint", f"http://127.0.0.1:{server.server_port}/v1/responses", "--model", "loopback-model"]
            if mode == "configured-provider":
                pos = args.index("--endpoint")
                del args[pos:pos+2]
            if mode != "no-live":
                args.append("--live")
            if mode == "composition-failure":
                args += ["--max-counted-bytes", "1"]
            if mode == "existing-output":
                output.write_text("retained original", encoding="utf-8")
            environment = {k: os.environ[k] for k in ("SYSTEMROOT", "WINDIR", "PATH") if k in os.environ}
            environment.update(TMP=folder, TEMP=folder,
                HTTPS_PROXY=f"http://127.0.0.1:{server.server_port}", NO_PROXY="127.0.0.1")
            if mode in ("configured-provider", "explicit-override"):
                environment["OPENAI_BASE_URL"] = "https://other-provider.invalid"
            if mode != "missing-key":
                environment["OPENAI_API_KEY"] = "PRIVATE_FIXTURE_KEY"
            result = subprocess.run(args, cwd=root, env=environment, capture_output=True, timeout=15)
        finally:
            server.shutdown()
            server.server_close()
            worker.join(timeout=5)
            assert not worker.is_alive(), "owned fixture not joined"
        assert not server.failures
        assert server.external_attempts == 0, "unexpected external provider dispatch"
        assert b"PRIVATE_FIXTURE_KEY" not in result.stdout + result.stderr
        expected_calls = int(mode in ("success", "explicit-override", "composition-failure"))
        assert len(server.requests) == expected_calls, (mode, len(server.requests))
        if mode == "no-live":
            assert result.returncode != 0 and not output.exists()
        elif mode == "existing-output":
            assert result.returncode != 0 and output.read_text(encoding="utf-8") == "retained original"
        else:
            record = json.loads(output.read_text(encoding="utf-8"))
            assert "PRIVATE_FIXTURE_KEY" not in output.read_text(encoding="utf-8")
            if mode in ("success", "explicit-override"):
                assert result.returncode == 0 and record["status"] == "ok" and record["provider_calls"] == 1
                exchange = record["exchange"]
                assert base64.b64decode(exchange["response_base64"], validate=True) == server.raw_response
                assert exchange["response_sha256"] == hashlib.sha256(server.raw_response).hexdigest()
                assert exchange["request_sha256"] == hashlib.sha256(server.requests[0][0]).hexdigest()
                assert exchange["identity"]["context_sha256"] == record["context_sha256"]
                assert server.requests[0][1]["store"] is False
                assert record["research"]["basis"] == "provider-asserted"
            else:
                assert result.returncode != 0 and record["status"] == "failed" and record["exchange"] is None
                assert "PRIVATE_SUCCESS_BODY" not in output.read_text(encoding="utf-8")
                if mode == "composition-failure":
                    assert record["provider_calls"] == 1
                    assert "token limit" in record["composition_error"]
                    assert base64.b64encode(server.raw_response).decode() not in output.read_text(encoding="utf-8")
        return {"case":mode,"requests":expected_calls,"exit_code":result.returncode,"result":"pass"}

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    for mode in ("no-live", "existing-output", "missing-key", "configured-provider", "composition-failure", "success", "explicit-override"):
        print(json.dumps(case(binary, mode)), flush=True)
