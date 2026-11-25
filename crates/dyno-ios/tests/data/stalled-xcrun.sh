#!/bin/sh
# xcrun for this crate's tests: one that never gets as far as devicectl. It does nothing for far
# longer than a test waits, and takes no signal's side: only a kill ends it.
exec sleep 300
