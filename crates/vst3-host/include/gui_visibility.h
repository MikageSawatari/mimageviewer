#pragma once
#include "gui_gate.h"

namespace miv {
enum class GuiSuppression : unsigned { Minimized = 1, RemoteSession = 2 };
// Single owner for requested visibility and the set of temporary hide reasons.
// Suppression never changes the user's request; the last reason leaving can
// restore only a previously requested-visible editor.
class GuiVisibility {
public:
    constexpr void request(bool visible) { requested_ = visible; }
    constexpr bool requested() const { return requested_; }
    // A rejected open must not create a request for restoration. Preserve an
    // earlier accepted request if this was only an explicit raise attempt.
    constexpr bool accept_show(GuiGateSnapshot permit, GuiGateSnapshot current, bool minimized, bool main_visible = true) {
        if (!permit.permits(current, minimized, main_visible)) return false;
        requested_ = true;
        return true;
    }
    constexpr void suppress(GuiSuppression reason, bool active) {
        const auto bit = static_cast<unsigned>(reason);
        if (active) reasons_ |= bit;
        else reasons_ &= ~bit;
    }
    constexpr void reconcile_main(bool main_exists, bool minimized, bool keep_visible) {
        suppress(GuiSuppression::Minimized, !main_exists || (minimized && !keep_visible));
    }
    constexpr bool should_show(bool unowned, bool app_active) const {
        return requested_ && (unowned ? reasons_ == 0 : app_active);
    }
private:
    bool requested_ = false;
    unsigned reasons_ = 0;
};
}
