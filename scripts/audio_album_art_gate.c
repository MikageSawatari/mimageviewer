// Legacy FFmpeg allocation repro, never an mImageViewer product executable.
// Preserve the failed gate evidence; redesigned album-art extraction excludes
// FFmpeg. This harness is not the acceptance gate for the new Rust reader.
// No stream-info probing, packet iteration, decoder, or global allocation cap.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <psapi.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <errno.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/mem.h>

#define INPUT_LIMIT (32LL * 1024 * 1024)
#define READ_LIMIT (INPUT_LIMIT + 64 * 1024)
#define EOF_ERROR (-541478725)
#define EXIT_ERROR (-1414092869)

typedef struct IoState {
    FILE *file;
    int64_t size;
    int64_t bytes;
    int reads, seeks, stops, reason;
    int64_t cancel_after, read_limit;
    ULONGLONG deadline;
} IoState;

static int interrupted(void *opaque) {
    IoState *io = opaque;
    if (io->reason) return 1;
    if (io->cancel_after >= 0 && io->bytes >= io->cancel_after) io->reason = 1;
    if (GetTickCount64() >= io->deadline) io->reason = 2;
    return io->reason != 0;
}

static int read_packet(void *opaque, uint8_t *buf, int size) {
    IoState *io = opaque;
    io->reads++;
    if (interrupted(io)) { io->stops++; return EXIT_ERROR; }
    if (io->bytes >= io->read_limit) { io->reason = 3; io->stops++; return EXIT_ERROR; }
    if (size > io->read_limit - io->bytes) size = (int)(io->read_limit - io->bytes);
    size_t count = fread(buf, 1, size, io->file);
    io->bytes += count;
    if (interrupted(io)) { io->stops++; return EXIT_ERROR; }
    if (!count) return ferror(io->file) ? -EIO : EOF_ERROR;
    return (int)count;
}

static int64_t seek_packet(void *opaque, int64_t offset, int whence) {
    IoState *io = opaque;
    io->seeks++;
    if (interrupted(io)) { io->stops++; return EXIT_ERROR; }
    if (whence & AVSEEK_SIZE) return io->size;
    whence &= ~AVSEEK_FORCE;
    int64_t base = whence == SEEK_SET ? 0 : whence == SEEK_CUR ? _ftelli64(io->file) : whence == SEEK_END ? io->size : -1;
    if (base < 0 || offset < -base || offset > io->size - base) return -EINVAL;
    if (_fseeki64(io->file, base + offset, SEEK_SET)) return -EIO;
    return base + offset;
}

// Inspect all consecutive ID3 headers before FFmpeg sees them. This bounds the
// total physical tag bytes; it intentionally does not implement an APIC parser.
static int preflight(IoState *io, int64_t *tag_bytes, int *headers) {
    uint8_t h[10];
    int64_t pos = 0;
    *tag_bytes = 0;
    *headers = 0;
    while (1) {
        if (interrupted(io)) return -1;
        if (_fseeki64(io->file, pos, SEEK_SET)) return -1;
        size_t n = fread(h, 1, 10, io->file);
        if (n < 3 || memcmp(h, "ID3", 3)) break;
        if (n != 10 || (h[6] | h[7] | h[8] | h[9]) & 0x80) return -1;
        int64_t len = 10 + ((int64_t)h[6] << 21) + ((int64_t)h[7] << 14) + ((int64_t)h[8] << 7) + h[9];
        if (h[3] == 4 && (h[5] & 0x10)) len += 10;
        if (len > INPUT_LIMIT - *tag_bytes || len > io->size - pos) return -1;
        *tag_bytes += len;
        (*headers)++;
        pos += len;
    }
    return _fseeki64(io->file, 0, SEEK_SET) ? -1 : 0;
}

static SIZE_T private_peak(void) {
    PROCESS_MEMORY_COUNTERS_EX mem = {0};
    mem.cb = sizeof(mem);
    GetProcessMemoryInfo(GetCurrentProcess(), (PROCESS_MEMORY_COUNTERS *)&mem, sizeof(mem));
    return mem.PeakPagefileUsage;
}

int main(int argc, char **argv) {
    if (argc != 5) return 2;
    IoState io = {0};
    io.cancel_after = _strtoi64(argv[2], NULL, 10);
    io.read_limit = _strtoi64(argv[3], NULL, 10);
    io.deadline = GetTickCount64() + _strtoui64(argv[4], NULL, 10);
    io.file = fopen(argv[1], "rb");
    if (!io.file) return 3;
    _fseeki64(io.file, 0, SEEK_END);
    io.size = _ftelli64(io.file);
    int64_t tag_bytes;
    int headers;
    int pf = preflight(&io, &tag_bytes, &headers);
    int ret = -1, pictures = 0, front = 0;
    int64_t largest = 0;
    SIZE_T baseline = private_peak(), peak = baseline;
    ULONGLONG started = GetTickCount64();
    AVFormatContext *ctx = NULL;
    AVIOContext *pb = NULL;
    if (!pf) {
        uint8_t *buffer = av_malloc(4096);
        ctx = avformat_alloc_context();
        if (!buffer || !ctx) return 4;
        pb = avio_alloc_context(buffer, 4096, 0, &io, read_packet, NULL, seek_packet);
        if (!pb) return 4;
        ctx->pb = pb;
        ctx->flags |= AVFMT_FLAG_CUSTOM_IO;
        ctx->interrupt_callback.callback = interrupted;
        ctx->interrupt_callback.opaque = &io;
        // 16 pictures + the audio stream; this only limits stream creation.
        ctx->max_streams = 17;
        ret = avformat_open_input(&ctx, NULL, av_find_input_format("mp3"), NULL);
        peak = private_peak();
        if (ret >= 0) {
            for (unsigned i = 0; i < ctx->nb_streams; i++) {
                AVStream *s = ctx->streams[i];
                if (!(s->disposition & AV_DISPOSITION_ATTACHED_PIC)) continue;
                pictures++;
                if (s->attached_pic.size > largest) largest = s->attached_pic.size;
                AVDictionaryEntry *type = av_dict_get(s->metadata, "comment", NULL, 0);
                if (type && !strcmp(type->value, "Cover (front)")) front++;
            }
        }
        avformat_close_input(&ctx);
        av_freep(&pb->buffer);
        avio_context_free(&pb);
    }
    printf("{\"ffmpeg\":\"%s\",\"preflight\":%d,\"ret\":%d,\"headers\":%d,\"tag_bytes\":%lld,\"bytes_read\":%lld,\"reads\":%d,\"seeks\":%d,\"stop_callbacks\":%d,\"reason\":%d,\"pictures\":%d,\"front\":%d,\"largest_picture\":%lld,\"peak_private_delta\":%llu,\"elapsed_ms\":%llu}\n",
        av_version_info(), pf, ret, headers, tag_bytes, io.bytes, io.reads, io.seeks, io.stops, io.reason,
        pictures, front, largest, (unsigned long long)(peak - baseline), (unsigned long long)(GetTickCount64() - started));
    fclose(io.file);
    return 0;
}
