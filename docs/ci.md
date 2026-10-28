# In CI

The scan needs the Odoo sources your modules build on. A shallow sparse clone into a hidden
directory is enough, and module discovery skips hidden directories. Leaving out translations,
static assets and tests makes it about 20 MB instead of 190 MB, with the same index. On GitHub, `--format github`
annotates the pull request, with no need for the Advanced Security that SARIF upload requires on a
private repository:

```yaml
- uses: actions/checkout@v7
- uses: actions/checkout@v7
  with:
    repository: odoo/odoo
    ref: "18.0"
    path: .odoo
    fetch-depth: 1
    filter: blob:none
    sparse-checkout-cone-mode: false
    sparse-checkout: |
      /addons/
      /odoo/addons/
      !**/i18n/
      !**/static/
      !**/tests/
    persist-credentials: false
- uses: astral-sh/setup-uv@v10
- run: uvx --from odooscan oscn check --addons-path .odoo --format github
```

On GitLab, `--format gitlab` writes a Code Quality report for the merge request:

```yaml
odooscan:
  image: python:3.13
  script:
    - git clone --depth 1 --branch 18.0 --filter=blob:none --no-checkout https://github.com/odoo/odoo .odoo
    - git -C .odoo sparse-checkout set --no-cone '/addons/' '/odoo/addons/' '!**/i18n/' '!**/static/' '!**/tests/'
    - git -C .odoo checkout 18.0
    - pip install odooscan
    - oscn check --addons-path .odoo --format gitlab > gl-code-quality-report.json
  artifacts:
    when: always
    reports:
      codequality: gl-code-quality-report.json
```

Pin `odooscan` to a version, and the actions to commit SHAs, in a real pipeline.
