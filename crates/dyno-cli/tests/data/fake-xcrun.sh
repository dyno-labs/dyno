#!/bin/sh
# xcrun for tests: just enough of `xcrun devicectl` to open a page in a phone's Safari. A launch
# writes the URL it opens to the file that FAKE_XCRUN_LAUNCHED names, when it names one. With
# FAKE_XCRUN_FAIL=launch, the launch fails.

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

if [ "$1 $4" != "devicectl launch" ]; then
	exit 64
fi
if [ "${FAKE_XCRUN_FAIL:-}" = launch ]; then
	echo "ERROR: devicectl launch failed for the test" >&2
	exit 1
fi
if [ -n "${FAKE_XCRUN_LAUNCHED:-}" ]; then
	option --payload-url "$@" >"$FAKE_XCRUN_LAUNCHED"
fi
echo "Launched application with com.apple.mobilesafari bundle identifier."
