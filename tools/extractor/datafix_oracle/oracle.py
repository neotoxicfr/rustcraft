"""Runs DatafixOracle.java with the classpath of the installed official game (development tool).

Usage: python -I tools/extractor/datafix_oracle/oracle.py <DatafixOracle arguments...>
Game folder: RUSTCRAFT_MC_DIR or %APPDATA%/.minecraft; version: RUSTCRAFT_ORACLE_VERSION or 26.3;
Java: RUSTCRAFT_JAVA, the launcher's runtime, or JAVA_HOME (the same search as rc-data).
"""

import json
import os
import platform
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def game_dir() -> Path:
    env = os.environ.get("RUSTCRAFT_MC_DIR")
    return Path(env) if env else Path(os.environ["APPDATA"]) / ".minecraft"


def launcher_os() -> str:
    return {"Windows": "windows", "Linux": "linux", "Darwin": "osx"}[platform.system()]


def rules_allow(rules) -> bool:
    if not rules:
        return True
    allowed = False
    for rule in rules:
        if "features" in rule:
            continue
        os_rule = rule.get("os", {}).get("name")
        if os_rule is None or os_rule == launcher_os():
            allowed = rule.get("action") == "allow"
    return allowed


def classpath(mc: Path, version: str) -> list[str]:
    vdir = mc / "versions" / version
    meta = json.loads((vdir / f"{version}.json").read_text(encoding="utf-8"))
    cp = [str(vdir / f"{version}.jar")]
    for lib in meta["libraries"]:
        if not rules_allow(lib.get("rules")):
            continue
        path = lib.get("downloads", {}).get("artifact", {}).get("path")
        if path:
            cp.append(str(mc / "libraries" / path))
    return cp


def java(mc: Path) -> str:
    env = os.environ.get("RUSTCRAFT_JAVA")
    if env:
        return env
    exe = "java.exe" if platform.system() == "Windows" else "java"
    plat = "windows-x64" if platform.system() == "Windows" else "linux"
    comp = "java-runtime-epsilon"
    local = os.environ.get("LOCALAPPDATA", "")
    store = Path(local) / "Packages" / "Microsoft.4297127D64EC6_8wekyb3d8bbwe" / "LocalCache" / "Local" / "runtime"
    for root in (store, mc / "runtime"):
        p = root / comp / plat / comp / "bin" / exe
        if p.is_file():
            return str(p)
    home = os.environ.get("JAVA_HOME")
    if home:
        return str(Path(home) / "bin" / exe)
    raise SystemExit("no Java runtime found")


def main() -> None:
    mc = game_dir()
    version = os.environ.get("RUSTCRAFT_ORACLE_VERSION", "26.3")
    cp = os.pathsep.join(classpath(mc, version))
    cmd = [java(mc), "-cp", cp, str(HERE / "DatafixOracle.java"), *sys.argv[1:]]
    sys.exit(subprocess.run(cmd, stdin=subprocess.DEVNULL).returncode)


if __name__ == "__main__":
    main()
