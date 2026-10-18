# Twiddly Gems
#
# The engine has no dependencies, so a wasm build is a plain cargo build with
# no bindgen step and nothing to fetch.
#
# The tooling around it wants node 22.4 or newer, which is where node grew a
# global WebSocket: the two targets that drive a headless browser talk to it
# over one, and so does the one that plays a real multiworld. They say so
# themselves rather than failing obscurely, but this is the shorter answer.

CARGO  ?= cargo
PYTHON ?= python3
TARGET := wasm32-unknown-unknown
BUILT  := target/$(TARGET)/release/twiddlygems.wasm
OUT    := web/twiddlygems.wasm
PORT   ?= 8080

# Git, told that this checkout is one it may read.
#
# A CI container runs its steps as a user other than the one the workspace
# belongs to, and git refuses to read a repository it thinks is somebody
# else's: "detected dubious ownership". Said per command rather than written
# into whatever global config the job happens to have, so this holds wherever
# the build runs and changes nothing outside it.
GIT := git -c safe.directory=$(CURDIR)

# The commit the page's footer names, beside the crate version. Exported so
# `build.rs` picks it up; a build with no checkout to ask gets `unknown`, which
# is a footer that says so rather than a build that fails.
#
# Whoever runs the build gets the last word: CI hands over the event's own sha,
# because it is answering inside a checkout it did not make. Asked of git only
# when nobody has said, and once rather than per recipe.
ifeq ($(origin TG_GIT_HASH), undefined)
TG_GIT_HASH := $(shell $(GIT) rev-parse --short=9 HEAD 2>/dev/null)
endif
export TG_GIT_HASH

# The Archipelago side. The world is a directory under worlds/; an .apworld is
# that directory zipped. AP is a pinned clone rather than a dependency, so the
# tests run against a tree that is not here until someone makes it: see
# ap-setup.
AP      := vendor/Archipelago
VENV    := .venv
WORLD   := worlds/twiddlygems
APDATA  := $(WORLD)/data
APWORLD := build/twiddlygems.apworld
TEMPLATE := build/twiddlygems.yaml
AP_TAG  ?= 0.6.7

# The released worlds today's game still has to be able to play, newest last.
# A seed carries the version of the world that rolled it and nothing else does,
# so this list is the only record of which ones are still expected to work.
#
# Add a tag here on every release. Take one off only when breaking it on
# purpose: that means moving `GENERATOR` in progression.rs, which orphans every
# seed in flight. See `ap-compat`.
COMPAT_TAGS ?= v0.1.1

.PHONY: all wasm test abi check serve smoke shots audio balance clean target-check \
	apdata apworld apworld-test apworld-gen apworld-install ap-setup ap-link ap-live \
	ap-compat ap-compat-one template site site-smoke favicon

all: check wasm

## Run the engine's test suite natively.
test:
	$(CARGO) test

## Check that the front end and the engine agree about the ABI.
abi:
	$(PYTHON) tools/check_abi.py

check: test abi

## Fail early and legibly when the wasm target is not installed.
target-check:
	@sysroot=$$($(CARGO) --quiet rustc -- --print sysroot 2>/dev/null || rustc --print sysroot); \
	if [ ! -d "$$sysroot/lib/rustlib/$(TARGET)" ]; then \
		echo "error: the $(TARGET) standard library is not installed."; \
		echo; \
		echo "  On Gentoo, enable the WebAssembly target for dev-lang/rust:"; \
		echo "    echo 'dev-lang/rust rust_targets_WebAssembly' >> /etc/portage/package.use/rust"; \
		echo "    emerge -1 dev-lang/rust"; \
		echo; \
		echo "  With rustup: rustup target add $(TARGET)"; \
		exit 1; \
	fi

## Build the wasm module and drop it next to the page.
wasm: target-check
	$(CARGO) build --release --target $(TARGET)
	cp $(BUILT) $(OUT)
	@echo "built $(OUT) ($$(wc -c < $(OUT) | awk '{printf "%.0f KiB", $$1/1024}'))"

## Exercise the wasm module's ABI from node, without a browser.
smoke: wasm
	node tools/abi_smoke.mjs $(OUT)
	node tools/page_smoke.mjs $(OUT)
	node tools/ap_smoke.mjs $(OUT)

## Serve the game. A wasm module cannot be loaded from a file:// page.
##
## Through tools/serve.py rather than `python3 -m http.server`, which sends no
## Cache-Control and leaves a browser guessing a lifetime for every file. That
## guess is what makes an edit fail to turn up on a phone: see the script.
serve: wasm
	@echo "http://localhost:$(PORT)/"
	$(PYTHON) tools/serve.py --port $(PORT) --directory web

## Build the server that ships the game, with the whole site inside it.
##
## After the module, which is one of the files it builds in: the wasm is a
## build artifact rather than something in the repository, so the feature that
## pulls it in is off by default and everything else builds without it.
site: wasm
	$(CARGO) build --release --features site --bin twiddlygems-serve
	./target/release/twiddlygems-serve --selftest

## Load the game out of that server, in a real browser.
##
## The one check that covers what only the built image does: everything but the
## page served under a prefix carrying a fingerprint of the build, and every
## relative path inside the site still finding what it wants from under there.
site-smoke: site
	node tools/site_smoke.mjs

## Redraw the browser-tab icon from the game's own gem painter.
##
## Its result is committed, unlike everything else a browser makes here: the
## icon changes approximately never, and building it in the image would mean
## the image build needed a browser in it.
favicon:
	node tools/favicon.mjs web/favicon.ico

## Play the game in a headless browser and write screenshots to shots/.
shots: wasm
	node tools/shoot.mjs shots

## Render the synthesized sounds offline and measure them.
audio:
	node tools/audio_check.mjs

## Play every level with two bots and report how hard they turned out to be.
balance:
	$(CARGO) run --release --bin balance

## Write the Archipelago world's data out of the engine.
##
## The rules, the items and the locations all live in src/progression.rs,
## because the solo game plays by them too. This is how they reach the Python
## side: as data, so there is one answer rather than two.
##
## Not checked in. It is a transformation of the engine and nothing else, so
## the engine is the copy worth keeping; everything that needs the file builds
## it first, and a checked-in copy could only ever be right or stale.
##
## The licence is copied in beside it, rather than kept as a second copy in the
## tree, so the one at the repository root stays the only one anybody edits.
## It goes in the world directory and not just into the zip, so that a world
## packaged by anything other than `make apworld` still carries it: Archipelago's
## own "Build APWorlds" reads the directory, and its ignore list excludes a
## manifest and a .gitignore but not a licence.
apdata:
	@$(CARGO) run --quiet --release --bin apworld -- $(APDATA)
	@cp LICENSE $(WORLD)/LICENSE
	@echo "copied LICENSE into $(WORLD)"

## Zip the world into an .apworld, which is all an .apworld is.
##
## The tests are left out: they import Archipelago's own test bases, which only
## exist inside a checkout, so they are for this repository rather than for the
## file a player installs.
##
## The manifest is stamped with the container's own version fields on the way
## past. They belong in the zip and must not be in the tree, so the staged copy
## is where they go: see tools/stamp_manifest.py.
##
## That step reads the container version out of Archipelago's source rather
## than keeping a copy of it here, so packaging needs the checkout. It reads the
## file rather than importing it, so it needs nothing installed and prints
## nothing: importing Archipelago loads every world in the checkout, and the
## ones missing an optional dependency log a screenful of tracebacks that have
## nothing to do with this one.
apworld: apdata
	@test -d $(AP) || { echo "error: no Archipelago checkout. Run 'make ap-setup'."; exit 1; }
	rm -rf build/apworld
	mkdir -p build/apworld
	cp -r $(WORLD) build/apworld/twiddlygems
	rm -rf build/apworld/twiddlygems/test build/apworld/twiddlygems/__pycache__
	$(PYTHON) tools/stamp_manifest.py $(AP) build/apworld/twiddlygems
	cd build/apworld && $(PYTHON) -m zipfile -c ../$(notdir $(APWORLD)) twiddlygems
	@echo "built $(APWORLD) ($$(wc -c < $(APWORLD) | awk '{printf "%.0f KiB", $$1/1024}'))"

## Play a real multiworld against Archipelago's own server.
##
## The check the stubbed one cannot be. `make smoke` drives the client against
## a socket written here, from a reading of the protocol document, so a
## misreading would be built into the stub and the tests would agree with it.
## This runs `MultiServer` out of the pinned checkout, on a seed our own world
## generated, over a real socket.
##
## The server is started by the node script and killed through its own child
## handle, so nothing is ever matched by name.
ap-live: wasm apworld-gen
	rm -rf build/ap/live
	mkdir -p build/ap/live
	$(PYTHON) -m zipfile -e "$$(ls build/ap/out/*.zip | head -1)" build/ap/live
	node tools/ap_live.mjs "$$(ls build/ap/live/*.archipelago | head -1)" $(OUT)

## Play a multiworld generated by every released world against today's game.
##
## The one check for a room already in flight. Every other target here builds
## the world and the game out of the same tree, so the two agree by
## construction; a real player's room was rolled by whatever version was
## current the day it was generated, and their browser loads today's game. This
## builds each released world from its own tag, rolls a seed with it, and plays
## today's client against that.
##
## Through `git archive` into build/ rather than a checkout, so it leaves the
## working tree alone and runs on a dirty one.
##
## A release that means to break this moves `GENERATOR` in progression.rs and
## drops the tags it has orphaned, which is a deliberate act and takes every
## seed in flight with it. See `PLAYABLE_GENERATORS`.
ap-compat: wasm
	@test -d $(AP) || { echo "error: no Archipelago checkout. Run 'make ap-setup'."; exit 1; }
	@for tag in $(COMPAT_TAGS); do \
		$(MAKE) --no-print-directory ap-compat-one COMPAT_TAG=$$tag || exit 1; \
	done

## One released world, named by COMPAT_TAG. `ap-compat` is how this is reached.
##
## Built by that version's own Makefile, pointed at this checkout's
## Archipelago, and generated from that version's own player file: a setting
## added since would make today's yaml one the old world has never heard of.
ap-compat-one: ARCHIVE := build/compat/$(COMPAT_TAG)
ap-compat-one:
	@test -n "$(COMPAT_TAG)" || { echo "error: this wants COMPAT_TAG=<tag>"; exit 1; }
	@$(GIT) rev-parse --verify --quiet "$(COMPAT_TAG)^{commit}" >/dev/null || { \
		echo "error: git cannot resolve $(COMPAT_TAG) here. CI has to fetch its tags"; \
		echo "       (fetch-depth: 0), and git has to be willing to read the checkout."; \
		exit 1; }
	rm -rf $(ARCHIVE)
	mkdir -p $(ARCHIVE)/players $(ARCHIVE)/out $(ARCHIVE)/live
	$(GIT) archive $(COMPAT_TAG) | tar -x -C $(ARCHIVE)
	$(MAKE) -C $(ARCHIVE) apworld AP=$(CURDIR)/$(AP) PYTHON=$(PYTHON)
	rm -f $(AP)/worlds/twiddlygems $(AP)/custom_worlds/twiddlygems.apworld
	cp $(ARCHIVE)/$(APWORLD) $(AP)/custom_worlds/
	cp $(ARCHIVE)/tools/twiddlygems.yaml $(ARCHIVE)/players/
	cd $(AP) && SKIP_REQUIREMENTS_UPDATE=1 $(CURDIR)/$(VENV)/bin/python Generate.py \
		--player_files_path $(CURDIR)/$(ARCHIVE)/players \
		--outputpath $(CURDIR)/$(ARCHIVE)/out \
		--seed 20260922
	$(PYTHON) -m zipfile -e "$$(ls $(ARCHIVE)/out/*.zip | head -1)" $(ARCHIVE)/live
	node tools/ap_live.mjs "$$(ls $(ARCHIVE)/live/*.archipelago | head -1)" $(OUT)
	@rm -f $(AP)/custom_worlds/twiddlygems.apworld
	@echo "compat ok: today's game played a $(COMPAT_TAG) multiworld"

## Run Archipelago's own tests against the world, in a pinned checkout.
##
## The three that come free from WorldTestBase are the ones worth having: that
## nothing is reachable from nowhere, that everything is reachable with
## everything, and that a real fill can be made. That last one is the
## completability gate, done by Archipelago's generator rather than by ours.
##
## The modules are named rather than discovered, so a new one has to be added
## here to run at all. `test_manifest` is about the packaged file rather than
## the game, and is the check that would have caught the manifest going missing.
##
## Archipelago's own conformance suites are run alongside ours, because they
## are the ones that catch what a world gets wrong about *being* a world rather
## than about this game. They test every world in the checkout, which costs a
## few seconds and is how the option classes were caught claiming to live in
## `abc`: unpicklable, so unhostable, and invisible to every test here.
apworld-test: apdata ap-link
	cd $(AP) && $(CURDIR)/$(VENV)/bin/python -m unittest \
		worlds.twiddlygems.test.test_logic \
		worlds.twiddlygems.test.test_manifest
	cd $(AP) && $(CURDIR)/$(VENV)/bin/python -m unittest \
		test.general.test_options \
		test.general.test_world_manifest \
		test.general.test_names \
		test.general.test_groups \
		test.general.test_ids

## Roll a real seed, which is the check the unit tests cannot be.
##
## Generation is the whole pipeline: options parsed, a world built, the pool
## filled, a playthrough calculated and an output archive written. The spoiler
## it leaves in build/ap/out is worth reading, since it says where this seed
## put everything and in what order a player would find it.
apworld-gen: apdata ap-link
	rm -rf build/ap/out
	mkdir -p build/ap/players build/ap/out
	cp tools/twiddlygems.yaml build/ap/players/
	cd $(AP) && SKIP_REQUIREMENTS_UPDATE=1 $(CURDIR)/$(VENV)/bin/python Generate.py \
		--player_files_path $(CURDIR)/build/ap/players \
		--outputpath $(CURDIR)/build/ap/out \
		--seed 20260922

## Install the zip the way a player would, and roll a seed from that.
##
## The check only the zip can fail. A module inside an .apworld has no
## directory to read a data file out of, so anything that opens one by path
## works in a checkout, passes every test here, and then fails for everybody
## who installed the zip. Ask me how I know.
apworld-install: apworld
	@test -d $(AP) || { echo "error: no Archipelago checkout. Run 'make ap-setup'."; exit 1; }
	rm -f $(AP)/worlds/twiddlygems
	cp $(APWORLD) $(AP)/custom_worlds/
	rm -rf build/ap/installed
	mkdir -p build/ap/players build/ap/installed
	cp tools/twiddlygems.yaml build/ap/players/
	cd $(AP) && SKIP_REQUIREMENTS_UPDATE=1 $(CURDIR)/$(VENV)/bin/python Generate.py \
		--player_files_path $(CURDIR)/build/ap/players \
		--outputpath $(CURDIR)/build/ap/installed \
		--seed 20260922

## Write the player yaml template, which is what a player fills in.
##
## Generated from the installed zip rather than from the checkout, so the
## version in its header is the one stamped into the .apworld and the file is
## the one that build of the world actually offers. That is also why it is
## shipped beside the .apworld in a release: the two belong to each other.
template: apworld
	@test -d $(AP) || { echo "error: no Archipelago checkout. Run 'make ap-setup'."; exit 1; }
	@rm -f $(AP)/worlds/twiddlygems
	cp $(APWORLD) $(AP)/custom_worlds/
	$(VENV)/bin/python tools/make_template.py $(AP) $(TEMPLATE)

## Put the world where Archipelago can import it. The checkout is ignored by
## git, so the link lives outside the repository's own tree.
##
## The installed zip goes first: two copies of one game under different names
## is a checkout that generates nothing.
ap-link:
	@test -d $(AP) || { echo "error: no Archipelago checkout. Run 'make ap-setup'."; exit 1; }
	@rm -f $(AP)/custom_worlds/twiddlygems.apworld
	@ln -sfn $(CURDIR)/$(WORLD) $(AP)/worlds/twiddlygems

## Clone Archipelago and make the virtualenv the apworld tests need.
##
## Far less than its requirements.txt: generation never opens the GUI, so kivy
## is not needed. Both directories are ignored by git.
ap-setup:
	test -d $(AP) || git clone --depth 1 --branch $(AP_TAG) \
		https://github.com/ArchipelagoMW/Archipelago.git $(AP)
	test -d $(VENV) || $(PYTHON) -m venv $(VENV)
	$(VENV)/bin/python -m pip install --quiet colorama PyYAML jellyfish schema orjson \
		typing_extensions platformdirs certifi pathspec
	# jinja2 renders the player yaml template, which is a jinja template in
	# Archipelago's own data directory. Pinned out of its requirements for the
	# reason websockets is below: the template is theirs, so the renderer
	# should be the version they render it with.
	grep -E '^jinja2[=<>]' $(AP)/requirements.txt | cut -d'#' -f1 \
		| xargs $(VENV)/bin/python -m pip install --quiet
	# websockets at the version Archipelago pins, read out of its own
	# requirements rather than written down here, so bumping AP_TAG follows it.
	#
	# It has to be the pinned one, which is not what "install websockets" gets.
	# Generation never opens a socket, so any version at all passed for as long
	# as this checkout only ever generated seeds. `make ap-live` runs the real
	# server, and websockets 14 removed the `ServerConnection.open` attribute
	# that MultiServer reads on every connection: every client is accepted and
	# then dropped with an AttributeError before it is told anything.
	# The `cut` drops the trailing comment on that line, which pip reads as
	# another package to install and refuses.
	grep -E '^websockets[=<>]' $(AP)/requirements.txt | cut -d'#' -f1 \
		| xargs $(VENV)/bin/python -m pip install --quiet
	# Generate.py reads every world's requirements through pkg_resources, which
	# setuptools stopped shipping at 81, and loading an installed .apworld goes
	# through worlds/Files.py, which imports bsdiff4. Neither is needed to run
	# the world's own tests out of the checkout.
	$(VENV)/bin/python -m pip install --quiet "setuptools<81" bsdiff4
	@echo "Archipelago $(AP_TAG) ready in $(AP)"

clean:
	$(CARGO) clean
	rm -f $(OUT)
	rm -rf build
