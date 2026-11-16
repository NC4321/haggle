# Build, check, demo and install targets for the haggle crate.
# Run `make check` before `make install`; install does not restart the service.

.PHONY: build test check demo install

build:
	cargo build --release

test:
	cargo test

check:
	cargo fmt --all --check
	cargo clippy --all-targets -- -D warnings
	cargo build --release
	cargo test

demo:
	cargo run --release -- demo

install:
	sudo install target/release/haggle /usr/local/bin/
	for unit in deploy/*.service deploy/*.timer; do \
		if [ -e "$$unit" ]; then sudo install -m 644 "$$unit" /etc/systemd/system/; fi; \
	done
	sudo systemctl daemon-reload
