#!/bin/sh
# xcrun for this crate's tests: a wrapper that starts another program and waits for it, as a
# wrapper around devicectl does. Neither ends for far longer than a test waits. The wrapper
# writes the other program's process id beside the file that --json-output names, as FILE.pid,
# so a test can see whether it is still there once the wrapper has been stopped.

# The value of the option named $1 in the rest of the arguments.
option() {
	name=$1
	shift
	while [ "$#" -gt 1 ]; do
		if [ "$1" = "$name" ]; then
			echo "$2"
			return
		fi
		shift
	done
}

sleep 300 &
echo "$!" >"$(option --json-output "$@").pid"
wait
