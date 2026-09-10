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
        self.checkout = self.root / "deployment's 100% checkout"
        self.git("init", "-b", "main", str(self.origin))
        self.git("-C", str(self.origin), "config", "user.name", "Deployment test")
        self.git("-C", str(self.origin), "config", "user.email", "test@example.invalid")
        (self.origin / ".gitignore").write_text(".env\n.env.*\n!.env.example\n")
        (self.origin / "scripts").mkdir()
        shutil.copy2(SCRIPT, self.origin / "scripts/deploy.sh")
        shutil.copy2(SCRIPT.parents[1] / "auto-deploy.sh", self.origin / "auto-deploy.sh")
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
            "TEST_ORIGIN": str(self.origin),
            "REAL_GIT": GIT,
            "TEST_FAILURE": "",
            "TEST_HEALTH": "ok",
        }
        self.executable("git", '''#!/bin/bash
if [[ "${TEST_FAILURE:-}" == fetch && "$1" == fetch ]]; then exit 1; fi
if [[ "${TEST_FAILURE:-}" == checkout && "$1" == checkout ]]; then exit 1; fi
if [[ "${TEST_FAILURE:-}" == remote && "$1" == ls-remote ]]; then exit 1; fi
if [[ "${TEST_FAILURE:-}" == fetch-advance && "$1" == fetch ]]; then
  "$REAL_GIT" -C "$TEST_ORIGIN" commit --allow-empty -m 'main advanced' >/dev/null
fi
exec "$REAL_GIT" "$@"
''')
        self.executable("docker", '''#!/bin/bash
printf '%s\\n' "$*" >> "$TEST_ROOT/docker.log"
case "$*" in
  "compose build") [[ "${TEST_FAILURE:-}" != build ]] ;;
  "compose up -d --no-build") [[ "${TEST_FAILURE:-}" != up ]] ;;
  "compose ps --all --quiet troute") echo troute-container ;;
  "compose ps --all --quiet testbed") echo testbed-container ;;
  inspect*testbed-container) echo 18081 ;;
  inspect*) echo 18080 ;;
esac
''')
        self.executable("curl", '''#!/bin/bash
printf '%s\\n' "$*" >> "$TEST_ROOT/curl.log"
url="${!#}"
while [[ $# -gt 0 ]]; do
  if [[ "$1" == --output ]]; then output=$2; shift; fi
  shift
done
if [[ "$url" == http://127.0.0.1:18081/ ]]; then
  printf '<html>testbed</html>' > "$output"
  if [[ "${TEST_FAILURE:-}" == testbed-health ]]; then printf 503; else printf 200; fi
  exit 0
fi
case "${TEST_HEALTH:-ok}" in
  ok) printf '{"status":"ok"}' > "$output"; printf 200 ;;
  wrong-body) printf '{"status":"failed"}' > "$output"; printf 200 ;;
  wrong-whitespace) printf '{"status":"o k"}' > "$output"; printf 200 ;;
  bad-status) printf '{"status":"ok"}' > "$output"; printf 503 ;;
  connection) exit 7 ;;
esac
''')
        self.executable("sleep", "#!/bin/bash\nexit 0\n")
        self.executable("crontab", '''#!/bin/bash
if [[ "$1" == -l ]]; then
  if [[ "${TEST_FAILURE:-}" == crontab-read ]]; then
    echo 'permission denied' >&2; exit 1
  fi
  if [[ -f "$TEST_ROOT/crontab" ]]; then cat "$TEST_ROOT/crontab"; else
    echo 'no crontab for test' >&2; exit 1
  fi
else
  cp "$1" "$TEST_ROOT/crontab"
fi
''')

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
        self.assertLess(commands.index("compose build"), commands.index("compose up"))
        self.assertIn("http://127.0.0.1:18080/health", (self.root / "curl.log").read_text())
        self.assertIn("http://127.0.0.1:18081/", (self.root / "curl.log").read_text())
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

    def watch(self, command="--once", **overrides):
        result = subprocess.run(
            ["/bin/bash", str(self.checkout / "auto-deploy.sh"), command],
            env={**self.env, **overrides}, capture_output=True, text=True, timeout=15,
        )
        self.assertNotIn("preserve-me", result.stdout + result.stderr)
        return result

    def state(self, name):
        path = self.checkout / ".git/troute-deploy" / name
        return path.read_text().strip() if path.exists() else ""

    def build_count(self):
        path = self.root / "docker.log"
        return path.read_text().count("compose build") if path.exists() else 0

    def clean_main(self):
        self.git("-C", str(self.checkout), "fetch", "origin")
        self.git("-C", str(self.checkout), "reset", "--hard", "origin/main")

    def test_poll_records_success_and_unchanged_sha_does_not_rebuild(self):
        result = self.watch()
        self.assertEqual(result.returncode, 0, result.stderr + self.state("logs/auto-deploy.log"))
        sha = self.git("-C", str(self.origin), "rev-parse", "HEAD")
        self.assertEqual(self.state("deployed-commit"), sha)
        self.assertEqual(self.state("last-status"), "succeeded")
        self.assertEqual(self.watch().returncode, 0)
        self.assertEqual(self.build_count(), 1)
        self.assertIn("preserve-me", (self.checkout / ".env").read_text())
        self.assertNotIn("preserve-me", self.state("logs/auto-deploy.log"))

    def test_failed_sha_waits_then_retries(self):
        self.assertNotEqual(self.watch(TEST_FAILURE="build").returncode, 0)
        self.assertEqual(self.state("last-status"), "failed")
        self.assertEqual(self.state("deployed-commit"), "")
        self.assertEqual(self.watch(TEST_FAILURE="build").returncode, 0)
        self.assertEqual(self.build_count(), 1)
        (self.checkout / ".git/troute-deploy/next-retry-at").write_text("0\n")
        self.assertEqual(self.watch().returncode, 0)
        self.assertEqual(self.build_count(), 2)
        self.assertEqual(self.state("last-status"), "succeeded")

    def test_new_commit_bypasses_failure_cooldown(self):
        self.assertNotEqual(self.watch(TEST_FAILURE="build").returncode, 0)
        (self.origin / "version").write_text("fixed\n")
        self.commit("fix build")
        self.assertEqual(self.watch().returncode, 0)
        self.assertEqual(self.build_count(), 2)
        self.assertEqual((self.checkout / "version").read_text(), "fixed\n")

    def test_health_failure_does_not_advance_successful_sha(self):
        self.assertEqual(self.watch().returncode, 0)
        deployed = self.state("deployed-commit")
        (self.origin / "version").write_text("unhealthy\n")
        self.commit("unhealthy change")
        self.assertNotEqual(self.watch(TEST_HEALTH="bad-status").returncode, 0)
        self.assertEqual(self.state("deployed-commit"), deployed)
        self.assertEqual(self.state("last-status"), "failed")
        # Returning main to a previously successful SHA must restore it, even
        # though the successful SHA file already contains that value.
        self.git("-C", str(self.origin), "reset", "--hard", deployed)
        self.assertEqual(self.watch().returncode, 0)
        self.assertEqual(self.build_count(), 3)

    def test_testbed_failure_does_not_stop_api_or_record_success(self):
        result = self.watch(TEST_FAILURE="testbed-health")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.state("last-status"), "failed")
        self.assertEqual(self.state("deployed-commit"), "")
        log = self.state("logs/auto-deploy.log")
        self.assertIn("troute /health passed: HTTP 200", log)
        self.assertIn("testbed health check attempt 20/20 failed", log)
        self.assertNotIn("compose down", (self.root / "docker.log").read_text())

    def test_remote_failure_does_not_build_or_mark_deployed(self):
        self.assertNotEqual(self.watch(TEST_FAILURE="remote").returncode, 0)
        self.assertEqual(self.build_count(), 0)
        self.assertEqual(self.state("deployed-commit"), "")

    def test_main_advancing_during_fetch_is_not_misreported(self):
        self.assertEqual(self.watch(TEST_FAILURE="fetch-advance").returncode, 0)
        self.assertEqual(self.state("last-status"), "superseded")
        self.assertEqual(self.state("deployed-commit"), "")
        self.assertEqual(self.build_count(), 0)
        self.assertEqual(self.watch().returncode, 0)
        self.assertEqual(self.state("deployed-commit"), self.git("-C", str(self.origin), "rev-parse", "HEAD"))

    def test_watcher_lock_skips_second_scan(self):
        lock = self.checkout / ".git/troute-deploy/watcher.lock"
        lock.mkdir(parents=True)
        self.assertEqual(self.watch().returncode, 0)
        self.assertEqual(self.build_count(), 0)
        self.assertTrue(lock.exists())

    def test_cron_install_replace_execute_status_uninstall(self):
        self.clean_main()
        cron = self.root / "crontab"
        original = "0 * * * * echo keep-existing # another-watcher\n"
        cron.write_text(original)
        for _ in range(2):
            result = self.watch("--install", TROUTE_AUTO_DEPLOY_RETRY_SEC="600")
            self.assertEqual(result.returncode, 0, result.stderr)
        content = cron.read_text()
        self.assertEqual(content.count("# troute-auto-deploy"), 1)
        self.assertIn(original, content)
        self.assertIn("TROUTE_AUTO_DEPLOY_RETRY_SEC=600", content)
        line = next(line for line in content.splitlines() if line.endswith("# troute-auto-deploy"))
        command = line.split(" ", 5)[5].replace("\\%", "%")
        result = subprocess.run(["/bin/sh", "-c", command], env=self.env, capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 0, result.stderr)
        status = self.watch("--status")
        self.assertIn("Watcher: installed", status.stdout)
        self.assertIn("Last deployment: succeeded", status.stdout)
        self.assertEqual(self.watch("--uninstall").returncode, 0)
        self.assertEqual(cron.read_text(), original)
        self.assertIn("Watcher: not installed", self.watch("--status").stdout)
        self.assertEqual(self.state("last-status"), "succeeded")

    def test_cron_read_failure_never_overwrites_existing_jobs(self):
        cron = self.root / "crontab"
        cron.write_text("keep this crontab\n")
        self.assertNotEqual(self.watch("--uninstall", TEST_FAILURE="crontab-read").returncode, 0)
        self.assertEqual(cron.read_text(), "keep this crontab\n")

    def test_install_refuses_development_branch(self):
        self.git("-C", str(self.checkout), "checkout", "-b", "impl")
        self.assertNotEqual(self.watch("--install").returncode, 0)
        self.assertNotEqual(self.watch().returncode, 0)
        self.assertFalse((self.root / "crontab").exists())

    def test_invalid_retry_configuration_is_rejected(self):
        self.assertNotEqual(self.watch(TROUTE_AUTO_DEPLOY_RETRY_SEC="invalid").returncode, 0)
        self.assertEqual(self.build_count(), 0)


if __name__ == "__main__":
    unittest.main()
