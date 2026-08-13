# Local Crossterm Patch

This directory contains the source of `crossterm` 0.28.1 under its MIT
license. The workspace patches crates.io to this copy because the Unix event
reader loops forever when a terminal read returns zero bytes after a PTY
disconnect.

The local behavior change is in `src/event/source/unix/mio.rs`: a zero-byte
terminal read returns `UnexpectedEof`, allowing the TUI input thread to stop
and the normal shutdown path to run. The adjacent unit test covers that case.
