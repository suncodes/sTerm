"""隔离验证旧 Hook 清理、Pi 扩展安装、用户配置保留与错误边界。"""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parents[1] / "integrations" / "install.py"
spec = importlib.util.spec_from_file_location("sterm_installer", SOURCE)
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class InstallerTests(unittest.TestCase):
    def test_cleanup_preserves_user_hooks_and_installs_pi_idempotently(self):
        with tempfile.TemporaryDirectory(prefix="sterm-test-") as temp:
            root = Path(temp) / "profile with spaces"
            path = root / ".claude" / "settings.json"
            path.parent.mkdir(parents=True)
            custom = {"type": "command", "command": "echo user-command"}
            original = {"permissions": {"allow": ["Read"]}, "hooks": {"Stop": [
                {"matcher": "*", "timeout": 99, "hooks": [
                    {"type": "command", "command": "old/sterm-agent-events.py claude stop"}, custom
                ]}
            ]}}
            text = json.dumps(original)
            path.write_text(text, encoding="utf-8")
            results = installer.install(root, ("codex", "claude", "pi"), use_environment=False)
            updated = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(updated["permissions"], original["permissions"])
            self.assertEqual(updated["hooks"], {"Stop": [
                {"matcher": "*", "timeout": 99, "hooks": [custom]}]})
            self.assertFalse((root / ".codex").exists())
            self.assertFalse((root / ".local/share/sterm").exists())
            self.assertTrue((root / ".pi/agent/extensions/sterm-events.ts").exists())
            self.assertEqual(path.with_name("settings.json.sterm.bak").read_text(encoding="utf-8"), text)
            before = {p: p.read_bytes() for p, _ in results if p.exists()}
            self.assertTrue(all(not changed for _, changed in
                                installer.install(root, ("codex", "claude", "pi"), use_environment=False)))
            self.assertEqual(before, {p: p.read_bytes() for p in before})

    def test_cleanup_handles_both_platforms_and_bare_configs(self):
        for agent, directory, filename in (
                ("codex", ".codex", "hooks.json"), ("claude", ".claude", "settings.json")):
            for bare in (False, True):
                with self.subTest(agent=agent, bare=bare), tempfile.TemporaryDirectory(prefix="sterm-test-") as temp:
                    root = Path(temp)
                    path = root / directory / filename
                    path.parent.mkdir(parents=True)
                    windows_command = '& "C:\\Users\\test\\sTerm-hook.cmd" ' + agent + "-stop"
                    hooks = {event: [
                        {"hooks": [{"type": "command", "command": windows_command}]},
                        {"type": "command", "command": windows_command},
                        {"matcher": "*", "hooks": [
                            {"command": "old/sterm-agent-events.py " + agent + " stop"},
                            {"command": "echo user-" + event}]}
                    ] for event in installer.EVENTS}
                    original = hooks if bare else {"hooks": hooks, "other": True}
                    path.write_text(json.dumps(original), encoding="utf-8")
                    installer.install(root, (agent,), use_environment=False)
                    updated = json.loads(path.read_text(encoding="utf-8"))
                    target = updated if bare else updated["hooks"]
                    self.assertEqual(target, {event: [
                        {"matcher": "*", "hooks": [{"command": "echo user-" + event}]}]
                        for event in installer.EVENTS})
                    if not bare:
                        self.assertTrue(updated["other"])
                    self.assertTrue(all(not changed for _, changed in
                                        installer.install(root, (agent,), use_environment=False)))

    def test_invalid_config_prevents_partial_install(self):
        for invalid in ('{broken', '{"hooks":{"Stop":"wrong"}}', '[]'):
            with self.subTest(invalid=invalid), tempfile.TemporaryDirectory(prefix="sterm-test-") as temp:
                root = Path(temp)
                path = root / ".claude/settings.json"
                path.parent.mkdir(parents=True)
                path.write_text(invalid, encoding="utf-8")
                with self.assertRaises(ValueError):
                    installer.install(root, ("codex", "claude", "pi"), use_environment=False)
                self.assertEqual(path.read_text(encoding="utf-8"), invalid)
                self.assertFalse((root / ".pi").exists())
                self.assertFalse(path.with_name("settings.json.sterm.bak").exists())

    def test_native_agents_do_not_create_or_reformat_configs(self):
        with tempfile.TemporaryDirectory(prefix="sterm-test-") as temp:
            root = Path(temp)
            installer.install(root, ("codex", "claude"), use_environment=False)
            self.assertEqual(list(root.iterdir()), [])
            path = root / ".codex/hooks.json"
            path.parent.mkdir(parents=True)
            original = '  { "hooks": { "Stop": [] }, "other": true }'
            path.write_text(original, encoding="utf-8")
            installer.install(root, ("codex",), use_environment=False)
            self.assertEqual(path.read_text(encoding="utf-8"), original)
            self.assertFalse(path.with_name("hooks.json.sterm.bak").exists())

    def test_custom_config_directories_are_respected(self):
        with tempfile.TemporaryDirectory(prefix="sterm-test-") as temp:
            root = Path(temp)
            custom = root / "custom codex"
            path = custom / "hooks.json"
            custom.mkdir()
            path.write_text('{"Stop":[{"command":"old/sTerm-hook.cmd codex-stop"}]}', encoding="utf-8")
            with patch.dict(os.environ, {"CODEX_HOME": str(custom)}):
                installer.install(root / "default", ("codex",))
            self.assertEqual(json.loads(path.read_text(encoding="utf-8")), {"Stop": []})
            self.assertFalse((root / "default").exists())


if __name__ == "__main__":
    unittest.main()
