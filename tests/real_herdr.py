#!/usr/bin/env python3
"""Exercise Ferry through a private Herdr server and background PTY client.

No live config, session, or terminal is addressed. Every CLI call has an explicit
owned session and isolated XDG directories; teardown stops only that session.
Run after cargo build --release --locked. Evidence includes raw terminal output,
server logs, confirmed snapshots, results, process IDs and preserved identities.
"""
import argparse
import errno
import json
import os
from pathlib import Path
import pty
import select
import shutil
import re
import subprocess
import tempfile
import time
import uuid


class Session:
    def __init__(self, evidence):
        self.root = Path(tempfile.mkdtemp(prefix="ferry-", dir="/tmp"))
        self.name = "ferry-test-" + uuid.uuid4().hex[:6]
        self.binary = shutil.which("herdr")
        assert self.binary, "herdr is required"
        self.evidence = evidence
        evidence.mkdir(parents=True, exist_ok=True)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("HERDR_")}
        self.env.update(XDG_CONFIG_HOME=str(self.root / "c"), XDG_STATE_HOME=str(self.root / "s"),
                        XDG_RUNTIME_DIR=str(self.root / "r"), SHELL="/bin/sh", TERM="xterm-256color")
        config = self.root / "c/herdr"
        config.mkdir(parents=True)
        (self.root / "r").mkdir()
        (config / "config.toml").write_text('onboarding=false\n[terminal]\ndefault_shell="/bin/sh"\nshell_mode="non_login"\n[ui]\nconfirm_close=false\n')
        self.log = open(evidence / "server.log", "w")
        self.server = subprocess.Popen([self.binary, "--session", self.name, "server"], env=self.env,
                                       stdout=self.log, stderr=self.log)
        self.client = None
        self.output = bytearray()
        self.transcript = open(evidence / "client.ansi", "wb")
        self.jobs = self.root / "s/herdr/plugins/shadowfax.ferry/close-jobs"
        self.records = []
        self.wait(lambda: (config / "sessions" / self.name / "herdr.sock").exists(), "server socket")
        self.call("plugin", "link", str(Path(__file__).resolve().parent.parent))
        self.client, self.master = pty.fork()
        if self.client == 0:
            os.execvpe(self.binary, [self.binary, "--session", self.name], self.env)
        import fcntl
        import struct
        import termios
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        self.pump(.4)

    def call(self, *args):
        result = subprocess.run([self.binary, "--session", self.name, *args], env=self.env,
                                capture_output=True, text=True, timeout=20)
        assert result.returncode == 0, (args, result.stdout, result.stderr)
        if result.stdout.strip().startswith("{"):
            return json.loads(result.stdout)["result"]
        return result.stdout

    def pump(self, duration=.1):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            if select.select([self.master], [], [], min(.05, max(0, until-time.monotonic())))[0]:
                try:
                    chunk = os.read(self.master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not chunk:
                    break
                self.output.extend(chunk)
                self.transcript.write(chunk)
                self.transcript.flush()

    def send(self, value):
        os.write(self.master, value.encode())
        self.pump(.2)

    def wait(self, check, label, seconds=12):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = check()
            if value:
                return value
            if self.client:
                self.pump(.05)
            else:
                time.sleep(.05)
        raise AssertionError("timed out: " + label)

    def expect(self, text):
        def visible():
            plain = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(self.output))
            return b"".join(text.encode().split()) in b"".join(plain.split())
        return self.wait(visible, text)

    def workspace(self, label):
        directory = self.root / label
        directory.mkdir(exist_ok=True)
        return self.call("workspace", "create", "--label", label, "--cwd", str(directory), "--no-focus")

    def snapshot(self):
        return self.call("api", "snapshot")["snapshot"]

    def processes(self, panes):
        result = []
        for pane in panes:
            info = self.call("pane", "process-info", "--pane", pane["pane_id"])["process_info"]
            result.append(info["shell_pid"])
            result.extend(p["pid"] for p in info.get("foreground_processes", []))
        return sorted(set(result))

    def sleep_in(self, pane):
        self.call("pane", "run", pane["pane_id"], "sleep 300")
        self.wait(lambda: any(p["name"] == "sleep" for p in self.call("pane", "process-info", "--pane", pane["pane_id"])["process_info"].get("foreground_processes", [])), "sleep foreground")

    def open(self, action="open-close", source=None):
        self.output.clear()
        if source:
            self.call("pane", "run", source, '"$HERDR_BIN_PATH" plugin action invoke shadowfax.ferry.' + action)
        else:
            self.call("plugin", "action", "invoke", "shadowfax.ferry." + action)
        self.expect("Type clear" if action == "clear-ft" else "Close or clear?")

    def choose(self, kind, query, multi=False):
        self.send(kind)
        self.send(query)
        if multi:
            self.send("\x01")
        self.output.clear()
        self.send("\r")
        self.expect("Type clear" if kind == "c" else "Type close")

    def confirm(self, word):
        previous = set(self.jobs.glob("*.result.json")) if self.jobs.exists() else set()
        self.send(word + "\r")
        path = self.wait(lambda: next(iter(set(self.jobs.glob("*.result.json"))-previous), None), "durable result", 20)
        report = json.loads(path.read_text())
        assert report["failed"] == 0, report
        self.pump(.3)
        self.send("\r")
        return report

    def record(self, label, before, report, pids, preserved):
        after = self.snapshot()
        self.wait(lambda: all(not alive(pid) for pid in pids), "closed processes exit")
        for pane in preserved:
            current = next(p for p in after["panes"] if p["pane_id"] == pane["pane_id"])
            assert current["terminal_id"] == pane["terminal_id"]
            assert current.get("cwd") == pane.get("cwd")
        self.records.append(dict(test=label, before=before, after=after, report=report, exited_pids=pids))
        print("PASS", label, report["completed"], "closed", flush=True)

    def close(self):
        try:
            self.call("session", "stop", self.name)
            self.server.wait(timeout=10)
        finally:
            if self.client:
                self.pump(.2)
                os.close(self.master)
                try:
                    os.waitpid(self.client, 0)
                except ChildProcessError:
                    pass
            self.log.close()
            self.transcript.close()
            (self.evidence / "evidence.json").write_text(json.dumps(dict(session=self.name, root=str(self.root), records=self.records), indent=2))
            if self.jobs.exists():
                shutil.copytree(self.jobs, self.evidence / "jobs", dirs_exist_ok=True)


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def run(evidence):
    s = Session(evidence)
    try:
        safe = s.workspace("safe")["root_pane"]
        s.call("workspace", "focus", safe["workspace_id"])
        s.sleep_in(safe)
        safe_pid = s.processes([safe])
        target = s.workspace("target")
        first = target["root_pane"]
        second = s.call("pane", "split", first["pane_id"], "--direction", "right", "--no-focus")["pane"]
        s.sleep_in(first)
        s.sleep_in(second)
        # Cancel both selection and review before any job exists.
        s.open()
        s.send("p")
        s.send("\x03")
        assert not list(s.jobs.glob("*.pending.json"))
        s.open()
        s.choose("p", first["pane_id"])
        s.send("\x03")
        assert not list(s.jobs.glob("*.pending.json"))
        before = s.snapshot()
        pids = s.processes([first])
        s.open()
        s.choose("p", first["pane_id"])
        report = s.confirm("close")
        s.record("close pane + cancel + preserve focus", before, report, pids, [safe, second])
        assert s.snapshot()["focused_pane_id"] == safe["pane_id"]

        extra = s.call("tab", "create", "--workspace", second["workspace_id"], "--cwd", str(s.root), "--no-focus")["root_pane"]
        third = s.call("pane", "split", second["pane_id"], "--direction", "down", "--no-focus")["pane"]
        s.sleep_in(third)
        before = s.snapshot()
        pids = s.processes([second, third])
        s.open()
        s.choose("t", second["tab_id"])
        report = s.confirm("close")
        s.record("close tab, preserve sibling tab", before, report, pids, [safe, extra])

        a = s.workspace("close-batch-a")["root_pane"]
        b = s.workspace("close-batch-b")["root_pane"]
        s.sleep_in(b)
        s.call("workspace", "focus", a["workspace_id"])
        before = s.snapshot()
        pids = s.processes([a, b])
        s.open(source=a["pane_id"])
        s.choose("w", "close-batch", multi=True)
        report = s.confirm("close")
        s.record("close workspaces including invoker; worker survives", before, report, pids, [safe, extra])
        assert not {a["workspace_id"], b["workspace_id"]} & {w["workspace_id"] for w in s.snapshot()["workspaces"]}

        ft = s.workspace("ft")["root_pane"]
        old = s.call("tab", "create", "--workspace", ft["workspace_id"], "--cwd", str(s.root), "--no-focus")["root_pane"]
        s.sleep_in(old)
        s.call("workspace", "focus", ft["workspace_id"])
        before = s.snapshot()
        pids = s.processes([ft, old])
        s.open("clear-ft", source=ft["pane_id"])
        report = s.confirm("clear")
        s.record("clear ft from invoker; fresh shell and workspace survive", before, report, pids, [safe, extra])
        after = s.snapshot()
        survivors = [p for p in after["panes"] if p["workspace_id"] == ft["workspace_id"]]
        assert len(survivors) == 1, survivors
        keeper = survivors[0]
        assert keeper["terminal_id"] not in {ft["terminal_id"], old["terminal_id"]}
        assert keeper["cwd"] == ft["cwd"], (keeper, ft)
        assert all(alive(pid) for pid in s.processes([keeper]))
        assert all(alive(pid) for pid in safe_pid)
        # A root's final pane can cascade when confirm_close=false. Build an
        # owned git fixture to prove Ferry blocks before Herdr sees any close.
        repo = s.root / "group-root"
        repo.mkdir()
        def git(*args):
            subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)
        git("init", "-b", "main")
        git("-c", "user.name=Ferry Test", "-c", "user.email=ferry@example.invalid", "commit", "--allow-empty", "-m", "Fixture")
        root = s.call("workspace", "create", "--cwd", str(repo), "--label", "group-root", "--no-focus")["root_pane"]
        linked = s.call("worktree", "create", "--workspace", root["workspace_id"], "--branch", "fixture", "--path", str(s.root / "linked"), "--no-focus", "--trust-repository")
        before_group = s.snapshot()
        root_info = next(w for w in before_group["workspaces"] if w["workspace_id"] == root["workspace_id"])
        assert root_info.get("worktree"), root_info
        s.call("workspace", "focus", keeper["workspace_id"])
        s.open()
        s.send("w")
        s.send("group-root")
        s.output.clear()
        s.send("\r")
        s.expect("no group closure was authorized")
        s.send("\x03")
        after_group = s.snapshot()
        assert {p["terminal_id"] for p in before_group["panes"]} == {p["terminal_id"] for p in after_group["panes"]}
        s.records.append(dict(test="implicit group cascade rejected with confirm_close=false", before=before_group, after=after_group))
        print("PASS implicit group cascade rejected with confirm_close=false", flush=True)
        s.call("pane", "run", keeper["pane_id"], "printf 'KEEPER-USABLE\\n'")
        s.wait(lambda: "KEEPER-USABLE" in json.dumps(s.call("pane", "read", keeper["pane_id"], "--source", "recent-unwrapped")), "usable keeper")
    finally:
        s.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    run(parser.parse_args().evidence_dir)
