# 代码生成模板覆写点

生成器的模板与生成逻辑分离：把内置模板复制到这里改名（`<模板名>.j2`），
再在 `config/default.yaml` 里把 `gen.template_dir` 指向本目录，即可**只改行为、不改框架代码**。

内置模板名（对应 `cicada-codegen` 的 `templates/`）：

| 模板 | 产出 |
| --- | --- |
| `entity.rs.j2` | SeaORM 实体（含数据范围声明） |
| `dto.rs.j2` | 列表/行视图/入参 |
| `handler.rs.j2` | axum 路由 + 权限点 + 审计宏 |
| `module.rs.j2` / `mod.rs.j2` | 模块清单（owns / perms / routes） |
| `migration.rs.j2` | 建表迁移 |
| `index.vue.j2` / `index-tree.vue.j2` / `api.ts.j2` | 前端页面与 API |
| `menu.sql.j2` | 菜单与权限点 SQL |
| `README.md.j2` | 产出说明 |

覆写只影响**你这一侧**的产物；框架升级不会覆盖本目录，本目录也不会回灌上游。
