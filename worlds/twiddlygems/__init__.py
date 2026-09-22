"""Twiddly Gems, as Archipelago sees it.

Nothing in this package decides anything. What the items are, where they can be
found and what each place asks for first are settled in the engine, in
``src/progression.rs``, because the solo game answers those same questions from
those same tables and the two have to be one answer rather than two. The engine
writes them out with ``cargo run --bin apworld``; ``game.json`` next to this
file is that output, and everything below is the reading of it.

The rules come over as rules, not as prose to be reimplemented. Archipelago's
rule builder serializes to dicts and reads them back with ``rule_from_dict``, so
a requirement written once in Rust arrives here as the real thing: ``And``,
``Has``, ``CanReachLocation``, ``True_``. Regenerate ``game.json`` and the logic
here follows, with nothing to keep in step by hand.
"""

from __future__ import annotations

import json
import pkgutil
from typing import Any

from BaseClasses import Item, ItemClassification, Location, Region
from Options import PerGameCommonOptions
from worlds.AutoWorld import World

# Through the loader rather than off the filesystem. An installed .apworld is a
# zip, and a module inside one has no directory to read a file out of: opening
# it by path works in a checkout, where the tests run, and fails for every
# player who installed the zip.
_DATA = pkgutil.get_data(__package__, "game.json")
if _DATA is None:  # pragma: no cover - a package missing its own data
    raise RuntimeError("twiddlygems is missing game.json; regenerate it with 'make apdata'")

GAME_DATA: dict[str, Any] = json.loads(_DATA.decode("utf-8"))

CLASSIFICATIONS = {
    "progression": ItemClassification.progression,
    "useful": ItemClassification.useful,
    "filler": ItemClassification.filler,
    "trap": ItemClassification.trap,
}

ITEMS_BY_NAME = {item["name"]: item for item in GAME_DATA["items"]}
LOCATIONS_BY_NAME = {at["name"]: at for at in GAME_DATA["locations"]}

#: The items there may be more of than the pool asks for. The engine says
#: which, for the same reason it says everything else here.
TOP_UP_NAMES = [item["name"] for item in GAME_DATA["items"] if item["top_up"]]


class TwiddlyGemsItem(Item):
    game = GAME_DATA["game"]


class TwiddlyGemsLocation(Location):
    game = GAME_DATA["game"]


class TwiddlyGemsWorld(World):
    """A match-3 game where the specials themselves are the progression.

    A new run matches and clears normally and leaves nothing behind: the line
    clearers, the cross, the rainbow and the rocket are all items. Levels are
    found by clearing the one below, and each one is worth three checks, for
    clearing it and for clearing it past two score marks. Long chains are
    checks of their own.
    """

    game = GAME_DATA["game"]
    options_dataclass = PerGameCommonOptions
    # Every location hangs off the one region, so there is no map to speak of:
    # what gates a level is the level below it, expressed as a rule rather than
    # as a connection.
    topology_present = False

    item_name_to_id = {item["name"]: item["id"] for item in GAME_DATA["items"]}
    location_name_to_id = {at["name"]: at["id"] for at in GAME_DATA["locations"]}

    def create_regions(self) -> None:
        menu = Region(self.origin_region_name, self.player, self.multiworld)
        menu.locations += [
            TwiddlyGemsLocation(self.player, at["name"], at["id"], menu)
            for at in GAME_DATA["locations"]
        ]
        self.multiworld.regions.append(menu)

    def create_item(self, name: str) -> TwiddlyGemsItem:
        data = ITEMS_BY_NAME[name]
        return TwiddlyGemsItem(
            name, CLASSIFICATIONS[data["classification"]], data["id"], self.player
        )

    def get_filler_item_name(self) -> str:
        """More moves on some level, which is the only harmless item there is.

        Everything else changes what a board can do. A copy past the two a
        level's gold asks for is pure score, so it can land anywhere without
        making a seed easier or harder to finish.
        """
        return self.random.choice(TOP_UP_NAMES)

    def create_items(self) -> None:
        pool = [
            self.create_item(item["name"])
            for item in GAME_DATA["items"]
            for _ in range(item["count"])
        ]
        # A world submits as many items as it has locations. The game has more
        # places to look than things to find, which is the shape that leaves
        # room for other worlds' items, and here it means topping up with
        # filler until the two match.
        while len(pool) < len(self.location_name_to_id):
            pool.append(self.create_filler())
        self.multiworld.itempool += pool

    def set_rules(self) -> None:
        for at in GAME_DATA["locations"]:
            self.set_rule(self.get_location(at["name"]), self.rule_from_dict(at["rule"]))
        self.set_completion_rule(self.rule_from_dict(GAME_DATA["goal"]))

    def fill_slot_data(self) -> dict[str, Any]:
        """What the game itself is told when it connects.

        The ladder, so the client can show the level picker without a second
        copy of the level list, and nothing else yet: what the run holds comes
        from the server as items, the same way the solo run gets it from its
        own placement.
        """
        return {"levels": GAME_DATA["levels"]}
