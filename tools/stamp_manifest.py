#!/usr/bin/env python3
"""Adds the container fields to a staged world's manifest, before it is zipped.

    stamp_manifest.py <archipelago checkout> <staged world directory>

A world's `archipelago.json` describes the world: its game, its version, who
wrote it. A packaged `.apworld` needs two more fields, `version` and
`compatible_version`, which describe the *container* rather than the world, and
which the zip reader indexes rather than gets with a default, so a zip without
them cannot be opened at all.

The source file must not carry them. Archipelago says so in
`docs/apworld specification.md` ("Do not write these fields yourself") and
enforces it in `test.general.test_world_manifest`, which fails a world whose
checked-in manifest defines either. So they belong exactly here: added to the
copy being packaged, and never to the one in the tree.

**Read out of Archipelago's source rather than imported from it.** Importing
`worlds.Files` means importing the `worlds` package, whose `__init__` loads
every world in the checkout, and the ones whose optional dependencies are not
installed log a screenful of tracebacks about jinja2 and zilliandomizer. None
of that has anything to do with packaging this world, and a build that prints
errors it does not mean teaches whoever reads it to ignore errors. Both numbers
are plain literals in one file, so they are read with `ast`: no import, no
dependencies, nothing loaded, and the values still come from Archipelago rather
than from a copy kept here.

This is the same merge Archipelago's own "Build APWorlds" launcher component
does. That component is not called directly because it ends by opening a file
manager, which is not a thing a build should do.
"""

import ast
import json
import sys
from pathlib import Path


def container_versions(checkout: Path) -> tuple[int, int]:
    """The `version` and `compatible_version` a packaged world declares.

    `version` is the module-level `container_version`. `compatible_version` is
    the literal `APWorldContainer.get_manifest` writes, which is its own number
    and deliberately not the same one: the base class writes 5 and the other
    container kinds write 6, so taking any of those would produce a file that
    reads as the wrong sort of thing.

    Raises rather than guessing if either has moved. A wrong number here is a
    file that will not open, reported as "this might be the incorrect world
    version", so failing at the build is much the kinder end of it.
    """
    source = checkout / "worlds" / "Files.py"
    tree = ast.parse(source.read_text(encoding="utf-8"), filename=str(source))

    version: int | None = None
    compatible: int | None = None
    for node in tree.body:
        # container_version: int = 7
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            if node.target.id == "container_version" and node.value is not None:
                version = ast.literal_eval(node.value)
        elif isinstance(node, ast.Assign):
            if any(isinstance(t, ast.Name) and t.id == "container_version" for t in node.targets):
                version = ast.literal_eval(node.value)
        # class APWorldContainer: ... manifest["compatible_version"] = 7
        elif isinstance(node, ast.ClassDef) and node.name == "APWorldContainer":
            for inner in ast.walk(node):
                if not isinstance(inner, ast.Assign):
                    continue
                for target in inner.targets:
                    if (
                        isinstance(target, ast.Subscript)
                        and isinstance(target.slice, ast.Constant)
                        and target.slice.value == "compatible_version"
                    ):
                        compatible = ast.literal_eval(inner.value)

    if version is None:
        raise SystemExit(f"could not find container_version in {source}")
    if compatible is None:
        raise SystemExit(f"could not find APWorldContainer's compatible_version in {source}")
    return version, compatible


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    checkout, staged = Path(argv[1]).resolve(), Path(argv[2]).resolve()

    manifest_path = staged / "archipelago.json"
    if not manifest_path.is_file():
        print(f"{manifest_path} does not exist; run 'make apdata' first", file=sys.stderr)
        return 1
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))

    version, compatible = container_versions(checkout)
    manifest["version"] = version
    manifest["compatible_version"] = compatible

    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"stamped {manifest_path.name}: container version {version}, compatible {compatible}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
