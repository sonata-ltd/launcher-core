#!/usr/bin/env python3
"""Verify core on a native runner, or compile (never execute) for a cross target."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
from datetime import datetime, timezone


def commands(root, target=None, offline=False):
    manifest = ["--manifest-path", str(root / "Cargo.toml")]
    options = [*manifest, "--locked"]
    if offline:
        options.append("--offline")
    if target:
        options += ["--target", target]
    checks = [("fmt", ["cargo", "fmt", *manifest, "--check"]),
              ("check", ["cargo", "check", *options, "--all-targets"])]
    if target:
        checks.append(("test-compile", ["cargo", "test", *options, "--no-run"]))
    else:
        checks.append(("test", ["cargo", "test", *options]))
    checks.append(("build", ["cargo", "build", *options]))
    return checks


def environment(root, parent):
    env = dict(parent)
    for name in ["CURSEFORGE_API_KEY", "DATABASE_URL", "SONATA_TEST_CONTROLLED_CHILD"]:
        env.pop(name, None)
    env["SQLX_OFFLINE"] = "true"
    env.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    # The local Arch host keeps the compatible OpenSSL ABI outside /usr/lib.
    if Path("/usr/include/openssl-3").is_dir() and Path("/usr/lib/openssl-3").is_dir():
        env.setdefault("OPENSSL_INCLUDE_DIR", "/usr/include/openssl-3")
        env.setdefault("OPENSSL_LIB_DIR", "/usr/lib/openssl-3")
    return env


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def version(command, env):
    try:
        return subprocess.check_output(command, env=env, text=True, stderr=subprocess.STDOUT,
                                       timeout=30).strip()
    except (OSError, subprocess.SubprocessError) as error:
        return f"unavailable: {type(error).__name__}"


def verify(root, output, target=None, offline=False, timeout=900):
    # Refuse reuse rather than overwrite evidence from another invocation.
    output.mkdir(parents=True, exist_ok=False)
    env = environment(root, os.environ)
    lock = root / "Cargo.lock"
    before = digest(lock)
    report = {
        "started_at": datetime.now(timezone.utc).isoformat(),
        "scope": "cross-compile-only" if target else "native-core",
        "target": target, "platform": platform.platform(),
        "core_head": version(["git", "-C", str(root), "rev-parse", "HEAD"], env),
        "core_status": version(["git", "-C", str(root), "status", "--porcelain=v1"], env),
        "rustc": version(["rustc", "-vV"], env),
        "cargo": version(["cargo", "--version"], env),
        "offline": offline, "lock_before_sha256": before,
        "gui_executed": False, "checks": [],
    }
    failed = False
    # Cargo starts outside the repository/GUI config hierarchy. SQLx resolves
    # checked-in metadata via the package manifest, not a live/user database.
    with tempfile.TemporaryDirectory(prefix="core-platform-cwd-") as scratch:
        for label, command in commands(root, target, offline):
            result = {"name": label, "command": command, "status": "not-run"}
            report["checks"].append(result)
            if failed:
                continue
            log = output / f"{label}.log"
            result["log"] = str(log)
            with log.open("w") as stream:
                try:
                    process = subprocess.run(command, cwd=scratch, env=env, stdout=stream,
                                             stderr=subprocess.STDOUT, timeout=timeout)
                    result["exit_code"] = process.returncode
                    result["status"] = "pass" if process.returncode == 0 else "fail"
                except (OSError, subprocess.TimeoutExpired) as error:
                    result["status"] = "timeout" if isinstance(error, subprocess.TimeoutExpired) else "error"
                    result["error"] = str(error)
                    stream.write(f"\nVerification runner: {error}\n")
            failed = result["status"] != "pass"
            print(f"{label}: {result['status']} — {log}", flush=True)
    report["lock_after_sha256"] = digest(lock)
    report["lock_unchanged"] = report["lock_after_sha256"] == before
    report["status"] = "fail" if failed or not report["lock_unchanged"] else "pass"
    report["finished_at"] = datetime.now(timezone.utc).isoformat()
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"{report['scope']}: {report['status']} — {output / 'report.json'}", flush=True)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new evidence directory")
    parser.add_argument("--target", help="compile-only mode; never run target tests/binaries")
    parser.add_argument("--offline", action="store_true", help="use only installed dependencies")
    parser.add_argument("--timeout", type=int, default=900, help="seconds per Cargo command")
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    root = Path(__file__).resolve().parents[1]
    report = verify(root, args.output.resolve(), args.target, args.offline, args.timeout)
    raise SystemExit(0 if report["status"] == "pass" else 1)


if __name__ == "__main__":
    main()
