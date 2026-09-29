"""Compare two repoglass indexes by content, not id.

Python 0.3.2 orders a file's symbols and chunks by py-tree-sitter's
capture order, which varies between processes, so ids are not stable
even between two Python builds of one tree.
"""
import json, sqlite3, sys
from collections import Counter
import numpy as np

py, rs = (sqlite3.connect(p) for p in sys.argv[1:3])
failures = 0

def compare(label, sql):
    global failures
    a, b = Counter(py.execute(sql).fetchall()), Counter(rs.execute(sql).fetchall())
    if a == b:
        print(f"{label}: identical ({sum(a.values())} rows)")
        return
    failures += 1
    only_a, only_b = a - b, b - a
    print(f"{label}: DIFFER a={sum(a.values())} b={sum(b.values())}"
          f" only_a={sum(only_a.values())} only_b={sum(only_b.values())}")
    for r in sorted(only_a, key=repr)[:4]:
        print("   a:", repr(r)[:300])
    for r in sorted(only_b, key=repr)[:4]:
        print("   b:", repr(r)[:300])

compare("meta identity", "SELECT schema_rev, embed_model, embed_backend, embed_dims, embed_variant,"
        " embed_doc_prefix, embed_pooling, coverage, extractor_rev, categories_rev, chunking_rev FROM meta")
compare("rg_meta declaration/space", "SELECT declaration, space, keywords_stale FROM rg_meta")
compare("file", "SELECT path, mtime_ns, size, lang, path_words, content_type FROM file")
compare("symbol", "SELECT f.path, s.name, s.tag, s.start_line, s.end_line, s.signature,"
        " e.name, e.start_line FROM symbol s JOIN file f ON f.id = s.file_id"
        " LEFT JOIN symbol e ON e.id = s.enclosing_id")
compare("chunk", "SELECT f.path, s.name, c.start_line, c.end_line, c.content_hash FROM chunk c"
        " JOIN file f ON f.id = c.file_id LEFT JOIN symbol s ON s.id = c.symbol_id")
compare("rg_items", "SELECT i.text, i.keywords, i.keyword_override, i.content_type, i.lang, i.path,"
        " i.extra, c.start_line, c.end_line FROM rg_items i JOIN chunk c ON c.id = i.id")
compare("items without a chunk", "SELECT count(*) FROM rg_items WHERE id NOT IN (SELECT id FROM chunk)")

vec_sql = ("SELECT f.path, c.start_line, c.end_line, c.content_hash, v.vec FROM rg_vectors v"
           " JOIN chunk c ON c.id = v.id JOIN file f ON f.id = c.file_id")
a = {r[:4]: r[4] for r in py.execute(vec_sql)}
b = {r[:4]: r[4] for r in rs.execute(vec_sql)}
same = sum(1 for k in a if b.get(k) == a[k])
print(f"rg_vectors: a={len(a)} b={len(b)} matched={len(a.keys() & b.keys())} bit-identical={same}")
diff = [k for k in a if k in b and a[k] != b[k]]
if diff:
    failures += 1
    va = np.stack([np.frombuffer(a[k], "<f2").astype("f4") for k in diff])
    vb = np.stack([np.frombuffer(b[k], "<f2").astype("f4") for k in diff])
    cos = (va * vb).sum(1) / np.linalg.norm(va, axis=1) / np.linalg.norm(vb, axis=1)
    print(f"   {len(diff)} differ: {int((va != vb).sum())} components,"
          f" max abs {np.abs(va - vb).max():.3g}, min cosine {cos.min():.7f}")
ca, cb = (json.loads(c.execute("SELECT canary FROM rg_meta").fetchone()[0]) for c in (py, rs))
print("canary values:", "identical" if ca == cb else "DIFFER")
print("RESULT:", "all content identical" if failures == 0 and ca == cb else f"{failures} tables differ")
sys.exit(0 if failures == 0 and ca == cb else 1)
