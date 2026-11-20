import sys

from .imported_base import __all__

if sys.version_info >= (3, 12):
    __all__ = ["imported_b"]
