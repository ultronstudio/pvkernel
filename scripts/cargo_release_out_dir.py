#!/usr/bin/env python3
"""Resolve pvkernel-runner's build-script OUT_DIR from Cargo JSON.

Usage:
    cargo_release_out_dir.py package-id <metadata.json>
    cargo_release_out_dir.py out-dir <package-id> <build-messages.jsonl>

Repeated build-script messages with the same OUT_DIR are accepted and
deduplicated. This treats repeated reports of one location as unambiguous;
distinct locations fail rather than selecting one based on message order.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


class CargoOutputError(ValueError):
    """Cargo metadata or build output did not identify one release OUT_DIR."""


def read_json(path: str | Path) -> Any:
    try:
        with open(path, encoding="utf-8") as source:
            return json.load(source)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise CargoOutputError(f"could not read valid Cargo JSON from {path}") from exc


def package_id_from_metadata(metadata: Any) -> str:
    if not isinstance(metadata, dict) or not isinstance(metadata.get("packages"), list):
        raise CargoOutputError("Cargo metadata has no packages array")

    matches = [
        package
        for package in metadata["packages"]
        if isinstance(package, dict) and package.get("name") == "pvkernel-runner"
    ]
    if len(matches) != 1:
        raise CargoOutputError(
            "expected exactly one Cargo package named pvkernel-runner; "
            f"found {len(matches)}"
        )

    package_id = matches[0].get("id")
    if not isinstance(package_id, str) or not package_id.strip():
        raise CargoOutputError("pvkernel-runner package has no valid Cargo package ID")
    return package_id


def out_dir_from_messages(package_id: str, lines: list[str]) -> str:
    if not isinstance(package_id, str) or not package_id.strip():
        raise CargoOutputError("Cargo package ID must be non-empty")

    matching_dirs: set[str] = set()
    for line_number, line in enumerate(lines, start=1):
        if not line.strip():
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as exc:
            raise CargoOutputError(
                f"Cargo build output line {line_number} is not valid JSON"
            ) from exc

        if not isinstance(message, dict):
            continue
        if (
            message.get("reason") != "build-script-executed"
            or message.get("package_id") != package_id
        ):
            continue

        out_dir = message.get("out_dir")
        if not isinstance(out_dir, str) or not out_dir.strip():
            raise CargoOutputError(
                "matching pvkernel-runner build-script message has no valid out_dir"
            )
        matching_dirs.add(out_dir)

    if not matching_dirs:
        raise CargoOutputError(
            "Cargo output contains no build-script-executed message for pvkernel-runner"
        )
    if len(matching_dirs) != 1:
        raise CargoOutputError(
            "Cargo output contains multiple distinct pvkernel-runner OUT_DIR values"
        )
    return next(iter(matching_dirs))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    package_id_command = commands.add_parser("package-id")
    package_id_command.add_argument("metadata_json")
    out_dir_command = commands.add_parser("out-dir")
    out_dir_command.add_argument("package_id")
    out_dir_command.add_argument("build_messages_jsonl")
    args = parser.parse_args(argv)

    try:
        if args.command == "package-id":
            result = package_id_from_metadata(read_json(args.metadata_json))
        else:
            try:
                with open(args.build_messages_jsonl, encoding="utf-8") as source:
                    lines = source.readlines()
            except (OSError, UnicodeDecodeError) as exc:
                raise CargoOutputError(
                    f"could not read Cargo build messages from {args.build_messages_jsonl}"
                ) from exc
            result = out_dir_from_messages(args.package_id, lines)
    except CargoOutputError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    print(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
