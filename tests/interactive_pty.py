#!/usr/bin/env python3
"""Exercise the interactive CLI through a PTY using only a synthetic home."""

import fcntl
import hashlib
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import termios
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BINARY = (ROOT / "target/debug/akcleaner").resolve()
TIMEOUT_SECONDS = 8
COLOR_SEQUENCE = re.compile(rb"\x1b\[[0-9;:]*m")
TERMINAL_SEQUENCE = re.compile(
    rb"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))"
)


class PtyCli:
    def __init__(self, root: Path, home: Path, rules: Path | None = None):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 100, 0, 0))
        environment = os.environ.copy()
        environment.update({"HOME": str(home), "NO_COLOR": "1", "TERM": "xterm-256color"})
        command = [str(BINARY), "clean", "--home", str(home)]
        if rules is not None:
            command.extend(["--rules-dir", str(rules)])
        self.process = subprocess.Popen(
            command,
            cwd=root,
            env=environment,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            close_fds=True,
        )
        os.close(slave)
        self.output = bytearray()

    def read_until(self, expected: str, timeout: float = TIMEOUT_SECONDS) -> None:
        needle = expected.encode("utf-8")
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if needle in self.output:
                return
            if self.process.poll() is not None:
                break
            readable, _, _ = select.select([self.master], [], [], 0.1)
            if readable:
                self._read_available()
        text = self.transcript()
        raise AssertionError(f"timed out waiting for {expected!r}; output tail:\n{text[-1200:]}")

    def send(self, data: bytes) -> None:
        time.sleep(0.1)
        os.write(self.master, data)

    def wait_for_exit(self, timeout: float = TIMEOUT_SECONDS) -> int:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                self._drain()
                return self.process.returncode
            readable, _, _ = select.select([self.master], [], [], 0.1)
            if readable:
                self._read_available()
        raise AssertionError(f"CLI did not exit; output tail:\n{self.transcript()[-1200:]}")

    def transcript(self) -> str:
        return TERMINAL_SEQUENCE.sub(b"", bytes(self.output)).decode("utf-8", errors="replace")

    def _read_available(self) -> None:
        try:
            self.output.extend(os.read(self.master, 8192))
        except OSError:
            pass

    def _drain(self) -> None:
        while select.select([self.master], [], [], 0.05)[0]:
            before = len(self.output)
            self._read_available()
            if len(self.output) == before:
                break

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=0.5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=0.5)
        try:
            os.close(self.master)
        except OSError:
            pass


def create_home(root: Path) -> tuple[Path, Path, bytes]:
    home = root / "synthetic-home"
    config = home / ".claude/settings.json"
    config.parent.mkdir(parents=True)
    original = json.dumps(
        {
            "hooks": {
                "Stop": [
                    {
                        "hooks": [
                            {"type": "command", "command": "runner muxy-notification-hook"},
                            {
                                "type": "command",
                                "command": "runner $SUPERSET_HOME_DIR/hooks/notify.sh",
                            },
                        ]
                    }
                ]
            }
        },
        separators=(",", ":"),
    ).encode()
    config.write_bytes(original)
    return home, config, original


def verify_no_write_before_confirmation(cli: PtyCli, config: Path, original: bytes) -> None:
    assert not COLOR_SEQUENCE.search(bytes(cli.output)), "NO_COLOR emitted a color sequence"
    assert config.read_bytes() == original, "synthetic config was modified"
    assert not config.parents[1].joinpath(".local/state/akcleaner/backups").exists(), (
        "a backup was created without a confirmed cleanup"
    )


def verify_yes_cleanup(home: Path, config_path: Path, original: bytes, original_mode: int) -> None:
    config = json.loads(config_path.read_text())
    remaining = config["hooks"]["Stop"][0]["hooks"]
    assert remaining == [{"type": "command", "command": "runner muxy-notification-hook"}], (
        "Yes must remove the selected Superset hook and keep the unselected Muxy hook"
    )

    backup_parent = home / ".local/state/akcleaner/backups"
    backup_dirs = list(backup_parent.iterdir())
    assert len(backup_dirs) == 1, "Yes must create exactly one private backup"
    backup = backup_dirs[0]
    assert (backup / "files/.claude/settings.json").read_bytes() == original
    backup_map = json.loads((backup / "backup-map.json").read_text())
    file_entry = backup_map["files"][0]
    assert file_entry["original_mode"] == original_mode
    assert file_entry["sha256"] == hashlib.sha256(original).hexdigest()
    assert (backup / "files/.claude/settings.json").stat().st_mode & 0o777 == 0o600
    receipt = json.loads((backup / "execution-receipt.json").read_text())
    assert receipt["status"] == "complete"
    assert all(operation["status"] == "succeeded" for operation in receipt["operations"])


def run_scenario(name: str, interaction: str) -> tuple[str, int]:
    with tempfile.TemporaryDirectory(prefix="akcleaner-pty-") as directory:
        root = Path(directory)
        home, config, original = create_home(root)
        original_mode = config.stat().st_mode & 0o7777
        cli = PtyCli(root, home)
        try:
            cli.read_until("输入搜索")
            assert "开始清理" not in cli.transcript(), "unexpected preliminary menu"
            assert "清理全部" not in cli.transcript(), "unexpected preliminary menu"
            assert "settings.json" not in cli.transcript(), "entry screen exposed artifact paths"
            assert "第 1 组" not in cli.transcript(), "entry screen exposed hook internals"
            if interaction == "enter_twice":
                cli.send(b"\r")
                cli.read_until("删除 Muxy 的全部集成？")
                cli.send(b"\r")
                expected = 0
            elif interaction == "escape":
                cli.send(b"\x1b")
                expected = 0
            elif interaction == "interrupt":
                cli.send(b"\x03")
                expected = 0
            else:
                cli.send(b"Super")
                time.sleep(0.2)
                cli.send(b"\r")
                cli.read_until("删除 Superset 的全部集成？")
                cli.read_until("↑↓ 选择 · Enter 确认")
                assert "查看具体改动" not in cli.transcript()
                assert "待核实" not in cli.transcript()
                assert "备份" not in cli.transcript()
                verify_no_write_before_confirmation(cli, config, original)
                if interaction == "no":
                    cli.send(b"\r")  # Default is Don't delete.
                    expected = 0
                elif interaction == "yes":
                    cli.send(b"\x1b[A\r")
                    cli.read_until("已删除。")
                    expected = 0
                elif interaction in ("confirm_escape", "confirm_interrupt"):
                    cli.send(b"\x1b" if interaction == "confirm_escape" else b"\x03")
                    expected = 0
                else:
                    raise AssertionError(f"unknown interaction {interaction!r}")

            exit_code = cli.wait_for_exit()
            assert exit_code == expected, (exit_code, cli.transcript()[-1200:])
            assert not COLOR_SEQUENCE.search(bytes(cli.output)), "NO_COLOR emitted a color sequence"
            if interaction == "yes":
                verify_yes_cleanup(home, config, original, original_mode)
            else:
                verify_no_write_before_confirmation(cli, config, original)
            transcript = cli.transcript()
            return transcript, exit_code
        finally:
            cli.close()


def print_excerpt(name: str, transcript: str, exit_code: int) -> None:
    matches = [
        line.strip()
        for line in transcript.replace("\r", "").splitlines()
        if any(
            marker in line
            for marker in (
                "选择 App",
                "全部集成？",
                "已删除。",
                "已取消",
            )
        )
    ]
    outcome = "selected-app-cleaned" if name == "Yes executes cleanup" else "unchanged"
    print(f"[{name}] exit={exit_code} NO_COLOR=clean synthetic-home={outcome}")
    print("\n".join(matches[-10:]))


def verify_empty_home() -> None:
    with tempfile.TemporaryDirectory(prefix="akcleaner-empty-") as directory:
        home = Path(directory)
        cli = PtyCli(home, home)
        try:
            cli.read_until("现在没有任何东西可以卸载。")
            assert cli.wait_for_exit() == 0
            assert "选择 App" not in cli.transcript()
            assert "全部集成？" not in cli.transcript()
            assert not COLOR_SEQUENCE.search(bytes(cli.output))
            print("[empty home] no-picker no-confirmation NO_COLOR=clean")
            print(cli.transcript().strip())
        finally:
            cli.close()


def verify_large_catalog() -> None:
    """Search must map back to the correct app, even beyond the first page."""
    with tempfile.TemporaryDirectory(prefix="akcleaner-catalog-") as directory:
        root = Path(directory)
        home = root / "home"
        rules = root / "rules"
        config = home / ".claude/settings.json"
        config.parent.mkdir(parents=True)
        actions = []
        for number in range(30):
            product = f"app-{number:02}"
            rule_file = rules / product / "rules.json"
            rule_file.parent.mkdir(parents=True)
            rule_file.write_text(json.dumps({
                "schema_version": 1,
                "product": {"id": product, "name": f"App {number:02}"},
                "verified": {"repository": "https://example.invalid/repo", "commit": "0" * 40},
                "rules": [{"id": "hooks", "surfaces": ["claude.hooks.user"],
                           "evidence": {"type": "command_marker", "marker": f"{product}:hook"}}],
            }))
            actions.append({"type": "command", "command": f"run {product}:hook"})
        config.write_text(json.dumps({"hooks": {"Stop": [{"hooks": actions}]}}))
        cli = PtyCli(root, home, rules)
        try:
            cli.read_until("输入搜索")
            assert "开始清理" not in cli.transcript(), "unexpected preliminary menu"
            visible = set(re.findall(r"App \d{2}", cli.transcript()))
            assert 0 < len(visible) <= 8, visible
            assert "App 29" not in visible
            cli.send(b"29")
            cli.read_until("App 29")
            cli.send(b"\r")
            cli.read_until("删除 App 29 的全部集成？")
            cli.read_until("↑↓ 选择 · Enter 确认")
            cli.send(b"\x1b[A\r")
            cli.read_until("已删除。")
            assert cli.wait_for_exit() == 0
            remaining = json.loads(config.read_text())["hooks"]["Stop"][0]["hooks"]
            assert remaining == actions[:29], "filtered selection cleaned the wrong application"
            assert not COLOR_SEQUENCE.search(bytes(cli.output))
            print("[30 apps] entry=app-picker picker<=8-rows search=correct-target cleanup=passed")
        finally:
            cli.close()


def main() -> None:
    if not BINARY.is_file():
        raise SystemExit(f"missing built binary: {BINARY}; run cargo build --bin akcleaner first")

    for name, interaction in (
        ("Enter twice", "enter_twice"),
        ("Esc", "escape"),
        ("Ctrl-C", "interrupt"),
        ("default No", "no"),
        ("confirmation Esc", "confirm_escape"),
        ("confirmation Ctrl-C", "confirm_interrupt"),
        ("Yes executes cleanup", "yes"),
    ):
        transcript, exit_code = run_scenario(name, interaction)
        print_excerpt(name, transcript, exit_code)
    verify_empty_home()
    verify_large_catalog()


if __name__ == "__main__":
    main()
