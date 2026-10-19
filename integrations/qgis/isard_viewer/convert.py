"""What the plugin derives from QGIS without QGIS: provider URIs to tile URLs, colors, bounds (tested without QGIS)."""

import math
from urllib.parse import parse_qsl, unquote, urlencode, urlsplit, urlunsplit

#: The latitudes of Web Mercator
MAX_LAT = 85.0511287798
#: Millimetres (QGIS symbol units) to CSS pixels at 96 dpi
MM_TO_PX = 96 / 25.4

BASEMAPS = [
    ('OpenStreetMap Positron', 'osm-positron'),
    ('OpenStreetMap Liberty', 'osm-liberty'),
    ('PLAN IGN', 'ign-plan'),
    ('IGN orthophotos', 'ign-ortho'),
    ('None', 'none'),
]
TERRAINS = [('Off', False), ('Worldwide', True), ('IGN (France)', 'ign')]
POINT_COLORS = [('RGB', 'rgb'), ('Class', 'classification'), ('Altitude', 'elevation'), ('Intensity', 'intensity')]


def parse_uri(uri):
    """The key/value pairs of a QGIS ``wms`` / ``vectortile`` provider URI (``a=1&url=https%3A…``), decoded."""
    out = {}
    for key, value in parse_qsl(uri or '', keep_blank_values=True):
        # Repeated keys (layers=…&layers=…, styles=…) are kept as lists joined later
        if key in out:
            prev = out[key]
            out[key] = (prev if isinstance(prev, list) else [prev]) + [value]
        else:
            out[key] = value
    return out


def _first(value):
    return value[0] if isinstance(value, list) else value


def _joined(value):
    return ','.join(value) if isinstance(value, list) else (value or '')


def _int(value):
    try:
        return int(float(_first(value)))
    except (TypeError, ValueError):
        return None


def _with_query(url, params):
    """``url`` with ``params`` added to its query (keys already there replaced, case-insensitively)."""
    parts = urlsplit(url)
    names = {k.lower() for k in params}
    kept = [(k, v) for k, v in parse_qsl(parts.query, keep_blank_values=True) if k.lower() not in names]
    query = urlencode(kept + list(params.items()), safe='{},:/')
    return urlunsplit((parts.scheme, parts.netloc, parts.path, query, parts.fragment))


def raster_source(uri):
    """``{'url', 'minZoom'?, 'maxZoom'?}`` for an ``addRaster`` message from a ``wms`` provider URI, or
    ``(None, reason)`` when it cannot be drawn by URL. XYZ templates are kept; WMS gets a GetMap URL in EPSG:3857 with
    ``{bbox}``; WMTS a KVP GetTile URL, for Web Mercator matrix sets whose matrices are named by their zoom."""
    params = parse_uri(uri)
    url = _first(params.get('url')) or ''
    kind = (_first(params.get('type')) or '').lower()
    if not url:
        return None, 'no URL in the layer source'
    if kind == 'xyz':
        if url.startswith('file:') or not url.startswith(('http://', 'https://')):
            return None, 'local XYZ tiles are not served'
        if '{q}' in url:
            return None, 'quadkey ({q}) URLs are not supported'
        out = {'url': url}
        if (z := _int(params.get('zmin'))) is not None:
            out['minZoom'] = z
        if (z := _int(params.get('zmax'))) is not None:
            out['maxZoom'] = z
        return out, None
    if 'tileMatrixSet' in params:
        crs = (_first(params.get('crs')) or '').upper()
        if crs not in ('EPSG:3857', 'EPSG:900913'):
            return None, f'WMTS in {crs or "an unknown CRS"}: only Web Mercator (EPSG:3857) matrix sets are drawn'
        base = url.split('?', 1)[0]
        query = {
            'SERVICE': 'WMTS',
            'REQUEST': 'GetTile',
            'VERSION': '1.0.0',
            'LAYER': _joined(params.get('layers')),
            'STYLE': _joined(params.get('styles')) or 'default',
            'TILEMATRIXSET': _first(params.get('tileMatrixSet')),
            'TILEMATRIX': '{z}',
            'TILEROW': '{y}',
            'TILECOL': '{x}',
            'FORMAT': _first(params.get('format')) or 'image/png',
        }
        return {'url': _with_query(base, query)}, None
    if params.get('layers') is not None:
        query = {
            'SERVICE': 'WMS',
            'REQUEST': 'GetMap',
            'VERSION': '1.3.0',
            'LAYERS': _joined(params.get('layers')),
            'STYLES': _joined(params.get('styles')),
            'FORMAT': _first(params.get('format')) or 'image/png',
            'TRANSPARENT': 'TRUE',
            'CRS': 'EPSG:3857',
            'WIDTH': '256',
            'HEIGHT': '256',
            'BBOX': '{bbox}',
        }
        return {'url': _with_query(url, query)}, None
    return None, 'not an XYZ, WMS or WMTS source'


def vector_tile_source(uri):
    """``('xyz', url, minZoom, maxZoom)``, ``('mbtiles', path, …)``, ``('pmtiles', url or path, …)`` from a
    ``vectortile`` provider URI, or ``(None, reason, None, None)``."""
    params = parse_uri(uri)
    url = _first(params.get('url')) or ''
    kind = (_first(params.get('type')) or '').lower()
    zmin, zmax = _int(params.get('zmin')), _int(params.get('zmax'))
    if not url:
        return None, 'no URL in the layer source', None, None
    local = local_path(url)
    if kind == 'mbtiles' or url.lower().endswith('.mbtiles'):
        return 'mbtiles', local or url, zmin, zmax
    if kind == 'pmtiles' or url.lower().split('?')[0].endswith('.pmtiles'):
        return 'pmtiles', local or url, zmin, zmax
    if kind == 'xyz' and url.startswith(('http://', 'https://')):
        return 'xyz', url, zmin, zmax
    if kind == 'arcgis':
        return None, 'ArcGIS vector tile services are not supported', None, None
    return None, 'not an XYZ, MBTiles or PMTiles vector tile source', None, None


def local_path(source):
    """The local file of a layer source (``/a/b.laz``, ``file:///a/b.laz``, ``/a/b.gpkg|layername=x``), or None."""
    if not source:
        return None
    path = source.split('|', 1)[0]
    if path.startswith('file://'):
        path = unquote(urlsplit(path).path)
    elif '://' in path:
        return None
    return path or None


def is_copc(path):
    return bool(path) and path.lower().endswith('.copc.laz')


def css_color(r, g, b, a=255):
    """``#rrggbb`` (or ``#rrggbbaa`` when not opaque) from 0–255 components."""
    parts = [r, g, b] + ([a] if a < 255 else [])
    return '#' + ''.join(f'{max(0, min(255, int(round(c)))):02x}' for c in parts)


def mm_to_px(mm, unit='mm'):
    """A QGIS symbol size in CSS pixels (millimetres, points, pixels; map units unknown here: 1 px)."""
    unit = (unit or 'mm').lower()
    if unit in ('mm', 'millimeters', 'millimeter'):
        return round(mm * MM_TO_PX, 2)
    if unit in ('pt', 'points', 'point'):
        return round(mm * 96 / 72, 2)
    if unit in ('px', 'pixels', 'pixel'):
        return round(mm, 2)
    if unit in ('in', 'inches', 'inch'):
        return round(mm * 96, 2)
    return 1.0


def clamp_bounds(bounds):
    """``[west, south, east, north]`` within the world (latitudes of Web Mercator), or None when empty or invalid."""
    if not bounds or len(bounds) != 4 or not all(isinstance(v, (int, float)) and math.isfinite(v) for v in bounds):
        return None
    w, s, e, n = bounds
    w, e = max(-180.0, min(180.0, w)), max(-180.0, min(180.0, e))
    s, n = max(-MAX_LAT, min(MAX_LAT, s)), max(-MAX_LAT, min(MAX_LAT, n))
    if e <= w or n <= s:
        return None
    return [w, s, e, n]


def bounds_close(a, b, tolerance=0.02):
    """True when two bounds differ by less than ``tolerance`` of their size on every side (to break the loop of a
    viewer following QGIS following the viewer)."""
    if not a or not b:
        return False
    span = max(a[2] - a[0], a[3] - a[1], b[2] - b[0], b[3] - b[1], 1e-9)
    return all(abs(x - y) <= tolerance * span for x, y in zip(a, b))


def layer_id(qgis_id):
    """The id of a QGIS layer in the viewer."""
    return f'qgis-{qgis_id}'


def simplify_tolerance(metres, geographic):
    """A simplification tolerance in the layer's units: degrees for a geographic CRS, else metres."""
    return metres / 111_320 if geographic else metres
