"""The settings a player can choose, built from the engine's own table.

A module of its own named ``options``, because Archipelago requires it.
"""

from __future__ import annotations

import dataclasses
from typing import Any

from Options import Choice, OptionCounter, PerGameCommonOptions, Range, Toggle

from . import SETTINGS


def _build_options() -> type[PerGameCommonOptions]:
    """Turns the settings table into real Option classes and a dataclass."""
    fields: dict[str, type] = {}
    for setting in SETTINGS:
        module, _, name = setting["ap_class"].rpartition(".")
        if module != __name__:
            raise RuntimeError(
                f"the engine expects these classes at {module}, but they are in {__name__}; "
                "the dotted paths in the rules will not resolve"
            )
        body: dict[str, Any] = {
            "display_name": setting["label"],
            "__doc__": setting["about"],
            # Where this class lives,
            "__module__": __name__,
        }
        # A number for most, and a mapping for a set of weights, which carries
        # one per line instead.
        if "default" in setting:
            body["default"] = setting["default"]
        if setting["kind"] == "range":
            body["range_start"] = setting["low"]
            body["range_end"] = setting["high"]
            base: type = Range
        elif setting["kind"] == "weights":
            # A mapping with a line per kind, which is how Archipelago spells
            # a set of relative chances. A Counter rather than a plain dict,
            # so a file naming only some of them reads the rest as nothing:
            # writing one line is a way of asking for only that kind.
            body["default"] = {
                weight["key"]: weight["default"] for weight in setting["weights"]
            }
            body["valid_keys"] = [weight["key"] for weight in setting["weights"]]
            body["min"] = 0
            body["max"] = setting["most"]
            base = OptionCounter
        elif setting["kind"] == "toggle":
            # Archipelago's own two-value type, which takes true, on, yes and
            # 1 alike, and their opposites. A two-value Choice would take only
            # the words the engine wrote down, so `true` in a player's file
            # would be an error on a line that looks perfectly reasonable.
            base = Toggle
        else:
            for choice in setting["choices"]:
                body[f"option_{choice['key']}"] = choice["value"]
            base = Choice
        option = type(name, (base,), body)
        # Bound as a module attribute as well as named: pickle finds a class by
        # importing the module it claims and looking the name up on it, and the
        # rules resolve their dotted paths the same way.
        globals()[name] = option
        fields[setting["key"]] = option

    built = dataclasses.make_dataclass(
        "TwiddlyGemsOptions",
        [(key, option) for key, option in fields.items()],
        bases=(PerGameCommonOptions,),
    )
    # The same correction as above, for the same reason: a dataclass built at
    # runtime is attributed to whatever frame built it. Set after the fact
    # rather than through `make_dataclass(module=...)`, which only arrived in
    # Python 3.12 and would tie this file to it for nothing.
    built.__module__ = __name__
    return built


TwiddlyGemsOptions = _build_options()
