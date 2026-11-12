#!/usr/bin/env python3
"""安装 Pi 状态扩展，并清理已由原生协议替代的 Claude/Codex 状态 Hook。"""
import argparse
import copy
import json
import os
from pathlib import Path
import tempfile

MANAGED_MARKERS = ("sterm-agent-events.py", "sTerm-hook.cmd")
EVENTS = ("SessionStart", "UserPromptSubmit", "Stop", "SessionEnd")


def is_managed(hook):
    if not isinstance(hook, dict):
        return False
    command = str(hook.get("command", ""))
    return any(marker in command for marker in MANAGED_MARKERS)


def remove_hooks(original):
    root = copy.deepcopy(original)
    if not isinstance(root, dict):
        raise ValueError("配置根节点必须是 JSON 对象")
    hooks = root if any(event in root for event in EVENTS) else root.get("hooks", {})
    if not isinstance(hooks, dict):
        raise ValueError("hooks 必须是 JSON 对象")
    for event in EVENTS:
        if event not in hooks:
            continue
        entries = hooks[event]
        if not isinstance(entries, list):
            raise ValueError(event + " 必须是数组")
        preserved = []
        for entry in entries:
            if is_managed(entry):
                continue
            if isinstance(entry, dict) and isinstance(entry.get("hooks"), list):
                remaining = [item for item in entry["hooks"] if not is_managed(item)]
                if len(remaining) != len(entry["hooks"]):
                    if not remaining:
                        continue
                    entry = dict(entry, hooks=remaining)
            preserved.append(entry)
        hooks[event] = preserved
    return root


def read_config(path):
    with path.open(encoding="utf-8-sig") as stream:
        return json.load(stream)


def write_changed(path, data):
    if path.exists() and path.read_bytes() == data:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        backup = path.with_name(path.name + ".sterm.bak")
        if not backup.exists():
            backup.write_bytes(path.read_bytes())
    temp_path = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".sterm-", delete=False) as stream:
            temp_path = Path(stream.name)
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp_path, path)
        temp_path = None
    finally:
        if temp_path is not None:
            temp_path.unlink(missing_ok=True)
    return True


def install(home_dir, agents, use_environment=True):
    source = Path(__file__).resolve().parent
    # 先读取并校验全部配置，格式错误时不写入任何安装文件。
    plans = []
    for agent in agents:
        if agent == "pi":
            default_dir = home_dir / ".pi" / "agent"
            agent_dir = Path(os.environ.get("PI_CODING_AGENT_DIR", default_dir)) if use_environment else default_dir
            plans.append((agent_dir / "extensions" / "sterm-events.ts",
                          (source / "sterm-events.ts").read_bytes()))
            continue
        if agent not in ("codex", "claude"):
            raise ValueError("未知 Agent：" + agent)
        env_name, default_dir, filename = (
            ("CODEX_HOME", home_dir / ".codex", "hooks.json") if agent == "codex"
            else ("CLAUDE_CONFIG_DIR", home_dir / ".claude", "settings.json"))
        config_dir = Path(os.environ.get(env_name, default_dir)) if use_environment else default_dir
        path = config_dir / filename
        data = None
        if path.exists():
            original = read_config(path)
            cleaned = remove_hooks(original)
            if cleaned != original:
                data = (json.dumps(cleaned, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
        plans.append((path, data))
    return [(path, write_changed(path, data) if data is not None else False) for path, data in plans]


def main():
    parser = argparse.ArgumentParser(description="安装 Pi 状态扩展并清理 Claude/Codex 旧状态 Hook")
    parser.add_argument("--agent", choices=("all", "codex", "claude", "pi"), default="all")
    parser.add_argument("--home", type=Path, help="指定隔离主目录，不读取 Agent 配置目录环境变量")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("请在 WSL/Linux/macOS 或 SSH 远端环境内运行；Windows 请在 sTerm 的“设置 → 集成”中手动安装")
    agents = ("codex", "claude", "pi") if args.agent == "all" else (args.agent,)
    try:
        for path, changed in install((args.home or Path.home()).expanduser().resolve(),
                                     agents, use_environment=args.home is None):
            print(("已更新：" if changed else "无需更改：") + str(path))
    except (OSError, ValueError) as error:
        parser.exit(1, "接入失败，原配置未被错误 JSON 覆盖：" + str(error) + "\n")
    print("Claude/Codex 使用 OSC 7501，无需 sTerm 状态 Hook。请退出并重新启动 Agent。")


if __name__ == "__main__":
    main()
