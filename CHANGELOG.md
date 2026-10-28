# Changelog

## [0.7.0](https://github.com/avanserv/odooscan/compare/v0.6.0...v0.7.0) (2026-10-28)


### Features

* **checks:** Follow tainted data across model methods ([#43](https://github.com/avanserv/odooscan/issues/43)) ([5d311cb](https://github.com/avanserv/odooscan/commit/5d311cbb16dcab6bb175c17b983fb80d98f02c70))
* **taint:** Report request data that reaches SQL, eval, Markup or a sudo() write ([#42](https://github.com/avanserv/odooscan/issues/42)) ([b8925c0](https://github.com/avanserv/odooscan/commit/b8925c0cf98e5e72d1bf051d5ec410ba2197a37d))

## [0.6.0](https://github.com/avanserv/odooscan/compare/v0.5.0...v0.6.0) (2026-10-28)


### Features

* **checks:** Check the models and methods of ORM calls ([#37](https://github.com/avanserv/odooscan/issues/37)) ([1b1338f](https://github.com/avanserv/odooscan/commit/1b1338fe4e25f961b2c4c9fe13c710ab637df823))
* **checks:** Report a patch whose super call has no parent method ([#36](https://github.com/avanserv/odooscan/issues/36)) ([704ac3b](https://github.com/avanserv/odooscan/commit/704ac3b2338a95231ce0d5b258cd9ab9db4bd531))
* **js:** Index JS modules and report unresolved imports ([#35](https://github.com/avanserv/odooscan/issues/35)) ([317f897](https://github.com/avanserv/odooscan/commit/317f8979fac0d5f7702461b0e02f784213d3177b))
* **js:** Report an RPC call to a private method ([#41](https://github.com/avanserv/odooscan/issues/41)) ([59df3fe](https://github.com/avanserv/odooscan/commit/59df3fe140e26d67fe63ec01393cc94c1fc5af86))
* **owl:** Check Owl template names and extension locators ([#40](https://github.com/avanserv/odooscan/issues/40)) ([79d2ccf](https://github.com/avanserv/odooscan/commit/79d2ccf48bb6bf1d1ad17bbaa779b2329e5f2593))

## [0.5.0](https://github.com/avanserv/odooscan/compare/v0.4.0...v0.5.0) (2026-10-28)


### Features

* **cli:** Print a rule's documentation with oscn explain ([#29](https://github.com/avanserv/odooscan/issues/29)) ([20084af](https://github.com/avanserv/odooscan/commit/20084afa6aec5a11a84184f09e91827c4f2f6ec6))
* **output:** Link each finding to its rule page ([#31](https://github.com/avanserv/odooscan/issues/31)) ([9df56e8](https://github.com/avanserv/odooscan/commit/9df56e8c20942b36e7484fc4e5ad9cc85321dd55))

## [0.4.0](https://github.com/avanserv/odooscan/compare/v0.3.0...v0.4.0) (2026-10-28)


### Features

* **checks:** Check the field names in literal domains ([#23](https://github.com/avanserv/odooscan/issues/23)) ([e782838](https://github.com/avanserv/odooscan/commit/e782838543c24a246f07a77594a2254b1f3199be))
* **checks:** Report a super() call no parent class can answer ([#24](https://github.com/avanserv/odooscan/issues/24)) ([6535b23](https://github.com/avanserv/odooscan/commit/6535b234ef042672db66f8e04a20d86036717c96))
* **checks:** Report definitions Odoo refuses when it loads the registry ([#22](https://github.com/avanserv/odooscan/issues/22)) ([9cbc530](https://github.com/avanserv/odooscan/commit/9cbc530e138cf193a1723c36a256c91b842e6852))


### Bug Fixes

* **checks:** Silence the false positives Odoo core raises ([#21](https://github.com/avanserv/odooscan/issues/21)) ([67f648d](https://github.com/avanserv/odooscan/commit/67f648dcd11933aba12bc57ff46be525f4e697bb))
* **project:** Print the series notice only when no series applies ([#20](https://github.com/avanserv/odooscan/issues/20)) ([ae5dc7a](https://github.com/avanserv/odooscan/commit/ae5dc7a93cbc5b0a0aedc9d2149ddcae6f2fec2c))

## [0.3.0](https://github.com/avanserv/odooscan/compare/v0.2.0...v0.3.0) (2026-10-27)


### Features

* **checks:** Report inherited view locators that match nothing ([#17](https://github.com/avanserv/odooscan/issues/17)) ([6d6edca](https://github.com/avanserv/odooscan/commit/6d6edca622cd5f28299456dce684faa60aa6e9c2))
* **python:** Check the XML ids Python code references ([#16](https://github.com/avanserv/odooscan/issues/16)) ([bd5f760](https://github.com/avanserv/odooscan/commit/bd5f760815aac8c5bf484659c9a88918f7d64891))

## [0.2.0](https://github.com/avanserv/odooscan/compare/v0.1.0...v0.2.0) (2026-10-27)


### ⚠ BREAKING CHANGES

* Install the scanner as oscn ([#6](https://github.com/avanserv/odooscan/issues/6))

### Features

* **cli:** Hide known findings with a baseline file ([#8](https://github.com/avanserv/odooscan/issues/8)) ([d82d454](https://github.com/avanserv/odooscan/commit/d82d4542828aaec193fd1cf22b7ab5859ba3383c))
* **cli:** Read settings from odooscan.toml or pyproject.toml ([#7](https://github.com/avanserv/odooscan/issues/7)) ([f86b1ef](https://github.com/avanserv/odooscan/commit/f86b1efee05e3bfbb35220c739a99443b3252bfe))
* **cli:** Report as GitHub annotations and GitLab Code Quality ([#9](https://github.com/avanserv/odooscan/issues/9)) ([719ccdc](https://github.com/avanserv/odooscan/commit/719ccdc7103a63824c1046faf1d923099fd8d196))
* Install the scanner as oscn ([#6](https://github.com/avanserv/odooscan/issues/6)) ([c04564c](https://github.com/avanserv/odooscan/commit/c04564c4408ab5ccc92dd28cf8b6cbeaaaba4f98))

## 0.1.0 (2026-10-27)


### Features

* Check Odoo modules against an index of every module they build on ([104d89a](https://github.com/avanserv/odooscan/commit/104d89a2c5e184c5d20ead765941dcf43ebd1e6a))
