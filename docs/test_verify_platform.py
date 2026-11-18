"""Runner contracts; mocks never count as native Rust/platform evidence."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("verify_platform", Path(__file__).with_name("verify-platform.py"))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class PlatformRunnerTests(unittest.TestCase):
    def test_native_includes_actual_tests_and_cross_never_executes_them(self):
        native = dict(runner.commands(Path("/fixture")))
        self.assertIn("test", native)
        self.assertNotIn("--no-run", native["test"])
        cross = dict(runner.commands(Path("/fixture"), "x86_64-pc-windows-gnu", True))
        self.assertNotIn("test", cross)
        self.assertIn("--no-run", cross["test-compile"])
        for name in ["check", "test-compile", "build"]:
            for option in ["--locked", "--offline", "--target"]:
                self.assertIn(option, cross[name])
        self.assertIn("--all-targets", cross["check"])

    def test_environment_excludes_credentials_database_and_fixture_controls(self):
        original = {"CURSEFORGE_API_KEY": "secret", "DATABASE_URL": "user.db",
                    "SONATA_TEST_CONTROLLED_CHILD": "user-dir", "PATH": "compiler-bin"}
        env = runner.environment(Path("/fixture"), original)
        for name in ["CURSEFORGE_API_KEY", "DATABASE_URL", "SONATA_TEST_CONTROLLED_CHILD"]:
            self.assertNotIn(name, env)
        self.assertEqual(env["SQLX_OFFLINE"], "true")
        self.assertEqual(env["PATH"], original["PATH"])
        self.assertEqual(original["CURSEFORGE_API_KEY"], "secret")

    def exercise(self, effect, target=None):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.lock").write_text("preserve lock")
            output = root / "evidence"
            with patch.object(runner, "version", return_value="fixture"), \
                 patch.object(runner.platform, "platform", return_value="fixture-os"), \
                 patch.object(runner.subprocess, "run", side_effect=effect) as run, \
                 contextlib.redirect_stdout(io.StringIO()):
                report = runner.verify(root, output, target, True, 1)
            self.assertTrue((output / "report.json").is_file())
            for call in run.call_args_list:
                self.assertFalse(Path(call.kwargs["cwd"]).is_relative_to(root))
            self.assertFalse(report["gui_executed"])
            return report, run.call_count

    def test_failure_retains_logs_and_does_not_claim_later_checks(self):
        report, count = self.exercise(lambda *a, **kw: subprocess.CompletedProcess(a[0], 2))
        self.assertEqual(count, 1)
        self.assertEqual(report["status"], "fail")
        self.assertEqual([c["status"] for c in report["checks"]], ["fail", "not-run", "not-run", "not-run"])

    def test_timeout_is_failure_not_a_pass(self):
        def expire(command, **kwargs):
            raise subprocess.TimeoutExpired(command, 1)
        report, count = self.exercise(expire)
        self.assertEqual(count, 1)
        self.assertEqual(report["status"], "fail")
        self.assertEqual(report["checks"][0]["status"], "timeout")

    def test_cross_report_is_explicitly_compile_only(self):
        report, count = self.exercise(lambda *a, **kw: subprocess.CompletedProcess(a[0], 0), "fixture-target")
        self.assertEqual(count, 4)
        self.assertEqual(report["scope"], "cross-compile-only")
        self.assertEqual(report["status"], "pass")
        self.assertTrue(report["lock_unchanged"])

    def test_changed_lock_fails_without_reverting_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            lock = root / "Cargo.lock"
            lock.write_text("original")
            def change(command, **kwargs):
                lock.write_text("unexpected change")
                return subprocess.CompletedProcess(command, 0)
            with patch.object(runner, "version", return_value="fixture"), \
                 patch.object(runner.platform, "platform", return_value="fixture-os"), \
                 patch.object(runner.subprocess, "run", side_effect=change), \
                 contextlib.redirect_stdout(io.StringIO()):
                report = runner.verify(root, root / "evidence")
            self.assertEqual(report["status"], "fail")
            self.assertFalse(report["lock_unchanged"])
            self.assertEqual(lock.read_text(), "unexpected change")

    def test_existing_evidence_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(FileExistsError):
                runner.verify(Path(tmp), Path(tmp))

    def test_missing_compiler_is_recorded_as_error(self):
        def missing(command, **kwargs):
            raise FileNotFoundError("cargo not installed")
        report, count = self.exercise(missing)
        self.assertEqual(count, 1)
        self.assertEqual(report["status"], "fail")
        self.assertEqual(report["checks"][0]["status"], "error")


if __name__ == "__main__":
    unittest.main()
