#!/usr/bin/env python3
"""Generate release third-party notices from repository metadata."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8").strip()
    except OSError as err:
        raise SystemExit(f"failed to read {path}: {err}") from err


def cargo_metadata(target: str | None) -> dict:
    command = ["cargo", "metadata", "--locked", "--format-version", "1"]
    if target:
        command.extend(["--filter-platform", target])

    try:
        output = subprocess.check_output(
            command,
            cwd=REPO_ROOT,
            text=True,
        )
    except subprocess.CalledProcessError as err:
        raise SystemExit(f"cargo metadata failed with exit code {err.returncode}") from err
    except OSError as err:
        raise SystemExit(f"failed to run cargo metadata: {err}") from err

    try:
        return json.loads(output)
    except json.JSONDecodeError as err:
        raise SystemExit(f"cargo metadata returned invalid json: {err}") from err


def markdown_escape(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ")


def rust_dependency_rows(metadata: dict) -> list[str]:
    workspace_members = set(metadata.get("workspace_members", []))
    resolved_ids = {
        node.get("id")
        for node in metadata.get("resolve", {}).get("nodes", [])
        if node.get("id")
    }
    packages = [
        package
        for package in metadata.get("packages", [])
        if package.get("id") in resolved_ids and package.get("id") not in workspace_members
    ]
    packages.sort(key=lambda package: (package.get("name", ""), package.get("version", "")))

    rows = []
    for package in packages:
        name = markdown_escape(package.get("name", ""))
        version = markdown_escape(package.get("version", ""))
        license_text = package.get("license") or package.get("license_file") or "NOASSERTION"
        license_text = markdown_escape(license_text)
        source = package.get("repository") or package.get("homepage") or package.get("source") or ""
        source = markdown_escape(source)
        rows.append(f"| {name} | {version} | {license_text} | {source} |")
    return rows


def build_notice(target: str | None) -> str:
    notice = read_text(REPO_ROOT / "NOTICE")
    libghostty_license = read_text(REPO_ROOT / "vendor/libghostty-vt/LICENSE")
    dependency_rows = rust_dependency_rows(cargo_metadata(target))
    metadata_command = "cargo metadata --locked"
    if target:
        metadata_command += f" --filter-platform {target}"

    lines = [
        "# Third-Party Notices",
        "",
        "This file is generated for release archives by `scripts/generate_third_party_notices.py`.",
        "",
        "## Project Notice",
        "",
        notice,
        "",
        "## Source Availability",
        "",
        "gmux source for each release is available from the corresponding tag at:",
        "https://github.com/stanley2058/gmux",
        "",
        "## Vendored libghostty-vt",
        "",
        "gmux statically links vendored libghostty-vt. Its license text follows.",
        "",
        "```text",
        libghostty_license,
        "```",
        "",
        "## Rust Dependencies",
        "",
        f"The following Rust dependency license metadata is reported by `{metadata_command}`.",
        "",
        "| Package | Version | License | Source |",
        "| --- | --- | --- | --- |",
    ]
    lines.extend(dependency_rows)
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path, help="output markdown path")
    parser.add_argument("--target", help="optional Rust target triple for filtered metadata")
    args = parser.parse_args()

    output = args.output
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(build_notice(args.target), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
