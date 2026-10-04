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

// Simulate the gap after Rust's final permit check: the GUI task has not run.
constexpr bool suppression_after_dispatch_cancels_first_show(bool minimized, bool remote, bool completed) {
    miv::GuiVisibility state;
    const miv::GuiGateSnapshot issued {4, 2}; // no Remote acquisition yet
    auto execution = issued;
    if (minimized) ++execution.minimized_sequence;
    if (remote) execution.remote = completed ? 6 : 7; // acquisition 1, current phase
    const bool shown = state.accept_show(issued, execution, minimized && !completed);
    state.suppress(miv::GuiSuppression::Minimized, false);
    state.suppress(miv::GuiSuppression::RemoteSession, false);
    return !shown && !state.requested() && !state.should_show(true, true);
}
constexpr bool cancelled_raise_keeps_prior_visible_request() {
    miv::GuiVisibility state;
    state.request(true);
    return !state.accept_show({4, 2}, {5, 2}, false) && state.requested();
}
static_assert(suppression_after_dispatch_cancels_first_show(true, false, false));
static_assert(suppression_after_dispatch_cancels_first_show(true, false, true));
static_assert(suppression_after_dispatch_cancels_first_show(false, true, false));
static_assert(suppression_after_dispatch_cancels_first_show(false, true, true));
static_assert(cancelled_raise_keeps_prior_visible_request());

constexpr bool minimized_policy_and_remote(bool keep_visible, bool release_remote_first) {
    miv::GuiVisibility state;
    state.request(true);
    state.reconcile_main(true, true, keep_visible);
    if (state.should_show(true, false) != keep_visible) return false;
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    if (state.should_show(true, true) || !state.requested()) return false;
    if (release_remote_first) {
        state.suppress(miv::GuiSuppression::RemoteSession, false);
        if (state.should_show(true, false) != keep_visible) return false;
        state.reconcile_main(true, false, keep_visible);
    } else {
        state.reconcile_main(true, false, keep_visible);
        if (state.should_show(true, true)) return false;
        state.suppress(miv::GuiSuppression::RemoteSession, false);
    }
    return state.should_show(true, false);
}
constexpr bool policy_changes_while_minimized() {
    miv::GuiVisibility state;
    state.request(true);
    state.reconcile_main(true, true, false);
    if (state.should_show(true, true)) return false;
    state.reconcile_main(true, true, true);
    if (!state.should_show(true, false)) return false;
    state.suppress(miv::GuiSuppression::RemoteSession, true);
    state.reconcile_main(true, true, false);
    state.reconcile_main(true, true, true);
    if (state.should_show(true, true)) return false;
    state.suppress(miv::GuiSuppression::RemoteSession, false);
    if (!state.should_show(true, false)) return false;
    state.reconcile_main(true, true, false);
    if (state.should_show(true, true)) return false;
    state.reconcile_main(true, false, false);
    return state.should_show(true, false);
}
constexpr bool policy_never_opens_user_hidden_or_missing_main(bool user_closed) {
    miv::GuiVisibility state;
    state.request(user_closed);
    state.reconcile_main(true, true, false);
    state.request(false);
    state.reconcile_main(true, true, true);
    state.reconcile_main(true, false, true);
    if (state.should_show(true, true)) return false;
    state.request(true);
    state.reconcile_main(false, false, true);
    return !state.should_show(true, true);
}
static_assert(minimized_policy_and_remote(false, false));
static_assert(minimized_policy_and_remote(false, true));
static_assert(minimized_policy_and_remote(true, false));
static_assert(minimized_policy_and_remote(true, true));
static_assert(policy_changes_while_minimized());
static_assert(policy_never_opens_user_hidden_or_missing_main(false));
static_assert(policy_never_opens_user_hidden_or_missing_main(true));
