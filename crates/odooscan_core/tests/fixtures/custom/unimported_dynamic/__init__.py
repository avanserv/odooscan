import importlib

for name in ("models.extra",):
    importlib.import_module(f"{__name__}.{name}")
