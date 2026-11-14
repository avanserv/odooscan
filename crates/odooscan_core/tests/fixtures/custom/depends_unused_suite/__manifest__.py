{
    "name": "Module that ships its manifest and a test suite",
    "version": "18.0.1.0.0",
    "depends": ["sale"],
    "assets": {
        "depends_unused_suite.mocha_tests": ["depends_unused_suite/static/src/spec/*.js"],
    },
    "license": "LGPL-3",
}
