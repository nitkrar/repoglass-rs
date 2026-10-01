# repoglass-rs

Local code and prose search with exact symbol lookup, in Rust. It installs
`rpg` and `repoglass`, the same commands as the Python
[repoglass](https://github.com/nitkrar/repoglass), and supersedes it.

v0.1.0 reads and writes the same index as Python repoglass 0.3.3 and gives
the same output; later versions make no such promise. See
[docs/parity.md](docs/parity.md).

## Use

```bash
rpg search "how are retries handled"
rpg defs Index
rpg refs humanise
rpg symbols --count-by lang
rpg status
rpg --help
rpg search --git https://github.com/pallets/flask.git --rev 3.1.0 "how are blueprints registered"
```

`--git URL --rev REV` indexes a remote repository instead of a directory.
One commit is fetched at depth 1 into `~/.repoglass/git/` the first time and
kept, so later commands and rebuilds reuse it; `-r` then names a directory
inside the checkout. A tag or branch is resolved once; delete its checkout
to fetch it again.

The first run downloads the tree-sitter grammars and the embedding model.
Installing this and Python repoglass puts two `rpg` commands on PATH;
whichever comes first runs.

## Build

```bash
cargo test                      # unoptimised: compiles fastest
cargo build --profile fast      # target/fast/rpg: optimised, quick to rebuild
cargo build --release           # target/release/rpg: what releases ship
```

Run tests unoptimised; they do not need the speed. `fast` skips link-time
optimisation, so it rebuilds in a fraction of the release time and searches
a little slower: use it for benchmarking while iterating, and
`--release` for numbers you will compare across releases. Building needs a
Rust toolchain and a C compiler (SQLite and tree-sitter are compiled in).

The first run of `rpg` downloads the tree-sitter grammars and the
embedding model, so it needs network access once.

`data/` holds the tag queries and the index schema, embedded at build time.
`eval/` holds the retrieval benchmarks; see [eval/README.md](eval/README.md).

### In Docker

For a machine without a Rust toolchain, or one that will not run locally
built binaries, build and run inside a container. From the repository
directory, with Docker running:

```bash
# once: a container whose cargo downloads and build output survive in volumes
docker run -d --name repoglass-build \
  -v repoglass-cargo:/usr/local/cargo/registry -v repoglass-target:/target \
  rust:1-bookworm sleep infinity

# after each change: copy the source in, then test or build
tar --exclude=./target -cf - . | docker exec -i repoglass-build sh -c 'rm -rf /src && mkdir /src && tar -C /src -xf -'
docker exec -w /src -e CARGO_TARGET_DIR=/target repoglass-build cargo test
docker exec -w /src -e CARGO_TARGET_DIR=/target repoglass-build cargo build --profile fast

# the binary is built for Linux, so it runs in the container
docker exec -w /src repoglass-build /target/fast/rpg search "how are retries handled"
```

The source is copied rather than mounted, so this works where the Docker
VM cannot see host directories. The first build compiles every dependency
and takes minutes; later ones reuse the volumes. The image has Python and
git, so `eval/` runs there too, with `--rpg /target/fast/rpg`.

## Release

Push a `v*` tag matching `version` in `Cargo.toml`. The release workflow
refuses a tag that disagrees, builds macOS and Linux binaries for arm64
and x86_64, and attaches them to a GitHub release. The Homebrew formula in
`nitkrar/homebrew-tap` is updated by hand to the new version and checksums.

## Licence

MIT, see [LICENSE](LICENSE). The tag queries are attributed in
[data/queries/NOTICE.md](data/queries/NOTICE.md).
