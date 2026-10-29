# KiCad-format files for ryueda's import tests

Every file here was written for ryueda's tests, in the formats KiCad 9
writes; none is copied from KiCad's libraries, which are never committed
(the release-only corpus test fetches them: `scripts/fetch-kicad.sh`).

- `symbols/Ryu_Test.kicad_sym`: a resistor with maker and supplier part
  numbers, a value variant derived from it, a power symbol, a two-gate
  part with a supply unit and a De Morgan style, and a part with pin
  alternates and a no-connect pin.
- `footprints/Ryu_Test.pretty`: a resistor footprint, and one with every
  pad kind and shape, a keepout, a graphic on a layer ryueda has no
  equivalent of, and models found, found beside their `.wrl`, and not
  found.
- `3dmodels/Ryu_Test.3dshapes`: the smallest STEP files that parse.
