#!/usr/bin/env python3
"""Structural validation of pvKernel's security-policy files.

Deterministic repository infrastructure, not a vulnerability detector. It
checks that the security-policy files exist and are well-formed, and that
no workflow contains advisory-publishing or CVE-request automation. See
docs/security/security-triage.md.

Output safety: this script runs in public CI on pull-request content. It
therefore never echoes file *contents* (only fixed messages and sanitized
paths), and never emits GitHub workflow commands (``::...``).

Usage: check_security_policy.py [REPO_ROOT]
Exit status: 0 if every check passes, 1 if any fails, 2 on usage error.
"""

from __future__ import annotations

import glob
import os
import re
import sys
from pathlib import Path

import yaml

REQUIRED_FILES = [
    "SECURITY.md",
    "docs/security/security-triage.md",
    "docs/security/vulnerability-management.md",
    "docs/security/github-configuration.md",
    ".github/copilot-instructions.md",
    ".github/agents/security-triage.agent.md",
    ".github/prompts/security-triage.prompt.md",
    ".github/VULNERABILITY_REPORT.yml",
]

# The triage agent analyses and recommends; it must not be able to edit
# files or run commands. Anything outside this set widens its authority.
AGENT_ALLOWED_TOOLS = {"read", "search"}
AGENT_MAX_BODY_CHARS = 30_000
AGENT_FILENAME = re.compile(r"^[A-Za-z0-9._-]+\.agent\.md$")
PROMPT_BUILTIN_AGENTS = {"ask", "agent", "plan"}
EXCLUDE_AGENT_VALUES = {"code-review", "cloud-agent"}

FORM_ELEMENT_TYPES = {"checkboxes", "dropdown", "input", "markdown", "textarea"}
FORM_TOP_LEVEL_KEYS = {"name", "description", "body"}
FORM_ID = re.compile(r"^[A-Za-z0-9_-]+$")

# Automation boundary: workflows must never publish advisories, request
# CVEs, or open public issues. A legitimate future need requires a separate
# security design review and an edit to this list.
FORBIDDEN_WORKFLOW_PATTERNS = [
    (re.compile(r"security-advisories", re.I), "repository security advisory API"),
    (re.compile(r"repository-advisories", re.I), "repository advisory permission"),
    (re.compile(r"cve[_-]?request|cveawg|cna-api|cve\.org", re.I), "CVE request"),
    (re.compile(r"gh\s+issue\s+create|issues\.create", re.I), "public issue creation"),
    (re.compile(r"\bissues\s*:\s*write\b", re.I), "issues: write permission"),
    (re.compile(r"\bpull_request_target\b"), "pull_request_target trigger"),
]

_UNSAFE_PATH_CHARS = re.compile(r"[^A-Za-z0-9._/\-]")


def safe(path: str | Path) -> str:
    """Sanitize a path before printing, so it can't smuggle `::` commands."""
    return _UNSAFE_PATH_CHARS.sub("?", str(path))


def parse_frontmatter(text: str):
    """Return (mapping, body). Raises ValueError with a fixed message."""
    if not text.startswith("---\n"):
        raise ValueError("missing YAML frontmatter")
    end = text.find("\n---\n", 4)
    if end == -1:
        raise ValueError("unterminated YAML frontmatter")
    try:
        data = yaml.safe_load(text[4:end])
    except yaml.YAMLError:
        raise ValueError("frontmatter is not valid YAML") from None
    if not isinstance(data, dict):
        raise ValueError("frontmatter is not a mapping")
    return data, text[end + 5 :]


def load_yaml(path: Path):
    try:
        return yaml.safe_load(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError):
        raise ValueError("file is unreadable") from None
    except yaml.YAMLError:
        raise ValueError("file is not valid YAML") from None


def check_required_files(root: Path) -> list[str]:
    return [
        f"{rel}: required security-policy file is missing"
        for rel in REQUIRED_FILES
        if not (root / rel).is_file()
    ]


def check_instructions(root: Path) -> list[str]:
    errors: list[str] = []
    repo_wide = root / ".github/copilot-instructions.md"
    if repo_wide.is_file() and not repo_wide.read_text(encoding="utf-8").strip():
        errors.append(".github/copilot-instructions.md: file is empty")
    for path in sorted((root / ".github/instructions").glob("*.instructions.md")):
        rel = safe(path.relative_to(root))
        try:
            front, body = parse_frontmatter(path.read_text(encoding="utf-8"))
        except ValueError as exc:
            errors.append(f"{rel}: {exc}")
            continue
        apply_to = front.get("applyTo")
        if not isinstance(apply_to, str) or not apply_to.strip():
            errors.append(f"{rel}: applyTo must be a non-empty string")
        else:
            for pattern in (p.strip() for p in apply_to.split(",")):
                if not pattern or not glob.glob(pattern, root_dir=root, recursive=True):
                    errors.append(f"{rel}: an applyTo pattern matches no file in the repository")
        exclude = front.get("excludeAgent")
        if exclude is not None and exclude not in EXCLUDE_AGENT_VALUES:
            errors.append(f"{rel}: excludeAgent must be 'code-review' or 'cloud-agent'")
        if not body.strip():
            errors.append(f"{rel}: instruction body is empty")
    return errors


def check_agents(root: Path) -> list[str]:
    errors: list[str] = []
    for path in sorted((root / ".github/agents").glob("*.md")):
        rel = safe(path.relative_to(root))
        if not AGENT_FILENAME.match(path.name):
            errors.append(f"{rel}: agent filename must match NAME.agent.md using [A-Za-z0-9._-]")
            continue
        try:
            front, body = parse_frontmatter(path.read_text(encoding="utf-8"))
        except ValueError as exc:
            errors.append(f"{rel}: {exc}")
            continue
        description = front.get("description")
        if not isinstance(description, str) or not description.strip():
            errors.append(f"{rel}: description is required")
        tools = front.get("tools")
        if not isinstance(tools, list) or not all(isinstance(t, str) for t in tools):
            errors.append(f"{rel}: tools must be an explicit list (an unset list grants all tools)")
        elif not {t.lower() for t in tools} <= AGENT_ALLOWED_TOOLS:
            errors.append(f"{rel}: tools exceed the read-only set {sorted(AGENT_ALLOWED_TOOLS)}")
        if "mcp-servers" in front:
            errors.append(f"{rel}: a triage agent must not declare mcp-servers")
        if front.get("disable-model-invocation") is not True:
            errors.append(f"{rel}: disable-model-invocation must be true (manual selection only)")
        if len(body) > AGENT_MAX_BODY_CHARS:
            errors.append(f"{rel}: prompt body exceeds {AGENT_MAX_BODY_CHARS} characters")
        if "PVK SECURITY TRIAGE" not in body:
            errors.append(f"{rel}: required output format block is missing")
    return errors


def check_prompts(root: Path) -> list[str]:
    errors: list[str] = []
    agent_names = {p.name[: -len(".agent.md")] for p in (root / ".github/agents").glob("*.agent.md")}
    for path in sorted((root / ".github/prompts").glob("*.md")):
        rel = safe(path.relative_to(root))
        if not path.name.endswith(".prompt.md"):
            errors.append(f"{rel}: prompt filename must end in .prompt.md")
            continue
        try:
            front, body = parse_frontmatter(path.read_text(encoding="utf-8"))
        except ValueError as exc:
            errors.append(f"{rel}: {exc}")
            continue
        description = front.get("description")
        if not isinstance(description, str) or not description.strip():
            errors.append(f"{rel}: description is required")
        agent = front.get("agent")
        if agent is not None and agent not in PROMPT_BUILTIN_AGENTS | agent_names:
            errors.append(f"{rel}: agent must be a built-in mode or an existing custom agent")
        if not body.strip():
            errors.append(f"{rel}: prompt body is empty")
    return errors


def check_vulnerability_form(root: Path) -> list[str]:
    rel = ".github/VULNERABILITY_REPORT.yml"
    path = root / rel
    if not path.is_file():
        return []  # reported by check_required_files
    try:
        form = load_yaml(path)
    except ValueError as exc:
        return [f"{rel}: {exc}"]
    if not isinstance(form, dict):
        return [f"{rel}: top level must be a mapping"]
    errors: list[str] = []
    if not set(form) <= FORM_TOP_LEVEL_KEYS:
        errors.append(f"{rel}: unexpected top-level key (allowed: name, description, body)")
    for key in ("name", "description"):
        if not isinstance(form.get(key), str) or not form[key].strip():
            errors.append(f"{rel}: {key} must be a non-empty string")
    body = form.get("body")
    if not isinstance(body, list) or not body:
        return errors + [f"{rel}: body must be a non-empty list"]

    ids: set[str] = set()
    labels: set[str] = set()
    has_required = False
    for index, element in enumerate(body):
        where = f"{rel}: body[{index}]"
        if not isinstance(element, dict) or element.get("type") not in FORM_ELEMENT_TYPES:
            errors.append(f"{where}: type must be one of {sorted(FORM_ELEMENT_TYPES)}")
            continue
        kind = element["type"]
        attrs = element.get("attributes")
        if not isinstance(attrs, dict):
            errors.append(f"{where}: attributes must be a mapping")
            continue
        if kind == "markdown":
            if not isinstance(attrs.get("value"), str) or not attrs["value"].strip():
                errors.append(f"{where}: markdown value must be a non-empty string")
            continue
        label = attrs.get("label")
        if not isinstance(label, str) or not label.strip():
            errors.append(f"{where}: label must be a non-empty string")
        elif label in labels:
            errors.append(f"{where}: duplicate label")
        else:
            labels.add(label)
        element_id = element.get("id")
        if element_id is not None:
            if not isinstance(element_id, str) or not FORM_ID.match(element_id):
                errors.append(f"{where}: id must match [A-Za-z0-9_-]+")
            elif element_id in ids:
                errors.append(f"{where}: duplicate id")
            else:
                ids.add(element_id)
        validations = element.get("validations") or {}
        required = isinstance(validations, dict) and validations.get("required") is True
        has_required = has_required or required
        if required and isinstance(label, str) and "cvss" in label.lower():
            errors.append(f"{where}: a CVSS score must not be required of reporters")
        if kind == "dropdown":
            options = attrs.get("options")
            if (
                not isinstance(options, list)
                or not options
                or not all(isinstance(o, str) and o.strip() for o in options)
            ):
                errors.append(f"{where}: dropdown options must be a non-empty list of strings")
            elif len(set(options)) != len(options) or "None" in options:
                errors.append(f"{where}: dropdown options must be unique and must not be 'None'")
        if kind == "checkboxes":
            options = attrs.get("options")
            if not isinstance(options, list) or not all(
                isinstance(o, dict) and isinstance(o.get("label"), str) for o in options or []
            ):
                errors.append(f"{where}: checkbox options must each have a label")
    if not has_required:
        errors.append(f"{rel}: at least one field should be required")
    return errors


def workflow_files(root: Path) -> list[Path]:
    files: list[Path] = []
    for pattern in ("*.yml", "*.yaml"):
        files += (root / ".github/workflows").glob(pattern)
        files += (root / ".github/actions").glob(f"**/{pattern}")
    return sorted(files)


def strip_comment_lines(text: str) -> str:
    """Drop full-line YAML/shell comments, which are never executed.

    Only whole-line comments are dropped. A trailing `# ...` is kept because
    it may sit inside a quoted string, and ignoring it could hide a command.
    """
    return "\n".join(l for l in text.split("\n") if not l.lstrip().startswith("#"))


def check_workflows(root: Path) -> list[str]:
    errors: list[str] = []
    for path in workflow_files(root):
        rel = safe(path.relative_to(root))
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            errors.append(f"{rel}: file is unreadable")
            continue
        scanned = strip_comment_lines(text)
        for pattern, what in FORBIDDEN_WORKFLOW_PATTERNS:
            if pattern.search(scanned):
                errors.append(
                    f"{rel}: contains forbidden security automation ({what}); "
                    "advisories, CVE requests and public issues are human-controlled"
                )
        if path.parent.name == "workflows":
            try:
                data = yaml.safe_load(text)
            except yaml.YAMLError:
                errors.append(f"{rel}: file is not valid YAML")
                continue
            if not isinstance(data, dict) or "permissions" not in data:
                errors.append(f"{rel}: must declare top-level permissions (least privilege)")
    return errors


CHECKS = [
    check_required_files,
    check_instructions,
    check_agents,
    check_prompts,
    check_vulnerability_form,
    check_workflows,
]


def run(root: Path) -> list[str]:
    errors: list[str] = []
    for check in CHECKS:
        errors += check(root)
    return errors


def write_summary(ok: bool, failures: int) -> None:
    """Append a static, non-sensitive maintainer summary to the job summary."""
    target = os.environ.get("GITHUB_STEP_SUMMARY")
    if not target:
        return
    status = "passed" if ok else f"failed ({failures} problem(s); see the job log)"
    lines = [
        "## Security policy check",
        "",
        f"Result: **{status}**.",
        "",
        "This check validates the structure of the security-policy files only. "
        "It does not detect vulnerabilities. For triage procedure see "
        "`docs/security/security-triage.md`.",
        "",
    ]
    try:
        with open(target, "a", encoding="utf-8") as fh:
            fh.write("\n".join(lines))
    except OSError:
        pass


def main(argv: list[str]) -> int:
    if len(argv) > 2:
        print("usage: check_security_policy.py [REPO_ROOT]", file=sys.stderr)
        return 2
    root = Path(argv[1] if len(argv) == 2 else ".").resolve()
    if not root.is_dir():
        print("usage: check_security_policy.py [REPO_ROOT]", file=sys.stderr)
        return 2
    errors = run(root)
    for error in errors:
        print(f"FAIL {error}")
    if errors:
        print(f"{len(errors)} security-policy problem(s) found.")
    else:
        print("Security-policy files are structurally valid.")
    write_summary(not errors, len(errors))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
