"""What the rules promise, asked of Archipelago's own state machine."""

from . import TwiddlyGemsTestBase
from .. import GAME_DATA, ITEMS_BY_NAME, LOCATIONS, SETTINGS


def count_of(name: str):
    """How many of an item the pool asks for: a number, or a pointer at a
    setting for the world to follow."""
    return ITEMS_BY_NAME[name]["count"]

LEVELS = GAME_DATA["levels"]
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

    def test_the_whole_ladder_is_open_to_a_player_holding_nothing(self) -> None:
        # A level is gated on the one below it and on nothing else, and no
        # level asks for an item, so the chain bottoms out at a player who has
        # found nothing: every clear on the ladder is in the first sphere.
        #
        # This is the claim that costs something. It means every level has to
        # be beatable on its own move budget, because a generator believing
        # this will happily put the last unlock behind the last level.
        for index in range(1, len(LEVELS) + 1):
            self.assertTrue(
                self.can_reach_location(f"Level {index} Clear"),
                f"level {index} asks for an item before it can be cleared",
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

    def test_a_chain_is_open_to_anybody(self) -> None:
        # A chain is made on whatever board is in front of you, and the
        # opening one is in front of everybody. The deep ones are rare rather
        # than gated, and nothing this world places is kept behind one.
        for length in range(GAME_DATA["shortest_chain"], GAME_DATA["longest_chain"] + 1):
            self.assertTrue(self.can_reach_location(f"{length} Chain"))

    def test_an_item_count_that_points_at_a_setting_resolves(self) -> None:
        # How many of an item the pool holds may be written as a pointer at
        # the setting that decides rather than as a number, because the
        # apworld is generated once and read by everybody.
        #
        # Nothing uses it at the moment: every item is one of one. The
        # resolver still has to work, because the settings that need it are
        # the next ones in, and an unused path that nothing checks is one
        # that breaks quietly. So this asks it directly, with a pointer built
        # here rather than one taken from the tables.
        self.assertEqual(self.world._count(count_of("Rainbow")), 1)
        self.assertEqual(self.world._count(count_of("Level 4 Moves Upgrade")), 1)

        goal = next(setting for setting in SETTINGS if setting["key"] == "goal")
        pointer = {"resolver": "FromOption", "option": goal["ap_class"], "field": "value"}
        self.assertEqual(
            self.world._count(pointer),
            self.world.options.goal.value,
            "a count did not follow its pointer to the setting",
        )

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


class TestClearingIsTheGoal(TwiddlyGemsTestBase):
    """The goal that asks for nothing, chosen on purpose.

    Worth having as a setting and worth knowing what it costs: clearing a
    level needs no items, so a run set this way is beatable the moment it
    starts and the generator will treat every item in the world as optional.
    Somebody who wants a relaxed slot should be able to ask for that.
    """

    options = {"goal": "clear_last_level"}

    def test_it_is_beatable_out_of_an_empty_inventory(self) -> None:
        self.assertBeatable(True)


class TestGoldEverywhereIsTheGoal(TwiddlyGemsTestBase):
    """And the other end, which wants everything the world has."""

    options = {"goal": "gold_on_every_level"}

    def test_it_wants_more_than_the_last_level(self) -> None:
        # Gold on the last level is not enough when every level wants one, so
        # the branches of the goal rule are doing their own work rather than
        # all collapsing onto the same thing.
        self.collect_by_name(UNLOCKS)
        self.collect_by_name(f"Level {len(LEVELS)} Moves Upgrade")
        self.assertTrue(self.can_reach_location(f"Level {len(LEVELS)} Gold"))
        self.assertBeatable(False)
        self.collect_all_but([])
        self.assertBeatable(True)
