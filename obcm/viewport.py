import math

class Viewport:
    def __init__(self, width, height, center_lat_micro):
        """
        center_lat_micro: Latitude in microdegrees used for aspect correction.
        """
        self.width = width
        self.height = height
        self.camera_lon = 0 # microdegrees
        self.camera_lat = 0 # microdegrees
        self._zoom = 1.0 # pixels per microdegree
        # Aspect correction: how much to squash/stretch the X axis
        self.aspect = math.cos(math.radians(center_lat_micro / 1e6))
        
        # Pre-calculate screen center
        self.half_width = width / 2
        self.half_height = height / 2
        self._update_zoom_factors()

    @property
    def zoom(self):
        return self._zoom

    @zoom.setter
    def zoom(self, value):
        self._zoom = value
        self._update_zoom_factors()

    def _update_zoom_factors(self):
        self.zoom_aspect = self._zoom * self.aspect
        self.inv_zoom_aspect = 1.0 / self.zoom_aspect
        self.inv_zoom = 1.0 / self._zoom

    def to_screen(self, lon, lat):
        """
        Converts (lon, lat) microdegrees to (x, y) screen pixels.
        """
        x = (lon - self.camera_lon) * self.zoom_aspect + self.half_width
        # Y is inverted in Pygame (0 is top)
        y = (self.camera_lat - lat) * self._zoom + self.half_height
        return int(x), int(y)

    def to_map(self, x, y):
        """
        Converts (x, y) screen pixels to (lon, lat) microdegrees.
        """
        lon = (x - self.half_width) * self.inv_zoom_aspect + self.camera_lon
        lat = self.camera_lat - (y - self.half_height) * self.inv_zoom
        return int(lon), int(lat)
