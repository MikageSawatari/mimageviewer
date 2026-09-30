#include "gui_visibility.h"

// Compile-time regressions; no host, editor, or native window is launched.
constexpr bool reasons_release_in_either_order(bool release_remote_first) {
    miv::GuiVisibility state;
    state.request(true);
    state.suppress(miv::GuiSuppression::Minimized, true);
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    if (state.should_show(true, true) || !state.requested()) return false;
    const auto first = release_remote_first ? miv::GuiSuppression::RemoteSession : miv::GuiSuppression::Minimized;
    const auto last = release_remote_first ? miv::GuiSuppression::Minimized : miv::GuiSuppression::RemoteSession;
    state.suppress(first, false);
    if (state.should_show(true, true)) return false;
    state.suppress(last, false);
    return state.should_show(true, false); // unowned remains visible on app deactivation
}
constexpr bool hidden_stays_hidden(bool close_during_suppression) {
    miv::GuiVisibility state;
    state.request(close_during_suppression);
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    state.suppress(miv::GuiSuppression::Minimized, true);
    state.request(false);
    state.suppress(miv::GuiSuppression::RemoteSession, false);
    state.suppress(miv::GuiSuppression::Minimized, false);
    return !state.should_show(true, true);
}
constexpr bool repeated_reason_is_a_set_not_a_counter() {
    miv::GuiVisibility state;
    state.request(true);
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    state.suppress(miv::GuiSuppression::RemoteSession, false);
    return state.should_show(true, true);
}
constexpr bool hidden_attach_under_remote_does_not_open_on_release() {
    miv::GuiVisibility state;
    state.request(false);
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    state.suppress(miv::GuiSuppression::RemoteSession, false);
    return !state.should_show(true, true);
}
constexpr bool owned_policy_keeps_existing_activation_behavior() {
    miv::GuiVisibility state;
    state.request(true);
    state.suppress(miv::GuiSuppression::Minimized, true);
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    return state.should_show(false, true) && !state.should_show(false, false);
}
static_assert(reasons_release_in_either_order(true));
static_assert(reasons_release_in_either_order(false));
static_assert(hidden_stays_hidden(false));
static_assert(hidden_stays_hidden(true));
static_assert(repeated_reason_is_a_set_not_a_counter());
static_assert(hidden_attach_under_remote_does_not_open_on_release());
static_assert(owned_policy_keeps_existing_activation_behavior());
