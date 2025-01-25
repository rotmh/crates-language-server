set shell := ["fish", "-c"]

lines:
    cargo unify --lib | rustfmt --edition 2024 | wc -l
