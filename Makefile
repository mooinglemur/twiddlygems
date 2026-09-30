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

# The Archipelago side. The world is a directory under worlds/; an .apworld is
# that directory zipped. AP is a pinned clone rather than a dependency, so the
# tests run against a tree that is not here until someone makes it: see
# ap-setup.
AP      := vendor/Archipelago
VENV    := .venv
WORLD   := worlds/twiddlygems
APDATA  := $(WORLD)/data
APWORLD := build/twiddlygems.apworld
AP_TAG  ?= 0.6.7

.PHONY: all wasm test abi check serve smoke shots audio balance clean target-check \
	apdata apworld apworld-test apworld-gen apworld-install ap-setup ap-link ap-live \
	site site-smoke favicon

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
apdata:
	@$(CARGO) run --quiet --release --bin apworld -- $(APDATA)

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
