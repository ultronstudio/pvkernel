"""Tests for scripts/check_security_policy.py.

Run with:  python3 -m unittest discover -s scripts/tests -v
"""

import contextlib
import importlib.util
import io
import os
import shutil
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "check_security_policy", REPO / "scripts" / "check_security_policy.py"
)
policy = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(policy)

AGENT = ".github/agents/security-triage.agent.md"
PROMPT = ".github/prompts/security-triage.prompt.md"
FORM = ".github/VULNERABILITY_REPORT.yml"
KERNEL_INSTR = ".github/instructions/kernel-security.instructions.md"


class PolicyTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name) / "repo"
        shutil.copytree(
            REPO,
            self.root,
            ignore=shutil.ignore_patterns(".git", "target", "__pycache__"),
        )

    def edit(self, rel, old, new):
        path = self.root / rel
        text = path.read_text(encoding="utf-8")
        self.assertIn(old, text, f"fixture drift: {old!r} not in {rel}")
        path.write_text(text.replace(old, new, 1), encoding="utf-8")

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def errors(self):
        return policy.run(self.root)

    def assertFails(self, fragment):
        errs = self.errors()
        self.assertTrue(
            any(fragment in e for e in errs), f"expected {fragment!r} in {errs}"
        )


class HappyPathTests(PolicyTestCase):
    def test_repository_passes(self):
        self.assertEqual(self.errors(), [])

    def test_real_repository_passes(self):
        self.assertEqual(policy.run(REPO), [])


class RequiredFilesTests(PolicyTestCase):
    def test_each_required_file_is_enforced(self):
        for rel in policy.REQUIRED_FILES:
            with self.subTest(rel=rel):
                backup = (self.root / rel).read_bytes()
                (self.root / rel).unlink()
                self.assertFails(f"{rel}: required security-policy file is missing")
                (self.root / rel).write_bytes(backup)


class AgentTests(PolicyTestCase):
    def test_execute_tool_is_rejected(self):
        self.edit(AGENT, 'tools: ["read", "search"]', 'tools: ["read", "search", "execute"]')
        self.assertFails("tools exceed the read-only set")

    def test_edit_tool_is_rejected(self):
        self.edit(AGENT, 'tools: ["read", "search"]', 'tools: ["read", "edit"]')
        self.assertFails("tools exceed the read-only set")

    def test_unset_tools_is_rejected(self):
        self.edit(AGENT, 'tools: ["read", "search"]\n', "")
        self.assertFails("tools must be an explicit list")

    def test_empty_tools_is_allowed(self):
        self.edit(AGENT, 'tools: ["read", "search"]', "tools: []")
        self.assertEqual(self.errors(), [])

    def test_mcp_servers_is_rejected(self):
        self.edit(AGENT, "disable-model-invocation: true", "disable-model-invocation: true\nmcp-servers: {}")
        self.assertFails("must not declare mcp-servers")

    def test_model_invocation_must_be_disabled(self):
        self.edit(AGENT, "disable-model-invocation: true", "disable-model-invocation: false")
        self.assertFails("disable-model-invocation must be true")

    def test_description_required(self):
        text = (self.root / AGENT).read_text(encoding="utf-8")
        lines = [l for l in text.split("\n") if not l.startswith("description:")]
        (self.root / AGENT).write_text("\n".join(lines), encoding="utf-8")
        self.assertFails("description is required")

    def test_bad_filename_is_rejected(self):
        shutil.move(self.root / AGENT, self.root / ".github/agents/bad name.agent.md")
        self.assertFails("agent filename must match")

    def test_body_limit(self):
        with open(self.root / AGENT, "a", encoding="utf-8") as fh:
            fh.write("x" * policy.AGENT_MAX_BODY_CHARS)
        self.assertFails("exceeds 30000 characters")

    def test_output_format_required(self):
        text = (self.root / AGENT).read_text(encoding="utf-8")
        (self.root / AGENT).write_text(text.replace("PVK SECURITY TRIAGE", "REPORT"), encoding="utf-8")
        self.assertFails("required output format block is missing")

    def test_missing_frontmatter(self):
        self.write(AGENT, "no frontmatter here\n")
        self.assertFails("missing YAML frontmatter")


class InstructionTests(PolicyTestCase):
    def test_apply_to_matching_nothing_is_rejected(self):
        self.edit(KERNEL_INSTR, 'applyTo: "kernel/src/**/*.rs,src/**/*.rs,build.rs"', 'applyTo: "nonexistent/**/*.rs"')
        self.assertFails("matches no file")

    def test_one_dead_pattern_in_a_list_is_rejected(self):
        self.edit(KERNEL_INSTR, "build.rs", "does-not-exist.rs")
        self.assertFails("matches no file")

    def test_apply_to_required(self):
        self.edit(KERNEL_INSTR, 'applyTo: "kernel/src/**/*.rs,src/**/*.rs,build.rs"', "other: 1")
        self.assertFails("applyTo must be a non-empty string")

    def test_exclude_agent_values(self):
        self.edit(KERNEL_INSTR, 'applyTo:', 'excludeAgent: "bogus"\napplyTo:')
        self.assertFails("excludeAgent must be")

    def test_valid_exclude_agent(self):
        self.edit(KERNEL_INSTR, 'applyTo:', 'excludeAgent: "cloud-agent"\napplyTo:')
        self.assertEqual(self.errors(), [])

    def test_invalid_yaml_frontmatter(self):
        self.write(".github/instructions/broken.instructions.md", "---\napplyTo: [unclosed\n---\nbody\n")
        self.assertFails("frontmatter is not valid YAML")

    def test_empty_repo_wide_instructions(self):
        self.write(".github/copilot-instructions.md", "  \n")
        self.assertFails("file is empty")


class PromptTests(PolicyTestCase):
    def test_unknown_agent_is_rejected(self):
        self.edit(PROMPT, "agent: security-triage", "agent: no-such-agent")
        self.assertFails("agent must be a built-in mode or an existing custom agent")

    def test_builtin_agent_is_allowed(self):
        self.edit(PROMPT, "agent: security-triage", "agent: ask")
        self.assertEqual(self.errors(), [])

    def test_filename_suffix(self):
        self.write(".github/prompts/oops.md", "---\ndescription: x\n---\nbody\n")
        self.assertFails("prompt filename must end in .prompt.md")


class FormTests(PolicyTestCase):
    def test_unknown_element_type(self):
        self.edit(FORM, "type: dropdown", "type: slider")
        self.assertFails("type must be one of")

    def test_duplicate_id(self):
        self.edit(FORM, "id: affected-version", "id: summary")
        self.assertFails("duplicate id")

    def test_duplicate_label(self):
        self.edit(FORM, "label: Affected pvKernel version or commit", "label: Summary")
        self.assertFails("duplicate label")

    def test_cvss_must_not_be_required(self):
        self.edit(
            FORM,
            "  - type: input\n    id: cwe\n",
            "  - type: input\n    id: cvss\n    attributes:\n      label: CVSS score\n"
            "    validations:\n      required: true\n\n  - type: input\n    id: cwe\n",
        )
        self.assertFails("CVSS score must not be required")

    def test_optional_cvss_is_allowed(self):
        self.edit(
            FORM,
            "  - type: input\n    id: cwe\n",
            "  - type: input\n    id: cvss\n    attributes:\n      label: CVSS score\n\n  - type: input\n    id: cwe\n",
        )
        self.assertEqual(self.errors(), [])

    def test_reserved_none_option(self):
        self.edit(FORM, "- Other or not sure", "- None")
        self.assertFails("must not be 'None'")

    def test_duplicate_dropdown_option(self):
        self.edit(FORM, "- Denial of service", "- Information disclosure")
        self.assertFails("options must be unique")

    def test_invalid_yaml(self):
        self.write(FORM, "name: [unclosed\n")
        self.assertFails("file is not valid YAML")

    def test_unexpected_top_level_key(self):
        self.edit(FORM, "body:\n", "labels: [security]\nbody:\n")
        self.assertFails("unexpected top-level key")

    def test_bad_id_characters(self):
        self.edit(FORM, "id: affected-version", "id: bad id!")
        self.assertFails("id must match")

    def test_proof_of_concept_is_optional(self):
        # Reporters must not be forced to supply exploit code.
        import yaml

        form = yaml.safe_load((self.root / FORM).read_text(encoding="utf-8"))
        poc = next(e for e in form["body"] if e.get("id") == "proof-of-concept")
        self.assertFalse(poc["validations"]["required"])


class WorkflowGuardTests(PolicyTestCase):
    def add_workflow(self, name, body, permissions=True):
        header = "name: t\non: workflow_dispatch\n" + ("permissions:\n  contents: read\n" if permissions else "")
        self.write(f".github/workflows/{name}", header + "jobs:\n  j:\n    runs-on: ubuntu-latest\n    steps:\n" + body)

    def test_advisory_api_is_rejected(self):
        self.add_workflow("a.yml", "      - run: gh api repos/o/r/security-advisories\n")
        self.assertFails("repository security advisory API")

    def test_cve_request_is_rejected(self):
        self.add_workflow("a.yml", "      - run: ./cve-request.sh\n")
        self.assertFails("CVE request")

    def test_issue_creation_is_rejected(self):
        self.add_workflow("a.yml", "      - run: gh issue create --title x\n")
        self.assertFails("public issue creation")

    def test_issues_write_is_rejected(self):
        self.add_workflow("a.yml", "      - run: echo hi\n")
        self.edit(".github/workflows/a.yml", "contents: read", "contents: read\n  issues: write")
        self.assertFails("issues: write permission")

    def test_pull_request_target_is_rejected(self):
        self.add_workflow("a.yml", "      - run: echo hi\n")
        self.edit(".github/workflows/a.yml", "workflow_dispatch", "pull_request_target")
        self.assertFails("pull_request_target trigger")

    def test_missing_permissions_is_rejected(self):
        self.add_workflow("a.yml", "      - run: echo hi\n", permissions=False)
        self.assertFails("must declare top-level permissions")

    def test_composite_action_is_also_scanned(self):
        self.write(".github/actions/x/action.yml", "name: x\nruns:\n  using: composite\n  steps:\n    - run: gh issue create\n      shell: bash\n")
        self.assertFails("public issue creation")

    def test_clean_workflow_passes(self):
        self.add_workflow("a.yml", "      - run: echo hi\n")
        self.assertEqual(self.errors(), [])

    def test_full_line_comment_mentioning_a_trigger_is_allowed(self):
        self.add_workflow(
            "a.yml",
            "      # Never use pull_request_target or gh issue create here.\n"
            "      - run: echo hi\n",
        )
        self.assertEqual(self.errors(), [])

    def test_trailing_comment_character_cannot_hide_a_command(self):
        self.add_workflow("a.yml", "      - run: echo '#'; gh issue create --title x\n")
        self.assertFails("public issue creation")

    def test_commented_out_text_inside_a_run_block_is_not_executed(self):
        self.add_workflow(
            "a.yml",
            "      - run: |\n          echo hi\n          # gh issue create is forbidden\n",
        )
        self.assertEqual(self.errors(), [])


class OutputSafetyTests(PolicyTestCase):
    def test_safe_sanitizes_workflow_command_characters(self):
        cleaned = policy.safe("a/::set-output name=x::y\n")
        self.assertNotIn("::", cleaned)
        self.assertNotIn("\n", cleaned)

    def test_messages_never_echo_file_contents(self):
        marker = "ATTACKER_CONTROLLED_MARKER"
        self.write(
            ".github/workflows/evil.yml",
            "name: t\non: workflow_dispatch\npermissions:\n  contents: read\njobs:\n  j:\n"
            f"    runs-on: ubuntu-latest\n    steps:\n      - run: echo '::error::{marker}' security-advisories\n",
        )
        self.edit(FORM, "type: dropdown", f"type: {marker}")
        self.edit(AGENT, 'tools: ["read", "search"]', f'tools: ["{marker}"]')
        errs = self.errors()
        self.assertTrue(errs)
        for e in errs:
            self.assertNotIn(marker, e)
            self.assertNotIn("::", e)

    def test_hostile_filename_is_sanitized(self):
        # `%` and `]` are valid on Windows and still exercise the path
        # sanitization applied before filenames are included in diagnostics.
        self.write(".github/workflows/evil%5D.yml", "name: t\non: push\n")
        errors = self.errors()
        self.assertTrue(errors)
        self.assertTrue(any("evil?5D.yml" in error for error in errors))
        for error in errors:
            self.assertNotIn("::", error)
            self.assertNotIn("evil%5D.yml", error)


class MainTests(PolicyTestCase):
    def run_main(self, args, env=None):
        out, err = io.StringIO(), io.StringIO()
        old = dict(os.environ)
        os.environ.update(env or {})
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = policy.main(["check_security_policy.py", *args])
        finally:
            os.environ.clear()
            os.environ.update(old)
        return code, out.getvalue(), err.getvalue()

    def test_pass_exit_code(self):
        code, out, _ = self.run_main([str(self.root)])
        self.assertEqual(code, 0)
        self.assertIn("structurally valid", out)

    def test_fail_exit_code(self):
        (self.root / FORM).unlink()
        code, out, _ = self.run_main([str(self.root)])
        self.assertEqual(code, 1)
        self.assertIn("FAIL", out)

    def test_usage_errors(self):
        self.assertEqual(self.run_main(["/no/such/dir"])[0], 2)
        self.assertEqual(self.run_main(["a", "b"])[0], 2)

    def test_summary_is_static(self):
        summary = Path(self._tmp.name) / "summary.md"
        (self.root / FORM).unlink()
        self.run_main([str(self.root)], {"GITHUB_STEP_SUMMARY": str(summary)})
        text = summary.read_text(encoding="utf-8")
        self.assertIn("Security policy check", text)
        self.assertIn("failed", text)
        self.assertNotIn("VULNERABILITY_REPORT", text)  # no per-file detail in the summary


class FrontmatterTests(unittest.TestCase):
    def test_parse(self):
        front, body = policy.parse_frontmatter("---\na: 1\n---\nhello\n")
        self.assertEqual(front, {"a": 1})
        self.assertEqual(body, "hello\n")

    def test_errors(self):
        for text, fragment in [
            ("a: 1\n", "missing"),
            ("---\na: 1\n", "unterminated"),
            ("---\n- x\n---\nb\n", "not a mapping"),
            ("---\na: [\n---\nb\n", "not valid YAML"),
        ]:
            with self.subTest(text=text):
                with self.assertRaisesRegex(ValueError, fragment):
                    policy.parse_frontmatter(text)


if __name__ == "__main__":
    unittest.main()
