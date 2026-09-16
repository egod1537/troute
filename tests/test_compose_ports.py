"""Static regression checks for the host/container port boundary."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ComposePortTests(unittest.TestCase):
    def test_troute_uses_safe_loopback_host_default(self):
        compose = (ROOT / "compose.yaml").read_text()
        self.assertIn(
            '"127.0.0.1:${TROUTE_HOST_PORT:-18080}:${TROUTE_PORT:-8080}"',
            compose,
        )
        self.assertNotIn("0.0.0.0:${TROUTE_HOST_PORT", compose)

    def test_example_keeps_container_port_and_changes_only_host_port(self):
        example = (ROOT / ".env.example").read_text().splitlines()
        self.assertIn("TROUTE_PORT=8080", example)
        self.assertIn("TROUTE_HOST_PORT=18080", example)


if __name__ == "__main__":
    unittest.main()
