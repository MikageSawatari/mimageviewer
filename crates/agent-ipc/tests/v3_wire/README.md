# Frozen adapter IPC 3 fixture

Source: commit `8d6201dce` (shipped v5.2.x). Do not regenerate from current types.
`mod.rs`, `read_types.rs`, `folder_types.rs` retain the shipped wire definitions,
limits, framing and thumbnail decoder. Only test modules were removed and
crate-relative imports adjusted for nesting. `output.rs` freezes the shipped
`tool_result` and image validator; `schema.rs` and `schema_check.rs` freeze its
schemas and test schema checker. Formatting may follow repository rustfmt.

`hello.frame` is the exact 4-byte LE + UTF-8 JSON written by the old PipeBackend
for request id 1, fixture/1, request kind tool. The test also serializes the
frozen Request and compares the bytes. Core tests consume this same fixture.
