#!/bin/sh
# xcrun for this crate's tests: one whose devicectl fails, with its reason on two lines.
echo "ERROR: devicectl failed for the test." >&2
echo "  The phone was not found." >&2
exit 1
