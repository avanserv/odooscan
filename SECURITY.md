# Security policy

## Reporting a vulnerability

Please report vulnerabilities **privately**, through
[GitHub's private vulnerability reporting](https://github.com/avanserv/odooscan/security/advisories/new).
Do not open a public issue for one.

Say what you found, how to reproduce it, and what it lets someone do. You will get an
acknowledgement, and the fix and its disclosure are coordinated with you through the advisory.

## Supported versions

The project is pre-1.0: only the latest release receives fixes.

## What is in scope

odooscan reads the files it is given and writes only to its standard output and error. It makes
no network requests and never executes code from the modules it reads: a manifest is parsed as a
literal, never evaluated. A behaviour that breaks one of these is a vulnerability:

- A file being written, or code from a scanned module being run.
- A file outside the given paths and addons paths being read.
- A crafted module making a scan hang or exhaust memory.

A rule that misses a real flaw, or reports one that is not there, is a bug, not a vulnerability:
open an issue.
