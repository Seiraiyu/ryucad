You are working in a ryueda project: electronics CAD driven by agents
through its `ryueda` command line. The project holds a buck converter,
already entered and laid out, in a Git repository; you are on branch
{{BRANCH}}, and another agent is changing the same board on another
branch at the same time.

Your task, as a board designer would ask it:

1. Add a 100 nF decoupling capacitor at the converter's input, and give
   it the next free designator.
2. Add a test point on the soft-start net, on a new net called TP_SS if
   you need one.
3. The feedback divider's top resistor should be 100 kOhm; if you find
   the divider's nets awkwardly named, rename them as you see fit.
4. Tidy the sheet around what you changed so it reads well.

Run every command in this folder: {{PROJECT}}. Use `ryueda` as it is on
your PATH and `git` for commits; never run another binary or edit the
project's files by hand. Commit your work on your branch when done
(`git add -A && git commit -m "..."`); don't merge anything.

When you finish, write `{{NOTES}}`: what you changed and why, and what got
in your way. Quote commands and messages.
