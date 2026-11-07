#!/usr/bin/env bash
# Downloads the coverage report of a workflow's push run at a commit, such as main's report at a
# pull request's base commit. With no such run or report it says so and succeeds: the report then
# shows no change, and only the floors apply.
#
#   .github/coverage/base.sh <workflow file> <artifact name> <commit> <dir>
#
# Needs gh, and a GH_TOKEN that may read the repository's actions.
set -euo pipefail
workflow="$1"
artifact="$2"
commit="$3"
dir="$4"
run="$(gh run list --workflow "$workflow" --commit "$commit" --event push --limit 1 \
	--json databaseId --jq '.[0].databaseId // empty')"
if [[ -z "$run" ]]; then
	echo "no $workflow run on a push of $commit"
	exit 0
fi
if gh run download "$run" --name "$artifact" --dir "$dir"; then
	echo "the $artifact report of run $run, at $commit"
else
	echo "run $run has no $artifact report"
fi
