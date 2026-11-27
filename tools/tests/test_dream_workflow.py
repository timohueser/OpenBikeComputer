"""The guards of the weekly dream job, read from its parsed workflow."""

import re
import unittest
from pathlib import Path

import yaml

WORKFLOW = Path(__file__).parents[2] / ".github/workflows/dream.yml"
TOOLS = "Read,Grep,Glob,Edit,Write,Agent"
DENIED = "Bash,WebFetch,WebSearch"


def text(value) -> str:
    return yaml.safe_dump(value)


class DreamWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = yaml.safe_load(WORKFLOW.read_text())
        cls.jobs = cls.workflow["jobs"]
        cls.steps = cls.jobs["dream"]["steps"]
        dreams = [i for i, step in enumerate(cls.steps) if "claude -p" in step.get("run", "")]
        assert len(dreams) == 1, "the job dream must have exactly one step that runs claude"
        cls.index = dreams[0]
        cls.dream = cls.steps[cls.index]

    def test_only_manual_trigger(self) -> None:
        # PyYAML reads the key `on` as True.
        self.assertEqual(list(self.workflow[True]), ["workflow_dispatch"], "a trigger other than workflow_dispatch")

    def test_api_key_is_the_only_secret_of_the_dream_job(self) -> None:
        for i, step in enumerate(self.steps):
            secrets = set(re.findall(r"secrets\.(\w+)", text(step)))
            expected = {"DREAM_ANTHROPIC_API_KEY"} if i == self.index else set()
            self.assertEqual(secrets, expected, f"step {i} of the job dream reads secrets {secrets}")
        job = {key: value for key, value in self.jobs["dream"].items() if key != "steps"}
        self.assertNotIn("secrets.", text(job), "the job dream sets a secret outside its steps")

    def test_dream_step_has_no_github_token(self) -> None:
        self.assertNotIn("GH_TOKEN", text(self.dream), "the dream step has GH_TOKEN")
        self.assertNotIn("github.token", text(self.dream), "the dream step reads github.token")

    def test_dream_command_is_confined(self) -> None:
        args = self.dream["run"].split()
        for flag, value in [("--tools", TOOLS), ("--disallowedTools", DENIED)]:
            self.assertIn(flag, args, f"the dream command lacks {flag}")
            self.assertEqual(args[args.index(flag) + 1], value, f"the dream command changed {flag}")
        self.assertIn("--restricted", args, "the dream command lacks --restricted")
        for flag, value in [("--model", "opus"), ("--permission-prompts", "none")]:
            self.assertIn(flag, args, f"the dream command lacks {flag}")
            self.assertEqual(args[args.index(flag) + 1], value, f"the dream command changed {flag}")
        self.assertIn("--max-budget-usd", args, "the dream command has no cost cap")
        cap = args[args.index("--max-budget-usd") + 1]
        self.assertRegex(cap, r"^\d+(\.\d+)?$", "the cost cap of the dream command is not a number")
        env = self.dream.get("env", {})
        self.assertEqual(env.get("CLAUDE_CODE_SUBAGENT_MODEL"), "opus", "the subagents of the dream step run another model")
        self.assertEqual(env.get("CLAUDE_CODE_SUBAGENT_MODEL_FORCE"), "1", "a subagent can pick its own model")
        self.assertRegex(self.dream["run"], r'> "\$RUNNER_TEMP/[^"]+"', "the agent can write the result of the dream command")
        self.assertNotIn("--bare", args, "--bare removes Grep, Glob, Write and the subagent tool")
        self.assertNotIn("--dangerously-skip-permissions", args, "the dream command skips permissions")

    def test_write_token_only_in_publish(self) -> None:
        holders = []
        for name, job in self.jobs.items():
            outside = {key: value for key, value in job.items() if key != "steps"}
            self.assertNotIn("DREAM_GITHUB_TOKEN", text(outside), f"the job {name} sets the write token outside its steps")
            holders += [(name, step.get("run", "")) for step in job["steps"] if "DREAM_GITHUB_TOKEN" in text(step)]
        self.assertEqual(len(holders), 1, "the write token must be in exactly one step")
        name, run = holders[0]
        self.assertEqual(name, "publish", f"the job {name} reads the write token")
        self.assertIn("tools/dream/publish.py", run, "the write token is in a step that does not publish")

    def test_both_jobs_run_on_develop_only(self) -> None:
        for name in ("dream", "publish"):
            self.assertIn(
                "github.ref == 'refs/heads/develop'", self.jobs[name].get("if", ""),
                f"the job {name} runs on a branch other than develop",
            )

    def test_checkouts_keep_no_credentials(self) -> None:
        for name, job in self.jobs.items():
            for step in job["steps"]:
                if step.get("uses", "").startswith("actions/checkout"):
                    self.assertIs(
                        step.get("with", {}).get("persist-credentials"), False,
                        f"a checkout of the job {name} keeps its token in .git/config",
                    )

    def test_nothing_runs_after_the_dream_step(self) -> None:
        for step in self.steps[self.index + 1:]:
            self.assertNotIn("run", step, f"the step {step.get('name')} runs a program after the dream step")

    def test_report_survives_a_failed_dream_step(self) -> None:
        uploads = [step for step in self.steps[self.index + 1:] if "upload-artifact" in step.get("uses", "")]
        self.assertEqual(len(uploads), 2, "the job dream must upload the result and the pages")
        for step in uploads:
            self.assertEqual(step.get("if"), "${{ !cancelled() }}", f"{step.get('name')} skips a failed run")
        publish = self.jobs["publish"]
        self.assertEqual(publish["needs"], "dream")
        self.assertIn("!cancelled()", publish["if"], "the job publish skips a failed dream job")


if __name__ == "__main__":
    unittest.main()
