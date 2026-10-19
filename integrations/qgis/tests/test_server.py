"""The local server, without QGIS: the viewer's files, the bridge, Range requests, MBTiles."""

import gzip
import http.client
import json
import os
import socket
import sqlite3
import tempfile
import threading
import unittest

from isard_viewer.server import Bridge, ViewerServer, parse_range


class ParseRange(unittest.TestCase):
    def test_ranges(self):
        self.assertIsNone(parse_range(None, 100))
        self.assertIsNone(parse_range('items=0-1', 100))
        self.assertEqual(parse_range('bytes=0-9', 100), (0, 9))
        self.assertEqual(parse_range('bytes=90-', 100), (90, 99))
        self.assertEqual(parse_range('bytes=90-200', 100), (90, 99))
        self.assertEqual(parse_range('bytes=-10', 100), (90, 99))
        self.assertEqual(parse_range('bytes=-500', 100), (0, 99))
        self.assertEqual(parse_range('bytes=100-', 100), 'unsatisfiable')
        self.assertEqual(parse_range('bytes=5-2', 100), 'unsatisfiable')
        # Several ranges: the whole file
        self.assertIsNone(parse_range('bytes=0-1,5-6', 100))


class BridgeState(unittest.TestCase):
    def test_replay_keeps_the_last_state_in_order(self):
        b = Bridge()
        b.send({'type': 'basemap', 'name': 'osm-positron'})
        b.send({'type': 'addGeoJSON', 'id': 'a', 'data': {}})
        b.send({'type': 'camera', 'center': [2, 48], 'zoom': 5})
        b.send({'type': 'addRaster', 'id': 'b', 'url': 'x'})
        b.send({'type': 'basemap', 'name': 'ign-plan'})
        b.send({'type': 'fitBounds', 'bounds': [1, 2, 3, 4]})
        b.send({'type': 'terrain', 'value': 'ign'})
        b.send({'type': 'addGeoJSON', 'id': 'a', 'data': {'v': 2}})
        b.send({'type': 'addPointCloud', 'id': 'c', 'url': 'y'})
        b.send({'type': 'remove', 'id': 'c'})
        b.send({'type': 'select', 'id': 'a', 'index': 1})
        state = b.state()
        self.assertEqual([m['type'] for m in state], ['basemap', 'terrain', 'fitBounds', 'addRaster', 'addGeoJSON'])
        self.assertEqual(state[0]['name'], 'ign-plan')
        self.assertEqual(state[-1]['data'], {'v': 2})
        b.send({'type': 'clear'})
        self.assertEqual([m['type'] for m in b.state()], ['basemap', 'terrain', 'fitBounds'])

    def test_subscribers_get_the_state_then_the_messages(self):
        b = Bridge()
        b.send({'type': 'basemap', 'name': 'none'})
        q = b.subscribe()
        b.send({'type': 'terrain', 'value': True})
        self.assertEqual(q.get_nowait()['type'], 'basemap')
        self.assertEqual(q.get_nowait()['type'], 'terrain')
        b.close()
        self.assertIsNone(q.get_nowait())

    def test_handler_errors_do_not_escape(self):
        b = Bridge()
        seen = []
        b.on_message(lambda m: 1 / 0)
        b.on_message(seen.append)
        b.receive({'type': 'ready', 'version': '1'})
        self.assertTrue(b.ready)
        self.assertEqual(seen, [{'type': 'ready', 'version': '1'}])


def make_mbtiles(path, fmt, tiles):
    db = sqlite3.connect(path)
    db.execute('CREATE TABLE metadata (name TEXT, value TEXT)')
    db.execute('CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB)')
    db.executemany(
        'INSERT INTO metadata VALUES (?, ?)', [('format', fmt), ('minzoom', '0'), ('maxzoom', '3'), ('name', 't')]
    )
    db.executemany('INSERT INTO tiles VALUES (?, ?, ?, ?)', tiles)
    db.commit()
    db.close()


class Served(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        root = os.path.join(cls.tmp.name, 'viewer')
        os.makedirs(os.path.join(root, 'assets'))
        with open(os.path.join(root, 'index.html'), 'w') as f:
            f.write('<!doctype html><title>viewer</title>')
        with open(os.path.join(root, 'assets', 'app.js'), 'w') as f:
            f.write('export {};')
        with open(os.path.join(cls.tmp.name, 'secret.txt'), 'w') as f:
            f.write('secret')
        cls.data = bytes(range(256)) * 40
        cls.copc = os.path.join(cls.tmp.name, 'cloud.copc.laz')
        with open(cls.copc, 'wb') as f:
            f.write(cls.data)
        cls.png = b'\x89PNG\r\n\x1a\n' + b'p' * 20
        cls.raster = os.path.join(cls.tmp.name, 'raster.mbtiles')
        # Tile z1 x1 y0 (XYZ) is stored at TMS row 1
        make_mbtiles(cls.raster, 'png', [(1, 1, 1, cls.png)])
        cls.pbf = gzip.compress(b'\x1a\x00vector')
        cls.vector = os.path.join(cls.tmp.name, 'vector.mbtiles')
        make_mbtiles(cls.vector, 'pbf', [(2, 3, 0, cls.pbf)])
        cls.server = ViewerServer(root).start()
        cls.port = cls.server.port

    @classmethod
    def tearDownClass(cls):
        cls.server.stop()
        cls.tmp.cleanup()

    def request(self, method, path, headers=None, body=None):
        conn = http.client.HTTPConnection('127.0.0.1', self.port, timeout=5)
        conn.request(method, path, body=body, headers=headers or {})
        res = conn.getresponse()
        data = res.read()
        conn.close()
        return res, data

    def test_viewer_files(self):
        res, body = self.request('GET', '/?bridge=1')
        self.assertEqual(res.status, 200)
        self.assertIn(b'viewer', body)
        self.assertTrue(res.getheader('Content-Type').startswith('text/html'))
        res, _ = self.request('GET', '/assets/app.js')
        self.assertEqual(res.status, 200)
        self.assertTrue(res.getheader('Content-Type').startswith('text/javascript'))
        self.assertEqual(self.request('GET', '/missing.js')[0].status, 404)
        self.assertIn(self.request('GET', '/../secret.txt')[0].status, (403, 404))
        self.assertIn(self.request('GET', '/assets/%2e%2e/%2e%2e/secret.txt')[0].status, (403, 404))

    def test_foreign_hosts_and_origins_are_refused(self):
        res, _ = self.request('GET', '/', headers={'Host': 'evil.example:80'})
        self.assertEqual(res.status, 403)
        body = json.dumps({'type': 'click', 'lonLat': None, 'hit': None})
        res, _ = self.request(
            'POST',
            '/bridge/message',
            headers={'Content-Type': 'text/plain', 'Origin': 'https://evil.example'},
            body=body,
        )
        self.assertEqual(res.status, 403)

    def test_files_with_ranges(self):
        url = self.server.add_file(self.copc)
        self.assertTrue(url.startswith('/files/') and url.endswith('/cloud.copc.laz'))
        self.assertEqual(self.server.add_file(self.copc), url)
        res, body = self.request('GET', url)
        self.assertEqual(res.status, 200)
        self.assertEqual(body, self.data)
        self.assertEqual(res.getheader('Accept-Ranges'), 'bytes')
        res, body = self.request('GET', url, headers={'Range': 'bytes=10-19'})
        self.assertEqual(res.status, 206)
        self.assertEqual(body, self.data[10:20])
        self.assertEqual(res.getheader('Content-Range'), f'bytes 10-19/{len(self.data)}')
        res, body = self.request('GET', url, headers={'Range': 'bytes=-5'})
        self.assertEqual(body, self.data[-5:])
        res, body = self.request('HEAD', url)
        self.assertEqual(res.getheader('Content-Length'), str(len(self.data)))
        self.assertEqual(body, b'')
        res, _ = self.request('GET', url, headers={'Range': f'bytes={len(self.data)}-'})
        self.assertEqual(res.status, 416)
        self.assertEqual(self.request('GET', '/files/nope/cloud.copc.laz')[0].status, 404)

    def test_parallel_range_requests(self):
        url = self.server.add_file(self.copc)
        results = {}

        def fetch(i):
            start = i * 100
            _, body = self.request('GET', url, headers={'Range': f'bytes={start}-{start + 99}'})
            results[i] = body == self.data[start : start + 100]

        threads = [threading.Thread(target=fetch, args=(i,)) for i in range(16)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        self.assertEqual(results, {i: True for i in range(16)})

    def test_mbtiles(self):
        template, tiles = self.server.add_mbtiles(self.raster)
        self.assertFalse(tiles.vector)
        self.assertEqual(tiles.zooms(), (0, 3))
        res, body = self.request('GET', template.format(z=1, x=1, y=0))
        self.assertEqual(res.status, 200)
        self.assertEqual(body, self.png)
        self.assertEqual(res.getheader('Content-Type'), 'image/png')
        self.assertEqual(self.request('GET', template.format(z=1, x=1, y=1))[0].status, 404)
        self.assertEqual(self.request('GET', template.format(z=1, x=5, y=0))[0].status, 404)

        template, tiles = self.server.add_mbtiles(self.vector)
        self.assertTrue(tiles.vector)
        res, body = self.request('GET', template.format(z=2, x=3, y=3) + '.pbf')
        self.assertEqual(res.status, 200)
        self.assertEqual(res.getheader('Content-Encoding'), 'gzip')
        self.assertEqual(res.getheader('Content-Type'), 'application/x-protobuf')
        self.assertEqual(body, self.pbf)

    def test_bridge_events_and_messages(self):
        self.server.send({'type': 'basemap', 'name': 'osm-liberty'})
        self.server.send({'type': 'addGeoJSON', 'id': 'events', 'data': {'type': 'FeatureCollection', 'features': []}})
        sock = socket.create_connection(('127.0.0.1', self.port), timeout=5)
        sock.sendall(f'GET /bridge/events HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\n\r\n'.encode())
        events = []
        buffer = b''
        while b'\r\n\r\n' not in buffer:
            buffer += sock.recv(65536)
        head, buffer = buffer.split(b'\r\n\r\n', 1)
        self.assertIn(b'text/event-stream', head)
        while len(events) < 2:
            buffer += sock.recv(65536)
            while b'\n\n' in buffer:
                block, buffer = buffer.split(b'\n\n', 1)
                for line in block.split(b'\n'):
                    if line.startswith(b'data: '):
                        events.append(json.loads(line[6:]))
        self.assertEqual(events[0], {'type': 'basemap', 'name': 'osm-liberty'})
        self.assertEqual(events[1]['id'], 'events')
        # A message sent after connecting arrives live
        self.server.send({'type': 'remove', 'id': 'events'})
        while b'\n\n' not in buffer:
            buffer += sock.recv(65536)
        self.assertEqual(json.loads(buffer.split(b'\n\n')[0][6:]), {'type': 'remove', 'id': 'events'})
        sock.close()

        received = []
        done = threading.Event()
        self.server.bridge.on_message(lambda m: (received.append(m), done.set()))
        body = json.dumps({'type': 'moveend', 'camera': {}, 'bounds': [0, 0, 1, 1]})
        origin = f'http://127.0.0.1:{self.port}'
        res, _ = self.request(
            'POST', '/bridge/message', headers={'Content-Type': 'application/json', 'Origin': origin}, body=body
        )
        self.assertEqual(res.status, 204)
        self.assertTrue(done.wait(2))
        self.assertEqual(received[-1]['type'], 'moveend')
        res, _ = self.request('POST', '/bridge/message', body='not json')
        self.assertEqual(res.status, 400)


class Shutdown(unittest.TestCase):
    def test_stop_ends_event_streams(self):
        server = ViewerServer(None).start()
        sock = socket.create_connection(('127.0.0.1', server.port), timeout=5)
        sock.sendall(f'GET /bridge/events HTTP/1.1\r\nHost: localhost:{server.port}\r\n\r\n'.encode())
        self.assertIn(b'200', sock.recv(1024))
        server.stop()
        # The stream is closed by the server
        data = b''
        while True:
            chunk = sock.recv(1024)
            if not chunk:
                break
            data += chunk
        sock.close()
        with self.assertRaises(OSError):
            socket.create_connection(('127.0.0.1', server.port), timeout=1).close()

    def test_without_a_viewer_the_page_says_so(self):
        server = ViewerServer(None).start()
        try:
            conn = http.client.HTTPConnection('127.0.0.1', server.port, timeout=5)
            conn.request('GET', '/')
            res = conn.getresponse()
            self.assertEqual(res.status, 404)
            self.assertIn(b'scripts/qgis.ts', res.read())
            conn.close()
        finally:
            server.stop()


if __name__ == '__main__':
    unittest.main()
