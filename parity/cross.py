"""Rust search over a copy of Python's own index, per settings variant: isolates search from index-build noise.

Usage: cross.py REPO PY_HOME [VARIANT...]. PY_HOME holds Python's default-settings index;
variant indexes are the ones variants.py left under RPG_SCRATCH.
"""
import json, os, shutil, subprocess, sys
repo, default_home = sys.argv[1:3]
SCRATCH = os.environ.get('RPG_SCRATCH', '/tmp/rgparity')
V = {}
src = open(os.path.join(os.path.dirname(__file__), 'variants.py')).read()
exec(src[src.index('VARIANTS = {'):src.index('QUERIES =')], V)
QUERIES = json.load(open(os.environ['RPG_QUERIES'])).get(repo, []) + ['Store', 'store', 'refresh', 'Index.refresh', 'pkg::Widget', 'how does the config load', 'the']
EXTRA = [['--content', 'code'], ['--content', 'tests'], ['-l', 'python'], ['--include', 'packages/*'], ['--exclude', '*test*'], ['--code', 'signature']]
def run(binary, home, args):
    env = dict(os.environ, REPOGLASS_HOME=home)
    p = subprocess.run([binary] + args, cwd=os.path.join(os.environ['RPG_WORK'], repo), env=env, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr.strip().splitlines()[-1:] if p.stderr.strip() else []
names = sys.argv[3:] or ['default'] + list(V['VARIANTS'])
grand = [0, 0]
for name in names:
    home = f'{SCRATCH}/v-py-{name}' if name != 'default' else default_home
    cfg = [] if name == 'default' else ['--config', f'{SCRATCH}/cfg/{name}.toml']
    copy = f'{SCRATCH}/cross-{name}'
    shutil.rmtree(copy, ignore_errors=True)
    shutil.copytree(home, copy)
    n = same = 0
    for q in QUERIES:
        for extra in [[]] + (EXTRA if name == 'default' else []):
            args = ['search', q, *extra, *cfg]
            a = run(os.environ['PY_RPG'], home, args)
            b = run(os.environ['RS_RPG'], copy, args)
            n += 1
            if a == b:
                same += 1
            else:
                print('  DIFF', name, args, a[0], b[0], a[2], b[2])
    grand[0] += n; grand[1] += same
    print(f'{name:14} {same}/{n} identical')
print('total', grand[1], '/', grand[0])
sys.exit(0 if grand[0] == grand[1] else 1)
