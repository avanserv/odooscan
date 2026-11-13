{
    "name": "Assets moved",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "qweb": ["static/src/xml/note.xml"],
    "assets": {
        "web.assets_backend": ["assets_moved/static/src/**/*"],
        "web.assets_qweb": ["assets_moved/static/src/xml/note.xml"],
    },
    "license": "LGPL-3",
}
