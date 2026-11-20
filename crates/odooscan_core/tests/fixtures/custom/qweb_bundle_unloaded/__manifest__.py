{
    "name": "QWeb bundle unloaded",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "assets": {
        "web.assets_backend": ["qweb_bundle_unloaded/static/src/main/*.xml"],
        "web.assets_qweb": [
            "qweb_bundle_unloaded/static/src/old.xml",
            "qweb_bundle_unloaded/static/src/ext.xml",
            "qweb_bundle_unloaded/static/src/main/shared.xml",
        ],
    },
    "license": "LGPL-3",
}
