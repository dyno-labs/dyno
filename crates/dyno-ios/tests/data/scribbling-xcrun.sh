#!/bin/sh
# xcrun for this crate's tests: one that fails with a reason, and leaves a program behind that
# goes on writing to the same error output for a few seconds, as fast as it can. It writes that
# program's process id to the file that its one argument names, for the test to end it.
echo "ERROR: devicectl failed for the test, and left a writer." >&2
(
	end=$(($(date +%s) + 5))
	while [ "$(date +%s)" -lt "$end" ]; do
		printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n' >&2
	done
) &
echo "$!" >"$1"
# Long enough that the writer has written when dyno reads.
sleep 1
exit 1
