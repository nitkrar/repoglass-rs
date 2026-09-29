"""A read that needs a refresh while another process holds the write lock, Python 0.3.2 vs Rust.

A separate process holds `BEGIN IMMEDIATE` for HOLD seconds, longer than SQLite's 5 s busy
timeout, so the refresh gives up and the read serves what the index holds.
"""
import glob, os, shutil, sqlite3, subprocess, sys, time
PY, RS = os.environ['PY_RPG'], os.environ['RS_RPG']
HOLDER = ("import sqlite3, sys, time\n"
          "c = sqlite3.connect(sys.argv[1], isolation_level=None)\n"
          "c.execute('BEGIN IMMEDIATE'); print('held', flush=True); time.sleep(float(sys.argv[2])); c.execute('ROLLBACK')\n")


def case(b, tag, built, hold):
    root, home = f'/tmp/lk-{tag}/tree', f'/tmp/lk-{tag}/home'
    shutil.rmtree(f'/tmp/lk-{tag}', ignore_errors=True)
    os.makedirs(f'/tmp/lk-{tag}')
    shutil.copytree(os.environ['RPG_SMALL_TREE'], root)
    env = dict(os.environ, REPOGLASS_HOME=home)
    subprocess.run([b, 'index' if built else 'status'], cwd=root, env=env, capture_output=True)
    db = glob.glob(f'{home}/index/*/index.db')[0]
    c0 = sqlite3.connect(db)
    c0.execute('UPDATE meta SET last_scan_at = ?', (1 if built else 0,))
    c0.commit()
    c0.close()
    open(os.path.join(root, 'lock_probe.py'), 'w').write('def lock_probe_symbol():\n    return "written while the index is locked"\n')
    holder = subprocess.Popen([sys.executable, '-c', HOLDER, db, str(hold)], stdout=subprocess.PIPE, text=True)
    holder.stdout.readline()
    start = time.time()
    p = subprocess.run([b, 'defs', 'lock_probe_symbol'], cwd=root, env=env, capture_output=True, text=True)
    holder.wait()
    return p.returncode, p.stdout.strip()[:60], round(time.time() - start)


failed = False
for built, hold in ((True, 7), (False, 7), (True, 12)):
    label = f"{'built' if built else 'never built'}, lock held {hold}s"
    a, b = case(PY, 'py', built, hold), case(RS, 'rs', built, hold)
    print(f"{'ok  ' if a == b else 'DIFF'} {label}: py={a} rs={b}")
    failed = failed or a != b
sys.exit(1 if failed else 0)
