#!/usr/bin/env python3
"""One cached asset per process, with time/memory/output limits and no SVG base URI."""
import json
import math
import os
from pathlib import Path
import resource
import sys

MAX_FILE_BYTES = 8 * 1024 * 1024
MAX_SOURCE_PIXELS = 16 * 1024 * 1024
MAX_RENDER_PIXELS = 4 * 1024 * 1024
MAX_DIMENSION = 4096
MAX_TEXT = 8192


def dimensions(width, height, limit=MAX_RENDER_PIXELS):
    if not all(math.isfinite(v) and v > 0 for v in (width, height)):
        raise ValueError('Invalid asset dimensions')
    if width > MAX_DIMENSION or height > MAX_DIMENSION or width * height > limit:
        raise ValueError('Asset exceeds the safe render size')
    return max(1, round(width)), max(1, round(height))


def render(layer, output_width, target):
    kind = layer.get('kind', 'Image')
    if kind == 'Text':
        import gi
        gi.require_version('Pango', '1.0')
        gi.require_version('PangoCairo', '1.0')
        from gi.repository import Pango, PangoCairo
        import cairo
        text = layer.get('text', '')
        if not isinstance(text, str) or len(text) > MAX_TEXT:
            raise ValueError('Text overlay is too long')
        width_fraction = float(layer.get('width', .5))
        size = float(layer.get('font_size', 36))
        if not math.isfinite(width_fraction) or not 0.02 <= width_fraction <= 1 or not math.isfinite(size) or not 8 <= size <= 200:
            raise ValueError('Invalid text size')
        width = max(16, round(output_width * width_fraction))
        scratch = cairo.ImageSurface(cairo.FORMAT_ARGB32, 1, 1)
        layout = PangoCairo.create_layout(cairo.Context(scratch))
        font = Pango.FontDescription()
        font.set_family(str(layer.get('font', 'Sans'))[:200])
        font.set_absolute_size(size * Pango.SCALE)
        font.set_weight(Pango.Weight.BOLD if layer.get('bold') else Pango.Weight.NORMAL)
        layout.set_font_description(font)
        layout.set_text(text, -1)
        layout.set_width((width - 8) * Pango.SCALE)
        layout.set_wrap(Pango.WrapMode.WORD_CHAR)
        _, height = layout.get_pixel_size()
        width, height = dimensions(width, max(8, height + 8))
        surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
        context = cairo.Context(surface)
        color = layer.get('color', [255] * 4)
        if not isinstance(color, list) or len(color) != 4 or any(type(v) is not int or not 0 <= v <= 255 for v in color):
            raise ValueError('Invalid text color')
        context.set_source_rgba(*(v / 255 for v in color))
        context.move_to(4, 4)
        PangoCairo.show_layout(context, layout)
        surface.write_to_png(str(target))
    else:
        path = Path(layer['path'])
        if path.suffix.lower() not in ('.svg', '.png', '.jpg', '.jpeg'):
            raise ValueError('Use SVG, PNG or JPEG images')
        if not path.is_file() or path.stat().st_size > MAX_FILE_BYTES:
            raise ValueError('Asset must be a regular image file no larger than 8 MiB')
        if path.suffix.lower() == '.svg':
            import gi
            gi.require_version('Rsvg', '2.0')
            from gi.repository import Rsvg
            import cairo
            import xml.etree.ElementTree as ET
            data = path.read_bytes()
            text = data.decode('utf-8-sig')
            if '<!DOCTYPE' in text or '<!ENTITY' in text or '<?xml-stylesheet' in text:
                raise ValueError('SVG entities, DTDs and external stylesheets are not supported')
            tree = ET.fromstring(text)
            for element in tree.iter():
                if element.tag.startswith('{http://www.w3.org/2001/XInclude}'):
                    raise ValueError('External SVG includes are not supported')
                for name, value in element.attrib.items():
                    if name.rsplit('}', 1)[-1] == 'href' and not value.startswith(('#', 'data:image/png;', 'data:image/jpeg;')):
                        raise ValueError('SVG images must embed resources; external files and URLs are blocked')
            # No base URI: librsvg cannot resolve local files or network references.
            handle = Rsvg.Handle.new_from_data(data)
            valid, iw, ih = handle.get_intrinsic_size_in_pixels()
            if not valid:
                info = handle.get_intrinsic_dimensions()
                box = info[-1]
                iw, ih = (box.width, box.height) if info[-2] else (512., 512.)
            dimensions(iw, ih, MAX_SOURCE_PIXELS)
            width = max(1, min(1280, round(iw)))
            width, height = dimensions(width, max(1, round(width * ih / iw)))
            surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, width, height)
            viewport = Rsvg.Rectangle()
            viewport.x = viewport.y = 0
            viewport.width, viewport.height = width, height
            handle.render_document(cairo.Context(surface), viewport)
            surface.write_to_png(str(target))
        else:
            raise ValueError('Raster images must use the bounded Rust decoder')
    return {'width': width, 'height': height}


def main():
    # Applied before loading native parsers. This is resource isolation, not a full sandbox.
    resource.setrlimit(resource.RLIMIT_AS, (768 * 1024 * 1024,) * 2)
    resource.setrlimit(resource.RLIMIT_CPU, (3, 4))
    resource.setrlimit(resource.RLIMIT_FSIZE, (16 * 1024 * 1024,) * 2)
    resource.setrlimit(resource.RLIMIT_NOFILE, (64, 64))
    import ctypes
    if ctypes.CDLL(None, use_errno=True).prctl(38, 1, 0, 0, 0) != 0:
        raise RuntimeError('Could not disable privilege gain in asset worker')
    data = sys.stdin.buffer.read(4 * 1024 * 1024 + 1)
    if len(data) > 4 * 1024 * 1024:
        raise ValueError('Asset request is too large')
    if os.environ.get('LAPTOP_CONTROL_ASSET_SANDBOX') == '1':
        config = Path('/tmp/fontconfig.xml')
        config.write_text('<fontconfig><include>/etc/fonts/fonts.conf</include><dir>/user-fonts-0</dir><dir>/user-fonts-1</dir><cachedir>/tmp/font-cache</cachedir></fontconfig>')
        os.environ['FONTCONFIG_FILE'] = str(config)
    request = json.loads(data)
    result = render(request['layer'], request['output_width'], Path(request['target']))
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1)
