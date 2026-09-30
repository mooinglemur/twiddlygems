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

The numbers come from Archipelago's own `APWorldContainer.get_manifest()`
rather than from a constant here, because a constant here is a second copy of
somebody else's version number and would be wrong the first time they changed
it. This is the same merge Archipelago's own "Build APWorlds" launcher
component does; that component is not called directly because it ends by
opening a file manager, which is not a thing a build should do.
"""

import json
import sys
from pathlib import Path


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

    # Imported rather than reimplemented, so the container version is whatever
    # this Archipelago says it is.
    sys.path.insert(0, str(checkout))
    from worlds.Files import APWorldContainer  # noqa: E402

    container = APWorldContainer(str(staged))
    container.game = manifest["game"]
    stamped = dict(manifest)
    stamped.update(container.get_manifest())

    # `get_manifest` also re-adds `game`, and would add the version fields the
    # container happens to be holding, which is none: the world's own
    # `world_version` and `minimum_ap_version` are strings in the file rather
    # than attributes on the container, so they survive from `manifest` above.
    for carried in ("world_version", "minimum_ap_version", "maximum_ap_version", "authors"):
        if carried in manifest:
            stamped[carried] = manifest[carried]

    manifest_path.write_text(json.dumps(stamped, indent=2) + "\n", encoding="utf-8")
    print(
        f"stamped {manifest_path.name}: container version "
        f"{stamped['version']}, compatible {stamped['compatible_version']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
