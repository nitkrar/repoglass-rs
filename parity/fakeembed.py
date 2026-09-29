import hashlib, json, struct
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert self.headers.get('Authorization') == 'Bearer sekrit', self.headers.get('Authorization')
        data = []
        for i, t in enumerate(body['input']):
            d = hashlib.sha256(t.encode()).digest()
            vec = [((b - 128) / 128.0) for b in d[:16]]
            data.append({'index': i, 'embedding': vec})
        data.reverse()
        out = json.dumps({'data': data}).encode()
        self.send_response(200); self.send_header('Content-Type', 'application/json'); self.send_header('Content-Length', str(len(out))); self.end_headers(); self.wfile.write(out)
    def log_message(self, *a): pass
HTTPServer(('127.0.0.1', 8765), H).serve_forever()
