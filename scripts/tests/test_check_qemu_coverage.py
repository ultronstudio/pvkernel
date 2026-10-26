"""Prevent regression to a green CI run that silently omits a Cargo scenario."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("check_qemu_coverage", REPO / "scripts/check_qemu_coverage.py")
coverage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coverage)


class CoverageTests(unittest.TestCase):
    def test_current_matrix_covers_all_34_scenarios(self):
        self.assertEqual(coverage.check(REPO), (34, set()))

    def test_removing_process_runtime_job_is_detected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".github/workflows").mkdir(parents=True)
            (root / "Cargo.toml").write_text((REPO / "Cargo.toml").read_text())
            ci = (REPO / ".github/workflows/ci.yml").read_text()
            (root / ".github/workflows/ci.yml").write_text(ci.split("\n  qemu-process:")[0])
            count, missing = coverage.check(root)
            self.assertEqual(count, 22)
            self.assertEqual(len(missing), 12)
            self.assertIn("address-space-isolation-test", missing)
            self.assertIn("elf-malformed-test", missing)

    def test_new_cargo_scenario_requires_runtime_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".github/workflows").mkdir(parents=True)
            cargo = (REPO / "Cargo.toml").read_text().replace("[features]\n", "[features]\nnew-regression-test = []\n")
            (root / "Cargo.toml").write_text(cargo)
            (root / ".github/workflows/ci.yml").write_text((REPO / ".github/workflows/ci.yml").read_text())
            self.assertEqual(coverage.check(root)[1], {"new-regression-test"})
