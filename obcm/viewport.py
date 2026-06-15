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
        self.zoom = 1.0 # pixels per microdegree
        # Aspect correction: how much to squash/stretch the X axis
        self.aspect = math.cos(math.radians(center_lat_micro / 1e6))

    def to_screen(self, lon, lat):
        """
        Converts (lon, lat) microdegrees to (x, y) screen pixels.
        """
        x = (lon - self.camera_lon) * self.zoom * self.aspect + self.width / 2
        # Y is inverted in Pygame (0 is top)
        y = (self.camera_lat - lat) * self.zoom + self.height / 2
        return int(x), int(y)

    def to_map(self, x, y):
        """
        Converts (x, y) screen pixels to (lon, lat) microdegrees.
        """
        lon = (x - self.width / 2) / (self.zoom * self.aspect) + self.camera_lon
        lat = self.camera_lat - (y - self.height / 2) / self.zoom
        return int(lon), int(lat)
