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

# Which commit the page's footer names.
#
# Handed in rather than read out of the tree: `.dockerignore` keeps `.git` out
# of the build context, because it is large and nothing else in here wants it,
# so `build.rs` has nothing to ask and would fall back to `unknown`. The
# verify stage below refuses a binary that says that, since this image is the
# one build whose footer anybody actually reads.
#
# Below the COPY on purpose. Both bust the cache on a new commit, but this way
# a rebuild of the same tree at a different commit still rebuilds, which is
# the case where the footer is the only thing that changed.
ARG TG_GIT_HASH
ENV TG_GIT_HASH=${TG_GIT_HASH}

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
    report="$("$bin" --selftest)"; \
    echo "$report"; \
    case "$report" in \
        *+unknown*) \
            echo "ERROR: this build does not know which commit it is, so the"; \
            echo "page's footer would say so to everybody. Pass it in:"; \
            echo "    --build-arg TG_GIT_HASH=\$(git rev-parse HEAD)"; \
            exit 1 ;; \
    esac; \
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

# Plain HTTP on a high port: the gateway in front terminates TLS. It does not
# compress, so the server does, once at startup.
#
# `::` takes both address families from one socket, because a pod's address may
# be either and a server listening on only one of them is a connection refused
# that reads as a crash. The binary parses this rather than pasting it onto the
# port, so the unbracketed spelling works.
ENV PORT=8080 BIND=:: DRAIN_SECONDS=5
EXPOSE 8080

# Numeric, and it has to stay numeric: `runAsNonRoot` cannot verify a user
# given as a name, and there is no /etc/passwd in a scratch image to resolve
# one from.
USER 65534:65534

# No shell here, so nothing can wrap this and no `preStop` hook can run
# `sleep` before the signal. The binary handles SIGTERM itself: it fails
# `/readyz` at once, keeps answering for DRAIN_SECONDS, and exits 0.
ENTRYPOINT ["/twiddlygems-serve"]
