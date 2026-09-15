#!/usr/bin/env python3
"""Count 16-byte node alignment cost on the pinned map; do not rewrite it."""
import argparse
from collections import Counter
import hashlib
import json
import struct

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('map')
args = parser.parse_args()
with open(args.map, 'rb') as stream:
    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    assert digest == 'feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72'
    stream.seek(0)
    header = stream.read(57)
    assert header[:5] == b'OBCM\x10'
    unit = 1 << header[40]
    word = lambda buf, at: struct.unpack_from('<I', buf, at)[0]
    stream.seek(word(header, 36) * unit)
    directory = stream.read(40)
    assert struct.unpack_from('<H', directory, 20)[0] == 512
    chunks = word(directory, 8)
    data = (word(directory, 0) * unit + word(directory, 4) * 4 + unit - 1) & ~(unit - 1)
    stream.seek(data)
    degrees = Counter()
    raw_bytes = aligned_bytes = overflow_chunks = largest_aligned_chunk = 0
    for _ in range(chunks):
        chunk = stream.read(512)
        assert len(chunk) == 512
        at = aligned_chunk = 0
        while at + 13 <= 512 and chunk[at + 12] != 255:
            degree = chunk[at + 12]
            size = 13 + 17 * degree
            assert degree <= 24 and at + size <= 512
            degrees[degree] += 1
            raw_bytes += size
            padded = (size + 15) & ~15
            aligned_bytes += padded
            aligned_chunk += padded
            at += size
        assert all(x == 255 for x in chunk[at:])
        overflow_chunks += aligned_chunk > 512
        largest_aligned_chunk = max(largest_aligned_chunk, aligned_chunk)
assert sum(degrees.values()) == 1010635
print(json.dumps({'map_sha256': digest, 'node_count': sum(degrees.values()), 'node_chunks': chunks, 'degree_histogram': dict(sorted(degrees.items())), 'raw_record_bytes': raw_bytes, 'aligned_record_bytes': aligned_bytes, 'extra_alignment_bytes': aligned_bytes - raw_bytes, 'existing_node_chunk_bytes': chunks * 512, 'chunks_that_overflow_if_membership_is_unchanged': overflow_chunks, 'largest_aligned_chunk_with_unchanged_membership': largest_aligned_chunk, 'scope': 'Payload arithmetic and current chunk membership only. Does not rebuild spatial leaves, bin packing, or node index; not an output size or runtime measurement.'}, indent=2))
