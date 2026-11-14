{
    "name": "Web keys",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "data": ["views/views.xml", "data/actions.xml"],
    "assets": {
        "web.assets_backend": [
            "web_keys/static/src/keys.js",
            "web_keys/static/src/services.js",
            "web_keys/static/src/probe.js",
            "web_keys/static/src/legacy.js",
        ],
        "web.assets_unit_tests": ["web_keys/static/tests/keys.test.js"],
        "web.assets_tests": ["web_keys/static/tests/tours/**/*"],
    },
    "license": "LGPL-3",
}
