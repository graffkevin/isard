"""The plugin: a toolbar button and menu entry opening the dock, the local server, and the sync with QGIS."""

import os
import time

from qgis.core import (
    Qgis,
    QgsCoordinateTransform,
    QgsCsException,
    QgsFeatureRequest,
    QgsProject,
    QgsRectangle,
    QgsSettings,
    QgsVectorLayer,
)
from qgis.PyQt.QtCore import QObject, QTimer, QUrl, pyqtSignal
from qgis.PyQt.QtGui import QDesktopServices, QIcon

from .convert import bounds_close
from .layers import Skipped, extent_bounds, layer_message, numeric_fields, wgs84
from .server import ViewerServer

HERE = os.path.dirname(os.path.abspath(__file__))
SETTINGS = 'isard_viewer/'
#: How long after the plugin moved one side the other side's move is taken as its echo (seconds)
ECHO = 1.5


def viewer_root():
    """The viewer bundled by scripts/qgis.ts, else the repository's build (the plugin linked from a checkout)."""
    bundled = os.path.join(HERE, 'viewer')
    if os.path.isfile(os.path.join(bundled, 'index.html')):
        return bundled
    checkout = os.path.normpath(os.path.join(HERE, '..', '..', '..', 'apps', 'viewer', 'dist'))
    return checkout if os.path.isdir(checkout) else bundled


class _Relay(QObject):
    """Brings the viewer's messages from the server's threads to the Qt main thread (a queued signal)."""

    received = pyqtSignal(object)


class IsardViewerPlugin:
    def __init__(self, iface):
        self.iface = iface
        self.action = None
        self.dock = None
        self.server = None
        self.sent = {}
        self._relay = None
        self._timer = None
        self._status_timer = None
        self._last_sent = None
        self._sent_at = 0.0
        self._from_viewer_at = 0.0
        self._connected = []

    # --- QGIS plugin interface

    def initGui(self):  # noqa: N802 (QGIS's name)
        from qgis.PyQt.QtWidgets import QAction

        icon = QIcon(os.path.join(HERE, 'icon.png'))
        self.action = QAction(icon, 'isard 3D view', self.iface.mainWindow())
        self.action.setToolTip('The map in isard 3D view (globe, relief, buildings, point clouds)')
        self.action.triggered.connect(self.show)
        self.iface.addToolBarIcon(self.action)
        self.iface.addPluginToMenu('&isard', self.action)

    def unload(self):
        if self.action is not None:
            self.iface.removePluginMenu('&isard', self.action)
            self.iface.removeToolBarIcon(self.action)
            self.action = None
        for signal, slot in self._connected:
            try:
                signal.disconnect(slot)
            except (TypeError, RuntimeError):
                pass
        self._connected = []
        for timer in (self._timer, self._status_timer):
            if timer is not None:
                timer.stop()
        if self.dock is not None:
            if self.dock.web is not None:
                self.dock.web.setUrl(QUrl('about:blank'))
            self.iface.removeDockWidget(self.dock)
            self.dock.deleteLater()
            self.dock = None
        if self.server is not None:
            self.server.stop()
            self.server = None

    # --- setup

    def _connect(self, signal, slot):
        signal.connect(slot)
        self._connected.append((signal, slot))

    def show(self):
        if self.dock is None:
            self._start()
        self.dock.show()
        self.dock.raise_()
        if self.dock.web is None and not self.server.bridge.clients():
            self.open_browser()

    def _start(self):
        from .dock import IsardDock

        self.server = ViewerServer(viewer_root()).start()
        self._relay = _Relay()
        self._relay.received.connect(self._on_viewer)
        self.server.bridge.on_message(self._relay.received.emit)

        self.dock = IsardDock(self.iface.mainWindow())
        self.iface.addDockWidget(_right_area(), self.dock)
        d = self.dock
        s = QgsSettings()
        d.basemap.setCurrentIndex(max(0, d.basemap.findData(s.value(SETTINGS + 'basemap', 'osm-positron'))))
        terrain = s.value(SETTINGS + 'terrain', 'off')
        d.terrain.setCurrentIndex({'off': 0, 'true': 1, 'ign': 2}.get(str(terrain), 0))
        d.follow.setChecked(_bool(s.value(SETTINGS + 'follow', True)))
        d.reverse.setChecked(_bool(s.value(SETTINGS + 'reverse', False)))
        d.max_features.setValue(int(s.value(SETTINGS + 'maxFeatures', 50_000)))

        d.open_browser.clicked.connect(self.open_browser)
        d.basemap.currentIndexChanged.connect(self._send_basemap)
        d.terrain.currentIndexChanged.connect(self._send_terrain)
        d.follow.toggled.connect(self._follow_toggled)
        d.reverse.toggled.connect(lambda on: QgsSettings().setValue(SETTINGS + 'reverse', on))
        d.max_features.valueChanged.connect(lambda v: QgsSettings().setValue(SETTINGS + 'maxFeatures', v))
        d.send.clicked.connect(self.send_selected)
        d.remove.clicked.connect(self.remove_selected)
        d.clear.clicked.connect(self.clear)

        self._timer = QTimer()
        self._timer.setSingleShot(True)
        self._timer.setInterval(250)
        self._timer.timeout.connect(self.send_camera)
        canvas = self.iface.mapCanvas()
        self._connect(canvas.extentsChanged, self._canvas_moved)
        self._connect(canvas.destinationCrsChanged, self._canvas_moved)
        self._connect(self.iface.currentLayerChanged, self._layer_changed)
        self._connect(QgsProject.instance().layerWillBeRemoved, self._layer_removed)

        self._send_basemap()
        self._send_terrain()
        self.send_camera(force=True)
        self._layer_changed(self.iface.activeLayer())
        # The number of viewers connected, shown as they come and go
        self._status_timer = QTimer()
        self._status_timer.setInterval(2000)
        self._status_timer.timeout.connect(self._status)
        self._status_timer.start()
        self._status()
        d.load(self.server.viewer_url)

    def _canvas_moved(self, *_):
        self._timer.start()

    # --- view

    def open_browser(self):
        QDesktopServices.openUrl(QUrl(self.server.viewer_url))

    def _send_basemap(self, *_):
        name = self.dock.basemap.currentData()
        QgsSettings().setValue(SETTINGS + 'basemap', name)
        self.server.send({'type': 'basemap', 'name': name})

    def _send_terrain(self, *_):
        value = self.dock.terrain.currentData()
        QgsSettings().setValue(SETTINGS + 'terrain', {False: 'off', True: 'true'}.get(value, value))
        self.server.send({'type': 'terrain', 'value': value})

    def _follow_toggled(self, on):
        QgsSettings().setValue(SETTINGS + 'follow', on)
        if on:
            self.send_camera(force=True)

    def canvas_bounds(self):
        """The map canvas's extent in degrees, ``[west, south, east, north]``, or None."""
        canvas = self.iface.mapCanvas()
        try:
            xform = QgsCoordinateTransform(canvas.mapSettings().destinationCrs(), wgs84(), QgsProject.instance())
            return extent_bounds(xform.transformBoundingBox(canvas.extent()))
        except QgsCsException:
            return None

    def send_camera(self, force=False):
        if self.server is None or not (force or self.dock.follow.isChecked()):
            return
        bounds = self.canvas_bounds()
        if bounds is None:
            return
        # The canvas moved by the viewer: its extent is not sent back
        if not force and time.monotonic() - self._from_viewer_at < ECHO:
            self._last_sent = bounds
            return
        if not force and bounds_close(bounds, self._last_sent, 0.001):
            return
        self._last_sent = bounds
        self._sent_at = time.monotonic()
        self.server.send({'type': 'fitBounds', 'bounds': bounds, 'padding': 0, 'duration': 0 if force else 400})

    def _viewer_moved(self, message):
        if not self.dock.reverse.isChecked():
            return
        bounds = message.get('bounds')
        if not bounds or len(bounds) != 4:
            return
        # The viewer following QGIS: its move is not sent back
        if time.monotonic() - self._sent_at < ECHO or bounds_close(bounds, self._last_sent, 0.05):
            return
        canvas = self.iface.mapCanvas()
        try:
            xform = QgsCoordinateTransform(wgs84(), canvas.mapSettings().destinationCrs(), QgsProject.instance())
            rect = xform.transformBoundingBox(QgsRectangle(*bounds))
        except QgsCsException:
            return
        self._from_viewer_at = time.monotonic()
        self._last_sent = bounds
        canvas.setExtent(rect)
        canvas.refresh()

    # --- layers

    def _layer_changed(self, layer):
        if self.dock is not None:
            self.dock.set_extrude_fields(numeric_fields(layer))

    def send_selected(self):
        layer = self.iface.activeLayer()
        if layer is None:
            self._notify('Select a layer in the Layers panel first.', warning=True)
            return
        self.send_layer(layer)

    def send_layer(self, layer):
        d = self.dock
        try:
            message, sent, notes = layer_message(
                layer,
                self.server,
                max_features=d.max_features.value(),
                simplify_m=d.simplify.value(),
                extrude=d.extrude.currentData(),
                color_by=d.color_by.currentData(),
            )
        except Skipped as reason:
            self._notify(f'{layer.name()} not sent: {reason}', warning=True)
            return None
        except Exception as error:  # a provider's failure is reported, not raised into QGIS
            self._notify(f'{layer.name()} not sent: {error}', warning=True)
            return None
        message.setdefault('name', layer.name())
        self.server.send(message)
        self.sent[sent.id] = sent
        self._refresh_list()
        text = f'{layer.name()} sent'
        if notes:
            text += ' (' + '; '.join(notes) + ')'
        self._notify(text, warning=bool(notes))
        return message

    def remove_selected(self):
        item = self.dock.sent.currentItem()
        ids = [item.data(_user_role())] if item else []
        if not ids:
            layer = self.iface.activeLayer()
            ids = [s.id for s in self.sent.values() if layer is not None and s.qgis_id == layer.id()]
        for lid in ids:
            self.sent.pop(lid, None)
            self.server.send({'type': 'remove', 'id': lid})
        self._refresh_list()

    def clear(self):
        self.sent.clear()
        self.server.send({'type': 'clear'})
        self._refresh_list()
        self.dock.show_attributes('', [])

    def _layer_removed(self, qgis_id):
        for lid in [s.id for s in self.sent.values() if s.qgis_id == qgis_id]:
            self.sent.pop(lid, None)
            self.server.send({'type': 'remove', 'id': lid})
        self._refresh_list()

    def _refresh_list(self):
        from qgis.PyQt.QtWidgets import QListWidgetItem

        self.dock.sent.clear()
        for sent in self.sent.values():
            item = QListWidgetItem(f'{sent.name} ({sent.kind})')
            item.setData(_user_role(), sent.id)
            self.dock.sent.addItem(item)

    # --- the viewer's messages (main thread)

    def _on_viewer(self, message):
        if self.dock is None:
            return
        kind = message.get('type')
        if kind == 'ready':
            self._status()
            self.send_camera(force=True)
        elif kind == 'moveend':
            self._viewer_moved(message)
        elif kind == 'click':
            self._clicked(message)
        elif kind == 'error':
            self._notify(f'Viewer: {message.get("message")}', warning=True)
        self._status()

    def _clicked(self, message):
        hit = message.get('hit')
        sent = self.sent.get(hit.get('id')) if isinstance(hit, dict) else None
        if sent is None or sent.kind != 'geojson':
            self.dock.show_attributes('', [])
            return
        layer = QgsProject.instance().mapLayer(sent.qgis_id)
        props = hit.get('properties') or {}
        fid = props.get('qgis_fid')
        index = hit.get('index')
        if fid is None and isinstance(index, int) and 0 <= index < len(sent.fids):
            fid = sent.fids[index]
        if isinstance(index, int):
            self.server.send({'type': 'select', 'id': sent.id, 'index': index})
        if not isinstance(layer, QgsVectorLayer) or fid is None:
            rows = [(k, v) for k, v in props.items() if not k.startswith('qgis_')]
            self.dock.show_attributes(sent.name, rows)
            return
        layer.selectByIds([int(fid)])
        feature = next(layer.getFeatures(QgsFeatureRequest().setFilterFid(int(fid))), None)
        if feature is None:
            return
        rows = [(field.name(), feature[field.name()]) for field in layer.fields()]
        self.dock.show_attributes(f'{layer.name()} #{fid}', rows)

    # --- status

    def _status(self):
        if self.dock is None or self.server is None:
            return
        clients = self.server.bridge.clients()
        where = 'in the panel' if self.dock.web is not None else 'in the browser'
        state = f'{clients} viewer(s) connected' if clients else f'viewer not connected (opens {where})'
        if not os.path.isfile(os.path.join(self.server.root, 'index.html')):
            state = 'the viewer is not bundled: package the plugin with bun scripts/qgis.ts'
        self.dock.status.setText(f'<a href="{self.server.viewer_url}">{self.server.viewer_url}</a><br>{state}')
        self.dock.status.setOpenExternalLinks(True)

    def _notify(self, text, warning=False):
        if self.dock is not None:
            self.dock.set_message(text)
        bar = self.iface.messageBar()
        levels = getattr(Qgis, 'MessageLevel', Qgis)
        level = getattr(levels, 'Warning' if warning else 'Info')
        try:
            bar.pushMessage('isard', text, level=level, duration=6)
        except TypeError:
            bar.pushMessage('isard', text, level, 6)


def _bool(value):
    return value if isinstance(value, bool) else str(value).lower() in ('true', '1', 'yes')


def _user_role():
    from qgis.PyQt.QtCore import Qt

    return getattr(Qt, 'ItemDataRole', Qt).UserRole


def _right_area():
    from qgis.PyQt.QtCore import Qt

    return getattr(Qt, 'DockWidgetArea', Qt).RightDockWidgetArea
