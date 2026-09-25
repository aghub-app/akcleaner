# JSON Pointer escaping

The fixture contains only a synthetic user hook under a key with both `/` and `~`. It must remain unclaimed by product rules, while the host artifact locator must escape those key characters per RFC 6901: `/` becomes `~1`, then `~` becomes `~0`.
