set shell := ["fish", "-c"]

lines:
    cargo unify --lib | rustfmt --edition 2024 | wc -l

emulate:
    env HELIX_RUNTIME=~/forks/helix/runtime ~/forks/helix/target/debug/hx .

