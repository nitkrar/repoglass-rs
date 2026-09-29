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
```

The first run downloads the tree-sitter grammars and the embedding model.
Installing this and Python repoglass puts two `rpg` commands on PATH;
whichever comes first runs.

## Build

```bash
cargo build --release     # target/release/rpg
cargo test
```

`data/` holds the tag queries and the index schema, embedded at build time.

## Licence

MIT, see [LICENSE](LICENSE). The tag queries are attributed in
[data/queries/NOTICE.md](data/queries/NOTICE.md).
