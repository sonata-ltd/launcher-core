#!/usr/bin/env python3
"""Apply source-only consumer patches and verify an isolated GUI snapshot."""
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("gui", type=Path, help="real GUI source tree (never modified)")
    parser.add_argument("--config", type=Path, help="external Cargo config, e.g. local Luminate overrides")
    parser.add_argument("--target-dir", type=Path, help="optional external build cache")
    parser.add_argument("--checks", nargs="+", choices=["fmt", "check", "test", "build"], default=["fmt", "check", "test", "build"])
    args = parser.parse_args()
    source = args.gui.resolve(strict=True)
    core = Path(__file__).resolve().parents[2]
    # Include current user edits/untracked sources, but not ignored build data.
    files = subprocess.check_output(["git", "ls-files", "-c", "-o", "--exclude-standard", "-z"], cwd=source).split(b"\0")
    snapshot = Path(tempfile.mkdtemp(prefix="core-gui-verify.", dir="/tmp/opencode"))
    for raw in set(files):
        if not raw:
            continue
        relative = Path(os.fsdecode(raw))
        original = source / relative
        if relative.is_absolute() or ".." in relative.parts or not original.resolve().is_relative_to(source):
            raise RuntimeError(f"unsafe source entry: {relative}")
        if original.is_file():
            destination = snapshot / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(original, destination)
    print(f"Snapshot: {snapshot}", flush=True)
    for patch in sorted(Path(__file__).parent.glob("*.patch")):
        subprocess.run(["git", "apply", "--check", str(patch)], cwd=snapshot, check=True)
        subprocess.run(["git", "apply", str(patch)], cwd=snapshot, check=True)
    manifest = snapshot / "Cargo.toml"
    text = manifest.read_text()
    import json
    text, count = re.subn(r'^sonata_launcher_core\s*=.*$', "sonata_launcher_core = { path = " + json.dumps(str(core)) + " }", text, flags=re.MULTILINE)
    if count != 1:
        raise RuntimeError("expected one single-line core dependency in the GUI manifest")
    manifest.write_text(text)
    environment = dict(os.environ, SQLX_OFFLINE="true")
    if args.target_dir:
        environment["CARGO_TARGET_DIR"] = str(args.target_dir.resolve())
    options = ["--offline"]
    if args.config:
        options += ["--config", str(args.config.resolve(strict=True))]
    # Only the snapshot lockfile is allowed to change for the new dependency.
    commands = [("resolve", ["cargo", "check", *options])]
    for check in args.checks:
        command = ["cargo", "fmt", "--check"] if check == "fmt" else ["cargo", check, "--locked", *options]
        commands.append((check, command))
    for label, command in commands:
        log = snapshot / f"verification-{label}.log"
        with log.open("w") as output:
            result = subprocess.run(command, cwd=snapshot, env=environment, stdout=output, stderr=subprocess.STDOUT)
        print(f"{label}: {'PASS' if result.returncode == 0 else 'FAIL'} — {log}", flush=True)
        if result.returncode:
            raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
