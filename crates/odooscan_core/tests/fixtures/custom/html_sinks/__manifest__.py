{
    "name": "HTML sinks",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "assets": {
        "web.assets_backend": [
            "html_sinks/static/src/note_field.js",
            "html_sinks/static/src/debug.js",
        ],
        "web.assets_unit_tests": ["html_sinks/static/tests/sinks.test.js"],
    },
    "license": "LGPL-3",
}
