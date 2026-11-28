# Frozen adapter IPC 8 fixture

Source: `ab61ae0df`, before the sound-chain command. Do not regenerate from current types.
The actual IPC types, MCP output projection, schemas and settings/operations/favorite
tool JSON were copied from that commit; test modules were removed and module imports
and helper visibility adjusted. The help query cap is frozen as its value, 200.
The schema checker is reused from the v7 fixture.
`hello.frame` is fixed length-prefixed UTF-8 JSON for fixture/1, request id 1.
