#!/usr/bin/env bash
# Posts a coverage report on a pull request as a comment, or updates the comment it posted
# before. The report's first line is its marker, such as <!-- coverage: engine -->, so each report
# keeps one comment however often the pull request changes.
#
#   .github/coverage/comment.sh <comment.md> <pull request number>
#
# Needs gh, GITHUB_REPOSITORY, and a GH_TOKEN that may write pull request comments.
set -euo pipefail
file="$1"
pr="$2"
marker="$(head -n 1 "$file")"
if [[ ! "$marker" =~ ^\<!--\ coverage:\ [a-z-]+\ --\>$ ]]; then
	echo "$file does not start with a coverage marker" >&2
	exit 64
fi
existing="$(gh api --paginate "repos/$GITHUB_REPOSITORY/issues/$pr/comments" \
	--jq ".[] | select(.user.login == \"github-actions[bot]\" and (.body | startswith(\"$marker\"))) | .id" |
	head -n 1)"
if [[ -n "$existing" ]]; then
	gh api --method PATCH "repos/$GITHUB_REPOSITORY/issues/comments/$existing" -F "body=@$file" >/dev/null
	echo "updated comment $existing"
else
	gh api --method POST "repos/$GITHUB_REPOSITORY/issues/$pr/comments" -F "body=@$file" >/dev/null
	echo "posted a comment"
fi
