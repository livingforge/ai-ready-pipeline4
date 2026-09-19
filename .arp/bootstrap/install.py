"""Install a verified ARP bundle into a project's private runtime (stdlib only)."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import venv


def python_in(runtime: Path) -> Path:
    return runtime / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def verify(bundle: Path) -> dict:
    manifest = json.loads((bundle / "manifest.json").read_text(encoding="utf-8"))
    expected = [sys.implementation.name, *platform.python_version_tuple()[:2],
                sys.platform, platform.machine().lower()]
    if manifest["environment"] != expected:
        raise ValueError(f"bundle requires {manifest['environment']}; current Python is {expected}")
    for name, checksum in manifest["files"].items():
        path = (bundle / name).resolve()
        if not path.is_relative_to(bundle.resolve()) or not path.is_file():
            raise ValueError(f"invalid/missing bundle file: {name}")
        if hashlib.sha256(path.read_bytes()).hexdigest() != checksum:
            raise ValueError(f"bundle checksum mismatch: {name}")
    actual = {p.relative_to(bundle).as_posix() for p in (bundle / "wheels").glob("*.whl")}
    expected_wheels = {name for name in manifest["files"] if name.startswith("wheels/")}
    if actual != expected_wheels:
        raise ValueError("wheel set differs from manifest")
    return manifest


def install(bundle: Path, root: Path) -> None:
    manifest = verify(bundle)
    root = root.resolve()
    arp = root / ".arp"
    runtime = arp / "runtime"
    for target in (arp, runtime, root / "knowledge"):
        if target.is_symlink() or not target.resolve().is_relative_to(root):
            raise ValueError(f"installation path must stay inside project: {target}")
    if (arp / "documents.yml").exists() and not (arp / "config.yml").exists():
        raise ValueError("legacy document layout: migrate with documents upgrade-layout first")
    if not (arp / "config.yml").exists() and (root / "knowledge").exists() and any((root / "knowledge").iterdir()):
        raise ValueError("knowledge/ is not empty; initialize/migrate ARP explicitly first")
    marker = runtime / ".arp-runtime.json"
    if runtime.exists() and any(runtime.iterdir()) and not marker.exists():
        raise ValueError("existing .arp/runtime is not owned by this installer")
    if marker.exists():
        prior = json.loads(marker.read_text(encoding="utf-8"))
        if prior.get("owner") != "arp4-installer" or prior.get("environment") != manifest["environment"]:
            raise ValueError("existing ARP runtime uses a different environment")
    runtime.mkdir(parents=True, exist_ok=True)
    marker.write_text(json.dumps({"owner": "arp4-installer", "environment": manifest["environment"]}), encoding="utf-8")
    venv.EnvBuilder(with_pip=True).create(runtime)
    python = python_in(runtime)
    env = {**os.environ, "PYTHONIOENCODING": "utf-8", "PYTHONNOUSERSITE": "1"}
    env.pop("PYTHONPATH", None)
    subprocess.run([str(python), "-m", "pip", "--isolated", "install", "--no-index",
                    "--find-links", str(bundle / "wheels"), "--require-hashes", "--force-reinstall",
                    "-r", str(bundle / "requirements.lock")], check=True, env=env)
    subprocess.run([str(python), "-m", "pip", "check"], check=True, env=env)
    command = [str(python), "-m", "arp4", "documents", "init", "--root", str(root)]
    # Keep a custom knowledge directory on repeated installation.
    if (arp / "config.yml").exists():
        query = "from arp4.documents.store import Store; import sys; print(Store(__import__('pathlib').Path(sys.argv[1])).config['directory'])"
        directory = subprocess.check_output([str(python), "-c", query, str(root)], env=env, text=True, encoding="utf-8").strip()
        command += ["--directory", directory]
    subprocess.run(command, check=True, env=env)
    print(f"ARP installed: {root}\nRun: {python} -m arp4 documents --help")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True, help="target application project")
    args = parser.parse_args()
    try:
        install(Path(__file__).resolve().parent, args.root)
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"ARP installation failed: {exc}\n")


if __name__ == "__main__":
    main()
