.PHONY: build install run clean

build:
	cargo build --release

install: build
	mkdir -p ~/.local/bin
	install -m 755 target/release/clear-status-interface ~/.local/bin/clear-status-interface

run:
	cargo run

clean:
	cargo clean
