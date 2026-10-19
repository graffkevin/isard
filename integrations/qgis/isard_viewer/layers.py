"""QGIS layers to bridge messages: vector layers to styled GeoJSON in WGS84, raster connections to tile URLs, COPC
point clouds and tile files served by the local server."""

import json
import os

from qgis.core import (
    QgsCoordinateReferenceSystem,
    QgsFeatureRequest,
    QgsJsonExporter,
    QgsPointCloudLayer,
    QgsRasterLayer,
    QgsUnitTypes,
    QgsVectorLayer,
    QgsVectorTileLayer,
    QgsWkbTypes,
)

from .convert import (
    clamp_bounds,
    css_color,
    is_copc,
    layer_id,
    local_path,
    mm_to_px,
    raster_source,
    simplify_tolerance,
    vector_tile_source,
)

#: The property holding a feature's color (categorized and graduated renderers)
COLOR_PROPERTY = 'qgis_color'
#: The property holding a feature's QGIS id
FID_PROPERTY = 'qgis_fid'
WGS84 = 'EPSG:4326'


class Skipped(Exception):  # noqa: N818 (a layer skipped is not an error)
    """A layer that cannot be sent, with the reason shown to the user."""


class Sent:
    """What the plugin keeps of a layer sent: its kind and, for vector layers, the feature id of each index."""

    def __init__(self, qgis_id, name, kind, fids=None):
        self.id = layer_id(qgis_id)
        self.qgis_id = qgis_id
        self.name = name
        self.kind = kind
        self.fids = fids or []


def geometry_kind(layer):
    """'point', 'line', 'polygon' or None, under QGIS 3 and 4 (their geometry type enums differ)."""
    try:
        name = QgsWkbTypes.geometryDisplayString(layer.geometryType())
    except Exception:
        name = str(layer.geometryType())
    name = name.lower()
    for kind in ('point', 'line', 'polygon'):
        if kind in name:
            return kind
    return None


def numeric_fields(layer):
    """The names of a vector layer's numeric fields (for extrusions)."""
    if not isinstance(layer, QgsVectorLayer):
        return []
    return [f.name() for f in layer.fields() if f.isNumeric()]


def _unit(symbol):
    try:
        return QgsUnitTypes.encodeUnit(symbol.outputUnit()).lower()
    except Exception:
        return 'mm'


def _color(qcolor):
    return css_color(qcolor.red(), qcolor.green(), qcolor.blue())


def symbol_style(symbol, kind, layer_opacity=1.0):
    """The bridge style of one QGIS symbol: color, opacity, width or radius."""
    color = symbol.color()
    style = {
        'color': _color(color),
        'opacity': round(max(0.0, min(1.0, symbol.opacity() * color.alphaF() * layer_opacity)), 3),
    }
    unit = _unit(symbol)
    if kind == 'line':
        style['width'] = max(0.5, mm_to_px(symbol.width(), unit))
    elif kind == 'point':
        style['radius'] = max(1.0, mm_to_px(symbol.size(), unit) / 2)
    elif kind == 'polygon':
        style['fill'] = style['color']
        sl = symbol.symbolLayer(0) if symbol.symbolLayerCount() else None
        # The viewer's `color` is the outlines' and `fill` the interiors'
        if sl is not None and hasattr(sl, 'strokeColor'):
            style['color'] = _color(sl.strokeColor())
        if sl is not None and hasattr(sl, 'strokeWidth'):
            try:
                style['width'] = mm_to_px(sl.strokeWidth(), QgsUnitTypes.encodeUnit(sl.strokeWidthUnit()).lower())
            except Exception:
                style['width'] = mm_to_px(sl.strokeWidth())
    return style


def renderer_style(layer, kind):
    """``(style, color_of, note)``: the layer's style from its renderer when simple; for categorized or graduated
    renderers a function of a feature giving its color (else None); a note when the renderer was simplified."""
    renderer = layer.renderer()
    opacity = layer.opacity() if hasattr(layer, 'opacity') else 1.0
    rtype = renderer.type() if renderer else ''
    if rtype == 'singleSymbol':
        return symbol_style(renderer.symbol(), kind, opacity), None, None
    if rtype == 'categorizedSymbol':
        field = renderer.classAttribute()
        colors, default = {}, None
        # Kept in a variable: the symbols belong to these copies of the categories
        categories = renderer.categories()
        for cat in categories:
            value, color = cat.value(), _color(cat.symbol().color())
            if value is None or value == '' or (hasattr(value, 'isNull') and value.isNull()):
                default = color
            elif isinstance(value, list):
                for v in value:
                    colors[str(v)] = color
            else:
                colors[str(value)] = color
        base = categories[0].symbol() if categories else renderer.sourceSymbol()
        style = symbol_style(base, kind, opacity) if base else {}
        if layer.fields().indexOf(field) < 0:
            return style, None, f'categories by an expression ({field}): one color'

        def color_of(feature):
            v = feature[field]
            return colors.get(str(v), default)

        return style, color_of, None
    if rtype == 'graduatedSymbol':
        field = renderer.classAttribute()
        classes = renderer.ranges()
        ranges = [(r.lowerValue(), r.upperValue(), _color(r.symbol().color())) for r in classes]
        base = classes[0].symbol() if classes else renderer.sourceSymbol()
        style = symbol_style(base, kind, opacity) if base else {}
        if layer.fields().indexOf(field) < 0:
            return style, None, f'classes by an expression ({field}): one color'

        def color_of(feature):
            try:
                v = float(feature[field])
            except (TypeError, ValueError):
                return None
            for low, high, color in ranges:
                if low <= v <= high:
                    return color
            return None

        return style, color_of, None
    symbols = renderer.symbols(_render_context()) if renderer else []
    if symbols:
        return symbol_style(symbols[0], kind, opacity), None, f'{rtype} renderer drawn with its first symbol'
    return {}, None, f'{rtype or "no"} renderer: default style'


def _render_context():
    from qgis.core import QgsRenderContext

    return QgsRenderContext()


def label_field(layer):
    """The field of the layer's simple labels (not an expression), or None."""
    try:
        if not layer.labelsEnabled() or layer.labeling() is None or layer.labeling().type() != 'simple':
            return None
        settings = layer.labeling().settings()
        return None if settings.isExpression else settings.fieldName or None
    except Exception:
        return None


def vector_message(layer, max_features=50_000, simplify_m=0.0, extrude=None):
    """``(message, Sent, notes)`` for a vector layer: its features (up to ``max_features``) in WGS84, styled."""
    kind = geometry_kind(layer)
    if kind is None:
        raise Skipped('a table without geometries')
    notes = []
    total = layer.featureCount()
    if total > max_features:
        notes.append(f'{max_features:,} features sent out of {total:,}')
    style, color_of, note = renderer_style(layer, kind)
    if note:
        notes.append(note)
    tolerance = simplify_tolerance(simplify_m, layer.crs().isGeographic()) if simplify_m > 0 else 0
    request = QgsFeatureRequest()
    request.setLimit(max_features)
    exporter = QgsJsonExporter(layer, 7)
    exporter.setSourceCrs(layer.crs())
    features, fids, colors = [], [], []
    for feature in layer.getFeatures(request):
        # Features without a geometry are left out: the viewer's indices are those of the features drawn
        if not feature.hasGeometry():
            continue
        if tolerance and kind != 'point':
            simpler = feature.geometry().simplify(tolerance)
            if simpler is not None and not simpler.isEmpty():
                feature.setGeometry(simpler)
        features.append(feature)
        fids.append(feature.id())
        colors.append(color_of(feature) if color_of else None)
    data = json.loads(exporter.exportFeatures(features)) if features else {'type': 'FeatureCollection', 'features': []}
    for i, feat in enumerate(data.get('features', [])):
        props = feat.get('properties') or {}
        props[FID_PROPERTY] = fids[i]
        if colors[i]:
            props[COLOR_PROPERTY] = colors[i]
        feat['properties'] = props
    if color_of:
        style['colorProperty'] = COLOR_PROPERTY
    if extrude and kind == 'polygon':
        style['extrude'] = {'height': extrude}
    elif extrude:
        notes.append('extrusion is for polygons only')
    label = label_field(layer)
    if label:
        style['label'] = label
    sent = Sent(layer.id(), layer.name(), 'geojson', fids)
    return {'type': 'addGeoJSON', 'id': sent.id, 'data': data, 'style': style}, sent, notes


def raster_message(layer, server):
    provider = layer.providerType()
    opacity = layer.renderer().opacity() if layer.renderer() else 1.0
    if provider == 'wms':
        source, reason = raster_source(layer.source())
        if source is None:
            raise Skipped(reason)
        message = {'type': 'addRaster', 'id': layer_id(layer.id()), **source}
    else:
        path = local_path(layer.source())
        if not path or not path.lower().endswith('.mbtiles'):
            raise Skipped('local raster files are not sent (only MBTiles and XYZ / WMS / WMTS connections)')
        url, tiles = server.add_mbtiles(path)
        if tiles.vector:
            raise Skipped('vector MBTiles opened as a raster')
        message = {'type': 'addRaster', 'id': layer_id(layer.id()), 'url': url}
        message.update(_zooms(tiles))
        if tiles.metadata.get('attribution'):
            message['attribution'] = tiles.metadata['attribution']
    attribution = layer.attribution() if hasattr(layer, 'attribution') else ''
    if attribution and 'attribution' not in message:
        message['attribution'] = attribution
    if opacity < 1:
        message['opacity'] = round(opacity, 3)
    return message, Sent(layer.id(), layer.name(), 'raster'), []


def _zooms(tiles):
    out = {}
    zmin, zmax = tiles.zooms()
    if zmin is not None:
        out['minZoom'] = zmin
    if zmax is not None:
        out['maxZoom'] = zmax
    return out


def _source_layers(tiles):
    """The source layers' names of vector MBTiles (their metadata's ``json``), or None."""
    try:
        names = [v['id'] for v in json.loads(tiles.metadata.get('json') or '{}').get('vector_layers', [])]
    except (ValueError, TypeError, KeyError, AttributeError):
        return None
    return names or None


def vector_tile_message(layer, server):
    kind, where, zmin, zmax = vector_tile_source(layer.source())
    lid = layer_id(layer.id())
    if kind is None:
        raise Skipped(where)
    if kind == 'mbtiles':
        if '://' in where:
            raise Skipped('remote MBTiles')
        url, tiles = server.add_mbtiles(where)
        if tiles.vector:
            message = {'type': 'addVectorTiles', 'id': lid, 'url': url, **_zooms(tiles)}
            names = _source_layers(tiles)
            if names:
                message['layers'] = names
        else:
            message = {'type': 'addRaster', 'id': lid, 'url': url, **_zooms(tiles)}
    elif kind == 'pmtiles':
        url = where if '://' in where else server.add_file(where)
        message = {'type': 'addPmtiles', 'id': lid, 'url': url, 'kind': 'vector'}
    else:
        message = {'type': 'addVectorTiles', 'id': lid, 'url': where}
        if zmin is not None:
            message['minZoom'] = zmin
        if zmax is not None:
            message['maxZoom'] = zmax
    return message, Sent(layer.id(), layer.name(), 'tiles'), []


def point_cloud_message(layer, server, color_by=None):
    source = layer.source()
    lid = layer_id(layer.id())
    if source.startswith(('http://', 'https://')):
        if not source.lower().split('?')[0].endswith('.copc.laz'):
            raise Skipped('remote point clouds other than COPC')
        url = source
    else:
        path = local_path(source)
        if not is_copc(path):
            # QGIS's PDAL provider indexes LAS / LAZ files into a COPC file next to them
            sibling = os.path.splitext(path or '')[0] + '.copc.laz'
            if path and os.path.isfile(sibling):
                path = sibling
            else:
                raise Skipped('not a COPC file (.copc.laz); QGIS writes one next to a LAS / LAZ once indexed')
        url = server.add_file(path)
    message = {'type': 'addPointCloud', 'id': lid, 'url': url}
    if color_by:
        message['colorBy'] = color_by
    return message, Sent(layer.id(), layer.name(), 'pointcloud'), []


def layer_message(layer, server, max_features=50_000, simplify_m=0.0, extrude=None, color_by=None):
    """``(message, Sent, notes)`` for any layer, or raises :class:`Skipped`."""
    if isinstance(layer, QgsVectorLayer):
        return vector_message(layer, max_features, simplify_m, extrude)
    if isinstance(layer, QgsRasterLayer):
        return raster_message(layer, server)
    if isinstance(layer, QgsVectorTileLayer):
        return vector_tile_message(layer, server)
    if isinstance(layer, QgsPointCloudLayer):
        return point_cloud_message(layer, server, color_by)
    raise Skipped('this kind of layer is not supported')


def wgs84():
    return QgsCoordinateReferenceSystem(WGS84)


def extent_bounds(rect):
    """A QgsRectangle in degrees to ``[west, south, east, north]`` clamped to the world, or None."""
    return clamp_bounds([rect.xMinimum(), rect.yMinimum(), rect.xMaximum(), rect.yMaximum()])
