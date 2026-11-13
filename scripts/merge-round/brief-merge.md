You are working in a ryueda project: electronics CAD driven by agents
through its `ryueda` command line, in the Git repository {{REPO}}. Two
other agents each changed the board on branches `a` and `b`. Your task:
merge both into `main` so that neither one's work is lost and the merged
board is as sound and as readable as each branch's.

Merge `a` first (`git merge a`), then `b` (`git merge b`). When Git stops,
`ryueda merge status` lists what is left to settle, each item with its
choices and repairs; `ryueda merge resolve <id> ...` settles one or more.
You may read (`ryueda diff`, `ryueda render`, `ryueda describe`,
`ryueda check`) but not edit the board otherwise: `ryueda` refuses other
commands here with code `round_role`. Commit the merge when ryueda lets
you (`git add -A && git commit`).

Run every command in this folder: {{PROJECT}}. When you finish, write
`{{NOTES}}`: each item the merge status showed you, what it said, what you
chose and why, whether the merged sheet reads as well as each branch's,
and what got in your way. Quote commands and messages.
