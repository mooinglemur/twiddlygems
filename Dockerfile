# The whole game, as one static binary on nothing.
#
# There are two Rust builds here and they are not alternatives. The game itself
# is compiled to wasm and ends up as a file; the thing that hands that file to
# a browser is compiled to a native static binary and ends up as the image. The
# second one builds the first one into itself, so the order matters and the
# `site` feature exists to express it: without the module on disk there is
# nothing to embed, and the feature is what says "the module is there now".
#
# Nothing links against the host. The server is std only, like the rest of this
# repository, so musl gives a binary with no interpreter and no shared objects,
# and `scratch` is a real option rather than a stunt: there is no shell in the
# image, no libc, no filesystem, and no path handling anywhere in the server,
# because a request is matched against a table of names rather than joined onto
# a directory.

FROM rust:1.95-alpine AS builder

# musl-dev for the C toolchain, and the wasm target for the game itself. The
# alpine image has neither.
RUN apk add --no-cache musl-dev && rustup target add wasm32-unknown-unknown

# The rust:alpine image ships RUSTFLAGS=-Ctarget-feature=-crt-static, which
# turns off the static linking a musl target would otherwise default to. An
# environment RUSTFLAGS also beats a .cargo/config.toml, so it is overridden
# here rather than in a file that would be ignored.
#
# Only for the native build: handing this to a wasm build is meaningless and
# the wasm target ignores it, but keeping the two apart is clearer than relying
# on that.
ENV RUSTFLAGS="-C target-feature=+crt-static"

WORKDIR /src
COPY . .

# The game. Ends up where `web/` expects it, which is where the server's
# `include_bytes!` looks.
RUN cargo build --release --target wasm32-unknown-unknown --locked \
    && cp target/wasm32-unknown-unknown/release/twiddlygems.wasm web/twiddlygems.wasm

# And the server, with the site now sitting on disk to be built into it.
RUN cargo build --release --target x86_64-unknown-linux-musl --locked \
    --features site --bin twiddlygems-serve

# Its own stage so `--target builder` stays inspectable when something here
# fails.
#
# Both checks earn their place, and they check different things. The selftest
# says the site inside the binary is coherent: every file present, the page
# pointed at this build's fingerprint, the module actually a module. "It
# linked" would not say any of that, and a scratch image has no test runner to
# ask later. The linkage check looks for an INTERP segment rather than reading
# ldd, because Rust emits a static-PIE for musl and musl's own ldd prints a
# loader line for those even when they are static.
FROM builder AS verify
RUN set -eux; \
    bin=target/x86_64-unknown-linux-musl/release/twiddlygems-serve; \
    "$bin" --selftest; \
    if readelf -l "$bin" | grep -q INTERP; then \
        echo "ERROR: the binary has a program interpreter, so it is not static:"; \
        readelf -l "$bin" | grep -A1 INTERP; exit 1; \
    fi; \
    if readelf -d "$bin" 2>/dev/null | grep -q NEEDED; then \
        echo "ERROR: the binary has shared library dependencies:"; \
        readelf -d "$bin" | grep NEEDED; exit 1; \
    fi; \
    echo "static: ok"

FROM scratch
COPY --from=verify /src/target/x86_64-unknown-linux-musl/release/twiddlygems-serve /twiddlygems-serve
# Where it listens. There is a reverse proxy in front of this doing TLS and
# compression, so this is plain HTTP on a high port and never runs as root.
ENV PORT=8080 BIND=0.0.0.0
EXPOSE 8080
USER 65534:65534
ENTRYPOINT ["/twiddlygems-serve"]
