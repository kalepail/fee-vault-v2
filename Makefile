default: build

test: build
	cargo test --all --tests

build:
	stellar contract build

	mkdir -p target/wasm32v1-none/optimized
	stellar contract optimize \
		--wasm target/wasm32v1-none/release/fee_vault_v2.wasm \
		--wasm-out target/wasm32v1-none/optimized/fee_vault_v2.wasm
	cd target/wasm32v1-none/optimized/ && \
		for i in *.wasm ; do \
			ls -l "$$i"; \
		done

fmt:
	cargo fmt --all

clean:
	cargo clean

  