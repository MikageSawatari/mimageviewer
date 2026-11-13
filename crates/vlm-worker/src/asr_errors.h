#pragma once
#include <stdexcept>

namespace asr {
// モデルやGPUの失敗と区別し、この入力だけを失敗にして次の入力へ進む。
struct InvalidUtf8 : std::runtime_error { using std::runtime_error::runtime_error; };
}
