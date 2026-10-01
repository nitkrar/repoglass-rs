# Retrieval evaluation

Two benchmarks make a retrieval change measurable: run one before and
after, and compare.

## Public: semble's code-search benchmark

```bash
python3 eval/semble.py --rpg target/release/rpg --json before.json
python3 eval/semble.py --rpg target/release/rpg --full     # all 63 repositories
```

semble's benchmark has 1,251 queries over 63 open-source repositories in 19
languages, each labelled with the files, and sometimes the lines, that
answer it. `semble.py` fetches it at a pinned commit and indexes each
repository with `rpg index --git URL --rev COMMIT`. Everything goes under
`~/.cache/repoglass-rs/eval`, and the checkouts are kept, so only the first
run downloads anything.

By default it scores one repository per language, 19 repositories and 376
queries, about a third of the full download (411 MB of 1.3 GB). It picks
each language's median repository by files indexed, because size drives
difficulty. `SUBSET` in `semble.py` names them. Expect the subset within
about 0.02 NDCG@10 of the full run; run `--full` before a release. A
per-language figure rests on about 20 queries, so read the overall score.

It reports NDCG@10, NDCG@5 and the share of queries with any correct hit,
by category and by language. Scoring follows semble's: a result matches a
label by path, and by line span when the label pins one.

## Local: hand-written questions

Hand-written questions over four real repositories, each with the files and
symbols that answer it.

```bash
python3 eval/eval.py check                       # are the answers still true?
python3 eval/eval.py run --rpg target/release/rpg --json before.json
```

`run` indexes each repository into a fresh temporary home, asks every
question with `-k 3` under the content filter the question names, and
prints recall@1 and recall@3 by kind, content and repository, with the
median time per question including process start. `--json` keeps each
question's returned paths for comparing two runs. `run` refuses to score
while `check` reports problems.

### Local files

`questions.json` and `roots.json` are not committed: two of the four
repositories are private, and the questions describe their code.
`roots.json` maps each repository name to a checkout:

```json
{"agent_broker": "~/Projects/nitkrar/agent_broker"}
```

A repository with no entry, or no checkout, is skipped rather than scored
as a miss.

| `repo` | source | language |
|---|---|---|
| `agent_broker` | private | Python |
| `llmctl` | private | Python |
| `citadel_vault` | `github.com/nitkrar/citadel_vault` | JavaScript, PHP |
| `personal_scribe_mac` | `github.com/nitkrar/personal_scribe_mac` | Swift, Markdown |

### Questions

A JSON array; every entry has the same fields:

| field | meaning |
|---|---|
| `id` | `qNNN`, stable and unique |
| `repo` | a key in `roots.json` |
| `query` | passed to `rpg search` verbatim |
| `kind` | `semantic`, `lexical` or `navigation` |
| `content` | the `--content` filter the question is asked under: `code`, `tests`, `docs` or `all` |
| `answer_paths` | every acceptable file, relative to the repository root |
| `answer_symbols` | names defined in those files; markdown headings for prose answers |
| `negative` | true when nothing in the repository answers it; `answer_paths` is then empty |
| `notes` | why this is the answer; read before disputing a result |

A hit is an answer path among the returned paths. `check` confirms each
answer path exists and together they contain every answer symbol; it does
not confirm the answer is the best one, which is the judgement recorded
in `notes`.

- **navigation**: the query is the bare identifier, defined in one place.
- **lexical**: a distinctive literal such as an error message, an index
  name or a settings key, which exercises the keyword index.
- **semantic**: natural language written away from the answer's
  identifiers. "take the ums and uhs out" should find
  `FillerRemovalStage`. A semantic question containing the answer's
  identifier measures the wrong thing.

### Limits

- One author, no second reader.
- Multi-file answers do not say which file should rank first, so recall@3
  is a fairer measure than recall@1.
- A difference of two or three questions is noise.
- rpg has no score cutoff and always returns hits, so the negatives pass
  only if nothing is returned.
- `citadel_vault/dist/` is tracked and duplicates much of `src/client/`, so
  a `dist/` file can outrank the source file that answers a client
  question.
- Answers were verified against the working trees; `check` catches a
  repository that has moved on.
