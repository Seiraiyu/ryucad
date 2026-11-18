You are working in a ryueda project: electronics CAD driven by agents
through its `ryueda` command line. The project holds one circuit, a
sensor breakout (a BME280 sensor and its decoupling capacitor, an LDO
regulator and its two capacitors, two I2C pull-up resistors and a 4-pin
header), already entered as the design `board`, with its parts, their
connections and their footprints.

Your task: lay out its printed circuit board so a fabricator could make
it. Make a two-layer board for the design, place every part and route
every net with tracks (and vias where you need them), until
`ryueda board drc` finds nothing. Then export the fabrication package
into the folder `fab` in the project. Keep the board small and the routing
tidy, as a person laying it out would. The header is how the board is
wired up, so it belongs at an edge. Do not change the circuit (its
parts, nets or footprints): only the board.

Start with `ryueda --help`, and each command's `--help` as you need it.
Run every command in this folder: {{PROJECT}}. Use `ryueda` as it is on
your PATH. Never run another binary, never edit the project's files by
hand, and never read ryueda's source code. You may render the board to
PNG (`ryueda board render`) and look at the pictures.

When you finish, write `{{NOTES}}`: a short account in plain prose of
how you laid the board out and why, what got in your way (a command, a
message, a refusal, a finding you couldn't understand or clear, something
you had to guess or couldn't say), and what you would want the interface
to do differently. Be specific: quote commands and messages.
