{
    "name": "Test imports",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "assets": {
        "web.assets_backend": ["test_imports/static/src/**/*"],
        "web.assets_unit_tests": [
            "test_imports/static/tests/**/*",
            ("remove", "test_imports/static/tests/stray.test.js"),
        ],
    },
    "license": "LGPL-3",
}
