#pragma once

namespace miv {
enum class GuiSuppression : unsigned { Minimized = 1, RemoteSession = 2 };
// Single owner for requested visibility and the set of temporary hide reasons.
// Suppression never changes the user's request; the last reason leaving can
// restore only a previously requested-visible editor.
class GuiVisibility {
public:
    constexpr void request(bool visible) { requested_ = visible; }
    constexpr bool requested() const { return requested_; }
    constexpr void suppress(GuiSuppression reason, bool active) {
        const auto bit = static_cast<unsigned>(reason);
        if (active) reasons_ |= bit;
        else reasons_ &= ~bit;
    }
    constexpr bool should_show(bool unowned, bool app_active) const {
        return requested_ && (unowned ? reasons_ == 0 : app_active);
    }
private:
    bool requested_ = false;
    unsigned reasons_ = 0;
};
}
