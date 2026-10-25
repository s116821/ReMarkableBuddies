"""Actual built answering example over loopback, dummy credentials and owned joined servers."""
import argparse, base64, hashlib, http.server, json, os, pathlib, subprocess, tempfile, threading
from research_example_check import Server

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_CONNECT(self):
        self.server.external_attempts += 1
        self.send_error(502, "owned fixture denies external forwarding")
    def do_POST(self):
        self.connection.settimeout(3)
        assert self.path == "/v1/chat/completions"
        assert self.headers["Authorization"] == "Bearer PRIVATE_FIXTURE_KEY"
        size = int(self.headers["Content-Length"])
        assert 0 < size <= 1048576
        raw = self.rfile.read(size)
        body = json.loads(raw)
        self.server.requests.append(body)
        assert body["model"] == "loopback-answer-model"
        assert body["max_completion_tokens"] == 4000
        assert len(body["messages"]) == 1 and body["messages"][0]["role"] == "user"
        parts = body["messages"][0]["content"]
        assert len(parts) == 2 and all(p["type"] == "text" for p in parts)
        frozen = json.loads(parts[1]["text"])
        assert frozen["research"]["basis"] == "synthetic"
        assert frozen["context"]["request_context"]["request"]["text"].startswith("  What exact Avogadro")
        assert frozen["context"]["request_context"]["history"][0]["text"].startswith(" Earlier exact history")
        reply = json.dumps({"status":"answered", "text":"Avogadro is exactly 6.02214076 x 10^23 per mole [S1]. The older note is tenfold too small [M1]; synthetic research agrees [R1].", "citations":["S1","M1","R1"]})
        if self.server.mode == "invalid-output": reply = "authored malformed returned answer"
        self.server.returned = reply
        raw = json.dumps({"choices":[{"message":{"content":reply}}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

def case(binary, mode):
    with tempfile.TemporaryDirectory(prefix="buddy-answer-example-") as folder:
        root = pathlib.Path(folder); output = root / "private-evidence.json"
        server = Server(("127.0.0.1", 0), Handler)
        server.requests, server.failures, server.external_attempts, server.mode = [], [], 0, mode
        worker = threading.Thread(target=lambda: server.serve_forever(poll_interval=0.01)); worker.start()
        try:
            args = [str(binary), "--output", str(output), "--base-url", f"http://127.0.0.1:{server.server_port}", "--model", "loopback-answer-model"]
            if mode != "no-live": args.append("--live")
            if mode == "configured-provider":
                pos = args.index("--base-url"); del args[pos:pos+2]
            if mode == "invalid-base": args[args.index("--base-url")+1] += "/v1/responses"
            if mode == "input-budget": args += ["--max-counted-bytes", "1"]
            if mode == "output-budget": args += ["--max-answer-bytes", "1"]
            if mode == "existing-output": output.write_text("retained original", encoding="utf-8")
            env = {k:os.environ[k] for k in ("SYSTEMROOT","WINDIR","PATH") if k in os.environ}
            env.update(TMP=folder,TEMP=folder,HTTPS_PROXY=f"http://127.0.0.1:{server.server_port}",NO_PROXY="127.0.0.1")
            if mode != "missing-key": env["OPENAI_API_KEY"] = "PRIVATE_FIXTURE_KEY"
            if mode in ("configured-provider","explicit-override"): env["OPENAI_BASE_URL"] = "https://other-provider.invalid"
            result = subprocess.run(args, cwd=root, env=env, capture_output=True, timeout=20)
        finally:
            server.shutdown(); server.server_close(); worker.join(timeout=5)
            assert not worker.is_alive(), "owned HTTP fixture not joined"
        assert not server.failures, (mode, server.failures)
        assert server.external_attempts == 0
        assert b"PRIVATE_FIXTURE_KEY" not in result.stdout + result.stderr
        calls = int(mode in ("output-budget","invalid-output","success","explicit-override"))
        assert len(server.requests) == calls, (mode,len(server.requests))
        if mode == "no-live": assert result.returncode != 0 and not output.exists()
        elif mode == "existing-output": assert result.returncode != 0 and output.read_text(encoding="utf-8") == "retained original"
        else:
            raw = output.read_text(encoding="utf-8"); record = json.loads(raw)
            assert "PRIVATE_FIXTURE_KEY" not in raw
            assert record["engine_invocations"] == calls
            if calls:
                returned = server.returned.encode()
                assert base64.b64decode(record["raw_model_response_base64"],validate=True) == returned
                assert record["response_sha256"] == hashlib.sha256(returned).hexdigest()
                assert record["engine_content_cleared"] is True
                parts = server.requests[0]["messages"][0]["content"]
                assert record["context_json"] == parts[1]["text"]
                assert record["context_sha256"] == hashlib.sha256(parts[1]["text"].encode()).hexdigest()
            if mode in ("success","explicit-override"):
                assert result.returncode == 0 and record["status"] == "ok", (mode, record["status"], record["fixture_assertions"], record["error"])
                assert all(record["fixture_assertions"].values())
                assert record["draft"]["context_sha256"] == record["context_sha256"]
                assert record["draft"]["prompt_sha256"] == record["prompt_sha256"]
                assert record["draft"]["citations"][2]["basis"] == "synthetic"
            else:
                assert result.returncode != 0 and record["status"] == "failed"
                assert record["draft"] is None
        return {"case":mode,"requests":calls,"exit_code":result.returncode,"result":"pass"}

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=pathlib.Path,required=True)
    args = parser.parse_args()
    for mode in ("no-live","existing-output","missing-key","configured-provider","invalid-base","input-budget","output-budget","invalid-output","success","explicit-override"):
        print(json.dumps(case(args.binary.resolve(strict=True),mode)),flush=True)
