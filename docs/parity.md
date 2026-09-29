# Parity with Python repoglass

**Status:** 0.1.0. Checked against Python repoglass 0.3.3 and semsift 0.0.6,
the versions pinned in `parity/reference.txt`. Parity runs on Linux; macOS
runs the unit tests.

`rpg` here reads and writes the same database as Python repoglass. An index
built by either opens in the other with no reindex.

## How an item is checked

`parity/ci.sh NAME=DIR ...` runs every check below on each tree and exits
non-zero on any difference. CI runs it on Python repoglass's source, this
repository, and Python repoglass's per-language samples. Its scripts read:

| Variable | Meaning |
|---|---|
| `PY_RPG`, `RS_RPG` | the Python and Rust `rpg` binaries |
| `RPG_WORK` | directory holding the trees to index, one per name |
| `RPG_QUERIES` | JSON mapping a tree name to queries (default `parity/queries.json`) |
| `RPG_SMALL_TREE` | a tree for the lock test |
| `RPG_SCRATCH` | where indexes and configs are written |

| Script | Checks |
|---|---|
| `parity.py A.db B.db` | two databases, row by row and vector by vector |
| `outputs.py` | the same commands through both binaries: stdout, stderr, exit code |
| `cross.py` | Rust reading a copy of Python's own index |
| `variants.py` | both implementations under each non-default setting |
| `life.py` | refresh lifecycle and CLI edge cases, on a tree it writes |
| `lock.py` | a read needing a refresh while another process holds the write lock |
| `fakeembed.py` | an OpenAI-compatible embeddings server for the `http` backend |

A box is ticked only after its check has run against this build.

## Result

On three trees built separately by each implementation (toolshed, 272
files; scribe, 707 files, Swift heavy; a corpus of 334 files covering all
20 grammars), and on the three CI corpora:
- **Databases:** every table and every stored vector is bit-identical,
  under the default settings and 19 settings variants.
- **Output:** every command compared is byte-identical in stdout, stderr
  and exit code: 57 on toolshed, 72 on scribe, 22 lifecycle scenarios and
  3 lock scenarios.
- **Search over Python's index:** 1,128 of 1,128 commands byte-identical.

Measured in the VM (8 cores) on scribe, against Python 0.3.2:

| | Python 0.3.2 | Rust |
|---|---|---|
| `rpg search` (sentence, median) | 305 ms | 70 ms |
| `rpg defs` (median) | 53 ms | 1 ms |
| `rpg index --force` | 4.35 s | 1.55 s |
| Peak memory, `index --force` | 350 MB | 176 MB |

## CLI

- [x] Two command names, `rpg` and `repoglass`, plus `-V/--version`
      (prints the version). *life.py*
- [x] Every subcommand takes `-r/--repo` (default `.`), `--config PATH`
      and `--text`. *outputs.py, life.py*
- [x] JSON on stdout by default, one line. `--text` renders the same
      payload for reading. Progress, warnings and errors go to stderr.
      *outputs.py, life.py*
- [x] Exit codes: 0 means it worked, including no results. 1 means a
      failure. 2 means the request can't be answered (unknown category,
      a category this index doesn't hold, a language it doesn't hold, or
      a `--config` that won't load). *life.py*
- [x] A closed pipe (`| head`) exits 0. *manual*
- [x] The first build of an index prints a notice on stderr. *life.py*
- [x] A `--config` that won't load exits 2 with
      `regenerate with: repoglass init --force`. *life.py*
- [x] Help text and epilogs match Python's in content. clap lays them out
      differently from argparse. *manual*
- [x] `search QUERY...` with `-k`, `--content`, `-l/--lang`, `--include`,
      `--exclude`, `--code full|signature|none` and `--no-code`. *cross.py, outputs.py*
- [x] `search` output: `query`, `count`, `results[]` with `path`,
      `start_line`, `end_line`, `name`, `score`, `tiers` and `code` or
      `signature`. *cross.py*
- [x] `defs NAME` and `refs NAME` with `-l`, and their output.
      *outputs.py, life.py*
- [x] `symbols [PATTERN]` with `--tag`, `--count-by`, `--limit`,
      `--content`, `-l`, `--include`, `--exclude`, and both output forms.
      *outputs.py, life.py*
- [x] `index [--force]` output. *life.py*
- [x] `status` output. *life.py*
- [x] `init [--force]`: writes resolved settings under the "PINS" header,
      refuses to overwrite without `--force`, and its output loads. *life.py*
- [x] `config` prints the resolved settings as TOML. *outputs.py, variants.py, life.py*
- [x] `clear [--all] [--dry-run]`, and `data_dir` from `--config`. *life.py*
- [ ] `clear` reporting a failed removal: not exercised.
- [x] A `--lang` the index doesn't hold exits 2 with a "did you mean" hint
      or the list of held languages. *outputs.py*
- [x] An unknown `--content` on `search` exits 2, as does a category in
      `index_excluded`. On `symbols`, an unknown `--content` exits 1 in
      both implementations. *outputs.py, life.py*

## Configuration

- [x] Every `Settings` field, with Python's defaults and allowed values. *test, variants.py*
- [x] Resolution order: user config, repository config, then
      `REPOGLASS_<NAME>`. With `--config`, and for `init` and `config`,
      the user config is not read. *test, life.py*
- [x] Grouped tables and flat keys mean the same thing. *test, variants.py*
- [x] Unknown keys are refused, and so is `embed_api_key` in the
      repository config. *test, life.py*
- [x] `REPOGLASS_<NAME>` values parse by the setting's type; lists are
      comma-separated. *test, variants.py*
- [x] `REPOGLASS_HOME`, the index directory name, and `data_dir`
      (absolute and relative). *life.py*
- [x] `init` and `config` render TOML identical to Python's, including
      doc comments and a whole number given for a float setting. *variants.py*

## Index lifecycle

- [x] WAL, `foreign_keys`, `busy_timeout = 5000`, and a `.gitignore` in
      the index directory. *parity.py, life.py*
- [x] `schema_rev` equals Python's, and a changed one rebuilds the tables.
      *parity.py, manual*
- [x] Identity fields and hashes equal Python's; a changed identity clears
      the content. *test, parity.py, life.py*
- [x] File discovery: `.gitignore`, `.repoglassignore` with `!`
      overrides, `hard_exclude`, `max_file_bytes`, generated-file probe.
      *test, variants.py*
- [x] Language detection matches grep_ast's table; `pem` is never
      indexed. *test, parity.py*
- [x] Classification and `index_excluded`. *test, variants.py*
- [x] Added, changed (including mtime moving backwards) and deleted
      files; `--force`. *life.py*
- [ ] One transaction per file; an unreadable file is skipped. Not
      exercised: the VM runs as root, which can read any file.
- [x] Deferred keyword sync, then embedding, then the scan time. *parity.py*
- [x] Automatic refresh before a read, the rescan interval, and manual
      mode. *life.py*
- [x] Under a held write lock a built index serves what it holds, and a
      never-built one fails. *lock.py*
- [x] Parsing runs on all cores, and writes stay in path order. *manual*

## Extraction

- [x] Tag queries and `extractor_rev`. *parity.py*
- [x] Grammars match Python's parse trees: all 20 query languages give
      identical symbols and chunks. *parity.py*
- [x] Definition spans, markdown sections, signatures, chunk size limits,
      `max_chunk_lines`. *parity.py, variants.py*
- [x] References and `enclosing`. *parity.py*
- [x] One chunk per symbol. *parity.py*
- [x] `coverage = definition | hybrid`. *variants.py*
- [x] Tree-walk windows and line windows, `window_chars`. *parity.py, variants.py*
- [x] `content_hash`. *parity.py*
- [x] Lexical text: `capped`,
      `lexical_enrich`, `split_identifiers`. *test, variants.py*
- [x] `distill_docs`. *variants.py*

## Storage

- [x] `rg_*` tables, declaration, and keyword triggers match semsift 0.0.5.
      *parity.py*
- [x] FTS5 keyword index and deferred rebuild. *parity.py, cross.py*
- [x] Vector codec, `missing_vectors`, adding vectors in one transaction.
      *parity.py*
- [x] `VectorSpace` and the canary; stale vectors fall back to keyword
      search with a warning. *parity.py, manual*
- [x] Filters applied before both searches. *cross.py*

## Embeddings

- [x] `static`: model2vec `minishlab/potion-code-16M-v2`, identical
      vectors when batches match. *parity.py*
- [x] The model downloads on first use into the Hugging Face cache, which
      Python then reads. *manual*
- [x] Query and document prefixes. *variants.py (prefixes)*
- [x] `none`. *variants.py*
- [x] `http`, including the API key, endpoint and out-of-order response
      items. *manual, fakeembed.py*
- [ ] `onnx` with `embed_providers`: not built (see Open). It exits 1
      with a message.

## Search

- [x] Exact, lexical and vector tiers. *cross.py*
- [x] Fusion: weighted RRF and `ranker = none`. *cross.py*
- [x] Reranking: coherence, definition, stem and embedded boosts, the
      non-candidate loader, path penalties, saturation, rescaling. *cross.py*
- [x] Content filter defaults and `all`. *test, cross.py*
- [x] Signature fallback to the first non-blank line. *cross.py*
- [x] Ties break as in Python. *cross.py*

## Distribution

- [x] Queries and SQL are built into the binary.
- [ ] Grammars built into the binary. They download on first use, from
      the same release bundle Python's grammar pack uses (1.20.0), into
      its cache. This needs network on first run, as Python repoglass does.
- [ ] Release binaries for macOS arm64/x86_64 and Linux x86_64/arm64,
      built from a `v*` tag.
- [ ] Runs on macOS. Santa blocks locally built binaries on the
      development Mac, so this needs a CI build or a Homebrew install.
- [ ] Homebrew formula installs the binary.
- [ ] PyPI `repoglass` ships a binary-only wheel (maturin `bin`).
- [x] One version source: `Cargo.toml`, checked against the tag at release.

## Differences from Python

- **Setting types.** A config value of the wrong type (a string for a
  number, say) is refused when the config loads. Python accepts it and
  fails, or ignores it, when the value is used.
- **Error text.** Where Python raises a traceback (a config error without
  `--config`, a SQLite error such as a lock outlasting the timeout), this
  prints one line. Exit codes match.
- **`onnx` backend.** Not built; see below.

## Open

- **`onnx` backend.** Bundling ONNX Runtime adds about 30 MB per platform
  (the runtime library is 33 MB on macOS arm64). Undecided.
