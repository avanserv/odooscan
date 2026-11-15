{
    "name": "Test scripts 15",
    "version": "15.0.1.0.0",
    "depends": ["web"],
    "assets": {
        "web.assets_backend": ["test_scripts15/static/src/js/*.js"],
        "web.qunit_suite_tests": [
            "test_scripts15/static/src/tests/*.js",
            "test_scripts15/static/src/js/shared.js",
        ],
        "test_scripts15.mocha_tests": ["test_scripts15/static/src/editor/test/*.js"],
    },
    "license": "LGPL-3",
}
