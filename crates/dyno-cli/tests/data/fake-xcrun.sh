#!/bin/sh
# xcrun for tests: just enough of `xcrun xctrace` for a device run. One iPhone is connected,
# unless FAKE_XCRUN_PHONES is 0. A recording saves on an interrupt. Its export has a WebContent
# process of 200 MiB at every second for a minute from the start, so a run finds it at idle.
# FAKE_XCRUN_FAIL names a step that fails instead: list, record or export.
[ "$1" = xctrace ] || exit 64
if [ "${FAKE_XCRUN_FAIL:-}" = "$2" ]; then
	echo "xctrace $2 failed for the test" >&2
	exit 1
fi
case "$2" in
list)
	echo "== Devices =="
	echo "Dev’s Mac mini (00000000-0000-0000-0000-00000000000A)"
	if [ "${FAKE_XCRUN_PHONES:-1}" != 0 ]; then
		echo "Test iPhone (26.6.2) (00008120-000000000000001E)"
	fi
	;;
record)
	while [ "$#" -gt 0 ]; do
		[ "$1" = --output ] && trace="$2"
		shift
	done
	mkdir "$trace" || exit 1
	date -u +%Y-%m-%dT%H:%M:%SZ >"$trace/start"
	trap 'exit 0' INT
	echo "Starting recording with the Activity Monitor template. Targeting All Processes."
	while :; do sleep 1; done
	;;
export)
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
