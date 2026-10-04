#!/usr/bin/env python3
"""Writes the player yaml template for this world, with its comments.

The file a player starts from: every setting this game offers, each one under
the sentence that explains it, with the defaults filled in. Archipelago
generates one of these per world for its own website, and this asks it for
ours rather than keeping a second copy by hand. A setting added to the engine
appears here with nothing edited.

Run against an installed `.apworld` rather than the checkout, so the version in
its header is the one stamped into the zip, and so the file shipped beside a
release is the one that release's world actually generates.

    python3 tools/make_template.py vendor/Archipelago build/twiddlygems.yaml
"""

import os
import shutil
import sys
import tempfile
from pathlib import Path

#: What the world calls itself. The template is written out under this name,
#: so this has to match the engine's own, not merely resemble it.
GAME = "Twiddly Gems"


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(f"usage: {argv[0]} <archipelago-checkout> <output.yaml>", file=sys.stderr)
        return 2
    checkout = Path(argv[1]).resolve()
    out = Path(argv[2]).resolve()
    if not (checkout / "Options.py").is_file():
        print(f"error: {checkout} is not an Archipelago checkout", file=sys.stderr)
        return 1

    # Importing Archipelago loads every world in the checkout, and the ones
    # missing an optional dependency log a traceback on the way past. None of
    # it is about this one.
    sys.path.insert(0, str(checkout))
    os.chdir(checkout)
    import Options

    with tempfile.TemporaryDirectory() as staging:
        # Archipelago's own generator, writing one file per world it can see.
        # Ours is picked out of the pile afterwards rather than asked for
        # alone, because the template's shape is Archipelago's business and
        # reaching past this call would mean owning a copy of it.
        Options.generate_yaml_templates(staging, generate_hidden=True)
        made = Path(staging) / f"{GAME}.yaml"
        if not made.is_file():
            # Loudly, because the failure this guards is silent: a world that
            # did not load leaves the pile full of everybody else's templates
            # and nothing of ours, and an empty output would ship.
            others = sorted(p.name for p in Path(staging).glob("*.yaml"))
            print(
                f"error: Archipelago generated no template for {GAME!r}. "
                f"It wrote {len(others)} others, so it ran and this world did "
                f"not load. Is the .apworld installed?",
                file=sys.stderr,
            )
            return 1
        out.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(made, out)

    print(f"wrote {out} ({out.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
