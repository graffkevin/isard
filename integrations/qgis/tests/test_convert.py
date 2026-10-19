"""Provider URIs to tile URLs, colors and bounds, without QGIS."""

import unittest
from urllib.parse import parse_qs, quote, urlsplit

from isard_viewer.convert import (
    bounds_close,
    clamp_bounds,
    css_color,
    is_copc,
    local_path,
    mm_to_px,
    raster_source,
    vector_tile_source,
)


class RasterSource(unittest.TestCase):
    def test_xyz(self):
        url = 'https://tile.openstreetmap.org/{z}/{x}/{y}.png'
        source, reason = raster_source(f'type=xyz&url={quote(url, safe="")}&zmax=19&zmin=0')
        self.assertIsNone(reason)
        self.assertEqual(source, {'url': url, 'minZoom': 0, 'maxZoom': 19})

    def test_xyz_local_or_quadkey(self):
        self.assertIsNone(raster_source('type=xyz&url=file:///tiles/{z}/{x}/{y}.png')[0])
        self.assertIsNone(raster_source('type=xyz&url=https%3A%2F%2Fa%2F%7Bq%7D.png')[0])

    def test_wms(self):
        uri = 'contextualWMSLegend=0&crs=EPSG:2154&dpiMode=7&format=image/png&layers=a&layers=b&styles=&styles=&url=https://data.geopf.fr/wms-r?SERVICE%3DWMS'
        source, _ = raster_source(uri)
        parts = urlsplit(source['url'])
        self.assertEqual(parts.netloc, 'data.geopf.fr')
        q = parse_qs(parts.query, keep_blank_values=True)
        self.assertEqual(q['REQUEST'], ['GetMap'])
        self.assertEqual(q['LAYERS'], ['a,b'])
        self.assertEqual(q['CRS'], ['EPSG:3857'])
        self.assertIn('BBOX={bbox}', source['url'])
        self.assertEqual(list(q).count('SERVICE'), 1)

    def test_wmts(self):
        uri = (
            'crs=EPSG:3857&dpiMode=7&format=image/jpeg&layers=ORTHOIMAGERY.ORTHOPHOTOS&styles=normal'
            '&tileMatrixSet=PM&url=https://data.geopf.fr/wmts?SERVICE%3DWMTS%26REQUEST%3DGetCapabilities'
        )
        source, _ = raster_source(uri)
        self.assertTrue(source['url'].startswith('https://data.geopf.fr/wmts?'))
        for part in ('REQUEST=GetTile', 'TILEMATRIX={z}', 'TILEROW={y}', 'TILECOL={x}', 'TILEMATRIXSET=PM'):
            self.assertIn(part, source['url'])
        self.assertNotIn('GetCapabilities', source['url'])
        source, reason = raster_source(uri.replace('EPSG:3857', 'EPSG:2154'))
        self.assertIsNone(source)
        self.assertIn('Web Mercator', reason)

    def test_unknown(self):
        self.assertEqual(raster_source('')[0], None)


class VectorTileSource(unittest.TestCase):
    def test_kinds(self):
        self.assertEqual(
            vector_tile_source('type=xyz&url=https://a.b/{z}/{x}/{y}.pbf&zmax=14&zmin=0'),
            ('xyz', 'https://a.b/{z}/{x}/{y}.pbf', 0, 14),
        )
        self.assertEqual(vector_tile_source('type=mbtiles&url=/data/t.mbtiles')[:2], ('mbtiles', '/data/t.mbtiles'))
        self.assertEqual(vector_tile_source('type=xyz&url=file:///data/t.pmtiles')[:2], ('pmtiles', '/data/t.pmtiles'))
        self.assertIsNone(vector_tile_source('type=arcgis&url=https://x')[0])


class Helpers(unittest.TestCase):
    def test_local_path(self):
        self.assertEqual(local_path('/a/b.gpkg|layername=x'), '/a/b.gpkg')
        self.assertEqual(local_path('file:///a/b%20c.laz'), '/a/b c.laz')
        self.assertIsNone(local_path('https://a/b.laz'))
        self.assertTrue(is_copc('/x/Y.COPC.LAZ'))
        self.assertFalse(is_copc('/x/y.laz'))

    def test_colors_and_sizes(self):
        self.assertEqual(css_color(255, 0, 16), '#ff0010')
        self.assertEqual(css_color(255, 0, 16, 128), '#ff001080')
        self.assertAlmostEqual(mm_to_px(0.26), 0.98, places=2)
        self.assertEqual(mm_to_px(3, 'Pixel'), 3)

    def test_bounds(self):
        self.assertEqual(clamp_bounds([-200, -90, 10, 89]), [-180, -85.0511287798, 10, 85.0511287798])
        self.assertIsNone(clamp_bounds([1, 1, 1, 2]))
        self.assertIsNone(clamp_bounds([0, 0, float('nan'), 1]))
        self.assertTrue(bounds_close([0, 0, 1, 1], [0.001, 0, 1, 1.001]))
        self.assertFalse(bounds_close([0, 0, 1, 1], [0.1, 0, 1, 1]))
        self.assertFalse(bounds_close(None, [0, 0, 1, 1]))


if __name__ == '__main__':
    unittest.main()
