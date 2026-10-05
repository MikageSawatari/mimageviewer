// MIT license. LibRaw never throws across this C ABI.
#include "miv_libraw.h"
#include "libraw/libraw.h"
#include <climits>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <new>

namespace {
constexpr size_t kMaxPreviewBytes = 512ull * 1024 * 1024;

struct FreeBuffer {
  void operator()(unsigned char *data) const noexcept { std::free(data); }
};

struct Handle {
  LibRaw raw;
  bool info_only = false;
  bool developed = false;
};

struct Progress {
  Handle *handle;
  MivRawProgress callback;
  void *user;
};

int miv_progress_callback(void *opaque, LibRaw_progress stage, int iteration, int expected) {
  Progress *progress = static_cast<Progress *>(opaque);
  if (progress->callback && progress->callback(progress->user, static_cast<int>(stage), iteration, expected)) {
    progress->handle->raw.setCancelFlag();
    return 1;
  }
  return 0;
}

void copy_text(char *out, size_t capacity, const char *text) {
  if (!text) text = "";
  std::strncpy(out, text, capacity - 1);
  out[capacity - 1] = '\0';
}

uint32_t preview_format(int format) {
  if (format == LIBRAW_INTERNAL_THUMBNAIL_JPEG) return LIBRAW_THUMBNAIL_JPEG;
  if (format == LIBRAW_INTERNAL_THUMBNAIL_PPM ||
      format == LIBRAW_INTERNAL_THUMBNAIL_KODAK_THUMB ||
      format == LIBRAW_INTERNAL_THUMBNAIL_KODAK_YCBCR ||
      format == LIBRAW_INTERNAL_THUMBNAIL_KODAK_RGB ||
      format == LIBRAW_INTERNAL_THUMBNAIL_DNG_YCBCR)
    return LIBRAW_THUMBNAIL_BITMAP;
  return LIBRAW_THUMBNAIL_UNKNOWN;
}
} // namespace

extern "C" void *miv_raw_new(void) {
  try { return new Handle(); } catch (...) { return nullptr; }
}

extern "C" int miv_raw_open_path(void *opaque, const wchar_t *path) {
  try {
    if (!opaque || !path) return LIBRAW_DATA_ERROR;
    return static_cast<Handle *>(opaque)->raw.open_file(path);
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_open_buffer(void *opaque, const unsigned char *data, size_t length) {
  try {
    if (!opaque || !data || !length) return LIBRAW_DATA_ERROR;
    return static_cast<Handle *>(opaque)->raw.open_buffer(data, length);
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_info(void *opaque, MivRawInfo *out) {
  try {
    if (!opaque || !out) return LIBRAW_DATA_ERROR;
    auto *h = static_cast<Handle *>(opaque);
    libraw_decoder_info_t decoder{};
    int result = h->raw.get_decoder_info(&decoder);
    if (result) return result;
    out->unsupported = (decoder.decoder_flags & LIBRAW_DECODER_UNSUPPORTED_FORMAT) != 0;
    out->flip = h->raw.imgdata.sizes.flip;
    copy_text(out->make, sizeof(out->make), h->raw.imgdata.idata.make);
    copy_text(out->model, sizeof(out->model), h->raw.imgdata.idata.model);
    out->preview_count = static_cast<uint32_t>(h->raw.imgdata.thumbs_list.thumbcount);
    result = h->raw.adjust_sizes_info_only();
    if (result) return result;
    out->width = h->raw.imgdata.sizes.iwidth;
    out->height = h->raw.imgdata.sizes.iheight;
    h->info_only = true;
    return 0;
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_preview_info(void *opaque, uint32_t index, MivRawPreviewInfo *out) {
  try {
    if (!opaque || !out) return LIBRAW_DATA_ERROR;
    auto &list = static_cast<Handle *>(opaque)->raw.imgdata.thumbs_list;
    if (index >= static_cast<uint32_t>(list.thumbcount)) return LIBRAW_REQUEST_FOR_NONEXISTENT_THUMBNAIL;
    const auto &item = list.thumblist[index];
    *out = {preview_format(item.tformat), item.twidth, item.theight, item.tflip, item.tlength};
    return 0;
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_preview_extract(void *opaque, uint32_t index, MivRawBuffer *out) {
  try {
    if (!opaque || !out) return LIBRAW_DATA_ERROR;
    *out = {};
    auto *h = static_cast<Handle *>(opaque);
    auto &list = h->raw.imgdata.thumbs_list;
    if (index >= static_cast<uint32_t>(list.thumbcount)) return LIBRAW_REQUEST_FOR_NONEXISTENT_THUMBNAIL;
    const auto &item = list.thumblist[index];
    int format = item.tformat;
    if (!preview_format(format))
      return LIBRAW_UNSUPPORTED_THUMBNAIL;
    // LibRaw allocates the thumbnail during unpack_thumb_ex, before we can inspect it.
    if (item.tlength > kMaxPreviewBytes)
      return LIBRAW_TOO_BIG;
    int result = h->raw.unpack_thumb_ex(static_cast<int>(index));
    if (result) return result;
    auto &thumb = h->raw.imgdata.thumbnail;
    if (thumb.tformat != LIBRAW_THUMBNAIL_JPEG && thumb.tformat != LIBRAW_THUMBNAIL_BITMAP)
      return LIBRAW_UNSUPPORTED_THUMBNAIL;
    if (!thumb.thumb || !thumb.tlength) return LIBRAW_DATA_ERROR;
    if (thumb.tlength > kMaxPreviewBytes)
      return LIBRAW_TOO_BIG;
    std::unique_ptr<unsigned char, FreeBuffer> copy(
        static_cast<unsigned char *>(std::malloc(thumb.tlength)));
    if (!copy) return LIBRAW_UNSUFFICIENT_MEMORY;
    std::memcpy(copy.get(), thumb.thumb, thumb.tlength);
    *out = {copy.release(), thumb.tlength, thumb.twidth, thumb.theight,
            static_cast<uint32_t>(thumb.tcolors), static_cast<uint32_t>(thumb.tformat)};
    return 0;
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_develop(void *opaque, int half, int bright_mode,
                                MivRawProgress callback, void *user) {
  try {
    if (!opaque) return LIBRAW_DATA_ERROR;
    auto *h = static_cast<Handle *>(opaque);
    if (h->info_only || h->developed) return LIBRAW_OUT_OF_ORDER_CALL;
    libraw_decoder_info_t decoder{};
    int decoder_result = h->raw.get_decoder_info(&decoder);
    if (decoder_result) return decoder_result;
    if (decoder.decoder_flags & LIBRAW_DECODER_UNSUPPORTED_FORMAT)
      return LIBRAW_NOT_IMPLEMENTED;
    auto &params = h->raw.imgdata.params;
    params.use_camera_wb = 1;
    params.output_color = 1;
    params.output_bps = 8;
    params.gamm[0] = 1.0 / 2.4;
    params.gamm[1] = 12.92;
    params.highlight = 0;
    params.user_qual = -1;
    params.half_size = half ? 1 : 0;
    params.no_auto_bright = bright_mode == 2;
    params.auto_bright_thr = bright_mode == 1 ? 0.001f : 0.01f;
    params.bright = 1.0f;
    Progress progress{h, callback, user};
    h->raw.set_progress_handler(miv_progress_callback, &progress);
    int result = h->raw.unpack();
    if (!result) result = h->raw.dcraw_process();
    h->raw.set_progress_handler(nullptr, nullptr);
    if (!result) h->developed = true;
    return result;
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_image_info(void *opaque, uint32_t *width, uint32_t *height) {
  try {
    if (!opaque || !width || !height) return LIBRAW_DATA_ERROR;
    auto *h = static_cast<Handle *>(opaque);
    if (!h->developed) return LIBRAW_OUT_OF_ORDER_CALL;
    int w = 0, hgt = 0, colors = 0, bits = 0;
    h->raw.get_mem_image_format(&w, &hgt, &colors, &bits);
    if (w <= 0 || hgt <= 0 || colors != 3 || bits != 8) return LIBRAW_DATA_ERROR;
    *width = static_cast<uint32_t>(w);
    *height = static_cast<uint32_t>(hgt);
    return 0;
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_copy_rgb(void *opaque, unsigned char *data, size_t stride) {
  try {
    if (!opaque || !data || stride > INT_MAX) return LIBRAW_DATA_ERROR;
    auto *h = static_cast<Handle *>(opaque);
    if (!h->developed) return LIBRAW_OUT_OF_ORDER_CALL;
    int w = 0, hgt = 0, colors = 0, bits = 0;
    h->raw.get_mem_image_format(&w, &hgt, &colors, &bits);
    if (w <= 0 || hgt <= 0 || colors != 3 || bits != 8 || stride < static_cast<size_t>(w) * 3)
      return LIBRAW_DATA_ERROR;
    return h->raw.copy_mem_image(data, static_cast<int>(stride), 0);
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_copy_rgb_adjusted(void *opaque, unsigned char *data,
                                           size_t stride, int bright_mode, float gain) {
  try {
    if (!opaque) return LIBRAW_DATA_ERROR;
    auto *h = static_cast<Handle *>(opaque);
    if (!h->developed || !std::isfinite(gain)) return LIBRAW_OUT_OF_ORDER_CALL;
    auto &params = h->raw.imgdata.params;
    if (bright_mode == 1) {
      params.no_auto_bright = 0;
      params.auto_bright_thr = 0.001f;
      params.bright = 1.0f;
    } else if (bright_mode == 3 && gain >= 0.125f && gain <= 8.0f) {
      params.no_auto_bright = 1;
      params.bright = gain;
    } else {
      return LIBRAW_DATA_ERROR;
    }
    // copy_mem_image rebuilds the gamma curve from these params without demosaicing again.
    return miv_raw_copy_rgb(opaque, data, stride);
  } catch (const std::bad_alloc &) { return LIBRAW_UNSUFFICIENT_MEMORY; }
    catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_set_cancel_flag(void *opaque) {
  try {
    if (!opaque) return LIBRAW_DATA_ERROR;
    static_cast<Handle *>(opaque)->raw.setCancelFlag();
    return 0;
  } catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_free(unsigned char *data) {
  try { std::free(data); return 0; } catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}

extern "C" int miv_raw_close(void *opaque) {
  try { delete static_cast<Handle *>(opaque); return 0; }
  catch (...) { return LIBRAW_UNSPECIFIED_ERROR; }
}
