"""Read and write the protobuf of Mapbox Vector Tiles for the planner bakes."""

EXTENT = 4096
POINT, LINESTRING = 1, 2


def read_varint(data, i):
    value = shift = 0
    while True:
        value |= (data[i] & 0x7F) << shift
        i += 1
        if data[i - 1] < 0x80: return value, i
        shift += 7


def fields(data):
    i = 0
    while i < len(data):
        key, i = read_varint(data, i)
        if key & 7 == 0: value, i = read_varint(data, i)
        elif key & 7 == 2:
            length, i = read_varint(data, i)
            value, i = data[i:i + length], i + length
        elif key & 7 in (1, 5): value, i = None, i + (8 if key & 7 == 1 else 4)
        else: raise ValueError("Unsupported protobuf wire type")
        yield key >> 3, value


def packed(data):
    i, values = 0, []
    while i < len(data):
        value, i = read_varint(data, i)
        values.append(value)
    return values


def varint(value):
    out = bytearray()
    while value > 0x7F:
        out.append(value & 0x7F | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)


def zigzag(value):
    return value << 1 if value >= 0 else (-value << 1) - 1


def encode(number, value):
    """One protobuf field: an integer as a varint, bytes as a length-delimited payload."""
    if isinstance(value, int): return varint(number << 3) + varint(value)
    return varint(number << 3 | 2) + varint(len(value)) + value


def line(parts):
    """The geometry commands of a (multi) line string in tile coordinates."""
    commands, x, y = [], 0, 0
    for part in parts:
        for i, (px, py) in enumerate(part):
            if i < 2: commands.append(9 if i == 0 else 2 | (len(part) - 1) << 3)
            commands += [zigzag(px - x), zigzag(py - y)]
            x, y = px, py
    return commands


class Layer:
    """One tile layer. Keys and values are stored once for all of its features."""

    def __init__(self, name):
        self.name, self.keys, self.values, self.features = name, {}, {}, bytearray()

    def add(self, identity, properties, kind, geometry):
        tags = []
        for key, value in properties.items():
            tags += [self.keys.setdefault(key, len(self.keys)), self.values.setdefault((type(value), value), len(self.values))]
        self.features += encode(2, encode(1, identity) + encode(2, b"".join(map(varint, tags)))
                                + encode(3, kind) + encode(4, b"".join(map(varint, geometry))))

    def encode(self):
        # Strings are field 1 and non-negative integers field 5 of a tile value.
        values = (encode(4, encode(1, v.encode()) if t is str else encode(5, v)) for t, v in self.values)
        return encode(3, encode(15, 2) + encode(1, self.name.encode()) + bytes(self.features)
                      + b"".join(encode(3, key.encode()) for key in self.keys) + b"".join(values) + encode(5, EXTENT))
