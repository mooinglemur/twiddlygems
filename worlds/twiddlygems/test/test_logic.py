"""What the rules promise, asked of Archipelago's own state machine."""

from . import TwiddlyGemsTestBase
from .. import GAME_DATA, ITEMS_BY_NAME


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
        # moves for it may be found anywhere at all, including on a later
        # level, which is exactly why they cannot be kept on this one.
        self.collect_by_name(UNLOCKS)
        self.assertTrue(self.can_reach_location("Level 2 Silver"))
        self.assertFalse(self.can_reach_location("Level 2 Gold"))

        # One at a time: `collect_by_name` takes every copy in the pool at
        # once, which would skip straight past the half that is the point.
        moves = self.get_items_by_name("Level 2 Progressive Moves")
        self.assertGreaterEqual(len(moves), 2, "the pool is short of that level's moves")
        self.collect(moves[0])
        self.assertFalse(
            self.can_reach_location("Level 2 Gold"),
            "one of the two move items was enough for gold",
        )
        self.collect(moves[1])
        self.assertTrue(self.can_reach_location("Level 2 Gold"))

    def test_a_chain_is_open_to_anybody(self) -> None:
        # A chain is made on whatever board is in front of you, and the
        # opening one is in front of everybody. The deep ones are rare rather
        # than gated, and nothing this world places is kept behind one.
        for length in range(GAME_DATA["shortest_chain"], GAME_DATA["longest_chain"] + 1):
            self.assertTrue(self.can_reach_location(f"{length} Chain"))

    def test_an_item_count_that_points_at_a_setting_resolves(self) -> None:
        # How many of a move item the pool holds is written as a pointer at
        # the setting that decides rather than as a number, because the
        # apworld is generated once and read by everybody. This is that
        # pointer being followed.
        #
        # Counting the pool would not show it: the world tops up its empty
        # locations with more move items, so what ends up in the pool is the
        # setting plus however much filler landed on the same level.
        self.assertEqual(self.world._count(count_of("Level 4 Progressive Moves")), 2)
        self.assertEqual(self.world._count(count_of("Rainbow")), 1)

    def test_the_goal_is_the_end_of_the_ladder(self) -> None:
        # Beaten with everything, and not before. A goal that asks for nothing
        # would pass the first half of this and leave the generator looking at
        # a game already won: an empty playthrough, no spheres, and every item
        # in the world optional. That is what clearing the last level would
        # be, since clearing a level asks for no items at all.
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
        self.collect_by_name(f"Level {len(LEVELS)} Progressive Moves")
        self.assertTrue(self.can_reach_location(f"Level {len(LEVELS)} Gold"))
        self.assertBeatable(False)
        self.collect_all_but([])
        self.assertBeatable(True)


class TestOneMovePerLevel(TwiddlyGemsTestBase):
    """The setting turned down, which changes the pool and the rules together."""

    options = {"moves_per_level": 1}

    def test_the_pool_and_the_rule_move_together(self) -> None:
        # One item per level asked for, and gold satisfied by one, both read
        # off the same setting. Either half alone would be a run that cannot
        # be finished: a gold wanting two of something there is only one of is
        # a location nobody can reach.
        self.assertEqual(self.world._count(count_of("Level 2 Progressive Moves")), 1)

        self.collect_by_name(UNLOCKS)
        self.assertFalse(self.can_reach_location("Level 2 Gold"))
        self.collect(self.get_items_by_name("Level 2 Progressive Moves")[0])
        self.assertTrue(
            self.can_reach_location("Level 2 Gold"),
            "gold still wants two moves when the setting says one",
        )
