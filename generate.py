#!/usr/bin/env python3
"""
Run all *-plugin crate binaries to regenerate their code-generated Rust files.

Each plugin crate (e.g. core-plugin, structure-plugin) defines a static PLUGIN
in its lib.rs and a main.rs that calls PLUGIN.generate().  Running this script
discovers every workspace member whose directory name ends in ``-plugin``,
parses its Cargo.toml for the package name, and runs ``cargo run -p <name>``.

To add a new plugin crate:
  1. Create ``my-plugin/Cargo.toml`` with ``[package] name = "lichen-my-plugin"``
     and add it to the workspace ``[members]`` list.
  2. Run this script — it will detect and build the new plugin automatically.
"""

from __future__ import annotations

import subprocess
import sys
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent


def find_workspace_members(cargo_toml: Path) -> list[str]:
    """Parse the workspace ``[members]`` list from the root ``Cargo.toml``."""
    with cargo_toml.open("rb") as f:
        data = tomllib.load(f)
    return data.get("workspace", {}).get("members", [])


def resolve_member_path(member: str) -> Path:
    """Resolve a workspace member string (e.g. ``"core-plugin"``) to an absolute path."""
    return (REPO_ROOT / member).resolve()


def find_plugin_crates(members: list[str]) -> list[Path]:
    """Filter workspace members to those whose directory name ends with ``-plugin``."""
    plugins: list[Path] = []
    for member in members:
        path = resolve_member_path(member)
        if path.is_dir() and path.name.endswith("-plugin"):
            plugins.append(path)
    return plugins


def read_crate_name(plugin_dir: Path) -> str | None:
    """Read the ``[package] name`` from a crate's ``Cargo.toml``."""
    cargo_toml = plugin_dir / "Cargo.toml"
    if not cargo_toml.exists():
        print(f"  ⚠ skipping {plugin_dir.name}: no Cargo.toml", file=sys.stderr)
        return None
    with cargo_toml.open("rb") as f:
        data = tomllib.load(f)
    return data.get("package", {}).get("name")


def generate_plugin(crate_name: str) -> bool:
    """Run ``cargo run -p <crate_name>`` and return success."""
    print(f"  ▶ cargo run -p {crate_name} ...", end=" ", flush=True)
    result = subprocess.run(
        ["cargo", "run", "-p", crate_name],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        print("OK")
        return True
    else:
        print("FAILED")
        print(result.stdout, file=sys.stderr)
        print(result.stderr, file=sys.stderr)
        return False


def main() -> None:
    cargo_toml = REPO_ROOT / "Cargo.toml"
    if not cargo_toml.exists():
        print("error: no Cargo.toml found in project root", file=sys.stderr)
        sys.exit(1)

    members = find_workspace_members(cargo_toml)
    plugin_dirs = find_plugin_crates(members)

    if not plugin_dirs:
        print("No *-plugin crates found in workspace members.")
        print("Members checked:", members)
        sys.exit(0)

    print(f"Found {len(plugin_dirs)} plugin crate(s):")
    for d in plugin_dirs:
        print(f"  {d.name}/")

    failures: list[str] = []
    for plugin_dir in plugin_dirs:
        crate_name = read_crate_name(plugin_dir)
        if crate_name is None:
            failures.append(plugin_dir.name)
            continue
        if not generate_plugin(crate_name):
            failures.append(plugin_dir.name)

    if failures:
        plural = "s" if len(failures) > 1 else ""
        print(
            f"\n✖ {len(failures)} plugin{plural} failed: {', '.join(failures)}",
            file=sys.stderr,
        )
        sys.exit(1)
    else:
        print(f"\n✔ All {len(plugin_dirs)} plugin(s) regenerated successfully.")


if __name__ == "__main__":
    main()
