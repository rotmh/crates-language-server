set shell := ["fish", "-c"]

loc:
    cargo unify --lib | rustfmt --edition 2024 | grep -cve '^\s*$'
