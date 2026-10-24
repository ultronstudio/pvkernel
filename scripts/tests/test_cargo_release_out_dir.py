"""Tests for scripts/cargo_release_out_dir.py."""

import importlib.util
import json
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "cargo_release_out_dir", REPO / "scripts" / "cargo_release_out_dir.py"
)
cargo_out_dir = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cargo_out_dir)

PACKAGE_ID = "path+file:///workspace/pvkernel#pvkernel-runner@0.0.6"


def build_message(package_id=PACKAGE_ID, out_dir="/workspace/target/build/pvkernel/out"):
    return {
        "reason": "build-script-executed",
        "package_id": package_id,
        "linked_libs": [],
        "linked_paths": [],
        "cfgs": [],
        "env": [],
        "out_dir": out_dir,
    }


def json_line(message):
    return json.dumps(message)


class PackageIdTests(unittest.TestCase):
    def test_matches_exact_package_name_only(self):
        metadata = {
            "packages": [
                {"name": "pvkernel-runner-helper", "id": "unrelated"},
                {"name": "kernel", "id": "kernel-id"},
                {"name": "pvkernel-runner", "id": PACKAGE_ID},
            ]
        }
        self.assertEqual(cargo_out_dir.package_id_from_metadata(metadata), PACKAGE_ID)

    def test_zero_or_multiple_package_matches_fail(self):
        for packages in ([], [{"name": "pvkernel-runner", "id": PACKAGE_ID}] * 2):
            with self.subTest(packages=len(packages)):
                with self.assertRaises(cargo_out_dir.CargoOutputError):
                    cargo_out_dir.package_id_from_metadata({"packages": packages})

    def test_missing_package_id_fails(self):
        with self.assertRaisesRegex(cargo_out_dir.CargoOutputError, "no valid Cargo package ID"):
            cargo_out_dir.package_id_from_metadata(
                {"packages": [{"name": "pvkernel-runner", "id": ""}]}
            )


class OutDirTests(unittest.TestCase):
    def test_one_matching_message_returns_out_dir(self):
        expected = "/workspace/target/build/pvkernel/out"
        self.assertEqual(
            cargo_out_dir.out_dir_from_messages(PACKAGE_ID, [json_line(build_message())]),
            expected,
        )

    def test_unrelated_package_messages_are_ignored(self):
        lines = [
            json_line(build_message(package_id="kernel-id")),
            json_line({"reason": "compiler-artifact", "package_id": PACKAGE_ID}),
            json_line(build_message()),
        ]
        self.assertEqual(
            cargo_out_dir.out_dir_from_messages(PACKAGE_ID, lines),
            "/workspace/target/build/pvkernel/out",
        )

    def test_zero_matching_messages_fail(self):
        with self.assertRaisesRegex(cargo_out_dir.CargoOutputError, "no build-script-executed"):
            cargo_out_dir.out_dir_from_messages(
                PACKAGE_ID, [json_line(build_message(package_id="other-id"))]
            )

    def test_distinct_out_dirs_fail(self):
        lines = [
            json_line(build_message(out_dir="/one/out")),
            json_line(build_message(out_dir="/two/out")),
        ]
        with self.assertRaisesRegex(cargo_out_dir.CargoOutputError, "multiple distinct"):
            cargo_out_dir.out_dir_from_messages(PACKAGE_ID, lines)

    def test_duplicate_identical_out_dirs_are_deduplicated(self):
        lines = [json_line(build_message()), json_line(build_message())]
        self.assertEqual(
            cargo_out_dir.out_dir_from_messages(PACKAGE_ID, lines),
            "/workspace/target/build/pvkernel/out",
        )

    def test_malformed_json_fails_clearly(self):
        with self.assertRaisesRegex(cargo_out_dir.CargoOutputError, "line 1 is not valid JSON"):
            cargo_out_dir.out_dir_from_messages(PACKAGE_ID, ["Cargo warning: something"])

    def test_matching_message_without_out_dir_fails(self):
        with self.assertRaisesRegex(cargo_out_dir.CargoOutputError, "no valid out_dir"):
            cargo_out_dir.out_dir_from_messages(
                PACKAGE_ID,
                [json_line({"reason": "build-script-executed", "package_id": PACKAGE_ID})],
            )

    def test_blank_lines_are_ignored(self):
        self.assertEqual(
            cargo_out_dir.out_dir_from_messages(PACKAGE_ID, ["", json_line(build_message())]),
            "/workspace/target/build/pvkernel/out",
        )


if __name__ == "__main__":
    unittest.main()
