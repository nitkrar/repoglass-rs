"""Index one tree under many settings with both implementations; compare databases and searches."""
import glob, json, os, shutil, subprocess, sys
repo = sys.argv[1]
only = sys.argv[2:]
PY, RS = os.environ['PY_RPG'], os.environ['RS_RPG']
SCRATCH = os.environ.get('RPG_SCRATCH', '/tmp/rgparity')
VARIANTS = {
    'definition': 'coverage = "definition"',
    'capped': 'lexical_mode = "capped"\nlexical_cap_chars = 300',
    'enrich': 'lexical_enrich = true',
    'split_inline': 'split_identifiers = "inline"',
    'split_append': 'split_identifiers = "append"',
    'distill': 'distill_docs = true',
    'windows': 'window_chars = 400\nmax_chunk_lines = 50',
    'no_vectors': 'embed_backend = "none"',
    'ranker_none': 'ranker = "none"',
    'no_rerank': 'rerank = false',
    'flat_alpha': 'adaptive_alpha = false',
    'with_data': 'index_excluded = []\ncontent = ["all", "data"]',
    'categories': 'test_markers = ["test"]\ndoc_languages = ["markdown"]\nconfig_languages = ["toml", "json"]',
    'no_gitignore': 'gitignore = false',
    'hard_exclude': 'hard_exclude = [".git", "tests"]',
    'small_files': 'max_file_bytes = 5000',
    'excluded': 'content_excluded = ["data", "config", "docs"]',
    'knobs': '[retrieval]\nfile_coherence = 0.0\nstem_boost = 2\ndefinition_boost = 1.0\nsaturation_decay = 1.0\ncandidate_depth = 10\nalpha_symbol = 0.6\nprose_min_words = 2',
    'prefixes': 'embed_query_prefix = "query: "\nembed_doc_prefix = "passage: "',
}
QUERIES = json.load(open(os.environ['RPG_QUERIES'])).get(repo, [])[:6] + ['Store', 'refresh', 'Index.refresh', 'how does the config load']
SQL = ["SELECT count(*) FROM file", "SELECT count(*) FROM chunk"]
def run(binary, home, args):
    env = dict(os.environ, REPOGLASS_HOME=home)
    p = subprocess.run([binary] + args, cwd=os.path.join(os.environ['RPG_WORK'], repo), env=env, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr.strip().splitlines()[-1:] if p.stderr.strip() else []
total_bad = 0
for name, body in VARIANTS.items():
    if only and name not in only:
        continue
    cfg = f'{SCRATCH}/cfg/{name}.toml'
    os.makedirs(f'{SCRATCH}/cfg', exist_ok=True)
    open(cfg, 'w').write(body + '\n')
    homes = {PY: f'{SCRATCH}/v-py-{name}', RS: f'{SCRATCH}/v-rs-{name}'}
    for b, h in homes.items():
        shutil.rmtree(h, ignore_errors=True)
        rc, out, err = run(b, h, ['index', '--config', cfg])
        if rc:
            print(name, b, 'index failed', rc, err)
    dbs = [glob.glob(f'{homes[b]}/index/*/index.db')[0] for b in (PY, RS)]
    par = subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), 'parity.py'), *dbs], capture_output=True, text=True).stdout
    bad_tables = [l for l in par.splitlines() if 'DIFFER' in l or 'differ:' in l]
    vec = [l for l in par.splitlines() if l.startswith('rg_vectors') or 'differ:' in l]
    diffs = []
    for q in QUERIES + ['config']:
        args = ['config', '--config', cfg] if q == 'config' else ['search', q, '--config', cfg]
        a, b = run(PY, homes[PY], args), run(RS, homes[RS], args)
        if a != b:
            diffs.append(q)
    total_bad += bool(bad_tables or diffs)
    print(f'{name:14} tables:{"ok" if not bad_tables else bad_tables}  {" ".join(vec)[:110]}  output diffs: {diffs}')
print('variants with differences:', total_bad)
sys.exit(1 if total_bad else 0)
