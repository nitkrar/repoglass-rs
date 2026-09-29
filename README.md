# repoglass-rs

A Rust build of [repoglass](https://github.com/nitkrar/repoglass): local code
and prose search with exact symbol lookup. It installs the same `rpg` and
`repoglass` commands, reads and writes the same index, and gives the same
output as the Python version it is checked against.

Status: unreleased. See [docs/parity.md](docs/parity.md) for what is checked
and what is not.

## Use

```bash
rpg search "how are retries handled"
rpg defs Index
rpg refs humanise
rpg symbols --count-by lang
rpg status
rpg --help
```

The first run downloads the tree-sitter grammars and the embedding model into
the caches Python repoglass uses, so an index built by either opens in the
other. Installing both puts two `rpg` commands on PATH; whichever comes first
runs.

## Build

```bash
cargo build --release     # target/release/rpg
cargo test
```

`data/` holds copies of Python repoglass's tag queries and index schema.
Parity CI fails if they drift, because the index identity hashes them.

## Parity

`parity/ci.sh` indexes trees with both implementations and compares
databases, command output, settings variants, the index lifecycle and
behaviour under a held lock. CI runs it against the Python version pinned in
`parity/reference.txt`.

## Licence

MIT, see [LICENSE](LICENSE). The tag queries are attributed in
[data/queries/NOTICE.md](data/queries/NOTICE.md).
