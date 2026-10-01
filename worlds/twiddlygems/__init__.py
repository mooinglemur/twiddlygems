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
#: which, for the same reason it says everything else here. There is one, and
#: a world with none of them could not fill its own locations, so this is
#: checked at import rather than found out during a generation.
TOP_UP_NAMES = [item["name"] for item in ITEMS if item["top_up"]]
assert TOP_UP_NAMES, "the engine named no item that may top up empty locations"


# The settings live in `options.py`, which is not a matter of taste: WebHost
# pickles an option value into its database and reads it back through a
# restricted unpickler that refuses any class outside a module whose name ends
# in "options". Built here, they were refused, and the world could not have
# been hosted. Imported after the data above, which that module reads.
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
    marks. Long chains are checks of their own. The ladder opens by clearing
    the level below, or, with progressive level unlock on, by finding the
    items that open it.
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
        """How many of each level's ten AP gems this run plays over.

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
        # A run can ask for nothing worth finding to be placed behind a score
        # mark, which is the same thing as naming every Silver and Gold in
        # `exclude_locations` and a great deal less typing. Archipelago's own
        # EXCLUDED is what says it: the location still exists and is still
        # checked, and the fill refuses to put anything advancement or useful
        # there. The engine raises the Archipelago gem floor to match, because
        # shutting two locations a level takes them away from exactly the items
        # that need somewhere to go.
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
        """What goes in a location with nothing better in it.

        There is exactly one such item and the engine says which, the same way
        it says everything else here. A spare unlock would be a second answer
        to a question the rules have settled, and a spare moves upgrade is
        worth nothing, since a level's upgrade lands whole and once.
        """
        return TOP_UP_NAMES[0]

    def _option(self, ap_class: str) -> int:
        """What one setting is set to, named by the class the engine gave it.

        The tables point at settings by class path rather than by key, because
        that is what the rules do, and one way of naming a setting is enough.
        """
        key = next(setting["key"] for setting in SETTINGS if setting["ap_class"] == ap_class)
        return int(getattr(self.options, key).value)

    def _shares(self) -> dict[str, int]:
        """How the items that share a total fall across the kinds sharing it.

        Some items have no count of their own. The bonus items are four kinds
        splitting one total: the setting says how many there are altogether,
        and which kind each one turns out to be is a draw at equal chance. So
        the split is rolled, here, with this slot's own generator: it belongs
        to one run, and the tables the engine writes are the same for every
        run there will ever be.

        Rolled once and kept. The pool gets counted more than once, and
        `_ap_gems_per_level` reads the count to decide which locations this
        world has at all: a second roll answering differently would hand the
        run a different set of places to look than the one it built.
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

        One per level past the first, which is what it takes to reach the top,
        plus the spares the setting asks for as a percentage of that, rounded
        to the nearest whole item. None at all when the run is not opening the
        ladder that way, and then nothing gates on them either.

        The engine works the same number out in `level_unlocks`. Neither side
        can read the other, so the table hands over the three numbers and both
        do the sum: they have to land on the same answer or this world submits
        a different number of items than it has places to put them.
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

    def _settings_sent(self) -> dict[str, int]:
        """Every setting this run was rolled with, keyed the way the engine keys it.

        The engine is set up by walking its own settings table and handing each
        line a number, which is what the solo screen does. A client can do the
        same thing with this: one key here for one line there, so a setting
        added to the engine later crosses the wire with nothing on either side
        edited to let it through.

        Which means the keys have to be the engine's, and for one setting they
        are not the same as ours. A set of relative weights is a single option
        in a yaml, spelled as a mapping with a line per kind, and four separate
        settings in the engine, one per kind. So it is expanded back out here.

        Read through `_weight` rather than off the mapping directly, because a
        missing line means nothing rather than the default: writing one line is
        how a file asks for only that kind. Two readings of that would be one
        reading too many, and the run this is describing was built with that
        one.
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

        Enough to put the run into the shape this seed was generated for,
        holding nothing else back. A browser that has never seen this seed and
        a browser coming back to it get the same thing, because none of this is
        remembered anywhere: the settings decide how many items there are, what
        the rules ask for and how the ladder opens, and a client that had to
        keep its own copy of them would be a copy that could go stale.

        `ap_gems_per_level` is the odd one out: it is worked out from the
        settings rather than set by anybody, so sending it is sending the same
        fact twice. That is deliberate, and it is a check rather than a source.
        Both sides derive it, by counting the pool against the places to put
        it, and if they ever land on different numbers the failure is silent
        and nasty: the board spawns gems for locations this seed does not have,
        and the checks behind them go nowhere. Better for a client to compare
        the two and refuse than to play a game that is subtly not the one that
        was generated.

        `generator` is the first thing the game reads and the first thing it
        can refuse on. Everything else here is only meaningful if the two sides
        agree about what the numbers in it mean, and nothing in a seed says so
        by itself: an item id is an integer whichever generation wrote it, and
        a setting's value is an integer whichever list it was an index into. So
        the generation says so explicitly, and a game that does not know the
        number stops rather than playing somebody else's rules.
        """
        return {
            "generator": GAME_DATA["generator"],
            "levels": GAME_DATA["levels"],
            "options": self._settings_sent(),
            "ap_gems_per_level": self._ap_gems_per_level(),
        }
