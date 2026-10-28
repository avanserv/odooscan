# Suppressing a finding

Write `odooscan: ignore[OD103]` in a comment, or `noqa: OD103`. A trailing comment covers its own
line, and a comment alone on a line covers the next one. `odooscan: ignore` without codes covers
every rule. XML takes the same text inside `<!-- -->`, and JS after `//`.
