#!/usr/bin/env python3
"""Serves the game for development, with caching turned off.

`python3 -m http.server` sends `Last-Modified` and no `Cache-Control` at all.
With neither `Cache-Control` nor `Expires`, a browser falls back to heuristic
freshness: it invents a lifetime of roughly a tenth of the file's age and, for
that long, serves its copy without asking the server anything. So a file left
alone for a few days is treated as good for several hours after it is edited,
and the page goes on running the version from before the change with no request
in the network log to say so.

That is a bad way to spend an afternoon, and it is worse on a phone, where
forcing a reload past the cache is fiddly and there are no developer tools to
notice it with. The screenshot tooling already had to work around the same
thing from the other side, with `Network.setCacheDisabled`.

`no-store` rather than `no-cache`: no-cache still stores the response and only
promises to revalidate it, which is enough here but leaves a copy on the device
to go wrong later. Nothing served by this is worth keeping between requests.

    make serve
"""

from __future__ import annotations

import argparse
import functools
import http.server


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store, must-revalidate")
        super().end_headers()

    def log_message(self, format: str, *args: object) -> None:
        # One line per request, without the timestamp the base class prefixes:
        # this runs in a terminal somebody is watching, not into a log file.
        print(f"{self.address_string()} {format % args}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8000)
    # Everything by default, because the point of running this is usually to
    # open the game on a phone on the same network.
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--directory", default="web")
    args = parser.parse_args()

    handler = functools.partial(Handler, directory=args.directory)
    with http.server.ThreadingHTTPServer((args.bind, args.port), handler) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()
