"""What the rules promise, asked of Archipelago's own state machine."""

from . import TwiddlyGemsTestBase
from .. import GAME_DATA

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

    def test_the_goal_is_the_end_of_the_ladder(self) -> None:
        # Beaten with everything, and not before. A goal that asks for nothing
        # would pass the first half of this and leave the generator looking at
        # a game already won: an empty playthrough, no spheres, and every item
        # in the world optional. That is what clearing the last level would
        # be, since clearing a level asks for no items at all.
        self.assertBeatable(False)
        self.collect_all_but([])
        self.assertBeatable(True)
