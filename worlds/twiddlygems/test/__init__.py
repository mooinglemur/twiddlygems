"""Archipelago's own checks, run against the rules the engine wrote out.

`WorldTestBase` brings the ones every world has to pass: that nothing is
reachable from nowhere, that everything is reachable with everything, and that
a real fill can be made. Those are the same claims `progression.rs` makes about
the solo placement, asked here of Archipelago's generator instead of ours,
which is the point of emitting the rules rather than restating them.

The tests below that are our own are the ones about this game in particular.
"""

from test.bases import WorldTestBase

from .. import GAME_DATA


class TwiddlyGemsTestBase(WorldTestBase):
    game = GAME_DATA["game"]
    player: int = 1
