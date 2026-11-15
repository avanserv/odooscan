{
    "name": "Bundle cut",
    "version": "18.0.1.0.0",
    "depends": ["bundle_moves"],
    "assets": {
        "web.assets_backend": [("remove", "bundle_moves/static/src/cut.js")],
    },
    "license": "LGPL-3",
}
