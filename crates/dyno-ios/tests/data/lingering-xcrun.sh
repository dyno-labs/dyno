#!/bin/sh
# xcrun for this crate's tests: one that fails at once, and leaves a program behind that keeps
# its error output open for a few seconds more.
sleep 5 &
echo "ERROR: devicectl failed for the test, and left a program behind." >&2
exit 1
