"""Index lifecycle and CLI edge cases, Python vs Rust, each on its own copy of a small tree."""
import glob, json, os, re, shutil, subprocess, sys, time
PY, RS = os.environ['PY_RPG'], os.environ['RS_RPG']
PROBE = """\
import subprocess


def parse_elapsed(text):
    \"\"\"Seconds from a ps elapsed-time field such as 1-02:03:04.\"\"\"
    days, _, rest = text.rpartition("-")
    parts = [int(p) for p in rest.split(":")]
    while len(parts) < 3:
        parts.insert(0, 0)
    hours, minutes, seconds = parts
    return int(days or 0) * 86400 + hours * 3600 + minutes * 60 + seconds


def parse_cpu_time(text):
    \"\"\"Seconds of CPU time from ps, the same format as elapsed time.\"\"\"
    return parse_elapsed(text)


def parse_ps(output):
    \"\"\"One record per ps line: pid, elapsed seconds and command.\"\"\"
    rows = []
    for line in output.splitlines()[1:]:
        pid, elapsed, command = line.split(None, 2)
        rows.append({"pid": int(pid), "elapsed": parse_elapsed(elapsed), "command": command})
    return rows


def run():
    out = subprocess.run(["ps", "-eo", "pid,etime,comm"], capture_output=True, text=True).stdout
    return parse_ps(out)
"""
TESTS = """\
from diag.core.probe import parse_elapsed, parse_ps


def test_ps_reads_elapsed_and_size():
    rows = parse_ps("PID ELAPSED COMMAND\\n  1 01:02 init\\n")
    assert rows[0]["elapsed"] == 62 and parse_elapsed("1-00:00:00") == 86400
"""
README = "# diag\n\nReports on running processes: how long each has run, and what it is.\n"
def mk(root):
    shutil.rmtree(root, ignore_errors=True)
    for rel, text in (("src/diag/core/probe.py", PROBE), ("tests/test_probe.py", TESTS),
                      ("README.md", README), ("pyproject.toml", '[project]\nname = "diag"\n')):
        os.makedirs(os.path.dirname(os.path.join(root, rel)) or root, exist_ok=True)
        open(os.path.join(root, rel), "w").write(text)
def run(b, root, home, args, env=None):
    e = dict(os.environ, REPOGLASS_HOME=home, **(env or {}))
    p = subprocess.run([b] + args, cwd=root, env=e, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr
def norm(s, root, home):
    s = s.replace(root, '<ROOT>').replace(home, '<HOME>')
    s = re.sub(r'"seconds": [0-9.]+', '"seconds": X', s)
    s = re.sub(r'seconds\s+[0-9.]+', 'seconds X', s)
    s = re.sub(r'(-rs|-py)', '', s)
    s = re.sub(r'diag-[0-9a-f]{8}', 'diag-KEY', s)
    return s
ok = bad = 0
def compare(label, steps, env=None, stderr=True):
    global ok, bad
    outs = []
    for b, tag in ((PY, 'py'), (RS, 'rs')):
        root, home = f'/tmp/life-{tag}/diag', f'/tmp/life-{tag}/home'
        shutil.rmtree(f'/tmp/life-{tag}', ignore_errors=True)
        os.makedirs(f'/tmp/life-{tag}')
        mk(root)
        res = []
        for step in steps:
            if callable(step):
                step(root, home)
                continue
            rc, out, err = run(b, root, home, step, env)
            res.append((step, rc, norm(out, root, home), norm(err, root, home) if stderr else ''))
        outs.append(res)
    if outs[0] == outs[1]:
        ok += 1
        print(f'ok    {label}')
    else:
        bad += 1
        print(f'DIFF  {label}')
        for x, y in zip(*outs):
            if x != y:
                print('   step', x[0]); print('     py', repr(x[1:])[:600]); print('     rs', repr(y[1:])[:600])
def write(rel, text):
    def f(root, home):
        p = os.path.join(root, rel); os.makedirs(os.path.dirname(p), exist_ok=True); open(p, 'w').write(text)
    return f
def delete(rel):
    return lambda root, home: os.remove(os.path.join(root, rel))
def touch_later(rel):
    def f(root, home):
        p = os.path.join(root, rel); st = os.stat(p); os.utime(p, ns=(st.st_atime_ns, st.st_mtime_ns - 10**9))
    return f
S = ['search', 'parse ps elapsed', '--no-code']
compare('first build then no-op', [['index'], ['index'], S])
compare('add, change, delete', [['index'], write('src/new_mod.py', 'def brand_new_thing():\n    return "hello world, this is a new function body"\n'),
        write('src/diag/core/probe.py', 'def replaced():\n    return "the probe module was rewritten entirely for this test"\n'),
        delete('tests/test_probe.py'), ['index'], ['defs', 'brand_new_thing'], ['defs', 'parse_ps'], S])
compare('mtime moves backwards', [['index'], touch_later('src/diag/core/probe.py'), ['index']])
compare('force', [['index'], ['index', '--force']])
compare('auto refresh on read', [['index'], write('src/auto.py', 'def auto_refreshed_symbol():\n    return "picked up by the read path"\n'),
        lambda r, h: time.sleep(6), ['defs', 'auto_refreshed_symbol']])
compare('rescan interval skips walk', [['index'], write('src/late.py', 'def too_soon_symbol():\n    return "not yet visible to reads"\n'),
        ['defs', 'too_soon_symbol']])
compare('manual refresh mode', [write('repoglass.toml', 'refresh_mode = "manual"\n'), ['index'],
        write('src/m.py', 'def manual_mode_symbol():\n    return "only an explicit index sees this"\n'), lambda r, h: time.sleep(6),
        ['defs', 'manual_mode_symbol'], ['index'], ['defs', 'manual_mode_symbol']])
compare('identity change reindexes', [['index'], write('repoglass.toml', 'coverage = "definition"\n'), ['index'], ['status']])
compare('status and text', [['index'], ['status'], ['status', '--text'], ['index', '--text']])
compare('defs refs symbols text', [['index'], ['defs', 'parse_ps', '--text'], ['refs', 'run', '--text'],
        ['symbols', 'parse_*', '--text'], ['symbols', '--count-by', 'lang', '--text'], ['symbols', '--count-by', 'name', '--limit', '3', '--text'],
        ['symbols', '--count-by', 'tag'], ['symbols', '--count-by', 'file', '--limit', '2']])
compare('not a directory', [['search', 'x', '-r', '/nonexistent/dir']])
compare('bad --config key', [write('bad.toml', 'rerankk = true\n'), ['search', 'x', '--config', 'bad.toml']])
compare('secret in --config', [write('bad.toml', 'embed_api_key = "k"\n'), ['search', 'x', '--config', 'bad.toml']], stderr=False)
compare('unknown content', [['index'], ['search', 'x', '--content', 'cod'], ['search', 'x', '--content', 'data']])
compare('init and config', [['init'], ['init'], ['init', '--force'], lambda r, h: shutil.copy(os.path.join(r, 'repoglass.toml'), os.path.join(r, 'init.out')),
        ['config'], ['config', '--text']])
compare('init output loads', [['init'], ['index'], ['search', 'probe', '--no-code', '-k', '2']])
compare('user config ignored by config', [lambda r, h: (os.makedirs(h, exist_ok=True), open(os.path.join(h, 'repoglass.toml'), 'w').write('window_chars = 123\n')),
        ['config'], ['index'], ['status']])
compare('clear', [['clear'], ['index'], ['clear', '--dry-run'], ['clear'], ['clear'], ['clear', '--all', '--dry-run']])
compare('clear --all', [['index'], ['clear', '--all']])
compare('relative data_dir', [write('repoglass.toml', 'data_dir = ".rgx"\n'), ['index'], lambda r, h: print('      exists', os.path.exists(os.path.join(r, '.rgx/index.db'))),
        ['status'], ['defs', 'parse_ps']])
compare('absolute data_dir via --config', [lambda r, h: open(os.path.join(r, 'abs.toml'), 'w').write(f'data_dir = "{h}/elsewhere"\n'),
        ['index', '--config', 'abs.toml'], ['status', '--config', 'abs.toml'], ['clear', '--config', 'abs.toml']])
compare('usage errors', [['search'], ['bogus'], ['symbols', '--count-by', 'nope']], stderr=False)
print(f'{ok} same, {bad} different')
sys.exit(1 if bad else 0)
