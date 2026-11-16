"""The package's version: its installed distribution's, which the release
stamps from ryucad's tag; ``0.0.0`` when run from a checkout."""

from importlib import metadata

try:
    VERSION: str = metadata.version("ryucad")
except metadata.PackageNotFoundError:
    VERSION = "0.0.0"
