{
    "name": "Unlisted data",
    "version": "18.0.1.0.0",
    "depends": ["base"],
    "data": [
        "security/ir.model.access.csv",
        "data/badges.xml",
        "data/signs.xml",
    ],
    "update_xml": ["security/ir.model.access-legacy.csv"],
    "post_init_hook": "x_load_extras",
    "license": "LGPL-3",
}
