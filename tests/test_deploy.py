"""Deployment regression checks using disposable Git repos and fake Docker/HTTP.

Run with: python3 -m unittest discover -s tests -p 'test_deploy.py' -v
"""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "deploy.sh"
GIT = shutil.which("git")


class DeploymentTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="troute-deploy-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.origin = self.root / "origin"
        self.checkout = self.root / "deployment's checkout"
        self.git("init", "-b", "main", str(self.origin))
        self.git("-C", str(self.origin), "config", "user.name", "Deployment test")
        self.git("-C", str(self.origin), "config", "user.email", "test@example.invalid")
        (self.origin / ".gitignore").write_text(".env\n.env.*\n!.env.example\n")
        (self.origin / "version").write_text("old\n")
        self.commit("initial")
        self.git("clone", str(self.origin), str(self.checkout))
        (self.origin / "version").write_text("new\n")
        self.commit("new main")
        (self.checkout / ".env").write_text("TROUTE_HOST_PORT=18080\nTEST_SECRET=preserve-me\n")
        (self.checkout / "version").write_text("local edit\n")

        self.bin = self.root / "bin"
        self.bin.mkdir(parents=True)
        self.env = {
            **os.environ,
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "TEST_ROOT": str(self.root),
            "REAL_GIT": GIT,
            "TEST_FAILURE": "",
            "TEST_HEALTH": "ok",
        }
        self.executable("git", '''#!/bin/bash
if [[ "${TEST_FAILURE:-}" == fetch && "$1" == fetch ]]; then exit 1; fi
if [[ "${TEST_FAILURE:-}" == checkout && "$1" == checkout ]]; then exit 1; fi
exec "$REAL_GIT" "$@"
''')
        self.executable("docker", '''#!/bin/bash
printf '%s\\n' "$*" >> "$TEST_ROOT/docker.log"
case "$*" in
  "compose build troute") [[ "${TEST_FAILURE:-}" != build ]] ;;
  "compose up -d --no-build troute") [[ "${TEST_FAILURE:-}" != up ]] ;;
  "compose ps --all --quiet troute") echo test-container ;;
  inspect*) echo 18080 ;;
esac
''')
        self.executable("curl", '''#!/bin/bash
printf '%s\\n' "$*" >> "$TEST_ROOT/curl.log"
while [[ $# -gt 0 ]]; do
  if [[ "$1" == --output ]]; then output=$2; shift; fi
  shift
done
case "${TEST_HEALTH:-ok}" in
  ok) printf '{"status":"ok"}' > "$output"; printf 200 ;;
  wrong-body) printf '{"status":"failed"}' > "$output"; printf 200 ;;
  wrong-whitespace) printf '{"status":"o k"}' > "$output"; printf 200 ;;
  bad-status) printf '{"status":"ok"}' > "$output"; printf 503 ;;
  connection) exit 7 ;;
esac
''')
        self.executable("sleep", "#!/bin/bash\nexit 0\n")

    def git(self, *args):
        return subprocess.run(
            [GIT, *args], check=True, capture_output=True, text=True
        ).stdout.strip()

    def commit(self, message):
        self.git("-C", str(self.origin), "add", ".")
        self.git("-C", str(self.origin), "commit", "-m", message)

    def executable(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def deploy(self, **overrides):
        result = subprocess.run(
            ["/bin/bash", str(SCRIPT), str(self.checkout)],
            env={**self.env, **overrides}, capture_output=True, text=True, timeout=15,
        )
        self.assertNotIn("preserve-me", result.stdout + result.stderr)
        self.assertEqual(
            (self.checkout / ".env").read_text(),
            "TROUTE_HOST_PORT=18080\nTEST_SECRET=preserve-me\n",
        )
        return result

    def test_updates_main_preserves_env_and_checks_actual_port(self):
        result = self.deploy()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.checkout / "version").read_text(), "new\n")
        self.assertEqual(self.git("-C", str(self.checkout), "branch", "--show-current"), "main")
        commands = (self.root / "docker.log").read_text()
        self.assertLess(commands.index("compose build troute"), commands.index("compose up"))
        self.assertIn("http://127.0.0.1:18080/health", (self.root / "curl.log").read_text())
        self.assertFalse((self.checkout / ".git/troute-deploy.lock").exists())

    def test_git_failures_stop_before_build(self):
        for failure in ("fetch", "checkout"):
            with self.subTest(failure=failure):
                result = self.deploy(TEST_FAILURE=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("compose build", (self.root / "docker.log").read_text())

    def test_build_failure_keeps_existing_container(self):
        result = self.deploy(TEST_FAILURE="build")
        self.assertNotEqual(result.returncode, 0)
        commands = (self.root / "docker.log").read_text()
        self.assertNotIn("compose up", commands)
        self.assertNotIn("compose down", commands)
        self.assertFalse((self.root / "curl.log").exists())

    def test_start_failure_does_not_report_healthy(self):
        self.assertNotEqual(self.deploy(TEST_FAILURE="up").returncode, 0)
        self.assertFalse((self.root / "curl.log").exists())

    def test_health_failures_are_bounded_and_fail_deployment(self):
        for health in ("wrong-body", "wrong-whitespace", "bad-status", "connection"):
            with self.subTest(health=health):
                (self.root / "curl.log").write_text("")
                result = self.deploy(TEST_HEALTH=health)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("20/20 failed", result.stdout)
                self.assertEqual(len((self.root / "curl.log").read_text().splitlines()), 20)

    def test_tracked_remote_env_is_rejected_before_reset(self):
        (self.origin / ".env").write_text("must-not-replace-runtime-config\n")
        self.git("-C", str(self.origin), "add", "--force", ".env")
        self.git("-C", str(self.origin), "commit", "-m", "accidentally track env")
        result = self.deploy()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refusing to overwrite", result.stderr)
        self.assertEqual((self.checkout / "version").read_text(), "local edit\n")

    def test_existing_lock_refuses_concurrent_deployment(self):
        lock = self.checkout / ".git/troute-deploy.lock"
        lock.mkdir()
        self.assertNotEqual(self.deploy().returncode, 0)
        self.assertTrue(lock.exists())
        self.assertNotIn("compose build", (self.root / "docker.log").read_text())


if __name__ == "__main__":
    unittest.main()
