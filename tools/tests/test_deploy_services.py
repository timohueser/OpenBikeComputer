import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).parents[2]


class DeployServicesTests(unittest.TestCase):
    def test_a_dry_run_prints_the_build_of_the_commit_and_never_connects(self):
        with tempfile.TemporaryDirectory() as bin_dir:
            ssh = Path(bin_dir) / "ssh"
            ssh.write_text("#!/bin/sh\nexit 99\n")
            ssh.chmod(0o755)
            env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}"}
            run = subprocess.run(["bash", "tools/vps/deploy-services.sh", "--dry-run", "vps.test", "HEAD"],
                                 cwd=ROOT, env=env, capture_output=True, text=True, check=True)
        commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True,
                                check=True).stdout.strip()
        self.assertIn(f"git archive --format=tar {commit} | ssh vps.test", run.stdout)
        self.assertIn(f"cd /opt/obc-data/{commit}\n", run.stdout)
        self.assertIn("cargo build --locked --release --bin obc-data-plumbing --bin planner-service", run.stdout)
        self.assertIn(f"ln -sfn /opt/obc-data/{commit} /opt/obc-data/current.next", run.stdout)
        self.assertIn("systemctl restart obc-data-serve", run.stdout)


if __name__ == "__main__":
    unittest.main()
