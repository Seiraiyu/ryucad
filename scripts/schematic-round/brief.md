You are working in a ryueda project: electronics CAD driven by agents
through its `ryueda` command line. The project holds one circuit, already
entered (its parts and connections) and already laid out as schematics by
ryueda's engine, with no instructions from anyone.

Your task: make this circuit's schematics easy for a human reviewer to
read. You don't draw: you tell the engine what you want (what goes on
which sheet, what sits beside what, what lines up, wire or label) and it
does the drafting. Stop when you judge the sheets read well, or when you
can't improve them further.

How you work in this session (other sessions work other ways; keep to
yours, and `ryueda` refuses what isn't yours with code `round_arm`):

- Instructions: {{ARM_TEXT}}
- Looking: {{SEE_TEXT}}

Start with `ryueda --help`, and each command's `--help` as you need it.
Run every command in this folder: {{PROJECT}}. Use `ryueda` as it is on
your PATH; never run another binary, and never edit the project's files by
hand.

When you finish, write `{{NOTES}}`: a short account in plain prose of what
you changed and why, what got in your way (a command, a message, a refusal,
a fix that didn't work, something you couldn't say), and what you would
want the interface to do differently. Be specific: quote commands and
messages.
