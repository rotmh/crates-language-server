# `crates-language-server`

A language server for `Cargo.toml`'s dependencies.

## Features

- **Diagnostic Hints** - show hints for latest version on every dependency line.
- **Description on Hover** - show a crate's description on hover event on a dependency name.
- **Code Actions** - a code action for updating a dependency version to latest.
- **Version Completion** - open a version's quotation mark (`"`), and you'll be presented with a list of different granularities for the latest version, e.g., for a crate with the latest version `0.1.3-alpha`: [ `0.1.3-alpha`, `0.1.3`, `0.1`, `0` ].
- **Features Completion** - open a features' quotation mark, and you'll be presented with a list of a crate's available features.
- **Goto definition opens docs.rs** - invoke a `goto definition` event on a dependency name, and the crate's docs.rs page will be opened in your browser.

---

<!--toc:start-->

- [Usage](#usage)
  - [Installation](#installation)
    - [Using `git` and `cargo build`](#using-git-and-cargo-build)
    - [Using a Nix Flake](#using-a-nix-flake)
  - [Editor Integration](#editor-integration)
    - [Helix](#helix)
    - [Neovim](#neovim)
    - [VS Code](#vs-code)
- [Technicalities, for the interested](#technicalities-for-the-interested)
  - [`crates.io`'s API](#cratesios-api)
- [Contributions](#contributions)
- [License](#license)

<!--toc:end-->

## Usage

Note: this project uses nightly rust.

### Installation

#### Using `git` and `cargo build`

Clone this repository:

```bash
git clone https://github.com/rotmh/crates-language-server.git
```

Then build the project:

```bash
cd crates-language-server
cargo build --release
```

Finally, make sure to put it somewhere in your `$PATH`.

#### Using a Nix Flake

Add this repository as an input:

```nix
inputs = {
  # ...

  crates-ls.url = "github:rotmh/crates-language-server";
};
```

Then in the system packages (`NixOS`) or the home packages (`home-manager`):

```nix
[
  # ...

  (inputs.crates-ls.packages.${pkgs.system}.default)
]
```

Finally, rebuild your system.

**NOTE: I will soon add this to crates.io and nixpkgs.**

### Editor Integration

#### Helix

Define a new language server:

```toml
[language-server.crates-ls]
command = "crates-language-server"
```

Note: to use the above snippet, you must have the binary in your `$PATH`, if you don't, you can also specify the full path to the binary.

Then define a language for `Cargo.toml`:

```toml
[[language]]
name = "crates"
scope = "source.toml"
file-types = [{ glob = "Cargo.toml" }]
language-servers = [ "crates-ls" ]
```

#### Neovim

Know how to integrate this LSP with neovim? Please issue a PR :)

#### VS Code

Know how to integrate this LSP with VS Code? Please issue a PR :)

---

Know how to integrate this LSP with another editor? Your PR will be appreciated!

## Technicalities, for the interested

### `crates.io`'s API

This project uses both crates.io's [API](https://crates.io/data-access#api), and the sparse index.

The API comes with limitations, notably a rate limit (1 request per second). This project enforces this rate limit, and does not perform more than 1 request per second.

This limitation does not impacts the performance of the tool, because the API is only used for the crates' descriptions (and the sparse index, which is used for the rest of the crates data, does not enforce a rate limit).

## Contributions

PRs, issues, suggestions, and ideas are all appreciated and very welcome :)

## License

This project is licensed under [MIT](https://choosealicense.com/licenses/mit/).
