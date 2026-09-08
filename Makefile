.PHONY: run-dev quality deploy

run-dev:
	cargo run

quality:
	cargo fmt -- --check
	cargo clippy -- -D warnings
	cargo test

deploy:
ifndef PI
	$(error PI is required: make deploy PI=<host>)
endif
	rsync -avz \
		--exclude target \
		--exclude .git \
		--exclude .superpowers \
		--exclude mockups \
		. $(PI):~/k3fall-src
	ssh -t $(PI) 'set -e; cd ~/k3fall-src && cargo build --release --features soapy && sudo install -m755 target/release/k3fall /usr/local/bin/k3fall && sudo mkdir -p /etc/k3fall && (sudo cp -n config.example.toml /etc/k3fall/config.toml || true) && sudo cp deploy/k3fall.service /etc/systemd/system/ && sudo cp deploy/avahi-cwsweep-remote.service /etc/avahi/services/ && sudo systemctl daemon-reload && sudo systemctl enable --now k3fall && sudo systemctl restart k3fall'
