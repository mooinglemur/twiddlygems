"""Twiddly Gems, as Archipelago sees it.

Nothing in this package decides anything. What the items are, where they can be
found and what each place asks for first are settled in the engine, in
``src/progression.rs``, because the solo game answers those same questions from
those same tables and the two have to be one answer rather than two. The engine
writes them out with ``cargo run --bin apworld``; ``data/`` next to this file is
that output, and everything below is the reading of it.

The rules come over as rules, not as prose to be reimplemented. Archipelago's
rule builder serializes to dicts and reads them back with ``rule_from_dict``, so
a requirement written once in Rust arrives here as the real thing: ``And``,
``Has``, ``CanReachLocation``, ``True_``. Regenerate the data and the logic here
follows, with nothing to keep in step by hand.

``data/`` holds four files: the items, the locations with their rules, the
settings a player can choose, and the world itself (its name, its ladder and
its goal). None of them are checked in.
"""

from __future__ import annotations

import dataclasses
import json
import pkgutil
from typing import Any

from BaseClasses import Item, ItemClassification, Location, Region
from Options import Choice, PerGameCommonOptions, Range
from rule_builder.field_resolvers import FromOption
from worlds.AutoWorld import World

def _data(name: str) -> Any:
    """Reads one of the generated files.

    Through the loader rather than off the filesystem. An installed .apworld is
    a zip, and a module inside one has no directory to read a file out of:
    opening it by path works in a checkout, where the tests run, and fails for
    every player who installed the zip.
    """
    raw = pkgutil.get_data(__package__, f"data/{name}")
    if raw is None:  # pragma: no cover - a package missing its own data
        raise RuntimeError(f"twiddlygems is missing data/{name}; write it with 'make apdata'")
    return json.loads(raw.decode("utf-8"))


GAME_DATA: dict[str, Any] = _data("game.json")
ITEMS: list[dict[str, Any]] = _data("items.json")
LOCATIONS: list[dict[str, Any]] = _data("locations.json")
SETTINGS: list[dict[str, Any]] = _data("options.json")

CLASSIFICATIONS = {
    "progression": ItemClassification.progression,
    "useful": ItemClassification.useful,
    "filler": ItemClassification.filler,
    "trap": ItemClassification.trap,
}

ITEMS_BY_NAME = {item["name"]: item for item in ITEMS}

#: The items there may be more of than the pool asks for. The engine says
#: which, for the same reason it says everything else here. There is one, and
#: a world with none of them could not fill its own locations, so this is
#: checked at import rather than found out during a generation.
TOP_UP_NAMES = [item["name"] for item in ITEMS if item["top_up"]]
assert TOP_UP_NAMES, "the engine named no item that may top up empty locations"


def _build_options() -> type[PerGameCommonOptions]:
    """Turns the settings table into real Option classes and a dataclass.

    Generated rather than written out because the rules point at these classes
    by name: a rule that depends on a setting carries the dotted path to its
    class and imports it, so the class has to exist here, under exactly the
    name the engine said it would. Writing them by hand would mean two lists to
    keep in step, which is the arrangement this whole world exists to avoid.
    """
    fields: dict[str, type] = {}
    for setting in SETTINGS:
        module, _, name = setting["ap_class"].rpartition(".")
        if module != __name__:
            raise RuntimeError(
                f"the engine expects this package at {module}, but it is {__name__}; "
                "the dotted paths in the rules will not resolve"
            )
        body: dict[str, Any] = {
            "display_name": setting["label"],
            "__doc__": setting["about"],
            "default": setting["default"],
        }
        if setting["kind"] == "range":
            body["range_start"] = setting["low"]
            body["range_end"] = setting["high"]
            base: type = Range
        else:
            for choice in setting["choices"]:
                body[f"option_{choice['key']}"] = choice["value"]
            base = Choice
        option = type(name, (base,), body)
        globals()[name] = option
        fields[setting["key"]] = option

    return dataclasses.make_dataclass(
        "TwiddlyGemsOptions",
        [(key, option) for key, option in fields.items()],
        bases=(PerGameCommonOptions,),
    )


TwiddlyGemsOptions = _build_options()


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
    options_dataclass = TwiddlyGemsOptions
    options: TwiddlyGemsOptions  # type: ignore[valid-type]
    # Every location hangs off the one region, so there is no map to speak of:
    # what gates a level is the level below it, expressed as a rule rather than
    # as a connection.
    topology_present = False

    item_name_to_id = {item["name"]: item["id"] for item in ITEMS}
    location_name_to_id = {at["name"]: at["id"] for at in LOCATIONS}

    def _ap_gems_per_level(self) -> int:
        """How many of each level's ten Archipelago gems this run plays over.

        The setting is a floor, not a count: every item has to have somewhere
        to go, and the options deciding how many items there are do not know
        how many places there are to put them. So when the pool outgrows
        everywhere else, the gems make up the difference.

        The engine works the same number out the same way for a solo run. Both
        sides have to land on it exactly, or a seed's locations and the game's
        would not be the same set, which is why this counts every other
        location rather than only the ones a solo fill likes.
        """
        levels = len(GAME_DATA["levels"])
        pool = sum(self._count(item["count"]) for item in ITEMS)
        elsewhere = sum(1 for at in LOCATIONS if "gem_index" not in at)
        needed = max(0, -(-(pool - elsewhere) // levels)) if levels else 0
        return min(
            max(self.options.ap_gems.value, needed), GAME_DATA["ap_gems_per_level"]
        )

    def _locations_in_play(self) -> list[dict[str, Any]]:
        """The locations this run actually has.

        Every gem is in the table because the table is a datapackage and
        fixed for everybody; the ones past what this run asked for are not in
        its world. Nothing will ever spawn for them, so an item left in one
        could never be found.
        """
        gems = self._ap_gems_per_level()
        return [at for at in LOCATIONS if at.get("gem_index", 0) < gems or "gem_index" not in at]

    def create_regions(self) -> None:
        menu = Region(self.origin_region_name, self.player, self.multiworld)
        menu.locations += [
            TwiddlyGemsLocation(self.player, at["name"], at["id"], menu)
            for at in self._locations_in_play()
        ]
        self.multiworld.regions.append(menu)

    def create_item(self, name: str) -> TwiddlyGemsItem:
        data = ITEMS_BY_NAME[name]
        return TwiddlyGemsItem(
            name, CLASSIFICATIONS[data["classification"]], data["id"], self.player
        )

    def get_filler_item_name(self) -> str:
        """What goes in a location with nothing better in it.

        There is exactly one such item and the engine says which, the same way
        it says everything else here. A spare unlock would be a second answer
        to a question the rules have settled, and a spare moves upgrade is
        worth nothing, since a level's upgrade lands whole and once.
        """
        return TOP_UP_NAMES[0]

    def _count(self, count: Any) -> int:
        """How many of an item this run's pool holds.

        A number for most, and for some a pointer at the setting that decides,
        which Archipelago's own resolver reads. Either way the engine said it;
        nothing here knows which items depend on what.
        """
        if isinstance(count, dict):
            return int(FromOption.from_dict(count).resolve(self))
        return int(count)

    def create_items(self) -> None:
        pool = [
            self.create_item(item["name"])
            for item in ITEMS
            for _ in range(self._count(item["count"]))
        ]
        # A world submits as many items as it has locations. The game has more
        # places to look than things to find, which is the shape that leaves
        # room for other worlds' items, and here it means topping up with
        # filler until the two match.
        #
        # Against the locations this run has rather than every name in the
        # table: the gems it did not ask for are not in its world, and filling
        # for them would submit more items than there are places.
        while len(pool) < len(self._locations_in_play()):
            pool.append(self.create_filler())
        self.multiworld.itempool += pool

    def set_rules(self) -> None:
        for at in self._locations_in_play():
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
