"""The plugin's dock: the viewer in a web view when Qt WebEngine works, else a control panel opening the browser."""

import os

from qgis.PyQt.QtCore import Qt, QUrl
from qgis.PyQt.QtWidgets import (
    QCheckBox,
    QComboBox,
    QDockWidget,
    QDoubleSpinBox,
    QFormLayout,
    QGroupBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QListWidget,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QSplitter,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .convert import BASEMAPS, POINT_COLORS, TERRAINS


def _enum(owner, scope, name):
    """``owner.scope.name`` (Qt 6 scoped enums) or ``owner.name`` (Qt 5)."""
    holder = getattr(owner, scope, None)
    if holder is not None and hasattr(holder, name):
        return getattr(holder, name)
    return getattr(owner, name, None)


def make_web_view():
    """A QWebEngineView with WebGL, or None when Qt WebEngine is missing or does not start (QGIS 4 on macOS ships
    PyQt6 without it). ``ISARD_QGIS_BROWSER=1`` forces the browser."""
    if os.environ.get('ISARD_QGIS_BROWSER'):
        return None
    try:
        from qgis.PyQt.QtWebEngineWidgets import QWebEngineView
    except Exception:
        return None
    try:
        view = QWebEngineView()
        try:
            from qgis.PyQt.QtWebEngineCore import QWebEngineSettings
        except Exception:
            from qgis.PyQt.QtWebEngineWidgets import QWebEngineSettings
        settings = view.settings()
        for name in ('WebGLEnabled', 'Accelerated2dCanvasEnabled', 'LocalStorageEnabled'):
            attribute = _enum(QWebEngineSettings, 'WebAttribute', name)
            if attribute is not None:
                settings.setAttribute(attribute, True)
        return view
    except Exception:
        return None


class IsardDock(QDockWidget):
    """The controls; the plugin connects their signals."""

    def __init__(self, parent=None):
        super().__init__('isard 3D view', parent)
        self.setObjectName('IsardViewerDock')
        self.web = make_web_view()

        controls = QWidget()
        layout = QVBoxLayout(controls)

        # Status and the browser
        self.status = QLabel('Starting…')
        self.status.setWordWrap(True)
        self.status.setTextInteractionFlags(_enum(Qt, 'TextInteractionFlag', 'TextSelectableByMouse'))
        self.open_browser = QPushButton('Open in browser')
        top = QHBoxLayout()
        top.addWidget(self.status, 1)
        top.addWidget(self.open_browser)
        layout.addLayout(top)
        if self.web is None:
            note = QLabel(
                'Qt WebEngine is not available in this QGIS: the 3D view opens in your web browser and stays '
                'synchronized with QGIS; this panel controls it.'
            )
            note.setWordWrap(True)
            layout.addWidget(note)

        # View
        view = QGroupBox('View')
        form = QFormLayout(view)
        self.basemap = QComboBox()
        for label, value in BASEMAPS:
            self.basemap.addItem(label, value)
        self.terrain = QComboBox()
        for label, value in TERRAINS:
            self.terrain.addItem(label, value)
        self.follow = QCheckBox('Follow the QGIS map')
        self.follow.setChecked(True)
        self.reverse = QCheckBox('Move the QGIS map with the 3D view')
        form.addRow('Basemap', self.basemap)
        form.addRow('Relief', self.terrain)
        form.addRow(self.follow)
        form.addRow(self.reverse)
        layout.addWidget(view)

        # Layers
        group = QGroupBox('Layers')
        form = QFormLayout(group)
        self.send = QPushButton('Send selected layer')
        self.remove = QPushButton('Remove')
        self.clear = QPushButton('Clear')
        buttons = QHBoxLayout()
        buttons.addWidget(self.send, 1)
        buttons.addWidget(self.remove)
        buttons.addWidget(self.clear)
        form.addRow(buttons)
        self.max_features = QSpinBox()
        self.max_features.setRange(100, 2_000_000)
        self.max_features.setSingleStep(10_000)
        self.max_features.setValue(50_000)
        form.addRow('Max. features', self.max_features)
        self.simplify = QDoubleSpinBox()
        self.simplify.setRange(0, 10_000)
        self.simplify.setDecimals(1)
        self.simplify.setSuffix(' m')
        self.simplify.setSpecialValueText('No simplification')
        form.addRow('Simplify', self.simplify)
        self.extrude = QComboBox()
        form.addRow('Extrude by', self.extrude)
        self.color_by = QComboBox()
        for label, value in POINT_COLORS:
            self.color_by.addItem(label, value)
        form.addRow('Point colors', self.color_by)
        self.sent = QListWidget()
        self.sent.setMaximumHeight(110)
        form.addRow(self.sent)
        layout.addWidget(group)

        # Clicked feature
        group = QGroupBox('Clicked feature')
        box = QVBoxLayout(group)
        self.attributes = QTableWidget(0, 2)
        self.attributes.setHorizontalHeaderLabels(['Field', 'Value'])
        self.attributes.verticalHeader().setVisible(False)
        self.attributes.horizontalHeader().setSectionResizeMode(1, _enum(QHeaderView, 'ResizeMode', 'Stretch'))
        box.addWidget(self.attributes)
        layout.addWidget(group, 1)

        self.message = QLabel('')
        self.message.setWordWrap(True)
        layout.addWidget(self.message)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setWidget(controls)
        if self.web is not None:
            split = QSplitter(_enum(Qt, 'Orientation', 'Vertical'))
            split.addWidget(self.web)
            split.addWidget(scroll)
            split.setStretchFactor(0, 3)
            split.setStretchFactor(1, 2)
            self.setWidget(split)
        else:
            self.setWidget(scroll)

    def load(self, url):
        if self.web is not None:
            self.web.setUrl(QUrl(url))

    def set_extrude_fields(self, fields):
        current = self.extrude.currentData()
        self.extrude.clear()
        self.extrude.addItem('(none)', None)
        for name in fields:
            self.extrude.addItem(name, name)
        index = self.extrude.findData(current)
        self.extrude.setCurrentIndex(max(0, index))

    def show_attributes(self, title, rows):
        self.attributes.setRowCount(0)
        for name, value in rows:
            row = self.attributes.rowCount()
            self.attributes.insertRow(row)
            self.attributes.setItem(row, 0, QTableWidgetItem(str(name)))
            self.attributes.setItem(row, 1, QTableWidgetItem('' if value is None else str(value)))
        self.attributes.parentWidget().setTitle(f'Clicked feature: {title}' if title else 'Clicked feature')

    def set_message(self, text):
        self.message.setText(text)
