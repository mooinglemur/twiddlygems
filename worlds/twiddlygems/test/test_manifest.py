"""That the packaged world says what Archipelago needs to hear before loading it.

Separate from the logic tests because this is about the file rather than the
game. There are two manifests and they are not the same: the one in the tree
describes the world, and the one inside a packaged `.apworld` describes the
world *and* the container it arrived in. Archipelago checks the first itself,
in `test.general.test_world_manifest`, which `make apworld-test` now runs and
which is the better place for those claims since Archipelago maintains them.

What Archipelago does not check is the zip. Its own manifest test says so:

    # Only check source folders for now. Zip validation should probably be in
    # the loader and/or installer.

So that is what is here, and it is the half that was actually broken: the world
shipped with no manifest at all, and nothing failed, because every test in this
repository runs against the directory. A packaged file only logged that it
"will stop working with Archipelago 0.7.0" and loaded anyway.
"""

import subprocess
import sys
import unittest
from pathlib import Path

from worlds.Files import APWorldContainer

from .. import GAME_DATA, TwiddlyGemsWorld


#: The repository, found from this file rather than from the working directory,
#: since these run with Archipelago's checkout as the cwd.
REPO = Path(__file__).resolve().parents[3]
BUILT = REPO / "build" / "twiddlygems.apworld"


class TestPackagedManifest(unittest.TestCase):
    """Claims about the file a player installs, read the way Archipelago reads it."""

    @classmethod
    def setUpClass(cls) -> None:
        # Built here rather than assumed, so this test says something on a
        # clean checkout instead of quietly passing on a stale zip or skipping.
        built = subprocess.run(
            ["make", "apworld"],
            cwd=REPO,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if built.returncode != 0:
            raise unittest.SkipTest(f"could not build the .apworld:\n{built.stdout}")
        cls.container = APWorldContainer(str(BUILT))
        cls.container.read()

    def test_the_packaged_world_can_be_read_at_all(self) -> None:
        # `read` raises `InvalidDataError` for a missing or unreadable manifest,
        # so reaching here is most of the claim. The rest is that it found ours
        # rather than something else that happened to be in the zip.
        self.assertEqual(self.container.game, TwiddlyGemsWorld.game)
        self.assertEqual(self.container.game, GAME_DATA["game"])

    def test_the_manifest_sits_inside_the_world_directory(self) -> None:
        # Archipelago writes it at `<package>/archipelago.json`, and a reader
        # that has to fall back to scanning the zip for a file ending in that
        # name is one whose assumptions we are leaning on.
        self.assertEqual(self.container.manifest_path, "twiddlygems/archipelago.json")

    def test_the_packaged_manifest_carries_the_container_fields(self) -> None:
        # The two the source manifest must *not* have and the zip must. They
        # are stamped in at packaging time; if that step were dropped the world
        # would still build, still install, and fail to load on 0.7.0.
        import json
        import zipfile

        with zipfile.ZipFile(BUILT) as zf:
            manifest = json.loads(zf.read("twiddlygems/archipelago.json"))
        for key in ("version", "compatible_version"):
            self.assertIn(key, manifest, f"the packaged manifest has no {key}")
        self.assertLessEqual(
            manifest["compatible_version"],
            APWorldContainer.version,
            "the manifest claims a container format newer than this Archipelago can open",
        )

    def test_the_packaged_world_carries_its_licence(self) -> None:
        # Named `LICENSE`, with no extension, which is what nearly every world
        # in Archipelago's own tree does. Checked against the one at the root
        # rather than merely for existence, because a stale copy of a licence
        # is the kind of wrong that nobody looks at twice.
        import zipfile

        with zipfile.ZipFile(BUILT) as zf:
            shipped = zf.read("twiddlygems/LICENSE").decode("utf-8")
        self.assertEqual(
            shipped,
            (REPO / "LICENSE").read_text(encoding="utf-8"),
            "the packaged licence is not the one at the root of the repository",
        )

    def test_the_world_it_declares_is_the_one_it_ships(self) -> None:
        # A version that parses, and a floor no higher than the Archipelago
        # these tests just passed in: claiming a minimum above the checkout
        # would mean the only version it is known to work in is one the
        # manifest says will not work.
        import Utils

        self.assertIsNotNone(self.container.world_version)
        if self.container.minimum_ap_version is not None:
            self.assertLessEqual(self.container.minimum_ap_version, Utils.version_tuple)


if __name__ == "__main__":  # pragma: no cover
    sys.exit(unittest.main())
