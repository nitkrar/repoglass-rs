"""Run the same rpg commands through Python 0.3.2 and the Rust build; diff stdout, stderr and exit codes.

Usage: outputs.py REPO PY_HOME RS_HOME [SECOND_BINARY]. A second binary replaces RS_RPG,
so two Python builds can be compared against each other.
"""
import json, os, subprocess, sys
repo, py_home, rs_home = sys.argv[1:4]
queries = json.load(open(os.environ['RPG_QUERIES'])).get(repo, [])
PY = [os.environ['PY_RPG']]; RS = [sys.argv[4]] if len(sys.argv) > 4 else [os.environ['RS_RPG']]
def run(cmd, home, args):
    env = dict(os.environ, REPOGLASS_HOME=home)
    p = subprocess.run(cmd + args, cwd=os.path.join(os.environ['RPG_WORK'], repo), env=env, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr.strip().splitlines()[-1:] if p.stderr.strip() else []
cases = [['search', q] for q in queries]
cases += [
    ['search', 'Store'], ['search', 'store'], ['search', 'Index.refresh'], ['search', 'pkg::Widget'],
    ['search', 'how', 'are', 'tokens', 'refreshed', '--content', 'code'],
    ['search', 'config', '--content', 'docs', 'config'], ['search', 'test', '--content', 'tests'],
    ['search', 'main', '-l', 'python'], ['search', 'main', '-l', 'pyton'], ['search', 'main', '-l', 'zzz'],
    ['search', 'client', '--include', 'packages/*'], ['search', 'client', '--exclude', '*test*'],
    ['search', 'retry logic', '--code', 'signature'], ['search', 'retry', '--no-code', '-k', '3'],
    ['search', 'retry', '--content', 'data'], ['search', 'retry', '--content', 'cod'],
    ['search', 'retry', '--text'], ['search', 'the'],
    ['defs', 'main'], ['defs', 'main', '-l', 'python'], ['defs', 'nope_nothing'], ['defs', 'main', '-l', 'pyton'],
    ['refs', 'main'], ['refs', 'print', '--text'],
    ['symbols', 'test_*', '--tag', 'def'], ['symbols', '--count-by', 'lang'],
    ['symbols', '--tag', 'ref', '--count-by', 'name', '--limit', '10'], ['symbols', '--count-by', 'file', '--content', 'tests'],
    ['symbols', '--count-by', 'content', '--text'], ['symbols', 'zz*', '-l', 'pyton'], ['symbols', '--limit', '5', '--text'],
    ['config'], ['index'],
]
same = 0
for args in cases:
    a, b = run(PY, py_home, args), run(RS, rs_home, args)
    if args == ['index']:
        a = (a[0], json.dumps({k: v for k, v in json.loads(a[1])['status'].items() if k != 'seconds'}), a[2])
        b = (b[0], json.dumps({k: v for k, v in json.loads(b[1])['status'].items() if k != 'seconds'}), b[2])
    if a == b:
        same += 1
        continue
    print('DIFF', args)
    if a[0] != b[0]: print('   exit', a[0], b[0])
    if a[2] != b[2]: print('   stderr py:', a[2], '\n   stderr rs:', b[2])
    if a[1] != b[1]:
        try:
            ja, jb = json.loads(a[1]), json.loads(b[1])
            ka = [(h['path'], h['start_line']) for h in ja.get('results', [])]
            kb = [(h['path'], h['start_line']) for h in jb.get('results', [])]
            print('   same ranking' if ka == kb else f'   ranking differs: {ka[:5]} vs {kb[:5]}')
            if ka == kb:
                for x, y in zip(ja.get('results', []), jb.get('results', [])):
                    if x != y: print('     ', {k: (x.get(k), y.get(k)) for k in x if x.get(k) != y.get(k)}); break
            if 'symbols' in ja: print('   symbols equal' if ja == jb else f"   symbols differ {len(ja['symbols'])} vs {len(jb['symbols'])}")
        except Exception:
            la, lb = a[1].splitlines(), b[1].splitlines()
            for i, (x, y) in enumerate(zip(la, lb)):
                if x != y: print(f'   line {i}: py={x!r}\n           rs={y!r}'); break
            else: print(f'   lengths {len(la)} vs {len(lb)}')
print(f'{repo}: {len(cases)} commands, identical {same}')
sys.exit(0 if same == len(cases) else 1)
