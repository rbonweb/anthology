"""The BrowserOS Neo bridge (design-taste-frontend/scripts/neo.mjs), driven the way the
agent's tools drive it, with a fake harness standing in for the BrowserOS tab.

Needs `node` on the PATH (the Docker test images have none, so it is skipped there; the
CI job `taste-bridge` runs it). neo.mjs uses only node: built-ins.
"""
import http.client
import json
import os
import re
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NEO = ROOT / "plugins" / "anthology" / "knowledge" / "design-taste-frontend" / "scripts" / "neo.mjs"
NODE = shutil.which("node")


@unittest.skipUnless(NODE, "node is not installed")
class Bridge(unittest.TestCase):
    """`neo.mjs serve` on a scratch root."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name).resolve()
        (self.dir / "site").mkdir()
        (self.dir / "site" / "index.html").write_text(
            "<!doctype html><html><head><title>t</title></head><body>hi</body></html>\n", encoding="utf-8")
        (self.dir / "site" / "app.js").write_text("var a = 1;\n", encoding="utf-8")
        (self.dir / "secret.txt").write_text("outside the root", encoding="utf-8")
        self.proc = subprocess.Popen([NODE, str(NEO), "serve", "--root", "site"], cwd=self.dir,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(self.stop)
        self.lines = [self.proc.stdout.readline().rstrip("\n") for _ in range(2)]
        info = self.dir / ".taste-skill" / "neo.json"
        self.assertTrue(info.is_file(), self.lines)
        self.info = json.loads(info.read_text(encoding="utf-8"))
        self.port = self.info["port"]

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        for stream in (self.proc.stdout, self.proc.stderr):
            if stream:
                stream.close()

    def call(self, method, path, headers=None, body=None, host=None, timeout=10):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
        try:
            conn.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
            conn.putheader("Host", host or f"127.0.0.1:{self.port}")
            data = body.encode() if isinstance(body, str) else body
            for k, v in (headers or {}).items():
                conn.putheader(k, v)
            if data is not None:
                conn.putheader("Content-Length", str(len(data)))
            conn.endheaders(data)
            res = conn.getresponse()
            return res.status, dict(res.getheaders()), res.read()
        finally:
            conn.close()

    def api(self, method, path, body=None, token=None, **kw):
        headers = {"X-Neo-Token": token or self.info["token"]}
        if body is not None:
            headers["Content-Type"] = "application/json"
            body = json.dumps(body)
        return self.call(method, path, headers=headers, body=body, **kw)


class StartupTest(Bridge):
    def test_prints_the_two_lines_and_writes_the_info_file(self):
        root = str(self.dir / "site")
        self.assertEqual(self.lines[0], f"NEO: serving {root} at http://127.0.0.1:{self.port}/")
        self.assertEqual(self.lines[1],
                         f"NEO: open the harness in a BrowserOS Neo tab: http://127.0.0.1:{self.port}/__neo/")
        self.assertTrue(5300 <= self.port <= 5309)
        self.assertEqual(self.info["root"], root)
        self.assertEqual(self.info["origin"], f"http://127.0.0.1:{self.port}")
        self.assertEqual(self.info["pid"], self.proc.pid)
        for key in ("token", "saveToken"):
            self.assertRegex(self.info[key], r"^[0-9a-f]{32}$")
        self.assertNotEqual(self.info["token"], self.info["saveToken"])

    def test_the_root_must_be_inside_the_working_folder(self):
        for bad in ("..", "/tmp"):
            out = subprocess.run([NODE, str(NEO), "serve", "--root", bad], cwd=self.dir / "site",
                                 capture_output=True, text=True, timeout=20)
            self.assertNotEqual(out.returncode, 0, bad)
            self.assertIn("inside", out.stderr)


class SecurityTest(Bridge):
    def test_wrong_host_gets_421(self):
        for path in ("/", "/__neo/", f"/__neo/next?t={self.info['token']}"):
            for host in (f"evil.example:{self.port}", "127.0.0.1", f"127.0.0.1.evil.example:{self.port}"):
                self.assertEqual(self.call("GET", path, host=host)[0], 421, (path, host))
        self.assertEqual(self.call("GET", "/", host=f"localhost:{self.port}")[0], 200)

    def test_foreign_origin_gets_403_and_there_is_no_cors(self):
        origin = {"Origin": "http://evil.example"}
        for method, path in (("GET", "/index.html"), ("GET", "/__neo/api/status"), ("POST", "/__neo/api/rpc")):
            status, headers, _ = self.call(method, path, headers={**origin, "X-Neo-Token": self.info["token"]})
            self.assertEqual(status, 403, path)
            self.assertNotIn("access-control-allow-origin", {k.lower() for k in headers})
        status, headers, _ = self.call("GET", "/index.html", headers={"Origin": self.info["origin"]})
        self.assertEqual(status, 200)
        self.assertNotIn("access-control-allow-origin", {k.lower() for k in headers})
        status, headers, _ = self.call("OPTIONS", "/__neo/api/rpc", headers={"Origin": self.info["origin"]})
        self.assertNotIn("access-control-allow-origin", {k.lower() for k in headers})

    def test_the_api_needs_the_token(self):
        for method, path in (("GET", "/__neo/api/status"), ("POST", "/__neo/api/rpc"),
                             ("GET", "/__neo/api/requests?tab=a"), ("GET", "/__neo/next"),
                             ("GET", "/__neo/next?t=" + "0" * 32), ("POST", "/__neo/done?t=nope&id=1"),
                             ("POST", "/__neo/hello?t=nope")):
            self.assertEqual(self.call(method, path, body="{}" if method == "POST" else None)[0], 401, path)
        self.assertEqual(self.call("GET", "/__neo/api/status", headers={"X-Neo-Token": "0" * 32})[0], 401)
        self.assertEqual(self.call("GET", "/__neo/api/status", headers={"X-Neo-Token": self.info["saveToken"]})[0], 401)
        self.assertEqual(self.api("GET", "/__neo/api/status")[0], 200)

    def test_save_is_limited_to_images_under_shots_and_the_save_token(self):
        save, token = "/__neo/api/save?file=", self.info["saveToken"]
        png = "iVBORw0KGgo="
        for rel in ("index.html", "../x.png", "shots/../../x.png", "shots/a.html", "shots/a.svg", "/etc/x.png",
                    "other/a.png", "shots/%2e%2e/a.png", "shots/a.png/../../b.js", "", "shots/\\x.png",
                    ".taste-skillx/a.png"):
            status, _, _ = self.call("POST", save + rel, headers={"X-Neo-Token": token}, body=png)
            self.assertIn(status, (400, 403), rel)
        self.assertEqual(self.call("POST", save + "shots/a.png", headers={"X-Neo-Token": self.info["token"]},
                                   body=png)[0], 401)
        self.assertFalse((self.dir / "site" / "shots").exists())
        self.assertFalse((self.dir / "x.png").exists())
        status, _, body = self.call("POST", save + "shots/1440-01.jpg", headers={"X-Neo-Token": token},
                                    body="data:image/jpeg;base64,/9j/4AAQ")
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body), {"file": "shots/1440-01.jpg", "bytes": 6})
        self.assertEqual((self.dir / "site" / "shots" / "1440-01.jpg").read_bytes(), bytes.fromhex("ffd8ffe000" "10"))
        status, _, body = self.call("POST", save + ".taste-skill/x.png", headers={"X-Neo-Token": token}, body=png)
        self.assertEqual(status, 200)
        self.assertTrue((self.dir / "site" / ".taste-skill" / "x.png").is_file())

    def test_static_files_never_leave_the_root(self):
        for path in ("/../secret.txt", "/%2e%2e/secret.txt", "/..%2fsecret.txt", "/a/../../secret.txt"):
            self.assertIn(self.call("GET", path)[0], (400, 403, 404), path)
        sock = socket.create_connection(("127.0.0.1", self.port), timeout=10)
        sock.sendall(f"GET /../secret.txt HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\nConnection: close\r\n\r\n".encode())
        raw = b""
        while chunk := sock.recv(4096):
            raw += chunk
        sock.close()
        self.assertNotIn(b"outside the root", raw)


class StaticTest(Bridge):
    def test_a_page_is_served_without_the_init_blocks_unless_asked(self):
        status, headers, body = self.call("GET", "/index.html")
        self.assertEqual(status, 200)
        self.assertNotIn(b"data-neo-init", body)
        self.assertEqual(headers.get("Cache-Control"), "no-store")
        status, _, body = self.call("GET", "/")
        self.assertEqual((status, b"hi" in body), (200, True))
        self.assertNotIn(b"data-neo-init", body)
        self.assertEqual(self.call("GET", "/nope.html")[0], 404)

    def test_the_init_blocks_go_in_only_with_the_neo_parameter(self):
        status, _, body = self.call("GET", "/index.html?__neo=tab1")
        text = body.decode()
        self.assertEqual(status, 200)
        self.assertEqual(text.count("<script data-neo-init>"), 2)
        self.assertLess(text.index("data-neo-init"), text.index("<title>"))
        self.assertTrue(text.startswith("<!doctype html><html><head><script data-neo-init>"))
        self.assertIn("__neoLogs", text)
        self.assertIn("HeadlessChrome", text)
        self.assertEqual(self.call("GET", "/app.js?__neo=tab1")[2], b"var a = 1;\n")

    def test_registered_init_scripts_are_injected_for_that_tab_only(self):
        status, _, _ = self.api("POST", "/__neo/api/rpc",
                                {"op": "init", "tab": "tab9", "source": "window.__MARK = 'zz-init-9';"})
        self.assertEqual(status, 200)
        text = self.call("GET", "/index.html?__neo=tab9")[2].decode()
        self.assertIn("window.__MARK = 'zz-init-9';", text)
        self.assertEqual(text.count("<script data-neo-init>"), 3)
        self.assertNotIn("zz-init-9", self.call("GET", "/index.html?__neo=tab8")[2].decode())

    def test_gzip_for_text_types(self):
        status, headers, body = self.call("GET", "/app.js", headers={"Accept-Encoding": "gzip"})
        self.assertEqual((status, headers.get("Content-Encoding")), (200, "gzip"))
        import gzip
        self.assertEqual(gzip.decompress(body), b"var a = 1;\n")

    def test_the_request_log_records_own_requests_per_tab(self):
        self.call("GET", "/index.html?__neo=tabL")
        self.call("GET", "/app.js", headers={"Referer": f"{self.info['origin']}/index.html?__neo=tabL"})
        self.call("GET", "/missing.png?__neo=tabL")
        status, _, body = self.api("GET", "/__neo/api/requests?tab=tabL")
        log = json.loads(body)
        self.assertEqual(status, 200)
        by_url = {r["url"].split("?")[0]: r for r in log}
        self.assertEqual(by_url[self.info["origin"] + "/index.html"]["type"], "Document")
        self.assertEqual(by_url[self.info["origin"] + "/app.js"]["type"], "Script")
        self.assertEqual(by_url[self.info["origin"] + "/missing.png"]["status"], 404)
        self.assertTrue(by_url[self.info["origin"] + "/missing.png"]["failed"])
        self.assertEqual(by_url[self.info["origin"] + "/app.js"]["decoded"], len(b"var a = 1;\n"))


class RootSecretsTest(unittest.TestCase):
    @unittest.skipUnless(NODE, "node is not installed")
    def test_the_bridge_info_file_is_not_served_when_the_root_is_the_working_folder(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp).resolve()
            (work / "index.html").write_text("<html><head></head><body>x</body></html>\n", encoding="utf-8")
            proc = subprocess.Popen([NODE, str(NEO), "serve"], cwd=work, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True)
            try:
                proc.stdout.readline()
                proc.stdout.readline()
                info = json.loads((work / ".taste-skill" / "neo.json").read_text(encoding="utf-8"))
                self.assertEqual(info["root"], str(work))
                self.assertEqual((work / ".taste-skill" / ".gitignore").read_text(encoding="utf-8"), "*\n")
                for path in ("/.taste-skill/neo.json", "/.taste-skill/", "/.taste-skill/.gitignore",
                             "/.TASTE-SKILL/neo.json", "/.Taste-Skill/neo.json", "/.TASTE-SKILL/", "/.TASTE-SKILL/.gitignore"):
                    conn = http.client.HTTPConnection("127.0.0.1", info["port"], timeout=10)
                    conn.request("GET", path)
                    res = conn.getresponse()
                    body = res.read()
                    conn.close()
                    self.assertEqual(res.status, 404, path)
                    self.assertNotIn(info["token"].encode(), body)
            finally:
                proc.terminate()
                proc.wait(10)
                proc.stdout.close()
                proc.stderr.close()


class FakeHarness:
    """What the BrowserOS tab does: poll /__neo/next, answer with /__neo/done."""

    def __init__(self, case, visible="visible", raf=60):
        self.case, self.visible, self.raf = case, visible, raf
        self.jobs = []
        self.answer = lambda job: {"ok": True, "value": None}
        self._stop = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def start(self):
        self.case.call("POST", f"/__neo/hello?t={self.case.info['token']}", body="{}")
        self.thread.start()
        deadline = time.time() + 10
        while time.time() < deadline:
            status, _, body = self.case.api("GET", "/__neo/api/status")
            if status == 200 and json.loads(body)["harness"]:
                return
            time.sleep(0.05)
        raise AssertionError("the harness never showed up")

    def stop(self):
        self._stop.set()
        self.thread.join(40)

    def _run(self):
        token = self.case.info["token"]
        while not self._stop.is_set():
            try:
                status, _, body = self.case.call(
                    "GET", f"/__neo/next?t={token}&v={self.visible}&r={self.raf}&wait=1", timeout=40)
            except OSError:
                return
            if status != 200:
                continue
            job = json.loads(body)
            self.jobs.append(job)
            reply = self.answer(job)
            self.case.call("POST", f"/__neo/done?t={token}&id={job['id']}", body=json.dumps(reply))


class JobRoundTripTest(Bridge):
    def test_one_eval_job_round_trips_between_a_client_and_the_harness(self):
        harness = FakeHarness(self)
        harness.answer = lambda job: {"ok": True, "value": {"echo": job["expr"], "tab": job["tab"]}}
        harness.start()
        self.addCleanup(harness.stop)
        status, _, body = self.api("POST", "/__neo/api/rpc", {"op": "eval", "tab": "t1", "expr": "1 + 1", "ms": 5000})
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body), {"ok": True, "value": {"echo": "1 + 1", "tab": "t1"}})
        self.assertEqual(harness.jobs[-1]["op"], "eval")
        status, _, body = self.api("GET", "/__neo/api/status")
        info = json.loads(body)
        self.assertEqual((info["harness"], info["visible"], info["raf"]), (True, True, 60))
        self.assertEqual(info["root"], str(self.dir / "site"))

    def test_the_heartbeat_keeps_the_harness_status_fresh_without_taking_jobs(self):
        token = self.info["token"]
        self.assertEqual(self.call("GET", f"/__neo/beat?t={token}&v=visible&r=58")[0], 204)
        self.assertEqual(self.call("GET", "/__neo/beat?t=nope&v=visible&r=58")[0], 401)
        info = json.loads(self.api("GET", "/__neo/api/status")[2])
        self.assertEqual((info["harness"], info["visible"], info["raf"]), (True, True, 58))
        self.assertIn("neo harness: connected", self.call("GET", "/__neo/")[2].decode())

    def test_a_job_with_no_harness_times_out_with_504(self):
        started = time.time()
        status, _, body = self.api("POST", "/__neo/api/rpc", {"op": "ping", "tab": "x", "ms": 100})
        self.assertEqual(status, 504)
        self.assertFalse(json.loads(body)["ok"])
        self.assertLess(time.time() - started, 20)

    def test_a_harness_error_comes_back_as_ok_false(self):
        harness = FakeHarness(self)
        harness.answer = lambda job: {"ok": False, "error": "ReferenceError: x is not defined"}
        harness.start()
        self.addCleanup(harness.stop)
        status, _, body = self.api("POST", "/__neo/api/rpc", {"op": "eval", "tab": "t", "expr": "x", "ms": 5000})
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body), {"ok": False, "error": "ReferenceError: x is not defined"})

    def test_hello_fails_every_pending_job(self):
        results = []
        thread = threading.Thread(target=lambda: results.append(
            self.api("POST", "/__neo/api/rpc", {"op": "eval", "tab": "t", "expr": "1", "ms": 20000})))
        thread.start()
        time.sleep(0.5)
        self.call("POST", f"/__neo/hello?t={self.info['token']}", body="{}")
        thread.join(15)
        status, _, body = results[0]
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body), {"ok": False, "error": "harness reloaded"})

    def test_the_harness_page_carries_the_token_and_the_title(self):
        status, headers, body = self.call("GET", "/__neo/")
        text = body.decode()
        self.assertEqual(status, 200)
        self.assertIn("text/html", headers["Content-Type"])
        self.assertIn(self.info["token"], text)
        self.assertNotIn(self.info["saveToken"], text)
        self.assertIn("neo harness", text)
        self.assertNotIn("data-neo-init", text)


CLIENT = r"""
import { connect, status, NeoTab, NeoError } from %(neo)s;
const out = {};
const s0 = await status({ cwd: process.cwd() });
out.status = { ok: s0.ok, why: s0.why };
const hub = await connect({ cwd: process.cwd(), wait: 3000 });
out.origin = hub.origin;
out.url = hub.urlFor(hub.root + '/dir with space/a b.html');
try { hub.urlFor('/etc/passwd'); } catch (e) { out.outside = [e instanceof NeoError, e.neo === true, e.message]; }
const tab = await NeoTab.open(hub, { width: 390, height: 844 }, ['window.__X = 1;']);
out.vw = [tab.vw, tab.vh];
await tab.goto(hub.urlFor(hub.root + '/index.html'), 5000);
out.value = await tab.eval('document.title', 5000);
out.shot = await tab.shot();
out.noop = await tab.send('Page.enable', {});
out.resize = await tab.send('Emulation.setDeviceMetricsOverride', { width: 768, height: 1024 });
out.vw2 = [tab.vw, tab.vh];
out.evalViaSend = await tab.send('Runtime.evaluate', { expression: '2 + 2' });
try { await tab.send('Page.captureScreenshot', {}); } catch (e) { out.cdp = [e instanceof NeoError, e.neo === true, e.message]; }
try { await tab.eval('throw new Error("boom")', 5000); } catch (e) { out.thrown = e.message; }
out.logs = await tab.logs();
await tab.syncFailed();
out.failed = tab.failed;
out.on = typeof hub.on('x', () => {});
out.hubSend = await hub.send('Browser.close');
await tab.close();
await hub.close();
console.log(JSON.stringify(out));
"""


class ClientTest(Bridge):
    def test_the_client_module_drives_the_hub_through_a_fake_harness(self):
        def answer(job):
            op = job["op"]
            if op == "eval":
                if "boom" in job["expr"]:
                    return {"ok": False, "error": "Error: boom\n    at <anonymous>"}
                return {"ok": True, "value": "TITLE" if "title" in job["expr"] else 4}
            if op == "goto":
                return {"ok": True, "value": {"url": job["url"], "status": "load"}}
            if op == "logs":
                return {"ok": True, "value": ["error: x"]}
            return {"ok": True, "value": None}

        harness = FakeHarness(self)
        harness.answer = answer
        harness.start()
        self.addCleanup(harness.stop)
        script = CLIENT % {"neo": json.dumps(NEO.as_uri())}
        done = subprocess.run([NODE, "--input-type=module", "-e", script], cwd=self.dir, capture_output=True,
                              text=True, timeout=60)
        self.assertEqual(done.returncode, 0, done.stderr)
        out = json.loads(done.stdout.strip().splitlines()[-1])
        self.assertEqual(out["status"], {"ok": True, "why": None})
        self.assertEqual(out["url"], out["origin"] + "/dir%20with%20space/a%20b.html")
        self.assertEqual(out["outside"][:2], [True, True])
        self.assertIn("outside the bridge's root", out["outside"][2])
        self.assertEqual(out["vw"], [390, 844])
        self.assertEqual(out["value"], "TITLE")
        self.assertIsNone(out["shot"])
        self.assertEqual(out["noop"], {})
        self.assertEqual(out["vw2"], [768, 1024])
        self.assertEqual(out["evalViaSend"], {"result": {"value": 4}})
        self.assertEqual(out["cdp"][:2], [True, True])
        self.assertEqual(out["cdp"][2], "Page.captureScreenshot is not available on BrowserOS Neo")
        self.assertTrue(out["thrown"].startswith("page: Error: boom"))
        self.assertEqual(out["logs"], ["error: x"])
        self.assertEqual(out["failed"], [])
        self.assertEqual(out["on"], "function")
        self.assertEqual(out["hubSend"], {})
        ops = [j["op"] for j in harness.jobs]
        self.assertEqual(ops[0], "open")
        self.assertIn("close", ops)
        goto = next(j for j in harness.jobs if j["op"] == "goto")
        self.assertEqual(goto["url"], out["origin"] + "/index.html")

    def test_a_bridge_timeout_is_not_an_unavailable_method(self):
        harness = FakeHarness(self)
        harness.start()
        script = (
            "import { connect, NeoTab, NeoError } from %s;"
            "const hub = await connect({ cwd: process.cwd(), wait: 3000 });"
            "const tab = await NeoTab.open(hub, { width: 390, height: 844 });"
            "const out = {};"
            "try { await tab.send('Page.captureScreenshot', {}); } catch (e) { out.unavailable = [e instanceof NeoError, e.unavailable === true]; }"
            "await new Promise((r) => setTimeout(r, 100));"
            "process.stdout.write('READY\\n');"
            "await new Promise((r) => process.stdin.once('data', r));"
            "try { await tab.eval('1', 200); } catch (e) { out.timeout = [e instanceof NeoError, e.unavailable === true, e.message]; }"
            "console.log(JSON.stringify(out)); process.exit(0);"
        ) % json.dumps(NEO.as_uri())
        proc = subprocess.Popen([NODE, "--input-type=module", "-e", script], cwd=self.dir, stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(proc.kill)
        self.assertEqual(proc.stdout.readline().strip(), "READY", proc.stderr.read() if proc.poll() else "")
        harness.answer = lambda job: {"ok": True, "value": None}
        harness.stop()
        proc.stdin.write("go\n")
        proc.stdin.flush()
        out = json.loads(proc.stdout.readline())
        self.assertEqual(out["unavailable"], [True, True])
        self.assertEqual(out["timeout"][:2], [True, False])
        self.assertIn("did not answer", out["timeout"][2])

    def test_status_says_what_to_do_when_the_harness_is_missing_or_hidden(self):
        script = ("import { status } from %s; const s = await status({ cwd: process.cwd() });"
                  "console.log(JSON.stringify({ ok: s.ok, why: s.why }));") % json.dumps(NEO.as_uri())
        run = lambda: json.loads(subprocess.run([NODE, "--input-type=module", "-e", script], cwd=self.dir,
                                                capture_output=True, text=True, timeout=30).stdout)
        missing = run()
        self.assertFalse(missing["ok"])
        self.assertIn(f"open http://127.0.0.1:{self.port}/__neo/ in a BrowserOS Neo tab", missing["why"])
        harness = FakeHarness(self, visible="hidden", raf=0)
        harness.start()
        self.addCleanup(harness.stop)
        hidden = run()
        self.assertFalse(hidden["ok"])
        self.assertIn("the harness tab is hidden", hidden["why"])
        self.assertIn("mcp__browseros-neo__run", hidden["why"])

    def test_status_cli_prints_json_and_exits_3_when_not_ready(self):
        done = subprocess.run([NODE, str(NEO), "status"], cwd=self.dir, capture_output=True, text=True, timeout=30)
        self.assertEqual(done.returncode, 3)
        data = json.loads(done.stdout)
        self.assertFalse(data["ok"])
        self.assertEqual((data["port"], data["root"]), (self.port, str(self.dir / "site")))
        harness = FakeHarness(self)
        harness.start()
        self.addCleanup(harness.stop)
        done = subprocess.run([NODE, str(NEO), "status"], cwd=self.dir, capture_output=True, text=True, timeout=30)
        self.assertEqual(done.returncode, 0, done.stdout)
        self.assertTrue(json.loads(done.stdout)["ok"])

    def test_status_with_no_bridge_says_how_to_start_it(self):
        with tempfile.TemporaryDirectory() as empty:
            done = subprocess.run([NODE, str(NEO), "status"], cwd=empty, capture_output=True, text=True, timeout=30)
        self.assertEqual(done.returncode, 3)
        self.assertRegex(json.loads(done.stdout)["why"], r"the BrowserOS Neo bridge is not running: run `node .*neo\.mjs serve`")


class StopTest(Bridge):
    def test_stop_ends_the_process_and_removes_the_info_file(self):
        done = subprocess.run([NODE, str(NEO), "stop"], cwd=self.dir, capture_output=True, text=True, timeout=30)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.proc.wait(10)
        self.assertFalse((self.dir / ".taste-skill" / "neo.json").exists())
        again = subprocess.run([NODE, str(NEO), "stop"], cwd=self.dir, capture_output=True, text=True, timeout=30)
        self.assertEqual(again.returncode, 0)

    def test_sigterm_removes_the_info_file(self):
        self.proc.terminate()
        self.proc.wait(10)
        self.assertFalse((self.dir / ".taste-skill" / "neo.json").exists())


class SourceTest(unittest.TestCase):
    def test_neo_mjs_uses_only_node_builtins_and_is_not_executable(self):
        text = NEO.read_text(encoding="utf-8")
        for module in re.findall(r"from '([^']+)'", text):
            self.assertTrue(module.startswith("node:"), module)
        self.assertNotRegex(text, r"\brequire\(|child_process|spawn|exec\(")
        self.assertFalse(NEO.stat().st_mode & 0o111)


if __name__ == "__main__":
    unittest.main()
