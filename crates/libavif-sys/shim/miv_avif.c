// Keep libavif's ABI private to C. The input remains borrowed until close.
#include <avif/avif.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>

typedef struct {
    uint32_t width, height, depth, alpha;
    uint32_t crop_x, crop_y, crop_width, crop_height;
    uint32_t rotation, mirror; // counterclockwise quarter turns; 0=none, 1=vertical, 2=horizontal
} MivAvifInfo;

void *miv_avif_new(uint32_t dimension, uint32_t pixels, int threads) {
    avifDecoder *decoder = avifDecoderCreate();
    if (!decoder) return NULL;
    decoder->codecChoice = AVIF_CODEC_CHOICE_DAV1D;
    decoder->strictFlags = AVIF_STRICT_DISABLED;
    decoder->ignoreExif = AVIF_TRUE;
    decoder->ignoreXMP = AVIF_TRUE;
    decoder->imageDimensionLimit = dimension;
    decoder->imageSizeLimit = pixels;
    decoder->maxThreads = threads;
    return decoder;
}

int miv_avif_parse(void *handle, const uint8_t *data, size_t length, MivAvifInfo *info) {
    avifDecoder *decoder = handle;
    avifResult result = avifDecoderSetIOMemory(decoder, data, length);
    if (result != AVIF_RESULT_OK) return result;
    result = avifDecoderParse(decoder);
    if (result != AVIF_RESULT_OK) return result;
    const avifImage *image = decoder->image;
    info->width = image->width;
    info->height = image->height;
    info->depth = image->depth;
    info->alpha = decoder->alphaPresent;
    info->crop_x = info->crop_y = 0;
    info->crop_width = image->width;
    info->crop_height = image->height;
    if (image->transformFlags & AVIF_TRANSFORM_CLAP) {
        avifCropRect rect;
        // Invalid clap alone is ignored. Never leave its diagnostic on a successful parse.
        avifDiagnostics diag;
        avifDiagnosticsClearError(&diag);
        if (avifCropRectFromCleanApertureBox(&rect, &image->clap, image->width, image->height, &diag)) {
            info->crop_x = rect.x;
            info->crop_y = rect.y;
            info->crop_width = rect.width;
            info->crop_height = rect.height;
        }
    }
    info->rotation = (image->transformFlags & AVIF_TRANSFORM_IROT) ? image->irot.angle : 0;
    info->mirror = (image->transformFlags & AVIF_TRANSFORM_IMIR) ? image->imir.axis + 1 : 0;
    return AVIF_RESULT_OK;
}

// Stage is separate from avifResult: NextImage and RGB conversion can return the same code.
int miv_avif_decode(void *handle, uint8_t *pixels, size_t length, uint32_t stride, int *stage) {
    avifDecoder *decoder = handle;
    *stage = 0;
    avifResult result = avifDecoderNextImage(decoder);
    if (result != AVIF_RESULT_OK) return result;
    *stage = 1;
    // NextImage must not turn parsed metadata into an out-of-bounds RGB write.
    if ((uint64_t)decoder->image->width * 4 > stride ||
        (uint64_t)stride * decoder->image->height > length) {
        return AVIF_RESULT_INVALID_ARGUMENT;
    }
    avifRGBImage rgb;
    avifRGBImageSetDefaults(&rgb, decoder->image);
    rgb.format = AVIF_RGB_FORMAT_RGBA;
    rgb.depth = 8;
    rgb.alphaPremultiplied = AVIF_FALSE;
    rgb.pixels = pixels;
    rgb.rowBytes = stride;
    return avifImageYUVToRGB(decoder->image, &rgb);
}

void miv_avif_error(void *handle, int code, char *out, size_t length) {
    const avifDecoder *decoder = handle;
    snprintf(out, length, "%s: %s", avifResultToString((avifResult)code), decoder->diag.error);
}
void miv_avif_close(void *handle) { avifDecoderDestroy(handle); }
