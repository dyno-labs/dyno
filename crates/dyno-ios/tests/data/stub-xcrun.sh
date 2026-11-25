#!/bin/sh
# xcrun for this crate's tests: just enough of `xcrun devicectl` to list one phone and to launch
# an app on it.
#
# `devicectl list devices ... --json-output FILE` writes a device list to FILE: the fake phone
# (src/fake.rs), with fd7b:e5b:6f53::1 as its address in this machine's tunnel to it.
#
# `devicectl device process launch ... --payload-url URL ...` makes an empty file at the path that
# URL gives. A test gives a path as the URL, and sees from the file whether a launch ran.

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
	connection='{"tunnelState":"connected","tunnelIPAddress":"fd7b:e5b:6f53::1"}'
	hardware='{"udid":"00008120-000000000000001E"}'
	device="{\"connectionProperties\":$connection,\"hardwareProperties\":$hardware}"
	echo "{\"result\":{\"devices\":[$device]}}" >"$(option --json-output "$@")"
	exit 0
fi
if [ "$1 $4" = "devicectl launch" ]; then
	: >"$(option --payload-url "$@")"
	exit 0
fi
exit 64
