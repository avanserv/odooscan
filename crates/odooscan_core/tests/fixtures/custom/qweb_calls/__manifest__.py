{
    "name": "QWeb calls",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "data": ["views/pages.xml"],
    "demo": ["demo/pages.xml"],
    "other_files": ["data/other_pages.xml"],
    "post_init_hook": "x_load_code_pages",
    "assets": {"web.assets_backend": ["qweb_calls/static/src/**/*"]},
}
