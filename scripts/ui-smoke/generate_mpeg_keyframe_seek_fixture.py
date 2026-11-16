#!/usr/bin/env python3
"""Generate numbered MPEG seek fixtures and independent full-decode oracles.

Only system FFmpeg/lavfi supplies media. No product code or application is used.
The Windows avformat-61 probe verifies the missing-PTS PS case against
the bundled public C ABI; its observations are separate from the decode oracle.
"""

from __future__ import annotations

import argparse
import ctypes as C
import hashlib
import json
import os
import re
import shutil
import subprocess
from fractions import Fraction
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUTPUT = ROOT / "testdata" / "mpeg-keyframe-seek"
WIDTH, HEIGHT, BITS = 96, 64, 16
ENCODING = {"bits": BITS, "bit_order": "least_significant_first",
            "sample_x": [4 + 6 * bit for bit in range(BITS)],
            "sample_y": 16, "threshold": 128, "low_y": 32, "high_y": 224}
NOPTS = -(2**63)


def run(command: list[str]) -> bytes:
    result = subprocess.run(command, check=False, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(f"Command failed: {command!r}\n"
                           + result.stderr.decode("utf-8", errors="replace"))
    return result.stdout


def source(fps: int, seconds: int) -> str:
    # N is the original display-order frame number, retained by byte-cut clips.
    return (f"nullsrc=s={WIDTH}x{HEIGHT}:r={fps}:d={seconds},"
            "geq=lum='if(lt(Y,32),32+192*mod(floor(N/pow(2,floor(X/6))),2),"
            "if(lt(X,48),32,224))':cb=128:cr=128")


def frame_id(gray: bytes, stride: int = WIDTH) -> int:
    return sum((gray[ENCODING["sample_y"] * stride + x] >= 128) << bit
               for bit, x in enumerate(ENCODING["sample_x"]))


def nullable_int(value: str | int | None) -> int | None:
    return None if value is None or value == "N/A" else int(value)


def time_ticks(seconds: str | None, tb: Fraction) -> int | None:
    return None if seconds is None else round(Fraction(seconds) / tb)


class Generator:
    def __init__(self, output: Path, ffmpeg: str, ffprobe: str):
        self.output, self.ffmpeg, self.ffprobe = output, ffmpeg, ffprobe
        self.versions = {"ffmpeg": run([ffmpeg, "-version"]).decode().splitlines()[0],
                         "ffprobe": run([ffprobe, "-version"]).decode().splitlines()[0]}

    def encode(self, name: str, codec: str, *, fps: int = 30, seconds: int = 12,
               gop: int = 15, offset: int = 0, extra: tuple[str, ...] = ()) -> None:
        options = {"libx264": ["-preset", "veryfast", "-crf", "18",
                               "-x264-params", f"keyint={gop}:min-keyint={gop}:scenecut=0:bframes=2:b-adapt=0:open-gop=0"],
                   "libx265": ["-preset", "ultrafast", "-crf", "18",
                               "-x265-params", f"keyint={gop}:min-keyint={gop}:scenecut=0:bframes=3:b-adapt=0:open-gop=1:pools=none:frame-threads=1:wpp=0:log-level=error"],
                   "mpeg2video": ["-q:v", "2", "-g", str(gop), "-bf", "2",
                                  "-sc_threshold", "1000000000"]}[codec]
        is_ps = name.endswith(".mpg")
        command = [self.ffmpeg, "-hide_banner", "-loglevel", "error", "-y",
                   "-f", "lavfi", "-i", source(fps, seconds),
                   "-f", "lavfi", "-i", f"sine=frequency=1000:sample_rate=48000:duration={seconds}",
                   "-map", "0:v", "-map", "1:a", "-c:v", codec,
                   "-threads:v", "1", "-pix_fmt", "yuv420p", *options,
                   "-c:a", "mp2" if codec == "mpeg2video" else "aac",
                   "-b:a", "64k" if codec == "mpeg2video" else "32k",
                   "-output_ts_offset", str(offset), *extra,
                   "-f", "mpeg" if is_ps else "mp4" if name.endswith(".mp4") else "mpegts",
                   str(self.output / name)]
        run(command)

    def probe(self, path: Path, *options: str) -> dict:
        return json.loads(run([self.ffprobe, "-v", "error", *options, "-of", "json", str(path)]))

    def oracle(self, name: str) -> dict:
        path = self.output / name
        metadata = self.probe(path, "-show_format", "-show_streams")
        stream = next(s for s in metadata["streams"] if s["codec_type"] == "video")
        tb = Fraction(stream["time_base"])
        full_frames = self.probe(path, "-select_streams", "v:0", "-show_frames")["frames"]
        full_packets = self.probe(path, "-select_streams", "v:0", "-show_packets",
                                  "-show_data_hash", "sha256")["packets"]
        # Passthrough is essential: default ffmpeg CFR sync duplicates clipped heads.
        raw = run([self.ffmpeg, "-hide_banner", "-loglevel", "error", "-i", str(path),
                   "-map", "0:v:0", "-fps_mode", "passthrough", "-pix_fmt", "gray",
                   "-f", "rawvideo", "pipe:1"])
        frame_size = WIDTH * HEIGHT
        if len(raw) != len(full_frames) * frame_size:
            raise ValueError(f"{name}: ffprobe/ffmpeg decoded-frame count differs")
        frames = []
        min_margin = 255
        for index, decoded in enumerate(full_frames):
            gray = raw[index * frame_size:(index + 1) * frame_size]
            number = frame_id(gray)
            min_margin = min(min_margin, *(abs(gray[16 * WIDTH + x] - 128)
                                          for x in ENCODING["sample_x"]))
            frame = {"display_index": index, "frame_id": number,
                     "best_effort_timestamp": nullable_int(decoded.get("best_effort_timestamp")),
                     "pts": nullable_int(decoded.get("pts")),
                     "pkt_dts": nullable_int(decoded.get("pkt_dts")),
                     "key_frame": bool(decoded.get("key_frame")),
                     "pict_type": decoded["pict_type"]}
            if frame["best_effort_timestamp"] is None:
                raise ValueError(f"{name}: decoded frame {index} has no timestamp")
            frames.append(frame)
        if any(b["frame_id"] != a["frame_id"] + 1 for a, b in zip(frames, frames[1:])):
            raise ValueError(f"{name}: frame numbers are not consecutive")
        if any(b["best_effort_timestamp"] <= a["best_effort_timestamp"] for a, b in zip(frames, frames[1:])):
            raise ValueError(f"{name}: decoded display timestamps are not increasing")
        if min_margin < 64:
            raise ValueError(f"{name}: compressed frame-number contrast too low")
        packets = [{"packet_index": index, "key": "K" in packet["flags"],
                    "data_sha256": packet["data_hash"].split(":", 1)[1],
                    **{field: nullable_int(packet.get(field)) for field in ("pts", "dts", "pos", "size", "duration")}}
                   for index, packet in enumerate(full_packets)]
        fmt = metadata["format"]
        result = {"schema_version": 1, "file": name, "width": WIDTH, "height": HEIGHT,
                  "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                  "generator_tools": self.versions, "frame_id_encoding": ENCODING,
                  "format": {"name": fmt["format_name"],
                             "start_time_us": time_ticks(fmt.get("start_time"), Fraction(1, 1000000)),
                             "duration_us": time_ticks(fmt.get("duration"), Fraction(1, 1000000)),
                             "container_start_ticks": time_ticks(fmt.get("start_time"), tb),
                             "duration_ticks": time_ticks(fmt.get("duration"), tb)},
                  "video_stream": {"index": stream["index"], "codec_name": stream["codec_name"],
                                   "time_base": stream["time_base"], "start_pts": nullable_int(stream.get("start_pts")),
                                   "duration_ts": nullable_int(stream.get("duration_ts")),
                                   "avg_frame_rate": stream["avg_frame_rate"]},
                  "audio_streams": [{"index": audio["index"], "codec_name": audio["codec_name"],
                                     "time_base": audio["time_base"], "start_pts": nullable_int(audio.get("start_pts")),
                                     "duration_ts": nullable_int(audio.get("duration_ts")),
                                     "sample_rate": int(audio["sample_rate"])}
                                    for audio in metadata["streams"] if audio["codec_type"] == "audio"],
                  "frames": frames, "packets": packets,
                  "validation": {"full_decode_frame_count": len(frames),
                                 "all_video_packet_count": len(packets),
                                 "b_frame_count": sum(f["pict_type"] == "B" for f in frames),
                                 "missing_pts_key_count": sum(p["key"] and p["pts"] is None for p in packets),
                                 "minimum_id_threshold_margin": min_margin},
                  "special_targets": {}}
        if not result["validation"]["b_frame_count"]:
            raise ValueError(f"{name}: no B frames")
        return result

    def write_oracle(self, oracle: dict) -> None:
        (self.output / (oracle["file"] + ".json")).write_text(
            json.dumps(oracle, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    def cut_head(self, source_name: str, name: str) -> dict:
        original = self.output / source_name
        data = original.read_bytes()
        packets = self.probe(original, "-select_streams", "v:0", "-show_packets")["packets"]
        positions = [int(p["pos"]) for p in packets[3:35]
                     if int(p.get("pos", -1)) > 0 and "K" not in p["flags"]]
        boundaries = ([p // 188 * 188 for p in positions] if name.endswith(".ts") else
                      [data.rfind(b"\x00\x00\x01\xba", 0, p + 1) for p in positions])
        for position in dict.fromkeys(boundaries):
            if position <= 0:
                continue
            (self.output / name).write_bytes(data[position:])
            candidate = self.oracle(name)
            if not candidate["packets"][0]["key"] and candidate["frames"][0]["frame_id"] > 0:
                candidate["validation"]["byte_cut"] = {"source": source_name, "offset": position,
                                                       "first_video_packet_is_nonkey": True}
                return candidate
        raise ValueError(f"{name}: could not make an incomplete-head GOP")


# Public FFmpeg 7.1 Windows x64 prefixes from vendor/ffmpeg/include. Fail closed
# for another ABI; these are generation-time probes, not product bindings.
class Rational(C.Structure):
    _fields_ = [("num", C.c_int), ("den", C.c_int)]


class Packet(C.Structure):
    _fields_ = [("buf", C.c_void_p), ("pts", C.c_int64), ("dts", C.c_int64),
                ("data", C.c_void_p), ("size", C.c_int), ("stream_index", C.c_int),
                ("flags", C.c_int), ("side_data", C.c_void_p), ("side_data_elems", C.c_int),
                ("duration", C.c_int64), ("pos", C.c_int64)]


class Stream(C.Structure):
    _fields_ = [("av_class", C.c_void_p), ("index", C.c_int), ("id", C.c_int),
                ("codecpar", C.c_void_p), ("priv_data", C.c_void_p),
                ("time_base", Rational), ("start_time", C.c_int64)]


class Format(C.Structure):
    _fields_ = [("av_class", C.c_void_p), ("iformat", C.c_void_p), ("oformat", C.c_void_p),
                ("priv_data", C.c_void_p), ("pb", C.c_void_p), ("ctx_flags", C.c_int),
                ("nb_streams", C.c_uint), ("streams", C.POINTER(C.POINTER(Stream)))]


class Frame(C.Structure):
    _fields_ = [("data", C.c_void_p * 8), ("linesize", C.c_int * 8), ("extended_data", C.c_void_p),
                ("width", C.c_int), ("height", C.c_int), ("nb_samples", C.c_int), ("format", C.c_int),
                ("key_frame", C.c_int), ("pict_type", C.c_int), ("sample_aspect_ratio", Rational),
                ("pts", C.c_int64), ("pkt_dts", C.c_int64), ("time_base", Rational), ("quality", C.c_int),
                ("opaque", C.c_void_p), ("repeat_pict", C.c_int), ("interlaced_frame", C.c_int),
                ("top_field_first", C.c_int), ("palette_has_changed", C.c_int), ("sample_rate", C.c_int),
                ("buf", C.c_void_p * 8), ("extended_buf", C.c_void_p), ("nb_extended_buf", C.c_int),
                ("side_data", C.c_void_p), ("nb_side_data", C.c_int), ("flags", C.c_int),
                ("color_range", C.c_int), ("color_primaries", C.c_int), ("color_trc", C.c_int),
                ("colorspace", C.c_int), ("chroma_location", C.c_int), ("best_effort_timestamp", C.c_int64)]


class BundledProbe:
    def __init__(self, directory: Path):
        if os.name != "nt" or C.sizeof(C.c_void_p) != 8:
            raise RuntimeError("Missing-PTS B probe currently requires Windows x64 bundled FFmpeg 7.1")
        self.dll_directory = os.add_dll_directory(str(directory.resolve()))
        self.util = C.CDLL(str(directory / "avutil-59.dll"))
        self.codec = C.CDLL(str(directory / "avcodec-61.dll"))
        self.format = C.CDLL(str(directory / "avformat-61.dll"))
        signatures = [(self.format, "avformat_version", C.c_uint, []),
                      (self.format, "avformat_open_input", C.c_int, [C.POINTER(C.c_void_p), C.c_char_p, C.c_void_p, C.c_void_p]),
                      (self.format, "avformat_find_stream_info", C.c_int, [C.c_void_p, C.c_void_p]),
                      (self.format, "av_find_best_stream", C.c_int, [C.c_void_p, C.c_int, C.c_int, C.c_int, C.c_void_p, C.c_int]),
                      (self.format, "av_seek_frame", C.c_int, [C.c_void_p, C.c_int, C.c_int64, C.c_int]),
                      (self.format, "av_read_frame", C.c_int, [C.c_void_p, C.c_void_p]),
                      (self.format, "avformat_close_input", None, [C.POINTER(C.c_void_p)]),
                      (self.codec, "av_packet_alloc", C.c_void_p, []),
                      (self.codec, "av_packet_clone", C.c_void_p, [C.c_void_p]),
                      (self.codec, "av_packet_unref", None, [C.c_void_p]),
                      (self.codec, "av_packet_free", None, [C.POINTER(C.c_void_p)]),
                      (self.codec, "avcodec_find_decoder", C.c_void_p, [C.c_int]),
                      (self.codec, "avcodec_alloc_context3", C.c_void_p, [C.c_void_p]),
                      (self.codec, "avcodec_parameters_to_context", C.c_int, [C.c_void_p, C.c_void_p]),
                      (self.codec, "avcodec_open2", C.c_int, [C.c_void_p, C.c_void_p, C.c_void_p]),
                      (self.codec, "avcodec_send_packet", C.c_int, [C.c_void_p, C.c_void_p]),
                      (self.codec, "avcodec_receive_frame", C.c_int, [C.c_void_p, C.c_void_p]),
                      (self.codec, "avcodec_free_context", None, [C.POINTER(C.c_void_p)]),
                      (self.util, "av_frame_alloc", C.c_void_p, []),
                      (self.util, "av_frame_free", None, [C.POINTER(C.c_void_p)]),
                      (self.util, "av_log_set_level", None, [C.c_int])]
        for library, name, result, arguments in signatures:
            function = getattr(library, name)
            function.restype, function.argtypes = result, arguments
        self.version = self.format.avformat_version()
        if self.version >> 16 != 61:
            raise RuntimeError("Expected bundled avformat major 61")
        self.util.av_log_set_level(-8)

    def open(self, path: Path) -> tuple[C.c_void_p, int, Stream]:
        context = C.c_void_p()
        if self.format.avformat_open_input(C.byref(context), str(path).encode("utf-8"), None, None) < 0:
            raise RuntimeError(f"Bundled avformat cannot open {path}")
        if self.format.avformat_find_stream_info(context, None) < 0:
            raise RuntimeError(f"Bundled avformat cannot probe {path}")
        index = self.format.av_find_best_stream(context, 0, -1, -1, None, 0)
        stream = C.cast(context, C.POINTER(Format)).contents.streams[index].contents
        return context, index, stream

    def observe_window(self, path: Path, target: int, back: int, tb: Fraction) -> dict:
        context, index, _ = self.open(path)
        packet = C.c_void_p(self.codec.av_packet_alloc())
        keys, end = [], None
        seek_target = target - round(back / tb)
        try:
            if self.format.av_seek_frame(context, index, seek_target, 1) < 0:
                raise ValueError("Counterexample probe seek failed")
            while self.format.av_read_frame(context, packet) >= 0:
                value = C.cast(packet, C.POINTER(Packet)).contents
                if value.stream_index == index:
                    record = {"key": bool(value.flags & 1),
                              "pts": None if value.pts == NOPTS else value.pts,
                              "dts": None if value.dts == NOPTS else value.dts}
                    if value.dts != NOPTS and value.dts > target:
                        end = record
                        break
                    if value.flags & 1 and value.dts != NOPTS:
                        keys.append(record)
                self.codec.av_packet_unref(packet)
            return {"target_ticks": target, "back_seconds": back, "seek_ticks": seek_target,
                    "j_keys": keys, "window_end_packet": end}
        finally:
            self.codec.av_packet_free(C.byref(packet))
            self.format.avformat_close_input(C.byref(context))

    def find_b_window(self, path: Path, oracle: dict) -> dict:
        """Observe actual av_seek_frame/read_frame packets, never infer from full-file PTS.

        PS parsers can assign timestamps differently after seek. Decode the same
        held packets and compare the first I picture with the independent oracle.
        """
        tb = Fraction(oracle["video_stream"]["time_base"])
        packet = C.c_void_p(self.codec.av_packet_alloc())
        try:
            for relative in range(20, 115):
                context, index, stream = self.open(path)
                try:
                    target = stream.start_time + round(Fraction(relative, 10) / tb)
                    for back in (1, 2, 4, 8, 16, 32):
                        seek_target = target - round(back / tb)
                        if seek_target <= stream.start_time:
                            break
                        if self.format.av_seek_frame(context, index, seek_target, 1) < 0:
                            continue
                        held, keys, first = [], [], None
                        try:
                            while self.format.av_read_frame(context, packet) >= 0:
                                value = C.cast(packet, C.POINTER(Packet)).contents
                                if value.stream_index == index:
                                    record = {"pts": None if value.pts == NOPTS else value.pts,
                                              "dts": None if value.dts == NOPTS else value.dts,
                                              "key": bool(value.flags & 1), "pos": value.pos,
                                              "sha256": hashlib.sha256(C.string_at(value.data, value.size)).hexdigest()}
                                    if first is None:
                                        first = record
                                    held.append(C.c_void_p(self.codec.av_packet_clone(packet)))
                                    if value.dts != NOPTS and value.dts > target:
                                        self.codec.av_packet_unref(packet)
                                        break
                                    if value.flags & 1 and value.dts != NOPTS:
                                        keys.append(record)
                                self.codec.av_packet_unref(packet)
                            a = any(k["pts"] is not None and k["pts"] <= target for k in keys)
                            b = len(keys) >= 2 and all(k["pts"] is None for k in keys)
                            if b:
                                decoded = self.first_i_frame(stream.codecpar, held)
                                correct = next(f for f in oracle["frames"] if f["frame_id"] == decoded["frame_id"])
                                full_keys = [p for p in oracle["packets"] if p["key"]]
                                key_index = next(i for i, p in enumerate(full_keys)
                                                 if p["data_sha256"] == keys[0]["sha256"])
                                # Key pictures are in display order in both independent
                                # full-decode output and full-file packet order.
                                expected = [f for f in oracle["frames"] if f["key_frame"]][key_index]
                                if decoded["frame_id"] != expected["frame_id"]:
                                    raise ValueError("Missing-PTS B decoder skipped the first key picture")
                                if decoded["best_effort_timestamp"] != correct["best_effort_timestamp"]:
                                    raise ValueError("Missing-PTS B first-key decoded time disagrees with full decode")
                                if correct["best_effort_timestamp"] > target:
                                    raise ValueError("Missing-PTS B first key displays after target")
                                return {"probe": "independent ctypes bundled avformat-61/avcodec-61", "avformat_version": self.version,
                                        "target_ticks": target, "target_media_seconds": float((target - oracle["format"]["container_start_ticks"]) * tb),
                                        "back_seconds": back, "seek_ticks": seek_target, "first_video_packet": first,
                                        "j_keys": keys, "a": False, "b": True,
                                        "first_key_full_packet_index": full_keys[key_index]["packet_index"],
                                        "first_key_decoded": decoded, "full_decode_frame": correct}
                            if a:
                                break  # The real helper stops at the first successful window.
                        finally:
                            for retained in held:
                                self.codec.av_packet_free(C.byref(retained))
                finally:
                    self.format.avformat_close_input(C.byref(context))
        finally:
            self.codec.av_packet_free(C.byref(packet))
        raise ValueError("PS fixture has no actual successful missing-PTS B window")

    def first_i_frame(self, parameters: int, packets: list[C.c_void_p]) -> dict:
        # AVCodecParameters starts with codec_type and codec_id (public ABI).
        codec_id = C.c_int.from_address(parameters + 4).value
        decoder = self.codec.avcodec_find_decoder(codec_id)
        context = C.c_void_p(self.codec.avcodec_alloc_context3(decoder))
        frame = C.c_void_p(self.util.av_frame_alloc())
        try:
            if self.codec.avcodec_parameters_to_context(context, parameters) < 0 or self.codec.avcodec_open2(context, decoder, None) < 0:
                raise RuntimeError("Could not open independent MPEG-2 decoder")
            for packet in [*packets, None]:
                result = self.codec.avcodec_send_packet(context, packet)
                if result < 0:
                    raise RuntimeError(f"Independent MPEG-2 send failed: {result}")
                while self.codec.avcodec_receive_frame(context, frame) >= 0:
                    value = C.cast(frame, C.POINTER(Frame)).contents
                    if value.pict_type != 1:
                        continue
                    if (value.width, value.height, value.format) != (WIDTH, HEIGHT, 0):
                        raise ValueError("Unexpected independent-decoder picture format")
                    gray = C.string_at(value.data[0], value.linesize[0] * HEIGHT)
                    return {"frame_id": frame_id(gray, value.linesize[0]),
                            "best_effort_timestamp": value.best_effort_timestamp,
                            "pts": None if value.pts == NOPTS else value.pts,
                            "pkt_dts": None if value.pkt_dts == NOPTS else value.pkt_dts}
        finally:
            self.util.av_frame_free(C.byref(frame))
            self.codec.avcodec_free_context(C.byref(context))
        raise ValueError("No I picture decoded from missing-PTS B window")


def generate(generator: Generator, bundled: Path) -> None:
    generator.output.mkdir(parents=True, exist_ok=True)
    specs = [("h264-small.ts", "libx264", {}),
             ("mpeg2-large.ts", "mpeg2video", {"offset": 36000}),
             ("hevc-open.ts", "libx265", {}),
             # 128 bytes gives missing-PTS keys and monotonic timestamped frames.
             # The actual post-seek B window is checked below, not assumed.
             ("mpeg2-missing-pts.mpg", "mpeg2video", {"extra": ("-packetsize", "128")}),
             ("long-gop.ts", "libx264", {"fps": 60, "seconds": 18, "gop": 360}),
             ("short-key-gap.ts", "mpeg2video", {"extra": ("-force_key_frames", "6,6.1")}),
             ("control.mp4", "libx264", {})]
    oracles = {}
    for name, codec, options in specs:
        print(f"Generating {name}", flush=True)
        generator.encode(name, codec, **options)
        oracles[name] = generator.oracle(name)
    # Preserve the encoded pictures and change only video PTS, not DTS.
    run([generator.ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-copyts",
         "-i", str(generator.output / "mpeg2-large.ts"), "-map", "0", "-c", "copy",
         "-bsf:v", "setts=pts=PTS+2/TB", "-muxdelay", "0", "-muxpreload", "0",
         "-f", "mpegts", str(generator.output / "pts-delayed.ts")])
    oracles["pts-delayed.ts"] = generator.oracle("pts-delayed.ts")
    for original, name in [("h264-small.ts", "mid-gop.ts"),
                           ("mpeg2-missing-pts.mpg", "mid-gop.mpg")]:
        print(f"Cutting {name}", flush=True)
        oracles[name] = generator.cut_head(original, name)
    hevc = run([generator.ffmpeg, "-hide_banner", "-loglevel", "error", "-i",
                str(generator.output / "hevc-open.ts"), "-map", "0:v:0", "-c:v", "copy", "-f", "hevc", "pipe:1"])
    nal_types = [(unit[0] >> 1) & 63 for unit in re.split(b"\x00\x00\x01", hevc) if unit]
    counts = {str(kind): nal_types.count(kind) for kind in sorted(set(nal_types))}
    if not counts.get("21") or not (counts.get("8") or counts.get("9")):
        raise ValueError("HEVC fixture needs both CRA and RASL NAL units")
    oracles["hevc-open.ts"]["validation"]["hevc_nal_unit_type_counts"] = counts
    short = oracles["short-key-gap.ts"]
    key_ids = [f["frame_id"] for f in short["frames"] if f["key_frame"]]
    short_gaps = [(a, b) for a, b in zip(key_ids, key_ids[1:]) if b - a == 3]
    if len(short_gaps) != 1:
        raise ValueError(f"Expected exactly one three-frame key gap, found {short_gaps}")
    short["validation"]["three_frame_key_gap"] = list(short_gaps[0])
    boundary_frame = next(f for f in short["frames"] if f["frame_id"] == short_gaps[0][1])
    boundary_key = next(p for p in short["packets"] if p["key"] and p["pts"] == boundary_frame["best_effort_timestamp"])
    short["special_targets"]["window_end_key_is_not_j"] = {
        "target_ticks": boundary_key["dts"] - 1, "end_key_packet_index": boundary_key["packet_index"],
        "end_key_pts": boundary_key["pts"], "end_key_dts": boundary_key["dts"]}
    for name in ("mpeg2-large.ts", "mpeg2-missing-pts.mpg"):
        packets = oracles[name]["packets"]
        open_gops = sum(any(p["pts"] is not None and key["pts"] is not None and p["pts"] < key["pts"]
                            for p in packets[index + 1:index + 4])
                        for index, key in enumerate(packets) if key["key"])
        if not open_gops:
            raise ValueError(f"{name}: no open-GOP leading B packets")
        oracles[name]["validation"]["open_gop_count_with_leading_b"] = open_gops
    long = oracles["long-gop.ts"]
    long_frame = next(f for f in long["frames"] if f["frame_id"] == 690)
    prior_key = max((f for f in long["frames"] if f["key_frame"] and
                     f["best_effort_timestamp"] <= long_frame["best_effort_timestamp"]),
                    key=lambda f: f["best_effort_timestamp"])
    if long_frame["frame_id"] - prior_key["frame_id"] <= 240:
        raise ValueError("Long-GOP target must exceed the old 240-picture preroll")
    long["special_targets"]["preroll_over_240"] = {
        "target_ticks": long_frame["best_effort_timestamp"], "frame_id": long_frame["frame_id"],
        "prior_key_frame_id": prior_key["frame_id"], "preroll_frame_count": long_frame["frame_id"] - prior_key["frame_id"]}
    probe = BundledProbe(bundled)
    long_windows = [probe.observe_window(generator.output / long["file"], long_frame["best_effort_timestamp"],
                                         back, Fraction(long["video_stream"]["time_base"])) for back in (1, 2, 4, 8)]
    if any(window["j_keys"] for window in long_windows[:3]) or not long_windows[3]["j_keys"]:
        raise ValueError("Long-GOP fixture must require at least two back doublings")
    long["special_targets"]["preroll_over_240"]["bundled_probe_windows"] = long_windows
    delayed = oracles["pts-delayed.ts"]
    delayed_keys = [p for p in delayed["packets"] if p["key"]]
    window = probe.observe_window(generator.output / delayed["file"], delayed_keys[10]["dts"], 1,
                                  Fraction(delayed["video_stream"]["time_base"]))
    delayed["special_targets"]["known_pts_after_target"] = window
    if len(window["j_keys"]) < 2 or not all(p["pts"] is not None and p["pts"] > window["target_ticks"]
                                           for p in window["j_keys"]):
        raise ValueError("Delayed-PTS counterexample no longer contradicts key-count inference")
    print("Verifying actual bundled-DLL missing-PTS B window", flush=True)
    ps = oracles["mpeg2-missing-pts.mpg"]
    if not ps["validation"]["missing_pts_key_count"]:
        raise ValueError("PS fixture contains no missing-PTS key")
    ps["special_targets"]["missing_pts_b"] = probe.find_b_window(generator.output / ps["file"], ps)
    for oracle in oracles.values():
        generator.write_oracle(oracle)
        print(f"{oracle['file']}: {(generator.output / oracle['file']).stat().st_size} bytes, "
              f"{len(oracle['frames'])} frames, {len(oracle['packets'])} packets", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, nargs="?", default=DEFAULT_OUTPUT)
    parser.add_argument("--ffmpeg", default=shutil.which("ffmpeg") or "ffmpeg")
    parser.add_argument("--ffprobe", help="Defaults to ffprobe beside ffmpeg, then PATH")
    parser.add_argument("--bundled-dll-dir", type=Path, default=ROOT / "vendor" / "ffmpeg" / "bin")
    args = parser.parse_args()
    sibling = Path(args.ffmpeg).with_name("ffprobe.exe" if os.name == "nt" else "ffprobe")
    ffprobe = args.ffprobe or (str(sibling) if sibling.exists() else shutil.which("ffprobe") or "ffprobe")
    generate(Generator(args.output, args.ffmpeg, ffprobe), args.bundled_dll_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
