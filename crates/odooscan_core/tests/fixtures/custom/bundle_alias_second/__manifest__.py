# The name sorts after bundle_alias_first, so that module claims the alias first.
{
    "name": "Bundle alias second",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "assets": {
        "web.assets_backend": [
            "bundle_alias_second/static/src/shared.js",
            "bundle_alias_second/static/src/main.js",
        ],
    },
    "license": "LGPL-3",
}
