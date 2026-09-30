"""That the packaged world says what Archipelago needs to hear before loading it.

Separate from the logic tests because this is about the file rather than the
game: an `.apworld` is a zip that Archipelago reads through
`worlds.Files.APWorldContainer`, and that reader wants a manifest. A checkout
does not, which is why this went missing without anything noticing: every test
here runs against the directory, and the directory loads happily with no
manifest at all. The packaged file only logged that it "will stop working with
Archipelago 0.7.0", and on 0.7.0 it raises and the world does not load.
"""

import json
import unittest
from pathlib import Path

from worlds.Files import APWorldContainer
from Utils import tuplize_version

from .. import GAME_DATA, TwiddlyGemsWorld


#: The manifest as it sits in the package, which is the file Archipelago's own
#: packager reads and copies into the zip.
MANIFEST_PATH = Path(__file__).resolve().parent.parent / "archipelago.json"


class TestManifest(unittest.TestCase):
    """No multiworld needed: these are claims about a file."""

    def setUp(self) -> None:
        self.assertTrue(
            MANIFEST_PATH.is_file(),
            f"there is no manifest at {MANIFEST_PATH}, so a packaged .apworld cannot be read",
        )
        self.manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))

    def test_the_manifest_names_the_game_the_world_class_does(self) -> None:
        # Archipelago's packager asserts exactly this and refuses to build
        # otherwise, so a mismatch is a broken release rather than a warning.
        self.assertEqual(self.manifest.get("game"), TwiddlyGemsWorld.game)
        self.assertEqual(self.manifest.get("game"), GAME_DATA["game"])

    def test_the_manifest_carries_what_the_zip_reader_reads(self) -> None:
        # `APWorldContainer.read_contents` indexes both of these rather than
        # getting them with a default, so either one missing is a KeyError
        # dressed up as "this might be the incorrect world version for this
        # file", which says nothing about what is actually wrong.
        for key in ("game", "compatible_version"):
            self.assertIn(key, self.manifest, f"the zip reader reads {key} and would fail without it")

    def test_the_container_version_is_one_this_archipelago_can_open(self) -> None:
        # The reader refuses a manifest claiming a number above its own. Read
        # from the reader rather than written here, so the check is against the
        # Archipelago in the checkout instead of against a copy of a number.
        self.assertLessEqual(
            self.manifest["compatible_version"],
            APWorldContainer.version,
            "the manifest claims a container format newer than this Archipelago can open",
        )

    def test_the_versions_are_ones_archipelago_can_parse(self) -> None:
        # A version string Archipelago cannot tuplize takes the world down on
        # load, which is a worse failure than the one this file fixes.
        for key in ("world_version", "minimum_ap_version", "maximum_ap_version"):
            if key in self.manifest:
                tuplize_version(self.manifest[key])

    def test_the_floor_is_no_higher_than_the_archipelago_it_is_tested_in(self) -> None:
        # Claiming a minimum above the checkout would mean the version these
        # tests pass in is one the manifest says will not work.
        if "minimum_ap_version" not in self.manifest:
            self.skipTest("no floor declared, so there is nothing to disagree with")
        import Utils

        self.assertLessEqual(
            tuplize_version(self.manifest["minimum_ap_version"]),
            Utils.version_tuple,
            "the manifest's floor is above the Archipelago these tests just passed in",
        )
