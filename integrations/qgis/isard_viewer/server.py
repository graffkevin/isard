"""The local server of the isard viewer: the viewer's files, the bridge, local files and MBTiles.

Pure standard library (no QGIS import), so that it is tested without QGIS. It listens on 127.0.0.1 on a free port in
a daemon thread:

- ``/``: the viewer's built folder (``index.html`` and its assets);
- ``GET /bridge/events``: Server-Sent Events host → viewer, the current state replayed on connect (basemap, terrain,
  camera, every layer still added, in order);
- ``POST /bridge/message``: one JSON message viewer → host, answered 204;
- ``/files/<token>/<name>``: a local file given to :meth:`ViewerServer.add_file`, with HTTP Range requests;
- ``/mbtiles/<token>/{z}/{x}/{y}``: the tiles of an MBTiles file as XYZ (TMS rows flipped, gzip vector tiles sent
  with ``Content-Encoding: gzip``).
"""

import json
import mimetypes
import os
import posixpath
import queue
import re
import secrets
import sqlite3
import threading
from collections import OrderedDict
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import quote, unquote, urlsplit

#: Messages that add a layer: kept by id and replayed to every viewer that connects
LAYER_MESSAGES = frozenset(
    {'addGeoJSON', 'addRaster', 'addPmtiles', 'addPointCloud', 'addTiles3d', 'addModels', 'addVectorTiles'}
)
#: Types not in ``mimetypes`` everywhere (or wrong on some systems)
CONTENT_TYPES = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8',
    '.mjs': 'text/javascript; charset=utf-8',
    '.css': 'text/css; charset=utf-8',
    '.json': 'application/json',
    '.wasm': 'application/wasm',
    '.svg': 'image/svg+xml',
    '.pbf': 'application/x-protobuf',
    '.pmtiles': 'application/octet-stream',
    '.laz': 'application/octet-stream',
    '.glb': 'model/gltf-binary',
    '.gltf': 'model/gltf+json',
}
TILE_TYPES = {
    'png': 'image/png',
    'jpg': 'image/jpeg',
    'jpeg': 'image/jpeg',
    'webp': 'image/webp',
    'pbf': 'application/x-protobuf',
    'mvt': 'application/x-protobuf',
}
CHUNK = 64 * 1024
#: An SSE comment sent when nothing happened for this long, so that dead connections are noticed
KEEPALIVE = 15.0


def content_type(path):
    ext = os.path.splitext(path)[1].lower()
    if ext in CONTENT_TYPES:
        return CONTENT_TYPES[ext]
    guess = mimetypes.guess_type(path)[0]
    return guess or 'application/octet-stream'


def parse_range(header, size):
    """``(start, end)`` inclusive for a single ``bytes=`` range, None to send the whole file, or ``'unsatisfiable'``.

    Several ranges at once are answered with the whole file (allowed by RFC 9110)."""
    if not header:
        return None
    m = re.fullmatch(r'\s*bytes\s*=\s*(\d*)\s*-\s*(\d*)\s*', header)
    if not m:
        return None
    first, last = m.group(1), m.group(2)
    if first == '' and last == '':
        return None
    if first == '':
        suffix = int(last)
        if suffix == 0:
            return 'unsatisfiable'
        return (max(0, size - suffix), size - 1) if size > 0 else 'unsatisfiable'
    start = int(first)
    end = int(last) if last != '' else size - 1
    if start >= size or end < start:
        return 'unsatisfiable'
    return (start, min(end, size - 1))


class Bridge:
    """The state sent to the viewers and their connections (one queue per connected viewer)."""

    def __init__(self):
        self._lock = threading.Lock()
        self._clients = []
        self._basemap = None
        self._terrain = None
        self._camera = None
        self._layers = OrderedDict()
        self._handlers = []
        self.ready = False

    def send(self, message):
        """Sends a message to every connected viewer and keeps what a viewer connecting later needs."""
        kind = message.get('type')
        with self._lock:
            if kind == 'basemap':
                self._basemap = message
            elif kind == 'terrain':
                self._terrain = message
            elif kind in ('camera', 'fitBounds'):
                self._camera = message
            elif kind in LAYER_MESSAGES:
                # A layer of the same id is replaced, drawn above the others as the viewer does
                self._layers.pop(message.get('id'), None)
                self._layers[message.get('id')] = message
            elif kind == 'remove':
                self._layers.pop(message.get('id'), None)
            elif kind == 'clear':
                self._layers.clear()
            clients = list(self._clients)
        for q in clients:
            q.put(message)

    def state(self):
        """The messages replayed to a viewer that connects, in order."""
        with self._lock:
            return self._state()

    def _state(self):
        head = [m for m in (self._basemap, self._terrain, self._camera) if m is not None]
        return head + list(self._layers.values())

    def layer_ids(self):
        with self._lock:
            return list(self._layers.keys())

    def subscribe(self):
        """A queue of the messages for a new connection, the state already in it."""
        q = queue.Queue()
        with self._lock:
            for m in self._state():
                q.put(m)
            self._clients.append(q)
        return q

    def unsubscribe(self, q):
        with self._lock:
            if q in self._clients:
                self._clients.remove(q)

    def clients(self):
        with self._lock:
            return len(self._clients)

    def close(self):
        """Ends every event stream."""
        with self._lock:
            clients, self._clients = self._clients, []
        for q in clients:
            q.put(None)

    def on_message(self, handler):
        """Calls ``handler(message)`` for each message of a viewer, in a server thread."""
        self._handlers.append(handler)

    def receive(self, message):
        if message.get('type') == 'ready':
            self.ready = True
        for handler in list(self._handlers):
            try:
                handler(message)
            except Exception:  # a handler's error must not stop the server
                pass


class MBTiles:
    """An MBTiles file read by tile (a connection per request: the requests come from several threads)."""

    def __init__(self, path):
        self.path = path
        self.metadata = {}
        with self._connect() as db:
            try:
                for name, value in db.execute('SELECT name, value FROM metadata'):
                    self.metadata[name] = value
            except sqlite3.DatabaseError:
                pass
            if not self.metadata.get('format'):
                row = db.execute('SELECT tile_data FROM tiles LIMIT 1').fetchone()
                self.metadata['format'] = sniff_format(row[0]) if row else 'png'

    def _connect(self):
        uri = 'file:' + quote(os.path.abspath(self.path)) + '?mode=ro'
        return sqlite3.connect(uri, uri=True, check_same_thread=False)

    @property
    def format(self):
        return (self.metadata.get('format') or 'png').lower()

    @property
    def vector(self):
        return self.format in ('pbf', 'mvt')

    def zooms(self):
        def num(name):
            try:
                return int(float(self.metadata[name]))
            except (KeyError, ValueError, TypeError):
                return None

        return num('minzoom'), num('maxzoom')

    def tile(self, z, x, y):
        """The bytes of tile z/x/y (XYZ rows), None when missing."""
        row = (1 << z) - 1 - y
        db = self._connect()
        try:
            found = db.execute(
                'SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?', (z, x, row)
            ).fetchone()
        finally:
            db.close()
        return bytes(found[0]) if found and found[0] is not None else None


def sniff_format(data):
    data = bytes(data or b'')
    if data.startswith(b'\x89PNG'):
        return 'png'
    if data.startswith(b'\xff\xd8'):
        return 'jpg'
    if data[:4] == b'RIFF' and data[8:12] == b'WEBP':
        return 'webp'
    return 'pbf'


class _Handler(BaseHTTPRequestHandler):
    server_version = 'isard-qgis/1'
    protocol_version = 'HTTP/1.1'

    # The server instance (a ViewerServer) set on the class built by ViewerServer
    owner = None

    def log_message(self, format, *args):  # noqa: A002 (the base class's name)
        if self.owner.log:
            self.owner.log(format % args)

    # --- dispatch

    def _allowed_host(self):
        port = self.server.server_address[1]
        host = (self.headers.get('Host') or '').strip().lower()
        return host in (f'127.0.0.1:{port}', f'localhost:{port}', f'[::1]:{port}')

    def _guard(self):
        if not self._allowed_host():
            self._error(HTTPStatus.FORBIDDEN)
            return False
        return True

    def do_HEAD(self):
        self._get(head=True)

    def do_GET(self):
        self._get(head=False)

    def _get(self, head):
        if not self._guard():
            return
        path = unquote(urlsplit(self.path).path)
        try:
            if path == '/bridge/events':
                self._events()
            elif path.startswith('/files/'):
                self._file(path, head)
            elif path.startswith('/mbtiles/'):
                self._mbtiles(path, head)
            else:
                self._static(path, head)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            self.close_connection = True

    def do_POST(self):
        if not self._guard():
            return
        path = urlsplit(self.path).path
        if path != '/bridge/message':
            self._error(HTTPStatus.NOT_FOUND)
            return
        origin = self.headers.get('Origin')
        if origin and origin.lower() not in self._origins():
            self._error(HTTPStatus.FORBIDDEN)
            return
        try:
            length = int(self.headers.get('Content-Length') or 0)
        except ValueError:
            length = 0
        if length <= 0 or length > 16 * 1024 * 1024:
            self._error(HTTPStatus.BAD_REQUEST)
            return
        try:
            message = json.loads(self.rfile.read(length).decode('utf-8'))
        except (ValueError, UnicodeDecodeError):
            self._error(HTTPStatus.BAD_REQUEST)
            return
        if not isinstance(message, dict) or not isinstance(message.get('type'), str):
            self._error(HTTPStatus.BAD_REQUEST)
            return
        self.send_response(HTTPStatus.NO_CONTENT)
        self.send_header('Content-Length', '0')
        self.end_headers()
        self.owner.bridge.receive(message)

    def _origins(self):
        port = self.server.server_address[1]
        return {f'http://127.0.0.1:{port}', f'http://localhost:{port}', f'http://[::1]:{port}'}

    # --- responses

    def _error(self, status):
        body = f'{status.value} {status.phrase}\n'.encode()
        self.send_response(status)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        if self.command != 'HEAD':
            self.wfile.write(body)

    def _events(self):
        self.send_response(HTTPStatus.OK)
        self.send_header('Content-Type', 'text/event-stream; charset=utf-8')
        self.send_header('Cache-Control', 'no-cache')
        self.send_header('X-Accel-Buffering', 'no')
        self.end_headers()
        self.close_connection = True
        bridge = self.owner.bridge
        q = bridge.subscribe()
        try:
            self.wfile.write(b'retry: 1000\n\n')
            self.wfile.flush()
            while not self.owner.stopping:
                try:
                    message = q.get(timeout=KEEPALIVE)
                except queue.Empty:
                    self.wfile.write(b': keepalive\n\n')
                    self.wfile.flush()
                    continue
                if message is None:
                    break
                data = json.dumps(message, ensure_ascii=False, separators=(',', ':'), allow_nan=False)
                self.wfile.write(b'data: ' + data.encode('utf-8') + b'\n\n')
                self.wfile.flush()
        finally:
            bridge.unsubscribe(q)

    def _static(self, path, head):
        root = self.owner.root
        if not root or not os.path.isdir(root):
            body = (
                b'<!doctype html><meta charset="utf-8"><title>isard viewer</title>'
                b'<p>The isard viewer is not bundled with this plugin: build it with <code>bun scripts/qgis.ts</code>.'
            )
            self.send_response(HTTPStatus.NOT_FOUND)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            if not head:
                self.wfile.write(body)
            return
        rel = posixpath.normpath(path).lstrip('/')
        if rel in ('', '.'):
            rel = 'index.html'
        parts = [p for p in rel.split('/') if p not in ('', '.')]
        if any(p == '..' for p in parts):
            self._error(HTTPStatus.FORBIDDEN)
            return
        full = os.path.join(root, *parts)
        if os.path.isdir(full):
            full = os.path.join(full, 'index.html')
        real_root = os.path.realpath(root)
        if not os.path.realpath(full).startswith(real_root + os.sep) or not os.path.isfile(full):
            self._error(HTTPStatus.NOT_FOUND)
            return
        self._send_file(full, head, cache='no-cache')

    def _file(self, path, head):
        # /files/<token>/<name>: the name is only there for the viewer (its extension tells the format)
        parts = path.split('/', 3)
        token = parts[2] if len(parts) > 2 else ''
        full = self.owner.files.get(token)
        if not full or not os.path.isfile(full):
            self._error(HTTPStatus.NOT_FOUND)
            return
        self._send_file(full, head, cache='no-store')

    def _send_file(self, full, head, cache):
        size = os.path.getsize(full)
        wanted = parse_range(self.headers.get('Range'), size)
        if wanted == 'unsatisfiable':
            self.send_response(HTTPStatus.REQUESTED_RANGE_NOT_SATISFIABLE)
            self.send_header('Content-Range', f'bytes */{size}')
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        start, end = wanted if wanted else (0, size - 1)
        length = max(0, end - start + 1)
        self.send_response(HTTPStatus.PARTIAL_CONTENT if wanted else HTTPStatus.OK)
        self.send_header('Content-Type', content_type(full))
        self.send_header('Content-Length', str(length))
        self.send_header('Accept-Ranges', 'bytes')
        self.send_header('Cache-Control', cache)
        if wanted:
            self.send_header('Content-Range', f'bytes {start}-{end}/{size}')
        self.end_headers()
        if head or length == 0:
            return
        with open(full, 'rb') as f:
            f.seek(start)
            left = length
            while left > 0:
                chunk = f.read(min(CHUNK, left))
                if not chunk:
                    break
                self.wfile.write(chunk)
                left -= len(chunk)

    def _mbtiles(self, path, head):
        m = re.fullmatch(r'/mbtiles/([^/]+)/(\d+)/(\d+)/(\d+)(?:\.\w+)?', path)
        if not m:
            self._error(HTTPStatus.NOT_FOUND)
            return
        tiles = self.owner.mbtiles.get(m.group(1))
        if tiles is None:
            self._error(HTTPStatus.NOT_FOUND)
            return
        z, x, y = int(m.group(2)), int(m.group(3)), int(m.group(4))
        if z > 30 or x >= (1 << z) or y >= (1 << z):
            self._error(HTTPStatus.NOT_FOUND)
            return
        try:
            data = tiles.tile(z, x, y)
        except sqlite3.Error:
            self._error(HTTPStatus.INTERNAL_SERVER_ERROR)
            return
        if data is None:
            self._error(HTTPStatus.NOT_FOUND)
            return
        self.send_response(HTTPStatus.OK)
        self.send_header('Content-Type', TILE_TYPES.get(tiles.format, 'application/octet-stream'))
        if data[:2] == b'\x1f\x8b':
            self.send_header('Content-Encoding', 'gzip')
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-cache')
        self.end_headers()
        if not head:
            self.wfile.write(data)


class ViewerServer:
    """The viewer, the bridge and the local files on ``http://127.0.0.1:<port>``, in a daemon thread."""

    def __init__(self, root, port=0, log=None):
        self.root = root
        self.log = log
        self.bridge = Bridge()
        self.files = {}
        self.mbtiles = {}
        self.stopping = False
        handler = type('Handler', (_Handler,), {'owner': self})
        self._httpd = ThreadingHTTPServer(('127.0.0.1', port), handler)
        self._httpd.daemon_threads = True
        self._thread = None

    @property
    def port(self):
        return self._httpd.server_address[1]

    @property
    def url(self):
        return f'http://127.0.0.1:{self.port}/'

    @property
    def viewer_url(self):
        """The viewer page with the HTTP bridge enabled."""
        return f'{self.url}?bridge=1'

    def start(self):
        if self._thread is None:
            self._thread = threading.Thread(target=self._httpd.serve_forever, name='isard-viewer', daemon=True)
            self._thread.start()
        return self

    def stop(self):
        """Ends the event streams, stops listening and waits for the server's thread."""
        if self.stopping:
            return
        self.stopping = True
        self.bridge.close()
        if self._thread is not None:
            self._httpd.shutdown()
            self._thread.join(timeout=5)
        self._httpd.server_close()

    def send(self, message):
        self.bridge.send(message)

    def add_file(self, path):
        """The relative URL serving a local file (with Range requests); the same file keeps its URL."""
        full = os.path.abspath(path)
        for token, known in self.files.items():
            if known == full:
                return self._file_url(token, full)
        token = secrets.token_urlsafe(9)
        self.files[token] = full
        return self._file_url(token, full)

    @staticmethod
    def _file_url(token, full):
        return f'/files/{token}/{quote(os.path.basename(full))}'

    def add_mbtiles(self, path):
        """``(template, MBTiles)``: the relative XYZ template of an MBTiles file and its metadata."""
        full = os.path.abspath(path)
        for token, known in self.mbtiles.items():
            if known.path == full:
                return f'/mbtiles/{token}/{{z}}/{{x}}/{{y}}', known
        tiles = MBTiles(full)
        token = secrets.token_urlsafe(9)
        self.mbtiles[token] = tiles
        return f'/mbtiles/{token}/{{z}}/{{x}}/{{y}}', tiles
