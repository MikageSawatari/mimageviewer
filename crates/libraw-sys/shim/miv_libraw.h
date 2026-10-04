// MIT license. Narrow C ABI for mImageViewer's LibRaw integration.
#pragma once
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  uint32_t width, height, flip;
  int unsupported;
  char make[128], model[128];
  uint32_t preview_count;
} MivRawInfo;

typedef struct {
  uint32_t format, width, height, tflip, length;
} MivRawPreviewInfo;

typedef struct {
  unsigned char *data;
  size_t length;
  uint32_t width, height, colors, format;
} MivRawBuffer;

typedef int (*MivRawProgress)(void *, int, int, int);

void *miv_raw_new(void);
int miv_raw_open_path(void *, const wchar_t *);
int miv_raw_open_buffer(void *, const unsigned char *, size_t);
int miv_raw_info(void *, MivRawInfo *);
int miv_raw_preview_info(void *, uint32_t, MivRawPreviewInfo *);
int miv_raw_preview_extract(void *, uint32_t, MivRawBuffer *);
int miv_raw_develop(void *, int, int, MivRawProgress, void *);
int miv_raw_image_info(void *, uint32_t *, uint32_t *);
int miv_raw_copy_rgb(void *, unsigned char *, size_t);
// After a no-auto-bright develop: 1 = auto threshold 0.001, 3 = linear gain.
int miv_raw_copy_rgb_adjusted(void *, unsigned char *, size_t, int, float);
int miv_raw_set_cancel_flag(void *);
int miv_raw_free(unsigned char *);
int miv_raw_close(void *);

#ifdef __cplusplus
}
#endif
