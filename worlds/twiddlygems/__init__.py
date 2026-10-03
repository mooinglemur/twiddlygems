"""Twiddly Gems, as Archipelago sees it.

The source of truth for the logic sits in Rust code, which generates the
JSON bundled with the apworld.

``cargo run --bin apworld`` regenerates the data/*.json, none of which are
committed to the repo.
"""

from __future__ import annotations

import dataclasses
import json
import pkgutil
from typing import Any

from BaseClasses import Item, ItemClassification, Location, LocationProgressType, Region
from Options import Choice, OptionCounter, PerGameCommonOptions, Range, Toggle
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
#: which, for the same reason it says everything else here. They are the
#: filler, which do nothing but make a sound, and a world with none of them
#: could not fill its own locations, so this is checked at import rather than
#: found out during a generation.
TOP_UP_NAMES = [item["name"] for item in ITEMS if item["top_up"]]
assert TOP_UP_NAMES, "the engine named no top_up filler items"

#: The traps, which take a share of that top-up rather than a count of their
#: own. Read off the classification the engine already writes, so a trap added
#: there needs nothing done here.
#:
#: Deliberately *not* folded into ``TOP_UP_NAMES``. That list is what
#: ``get_filler_item_name`` draws from, and Archipelago calls that for its own
#: purposes as well as ours: filling a location somebody excluded, for one. A
#: trap reachable through it would turn up at a rate nobody set and nobody
#: could turn off, which is the opposite of what the percentage is for.
TRAP_NAMES = [item["name"] for item in ITEMS if item["classification"] == "trap"]


# The settings live in `options.py`, which is not a matter of taste: WebHost
# pickles an option value into its database and reads it back through a
# restricted unpickler that refuses any class outside a module whose name ends
# in "options".
from .options import TwiddlyGemsOptions  # noqa: E402


class TwiddlyGemsItem(Item):
    game = GAME_DATA["game"]


class TwiddlyGemsLocation(Location):
    game = GAME_DATA["game"]


class TwiddlyGemsWorld(World):
    """A match-3 game where the specials themselves are the progression.

    A new run matches and clears normally and leaves nothing behind: the line
    clearers, the cross, the rainbow and the rocket are all items. Each level
    is worth three checks, for clearing it and for clearing it past two score
    marks (Gold and Silver). Long chains are checks of their own. The ladder
    opens by clearing the level below, or, with progressive level unlock on, by
    finding the items that open it.
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

    #: The split of the items that share a total, once it has been rolled. See
    #: `_shares`, which rolls it and explains why it is only ever rolled once.
    _shares_drawn: dict[str, int] | None = None

    def _ap_gems_per_level(self) -> int:
        """The minimum number of AP gems per level.

        The setting is a floor, not a count: every item has to have somewhere
        to go, and the options deciding how many items there are do not know
        how many places there are to put them. So when the pool outgrows
        everywhere else, the gems make up the difference.
        """
        levels = len(GAME_DATA["levels"])
        pool = sum(self._count(item) for item in ITEMS)
        # Everywhere a fill will actually put something. `counts_as_room` is
        # how the engine marks a place that does not qualify, and it marks none
        # today: the deep chains were the only ones, back when their rule said
        # anybody could reach them and the solo fill refused to use them
        # anyway. They ask for a rocket and an opener now, both fills use them,
        # and both count them. The flag stays because the next location the
        # engine decides not to count will want it.
        elsewhere = sum(
            1 for at in LOCATIONS if "gem_index" not in at and at.get("counts_as_room", True)
        )
        short = pool - elsewhere

        # The second constraint, which only bites when a run has asked for
        # nothing worth finding behind its score marks. Those locations are
        # still places, so `short` above still counts them, but they stop being
        # places for anything that matters: the items which may not go there
        # have thirty-two fewer homes and have not gone away. Archipelago fails
        # such a seed outright rather than dealing a worse one, with either
        # "No more spots to place" or "Not enough filler items for excluded
        # locations", so the gems have to cover it before generation starts.
        barred = (
            sum(1 for at in LOCATIONS if at.get("is_mark"))
            if self.options.exclude_gold_and_silver
            else 0
        )
        # The same line Archipelago draws for an excluded location, which
        # refuses advancement and useful alike and takes filler happily. Traps
        # count as filler here for the same reason they do to Archipelago: a
        # trap is something it is willing to put in a place nobody has to go.
        important = sum(
            self._count(item)
            for item in ITEMS
            if item["classification"] not in ("filler", "trap")
        )
        tight = important - (elsewhere - barred)

        needed = max(0, -(-max(short, tight) // levels)) if levels else 0
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
        # A run can ask for filler/traps to be placed behind a score
        # mark, which is the same thing as naming every Silver and Gold in
        # `exclude_locations` and a great deal less verbose. Archipelago's own
        # EXCLUDED is what says it: the location still exists and is still
        # checked, and the fill refuses to put anything advancement or useful
        # there.
        shut_marks = bool(self.options.exclude_gold_and_silver)
        for at in self._locations_in_play():
            location = TwiddlyGemsLocation(self.player, at["name"], at["id"], menu)
            if shut_marks and at.get("is_mark"):
                location.progress_type = LocationProgressType.EXCLUDED
            menu.locations.append(location)
        self.multiworld.regions.append(menu)

    def create_item(self, name: str) -> TwiddlyGemsItem:
        data = ITEMS_BY_NAME[name]
        return TwiddlyGemsItem(
            name, CLASSIFICATIONS[data["classification"]], data["id"], self.player
        )

    def get_filler_item_name(self) -> str:
        return self.random.choice(TOP_UP_NAMES)

    def _option(self, ap_class: str) -> int:
        key = next(setting["key"] for setting in SETTINGS if setting["ap_class"] == ap_class)
        return int(getattr(self.options, key).value)

    def _shares(self) -> dict[str, int]:
        """How the items that share a total fall across the kinds sharing it.

        Some items have no count of their own. The bonus items are four kinds
        splitting one total: the setting says how many there are altogether,
        and which kind each one turns out to be is a draw at weighted chance.
        So the split is rolled, here, with this slot's own generator: it
        belongs to one run, and the tables the engine writes are the same for
        every run there will ever be.

        Rolled once and kept.
        """
        if self._shares_drawn is not None:
            return self._shares_drawn

        drawn: dict[str, int] = {}
        # By the total they name, so any number of items can share one, and
        # a second group sharing a different total would work the same way
        # without anything here being told about it. Each carries the weight
        # it is drawn against, which is a line in a mapping the player sets.
        sharing: dict[str, list[tuple[str, int]]] = {}
        for item in ITEMS:
            count = item["count"]
            share = count.get("share_of") if isinstance(count, dict) else None
            if share is None:
                continue
            drawn[item["name"]] = 0
            sharing.setdefault(share["option"], []).append(
                (item["name"], self._weight(share["weight"]))
            )

        for ap_class, members in sharing.items():
            names = [name for name, _ in members]
            weights = [weight for _, weight in members]
            # Every kind weighted at nothing is how a file says it wants none
            # of these at all, and it is the one answer a weighted draw cannot
            # give: `random.choices` refuses a total of zero, and it would be
            # wrong to fall back to an even draw when what was asked for was
            # none. The engine's `inventory_pool` stops in the same place.
            if sum(weights) == 0:
                continue
            for name in self.random.choices(
                names, weights=weights, k=self._option(ap_class)
            ):
                drawn[name] += 1

        self._shares_drawn = drawn
        return drawn

    def _weight(self, pointer: dict[str, Any]) -> int:
        """One line out of the mapping a set of weights is written as."""
        key = next(
            setting["key"] for setting in SETTINGS if setting["ap_class"] == pointer["option"]
        )
        return int(getattr(self.options, key).value.get(pointer["key"], 0))

    def _count(self, item: dict[str, Any]) -> int:
        """How many of an item this run's pool holds.

        A number for most, and for some a pointer at the setting that decides,
        which Archipelago's own resolver reads. For the ones with no count of
        their own it is a share of somebody else's total; see `_shares`. Either
        way the engine said which; nothing here knows what depends on what.
        """
        count = item["count"]
        if isinstance(count, dict):
            if "share_of" in count:
                return self._shares()[item["name"]]
            if "spare_percent" in count:
                return self._ladder(count)
            return int(FromOption.from_dict(count).resolve(self))
        return int(count)

    def _ladder(self, count: dict[str, Any]) -> int:
        """How many progressive level unlocks this run's pool holds.

        One per level past the first, unless progress_levels is false.
        """
        switch = count["only_when"]
        if self._option(switch["option"]) != int(switch["is"]):
            return 0
        needed = int(count["needed"])
        percent = self._option(count["spare_percent"]["option"])
        return needed + (needed * percent + 50) // 100

    def create_items(self) -> None:
        pool = [
            self.create_item(item["name"]) for item in ITEMS for _ in range(self._count(item))
        ]
        # A world submits as many items as it has locations. The game has more
        # places to look than things to find, which is the shape that leaves
        # room for other worlds' items, and here it means topping up with
        # filler until the two match.
        #
        # Optionally, some share of the top-up is a trap instead, gated by
        # `trap_percent`.
        #
        # Rolled per item rather than worked out as a quota, so a seed holds
        # roughly the share asked for.
        traps = int(self.options.trap_percent.value) if TRAP_NAMES else 0
        while len(pool) < len(self._locations_in_play()):
            if traps and self.random.randrange(100) < traps:
                pool.append(self.create_item(self.random.choice(TRAP_NAMES)))
            else:
                pool.append(self.create_filler())
        self.multiworld.itempool += pool

    def set_rules(self) -> None:
        for at in self._locations_in_play():
            self.set_rule(self.get_location(at["name"]), self.rule_from_dict(at["rule"]))
        self.set_completion_rule(self.rule_from_dict(GAME_DATA["goal"]))

    def _settings_sent(self) -> dict[str, int]:
        """Every setting this run was rolled with, keyed the way the engine keys it.

        The engine is set up by walking its own settings table and handing each
        line a number, which is what the solo screen does. The AP client
        portion can do the same thing with this: one key here for one line
        there, so a setting added to the engine later passes.
        """
        sent: dict[str, int] = {}
        for setting in SETTINGS:
            if setting["kind"] == "weights":
                for weight in setting["weights"]:
                    sent[weight["key"]] = self._weight(
                        {"option": setting["ap_class"], "key": weight["key"]}
                    )
                continue
            sent[setting["key"]] = int(getattr(self.options, setting["key"]).value)
        return sent

    def fill_slot_data(self) -> dict[str, Any]:
        """What the game itself is told when it connects.

        Enough to inform the engine the relevant options.

        `ap_gems_per_level` is the odd one out: it is worked out from the
        settings and may be higher than the minimum.

        `generator` is an integer, and the client makes the decision whether to
        continue to connect based on what compatibility it knows how to handle.
        """
        return {
            "generator": GAME_DATA["generator"],
            "levels": GAME_DATA["levels"],
            "options": self._settings_sent(),
            "ap_gems_per_level": self._ap_gems_per_level(),
        }
