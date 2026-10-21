"""Regenerate the AVIF test fixtures in this directory (agent-only helper).

Requires: ffmpeg with libaom-av1 on PATH, and Pillow >= 11.3 (built-in AVIF
support) in the running interpreter. Run with an isolated interpreter:

    python -I testdata/avif/make_fixtures.py

Every fixture except the 128x128 grid is a 64x64 "quadrant" image so tests can check colours at the
centre of each quadrant: top-left red, top-right green, bottom-left blue,
bottom-right white. See README.md for what each file covers.
"""

import pathlib
import struct
import subprocess
import tempfile

from PIL import Image

HERE = pathlib.Path(__file__).resolve().parent
SIZE = 64
HALF = SIZE // 2
COLORS = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)]


def quadrant(size=SIZE, mode="RGB", alpha_left=255):
    img = Image.new(mode, (size, size))
    half = size // 2
    for index, color in enumerate(COLORS):
        x0, y0 = (index % 2) * half, (index // 2) * half
        fill = color if mode == "RGB" else color + ((alpha_left if x0 == 0 else 255),)
        img.paste(fill, (x0, y0, x0 + half, y0 + half))
    return img


def ffmpeg(src_png, out, pix_fmt, extra=()):
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(src_png),
         "-vf", "scale=out_color_matrix=bt709:out_range=tv",
         "-c:v", "libaom-av1", "-still-picture", "1", "-crf", "10", "-pix_fmt", pix_fmt,
         "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709",
         "-color_range", "tv", *extra, str(out)],
        check=True,
    )


def boxes(data, start, end):
    while start < end:
        size, kind = struct.unpack(">I4s", data[start:start + 8])
        yield kind.decode(), start, start + 8, start + size
        start += size


def find(data, path, start=0, end=None):
    end = len(data) if end is None else end
    for kind, _, body, stop in boxes(data, start, end):
        if kind == path[0]:
            if len(path) == 1:
                return body - 8, stop
            return find(data, path[1:], body + (4 if kind == "meta" else 0), stop)
    raise KeyError(path)


def single_item(path):
    """Return (av1C box bytes, coded item payload) of a one-item ffmpeg AVIF."""
    data = path.read_bytes()
    start, stop = find(data, ["meta", "iprp", "ipco", "av1C"])
    av1c = data[start:stop]
    start, _ = find(data, ["meta", "iloc"])
    pos = start + 8
    version = data[pos]
    pos += 4
    assert data[pos] == 0x44 and data[pos + 1] >> 4 == 0, "unexpected iloc field sizes"
    pos += 2
    assert struct.unpack(">H", data[pos:pos + 2])[0] == 1
    pos += 2 + (2 if version == 1 else 0) + 4 + 2
    offset, length = struct.unpack(">II", data[pos:pos + 8])
    return av1c, data[offset:offset + length]


def box(kind, payload):
    return struct.pack(">I4s", 8 + len(payload), kind.encode()) + payload


def full_box(kind, version, flags, payload):
    return box(kind, struct.pack(">I", (version << 24) | flags) + payload)


def write_grid(out, tiles, tile_size, grid_size):
    """Write a 2x2 'grid' derived image item built from four single-item AVIFs."""
    av1c = tiles[0][0]
    descriptor = struct.pack(">BBBBHH", 0, 0, 1, 1, grid_size, grid_size)
    payloads = [tile[1] for tile in tiles] + [descriptor]
    ftyp = box("ftyp", b"avif" + struct.pack(">I", 0) + b"avifmif1miafMA1B")

    def meta(offsets):
        hdlr = full_box("hdlr", 0, 0, struct.pack(">I", 0) + b"pict" + b"\0" * 13)
        pitm = full_box("pitm", 0, 0, struct.pack(">H", 5))
        iloc = struct.pack(">BBH", 0x44, 0x00, 5)
        for item_id, (payload, offset) in enumerate(zip(payloads, offsets), start=1):
            iloc += struct.pack(">HHHII", item_id, 0, 1, offset, len(payload))
        infe = b"".join(
            full_box("infe", 2, 0, struct.pack(">HH", item_id, 0)
                     + (b"grid" if item_id == 5 else b"av01") + b"\0")
            for item_id in range(1, 6)
        )
        iinf = full_box("iinf", 0, 0, struct.pack(">H", 5) + infe)
        iref = full_box("iref", 0, 0, box("dimg", struct.pack(">HHHHHH", 5, 4, 1, 2, 3, 4)))
        ipco = box("ipco", av1c
                   + full_box("ispe", 0, 0, struct.pack(">II", tile_size, tile_size))
                   + full_box("ispe", 0, 0, struct.pack(">II", grid_size, grid_size))
                   + full_box("pixi", 0, 0, bytes([3, 8, 8, 8])))
        entries = b"".join(struct.pack(">HB", i, 3) + bytes([0x81, 0x02, 0x04]) for i in range(1, 5))
        entries += struct.pack(">HB", 5, 2) + bytes([0x03, 0x04])
        ipma = full_box("ipma", 0, 0, struct.pack(">I", 5) + entries)
        return full_box("meta", 0, 0, hdlr + pitm + full_box("iloc", 0, 0, iloc) + iinf + iref
                        + box("iprp", ipco + ipma))

    first = meta([0] * 5)
    base = len(ftyp) + len(first) + 8
    offsets, cursor = [], base
    for payload in payloads:
        offsets.append(cursor)
        cursor += len(payload)
    second = meta(offsets)
    assert len(second) == len(first)
    out.write_bytes(ftyp + second + box("mdat", b"".join(payloads)))


def main():
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        src = tmp / "quadrant.png"
        quadrant().save(src)
        ffmpeg(src, HERE / "quadrant_8bit_420.avif", "yuv420p")
        ffmpeg(src, HERE / "quadrant_10bit_420.avif", "yuv420p10le")
        ffmpeg(src, HERE / "quadrant_12bit_444.avif", "yuv444p12le")
        ffmpeg(src, HERE / "quadrant_8bit_422.avif", "yuv422p")

        # Libavif via Pillow: BT.601 full range written explicitly by the encoder.
        quadrant().save(HERE / "quadrant_full_range_444.avif", subsampling="4:4:4", quality=95)
        # Left half fully transparent, right half opaque.
        quadrant(mode="RGBA", alpha_left=0).save(
            HERE / "quadrant_alpha_444.avif", subsampling="4:4:4", quality=95)
        # Pillow turns EXIF orientation into irot / imir and drops the EXIF tag.
        for orientation, name in ((6, "irot90cw"), (2, "imir_horizontal")):
            exif = Image.Exif()
            exif[0x0112] = orientation
            quadrant().save(HERE / f"quadrant_{name}.avif", subsampling="4:4:4",
                            quality=95, exif=exif.tobytes())
        # Animated: first frame is the quadrant, second frame solid black.
        quadrant().save(HERE / "animated_first_quadrant.avif", save_all=True,
                        append_images=[Image.new("RGB", (SIZE, SIZE))], duration=100,
                        subsampling="4:4:4", quality=95)
        Image.new("L", (SIZE, SIZE), 128).save(HERE / "gray_128.avif", quality=95)

        tiles = []
        for index, color in enumerate(COLORS):
            tile_png = tmp / f"tile{index}.png"
            # libavif rejects grid tiles smaller than 64x64 (MIAF), so the grid is 128x128.
            Image.new("RGB", (SIZE, SIZE), color).save(tile_png)
            tile_avif = tmp / f"tile{index}.avif"
            ffmpeg(tile_png, tile_avif, "yuv444p")
            tiles.append(single_item(tile_avif))
        write_grid(HERE / "quadrant_grid_2x2.avif", tiles, SIZE, SIZE * 2)

    data = (HERE / "quadrant_grid_2x2.avif").read_bytes()
    (HERE / "truncated.avif").write_bytes(data[: len(data) // 2])
    (HERE / "not_avif.avif").write_bytes(b"\x89PNG\r\n\x1a\n" + b"\0" * 64)
    print("fixtures written to", HERE)


if __name__ == "__main__":
    main()
