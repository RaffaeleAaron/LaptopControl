"""Tiny trusted test PNG encoder; no third-party image parser dependency."""
import struct
import zlib


def write_png(path, width=16, height=16, rgba=(255, 0, 0, 255)):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    rows = (b'\0' + bytes(rgba) * width) * height
    header = struct.pack('>IIBBBBB', width, height, 8, 6, 0, 0, 0)
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', header) + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))
