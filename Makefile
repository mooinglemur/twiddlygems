# Twiddly Gems
#
# The engine has no dependencies, so a wasm build is a plain cargo build with
# no bindgen step and nothing to fetch.

CARGO  ?= cargo
PYTHON ?= python3
TARGET := wasm32-unknown-unknown
BUILT  := target/$(TARGET)/release/twiddlygems.wasm
OUT    := web/twiddlygems.wasm
PORT   ?= 8080

.PHONY: all wasm test abi check serve smoke shots audio balance clean target-check

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

## Serve the game. A wasm module cannot be loaded from a file:// page.
serve: wasm
	@echo "http://localhost:$(PORT)/"
	$(PYTHON) -m http.server $(PORT) --directory web

## Play the game in a headless browser and write screenshots to shots/.
shots: wasm
	node tools/shoot.mjs shots

## Render the synthesized sounds offline and measure them.
audio:
	node tools/audio_check.mjs

## Play every level with two bots and report how hard they turned out to be.
balance:
	$(CARGO) run --release --bin balance

clean:
	$(CARGO) clean
	rm -f $(OUT)
