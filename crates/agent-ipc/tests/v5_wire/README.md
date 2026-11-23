# Frozen adapter IPC 5 fixture

Source: `ee9236a03c9775ba2dbfea0dfaa1c04c8f210f19`, before chunk B. Do not regenerate from current types.
Wire types, output projection, schemas and checker are copied from that revision;
only test modules were removed, nested imports adjusted and test helpers exposed.
`hello.frame` is the fixed length-prefixed UTF-8 JSON for fixture/1, request id 1.
The core compatibility tests use these independent types to prove that a v5
adapter drops new fields while keeping its shipped output and budget behavior.
