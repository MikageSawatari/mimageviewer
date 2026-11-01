#pragma once
#include "now_playing.h"
#include "pluginterfaces/vst/ivsthostapplication.h"

namespace miv {
// Must be called on the slot GUI thread. No editor creation or audio fence.
void notify_now_playing(Steinberg::Vst::IHostApplication* host,
                        Steinberg::FUnknown* component,
                        Steinberg::FUnknown* controller,
                        const NowPlayingSnapshot& snapshot);
void notify_playback_position(Steinberg::Vst::IHostApplication* host,
                              Steinberg::FUnknown* component,
                              Steinberg::FUnknown* controller,
                              const PlaybackPositionSnapshot& snapshot);
}
