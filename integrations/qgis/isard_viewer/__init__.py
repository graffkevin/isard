"""isard 3D view: the map canvas and its layers in isard's WebGL2 viewer (globe, relief, buildings, point clouds)."""


def classFactory(iface):  # noqa: N802 (QGIS's name)
    # Imported here: server.py and convert.py import nothing from QGIS and are tested without it
    from .plugin import IsardViewerPlugin

    return IsardViewerPlugin(iface)
