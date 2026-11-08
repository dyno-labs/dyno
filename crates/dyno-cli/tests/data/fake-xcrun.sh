#!/bin/sh
# xcrun for tests: just enough of `xcrun xctrace` and `xcrun devicectl` for a device run. One
# iPhone is connected, unless FAKE_XCRUN_PHONES is 0. It is unlocked, unless FAKE_XCRUN_LOCKED is
# 1, and the Mac itself has no lock state. A launch writes the URL it opens to the file that
# FAKE_XCRUN_LAUNCHED names, when it names one. A recording saves on an interrupt. Its export has
# a WebContent process of 200 MiB at every second for a minute from the start, so a run finds it
# at idle.
# FAKE_XCRUN_FAIL names a step that fails instead: list, record, export, lockState or launch. With
# disconnect, a recording loses the phone once it starts, and an interrupt then does not stop it.
# With disconnect-save, it loses the phone, then saves and exits by itself.
phone=00008120-000000000000001E

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

step=$2
[ "$1" = devicectl ] && step=$4
if [ "${FAKE_XCRUN_FAIL:-}" = "$step" ]; then
	echo "$1 $step failed for the test" >&2
	exit 1
fi
case "$1 $step" in
"devicectl lockState")
	if [ "$(option --device "$@")" != "$phone" ]; then
		echo "ERROR: The device is not available." >&2
		exit 1
	fi
	locked=false
	[ "${FAKE_XCRUN_LOCKED:-0}" = 1 ] && locked=true
	echo "• passcodeRequired: $locked"
	echo "• unlockedSinceBoot: true"
	;;
"devicectl launch")
	if [ -n "${FAKE_XCRUN_LAUNCHED:-}" ]; then
		option --payload-url "$@" >"$FAKE_XCRUN_LAUNCHED"
	fi
	echo "Launched application with com.apple.mobilesafari bundle identifier."
	;;
"xctrace list")
	echo "== Devices =="
	echo "Dev’s Mac mini (00000000-0000-0000-0000-00000000000A)"
	if [ "${FAKE_XCRUN_PHONES:-1}" != 0 ]; then
		echo "Test iPhone (26.6.2) ($phone)"
	fi
	;;
"xctrace record")
	trace=$(option --output "$@")
	mkdir "$trace" || exit 1
	date -u +%Y-%m-%dT%H:%M:%SZ >"$trace/start"
	trap 'exit 0' INT
	echo "Starting recording with the Activity Monitor template. Targeting All Processes."
	if [ "${FAKE_XCRUN_FAIL:-}" = disconnect ]; then
		trap 'echo "Stopping recording..."' INT
		echo "Device got disconnected, ending recording..."
	fi
	if [ "${FAKE_XCRUN_FAIL:-}" = disconnect-save ]; then
		echo "Device got disconnected, ending recording..."
		echo "Recording completed. Saving output file..."
		exit 0
	fi
	while :; do sleep 1; done
	;;
"xctrace export")
	trace="$4"
	if [ "$5" = --toc ]; then
		printf '<trace-toc><run number="1"><info><target>'
		printf '<device platform="iOS" model="iPhone 14 Pro Max" memory-size="6442450944" name="Test iPhone" os-version="26.6.2"/>'
		printf '</target><summary><start-date>%s</start-date></summary></info></run></trace-toc>\n' "$(cat "$trace/start")"
		exit 0
	fi
	printf '<trace-query-result><node><schema name="activity-monitor-process-live">'
	for column in start process pid memory-physical-footprint; do
		printf '<col><mnemonic>%s</mnemonic></col>' "$column"
	done
	printf '</schema>\n'
	second=0
	while [ "$second" -le 60 ]; do
		printf '<row><start-time fmt="%s s">%s000000000</start-time><process fmt="com.apple.WebKit.WebContent (412)"/><pid fmt="412">412</pid><size-in-bytes fmt="200 MiB">209715200</size-in-bytes></row>\n' "$second" "$second"
		second=$((second + 1))
	done
	printf '</node></trace-query-result>\n'
	;;
*) exit 64 ;;
esac
