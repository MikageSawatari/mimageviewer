// WASAPI loopback recorder for the lyrics trial sync measurement.
// usage: loopback.exe <seconds> <out_prefix>
// Writes <out_prefix>.f32 (interleaved float32 if the mix format is float) and
// <out_prefix>.csv with one row per packet: first_frame,frames,qpc_100ns,flags.
// The QPC position is the time the first frame of the packet was written to the
// endpoint buffer (IAudioCaptureClient::GetBuffer), in 100 ns units.
#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <cstdio>
#include <cstdlib>
#include <vector>

#pragma comment(lib, "ole32.lib")

int wmain(int argc, wchar_t **argv) {
  if (argc < 3) { fwprintf(stderr, L"usage: loopback <seconds> <out_prefix>\n"); return 2; }
  const double seconds = _wtof(argv[1]);
  CoInitializeEx(nullptr, COINIT_MULTITHREADED);
  IMMDeviceEnumerator *enumerator = nullptr;
  if (FAILED(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                              __uuidof(IMMDeviceEnumerator), (void **)&enumerator))) return 3;
  IMMDevice *device = nullptr;
  if (FAILED(enumerator->GetDefaultAudioEndpoint(eRender, eConsole, &device))) return 4;
  IAudioClient *client = nullptr;
  if (FAILED(device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, (void **)&client))) return 5;
  WAVEFORMATEX *format = nullptr;
  client->GetMixFormat(&format);
  if (FAILED(client->Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
                                10000000, 0, format, nullptr))) return 6;
  IAudioCaptureClient *capture = nullptr;
  if (FAILED(client->GetService(__uuidof(IAudioCaptureClient), (void **)&capture))) return 7;
  LARGE_INTEGER frequency; QueryPerformanceFrequency(&frequency);
  wchar_t path[MAX_PATH];
  swprintf_s(path, L"%s.f32", argv[2]);
  FILE *raw = _wfopen(path, L"wb");
  swprintf_s(path, L"%s.csv", argv[2]);
  FILE *csv = _wfopen(path, L"w");
  fprintf(csv, "# rate=%lu channels=%u bits=%u tag=%u qpc_freq=%lld\n", format->nSamplesPerSec,
          format->nChannels, format->wBitsPerSample, format->wFormatTag, frequency.QuadPart);
  fprintf(csv, "first_frame,frames,qpc_100ns,flags\n");
  client->Start();
  LARGE_INTEGER start, now; QueryPerformanceCounter(&start);
  unsigned long long total = 0;
  std::vector<char> silence;
  for (;;) {
    QueryPerformanceCounter(&now);
    if ((double)(now.QuadPart - start.QuadPart) / frequency.QuadPart > seconds) break;
    Sleep(2);
    UINT32 packet = 0;
    while (SUCCEEDED(capture->GetNextPacketSize(&packet)) && packet > 0) {
      BYTE *data = nullptr; UINT32 frames = 0; DWORD flags = 0; UINT64 position = 0, qpc = 0;
      if (FAILED(capture->GetBuffer(&data, &frames, &flags, &position, &qpc))) break;
      const size_t bytes = (size_t)frames * format->nBlockAlign;
      if (flags & AUDCLNT_BUFFERFLAGS_SILENT) {
        silence.assign(bytes, 0);
        fwrite(silence.data(), 1, bytes, raw);
      } else {
        fwrite(data, 1, bytes, raw);
      }
      fprintf(csv, "%llu,%u,%llu,%lu\n", total, frames, qpc, flags);
      total += frames;
      capture->ReleaseBuffer(frames);
    }
  }
  client->Stop();
  fclose(raw); fclose(csv);
  wprintf(L"frames=%llu rate=%lu channels=%u bits=%u\n", total, format->nSamplesPerSec,
          format->nChannels, format->wBitsPerSample);
  return 0;
}
