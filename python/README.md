# ryucad for Python

The official Python client for [ryucad](https://github.com/Seiraiyu/ryucad),
CAD built for AI agents to drive. It runs an installed `ryucad` (it ships
none) as `ryucad session --stdio` and sends its requests by name.

    import ryucad

    with ryucad.connect("~/work/gearbox") as cad:
        answer = cad.batch([ryucad.op("part.ensure", part="shaft", features=[...])])
        print(answer["parts"])

Standard library only; Python 3.10 or newer. Apache-2.0.
