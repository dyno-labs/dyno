#!/bin/sh
# xcrun for tests: just enough of `xcrun devicectl` to open a page in a phone's Safari, and to
# list the fake phone (crates/dyno-ios/src/fake.rs) with its tunnel.
#
# A launch writes the URL it opens to the file that FAKE_XCRUN_LAUNCHED names, when it names one.
# With FAKE_XCRUN_FAIL=launch, the launch fails.
#
# A list writes the device list to the file that --json-output names. The fake phone has the
# tunnel address that FAKE_XCRUN_TUNNEL gives, and no tunnel when it gives none. With
# FAKE_XCRUN_FAIL=list, the list fails.

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

if [ "$1 $2 $3" = "devicectl list devices" ]; then
	if [ "${FAKE_XCRUN_FAIL:-}" = list ]; then
		echo "ERROR: devicectl list failed for the test" >&2
		exit 1
	fi
	if [ -n "${FAKE_XCRUN_TUNNEL:-}" ]; then
		connection="{\"tunnelState\":\"connected\",\"tunnelIPAddress\":\"$FAKE_XCRUN_TUNNEL\"}"
	else
		connection='{"tunnelState":"disconnected"}'
	fi
	hardware='{"udid":"00008120-000000000000001E"}'
	device="{\"connectionProperties\":$connection,\"hardwareProperties\":$hardware}"
	echo "{\"result\":{\"devices\":[$device]}}" >"$(option --json-output "$@")"
	exit 0
fi

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
