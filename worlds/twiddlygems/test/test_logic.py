"""What the rules promise, asked of Archipelago's own state machine."""

from . import TwiddlyGemsTestBase
from .. import GAME_DATA, ITEMS_BY_NAME, LOCATIONS, SETTINGS


LEVELS = GAME_DATA["levels"]

#: The items with no count of their own, which split a total between them. How
#: many of one of these a world holds is a roll rather than a number, so they
#: are the exception to every check below that counts copies.
SHARED = [
    item["name"]
    for item in ITEMS_BY_NAME.values()
    if isinstance(item["count"], dict) and "share_of" in item["count"]
]

#: The ladder's own item, which the pool holds many of on purpose.
LADDER = "Progressive Level Unlock"

#: How many levels from the bottom go down bare-handed. Read from the engine
#: rather than written here, because it is the engine's decision and a second
#: copy would be one to keep in step.
FIRST_GATED_LEVEL = GAME_DATA["first_gated_level"]

UNLOCKS = [
    "Horizontal Line Clear",
    "Vertical Line Clear",
    "Cross Clear",
    "Rainbow",
    "Rocket",
]


class TestDefault(TwiddlyGemsTestBase):
    """The default yaml, which is the only yaml there is so far."""

    options: dict = {}

    def test_the_opening_level_needs_nothing(self) -> None:
        # Sphere one. Until this goes down there is not a location in the game
        # a player can check, so a seed where it asks for anything is a seed
        # nobody can start.
        self.assertTrue(self.can_reach_location("Level 1 Clear"))

    def test_the_ladder_is_shut_past_the_level_a_run_has_reached(self) -> None:
        # The default opens the ladder by item, so the clears are a real chain
        # of spheres rather than all sitting in the first one.
        self.assertTrue(self.can_reach_location("Level 1 Clear"))
        for index in range(2, len(LEVELS) + 1):
            self.assertFalse(
                self.can_reach_location(f"Level {index} Clear"),
                f"level {index} is open to a run that has found nothing",
            )

        # The whole ladder in hand opens the teaching half and no further: from
        # the sixth level on, a clear wants something to make as well as
        # somewhere to make it.
        self.collect_by_name(LADDER)
        for index in range(1, FIRST_GATED_LEVEL + 1):
            self.assertTrue(
                self.can_reach_location(f"Level {index} Clear"),
                f"level {index} asks for more than its place in the ladder",
            )
        for index in range(FIRST_GATED_LEVEL + 1, len(LEVELS) + 1):
            self.assertFalse(
                self.can_reach_location(f"Level {index} Clear"),
                f"level {index} goes down bare-handed, past where a tool is asked for",
            )

        # Any one of them is enough for the middle of the ladder. The top is
        # not in that band: it wants the set, and is checked on its own below.
        self.collect_by_name(UNLOCKS[0])
        for index in range(FIRST_GATED_LEVEL + 1, len(LEVELS)):
            self.assertTrue(
                self.can_reach_location(f"Level {index} Clear"),
                f"level {index} wants more than one special, where the rule says any",
            )
        self.assertFalse(
            self.can_reach_location(f"Level {len(LEVELS)} Clear"),
            "the top of the ladder went down one special short of the set",
        )
        self.collect_by_name(UNLOCKS[1:])
        self.assertTrue(
            self.can_reach_location(f"Level {len(LEVELS)} Clear"),
            "the top of the ladder will not go down holding every special",
        )

    def test_each_special_on_its_own_opens_the_middle_of_the_ladder(self) -> None:
        # The rule says *any* of the five, so it has to hold for each of them
        # one at a time. A rule that were true of only some would be true on
        # some seeds and false on others, which is the shape of thing that
        # strands a player halfway up with no way to say why.
        for special in UNLOCKS:
            state = self.multiworld.get_all_state(False)
            for _ in range(len(LEVELS)):
                state.collect(self.world.create_item(LADDER), prevent_sweep=True)
            state.collect(self.world.create_item(special), prevent_sweep=True)
            for index in range(FIRST_GATED_LEVEL + 1, len(LEVELS)):
                self.assertTrue(
                    self.multiworld.get_location(f"Level {index} Clear", self.player)
                    .can_reach(state),
                    f"level {index} will not go down holding only {special}",
                )

    def test_a_score_mark_wants_the_specials(self) -> None:
        # Nearly all of a good score comes from the flourish at the end of a
        # level, and the flourish has nothing to mint without the unlocks. A
        # run holding none of them is at best flipping a coin for a mark, and
        # logic should not depend on a coin landing.
        self.assertFalse(self.can_reach_location("Level 1 Silver"))
        for unlock in UNLOCKS[:-1]:
            self.collect_by_name(unlock)
            self.assertFalse(
                self.can_reach_location("Level 1 Silver"),
                f"silver was reachable while {UNLOCKS[-1]} was still missing",
            )
        self.collect_by_name(UNLOCKS[-1])
        self.assertTrue(self.can_reach_location("Level 1 Silver"))

    def test_a_gold_wants_that_levels_own_moves(self) -> None:
        # Gold means beating a level as well as it can be beaten, so
        # everything that level has to offer should be in hand first. The
        # upgrade for it may be found anywhere at all, including on a later
        # level, which is exactly why it cannot be kept on this one.
        # Level two first, since the ladder opens by item: a mark on a level
        # nobody can play is a mark nobody can reach.
        self.collect_by_name("Progressive Level Unlock")
        self.collect_by_name(UNLOCKS)
        self.assertTrue(self.can_reach_location("Level 2 Silver"))
        self.assertFalse(self.can_reach_location("Level 2 Gold"))

        # One item, holding the whole of what that level grants. The pool may
        # hold more than one of it, because leftover locations are topped up
        # with upgrades, so this takes a single copy rather than all of them.
        upgrades = self.get_items_by_name("Level 2 Moves Upgrade")
        self.assertGreaterEqual(len(upgrades), 1, "that level's upgrade is not in the pool")
        self.collect(upgrades[0])
        self.assertTrue(self.can_reach_location("Level 2 Gold"))

    def test_only_the_gems_this_run_asked_for_are_in_its_world(self) -> None:
        # Every level's ten gems are in the table, because the table is a
        # datapackage and the same for everybody. A run plays over as many of
        # them as it asked for, and no more: nothing spawns for the rest, so
        # an item left in one could never be found.
        levels = len(LEVELS)
        self.assertEqual(self.world._ap_gems_per_level(), 1, "the default is one a level")

        named = [at for at in LOCATIONS if "gem_index" in at]
        self.assertEqual(
            len(named),
            levels * GAME_DATA["ap_gems_per_level"],
            "the table should hold every level's ten however few are played",
        )
        in_play = [at for at in self.world._locations_in_play() if "gem_index" in at]
        self.assertEqual(len(in_play), levels, "one gem a level should be in play")
        self.assertTrue(
            all(at["gem_index"] == 0 for at in in_play),
            "the gems in play should be the start of each level's sequence",
        )

        # And the world built only those: asking for a location it did not
        # create raises.
        self.world.get_location("Level 1 AP Gem 1")
        with self.assertRaises(KeyError):
            self.world.get_location("Level 1 AP Gem 2")

    def test_the_leftover_locations_hold_filler_and_nothing_borrowed(self) -> None:
        # There are more places to look than things to find, so the rest are
        # topped up. What with matters: a spare unlock would be a second
        # answer to a question the rules have settled, and a spare moves
        # upgrade is worth nothing at all, because a level's upgrade lands
        # whole and once. Either would tell the player they had found
        # something when they had not.
        self.assertEqual(self.world.get_filler_item_name(), "Filler")

        mine = [item for item in self.multiworld.itempool if item.player == self.player]
        self.assertEqual(
            len(mine),
            len(self.world._locations_in_play()),
            "a world submits as many items as it has locations",
        )
        by_name: dict[str, int] = {}
        for item in mine:
            by_name[item.name] = by_name.get(item.name, 0) + 1

        for name, count in by_name.items():
            if name == "Filler" or name in SHARED or name == LADDER:
                continue
            self.assertEqual(
                count, 1, f"{name} was submitted {count} times to fill the world out"
            )
        # The ladder's own item is the other exception, and for the opposite
        # reason to the shared ones: how many there are is not a roll but a
        # count, and it is the ladder's length plus the spares asked for.
        self.assertEqual(
            by_name.get(LADDER, 0),
            self.world._count(ITEMS_BY_NAME[LADDER]),
            "the world submitted a different number of level unlocks than its settings ask for",
        )
        self.assertGreater(by_name.get("Filler", 0), 0, "nothing filled the leftovers")

        # The shared items are exempt above because how many of each there are
        # is a roll. What is not a roll is how many there are altogether: the
        # setting says so, and a world that submitted a different number would
        # be handing the player a different game than they asked for.
        self.assertEqual(
            sum(by_name.get(name, 0) for name in SHARED),
            self.world.options.inventory_items.value,
            "the world submitted a different number of bonus items than were asked for",
        )
        # And every one of the four has to be able to come up, or a kind of
        # item would be in the table and never in anybody's game.
        self.assertGreater(len(SHARED), 1, "nothing is sharing a total, so this checks nothing")

    def test_a_shallow_chain_is_open_to_anybody_and_a_deep_one_is_not(self) -> None:
        # A chain up to the reliable depth is made on whatever board is in
        # front of you, and the opening one is in front of everybody.
        reliable = GAME_DATA["reliable_chain"]
        for length in range(GAME_DATA["shortest_chain"], reliable + 1):
            self.assertTrue(self.can_reach_location(f"{length} Chain"))

        # Past that it asks for a rocket and for something to open the board
        # up with. Every length used to be reachable from nothing, and this
        # world believed it: a real seed put a Rocket behind an eleven chain
        # and a level unlock behind a twelve, both in the opening sphere, on
        # a board a bare playthrough reaches one time in ten.
        deep = range(reliable + 1, GAME_DATA["longest_chain"] + 1)
        for length in deep:
            self.assertFalse(
                self.can_reach_location(f"{length} Chain"),
                f"a {length} chain is offered to a run holding nothing",
            )

        # A rocket on its own is not enough, and neither is an opener on its
        # own: the rule wants both halves.
        self.collect_by_name("Rocket")
        for length in deep:
            self.assertFalse(
                self.can_reach_location(f"{length} Chain"),
                f"a {length} chain came with a rocket and nothing to open the board",
            )
        self.collect_by_name("Cross Clear")
        for length in deep:
            self.assertTrue(
                self.can_reach_location(f"{length} Chain"),
                f"a {length} chain will not open to a rocket and a cross",
            )

    def test_both_line_clears_are_the_other_way_into_a_deep_chain(self) -> None:
        # The rule takes a cross *or* both line clears, so the pair has to work
        # on its own. A rule true of only one of its branches would be true on
        # some seeds and false on others.
        deep = range(GAME_DATA["reliable_chain"] + 1, GAME_DATA["longest_chain"] + 1)
        self.collect_by_name(["Rocket", "Horizontal Line Clear"])
        for length in deep:
            self.assertFalse(
                self.can_reach_location(f"{length} Chain"),
                f"a {length} chain took one line clear where it asks for both",
            )
        self.collect_by_name("Vertical Line Clear")
        for length in deep:
            self.assertTrue(
                self.can_reach_location(f"{length} Chain"),
                f"a {length} chain will not open to a rocket and both line clears",
            )

    def test_an_item_count_that_points_at_a_setting_resolves(self) -> None:
        # How many of an item the pool holds may be written as a pointer at
        # the setting that decides rather than as a number, because the
        # apworld is generated once and read by everybody.
        #
        # No item uses it at the moment: the ones with a count have a number,
        # and the ones without share a total, which is a different shape. The
        # resolver still has to work, because the settings that need it are
        # the next ones in, and an unused path that nothing checks is one that
        # breaks quietly. So this asks it directly, with a pointer built here
        # rather than one taken from the tables.
        self.assertEqual(self.world._count(ITEMS_BY_NAME["Rainbow"]), 1)
        self.assertEqual(self.world._count(ITEMS_BY_NAME["Level 4 Moves Upgrade"]), 1)

        goal = next(setting for setting in SETTINGS if setting["key"] == "goal")
        pointer = {"resolver": "FromOption", "option": goal["ap_class"], "field": "value"}
        self.assertEqual(
            self.world._count({"name": "made up", "count": pointer}),
            self.world.options.goal.value,
            "a count did not follow its pointer to the setting",
        )

    def test_the_items_that_share_a_total_split_it(self) -> None:
        # Four kinds of bonus item and one setting saying how many there are
        # altogether. Which kind each one is comes out of a draw, so what can
        # be asked of the split is that it adds up and that it stays put: the
        # pool is counted more than once, and how many items there are decides
        # which locations this world has at all. A split that answered
        # differently the second time would build a world around one set of
        # places and fill a different one.
        wanted = self.world.options.inventory_items.value
        self.assertGreater(wanted, 0, "the default run asks for no bonus items")

        drawn = self.world._shares()
        self.assertEqual(sorted(drawn), sorted(SHARED))
        self.assertEqual(sum(drawn.values()), wanted, "the split does not add up to the total")
        self.assertEqual(drawn, self.world._shares(), "the split was rolled twice")

    def test_the_goal_is_the_end_of_the_ladder(self) -> None:
        # Beaten with everything, and not before. A goal that asks for nothing
        # would pass the first half of this and leave the generator looking at
        # a game already won: an empty playthrough, no spheres, and every item
        # in the world optional. That is what clearing the last level would
        # be, since clearing a level asks for no items at all.
        self.assertBeatable(False)
        self.collect_all_but([])
        self.assertBeatable(True)


class TestGemsTurnedUp(TwiddlyGemsTestBase):
    """Asked for more gems than the default, which is a bigger world."""

    options = {"ap_gems": 6}

    def test_the_world_grows_with_the_setting(self) -> None:
        self.assertEqual(self.world._ap_gems_per_level(), 6)
        in_play = [at for at in self.world._locations_in_play() if "gem_index" in at]
        self.assertEqual(len(in_play), len(LEVELS) * 6)
        self.world.get_location("Level 1 AP Gem 6")
        with self.assertRaises(KeyError):
            self.world.get_location("Level 1 AP Gem 7")


class TestNoGems(TwiddlyGemsTestBase):
    """Asked for none, which today's pool has room for."""

    options = {"ap_gems": 0}

    def test_a_run_can_ask_for_none(self) -> None:
        self.assertEqual(self.world._ap_gems_per_level(), 0)
        self.assertEqual(
            [at for at in self.world._locations_in_play() if "gem_index" in at], []
        )
        # And the seed is still one that can be generated and finished: the
        # items have to fit in what is left.
        self.assertBeatable(False)
        self.collect_all_but([])
        self.assertBeatable(True)


class TestTheLadderOpensByItem(TwiddlyGemsTestBase):
    """Progressive level unlock, which is the other way to climb the ladder.

    The setting that changes the shape of the world most: with it on, every
    level past the first is behind a count of one item, so a generator has a
    real ladder to fill along instead of a game that is open from the start.
    """

    options = {"progressive_levels": "on"}

    def _unlocks(self) -> list:
        """The copies in the pool, to be collected one at a time.

        `collect_by_name` takes every copy of a name at once, which is the
        wrong tool for a progressive item: it would open the whole ladder in
        one call and every assertion below would pass whatever the rules said.
        """
        return [
            item
            for item in self.multiworld.itempool
            if item.name == "Progressive Level Unlock"
        ]

    def test_a_level_is_shut_until_its_unlocks_turn_up(self) -> None:
        copies = self._unlocks()
        self.assertTrue(self.can_reach_location("Level 1 Clear"))
        self.assertFalse(self.can_reach_location("Level 2 Clear"))
        self.collect(copies[0])
        self.assertTrue(self.can_reach_location("Level 2 Clear"))
        self.assertFalse(self.can_reach_location("Level 3 Clear"))
        self.collect(copies[1])
        self.assertTrue(self.can_reach_location("Level 3 Clear"))

    def test_the_gems_on_a_level_are_shut_with_it(self) -> None:
        # A gem is collected by playing its level, so it asks for what playing
        # that level asks for. A run that could reach one on a level it cannot
        # play would be a seed with an item nobody can take.
        self.assertFalse(self.can_reach_location("Level 2 AP Gem 1"))
        self.collect(self._unlocks()[0])
        self.assertTrue(self.can_reach_location("Level 2 AP Gem 1"))

    def test_the_pool_holds_the_ladder_and_its_spares(self) -> None:
        # One per level past the first, plus the spares the setting asks for:
        # a fifth of twelve is 2.4, which is two.
        item = ITEMS_BY_NAME["Progressive Level Unlock"]
        needed = len(LEVELS) - 1
        self.assertEqual(self.world._count(item), needed + round(needed * 0.2))
        held = [
            name
            for name in self.multiworld.itempool
            if name.name == "Progressive Level Unlock"
        ]
        self.assertEqual(len(held), self.world._count(item))

    def test_a_seed_is_still_finishable(self) -> None:
        self.assertBeatable(False)
        self.collect_all_but([])
        self.assertBeatable(True)


class TestTheLadderOpensByClearing(TwiddlyGemsTestBase):
    """The other way round, which holds none of that item at all."""

    options = {"progressive_levels": "off"}

    def test_nothing_is_dealt_an_item_it_cannot_use(self) -> None:
        self.assertEqual(self.world._count(ITEMS_BY_NAME[LADDER]), 0)
        self.assertNotIn(LADDER, [item.name for item in self.multiworld.itempool])


class TestTheBonusItemsAreWeighted(TwiddlyGemsTestBase):
    """One kind asked for far more often than the other three.

    The weights are relative and nothing else: what they decide is each kind's
    share of a total the other setting names. A file that wants mostly rockets
    says so by making that line bigger than the rest.
    """

    options = {
        "inventory_items": 20,
        "inventory_item_chance": {
            "rocket": 1000,
            "rainbow": 1,
            "cross_clear": 1,
            "rocket_cluster": 1,
        },
    }

    def test_the_heavy_kind_takes_most_of_the_total(self) -> None:
        held = [item.name for item in self.multiworld.itempool if item.name in SHARED]
        self.assertEqual(len(held), 20, "the total is the other setting's business")
        rockets = [name for name in held if name == "Inventory Item: Rocket"]
        self.assertGreater(
            len(rockets),
            len(held) // 2,
            f"a kind weighted a thousand to one took {len(rockets)} of {len(held)}",
        )


class TestNoBonusItemsAtAll(TwiddlyGemsTestBase):
    """Every weight at nothing, which is how a file asks for none of them.

    The one answer a weighted draw cannot give on its own: a total of zero is
    not an even chance again, it is no item. Both sides stop in the same
    place, the engine in `inventory_pool` and this world in `_shares`.
    """

    options = {
        "inventory_items": 10,
        "inventory_item_chance": {
            "rocket": 0,
            "rainbow": 0,
            "cross_clear": 0,
            "rocket_cluster": 0,
        },
    }

    def test_none_of_them_are_in_the_world(self) -> None:
        held = [item.name for item in self.multiworld.itempool if item.name in SHARED]
        self.assertEqual(held, [], "a run that weighted every kind at nothing got some anyway")
        # And the world is still a world: it submits as many items as it has
        # places, with filler where the bonus items would have been.
        self.assertEqual(
            len([item for item in self.multiworld.itempool if item.player == self.player]),
            len(self.world._locations_in_play()),
        )

    def test_a_seed_is_still_finishable(self) -> None:
        self.collect_all_but([])
        self.assertBeatable(True)


class TestFalseIsAWayToSayOff(TwiddlyGemsTestBase):
    """A two-value setting written the way a player would write it.

    It is a Toggle rather than a Choice of two for exactly this: a yaml that
    says `false` should mean off, and so should `no` and `0`. A Choice would
    take only the words the engine happened to write down, and `false` would
    be an error on a line nobody can see the fault in.
    """

    options = {"progressive_levels": False}

    def test_it_reads_as_off(self) -> None:
        self.assertEqual(self.world.options.progressive_levels.value, 0)
        self.assertEqual(self.world._count(ITEMS_BY_NAME[LADDER]), 0)
        # And the ladder opens by clearing, which shows as the teaching half
        # being open to a run holding nothing. That is what off means, and it
        # is not true of the default.
        #
        # Only the teaching half: what the setting changes is how a level is
        # reached, not what clearing it takes, so the gate above the fifth
        # level is untouched by it.
        for index in range(1, FIRST_GATED_LEVEL + 1):
            self.assertTrue(self.can_reach_location(f"Level {index} Clear"))
        self.assertFalse(
            self.can_reach_location(f"Level {FIRST_GATED_LEVEL + 1} Clear"),
            "turning the ladder setting off also opened the levels that want a special",
        )


class TestClearingIsTheGoal(TwiddlyGemsTestBase):
    """The goal that asks for the least, on a ladder that opens by clearing.

    These two settings together used to make a world with nothing at all to
    find: clearing asked for no items and the ladder asked for none either, so
    a run was beatable the moment it started and every item in it was optional.

    It is not that any more, and this is where that shows. The top of the
    ladder wants all five specials whatever else is turned down, so the most
    relaxed slot anybody can ask for still has five things in it that have to
    be found. Which is the point of the gate: there is no longer a way to
    configure this game into having no progression.
    """

    options = {"goal": "clear_last_level", "progressive_levels": "off"}

    def test_the_specials_are_the_floor_under_every_other_setting(self) -> None:
        self.assertBeatable(False)
        self.collect_by_name(UNLOCKS)
        self.assertBeatable(True)


class TestClearingIsTheGoalOnAnItemLadder(TwiddlyGemsTestBase):
    """The same goal with the ladder opening by item, which is the default.

    The ladder is what makes this goal a goal. Reaching the last level means
    holding every unlock below it, so a generator has something to place and a
    playthrough has spheres, where the pair above has neither.
    """

    options = {"goal": "clear_last_level"}

    def test_the_ladder_and_the_specials_are_both_what_it_asks_for(self) -> None:
        self.assertBeatable(False)
        # The ladder alone reaches the top level and cannot clear it.
        self.collect_by_name(LADDER)
        self.assertBeatable(False)
        self.collect_by_name(UNLOCKS)
        self.assertBeatable(True)


class TestGoldEverywhereIsTheGoal(TwiddlyGemsTestBase):
    """And the other end, which wants everything the world has."""

    options = {"goal": "gold_on_every_level"}

    def test_it_wants_more_than_the_last_level(self) -> None:
        # Gold on the last level is not enough when every level wants one, so
        # the branches of the goal rule are doing their own work rather than
        # all collapsing onto the same thing.
        self.collect_by_name(LADDER)
        self.collect_by_name(UNLOCKS)
        self.collect_by_name(f"Level {len(LEVELS)} Moves Upgrade")
        self.assertTrue(self.can_reach_location(f"Level {len(LEVELS)} Gold"))
        self.assertBeatable(False)
        self.collect_all_but([])
        self.assertBeatable(True)


class TestWhatTheGameIsTold(TwiddlyGemsTestBase):
    """The slot data, which is everything a client needs and nothing it keeps.

    A browser has no memory of a seed it has never seen, and should need none
    for a seed it has. Whatever is missing here is something the player would
    have to be asked for or something a client would have to write down, and
    both of those are ways to be wrong later.
    """

    options: dict = {}

    def test_every_setting_crosses_the_wire(self) -> None:
        # By the engine's keys, one per line of its own settings table, so a
        # client can walk that table and set each one without knowing what any
        # of them mean. A setting the engine grows later has to arrive here on
        # its own or that stops being true.
        sent = self.world.fill_slot_data()["options"]
        expected = set()
        for setting in SETTINGS:
            if setting["kind"] == "weights":
                expected.update(weight["key"] for weight in setting["weights"])
            else:
                expected.add(setting["key"])
        self.assertEqual(set(sent), expected)
        self.assertTrue(all(isinstance(value, int) for value in sent.values()))

    def test_the_settings_sent_are_the_ones_the_world_was_built_with(self) -> None:
        sent = self.world.fill_slot_data()["options"]
        self.assertEqual(sent["goal"], self.world.options.goal.value)
        self.assertEqual(sent["progressive_levels"], 1, "the default is an item ladder")
        # And the weights, which are one option here and four settings there.
        self.assertEqual(sent["rocket"], 50)
        self.assertEqual(sent["rocket_cluster"], 50)

    def test_the_gems_a_level_carries_are_sent_as_well_as_derivable(self) -> None:
        # Sent on purpose although both sides can work it out, because the two
        # working it out differently is a silent failure: the board would spawn
        # gems for locations this seed does not have.
        data = self.world.fill_slot_data()
        self.assertEqual(data["ap_gems_per_level"], self.world._ap_gems_per_level())

    def test_the_ladder_is_named(self) -> None:
        self.assertEqual(self.world.fill_slot_data()["levels"], LEVELS)

    def test_the_seed_says_which_generator_built_it(self) -> None:
        # The first thing the game reads and the first thing it can refuse on.
        # Everything else in here is integers whose meaning is settled
        # elsewhere, and this is the only thing that says which elsewhere.
        data = self.world.fill_slot_data()
        self.assertIn("generator", data)
        self.assertIsInstance(data["generator"], int)
        self.assertEqual(data["generator"], GAME_DATA["generator"])


class TestWhatAnUnusualSlotIsTold(TwiddlyGemsTestBase):
    """The same, for a run that asked for something other than the defaults.

    The pair matters more than either alone: a slot data built out of the
    defaults rather than out of this run's settings would pass every check
    above and none of these.
    """

    options = {
        "goal": "gold_on_every_level",
        "ap_gems": 4,
        "ap_gem_odds": 70,
        "progressive_levels": False,
        "inventory_items": 12,
        "inventory_item_chance": {"rocket": 200, "rainbow": 0},
    }

    def test_it_is_told_what_this_run_asked_for(self) -> None:
        sent = self.world.fill_slot_data()["options"]
        self.assertEqual(sent["goal"], 3)
        self.assertEqual(sent["ap_gems"], 4)
        self.assertEqual(sent["ap_gem_odds"], 70)
        self.assertEqual(sent["progressive_levels"], 0)
        self.assertEqual(sent["inventory_items"], 12)

    def test_a_weight_left_out_of_the_file_is_sent_as_nothing(self) -> None:
        # Naming some of the lines is how a file asks for only those kinds, so
        # the two that were left out are zero rather than the default. The run
        # was built that way; what it is told has to say the same.
        sent = self.world.fill_slot_data()["options"]
        self.assertEqual(sent["rocket"], 200)
        self.assertEqual(sent["rainbow"], 0)
        self.assertEqual(sent["cross_clear"], 0, "a line nobody wrote came back as the default")
        self.assertEqual(sent["rocket_cluster"], 0)

    def test_the_gem_count_sent_is_this_run_s_own(self) -> None:
        data = self.world.fill_slot_data()
        self.assertEqual(data["ap_gems_per_level"], 4)
