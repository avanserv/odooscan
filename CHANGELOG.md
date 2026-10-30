# Changelog

## [0.8.0](https://github.com/avanserv/odooscan/compare/v0.7.0...v0.8.0) (2026-10-30)


### Features

* **checks:** Report a delegate=True field with no ondelete as OD228 ([#101](https://github.com/avanserv/odooscan/issues/101)) ([93c3860](https://github.com/avanserv/odooscan/commit/93c3860fdc1a1cbbdf8750fbcc8fe2768cea92e5))
* **checks:** Report field definitions Odoo refuses at registry setup ([#48](https://github.com/avanserv/odooscan/issues/48)) ([00bf2bc](https://github.com/avanserv/odooscan/commit/00bf2bca92f0941eceaf3ecea3af64254030c2ed))
* **checks:** Report field options Odoo ignores or logs ([#50](https://github.com/avanserv/odooscan/issues/50)) ([1209945](https://github.com/avanserv/odooscan/commit/1209945971d5a286182c7127fd2407fbca296e41))
* **checks:** Report model attributes Odoo logs or ignores ([#49](https://github.com/avanserv/odooscan/issues/49)) ([52657db](https://github.com/avanserv/odooscan/commit/52657db9f0d8b199aced0bd6cfb49ae7d9d3777d))
* **checks:** Report SQL columns that no stored field provides ([#53](https://github.com/avanserv/odooscan/issues/53)) ([e9918f4](https://github.com/avanserv/odooscan/commit/e9918f48037f3eaf61727dd4e3eeb4e1872567a2))
* **checks:** Report SQL views and constraints that miss field columns ([#54](https://github.com/avanserv/odooscan/issues/54)) ([b761e45](https://github.com/avanserv/odooscan/commit/b761e456f1919ecfbb98cae5bca13d50ef4c2959))
* **python:** Report ORM overrides and caches that fail at runtime ([#51](https://github.com/avanserv/odooscan/issues/51)) ([15a6c3d](https://github.com/avanserv/odooscan/commit/15a6c3d23ba809a5b5239f7775dc0e474988db26))
* **python:** Report rollbacks, raw SQL writes and import-time state ([#52](https://github.com/avanserv/odooscan/issues/52)) ([4cefa44](https://github.com/avanserv/odooscan/commit/4cefa440e489fc55113508bfb8363869d15b2c90))


### Bug Fixes

* **checks:** Call _table_query an attribute of BaseModel in OD116 ([#80](https://github.com/avanserv/odooscan/issues/80)) ([7d38486](https://github.com/avanserv/odooscan/commit/7d38486024a0b1a8be41a87e68b4da49daf6d30e))
* **checks:** Check a related field's model_field in OD203 ([#62](https://github.com/avanserv/odooscan/issues/62)) ([0260f74](https://github.com/avanserv/odooscan/commit/0260f74749eff94195a6486b649e13619a21a04f))
* **checks:** Close OD223 gaps on continue and guarded loops ([#81](https://github.com/avanserv/odooscan/issues/81)) ([8563fa0](https://github.com/avanserv/odooscan/commit/8563fa00721e8040063bc05d1a5529fe0fc429b8))
* **checks:** Count a filling endless while in the OD223 exit scan ([#94](https://github.com/avanserv/odooscan/issues/94)) ([48ae499](https://github.com/avanserv/odooscan/commit/48ae499c24981c932bdd650e747112510fb22f1b))
* **checks:** Date _condition_to_sql and _table_query in OD116 by release ([#63](https://github.com/avanserv/odooscan/issues/63)) ([3ae9bbf](https://github.com/avanserv/odooscan/commit/3ae9bbfaa000116be429c864cf78057f9790a9db))
* **checks:** Date the OD220 magic-field rules by series ([#64](https://github.com/avanserv/odooscan/issues/64)) ([9a1e616](https://github.com/avanserv/odooscan/commit/9a1e616a77257c50f7bf9e0d63cfa8cf541e2f91))
* **checks:** Drop the base clause of the _inherits order in OD224 ([#91](https://github.com/avanserv/odooscan/issues/91)) ([f9093a3](https://github.com/avanserv/odooscan/commit/f9093a3c0f1cee8f959e2670ce0ecac981597ab2))
* **checks:** Explain OD225 on a dependency field, share query lookup ([#76](https://github.com/avanserv/odooscan/issues/76)) ([f73c6eb](https://github.com/avanserv/odooscan/commit/f73c6ebeaa099f71c6aab45e64d91cfb780b8d41))
* **checks:** Fix OD224 on comments, psycopg2 SQL and log columns ([#73](https://github.com/avanserv/odooscan/issues/73)) ([c49be3b](https://github.com/avanserv/odooscan/commit/c49be3b6437dabcff0d91bd678c181f9cede96e3))
* **checks:** Fix OD226 manifest keys, view names and any-view ([#75](https://github.com/avanserv/odooscan/issues/75)) ([0f250d4](https://github.com/avanserv/odooscan/commit/0f250d46befaba83085c40dc004290b346d02428))
* **checks:** Let a selection list replace earlier values in OD216 ([#59](https://github.com/avanserv/odooscan/issues/59)) ([65a5745](https://github.com/avanserv/odooscan/commit/65a574549af55f48ae51c07c68db21377c38bacf))
* **checks:** Name a shorter relation as the fix in every OD215 message ([#60](https://github.com/avanserv/odooscan/issues/60)) ([45f74c5](https://github.com/avanserv/odooscan/commit/45f74c5ac0b70cbb4d01e2257b9e7f58a3ecdb76))
* **checks:** Order the parents of one _inherit in OD224 and OD227 ([#86](https://github.com/avanserv/odooscan/issues/86)) ([35d371e](https://github.com/avanserv/odooscan/commit/35d371e09fb44142b34e0fb15c9280d161cc630b))
* **checks:** Read _inherits as unknown past a non-literal _inherit ([#93](https://github.com/avanserv/odooscan/issues/93)) ([6fcb897](https://github.com/avanserv/odooscan/commit/6fcb89725a0056c0cbf4b8500da18b7528b1f909))
* **checks:** Read an endless while in OD223 without its continue paths ([#90](https://github.com/avanserv/odooscan/issues/90)) ([adfc2d9](https://github.com/avanserv/odooscan/commit/adfc2d940240c3a1ce37e2d55326c2bb7398e0b2))
* **checks:** Read inherited _inherits and the ondelete rewrite ([#61](https://github.com/avanserv/odooscan/issues/61)) ([e052209](https://github.com/avanserv/odooscan/commit/e0522092e774aefbcc5ae094d9fd27c784d7b8a7))
* **checks:** Read inherited _inherits entries in OD224 and OD227 ([#77](https://github.com/avanserv/odooscan/issues/77)) ([ca3af31](https://github.com/avanserv/odooscan/commit/ca3af31013e31f95fc7d0debe4fc5817438c65ba))
* **checks:** Read mixin magic fields in OD220 as Odoo merges them ([#79](https://github.com/avanserv/odooscan/issues/79)) ([dab5047](https://github.com/avanserv/odooscan/commit/dab504792bdddf6d689ba1c3e21d0841e3642f49))
* **checks:** Skip a model with a non-literal _inherit like a plain base ([#96](https://github.com/avanserv/odooscan/issues/96)) ([0abaa81](https://github.com/avanserv/odooscan/commit/0abaa81c178b65c2aa0983993589b43106958c1d))
* **checks:** Skip classes with _register = False in OD401 ([#74](https://github.com/avanserv/odooscan/issues/74)) ([c2dbb27](https://github.com/avanserv/odooscan/commit/c2dbb2701de626573dc9298d53550ef1d113877e))
* **checks:** Stop OD204 counting methods of _inherits parents ([#78](https://github.com/avanserv/odooscan/issues/78)) ([4fcd730](https://github.com/avanserv/odooscan/commit/4fcd730ad9596ff71780395dbd0e7d562a2866c7))
* **checks:** Stop OD211 counting methods of _inherits parents ([#89](https://github.com/avanserv/odooscan/issues/89)) ([71ad156](https://github.com/avanserv/odooscan/commit/71ad15613d92558aef8530ba4bd5c8806097fde9))
* **checks:** Stop OD223 and OD120 false positives ([#67](https://github.com/avanserv/odooscan/issues/67)) ([8348c83](https://github.com/avanserv/odooscan/commit/8348c83922d354dbd7db228e5c785ed1ad806078))
* **checks:** Stop OD604 counting methods of _inherits parents ([#88](https://github.com/avanserv/odooscan/issues/88)) ([0a03d3f](https://github.com/avanserv/odooscan/commit/0a03d3fc83c63893f53dd8c32283c6c81bfdb42c))
* **checks:** Tell model classes apart in OD123 advice by the index ([#71](https://github.com/avanserv/odooscan/issues/71)) ([1dfab82](https://github.com/avanserv/odooscan/commit/1dfab825459e4b7debf137b986b96e9b7c870922))
* **index:** Count always-installed modules as in depends ([#47](https://github.com/avanserv/odooscan/issues/47)) ([c4a696c](https://github.com/avanserv/odooscan/commit/c4a696cbb75502b41a6e16623381faeae0c34d7f))
* **index:** Count list-form auto_install modules as always installed ([#56](https://github.com/avanserv/odooscan/issues/56)) ([e41dc8d](https://github.com/avanserv/odooscan/commit/e41dc8d17222948120acd2e6fcc3bf8616fd1b40))
* **index:** Name the module that defines a model in OD202 ([#46](https://github.com/avanserv/odooscan/issues/46)) ([e6848df](https://github.com/avanserv/odooscan/commit/e6848dfb3b468fc2a60a5e09d681a06fd0b95d15))
* **manifest:** Load data files under legacy keys by series ([#85](https://github.com/avanserv/odooscan/issues/85)) ([b262633](https://github.com/avanserv/odooscan/commit/b262633bf9dfa54abd6df03e7cbc40db00badb1e))
* **method_body:** Report OD223 on unpaired writes and exits ([#66](https://github.com/avanserv/odooscan/issues/66)) ([bd98dd8](https://github.com/avanserv/odooscan/commit/bd98dd8efd9f16fd1c8275be08c7e06e672cc55d))
* **python:** Count models.CachedModel as a model base ([#72](https://github.com/avanserv/odooscan/issues/72)) ([a9e9720](https://github.com/avanserv/odooscan/commit/a9e97209518297af134d7b031049d264408a6681))
* **python:** Name a class without _name as Odoo does in its series ([#95](https://github.com/avanserv/odooscan/issues/95)) ([3b503b9](https://github.com/avanserv/odooscan/commit/3b503b950b08e158358d5dd1fff1a174cce4ce8e))
* **python:** Stop OD103 and OD121 on cursors the function opened ([#69](https://github.com/avanserv/odooscan/issues/69)) ([af09778](https://github.com/avanserv/odooscan/commit/af09778fc1f146e3233c223d1361047af1f00b43))
* **rules:** Report OD222 as an error from saas~16.4 ([#97](https://github.com/avanserv/odooscan/issues/97)) ([fcd973c](https://github.com/avanserv/odooscan/commit/fcd973c5bbe9dd686453fef50ccc51310c3e9706))
* **rules:** Report OD223 from saas~13.3, where Odoo starts raising ([#82](https://github.com/avanserv/odooscan/issues/82)) ([760fc39](https://github.com/avanserv/odooscan/commit/760fc3941bc200db9af17de02de8e2b73b9ee8e9))
* **sql:** Cut statements with the query lexer's literals ([#84](https://github.com/avanserv/odooscan/issues/84)) ([bc76ee2](https://github.com/avanserv/odooscan/commit/bc76ee233a532a10da583577508fd385e598904c))
* **sql:** End a cursor scan path at a block whose every branch leaves ([#83](https://github.com/avanserv/odooscan/issues/83)) ([8e3e810](https://github.com/avanserv/odooscan/commit/8e3e810e673eed16648d8ff3af767936f75a74cd))
* **sql:** Track own cursors by path and limit create_column to UPDATE ([#70](https://github.com/avanserv/odooscan/issues/70)) ([8bfddeb](https://github.com/avanserv/odooscan/commit/8bfddeb925d03903fa0c375aa5e02c75027df8dd))

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
