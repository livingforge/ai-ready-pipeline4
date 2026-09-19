"""Install a verified ARP bundle into a project's private runtime (stdlib only)."""
from __future__ import annotations

import argparse
from email.parser import BytesParser
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import venv
from zipfile import ZipFile


def python_in(runtime: Path) -> Path:
    return runtime / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def environment() -> list[str]:
    return [sys.implementation.name, *platform.python_version_tuple()[:2],
            sys.platform, platform.machine().lower()]


def lock_wheels(wheels: Path, destination: Path) -> None:
    requirements = []
    for path in sorted(wheels.glob("*.whl")):
        with ZipFile(path) as archive:
            metadata = next(n for n in archive.namelist() if n.endswith(".dist-info/METADATA"))
            info = BytesParser().parsebytes(archive.read(metadata))
        requirements.append(f"{info['Name']}=={info['Version']} --hash=sha256:{hashlib.sha256(path.read_bytes()).hexdigest()}")
    if not requirements:
        raise ValueError("no wheels were downloaded")
    destination.write_text("\n".join(requirements) + "\n", encoding="utf-8")


def verify(bundle: Path, mode: str = "offline") -> dict:
    manifest = json.loads((bundle / "manifest.json").read_text(encoding="utf-8"))
    if mode not in ("online", "offline"):
        raise ValueError("mode must be online or offline")
    if sys.version_info < (3, 11):
        raise ValueError("Python 3.11 or newer is required")
    if mode == "offline" and manifest["environment"] != environment():
        raise ValueError(f"bundle requires {manifest['environment']}; current Python is {environment()}; use --mode online or a matching offline bundle")
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


def install(bundle: Path, root: Path, *, mode: str = "offline", index_url: str | None = None,
            lock: Path | None = None) -> None:
    verify(bundle, mode)
    if mode != "online" and (index_url or lock):
        raise ValueError("--index-url and --lock require --mode online")
    if lock is not None:
        lock = lock.resolve()
        if not lock.is_file():
            raise ValueError(f"lock file not found: {lock}")
    root = root.resolve()
    arp = root / ".arp"
    runtime = arp / "runtime"
    for target in (arp, runtime, arp / "installations", root / "knowledge"):
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
        if prior.get("owner") != "arp4-installer" or prior.get("environment") != environment():
            raise ValueError("existing ARP runtime uses a different environment")
    runtime.mkdir(parents=True, exist_ok=True)
    marker.write_text(json.dumps({"owner": "arp4-installer", "environment": environment()}), encoding="utf-8")
    venv.EnvBuilder(with_pip=True).create(runtime)
    python = python_in(runtime)
    env = {**os.environ, "PYTHONIOENCODING": "utf-8", "PYTHONNOUSERSITE": "1"}
    env.pop("PYTHONPATH", None)
    with tempfile.TemporaryDirectory(prefix="arp-install-") as temp:
        wheels, requirements = bundle / "wheels", bundle / "requirements.lock"
        if mode == "online":
            wheels = Path(temp) / "wheels"
            wheels.mkdir()
            application = list((bundle / "wheels").glob("ai_ready_pipeline4-*-py3-none-any.whl"))
            if len(application) != 1:
                raise ValueError("bundle must contain one portable ARP wheel")
            download = [str(python), "-m", "pip", "--isolated", "download", "--only-binary=:all:", "--dest", str(wheels)]
            if index_url:
                download += ["--index-url", index_url]
            if lock:
                download += ["--find-links", str(bundle / "wheels"), "--require-hashes", "-r", str(lock)]
            else:
                download += [f"{application[0]}[parse,writeback]"]
            subprocess.run(download, check=True, env=env)
            # A supplied lock cannot substitute another version/build of ARP.
            if not (wheels / application[0].name).is_file() or (wheels / application[0].name).read_bytes() != application[0].read_bytes():
                raise ValueError("downloaded ARP does not match this distribution")
            requirements = Path(temp) / "requirements.lock"
            lock_wheels(wheels, requirements)
        subprocess.run([str(python), "-m", "pip", "--isolated", "install", "--no-index",
                        "--find-links", str(wheels), "--require-hashes", "--force-reinstall",
                        "-r", str(requirements)], check=True, env=env)
        # Keep the exact selected dependencies for audit/replay (outside ignored runtime).
        record = arp / "installations" / ("-".join(environment()))
        record.mkdir(parents=True, exist_ok=True)
        checksum = hashlib.sha256(requirements.read_bytes()).hexdigest()
        (record / f"{checksum}.lock").write_bytes(requirements.read_bytes())
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
    parser.add_argument("--mode", choices=("offline", "online"), default="offline")
    parser.add_argument("--index-url", help="package index or internal mirror (online only)")
    parser.add_argument("--lock", type=Path, help="replay a hash-locked dependency file (online only)")
    args = parser.parse_args()
    try:
        install(Path(__file__).resolve().parent, args.root, mode=args.mode, index_url=args.index_url, lock=args.lock)
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"ARP installation failed: {exc}\n")


if __name__ == "__main__":
    main()
