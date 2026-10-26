#!/usr/bin/env python3
"""Fail if a deterministic Cargo scenario is absent from the CI QEMU matrix."""

from pathlib import Path
import sys
import tomllib

import yaml


def check(root: Path) -> tuple[int, set[str]]:
    features = tomllib.loads((root / "Cargo.toml").read_text())["features"]
    expected = {
        name for name in features if name.endswith(("-test", "-tests"))
    } - {"userspace-test"}  # Shared scaffolding, not a standalone scenario.
    workflow = yaml.safe_load((root / ".github/workflows/ci.yml").read_text())
    covered: set[str] = set()
    count = 0
    for job in workflow["jobs"].values():
        for step in job.get("steps", []):
            if step.get("uses") != "./.github/actions/qemu-scenario":
                continue
            selected = step["with"]["features"]
            if selected == "${{ matrix.features }}":
                matrix = job["strategy"]["matrix"]
                selections = matrix.get("features")
                if selections is None:
                    selections = [row["features"] for row in matrix["include"]]
            elif "${{" in selected:
                raise ValueError("unsupported dynamic QEMU feature selection")
            else:
                selections = [selected]
            for selection in selections:
                covered.update(selection.split())
                count += 1
    return count, expected - covered


if __name__ == "__main__":
    count, missing = check(Path(__file__).resolve().parents[1])
    if missing:
        print("Missing deterministic QEMU scenarios: " + ", ".join(sorted(missing)))
        sys.exit(1)
    print(f"Deterministic QEMU coverage: PASS ({count} scenarios)")
