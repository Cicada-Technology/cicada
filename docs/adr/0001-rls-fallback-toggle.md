# ADR 0001：RLS 兜底采用「配置按表开启 + 启动期白名单校验」

- 日期：2026-10-10
- 状态：已采纳

## 背景

设计文档 §6.3 要求数据范围过滤有数据库层兜底：应用层算错归属条件时数据仍不外泄，
但该层「默认关闭，按表开启」。需要决定开关放哪里、谁校验表名、缺席租户上下文时怎么办。

## 选项

1. 迁移里直接给全部敏感表建策略：一次到位，但会把「未走 `ScopedDb` 的读路径」一起打死
   （登录、授权装载等），且无法按需回退。
2. 运行期 DDL（配置 + CLI）按表开启：可控、可回退，但需要白名单与文档约束。
3. 不启用，仅靠应用层：省事，但失去兜底这一设计目标。

## 结论

采用选项 2，落在 `cicada-infra::rls`：

- 表名必须命中 `TENANT_TABLES` 白名单（identifier 只来自代码常量，不拼接用户输入）；越界即报错。
- 策略 fail-closed：`current_setting('app.tenant_id', true)` 未设置时 `tenant_id = NULL` 为 NULL，
  行被过滤 —— **没有租户上下文就看不到任何行**。
- 用 `FORCE ROW LEVEL SECURITY` 让表属主也受策略约束；需要绕过时用单独的 `BYPASSRLS` 角色，
  而不是关掉 `FORCE`。
- 开关出口：`config.rls.tables`（启动期 `enable`，失败 fail fast）与 `cicada-cli rls <enable|disable|status>`。

## 代价

开启某表的 RLS 后，**所有**读该表的路径必须经 `Db::begin_scoped` 注入 `app.tenant_id`，
否则查询返回空集。因此默认关闭；开启前需确认该表的读路径都已受控（验证见
`crates/infra/tests/rls.rs`：无上下文 0 行、租户 1/2 各见 1 行、跨租户 UPDATE 0 行）。
