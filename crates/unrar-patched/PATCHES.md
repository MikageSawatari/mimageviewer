# Local patches to unrar 0.5.8

This directory contains the source of
[`unrar` 0.5.8](https://github.com/muja/unrar.rs), licensed under MIT or
Apache-2.0. The original license texts are kept alongside the source.

## `UCM_CHANGEVOLUMEW` variable-length callback string

Upstream constructs a 2048-element slice from the `P1` pointer before looking
for its nul terminator. UnRAR only guarantees that `P1` points to a
nul-terminated volume name. For `RAR_VOL_NOTIFY`, the pointer can refer to a
`std::wstring` allocation sized to the current name, so the fixed-length slice
can cross the allocation boundary and cause an access violation while reading a
multipart RAR.

The local patch uses `WideCString::from_ptr_str`, which follows the callback
contract and copies through the first nul terminator only. Remove this fork once
an upstream release contains an equivalent fix.
