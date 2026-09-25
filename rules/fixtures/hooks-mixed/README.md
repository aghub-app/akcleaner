# Mixed hook group

`before/home/.claude/settings.json` puts a Muxy marker command, a Superset guarded notify command, and a user command in the same Stop hook group. The expected scan reports the two product rules as candidates with unknown integrity; it does not find the unmarked user action. This is a read-only expectation, not an edited settings file.
